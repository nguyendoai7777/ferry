//! Cửa sổ `ferry-ui` (GPUI + materiant). Chỉ vẽ trạng thái daemon `ferry` gửi qua IPC và gửi lệnh ngược lại — không mạng,
//! không kho khoá, không SQLite (`CLAUDE.md` mục Bố cục). Đóng cửa sổ ⇒ tiến trình thoát, nhả GPU / RAM; daemon vẫn chạy.
//!
//! Một cửa sổ, vỏ như Groovex (không viền / thanh tiêu đề OS, cột trái điều hướng, thanh trên với ba nút cửa sổ): trang Gửi,
//! Thư viện điện thoại, Cài đặt (`views::main_window`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod appearance;
mod assets;
mod daemon_link;
mod fmt;
mod model;
mod platform;
mod settings_link;
mod ui_state;
mod views;

use std::rc::Rc;

use ferry_core::i18n::I18n;
use ferry_core::{VERSION, config, logging, paths};
use materiant::gpui_kit::{self, App, AppContext, Bounds, WindowBounds, WindowDecorations, WindowOptions, px, size};
use model::Model;
use views::main_window::MainWindow;

/// Cỡ mở đầu của cửa sổ chính, px logic.
const WINDOW_SIZE: (f32, f32) = (860., 620.);
const WINDOW_MIN_SIZE: (f32, f32) = (640., 460.);

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
    // `ferry-ui --settings`: daemon bật từ menu khay "Cài đặt…" (gd1_12) ⇒ mở cửa sổ chính ở trang Cài đặt.
    let open_settings = std::env::args().skip(1).any(|a| a == "--settings");
    let link = daemon_link::connect();
    let sender = link.as_ref().map(daemon_link::sender);

    gpui_kit::application().with_assets(assets::Assets).run(move |cx: &mut App| {
        materiant::init(cx);
        // Nền + bảng màu đã lưu, font nhúng (D-20); gốc cửa sổ đặt `.font_family(cx.theme().font_family())`.
        appearance::apply(theme, &theme_preset, cx);
        devtool::init(cx);

        let model = cx.new(|_| Model::new(sender));
        let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // Như Groovex: không thanh tiêu đề OS trên mọi nền, thanh trên + ba nút cửa sổ tự vẽ (`views::app_bar`). Tên
            // cửa sổ (taskbar, Alt+Tab) đặt bằng `set_window_title` ở lần vẽ đầu.
            titlebar: None,
            window_decorations: Some(WindowDecorations::Client),
            window_min_size: Some(size(px(WINDOW_MIN_SIZE.0), px(WINDOW_MIN_SIZE.1))),
            app_id: Some("ferry".into()),
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
            if open_settings {
                main.update(cx, |main, cx| main.open_settings(window, cx));
            }
        });
        // Một cửa sổ: đóng là thoát tiến trình (daemon vẫn chạy).
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
        if let Some(link) = link {
            daemon_link::listen(link, model, handle, main.downgrade(), cx);
        }
    });
    log::info!("ferry-ui stopped");
}
