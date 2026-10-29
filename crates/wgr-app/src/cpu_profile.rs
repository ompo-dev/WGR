//! Diagnostic-only sampling of our WebView's dedicated workers. No debugger,
//! heap snapshot, remote port, or data from other applications is involved.
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
};
use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};
use windows::core::{Interface, w};

#[derive(Default)]
struct State {
    stopped: Cell<bool>,
    sessions: RefCell<Vec<(String, PathBuf, u128)>>,
    saved: Cell<usize>,
    errors: Cell<usize>,
    targets: RefCell<std::collections::HashSet<String>>,
    pending_stops: Cell<usize>,
    stopping_batch: Cell<bool>,
    detach_sessions: RefCell<Vec<String>>,
}
pub struct CpuProfiles {
    view: ICoreWebView2,
    state: Rc<State>,
    receiver: ICoreWebView2DevToolsProtocolEventReceiver,
    token: i64,
}
impl CpuProfiles {
    pub fn start(view: ICoreWebView2, path: PathBuf) -> windows::core::Result<Self> {
        let state = Rc::new(State::default());
        let receiver =
            unsafe { view.GetDevToolsProtocolEventReceiver(w!("Target.attachedToTarget"))? };
        let mut token = 0;
        let child_state = state.clone();
        let child_path = path.clone();
        unsafe {
            receiver.add_DevToolsProtocolEventReceived(
                &DevToolsProtocolEventReceivedEventHandler::create(Box::new(
                    move |sender, args| {
                        if child_state.stopped.get() {
                            return Ok(());
                        }
                        if let (Some(view), Some(args)) = (sender, args) {
                            let mut raw = windows::core::PWSTR::null();
                            args.ParameterObjectAsJson(&mut raw)?;
                            let value = serde_json::from_str::<serde_json::Value>(
                                &CoTaskMemPWSTR::from(raw).to_string(),
                            )
                            .unwrap_or_default();
                            if !eligible(&value["targetInfo"]) {
                                return Ok(());
                            }
                            if let (Some(session), Some(id)) = (
                                value["sessionId"].as_str(),
                                value["targetInfo"]["targetId"].as_str(),
                            ) {
                                let index = {
                                    let mut targets = child_state.targets.borrow_mut();
                                    if targets.len() >= 8 || !targets.insert(id.into()) {
                                        return Ok(());
                                    }
                                    targets.len() - 1
                                };
                                auto_attach(&view, session);
                                profile_step(
                                    view,
                                    child_state.clone(),
                                    session.into(),
                                    child_path.with_extension(format!("worker-{index}.cpuprofile")),
                                    0,
                                );
                            }
                        }
                        Ok(())
                    },
                )),
                &mut token,
            )?;
        }
        let result = Self {
            view: view.clone(),
            state: state.clone(),
            receiver,
            token,
        };
        let target_view = view.clone();
        unsafe {
            view.CallDevToolsProtocolMethod(
                w!("Target.getTargets"),
                w!("{}"),
                &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                    move |result, raw| {
                        if result.is_err() {
                            state.errors.set(state.errors.get() + 1);
                            return Ok(());
                        }
                        let value =
                            serde_json::from_str::<serde_json::Value>(&raw).unwrap_or_default();
                        for target in value["targetInfos"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|t| eligible(t))
                            .take(8)
                        {
                            let index = {
                                let mut targets = state.targets.borrow_mut();
                                if targets.len() >= 8
                                    || !targets.insert(
                                        target["targetId"].as_str().unwrap_or_default().into(),
                                    )
                                {
                                    continue;
                                }
                                targets.len() - 1
                            };
                            let args =
                                serde_json::json!({"targetId":target["targetId"],"flatten":true})
                                    .to_string();
                            let worker_view = target_view.clone();
                            let worker_state = state.clone();
                            let output = path.with_extension(format!("worker-{index}.cpuprofile"));
                            target_view.CallDevToolsProtocolMethod(
                                w!("Target.attachToTarget"),
                                *CoTaskMemPWSTR::from(args.as_str()).as_ref().as_pcwstr(),
                                &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                                    move |result, raw| {
                                        if result.is_err() {
                                            worker_state.errors.set(worker_state.errors.get() + 1);
                                            return Ok(());
                                        }
                                        let value = serde_json::from_str::<serde_json::Value>(&raw)
                                            .unwrap_or_default();
                                        if let Some(session) = value["sessionId"].as_str() {
                                            auto_attach(&worker_view, session);
                                            profile_step(
                                                worker_view.clone(),
                                                worker_state.clone(),
                                                session.into(),
                                                output.clone(),
                                                0,
                                            );
                                        }
                                        Ok(())
                                    },
                                )),
                            )?;
                        }
                        Ok(())
                    },
                )),
            )?;
        }
        Ok(result)
    }
    pub fn stop(&self) {
        if self.state.stopped.replace(true) {
            return;
        }
        let _ = unsafe {
            self.receiver
                .remove_DevToolsProtocolEventReceived(self.token)
        };
        self.state.stopping_batch.set(true);
        for (session, path, epoch) in self.state.sessions.take() {
            stop_worker(self.view.clone(), self.state.clone(), session, path, epoch);
        }
        self.state.stopping_batch.set(false);
        finish_detach(&self.view, &self.state);
    }
    pub fn status(&self) -> serde_json::Value {
        serde_json::json!({"saved":self.state.saved.get(),"errors":self.state.errors.get(),"active":self.state.sessions.borrow().len(),"discovered":self.state.targets.borrow().len(),"pending_stops":self.state.pending_stops.get(),"sample_interval_us":5000,"max_workers":8,"max_file_bytes":4194304})
    }
}
impl Drop for CpuProfiles {
    fn drop(&mut self) {
        self.stop();
    }
}
fn command(
    view: &ICoreWebView2,
    session: &str,
    method: &str,
    args: &str,
    handler: ICoreWebView2CallDevToolsProtocolMethodCompletedHandler,
) -> windows::core::Result<()> {
    unsafe {
        view.cast::<ICoreWebView2_11>()?
            .CallDevToolsProtocolMethodForSession(
                *CoTaskMemPWSTR::from(session).as_ref().as_pcwstr(),
                *CoTaskMemPWSTR::from(method).as_ref().as_pcwstr(),
                *CoTaskMemPWSTR::from(args).as_ref().as_pcwstr(),
                &handler,
            )
    }
}
fn detach(view: &ICoreWebView2, session: &str) {
    let args = serde_json::json!({"sessionId":session}).to_string();
    let _ = unsafe {
        view.CallDevToolsProtocolMethod(
            w!("Target.detachFromTarget"),
            *CoTaskMemPWSTR::from(args.as_str()).as_ref().as_pcwstr(),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(()))),
        )
    };
}
fn eligible(target: &serde_json::Value) -> bool {
    target["type"] == "worker"
        && target["url"].as_str().is_some_and(|url| {
            [
                "https://fiveplay.lol/",
                "blob:https://fiveplay.lol/",
                "https://cdn.fiveplay.lol/",
                "blob:https://cdn.fiveplay.lol/",
            ]
            .iter()
            .any(|prefix| url.starts_with(prefix))
        })
}
fn auto_attach(view: &ICoreWebView2, session: &str) {
    let _ = command(
        view,
        session,
        "Target.setAutoAttach",
        "{\"autoAttach\":true,\"waitForDebuggerOnStart\":false,\"flatten\":true,\"filter\":[{\"type\":\"worker\"},{\"exclude\":true}]}",
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(()))),
    );
}
fn finish_detach(view: &ICoreWebView2, state: &State) {
    if state.pending_stops.get() == 0 && !state.stopping_batch.get() {
        for session in state.detach_sessions.take() {
            detach(view, &session);
        }
    }
}
fn profile_step(
    view: ICoreWebView2,
    state: Rc<State>,
    session: String,
    path: PathBuf,
    step: usize,
) {
    if state.stopped.get() {
        stop_worker(view, state, session, path, 0);
        return;
    }
    let steps = [
        ("Profiler.enable", "{}"),
        ("Profiler.setSamplingInterval", "{\"interval\":5000}"),
        ("Profiler.start", "{}"),
    ];
    if step == steps.len() {
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        state.sessions.borrow_mut().push((session, path, epoch));
        return;
    }
    let next_view = view.clone();
    let next_state = state.clone();
    let next_session = session.clone();
    let result = command(
        &view,
        &session,
        steps[step].0,
        steps[step].1,
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, _| {
            if result.is_ok() {
                profile_step(
                    next_view.clone(),
                    next_state.clone(),
                    next_session.clone(),
                    path.clone(),
                    step + 1,
                )
            } else {
                next_state.errors.set(next_state.errors.get() + 1);
                detach(&next_view, &next_session);
            }
            Ok(())
        })),
    );
    if result.is_err() {
        state.errors.set(state.errors.get() + 1);
        detach(&view, &session);
    }
}
fn stop_worker(view: ICoreWebView2, state: Rc<State>, session: String, path: PathBuf, epoch: u128) {
    state.pending_stops.set(state.pending_stops.get() + 1);
    state.detach_sessions.borrow_mut().push(session.clone());
    let done_view = view.clone();
    let done_state = state.clone();
    let result = command(
        &view,
        &session,
        "Profiler.stop",
        "{}",
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, raw| {
            let saved = result.is_ok()
                && raw.len() <= 4 * 1024 * 1024
                && (|| -> std::io::Result<()> {
                    use std::io::Write;
                    let wrapped = format!("{{\"approx_start_epoch_ms\":{epoch},\"result\":{raw}}}");
                    std::fs::File::create_new(&path)?.write_all(wrapped.as_bytes())
                })()
                .is_ok();
            if saved {
                done_state.saved.set(done_state.saved.get() + 1)
            } else {
                done_state.errors.set(done_state.errors.get() + 1)
            }
            done_state
                .pending_stops
                .set(done_state.pending_stops.get().saturating_sub(1));
            finish_detach(&done_view, &done_state);
            Ok(())
        })),
    );
    if result.is_err() {
        state.errors.set(state.errors.get() + 1);
        state
            .pending_stops
            .set(state.pending_stops.get().saturating_sub(1));
        finish_detach(&view, &state);
    }
}
