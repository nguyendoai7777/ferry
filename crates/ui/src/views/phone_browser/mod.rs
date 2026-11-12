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
//! - Menu chuột phải + thư mục mới / đổi tên / xoá (W-8, §6.12): [`edit`].
//! - Mở lại cửa sổ (12/11/2026): nhận [`BrowserState`] (thư mục, Lùi / Tiến, nhánh cây, mục chọn, vị trí cuộn) và xuất lại
//!   bằng [`PhoneBrowser::state`]. Thư mục chưa đọc trong phiên ⇒ hỏi bộ đệm của daemon (`LibraryOp::Cached`, vẽ ngay) song
//!   song với `Dirs` (làm mới). Thư viện: thư mục đang mở không còn (`404`) ⇒ lùi về thư mục cha, không báo lỗi.

mod breadcrumb;
mod clip;
mod edit;
mod export;
mod table;
mod tree;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use ferry_core::browse::{self, Entry, EntryKind, FsCache, KnownDir, Location, SortKey};
use ferry_core::device::{DeviceId, DeviceState};
use ferry_core::i18n::I18n;
use ferry_core::ipc::{CommandError, LibraryOp, Outcome};
use ferry_core::protocol::LibraryItem;
use ferry_core::view::{BrowserState, HISTORY_MAX, SELECTED_MAX, TREE_OPEN_MAX};
use materiant::button::{Button, Size};
use materiant::field::{FieldEvent, TextField};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    Context, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription, Window, div, px,
};
use materiant::icon::IconName;
use materiant::list::{Align, VirtualListHandle};
use materiant::overlay::dialog::DialogHandle;
use materiant::theme::ActiveTheme;

use crate::fmt;
use crate::model::{Model, ModelEvent};

/// Làm mới (`refresh`) đọc lại tối đa chừng này nhánh cây đang mở ngoài đường tới thư mục đang mở.
const REVALIDATE_OPEN_MAX: usize = 16;

fn location(dir: &Option<String>) -> Location {
    match dir {
        Some(p) => Location::Dir(p.clone()),
        None => Location::Other,
    }
}

/// Chỗ mở trình duyệt: một thư mục (đích lần trước / gốc), hoặc y như lần trước (mở lại cửa sổ).
#[derive(Debug, Clone, PartialEq)]
pub enum BrowserStart {
    Dir(String),
    Restore(BrowserState),
}

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
    /// Dán tệp / thư mục của máy (clipboard hệ thống) vào `dest_dir` trên điện thoại — trang thư viện gửi như thả vào.
    PasteFiles {
        paths: Vec<std::path::PathBuf>,
        dest_dir: String,
    },
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
    /// `req` của các lần `Cached` (bộ đệm daemon) đang chờ → thư mục.
    cached_req: HashMap<u64, String>,
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
    /// Số lần bấm làm mới — id hoạt ảnh quay một vòng của nút (`breadcrumb::refresh_button`).
    pub(crate) refresh_spins: usize,
    pub(crate) entries: Vec<Entry>,
    pub(crate) selected: BTreeSet<usize>,
    pub(crate) anchor: Option<usize>,
    suppress_select: bool,
    pub(crate) list: VirtualListHandle,
    /// Cuộn của cột cây (giữ khi mở lại cửa sổ).
    pub(crate) tree_list: VirtualListHandle,
    tree_open: HashSet<String>,
    /// Dòng thiết bị (gốc cây) đang mở.
    root_open: bool,
    /// W-8: hộp thoại thư mục mới / đổi tên / xoá.
    edit_dialog: DialogHandle,
    edit: Option<edit::Edit>,
    name_field: Entity<TextField>,
    /// Đã gửi thư mục mới / đổi tên, chờ trả lời (tắt nút xác nhận).
    edit_busy: bool,
    edit_req: HashMap<u64, edit::Pending>,
    deleting: Option<edit::DeleteRun>,
    /// W-9: lượt Dán đang chờ trả lời.
    pasting: Option<clip::PasteRun>,
    /// Lỗi của lệnh sửa vừa rồi (băng trên bảng), mất khi đổi thư mục / làm mới.
    notice: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<BrowserEvent> for PhoneBrowser {}

impl PhoneBrowser {
    pub fn new(
        model: Entity<Model>,
        i18n: Rc<I18n>,
        device: DeviceId,
        mode: BrowserMode,
        start: BrowserStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (start_dir, restore) = match start {
            BrowserStart::Restore(b) if b.device == device => (String::new(), Some(b)),
            BrowserStart::Restore(_) => (String::new(), None),
            BrowserStart::Dir(d) => (d, None),
        };
        let search_hint = i18n.t("browser.search").to_owned();
        let search = cx.new(|cx| {
            TextField::new("br-search", window, cx)
                .placeholder(search_hint)
                .leading_icon(IconName::Search)
                .clearable(true)
        });
        let name_hint = i18n.t("browser.name_placeholder").to_owned();
        let name_field = cx.new(|cx| TextField::new("br-name", window, cx).placeholder(name_hint));
        let mut subs = vec![
            cx.subscribe(&search, |this, _, ev: &FieldEvent, cx| {
                if let FieldEvent::Change(q) = ev {
                    this.query = q.to_string();
                    this.recompute(cx);
                }
            }),
            cx.subscribe_in(&name_field, window, |this, _, ev: &FieldEvent, window, cx| {
                if let FieldEvent::Submit(_) = ev {
                    this.confirm_edit(window, cx);
                }
            }),
        ];
        subs.push(
            cx.subscribe_in(&model, window, |this, _, ev: &ModelEvent, window, cx| this.on_model(ev, window, cx)),
        );

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
            cached_req: HashMap::new(),
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
            refresh_spins: 0,
            entries: Vec::new(),
            selected: BTreeSet::new(),
            anchor: None,
            suppress_select: false,
            list: VirtualListHandle::new(),
            tree_list: VirtualListHandle::new(),
            tree_open,
            root_open: true,
            edit_dialog: DialogHandle::new(cx),
            edit: None,
            name_field,
            edit_busy: false,
            edit_req: HashMap::new(),
            deleting: None,
            pasting: None,
            notice: None,
            _subs: subs,
        };
        if let Some(b) = &restore {
            this.at = location(&b.at);
            this.back = b.back.iter().map(location).collect();
            this.forward = b.forward.iter().map(location).collect();
            this.tree_open = b.tree_open.iter().cloned().collect();
            if let Location::Dir(p) = &this.at {
                this.tree_open.extend(browse::breadcrumb(p).into_iter().map(|(_, full)| full));
            }
            this.root_open = !b.root_closed;
        }
        this.sync_storage();
        this.ensure_start_dir_visible();
        this.recompute(cx);
        if let Some(b) = restore {
            this.select_names(&b.selected);
            this.list.scroll_to(b.table_top as usize, Align::Start);
            this.tree_list.scroll_to(b.tree_top as usize, Align::Start);
        }
        this.refresh(cx);
        this
    }

    /// Trạng thái để mở lại cửa sổ y như cũ (`ferry_core::view`).
    pub fn state(&self) -> BrowserState {
        let dir = |l: &Location| match l {
            Location::Dir(p) => Some(p.clone()),
            Location::Other => None,
        };
        let tail = |v: &[Location]| v[v.len().saturating_sub(HISTORY_MAX)..].iter().map(dir).collect();
        let mut tree_open: Vec<String> = self.tree_open.iter().cloned().collect();
        tree_open.sort();
        tree_open.truncate(TREE_OPEN_MAX);
        let top = |h: &VirtualListHandle| (h.scroll_top() / table::ROW_HEIGHT).round().max(0.) as u32;
        BrowserState {
            device: self.device.clone(),
            at: dir(&self.at),
            back: tail(&self.back),
            forward: tail(&self.forward),
            tree_open,
            root_closed: !self.root_open,
            selected: self
                .selected
                .iter()
                .filter_map(|ix| self.entries.get(*ix))
                .map(|e| e.name.clone())
                .take(SELECTED_MAX)
                .collect(),
            table_top: top(&self.list),
            tree_top: top(&self.tree_list),
        }
    }

    /// Chọn lại các mục theo tên (sau khi dựng lại `entries`).
    fn select_names(&mut self, names: &[String]) {
        if names.is_empty() {
            return;
        }
        let want: HashSet<&str> = names.iter().map(String::as_str).collect();
        self.selected =
            self.entries.iter().enumerate().filter(|(_, e)| want.contains(e.name.as_str())).map(|(i, _)| i).collect();
        self.anchor = self.selected.first().copied();
    }

    /// Hỏi daemon bản đã đọc lần trước của `path` (không gọi điện thoại) — vẽ ngay trong lúc `Dirs` làm mới.
    fn ask_cached(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.storage.as_ref().is_none_or(|fs| fs.is_loaded(path)) || self.cached_req.values().any(|p| p == path) {
            return;
        }
        let op = LibraryOp::Cached { device: self.device.clone(), path: path.to_owned() };
        if let Some(req) = self.model.update(cx, |m, _| m.library(op)) {
            self.cached_req.insert(req, path.to_owned());
        }
    }

    fn on_cached(&mut self, outcome: &Outcome, cx: &mut Context<Self>) {
        let Outcome::Dirs { listing } = outcome else { return };
        let Some(fs) = self.storage.as_mut() else { return };
        // `Dirs` mới về trước thì giữ bản mới.
        if fs.is_loaded(&listing.path) {
            return;
        }
        fs.insert(listing.clone());
        let (snapshot, device) = (fs.clone(), self.device.clone());
        self.model.update(cx, |m, _| {
            m.storage.insert(device, snapshot);
        });
        self.sync_storage();
        self.recompute(cx);
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
            self.ask_cached(path, cx);
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
            Outcome::Failed { error: CommandError::InvalidInput } => {
                fs.remove(&path);
                // Thư viện: đang đứng trong thư mục đã mất (xoá trên điện thoại lúc cửa sổ đóng) ⇒ về thư mục cha gần nhất.
                if self.mode == BrowserMode::Library
                    && let Location::Dir(at) = &self.at
                    && !path.is_empty()
                    && (*at == path || at.starts_with(&format!("{path}/")))
                {
                    self.at = Location::Dir(browse::parent_dir(&path).to_owned());
                    self.selected.clear();
                }
            }
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

    fn on_model(&mut self, ev: &ModelEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            ModelEvent::Reply { req, outcome } if self.edit_req.contains_key(req) => {
                self.on_edit_reply(*req, outcome, window, cx);
            }
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
                self.sync_export(cx);
                cx.notify();
            }
            ModelEvent::Reply { req, outcome } if self.cached_req.contains_key(req) => {
                self.cached_req.remove(req);
                self.on_cached(outcome, cx);
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
        self.notice = None;
        if self.browses_storage() {
            // Gốc + các thư mục cha của chỗ đang mở (cây hiện đủ đường tới đó) + chính nó, rồi các nhánh cây đang mở (nông
            // trước, có trần). Chưa có trong phiên ⇒ hỏi bộ đệm daemon trước (cả khi offline).
            let mut want = vec![String::new()];
            if let Location::Dir(p) = &self.at {
                want.extend(browse::breadcrumb(p).into_iter().map(|(_, full)| full));
            }
            let mut open: Vec<&String> = self.tree_open.iter().collect();
            open.sort_by_key(|p| (p.matches('/').count(), p.as_str()));
            want.extend(open.into_iter().take(REVALIDATE_OPEN_MAX).cloned());
            for p in &want {
                self.ask_cached(p, cx);
            }
            if !self.offline {
                for p in want {
                    self.fetch_dir(p, cx);
                }
                return;
            }
        }
        if self.offline {
            self.loading = false;
            cx.notify();
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
        // Ở gốc, thư mục cấp một đã có ở cột cây bên trái ⇒ bảng không liệt kê lại, chỉ còn tệp nằm thẳng ở gốc. Đang tìm
        // thì vẫn hiện để tìm được theo tên.
        if self.at == Location::root() && self.query.trim().is_empty() {
            entries.retain(|e| e.kind != EntryKind::Folder);
        }
        browse::sort_entries(&mut entries, self.sort.0, self.sort.1);
        // Giữ mục đang chọn theo tên qua lần dựng lại (làm mới nền, đổi cách xếp, lọc): đọc lại thư mục không làm mất chọn.
        let key = |e: &Entry| (e.kind == EntryKind::Folder, e.name.clone());
        let keep: HashSet<(bool, String)> =
            self.selected.iter().filter_map(|ix| self.entries.get(*ix)).map(key).collect();
        let anchor = self.anchor.and_then(|ix| self.entries.get(ix)).map(key);
        self.selected = entries.iter().enumerate().filter(|(_, e)| keep.contains(&key(e))).map(|(i, _)| i).collect();
        self.anchor = anchor.and_then(|a| entries.iter().position(|e| key(e) == a));
        self.entries = entries;
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
        self.notice = None;
        self.selected.clear();
        self.anchor = None;
        if let Location::Dir(p) = self.at.clone() {
            self.fetch_if_unloaded(&p, cx);
        }
        self.recompute(cx);
    }

    pub(crate) fn toggle_root(&mut self, cx: &mut Context<Self>) {
        self.root_open = !self.root_open;
        cx.notify();
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
            tree::flatten(&tree, &self.tree_open, self.root_open, &self.device_name, self.i18n.t("browser.other"))
        };
        let t = |key: &str| self.i18n.t(key).to_owned();
        let banner = if self.at == Location::Other {
            Some((t("browser.other_hint"), c.muted))
        } else if let Some(n) = &self.notice {
            Some((n.clone(), c.danger))
        } else if self.failed {
            Some((t("browser.failed"), c.danger))
        } else if self.offline && self.has_data() {
            Some((t("browser.offline"), c.warning))
        } else if !self.offline && !self.browses_storage() && self.at == Location::root() {
            Some((t("browser.browse_hint"), c.muted))
        } else {
            None
        };
        let toolbar = breadcrumb::toolbar(self, &search, window, cx);
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
                let left = Button::new("br-new-folder", self.i18n.t("browser.new_folder"))
                    .size(Size::Sm)
                    .outlined()
                    .disabled(in_other)
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Location::Dir(p) = this.at.clone() {
                            this.start_new_folder(p, true, window, cx);
                        }
                    }));
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
            .children(
                banner.map(|(text, color)| {
                    div().px_4().py_1().text_xs().text_color(color).child(SharedString::from(text))
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .on_key_down(cx.listener(Self::on_browser_key))
                    .child(tree_view)
                    .child(table_view),
            )
            .children(self.render_export_bar(cx))
            .children(bottom)
            .child(self.render_edit_dialog(cx))
    }
}
