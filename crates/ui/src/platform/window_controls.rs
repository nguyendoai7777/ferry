//! Bê từ Groovex (window-buttons 03/11/2026): ba nút cửa sổ tự vẽ của thanh trên (thu nhỏ, phóng to / khôi phục, đóng).
//!
//! Nút là phần tử bấm thường của GPUI (`on_click`), không phải vùng `WindowControlArea` để OS hit-test: thanh trên nhúng
//! `cached`, mà gpui-pre 0.3.10 không chép vùng `WindowControlArea` khi dùng lại khung của view `cached` — trên màn thật
//! (có hoạt ảnh, rê chuột) `HTMINBUTTON` / `HTMAXBUTTON` mất, bấm không ăn. Nút bấm thường đi đường mọi nút khác của app.
//! `zoom_window` của gpui-pre-windows chỉ `SW_MAXIMIZE`, nên Windows gọi thẳng `ShowWindowAsync` để bấm lần hai khôi phục cỡ
//! cũ; đóng gửi `WM_CLOSE` như nút tiêu đề thật (đi qua `on_window_should_close`). Nền khác: API cửa sổ của GPUI.
//!
//! macOS (macos-window-chrome, 11/11/2026): cửa sổ có thanh tiêu đề trong suốt của hệ thống ([`titlebar`]) — đèn giao thông
//! native ở góc trái, căn giữa thanh trên — vì `titlebar: None` khiến gpui-pre-macos 0.3.10 tạo NSWindow thiếu cờ
//! `Miniaturizable` / `Resizable` ⇒ `miniaturize:` / `zoom:` bị AppKit bỏ qua (ba nút tự vẽ bấm không ăn). Ba nút tự vẽ vẫn
//! ở góc phải như cửa sổ Cài đặt của fastcast (kiểu app Claude desktop: ô vuông bo góc, có khe, chừa mép phải cho góc bo).

use materiant::gpui_kit::{Pixels, Point, TitlebarOptions, Window, point, px};

/// macOS: hệ thống vẽ đèn giao thông ở góc trái trên; thanh trên + đầu cột trái chừa chỗ cho chúng.
pub const NATIVE_CAPTION_BUTTONS: bool = cfg!(target_os = "macos");
/// Mép trái nút Đóng của đèn giao thông.
const TRAFFIC_LIGHT_X: f32 = 12.;

/// Cạnh một nút đèn giao thông, điểm logic (macOS 26: 14, trước đó 12). Nền khác không có: số không dùng tới.
pub fn caption_button_size() -> f32 {
    #[cfg(target_os = "macos")]
    return super::macos::caption_button_size();
    #[cfg(not(target_os = "macos"))]
    12.
}

/// Chỗ đặt đèn giao thông (`TitlebarOptions::traffic_light_position`) để chúng nằm giữa hàng cao `bar_height` theo chiều
/// dọc: GPUI đặt nút cách mép trên đúng `y` ⇒ `y` = (cao hàng − cạnh nút) / 2.
pub fn traffic_lights(bar_height: f32) -> Point<Pixels> {
    point(px(TRAFFIC_LIGHT_X), px((bar_height - caption_button_size()) / 2.))
}

/// macOS: thanh tiêu đề của hệ thống trong suốt (nội dung vẽ tràn lên dưới nó), chỉ còn đèn giao thông; nền khác `None`
/// (không thanh của OS, `WindowDecorations::Client`).
pub fn titlebar(title: &str, bar_height: f32) -> Option<TitlebarOptions> {
    NATIVE_CAPTION_BUTTONS.then(|| TitlebarOptions {
        title: Some(title.to_owned().into()),
        appears_transparent: true,
        traffic_light_position: Some(traffic_lights(bar_height)),
    })
}

pub fn minimize(window: &Window) {
    window.minimize_window();
}

/// Phóng to, hoặc khôi phục khi đang phóng to.
pub fn toggle_maximize(window: &Window) {
    #[cfg(windows)]
    if let Some(hwnd) = sys::hwnd(window) {
        sys::toggle_maximize(hwnd);
        return;
    }
    window.zoom_window();
}

#[cfg(windows)]
pub fn hwnd(window: &Window) -> Option<windows_sys::Win32::Foundation::HWND> {
    sys::hwnd(window)
}

pub fn close(window: &mut Window) {
    #[cfg(windows)]
    if let Some(hwnd) = sys::hwnd(window) {
        sys::close(hwnd);
        return;
    }
    window.remove_window();
}

#[cfg(windows)]
mod sys {
    use materiant::gpui_kit::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsZoomed, PostMessageW, SW_MAXIMIZE, SW_RESTORE, ShowWindowAsync, WM_CLOSE,
    };

    pub fn hwnd(window: &Window) -> Option<HWND> {
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
            _ => None,
        }
    }

    pub fn toggle_maximize(hwnd: HWND) {
        // SAFETY: `hwnd` là cửa sổ đang sống của tiến trình này (vừa lấy từ `Window` đang vẽ).
        unsafe {
            let show = if IsZoomed(hwnd) != 0 { SW_RESTORE } else { SW_MAXIMIZE };
            ShowWindowAsync(hwnd, show);
        }
    }

    pub fn close(hwnd: HWND) {
        // SAFETY: như trên; `WM_CLOSE` gửi bất đồng bộ, GPUI xử lý ở vòng thông điệp kế.
        unsafe {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    }
}
