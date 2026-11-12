//! Tải về nối tiếp của một điện thoại (`PROTOCOL.md` 5, 6.4) — task gd1_10.
//!
//! Không có runner riêng: mục tải về nằm chung hàng đợi thiết bị với upload (một tệp một lúc, PROTOCOL 2.8) và chạy
//! trong cùng vòng của [`UploadRunner`] — vòng chọn đầu hàng theo `id` của bảng `items` (thả trước chạy trước).
//!
//! - **`.part`**: ghi dần ra `<đích>.<id>.part` cạnh tệp đích (cùng thư mục ⇒ đổi tên là nguyên tử, cùng ổ). `<id>` của
//!   mục để hai mục cùng tên không giẫm nhau. Không đọc cả tệp vào RAM: thân `GET` chảy qua [`MeteredWriter`].
//! - **Offset thật = cỡ `.part`** (byte ghi tuần tự nên mọi byte trong tệp đều đúng); offset trong store chỉ là bản
//!   lưu theo nhịp tiến độ. Effect `Get { offset: 0 }` (mục mới, `416`, tệp đã đổi) ⇒ cắt `.part` về 0.
//! - **Tiếp**: `Range: bytes=<cỡ .part>-` + `If-Range: <etag>`; `206` ⇒ ghi tiếp; `200` ⇒ tệp trên điện thoại đã đổi
//!   (ETag khác) hoặc server bỏ `Range` ⇒ cắt về 0, ghi lại từ đầu; `416` ⇒ `.part` đủ cỡ thì xong, không thì tải lại.
//! - **Xong**: tên từ `Content-Disposition` (`filename*`), lọc ký tự cấm ([`sanitize_file_name`]), trùng tệp có sẵn
//!   ⇒ thêm ` (2)`, ` (3)` … ([`unique_path`]), đổi tên `.part` ⇒ tệp đích, ghi đường dẫn thật vào store.
//! - **Lỗi** (PROTOCOL 5): mạng / `503` / `429` / thân ngắn ⇒ `Waiting` (backoff 2 s … 60 s, `Retry-After` thắng);
//!   `401` ⇒ dừng hàng của thiết bị tới khi ghép lại (như upload); `404` / `4xx` khác ⇒ Failed, xoá `.part`; không ghi
//!   được đĩa ⇒ Failed (giữ `.part` để "Thử lại" chạy tiếp).

use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::meter::{MeteredWriter, SpeedMeter};
use super::upload::{Flow, UploadRunner};
use super::{Command, ItemAction, Link, SharedStore, TransferEvent};
use crate::client::ClientError;
use crate::device::DeviceId;
use crate::queue::store::DownloadItem;
use crate::queue::{DownloadEffect, DownloadEvent, DownloadState, ItemId, now_ms};

/// Đuôi tệp đang tải.
pub const PART_EXT: &str = "part";
/// Tên tệp tối đa (byte UTF-8) — trần chung của NTFS / APFS / ext4.
const NAME_MAX_BYTES: usize = 255;
/// Tên khi điện thoại không cho tên dùng được.
const FALLBACK_NAME: &str = "download";
/// Bộ đếm tiến độ chưa có offset hợp lệ (chưa nhận header) — nhịp tiến độ không ghi gì.
const NOT_STARTED: u64 = u64::MAX;

/// Tệp `.part` của một mục: `<tên đích>.<id>.part` cạnh tệp đích.
pub fn part_path(dest: &Path, id: ItemId) -> PathBuf {
    let name = dest.file_name().map_or_else(|| FALLBACK_NAME.into(), |n| n.to_string_lossy().into_owned());
    dest.with_file_name(format!("{name}.{id}.{PART_EXT}"))
}

/// Tệp đích dự kiến khi xếp hàng một video của `list` (gd1_11 gọi): `<downloadDir>/<tên đã làm sạch>`. Tên cuối cùng
/// chốt lúc xong (theo `Content-Disposition`, tránh trùng).
pub fn default_dest(download_dir: &Path, remote_name: &str) -> PathBuf {
    download_dir.join(sanitize_file_name(remote_name))
}

/// Tên tệp hợp lệ trên mọi OS (cùng luật cho cả ba: tệp hay bị chép sang ổ Windows / exFAT):
/// `\ / : * ? " < > |` và ký tự điều khiển ⇒ `_`; bỏ khoảng trắng hai đầu, `.` đầu (tệp ẩn, `..`), `.` / khoảng trắng
/// cuối (Windows cắt mất); tên thiết bị Windows (`CON`, `NUL`, `COM1` …) ⇒ thêm `_` trước; quá 255 byte ⇒ cắt phần
/// tên, giữ đuôi. Rỗng ⇒ `download`.
pub fn sanitize_file_name(raw: &str) -> String {
    let cleaned: String =
        raw.chars()
            .map(|c| {
                if c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                    '_'
                } else {
                    c
                }
            })
            .collect();
    let mut name = cleaned.trim().trim_start_matches('.').trim_end_matches(['.', ' ']).trim().to_owned();
    if name.is_empty() {
        return FALLBACK_NAME.into();
    }
    if is_windows_reserved(&name) {
        name.insert(0, '_');
    }
    truncate_name(&name, NAME_MAX_BYTES)
}

fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

/// `(phần tên, đuôi có dấu chấm)`; đuôi chỉ khi phần tên không rỗng và đuôi ngắn (≤ 16 byte).
fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 && name.len() - i <= 16 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

fn truncate_name(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_owned();
    }
    let (stem, ext) = split_ext(name);
    let mut cut = max.saturating_sub(ext.len()).min(stem.len());
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{ext}", stem[..cut].trim_end())
}

/// `dir/name` nếu chưa có, không thì `dir/<tên> (2).<đuôi>`, ` (3)` … (như Explorer / Finder khi tải trùng).
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let free = |p: &Path| matches!(std::fs::symlink_metadata(p), Err(e) if e.kind() == io::ErrorKind::NotFound);
    let first = dir.join(name);
    if free(&first) {
        return first;
    }
    let (stem, ext) = split_ext(name);
    for n in 2..10_000u32 {
        let suffix = format!(" ({n})");
        let stem = truncate_name(stem, NAME_MAX_BYTES - ext.len() - suffix.len());
        let candidate = dir.join(format!("{stem}{suffix}{ext}"));
        if free(&candidate) {
            return candidate;
        }
    }
    dir.join(format!("{stem} ({}){ext}", uuid::Uuid::new_v4().simple()))
}

/// Xoá `.part` (huỷ / video không còn trên điện thoại). Không có thì thôi.
pub fn discard_partial(item: &DownloadItem) {
    let part = part_path(&item.dest, item.id);
    match std::fs::remove_file(&part) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => log::warn!("download {}: cannot remove partial file: {e}", item.id),
    }
}

// ---------------------------------------------------------------- một request GET

/// Kết quả một `GET` (sự kiện cuối cho máy trạng thái; `Responded` / `Progress` đã ghi store giữa chừng).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Fetched {
    Event(DownloadEvent),
    /// `401` — không có sự kiện của máy tải về; runner dừng hàng như upload.
    Unauthorized,
}

/// Những gì một `GET` cần, mượn từ runner (tách khỏi `commands` để `select!` song song).
struct FetchCx<'a> {
    device: &'a DeviceId,
    link: &'a Link,
    store: &'a SharedStore,
    events: &'a tokio::sync::mpsc::UnboundedSender<TransferEvent>,
    /// Offset tuyệt đối đã ghi vào `.part`; [`NOT_STARTED`] tới khi có header.
    counter: Arc<AtomicU64>,
}

impl FetchCx<'_> {
    fn apply(&self, id: ItemId, ev: &DownloadEvent) {
        let mut s = self.store.lock().unwrap_or_else(|p| p.into_inner());
        if let Err(e) = s.apply_download(id, ev, now_ms()) {
            log::error!("download {id}: queue store: {e}");
        }
    }

    async fn fetch(&self, item: &DownloadItem, offset: u64, etag: Option<String>) -> Fetched {
        use DownloadEvent as Ev;
        let part = part_path(&item.dest, item.id);
        if let Some(dir) = part.parent()
            && let Err(e) = tokio::fs::create_dir_all(dir).await
        {
            return write_failed(item, &e);
        }
        let open = tokio::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&part).await;
        let mut file = match open {
            Ok(f) => f,
            Err(e) => return write_failed(item, &e),
        };
        let mut from = if offset == 0 {
            if let Err(e) = file.set_len(0).await {
                return write_failed(item, &e);
            }
            0
        } else {
            match file.metadata().await {
                Ok(m) => m.len(),
                Err(e) => return write_failed(item, &e),
            }
        };
        let if_range = etag.as_deref().filter(|_| from > 0);
        let resp = match self.link.client.file_get_range(&item.remote_id, from, if_range).await {
            Ok(r) => r,
            // `416`: đã có đủ (lần trước đứt ngay sau byte cuối / mất phản hồi) ⇒ xong; không ⇒ tải lại từ 0.
            Err(ClientError::RangeNotSatisfiable) if from > 0 && from == item.size => {
                drop(file);
                return self.finish(item, &part, None).await;
            }
            Err(ClientError::RangeNotSatisfiable) => return Fetched::Event(Ev::RangeInvalid),
            Err(e) => return self.map_error(e),
        };
        if resp.partial && resp.start != from {
            // Server trả khoảng khác khoảng đã xin: lùi được thì ghi đè từ đó, không thì tải lại.
            if resp.start > from {
                log::warn!("download {}: 206 from {} but asked {from}", item.id, resp.start);
                return Fetched::Event(Ev::RangeInvalid);
            }
            from = resp.start;
            if let Err(e) = file.set_len(from).await {
                return write_failed(item, &e);
            }
        }
        if !resp.partial && from > 0 {
            log::debug!("download {}: file changed on phone (200 to If-Range), restarting", item.id);
            from = 0;
            if let Err(e) = file.set_len(0).await {
                return write_failed(item, &e);
            }
        }
        self.apply(item.id, &Ev::Responded { etag: resp.etag.clone(), full: !resp.partial });
        if from > 0 {
            // `Responded { full: false }` giữ offset cũ của store — đưa về cỡ `.part` thật.
            self.apply(item.id, &Ev::Progress { offset: from });
        }
        if let Err(e) = file.seek(SeekFrom::Start(from)).await {
            return write_failed(item, &e);
        }
        let total = resp.total;
        let file_name = resp.file_name.clone();
        self.counter.store(from, Ordering::Relaxed);
        let mut writer = MeteredWriter::new(file, self.counter.clone());
        let result = resp.write_to(&mut writer).await;
        let mut file = writer.into_inner();
        let have = self.counter.load(Ordering::Relaxed);
        if let Err(e) = result {
            // Byte đã ghi vẫn trong `.part` — lưu offset để lần sau (kể cả sau khi mở lại app) tiếp đúng chỗ.
            let _ = file.flush().await;
            self.apply(item.id, &Ev::Progress { offset: have });
            return match e {
                ClientError::Local(err) => write_failed(item, &err),
                e => self.map_error(e),
            };
        }
        if let Err(e) = file.sync_all().await {
            return write_failed(item, &e);
        }
        drop(file);
        self.apply(item.id, &Ev::Progress { offset: have });
        if let Some(t) = total
            && t != have
        {
            log::debug!("download {}: have {have} of {t}, retrying", item.id);
            return Fetched::Event(Ev::Transient { retry_after: None });
        }
        self.finish(item, &part, file_name).await
    }

    /// `.part` đủ ⇒ đổi tên thành tệp đích (tên của điện thoại, làm sạch, tránh trùng) và ghi vào store.
    async fn finish(&self, item: &DownloadItem, part: &Path, remote_name: Option<String>) -> Fetched {
        let dir = item.dest.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let name = remote_name.as_deref().map_or_else(
            || item.dest.file_name().map_or_else(|| sanitize_file_name(&item.name), |n| n.to_string_lossy().into()),
            sanitize_file_name,
        );
        let dest = unique_path(&dir, &name);
        if let Err(e) = tokio::fs::rename(part, &dest).await {
            return write_failed(item, &e);
        }
        let mut s = self.store.lock().unwrap_or_else(|p| p.into_inner());
        if let Err(e) = s.set_download_dest(item.id, &dest) {
            log::error!("download {}: queue store: {e}", item.id);
        }
        Fetched::Event(DownloadEvent::Finished)
    }

    /// Lỗi ⇒ sự kiện PROTOCOL 5. Mã lạ `4xx` ⇒ Failed (không vòng thử lại mãi); `5xx` ⇒ thử lại.
    fn map_error(&self, e: ClientError) -> Fetched {
        use DownloadEvent as Ev;
        let ev = match e {
            ClientError::Network(err) => {
                log::debug!("download {}: network at {}: {err}", self.device, self.link.addr());
                let _ = self
                    .events
                    .send(TransferEvent::ConnectionLost { device: self.device.clone(), addr: self.link.addr() });
                Ev::Transient { retry_after: None }
            }
            ClientError::Unauthorized => return Fetched::Unauthorized,
            ClientError::NotFound => Ev::Gone,
            ClientError::Busy { retry_after } | ClientError::RateLimited { retry_after } => {
                Ev::Transient { retry_after }
            }
            ClientError::Local(_) => Ev::WriteFailed,
            ClientError::RangeNotSatisfiable => Ev::RangeInvalid,
            ClientError::Server(code) if (500..600).contains(&code) => Ev::Transient { retry_after: None },
            ClientError::InvalidResponse(m) => {
                log::warn!("download {}: invalid response: {m}", self.device);
                Ev::Transient { retry_after: None }
            }
            ClientError::InsufficientStorage => Ev::Transient { retry_after: None },
            other => {
                log::warn!("download {}: unexpected {other}", self.device);
                Ev::Gone
            }
        };
        Fetched::Event(ev)
    }
}

fn write_failed(item: &DownloadItem, e: &io::Error) -> Fetched {
    log::warn!("download {}: local write: {e}", item.id);
    Fetched::Event(DownloadEvent::WriteFailed)
}

// ---------------------------------------------------------------- trong vòng của runner

enum InFlight {
    Done(Fetched),
    Interrupt(Command),
    Stop,
}

/// Lệnh này có phải cắt `GET` đang bay của mục `id`?
fn touches(cmd: &Command, id: ItemId) -> bool {
    matches!(cmd, Command::Item { id: i, action: ItemAction::Pause | ItemAction::Cancel } if *i == id)
}

fn user_event(a: ItemAction) -> DownloadEvent {
    match a {
        ItemAction::Pause => DownloadEvent::Pause,
        ItemAction::Resume => DownloadEvent::Resume,
        ItemAction::Retry => DownloadEvent::Retry,
        ItemAction::Cancel => DownloadEvent::Cancel,
    }
}

impl UploadRunner {
    /// Một mục tải về tới khi xong, chờ, tạm dừng hoặc thất bại.
    pub(super) async fn run_download(&mut self, item: DownloadItem) -> Flow {
        let id = item.id;
        let mut effect = match &item.state {
            DownloadState::Queued => self.apply_download(id, &DownloadEvent::Start),
            DownloadState::Waiting { .. } => self.apply_download(id, &DownloadEvent::WakeUp),
            // Runner dựng lại trong cùng tiến trình: chạy tiếp từ offset đã lưu.
            DownloadState::Fetching { offset, etag } => {
                Some(DownloadEffect::Get { offset: *offset, etag: etag.clone() })
            }
            _ => None,
        };
        let mut meter = SpeedMeter::new();
        let mut deferred = Vec::new();
        let flow = loop {
            let (offset, etag) = match effect.take() {
                Some(DownloadEffect::Get { offset, etag }) => (offset, etag),
                Some(DownloadEffect::DiscardPartial) => {
                    if let Some(item) = self.with_store(|s| s.download(id)).flatten() {
                        discard_partial(&item);
                    }
                    break Flow::Continue;
                }
                // `Truncate` chỉ sinh từ `Responded`, `fetch` đã tự cắt `.part`.
                None | Some(DownloadEffect::None | DownloadEffect::Wait { .. } | DownloadEffect::Truncate) => {
                    break Flow::Continue;
                }
            };
            let Some(item) = self.with_store(|s| s.download(id)).flatten() else { break Flow::Continue };
            let Some(link) = self.current_link() else {
                effect = self.apply_download(id, &DownloadEvent::Transient { retry_after: None });
                continue;
            };
            match self.download_in_flight(&item, offset, etag, &link, &mut meter, &mut deferred).await {
                InFlight::Done(Fetched::Event(ev)) => {
                    effect = self.apply_download(id, &ev);
                    if matches!(effect, Some(DownloadEffect::None))
                        && self.with_store(|s| s.download(id)).flatten().is_some_and(|i| i.state.is_active())
                    {
                        log::warn!("download {id}: event {ev:?} ignored in active state, backing off");
                        effect = self.apply_download(id, &DownloadEvent::Transient { retry_after: None });
                    }
                }
                InFlight::Done(Fetched::Unauthorized) => {
                    self.repair_blocked = true;
                    self.emit(TransferEvent::DeviceNeedsRepair { device: self.device.clone() });
                    effect = self.apply_download(id, &DownloadEvent::Transient { retry_after: None });
                }
                InFlight::Interrupt(cmd) => break self.handle_command(cmd).await,
                InFlight::Stop => break Flow::Stop,
            }
        };
        if flow == Flow::Stop {
            return Flow::Stop;
        }
        for cmd in deferred {
            if self.handle_command(cmd).await == Flow::Stop {
                return Flow::Stop;
            }
        }
        Flow::Continue
    }

    /// Một `GET`, song song với lệnh người dùng và nhịp tiến độ (nhịp vừa báo `Progress` vừa lưu offset vào store).
    async fn download_in_flight(
        &mut self,
        item: &DownloadItem,
        offset: u64,
        etag: Option<String>,
        link: &Link,
        meter: &mut SpeedMeter,
        deferred: &mut Vec<Command>,
    ) -> InFlight {
        let counter = Arc::new(AtomicU64::new(NOT_STARTED));
        let cx =
            FetchCx { device: &self.device, link, store: &self.store, events: &self.events, counter: counter.clone() };
        let fut = cx.fetch(item, offset, etag);
        tokio::pin!(fut);
        let start = tokio::time::Instant::now() + self.cfg.progress_interval;
        let mut tick = tokio::time::interval_at(start, self.cfg.progress_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                r = &mut fut => break InFlight::Done(r),
                cmd = self.commands.recv() => match cmd {
                    None | Some(Command::Stop) => break InFlight::Stop,
                    Some(c) if touches(&c, item.id) => break InFlight::Interrupt(c),
                    Some(Command::Wake) => {}
                    Some(c) => deferred.push(c),
                },
                _ = tick.tick() => {
                    let have = counter.load(Ordering::Relaxed);
                    if have != NOT_STARTED {
                        meter.record(Instant::now(), have);
                        {
                            let mut s = self.store.lock().unwrap_or_else(|p| p.into_inner());
                            if let Err(e) = s.apply_download(item.id, &DownloadEvent::Progress { offset: have }, now_ms()) {
                                log::error!("download {}: queue store: {e}", item.id);
                            }
                        }
                        let _ = self.events.send(TransferEvent::Progress {
                            id: item.id,
                            sent: have.min(item.size),
                            size: item.size,
                            bytes_per_sec: meter.bytes_per_sec(),
                        });
                    }
                }
            }
        }
    }

    /// Lệnh người dùng cho một mục tải về của thiết bị này. `false` ⇒ không phải mục tải về của mình.
    pub(super) fn download_command(&mut self, id: ItemId, action: ItemAction) -> bool {
        let Some(item) = self.with_store(|s| s.download(id)).flatten() else { return false };
        if item.device_id != self.device {
            return false;
        }
        if self.apply_download(id, &user_event(action)) == Some(DownloadEffect::DiscardPartial) {
            discard_partial(&item);
        }
        true
    }

    /// Áp sự kiện, phát `DownloadChanged` khi trạng thái đổi.
    fn apply_download(&mut self, id: ItemId, ev: &DownloadEvent) -> Option<DownloadEffect> {
        let before = self.with_store(|s| s.download(id)).flatten();
        let effect = self.with_store(|s| s.apply_download(id, ev, now_ms())).flatten();
        if let Some(after) = self.with_store(|s| s.download(id)).flatten()
            && before.is_none_or(|b| b.state != after.state)
        {
            self.emit(TransferEvent::DownloadChanged { id, state: after.state });
        }
        effect
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_windows_forbidden_and_edges() {
        assert_eq!(sanitize_file_name("Chuyến đi: ngày 1?.mp4"), "Chuyến đi_ ngày 1_.mp4");
        assert_eq!(sanitize_file_name("a\\b/c*d\"e<f>g|h.mkv"), "a_b_c_d_e_f_g_h.mkv");
        assert_eq!(sanitize_file_name("tab\there\u{7}.mp4"), "tab_here_.mp4");
        assert_eq!(sanitize_file_name("  ..hidden.mp4. . "), "hidden.mp4");
        assert_eq!(sanitize_file_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize_file_name("..."), "download");
        assert_eq!(sanitize_file_name(""), "download");
        assert_eq!(sanitize_file_name("CON.mp4"), "_CON.mp4");
        assert_eq!(sanitize_file_name("com1"), "_com1");
        assert_eq!(sanitize_file_name("COM0.mp4"), "COM0.mp4");
        assert_eq!(sanitize_file_name("CONSOLE.mp4"), "CONSOLE.mp4");
    }

    #[test]
    fn sanitize_truncates_keeping_extension() {
        let long = format!("{}.mp4", "ế".repeat(200)); // 600 byte
        let s = sanitize_file_name(&long);
        assert!(s.len() <= NAME_MAX_BYTES, "{}", s.len());
        assert!(s.ends_with("ế.mp4"), "{s}");
    }

    #[test]
    fn unique_path_appends_counter() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(unique_path(dir.path(), "a.mp4"), dir.path().join("a.mp4"));
        std::fs::write(dir.path().join("a.mp4"), b"x").unwrap();
        assert_eq!(unique_path(dir.path(), "a.mp4"), dir.path().join("a (2).mp4"));
        std::fs::write(dir.path().join("a (2).mp4"), b"x").unwrap();
        assert_eq!(unique_path(dir.path(), "a.mp4"), dir.path().join("a (3).mp4"));
        std::fs::write(dir.path().join("noext"), b"x").unwrap();
        assert_eq!(unique_path(dir.path(), "noext"), dir.path().join("noext (2)"));
    }

    #[test]
    fn part_path_sits_next_to_dest() {
        let p = part_path(Path::new("/d/clip.mp4"), 42);
        assert_eq!(p, Path::new("/d/clip.mp4.42.part"));
        assert_eq!(default_dest(Path::new("/d"), "a:b.mp4"), Path::new("/d/a_b.mp4"));
    }
}
