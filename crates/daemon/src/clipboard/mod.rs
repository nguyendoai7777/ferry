//! Clipboard danh sách tệp của máy (Cắt / Sao chép sang máy tính, PROTOCOL §6.14, W-10).
//!
//! Daemon giữ clipboard, không phải `ferry-ui`: cửa sổ đóng là tiến trình UI thoát, mà trên X11 / XWayland dữ liệu
//! clipboard do tiến trình sở hữu phục vụ lúc dán. Chỉ đặt mục cấp trên (thư mục / tệp người dùng chọn), không đặt
//! từng tệp con.
//!
//! - Linux: tự làm chủ selection `CLIPBOARD` qua X11 (`x11rb`, GNOME Wayland đi qua XWayland — Mutter chuyển sang
//!   Wayland), đưa `text/uri-list` + `x-special/gnome-copied-files` (`copy` / `cut` cho Files, Dolphin, Nemo…).
//! - Windows: `CF_HDROP` + `Preferred DropEffect` (2 = dời, 5 = chép) — Explorer xoá mục nguồn sau khi dán khi dời.
//! - macOS: URL tệp trên `NSPasteboard`; Finder không có "cắt" cho clipboard (⌥⌘V là dời) nên `cut` chỉ ảnh hưởng
//!   chữ báo ở UI.

use std::path::PathBuf;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(windows)]
use windows as imp;

/// Hai hàm clipboard engine dùng — test thay bằng hàm ghi lại (không đụng clipboard thật của máy chạy test).
#[derive(Clone, Copy)]
pub struct Clipboard {
    pub set_files: fn(&[PathBuf], bool) -> Result<(), String>,
    pub clear: fn(),
}

impl Clipboard {
    pub const OS: Self = Self { set_files, clear };
}

/// Đặt danh sách tệp / thư mục lên clipboard. `cut` = dán xong thì nguồn bị dời đi.
pub fn set_files(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    if paths.is_empty() {
        return Err("empty file list".into());
    }
    imp::set_files(paths, cut)
}

/// Bỏ danh sách tệp ta đã đặt (nếu clipboard vẫn còn của ta); lỗi chỉ ghi log.
pub fn clear() {
    if let Err(e) = imp::clear() {
        log::debug!("clipboard clear: {e}");
    }
}

/// `file:///a%20b` theo RFC 8089: mã hoá mọi byte ngoài tập không dự trữ và `/`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn file_uri(path: &std::path::Path) -> String {
    let mut out = String::from("file://");
    let s = path.to_string_lossy();
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(*b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_escapes_spaces_and_unicode() {
        assert_eq!(file_uri(std::path::Path::new("/tmp/a b/Phim ả.mp4")), "file:///tmp/a%20b/Phim%20%E1%BA%A3.mp4");
    }
}
