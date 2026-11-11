//! Cửa sổ `ferry-ui` (GPUI + materiant). Chỉ vẽ trạng thái daemon `ferry` gửi qua IPC và gửi lệnh ngược lại — không mạng,
//! không kho khoá, không SQLite (`CLAUDE.md` mục Bố cục). Đóng cửa sổ ⇒ tiến trình thoát, nhả GPU / RAM; daemon vẫn chạy.
//!
//! gd1_08: màn chính (vùng thả + khay chờ + hàng đợi) và trình duyệt điện thoại; màn thêm điện thoại / cài đặt là gd1_09 / gd1_12.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod daemon_link;
mod fmt;
mod model;
mod settings_link;
mod ui_state;
mod views;

use std::rc::Rc;

use ferry_core::i18n::I18n;
use ferry_core::{VERSION, config, logging, paths};
use materiant::gpui_kit::{
    self, App, AppContext, Bounds, SharedString, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};
use model::Model;
use views::main_window::MainWindow;

/// Cỡ mở đầu của cửa sổ chính, px logic.
const WINDOW_SIZE: (f32, f32) = (860., 620.);
const WINDOW_MIN_SIZE: (f32, f32) = (640., 460.);

fn main() {
    let log_file = logging::init("ui");
    log::info!("ferry-ui {VERSION} starting (pid {}, log: {log_file:?})", std::process::id());

    // Chỉ đọc: daemon là bên ghi `config.json`.
    let language = paths::config_file()
        .and_then(|p| config::load(&p))
        .map_err(|e| log::warn!("config: {e}; using defaults"))
        .unwrap_or_default()
        .language
        .resolve();
    let i18n = I18n::new(language);
    let i18n = Rc::new(i18n);
    // `ferry-ui --settings`: daemon bật từ menu khay "Cài đặt…" (gd1_12) ⇒ chỉ mở cửa sổ cài đặt.
    let settings_only = std::env::args().skip(1).any(|a| a == "--settings");
    let link = daemon_link::connect();
    let sender = link.as_ref().map(daemon_link::sender);

    gpui_kit::application().with_assets(assets::Assets).run(move |cx: &mut App| {
        materiant::init(cx);
        // Font nhúng cho mọi cửa sổ (D-20); gốc mỗi cửa sổ đặt `.font_family(cx.theme().font_family())`.
        materiant::theme::set_theme(
            &materiant::theme::MateriantTheme {
                font_family: Some(materiant::font::MONA_SANS.into()),
                ..Default::default()
            },
            cx,
        );
        devtool::init(cx);

        if settings_only {
            views::settings::open(cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            if let Some(link) = link {
                // Kho của cửa sổ chính vẫn dựng để nhận ảnh chụp, không cửa sổ nào đọc nó.
                let model = cx.new(|_| Model::new(sender));
                daemon_link::listen(link, model, None, cx);
            }
            return;
        }
        let title: SharedString = i18n.t("app.name").to_owned().into();
        let model = cx.new(|_| Model::new(sender));
        let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some(title.clone()), ..Default::default() }),
            window_min_size: Some(size(px(WINDOW_MIN_SIZE.0), px(WINDOW_MIN_SIZE.1))),
            app_id: Some("ferry".into()),
            window_decorations: views::title_bar::requested_decorations(),
            ..materiant::window::window_options()
        };
        let handle = match gpui_kit::open_window(options, cx, |window, cx| {
            let (model, i18n) = (model.clone(), i18n.clone());
            cx.new(|cx| MainWindow::new(model, i18n, window, cx))
        }) {
            Ok((handle, _)) => handle,
            Err(err) => {
                // Bản phát hành không có console: log mới là chỗ đọc được lý do.
                log::error!("cannot open the main window: {err:#}");
                cx.quit();
                return;
            }
        };
        // Một cửa sổ: đóng là thoát tiến trình (daemon vẫn chạy).
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
        if let Some(link) = link {
            daemon_link::listen(link, model, Some(handle), cx);
        }
    });
    log::info!("ferry-ui stopped");
}
