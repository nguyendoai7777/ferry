//! Icon khay + menu, chung mọi OS: chữ qua i18n, icon vẽ bằng mã theo [`TrayMode`]. Phần gắn vào OS: `tray-icon` trên
//! Windows / macOS ([`Tray`] dưới đây), StatusNotifierItem tự viết trên Linux (`platform/linux/sni.rs`).
//!
//! Ba trạng thái (gd1_07): rảnh = khung điện thoại; đang gửi = màn hình đầy dần từ đáy theo phần tư (≥ một phần tư);
//! cần chú ý (cần ghép lại / hết chỗ) = chấm tròn góc trên phải, cắt một vòng trong suốt quanh chấm cho tách khỏi khung.
//! Icon tạm, chờ bộ icon (`plan/_backlog.md`). macOS ảnh template (đen + alpha, OS tự đổi theo thanh menu sáng / tối);
//! Windows màu nhấn của app, chấm cam; Linux gần trắng (thanh trên GNOME luôn tối), chấm cam.

use ferry_core::i18n::I18n;

use crate::tray_state::{PauseItem, TrayMode};

pub const OPEN_ID: &str = "open";
pub const SETTINGS_ID: &str = "settings";
pub const PAUSE_ID: &str = "pause";
pub const QUIT_ID: &str = "quit";

#[derive(Debug, PartialEq, Eq)]
pub enum MenuAction {
    OpenWindow,
    /// "Cài đặt…" (gd1_12).
    OpenSettings,
    /// "Tạm dừng tất cả" / "Tiếp tục tất cả" — engine chọn theo trạng thái lúc bấm.
    TogglePause,
    Quit,
}

pub fn menu_action(id: &str) -> Option<MenuAction> {
    match id {
        OPEN_ID => Some(MenuAction::OpenWindow),
        SETTINGS_ID => Some(MenuAction::OpenSettings),
        PAUSE_ID => Some(MenuAction::TogglePause),
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

/// Menu khay: Mở cửa sổ · Cài đặt… · Tạm dừng / Tiếp tục tất cả · Thoát.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn menu_entries(i18n: &I18n, pause: PauseItem) -> Vec<MenuEntry> {
    vec![
        MenuEntry::Item { id: OPEN_ID, label: i18n.t("tray.open_window").to_owned(), enabled: true },
        MenuEntry::Item { id: SETTINGS_ID, label: i18n.t("tray.settings").to_owned(), enabled: true },
        MenuEntry::Item {
            id: PAUSE_ID,
            label: pause_label(i18n, pause).to_owned(),
            enabled: pause != PauseItem::Disabled,
        },
        MenuEntry::Separator,
        MenuEntry::Item { id: QUIT_ID, label: i18n.t("tray.quit").to_owned(), enabled: true },
    ]
}

pub fn pause_label(i18n: &I18n, pause: PauseItem) -> &str {
    match pause {
        PauseItem::Resume => i18n.t("tray.resume_all"),
        PauseItem::Pause | PauseItem::Disabled => i18n.t("tray.pause_all"),
    }
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

/// Màu khung + màu chấm "cần chú ý".
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub frame: [u8; 3],
    pub badge: [u8; 3],
}

/// Template macOS: một màu, OS tô lại.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const TEMPLATE: Palette = Palette { frame: [0, 0, 0], badge: [0, 0, 0] };
#[cfg_attr(not(windows), allow(dead_code))]
pub const WINDOWS: Palette = Palette { frame: [0x00, 0x5c, 0xbb], badge: [0xf0, 0x8a, 0x00] };
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const LINUX: Palette = Palette { frame: [0xf2, 0xf2, 0xf2], badge: [0xff, 0x9a, 0x1f] };

/// Khung điện thoại đứng: viền bo góc dày ~1/12 cạnh + chấm ở đáy; thêm phần màn đã đầy / chấm chú ý theo `mode`.
/// RGBA thẳng (không nhân trước alpha).
pub fn icon_rgba(size: u32, mode: TrayMode, palette: Palette) -> Vec<u8> {
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
    let cover = |d: f32| (0.5 - d).clamp(0.0, 1.0);
    let dot = (s / 2.0, y0 + h - stroke * 2.2, stroke * 0.9);
    // Màn hình: trong khung, chừa khe một nét, trên chấm đáy.
    let gap = stroke * 2.0;
    let (screen_top, screen_bottom) = (y0 + gap, dot.1 - dot.2 - stroke * 0.8);
    let fill_from = match mode {
        TrayMode::Sending { percent } => {
            Some(screen_bottom - (screen_bottom - screen_top) * f32::from(quarters(percent)) / 4.0)
        }
        _ => None,
    };
    let badge = (mode == TrayMode::Attention).then(|| {
        let br = s * 0.19;
        (x0 + w - br * 0.5, y0 + br * 1.05, br)
    });
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let ring = cover(sdf(px, py, 0.0)) - cover(sdf(px, py, stroke));
            let mut a = ring.max(cover((px - dot.0).hypot(py - dot.1) - dot.2));
            if let Some(top) = fill_from {
                let inside = cover(sdf(px, py, gap));
                let rows = (py - top + 0.5).clamp(0.0, 1.0) * (screen_bottom - py + 0.5).clamp(0.0, 1.0);
                a = a.max(inside * rows);
            }
            let mut rgb = palette.frame;
            if let Some((bx, by, br)) = badge {
                let d = (px - bx).hypot(py - by);
                let disc = cover(d - br);
                // Vòng trống quanh chấm (dày ~ một nét) xoá phần khung bên dưới.
                let cut = cover(d - br - stroke);
                a = a * (1.0 - cut) + disc;
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
    use crate::tray_state::{PauseItem, TrayMode, TrayStatus};

    pub struct Tray {
        icon: TrayIcon,
        open: MenuItem,
        settings: MenuItem,
        pause: MenuItem,
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
            let open = MenuItem::with_id(OPEN_ID, i18n.t("tray.open_window"), true, None);
            let settings = MenuItem::with_id(SETTINGS_ID, i18n.t("tray.settings"), true, None);
            let pause = MenuItem::with_id(PAUSE_ID, i18n.t("tray.pause_all"), false, None);
            let quit = MenuItem::with_id(QUIT_ID, i18n.t("tray.quit"), true, None);
            menu.append_items(&[&open, &settings, &pause, &PredefinedMenuItem::separator(), &quit])
                .map_err(|e| format!("menu: {e}"))?;
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
            Ok(Tray { icon, open, settings, pause, quit, key: 0, status: None })
        }

        pub fn apply(&mut self, status: &TrayStatus, i18n: &I18n) {
            if self.status.as_ref() == Some(status) {
                return;
            }
            let key = icon_key(status.mode);
            if key != self.key {
                self.key = key;
                #[cfg(target_os = "macos")]
                let r = self.icon.set_icon_with_as_template(Some(icon(status.mode)), true);
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
                self.open.set_text(i18n.t("tray.open_window"));
                self.settings.set_text(i18n.t("tray.settings"));
                self.quit.set_text(i18n.t("tray.quit"));
            }
            self.pause.set_text(pause_label(i18n, status.pause));
            self.pause.set_enabled(status.pause != PauseItem::Disabled);
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
        assert_eq!(menu_action(SETTINGS_ID), Some(MenuAction::OpenSettings));
        assert_eq!(menu_action(PAUSE_ID), Some(MenuAction::TogglePause));
        assert_eq!(menu_action(QUIT_ID), Some(MenuAction::Quit));
        assert_eq!(menu_action("other"), None);
    }

    /// gd1_12: đổi ngôn ngữ trong cài đặt ⇒ menu dựng lại đúng chữ.
    #[test]
    fn menu_follows_the_language() {
        let first = |lang| match &menu_entries(&I18n::new(lang), PauseItem::Disabled)[1] {
            MenuEntry::Item { id, label, .. } => (*id, label.clone()),
            MenuEntry::Separator => unreachable!(),
        };
        assert_eq!(first(Language::En), (SETTINGS_ID, "Settings…".to_owned()));
        assert_eq!(first(Language::Vi), (SETTINGS_ID, "Cài đặt…".to_owned()));
    }

    #[test]
    fn menu_follows_the_pause_state() {
        let i18n = I18n::new(Language::En);
        let label = |p| match &menu_entries(&i18n, p)[2] {
            MenuEntry::Item { label, enabled, .. } => (label.clone(), *enabled),
            MenuEntry::Separator => unreachable!(),
        };
        assert_eq!(label(PauseItem::Pause), ("Pause all".to_owned(), true));
        assert_eq!(label(PauseItem::Resume), ("Resume all".to_owned(), true));
        assert_eq!(label(PauseItem::Disabled), ("Pause all".to_owned(), false));
        for e in menu_entries(&i18n, PauseItem::Pause) {
            if let MenuEntry::Item { id, .. } = e {
                assert!(menu_action(id).is_some(), "{id}");
            }
        }
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

    #[test]
    fn idle_icon_has_transparent_corners_and_a_hollow_screen() {
        for size in [16, 22, 24, 32, 36, 48, 64] {
            let rgba = icon_rgba(size, TrayMode::Idle, TEMPLATE);
            let n = size as usize;
            assert_eq!(rgba.len(), n * n * 4);
            assert_eq!(alpha(&rgba, n, 0, 0), 0, "corner, size {size}");
            assert_eq!(alpha(&rgba, n, n / 2, n / 2), 0, "screen is hollow, size {size}");
            assert!((0..n).any(|x| alpha(&rgba, n, x, n / 2) == 255), "frame edge is solid, size {size}");
        }
    }

    #[test]
    fn sending_fills_the_screen_from_the_bottom() {
        let n = 48usize;
        let ink =
            |p| icon_rgba(n as u32, TrayMode::Sending { percent: p }, TEMPLATE).chunks(4).filter(|c| c[3] > 0).count();
        let idle = icon_rgba(n as u32, TrayMode::Idle, TEMPLATE).chunks(4).filter(|c| c[3] > 0).count();
        assert!(ink(0) > idle, "sending shows at least a quarter");
        assert!(ink(50) > ink(10));
        assert!(ink(100) > ink(50));
        let half = icon_rgba(n as u32, TrayMode::Sending { percent: 50 }, TEMPLATE);
        // Giữa màn, hơi dưới tâm: đã đầy; gần đỉnh màn: còn trống.
        assert!(alpha(&half, n, n / 2, n / 2 + n / 16) > 200);
        assert_eq!(alpha(&half, n, n / 2, n / 4), 0);
    }

    #[test]
    fn attention_draws_a_badge_in_the_badge_color() {
        let n = 32usize;
        let rgba = icon_rgba(n as u32, TrayMode::Attention, WINDOWS);
        let orange = rgba.chunks(4).filter(|c| c[3] > 200 && c[..3] == WINDOWS.badge).count();
        assert!(orange > 10, "{orange}");
        let idle = icon_rgba(n as u32, TrayMode::Idle, WINDOWS);
        assert_ne!(rgba, idle);
    }
}
