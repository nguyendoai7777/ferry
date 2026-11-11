//! Chạy hàng đợi: upload nối tiếp (`PROTOCOL.md` 6.5) — task gd1_04; tải về `Range` (6.4) — task gd1_10.
//!
//! Một [`upload::UploadRunner`] cho mỗi điện thoại (một tệp một lúc, PROTOCOL 2.8): lấy đầu hàng của thiết bị trong
//! [`QueueStore`], thực thi `UploadEffect` của máy trạng thái bằng [`PhoneClient`], báo kết quả bằng `UploadEvent`.
//!
//! **Cắm vào tầng trên** (gd1_05 discovery, gd1_07 daemon):
//! - Địa chỉ đi vào qua [`LinkSender`] (`tokio::sync::watch`): `Some(Link)` = điện thoại Online ở địa chỉ đã chọn (đã
//!   happy eyeballs, đã có token); `None` = Offline ⇒ runner ngủ, không backoff vô ích. Địa chỉ đổi (IPv4 ↔ IPv6, IP
//!   mới) ⇒ gửi `Link` mới: runner dùng nó từ request kế tiếp, cùng `uid` (phiên theo `uid`, không theo kết nối, §6.8);
//!   đang chờ backoff thì thức ngay.
//! - Lệnh người dùng vào qua [`RunnerHandle`] ([`Command`]); runner là chỗ duy nhất áp lệnh lên mục của thiết bị nó
//!   (huỷ mục đang bay phải cắt request + `DELETE` phiên).
//! - Kết quả ra kênh [`TransferEvent`]: đổi trạng thái mục, tiến độ (theo nhịp, không mỗi gói), lô xong, cần ghép lại,
//!   địa chỉ hỏng (discovery chọn lại), điện thoại cần cập nhật (không có `dirs`).

pub mod throttle;
pub mod upload;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, watch};

pub use throttle::{SpeedMeter, Throttle};
pub use upload::{RunnerConfig, UploadRunner};

use crate::client::PhoneClient;
use crate::device::DeviceId;
use crate::protocol::{Info, feature};
use crate::queue::{BatchId, ItemId, QueueStore, UploadState};

/// Hàng đợi dùng chung giữa các runner + lệnh IPC `AddFiles` (gd1_07). Mỗi thao tác SQLite ngắn, không giữ khoá qua
/// `await`.
pub type SharedStore = Arc<Mutex<QueueStore>>;

/// Đường tới một điện thoại đang Online: client ở địa chỉ đã chọn (kèm token) + tính năng từ `info`.
#[derive(Debug, Clone)]
pub struct Link {
    pub client: Arc<PhoneClient>,
    /// `info.features` có `dirs` (§6.9) ⇒ gửi `X-File-Dir`; không ⇒ gửi phẳng.
    pub dirs: bool,
}

impl Link {
    pub fn new(client: PhoneClient, info: &Info) -> Link {
        Link { client: Arc::new(client), dirs: info.has_feature(feature::DIRS) }
    }

    pub fn addr(&self) -> SocketAddr {
        self.client.addr()
    }
}

/// Phía tầng trên giữ (discovery / daemon): `send_replace(Some(link))` khi Online / đổi địa chỉ, `None` khi Offline.
pub type LinkSender = watch::Sender<Option<Link>>;
/// Phía runner giữ.
pub type LinkWatch = watch::Receiver<Option<Link>>;

/// Kênh địa chỉ mới, bắt đầu Offline.
pub fn link_channel() -> (LinkSender, LinkWatch) {
    watch::channel(None)
}

/// Thao tác người dùng trên một mục / một lô (nút ở hàng đợi, `ItemAction` của IPC gd1_07).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemAction {
    Pause,
    /// Chạy tiếp sau tạm dừng / hết chỗ / ghép lại.
    Resume,
    Cancel,
    /// Thử lại sau Failed (đọc lại cỡ + mtime khi tệp nguồn đã đổi).
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Có mục mới trong hàng đợi của thiết bị (sau `AddFiles`).
    Wake,
    Item {
        id: ItemId,
        action: ItemAction,
    },
    Batch {
        batch: BatchId,
        action: ItemAction,
    },
    /// Thoát vòng chạy (tắt app). Request đang bay bị cắt; mở lại thì `QueueStore::open` đưa về `Waiting`.
    Stop,
}

/// Sự kiện runner phát ra (gd1_07 dựng snapshot / khay / thông báo từ đây).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferEvent {
    /// Trạng thái mục vừa đổi (đã ghi vào store).
    ItemChanged { id: ItemId, state: UploadState },
    /// Tiến độ trong khúc, mỗi `RunnerConfig::progress_interval`. `bytes_per_sec`: trung bình trượt 5 s.
    Progress { id: ItemId, sent: u64, size: u64, bytes_per_sec: u64 },
    /// Cả lô không còn mục nào chạy (PROTOCOL 2.6: một thông báo khi cả lô xong).
    BatchFinished { batch: BatchId, sent: u64, skipped: u64, failed: u64 },
    /// `401`: điện thoại đã "Quên máy tính". Runner dừng hàng của thiết bị tới khi nhận `Link` mới (đã ghép lại).
    DeviceNeedsRepair { device: DeviceId },
    /// Không nối được / đứt ở địa chỉ này — discovery nên chọn lại địa chỉ (§6.8 "lỗi thì chọn lại từ đầu").
    ConnectionLost { device: DeviceId, addr: SocketAddr },
    /// Điện thoại không có `dirs`: mục có thư mục được gửi phẳng — UI báo "Cập nhật app trên điện thoại để giữ thư mục"
    /// (§6.3). Một lần cho mỗi `Link`.
    PhoneNeedsUpdate { device: DeviceId },
}

/// Gửi lệnh cho một runner. Runner đã thoát ⇒ lệnh bị bỏ (trả `false`).
#[derive(Debug, Clone)]
pub struct RunnerHandle {
    tx: mpsc::UnboundedSender<Command>,
}

impl RunnerHandle {
    pub fn send(&self, cmd: Command) -> bool {
        self.tx.send(cmd).is_ok()
    }
}
