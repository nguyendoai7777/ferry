//! Vùng thả lớn + hai nút "Chọn tệp…" / "Chọn thư mục…" (PROTOCOL 4.2). Thả / dán chỉ đưa vào khay chờ, chưa gửi.

use std::rc::Rc;

use ferry_core::i18n::I18n;
use materiant::button::{Button, Size};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{App, ExternalPaths, IntoElement, SharedString, Window, div, px};
use materiant::theme::{ActiveTheme, Neutral};

/// Phím dán theo OS để ghi vào gợi ý.
pub fn paste_keys() -> &'static str {
    if cfg!(target_os = "macos") { "Cmd+V" } else { "Ctrl+V" }
}

pub fn drop_zone(
    i18n: &I18n,
    enabled: bool,
    on_pick_files: Rc<dyn Fn(&mut Window, &mut App)>,
    on_pick_folders: Rc<dyn Fn(&mut Window, &mut App)>,
    on_drop: Rc<dyn Fn(&ExternalPaths, &mut Window, &mut App)>,
    cx: &App,
) -> impl IntoElement {
    let c = cx.theme().colors().clone();
    let accent = c.primary;
    div()
        .id("drop-zone")
        .mx_4()
        .my_3()
        .py_5()
        .flex()
        .flex_col()
        .items_center()
        .gap_3()
        .rounded_lg()
        .border_2()
        .border_dashed()
        .border_color(c.border)
        .bg(c.layer(Neutral::N50))
        .when(enabled, |d| {
            d.drag_over::<ExternalPaths>(move |style, _, _, _| style.border_color(accent).bg(accent.opacity(0.08)))
                .on_drop(move |paths: &ExternalPaths, window, cx| on_drop(paths, window, cx))
        })
        .child(div().font_weight(cx.theme().title_weight()).text_color(c.text).child(SharedString::from(
            i18n.t("main.drop_title").to_owned(),
        )))
        .child(div().text_xs().text_color(c.muted).child(SharedString::from(
            i18n.t_args("main.drop_hint", &[("keys", paste_keys())]),
        )))
        .child(
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("pick-files", i18n.t("main.pick_files"))
                        .size(Size::Sm)
                        .outlined()
                        .disabled(!enabled)
                        .on_click(move |_, window, cx| on_pick_files(window, cx)),
                )
                .child(
                    Button::new("pick-folders", i18n.t("main.pick_folders"))
                        .size(Size::Sm)
                        .outlined()
                        .disabled(!enabled)
                        .on_click(move |_, window, cx| on_pick_folders(window, cx)),
                ),
        )
        .min_h(px(0.))
}
