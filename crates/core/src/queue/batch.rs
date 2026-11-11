//! Dựng một lô từ tệp / thư mục người dùng thả, dán hoặc chọn (`PROTOCOL.md` 4.2, 5, 6.9; W-3, W-4) — đầu vào của lệnh
//! `AddFiles { paths, dest_dir }`. Chỉ đọc metadata, không đọc nội dung tệp.
//!
//! - Tệp lẻ ⇒ `dir = <đích>`.
//! - Thư mục `D:\Phim\Chuyến đi` ⇒ mỗi video bên trong `dir = <đích>/Chuyến đi/<đường con>` (giữ tên thư mục được thả
//!   làm gốc, như paste). Thư mục không có video thì không sinh mục nào. Tệp không phải video bỏ qua, đếm lại để báo gộp.
//! - Không theo symlink thư mục (tránh vòng lặp, không lan ra ngoài cây người dùng thả).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::UnixMs;

/// Đuôi video điện thoại nhận (`PROTOCOL.md` 6.6) — một nguồn ở `protocol` (gd1_02).
pub use crate::protocol::VIDEO_EXTENSIONS;

/// Trần độ sâu khi duyệt (điện thoại nhận tối đa 8 cấp `dir`, §6.9; dư cho đích đã có vài cấp).
const MAX_DEPTH: usize = 32;

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.iter().any(|v| v.eq_ignore_ascii_case(e)))
}

/// Chuẩn hoá `dir` đích: `\` ⇒ `/`, bỏ đoạn rỗng và `.`, cắt khoảng trắng hai đầu mỗi đoạn. `None` khi có `..`
/// (điện thoại sẽ trả `400`). Chuỗi rỗng = gốc nơi lưu.
pub fn normalize_dir(dir: &str) -> Option<String> {
    let mut parts = Vec::new();
    for seg in dir.split(['/', '\\']) {
        let seg = seg.trim();
        match seg {
            "" | "." => {}
            ".." => return None,
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

fn join_dir(base: &str, name: &str) -> String {
    if base.is_empty() { name.to_owned() } else { format!("{base}/{name}") }
}

/// Một tệp sẽ thành một mục upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUpload {
    pub path: PathBuf,
    /// Tên gửi (`X-File-Name`).
    pub name: String,
    /// `dir` đích đã chuẩn hoá (`X-File-Dir`).
    pub dir: String,
    pub size: u64,
    /// mtime lúc thả vào, mili giây Unix — phát hiện "File đã thay đổi".
    pub mtime: Option<UnixMs>,
}

/// Kết quả dựng lô.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchPlan {
    /// `dir` đích đã chuẩn hoá.
    pub dest_dir: String,
    /// Nhãn dòng gộp: tên thư mục khi thả đúng một thư mục; `None` ⇒ UI hiện "N tệp".
    pub label: Option<String>,
    pub files: Vec<NewUpload>,
    /// Tệp không phải video bị bỏ qua ("Bỏ qua N tệp không phải video").
    pub skipped_non_video: usize,
    /// Tệp / thư mục không đọc được (mất, không quyền) — bỏ qua, log.
    pub unreadable: usize,
}

/// Dựng lô từ đường dẫn người dùng đưa. `dest_dir` sai (`..`) ⇒ `InvalidInput`.
pub fn plan_batch(paths: &[PathBuf], dest_dir: &str) -> io::Result<BatchPlan> {
    let dest = normalize_dir(dest_dir)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination dir contains '..'"))?;
    let mut plan = BatchPlan { dest_dir: dest.clone(), ..BatchPlan::default() };
    let mut dirs_dropped = 0usize;
    for path in paths {
        let Ok(meta) = fs::metadata(path) else {
            log::warn!("queue: cannot stat dropped path {}", path.display());
            plan.unreadable += 1;
            continue;
        };
        if meta.is_dir() {
            dirs_dropped += 1;
            let Some(name) = path.file_name().and_then(|n| n.to_str()).and_then(normalize_dir) else {
                plan.unreadable += 1;
                continue;
            };
            // Thả cả ổ (`D:\`) không có tên ⇒ gửi thẳng vào đích.
            let root = if name.is_empty() { dest.clone() } else { join_dir(&dest, &name) };
            walk(path, &root, 0, &mut plan);
        } else {
            add_file(path, &meta, &dest, &mut plan);
        }
    }
    if dirs_dropped == 1 && paths.len() == 1 {
        plan.label = paths[0].file_name().map(|n| n.to_string_lossy().into_owned());
    }
    Ok(plan)
}

fn walk(dir: &Path, remote: &str, depth: usize, plan: &mut BatchPlan) {
    if depth > MAX_DEPTH {
        plan.unreadable += 1;
        return;
    }
    let Ok(read) = fs::read_dir(dir) else {
        log::warn!("queue: cannot read dir {}", dir.display());
        plan.unreadable += 1;
        return;
    };
    let mut entries: Vec<_> = read.filter_map(Result::ok).collect();
    // Tệp trước thư mục con, mỗi nhóm theo tên — thứ tự gửi đoán được.
    entries.sort_by_key(|e| (e.file_type().is_ok_and(|t| t.is_dir()), e.file_name()));
    for entry in entries {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else {
            plan.unreadable += 1;
            continue;
        };
        if ft.is_dir() {
            match entry.file_name().to_str().and_then(normalize_dir) {
                Some(name) if !name.is_empty() => walk(&path, &join_dir(remote, &name), depth + 1, plan),
                _ => plan.unreadable += 1,
            }
        } else if ft.is_symlink() {
            // Symlink tới tệp thì gửi; tới thư mục thì bỏ qua.
            match fs::metadata(&path) {
                Ok(meta) if meta.is_file() => add_file(&path, &meta, remote, plan),
                Ok(_) => {}
                Err(_) => plan.unreadable += 1,
            }
        } else {
            match entry.metadata() {
                Ok(meta) => add_file(&path, &meta, remote, plan),
                Err(_) => plan.unreadable += 1,
            }
        }
    }
}

fn add_file(path: &Path, meta: &fs::Metadata, dir: &str, plan: &mut BatchPlan) {
    if !is_video(path) {
        plan.skipped_non_video += 1;
        return;
    }
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        plan.unreadable += 1;
        return;
    };
    // `Upload-Length` phải > 0 (§6.5): tệp rỗng không gửi được.
    if meta.len() == 0 {
        plan.unreadable += 1;
        return;
    }
    plan.files.push(NewUpload {
        path: path.to_owned(),
        name,
        dir: dir.to_owned(),
        size: meta.len(),
        mtime: mtime_ms(meta),
    });
}

/// mtime của tệp, mili giây Unix.
pub fn mtime_ms(meta: &fs::Metadata) -> Option<UnixMs> {
    let t = meta.modified().ok()?;
    let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    UnixMs::try_from(d.as_millis()).ok()
}

/// Tệp nguồn còn đúng như lúc thả vào? `Err(NotFound)` khi mất.
pub fn source_matches(path: &Path, size: u64, mtime: Option<UnixMs>) -> io::Result<bool> {
    let meta = fs::metadata(path)?;
    Ok(meta.len() == size && (mtime.is_none() || mtime_ms(&meta) == mtime))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, bytes: usize) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, vec![7u8; bytes]).unwrap();
    }

    #[test]
    fn normalize() {
        assert_eq!(normalize_dir("").as_deref(), Some(""));
        assert_eq!(normalize_dir("/").as_deref(), Some(""));
        assert_eq!(normalize_dir(r"Trip\Day 1/ ./x/").as_deref(), Some("Trip/Day 1/x"));
        assert_eq!(normalize_dir("a/../b"), None);
    }

    #[test]
    fn video_extensions() {
        assert!(is_video(Path::new("a.MP4")));
        assert!(is_video(Path::new("dir/b.ts")));
        assert!(!is_video(Path::new("c.txt")));
        assert!(!is_video(Path::new("mp4")));
    }

    #[test]
    fn folder_keeps_its_name_as_root() {
        let tmp = tempfile::tempdir().unwrap();
        let trip = tmp.path().join("Chuyến đi");
        touch(&trip.join("a.mp4"), 3);
        touch(&trip.join("Ngày 1/b.mkv"), 5);
        touch(&trip.join("Ngày 1/notes.txt"), 1);
        touch(&trip.join("Rỗng/readme.md"), 1); // không video ⇒ không mục nào
        let loose = tmp.path().join("c.MOV");
        touch(&loose, 2);

        let plan = plan_batch(std::slice::from_ref(&trip), "Phim/").unwrap();
        assert_eq!(plan.label.as_deref(), Some("Chuyến đi"));
        let got: Vec<_> = plan.files.iter().map(|f| (f.name.as_str(), f.dir.as_str(), f.size)).collect();
        assert_eq!(got, [("a.mp4", "Phim/Chuyến đi", 3), ("b.mkv", "Phim/Chuyến đi/Ngày 1", 5)]);
        assert_eq!(plan.skipped_non_video, 2);

        let plan = plan_batch(&[trip, loose], "").unwrap();
        assert_eq!(plan.label, None);
        assert_eq!(plan.files.len(), 3);
        assert_eq!(plan.files[2].dir, "");
        assert!(plan.files[2].mtime.is_some());
    }

    #[test]
    fn bad_dest_and_missing_paths() {
        assert!(plan_batch(&[], "../x").is_err());
        let plan = plan_batch(&[PathBuf::from("/definitely/not/here.mp4")], "").unwrap();
        assert_eq!((plan.files.len(), plan.unreadable), (0, 1));
    }

    #[test]
    fn detects_source_change() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.mp4");
        touch(&f, 4);
        let meta = fs::metadata(&f).unwrap();
        assert!(source_matches(&f, 4, mtime_ms(&meta)).unwrap());
        assert!(!source_matches(&f, 5, mtime_ms(&meta)).unwrap());
        fs::remove_file(&f).unwrap();
        assert_eq!(source_matches(&f, 4, None).unwrap_err().kind(), io::ErrorKind::NotFound);
    }
}
