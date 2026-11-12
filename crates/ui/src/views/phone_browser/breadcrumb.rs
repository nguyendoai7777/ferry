//! Thanh trên của trình duyệt: Lùi / Tiến / Lên, breadcrumb (bấm từng đoạn để nhảy), làm mới, ô tìm.

use ferry_core::browse::{self, Location};
use materiant::button::{Button, IconButton, Size};
use materiant::field::TextField;
use materiant::gpui_kit::prelude::*;
use std::time::Duration;

use materiant::effect::ripple::{RippleConfig, RippleExt};
use materiant::gpui_kit::base::Button as BaseButton;
use materiant::gpui_kit::{
    Animation, AnimationExt, Context, ElementId, Entity, IntoElement, Transformation, Window, div, percentage, px, svg,
};
use materiant::icon::{Icon, IconName};
use materiant::style::ActiveTokens;
use materiant::theme::ActiveTheme;

use super::PhoneBrowser;

/// Đoạn breadcrumb: nhãn + nơi nhảy tới. Gốc = tên điện thoại; "Khác" là một đoạn riêng.
pub fn segments(device_name: &str, other_label: &str, at: &Location) -> Vec<(String, Location)> {
    let mut out = vec![(device_name.to_owned(), Location::root())];
    match at {
        Location::Other => out.push((other_label.to_owned(), Location::Other)),
        Location::Dir(p) => out.extend(browse::breadcrumb(p).into_iter().map(|(n, full)| (n, Location::Dir(full)))),
    }
    out
}

pub fn toolbar(
    this: &PhoneBrowser,
    search: &Entity<TextField>,
    window: &mut Window,
    cx: &mut Context<PhoneBrowser>,
) -> materiant::gpui_kit::AnyElement {
    let refresh = refresh_button(this, t_refresh(this), window, cx);
    let c = cx.theme().colors().clone();
    let t = |k: &str| this.i18n.t(k).to_owned();
    let segs = segments(&this.device_name, &t("browser.other"), &this.at);
    let last = segs.len() - 1;
    let mut crumbs = div().flex_1().min_w_0().flex().items_center().gap_1().overflow_hidden();
    for (ix, (label, loc)) in segs.into_iter().enumerate() {
        if ix > 0 {
            crumbs = crumbs.child(Icon::new(IconName::ChevronRight).size(px(14.)).color(c.muted));
        }
        crumbs = crumbs.child(
            Button::new(materiant::gpui_kit::ElementId::Name(format!("crumb-{ix}").into()), label)
                .size(Size::Sm)
                .text()
                .base()
                .selected(ix == last)
                .on_click(cx.listener(move |this, _, _, cx| this.go(loc.clone(), cx))),
        );
    }
    div()
        .flex()
        .items_center()
        .gap_1()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(c.border)
        .child(
            IconButton::new("br-back", IconName::ChevronLeft)
                .size(Size::Sm)
                .accessibility_label(t("browser.back"))
                .disabled(this.back.is_empty())
                .on_click(cx.listener(|this, _, _, cx| this.go_back(cx))),
        )
        .child(
            IconButton::new("br-fwd", IconName::ChevronRight)
                .size(Size::Sm)
                .accessibility_label(t("browser.forward"))
                .disabled(this.forward.is_empty())
                .on_click(cx.listener(|this, _, _, cx| this.go_forward(cx))),
        )
        .child(
            IconButton::new("br-up", IconName::ChevronUp)
                .size(Size::Sm)
                .accessibility_label(t("browser.up"))
                .disabled(this.at.parent().is_none())
                .on_click(cx.listener(|this, _, _, cx| {
                    if let Some(p) = this.at.parent() {
                        this.go(p, cx);
                    }
                })),
        )
        .child(crumbs)
        .child(refresh)
        .child(div().w(px(180.)).flex_none().child(search.clone()))
        .into_any_element()
}

fn t_refresh(this: &PhoneBrowser) -> materiant::gpui_kit::SharedString {
    this.i18n.t("browser.refresh").to_owned().into()
}

/// Cạnh nút làm mới (= `Size::Sm` của IconButton), icon, bo góc mặc định của nút có nền (6 px), một vòng quay.
const REFRESH_EDGE: f32 = 32.;
const REFRESH_ICON: f32 = 18.;
const REFRESH_RADIUS: f32 = 6.;
const REFRESH_TURN: Duration = Duration::from_millis(600);

/// Nút làm mới: ô vuông bo góc thường (không tròn), bấm thì icon quay đúng một vòng. Tự dựng vì `IconButton` dáng text
/// luôn tròn và chỉ có spinner lặp (`loading`). Mỗi lần bấm tăng `refresh_spins` ⇒ id hoạt ảnh mới ⇒ chạy lại từ đầu.
fn refresh_button(
    this: &PhoneBrowser,
    label: materiant::gpui_kit::SharedString,
    window: &mut Window,
    cx: &mut Context<PhoneBrowser>,
) -> materiant::gpui_kit::AnyElement {
    let c = cx.theme().colors().clone();
    let focus = window.use_keyed_state("br-refresh-focus", cx, |_, cx| cx.focus_handle()).read(cx).clone();
    let ring = materiant::style::focus_visible(&focus, window, cx);
    let icon = svg().path(IconName::Refresh.path()).flex_none().size(px(REFRESH_ICON)).text_color(c.muted);
    let spins = this.refresh_spins;
    let glyph = if spins == 0 || cx.reduce_motion() {
        icon.into_any_element()
    } else {
        icon.with_animation(
            ElementId::from(("br-refresh-spin", spins)),
            Animation::new(REFRESH_TURN).with_easing(materiant::gpui_kit::ease_in_out),
            |icon, delta| icon.with_transformation(Transformation::rotate(percentage(delta))),
        )
        .into_any_element()
    };
    BaseButton::new("br-refresh")
        .track_focus(&focus)
        .accessibility_label(label)
        .relative()
        .flex_none()
        .size(px(REFRESH_EDGE))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(REFRESH_RADIUS))
        .cursor_pointer()
        .hover(|s| s.bg(c.text.opacity(0.09)))
        .active(|s| s.bg(c.text.opacity(0.12)))
        .on_click(cx.listener(|this, _, _, cx| {
            this.refresh_spins += 1;
            this.refresh(cx);
        }))
        .child(
            div()
                .id("br-refresh-ripple")
                .absolute()
                .inset_0()
                .rounded(px(REFRESH_RADIUS))
                .ripple(RippleConfig::default()),
        )
        .child(glyph)
        .when(ring, |d| d.child(materiant::style::ring_overlay(cx.focus_ring_color(), px(REFRESH_RADIUS))))
        .into_any_element()
}
