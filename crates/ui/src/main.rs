//! Cửa sổ `mws-ui` (GPUI + materiant). Chỉ vẽ trạng thái daemon `mws` gửi qua IPC và gửi lệnh ngược lại — không mạng,
//! không kho khoá, không SQLite (`CLAUDE.md` mục Bố cục). Đóng cửa sổ ⇒ tiến trình thoát, nhả GPU / RAM; daemon vẫn chạy.
//!
//! gd1_01: cửa sổ trống. Màn chính (vùng thả + hàng đợi) là gd1_08.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod daemon_link;

use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    self, App, AppContext, Bounds, Context, IntoElement, Render, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, px, size,
};
use materiant::theme::ActiveTheme;
use mws_core::i18n::I18n;
use mws_core::{VERSION, config, logging, paths};

/// Cỡ mở đầu của cửa sổ chính, px logic.
const WINDOW_SIZE: (f32, f32) = (720., 520.);
const WINDOW_MIN_SIZE: (f32, f32) = (480., 360.);

fn main() {
    let log_file = logging::init("ui");
    log::info!("mws-ui {VERSION} starting (pid {}, log: {log_file:?})", std::process::id());

    // Chỉ đọc: daemon là bên ghi `config.json`.
    let language = paths::config_file()
        .and_then(|p| config::load(&p))
        .map_err(|e| log::warn!("config: {e}; using defaults"))
        .unwrap_or_default()
        .language
        .resolve();
    let i18n = I18n::new(language);
    let link = daemon_link::connect();

    gpui_kit::application().with_assets(materiant::icon::Assets).run(move |cx: &mut App| {
        materiant::init(cx);
        devtool::init(cx);

        let title: SharedString = i18n.t("app.name").to_owned().into();
        let placeholder: SharedString = i18n.t("window.placeholder").to_owned().into();
        let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some(title.clone()), ..Default::default() }),
            window_min_size: Some(size(px(WINDOW_MIN_SIZE.0), px(WINDOW_MIN_SIZE.1))),
            app_id: Some("media-wireless-sync".into()),
            ..materiant::window::window_options()
        };
        let handle = match gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| MainView { placeholder })) {
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
            daemon_link::listen(link, handle, cx);
        }
    });
    log::info!("mws-ui stopped");
}

struct MainView {
    placeholder: SharedString,
}

impl Render for MainView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(colors.surface)
            .text_color(colors.muted)
            .child(self.placeholder.clone())
    }
}
