//! Cây thư mục bên trái (gốc = nơi lưu của điện thoại, nhóm "Khác" cuối). Phẳng hoá theo trạng thái mở rồi vẽ bằng
//! `VirtualList` (D-17: thư mục điện thoại có thể tới hàng trăm).

use std::collections::HashSet;

use ferry_core::browse::{DirNode, Location, Tree};
use materiant::button::{IconButton, Size};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{Context, ElementId, IntoElement, SharedString, div, px};
use materiant::icon::{Icon, IconName};
use materiant::list::VirtualList;
use materiant::theme::ActiveTheme;

use super::PhoneBrowser;
use crate::assets;

pub const ROW_HEIGHT: f32 = 28.;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub loc: Location,
    pub label: String,
    pub depth: usize,
    pub has_children: bool,
    pub open: bool,
    pub is_virtual: bool,
}

/// Dòng hiển thị của cây: gốc, rồi các thư mục theo trạng thái mở, rồi "Khác". Gốc luôn mở.
pub fn flatten(tree: &Tree, open: &HashSet<String>, root_label: &str, other_label: &str) -> Vec<TreeRow> {
    fn walk(nodes: &[DirNode], open: &HashSet<String>, depth: usize, out: &mut Vec<TreeRow>) {
        for n in nodes {
            let is_open = open.contains(&n.path);
            out.push(TreeRow {
                loc: Location::Dir(n.path.clone()),
                label: n.name.clone(),
                depth,
                has_children: !n.children.is_empty(),
                open: is_open,
                is_virtual: n.is_virtual,
            });
            if is_open {
                walk(&n.children, open, depth + 1, out);
            }
        }
    }
    let mut out = vec![TreeRow {
        loc: Location::root(),
        label: root_label.to_owned(),
        depth: 0,
        has_children: !tree.roots.is_empty(),
        open: true,
        is_virtual: false,
    }];
    walk(&tree.roots, open, 1, &mut out);
    if tree.has_other {
        out.push(TreeRow {
            loc: Location::Other,
            label: other_label.to_owned(),
            depth: 0,
            has_children: false,
            open: false,
            is_virtual: false,
        });
    }
    out
}

pub fn tree_view(
    this: &PhoneBrowser,
    rows: Vec<TreeRow>,
    cx: &mut Context<PhoneBrowser>,
) -> materiant::gpui_kit::AnyElement {
    let selected = rows.iter().position(|r| r.loc == this.at);
    let weak = cx.weak_entity();
    let click_rows = rows.clone();
    let count = rows.len();
    div()
        .w(px(210.))
        .flex_none()
        .h_full()
        .border_r_1()
        .border_color(cx.theme().colors().border)
        .child(
            VirtualList::new("br-tree", count, px(ROW_HEIGHT), move |ix, _, cx| {
                let r = &rows[ix];
                let c = cx.theme().colors().clone();
                let toggle = weak.clone();
                let path = match &r.loc {
                    Location::Dir(p) => p.clone(),
                    Location::Other => String::new(),
                };
                let chevron = if r.has_children && !path.is_empty() {
                    IconButton::new(
                        ElementId::Name(format!("tree-t-{ix}").into()),
                        if r.open { IconName::ChevronDown } else { IconName::ChevronRight },
                    )
                    .size(Size::Sm)
                    .accessibility_label(r.label.clone())
                    .on_click(move |_, _, cx| {
                        let path = path.clone();
                        let _ = toggle.update(cx, |this, cx| this.toggle_tree(&path, cx));
                    })
                    .into_any_element()
                } else {
                    div().w(px(24.)).flex_none().into_any_element()
                };
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl(px(4. + 12. * r.depth as f32))
                    .child(chevron)
                    .child(Icon::new(assets::folder()).size(px(16.)).color(c.muted))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .when(r.is_virtual, |d| d.italic())
                            .child(SharedString::from(r.label.clone())),
                    )
            })
            .selected(selected)
            .on_select(cx.listener(move |this, ix: &usize, _, cx| {
                if let Some(r) = click_rows.get(*ix) {
                    this.go(r.loc.clone(), cx);
                }
            }))
            .size_full(),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use ferry_core::browse::build_tree;
    use ferry_core::protocol::LibraryItem;

    use super::*;

    fn item(dir: Option<&str>) -> LibraryItem {
        LibraryItem {
            id: "i".into(),
            name: "a.mp4".into(),
            size: 1,
            modified: 1,
            mime: String::new(),
            dir: dir.map(Into::into),
        }
    }

    #[test]
    fn flatten_follows_open_state() {
        let tree = build_tree(&[item(Some("A/B")), item(Some("C")), item(None)], &[]);
        let closed = flatten(&tree, &HashSet::new(), "Pixel", "Khác");
        let labels: Vec<_> = closed.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Pixel", "A", "C", "Khác"]);
        let open: HashSet<String> = ["A".to_owned()].into();
        let rows = flatten(&tree, &open, "Pixel", "Khác");
        let labels: Vec<_> = rows.iter().map(|r| (r.label.as_str(), r.depth)).collect();
        assert_eq!(labels, [("Pixel", 0), ("A", 1), ("B", 2), ("C", 1), ("Khác", 0)]);
        assert!(rows[1].has_children && rows[1].open);
        assert!(!rows[4].has_children);
    }
}
