//! Bê từ Groovex (drag-lag 03/11/2026): kéo cửa sổ không vẽ lại cả cây giao diện mỗi nhịp.
//!
//! gpui-pre 0.3.10: mỗi `WM_MOVE` gọi `Window::bounds_changed` → `refresh()`, mà `refresh` bỏ qua cache của mọi view `cached`
//! ⇒ mỗi nhịp kéo dựng và vẽ lại toàn bộ cửa sổ ngay trong vòng dời cửa sổ modal của Windows (`DefWindowProc` với
//! `HTCAPTION`): thấp khung hình, có bóng mờ, tốn CPU. Dời cửa sổ không đổi gì cần vẽ (toạ độ chuột, nội dung đều theo client).
//! Subclass này giữ lại `WM_MOVE` giữa `WM_ENTERSIZEMOVE` và `WM_EXITSIZEMOVE`, rồi chuyển đúng một `WM_MOVE` cuối cho GPUI khi
//! thả chuột: vị trí, màn hiện tại, `observe_window_bounds` (lưu vị trí) vẫn đúng sau khi kéo. Trong lúc kéo hoạt ảnh (tên
//! chạy, thanh nhảy) vẫn vẽ qua bộ hẹn giờ của GPUI. Đổi cỡ không bị giữ (`WM_SIZE` đi thẳng); kéo mép trái / trên thì vị
//! trí gốc báo trễ tới lúc thả. Nền khác: không làm gì.

#[cfg(windows)]
pub use sys::install;

#[cfg(not(windows))]
pub fn install(_: &materiant::gpui_kit::Window) {}

#[cfg(windows)]
mod sys {
    use std::cell::Cell;

    use materiant::gpui_kit::Window;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows_sys::Win32::UI::WindowsAndMessaging::{WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_MOVE, WM_NCDESTROY};

    const SUBCLASS_ID: usize = 0x6d6f_7665;

    #[derive(Default)]
    struct State {
        /// Đang trong vòng dời / đổi cỡ của OS.
        in_loop: Cell<bool>,
        /// `lParam` của `WM_MOVE` cuối cùng đã giữ lại.
        pending: Cell<Option<LPARAM>>,
    }

    /// Cài subclass cho cửa sổ (gọi trên luồng UI, một lần sau khi mở).
    pub fn install(window: &Window) {
        let Some(hwnd) = crate::platform::window_controls::hwnd(window) else { return };
        let state = Box::into_raw(Box::<State>::default());
        // SAFETY: `state` thuộc subclass, nhả ở `WM_NCDESTROY` hoặc ngay dưới đây nếu cài hỏng. `SetWindowSubclass` không gửi
        // thông điệp cho cửa sổ nên không chạy lồng wndproc của GPUI.
        unsafe {
            if SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, state as usize) == 0 {
                drop(Box::from_raw(state));
                log::warn!("move loop: cannot subclass the window, dragging repaints every frame");
            }
        }
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _: usize,
        data: usize,
    ) -> LRESULT {
        // SAFETY: `data` là `State` đã cài ở `install`, sống tới `WM_NCDESTROY`.
        let state = unsafe { &*(data as *const State) };
        match message {
            WM_ENTERSIZEMOVE => state.in_loop.set(true),
            WM_MOVE if state.in_loop.get() => {
                state.pending.set(Some(lparam));
                return 0;
            }
            WM_EXITSIZEMOVE => {
                state.in_loop.set(false);
                // SAFETY: chuyển tiếp nguyên tham số cho wndproc của GPUI, rồi một `WM_MOVE` với vị trí cuối.
                unsafe {
                    let result = DefSubclassProc(hwnd, message, wparam, lparam);
                    if let Some(last) = state.pending.take() {
                        DefSubclassProc(hwnd, WM_MOVE, 0, last);
                    }
                    return result;
                }
            }
            WM_NCDESTROY => {
                // SAFETY: gỡ đúng subclass đã cài, sau đó không còn ai gọi với `data` này.
                unsafe {
                    RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
                    drop(Box::from_raw(data as *mut State));
                    return DefSubclassProc(hwnd, message, wparam, lparam);
                }
            }
            _ => {}
        }
        // SAFETY: chuyển tiếp nguyên tham số cho wndproc của GPUI.
        unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
    }
}
