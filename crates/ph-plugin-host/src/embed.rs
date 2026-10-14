//! Embedding plugin editors in the app window (M2/T3; Windows only for now).
//!
//! A plugin editor is a native child window. The UI draws everything else with egui into one
//! OS window, so an embedded editor needs a native rectangle of its own inside that window:
//! an [`EditorFrame`], a plain child window created on the plugins' main thread (the thread
//! that also creates, pumps and destroys the plugin's own windows). The plugin's editor is
//! opened with the frame as its parent, and the UI moves the frame to wherever the floating
//! workspace window shows the editor, every frame, without waiting
//! (`SWP_ASYNCWINDOWPOS`).
//!
//! macOS (NSView) and Linux (X11) embedding is not implemented: [`EditorFrame::create`]
//! returns an error there and the UI opens editors as floating windows instead.
//!
//! Nothing here runs in tests: creating windows is the owner's desktop.

use crate::*;

/// A native child window that hosts one plugin editor (see the module docs).
pub struct EditorFrame {
    #[cfg_attr(not(windows), allow(dead_code))]
    hwnd: isize,
}

/// Whether editors can be embedded on this platform.
pub const EMBEDDING: bool = cfg!(windows);

impl EditorFrame {
    /// Creates a hidden frame inside `parent` (the app window).
    pub fn create(parent: RawWindowHandle) -> PluginResult<Self> {
        #[cfg(windows)]
        {
            let RawWindowHandle::Win32(h) = parent else {
                return Err(PluginError::Call(
                    "the app window is not a Win32 window".into(),
                ));
            };
            let parent = h.hwnd.get();
            let hwnd = crate::main_thread::main_thread().run(move || win::create(parent));
            if hwnd == 0 {
                return Err(PluginError::Call("cannot create the editor frame".into()));
            }
            Ok(Self { hwnd })
        }
        #[cfg(not(windows))]
        {
            let _ = parent;
            Err(PluginError::Call(
                "embedded plugin editors are Windows-only for now; opening it floating".into(),
            ))
        }
    }

    /// The handle to give the plugin as its parent.
    pub fn handle(&self) -> RawWindowHandle {
        #[cfg(windows)]
        {
            let h = raw_window_handle::Win32WindowHandle::new(
                std::num::NonZeroIsize::new(self.hwnd).expect("created"),
            );
            RawWindowHandle::Win32(h)
        }
        #[cfg(not(windows))]
        unreachable!("frames are only created on Windows")
    }

    /// Moves the frame to `rect` (x, y, width, height in physical pixels of the parent's
    /// client area) and shows or hides it. Returns at once.
    pub fn place(&self, rect: [i32; 4], visible: bool) {
        #[cfg(windows)]
        win::place(self.hwnd, rect, visible);
        #[cfg(not(windows))]
        let _ = (rect, visible);
    }
}

impl Drop for EditorFrame {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            let hwnd = self.hwnd;
            crate::main_thread::main_thread().post(move || win::destroy(hwnd));
        }
    }
}

#[cfg(windows)]
mod win {
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const CLASS: &[u16] = &[
        b'P' as u16,
        b'h' as u16,
        b'E' as u16,
        b'd' as u16,
        b'i' as u16,
        b't' as u16,
        b'o' as u16,
        b'r' as u16,
        b'F' as u16,
        b'r' as u16,
        b'a' as u16,
        b'm' as u16,
        b'e' as u16,
        0,
    ];

    unsafe extern "system" fn proc_(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        // SAFETY: forwarding to the default procedure
        unsafe { DefWindowProcW(h, m, w, l) }
    }

    fn register() -> bool {
        static DONE: OnceLock<bool> = OnceLock::new();
        *DONE.get_or_init(|| {
            // SAFETY: registering a plain window class once
            unsafe {
                let wc = WNDCLASSW {
                    style: 0,
                    lpfnWndProc: Some(proc_),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: GetModuleHandleW(std::ptr::null()),
                    hIcon: std::ptr::null_mut(),
                    hCursor: std::ptr::null_mut(),
                    hbrBackground: std::ptr::null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: CLASS.as_ptr(),
                };
                RegisterClassW(&wc) != 0
            }
        })
    }

    /// Main thread: a hidden child of `parent`.
    pub fn create(parent: isize) -> isize {
        if !register() {
            return 0;
        }
        // SAFETY: plain CreateWindowExW with a registered class and a live parent
        unsafe {
            CreateWindowExW(
                0,
                CLASS.as_ptr(),
                std::ptr::null(),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                parent as HWND,
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null(),
            ) as isize
        }
    }

    pub fn place(hwnd: isize, [x, y, w, h]: [i32; 4], visible: bool) {
        let flags = SWP_NOACTIVATE
            | SWP_ASYNCWINDOWPOS
            | if visible {
                SWP_SHOWWINDOW
            } else {
                SWP_HIDEWINDOW
            };
        // SAFETY: async positioning of our own child window
        unsafe {
            SetWindowPos(hwnd as HWND, HWND_TOP, x, y, w.max(1), h.max(1), flags);
        }
    }

    /// Main thread.
    pub fn destroy(hwnd: isize) {
        // SAFETY: destroying our own window on the thread that created it
        unsafe {
            DestroyWindow(hwnd as HWND);
        }
    }
}
