//! Nút icon "lặng" kiểu thanh trên của app Claude desktop: ô vuông bo 6 px (không tròn), icon `muted`; bật thì rê ⇒ icon
//! màu chữ + nền xám nhạt, nhấn nhạt hơn, ripple từ tâm; tắt thì icon mờ, không phản hồi. Dùng cho nút Lùi của thanh trên,
//! Lùi / Tiến của trình duyệt điện thoại và nút mở / gập của cây thư mục — mỗi chỗ chọn cỡ ô + cỡ icon vừa khung của nó.

use materiant::effect::ripple::{RippleConfig, RippleExt};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{AnyElement, App, ClickEvent, ElementId, IntoElement, SharedString, Window, div, px, svg};
use materiant::icon::IconName;
use materiant::overlay::tooltip::TooltipExt;
use materiant::theme::ActiveTheme;

/// Bo góc của nút (bằng bo mặc định của nút có nền).
const RADIUS: f32 = 6.;

/// Cỡ ô + cỡ icon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuietSize {
    pub edge: f32,
    pub icon: f32,
}

/// Thanh trên cửa sổ, thanh công cụ của trình duyệt điện thoại.
pub const BAR: QuietSize = QuietSize { edge: 24., icon: 16. };
/// Nút mở / gập trong dòng cây thư mục.
pub const TREE: QuietSize = QuietSize { edge: 20., icon: 14. };

pub fn quiet_icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    size: QuietSize,
    enabled: bool,
    tooltip: Option<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let id: ElementId = id.into();
    let group: SharedString = format!("{id}").into();
    let (text, muted) = (cx.theme().colors().text, cx.theme().colors().muted);
    let hover_group = group.clone();
    let tip_id = ElementId::NamedChild(std::sync::Arc::new(id.clone()), "tip".into());
    let button = div()
        .id(id)
        .group(group)
        .occlude()
        .flex_none()
        .size(px(size.edge))
        // Trước `.ripple`: ripple chép bo góc để cắt vòng.
        .rounded(px(RADIUS))
        .flex()
        .items_center()
        .justify_center()
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(move |s| s.bg(text.opacity(0.09)))
                .active(move |s| s.bg(text.opacity(0.06)))
                .ripple(RippleConfig::centered().color(text))
                .on_click(on_click)
        })
        .child(
            svg()
                .path(icon.path())
                .size(px(size.icon))
                .flex_none()
                .text_color(if enabled { muted } else { muted.opacity(0.4) })
                .when(enabled, |s| s.group_hover(hover_group, move |s| s.text_color(text))),
        );
    match tooltip {
        Some(tip) => button.with_tooltip(tip_id, tip).into_any_element(),
        None => button.into_any_element(),
    }
}
