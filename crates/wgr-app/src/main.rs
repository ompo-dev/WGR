#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod caption;
#[cfg(feature = "web-compat")]
mod cpu_profile;
#[cfg(feature = "web-compat")]
mod gta_browser;
mod input;
mod momor;
#[cfg(not(feature = "web-compat"))]
mod native_child;
#[cfg(not(feature = "web-compat"))]
mod native_player;
pub mod player;
mod screenshot;
mod service;
mod ui;
#[cfg(feature = "web-compat")]
mod webview;
use anyhow::Result;
use gpui::{prelude::*, *};
use std::{path::PathBuf, sync::Arc};
use wgr_core::domain::Paths;

fn main() {
    // The game owns a native child HWND; keep its swap chain above GPUI's HWND.
    // This selects GPUI's compositor and does not load a browser engine.
    unsafe {
        std::env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "1");
    }
    if let Err(error) = run() {
        let message = format!("Não foi possível iniciar WGR:\n{error:#}");
        eprintln!("{message}");
        if std::env::args().any(|arg| {
            matches!(
                arg.as_str(),
                "--configure-gta"
                    | "--audit-gta"
                    | "--smoke"
                    | "--clear-cache"
                    | "--clear-profile-cache"
            )
        }) {
            std::process::exit(1);
        }
        unsafe {
            let title: Vec<u16> = "WGR".encode_utf16().chain(Some(0)).collect();
            let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
            windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                None,
                windows::core::PCWSTR(text.as_ptr()),
                windows::core::PCWSTR(title.as_ptr()),
                windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
            );
        }
    }
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let argument = |key: &str| {
        args.iter()
            .position(|v| v == key)
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
    };
    let paths = if let Some(root) = argument("--data-dir") {
        Paths::at(root)?
    } else {
        service::default_paths()?
    };
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(paths.root.join("data/runtime.lock"))?;
    lock.try_lock().map_err(|_| {
        anyhow::anyhow!(
            "Outra instância do WGR está usando esta biblioteca. Abra a janela existente."
        )
    })?;
    let file = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("wgr")
        .filename_suffix("log")
        .max_log_files(7)
        .build(&paths.logs)?;
    let (nonblocking, _log_guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(nonblocking)
        .json()
        .init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()?;
    let services = Arc::new(service::Services::new(paths)?);
    services.seed()?;
    if args.iter().any(|arg| arg == "--clear-cache") {
        let removed = services.cache.clear()?;
        std::fs::write(
            services.paths.root.join("cache-cleanup.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"removed_bytes":removed,"saves_preserved":true}),
            )?,
        )?;
        return Ok(());
    }
    if let Some(reference) = argument("--configure-gta") {
        let origins = args
            .windows(2)
            .filter(|p| p[0] == "--archive-origin")
            .map(|p| p[1].clone())
            .collect();
        runtime.block_on(services.configure_gta(&reference, origins))?;
        return Ok(());
    }
    if args.iter().any(|a| a == "--audit-gta") {
        let audit = wgr_core::gta::audit(&services.paths.root.join("adapters/gta"))?;
        let path = services.paths.root.join("gta-audit.json");
        std::fs::write(path, serde_json::to_vec_pretty(&audit)?)?;
        return Ok(());
    }
    let clear_profile = args.iter().any(|arg| arg == "--clear-profile-cache");
    let smoke = argument("--smoke")
        .or_else(|| clear_profile.then(|| services.paths.root.join("profile-cache-cleanup.json")));
    let gta_smoke = clear_profile || args.iter().any(|a| a == "--gta-smoke");
    let handle = runtime.handle().clone();
    Application::new()
        .with_assets(momor::Assets)
        .run(move |cx| {
            input::init(cx);
            momor::init(cx);
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let bounds = Bounds::centered(None, size(px(1230.), px(820.)), cx);
            let result = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(900.), px(600.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("WGR · Web Game Runtime".into()),
                        appears_transparent: false,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    cx.new(|cx| ui::Shell::new(services, handle, smoke, gta_smoke, window, cx))
                },
            );
            if let Err(error) = result {
                tracing::error!(%error,"window_creation_failed");
                cx.quit();
            }
            cx.activate(true);
        });
    runtime.shutdown_timeout(std::time::Duration::from_secs(3));
    Ok(())
}
