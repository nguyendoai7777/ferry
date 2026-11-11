//! Engine của daemon: một task tokio (runtime 2 luồng, `main.rs`) giữ mọi trạng thái sống — sổ thiết bị (`Devices`),
//! một `UploadRunner` mỗi điện thoại đã ghép, mDNS, danh sách UI đang nghe — và xử lý tuần tự trong một `select!`
//! (không mutex chia sẻ trừ `SharedStore` của runner). Ra ngoài chỉ qua: [`Outbox`] của kết nối IPC (ảnh chụp, tiến
//! độ, trả lời lệnh), [`Sinks::tray`] (trạng thái khay cho luồng chính) và [`Sinks::notice`] (thông báo hệ thống).
//!
//! Đẩy cho UI (gd1_07 chốt, `ferry_core::ipc`): kết nối mới ⇒ [`Snapshot`] đầy đủ ngay; sau đó gộp tối đa
//! [`MAX_PUSH_PER_SEC`] lần / s — có gì đổi ngoài tiến độ (mục, lô, thiết bị) ⇒ `Snapshot` đầy đủ, chỉ tiến độ trong
//! khúc ⇒ `Progress` (mục đã đổi). Client đọc chậm ([`Outbox::backed_up`]) bị bỏ lượt rồi nhận bản đầy đủ khi kịp.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferry_core::config::{self, Config};
use ferry_core::device::{DeviceEvent, DeviceId, DeviceState, Devices};
use ferry_core::discovery::address::{self, AddressError, LocalInterface};
use ferry_core::discovery::mdns::{Browser, MdnsEvent};
use ferry_core::discovery::{self, ConnectError, Connected};
use ferry_core::i18n::I18n;
use ferry_core::ipc::{
    Action, BatchView, CommandError, DaemonToUi, DeviceView, FINISHED_IN_SNAPSHOT, ItemProgress, LibraryOp,
    MAX_PUSH_PER_SEC, Outcome, PhoneOp, ProgressDelta, QUEUED_IN_SNAPSHOT, QueueOp, Snapshot,
};
use ferry_core::pairing::{self, Identity, PairError, Tokens};
use ferry_core::protocol::{Info, Secret};
use ferry_core::queue::state::PauseReason;
use ferry_core::queue::store::UploadItem;
use ferry_core::queue::{BatchId, ItemId, QueueStore, UploadState, now_ms, plan_batch};
use ferry_core::transfer::{
    Command, ItemAction, Link, LinkSender, RunnerConfig, RunnerHandle, SharedStore, Throttle, TransferEvent,
    UploadRunner, link_channel,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::ipc_server::Outbox;
use crate::notify::Notice;
use crate::tray_state::{self, Summary, TrayStatus, plural};

/// Khoảng tối thiểu giữa hai lần đẩy cho UI (10 lần / s).
const PUSH_INTERVAL: Duration = Duration::from_millis(1000 / MAX_PUSH_PER_SEC as u64);
/// Dọn mục đã xong quá 7 ngày (`QueueStore::purge_finished`).
const PURGE_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// Tắt app: chờ runner dừng tối đa (request đang bay bị cắt; mở lại thì nối tiếp).
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

pub struct EngineConfig {
    pub config_path: PathBuf,
    pub config: Config,
    pub store: SharedStore,
    pub tokens: Tokens,
    /// `false` ⇒ không dò mDNS (test: chỉ địa chỉ trong config / nhập tay).
    pub mdns: bool,
    pub runner: RunnerConfig,
    /// Trần tốc độ chung mọi điện thoại (cài đặt của gd1_12; giờ không giới hạn).
    pub throttle: Arc<Throttle>,
    pub i18n: I18n,
}

/// Đầu ra không qua IPC. Test thay bằng hàm ghi lại.
pub struct Sinks {
    pub tray: Box<dyn Fn(TrayStatus) + Send>,
    pub notice: Box<dyn Fn(Notice) + Send>,
}

pub enum Op {
    Queue(QueueOp),
    Phone(PhoneOp),
    Library(LibraryOp),
}

pub enum Input {
    /// Lệnh của UI; trả lời `Reply { req }` vào `outbox`.
    Request {
        outbox: Outbox,
        req: u64,
        op: Op,
    },
    /// UI mới nối: nhận ảnh chụp đầy đủ ngay rồi nhận đẩy.
    Subscribe {
        conn: u64,
        outbox: Outbox,
    },
    Unsubscribe {
        conn: u64,
    },
    /// Menu khay "Tạm dừng tất cả" / "Tiếp tục tất cả" (theo trạng thái hiện tại).
    TogglePauseAll,
    /// Dừng runner rồi báo `done`.
    Shutdown {
        done: std::sync::mpsc::Sender<()>,
    },
}

#[derive(Clone)]
pub struct EngineHandle(mpsc::UnboundedSender<Input>);

impl EngineHandle {
    pub fn send(&self, input: Input) {
        if self.0.send(input).is_err() {
            log::debug!("engine: stopped, input dropped");
        }
    }

    /// Chặn tới khi engine dừng runner (tối đa [`STOP_TIMEOUT`] + chút).
    pub fn shutdown(&self) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(Input::Shutdown { done: tx });
        if rx.recv_timeout(STOP_TIMEOUT + Duration::from_secs(1)).is_err() {
            log::warn!("engine: shutdown timed out");
        }
    }
}

/// Dựng engine trên `rt`. Gọi trong ngữ cảnh runtime không bắt buộc.
pub fn spawn(rt: &tokio::runtime::Handle, cfg: EngineConfig, sinks: Sinks) -> EngineHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = EngineHandle(tx);
    rt.spawn(async move {
        let (engine, receivers) = Engine::new(cfg, sinks);
        engine.run(rx, receivers).await;
    });
    handle
}

struct Runner {
    handle: RunnerHandle,
    link: LinkSender,
    task: JoinHandle<()>,
}

/// Kết quả task con trả về vòng chính.
enum Internal {
    /// Nối thiết bị có token xong (`probe_device`).
    Connected {
        id: DeviceId,
        result: Result<Connected, ConnectError>,
    },
    /// Thiết bị có trạng thái có token mà kho khoá không có token.
    NoToken {
        id: DeviceId,
    },
    /// Kho khoá hỏng lúc đọc token (không đổi trạng thái — thử lại lượt sau).
    KeyringFailed {
        id: DeviceId,
    },
    Paired {
        outbox: Outbox,
        req: u64,
        result: Result<pairing::Paired, CommandError>,
    },
    Probed {
        outbox: Outbox,
        req: u64,
        addr: SocketAddr,
        result: Result<Info, CommandError>,
    },
    Added {
        outbox: Outbox,
        req: u64,
        device: DeviceId,
        outcome: Outcome,
    },
    Forgot {
        outbox: Outbox,
        req: u64,
    },
}

struct Receivers {
    transfer: mpsc::UnboundedReceiver<TransferEvent>,
    internal: mpsc::UnboundedReceiver<Internal>,
    browser: Option<Browser>,
}

struct Engine {
    config_path: PathBuf,
    config: Config,
    store: SharedStore,
    tokens: Tokens,
    runner_cfg: RunnerConfig,
    throttle: Arc<Throttle>,
    i18n: I18n,
    sinks: Sinks,
    devices: Devices,
    locals: Vec<LocalInterface>,
    runners: HashMap<DeviceId, Runner>,
    probing: HashSet<DeviceId>,
    transfer_tx: mpsc::UnboundedSender<TransferEvent>,
    internal_tx: mpsc::UnboundedSender<Internal>,
    subscribers: HashMap<u64, Subscriber>,
    /// Tiến độ trong khúc của mục đang gửi (mới hơn `offset` đã ghi store).
    progress: HashMap<ItemId, ItemProgress>,
    /// Có gì đổi ngoài tiến độ ⇒ lần đẩy kế là ảnh chụp đầy đủ.
    dirty: bool,
    progress_dirty: HashSet<ItemId>,
    last_push: Instant,
    summary: Summary,
    tray: Option<TrayStatus>,
    /// Đã báo "cần ghép lại" / "hết chỗ" — báo một lần tới khi thiết bị chạy lại.
    told_repair: HashSet<DeviceId>,
    told_no_space: HashSet<DeviceId>,
}

struct Subscriber {
    outbox: Outbox,
    /// Đã bị bỏ lượt (đọc chậm) ⇒ lần kế gửi ảnh chụp đầy đủ.
    resync: bool,
}

impl Engine {
    fn new(cfg: EngineConfig, sinks: Sinks) -> (Engine, Receivers) {
        let (transfer_tx, transfer) = mpsc::unbounded_channel();
        let (internal_tx, internal) = mpsc::unbounded_channel();
        let locals = address::local_interfaces();
        let devices = Devices::from_config(&cfg.config.devices, locals.clone(), Instant::now());
        let browser = if cfg.mdns {
            match Browser::start() {
                Ok(b) => Some(b),
                Err(e) => {
                    log::warn!("engine: mdns unavailable ({e}), only saved / manual addresses");
                    None
                }
            }
        } else {
            None
        };
        let mut engine = Engine {
            config_path: cfg.config_path,
            config: cfg.config,
            store: cfg.store,
            tokens: cfg.tokens,
            runner_cfg: cfg.runner,
            throttle: cfg.throttle,
            i18n: cfg.i18n,
            sinks,
            devices,
            locals,
            runners: HashMap::new(),
            probing: HashSet::new(),
            transfer_tx,
            internal_tx,
            subscribers: HashMap::new(),
            progress: HashMap::new(),
            dirty: true,
            progress_dirty: HashSet::new(),
            last_push: Instant::now() - PUSH_INTERVAL,
            summary: Summary::default(),
            tray: None,
            told_repair: HashSet::new(),
            told_no_space: HashSet::new(),
        };
        let paired: Vec<DeviceId> = engine.config.devices.iter().map(|d| d.id.clone()).collect();
        for id in paired {
            engine.ensure_runner(&id);
        }
        (engine, Receivers { transfer, internal, browser })
    }

    async fn run(mut self, mut inputs: mpsc::UnboundedReceiver<Input>, mut rx: Receivers) {
        log::info!("engine: started ({} paired phone(s))", self.runners.len());
        self.push(true);
        let mut purge = tokio::time::interval(PURGE_INTERVAL);
        loop {
            let deadline = self.devices.next_deadline().map(tokio::time::Instant::from_std);
            let push_at = self.push_due().map(tokio::time::Instant::from_std);
            tokio::select! {
                input = inputs.recv() => match input {
                    Some(Input::Shutdown { done }) => {
                        self.stop_runners().await;
                        let _ = done.send(());
                        break;
                    }
                    Some(input) => self.on_input(input),
                    None => {
                        self.stop_runners().await;
                        break;
                    }
                },
                Some(ev) = rx.transfer.recv() => self.on_transfer(ev),
                Some(msg) = rx.internal.recv() => self.on_internal(msg),
                ev = next_mdns(rx.browser.as_ref()) => match ev {
                    Some(MdnsEvent::Announced(ann)) => {
                        let ev = self.devices.announce(ann, Instant::now());
                        self.on_device_events(ev);
                    }
                    Some(MdnsEvent::Withdrawn { fullname }) => self.devices.withdraw(&fullname, Instant::now()),
                    None => {
                        log::warn!("engine: mdns stopped");
                        rx.browser = None;
                    }
                },
                () = sleep_until(deadline) => self.on_deadline(),
                () = sleep_until(push_at) => self.push(false),
                _ = purge.tick() => self.purge(),
            }
        }
        log::info!("engine: stopped");
    }

    // ---- đầu vào ----

    fn on_input(&mut self, input: Input) {
        match input {
            Input::Request { outbox, req, op } => self.on_request(outbox, req, op),
            Input::Subscribe { conn, outbox } => {
                let snapshot = self.snapshot();
                let resync = !outbox.push(DaemonToUi::Snapshot(snapshot));
                self.subscribers.insert(conn, Subscriber { outbox, resync });
            }
            Input::Unsubscribe { conn } => {
                self.subscribers.remove(&conn);
            }
            Input::TogglePauseAll => {
                let action = if self.summary.can_pause_all() { Action::Pause } else { Action::Resume };
                if let Err(e) = self.all(action) {
                    log::debug!("engine: pause all: {e:?}");
                }
            }
            Input::Shutdown { .. } => unreachable!("handled in run"),
        }
    }

    fn on_request(&mut self, outbox: Outbox, req: u64, op: Op) {
        let reply = match op {
            Op::Queue(op) => self.on_queue(&outbox, req, op),
            Op::Phone(op) => self.on_phone(&outbox, req, op),
            Op::Library(op) => self.on_library(&outbox, req, op),
        };
        // `None` ⇒ task con trả lời sau.
        if let Some(r) = reply {
            reply_to(&outbox, req, r);
        }
    }

    fn on_queue(&mut self, outbox: &Outbox, req: u64, op: QueueOp) -> Option<Result<Outcome, CommandError>> {
        match op {
            QueueOp::AddFiles { device, paths, dest_dir } => {
                if let Err(e) = self.require_paired(&device) {
                    return Some(Err(e));
                }
                let (store, tx, outbox) = (self.store.clone(), self.internal_tx.clone(), outbox.clone());
                tokio::task::spawn_blocking(move || {
                    let outcome = add_files(&store, &device, &paths, &dest_dir);
                    let outcome = outcome.unwrap_or_else(|error| Outcome::Failed { error });
                    let _ = tx.send(Internal::Added { outbox, req, device, outcome });
                });
                None
            }
            QueueOp::Item { id, action } => Some(self.item(id, action)),
            QueueOp::Batch { batch, action } => Some(self.batch(&batch, action)),
            QueueOp::All { action } => Some(self.all(action)),
        }
    }

    fn on_phone(&mut self, outbox: &Outbox, req: u64, op: PhoneOp) -> Option<Result<Outcome, CommandError>> {
        match op {
            PhoneOp::Pair { device, pin } => {
                let pin = pin.0.trim().to_owned();
                if pin.len() != 6 || !pin.bytes().all(|b| b.is_ascii_digit()) {
                    return Some(Err(CommandError::InvalidInput));
                }
                if self.devices.get(&device).is_none() {
                    return Some(Err(CommandError::UnknownDevice));
                }
                let addrs = self.devices.addresses(&device);
                let (tokens, tx, outbox) = (self.tokens.clone(), self.internal_tx.clone(), outbox.clone());
                let (client_id, client_name) = (self.config.client_id.clone(), self.config.client_name.clone());
                tokio::spawn(async move {
                    let result = async {
                        let mut c =
                            discovery::connect(&device, &addrs, None).await.map_err(|_| CommandError::Unreachable)?;
                        let me = Identity { client_id: &client_id, client_name: &client_name };
                        pairing::pair(&mut c.client, Some(&device), &Secret::new(pin), me, &tokens)
                            .await
                            .map_err(pair_error)
                    }
                    .await;
                    let _ = tx.send(Internal::Paired { outbox, req, result });
                });
                None
            }
            PhoneOp::Forget { device } => {
                if self.devices.get(&device).is_none() && !self.runners.contains_key(&device) {
                    return Some(Err(CommandError::UnknownDevice));
                }
                // Gỡ khỏi sổ + config ngay (không probe / dựng runner lại), dọn kho khoá + hàng đợi trong task con.
                let runner = self.runners.remove(&device);
                let online = runner.as_ref().and_then(|r| r.link.borrow().as_ref().map(|l| l.client.clone()));
                if let Some(r) = &runner {
                    r.handle.send(Command::Stop);
                }
                self.config.forget_device(&device);
                let ev = self.devices.forget(&device);
                self.on_device_events(ev);
                self.save_config();
                self.told_repair.remove(&device);
                self.told_no_space.remove(&device);
                let (tokens, store, tx, outbox) =
                    (self.tokens.clone(), self.store.clone(), self.internal_tx.clone(), outbox.clone());
                tokio::spawn(async move {
                    if let Some(r) = runner {
                        let _ = tokio::time::timeout(STOP_TIMEOUT, r.task).await;
                    }
                    pairing::forget(&device, online.as_deref(), &tokens).await;
                    let removed = tokio::task::spawn_blocking(move || lock(&store).remove_device(&device)).await;
                    match removed {
                        Ok(Ok(n)) => log::info!("engine: removed {n} queued item(s) of the forgotten phone"),
                        Ok(Err(e)) => log::warn!("engine: remove queue of forgotten phone: {e}"),
                        Err(e) => log::warn!("engine: remove queue task: {e}"),
                    }
                    let _ = tx.send(Internal::Forgot { outbox, req });
                });
                None
            }
            PhoneOp::AddAddress { address } => {
                self.refresh_locals();
                let addr = match address::parse_address(&address, &self.locals) {
                    Ok(a) => a,
                    Err(AddressError::Invalid) => return Some(Err(CommandError::InvalidInput)),
                    Err(AddressError::UnknownInterface(name)) => {
                        return Some(Err(CommandError::UnknownInterface { name }));
                    }
                };
                let (tx, outbox) = (self.internal_tx.clone(), outbox.clone());
                tokio::spawn(async move {
                    let result = discovery::probe(addr).await.map_err(|e| {
                        log::info!("engine: manual address {addr}: {e}");
                        CommandError::Unreachable
                    });
                    let _ = tx.send(Internal::Probed { outbox, req, addr, result });
                });
                None
            }
        }
    }

    fn on_library(&mut self, outbox: &Outbox, req: u64, op: LibraryOp) -> Option<Result<Outcome, CommandError>> {
        match op {
            LibraryOp::List { device } => {
                let client = match self.online_client(&device) {
                    Ok(c) => c,
                    Err(e) => return Some(Err(e)),
                };
                let outbox = outbox.clone();
                tokio::spawn(async move {
                    let r = client.list().await.map(|items| Outcome::Library { items }).map_err(|e| {
                        log::info!("engine: list {device}: {e}");
                        CommandError::Unreachable
                    });
                    reply_to(&outbox, req, r);
                });
                None
            }
        }
    }

    // ---- lệnh hàng đợi ----

    fn item(&mut self, id: ItemId, action: Action) -> Result<Outcome, CommandError> {
        let item = lock(&self.store).upload(id).map_err(internal)?.ok_or(CommandError::InvalidInput)?;
        let runner = self.runners.get(&item.device_id).ok_or(CommandError::NotPaired)?;
        runner.handle.send(Command::Item { id, action: item_action(action) });
        Ok(Outcome::Done)
    }

    fn batch(&mut self, batch: &BatchId, action: Action) -> Result<Outcome, CommandError> {
        let batches = lock(&self.store).batches().map_err(internal)?;
        let info = batches.iter().find(|b| b.id == *batch).ok_or(CommandError::InvalidInput)?;
        let runner = self.runners.get(&info.device_id).ok_or(CommandError::NotPaired)?;
        runner.handle.send(Command::Batch { batch: batch.clone(), action: item_action(action) });
        Ok(Outcome::Done)
    }

    /// Tạm dừng mọi mục đang chờ / chạy, hoặc tiếp tục mọi mục đang tạm dừng (mọi lý do — người dùng vừa giải phóng
    /// chỗ cũng bấm "Tiếp tục tất cả").
    fn all(&mut self, action: Action) -> Result<Outcome, CommandError> {
        let uploads = lock(&self.store).uploads().map_err(internal)?;
        for item in uploads {
            let paused = matches!(item.state, UploadState::Paused { .. });
            let hit = match action {
                Action::Pause => !paused && !item.state.is_terminal(),
                Action::Resume => paused,
                Action::Cancel | Action::Retry => return Err(CommandError::InvalidInput),
            };
            if hit && let Some(r) = self.runners.get(&item.device_id) {
                r.handle.send(Command::Item { id: item.id, action: item_action(action) });
            }
        }
        Ok(Outcome::Done)
    }

    fn require_paired(&self, device: &DeviceId) -> Result<(), CommandError> {
        if self.runners.contains_key(device) {
            Ok(())
        } else if self.devices.get(device).is_some() {
            Err(CommandError::NotPaired)
        } else {
            Err(CommandError::UnknownDevice)
        }
    }

    fn online_client(&self, device: &DeviceId) -> Result<Arc<ferry_core::client::PhoneClient>, CommandError> {
        self.require_paired(device)?;
        let runner = &self.runners[device];
        runner.link.borrow().as_ref().map(|l| l.client.clone()).ok_or(CommandError::Offline)
    }

    // ---- thiết bị ----

    fn ensure_runner(&mut self, id: &DeviceId) {
        if self.runners.contains_key(id) {
            return;
        }
        let (link, watch) = link_channel();
        let (runner, handle) = UploadRunner::new(
            id.clone(),
            self.store.clone(),
            watch,
            self.throttle.clone(),
            self.transfer_tx.clone(),
            self.runner_cfg.clone(),
        );
        let task = tokio::spawn(runner.run());
        log::info!("engine: runner for {id}");
        self.runners.insert(id.clone(), Runner { handle, link, task });
    }

    /// Nối thiết bị có token (đọc token trong kho khoá) qua mọi địa chỉ biết được (+ `extra` nhập tay, thử trước).
    fn probe_device(&mut self, id: &DeviceId, extra: Option<SocketAddr>) {
        if !self.probing.insert(id.clone()) {
            return;
        }
        let mut addrs = self.devices.addresses(id);
        if let Some(a) = extra {
            addrs.retain(|x| *x != a);
            addrs.insert(0, a);
        }
        let (id, tokens, tx) = (id.clone(), self.tokens.clone(), self.internal_tx.clone());
        tokio::spawn(async move {
            let msg = match tokens.get(&id).await {
                Ok(Some(token)) => {
                    let result = discovery::connect(&id, &addrs, Some(&token)).await;
                    Internal::Connected { id, result }
                }
                Ok(None) => Internal::NoToken { id },
                Err(e) => {
                    log::warn!("engine: read token of {id}: {e}");
                    Internal::KeyringFailed { id }
                }
            };
            let _ = tx.send(msg);
        });
    }

    fn on_deadline(&mut self) {
        let now = Instant::now();
        let ev = self.devices.tick(now);
        self.on_device_events(ev);
        for id in self.devices.due_probes(now) {
            self.probe_device(&id, None);
        }
    }

    fn on_device_events(&mut self, events: Vec<DeviceEvent>) {
        if events.is_empty() {
            return;
        }
        self.dirty = true;
        for ev in events {
            log::debug!("engine: {ev:?}");
            if let DeviceEvent::StateChanged { id, to, .. } = ev {
                if to != DeviceState::Online
                    && let Some(r) = self.runners.get(&id)
                {
                    r.link.send_replace(None);
                }
                match to {
                    DeviceState::Online => {
                        self.told_repair.remove(&id);
                    }
                    DeviceState::NeedsRepair => self.notify_repair(&id),
                    _ => {}
                }
            }
        }
        self.save_config();
    }

    fn on_internal(&mut self, msg: Internal) {
        let now = Instant::now();
        match msg {
            Internal::Connected { id, result } => {
                self.probing.remove(&id);
                // Quên trong lúc nối ⇒ bỏ kết quả.
                if !self.devices.get(&id).is_some_and(|d| d.state.has_token()) {
                    return;
                }
                match result {
                    Ok(Connected { client, info, addr }) => {
                        let ev = self.devices.info_ok(&info, addr, now);
                        self.on_device_events(ev);
                        if self.devices.get(&id).is_some_and(|d| d.state == DeviceState::Online) {
                            log::info!("engine: {id} online at {addr}");
                            self.ensure_runner(&id);
                            self.runners[&id].link.send_replace(Some(Link::new(client, &info)));
                            self.dirty = true;
                        }
                    }
                    Err(e) => {
                        log::debug!("engine: connect {id}: {e}");
                        let ev = self.devices.connect_failed(&id, now);
                        self.on_device_events(ev);
                    }
                }
            }
            Internal::NoToken { id } => {
                self.probing.remove(&id);
                log::warn!("engine: no token for {id} in the keyring, needs pairing again");
                let ev = self.devices.unauthorized(&id);
                self.on_device_events(ev);
            }
            Internal::KeyringFailed { id } => {
                self.probing.remove(&id);
                let ev = self.devices.connect_failed(&id, now);
                self.on_device_events(ev);
            }
            Internal::Paired { outbox, req, result } => {
                let r = result.map(|p| {
                    let ev = self.devices.paired(&p.id, now);
                    self.on_device_events(ev);
                    self.ensure_runner(&p.id);
                    // Ghép lại sau 401: runner chờ `Link` mới — probe ngay đưa nó về Online.
                    self.save_config();
                    self.on_deadline();
                    Outcome::Paired { device: p.id, name: p.name }
                });
                reply_to(&outbox, req, r);
            }
            Internal::Probed { outbox, req, addr, result } => {
                let r = result.map(|info| {
                    let has_token = self.devices.get(&info.id).is_some_and(|d| d.state.has_token());
                    if has_token {
                        // `info` không token luôn `paired: false` — không cho máy trạng thái nhầm thành NeedsRepair:
                        // nối lại kèm token, địa chỉ mới thử trước.
                        self.probing.remove(&info.id);
                        self.probe_device(&info.id, Some(addr));
                    } else {
                        let ev = self.devices.info_ok(&info, addr, now);
                        self.on_device_events(ev);
                    }
                    Outcome::Found { device: info.id, name: info.name }
                });
                reply_to(&outbox, req, r);
            }
            Internal::Added { outbox, req, device, outcome } => {
                if matches!(outcome, Outcome::Added { batch: Some(_), .. })
                    && let Some(r) = self.runners.get(&device)
                {
                    r.handle.send(Command::Wake);
                }
                self.dirty = true;
                reply_to(&outbox, req, Ok(outcome));
            }
            Internal::Forgot { outbox, req } => {
                self.dirty = true;
                reply_to(&outbox, req, Ok(Outcome::Done));
            }
        }
    }

    fn on_transfer(&mut self, ev: TransferEvent) {
        match ev {
            TransferEvent::Progress { id, sent, size, bytes_per_sec } => {
                self.progress.insert(id, ItemProgress { id, sent, size, bytes_per_sec });
                self.progress_dirty.insert(id);
            }
            TransferEvent::ItemChanged { id, state } => {
                self.dirty = true;
                match &state {
                    UploadState::Sending { offset, .. } => {
                        if let Some(p) = self.progress.get_mut(&id) {
                            p.sent = p.sent.max(*offset);
                        }
                        if let Some(dev) = self.item_device(id) {
                            self.told_no_space.remove(&dev);
                        }
                    }
                    UploadState::Paused { reason: PauseReason::NoSpace { short_by }, .. } => {
                        self.progress.remove(&id);
                        if let Some(dev) = self.item_device(id) {
                            self.notify_no_space(&dev, *short_by);
                        }
                    }
                    _ => {
                        self.progress.remove(&id);
                    }
                }
            }
            TransferEvent::BatchFinished { batch, sent, skipped, failed } => {
                self.dirty = true;
                self.notify_batch(&batch, sent, skipped, failed);
            }
            TransferEvent::DeviceNeedsRepair { device } => {
                let ev = self.devices.unauthorized(&device);
                self.on_device_events(ev);
            }
            TransferEvent::ConnectionLost { device, addr } => {
                log::info!("engine: lost {device} at {addr}, reconnecting");
                self.devices.address_failed(&device, addr);
                self.probe_device(&device, None);
            }
            TransferEvent::PhoneNeedsUpdate { device } => {
                // UI đọc `features` của DeviceView (thiếu `dirs` ⇒ nhắc cập nhật app điện thoại).
                log::info!("engine: {device} has no 'dirs', folders are sent flat");
            }
        }
    }

    fn item_device(&self, id: ItemId) -> Option<DeviceId> {
        lock(&self.store).upload(id).ok().flatten().map(|i| i.device_id)
    }

    // ---- thông báo ----

    fn phone_name(&self, id: &DeviceId) -> String {
        self.devices.get(id).map(|d| d.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| id.0.clone())
    }

    fn notify(&self, title: String, body: String) {
        (self.sinks.notice)(Notice { title, body });
    }

    fn notify_batch(&self, batch: &BatchId, sent: u64, skipped: u64, failed: u64) {
        let device = lock(&self.store).batches().ok().and_then(|b| b.into_iter().find(|b| b.id == *batch));
        let phone = device.map_or_else(String::new, |b| self.phone_name(&b.device_id));
        let t = |key: &str, n: u64| self.i18n.t_args(&plural(key, n), &[("count", &n.to_string()), ("phone", &phone)]);
        let mut lines = Vec::new();
        if sent > 0 {
            lines.push(t("notice.batch_done", sent));
        }
        if skipped > 0 {
            lines.push(t("notice.already_there", skipped));
        }
        if failed > 0 {
            lines.push(t("notice.failed", failed));
        }
        // Cả lô bị huỷ ⇒ người dùng tự bấm, không báo.
        if lines.is_empty() {
            return;
        }
        let title = lines.remove(0);
        self.notify(title, lines.join("\n"));
    }

    fn notify_repair(&mut self, id: &DeviceId) {
        let pending = lock(&self.store)
            .uploads()
            .map(|u| u.iter().any(|i| i.device_id == *id && !i.state.is_terminal()))
            .unwrap_or(false);
        if !pending || !self.told_repair.insert(id.clone()) {
            return;
        }
        let phone = self.phone_name(id);
        let title = self.i18n.t_args("notice.needs_repair_title", &[("phone", &phone)]);
        self.notify(title, self.i18n.t("notice.needs_repair_body").to_owned());
    }

    fn notify_no_space(&mut self, id: &DeviceId, short_by: Option<u64>) {
        if !self.told_no_space.insert(id.clone()) {
            return;
        }
        let phone = self.phone_name(id);
        let title = self.i18n.t_args("notice.no_space_title", &[("phone", &phone)]);
        let body = match short_by {
            Some(n) => self
                .i18n
                .t_args("notice.no_space_body_by", &[("size", &tray_state::format_size(n, self.i18n.language()))]),
            None => self.i18n.t("notice.no_space_body").to_owned(),
        };
        self.notify(title, body);
    }

    // ---- đẩy cho UI + khay ----

    fn push_due(&self) -> Option<Instant> {
        (self.dirty || !self.progress_dirty.is_empty()).then_some(self.last_push + PUSH_INTERVAL)
    }

    /// `force` ⇒ tính lại ngay kể cả khi không đổi (lúc khởi động).
    fn push(&mut self, force: bool) {
        self.last_push = Instant::now();
        if self.dirty || force {
            self.dirty = false;
            self.progress_dirty.clear();
            let uploads = match lock(&self.store).uploads() {
                Ok(u) => u,
                Err(e) => {
                    log::warn!("engine: read queue: {e}");
                    return;
                }
            };
            self.summary =
                tray_state::summarize(&uploads, |id| self.devices.get(id).map(|d| (d.state, d.name.clone())));
            self.update_tray();
            if self.subscribers.is_empty() {
                return;
            }
            let snapshot = self.snapshot_from(uploads);
            for s in self.subscribers.values_mut() {
                send_full(s, &snapshot);
            }
        } else {
            let items: Vec<ItemProgress> =
                self.progress_dirty.drain().filter_map(|id| self.progress.get(&id).copied()).collect();
            self.update_tray();
            if items.is_empty() || self.subscribers.is_empty() {
                return;
            }
            let mut snapshot = None;
            for s in self.subscribers.values_mut() {
                if s.resync {
                    let snap = snapshot.get_or_insert_with(|| {
                        let uploads = lock(&self.store).uploads().unwrap_or_default();
                        snapshot_parts(&self.store, &self.devices, &self.locals, &self.summary, &self.progress, uploads)
                    });
                    send_full(s, snap);
                } else if s.outbox.backed_up() {
                    s.resync = true;
                } else {
                    s.outbox.push(DaemonToUi::Progress(ProgressDelta { items: items.clone() }));
                }
            }
        }
    }

    fn update_tray(&mut self) {
        let status = tray_state::status(&self.summary, &self.progress, &self.i18n);
        if self.tray.as_ref() != Some(&status) {
            (self.sinks.tray)(status.clone());
            self.tray = Some(status);
        }
    }

    fn snapshot(&self) -> Snapshot {
        let uploads = lock(&self.store).uploads().unwrap_or_else(|e| {
            log::warn!("engine: read queue: {e}");
            Vec::new()
        });
        self.snapshot_from(uploads)
    }

    fn snapshot_from(&self, uploads: Vec<UploadItem>) -> Snapshot {
        snapshot_parts(&self.store, &self.devices, &self.locals, &self.summary, &self.progress, uploads)
    }

    // ---- việc nền ----

    fn refresh_locals(&mut self) {
        let locals = address::local_interfaces();
        if locals != self.locals {
            self.locals = locals.clone();
            let ev = self.devices.set_local_interfaces(locals);
            self.on_device_events(ev);
        }
    }

    fn purge(&mut self) {
        match lock(&self.store).purge_finished(now_ms()) {
            Ok(0) => {}
            Ok(n) => {
                log::info!("engine: purged {n} finished item(s)");
                self.dirty = true;
            }
            Err(e) => log::warn!("engine: purge: {e}"),
        }
        self.refresh_locals();
    }

    /// Ghi config khi danh sách điện thoại đã ghép (tên, địa chỉ) đổi.
    fn save_config(&mut self) {
        let devices = self.devices.to_config();
        if devices == self.config.devices {
            return;
        }
        // Giữ thứ tự cũ, thêm mới ở cuối (diff config dễ đọc).
        let mut next: Vec<_> =
            self.config.devices.iter().filter_map(|d| devices.iter().find(|n| n.id == d.id)).cloned().collect();
        next.extend(devices.iter().filter(|n| !self.config.devices.iter().any(|d| d.id == n.id)).cloned());
        if next == self.config.devices {
            return;
        }
        self.config.devices = next;
        if let Err(e) = config::save(&self.config_path, &self.config) {
            log::warn!("engine: save {}: {e}", self.config_path.display());
        }
    }

    async fn stop_runners(&mut self) {
        let runners: Vec<Runner> = self.runners.drain().map(|(_, r)| r).collect();
        for r in &runners {
            r.handle.send(Command::Stop);
        }
        let all = futures::future::join_all(runners.into_iter().map(|r| r.task));
        if tokio::time::timeout(STOP_TIMEOUT, all).await.is_err() {
            log::warn!("engine: runners did not stop in time");
        }
    }
}

fn send_full(s: &mut Subscriber, snapshot: &Snapshot) {
    if s.outbox.backed_up() {
        s.resync = true;
    } else {
        s.resync = !s.outbox.push(DaemonToUi::Snapshot(snapshot.clone()));
    }
}

/// Ảnh chụp: [`QUEUED_IN_SNAPSHOT`] mục chưa xong đầu hàng + [`FINISHED_IN_SNAPSHOT`] mục xong mới nhất, lô có mục trong ảnh, mọi thiết bị trong sổ.
fn snapshot_parts(
    store: &SharedStore,
    devices: &Devices,
    locals: &[LocalInterface],
    summary: &Summary,
    progress: &HashMap<ItemId, ItemProgress>,
    uploads: Vec<UploadItem>,
) -> Snapshot {
    let (mut live, mut finished): (Vec<UploadItem>, Vec<UploadItem>) =
        uploads.into_iter().partition(|u| !u.state.is_terminal());
    live.sort_by_key(|u| u.id);
    let queued_omitted = live.len().saturating_sub(QUEUED_IN_SNAPSHOT) as u64;
    live.truncate(QUEUED_IN_SNAPSHOT);
    finished.sort_by_key(|u| std::cmp::Reverse((u.updated, u.id)));
    let finished_omitted = finished.len().saturating_sub(FINISHED_IN_SNAPSHOT) as u64;
    finished.truncate(FINISHED_IN_SNAPSHOT);
    let mut uploads = live;
    uploads.extend(finished);
    uploads.sort_by_key(|u| u.id);

    let referenced: HashSet<&BatchId> = uploads.iter().filter_map(|u| u.batch_id.as_ref()).collect();
    let batches = {
        let store = lock(store);
        store
            .batches()
            .unwrap_or_default()
            .into_iter()
            .filter(|b| referenced.contains(&b.id))
            .filter_map(|info| store.batch_progress(&info.id).ok().map(|progress| BatchView { info, progress }))
            .collect()
    };
    let devices = devices
        .iter()
        .map(|d| DeviceView {
            id: d.id.clone(),
            name: d.name.clone(),
            state: d.state,
            visible: d.visible(),
            newer_api: d.newer_api,
            features: d.features.clone(),
            address: d
                .winner()
                .or_else(|| devices.addresses(&d.id).first().copied())
                .map(|a| address::format_address(&a, locals)),
        })
        .collect();
    let progress = uploads.iter().filter_map(|u| progress.get(&u.id).copied()).collect();
    Snapshot {
        devices,
        batches,
        uploads,
        progress,
        finished_omitted,
        queued_omitted,
        can_pause_all: summary.can_pause_all(),
        can_resume_all: summary.can_resume_all(),
    }
}

fn add_files(
    store: &SharedStore,
    device: &DeviceId,
    paths: &[PathBuf],
    dest_dir: &str,
) -> Result<Outcome, CommandError> {
    let plan = plan_batch(paths, dest_dir).map_err(|_| CommandError::InvalidInput)?;
    let (files, skipped_non_video, unreadable) =
        (plan.files.len() as u64, plan.skipped_non_video as u64, plan.unreadable as u64);
    let batch = if plan.files.is_empty() {
        None
    } else {
        let (batch, _) = lock(store).add_batch(device, &plan, now_ms()).map_err(internal)?;
        log::info!("engine: queued {files} file(s) for {device}");
        Some(batch)
    };
    Ok(Outcome::Added { batch, files, skipped_non_video, unreadable })
}

fn reply_to(outbox: &Outbox, req: u64, r: Result<Outcome, CommandError>) {
    let outcome = r.unwrap_or_else(|error| Outcome::Failed { error });
    outbox.push(DaemonToUi::Reply { req, outcome });
}

fn item_action(a: Action) -> ItemAction {
    match a {
        Action::Pause => ItemAction::Pause,
        Action::Resume => ItemAction::Resume,
        Action::Cancel => ItemAction::Cancel,
        Action::Retry => ItemAction::Retry,
    }
}

fn pair_error(e: PairError) -> CommandError {
    match e {
        PairError::WrongPin => CommandError::WrongPin,
        PairError::RateLimited { retry_after } => CommandError::RateLimited { retry_after_secs: retry_after.as_secs() },
        PairError::OtherPhone { .. } => CommandError::OtherPhone,
        PairError::Keyring(_) => CommandError::Keyring,
        PairError::Client(e) => {
            log::info!("engine: pair: {e}");
            CommandError::Unreachable
        }
    }
}

fn internal(e: impl std::fmt::Display) -> CommandError {
    log::warn!("engine: {e}");
    CommandError::Internal
}

fn lock(store: &SharedStore) -> std::sync::MutexGuard<'_, QueueStore> {
    store.lock().unwrap_or_else(|p| p.into_inner())
}

async fn next_mdns(browser: Option<&Browser>) -> Option<MdnsEvent> {
    match browser {
        Some(b) => b.next().await,
        None => std::future::pending().await,
    }
}

async fn sleep_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[path = "../../core/tests/fake_phone/mod.rs"]
mod fake_phone;

#[cfg(test)]
mod tests {
    //! Engine thật + điện thoại giả trong tiến trình (`ferry-core/tests/fake_phone`), kho khoá trong RAM, không mDNS.

    use std::path::Path;

    use ferry_core::config::PairedDevice;
    use ferry_core::i18n::Language;
    use ferry_core::pairing::MemoryKeyring;

    use super::fake_phone::{self, FakePhone};
    use super::*;
    use crate::ipc_server::OutboxReceiver;
    use crate::tray_state::TrayMode;

    const CLIENT_ID: &str = "0e5e6d0c-test-engine";
    const WAIT: Duration = Duration::from_secs(30);

    fn device() -> DeviceId {
        DeviceId(fake_phone::ID.into())
    }

    #[derive(Default)]
    struct Captured {
        tray: Vec<TrayStatus>,
        notices: Vec<Notice>,
    }

    struct Rig {
        engine: EngineHandle,
        captured: Arc<std::sync::Mutex<Captured>>,
        store: SharedStore,
    }

    async fn start(dir: &Path, phone: &FakePhone, throttle: Option<u64>) -> Rig {
        let token = phone.issue_token(CLIENT_ID);
        let tokens = Tokens::new(Arc::new(MemoryKeyring::new()));
        tokens.set(&device(), &Secret::new(token)).await.unwrap();
        start_with(dir, phone, tokens, throttle)
    }

    fn start_with(dir: &Path, phone: &FakePhone, tokens: Tokens, throttle: Option<u64>) -> Rig {
        let config = Config {
            client_id: CLIENT_ID.into(),
            client_name: "Test PC".into(),
            devices: vec![PairedDevice {
                id: device(),
                name: fake_phone::NAME.into(),
                addresses: vec![phone.v4().to_string()],
            }],
            ..Config::default()
        };
        let store = Arc::new(std::sync::Mutex::new(QueueStore::open(&dir.join("queue.sqlite3"), now_ms()).unwrap()));
        let captured = Arc::new(std::sync::Mutex::new(Captured::default()));
        let (c1, c2) = (captured.clone(), captured.clone());
        let sinks = Sinks {
            tray: Box::new(move |s| c1.lock().unwrap().tray.push(s)),
            notice: Box::new(move |n| c2.lock().unwrap().notices.push(n)),
        };
        let cfg = EngineConfig {
            config_path: dir.join("config.json"),
            config,
            store: store.clone(),
            tokens,
            mdns: false,
            runner: RunnerConfig {
                chunk_size: 256 * 1024,
                progress_interval: Duration::from_millis(10),
                ..RunnerConfig::default()
            },
            throttle: Throttle::new(throttle),
            i18n: I18n::new(Language::En),
        };
        let engine = spawn(&tokio::runtime::Handle::current(), cfg, sinks);
        Rig { engine, captured, store }
    }

    fn write_videos(dir: &Path, n: usize, len: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| {
                let p = dir.join(format!("clip{i}.mp4"));
                std::fs::write(&p, (0..len).map(|b| ((b * 7 + i) % 251) as u8).collect::<Vec<u8>>()).unwrap();
                p
            })
            .collect()
    }

    /// Nhận trên luồng chặn tới khi `done` khớp; trả mọi thông điệp kèm lúc nhận.
    async fn collect(
        rx: OutboxReceiver,
        done: impl Fn(&DaemonToUi) -> bool + Send + 'static,
    ) -> (OutboxReceiver, Vec<(Instant, DaemonToUi)>) {
        tokio::task::spawn_blocking(move || {
            let deadline = Instant::now() + WAIT;
            let mut got = Vec::new();
            while Instant::now() < deadline {
                let Some(msg) = rx.recv_timeout(Duration::from_millis(200)) else { continue };
                let stop = done(&msg);
                got.push((Instant::now(), msg));
                if stop {
                    return (rx, got);
                }
            }
            panic!("timed out; got {} message(s)", got.len());
        })
        .await
        .unwrap()
    }

    fn is_online(msg: &DaemonToUi) -> bool {
        matches!(msg, DaemonToUi::Snapshot(s) if s.devices.iter().any(|d| d.state == DeviceState::Online))
    }

    fn all_done(msg: &DaemonToUi, n: usize) -> bool {
        matches!(msg, DaemonToUi::Snapshot(s)
            if s.uploads.len() == n && s.uploads.iter().all(|u| matches!(u.state, UploadState::Done { .. })))
    }

    fn add_files(engine: &EngineHandle, outbox: &Outbox, paths: Vec<PathBuf>) {
        let op = QueueOp::AddFiles { device: device(), paths, dest_dir: String::new() };
        engine.send(Input::Request { outbox: outbox.clone(), req: 7, op: Op::Queue(op) });
    }

    async fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let deadline = Instant::now() + WAIT;
        while !ok() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// C1: UI mới nối nhận ảnh chụp đầy đủ ngay; sau đó đẩy (ảnh chụp / tiến độ) cách nhau ≥ 100 ms (≤ 10 lần / s).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn c1_full_snapshot_then_at_most_ten_pushes_per_second() {
        let tmp = tempfile::tempdir().unwrap();
        let phone = FakePhone::start().await;
        // 2 MB ở 1 MB/s ⇒ ~2 s gửi, runner phát tiến độ mỗi 10 ms.
        let rig = start(tmp.path(), &phone, Some(1024 * 1024)).await;
        let (outbox, rx) = Outbox::new();
        let t0 = Instant::now();
        rig.engine.send(Input::Subscribe { conn: 1, outbox: outbox.clone() });
        let (rx, first) = collect(rx, |_| true).await;
        assert!(matches!(first[0].1, DaemonToUi::Snapshot(_)), "first message is a full snapshot");
        assert!(first[0].0 - t0 < Duration::from_millis(500), "snapshot arrives right away");
        let (rx, _) = collect(rx, is_online).await;

        add_files(&rig.engine, &outbox, write_videos(tmp.path(), 1, 2 * 1024 * 1024));
        let (_rx, got) = collect(rx, |m| all_done(m, 1)).await;
        let pushes: Vec<Instant> = got
            .iter()
            .filter(|(_, m)| matches!(m, DaemonToUi::Snapshot(_) | DaemonToUi::Progress(_)))
            .map(|(t, _)| *t)
            .collect();
        let progress = got.iter().filter(|(_, m)| matches!(m, DaemonToUi::Progress(_))).count();
        assert!(progress >= 5, "progress deltas while sending: {progress}");
        assert!(
            got.iter()
                .any(|(_, m)| matches!(m, DaemonToUi::Reply { req: 7, outcome: Outcome::Added { files: 1, .. } }))
        );
        // Đồng hồ phía nhận lệch chút so với phía gửi ⇒ dung sai 20 ms.
        for w in pushes.windows(2) {
            let gap = w[1] - w[0];
            assert!(gap >= Duration::from_millis(80), "two pushes {gap:?} apart");
        }
        let span = (*pushes.last().unwrap() - pushes[0]).as_secs_f64();
        assert!(
            pushes.len() as f64 <= span * f64::from(MAX_PUSH_PER_SEC) + 2.0,
            "{} pushes in {span:.2} s",
            pushes.len()
        );
        rig.engine.shutdown();
    }

    /// C2: 3 tệp ⇒ khay đang gửi rồi về rảnh, một thông báo cho cả lô, điện thoại có đủ 3 tệp.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn c2_three_files_tray_goes_sending_then_idle_with_one_notice() {
        let tmp = tempfile::tempdir().unwrap();
        let phone = FakePhone::start().await;
        let rig = start(tmp.path(), &phone, Some(4 * 1024 * 1024)).await;
        let (outbox, rx) = Outbox::new();
        rig.engine.send(Input::Subscribe { conn: 1, outbox: outbox.clone() });
        let (rx, _) = collect(rx, is_online).await;
        add_files(&rig.engine, &outbox, write_videos(tmp.path(), 3, 1024 * 1024));
        let (_rx, _) = collect(rx, |m| all_done(m, 3)).await;
        wait_for("batch notice", || !rig.captured.lock().unwrap().notices.is_empty()).await;
        tokio::time::sleep(Duration::from_millis(300)).await;

        let c = rig.captured.lock().unwrap();
        let modes: Vec<TrayMode> = c.tray.iter().map(|s| s.mode).collect();
        let sending = modes.iter().position(|m| matches!(m, TrayMode::Sending { .. })).expect("tray went sending");
        assert_eq!(modes.last(), Some(&TrayMode::Idle), "{modes:?}");
        assert!(sending < modes.len() - 1);
        assert!(c.tray.iter().any(|s| s.tooltip.starts_with("Sending 3 files")), "{:?}", c.tray);
        assert_eq!(c.notices.len(), 1, "{:?}", c.notices);
        assert_eq!(c.notices[0].title, format!("Sent 3 files to {}", fake_phone::NAME));
        assert_eq!(phone.files().len(), 3);
        drop(c);
        rig.engine.shutdown();
    }

    /// C3 (mức engine): tắt giữa khúc rồi mở lại trên cùng hàng đợi ⇒ gửi tiếp từ offset điện thoại đã có, tệp đúng.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn c3_restart_mid_chunk_resumes() {
        let tmp = tempfile::tempdir().unwrap();
        let phone = FakePhone::start().await;
        let token = phone.issue_token(CLIENT_ID);
        let keyring = Arc::new(MemoryKeyring::new());
        let tokens = Tokens::new(keyring.clone());
        tokens.set(&device(), &Secret::new(token)).await.unwrap();
        let src = write_videos(tmp.path(), 1, 3 * 1024 * 1024);
        let expected = std::fs::read(&src[0]).unwrap();

        let rig = start_with(tmp.path(), &phone, tokens.clone(), Some(1024 * 1024));
        let (outbox, rx) = Outbox::new();
        rig.engine.send(Input::Subscribe { conn: 1, outbox: outbox.clone() });
        let (_rx, _) = collect(rx, is_online).await;
        add_files(&rig.engine, &outbox, src);
        wait_for("half sent", || phone.uploads_in_progress().iter().any(|(_, off)| *off >= 1024 * 1024)).await;
        rig.engine.shutdown();
        drop(rig);
        let (_, at_stop) = phone.uploads_in_progress()[0].clone();
        assert!(at_stop < expected.len() as u64);

        let rig = start_with(tmp.path(), &phone, tokens, None);
        let resumed = lock(&rig.store).uploads().unwrap()[0].clone();
        assert!(resumed.state.bytes_done(resumed.size) > 0, "reopened item keeps its offset: {:?}", resumed.state);
        let (outbox, rx) = Outbox::new();
        rig.engine.send(Input::Subscribe { conn: 1, outbox });
        let (_rx, _) = collect(rx, |m| all_done(m, 1)).await;
        let files = phone.files();
        assert_eq!(files.len(), 1);
        assert_eq!(*files[0].data, expected);
        rig.engine.shutdown();
    }

    /// Lô rất lớn: ảnh chụp giữ đầu hàng đợi + mục xong mới nhất, đếm phần bỏ, dòng JSON dưới `MAX_LINE`.
    #[test]
    fn huge_queue_snapshot_stays_under_the_line_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let store: SharedStore =
            Arc::new(std::sync::Mutex::new(QueueStore::open(&tmp.path().join("q.sqlite3"), now_ms()).unwrap()));
        let item = |id: i64, state: UploadState| UploadItem {
            id,
            device_id: device(),
            batch_id: None,
            path: PathBuf::from(format!("/home/someone/Videos/a rather long folder name/trip {id}/VID_{id:08}.mp4")),
            name: format!("VID_{id:08}.mp4"),
            dir: format!("a rather long folder name/trip {id}"),
            size: 123_456_789,
            mtime: Some(1_762_761_600_000),
            state,
            failures: 0,
            created: id,
            updated: id,
        };
        let done = || UploadState::Done { remote_name: "x.mp4".into(), remote_dir: Some("a/b".into()) };
        let mut uploads: Vec<UploadItem> = (1..=5000).map(|i| item(i, UploadState::Queued)).collect();
        uploads.extend((5001..=6000).map(|i| item(i, done())));
        let snap = snapshot_parts(&store, &Devices::default(), &[], &Summary::default(), &HashMap::new(), uploads);
        assert_eq!(snap.uploads.len(), QUEUED_IN_SNAPSHOT + FINISHED_IN_SNAPSHOT);
        assert_eq!(snap.queued_omitted, 5000 - QUEUED_IN_SNAPSHOT as u64);
        assert_eq!(snap.finished_omitted, 1000 - FINISHED_IN_SNAPSHOT as u64);
        assert_eq!(snap.uploads[0].id, 1, "head of the queue first");
        assert_eq!(snap.uploads.last().unwrap().id, 6000, "newest finished kept");
        let line = serde_json::to_string(&DaemonToUi::Snapshot(snap)).unwrap();
        assert!((line.len() as u64) < ferry_core::ipc::MAX_LINE, "{} bytes", line.len());
    }
}
