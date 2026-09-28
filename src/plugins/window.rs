//! A plain Win32 top-level window that hosts a plugin's own editor.
//!
//! It is created on the GUI thread, so eframe's message loop also pumps its messages.

use clack_extensions::gui::{GuiApiType, GuiConfiguration, PluginGui, Window as ClapWindow};
use clack_host::prelude::PluginMainThreadHandle;
use std::ffi::c_void;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Windows whose close button was pressed; the owner tears the editor down on its next idle.
static CLOSE_REQUESTS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

const STYLE: WINDOW_STYLE = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_CLOSE {
        if let Ok(mut v) = CLOSE_REQUESTS.lock() {
            v.push(hwnd as usize);
        }
        return 0;
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn register_class() -> Vec<u16> {
    static REGISTERED: std::sync::Once = std::sync::Once::new();
    let name = wide("ShodanVoicePluginWindow");
    REGISTERED.call_once(|| unsafe {
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
        };
        RegisterClassW(&wc);
    });
    name
}

fn outer_size(width: u32, height: u32) -> (i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 };
    unsafe { AdjustWindowRectEx(&mut r, STYLE, 0, 0) };
    (r.right - r.left, r.bottom - r.top)
}

pub struct PluginWindow {
    /// Null for plugins that manage their own floating window.
    hwnd: HWND,
}

impl PluginWindow {
    pub fn open(gui: PluginGui, plugin: &PluginMainThreadHandle, title: &str) -> Result<Self, String> {
        let api_type = GuiApiType::default_for_current_platform().ok_or("no GUI API for this platform")?;
        let embedded = GuiConfiguration { api_type, is_floating: false };
        if !gui.is_api_supported(plugin, embedded) {
            let floating = GuiConfiguration { api_type, is_floating: true };
            if !gui.is_api_supported(plugin, floating) {
                return Err("plugin editor is not supported on this platform".into());
            }
            gui.create(plugin, floating).map_err(|e| format!("editor failed: {e:?}"))?;
            if let Ok(t) = std::ffi::CString::new(title) {
                gui.suggest_title(plugin, &t);
            }
            gui.show(plugin).map_err(|e| format!("editor failed: {e:?}"))?;
            return Ok(Self { hwnd: std::ptr::null_mut() });
        }

        gui.create(plugin, embedded).map_err(|e| format!("editor failed: {e:?}"))?;
        let size = gui.get_size(plugin);
        let (w, h) = size.map(|s| outer_size(s.width, s.height)).unwrap_or((640, 480));
        let class = register_class();
        let title_w = wide(&format!("{title} - SHODAN Voice"));
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title_w.as_ptr(),
                STYLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                w,
                h,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null::<c_void>(),
            )
        };
        if hwnd.is_null() {
            gui.destroy(plugin);
            return Err("could not create editor window".into());
        }
        // SAFETY: the window outlives the embedded editor: `close_editor` destroys the editor first.
        let parent = unsafe { ClapWindow::from_win32_hwnd(hwnd) };
        if let Err(e) = unsafe { gui.set_parent(plugin, parent) } {
            gui.destroy(plugin);
            unsafe { DestroyWindow(hwnd) };
            return Err(format!("editor failed to attach: {e:?}"));
        }
        let _ = gui.show(plugin);
        unsafe { ShowWindow(hwnd, SW_SHOW) };
        Ok(Self { hwnd })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if self.hwnd.is_null() {
            return;
        }
        let (w, h) = outer_size(width, height);
        unsafe { SetWindowPos(self.hwnd, std::ptr::null_mut(), 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    pub fn close_requested(&self) -> bool {
        if self.hwnd.is_null() {
            return false;
        }
        let Ok(mut v) = CLOSE_REQUESTS.lock() else { return false };
        let before = v.len();
        v.retain(|&h| h != self.hwnd as usize);
        v.len() != before
    }
}

impl Drop for PluginWindow {
    fn drop(&mut self) {
        if !self.hwnd.is_null() {
            unsafe { DestroyWindow(self.hwnd) };
        }
    }
}
