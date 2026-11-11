//! Thư mục theo dõi (`PROTOCOL.md` mục 4 "Cài đặt", W-3) — gd1_13: tệp video mới **ổn định** trong thư mục người dùng chọn
//! ⇒ daemon thêm vào hàng đợi của điện thoại, giữ cây thư mục như copy/paste (`dir = <tên thư mục theo dõi>/<đường con>`).
//!
//! - Một luồng OS riêng ([`FolderWatcher`]) giữ `notify` và bộ đếm ổn định [`Stability`]; engine chỉ nhận [`Ready`] qua
//!   kênh tokio, không bao giờ chờ đĩa / `notify` trong vòng chính (bẫy "Engine một task"). Đăng ký theo dõi đệ quy
//!   (Linux: một inotify watch mỗi thư mục con) chạy trên luồng này, cây lớn không chặn engine.
//! - Rảnh không giữ CPU: luồng chặn ở `recv` khi không có tệp chờ; chỉ có đồng hồ khi có tệp đang chờ ổn định.
//! - "Ổn định" = cỡ + mtime không đổi [`STABLE_FOR`] kể từ sự kiện cuối, tệp mở được (Windows: mở với chia sẻ chỉ đọc —
//!   tiến trình đang ghi giữ quyền ghi thì hỏng ⇒ chờ tiếp).
//! - Không quét ổ (§2.8): chỉ cây của thư mục đã chọn, sâu tối đa [`MAX_DEPTH`] cấp con (điện thoại nhận 8 cấp `dir`,
//!   §6.9 — một cấp là tên thư mục theo dõi). Tệp có sẵn lúc bắt đầu theo dõi **không** gửi — chỉ tệp mới.
//! - Không thêm trùng ([`is_known`]): tệp đã có trong hàng đợi / "Đã gửi" (đường dẫn + cỡ + mtime; đổi tên ⇒ đường cũ mất,
//!   cỡ + mtime giữ ⇒ vẫn là tệp cũ) và tệp chính ferry vừa tải về từ điện thoại (thư mục tải về trùng thư mục theo dõi).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;

use crate::queue::UnixMs;
use crate::queue::batch::{self, BatchPlan, NewUpload, is_video, normalize_dir};
use crate::queue::store::{DownloadItem, UploadItem};

/// Tệp phải đứng yên (cỡ + mtime) chừng này kể từ sự kiện cuối mới vào hàng đợi.
pub const STABLE_FOR: Duration = Duration::from_secs(5);
/// Số cấp thư mục con tối đa dưới thư mục theo dõi (+1 cấp tên thư mục theo dõi = 8 cấp `dir`, PROTOCOL 6.9).
pub const MAX_DEPTH: usize = 7;
/// Trần tệp đang chờ ổn định — chép cả nghìn tệp một lúc vẫn chạy, quá thì bỏ (log) cho RAM có trần.
pub const MAX_PENDING: usize = 4096;

/// Lô tệp đã ổn định, gửi về engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    /// Thế hệ của bộ theo dõi đã sinh ra lô này — engine bỏ lô của bộ đã dừng (đổi thư mục giữa chừng).
    pub generation: u64,
    /// Thư mục theo dõi (đường dẫn thật, đã `canonicalize` như `notify` trả về).
    pub root: PathBuf,
    /// `dir` gốc phía điện thoại: tên thư mục theo dõi (rỗng khi theo dõi gốc ổ).
    pub remote_root: String,
    pub files: Vec<PathBuf>,
}

/// Cỡ + mtime một lần `stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub size: u64,
    pub mtime: Option<UnixMs>,
}

fn probe(path: &Path) -> Option<Probe> {
    let meta = fs::metadata(path).ok()?;
    meta.is_file().then(|| Probe { size: meta.len(), mtime: batch::mtime_ms(&meta) })
}

/// Tệp mở được để đọc mà không ai còn giữ quyền ghi? Windows: mở chia sẻ chỉ đọc ⇒ trình chép đang giữ handle ghi làm
/// lệnh mở hỏng (sharing violation). Unix không có khoá bắt buộc ⇒ chỉ cần mở được.
fn openable(path: &Path) -> bool {
    let mut opts = fs::OpenOptions::new();
    opts.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        opts.share_mode(FILE_SHARE_READ);
    }
    opts.open(path).is_ok()
}

// ------------------------------------------------------------ bộ đếm ổn định (thuần, test được)

struct Seen {
    last: Option<Probe>,
    since: Instant,
}

/// Tệp đang chờ ổn định. Không tự đọc đĩa: hàm `stat` / `check` truyền vào (test giả lập tệp đang chép).
pub struct Stability {
    stable_for: Duration,
    pending: HashMap<PathBuf, Seen>,
}

impl Stability {
    pub fn new(stable_for: Duration) -> Stability {
        Stability { stable_for, pending: HashMap::new() }
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Có sự kiện ở `path` (tạo / ghi / chuyển vào): đồng hồ chạy lại từ `now`. `false` ⇒ đầy, bỏ.
    pub fn observe(&mut self, path: &Path, probe: Option<Probe>, now: Instant) -> bool {
        if let Some(seen) = self.pending.get_mut(path) {
            seen.last = probe;
            seen.since = now;
            return true;
        }
        if self.pending.len() >= MAX_PENDING {
            return false;
        }
        self.pending.insert(path.to_owned(), Seen { last: probe, since: now });
        true
    }

    /// Đổi tên trong cây: tệp đang chờ đi theo tên mới (không tính lại từ đầu, không thành tệp mới). Tệp không chờ ⇒ bỏ
    /// qua — đổi tên tệp cũ không phải tệp mới.
    pub fn rename(&mut self, from: &Path, to: &Path) {
        if let Some(seen) = self.pending.remove(from) {
            self.pending.insert(to.to_owned(), seen);
        }
    }

    pub fn remove(&mut self, path: &Path) {
        self.pending.remove(path);
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.pending.contains_key(path)
    }

    /// Lúc sớm nhất cần soát lại; `None` ⇒ không có gì chờ (luồng ngủ tới sự kiện kế).
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().map(|s| s.since + self.stable_for).min()
    }

    /// Soát các tệp đã tới hạn: `check(path)` = cỡ + mtime hiện tại và có mở được không (`None` ⇒ mất / không còn là
    /// tệp). Đứng yên + mở được + không rỗng ⇒ trả ra (bỏ khỏi danh sách chờ); còn đổi / đang bị giữ ⇒ chờ thêm.
    pub fn poll(&mut self, now: Instant, mut check: impl FnMut(&Path) -> Option<(Probe, bool)>) -> Vec<PathBuf> {
        let due: Vec<PathBuf> =
            self.pending.iter().filter(|(_, s)| s.since + self.stable_for <= now).map(|(p, _)| p.clone()).collect();
        let mut ready = Vec::new();
        for path in due {
            let Some((now_probe, open)) = check(&path) else {
                self.pending.remove(&path);
                continue;
            };
            let seen = self.pending.get_mut(&path).expect("due path is pending");
            if seen.last == Some(now_probe) && open && now_probe.size > 0 {
                self.pending.remove(&path);
                ready.push(path);
            } else {
                seen.last = Some(now_probe);
                seen.since = now;
            }
        }
        ready.sort();
        ready
    }
}

// ------------------------------------------------------------ cây thư mục ⇄ `dir` phía điện thoại

/// `dir` gốc phía điện thoại cho thư mục theo dõi: tên của nó (giữ cây như copy/paste, W-3). Gốc ổ ⇒ rỗng.
pub fn remote_root(root: &Path) -> String {
    root.file_name().and_then(|n| n.to_str()).and_then(normalize_dir).unwrap_or_default()
}

/// Số cấp thư mục con giữa `root` và tệp `file` (tệp nằm thẳng trong `root` ⇒ 0). `None` ⇒ ngoài cây.
fn depth(root: &Path, file: &Path) -> Option<usize> {
    let rel = file.strip_prefix(root).ok()?;
    Some(rel.components().count().saturating_sub(1))
}

/// `dir` gửi kèm tệp `file` trong thư mục theo dõi `root`: `<remote_root>/<đường con>`. `None` ⇒ ngoài cây, quá sâu, hoặc
/// tên thư mục không gửi được.
pub fn remote_dir(root: &Path, remote_root: &str, file: &Path) -> Option<String> {
    if depth(root, file)? > MAX_DEPTH {
        return None;
    }
    let parent = file.parent()?.strip_prefix(root).ok()?;
    let mut dir = remote_root.to_owned();
    for c in parent.components() {
        let name = c.as_os_str().to_str()?;
        let name = normalize_dir(name).filter(|n| !n.is_empty())?;
        if !dir.is_empty() {
            dir.push('/');
        }
        dir.push_str(&name);
    }
    Some(dir)
}

/// Tệp đã biết, không thêm lại: (a) mục upload cùng cỡ + mtime, cùng đường dẫn hoặc đường cũ đã mất (đổi tên / dời trong
/// cây); (b) đích của một mục tải về (ferry vừa kéo từ điện thoại về — không đẩy ngược).
pub fn is_known(path: &Path, p: Probe, uploads: &[UploadItem], downloads: &[DownloadItem]) -> bool {
    let renamed_or_same = |u: &UploadItem| u.path == path || !u.path.exists();
    uploads.iter().any(|u| u.size == p.size && u.mtime == p.mtime && renamed_or_same(u))
        || downloads.iter().any(|d| d.dest == path)
}

/// Dựng một lô từ tệp đã ổn định, bỏ tệp đã biết / không còn / không đủ điều kiện. Nhãn lô = tên thư mục theo dõi.
pub fn plan(ready: &Ready, uploads: &[UploadItem], downloads: &[DownloadItem]) -> BatchPlan {
    let mut plan = BatchPlan {
        dest_dir: ready.remote_root.clone(),
        label: ready.root.file_name().map(|n| n.to_string_lossy().into_owned()),
        ..BatchPlan::default()
    };
    for path in &ready.files {
        let (Some(p), Some(dir), Some(name)) = (
            probe(path),
            remote_dir(&ready.root, &ready.remote_root, path),
            path.file_name().map(|n| n.to_string_lossy().into_owned()),
        ) else {
            continue;
        };
        if p.size == 0 || !is_video(path) || is_known(path, p, uploads, downloads) {
            continue;
        }
        plan.files.push(NewUpload { path: path.clone(), name, dir, size: p.size, mtime: p.mtime });
    }
    plan
}

// ------------------------------------------------------------ luồng theo dõi

enum Msg {
    Event(notify::Result<notify::Event>),
    Stop,
}

/// Bộ theo dõi đang chạy. Thả (drop) ⇒ luồng dừng, `notify` nhả watch; không chờ luồng (engine không chặn).
pub struct FolderWatcher {
    tx: std_mpsc::Sender<Msg>,
}

impl FolderWatcher {
    /// Đăng ký theo dõi `root` (đệ quy — Linux: một inotify watch mỗi thư mục con, cây lớn mất thời gian ⇒ **chặn**, gọi
    /// trong `spawn_blocking`) rồi chạy vòng sự kiện trên luồng mới. `Err` = chữ lỗi của OS (thư mục mất, không quyền,
    /// hết inotify watch) — màn cài đặt hiện kèm cách gỡ.
    pub fn start(
        root: PathBuf,
        generation: u64,
        stable_for: Duration,
        out: mpsc::UnboundedSender<Ready>,
    ) -> Result<FolderWatcher, String> {
        let remote_root = remote_root(&root);
        // `notify` trả đường dẫn thật (macOS: `/private/var/…`) ⇒ so cây theo bản canonical.
        let root = fs::canonicalize(&root).map_err(|e| e.to_string())?;
        let (tx, rx) = std_mpsc::channel();
        let events = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = events.send(Msg::Event(res));
        })
        .map_err(|e| e.to_string())?;
        watcher.watch(&root, RecursiveMode::Recursive).map_err(|e| e.to_string())?;
        log::info!("watch: watching {} (gen {generation})", root.display());
        let ctx = Loop { root, remote_root, generation, stable_for };
        std::thread::Builder::new()
            .name("ferry-watch".into())
            .spawn(move || ctx.run(watcher, rx, out))
            .map_err(|e| e.to_string())?;
        Ok(FolderWatcher { tx })
    }
}

impl Drop for FolderWatcher {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
    }
}

struct Loop {
    root: PathBuf,
    remote_root: String,
    generation: u64,
    stable_for: Duration,
}

impl Loop {
    fn run(self, watcher: notify::RecommendedWatcher, rx: std_mpsc::Receiver<Msg>, out: mpsc::UnboundedSender<Ready>) {
        let Loop { root, remote_root, generation, stable_for } = self;
        let mut tracker = Stability::new(stable_for);
        let mut last_from = None;
        // Tệp đã ổn định giữ lại tới khi không còn tệp nào chờ (hoặc giữ quá `stable_for`) ⇒ chép một loạt tệp ra một lô,
        // không phải mỗi tệp một lô.
        let mut held: Vec<PathBuf> = Vec::new();
        let mut held_since = Instant::now();
        loop {
            let flush_at = (!held.is_empty()).then(|| held_since + stable_for);
            let msg = match tracker.next_deadline().into_iter().chain(flush_at).min() {
                None => rx.recv().ok(),
                Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                    Ok(m) => Some(m),
                    Err(std_mpsc::RecvTimeoutError::Timeout) => {
                        let now = Instant::now();
                        let files = tracker.poll(now, |p| probe(p).map(|pr| (pr, openable(p))));
                        if held.is_empty() {
                            held_since = now;
                        }
                        held.extend(files);
                        if !held.is_empty() && (tracker.is_empty() || now >= held_since + stable_for) {
                            let mut files = std::mem::take(&mut held);
                            files.sort();
                            files.dedup();
                            log::info!("watch: {} new file(s) settled", files.len());
                            let ready =
                                Ready { generation, root: root.clone(), remote_root: remote_root.clone(), files };
                            if out.send(ready).is_err() {
                                break;
                            }
                        }
                        continue;
                    }
                    Err(std_mpsc::RecvTimeoutError::Disconnected) => None,
                },
            };
            match msg {
                None | Some(Msg::Stop) => break,
                Some(Msg::Event(Ok(ev))) => on_event(&root, &mut tracker, &mut last_from, ev),
                Some(Msg::Event(Err(e))) => log::warn!("watch: {e}"),
            }
            if out.is_closed() {
                break;
            }
        }
        drop(watcher);
        log::info!("watch: stopped {} (gen {generation})", root.display());
    }
}

/// Một sự kiện `notify` ⇒ cập nhật danh sách chờ. `last_from` = đường của sự kiện `Name(From)` gần nhất + nó có đang chờ
/// không: inotify phát `From` → `To` → `Both` cho một lần đổi tên trong cây, `To` đã đưa tên mới vào danh sách chờ trước
/// khi `Both` cho biết đó chỉ là đổi tên.
fn on_event(root: &Path, tracker: &mut Stability, last_from: &mut Option<(PathBuf, bool)>, ev: notify::Event) {
    let now = Instant::now();
    match ev.kind {
        // Đổi tên trong cây: tệp đang chờ đi theo tên mới; tệp cũ (không chờ) đổi tên không thành tệp mới (C2).
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if ev.paths.len() == 2 => {
            let (from, to) = (&ev.paths[0], &ev.paths[1]);
            match last_from.take() {
                Some((p, was_pending)) if p == *from => {
                    if !was_pending {
                        tracker.remove(to);
                    }
                }
                // Nền chỉ phát `Both` (không `From` / `To` riêng).
                _ => tracker.rename(from, to),
            }
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
            for p in &ev.paths {
                *last_from = Some((p.clone(), tracker.contains(p)));
                tracker.remove(p);
            }
        }
        EventKind::Remove(_) => {
            for p in &ev.paths {
                tracker.remove(p);
            }
        }
        // Siêu dữ liệu (quyền, giờ truy cập) không làm tệp cũ thành mới — chỉ làm mới đồng hồ của tệp đang chờ.
        EventKind::Modify(ModifyKind::Metadata(_)) => {
            for p in &ev.paths {
                if tracker.contains(p) {
                    tracker.observe(p, probe(p), now);
                }
            }
        }
        // Tạo, ghi, chuyển vào (`To`; macOS `Any` cho cả hai chiều — đường không còn thì `consider` bỏ).
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any => {
            for p in &ev.paths {
                consider(root, tracker, p, now);
            }
        }
        EventKind::Access(_) | EventKind::Other => {}
    }
}

/// Đường dẫn mới xuất hiện: tệp video trong cây ⇒ chờ ổn định; thư mục (dời cả thư mục vào — inotify không báo từng
/// tệp bên trong) ⇒ duyệt có trần độ sâu.
fn consider(root: &Path, tracker: &mut Stability, path: &Path, now: Instant) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        tracker.remove(path);
        return;
    };
    if meta.is_dir() {
        if depth(root, path).is_some_and(|d| d < MAX_DEPTH) {
            walk(root, tracker, path, now);
        }
        return;
    }
    // Symlink: theo khi trỏ tới tệp (như `plan_batch`), bỏ khi trỏ tới thư mục.
    let Some(p) = probe(path) else { return };
    if !is_video(path) || depth(root, path).is_none_or(|d| d > MAX_DEPTH) {
        return;
    }
    if !tracker.observe(path, Some(p), now) {
        log::warn!("watch: too many pending files, skipped {}", path.display());
    }
}

fn walk(root: &Path, tracker: &mut Stability, dir: &Path, now: Instant) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.filter_map(Result::ok) {
        let path = entry.path();
        match entry.file_type() {
            Ok(t) if t.is_dir() => {
                if depth(root, &path).is_some_and(|d| d < MAX_DEPTH) {
                    walk(root, tracker, &path, now);
                }
            }
            Ok(_) => consider(root, tracker, &path, now),
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceId;
    use crate::queue::{ItemId, UploadState};

    const S: Duration = STABLE_FOR;

    fn pr(size: u64, mtime: UnixMs) -> Probe {
        Probe { size, mtime: Some(mtime) }
    }

    /// C1: tệp đang chép (cỡ tăng dần, sự kiện ghi liên tục) không ra cho tới khi đứng yên đủ 5 s sau sự kiện cuối.
    #[test]
    fn c1_growing_file_waits_until_copy_finishes() {
        let t0 = Instant::now();
        let f = Path::new("/w/big.mp4");
        let mut st = Stability::new(S);
        let gb = 1u64 << 30;
        // Chép 2 GB trong 20 s: sự kiện ghi mỗi giây, cỡ tăng.
        for sec in 0..=20u64 {
            let size = 2 * gb * sec / 20;
            st.observe(f, Some(pr(size, sec as UnixMs)), t0 + Duration::from_secs(sec));
            // Không lần soát nào giữa chừng cho ra tệp (đồng hồ luôn chạy lại).
            let out = st.poll(t0 + Duration::from_secs(sec), |_| Some((pr(size, sec as UnixMs), true)));
            assert!(out.is_empty(), "sec {sec}");
        }
        let done = pr(2 * gb, 20);
        assert_eq!(st.next_deadline(), Some(t0 + Duration::from_secs(20) + S));
        // 4,9 s sau sự kiện cuối: chưa tới hạn.
        assert!(st.poll(t0 + Duration::from_millis(24_900), |_| Some((done, true))).is_empty());
        // Đủ 5 s, cỡ + mtime y như sự kiện cuối, mở được ⇒ ra.
        assert_eq!(st.poll(t0 + Duration::from_secs(25), |_| Some((done, true))), vec![f.to_owned()]);
        assert!(st.is_empty() && st.next_deadline().is_none(), "idle: no timer");
    }

    /// C1 (Windows): cỡ cấp trước đủ 2 GB nhưng trình chép còn giữ handle ghi ⇒ chờ tới khi mở được.
    #[test]
    fn c1_locked_file_waits() {
        let t0 = Instant::now();
        let f = Path::new("/w/pre.mkv");
        let mut st = Stability::new(S);
        let p = pr(2 << 30, 1);
        st.observe(f, Some(p), t0);
        assert!(st.poll(t0 + S, |_| Some((p, false))).is_empty());
        assert_eq!(st.next_deadline(), Some(t0 + S + S));
        assert_eq!(st.poll(t0 + S + S, |_| Some((p, true))), vec![f.to_owned()]);
    }

    /// Cỡ đổi mà không có sự kiện (ổ mạng, FSEvents gộp) ⇒ soát thấy khác, chờ thêm một nhịp.
    #[test]
    fn change_without_event_restarts_clock() {
        let t0 = Instant::now();
        let f = Path::new("/w/a.mp4");
        let mut st = Stability::new(S);
        st.observe(f, Some(pr(10, 1)), t0);
        assert!(st.poll(t0 + S, |_| Some((pr(20, 2), true))).is_empty());
        assert_eq!(st.poll(t0 + S + S, |_| Some((pr(20, 2), true))), vec![f.to_owned()]);
    }

    #[test]
    fn empty_or_vanished_file_is_not_ready() {
        let t0 = Instant::now();
        let mut st = Stability::new(S);
        st.observe(Path::new("/w/zero.mp4"), Some(pr(0, 1)), t0);
        st.observe(Path::new("/w/gone.mp4"), Some(pr(5, 1)), t0);
        let out = st.poll(t0 + S, |p| p.ends_with("zero.mp4").then_some((pr(0, 1), true)));
        assert!(out.is_empty());
        assert_eq!(st.len(), 1, "vanished file dropped, empty file keeps waiting");
    }

    /// C2: đổi tên tệp đang chờ ⇒ một mục theo tên mới; đổi tên tệp không chờ (tệp cũ) ⇒ không gì.
    #[test]
    fn c2_rename_does_not_duplicate() {
        let t0 = Instant::now();
        let mut st = Stability::new(S);
        let (a, b) = (Path::new("/w/a.mp4"), Path::new("/w/b.mp4"));
        st.observe(a, Some(pr(5, 1)), t0);
        st.rename(a, b);
        st.rename(Path::new("/w/old.mp4"), Path::new("/w/old2.mp4"));
        assert_eq!(st.poll(t0 + S, |_| Some((pr(5, 1), true))), vec![b.to_owned()]);
    }

    fn upload(path: &Path, size: u64, mtime: Option<UnixMs>) -> UploadItem {
        UploadItem {
            id: 1 as ItemId,
            device_id: DeviceId("p".into()),
            batch_id: None,
            path: path.to_owned(),
            name: "x".into(),
            dir: String::new(),
            size,
            mtime,
            state: UploadState::Queued,
            failures: 0,
            created: 0,
            updated: 0,
        }
    }

    /// C2: tệp đã gửi rồi bị đổi tên (đường cũ mất, cỡ + mtime giữ) ⇒ đã biết; tệp khác cùng cỡ mà đường cũ còn ⇒ mới.
    #[test]
    fn c2_known_after_rename_or_already_queued() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new, other) = (dir.path().join("a.mp4"), dir.path().join("b.mp4"), dir.path().join("c.mp4"));
        fs::write(&new, b"12345").unwrap();
        fs::write(&other, b"12345").unwrap();
        let p = probe(&new).unwrap();
        // Mục cũ trỏ `a.mp4` (đã đổi tên thành `b.mp4`, không còn).
        assert!(is_known(&new, p, &[upload(&old, 5, p.mtime)], &[]));
        // Cùng đường dẫn, cùng cỡ + mtime ⇒ đã trong hàng đợi.
        assert!(is_known(&new, p, &[upload(&new, 5, p.mtime)], &[]));
        // Tệp khác cùng cỡ + mtime nhưng tệp của mục cũ vẫn còn ⇒ tệp mới thật.
        assert!(!is_known(&new, p, &[upload(&other, 5, p.mtime)], &[]));
        // Cùng đường mà đã sửa (cỡ khác) ⇒ tệp mới.
        assert!(!is_known(&new, p, &[upload(&new, 4, p.mtime)], &[]));
    }

    /// C-W3: tệp ở thư mục con ⇒ `dir = <tên thư mục theo dõi>/<đường con>`; quá sâu / ngoài cây ⇒ bỏ.
    #[test]
    fn cw3_remote_dir_keeps_tree() {
        let root = Path::new("/home/u/Gửi điện thoại");
        let rr = remote_root(root);
        assert_eq!(rr, "Gửi điện thoại");
        assert_eq!(remote_dir(root, &rr, &root.join("a.mp4")).as_deref(), Some("Gửi điện thoại"));
        assert_eq!(remote_dir(root, &rr, &root.join("Phim/2026/a.mp4")).as_deref(), Some("Gửi điện thoại/Phim/2026"));
        assert_eq!(remote_dir(root, &rr, Path::new("/elsewhere/a.mp4")), None);
        let deep: PathBuf = (0..=MAX_DEPTH).fold(root.to_owned(), |p, i| p.join(i.to_string())).join("a.mp4");
        assert_eq!(remote_dir(root, &rr, &deep), None);
        let ok: PathBuf = (0..MAX_DEPTH).fold(root.to_owned(), |p, i| p.join(i.to_string())).join("a.mp4");
        assert_eq!(remote_dir(root, &rr, &ok).unwrap().split('/').count(), 8, "8 dir levels max (PROTOCOL 6.9)");
        assert_eq!(remote_root(Path::new("/")), "");
    }

    #[test]
    fn plan_skips_known_downloaded_and_non_video() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Gửi");
        fs::create_dir_all(root.join("Sub")).unwrap();
        let (a, b, c, d) = (root.join("Sub/a.mp4"), root.join("b.mkv"), root.join("c.txt"), root.join("d.mp4"));
        for f in [&a, &b, &c, &d] {
            fs::write(f, b"data").unwrap();
        }
        let ready = Ready {
            generation: 1,
            root: root.clone(),
            remote_root: "Gửi".into(),
            files: vec![a.clone(), b, c, d.clone()],
        };
        let dl = DownloadItem {
            id: 2,
            device_id: DeviceId("p".into()),
            remote_id: "r".into(),
            name: "d.mp4".into(),
            remote_dir: None,
            size: 4,
            dest: d,
            state: crate::queue::DownloadState::Done,
            failures: 0,
            created: 0,
            updated: 0,
        };
        let pb = plan(&ready, &[upload(&root.join("b.mkv"), 4, probe(&root.join("b.mkv")).unwrap().mtime)], &[dl]);
        assert_eq!(pb.files.len(), 1);
        assert_eq!(pb.files[0].path, a);
        assert_eq!(pb.files[0].dir, "Gửi/Sub");
        assert_eq!(pb.dest_dir, "Gửi");
        assert_eq!(pb.label.as_deref(), Some("Gửi"));
    }

    fn recv(rx: &mut mpsc::UnboundedReceiver<Ready>, within: Duration) -> Option<Ready> {
        let end = Instant::now() + within;
        while Instant::now() < end {
            if let Ok(r) = rx.try_recv() {
                return Some(r);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    /// Chạy thật với `notify`: tệp có sẵn không gửi; tệp chép từng khúc chỉ ra sau khi chép xong; tệp ở thư mục con và
    /// thư mục dời vào đều ra; đổi tên tệp cũ không ra.
    #[test]
    fn watcher_end_to_end() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Watch");
        fs::create_dir_all(root.join("Sub")).unwrap();
        fs::write(root.join("old.mp4"), b"old").unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let stable = Duration::from_millis(400);
        let w = FolderWatcher::start(root.clone(), 7, stable, tx).unwrap();
        assert!(FolderWatcher::start(dir.path().join("missing"), 8, stable, mpsc::unbounded_channel().0).is_err());
        std::thread::sleep(Duration::from_millis(300)); // đăng ký watch xong

        // Chép từng khúc, mỗi khúc cách 150 ms (< ngưỡng) ⇒ không ra giữa chừng.
        let big = root.join("Sub/big.mp4");
        let mut f = fs::File::create(&big).unwrap();
        for _ in 0..6 {
            f.write_all(&[7u8; 64 * 1024]).unwrap();
            f.flush().unwrap();
            assert!(rx.try_recv().is_err(), "not ready while copying");
            std::thread::sleep(Duration::from_millis(150));
        }
        drop(f);
        let r = recv(&mut rx, Duration::from_secs(5)).expect("ready after copy finishes");
        assert_eq!(r.generation, 7);
        assert_eq!(r.remote_root, "Watch");
        assert_eq!(r.files.len(), 1);
        assert!(r.files[0].ends_with("Sub/big.mp4"));
        assert_eq!(fs::metadata(&r.files[0]).unwrap().len(), 6 * 64 * 1024);

        // Đổi tên tệp cũ (có sẵn) ⇒ không ra (Linux: cặp `Both`; nền khác: `plan` + `is_known` chặn ở engine).
        fs::rename(root.join("old.mp4"), root.join("old-renamed.mp4")).unwrap();
        // Dời cả thư mục vào cây.
        let outside = dir.path().join("Trip");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("t.mov"), b"trip").unwrap();
        fs::rename(&outside, root.join("Trip")).unwrap();
        let r = recv(&mut rx, Duration::from_secs(5)).expect("moved-in folder");
        let names: Vec<_> = r.files.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        if cfg!(target_os = "linux") {
            assert_eq!(names, vec!["t.mov"], "rename of old file is not new (C2)");
        } else {
            assert!(names.contains(&"t.mov".to_owned()));
        }
        let pb = plan(&r, &[], &[]);
        let trip = pb.files.iter().find(|f| f.name == "t.mov").unwrap();
        assert_eq!(trip.dir, "Watch/Trip");

        // Chép một loạt ⇒ một lô.
        for i in 0..5 {
            fs::write(root.join(format!("n{i}.mp4")), b"n").unwrap();
            std::thread::sleep(Duration::from_millis(50));
        }
        let r = recv(&mut rx, Duration::from_secs(5)).expect("bulk copy");
        assert_eq!(r.files.len(), 5, "bulk copy is one batch");

        drop(w);
        std::thread::sleep(Duration::from_millis(200));
        fs::write(root.join("after.mp4"), b"x").unwrap();
        assert!(recv(&mut rx, stable * 3).is_none(), "stopped watcher sends nothing");
    }
}
