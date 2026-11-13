//! Cửa sổ `ferry-ui` (GPUI + materiant). Chỉ vẽ trạng thái daemon `ferry` gửi qua IPC và gửi lệnh ngược lại — không mạng,
//! không kho khoá, không SQLite (`CLAUDE.md` mục Bố cục). Đóng cửa sổ ⇒ tiến trình thoát, nhả GPU / RAM; daemon vẫn chạy.
//!
//! Một cửa sổ, vỏ như Groovex (không viền / thanh tiêu đề OS, cột trái điều hướng, thanh trên với ba nút cửa sổ): trang Gửi,
//! Thư viện điện thoại, Cài đặt (`views::main_window`).
//!
//! Mở lại như lúc đóng (12/11/2026): `daemon_link::connect` chờ `Restore` + `Snapshot` đầu, `Model` áp chúng, cửa sổ mở
//! đúng khung lần trước (daemon giữ, hoặc `ui-state.json` khi daemon mới chạy) và `MainWindow::restore` dựng trang trước
//! khung đầu.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_menu;
mod appearance;
mod assets;
mod daemon_link;
mod fmt;
mod model;
mod platform;
mod settings_link;
mod ui_state;
mod update;
mod views;

use std::rc::Rc;

use ferry_core::i18n::I18n;
use ferry_core::view::WindowState;
use ferry_core::{VERSION, config, logging, paths};
use materiant::gpui_kit::{
    self, ActivationPolicy, App, AppContext, Bounds, WindowBounds, WindowDecorations, WindowOptions, point, px, size,
};
use model::Model;
use platform::window_controls;
use views::app_bar;
use views::main_window::MainWindow;

/// Cỡ mở đầu của cửa sổ chính, px logic.
const WINDOW_SIZE: (f32, f32) = (860., 620.);
const WINDOW_MIN_SIZE: (f32, f32) = (640., 460.);
/// Wayland: dock lấy icon từ `<app_id>.desktop`. Bản cài là `ferry` (`packaging/linux/ferry.desktop`); bản debug mã riêng
/// `ferry.dev` ⇒ mục `ferry.dev.desktop` do `start.sh` ghi, chạy cạnh bản đã cài không gộp nhầm icon.
pub const APP_ID: &str = if cfg!(debug_assertions) { "ferry.dev" } else { "ferry" };

fn main() {
    let log_file = logging::init("ui");
    log::info!("ferry-ui {VERSION} starting (pid {}, log: {log_file:?})", std::process::id());

    // Chỉ đọc: daemon là bên ghi `config.json`.
    let config = paths::config_file()
        .and_then(|p| config::load(&p))
        .map_err(|e| log::warn!("config: {e}; using defaults"))
        .unwrap_or_default();
    let i18n = Rc::new(I18n::new(config.language.resolve()));
    let (theme, theme_preset) = (config.theme, config.theme_preset().to_owned());
    // `ferry-ui --settings`: mục "Cài đặt" của `ferry.desktop` (chuột phải icon dock, Linux) ⇒ mở thẳng trang Cài đặt.
    let open_settings = std::env::args().skip(1).any(|a| a == "--settings");
    let mut link = daemon_link::connect();
    let sender = link.as_ref().map(daemon_link::Link::sender);

    gpui_kit::application().with_assets(assets::Assets).run(move |cx: &mut App| {
        materiant::init(cx);
        // Nền + bảng màu đã lưu, font nhúng (D-20); gốc cửa sổ đặt `.font_family(cx.theme().font_family())`.
        appearance::apply(theme, &theme_preset, cx);
        devtool::init(cx);
        if cfg!(target_os = "macos") {
            // Exe trần (bản dev) không có `Info.plist`: nói rõ là app thường (Dock + thanh menu), rồi dựng thanh menu — không
            // có menu chính thì thanh menu giữ tên + menu của app trước đó.
            cx.set_activation_policy(ActivationPolicy::Regular);
            #[cfg(all(target_os = "macos", debug_assertions))]
            platform::macos::set_dev_dock_icon();
            app_menu::bind_keys(cx);
            app_menu::install(&i18n, cx);
        }

        let model = cx.new(|_| Model::new(sender));
        // ferry-gd2_01: Cmd / Ctrl+D cài bản mới.
        update::bind(model.clone(), cx);
        // `Restore` + `Snapshot` đầu (đã nhận trước khi mở) ⇒ khung đầu vẽ đúng trang, thư mục, hàng đợi.
        for msg in link.as_mut().map(|l| std::mem::take(&mut l.initial)).unwrap_or_default() {
            model.update(cx, |m, cx| m.handle(msg, cx));
        }
        let restore = model.update(cx, |m, _| m.restore.take());
        let saved_window = restore.as_ref().and_then(|v| v.window).or(model.read(cx).state.window);
        let options = WindowOptions {
            window_bounds: Some(initial_bounds(saved_window, cx)),
            // Như Groovex: không thanh tiêu đề OS, thanh trên + ba nút cửa sổ tự vẽ (`views::app_bar`). macOS: thanh tiêu
            // đề trong suốt của hệ thống, chỉ còn đèn giao thông giữa thanh trên (`window_controls::titlebar`). Tên cửa sổ
            // (taskbar, Alt+Tab) đặt bằng `set_window_title` ở lần vẽ đầu.
            titlebar: window_controls::titlebar(i18n.t("app.name"), app_bar::HEIGHT),
            window_decorations: Some(WindowDecorations::Client),
            window_min_size: Some(size(px(WINDOW_MIN_SIZE.0), px(WINDOW_MIN_SIZE.1))),
            app_id: Some(APP_ID.into()),
            ..materiant::window::window_options()
        };
        let (handle, main) = match gpui_kit::open_window(options, cx, |window, cx| {
            let (model, i18n) = (model.clone(), i18n.clone());
            cx.new(|cx| MainWindow::new(model, i18n, window, cx))
        }) {
            Ok(opened) => opened,
            Err(err) => {
                // Bản phát hành không có console: log mới là chỗ đọc được lý do.
                log::error!("cannot open the main window: {err:#}");
                cx.quit();
                return;
            }
        };
        let _ = handle.update(cx, |_, window, cx| {
            // Windows: kéo cửa sổ không vẽ lại cả cây mỗi nhịp (`platform::move_loop`); nền khác không làm gì.
            platform::move_loop::install(window);
            if let Some(view) = restore {
                main.update(cx, |main, cx| main.restore(view, window, cx));
            }
            if open_settings {
                main.update(cx, |main, cx| main.open_settings(window, cx));
            }
            // Mọi đường đóng cửa sổ: gửi trạng thái cho daemon + ghi khung cửa sổ trước khi tiến trình thoát.
            let weak = main.downgrade();
            window_controls::before_close(
                window,
                cx,
                Rc::new(move |window, cx| {
                    let _ = weak.update(cx, |main, cx| main.save_view(window, true, cx));
                }),
            );
        });
        // Thoát cả app khi cửa sổ còn mở (macOS Cmd+Q).
        let weak = main.downgrade();
        cx.on_app_quit(move |cx| {
            let weak = weak.clone();
            let _ = handle.update(cx, |_, window, cx| {
                let _ = weak.update(cx, |main, cx| main.save_view(window, true, cx));
            });
            async {}
        })
        .detach();
        if cfg!(target_os = "macos") {
            app_menu::register(handle, &main, cx);
        }
        // Một cửa sổ: đóng là thoát tiến trình (daemon vẫn chạy).
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
        // ferry-gd2_01: bản mới bật lần đầu ⇒ hộp "Có gì mới".
        update::whats_new_on_start(I18n::new(i18n.language()), cx);
        if let Some(link) = link {
            daemon_link::listen(link, model, handle, cx);
        }
    });
    log::info!("ferry-ui stopped");
}

/// Khung mở đầu: khung lần trước (kẹp cỡ tối thiểu; nằm ngoài mọi màn — đã rút màn phụ — thì giữ cỡ, đặt giữa màn chính),
/// hoặc cỡ mặc định giữa màn.
fn initial_bounds(saved: Option<WindowState>, cx: &App) -> WindowBounds {
    let Some(w) = saved.filter(WindowState::is_sane) else {
        return WindowBounds::Windowed(Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx));
    };
    let sz = size(px(w.width.max(WINDOW_MIN_SIZE.0)), px(w.height.max(WINDOW_MIN_SIZE.1)));
    let b = Bounds::new(point(px(w.x), px(w.y)), sz);
    // Thấy được ít nhất một góc đủ lớn để kéo về.
    let visible = cx.displays().iter().any(|d| {
        let i = d.bounds().intersect(&b);
        f32::from(i.size.width) >= 120. && f32::from(i.size.height) >= 80.
    });
    let b = if visible { b } else { Bounds::centered(None, sz, cx) };
    if w.maximized { WindowBounds::Maximized(b) } else { WindowBounds::Windowed(b) }
}
