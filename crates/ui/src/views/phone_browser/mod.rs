//! Trình duyệt điện thoại kiểu Explorer (PROTOCOL 4.6, W-4): một component cho hộp chọn đích khi gửi (gd1_08) và tab
//! thư viện (gd1_11, [`BrowserMode::Library`]). Dựng hoàn toàn từ `GET /list` qua IPC `Library::List`; cây và bảng là
//! hàm thuần trong `ferry_core::browse`.
//!
//! - Thanh trên: Lùi / Tiến / Lên, breadcrumb, làm mới, ô tìm ([`breadcrumb`]).
//! - Trái: cây thư mục ([`tree`]); phải: bảng chi tiết ([`table`]).
//! - Chế độ chọn đích: thanh dưới "Gửi N tệp vào: …" + [`BrowserEvent::Send`] / [`BrowserEvent::Cancel`]; "Thư mục
//!   mới" tạo thư mục ảo phía PC (in nghiêng) cho tới khi có tệp đầu tiên gửi vào.
//! - Điện thoại offline ⇒ hiện danh sách lần trước, mờ đi, không cho "Gửi vào đây".
//! - Điện thoại có `features: browse` (§6.11, W-7) ⇒ dựng từ `GET /dirs` thay vì `list`: mở thư mục nào đọc thư mục
//!   đó (cùng các thư mục cha khi mở sẵn ở đích lần trước), nhớ các lần đã đọc ([`FsCache`], giữ ở `Model::storage`).

mod breadcrumb;
mod table;
mod tree;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use ferry_core::browse::{self, Entry, EntryKind, FsCache, KnownDir, Location, SortKey};
use ferry_core::device::{DeviceId, DeviceState};
use ferry_core::i18n::I18n;
use ferry_core::ipc::{CommandError, LibraryOp, Outcome};
use ferry_core::protocol::LibraryItem;
use materiant::button::{Button, Size};
use materiant::field::{FieldEvent, TextField};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    Context, Entity, EventEmitter, Focusable, IntoElement, Render, SharedString, Subscription, Window, div, px,
};
use materiant::icon::IconName;
use materiant::list::VirtualListHandle;
use materiant::theme::ActiveTheme;

use crate::fmt;
use crate::model::{Model, ModelEvent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserMode {
    /// Chọn thư mục đích cho khay chờ: `what` = "3 tệp, 1 thư mục", `bytes` = tổng cỡ khi biết.
    Destination { what: String, bytes: Option<u64> },
    /// Thư viện điện thoại (gd1_11 thêm nút "Tải về máy").
    Library,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserEvent {
    /// "Gửi vào đây": `dest_dir` rỗng = gốc nơi lưu.
    Send {
        dest_dir: String,
    },
    Cancel,
}

pub struct PhoneBrowser {
    model: Entity<Model>,
    pub(crate) i18n: Rc<I18n>,
    device: DeviceId,
    pub(crate) device_name: String,
    mode: BrowserMode,
    pub(crate) items: Vec<LibraryItem>,
    /// `Some` khi điện thoại cho duyệt bộ nhớ (W-7): `items` + `known` dựng lại từ đây sau mỗi lần đọc.
    storage: Option<FsCache>,
    known: Vec<KnownDir>,
    /// `req` của các lần `Dirs` đang chờ → thư mục.
    pending: HashMap<u64, String>,
    pub(crate) loading: bool,
    failed: bool,
    pub(crate) offline: bool,
    req: Option<u64>,
    virtual_dirs: Vec<String>,
    pub(crate) at: Location,
    pub(crate) back: Vec<Location>,
    pub(crate) forward: Vec<Location>,
    pub(crate) sort: (SortKey, bool),
    search: Entity<TextField>,
    pub(crate) query: String,
    clear_search: bool,
    pub(crate) entries: Vec<Entry>,
    pub(crate) selected: BTreeSet<usize>,
    pub(crate) anchor: Option<usize>,
    suppress_select: bool,
    pub(crate) list: VirtualListHandle,
    tree_open: HashSet<String>,
    creating: bool,
    folder_field: Entity<TextField>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<BrowserEvent> for PhoneBrowser {}

impl PhoneBrowser {
    pub fn new(
        model: Entity<Model>,
        i18n: Rc<I18n>,
        device: DeviceId,
        mode: BrowserMode,
        start_dir: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_hint = i18n.t("browser.search").to_owned();
        let search = cx.new(|cx| {
            TextField::new("br-search", window, cx)
                .placeholder(search_hint)
                .leading_icon(IconName::Search)
                .clearable(true)
        });
        let folder_hint = i18n.t("browser.new_folder_name").to_owned();
        let folder_field = cx.new(|cx| TextField::new("br-folder", window, cx).placeholder(folder_hint));
        let mut subs = vec![
            cx.subscribe(&search, |this, _, ev: &FieldEvent, cx| {
                if let FieldEvent::Change(q) = ev {
                    this.query = q.to_string();
                    this.recompute(cx);
                }
            }),
            cx.subscribe(&folder_field, |this, _, ev: &FieldEvent, cx| {
                if let FieldEvent::Submit(name) = ev {
                    this.create_folder(name.to_string(), cx);
                }
            }),
        ];
        subs.push(cx.subscribe(&model, |this, _, ev: &ModelEvent, cx| this.on_model(ev, cx)));

        let (offline, name, cached, storage) = {
            let m = model.read(cx);
            let dev = m.snapshot.devices.iter().find(|d| d.id == device);
            let storage = m.browses_storage(&device).then(|| m.storage.get(&device).cloned().unwrap_or_default());
            (
                dev.is_none_or(|d| d.state != DeviceState::Online),
                dev.map(|d| d.name.clone()).unwrap_or_default(),
                m.libraries.get(&device).cloned().unwrap_or_default(),
                storage,
            )
        };
        let mut tree_open = HashSet::new();
        for (_, full) in browse::breadcrumb(&start_dir) {
            tree_open.insert(full);
        }
        let mut this = PhoneBrowser {
            model,
            i18n,
            device,
            device_name: name,
            mode,
            items: if storage.is_some() { Vec::new() } else { cached },
            storage,
            known: Vec::new(),
            pending: HashMap::new(),
            loading: false,
            failed: false,
            offline,
            req: None,
            virtual_dirs: Vec::new(),
            at: Location::Dir(start_dir),
            back: Vec::new(),
            forward: Vec::new(),
            sort: (SortKey::Name, true),
            search,
            query: String::new(),
            clear_search: false,
            entries: Vec::new(),
            selected: BTreeSet::new(),
            anchor: None,
            suppress_select: false,
            list: VirtualListHandle::new(),
            tree_open,
            creating: false,
            folder_field,
            _subs: subs,
        };
        this.sync_storage();
        this.ensure_start_dir_visible();
        this.recompute(cx);
        this.refresh(cx);
        this
    }

    /// Đang duyệt bộ nhớ trong thật (W-7).
    pub(crate) fn browses_storage(&self) -> bool {
        self.storage.is_some()
    }

    /// Dựng lại `items` + `known` từ các lần `dirs` đã đọc.
    fn sync_storage(&mut self) {
        if let Some(fs) = &self.storage {
            self.items = fs.items();
            self.known = fs.known_dirs();
        }
    }

    /// Đọc một thư mục bằng `GET /dirs` (W-7). Offline ⇒ giữ bản đã có.
    fn fetch_dir(&mut self, path: String, cx: &mut Context<Self>) {
        if self.offline || self.pending.values().any(|p| *p == path) {
            return;
        }
        let device = self.device.clone();
        let op = LibraryOp::Dirs { device, path: path.clone() };
        match self.model.update(cx, |m, _| m.library(op)) {
            Some(req) => {
                self.pending.insert(req, path);
                self.loading = true;
            }
            None => self.failed = true,
        }
        cx.notify();
    }

    /// Thư mục đang mở chưa đọc lần nào ⇒ đọc.
    fn fetch_if_unloaded(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.storage.as_ref().is_some_and(|fs| !fs.is_loaded(path)) {
            self.fetch_dir(path.to_owned(), cx);
        }
    }

    fn on_dirs(&mut self, path: String, outcome: &Outcome, cx: &mut Context<Self>) {
        let Some(fs) = self.storage.as_mut() else { return };
        match outcome {
            Outcome::Dirs { listing } => {
                fs.insert(listing.clone());
                self.failed = false;
            }
            // Thư mục không còn trên điện thoại: bỏ khỏi cây; đang đứng trong đó thì giữ làm thư mục ảo (vẫn gửi được,
            // điện thoại tạo lại khi commit).
            Outcome::Failed { error: CommandError::InvalidInput } => fs.remove(&path),
            Outcome::Failed { error: CommandError::Offline } => self.offline = true,
            _ => self.failed = true,
        }
        let snapshot = fs.clone();
        let device = self.device.clone();
        self.model.update(cx, |m, _| {
            m.storage.insert(device, snapshot);
        });
        self.sync_storage();
        self.ensure_start_dir_visible();
        self.recompute(cx);
    }

    /// Mục thư viện đang chọn (cho gd1_11 "Tải về máy").
    pub fn selected_items(&self) -> Vec<&LibraryItem> {
        self.selected
            .iter()
            .filter_map(|&i| self.entries.get(i))
            .filter_map(|e| e.item)
            .filter_map(|i| self.items.get(i))
            // Tệp ngoài thư viện (duyệt bộ nhớ, W-7) không có `id` ⇒ không tải về được.
            .filter(|it| !it.id.is_empty())
            .collect()
    }

    /// Thư mục đang mở nếu là đích hợp lệ.
    pub fn destination(&self) -> Option<&str> {
        match &self.at {
            Location::Dir(p) => Some(p),
            Location::Other => None,
        }
    }

    pub(crate) fn is_library(&self) -> bool {
        self.mode == BrowserMode::Library
    }

    pub(crate) fn dim(&self) -> bool {
        self.offline && self.has_data()
    }

    /// Đã có gì để hiện (danh sách, hoặc thư mục đã đọc khi duyệt bộ nhớ).
    pub(crate) fn has_data(&self) -> bool {
        !self.items.is_empty() || !self.known.is_empty()
    }

    /// Thư mục đích lần trước chưa có trên điện thoại (đã dời / chưa có tệp) ⇒ vẫn cho gửi vào: coi như thư mục ảo.
    fn ensure_start_dir_visible(&mut self) {
        let Location::Dir(dir) = &self.at else { return };
        if dir.is_empty() {
            return;
        }
        let tree = browse::build_tree(&self.items, &self.known, &self.virtual_dirs);
        let exists = browse::breadcrumb(dir).iter().all(|(_, full)| {
            fn find(nodes: &[browse::DirNode], full: &str) -> bool {
                nodes
                    .iter()
                    .any(|n| n.path == full || (full.starts_with(&format!("{}/", n.path)) && find(&n.children, full)))
            }
            find(&tree.roots, full)
        });
        if !exists {
            self.virtual_dirs.push(dir.clone());
        }
    }

    fn on_model(&mut self, ev: &ModelEvent, cx: &mut Context<Self>) {
        match ev {
            ModelEvent::Changed => {
                let (offline, name, browses) = {
                    let m = self.model.read(cx);
                    let dev = m.snapshot.devices.iter().find(|d| d.id == self.device);
                    let browses = m.browses_storage(&self.device);
                    (dev.is_none_or(|d| d.state != DeviceState::Online), dev.map(|d| d.name.clone()), browses)
                };
                let mut came_online = self.offline && !offline;
                // Người dùng vừa bật / tắt "Cho máy tính duyệt bộ nhớ": gốc của mọi `dir` đổi ⇒ về gốc, đọc lại.
                if browses != self.browses_storage() {
                    self.storage = browses.then(FsCache::default);
                    self.items.clear();
                    self.known.clear();
                    self.virtual_dirs.clear();
                    self.pending.clear();
                    self.at = Location::root();
                    self.back.clear();
                    self.forward.clear();
                    self.recompute(cx);
                    came_online = !offline;
                }
                if let Some(n) = name {
                    self.device_name = n;
                }
                self.offline = offline;
                if came_online {
                    self.refresh(cx);
                }
                cx.notify();
            }
            ModelEvent::Reply { req, outcome } if self.pending.contains_key(req) => {
                let path = self.pending.remove(req).unwrap_or_default();
                self.loading = self.req.is_some() || !self.pending.is_empty();
                self.on_dirs(path, outcome, cx);
            }
            ModelEvent::Reply { req, outcome } if Some(*req) == self.req => {
                self.req = None;
                self.loading = false;
                match outcome {
                    Outcome::Library { items } => {
                        self.items = items.clone();
                        self.failed = false;
                        self.ensure_start_dir_visible();
                    }
                    Outcome::Failed { error: CommandError::Offline } => self.offline = true,
                    _ => self.failed = true,
                }
                self.recompute(cx);
            }
            ModelEvent::Reply { .. } => {}
        }
    }

    /// Đọc lại danh sách từ điện thoại (offline ⇒ giữ bản lần trước).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.offline {
            self.loading = false;
            cx.notify();
            return;
        }
        if self.browses_storage() {
            // Gốc + các thư mục cha của chỗ đang mở (cây hiện đủ đường tới đó) + chính nó.
            self.fetch_dir(String::new(), cx);
            if let Location::Dir(p) = self.at.clone() {
                for (_, full) in browse::breadcrumb(&p) {
                    self.fetch_dir(full, cx);
                }
            }
            return;
        }
        let device = self.device.clone();
        self.req = self.model.update(cx, |m, _| m.library(LibraryOp::List { device }));
        self.loading = self.req.is_some();
        self.failed = self.req.is_none();
        cx.notify();
    }

    fn recompute(&mut self, cx: &mut Context<Self>) {
        let mut entries = browse::list_entries(&self.items, &self.known, &self.virtual_dirs, &self.at, &self.query);
        browse::sort_entries(&mut entries, self.sort.0, self.sort.1);
        self.entries = entries;
        self.selected.clear();
        self.anchor = None;
        cx.notify();
    }

    pub(crate) fn go(&mut self, to: Location, cx: &mut Context<Self>) {
        if to == self.at {
            return;
        }
        let from = std::mem::replace(&mut self.at, to);
        self.back.push(from);
        self.forward.clear();
        self.after_move(cx);
    }

    pub(crate) fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(prev) = self.back.pop() {
            let cur = std::mem::replace(&mut self.at, prev);
            self.forward.push(cur);
            self.after_move(cx);
        }
    }

    pub(crate) fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.forward.pop() {
            let cur = std::mem::replace(&mut self.at, next);
            self.back.push(cur);
            self.after_move(cx);
        }
    }

    fn after_move(&mut self, cx: &mut Context<Self>) {
        if let Location::Dir(p) = &self.at {
            for (_, full) in browse::breadcrumb(p) {
                self.tree_open.insert(full);
            }
        }
        self.query.clear();
        self.clear_search = true;
        self.creating = false;
        if let Location::Dir(p) = self.at.clone() {
            self.fetch_if_unloaded(&p, cx);
        }
        self.recompute(cx);
    }

    pub(crate) fn toggle_tree(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.tree_open.remove(path) {
            self.tree_open.insert(path.to_owned());
            self.fetch_if_unloaded(path, cx);
        }
        cx.notify();
    }

    pub(crate) fn sort_by(&mut self, key: SortKey, cx: &mut Context<Self>) {
        self.sort = if self.sort.0 == key { (key, !self.sort.1) } else { (key, true) };
        self.recompute(cx);
    }

    pub(crate) fn click_row(&mut self, ix: usize, ctrl: bool, shift: bool, cx: &mut Context<Self>) {
        self.suppress_select = true;
        let (set, anchor) = table::select_click(&self.selected, self.anchor, ix, ctrl, shift);
        self.selected = set;
        self.anchor = anchor;
        cx.notify();
    }

    /// Phím mũi tên đổi dòng chọn (click đã xử lý ở `click_row`, bỏ lần `on_select` đi kèm).
    pub(crate) fn key_select(&mut self, ix: usize, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.suppress_select) {
            return;
        }
        self.selected = BTreeSet::from([ix]);
        self.anchor = Some(ix);
        cx.notify();
    }

    pub(crate) fn activate(&mut self, entry: Entry, cx: &mut Context<Self>) {
        if entry.kind == EntryKind::Folder {
            self.go(Location::Dir(entry.path), cx);
        }
    }

    fn create_folder(&mut self, name: String, cx: &mut Context<Self>) {
        let Location::Dir(parent) = self.at.clone() else { return };
        let Some(name) = browse::sanitize_folder_name(&name) else {
            let msg: SharedString = self.i18n.t("browser.new_folder_bad").to_owned().into();
            self.folder_field.update(cx, |f, cx| f.set_error(Some(msg), cx));
            return;
        };
        let full = browse::join_dir(&parent, &name);
        let exists = self.entries.iter().any(|e| e.kind == EntryKind::Folder && e.path == full);
        if !exists {
            self.virtual_dirs.push(full.clone());
        }
        self.creating = false;
        self.tree_open.insert(parent);
        self.go(Location::Dir(full), cx);
        self.recompute(cx);
    }

    fn destination_label(&self) -> String {
        match &self.at {
            Location::Dir(p) if !p.is_empty() => p.clone(),
            _ if self.browses_storage() => self.i18n.t("browser.dest_storage").to_owned(),
            _ => self.i18n.t("browser.dest_root").to_owned(),
        }
    }
}

impl Render for PhoneBrowser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.clear_search) {
            self.search.update(cx, |f, cx| f.set_value("", window, cx));
        }
        let c = cx.theme().colors().clone();
        let search = self.search.clone();
        let tree_rows = {
            let tree = browse::build_tree(&self.items, &self.known, &self.virtual_dirs);
            tree::flatten(&tree, &self.tree_open, &self.device_name, self.i18n.t("browser.other"))
        };
        let banner = if self.at == Location::Other {
            Some(("browser.other_hint", c.muted))
        } else if self.failed {
            Some(("browser.failed", c.danger))
        } else if self.offline && self.has_data() {
            Some(("browser.offline", c.warning))
        } else if !self.offline && !self.browses_storage() && self.at == Location::root() {
            Some(("browser.browse_hint", c.muted))
        } else {
            None
        };
        let toolbar = breadcrumb::toolbar(self, &search, cx);
        let tree_view = tree::tree_view(self, tree_rows, cx);
        let table_view = table::table_view(self, cx);

        let bottom = match self.mode.clone() {
            BrowserMode::Library => None,
            BrowserMode::Destination { what, bytes } => {
                let dest = self.destination_label();
                let summary = match bytes {
                    Some(b) => self.i18n.t_args(
                        "browser.send_summary_size",
                        &[("what", &what), ("size", &fmt::size(b, self.i18n.language())), ("dest", &dest)],
                    ),
                    None => self.i18n.t_args("browser.send_summary", &[("what", &what), ("dest", &dest)]),
                };
                let can_send = self.at.is_destination() && !self.offline;
                let in_other = !self.at.is_destination();
                let left = if self.creating {
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(200.)).child(self.folder_field.clone()))
                        .child(
                            Button::new("br-folder-ok", self.i18n.t("browser.new_folder_ok")).size(Size::Sm).on_click(
                                cx.listener(|this, _, _, cx| {
                                    let name = this.folder_field.read(cx).value(cx).to_string();
                                    this.create_folder(name, cx);
                                }),
                            ),
                        )
                        .into_any_element()
                } else {
                    Button::new("br-new-folder", self.i18n.t("browser.new_folder"))
                        .size(Size::Sm)
                        .outlined()
                        .disabled(in_other)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.creating = true;
                            this.folder_field.update(cx, |f, cx| {
                                f.set_value("", window, cx);
                                f.set_error(None, cx);
                            });
                            let handle = this.folder_field.focus_handle(cx);
                            window.focus(&handle, cx);
                            cx.notify();
                        }))
                        .into_any_element()
                };
                Some(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_4()
                        .py_2()
                        .border_t_1()
                        .border_color(c.border)
                        .child(left)
                        .child(div().flex_1().min_w_0().truncate().text_sm().text_color(c.muted).child(
                            SharedString::from(if in_other {
                                self.i18n.t("browser.no_dest_here").to_owned()
                            } else {
                                summary
                            }),
                        ))
                        .child(
                            Button::new("br-cancel", self.i18n.t("browser.cancel"))
                                .size(Size::Sm)
                                .outlined()
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(BrowserEvent::Cancel))),
                        )
                        .child(
                            Button::new("br-send", self.i18n.t("browser.send_here"))
                                .size(Size::Sm)
                                .disabled(!can_send)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(dest) = this.destination() {
                                        let dest_dir = dest.to_owned();
                                        cx.emit(BrowserEvent::Send { dest_dir });
                                    }
                                })),
                        ),
                )
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .text_color(c.text)
            .child(toolbar)
            .children(banner.map(|(key, color)| {
                div().px_4().py_1().text_xs().text_color(color).child(SharedString::from(self.i18n.t(key).to_owned()))
            }))
            .child(div().flex_1().min_h(px(0.)).flex().child(tree_view).child(table_view))
            .children(bottom)
    }
}
