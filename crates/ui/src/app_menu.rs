//! Thanh menu của OS (macOS; GPUI chỉ dựng menu trên macOS, nền khác `set_menus` không làm gì). Không có menu thì AppKit
//! không có menu chính cho app ⇒ thanh menu giữ tên + menu của app trước đó (chủ dự án gặp 11/11/2026: "Claude").
//!
//! Menu tối thiểu: menu app (Giới thiệu, Kiểm tra cập nhật… — ferry-gd2_01, Cài đặt…, Dịch vụ, Ẩn / Ẩn ứng dụng khác / Hiện tất cả, Thoát) + Cửa sổ (Thu nhỏ,
//! Phóng to, Đóng). Chữ qua i18n, dựng lại khi đổi ngôn ngữ ([`install`]). Tên in đậm đầu thanh menu là tên app của AppKit:
//! `CFBundleName` của `Ferry.app`; exe trần (bản dev) lấy tên tiến trình `ferry-ui` — chỉ bản đóng gói hiện "Ferry".

use ferry_core::VERSION;
use ferry_core::i18n::I18n;
use materiant::gpui_kit::{self, AnyWindowHandle, App, KeyBinding, Menu, MenuItem, SystemMenuType};
use materiant::gpui_kit::{Entity, WeakEntity};
// `actions!` trỏ crate `gpui`; bản không test chỉ có `materiant::gpui_kit`.
use materiant::gpui_kit as gpui;

use crate::platform::window_controls;
use crate::views::main_window::MainWindow;

gpui_kit::actions!(
    ferry,
    [About, CheckForUpdates, OpenSettings, HideApp, HideOthers, ShowAll, Quit, Minimize, Zoom, CloseWindow]
);

/// Phím tắt chuẩn của macOS. Gọi một lần lúc khởi động (phím hiện cạnh mục menu).
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-h", HideApp, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
    ]);
}

/// Đăng ký xử lý cho các mục menu. `handle` / `main`: cửa sổ chính (mọi mục cửa sổ nhắm vào nó).
pub fn register(handle: AnyWindowHandle, main: &Entity<MainWindow>, cx: &mut App) {
    let main: WeakEntity<MainWindow> = main.downgrade();
    cx.on_action(|_: &About, cx| {
        let name = cx.try_global::<MenuName>().map(|n| n.0.clone()).unwrap_or_else(|| "Ferry".into());
        about(&name);
    });
    let check_main = main.clone();
    cx.on_action(move |_: &CheckForUpdates, cx| {
        let main = check_main.clone();
        let _ = handle.update(cx, move |_, window, cx| {
            window.activate_window();
            let _ = main.update(cx, |main, cx| main.check_update(window, cx));
        });
    });
    cx.on_action(move |_: &OpenSettings, cx| {
        let main = main.clone();
        let _ = handle.update(cx, move |_, window, cx| {
            window.activate_window();
            let _ = main.update(cx, |main, cx| main.open_settings(window, cx));
        });
    });
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(move |_: &Minimize, cx| {
        let _ = handle.update(cx, |_, window, _| window_controls::minimize(window));
    });
    cx.on_action(move |_: &Zoom, cx| {
        let _ = handle.update(cx, |_, window, _| window_controls::toggle_maximize(window));
    });
    cx.on_action(move |_: &CloseWindow, cx| {
        let _ = handle.update(cx, |_, window, cx| window_controls::close(window, cx));
    });
}

/// Tên app đã dịch, cho bảng Giới thiệu.
struct MenuName(String);
impl gpui_kit::Global for MenuName {}

/// Dựng (lại) thanh menu theo ngôn ngữ của `i18n`.
pub fn install(i18n: &I18n, cx: &mut App) {
    let name = i18n.t("app.name").to_owned();
    let t = |key: &str| i18n.t_args(key, &[("app", &name)]);
    cx.set_global(MenuName(name.clone()));
    cx.set_menus([
        Menu {
            name: name.clone().into(),
            disabled: false,
            items: vec![
                MenuItem::action(t("menu.about"), About),
                MenuItem::action(t("menu.check_updates"), CheckForUpdates),
                MenuItem::separator(),
                MenuItem::action(t("menu.settings"), OpenSettings),
                MenuItem::separator(),
                MenuItem::os_submenu(t("menu.services"), SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action(t("menu.hide"), HideApp),
                MenuItem::action(t("menu.hide_others"), HideOthers),
                MenuItem::action(t("menu.show_all"), ShowAll),
                MenuItem::separator(),
                MenuItem::action(t("menu.quit"), Quit),
            ],
        },
        Menu {
            name: t("menu.window").into(),
            disabled: false,
            items: vec![
                MenuItem::action(t("menu.minimize"), Minimize),
                MenuItem::action(t("menu.zoom"), Zoom),
                MenuItem::separator(),
                MenuItem::action(t("menu.close"), CloseWindow),
            ],
        },
    ]);
}

fn about(name: &str) {
    #[cfg(target_os = "macos")]
    crate::platform::macos::about_panel(name, VERSION);
    #[cfg(not(target_os = "macos"))]
    let _ = (name, VERSION);
}
