//! Trạng thái riêng của cửa sổ, sống qua các lần mở: thư mục đích lần trước của từng điện thoại (W-4).
//! Không phải cấu hình người dùng ⇒ tệp riêng `ui-state.json` ở thư mục dữ liệu (daemon là bên duy nhất ghi
//! `config.json`, `CLAUDE.md` mục Bố cục). Lỗi đọc / ghi chỉ log: mất thì lần sau mở ở gốc.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ferry_core::paths;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiState {
    /// `id` điện thoại → thư mục đích (`/` phân cách, rỗng = gốc).
    pub last_dest: BTreeMap<String, String>,
}

fn file() -> Option<PathBuf> {
    paths::data_dir().ok().map(|d| d.join("ui-state.json"))
}

impl UiState {
    pub fn load() -> UiState {
        let Some(path) = file() else { return UiState::default() };
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                log::warn!("ui-state: {e}; starting empty");
                UiState::default()
            }),
            Err(_) => UiState::default(),
        }
    }

    pub fn save(&self) {
        let Some(path) = file() else { return };
        let result = serde_json::to_vec_pretty(self).map_err(std::io::Error::other).and_then(|bytes| {
            std::fs::create_dir_all(path.parent().unwrap_or(&path)).and_then(|()| std::fs::write(&path, bytes))
        });
        if let Err(e) = result {
            log::warn!("ui-state: cannot save: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_is_camel_case() {
        let mut s = UiState::default();
        s.last_dest.insert("abc".into(), "Phim/Mới".into());
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"lastDest":{"abc":"Phim/Mới"}}"#);
        assert_eq!(serde_json::from_str::<UiState>("{}").unwrap(), UiState::default());
    }
}
