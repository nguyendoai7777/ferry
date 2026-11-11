//! Runner upload của một điện thoại (`PROTOCOL.md` 5, 6.5, 6.8, 6.9) — task gd1_04.
//!
//! Vòng chạy: lấy đầu hàng của thiết bị ⇒ áp sự kiện mở đầu (`Start` / `WakeUp`) ⇒ thực thi `UploadEffect` bằng
//! `PhoneClient` của [`Link`] hiện tại ⇒ báo `UploadEvent` ⇒ effect kế … tới khi mục kết thúc, chờ, hoặc tạm dừng.
//!
//! - **Rảnh không giữ CPU**: không có việc / Offline ⇒ `select!` chờ lệnh hoặc `Link` đổi; đang backoff ⇒ ngủ tới
//!   `Waiting.until` của mục đầu (thức sớm khi điện thoại hiện lại ở địa chỉ mới).
//! - **Checking**: `GET /list` một lần mỗi lô (chỉ mục của lô ⇒ [`RemoteIndex`]), trùng `dir` + tên + cỡ ⇒ Skipped.
//! - **Sending**: khúc `RunnerConfig::chunk_size` stream từ đĩa qua [`MeteredReader`] (không đọc cả khúc vào RAM),
//!   kiểm cỡ + mtime nguồn trước mỗi khúc, offset ghi store sau mỗi khúc (máy trạng thái), tiến độ theo nhịp.
//! - **Địa chỉ**: mỗi request đọc `Link` mới nhất — khúc sau đi địa chỉ khác vẫn cùng `uid` (§6.8).
//! - **Lệnh người dùng** tới giữa request: lệnh chạm mục đang bay (tạm dừng / huỷ) cắt request rồi áp; lệnh khác để
//!   sau khi request xong (≤ một khúc).

use std::collections::HashSet;
use std::io::{self, SeekFrom};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::io::AsyncSeekExt;
use tokio::sync::mpsc;

use super::throttle::{MeteredReader, SpeedMeter, Throttle};
use super::{Command, ItemAction, Link, LinkWatch, RunnerHandle, SharedStore, TransferEvent};
use crate::client::{self, ClientError, PatchOutcome};
use crate::device::DeviceId;
use crate::protocol::{self, LibraryItem};
use crate::queue::batch::source_matches;
use crate::queue::state::{FailReason, PauseReason};
use crate::queue::store::UploadItem;
use crate::queue::{BatchId, ItemId, QueueStore, UploadEffect, UploadEvent, UploadState, now_ms, store};

/// Tham số runner. Mặc định đúng PROTOCOL; test hạ khúc / nhịp.
#[derive(Debug, Clone)]
pub struct RunnerConfig {
    /// Cỡ khúc `PATCH` (PROTOCOL 6.5: 32 MB, đừng nhỏ hơn 4 MB trên mạng thật).
    pub chunk_size: u64,
    /// Nhịp phát [`TransferEvent::Progress`] khi đang gửi khúc.
    pub progress_interval: Duration,
    /// `list` của một lô dùng lại trong chừng này (lô rất dài / người dùng thử lại sau lâu ⇒ hỏi lại).
    pub check_ttl: Duration,
    /// `DELETE` phiên khi huỷ: chờ tối đa (điện thoại đã đi ⇒ phiên tự huỷ sau 60 phút).
    pub delete_timeout: Duration,
}

impl Default for RunnerConfig {
    fn default() -> RunnerConfig {
        RunnerConfig {
            chunk_size: client::CHUNK_SIZE,
            progress_interval: Duration::from_millis(500),
            check_ttl: Duration::from_secs(10 * 60),
            delete_timeout: Duration::from_secs(10),
        }
    }
}

/// Khoá so trùng (§5, §6.9): `dir` đã chuẩn hoá + tên + cỡ.
type RemoteKey = (String, String, u64);

/// Thư viện điện thoại đã hỏi cho một lô.
#[derive(Debug)]
struct RemoteIndex {
    batch: BatchId,
    at: Instant,
    keys: HashSet<RemoteKey>,
}

fn remote_key(item: &LibraryItem) -> Option<RemoteKey> {
    item.dir.as_deref().map(|d| (protocol::normalize_dir(d), item.name.clone(), item.size))
}

/// `dir` mục sẽ nằm trên điện thoại: không có `dirs` ⇒ gửi phẳng vào gốc.
fn target_dir(item: &UploadItem, dirs: bool) -> String {
    if dirs { protocol::normalize_dir(&item.dir) } else { String::new() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Continue,
    Stop,
}

/// Pha của request để map lỗi về sự kiện máy trạng thái nhận ở pha đó (không để mục kẹt ở trạng thái đang bay).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Check,
    Create,
    Transfer,
}

enum InFlight {
    Done(UploadEvent),
    /// Lệnh chạm mục đang bay: request đã bị cắt.
    Interrupt(Command),
    Stop,
}

enum Idle {
    Command(Option<Command>),
    LinkChanged(bool),
    Timer,
}

/// Runner upload của một thiết bị. Dựng bằng [`UploadRunner::new`], chạy bằng `tokio::spawn(runner.run())`.
pub struct UploadRunner {
    device: DeviceId,
    store: SharedStore,
    link: LinkWatch,
    commands: mpsc::UnboundedReceiver<Command>,
    events: mpsc::UnboundedSender<TransferEvent>,
    throttle: Arc<Throttle>,
    cfg: RunnerConfig,
    index: Option<RemoteIndex>,
    /// Sau `401`: hàng dừng tới khi có `Link` mới (đã ghép lại).
    repair_blocked: bool,
    /// `Link` đổi từ lần chạy mục trước ⇒ mục đang backoff thức ngay.
    link_dirty: bool,
    /// Đã phát `PhoneNeedsUpdate` cho `Link` hiện tại.
    warned_flat: bool,
    /// Khúc cuối đứt (có thể điện thoại đã commit mà phản hồi `201` mất) ⇒ `404` sau đó thì hỏi `list` trước khi tạo lại.
    final_chunk_lost: bool,
    /// Byte đã đọc từ đĩa để gửi (mọi mục, cả phần gửi lại) — chẩn đoán / test.
    bytes_read: Arc<AtomicU64>,
}

impl UploadRunner {
    pub fn new(
        device: DeviceId,
        store: SharedStore,
        link: LinkWatch,
        throttle: Arc<Throttle>,
        events: mpsc::UnboundedSender<TransferEvent>,
        cfg: RunnerConfig,
    ) -> (UploadRunner, RunnerHandle) {
        let (tx, commands) = mpsc::unbounded_channel();
        let runner = UploadRunner {
            device,
            store,
            link,
            commands,
            events,
            throttle,
            cfg,
            index: None,
            repair_blocked: false,
            link_dirty: false,
            warned_flat: false,
            final_chunk_lost: false,
            bytes_read: Arc::new(AtomicU64::new(0)),
        };
        (runner, RunnerHandle { tx })
    }

    /// Bộ đếm byte đã đọc từ đĩa để gửi (gồm cả phần gửi lại sau khi đứt).
    pub fn bytes_read(&self) -> Arc<AtomicU64> {
        self.bytes_read.clone()
    }

    /// Chạy tới [`Command::Stop`], hoặc khi mọi [`RunnerHandle`] / [`super::LinkSender`] bị bỏ.
    pub async fn run(mut self) {
        log::debug!("upload runner {}: start", self.device);
        loop {
            while let Ok(cmd) = self.commands.try_recv() {
                if self.handle_command(cmd).await == Flow::Stop {
                    return self.stopped();
                }
            }
            if self.link.has_changed().unwrap_or(false) {
                self.link.borrow_and_update();
                self.on_link_changed();
            }
            let online = self.link.borrow().is_some();
            if !online || self.repair_blocked {
                if self.idle(None).await == Flow::Stop {
                    return self.stopped();
                }
                continue;
            }
            let head = self.with_store(|s| s.pending_upload(&self.device)).flatten();
            let flow = match head {
                None => {
                    self.link_dirty = false;
                    self.idle(None).await
                }
                Some(item) => match item.state {
                    UploadState::Waiting { until, .. } if until > now_ms() && !self.link_dirty => {
                        self.idle(Some(until)).await
                    }
                    _ => {
                        self.link_dirty = false;
                        self.run_item(item).await
                    }
                },
            };
            if flow == Flow::Stop {
                return self.stopped();
            }
        }
    }

    fn stopped(&self) {
        log::debug!("upload runner {}: stop", self.device);
    }

    /// Chờ lệnh / `Link` đổi / đồng hồ backoff. Không vòng bận.
    async fn idle(&mut self, until: Option<i64>) -> Flow {
        let timer = async {
            match until {
                Some(u) => tokio::time::sleep(Duration::from_millis(u.saturating_sub(now_ms()).max(0) as u64)).await,
                None => std::future::pending().await,
            }
        };
        let got = tokio::select! {
            cmd = self.commands.recv() => Idle::Command(cmd),
            r = self.link.changed() => Idle::LinkChanged(r.is_ok()),
            () = timer => Idle::Timer,
        };
        match got {
            Idle::Command(None) | Idle::LinkChanged(false) => Flow::Stop,
            Idle::Command(Some(cmd)) => self.handle_command(cmd).await,
            Idle::LinkChanged(true) => {
                self.on_link_changed();
                Flow::Continue
            }
            Idle::Timer => Flow::Continue,
        }
    }

    fn on_link_changed(&mut self) {
        self.link_dirty = true;
        self.warned_flat = false;
        let online = self.link.borrow().is_some();
        if self.repair_blocked && online {
            // Tầng trên chỉ gửi lại `Link` sau khi có token mới ⇒ mở hàng, chạy tiếp các mục chờ ghép lại.
            self.repair_blocked = false;
            let paused: Vec<ItemId> = self
                .with_store(|s| s.uploads())
                .unwrap_or_default()
                .into_iter()
                .filter(|i| {
                    i.device_id == self.device
                        && matches!(i.state, UploadState::Paused { reason: PauseReason::NeedsPairing, .. })
                })
                .map(|i| i.id)
                .collect();
            for id in paused {
                self.apply(id, &UploadEvent::Resume);
            }
        }
    }

    fn current_link(&self) -> Option<Link> {
        if self.repair_blocked { None } else { self.link.borrow().clone() }
    }

    // ------------------------------------------------------------ một mục

    async fn run_item(&mut self, item: UploadItem) -> Flow {
        let id = item.id;
        self.final_chunk_lost = false;
        let mut effect = match &item.state {
            UploadState::Queued => self.apply(id, &UploadEvent::Start),
            UploadState::Waiting { .. } => self.apply(id, &UploadEvent::WakeUp),
            // Runner dựng lại trong cùng tiến trình (store không mở lại): chạy tiếp đúng pha.
            UploadState::Checking => Some(UploadEffect::CheckRemote),
            UploadState::Creating => Some(UploadEffect::Create),
            UploadState::Sending { uid, .. } => Some(UploadEffect::Head { uid: uid.clone() }),
            _ => None,
        };
        let mut meter = SpeedMeter::new();
        let mut deferred = Vec::new();
        let flow = loop {
            let eff = match effect.take() {
                None | Some(UploadEffect::None | UploadEffect::Wait { .. }) => break Flow::Continue,
                Some(UploadEffect::Delete { uid }) => {
                    self.delete_remote(&uid).await;
                    break Flow::Continue;
                }
                Some(e) => e,
            };
            let Some(item) = self.with_store(|s| s.upload(id)).flatten() else { break Flow::Continue };
            let Some(link) = self.current_link() else {
                // Offline giữa chừng: chờ (thức ngay khi điện thoại hiện lại).
                self.apply(id, &UploadEvent::Transient { retry_after: None });
                break Flow::Continue;
            };
            match self.in_flight(&item, &eff, &link, &mut meter, &mut deferred).await {
                InFlight::Done(ev) => {
                    if ev == UploadEvent::Unauthorized {
                        self.repair_blocked = true;
                        self.emit(TransferEvent::DeviceNeedsRepair { device: self.device.clone() });
                    }
                    effect = self.apply(id, &ev);
                    // Sự kiện máy trạng thái bỏ qua mà mục vẫn "đang bay" ⇒ chờ backoff, không quay vòng nóng.
                    if matches!(effect, Some(UploadEffect::None))
                        && self.with_store(|s| s.upload(id)).flatten().is_some_and(|i| i.state.is_active())
                    {
                        log::warn!("upload {id}: event {ev:?} ignored in active state, backing off");
                        effect = self.apply(id, &UploadEvent::Transient { retry_after: None });
                    }
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

    /// Một request, song song với lệnh người dùng và nhịp tiến độ.
    async fn in_flight(
        &mut self,
        item: &UploadItem,
        eff: &UploadEffect,
        link: &Link,
        meter: &mut SpeedMeter,
        deferred: &mut Vec<Command>,
    ) -> InFlight {
        let counter = Arc::new(AtomicU64::new(0));
        let patch_from = match eff {
            UploadEffect::Patch { offset, .. } => Some(*offset),
            _ => None,
        };
        let mut cx = Exec {
            device: &self.device,
            link,
            throttle: &self.throttle,
            cfg: &self.cfg,
            index: &mut self.index,
            events: &self.events,
            counter: counter.clone(),
            warned_flat: &mut self.warned_flat,
            final_chunk_lost: &mut self.final_chunk_lost,
        };
        let fut = cx.execute(item, eff);
        tokio::pin!(fut);
        let start = tokio::time::Instant::now() + self.cfg.progress_interval;
        let mut tick = tokio::time::interval_at(start, self.cfg.progress_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let result = loop {
            tokio::select! {
                ev = &mut fut => break InFlight::Done(ev),
                cmd = self.commands.recv() => match cmd {
                    None | Some(Command::Stop) => break InFlight::Stop,
                    Some(c) if touches(&c, item) => break InFlight::Interrupt(c),
                    Some(Command::Wake) => {}
                    Some(c) => deferred.push(c),
                },
                _ = tick.tick(), if patch_from.is_some() => {
                    let sent = patch_from.unwrap_or(0) + counter.load(Ordering::Relaxed);
                    meter.record(Instant::now(), sent);
                    let _ = self.events.send(TransferEvent::Progress {
                        id: item.id,
                        sent: sent.min(item.size),
                        size: item.size,
                        bytes_per_sec: meter.bytes_per_sec(),
                    });
                }
            }
        };
        self.bytes_read.fetch_add(counter.load(Ordering::Relaxed), Ordering::Relaxed);
        result
    }

    // ------------------------------------------------------------ lệnh

    async fn handle_command(&mut self, cmd: Command) -> Flow {
        match cmd {
            Command::Stop => return Flow::Stop,
            Command::Wake => {}
            Command::Item { id, action } => {
                let mine = self.with_store(|s| s.upload(id)).flatten().is_some_and(|i| i.device_id == self.device);
                if !mine {
                    return Flow::Continue;
                }
                let effect = match action {
                    ItemAction::Retry => self.retry(id),
                    a => self.apply(id, &user_event(a)),
                };
                if let Some(UploadEffect::Delete { uid }) = effect {
                    self.delete_remote(&uid).await;
                }
            }
            Command::Batch { batch, action } => {
                let mut items = self.with_store(|s| s.batch_uploads(&batch)).unwrap_or_default();
                items.retain(|i| i.device_id == self.device);
                let mut deletes = Vec::new();
                for item in items {
                    let effect = match action {
                        ItemAction::Retry if matches!(item.state, UploadState::Failed { .. }) => self.retry(item.id),
                        ItemAction::Retry => None,
                        a => self.apply(item.id, &user_event(a)),
                    };
                    if let Some(UploadEffect::Delete { uid }) = effect {
                        deletes.push(uid);
                    }
                }
                for uid in deletes {
                    self.delete_remote(&uid).await;
                }
            }
        }
        Flow::Continue
    }

    fn retry(&mut self, id: ItemId) -> Option<UploadEffect> {
        let before = self.with_store(|s| s.upload(id)).flatten();
        let effect = self.with_store(|s| s.retry_upload(id, now_ms())).flatten();
        self.after_apply(id, before.as_ref());
        effect
    }

    /// `DELETE` phiên dở (huỷ / tệp nguồn đổi) — cố một lần, không chặn lâu; điện thoại vắng thì phiên tự huỷ sau 60 phút.
    async fn delete_remote(&self, uid: &str) {
        let Some(link) = self.current_link() else { return };
        match tokio::time::timeout(self.cfg.delete_timeout, link.client.upload_delete(uid)).await {
            Ok(Ok(())) | Ok(Err(ClientError::NotFound)) => {}
            Ok(Err(e)) => log::debug!("upload delete {uid}: {e}"),
            Err(_) => log::debug!("upload delete {uid}: timeout"),
        }
    }

    // ------------------------------------------------------------ store + sự kiện

    fn with_store<T>(&self, f: impl FnOnce(&mut QueueStore) -> store::Result<T>) -> Option<T> {
        let mut s = self.store.lock().unwrap_or_else(|p| p.into_inner());
        match f(&mut s) {
            Ok(v) => Some(v),
            Err(e) => {
                log::error!("upload runner {}: queue store: {e}", self.device);
                None
            }
        }
    }

    /// Áp sự kiện, phát `ItemChanged` khi trạng thái đổi, `BatchFinished` khi mục làm lô yên.
    fn apply(&mut self, id: ItemId, ev: &UploadEvent) -> Option<UploadEffect> {
        let before = self.with_store(|s| s.upload(id)).flatten();
        let effect = self.with_store(|s| s.apply_upload(id, ev, now_ms())).flatten();
        self.after_apply(id, before.as_ref());
        effect
    }

    fn after_apply(&mut self, id: ItemId, before: Option<&UploadItem>) {
        let Some(after) = self.with_store(|s| s.upload(id)).flatten() else { return };
        if before.is_some_and(|b| b.state == after.state) {
            return;
        }
        self.emit(TransferEvent::ItemChanged { id, state: after.state.clone() });
        let was_terminal = before.is_some_and(|b| b.state.is_terminal());
        if after.state.is_terminal()
            && !was_terminal
            && let Some(batch) = &after.batch_id
        {
            self.check_batch(batch);
        }
    }

    fn check_batch(&mut self, batch: &BatchId) {
        let Some(items) = self.with_store(|s| s.batch_uploads(batch)) else { return };
        if !items.iter().all(|i| i.state.is_terminal()) {
            return;
        }
        let count = |f: fn(&UploadState) -> bool| items.iter().filter(|i| f(&i.state)).count() as u64;
        let sent = count(|s| matches!(s, UploadState::Done { .. }));
        let skipped = count(|s| matches!(s, UploadState::Skipped));
        let failed = count(|s| matches!(s, UploadState::Failed { .. }));
        if self.index.as_ref().is_some_and(|i| &i.batch == batch) {
            self.index = None;
        }
        if sent + skipped + failed > 0 {
            self.emit(TransferEvent::BatchFinished { batch: batch.clone(), sent, skipped, failed });
        }
    }

    fn emit(&self, ev: TransferEvent) {
        let _ = self.events.send(ev);
    }
}

fn user_event(a: ItemAction) -> UploadEvent {
    match a {
        ItemAction::Pause => UploadEvent::Pause,
        ItemAction::Resume | ItemAction::Retry => UploadEvent::Resume,
        ItemAction::Cancel => UploadEvent::Cancel,
    }
}

/// Lệnh này có phải cắt request đang bay của `item`?
fn touches(cmd: &Command, item: &UploadItem) -> bool {
    let stops = |a: &ItemAction| matches!(a, ItemAction::Pause | ItemAction::Cancel);
    match cmd {
        Command::Item { id, action } => *id == item.id && stops(action),
        Command::Batch { batch, action } => item.batch_id.as_ref() == Some(batch) && stops(action),
        _ => false,
    }
}

// ---------------------------------------------------------------- thực thi effect

/// Những gì một request cần, mượn từ runner (tách khỏi `commands` để `select!` song song).
struct Exec<'a> {
    device: &'a DeviceId,
    link: &'a Link,
    throttle: &'a Arc<Throttle>,
    cfg: &'a RunnerConfig,
    index: &'a mut Option<RemoteIndex>,
    events: &'a mpsc::UnboundedSender<TransferEvent>,
    counter: Arc<AtomicU64>,
    warned_flat: &'a mut bool,
    final_chunk_lost: &'a mut bool,
}

impl Exec<'_> {
    async fn execute(&mut self, item: &UploadItem, eff: &UploadEffect) -> UploadEvent {
        match eff {
            UploadEffect::CheckRemote => self.check(item).await,
            UploadEffect::Create => self.create(item).await,
            UploadEffect::Patch { uid, offset } => self.patch(item, uid, *offset).await,
            UploadEffect::Head { uid } => self.head(item, uid).await,
            // Runner không gửi các effect còn lại vào đây.
            UploadEffect::None | UploadEffect::Wait { .. } | UploadEffect::Delete { .. } => UploadEvent::WakeUp,
        }
    }

    /// Checking: trùng `dir` + tên + cỡ trên điện thoại ⇒ Skipped. `list` dùng lại trong cả lô.
    async fn check(&mut self, item: &UploadItem) -> UploadEvent {
        if !protocol::is_video_name(&item.name) {
            return UploadEvent::Rejected(FailReason::Unsupported);
        }
        if let Some(ev) = source_event(item) {
            return ev;
        }
        let key = (target_dir(item, self.link.dirs), item.name.clone(), item.size);
        let cached = self.index.as_ref().filter(|ix| {
            item.batch_id.as_ref() == Some(&ix.batch) && ix.at.elapsed() < self.cfg.check_ttl
        });
        if let Some(ix) = cached {
            return UploadEvent::Checked { exists: ix.keys.contains(&key) };
        }
        let items = match self.link.client.list().await {
            Ok(items) => items,
            Err(e) => return self.map_error(e, Phase::Check),
        };
        let keys: HashSet<RemoteKey> = items.iter().filter_map(remote_key).collect();
        let exists = keys.contains(&key);
        *self.index = item.batch_id.clone().map(|batch| RemoteIndex { batch, at: Instant::now(), keys });
        UploadEvent::Checked { exists }
    }

    async fn create(&mut self, item: &UploadItem) -> UploadEvent {
        if let Some(ev) = source_event(item) {
            return ev;
        }
        let dir = if self.link.dirs {
            Some(item.dir.as_str()).filter(|d| !d.is_empty())
        } else {
            if !item.dir.is_empty() && !*self.warned_flat {
                *self.warned_flat = true;
                let _ = self.events.send(TransferEvent::PhoneNeedsUpdate { device: self.device.clone() });
            }
            None
        };
        match self.link.client.upload_create(&item.name, item.size, dir).await {
            Ok(created) => UploadEvent::Created { uid: created.id },
            Err(e) => self.map_error(e, Phase::Create),
        }
    }

    async fn patch(&mut self, item: &UploadItem, uid: &str, offset: u64) -> UploadEvent {
        if let Some(ev) = source_event(item) {
            return ev;
        }
        if offset >= item.size {
            // Điện thoại đã nhận đủ mà chưa thấy `201`: hỏi lại thư viện.
            return self.recover_commit(item).await;
        }
        let len = self.cfg.chunk_size.min(item.size - offset);
        let file = match open_at(&item.path, offset).await {
            Ok(f) => f,
            Err(e) => return local_event(&e),
        };
        let reader = MeteredReader::new(file, self.throttle.clone(), self.counter.clone());
        match self.link.client.upload_patch(uid, offset, reader, len).await {
            Ok(PatchOutcome::Partial(o)) => {
                *self.final_chunk_lost = false;
                UploadEvent::ChunkAccepted { offset: o }
            }
            Ok(PatchOutcome::Done(lib)) => self.committed(item, lib),
            Err(ClientError::Network(e)) if offset + len == item.size => {
                *self.final_chunk_lost = true;
                self.map_error(ClientError::Network(e), Phase::Transfer)
            }
            Err(e) => self.map_error(e, Phase::Transfer),
        }
    }

    async fn head(&mut self, item: &UploadItem, uid: &str) -> UploadEvent {
        match self.link.client.upload_head(uid).await {
            Ok(st) if st.length == item.size => UploadEvent::OffsetReported { offset: st.offset },
            // Phiên của tệp khác cỡ (không nên xảy ra) ⇒ tạo lại.
            Ok(_) => UploadEvent::UploadGone,
            Err(ClientError::NotFound) if *self.final_chunk_lost => self.recover_commit(item).await,
            Err(e) => self.map_error(e, Phase::Transfer),
        }
    }

    /// Khúc cuối đứt rồi phiên mất: điện thoại có thể đã commit. Có tệp trùng `dir` + tên + cỡ (Checking đã thấy chưa
    /// có lúc đầu) ⇒ coi là xong; không ⇒ tạo lại.
    async fn recover_commit(&mut self, item: &UploadItem) -> UploadEvent {
        *self.final_chunk_lost = false;
        let dir = target_dir(item, self.link.dirs);
        match self.link.client.list().await {
            Ok(items) => items
                .into_iter()
                .find(|l| remote_key(l).is_some_and(|k| k == (dir.clone(), item.name.clone(), item.size)))
                .map_or(UploadEvent::UploadGone, |lib| self.committed(item, lib)),
            Err(e) => self.map_error(e, Phase::Check),
        }
    }

    fn committed(&mut self, item: &UploadItem, lib: LibraryItem) -> UploadEvent {
        *self.final_chunk_lost = false;
        if let (Some(ix), Some(key)) = (self.index.as_mut(), remote_key(&lib))
            && item.batch_id.as_ref() == Some(&ix.batch)
        {
            ix.keys.insert(key);
        }
        UploadEvent::Committed { remote_name: lib.name, remote_dir: lib.dir }
    }

    /// Lỗi ⇒ sự kiện của PROTOCOL 5 (bảng ở doc `UploadEvent`), hợp với pha đang chạy.
    fn map_error(&self, e: ClientError, phase: Phase) -> UploadEvent {
        use UploadEvent as Ev;
        let transient = Ev::Transient { retry_after: None };
        match e {
            ClientError::Network(err) => {
                log::debug!("upload {}: network at {}: {err}", self.device, self.link.addr());
                let _ = self
                    .events
                    .send(TransferEvent::ConnectionLost { device: self.device.clone(), addr: self.link.addr() });
                transient
            }
            ClientError::Unauthorized => Ev::Unauthorized,
            ClientError::InsufficientStorage => Ev::InsufficientStorage { short_by: None },
            ClientError::Unsupported => Ev::Rejected(FailReason::Unsupported),
            ClientError::TooLarge => Ev::Rejected(FailReason::TooLarge),
            ClientError::BadRequest | ClientError::WrongPin => Ev::Rejected(FailReason::BadRequest),
            ClientError::Busy { retry_after } | ClientError::RateLimited { retry_after } => Ev::Transient { retry_after },
            ClientError::Local(err) => local_event(&err),
            ClientError::NotFound if phase == Phase::Transfer => Ev::UploadGone,
            ClientError::OffsetMismatch(offset) if phase == Phase::Transfer => Ev::OffsetReported { offset },
            ClientError::Locked if phase == Phase::Transfer => Ev::Locked { retry_after: None },
            ClientError::Server(code) if (400..500).contains(&code) => {
                log::warn!("upload {}: unexpected HTTP {code}", self.device);
                Ev::Rejected(FailReason::BadRequest)
            }
            other => {
                log::debug!("upload {}: {other}", self.device);
                transient
            }
        }
    }
}

/// Tệp nguồn còn như lúc thả vào? Không ⇒ sự kiện Failed tương ứng.
fn source_event(item: &UploadItem) -> Option<UploadEvent> {
    match source_matches(&item.path, item.size, item.mtime) {
        Ok(true) => None,
        Ok(false) => Some(UploadEvent::SourceChanged),
        Err(e) => Some(local_event(&e)),
    }
}

fn local_event(e: &io::Error) -> UploadEvent {
    if e.kind() == io::ErrorKind::NotFound { UploadEvent::SourceMissing } else { UploadEvent::SourceChanged }
}

async fn open_at(path: &std::path::Path, offset: u64) -> io::Result<tokio::fs::File> {
    let mut f = tokio::fs::File::open(path).await?;
    f.seek(SeekFrom::Start(offset)).await?;
    Ok(f)
}
