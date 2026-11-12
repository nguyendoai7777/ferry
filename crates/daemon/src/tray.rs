//! Icon khay + menu, chung mọi OS: chữ qua i18n, icon vẽ bằng mã theo [`TrayMode`]. Phần gắn vào OS: `tray-icon` trên
//! Windows / macOS ([`Tray`] dưới đây), StatusNotifierItem tự viết trên Linux (`platform/linux/sni.rs`).
//!
//! Ba trạng thái: rảnh = máy bay giấy (cùng dáng icon app, `packaging/icons/gen-icons.mjs`): cánh trên + sống gấp đậm, cánh
//! dưới nhạt, các mặt gấp cách nhau một khe trong suốt; đang gửi = máy bay mờ, phần đã gửi đậm dần từ đuôi lên mũi theo
//! phần tư (≥ một phần tư); cần chú ý (cần ghép lại / hết chỗ) = chấm tròn góc dưới phải, cắt một vòng trong suốt quanh chấm.
//! macOS ảnh template (đen + alpha, OS tự đổi theo thanh menu sáng / tối); Windows xanh của icon app, chấm cam; Linux gần
//! trắng (thanh trên GNOME luôn tối), chấm cam.

use ferry_core::i18n::I18n;

use crate::tray_state::TrayMode;

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

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub enum MenuEntry {
    Item { id: &'static str, label: String, enabled: bool },
    Separator,
}

/// Menu khay: Mở · Thoát (12/11/2026: bỏ "Cài đặt…" và "Tạm dừng / Tiếp tục tất cả" — cả hai có trong cửa sổ).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn menu_entries(i18n: &I18n) -> Vec<MenuEntry> {
    vec![
        MenuEntry::Item { id: OPEN_ID, label: i18n.t("tray.open").to_owned(), enabled: true },
        MenuEntry::Separator,
        MenuEntry::Item { id: QUIT_ID, label: i18n.t("tray.quit").to_owned(), enabled: true },
    ]
}

/// Icon chỉ đổi theo bậc này (đang gửi: theo phần tư) — khỏi đẩy ảnh mới cho OS mỗi % tiến độ.
pub fn icon_key(mode: TrayMode) -> u8 {
    match mode {
        TrayMode::Idle => 0,
        TrayMode::Sending { percent } => quarters(percent),
        TrayMode::Attention => 9,
    }
}

fn quarters(percent: u8) -> u8 {
    percent.min(100).div_ceil(25).max(1)
}

/// Màu máy bay + màu chấm "cần chú ý".
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub frame: [u8; 3],
    pub badge: [u8; 3],
}

/// Template macOS: một màu, OS tô lại.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const TEMPLATE: Palette = Palette { frame: [0, 0, 0], badge: [0, 0, 0] };
#[cfg_attr(not(windows), allow(dead_code))]
pub const WINDOWS: Palette = Palette { frame: [0x4d, 0x6b, 0xfe], badge: [0xf0, 0x8a, 0x00] };
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const LINUX: Palette = Palette { frame: [0xf2, 0xf2, 0xf2], badge: [0xff, 0x9a, 0x1f] };

/// Các mặt gấp của máy bay trong lưới 48 (mũi phải-trên), thứ tự vẽ: cánh trên, cánh dưới, sống gấp — cùng toạ độ với icon
/// app. Mặt vẽ sau khoét mặt vẽ trước một khe. Kèm độ đậm (alpha) của mặt.
const FACES: [(&[(f32, f32)], f32); 3] = [
    (&[(42.0, 7.0), (6.0, 21.0), (18.0, 26.0)], 1.0),
    (&[(42.0, 7.0), (18.0, 26.0), (35.0, 35.0)], 0.55),
    (&[(42.0, 7.0), (18.0, 26.0), (21.0, 37.0), (26.0, 31.0)], 1.0),
];
/// Khung bao máy bay trong lưới 48: x 6..42, y 7..37.
const PLANE_W: f32 = 36.0;
const PLANE_CENTER: (f32, f32) = (24.0, 22.0);
/// Trục bay (đuôi ⇒ mũi) để tô tiến độ gửi.
const TAIL: (f32, f32) = (8.0, 33.0);
const NOSE: (f32, f32) = (42.0, 7.0);
/// Độ đậm phần chưa gửi khi đang gửi.
const PENDING_ALPHA: f32 = 0.3;

/// Khoảng cách có dấu (đơn vị lưới 48) tới đa giác lồi, âm = bên trong; xấp xỉ bằng max các cạnh (đủ cho khử răng cưa).
fn polygon_distance(pts: &[(f32, f32)], x: f32, y: f32) -> f32 {
    let area: f32 = (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            a.0 * b.1 - b.0 * a.1
        })
        .sum();
    let sign = if area < 0.0 { -1.0 } else { 1.0 };
    (0..pts.len())
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            let len = (b.0 - a.0).hypot(b.1 - a.1);
            sign * ((x - a.0) * (b.1 - a.1) - (y - a.1) * (b.0 - a.0)) / len
        })
        .fold(f32::MIN, f32::max)
}

/// Máy bay giấy ba mặt + phần đã gửi / chấm chú ý theo `mode`. RGBA thẳng (không nhân trước alpha).
pub fn icon_rgba(size: u32, mode: TrayMode, palette: Palette) -> Vec<u8> {
    let s = size as f32;
    // Điểm ảnh trên một đơn vị lưới 48: máy bay rộng ~0,94 cạnh.
    let k = s * 0.94 / PLANE_W;
    // Khe giữa các mặt: 1,5 / 48 như icon app, nhưng không mảnh hơn một điểm ảnh (cỡ 16 vẫn tách được).
    let gap = (1.5 * k).max(1.0);
    let cover = |d: f32| (0.5 - d).clamp(0.0, 1.0);
    let done = match mode {
        // Gửi xong (đủ bốn phần tư) ⇒ cả máy bay đậm như lúc rảnh, không để viền mềm làm mờ mũi.
        TrayMode::Sending { percent } if quarters(percent) < 4 => Some(f32::from(quarters(percent)) / 4.0),
        _ => None,
    };
    let axis = (NOSE.0 - TAIL.0, NOSE.1 - TAIL.1);
    let axis_len2 = axis.0 * axis.0 + axis.1 * axis.1;
    let badge = (mode == TrayMode::Attention).then(|| {
        let br = s * 0.19;
        (s - br * 1.05, s - br * 1.05, br)
    });
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // Toạ độ lưới 48.
            let (gx, gy) = (PLANE_CENTER.0 + (px - s / 2.0) / k, PLANE_CENTER.1 + (py - s / 2.0) / k);
            // Khoảng cách theo điểm ảnh tới từng mặt.
            let d: Vec<f32> = FACES.iter().map(|(pts, _)| polygon_distance(pts, gx, gy) * k).collect();
            let mut a = 0.0f32;
            for (i, (_, weight)) in FACES.iter().enumerate() {
                let mut c = cover(d[i]);
                for &later in &d[i + 1..] {
                    c *= 1.0 - cover(later - gap);
                }
                a += c * weight;
            }
            if let Some(done) = done {
                // Vị trí dọc trục bay (0 = đuôi, 1 = mũi), viền mềm khoảng một điểm ảnh.
                let t = ((gx - TAIL.0) * axis.0 + (gy - TAIL.1) * axis.1) / axis_len2;
                let soft = 1.0 / (k * axis_len2.sqrt());
                let sent = ((done - t) / soft + 0.5).clamp(0.0, 1.0);
                a *= PENDING_ALPHA + (1.0 - PENDING_ALPHA) * sent;
            }
            let mut rgb = palette.frame;
            if let Some((bx, by, br)) = badge {
                let dist = (px - bx).hypot(py - by);
                let disc = cover(dist - br);
                // Vòng trống quanh chấm (dày bằng khe) xoá phần máy bay bên dưới.
                a = a * (1.0 - cover(dist - br - gap)) + disc;
                if disc > 0.0 {
                    rgb = palette.badge;
                }
            }
            out.extend_from_slice(&[rgb[0], rgb[1], rgb[2], (a.min(1.0) * 255.0).round() as u8]);
        }
    }
    out
}

#[cfg(not(target_os = "linux"))]
pub use native::Tray;

#[cfg(not(target_os = "linux"))]
mod native {
    use ferry_core::i18n::I18n;
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    use super::*;
    use crate::tray_state::{TrayMode, TrayStatus};

    pub struct Tray {
        icon: TrayIcon,
        open: MenuItem,
        quit: MenuItem,
        key: u8,
        status: Option<TrayStatus>,
    }

    #[cfg(target_os = "macos")]
    const SIZE: u32 = 36;
    #[cfg(not(target_os = "macos"))]
    const SIZE: u32 = 32;

    fn icon(mode: TrayMode) -> Icon {
        #[cfg(target_os = "macos")]
        let palette = TEMPLATE;
        #[cfg(not(target_os = "macos"))]
        let palette = WINDOWS;
        Icon::from_rgba(icon_rgba(SIZE, mode, palette), SIZE, SIZE).expect("valid icon size")
    }

    impl Tray {
        pub fn new(i18n: &I18n) -> Result<Tray, String> {
            let menu = Menu::new();
            let open = MenuItem::with_id(OPEN_ID, i18n.t("tray.open"), true, None);
            let quit = MenuItem::with_id(QUIT_ID, i18n.t("tray.quit"), true, None);
            menu.append_items(&[&open, &PredefinedMenuItem::separator(), &quit]).map_err(|e| format!("menu: {e}"))?;
            let builder = TrayIconBuilder::new();
            #[cfg(target_os = "macos")]
            let builder = builder.with_icon_templated(icon(TrayMode::Idle)).with_menu_on_left_click(true);
            // Windows: chuột trái mở cửa sổ (`platform::install_event_handlers`), chuột phải hiện menu.
            #[cfg(not(target_os = "macos"))]
            let builder = builder.with_icon(icon(TrayMode::Idle)).with_menu_on_left_click(false);
            let icon = builder
                .with_tooltip(i18n.t("app.name"))
                .with_menu(Box::new(menu))
                .build()
                .map_err(|e| e.to_string())?;
            Ok(Tray { icon, open, quit, key: 0, status: None })
        }

        pub fn apply(&mut self, status: &TrayStatus, i18n: &I18n) {
            if self.status.as_ref() == Some(status) {
                return;
            }
            let key = icon_key(status.mode);
            if key != self.key {
                self.key = key;
                #[cfg(target_os = "macos")]
                let r = self.icon.set_icon_templated(Some(icon(status.mode)));
                #[cfg(not(target_os = "macos"))]
                let r = self.icon.set_icon(Some(icon(status.mode)));
                if let Err(e) = r {
                    log::warn!("tray: set icon: {e}");
                }
            }
            if let Err(e) = self.icon.set_tooltip(Some(&status.tooltip)) {
                log::warn!("tray: set tooltip: {e}");
            }
            if self.status.as_ref().is_none_or(|s| s.language != status.language) {
                self.open.set_text(i18n.t("tray.open"));
                self.quit.set_text(i18n.t("tray.quit"));
            }
            self.status = Some(status.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::i18n::Language;

    #[test]
    fn menu_ids_round_trip() {
        assert_eq!(menu_action(OPEN_ID), Some(MenuAction::OpenWindow));
        assert_eq!(menu_action(QUIT_ID), Some(MenuAction::Quit));
        assert_eq!(menu_action("settings"), None);
        assert_eq!(menu_action("pause"), None);
    }

    /// Menu chỉ còn Mở · Thoát, đổi ngôn ngữ trong cài đặt (gd1_12) ⇒ dựng lại đúng chữ.
    #[test]
    fn menu_is_open_and_quit_in_the_language() {
        let labels = |lang| {
            menu_entries(&I18n::new(lang))
                .into_iter()
                .map(|e| match e {
                    MenuEntry::Item { id, label, enabled } => {
                        assert!(enabled && menu_action(id).is_some(), "{id}");
                        label
                    }
                    MenuEntry::Separator => "-".to_owned(),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(labels(Language::En), ["Open", "-", "Quit"]);
        assert_eq!(labels(Language::Vi), ["Mở", "-", "Thoát"]);
    }

    #[test]
    fn icon_changes_only_per_quarter() {
        assert_eq!(icon_key(TrayMode::Sending { percent: 0 }), 1);
        assert_eq!(icon_key(TrayMode::Sending { percent: 25 }), 1);
        assert_eq!(icon_key(TrayMode::Sending { percent: 26 }), 2);
        assert_eq!(icon_key(TrayMode::Sending { percent: 100 }), 4);
        assert_ne!(icon_key(TrayMode::Idle), icon_key(TrayMode::Attention));
    }

    fn alpha(rgba: &[u8], n: usize, x: usize, y: usize) -> u8 {
        rgba[(y * n + x) * 4 + 3]
    }

    fn ink(rgba: &[u8]) -> u32 {
        rgba.chunks(4).map(|c| u32::from(c[3])).sum()
    }

    #[test]
    fn idle_icon_is_a_plane_with_gaps_between_faces() {
        for size in [16, 22, 24, 32, 36, 48, 64] {
            let rgba = icon_rgba(size, TrayMode::Idle, TEMPLATE);
            let n = size as usize;
            assert_eq!(rgba.len(), n * n * 4);
            // Góc trên trái + dưới trái trống (máy bay chéo lên phải).
            assert_eq!(alpha(&rgba, n, 0, 0), 0, "top-left, size {size}");
            assert_eq!(alpha(&rgba, n, 0, n - 1), 0, "bottom-left, size {size}");
            assert!(rgba.chunks(4).any(|c| c[3] == 255), "solid wing, size {size}");
            // Cánh dưới nhạt hơn: có điểm ảnh độ đậm vừa (không chỉ viền khử răng cưa).
            let mid = rgba.chunks(4).filter(|c| (120..=160).contains(&c[3])).count();
            assert!(mid >= n / 2, "lighter lower wing, size {size}: {mid}");
        }
    }

    #[test]
    fn sending_darkens_the_plane_from_tail_to_nose() {
        let n = 48u32;
        let at = |p| icon_rgba(n, TrayMode::Sending { percent: p }, TEMPLATE);
        let idle = icon_rgba(n, TrayMode::Idle, TEMPLATE);
        assert!(ink(&at(0)) < ink(&at(50)));
        assert!(ink(&at(50)) < ink(&at(100)));
        assert_eq!(at(100), idle, "all sent = full plane");
        // Một phần tư: đuôi (cánh trên gần mép trái) đã đậm, mũi còn mờ.
        let q = at(10);
        let (tail_x, tail_y) = ((10.0 - 24.0) * 48.0 * 0.94 / 36.0 + 24.0, (21.5 - 22.0) * 48.0 * 0.94 / 36.0 + 24.0);
        assert!(alpha(&q, 48, tail_x as usize, tail_y as usize) > 200);
        let (nose_x, nose_y) = ((38.0 - 24.0) * 48.0 * 0.94 / 36.0 + 24.0, (10.5 - 22.0) * 48.0 * 0.94 / 36.0 + 24.0);
        assert!(alpha(&q, 48, nose_x as usize, nose_y as usize) < 120);
    }

    #[test]
    fn attention_draws_a_badge_in_the_badge_color() {
        let n = 32usize;
        let rgba = icon_rgba(n as u32, TrayMode::Attention, WINDOWS);
        let orange = rgba.chunks(4).filter(|c| c[3] > 200 && c[..3] == WINDOWS.badge).count();
        assert!(orange > 10, "{orange}");
        let idle = icon_rgba(n as u32, TrayMode::Idle, WINDOWS);
        assert_ne!(rgba, idle);
        // Chấm ở góc dưới phải, góc trên phải (mũi máy bay) không đổi.
        assert_eq!(alpha(&rgba, n, n - 3, n - 3), 255);
        assert_eq!(&rgba[..n * 4 * 4], &idle[..n * 4 * 4]);
    }
}
