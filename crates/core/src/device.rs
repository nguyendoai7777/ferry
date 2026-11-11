//! Thiết bị (điện thoại) và máy trạng thái của nó (`PROTOCOL.md` mục 5) — task gd1_05.
//!
//! gd1_01 chỉ đặt định danh dùng chung cho `config` / `ipc` / `queue` (W-3): thiết bị nhận theo `id` (TXT `id=`),
//! không theo IP; địa chỉ là một **tập** IPv4 + IPv6 đổi được (`PROTOCOL.md` 6.8).

use serde::{Deserialize, Serialize};

/// `id` của điện thoại (TXT `id=` / `info.id`). Trên dây là chuỗi JSON thường.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
