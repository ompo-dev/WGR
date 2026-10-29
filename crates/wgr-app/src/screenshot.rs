//! Captura exclusivamente da própria janela, usada pelo smoke opt-in.
use anyhow::{Result, ensure};
use std::path::Path;
use windows::Win32::{
    Foundation::HWND,
    Graphics::Gdi::*,
    Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow},
    UI::WindowsAndMessaging::GetClientRect,
};
struct GdiFrame {
    window: HWND,
    screen: HDC,
    memory: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}
impl Drop for GdiFrame {
    fn drop(&mut self) {
        unsafe {
            let _ = SelectObject(self.memory, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.memory);
            let _ = ReleaseDC(Some(self.window), self.screen);
        }
    }
}
pub fn capture_owned_handle(handle: isize, path: &Path) -> Result<()> {
    capture(handle, path, false)
}
pub fn capture_owned_frame(handle: isize, path: &Path) -> Result<()> {
    capture(handle, path, true)
}
fn capture(handle: isize, path: &Path, frame: bool) -> Result<()> {
    let hwnd = HWND(handle as *mut _);
    let mut process = 0;
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut process));
    }
    ensure!(
        process == std::process::id(),
        "Captura limitada à própria janela WGR"
    );
    unsafe {
        let mut rect = windows::Win32::Foundation::RECT::default();
        if frame {
            windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut rect)?;
        } else {
            GetClientRect(hwnd, &mut rect)?;
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        ensure!(width > 0 && height > 0, "Janela vazia");
        let screen = if frame {
            GetWindowDC(Some(hwnd))
        } else {
            GetDC(Some(hwnd))
        };
        let memory = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let previous = SelectObject(memory, bitmap.into());
        let _frame = GdiFrame {
            window: hwnd,
            screen,
            memory,
            bitmap,
            previous,
        };
        ensure!(!bits.is_null(), "Buffer de captura indisponível");
        let _ = PrintWindow(hwnd, memory, PRINT_WINDOW_FLAGS(if frame { 2 } else { 3 }));
        if frame && windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() == hwnd {
            let desktop = GetDC(None);
            let copied = BitBlt(
                memory,
                0,
                0,
                width,
                height,
                Some(desktop),
                rect.left,
                rect.top,
                ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0),
            );
            let _ = ReleaseDC(None, desktop);
            copied?;
        }
        let mut pixels =
            std::slice::from_raw_parts(bits.cast::<u8>(), width as usize * height as usize * 4)
                .to_vec();
        if !pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[..3] != pixels[..3])
        {
            ensure!(
                windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() == hwnd,
                "Captura GPU exige que a própria janela WGR esteja à frente"
            );
            BitBlt(
                memory,
                0,
                0,
                width,
                height,
                Some(screen),
                0,
                0,
                ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0),
            )?;
            pixels.copy_from_slice(std::slice::from_raw_parts(
                bits.cast::<u8>(),
                width as usize * height as usize * 4,
            ));
        }
        for p in pixels.as_chunks_mut::<4>().0 {
            p.swap(0, 2);
            p[3] = 255;
        }
        ensure!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|p| p[..3] != pixels[..3]),
            "A captura não contém um frame renderizado"
        );
        image::save_buffer(
            path,
            &pixels,
            width as u32,
            height as u32,
            image::ColorType::Rgba8,
        )?;
    }
    Ok(())
}
