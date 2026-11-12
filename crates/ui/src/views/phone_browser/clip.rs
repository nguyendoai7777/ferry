//! Cắt · Sao chép · Dán trên bộ nhớ điện thoại (W-9, PROTOCOL §6.13), như menu chuột phải của Explorer.
//!
//! Cắt / Sao chép chỉ ghi đường vào bảng nháp của [`Model`] (không gọi mạng). Dán gửi lần lượt `Copy` / `Move` từng mục
//! vào thư mục đích; đủ trả lời thì đọc lại thư mục đích (và thư mục nguồn khi dời). Trùng tên ở đích: điện thoại tự đặt
//! `tên (2)`. Dán xám khi bảng nháp trống / của máy khác, điện thoại chưa khai `copy`, đang offline hoặc đang dán.
//! Điện thoại khai `fetch` (W-10) ⇒ Cắt / Sao chép còn chuẩn bị để dán ở trình quản lý tệp của máy ([`super::export`]).

use std::collections::BTreeSet;

use ferry_core::browse::Location;
use ferry_core::ipc::{CommandError, LibraryOp, Outcome};
use materiant::gpui_kit::base::Disableable;
use materiant::gpui_kit::{App, Context, WeakEntity};
use materiant::menu::{Menu, MenuItem};

use super::PhoneBrowser;
use super::edit::{Target, error_key, inside};
use crate::model::{ClipEntry, StorageClip};

/// Một lượt dán nhiều mục.
#[derive(Debug)]
pub struct PasteRun {
    left: usize,
    failed: Vec<ClipEntry>,
    first_error: Option<CommandError>,
    cut: bool,
    dest: String,
    /// Mục đã chép / dời xong.
    done: Vec<ClipEntry>,
}

impl PhoneBrowser {
    /// Điện thoại khai `edit` + `copy` và đang duyệt bộ nhớ thật.
    pub(crate) fn copy_known(&self, cx: &App) -> bool {
        self.edit_known(cx) && self.model.read(cx).copies_storage(&self.device)
    }

    /// Bảng nháp của điện thoại này (bảng nháp của máy khác coi như trống).
    fn clip<'a>(&self, cx: &'a App) -> Option<&'a StorageClip> {
        self.model.read(cx).clip.as_ref().filter(|c| c.device == self.device && !c.entries.is_empty())
    }

    /// Đường đang nằm trong bảng nháp ở chế độ Cắt (bảng vẽ mờ như Explorer).
    pub(crate) fn cut_paths(&self, cx: &App) -> BTreeSet<String> {
        self.clip(cx).filter(|c| c.cut).map(|c| c.entries.iter().map(|e| e.path.clone()).collect()).unwrap_or_default()
    }

    pub(crate) fn can_paste(&self, cx: &App) -> bool {
        self.copy_known(cx) && !self.offline && self.pasting.is_none() && self.clip(cx).is_some()
    }

    /// "Cắt" + "Sao chép" cho các mục `targets`. Thư mục ảo (chỉ có phía PC) không chép được.
    pub(crate) fn clip_items(&self, menu: Menu, targets: Vec<Target>, weak: &WeakEntity<Self>, cx: &App) -> Menu {
        let none = targets.is_empty() || targets.iter().any(|t| t.is_virtual);
        let (cut_off, copy_off) = (none || !self.can_cut_out(cx), none || !self.can_copy_out(cx));
        let (w1, t1, w2, t2) = (weak.clone(), targets.clone(), weak.clone(), targets);
        menu.item(MenuItem::new("br-m-cut", self.i18n.t("browser.menu_cut").to_owned()).disabled(cut_off).on_select(
            move |_, cx| {
                let t = t1.clone();
                let _ = w1.update(cx, |this, cx| this.set_clip(t, true, cx));
            },
        ))
        .item(
            MenuItem::new("br-m-copy", self.i18n.t("browser.menu_copy").to_owned()).disabled(copy_off).on_select(
                move |_, cx| {
                    let t = t2.clone();
                    let _ = w2.update(cx, |this, cx| this.set_clip(t, false, cx));
                },
            ),
        )
    }

    /// "Dán" vào thư mục `dest`.
    pub(crate) fn paste_item(&self, menu: Menu, dest: &str, weak: &WeakEntity<Self>, cx: &App) -> Menu {
        let (w, dest) = (weak.clone(), dest.to_owned());
        menu.item(
            MenuItem::new("br-m-paste", self.i18n.t("browser.menu_paste").to_owned())
                .disabled(!self.can_paste(cx))
                .on_select(move |_, cx| {
                    let d = dest.clone();
                    let _ = w.update(cx, |this, cx| this.paste_into(d, cx));
                }),
        )
    }

    pub(crate) fn set_clip(&mut self, targets: Vec<Target>, cut: bool, cx: &mut Context<Self>) {
        self.start_export(&targets, cut, cx);
        let entries = targets.into_iter().map(|t| ClipEntry { path: t.path, folder: t.folder }).collect();
        let clip = StorageClip { device: self.device.clone(), entries, cut };
        self.model.update(cx, |m, _| m.clip = Some(clip));
        cx.notify();
    }

    pub(crate) fn paste_into(&mut self, dest: String, cx: &mut Context<Self>) {
        if !self.can_paste(cx) {
            return;
        }
        let Some(clip) = self.clip(cx).cloned() else { return };
        if clip.cut {
            let paths: Vec<String> = clip.entries.iter().map(|e| e.path.clone()).collect();
            self.drop_export_of(&paths, cx);
        }
        let mut run = PasteRun {
            left: 0,
            failed: Vec::new(),
            first_error: None,
            cut: clip.cut,
            dest: dest.clone(),
            done: Vec::new(),
        };
        for e in clip.entries {
            // Thư mục vào chính nó / cây con của nó: điện thoại cũng trả `400`, chặn sớm khỏi tốn một vòng mạng.
            if e.folder && inside(&dest, &e.path) {
                run.first_error.get_or_insert(CommandError::InvalidInput);
                run.failed.push(e);
                continue;
            }
            let (device, path, dir) = (self.device.clone(), e.path.clone(), dest.clone());
            let op =
                if clip.cut { LibraryOp::Move { device, path, dir } } else { LibraryOp::Copy { device, path, dir } };
            match self.model.update(cx, |m, _| m.library(op)) {
                Some(req) => {
                    self.edit_req.insert(req, super::edit::Pending::Paste(e));
                    run.left += 1;
                }
                None => {
                    run.first_error.get_or_insert(CommandError::Offline);
                    run.failed.push(e);
                }
            }
        }
        self.notice = None;
        self.pasting = Some(run);
        if self.pasting.as_ref().is_some_and(|r| r.left == 0) {
            return self.finish_paste(cx);
        }
        self.loading = true;
        cx.notify();
    }

    /// Trả lời của một mục đang dán.
    pub(crate) fn on_paste_reply(&mut self, entry: ClipEntry, outcome: &Outcome, cx: &mut Context<Self>) {
        let Some(run) = self.pasting.as_mut() else { return };
        run.left = run.left.saturating_sub(1);
        match outcome {
            Outcome::Edited { .. } => run.done.push(entry),
            Outcome::Failed { error } => {
                run.first_error.get_or_insert(error.clone());
                run.failed.push(entry);
            }
            _ => {
                run.first_error.get_or_insert(CommandError::Internal);
                run.failed.push(entry);
            }
        }
        if run.left == 0 {
            self.finish_paste(cx);
        }
    }

    fn finish_paste(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.pasting.take() else { return };
        self.loading = self.req.is_some() || !self.pending.is_empty() || self.deleting.is_some();
        if !run.failed.is_empty() {
            let n = run.failed.len().to_string();
            let why = self.i18n.t(error_key(run.first_error.as_ref().unwrap_or(&CommandError::Internal))).to_owned();
            self.notice = Some(self.i18n.t_args("browser.paste_failed", &[("n", &n), ("why", &why)]));
        }
        let mut refetch: BTreeSet<String> = BTreeSet::from([run.dest.clone()]);
        if run.cut {
            // Mục đã dời: bỏ khỏi bảng nháp (mục lỗi giữ lại để thử lại), thư mục dời đi rời bộ đệm như lúc xoá.
            let failed = run.failed.clone();
            self.model.update(cx, |m, _| {
                if let Some(c) = m.clip.as_mut() {
                    c.entries.retain(|e| failed.contains(e));
                    if c.entries.is_empty() {
                        m.clip = None;
                    }
                }
            });
            let moved: Vec<Target> = run.done.iter().map(Target::of_clip).collect();
            if let Some(fs) = self.storage.as_mut() {
                for t in moved.iter().filter(|t| t.folder) {
                    fs.remove(&t.path);
                }
            }
            self.leave_deleted(&moved, cx);
            refetch.extend(moved.iter().map(Target::parent));
        }
        for p in refetch {
            self.fetch_dir(p, cx);
        }
        self.publish_storage(cx);
        if let Location::Dir(p) = self.at.clone() {
            self.fetch_if_unloaded(&p, cx);
        }
        self.recompute(cx);
    }
}

impl Target {
    fn of_clip(e: &ClipEntry) -> Target {
        let name = e.path.rsplit('/').next().unwrap_or(&e.path).to_owned();
        Target { path: e.path.clone(), name, folder: e.folder, is_virtual: false }
    }
}
