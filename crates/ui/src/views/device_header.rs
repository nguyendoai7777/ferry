//! Đầu cửa sổ (PROTOCOL 4.2): điện thoại đang chọn — chấm trạng thái + tên (nhiều máy ⇒ `Select`) — và nút
//! "Thêm điện thoại" (Cài đặt nằm ở đáy cột trái). Chưa ghép máy nào ⇒ chỉ nút bên phải. Máy đang chọn cần ghép lại / chưa
//! nối được ⇒ nút đổi thành "Kết nối lại": máy chỉ Offline ⇒ daemon thử nối + quét LAN ngay (`PhoneOp::Reconnect`, nút hiện
//! "Đang kết nối lại…"); máy cần ghép lại (`NeedsRepair`) ⇒ nút "Ghép lại" mở thẳng bước PIN.

use std::rc::Rc;

use ferry_core::device::DeviceState;
use ferry_core::i18n::I18n;
use ferry_core::ipc::DeviceView;
use materiant::button::{Button, Size};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{App, IntoElement, SharedString, Window, div, px};
use materiant::icon::{Icon, IconSource};
use materiant::select::Select;
use materiant::theme::{ActiveTheme, Neutral};

use super::devices::needs_reconnect;

/// Nút "Kết nối lại" của máy đang chọn.
pub struct Reconnect {
    /// Đang chờ kết quả lần bấm trước ⇒ nút xám "Đang kết nối lại…".
    pub busy: bool,
    pub on_click: Rc<dyn Fn(&mut Window, &mut App)>,
}

pub fn device_header(
    i18n: &I18n,
    paired: &[DeviceView],
    selected: Option<usize>,
    on_select: Rc<dyn Fn(usize, &mut Window, &mut App)>,
    on_add: Rc<dyn Fn(&mut Window, &mut App)>,
    reconnect: Reconnect,
    cx: &App,
) -> impl IntoElement {
    let c = cx.theme().colors().clone();
    let current = selected.and_then(|i| paired.get(i));
    let Reconnect { busy: reconnecting, on_click: on_reconnect } = reconnect;
    let reconnect = current.is_some_and(|d| needs_reconnect(d.state));
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
        .child(if reconnect {
            let (label, busy) = match current.map(|d| d.state) {
                Some(DeviceState::NeedsRepair) => ("main.repair", false),
                _ if reconnecting => ("main.reconnecting", true),
                _ => ("main.reconnect", false),
            };
            Button::new("reconnect-phone", i18n.t(label))
                .size(Size::Sm)
                .tonal()
                .disabled(busy)
                .on_click(move |_, window, cx| on_reconnect(window, cx))
        } else {
            Button::new("add-phone", i18n.t("main.add_phone"))
                .size(Size::Sm)
                .tonal()
                .on_click(move |_, window, cx| on_add(window, cx))
        })
}

fn status(i18n: &I18n, state: DeviceState, color: materiant::gpui_kit::Hsla) -> impl IntoElement {
    let key = match state {
        DeviceState::Online => "main.phone_online",
        DeviceState::NeedsRepair => "main.phone_needs_repair",
        _ => "main.phone_offline",
    };
    div().text_xs().text_color(color).child(SharedString::from(i18n.t(key).to_owned()))
}
