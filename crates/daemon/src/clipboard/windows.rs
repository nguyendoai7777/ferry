//! `CF_HDROP` + `Preferred DropEffect`: Explorer dán xong một lần "dời" thì tự xoá nguồn (thư mục tạm của ta).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use clipboard_win::{Clipboard, raw};

const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;
const DROPEFFECT_LINK: u32 = 4;

/// `GetClipboardSequenceNumber` ngay sau khi ta đặt — đổi ⇒ đã có người chép thứ khác, đừng xoá.
static OURS: AtomicU32 = AtomicU32::new(0);

pub(super) fn set_files(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    let list: Vec<String> = paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    {
        let _open = Clipboard::new_attempts(10).map_err(|e| format!("open clipboard: {e}"))?;
        raw::empty().map_err(|e| format!("empty clipboard: {e}"))?;
        raw::set_file_list(&list).map_err(|e| format!("set file list: {e}"))?;
        if let Some(format) = raw::register_format("Preferred DropEffect") {
            let effect = if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY | DROPEFFECT_LINK };
            raw::set_without_clear(format.get(), &effect.to_le_bytes()).map_err(|e| format!("drop effect: {e}"))?;
        }
    }
    OURS.store(raw::seq_num().map_or(0, |n| n.get()), Ordering::Relaxed);
    Ok(())
}

pub(super) fn clear() -> Result<(), String> {
    let ours = OURS.swap(0, Ordering::Relaxed);
    if ours == 0 || raw::seq_num().map(|n| n.get()) != Some(ours) {
        return Ok(());
    }
    let _open = Clipboard::new_attempts(10).map_err(|e| format!("open clipboard: {e}"))?;
    raw::empty().map_err(|e| format!("empty clipboard: {e}"))
}
