//! Thanh trên của trình duyệt: Lùi / Tiến / Lên, breadcrumb (bấm từng đoạn để nhảy), làm mới, ô tìm.

use ferry_core::browse::{self, Location};
use materiant::button::{Button, IconButton, Size};
use materiant::field::TextField;
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{Context, Entity, IntoElement, div, px};
use materiant::icon::{Icon, IconName};
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
    cx: &mut Context<PhoneBrowser>,
) -> materiant::gpui_kit::AnyElement {
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
        .child(
            IconButton::new("br-refresh", IconName::Refresh)
                .size(Size::Sm)
                .accessibility_label(t("browser.refresh"))
                .loading(this.loading)
                .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
        )
        .child(div().w(px(180.)).flex_none().child(search.clone()))
        .into_any_element()
}
