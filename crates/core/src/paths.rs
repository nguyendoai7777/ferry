//! Đường dẫn theo OS: cấu hình, dữ liệu (log, khoá một-instance, hàng đợi), socket IPC.

use std::io;
use std::path::PathBuf;

/// Tên thư mục con của app trong thư mục cấu hình / dữ liệu của OS.
pub const APP_DIR: &str = "media-wireless-sync";
pub const CONFIG_FILE: &str = "config.json";
/// Ghi đè đường dẫn `config.json` — test tích hợp / chạy dev không đụng cấu hình thật của máy.
pub const CONFIG_FILE_VAR: &str = "MWS_CONFIG_FILE";
/// Ghi đè thư mục dữ liệu (log, khoá, socket, hàng đợi) — chạy song song hai bản dev, test tích hợp.
pub const DATA_DIR_VAR: &str = "MWS_DATA_DIR";

/// Windows: `%APPDATA%\media-wireless-sync` · macOS: `~/Library/Application Support/media-wireless-sync` · Linux:
/// `~/.config/media-wireless-sync`.
pub fn config_dir() -> io::Result<PathBuf> {
    dirs::config_dir().map(|d| d.join(APP_DIR)).ok_or_else(|| not_found("config dir"))
}

/// `config.json`; `MWS_CONFIG_FILE` thắng.
pub fn config_file() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os(CONFIG_FILE_VAR) {
        return Ok(path.into());
    }
    config_dir().map(|d| d.join(CONFIG_FILE))
}

/// Log, khoá một-instance, hàng đợi SQLite. Windows: `%LOCALAPPDATA%\media-wireless-sync` · macOS: `~/Library/Application
/// Support/media-wireless-sync` · Linux: `~/.local/share/media-wireless-sync`. `MWS_DATA_DIR` thắng.
pub fn data_dir() -> io::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(DATA_DIR_VAR) {
        return Ok(dir.into());
    }
    dirs::data_local_dir().map(|d| d.join(APP_DIR)).ok_or_else(|| not_found("data dir"))
}

/// `<data_dir>/logs`.
pub fn log_dir() -> io::Result<PathBuf> {
    data_dir().map(|d| d.join("logs"))
}

/// Thư mục chứa socket IPC (Unix): `$XDG_RUNTIME_DIR/media-wireless-sync` khi có (Linux), không thì `<data_dir>/run`.
/// Có `MWS_DATA_DIR` thì luôn `<data_dir>/run` — hai bản dev không chung socket.
#[cfg(unix)]
pub fn runtime_dir() -> io::Result<PathBuf> {
    if std::env::var_os(DATA_DIR_VAR).is_none()
        && let Some(d) = dirs::runtime_dir()
    {
        return Ok(d.join(APP_DIR));
    }
    data_dir().map(|d| d.join("run"))
}

fn not_found(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("OS gives no {what}"))
}
