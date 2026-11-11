//! Đầu cửa sổ (PROTOCOL 4.2): điện thoại đang chọn — chấm trạng thái + tên (nhiều máy ⇒ `Select`) — và nút
//! "Thêm điện thoại" + nút cài đặt (mở cửa sổ cài đặt riêng, gd1_12). Chưa ghép máy nào ⇒ chỉ hai nút bên phải.

use std::rc::Rc;

use ferry_core::device::DeviceState;
use ferry_core::i18n::I18n;
use ferry_core::ipc::DeviceView;
use materiant::button::{Button, IconButton, Size};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{App, IntoElement, SharedString, Window, div, px};
use materiant::icon::{Icon, IconName, IconSource};
use materiant::select::Select;
use materiant::theme::{ActiveTheme, Neutral};

pub fn device_header(
    i18n: &I18n,
    paired: &[DeviceView],
    selected: Option<usize>,
    on_select: Rc<dyn Fn(usize, &mut Window, &mut App)>,
    on_add: Rc<dyn Fn(&mut Window, &mut App)>,
    cx: &App,
) -> impl IntoElement {
    let c = cx.theme().colors().clone();
    let current = selected.and_then(|i| paired.get(i));
    let dot = |state: DeviceState| match state {
        DeviceState::Online => c.success,
        DeviceState::NeedsRepair => c.warning,
        _ => c.neutral(Neutral::N400),
    };
    let left = match (paired.len(), current) {
        (0, _) | (_, None) => div().into_any_element(),
        (1, Some(d)) => div()
            .flex()
            .items_center()
            .gap_2()
            .child(Icon::new(IconSource::dot(dot(d.state))).size(px(16.)))
            .child(div().font_weight(cx.theme().title_weight()).child(SharedString::from(d.name.clone())))
            .child(status(i18n, d.state, c.muted))
            .into_any_element(),
        (_, Some(d)) => {
            let names: Vec<SharedString> = paired.iter().map(|p| SharedString::from(p.name.clone())).collect();
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(Icon::new(IconSource::dot(dot(d.state))).size(px(16.)))
                .child(
                    Select::new("device")
                        .items(names)
                        .selected(selected)
                        .on_change(move |ix: &usize, window, cx| on_select(*ix, window, cx))
                        .w(px(220.)),
                )
                .child(status(i18n, d.state, c.muted))
                .into_any_element()
        }
    };
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .px_4()
        .py_2()
        .border_b_1()
        .border_color(c.border)
        .child(left)
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(
                    Button::new("add-phone", i18n.t("main.add_phone"))
                        .size(Size::Sm)
                        .tonal()
                        .on_click(move |_, window, cx| on_add(window, cx)),
                )
                .child(
                    IconButton::new("open-settings", IconName::Settings)
                        .size(Size::Sm)
                        .accessibility_label(SharedString::from(i18n.t("main.settings").to_owned()))
                        .on_click(|_, _, cx| crate::views::settings::open(cx)),
                ),
        )
}

fn status(i18n: &I18n, state: DeviceState, color: materiant::gpui_kit::Hsla) -> impl IntoElement {
    let key = match state {
        DeviceState::Online => "main.phone_online",
        DeviceState::NeedsRepair => "main.phone_needs_repair",
        _ => "main.phone_offline",
    };
    div().text_xs().text_color(color).child(SharedString::from(i18n.t(key).to_owned()))
}
