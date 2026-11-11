//! Khay chờ gửi (PROTOCOL 4.2): danh sách ngắn tệp / thư mục đã chọn, bỏ được từng mục, và nút "Gửi sang điện thoại…".

use std::path::PathBuf;
use std::rc::Rc;

use ferry_core::i18n::I18n;
use materiant::button::{Button, IconButton, Size};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{App, ElementId, IntoElement, SharedString, Window, div, px};
use materiant::icon::{Icon, IconName};
use materiant::scroll::{ScrollArea, TRACK_THICKNESS};
use materiant::theme::{ActiveTheme, Neutral};

use crate::assets;
use crate::fmt;

/// Số dòng khay vẽ tối đa; phần còn lại gộp "… và N mục nữa".
pub const SHOWN: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayEntry {
    pub path: PathBuf,
    pub is_dir: bool,
    /// Cỡ tệp; thư mục để 0 (không quét đĩa ở luồng UI).
    pub size: u64,
}

/// Thêm đường dẫn vào khay, bỏ trùng; trả số mục mới.
pub fn add_paths(tray: &mut Vec<TrayEntry>, paths: &[PathBuf]) -> usize {
    let mut added = 0;
    for p in paths {
        if tray.iter().any(|e| &e.path == p) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(p) else {
            log::warn!("tray: cannot stat {p:?}");
            continue;
        };
        tray.push(TrayEntry {
            path: p.clone(),
            is_dir: meta.is_dir(),
            size: if meta.is_dir() { 0 } else { meta.len() },
        });
        added += 1;
    }
    added
}

/// "3 tệp, 1 thư mục" (bỏ vế bằng 0).
pub fn summary(i18n: &I18n, tray: &[TrayEntry]) -> String {
    let files = tray.iter().filter(|e| !e.is_dir).count() as u64;
    let dirs = tray.len() as u64 - files;
    let mut parts = Vec::new();
    if files > 0 {
        parts.push(fmt::plural(i18n, "main.tray_files", files, &[("count", &files.to_string())]));
    }
    if dirs > 0 {
        parts.push(fmt::plural(i18n, "main.tray_folders", dirs, &[("count", &dirs.to_string())]));
    }
    parts.join(", ")
}

/// Tổng cỡ khi biết đủ (khay chỉ có tệp); có thư mục ⇒ `None` (không đoán).
pub fn known_size(tray: &[TrayEntry]) -> Option<u64> {
    (!tray.is_empty() && tray.iter().all(|e| !e.is_dir)).then(|| tray.iter().map(|e| e.size).sum())
}

#[allow(clippy::too_many_arguments)]
pub fn send_tray(
    i18n: &I18n,
    tray: &[TrayEntry],
    can_send: bool,
    on_remove: Rc<dyn Fn(usize, &mut Window, &mut App)>,
    on_clear: Rc<dyn Fn(&mut Window, &mut App)>,
    on_send: Rc<dyn Fn(&mut Window, &mut App)>,
    cx: &App,
) -> impl IntoElement {
    let c = cx.theme().colors().clone();
    let lang = i18n.language();
    let mut head = summary(i18n, tray);
    if let Some(total) = known_size(tray) {
        head = format!("{head} ({})", fmt::size(total, lang));
    }
    let rows = tray.iter().take(SHOWN).enumerate().map(|(ix, e)| {
        let remove = on_remove.clone();
        let name =
            e.path.file_name().map_or_else(|| e.path.display().to_string(), |n| n.to_string_lossy().into_owned());
        div()
            .h(px(28.))
            .flex()
            .items_center()
            .gap_2()
            .child(Icon::new(if e.is_dir { assets::folder() } else { assets::video() }).size(px(16.)).color(c.muted))
            .child(div().flex_1().min_w_0().truncate().text_sm().child(SharedString::from(name)))
            .when(!e.is_dir, |d| {
                d.child(div().text_xs().text_color(c.muted).child(SharedString::from(fmt::size(e.size, lang))))
            })
            .child(
                IconButton::new(ElementId::Name(format!("tray-x-{ix}").into()), IconName::CloseThin)
                    .size(Size::Sm)
                    .accessibility_label(i18n.t("main.tray_remove"))
                    .on_click(move |_, window, cx| remove(ix, window, cx)),
            )
    });
    let more = (tray.len() > SHOWN).then(|| {
        div().h(px(24.)).text_xs().text_color(c.muted).child(SharedString::from(fmt::plural(
            i18n,
            "main.tray_more",
            (tray.len() - SHOWN) as u64,
            &[("count", &(tray.len() - SHOWN).to_string())],
        )))
    });
    div()
        .mx_4()
        .mb_3()
        .rounded_lg()
        .border_1()
        .border_color(c.border)
        .bg(c.layer(Neutral::N50))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .px_3()
                .py_2()
                .child(
                    div()
                        .font_weight(cx.theme().label_weight())
                        .child(SharedString::from(i18n.t_args("main.tray_summary", &[("parts", &head)]))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("tray-clear", i18n.t("main.tray_clear"))
                                .size(Size::Sm)
                                .text()
                                .on_click(move |_, window, cx| on_clear(window, cx)),
                        )
                        .child(
                            Button::new("tray-send", i18n.t("main.send"))
                                .size(Size::Sm)
                                .disabled(!can_send)
                                .on_click(move |_, window, cx| on_send(window, cx)),
                        ),
                ),
        )
        .child(
            ScrollArea::new("tray-scroll")
                .vertical()
                .max_h(px(112.))
                .padding_x(px(12.).max(TRACK_THICKNESS))
                .child(div().flex().flex_col().children(rows).children(more)),
        )
}

#[cfg(test)]
mod tests {
    use ferry_core::i18n::Language;

    use super::*;

    fn entry(name: &str, is_dir: bool, size: u64) -> TrayEntry {
        TrayEntry { path: name.into(), is_dir, size }
    }

    /// C-W4b (phần khay) — chọn 3 tệp + 1 thư mục ⇒ khay đúng.
    #[test]
    fn summary_counts_files_and_folders() {
        let tray = [entry("a", false, 1), entry("b", false, 2), entry("c", false, 3), entry("d", true, 0)];
        assert_eq!(summary(&I18n::new(Language::Vi), &tray), "3 tệp, 1 thư mục");
        assert_eq!(summary(&I18n::new(Language::En), &tray), "3 files, 1 folder");
        assert_eq!(summary(&I18n::new(Language::En), &tray[..1]), "1 file");
        assert_eq!(known_size(&tray), None);
        assert_eq!(known_size(&tray[..3]), Some(6));
        assert_eq!(known_size(&[]), None);
    }

    #[test]
    fn add_paths_dedups_and_skips_missing() {
        let dir = std::env::temp_dir();
        let mut tray = Vec::new();
        let a = dir.clone();
        assert_eq!(add_paths(&mut tray, &[a.clone(), a, dir.join("ferry-no-such-path-xyz")]), 1);
        assert!(tray[0].is_dir);
    }
}
