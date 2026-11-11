//! Hàng đợi bền (SQLite) + máy trạng thái mục upload / download (`PROTOCOL.md` mục 5) — task gd1_03.
//!
//! - [`state`]: máy trạng thái thuần `(trạng thái, sự kiện, giờ) ⇒ (trạng thái, effect)`; runner (gd1_04 upload, gd1_10
//!   tải về) thực thi effect rồi báo kết quả.
//! - [`backoff`]: 2 s, 4 s … trần 60 s; `Retry-After` thắng.
//! - [`batch`]: dựng lô từ tệp / thư mục thả, dán, chọn (`AddFiles { paths, dest_dir }`, W-3 / W-4, §6.9).
//! - [`store`]: SQLite ở `paths::data_dir()`, ghi mỗi lần chuyển trạng thái, tổng tiến độ theo lô, dọn "Đã gửi" sau 7 ngày.
//!
//! Một lần thả / dán = một lô (W-3, `PROTOCOL.md` 5, 6.9); mỗi mục upload mang `device_id`
//! ([`crate::device::DeviceId`]), `dir` đích (đã chuẩn hoá `/`, rỗng = gốc nơi lưu) và `batch_id`.

pub mod backoff;
pub mod batch;
pub mod state;
pub mod store;

pub use batch::{BatchPlan, NewUpload, plan_batch};
pub use state::{DownloadEffect, DownloadEvent, DownloadState, UploadEffect, UploadEvent, UploadState};
pub use store::{BatchProgress, ItemId, QueueStore};

use serde::{Deserialize, Serialize};

/// Mốc thời gian: mili giây Unix (cùng đơn vị `modified` của PROTOCOL 6.4; kiểu `INTEGER` của SQLite).
pub type UnixMs = i64;

/// Giờ hiện tại theo [`UnixMs`].
pub fn now_ms() -> UnixMs {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| UnixMs::try_from(d.as_millis()).unwrap_or(UnixMs::MAX))
}

/// Định danh một lô thả / dán (W-3). Trên dây là chuỗi JSON thường.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BatchId(pub String);

impl BatchId {
    /// Lô mới, UUID v4.
    pub fn new() -> BatchId {
        BatchId(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for BatchId {
    fn default() -> BatchId {
        BatchId::new()
    }
}
