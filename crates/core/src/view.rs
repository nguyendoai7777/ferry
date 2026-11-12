//! Khôi phục cửa sổ (12/11/2026, "mở lại như Claude desktop"): `ferry-ui` thoát khi đóng cửa sổ (luật RAM), nên trạng thái
//! điều hướng và các thư mục đã đọc từ điện thoại sống ở daemon `ferry`, trong RAM.
//!
//! - [`ViewState`]: trang đang mở + lịch sử Lùi, trình duyệt điện thoại (thư mục, Lùi / Tiến, nhánh cây, mục chọn, vị trí
//!   cuộn), khay chờ, bảng nháp, cỡ / vị trí cửa sổ. UI gửi `UiToDaemon::SaveView` khi đổi (gộp ≥ 0,5 s) và lúc thoát;
//!   daemon trả lại trong `DaemonToUi::Restore` ngay sau `Hello`, trước ảnh chụp đầu. Không giữ hộp thoại.
//! - [`DirCache`]: các lần `GET /dirs` + `GET /list` gần nhất của từng điện thoại, có trần mục + byte, bỏ cái dùng lâu
//!   nhất. `Restore` chỉ mang các thư mục trình duyệt cần để vẽ khung đầu ([`DirCache::restore`], trần
//!   [`RESTORE_BUDGET`] để dòng IPC xa `MAX_LINE`); thư mục khác UI hỏi `LibraryOp::Cached` khi mở tới.
//!
//! Daemon thoát thì mất cả hai — chấp nhận (UI mở ở trang Gửi, đọc lại từ điện thoại). Riêng cỡ / vị trí cửa sổ UI còn ghi
//! `ui-state.json` để qua được lần khởi động lại máy.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::browse;
use crate::device::DeviceId;
use crate::protocol::{self, DirListing, LibraryItem};
use crate::queue::BatchId;

/// Trần lịch sử Lùi của cửa sổ và Lùi / Tiến của trình duyệt.
pub const HISTORY_MAX: usize = 50;
/// Trần nhánh cây đang mở / mục đang chọn / đường trong khay chờ giữ lại.
pub const TREE_OPEN_MAX: usize = 256;
pub const SELECTED_MAX: usize = 500;
pub const TRAY_MAX: usize = 2000;
/// Byte tối đa của thư mục + danh sách trong một `Restore` (dòng JSON < `ipc::MAX_LINE` 1 MiB, chừa chỗ cho khay chờ,
/// lịch sử…).
pub const RESTORE_BUDGET: usize = 640 * 1024;
/// Trần [`DirCache`] của daemon: tổng byte (JSON) và số mục (thư mục + danh sách thư viện).
pub const CACHE_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const CACHE_MAX_ENTRIES: usize = 512;

/// Trạng thái cửa sổ UI gửi daemon giữ hộ.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ViewState {
    pub page: PageState,
    /// Trang đã đi qua, cuối là trang ngay trước (nút Lùi của thanh trên).
    pub history: Vec<PageState>,
    /// Điện thoại đang chọn ở trang Gửi.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<DeviceId>,
    /// Khay chờ gửi (đường tệp / thư mục trên máy).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tray: Vec<PathBuf>,
    /// Lô đang mở rộng trong hàng đợi.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub expanded: Vec<BatchId>,
    /// Trình duyệt điện thoại của trang đang mở (`Library` / `Browse`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browser: Option<BrowserState>,
    /// Bảng nháp Cắt / Sao chép trong bộ nhớ điện thoại (W-9).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowState>,
}

/// Trang của cửa sổ chính.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "page", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum PageState {
    /// Trang Gửi.
    #[default]
    Main,
    /// Chọn nơi gửi khay chờ (trình duyệt ở chế độ chọn đích) — luồng tạm, không vào lịch sử.
    Browse,
    Devices,
    Library {
        device: DeviceId,
    },
    Settings,
    /// Thêm điện thoại, mở từ trang Gửi (`fromDevices = false`) hoặc Danh sách thiết bị. Bước PIN không giữ (như hộp
    /// thoại): mở lại thì về bước dò.
    AddPhone {
        from_devices: bool,
    },
}

/// Trình duyệt điện thoại (PROTOCOL 4.6).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BrowserState {
    pub device: DeviceId,
    /// Thư mục đang mở (`""` = gốc); `None` = nhóm "Khác".
    pub at: Option<String>,
    pub back: Vec<Option<String>>,
    pub forward: Vec<Option<String>>,
    /// Nhánh cây đang mở.
    pub tree_open: Vec<String>,
    /// Dòng thiết bị (gốc cây) đang thu gọn.
    pub root_closed: bool,
    /// Tên các mục đang chọn trong thư mục đang mở.
    pub selected: Vec<String>,
    /// Dòng trên cùng của bảng / cây đang thấy.
    pub table_top: u32,
    pub tree_top: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipState {
    pub device: DeviceId,
    pub entries: Vec<ClipPath>,
    pub cut: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipPath {
    pub path: String,
    pub folder: bool,
}

/// Cỡ / vị trí cửa sổ (px logic, toạ độ màn hình) + phóng to. Đang phóng to thì khung là cỡ khi khôi phục.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

impl WindowState {
    /// Số hữu hạn, cỡ dương — bản hỏng (tệp sửa tay, màn lạ) thì bỏ.
    pub fn is_sane(&self) -> bool {
        [self.x, self.y, self.width, self.height].iter().all(|v| v.is_finite() && v.abs() < 1e6)
            && self.width >= 1.
            && self.height >= 1.
    }
}

fn cap<T>(v: &mut Vec<T>, max: usize, keep_tail: bool) {
    if v.len() > max {
        if keep_tail {
            v.drain(..v.len() - max);
        } else {
            v.truncate(max);
        }
    }
}

impl ViewState {
    /// Kẹp mọi mảng theo trần (daemon gọi khi nhận — UI hỏng / độc không làm daemon phình RAM).
    pub fn clamp(&mut self) {
        cap(&mut self.history, HISTORY_MAX, true);
        cap(&mut self.tray, TRAY_MAX, false);
        cap(&mut self.expanded, TRAY_MAX, false);
        if let Some(b) = &mut self.browser {
            cap(&mut b.back, HISTORY_MAX, true);
            cap(&mut b.forward, HISTORY_MAX, true);
            cap(&mut b.tree_open, TREE_OPEN_MAX, false);
            cap(&mut b.selected, SELECTED_MAX, false);
        }
        if let Some(c) = &mut self.clip {
            cap(&mut c.entries, SELECTED_MAX, false);
        }
        if self.window.is_some_and(|w| !w.is_sane()) {
            self.window = None;
        }
    }

    /// Điện thoại bị Quên / thay bằng mã mới ⇒ bỏ mọi chỗ nhắc tới nó: đang ở thư viện của nó thì về Danh sách thiết bị.
    /// UI cũng tự soát lại khi khôi phục (máy có thể đã mất trong lúc cửa sổ đóng).
    pub fn forget_device(&mut self, device: &DeviceId) {
        let gone = |p: &PageState| matches!(p, PageState::Library { device: d } if d == device);
        if gone(&self.page) {
            self.page = PageState::Devices;
        }
        self.history.retain(|p| !gone(p));
        self.history.dedup();
        while self.history.last() == Some(&self.page) {
            self.history.pop();
        }
        if self.selected.as_ref() == Some(device) {
            self.selected = None;
        }
        if self.browser.as_ref().is_some_and(|b| &b.device == device) {
            self.browser = None;
            if self.page == PageState::Browse {
                self.page = PageState::Main;
            }
        }
        if self.clip.as_ref().is_some_and(|c| &c.device == device) {
            self.clip = None;
        }
    }
}

/// Dữ liệu điện thoại đi kèm `Restore`: các thư mục trình duyệt cần cho khung đầu + danh sách thư viện (điện thoại chưa
/// cho duyệt bộ nhớ). Có thể thiếu (vượt [`RESTORE_BUDGET`] / chưa đọc lần nào) — UI đọc lại như thường.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CachedData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceId>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dirs: Vec<DirListing>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library: Option<Vec<LibraryItem>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Slot {
    Dir(String),
    Library,
}

#[derive(Debug, Clone)]
enum Data {
    Dir(DirListing),
    Library(Vec<LibraryItem>),
}

#[derive(Debug, Clone)]
struct Cached {
    data: Data,
    bytes: usize,
    used: u64,
}

type Key = (DeviceId, Slot);

/// Bộ đệm thư mục / thư viện của daemon (LRU theo lần dùng, trần mục + byte). Không IO.
#[derive(Debug, Clone)]
pub struct DirCache {
    map: HashMap<Key, Cached>,
    bytes: usize,
    tick: u64,
    max_bytes: usize,
    max_entries: usize,
}

impl Default for DirCache {
    fn default() -> Self {
        DirCache::with_limits(CACHE_MAX_BYTES, CACHE_MAX_ENTRIES)
    }
}

fn json_len<T: Serialize>(v: &T) -> usize {
    serde_json::to_vec(v).map_or(usize::MAX, |b| b.len())
}

impl DirCache {
    pub fn with_limits(max_bytes: usize, max_entries: usize) -> DirCache {
        DirCache { map: HashMap::new(), bytes: 0, tick: 0, max_bytes, max_entries }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Tổng byte (JSON) đang giữ.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    fn put(&mut self, key: Key, data: Data, bytes: usize) {
        self.drop_key(&key);
        // Một mục to hơn nửa trần (thư viện rất lớn) ⇒ không giữ: khỏi đuổi cả bộ đệm vì một mục.
        if bytes > self.max_bytes / 2 {
            return;
        }
        self.tick += 1;
        self.bytes += bytes;
        self.map.insert(key, Cached { data, bytes, used: self.tick });
        while self.bytes > self.max_bytes || self.map.len() > self.max_entries {
            let Some(oldest) = self.map.iter().min_by_key(|(_, c)| c.used).map(|(k, _)| k.clone()) else { break };
            self.drop_key(&oldest);
        }
    }

    fn drop_key(&mut self, key: &Key) {
        if let Some(c) = self.map.remove(key) {
            self.bytes -= c.bytes;
        }
    }

    fn touch(&mut self, key: &Key) -> Option<&Data> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(key).map(|c| {
            c.used = tick;
            &c.data
        })
    }

    /// Kết quả `GET /dirs` mới đọc (ghi đè bản cũ).
    pub fn put_dir(&mut self, device: &DeviceId, listing: DirListing) {
        let bytes = json_len(&listing);
        let key = (device.clone(), Slot::Dir(protocol::normalize_dir(&listing.path)));
        self.put(key, Data::Dir(listing), bytes);
    }

    /// Thư mục không còn trên điện thoại (`404`) ⇒ bỏ nó, mọi thư mục con, và tên nó trong thư mục cha.
    pub fn remove_dir(&mut self, device: &DeviceId, path: &str) {
        let p = protocol::normalize_dir(path);
        let prefix = format!("{p}/");
        let gone: Vec<Key> = self
            .map
            .keys()
            .filter(|(d, s)| {
                d == device && matches!(s, Slot::Dir(k) if *k == p || (!p.is_empty() && k.starts_with(&prefix)))
            })
            .cloned()
            .collect();
        for k in gone {
            self.drop_key(&k);
        }
        if p.is_empty() {
            return;
        }
        let name = p.rsplit_once('/').map_or(p.as_str(), |(_, n)| n).to_owned();
        let parent = (device.clone(), Slot::Dir(browse::parent_dir(&p).to_owned()));
        if let Some(c) = self.map.get_mut(&parent)
            && let Data::Dir(l) = &mut c.data
        {
            l.dirs.retain(|d| d.name != name);
            let now = json_len(&*l);
            self.bytes = self.bytes - c.bytes + now;
            c.bytes = now;
        }
    }

    pub fn dir(&mut self, device: &DeviceId, path: &str) -> Option<DirListing> {
        match self.touch(&(device.clone(), Slot::Dir(protocol::normalize_dir(path))))? {
            Data::Dir(l) => Some(l.clone()),
            Data::Library(_) => None,
        }
    }

    /// Kết quả `GET /list` mới đọc.
    pub fn put_library(&mut self, device: &DeviceId, items: Vec<LibraryItem>) {
        let bytes = json_len(&items);
        self.put((device.clone(), Slot::Library), Data::Library(items), bytes);
    }

    pub fn library(&mut self, device: &DeviceId) -> Option<Vec<LibraryItem>> {
        match self.touch(&(device.clone(), Slot::Library))? {
            Data::Library(items) => Some(items.clone()),
            Data::Dir(_) => None,
        }
    }

    /// Điện thoại bị Quên / thay mã ⇒ bỏ hết của nó.
    pub fn forget_device(&mut self, device: &DeviceId) {
        let gone: Vec<Key> = self.map.keys().filter(|(d, _)| d == device).cloned().collect();
        for k in gone {
            self.drop_key(&k);
        }
    }

    /// Dữ liệu cho khung đầu của trình duyệt `b`: gốc, các thư mục cha của chỗ đang mở + chính nó, rồi các nhánh cây đang
    /// mở (nông trước); kèm danh sách thư viện nếu còn chỗ. Dừng ở thư mục đầu tiên vượt [`RESTORE_BUDGET`].
    pub fn restore(&mut self, b: &BrowserState) -> CachedData {
        self.restore_within(b, RESTORE_BUDGET)
    }

    fn restore_within(&mut self, b: &BrowserState, budget: usize) -> CachedData {
        let mut want: Vec<String> = vec![String::new()];
        if let Some(at) = &b.at {
            want.extend(browse::breadcrumb(at).into_iter().map(|(_, full)| full));
        }
        let mut open: Vec<&String> = b.tree_open.iter().collect();
        open.sort_by_key(|p| (p.matches('/').count(), p.as_str()));
        want.extend(open.into_iter().cloned());
        let mut seen = HashSet::new();
        let mut out = CachedData { device: Some(b.device.clone()), ..CachedData::default() };
        let mut left = budget;
        for path in want {
            let path = protocol::normalize_dir(&path);
            if !seen.insert(path.clone()) {
                continue;
            }
            let key = (b.device.clone(), Slot::Dir(path));
            let Some(bytes) = self.map.get(&key).map(|c| c.bytes) else { continue };
            if bytes > left {
                break;
            }
            if let Some(Data::Dir(l)) = self.touch(&key) {
                out.dirs.push(l.clone());
                left -= bytes;
            }
        }
        let key = (b.device.clone(), Slot::Library);
        if self.map.get(&key).is_some_and(|c| c.bytes <= left)
            && let Some(Data::Library(items)) = self.touch(&key)
        {
            out.library = Some(items.clone());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{RemoteDir, RemoteFile};

    fn dev(s: &str) -> DeviceId {
        DeviceId(s.into())
    }

    fn listing(path: &str, dirs: &[&str], files: usize) -> DirListing {
        DirListing {
            path: path.into(),
            dirs: dirs.iter().map(|n| RemoteDir { name: (*n).into(), modified: None, count: Some(1) }).collect(),
            files: (0..files)
                .map(|i| RemoteFile { name: format!("f{i}.mp4"), size: 1, modified: 0, id: None })
                .collect(),
        }
    }

    #[test]
    fn view_state_round_trips_camel_case() {
        let v = ViewState {
            page: PageState::Library { device: dev("p") },
            history: vec![PageState::Main, PageState::Devices],
            selected: Some(dev("p")),
            browser: Some(BrowserState {
                device: dev("p"),
                at: Some("DCIM/Camera".into()),
                back: vec![Some(String::new()), None],
                tree_open: vec!["DCIM".into()],
                selected: vec!["a.mp4".into()],
                table_top: 12,
                ..BrowserState::default()
            }),
            window: Some(WindowState { x: 10., y: 20., width: 900., height: 600., maximized: true }),
            ..ViewState::default()
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains(r#""page":{"page":"library","device":"p"}"#), "{json}");
        assert!(json.contains(r#""treeOpen":["DCIM"]"#) && json.contains(r#""tableTop":12"#), "{json}");
        assert_eq!(serde_json::from_str::<ViewState>(&json).unwrap(), v);
        // Trường thiếu (bản cũ / rỗng) ⇒ mặc định.
        assert_eq!(serde_json::from_str::<ViewState>("{}").unwrap(), ViewState::default());
        let add = serde_json::to_string(&PageState::AddPhone { from_devices: true }).unwrap();
        assert_eq!(add, r#"{"page":"add_phone","fromDevices":true}"#);
    }

    #[test]
    fn clamp_bounds_every_list_and_drops_broken_windows() {
        let mut v = ViewState {
            history: vec![PageState::Main; HISTORY_MAX + 10],
            tray: vec![PathBuf::from("/x"); TRAY_MAX + 1],
            browser: Some(BrowserState {
                tree_open: vec!["a".into(); TREE_OPEN_MAX + 5],
                back: vec![None; HISTORY_MAX + 1],
                ..BrowserState::default()
            }),
            window: Some(WindowState { x: f32::NAN, y: 0., width: 10., height: 10., maximized: false }),
            ..ViewState::default()
        };
        v.clamp();
        assert_eq!(v.history.len(), HISTORY_MAX);
        assert_eq!(v.tray.len(), TRAY_MAX);
        let b = v.browser.unwrap();
        assert_eq!((b.tree_open.len(), b.back.len()), (TREE_OPEN_MAX, HISTORY_MAX));
        assert!(v.window.is_none());
    }

    #[test]
    fn forgetting_a_phone_falls_back_to_the_devices_page() {
        let p = dev("p");
        let mut v = ViewState {
            page: PageState::Library { device: p.clone() },
            history: vec![PageState::Main, PageState::Devices, PageState::Library { device: p.clone() }],
            selected: Some(p.clone()),
            browser: Some(BrowserState { device: p.clone(), ..BrowserState::default() }),
            clip: Some(ClipState { device: p.clone(), entries: vec![], cut: false }),
            ..ViewState::default()
        };
        v.forget_device(&dev("other"));
        assert_eq!(v.page, PageState::Library { device: p.clone() });
        v.forget_device(&p);
        assert_eq!(v.page, PageState::Devices);
        // Lùi không về chính trang đang đứng.
        assert_eq!(v.history, [PageState::Main]);
        assert!(v.selected.is_none() && v.browser.is_none() && v.clip.is_none());
    }

    #[test]
    fn cache_evicts_least_recently_used_within_limits() {
        let p = dev("p");
        let one = json_len(&listing("a", &[], 3));
        let mut c = DirCache::with_limits(one * 3 + one / 2, 100);
        for name in ["a", "b", "c"] {
            c.put_dir(&p, listing(name, &[], 3));
        }
        assert_eq!(c.len(), 3);
        // "a" dùng lại ⇒ "b" là cũ nhất, bị đuổi khi thêm "d".
        assert!(c.dir(&p, "a").is_some());
        c.put_dir(&p, listing("d", &[], 3));
        assert!(c.dir(&p, "b").is_none());
        assert!(c.dir(&p, "a").is_some() && c.dir(&p, "c").is_some() && c.dir(&p, "d").is_some());
        assert!(c.bytes() <= one * 3 + one / 2);
        // Ghi đè cùng thư mục không đếm hai lần.
        c.put_dir(&p, listing("d/", &[], 3));
        assert_eq!(c.len(), 3);
        // Trần số mục.
        let mut c = DirCache::with_limits(usize::MAX / 2, 2);
        for name in ["a", "b", "c"] {
            c.put_dir(&p, listing(name, &[], 1));
        }
        assert_eq!(c.len(), 2);
        assert!(c.dir(&p, "a").is_none());
        // Mục quá to (hơn nửa trần) không giữ.
        let mut c = DirCache::with_limits(one, 10);
        c.put_dir(&p, listing("big", &[], 3));
        assert!(c.is_empty() && c.bytes() == 0);
    }

    #[test]
    fn removed_dir_takes_children_and_parent_entry() {
        let p = dev("p");
        let mut c = DirCache::default();
        c.put_dir(&p, listing("", &["A", "B"], 0));
        c.put_dir(&p, listing("A", &["X"], 1));
        c.put_dir(&p, listing("A/X", &[], 1));
        c.put_dir(&dev("q"), listing("A", &[], 1));
        let before = c.bytes();
        c.remove_dir(&p, "A");
        assert!(c.dir(&p, "A").is_none() && c.dir(&p, "A/X").is_none());
        assert_eq!(c.dir(&p, "").unwrap().dirs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["B"]);
        assert!(c.dir(&dev("q"), "A").is_some(), "other phone untouched");
        assert!(c.bytes() < before);
        c.forget_device(&p);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn restore_sends_the_path_to_the_open_dir_first_within_budget() {
        let p = dev("p");
        let mut c = DirCache::default();
        for (path, dirs) in [("", &["DCIM", "Music"][..]), ("DCIM", &["Camera"]), ("DCIM/Camera", &[]), ("Music", &[])]
        {
            c.put_dir(&p, listing(path, dirs, 2));
        }
        c.put_dir(&p, listing("Other", &[], 2));
        c.put_library(&p, vec![]);
        let b = BrowserState {
            device: p.clone(),
            at: Some("DCIM/Camera".into()),
            tree_open: vec!["Music".into(), "Gone".into()],
            ..BrowserState::default()
        };
        let r = c.restore(&b);
        let paths: Vec<_> = r.dirs.iter().map(|l| l.path.as_str()).collect();
        assert_eq!(paths, ["", "DCIM", "DCIM/Camera", "Music"]);
        assert_eq!(r.library, Some(vec![]));
        // Ngân sách chỉ đủ hai thư mục ⇒ gốc + cha gần gốc nhất, không thư viện.
        let two = json_len(&listing("", &["DCIM", "Music"], 2)) + json_len(&listing("DCIM", &["Camera"], 2));
        let r = c.restore_within(&b, two);
        assert_eq!(r.dirs.iter().map(|l| l.path.as_str()).collect::<Vec<_>>(), ["", "DCIM"]);
        assert!(r.library.is_none());
    }
}
