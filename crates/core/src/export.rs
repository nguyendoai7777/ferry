//! Cắt / Sao chép từ điện thoại sang máy tính (`PROTOCOL.md` §6.14, W-10).
//!
//! Trình quản lý tệp của máy (Files, Explorer, Finder) chỉ dán được tệp đã nằm trên đĩa, nên Sao chép / Cắt trong trình
//! duyệt điện thoại = tải các mục đã chọn vào một **thư mục tạm của phiên** (`<data_dir>/clipboard/<phiên>/`), giữ nguyên
//! cây, rồi daemon mới đặt danh sách mục cấp đầu lên clipboard của hệ thống.
//!
//! - [`plan`]: hỏi `GET /dirs` (§6.11) để biết cỡ / ngày sửa của từng tệp (UI chỉ gửi đường), thư mục thì đi cả cây con.
//!   Mục ẩn (`dirs` không trả) không chép.
//! - [`download`]: từng tệp một (§2.8), `GET /fetch?path=` ghi dần ra `<tệp>.ferry-part` (không đọc cả tệp vào RAM),
//!   đứt thì tải tiếp bằng `Range` + `If-Range`; đủ thì đổi tên, đặt ngày sửa theo điện thoại.
//! - Cắt: chỉ xoá trên điện thoại khi **biết đã dán** — mục cấp đầu biến khỏi thư mục phiên mà thư mục phiên vẫn còn
//!   ([`moved_tops`]). Khi đó [`remove_moved`] xoá từng tệp còn đúng cỡ + ngày sửa như lúc tải, thư mục xoá từ dưới lên
//!   bằng `ifEmpty=1` ⇒ thư mục còn mục ẩn / tệp mới thì giữ. Không dán ⇒ không xoá gì.
//! - Dọn: [`discard`] khi Sao chép / Cắt lần sau hoặc người dùng bỏ; [`clean_stale`] lúc khởi động (phiên quá
//!   [`KEEP_FOR`]).

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use crate::browse::parent_dir;
use crate::client::{ClientError, PhoneClient};
use crate::protocol::{DirListing, normalize_dir};
use crate::transfer::MeteredWriter;
use crate::transfer::download::sanitize_file_name;

/// Thư mục con của `data_dir` chứa các phiên.
pub const DIR_NAME: &str = "clipboard";
/// Phiên sống tối đa chừng này rồi bị dọn (dán sau đó thì trình quản lý tệp báo không còn tệp).
pub const KEEP_FOR: Duration = Duration::from_secs(24 * 3600);
/// Trần số tệp một lần Sao chép (cây quá lớn ⇒ `TooMany`, khỏi tải cả bộ nhớ điện thoại vì một cú bấm nhầm).
pub const MAX_FILES: usize = 20_000;
/// Đuôi tệp đang tải trong thư mục phiên.
const PART_EXT: &str = "ferry-part";
/// Lỗi tạm liên tiếp (không thêm được byte nào) tối đa trên một tệp.
const MAX_ATTEMPTS: u32 = 6;

/// Một mục UI chọn: đường trên bộ nhớ dùng chung + là thư mục hay tệp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEntry {
    pub path: String,
    pub folder: bool,
}

/// Mục cấp đầu (cái đặt lên clipboard).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedTop {
    pub remote: String,
    pub local: PathBuf,
    pub folder: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Chỉ số trong [`ExportPlan::tops`].
    pub top: usize,
    pub remote: String,
    pub local: PathBuf,
    pub size: u64,
    pub modified: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedDir {
    pub top: usize,
    pub remote: String,
    pub local: PathBuf,
}

/// Việc của một lần Sao chép / Cắt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportPlan {
    /// Thư mục phiên.
    pub root: PathBuf,
    pub tops: Vec<PlannedTop>,
    pub files: Vec<PlannedFile>,
    /// Mọi thư mục (cả mục cấp đầu là thư mục), cha trước con.
    pub dirs: Vec<PlannedDir>,
}

impl ExportPlan {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Đường đặt lên clipboard.
    pub fn top_paths(&self) -> Vec<PathBuf> {
        self.tops.iter().map(|t| t.local.clone()).collect()
    }
}

#[derive(Debug)]
pub enum ExportError {
    /// Lỗi điện thoại / mạng (theo mã trạng thái).
    Client(ClientError),
    /// Mục đã chọn không còn trên điện thoại.
    Missing(String),
    /// Quá [`MAX_FILES`] tệp.
    TooMany,
    /// Không ghi được thư mục tạm.
    Local(io::Error),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(e) => write!(f, "{e}"),
            Self::Missing(p) => write!(f, "missing on phone: {p}"),
            Self::TooMany => write!(f, "more than {MAX_FILES} files"),
            Self::Local(e) => write!(f, "local: {e}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl From<ClientError> for ExportError {
    fn from(e: ClientError) -> Self {
        match e {
            ClientError::Local(e) => Self::Local(e),
            e => Self::Client(e),
        }
    }
}

/// Thư mục phiên mới (chưa tạo) dưới `base`: `<mili giây>-<ngẫu nhiên>`.
pub fn new_session_dir(base: &Path) -> PathBuf {
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let tail = uuid::Uuid::new_v4().simple().to_string();
    base.join(format!("{ms}-{}", &tail[..8]))
}

/// Tên `name` (đã làm sạch) chưa dùng trong `taken` (so không phân biệt hoa thường: NTFS / APFS mặc định không phân biệt).
fn unique_name(taken: &mut HashSet<String>, name: &str, folder: bool) -> String {
    let clean = keep_hidden(name);
    let (stem, ext) = match clean.rsplit_once('.') {
        Some((s, e)) if !folder && !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (clean.clone(), String::new()),
    };
    let mut n = 1;
    loop {
        let cand = if n == 1 { clean.clone() } else { format!("{stem} ({n}){ext}") };
        if taken.insert(cand.to_lowercase()) {
            return cand;
        }
        n += 1;
    }
}

/// Như [`sanitize_file_name`] nhưng giữ một dấu chấm đầu: mục ẩn trên điện thoại (W-11) chép sang máy vẫn là mục ẩn
/// (`.nomedia`, không thành `nomedia`).
fn keep_hidden(name: &str) -> String {
    match name.trim_start().strip_prefix('.') {
        Some(rest) if !rest.trim_start_matches('.').trim().is_empty() => format!(".{}", sanitize_file_name(rest)),
        _ => sanitize_file_name(name),
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_owned() } else { format!("{dir}/{name}") }
}

fn last_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Dựng [`ExportPlan`] cho `entries` (đường điện thoại), thư mục phiên `root` (chưa cần có).
pub async fn plan(client: &PhoneClient, entries: &[ExportEntry], root: &Path) -> Result<ExportPlan, ExportError> {
    let mut out = ExportPlan { root: root.to_path_buf(), tops: Vec::new(), files: Vec::new(), dirs: Vec::new() };
    let mut top_names = HashSet::new();
    // Tệp cấp đầu: một lần `dirs` cho mỗi thư mục cha.
    let mut parents: BTreeMap<String, DirListing> = BTreeMap::new();
    for e in entries {
        let remote = normalize_dir(&e.path);
        if remote.is_empty() {
            return Err(ExportError::Missing(remote));
        }
        let name = last_segment(&remote).to_owned();
        let local = root.join(unique_name(&mut top_names, &name, e.folder));
        let top = out.tops.len();
        out.tops.push(PlannedTop { remote: remote.clone(), local: local.clone(), folder: e.folder });
        if e.folder {
            walk(client, top, &remote, &local, &mut out).await?;
            continue;
        }
        let parent = parent_dir(&remote).to_owned();
        if !parents.contains_key(&parent) {
            let listing = client.dirs(&parent).await.map_err(|e| missing_or(e, &remote))?;
            parents.insert(parent.clone(), listing);
        }
        let file = find_file(&parents[&parent], &name).ok_or_else(|| ExportError::Missing(remote.clone()))?;
        out.files.push(PlannedFile { top, remote, local, size: file.size, modified: file.modified });
        if out.files.len() > MAX_FILES {
            return Err(ExportError::TooMany);
        }
    }
    Ok(out)
}

fn missing_or(e: ClientError, remote: &str) -> ExportError {
    match e {
        ClientError::NotFound => ExportError::Missing(remote.to_owned()),
        e => e.into(),
    }
}

fn find_file<'a>(listing: &'a DirListing, name: &str) -> Option<&'a crate::protocol::RemoteFile> {
    listing
        .files
        .iter()
        .find(|f| f.name == name)
        .or_else(|| listing.files.iter().find(|f| f.name.to_lowercase() == name.to_lowercase()))
}

/// Đi cây con của thư mục `remote` (chiều rộng, cha trước con).
async fn walk(
    client: &PhoneClient,
    top: usize,
    remote: &str,
    local: &Path,
    out: &mut ExportPlan,
) -> Result<(), ExportError> {
    let mut queue = std::collections::VecDeque::from([(remote.to_owned(), local.to_path_buf())]);
    while let Some((dir, here)) = queue.pop_front() {
        let listing = client.dirs(&dir).await.map_err(|e| missing_or(e, &dir))?;
        out.dirs.push(PlannedDir { top, remote: dir.clone(), local: here.clone() });
        let mut taken = HashSet::new();
        for d in &listing.dirs {
            let name = unique_name(&mut taken, &d.name, true);
            queue.push_back((join(&dir, &d.name), here.join(name)));
        }
        for f in &listing.files {
            let name = unique_name(&mut taken, &f.name, false);
            out.files.push(PlannedFile {
                top,
                remote: join(&dir, &f.name),
                local: here.join(name),
                size: f.size,
                modified: f.modified,
            });
        }
        if out.files.len() > MAX_FILES {
            return Err(ExportError::TooMany);
        }
    }
    Ok(())
}

/// Tiến độ đọc được từ luồng khác: byte = `base` (tệp đã xong) + `current` (tệp đang tải).
#[derive(Debug, Default)]
pub struct Meter {
    base: AtomicU64,
    current: Arc<AtomicU64>,
    files: AtomicU64,
}

impl Meter {
    pub fn bytes(&self) -> u64 {
        self.base.load(Ordering::Relaxed) + self.current.load(Ordering::Relaxed)
    }

    pub fn files(&self) -> u64 {
        self.files.load(Ordering::Relaxed)
    }
}

/// Tải mọi tệp của `plan` vào thư mục phiên (tạo cả thư mục rỗng). Huỷ = bỏ (abort) future rồi [`discard`].
pub async fn download(client: &PhoneClient, plan: &ExportPlan, meter: &Meter) -> Result<(), ExportError> {
    tokio::fs::create_dir_all(&plan.root).await.map_err(ExportError::Local)?;
    for d in &plan.dirs {
        tokio::fs::create_dir_all(&d.local).await.map_err(ExportError::Local)?;
    }
    for f in &plan.files {
        if let Some(parent) = f.local.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(ExportError::Local)?;
        }
        let got = fetch_file(client, f, &meter.current).await?;
        meter.current.store(0, Ordering::Relaxed);
        meter.base.fetch_add(got, Ordering::Relaxed);
        meter.files.fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
}

fn part_of(local: &Path) -> PathBuf {
    let mut name = local.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".{PART_EXT}"));
    local.with_file_name(name)
}

/// Một tệp: tải tiếp được, thử lại khi lỗi tạm. Trả cỡ thật đã tải.
async fn fetch_file(client: &PhoneClient, f: &PlannedFile, current: &Arc<AtomicU64>) -> Result<u64, ExportError> {
    let part = part_of(&f.local);
    let mut etag: Option<String> = None;
    let mut fails = 0u32;
    loop {
        let before = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
        match fetch_once(client, f, &part, &mut etag, current).await {
            Ok(Step::Done(len)) => {
                tokio::fs::rename(&part, &f.local).await.map_err(ExportError::Local)?;
                set_mtime(&f.local, f.modified);
                return Ok(len);
            }
            Ok(Step::Again) => {}
            Err(e) if transient(&e) => {
                let now = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);
                fails = if now > before { 0 } else { fails + 1 };
                if fails >= MAX_ATTEMPTS {
                    return Err(e.into());
                }
                let wait = match &e {
                    ClientError::Busy { retry_after: Some(w) } | ClientError::RateLimited { retry_after: Some(w) } => {
                        *w
                    }
                    _ => Duration::from_secs(1 << fails.min(5)),
                };
                log::debug!("export {}: {e}, retry in {wait:?}", f.remote);
                tokio::time::sleep(wait).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

enum Step {
    Done(u64),
    /// Chưa đủ (thân ngắn, `416` lạ) — thử lại.
    Again,
}

fn transient(e: &ClientError) -> bool {
    match e {
        ClientError::Network(_)
        | ClientError::Busy { .. }
        | ClientError::RateLimited { .. }
        | ClientError::InvalidResponse(_) => true,
        ClientError::Server(c) => (500..600).contains(c),
        _ => false,
    }
}

async fn fetch_once(
    client: &PhoneClient,
    f: &PlannedFile,
    part: &Path,
    etag: &mut Option<String>,
    current: &Arc<AtomicU64>,
) -> Result<Step, ClientError> {
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(part)
        .await
        .map_err(ClientError::Local)?;
    let mut from = file.metadata().await.map_err(ClientError::Local)?.len();
    // Chưa có `ETag` (lần đầu trong tiến trình này) thì không tin phần đã có: tải lại từ đầu.
    if etag.is_none() && from > 0 {
        file.set_len(0).await.map_err(ClientError::Local)?;
        from = 0;
    }
    let resp = match client.fetch_range(&f.remote, from, etag.as_deref()).await {
        Ok(r) => r,
        Err(ClientError::RangeNotSatisfiable) if from > 0 && from == f.size => {
            current.store(from, Ordering::Relaxed);
            return Ok(Step::Done(from));
        }
        Err(ClientError::RangeNotSatisfiable) => {
            file.set_len(0).await.map_err(ClientError::Local)?;
            *etag = None;
            return Ok(Step::Again);
        }
        Err(e) => return Err(e),
    };
    if resp.partial && resp.start > from {
        // Khoảng khác khoảng đã xin, không lấp được ⇒ tải lại từ đầu.
        file.set_len(0).await.map_err(ClientError::Local)?;
        *etag = None;
        return Ok(Step::Again);
    }
    if !resp.partial || resp.start < from {
        // `200`: tệp đổi (ETag khác) hoặc server bỏ `Range` ⇒ ghi lại từ đầu; `206` lùi hơn ⇒ ghi đè từ đó.
        from = if resp.partial { resp.start } else { 0 };
        file.set_len(from).await.map_err(ClientError::Local)?;
    }
    file.seek(io::SeekFrom::Start(from)).await.map_err(ClientError::Local)?;
    // Giữ `ETag` ngay khi có phản hồi: đứt giữa thân thì lần sau vẫn tải tiếp được.
    etag.clone_from(&resp.etag);
    let total = resp.total;
    current.store(from, Ordering::Relaxed);
    let mut writer = MeteredWriter::new(file, current.clone());
    let result = resp.write_to(&mut writer).await;
    let mut file = writer.into_inner();
    let _ = file.flush().await;
    result?;
    file.sync_all().await.map_err(ClientError::Local)?;
    let have = current.load(Ordering::Relaxed);
    match total {
        Some(t) if t != have => Ok(Step::Again),
        _ => Ok(Step::Done(have)),
    }
}

fn set_mtime(path: &Path, modified_ms: i64) {
    if modified_ms <= 0 {
        return;
    }
    let at = UNIX_EPOCH + Duration::from_millis(modified_ms as u64);
    if let Err(e) = std::fs::File::options().write(true).open(path).and_then(|f| f.set_modified(at)) {
        log::debug!("export: set mtime {}: {e}", path.display());
    }
}

/// Mục cấp đầu đã bị trình quản lý tệp dời đi (Cắt → Dán). `None` khi chính thư mục phiên mất (bị dọn tay, không rõ
/// có phải dán không) ⇒ không xoá gì trên điện thoại.
pub fn moved_tops(plan: &ExportPlan) -> Option<Vec<usize>> {
    if !plan.root.is_dir() {
        return None;
    }
    Some((0..plan.tops.len()).filter(|&i| std::fs::symlink_metadata(&plan.tops[i].local).is_err()).collect())
}

/// Kết quả xoá trên điện thoại sau khi dán một mục Cắt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Removal {
    pub deleted: u64,
    /// Tệp đổi trên điện thoại sau khi tải / thư mục còn mục khác ⇒ giữ.
    pub kept: u64,
}

/// Xoá trên điện thoại những gì của mục cấp đầu `top` đã sang máy (§6.14). Lỗi mạng giữa chừng ⇒ `Err`, phần đã xoá
/// vẫn xoá; gọi lại an toàn (tệp không còn ⇒ bỏ qua).
pub async fn remove_moved(client: &PhoneClient, plan: &ExportPlan, top: usize) -> Result<Removal, ClientError> {
    let mut r = Removal::default();
    let mut by_parent: BTreeMap<&str, Vec<&PlannedFile>> = BTreeMap::new();
    for f in plan.files.iter().filter(|f| f.top == top) {
        by_parent.entry(parent_dir(&f.remote)).or_default().push(f);
    }
    for (parent, files) in by_parent {
        let listing = match client.dirs(parent).await {
            Ok(l) => l,
            Err(ClientError::NotFound) => continue,
            Err(e) => return Err(e),
        };
        for f in files {
            let name = last_segment(&f.remote);
            match listing.files.iter().find(|x| x.name == name) {
                Some(x) if x.size == f.size && x.modified == f.modified => match client.delete_entry(&f.remote).await {
                    Ok(()) => r.deleted += 1,
                    Err(ClientError::NotFound) => {}
                    Err(ClientError::Conflict) => r.kept += 1,
                    Err(e) => return Err(e),
                },
                Some(_) => r.kept += 1,
                None => {}
            }
        }
    }
    let mut dirs: Vec<&PlannedDir> = plan.dirs.iter().filter(|d| d.top == top).collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.remote.matches('/').count()));
    for d in dirs {
        match client.delete_dir_if_empty(&d.remote).await {
            Ok(()) => r.deleted += 1,
            Err(ClientError::NotFound) => {}
            Err(ClientError::Conflict) => r.kept += 1,
            Err(e) => return Err(e),
        }
    }
    Ok(r)
}

/// Xoá thư mục phiên (kể cả khi tải dở).
pub fn discard(root: &Path) {
    match std::fs::remove_dir_all(root) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => log::warn!("export: cannot remove {}: {e}", root.display()),
    }
}

/// Dọn các phiên dưới `base` sửa lần cuối quá `older_than` (lúc daemon khởi động).
pub fn clean_stale(base: &Path, older_than: Duration) {
    let Ok(rd) = std::fs::read_dir(base) else { return };
    let now = SystemTime::now();
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > older_than));
        if old {
            discard(&e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names_ignore_case_and_keep_extension() {
        let mut taken = HashSet::new();
        assert_eq!(unique_name(&mut taken, "a.mp4", false), "a.mp4");
        assert_eq!(unique_name(&mut taken, "A.mp4", false), "A (2).mp4");
        assert_eq!(unique_name(&mut taken, "a:b", true), "a_b");
        assert_eq!(unique_name(&mut taken, "a_b", true), "a_b (2)");
    }

    #[test]
    fn hidden_names_keep_their_dot() {
        assert_eq!(keep_hidden(".nomedia"), ".nomedia");
        assert_eq!(keep_hidden(".thu:mục"), ".thu_mục");
        assert_eq!(keep_hidden(".."), "download");
        assert_eq!(keep_hidden("a.mp4"), "a.mp4");
        let mut taken = HashSet::new();
        assert_eq!(unique_name(&mut taken, ".vdr", true), ".vdr");
        assert_eq!(unique_name(&mut taken, ".VDR", true), ".VDR (2)");
    }

    #[test]
    fn part_name_sits_next_to_the_file() {
        assert_eq!(part_of(Path::new("/x/a.mp4")), PathBuf::from("/x/a.mp4.ferry-part"));
    }

    #[test]
    fn moved_tops_needs_the_session_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("s");
        let plan = ExportPlan {
            root: root.clone(),
            tops: vec![
                PlannedTop { remote: "a".into(), local: root.join("a"), folder: false },
                PlannedTop { remote: "b".into(), local: root.join("b"), folder: true },
            ],
            files: Vec::new(),
            dirs: Vec::new(),
        };
        assert_eq!(moved_tops(&plan), None);
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::fs::write(root.join("a"), b"x").unwrap();
        assert_eq!(moved_tops(&plan), Some(vec![]));
        std::fs::remove_file(root.join("a")).unwrap();
        assert_eq!(moved_tops(&plan), Some(vec![0]));
    }
}
