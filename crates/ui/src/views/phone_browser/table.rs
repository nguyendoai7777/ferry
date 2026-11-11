//! Bảng chi tiết bên phải: Tên · Ngày sửa · Loại · Cỡ. Thư mục trước, bấm tiêu đề cột để sắp, nhấp đúp thư mục để vào,
//! chọn nhiều bằng Ctrl/Cmd + click, Shift + click. Dòng vẽ bằng `VirtualList` (D-17).

use std::collections::BTreeSet;

use ferry_core::browse::{Entry, EntryKind, SortKey};
use ferry_core::i18n::I18n;
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{Context, IntoElement, MouseButton, SharedString, div, px};
use materiant::icon::{Icon, IconName};
use materiant::list::VirtualList;
use materiant::theme::{ActiveTheme, Neutral};

use super::PhoneBrowser;
use crate::{assets, fmt};

pub const ROW_HEIGHT: f32 = 32.;
const MODIFIED_W: f32 = 112.;
const KIND_W: f32 = 84.;
const SIZE_W: f32 = 72.;

/// Kết quả chọn sau một cú click (hàm thuần, có test).
pub fn select_click(
    selected: &BTreeSet<usize>,
    anchor: Option<usize>,
    ix: usize,
    ctrl: bool,
    shift: bool,
) -> (BTreeSet<usize>, Option<usize>) {
    if shift && let Some(a) = anchor {
        let (lo, hi) = (a.min(ix), a.max(ix));
        let mut set = if ctrl { selected.clone() } else { BTreeSet::new() };
        set.extend(lo..=hi);
        return (set, Some(a));
    }
    if ctrl {
        let mut set = selected.clone();
        if !set.remove(&ix) {
            set.insert(ix);
        }
        return (set, Some(ix));
    }
    (BTreeSet::from([ix]), Some(ix))
}

fn kind_text(i18n: &I18n, e: &Entry) -> String {
    match e.kind {
        EntryKind::Folder => i18n.t("browser.kind_folder").to_owned(),
        EntryKind::Video if e.ext.is_empty() => i18n.t("browser.kind_video_plain").to_owned(),
        EntryKind::Video => i18n.t_args("browser.kind_video", &[("ext", &e.ext)]),
        EntryKind::File if e.ext.is_empty() => i18n.t("browser.kind_file_plain").to_owned(),
        EntryKind::File => i18n.t_args("browser.kind_file", &[("ext", &e.ext)]),
    }
}

fn header_cell(
    this: &PhoneBrowser,
    key: SortKey,
    label_key: &str,
    cx: &mut Context<PhoneBrowser>,
) -> materiant::gpui_kit::Stateful<materiant::gpui_kit::Div> {
    let c = cx.theme().colors().clone();
    let active = this.sort.0 == key;
    div()
        .id(materiant::gpui_kit::ElementId::Name(format!("th-{key:?}").into()))
        .flex()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .text_xs()
        .font_weight(cx.theme().label_weight())
        .text_color(if active { c.text } else { c.muted })
        .child(SharedString::from(this.i18n.t(label_key).to_owned()))
        .when(active, |d| {
            d.child(Icon::new(if this.sort.1 { IconName::ChevronUp } else { IconName::ChevronDown }).size(px(12.)))
        })
        .on_click(cx.listener(move |this, _, _, cx| this.sort_by(key, cx)))
}

/// Trần bề ngang của dòng báo khi bảng trống — chữ dài xuống dòng thay vì tràn mép.
const EMPTY_TEXT_MAX_W: f32 = 380.;

pub fn table_view(this: &PhoneBrowser, cx: &mut Context<PhoneBrowser>) -> materiant::gpui_kit::AnyElement {
    let c = cx.theme().colors().clone();
    let entries = std::rc::Rc::new(this.entries.clone());
    let selected = this.selected.clone();
    let i18n = this.i18n.clone();
    let lang = i18n.language();
    let now = ferry_core::queue::now_ms();
    let dim = this.dim();
    let weak = cx.weak_entity();
    let weak_select = weak.clone();
    let count = entries.len();

    let header = div()
        .flex()
        .items_center()
        .gap_2()
        .px_4()
        .h(px(28.))
        .border_b_1()
        .border_color(c.border)
        .child(div().flex_1().min_w_0().child(header_cell(this, SortKey::Name, "browser.col_name", cx)))
        .child(div().w(px(MODIFIED_W)).child(header_cell(this, SortKey::Modified, "browser.col_modified", cx)))
        .child(div().w(px(KIND_W)).child(header_cell(this, SortKey::Kind, "browser.col_kind", cx)))
        .child(div().w(px(SIZE_W)).flex().justify_end().child(header_cell(
            this,
            SortKey::Size,
            "browser.col_size",
            cx,
        )));

    let body =
        if count == 0 {
            let msg = if this.loading {
                "browser.loading"
            } else if !this.has_data() && this.offline {
                "browser.offline_empty"
            } else if this.query.is_empty() && this.is_library() {
                "browser.empty_library"
            } else if this.query.is_empty() {
                "browser.empty"
            } else {
                "browser.no_match"
            };
            // Chữ xuống dòng trong khối có trần bề ngang (con trực tiếp của hàng flex không tự xuống dòng ⇒ tràn mép).
            div()
                .flex_1()
                .min_h(px(0.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .px_6()
                .child(
                    div()
                        .w_full()
                        .max_w(px(EMPTY_TEXT_MAX_W))
                        .text_center()
                        .text_sm()
                        .text_color(c.muted)
                        .child(SharedString::from(i18n.t(msg).to_owned())),
                )
                .into_any_element()
        } else {
            let sel_rows = selected.clone();
            let entries_for_rows = entries.clone();
            let entries_for_activate = entries.clone();
            div()
                .flex_1()
                .min_h(px(0.))
                .opacity(if dim { 0.6 } else { 1. })
                .child(
                    VirtualList::new("br-table", count, px(ROW_HEIGHT), move |ix, _, cx| {
                        let e = &entries_for_rows[ix];
                        let c = cx.theme().colors().clone();
                        let is_sel = sel_rows.contains(&ix);
                        let click = weak.clone();
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(is_sel, |d| d.bg(c.primary.opacity(0.14)))
                            .on_mouse_down(MouseButton::Left, move |ev, _, cx| {
                                let (ctrl, shift) = (ev.modifiers.secondary(cx), ev.modifiers.shift);
                                let _ = click.update(cx, |this, cx| this.click_row(ix, ctrl, shift, cx));
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Icon::new(match e.kind {
                                            EntryKind::Folder => assets::folder(),
                                            EntryKind::Video => assets::video(),
                                            EntryKind::File => assets::file(),
                                        })
                                        .size(px(16.))
                                        .color(c.muted),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .when(e.is_virtual, |d| d.italic())
                                            .child(SharedString::from(e.name.clone())),
                                    ),
                            )
                            .child(
                                div().w(px(MODIFIED_W)).truncate().text_xs().text_color(c.muted).child(
                                    SharedString::from(e.modified.map(|m| fmt::date(m, now)).unwrap_or_default()),
                                ),
                            )
                            .child(
                                div()
                                    .w(px(KIND_W))
                                    .truncate()
                                    .text_xs()
                                    .text_color(c.muted)
                                    .child(SharedString::from(kind_text(&i18n, e))),
                            )
                            .child(div().w(px(SIZE_W)).flex().justify_end().text_xs().text_color(c.muted).child(
                                SharedString::from(if e.kind != EntryKind::Folder {
                                    fmt::size(e.size, lang)
                                } else {
                                    String::new()
                                }),
                            ))
                    })
                    .track(&this.list)
                    .selected(this.anchor)
                    .padding_x(px(16.))
                    .on_select(cx.listener(|this, ix: &usize, _, cx| this.key_select(*ix, cx)))
                    .on_activate(cx.listener(move |this, ix: &usize, _, cx| {
                        if let Some(e) = entries_for_activate.get(*ix) {
                            this.activate(e.clone(), cx);
                        }
                    }))
                    .size_full(),
                )
                .into_any_element()
        };
    let _ = weak_select;
    // Chưa có danh sách (đang tải, chưa nối được, thư mục trống): không vẽ đầu bảng rỗng, chỉ dòng báo giữa vùng.
    let show_header = count > 0 || (this.has_data() && !this.query.is_empty());
    div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .bg(c.layer(Neutral::N50).opacity(0.0))
        .when(show_header, |d| d.child(header))
        .child(body)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(v: &[usize]) -> BTreeSet<usize> {
        v.iter().copied().collect()
    }

    #[test]
    fn plain_ctrl_and_shift_clicks() {
        let (s, a) = select_click(&set(&[]), None, 2, false, false);
        assert_eq!((s, a), (set(&[2]), Some(2)));
        let (s, a) = select_click(&set(&[2]), Some(2), 5, true, false);
        assert_eq!((s, a), (set(&[2, 5]), Some(5)));
        let (s, a) = select_click(&set(&[2, 5]), Some(5), 2, true, false);
        assert_eq!((s, a), (set(&[5]), Some(2)));
        let (s, a) = select_click(&set(&[2]), Some(2), 4, false, true);
        assert_eq!((s, a), (set(&[2, 3, 4]), Some(2)));
        let (s, a) = select_click(&set(&[0, 2, 3]), Some(2), 5, true, true);
        assert_eq!((s, a), (set(&[0, 2, 3, 4, 5]), Some(2)));
        // Shift khi chưa có mốc ⇒ như click thường.
        let (s, a) = select_click(&set(&[]), None, 3, false, true);
        assert_eq!((s, a), (set(&[3]), Some(3)));
    }
}
