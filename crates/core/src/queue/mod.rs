//! Hàng đợi bền (SQLite) + máy trạng thái mục upload / download (`PROTOCOL.md` mục 5) — task gd1_03.
//!
//! gd1_01 chỉ đặt định danh lô (W-3, `PROTOCOL.md` 5, 6.9): một lần thả / dán = một lô; mỗi mục upload mang
//! `device_id` ([`crate::device::DeviceId`]), `dir` đích (đã chuẩn hoá `/`, rỗng = gốc nơi lưu) và `batch_id`.

use serde::{Deserialize, Serialize};

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
