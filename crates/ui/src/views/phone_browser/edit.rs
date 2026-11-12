//! Sửa bộ nhớ điện thoại (W-8, PROTOCOL §4.6, §6.12): menu chuột phải theo loại mục, hộp nhập tên (thư mục mới / đổi
//! tên), hộp xác nhận xoá, và xử lý trả lời của daemon.
//!
//! Menu như Explorer (Cắt / Sao chép / Dán ở `clip.rs`, W-9):
//! - Tệp: Cắt · Sao chép │ Đổi tên · Xoá. Chọn nhiều + chuột phải lên mục đang chọn ⇒ Cắt · Sao chép │ Xoá N mục.
//! - Thư mục (bảng / cây): Mở │ Cắt · Sao chép · Dán (vào nó) │ Thư mục mới │ Đổi tên · Xoá. Gốc cây: Dán │ Thư mục mới.
//!   "Khác": không menu.
//! - Khoảng trống của bảng: Làm mới │ Dán │ Thư mục mới.
//! - Điện thoại không khai `edit` (hoặc đang dựng từ `list`): thư mục mới là thư mục ảo phía PC như trước; đổi tên / xoá
//!   chỉ cho thư mục ảo, mục thật vẫn hiện nhưng xám. Offline: mục sửa thật bị tắt.

use std::collections::BTreeSet;

use ferry_core::browse::{self, Entry, EntryKind, Location};
use ferry_core::ipc::{CommandError, LibraryOp, Outcome};
use materiant::button::Button;
use materiant::gpui_kit::base::Disableable;
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{App, ClickEvent, Context, Focusable, SharedString, WeakEntity, Window, div, px};
use materiant::icon::IconName;
use materiant::menu::{Menu, MenuItem};
use materiant::overlay::dialog::Dialog;
use materiant::theme::ActiveTheme;

use super::PhoneBrowser;
use crate::model::ClipEntry;

/// Một mục bị sửa (đường đầy đủ từ gốc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: String,
    pub name: String,
    pub folder: bool,
    pub is_virtual: bool,
}

impl Target {
    pub fn of(e: &Entry) -> Target {
        let folder = e.kind == EntryKind::Folder;
        Target {
            path: if folder { e.path.clone() } else { browse::join_dir(&e.path, &e.name) },
            name: e.name.clone(),
            folder,
            is_virtual: e.is_virtual,
        }
    }

    pub(super) fn parent(&self) -> String {
        browse::parent_dir(&self.path).to_owned()
    }
}

/// Hộp thoại đang mở.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// `enter`: tạo xong thì vào thư mục mới (nút "Thư mục mới" của chế độ chọn đích).
    NewFolder {
        parent: String,
        enter: bool,
    },
    Rename(Target),
    Delete(Vec<Target>),
}

/// Lệnh đã gửi, chờ điện thoại trả lời.
#[derive(Debug, Clone)]
pub enum Pending {
    NewFolder {
        parent: String,
        enter: bool,
    },
    Rename(Target),
    Delete(Target),
    /// Một mục của lượt Dán (W-9).
    Paste(ClipEntry),
    /// Cắt / Sao chép sang máy tính vừa gửi (W-10): chỉ cần biết lỗi bắt đầu.
    Export,
}

/// Một lượt xoá nhiều mục: đợi đủ trả lời mới đọc lại các thư mục cha.
#[derive(Debug, Default)]
pub struct DeleteRun {
    left: usize,
    failed: usize,
    first_error: Option<CommandError>,
    deleted: Vec<Target>,
}

/// `path` nằm trong (hoặc là) thư mục `dir`.
pub(super) fn inside(path: &str, dir: &str) -> bool {
    path == dir || path.starts_with(&format!("{dir}/"))
}

/// Đổi tiền tố `old` của `path` thành `new` (đổi tên thư mục).
fn moved(path: &str, old: &str, new: &str) -> Option<String> {
    if path == old {
        Some(new.to_owned())
    } else {
        path.strip_prefix(&format!("{old}/")).map(|rest| format!("{new}/{rest}"))
    }
}

/// Tên mục người dùng gõ: như tên thư mục. Tên bắt đầu bằng `.` hợp lệ — tạo / đổi thành mục ẩn (W-11, §6.12).
pub fn sanitize_name(name: &str) -> Option<String> {
    browse::sanitize_folder_name(name)
}

pub(super) fn error_key(e: &CommandError) -> &'static str {
    match e {
        CommandError::Exists => "browser.err_exists",
        CommandError::Missing => "browser.err_missing",
        CommandError::Forbidden => "browser.err_forbidden",
        CommandError::InvalidInput => "browser.err_name_bad",
        CommandError::Offline | CommandError::Unreachable => "browser.err_offline",
        _ => "browser.err_failed",
    }
}

impl PhoneBrowser {
    /// Điện thoại khai `edit` (lần đọc `info` cuối) và đang duyệt bộ nhớ thật.
    pub(crate) fn edit_known(&self, cx: &App) -> bool {
        self.browses_storage() && self.model.read(cx).edits_storage(&self.device)
    }

    fn label(&self, key: &str) -> SharedString {
        self.i18n.t(key).to_owned().into()
    }

    // ---- menu ----

    /// Menu của một dòng bảng.
    pub(crate) fn row_menu(&self, ix: usize, menu: Menu, weak: &WeakEntity<Self>, cx: &App) -> Menu {
        let Some(e) = self.entries.get(ix) else { return menu };
        let real = self.edit_known(cx);
        let online = !self.offline;
        if self.selected.len() > 1 && self.selected.contains(&ix) {
            let targets: Vec<Target> =
                self.selected.iter().filter_map(|&i| self.entries.get(i)).map(Target::of).collect();
            let n = targets.len().to_string();
            let all_virtual = targets.iter().all(|t| t.is_virtual);
            let off = !(all_virtual || real) || (!online && !all_virtual);
            let w = weak.clone();
            let del = targets.clone();
            return self.clip_items(menu, targets, weak, cx).separator().item(
                MenuItem::new("br-m-delete-n", self.i18n.t_args("browser.menu_delete_n", &[("n", &n)]))
                    .danger(true)
                    .disabled(off)
                    .on_select(move |window, cx| {
                        let t = del.clone();
                        let _ = w.update(cx, |this, cx| this.start_delete(t, window, cx));
                    }),
            );
        }
        let target = Target::of(e);
        let mut menu = menu;
        if target.folder {
            let (w, path) = (weak.clone(), target.path.clone());
            menu = menu
                .item(MenuItem::new("br-m-open", self.label("browser.menu_open")).on_select(move |_, cx| {
                    let p = path.clone();
                    let _ = w.update(cx, |this, cx| this.go(Location::Dir(p), cx));
                }))
                .separator();
        }
        menu = self.clip_items(menu, vec![target.clone()], weak, cx);
        if target.folder {
            menu = self.paste_item(menu, &target.path, weak, cx).separator();
            menu = self.new_folder_item(menu, &target.path, weak, real && !online);
        }
        self.rename_delete_items(menu, &target, weak, real, online)
    }

    /// Menu của khoảng trống trong bảng.
    pub(crate) fn blank_menu(&self, menu: Menu, weak: &WeakEntity<Self>, cx: &App) -> Menu {
        let w = weak.clone();
        let menu = menu.item(
            MenuItem::new("br-m-refresh", self.label("browser.menu_refresh")).icon(IconName::Refresh).on_select(
                move |_, cx| {
                    let _ = w.update(cx, |this, cx| this.refresh(cx));
                },
            ),
        );
        let Location::Dir(p) = &self.at else { return menu };
        let menu = self.paste_item(menu.separator(), p, weak, cx).separator();
        self.new_folder_item(menu, p, weak, self.edit_known(cx) && self.offline)
    }

    /// Menu của một dòng cây.
    pub(crate) fn tree_menu(
        &self,
        loc: &Location,
        label: &str,
        is_virtual: bool,
        menu: Menu,
        weak: &WeakEntity<Self>,
        cx: &App,
    ) -> Menu {
        let Location::Dir(path) = loc else { return menu };
        let real = self.edit_known(cx);
        let online = !self.offline;
        if path.is_empty() {
            let menu = self.paste_item(menu, "", weak, cx).separator();
            return self.new_folder_item(menu, "", weak, real && !online);
        }
        let target = Target { path: path.clone(), name: label.to_owned(), folder: true, is_virtual };
        let (w, p) = (weak.clone(), path.clone());
        let menu = menu
            .item(MenuItem::new("br-m-open", self.label("browser.menu_open")).on_select(move |_, cx| {
                let p = p.clone();
                let _ = w.update(cx, |this, cx| this.go(Location::Dir(p), cx));
            }))
            .separator();
        let menu = self.clip_items(menu, vec![target.clone()], weak, cx);
        let menu = self.paste_item(menu, path, weak, cx).separator();
        let menu = self.new_folder_item(menu, path, weak, real && !online);
        self.rename_delete_items(menu, &target, weak, real, online)
    }

    fn new_folder_item(&self, menu: Menu, parent: &str, weak: &WeakEntity<Self>, disabled: bool) -> Menu {
        let (w, parent) = (weak.clone(), parent.to_owned());
        menu.item(MenuItem::new("br-m-new-folder", self.label("browser.menu_new_folder")).disabled(disabled).on_select(
            move |window, cx| {
                let p = parent.clone();
                let _ = w.update(cx, |this, cx| this.start_new_folder(p, false, window, cx));
            },
        ))
    }

    /// │ Đổi tên · Xoá. Mục thật khi điện thoại chưa khai `edit` (hoặc offline) vẫn hiện, xám — như Explorer, menu không
    /// đổi hình theo trạng thái.
    fn rename_delete_items(&self, menu: Menu, t: &Target, weak: &WeakEntity<Self>, real: bool, online: bool) -> Menu {
        let off = !t.is_virtual && !(real && online);
        let (w1, t1, w2, t2) = (weak.clone(), t.clone(), weak.clone(), t.clone());
        menu.separator()
            .item(MenuItem::new("br-m-rename", self.label("browser.menu_rename")).disabled(off).on_select(
                move |window, cx| {
                    let t = t1.clone();
                    let _ = w1.update(cx, |this, cx| this.start_rename(t, window, cx));
                },
            ))
            .item(MenuItem::new("br-m-delete", self.label("browser.menu_delete")).danger(true).disabled(off).on_select(
                move |window, cx| {
                    let t = t2.clone();
                    let _ = w2.update(cx, |this, cx| this.start_delete(vec![t], window, cx));
                },
            ))
    }

    /// Chuột phải lên một dòng chưa chọn ⇒ chọn riêng nó (như Explorer); dòng đang trong vùng chọn thì giữ.
    pub(crate) fn context_select(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !self.selected.contains(&ix) {
            self.selected = BTreeSet::from([ix]);
            self.anchor = Some(ix);
            cx.notify();
        }
    }

    // ---- mở hộp thoại ----

    fn open_name_dialog(&mut self, edit: Edit, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.edit = Some(edit);
        self.edit_busy = false;
        let v = value.to_owned();
        self.name_field.update(cx, |f, cx| {
            f.set_value(v, window, cx);
            f.set_error(None, cx);
        });
        self.edit_dialog.open(window, cx);
        window.focus(&self.name_field.focus_handle(cx), cx);
        cx.notify();
    }

    pub(crate) fn start_new_folder(
        &mut self,
        parent: String,
        enter: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_name_dialog(Edit::NewFolder { parent, enter }, "", window, cx);
    }

    fn start_rename(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let name = target.name.clone();
        self.open_name_dialog(Edit::Rename(target), &name, window, cx);
    }

    fn start_delete(&mut self, targets: Vec<Target>, window: &mut Window, cx: &mut Context<Self>) {
        if targets.is_empty() {
            return;
        }
        self.edit = Some(Edit::Delete(targets));
        self.edit_busy = false;
        self.edit_dialog.open(window, cx);
        cx.notify();
    }

    pub(crate) fn close_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit = None;
        self.edit_busy = false;
        self.edit_dialog.close(window, cx);
        cx.notify();
    }

    fn field_error(&mut self, key: &str, cx: &mut Context<Self>) {
        let msg = self.label(key);
        self.name_field.update(cx, |f, cx| f.set_error(Some(msg), cx));
        self.edit_busy = false;
        cx.notify();
    }

    /// Có mục cùng tên (không phân biệt hoa thường) trong thư mục `parent`, trừ chính `except`.
    fn sibling_exists(&self, parent: &str, name: &str, except: Option<&str>) -> bool {
        let want = browse::join_dir(parent, name).to_lowercase();
        let in_entries = |e: &Entry| {
            let t = Target::of(e);
            t.path.to_lowercase() == want && except.is_none_or(|x| x != t.path)
        };
        if self.at == Location::Dir(parent.to_owned()) {
            return self.entries.iter().any(in_entries);
        }
        let all = browse::list_entries(&self.items, &self.known, &self.virtual_dirs, &Location::Dir(parent.into()), "");
        all.iter().any(in_entries)
    }

    // ---- xác nhận ----

    pub(crate) fn confirm_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.edit_busy {
            return;
        }
        match self.edit.clone() {
            Some(Edit::NewFolder { parent, enter }) => self.confirm_new_folder(parent, enter, window, cx),
            Some(Edit::Rename(t)) => self.confirm_rename(t, window, cx),
            Some(Edit::Delete(ts)) => self.confirm_delete(ts, window, cx),
            None => {}
        }
    }

    fn typed_name(&self, cx: &App) -> Option<String> {
        sanitize_name(&self.name_field.read(cx).value(cx))
    }

    fn confirm_new_folder(&mut self, parent: String, enter: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.typed_name(cx) else { return self.field_error("browser.err_name_bad", cx) };
        if self.sibling_exists(&parent, &name, None) {
            return self.field_error("browser.err_exists", cx);
        }
        let full = browse::join_dir(&parent, &name);
        if self.edit_known(cx) {
            let op = LibraryOp::MakeDir { device: self.device.clone(), path: full };
            return self.send_edit(op, Pending::NewFolder { parent, enter }, cx);
        }
        // Không có `edit`: thư mục ảo phía PC tới khi có tệp gửi vào (§4.6).
        self.virtual_dirs.push(full.clone());
        self.tree_open.insert(parent);
        self.close_edit(window, cx);
        if enter {
            self.go(Location::Dir(full), cx);
        }
        self.recompute(cx);
    }

    fn confirm_rename(&mut self, t: Target, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.typed_name(cx) else { return self.field_error("browser.err_name_bad", cx) };
        if name == t.name {
            return self.close_edit(window, cx);
        }
        let parent = t.parent();
        if self.sibling_exists(&parent, &name, Some(&t.path)) {
            return self.field_error("browser.err_exists", cx);
        }
        if t.is_virtual {
            let new = browse::join_dir(&parent, &name);
            for d in &mut self.virtual_dirs {
                if let Some(m) = moved(d, &t.path, &new) {
                    *d = m;
                }
            }
            self.after_dir_moved(&t.path, &new, cx);
            self.close_edit(window, cx);
            return self.recompute(cx);
        }
        let op = LibraryOp::Rename { device: self.device.clone(), path: t.path.clone(), name };
        self.send_edit(op, Pending::Rename(t), cx);
    }

    fn confirm_delete(&mut self, targets: Vec<Target>, window: &mut Window, cx: &mut Context<Self>) {
        self.close_edit(window, cx);
        let (virt, real): (Vec<Target>, Vec<Target>) = targets.into_iter().partition(|t| t.is_virtual);
        for t in &virt {
            self.virtual_dirs.retain(|d| !inside(d, &t.path));
        }
        if !virt.is_empty() {
            self.leave_deleted(&virt, cx);
            self.recompute(cx);
        }
        if real.is_empty() {
            return;
        }
        let mut run = DeleteRun { left: 0, ..DeleteRun::default() };
        for t in real {
            let op = LibraryOp::Delete { device: self.device.clone(), path: t.path.clone() };
            match self.model.update(cx, |m, _| m.library(op)) {
                Some(req) => {
                    self.edit_req.insert(req, Pending::Delete(t));
                    run.left += 1;
                }
                None => {
                    run.failed += 1;
                    run.first_error.get_or_insert(CommandError::Offline);
                }
            }
        }
        self.notice = None;
        if run.left == 0 {
            self.deleting = Some(run);
            return self.finish_delete(cx);
        }
        self.loading = true;
        self.deleting = Some(run);
        cx.notify();
    }

    fn send_edit(&mut self, op: LibraryOp, pending: Pending, cx: &mut Context<Self>) {
        match self.model.update(cx, |m, _| m.library(op)) {
            Some(req) => {
                self.edit_req.insert(req, pending);
                self.edit_busy = true;
                cx.notify();
            }
            None => self.field_error("browser.err_failed", cx),
        }
    }

    // ---- trả lời ----

    /// Trả lời của một lệnh sửa; `false` nếu `req` không phải của nó.
    pub(crate) fn on_edit_reply(
        &mut self,
        req: u64,
        outcome: &Outcome,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pending) = self.edit_req.remove(&req) else { return false };
        let error = match outcome {
            Outcome::Failed { error } => Some(error.clone()),
            _ => None,
        };
        match pending {
            Pending::NewFolder { parent, enter } => match (outcome, error) {
                (Outcome::Edited { path }, _) => {
                    self.close_edit(window, cx);
                    self.tree_open.insert(parent.clone());
                    self.fetch_dir(parent, cx);
                    if enter {
                        self.go(Location::Dir(path.clone()), cx);
                    }
                }
                (_, Some(e)) => self.edit_failed(&e, window, cx),
                _ => self.edit_failed(&CommandError::Internal, window, cx),
            },
            Pending::Rename(t) => match (outcome, error) {
                (Outcome::Edited { path }, _) => {
                    self.close_edit(window, cx);
                    if t.folder {
                        if let Some(fs) = self.storage.as_mut() {
                            fs.remove(&t.path);
                        }
                        self.after_dir_moved(&t.path, path, cx);
                    }
                    self.fetch_dir(t.parent(), cx);
                    if let Location::Dir(p) = self.at.clone()
                        && t.folder
                        && inside(&p, path)
                    {
                        self.fetch_dir(p, cx);
                    }
                }
                (_, Some(e)) => self.edit_failed(&e, window, cx),
                _ => self.edit_failed(&CommandError::Internal, window, cx),
            },
            Pending::Paste(e) => self.on_paste_reply(e, outcome, cx),
            Pending::Export => self.on_export_reply(outcome, cx),
            Pending::Delete(t) => {
                let Some(run) = self.deleting.as_mut() else { return true };
                run.left = run.left.saturating_sub(1);
                match error {
                    // Đã không còn trên điện thoại = đúng ý người dùng.
                    None | Some(CommandError::Missing) => run.deleted.push(t),
                    Some(e) => {
                        run.failed += 1;
                        run.first_error.get_or_insert(e);
                    }
                }
                if run.left == 0 {
                    self.finish_delete(cx);
                }
            }
        }
        true
    }

    /// Lỗi của thư mục mới / đổi tên: tên sai / trùng thì giữ hộp thoại và báo ở ô; lỗi khác đóng hộp, báo ở băng trên.
    fn edit_failed(&mut self, e: &CommandError, window: &mut Window, cx: &mut Context<Self>) {
        match e {
            CommandError::Exists | CommandError::InvalidInput => self.field_error(error_key(e), cx),
            _ => {
                self.close_edit(window, cx);
                self.notice = Some(self.i18n.t(error_key(e)).to_owned());
                if let Location::Dir(p) = self.at.clone() {
                    self.fetch_dir(p, cx);
                }
            }
        }
    }

    fn finish_delete(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.deleting.take() else { return };
        self.loading = self.req.is_some() || !self.pending.is_empty();
        if let Some(fs) = self.storage.as_mut() {
            for t in run.deleted.iter().filter(|t| t.folder) {
                fs.remove(&t.path);
            }
        }
        if run.failed > 0 {
            let n = run.failed.to_string();
            let why = self.i18n.t(error_key(run.first_error.as_ref().unwrap_or(&CommandError::Internal))).to_owned();
            self.notice = Some(self.i18n.t_args("browser.delete_failed", &[("n", &n), ("why", &why)]));
        }
        self.leave_deleted(&run.deleted, cx);
        let parents: BTreeSet<String> = run.deleted.iter().map(Target::parent).collect();
        for p in parents {
            self.fetch_dir(p, cx);
        }
        self.publish_storage(cx);
        self.recompute(cx);
    }

    /// Bộ đệm thư mục vừa sửa tay ⇒ chép sang `Model` (trình duyệt mở lại thấy ngay) và dựng lại `items` / `known`.
    pub(super) fn publish_storage(&mut self, cx: &mut Context<Self>) {
        if let Some(fs) = &self.storage {
            let snapshot = fs.clone();
            let device = self.device.clone();
            self.model.update(cx, |m, _| {
                m.storage.insert(device, snapshot);
            });
        }
        self.sync_storage();
    }

    /// Đang đứng trong thư mục vừa xoá ⇒ về thư mục cha của nó (không ghi lịch sử).
    pub(super) fn leave_deleted(&mut self, deleted: &[Target], cx: &mut Context<Self>) {
        let Location::Dir(at) = self.at.clone() else { return };
        if let Some(t) = deleted.iter().find(|t| t.folder && inside(&at, &t.path)) {
            self.at = Location::Dir(t.parent());
            self.back.retain(|l| !matches!(l, Location::Dir(p) if inside(p, &t.path)));
            self.forward.clear();
            if let Location::Dir(p) = self.at.clone() {
                self.fetch_if_unloaded(&p, cx);
            }
        }
        self.tree_open.retain(|p| !deleted.iter().any(|t| t.folder && inside(p, &t.path)));
    }

    /// Thư mục `old` đổi thành `new`: vị trí đang mở, lịch sử, nhánh cây đang mở đi theo.
    fn after_dir_moved(&mut self, old: &str, new: &str, cx: &mut Context<Self>) {
        let fix = |l: &mut Location| {
            if let Location::Dir(p) = l
                && let Some(m) = moved(p, old, new)
            {
                *p = m;
            }
        };
        fix(&mut self.at);
        self.back.iter_mut().for_each(fix);
        self.forward.iter_mut().for_each(fix);
        self.tree_open = self.tree_open.iter().map(|p| moved(p, old, new).unwrap_or_else(|| p.clone())).collect();
        cx.notify();
    }

    // ---- vẽ ----

    pub(crate) fn render_edit_dialog(&self, cx: &mut Context<Self>) -> Dialog {
        let c = cx.theme().colors().clone();
        let cancel = Button::new("br-edit-cancel", self.label("browser.cancel"))
            .text()
            .base()
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.close_edit(window, cx)))
            .into_any_element();
        let dialog = Dialog::new("br-edit-dialog", &self.edit_dialog).max_width(px(420.));
        match &self.edit {
            Some(Edit::Delete(ts)) => {
                let title = match ts.as_slice() {
                    [one] => self.i18n.t_args("browser.delete_title_one", &[("name", &one.name)]),
                    many => self.i18n.t_args("browser.delete_title_n", &[("n", &many.len().to_string())]),
                };
                let body = if ts.iter().all(|t| t.is_virtual) {
                    self.i18n.t("browser.delete_body_virtual").to_owned()
                } else {
                    self.i18n.t_args("browser.delete_body", &[("phone", &self.device_name)])
                };
                dialog.title(title).child(div().text_color(c.muted).child(SharedString::from(body))).actions([
                    cancel,
                    Button::new("br-edit-delete", self.label("browser.delete_ok"))
                        .danger()
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.confirm_edit(window, cx)))
                        .into_any_element(),
                ])
            }
            edit => {
                let (title, ok) = match edit {
                    Some(Edit::Rename(_)) => ("browser.rename_title", "browser.rename_ok"),
                    _ => ("browser.new_folder", "browser.new_folder_ok"),
                };
                dialog
                    .title(self.label(title))
                    .child(div().flex().flex_col().gap_2().child(self.name_field.clone()))
                    .actions([
                        cancel,
                        Button::new("br-edit-ok", self.label(ok))
                            .disabled(self.edit_busy)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.confirm_edit(window, cx)))
                            .into_any_element(),
                    ])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_and_inside_follow_prefixes() {
        assert_eq!(moved("A/B", "A", "X").as_deref(), Some("X/B"));
        assert_eq!(moved("A", "A", "X").as_deref(), Some("X"));
        assert_eq!(moved("AB/C", "A", "X"), None);
        assert!(inside("A/B", "A") && inside("A", "A") && !inside("AB", "A"));
    }

    #[test]
    fn names_allow_hidden_reject_separators() {
        assert_eq!(sanitize_name("  Phim  ").as_deref(), Some("Phim"));
        assert_eq!(sanitize_name(".vdr").as_deref(), Some(".vdr"));
        assert_eq!(sanitize_name(".."), None);
        assert_eq!(sanitize_name("a/b"), None);
        assert_eq!(sanitize_name(""), None);
    }

    #[test]
    fn target_of_file_joins_dir_and_name() {
        let e = Entry {
            kind: EntryKind::File,
            name: "a.txt".into(),
            path: "Download".into(),
            modified: None,
            size: 1,
            ext: "TXT".into(),
            is_virtual: false,
            item: Some(0),
        };
        assert_eq!(Target::of(&e).path, "Download/a.txt");
        let d = Entry { kind: EntryKind::Folder, path: "Download/Phim".into(), name: "Phim".into(), ..e };
        assert_eq!(Target::of(&d).path, "Download/Phim");
    }
}
