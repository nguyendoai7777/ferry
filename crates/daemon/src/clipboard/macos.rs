//! URL tệp trên `NSPasteboard` (qua `arboard`). Finder không có "cắt": người dùng dán bằng ⌥⌘V để dời.

use std::path::PathBuf;
use std::sync::Mutex;

/// Danh sách ta vừa đặt — clipboard còn đúng nó mới xoá (người dùng đã chép thứ khác thì để nguyên).
static OURS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

pub(super) fn set_files(paths: &[PathBuf], _cut: bool) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    cb.set().file_list(paths).map_err(|e| e.to_string())?;
    *OURS.lock().unwrap_or_else(|p| p.into_inner()) =
        paths.iter().map(|p| p.canonicalize().unwrap_or_else(|_| p.clone())).collect();
    Ok(())
}

pub(super) fn clear() -> Result<(), String> {
    let ours = std::mem::take(&mut *OURS.lock().unwrap_or_else(|p| p.into_inner()));
    if ours.is_empty() {
        return Ok(());
    }
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    let now = cb.get().file_list().unwrap_or_default();
    let now: Vec<PathBuf> = now.iter().map(|p| p.canonicalize().unwrap_or_else(|_| p.clone())).collect();
    if now == ours {
        cb.clear().map_err(|e| e.to_string())?;
    }
    Ok(())
}
