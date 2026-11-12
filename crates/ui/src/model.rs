//! Kho trạng thái của cửa sổ: ảnh chụp daemon gửi + thiết bị đang chọn + bộ nhớ đệm thư viện + đường gửi lệnh.
//! Là một `Entity` GPUI; view nghe `ModelEvent`. UI không giữ mutex nào chung với luồng đọc IPC (`daemon_link`).

use std::collections::HashMap;

use ferry_core::browse::FsCache;
use ferry_core::device::{DeviceId, DeviceState};
use ferry_core::ipc::{
    DaemonToUi, DeviceView, LibraryOp, Outcome, PhoneOp, ProgressDelta, QueueOp, Sender, SettingsOp, Snapshot,
    UiToDaemon,
};
use ferry_core::protocol::{LibraryItem, feature};
use materiant::gpui_kit::{Context, EventEmitter};

use crate::ui_state::UiState;

pub enum ModelEvent {
    /// `Snapshot` / `Progress` mới đã áp.
    Changed,
    /// Trả lời đúng một lần cho lệnh có `req`.
    Reply { req: u64, outcome: Outcome },
}

/// Bảng nháp Cắt / Sao chép của bộ nhớ điện thoại (W-9): chỉ giữ đường, không gọi mạng tới lúc Dán. Sống ở `Model` để
/// trình duyệt mở lại (chọn đích ⇄ thư viện) vẫn dán được.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageClip {
    pub device: DeviceId,
    pub entries: Vec<ClipEntry>,
    /// Cắt ⇒ Dán là dời, xong thì bảng nháp rỗng; Sao chép thì giữ để dán tiếp.
    pub cut: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipEntry {
    /// Đường đầy đủ từ gốc bộ nhớ.
    pub path: String,
    pub folder: bool,
}

pub struct Model {
    pub snapshot: Snapshot,
    /// Đã nhận ít nhất một `Snapshot` (trước đó chưa biết gì về thiết bị).
    pub ready: bool,
    pub selected: Option<DeviceId>,
    /// Danh sách điện thoại lần cuối đọc được, theo thiết bị — hiện mờ khi offline.
    pub libraries: HashMap<DeviceId, Vec<LibraryItem>>,
    /// Các thư mục đã đọc khi điện thoại cho duyệt bộ nhớ (W-7) — trình duyệt mở lại thấy ngay, offline hiện mờ.
    pub storage: HashMap<DeviceId, FsCache>,
    pub state: UiState,
    pub clip: Option<StorageClip>,
    sender: Option<Sender<UiToDaemon>>,
    next_req: u64,
}

impl EventEmitter<ModelEvent> for Model {}

impl Model {
    pub fn new(sender: Option<Sender<UiToDaemon>>) -> Model {
        Model {
            snapshot: Snapshot::default(),
            ready: false,
            selected: None,
            libraries: HashMap::new(),
            storage: HashMap::new(),
            state: UiState::load(),
            clip: None,
            sender,
            next_req: 1,
        }
    }

    pub fn connected(&self) -> bool {
        self.sender.is_some()
    }

    /// Điện thoại đã ghép (có token), theo thứ tự trong ảnh chụp.
    pub fn paired(&self) -> Vec<&DeviceView> {
        self.snapshot.devices.iter().filter(|d| d.state.has_token()).collect()
    }

    pub fn current(&self) -> Option<&DeviceView> {
        let id = self.selected.as_ref()?;
        self.snapshot.devices.iter().find(|d| &d.id == id && d.state.has_token())
    }

    pub fn select(&mut self, id: DeviceId, cx: &mut Context<Self>) {
        if self.selected.as_ref() != Some(&id) {
            self.selected = Some(id);
            cx.emit(ModelEvent::Changed);
            cx.notify();
        }
    }

    /// Gửi lệnh hàng đợi; trả `req` để đối chiếu `ModelEvent::Reply`. `None` khi chưa nối daemon.
    pub fn queue(&mut self, op: QueueOp) -> Option<u64> {
        self.send(|req| UiToDaemon::Queue { req, op })
    }

    pub fn phone(&mut self, op: PhoneOp) -> Option<u64> {
        self.send(|req| UiToDaemon::Phone { req, op })
    }

    pub fn library(&mut self, op: LibraryOp) -> Option<u64> {
        self.send(|req| UiToDaemon::Library { req, op })
    }

    /// Lệnh cài đặt qua kết nối chính (màn Danh sách thiết bị: đọc bí danh, đổi tên).
    pub fn settings(&mut self, op: SettingsOp) -> Option<u64> {
        self.send(|req| UiToDaemon::Settings { req, op })
    }

    fn send(&mut self, make: impl FnOnce(u64) -> UiToDaemon) -> Option<u64> {
        let sender = self.sender.as_ref()?;
        let req = self.next_req;
        self.next_req += 1;
        match sender.send(&make(req)) {
            Ok(()) => Some(req),
            Err(e) => {
                log::warn!("ipc: send failed: {e}");
                None
            }
        }
    }

    /// Điện thoại cho duyệt bộ nhớ trong (W-7, `features: browse` lần cuối biết).
    pub fn browses_storage(&self, device: &DeviceId) -> bool {
        self.snapshot
            .devices
            .iter()
            .find(|d| &d.id == device)
            .is_some_and(|d| d.features.iter().any(|f| f == feature::BROWSE))
    }

    /// Điện thoại cho sửa bộ nhớ (W-8, `features: edit` lần cuối biết) — chỉ có nghĩa cùng [`Self::browses_storage`].
    pub fn edits_storage(&self, device: &DeviceId) -> bool {
        self.snapshot
            .devices
            .iter()
            .find(|d| &d.id == device)
            .is_some_and(|d| d.features.iter().any(|f| f == feature::EDIT))
    }

    /// Điện thoại cho chép / dời giữa thư mục (W-9, `features: copy`) — chỉ có nghĩa cùng [`Self::edits_storage`].
    pub fn copies_storage(&self, device: &DeviceId) -> bool {
        self.snapshot
            .devices
            .iter()
            .find(|d| &d.id == device)
            .is_some_and(|d| d.features.iter().any(|f| f == feature::COPY))
    }

    /// Điện thoại cho tải theo đường dẫn (W-10, `features: fetch`) — Cắt / Sao chép sang máy tính.
    pub fn fetches_storage(&self, device: &DeviceId) -> bool {
        self.snapshot
            .devices
            .iter()
            .find(|d| &d.id == device)
            .is_some_and(|d| d.features.iter().any(|f| f == feature::FETCH))
    }

    /// Khoá thư mục đích nhớ theo chế độ: cùng chuỗi `dir` mang nghĩa khác khi có / không `browse` (§6.11).
    fn dest_key(&self, device: &DeviceId) -> String {
        if self.browses_storage(device) { format!("{}#browse", device.0) } else { device.0.clone() }
    }

    /// Thư mục đích lần trước của điện thoại (rỗng = gốc).
    pub fn last_dest(&self, device: &DeviceId) -> String {
        self.state.last_dest.get(&self.dest_key(device)).cloned().unwrap_or_default()
    }

    pub fn remember_dest(&mut self, device: &DeviceId, dest: &str) {
        let key = self.dest_key(device);
        if self.state.last_dest.get(&key).map(String::as_str) != Some(dest) {
            self.state.last_dest.insert(key, dest.to_owned());
            self.state.save();
        }
    }

    pub fn handle(&mut self, msg: DaemonToUi, cx: &mut Context<Self>) {
        match msg {
            DaemonToUi::Snapshot(s) => {
                self.snapshot = s;
                self.ready = true;
                self.fix_selection();
                cx.emit(ModelEvent::Changed);
                cx.notify();
            }
            DaemonToUi::Progress(p) => {
                apply_progress(&mut self.snapshot, &p);
                cx.emit(ModelEvent::Changed);
                cx.notify();
            }
            DaemonToUi::Reply { req, outcome } => {
                if let Outcome::Library { items } = &outcome
                    && let Some(id) = &self.selected
                {
                    self.libraries.insert(id.clone(), items.clone());
                }
                cx.emit(ModelEvent::Reply { req, outcome });
            }
            // Focus xử lý ở `daemon_link` (cần cửa sổ); `Ok` / `Error` chỉ log ở đó.
            // `OpenSettings` cũng ở `daemon_link` (mở cửa sổ cài đặt, gd1_12).
            DaemonToUi::Focus | DaemonToUi::OpenSettings | DaemonToUi::Ok | DaemonToUi::Error { .. } => {}
        }
    }

    /// Thiết bị đang chọn biến mất / bị quên ⇒ chọn lại: ưu tiên Online, rồi máy đầu tiên.
    fn fix_selection(&mut self) {
        let keep = self.current().is_some();
        if keep {
            return;
        }
        let paired = self.paired();
        let pick = paired.iter().find(|d| d.state == DeviceState::Online).or(paired.first()).map(|d| d.id.clone());
        self.selected = pick;
    }
}

/// Vá tiến độ lên ảnh chụp: chỉ mục đã có trong `uploads` (id lạ ⇒ bỏ qua, chờ ảnh chụp kế); `sent` mới thay cũ.
pub fn apply_progress(snapshot: &mut Snapshot, delta: &ProgressDelta) {
    for p in &delta.items {
        if !snapshot.uploads.iter().any(|u| u.id == p.id) && !snapshot.downloads.iter().any(|d| d.id == p.id) {
            continue;
        }
        match snapshot.progress.iter_mut().find(|q| q.id == p.id) {
            Some(q) => *q = *p,
            None => snapshot.progress.push(*p),
        }
    }
}

#[cfg(test)]
mod tests {
    use ferry_core::ipc::ItemProgress;
    use ferry_core::queue::UnixMs;
    use ferry_core::queue::state::UploadState;
    use ferry_core::queue::store::UploadItem;

    use super::*;

    pub fn upload(id: i64, device: &str, name: &str, size: u64, state: UploadState) -> UploadItem {
        UploadItem {
            id,
            device_id: DeviceId(device.into()),
            batch_id: None,
            path: format!("/x/{name}").into(),
            name: name.into(),
            dir: String::new(),
            size,
            mtime: None,
            state,
            failures: 0,
            created: 0 as UnixMs,
            updated: 0 as UnixMs,
        }
    }

    #[test]
    fn progress_patches_known_ids_only() {
        let mut s = Snapshot::default();
        s.uploads.push(upload(1, "p", "a.mp4", 100, UploadState::Queued));
        let delta = ProgressDelta {
            items: vec![
                ItemProgress { id: 1, sent: 10, size: 100, bytes_per_sec: 5 },
                ItemProgress { id: 99, sent: 1, size: 1, bytes_per_sec: 1 },
            ],
        };
        apply_progress(&mut s, &delta);
        assert_eq!(s.progress.len(), 1);
        assert_eq!(s.progress[0].sent, 10);
        let delta = ProgressDelta { items: vec![ItemProgress { id: 1, sent: 50, size: 100, bytes_per_sec: 5 }] };
        apply_progress(&mut s, &delta);
        assert_eq!(s.progress.len(), 1);
        assert_eq!(s.progress[0].sent, 50);
    }
}
