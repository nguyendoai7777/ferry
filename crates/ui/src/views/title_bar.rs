//! Thanh tiêu đề tự vẽ (CSD) cho cửa sổ của ferry. GNOME Wayland không cấp viền hệ thống (SSD) ⇒ GPUI rơi về
//! `Decorations::Client` không có thanh tiêu đề: không kéo được, không có nút đóng. Thanh này chỉ hiện khi đúng như
//! vậy; Windows / macOS có viền hệ thống thì [`title_bar`] trả `None`.

use materiant::button::{IconButton, Size};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{App, Decorations, Div, SharedString, Window, WindowDecorations, div, px};
use materiant::icon::IconName;
use materiant::style::Accent;
use materiant::theme::ActiveTheme;
use materiant::window::DragRegionExt;

/// Cao thanh — khớp nút `IconButton` cỡ `Sm` + đệm dọc.
pub const HEIGHT: f32 = 36.;

/// Viền cửa sổ xin khi mở: Linux xin CSD (GNOME không có SSD), nền khác để mặc định của hệ thống.
pub fn requested_decorations() -> Option<WindowDecorations> {
    cfg!(target_os = "linux").then_some(WindowDecorations::Client)
}

/// `id`: tiền tố id phần tử (mỗi cửa sổ một id). `close_label`: nhãn trợ năng của nút đóng.
pub fn title_bar(
    id: &'static str,
    title: impl Into<SharedString>,
    close_label: impl Into<SharedString>,
    window: &Window,
    cx: &App,
) -> Option<Div> {
    if !matches!(window.window_decorations(), Decorations::Client { .. }) {
        return None;
    }
    let theme = cx.theme();
    let (border, title_weight, font_size) = (theme.colors().border, theme.title_weight(), theme.font_size());
    Some(
        div()
            .w_full()
            .h(px(HEIGHT))
            .text_size(font_size)
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .pl_4()
            .pr_1()
            .border_b_1()
            .border_color(border)
            .drag_region()
            .child(div().truncate().font_weight(title_weight).child(title.into()))
            .child(
                div().occlude().child(
                    IconButton::new(id, IconName::CloseThin)
                        .size(Size::Sm)
                        .base_hover(Accent::Danger)
                        .accessibility_label(close_label.into())
                        .on_click(|_, window, _| window.remove_window()),
                ),
            ),
    )
}
