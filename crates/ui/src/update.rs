//! ferry-gd2_01: phía cửa sổ của tự cập nhật. Daemon soát / tải / cài (`ferry/crates/daemon/src/update.rs`); ở đây chỉ:
//!
//! - phím Cmd+D (macOS) / Ctrl+D ⇒ `UiToDaemon::Update { Install }` (nút ở cột trái, mục khay làm cùng việc);
//! - hộp "Có gì mới": bản mới bật lần đầu (sổ `<data_dir>/last-version`, crate `updater::notes`) ⇒ cửa sổ nhỏ hiện release
//!   note — của manifest lúc tự cập nhật (mọi bản xen giữa), không thì tệp `release-notes/v<version>.md` nhúng lúc build.
//!   Enter / Esc / nút OK đóng. Chạy thử: `FERRY_WHATS_NEW=1` (bản debug) mở hộp bất kể sổ.

use std::time::Instant;

use ferry_core::i18n::I18n;
use ferry_core::{VERSION, paths};
use materiant::button::Button;
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    self, App, AsyncApp, Bounds, Context, Entity, FocusHandle, KeyBinding, KeyDownEvent, Pixels, ScrollHandle,
    SharedString, Window, WindowBounds, WindowDecorations, WindowOptions, div, point, px, size,
};
// `actions!` trỏ crate `gpui`; bản không test chỉ có `materiant::gpui_kit`.
use materiant::gpui_kit as gpui;
use materiant::scroll::ScrollArea;
use materiant::theme::ActiveTheme;
use materiant::window::DragRegionExt;
use updater::ReleaseNote;
use updater::notes::{self, Block};

use crate::model::Model;
use crate::platform::window_controls;

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/release_note.rs"));
}

gpui_kit::actions!(ferry_update, [InstallUpdate]);

/// Chữ phím của lệnh cập nhật (cạnh nút ở cột trái).
pub fn shortcut_label() -> &'static str {
    if cfg!(target_os = "macos") { "⌘D" } else { "Ctrl+D" }
}

/// Phím Cmd / Ctrl+D: bản mới đang có ⇒ tải + cài (daemon quyết mở trang tải hay cài).
pub fn bind(model: Entity<Model>, cx: &mut App) {
    let key = if cfg!(target_os = "macos") { "cmd-d" } else { "ctrl-d" };
    cx.bind_keys([KeyBinding::new(key, InstallUpdate, None)]);
    cx.on_action(move |_: &InstallUpdate, cx| {
        model.update(cx, |m, _| m.install_update());
    });
}

const FORCE_VAR: &str = "FERRY_WHATS_NEW";
const WIDTH: f32 = 440.;
const HEIGHT: f32 = 480.;
const HEADER: f32 = 36.;
const PAD: f32 = 20.;

/// Gọi một lần sau khi cửa sổ chính mở: đọc / ghi sổ ở luồng nền, phải hiện thì mở hộp.
pub fn whats_new_on_start(i18n: I18n, cx: &mut App) {
    let forced = cfg!(debug_assertions) && std::env::var_os(FORCE_VAR).is_some_and(|v| v == "1");
    let task = cx.background_spawn(async move {
        if forced {
            let note = embedded::RELEASE_NOTE.unwrap_or("# Ferry\n\n## Mới\n\n- Hộp chạy thử (FERRY_WHATS_NEW=1).\n");
            return Some(vec![ReleaseNote { version: VERSION.into(), markdown: note.into() }]);
        }
        let data = paths::data_dir().ok()?;
        notes::on_start(&data, &data.join("update"), VERSION, embedded::RELEASE_NOTE)
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        if let Some(notes) = task.await {
            log::info!("whats new: showing {} note(s) for {VERSION}", notes.len());
            cx.update(|cx| open(&notes, &i18n, cx));
        }
    })
    .detach();
}

fn open(notes: &[ReleaseNote], i18n: &I18n, cx: &mut App) {
    let started = Instant::now();
    let blocks = notes::blocks(notes);
    let title: SharedString = i18n.t("whats_new.title").to_owned().into();
    let subtitle: SharedString = i18n.t_args("whats_new.version", &[("version", VERSION)]).into();
    let ok: SharedString = i18n.t("whats_new.ok").to_owned().into();
    let Some(display) = cx.primary_display() else {
        log::warn!("whats new: no display");
        return;
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(centered(display.visible_bounds()))),
        titlebar: window_controls::titlebar(&title, HEADER),
        window_decorations: Some(WindowDecorations::Client),
        is_resizable: false,
        app_id: Some(crate::APP_ID.into()),
        ..materiant::window::window_options()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        window.set_window_title(&title);
        cx.new(|cx| WhatsNew::new(blocks, title.clone(), subtitle, ok, started, window, cx))
    });
    if let Err(e) = opened {
        log::error!("whats new: cannot open window: {e:#}");
    }
}

fn centered(area: Bounds<Pixels>) -> Bounds<Pixels> {
    let w = px(WIDTH).min(area.size.width);
    let h = px(HEIGHT).min(area.size.height);
    Bounds {
        origin: point(area.origin.x + (area.size.width - w) / 2.0, area.origin.y + (area.size.height - h) / 2.0),
        size: size(w, h),
    }
}

struct WhatsNew {
    blocks: Vec<Block>,
    title: SharedString,
    subtitle: SharedString,
    ok: SharedString,
    focus: FocusHandle,
    scroll: ScrollHandle,
}

impl WhatsNew {
    fn new(
        blocks: Vec<Block>,
        title: SharedString,
        subtitle: SharedString,
        ok: SharedString,
        started: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        window.on_next_frame(move |_, _| {
            log::info!("whats new: first frame in {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
        });
        WhatsNew { blocks, title, subtitle, ok, focus, scroll: ScrollHandle::new() }
    }

    fn close(how: &str, window: &mut Window) {
        log::info!("whats new: closed by {how}");
        window.remove_window();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.modified() {
            return;
        }
        match keystroke.key.as_str() {
            "enter" | "escape" => Self::close(&keystroke.key, window),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for WhatsNew {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors().clone();
        let mut body = div().w_full().flex().flex_col().gap(px(6.)).px(px(PAD)).pb(px(PAD));
        for block in &self.blocks {
            body = match block {
                // Nhiều bản: tiêu đề từng bản.
                Block::Title(text) => {
                    body.child(div().pt(px(14.)).text_lg().text_color(c.text).child(SharedString::from(text.clone())))
                }
                Block::Heading(text) => {
                    body.child(div().pt(px(10.)).text_color(c.primary).child(SharedString::from(text.clone())))
                }
                Block::Bullet(text) => body.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(div().flex_none().text_color(c.muted).child("•"))
                        .child(div().flex_1().min_w_0().child(SharedString::from(text.clone()))),
                ),
                Block::Paragraph(text) => body.child(div().child(SharedString::from(text.clone()))),
            };
        }
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .text_color(c.text)
            .font_family(cx.theme().font_family())
            .text_size(cx.theme().font_size())
            // Thanh kéo: macOS chứa đèn giao thông của hệ thống, nơi khác chỉ để kéo cửa sổ không viền.
            .child(div().id("whats-new-drag").flex_none().w_full().h(px(HEADER)).drag_region())
            .child(
                div()
                    .flex_none()
                    .px(px(PAD))
                    .pb(px(12.))
                    .child(div().text_xl().child(self.title.clone()))
                    .child(div().text_size(cx.theme().caption_size()).text_color(c.muted).child(self.subtitle.clone())),
            )
            .child(
                ScrollArea::new("whats-new-body").vertical().track_scroll(&self.scroll).flex_1().min_h_0().child(body),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .justify_end()
                    .px(px(PAD))
                    .py(px(12.))
                    .border_t_1()
                    .border_color(c.border)
                    .child(
                        Button::new("whats-new-ok", self.ok.clone())
                            .on_click(|_, window, _| Self::close("button", window)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use ferry_core::i18n::Language;

    use super::*;

    #[test]
    fn keys_are_translated() {
        for lang in [Language::Vi, Language::En] {
            let i18n = I18n::new(lang);
            for key in [
                "whats_new.title",
                "whats_new.ok",
                "update.nav",
                "update.downloading",
                "update.installing",
                "update.failed",
                "update.about",
                "update.check",
                "update.checking",
                "update.up_to_date",
                "update.check_failed",
                "update.available",
                "menu.check_updates",
            ] {
                assert_ne!(i18n.t(key), key, "missing `{key}` for {lang:?}");
            }
            assert!(i18n.t_args("whats_new.version", &[("version", "1.2.3")]).contains("1.2.3"));
        }
    }

    #[test]
    fn this_build_has_its_release_note_or_none() {
        if let Some(note) = embedded::RELEASE_NOTE {
            assert!(!notes::parse(note).is_empty());
        }
    }
}
