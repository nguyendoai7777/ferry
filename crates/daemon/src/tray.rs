//! Icon khay + menu (crate `tray-icon`; Linux chưa có — `platform/linux.rs`). Chữ qua i18n. gd1_01: một trạng thái,
//! menu "Mở cửa sổ" / "Thoát"; task gd1_07 thêm 3 trạng thái (rảnh / đang gửi / cần chú ý) và "Tạm dừng tất cả".
//!
//! Icon vẽ bằng mã (tạm, chờ bộ icon — `plan/_backlog.md`): khung điện thoại bo góc. macOS dùng ảnh template (đen +
//! alpha, OS tự đổi theo thanh menu sáng / tối); Windows màu nhấn của app.

#[cfg(not(target_os = "linux"))]
use mws_core::i18n::I18n;
#[cfg(not(target_os = "linux"))]
use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
#[cfg(not(target_os = "linux"))]
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub const OPEN_ID: &str = "open";
pub const QUIT_ID: &str = "quit";

#[derive(Debug, PartialEq, Eq)]
pub enum MenuAction {
    OpenWindow,
    Quit,
}

pub fn menu_action(id: &str) -> Option<MenuAction> {
    match id {
        OPEN_ID => Some(MenuAction::OpenWindow),
        QUIT_ID => Some(MenuAction::Quit),
        _ => None,
    }
}

#[cfg(not(target_os = "linux"))]
pub struct Tray {
    _icon: TrayIcon,
}

#[cfg(not(target_os = "linux"))]
impl Tray {
    pub fn new(i18n: &I18n) -> Result<Tray, String> {
        let menu = Menu::new();
        menu.append_items(&[
            &MenuItem::with_id(OPEN_ID, i18n.t("tray.open_window"), true, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id(QUIT_ID, i18n.t("tray.quit"), true, None),
        ])
        .map_err(|e| format!("menu: {e}"))?;
        let builder = TrayIconBuilder::new();
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(app_icon(36, [0, 0, 0])).with_menu_on_left_click(true);
        // Windows: chuột trái mở cửa sổ (`platform::install_event_handlers`), chuột phải hiện menu.
        #[cfg(not(target_os = "macos"))]
        let builder = builder.with_icon(app_icon(32, [0x00, 0x5c, 0xbb])).with_menu_on_left_click(false);
        let icon =
            builder.with_tooltip(i18n.t("app.name")).with_menu(Box::new(menu)).build().map_err(|e| e.to_string())?;
        Ok(Tray { _icon: icon })
    }
}

#[cfg(not(target_os = "linux"))]
fn app_icon(size: u32, rgb: [u8; 3]) -> Icon {
    Icon::from_rgba(phone_rgba(size, rgb), size, size).expect("valid icon size")
}

/// Khung điện thoại đứng: viền bo góc dày ~1/12 cạnh + chấm ở đáy. RGBA thẳng (không nhân trước alpha).
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub fn phone_rgba(size: u32, rgb: [u8; 3]) -> Vec<u8> {
    let s = size as f32;
    let (w, h) = (s * 0.56, s * 0.88);
    let (x0, y0) = ((s - w) / 2.0, (s - h) / 2.0);
    let r = s * 0.14;
    let stroke = (s / 12.0).max(1.5);
    // Khoảng cách có dấu tới hình chữ nhật bo góc (âm = bên trong).
    let sdf = |px: f32, py: f32, inset: f32| {
        let (hw, hh, rr) = (w / 2.0 - inset, h / 2.0 - inset, (r - inset).max(0.0));
        let dx = (px - (x0 + w / 2.0)).abs() - (hw - rr);
        let dy = (py - (y0 + h / 2.0)).abs() - (hh - rr);
        dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - rr
    };
    let dot = (s / 2.0, y0 + h - stroke * 2.2, stroke * 0.9);
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let outer = (0.5 - sdf(px, py, 0.0)).clamp(0.0, 1.0);
            let inner = (0.5 - sdf(px, py, stroke)).clamp(0.0, 1.0);
            let ring = outer - inner;
            let d = (px - dot.0).hypot(py - dot.1) - dot.2;
            let a = ring.max((0.5 - d).clamp(0.0, 1.0));
            out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], (a * 255.0).round() as u8]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_round_trip() {
        assert_eq!(menu_action(OPEN_ID), Some(MenuAction::OpenWindow));
        assert_eq!(menu_action(QUIT_ID), Some(MenuAction::Quit));
        assert_eq!(menu_action("other"), None);
    }

    #[test]
    fn icon_has_transparent_corners_and_a_visible_frame() {
        for size in [16, 32, 36, 64] {
            let rgba = phone_rgba(size, [0, 0, 0]);
            let n = size as usize;
            assert_eq!(rgba.len(), n * n * 4);
            let alpha = |x: usize, y: usize| rgba[(y * n + x) * 4 + 3];
            assert_eq!(alpha(0, 0), 0, "corner, size {size}");
            assert_eq!(alpha(n / 2, n / 2), 0, "screen is hollow, size {size}");
            assert!((0..n).any(|x| alpha(x, n / 2) == 255), "frame edge is solid, size {size}");
        }
    }
}
