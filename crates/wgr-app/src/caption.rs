//! Controls anchored over the original Windows caption. DWM keeps the title,
//! native system buttons, resize border and window movement.
use anyhow::{Result, ensure};
use std::cell::{Cell, RefCell};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        UI::{
            Controls::SetWindowTheme,
            HiDpi::GetDpiForWindow,
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};
#[derive(Clone, Copy)]
pub enum CaptionAction {
    Back,
    Mute,
    Fullscreen,
}
struct State {
    window: HWND,
    popup: Cell<HWND>,
    alive: Cell<bool>,
    muted: Cell<bool>,
    pending: RefCell<Vec<CaptionAction>>,
    title: RefCell<String>,
    placement: Cell<Option<[i32; 5]>>,
}
pub struct Caption {
    state: Box<State>,
}
const ID: usize = 0x574752;
impl Caption {
    pub fn new(window: HWND) -> Result<Self> {
        unsafe {
            let state = Box::new(State {
                window,
                popup: Cell::new(HWND::default()),
                alive: Cell::new(true),
                muted: Cell::new(false),
                pending: RefCell::new(Vec::new()),
                title: RefCell::new(String::new()),
                placement: Cell::new(None),
            });
            let caption = Self { state };
            let state = caption.state.as_ref();
            let pointer = (state as *const State) as usize;
            let popup = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("WGR Caption Controls"),
                WS_POPUP | WS_CLIPCHILDREN,
                0,
                0,
                1,
                1,
                Some(window),
                None,
                None,
                None,
            )?;
            state.popup.set(popup);
            ensure!(
                SetWindowSubclass(popup, Some(popup_proc), ID, pointer).as_bool(),
                "Não foi possível criar os controles da barra"
            );
            for (index, label) in ["← Biblioteca", "Silenciar", "F11"].iter().enumerate() {
                let text: Vec<u16> = label.encode_utf16().chain(Some(0)).collect();
                let button = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("BUTTON"),
                    PCWSTR(text.as_ptr()),
                    WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_PUSHBUTTON as u32),
                    0,
                    0,
                    1,
                    1,
                    Some(popup),
                    Some(HMENU((1001 + index) as *mut _)),
                    None,
                    None,
                )?;
                let _ = SetWindowTheme(button, w!("Explorer"), PCWSTR::null());
                SendMessageW(
                    button,
                    WM_SETFONT,
                    Some(WPARAM(GetStockObject(DEFAULT_GUI_FONT).0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            ensure!(
                SetWindowSubclass(window, Some(window_proc), ID, pointer).as_bool(),
                "Não foi possível acompanhar a barra da janela"
            );
            position(state);
            Ok(caption)
        }
    }
    pub fn update(&self, title: &str, muted: bool) {
        unsafe {
            if *self.state.title.borrow() != title {
                *self.state.title.borrow_mut() = title.into();
                let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
                let _ = SetWindowTextW(self.state.window, PCWSTR(title.as_ptr()));
            }
            if self.state.muted.replace(muted) != muted
                && let Ok(button) = GetDlgItem(Some(self.state.popup.get()), 1002)
            {
                let text: Vec<u16> = (if muted { "Ativar áudio" } else { "Silenciar" })
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                let _ = SetWindowTextW(button, PCWSTR(text.as_ptr()));
            }
        }
    }
    pub fn actions(&self) -> Vec<CaptionAction> {
        self.state.pending.borrow_mut().drain(..).collect()
    }
}
impl Drop for Caption {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(Some(self.state.popup.get())).as_bool() {
                let _ = DestroyWindow(self.state.popup.get());
            }
            if self.state.alive.get() {
                let _ = RemoveWindowSubclass(self.state.window, Some(window_proc), ID);
                let _ = SetWindowTextW(self.state.window, w!("WGR · Web Game Runtime"));
            }
        }
    }
}
unsafe fn position(state: &State) {
    unsafe {
        let popup = state.popup.get();
        if !IsWindowVisible(state.window).as_bool()
            || IsIconic(state.window).as_bool()
            || (GetWindowLongPtrW(state.window, GWL_STYLE) as u32 & WS_CAPTION.0) == 0
        {
            let _ = ShowWindow(popup, SW_HIDE);
            return;
        }
        let mut bounds = RECT::default();
        if GetWindowRect(state.window, &mut bounds).is_err() {
            return;
        }
        let scale = GetDpiForWindow(state.window).max(96) as f32 / 96.;
        let mut info = TITLEBARINFOEX {
            cbSize: std::mem::size_of::<TITLEBARINFOEX>() as u32,
            ..Default::default()
        };
        SendMessageW(
            state.window,
            WM_GETTITLEBARINFOEX,
            Some(WPARAM(0)),
            Some(LPARAM((&mut info as *mut TITLEBARINFOEX) as isize)),
        );
        let right = [info.rgrect[2], info.rgrect[3], info.rgrect[5]]
            .iter()
            .filter(|rect| rect.right > rect.left)
            .map(|rect| rect.left)
            .min()
            .unwrap_or(bounds.right - (142. * scale) as i32)
            - (5. * scale) as i32;
        let top = if info.rcTitleBar.bottom > info.rcTitleBar.top {
            info.rcTitleBar.top + 2
        } else {
            bounds.top + (8. * scale) as i32
        };
        let height = if info.rcTitleBar.bottom > info.rcTitleBar.top {
            info.rcTitleBar.bottom - info.rcTitleBar.top - 4
        } else {
            (23. * scale) as i32
        };
        if height <= 0 {
            return;
        }
        let widths = [
            (90. * scale) as i32,
            (80. * scale) as i32,
            (38. * scale) as i32,
        ];
        let gap = (4. * scale) as i32;
        let width = widths.iter().sum::<i32>() + gap * 2;
        let placement = [right - width, top, width, height, (scale * 96.) as i32];
        if state.placement.get() == Some(placement) && IsWindowVisible(popup).as_bool() {
            return;
        }
        let _ = SetWindowPos(
            popup,
            None,
            right - width,
            top,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
        let mut x = 0;
        let region = CreateRectRgn(0, 0, 0, 0);
        for (index, width) in widths.into_iter().enumerate() {
            if let Ok(button) = GetDlgItem(Some(popup), 1001 + index as i32) {
                let _ = SetWindowPos(
                    button,
                    None,
                    x,
                    0,
                    width,
                    height,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                );
            }
            let part = CreateRectRgn(x, 0, x + width, height);
            CombineRgn(Some(region), Some(region), Some(part), RGN_OR);
            let _ = DeleteObject(part.into());
            x += width + gap;
        }
        if SetWindowRgn(popup, Some(region), true) == 0 {
            let _ = DeleteObject(region.into());
        } else {
            state.placement.set(Some(placement));
        }
        if !IsWindowVisible(popup).as_bool() {
            let _ = ShowWindow(popup, SW_SHOWNA);
        }
    }
}
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    unsafe {
        let state = &*(data as *const State);
        if message == WM_NCDESTROY {
            state.alive.set(false);
        }
        let result = DefSubclassProc(window, message, wparam, lparam);
        if matches!(
            message,
            WM_WINDOWPOSCHANGED | WM_SHOWWINDOW | WM_NCACTIVATE | WM_DPICHANGED
        ) {
            position(state);
        }
        result
    }
}
unsafe extern "system" fn popup_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    unsafe {
        let state = &*(data as *const State);
        match message {
            WM_ERASEBKGND => {
                let dc = HDC(wparam.0 as *mut _);
                let mut rect = RECT::default();
                let _ = GetClientRect(window, &mut rect);
                let brush = CreateSolidBrush(COLORREF(0x00222222));
                FillRect(dc, &rect, brush);
                let _ = DeleteObject(brush.into());
                return LRESULT(1);
            }
            WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
            WM_COMMAND => {
                match wparam.0 & 0xffff {
                    1001 => state.pending.borrow_mut().push(CaptionAction::Back),
                    1002 => state.pending.borrow_mut().push(CaptionAction::Mute),
                    1003 => state.pending.borrow_mut().push(CaptionAction::Fullscreen),
                    _ => {}
                }
                return LRESULT(0);
            }
            _ => {}
        }
        DefSubclassProc(window, message, wparam, lparam)
    }
}
