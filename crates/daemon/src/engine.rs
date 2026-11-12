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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferry_core::client::{ClientError, PhoneClient};
use ferry_core::config::{self, Config};
use ferry_core::device::{DeviceEvent, DeviceId, DeviceState, Devices};
use ferry_core::discovery::address::{self, AddressError, LocalInterface};
use ferry_core::discovery::mdns::{Browser, MdnsEvent};
use ferry_core::discovery::{self, ConnectError, Connected, scan};
use ferry_core::firewall::{self, AllowResult};
use ferry_core::i18n::I18n;
use ferry_core::ipc::{
    Action, BatchView, CommandError, DOWNLOADS_IN_SNAPSHOT, DaemonToUi, DeviceSetting, DeviceView,
    FINISHED_IN_SNAPSHOT, ItemProgress, LibraryOp, MAX_PUSH_PER_SEC, Outcome, PhoneOp, ProgressDelta,
    QUEUED_IN_SNAPSHOT, QueueOp, Restore, SCAN_MIN_INTERVAL_SECS, SettingsOp, SettingsView, Snapshot,
};
use ferry_core::pairing::{self, Identity, PairError, Tokens};
use ferry_core::protocol::{DirListing, Info, LibraryItem, Secret, feature};
use ferry_core::queue::state::PauseReason;
use ferry_core::queue::store::{DownloadItem, NewDownload, UploadItem};
use ferry_core::queue::{BatchId, DownloadState, ItemId, QueueStore, UploadState, now_ms, plan_batch};
use ferry_core::transfer::download;
use ferry_core::transfer::{
    Command, ItemAction, Link, LinkSender, RunnerConfig, RunnerHandle, SharedStore, TransferEvent, UploadRunner,
    link_channel,
};
use ferry_core::view::{CachedData, DirCache, ViewState};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

mod export;

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
    pub i18n: I18n,
    /// Áp công tắc "Khởi động cùng hệ thống" vào OS (`autostart::apply`; test thay bằng hàm giả). Chạy trong
    /// `spawn_blocking` (Windows gọi `schtasks`).
    pub autostart: fn(bool) -> Result<(), String>,
    /// Lỗi của lần đồng bộ autostart lúc khởi động (`autostart::sync_on_start`).
    pub autostart_error: Option<String>,
    /// Thư mục tạm của Cắt / Sao chép sang máy tính (W-10): `<data_dir>/clipboard`, mỗi lần một thư mục con.
    pub export_dir: PathBuf,
    pub clipboard: crate::clipboard::Clipboard,
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
    Settings(SettingsOp),
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
    /// `UiToDaemon::SaveView`: giữ trong RAM, trả lại cho UI nối sau (`DaemonToUi::Restore`).
    SaveView(Box<ViewState>),
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
/// Ghép lại một điện thoại mà địa chỉ của nó giờ trả lời bằng mã khác (app cài lại): `info` + địa chỉ của mã mới.
#[derive(Debug)]
struct Replaced {
    old: DeviceId,
    info: Info,
    addr: SocketAddr,
}

/// Hai lượt quét thưa cho máy đã ghép Offline cách nhau ít nhất chừng này (PROTOCOL 6.2).
const OFFLINE_SCAN_EVERY: Duration = Duration::from_secs(300);
/// Máy Online chỉ đọc `info` lúc nối ⇒ `features` (`browse`, `dirs`) đổi sau đó (bật công tắc, cấp / thu hồi quyền) PC không
/// biết. Mở / làm mới màn duyệt điện thoại (`LibraryOp::List`, `Dirs`) đọc lại `info`, mỗi máy cách nhau ít nhất chừng này.
const INFO_REFRESH_EVERY: Duration = Duration::from_secs(3);

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
        result: Result<(pairing::Paired, Option<Replaced>), CommandError>,
    },
    /// Hàng đợi của mã cũ đã chuyển sang máy cài lại (`Replaced`) ⇒ đánh thức runner mã mới.
    Moved {
        device: DeviceId,
    },
    /// Lệnh ngoài runner (`list`) lỗi mạng ở địa chỉ đang dùng ⇒ như `TransferEvent::ConnectionLost`.
    LinkFailed {
        device: DeviceId,
        addr: SocketAddr,
    },
    /// `info` đọc lại của máy đang Online (kèm token) — cập nhật `features`, `paired: false` ⇒ cần ghép lại.
    InfoRefreshed {
        device: DeviceId,
        addr: SocketAddr,
        info: Info,
    },
    /// `info` của địa chỉ nhập tay ở màn Thêm điện thoại (`PhoneOp::AddAddress`).
    Probed {
        outbox: Outbox,
        req: u64,
        addr: SocketAddr,
        result: Result<Info, CommandError>,
    },
    /// Cài đặt cần đĩa / OS (soát thư mục, `schtasks`) xong trong `spawn_blocking` — áp + lưu ở vòng chính.
    Settled {
        outbox: Outbox,
        req: u64,
        change: Settled,
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
    /// Quét LAN dự phòng xong (`discovery::scan`).
    Scanned {
        found: Vec<(SocketAddr, Info)>,
        /// Lượt đầy đủ (màn Thêm điện thoại) hay lượt thưa (máy Offline).
        full: bool,
    },
    /// `pkexec ufw allow …` xong (W-6).
    FirewallAllowed {
        outbox: Outbox,
        req: u64,
        result: AllowResult,
    },
    /// Cắt / Sao chép sang máy tính (W-10, `engine/export.rs`).
    Export(export::ExportMsg),
    /// `GET /dirs` xong (`LibraryOp::Dirs`) ⇒ bộ đệm; `None` = thư mục không còn (`404`).
    Listed {
        device: DeviceId,
        path: String,
        listing: Option<DirListing>,
    },
    /// `GET /list` xong ⇒ bộ đệm.
    LibraryRead {
        device: DeviceId,
        items: Vec<LibraryItem>,
    },
}

/// Thay đổi cài đặt đã soát xong ngoài vòng chính.
enum Settled {
    DownloadDir(Option<PathBuf>, Result<(), CommandError>),
    /// Lỗi OS không chặn: cấu hình vẫn theo công tắc, lỗi hiện ở màn cài đặt (lần khởi động sau thử lại).
    Autostart(bool, Result<(), String>),
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
    i18n: I18n,
    autostart: fn(bool) -> Result<(), String>,
    autostart_error: Option<String>,
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
    /// W-6: tường lửa đoán lúc khởi động (`firewall::detect`); chỉ khi có dò mDNS.
    firewall: firewall::Detected,
    /// Dò mDNS từ lúc này (`None` ⇒ không dò).
    mdns_since: Option<Instant>,
    /// Đã thấy ít nhất một điện thoại qua mDNS ⇒ tường lửa không chặn.
    mdns_found: bool,
    /// Hộp `pkexec` đang mở.
    fw_allowing: bool,
    /// Vòng chính dựng lại `Browser` ở lượt kế (sau khi mở tường lửa).
    restart_mdns: bool,
    /// Quét LAN dự phòng đang chạy (PROTOCOL 6.2): `Some(true)` = lượt đầy đủ của màn Thêm điện thoại, `Some(false)` =
    /// lượt thưa cho máy đã ghép Offline.
    scan_running: Option<bool>,
    /// Lệnh `ScanLan` chờ lượt đầy đủ kế xong.
    scan_waiters: Vec<(Outbox, u64)>,
    /// Có lệnh `ScanLan` tới lúc lượt thưa đang chạy ⇒ chạy lượt đầy đủ ngay sau.
    full_scan_wanted: bool,
    /// Lượt đầy đủ xong gần nhất — lệnh tới sớm hơn `SCAN_MIN_INTERVAL_SECS` trả ngay.
    last_scan: Option<Instant>,
    /// Lượt thưa (máy Offline) bắt đầu gần nhất — cách nhau ít nhất [`OFFLINE_SCAN_EVERY`].
    last_offline_scan: Option<Instant>,
    /// Lần đọc lại `info` gần nhất của từng máy Online ([`INFO_REFRESH_EVERY`]).
    info_refreshed: HashMap<DeviceId, Instant>,
    /// W-10: lần Cắt / Sao chép sang máy tính đang có, số thứ tự lần gần nhất, thư mục tạm gốc.
    export: Option<export::ExportJob>,
    export_seq: u64,
    export_base: PathBuf,
    clipboard: crate::clipboard::Clipboard,
    /// Khôi phục cửa sổ (`ferry_core::view`): trạng thái UI gửi lần cuối + thư mục / thư viện đã đọc từ điện thoại. Chỉ
    /// trong RAM; daemon thoát thì mất.
    view: Option<ViewState>,
    dir_cache: DirCache,
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
            i18n: cfg.i18n,
            autostart: cfg.autostart,
            autostart_error: cfg.autostart_error,
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
            firewall: firewall::Detected::NONE,
            mdns_since: browser.as_ref().map(|_| Instant::now()),
            mdns_found: false,
            fw_allowing: false,
            restart_mdns: false,
            scan_running: None,
            scan_waiters: Vec::new(),
            full_scan_wanted: false,
            last_scan: None,
            last_offline_scan: None,
            info_refreshed: HashMap::new(),
            export: None,
            export_seq: 0,
            export_base: cfg.export_dir,
            clipboard: cfg.clipboard,
            view: None,
            dir_cache: DirCache::default(),
        };
        // Phiên Cắt / Sao chép của lần chạy trước: còn trong hạn thì giữ (Windows / macOS clipboard sống qua tiến trình).
        ferry_core::export::clean_stale(&engine.export_base, ferry_core::export::KEEP_FOR);
        if browser.is_some() {
            engine.firewall = firewall::detect(&engine.firewall_marker());
            if engine.firewall.may_block {
                log::info!("engine: ufw is enabled and mDNS not allowed yet, asking on the Add phone screen");
            }
        }
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
                        if !std::mem::replace(&mut self.mdns_found, true) && self.firewall.may_block {
                            self.dirty = true;
                        }
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
            if std::mem::take(&mut self.restart_mdns) {
                // Thả bộ cũ trước (tắt luồng + socket của nó) rồi dò lại từ đầu.
                rx.browser = None;
                rx.browser = Browser::start().map_err(|e| log::warn!("engine: mdns restart failed: {e}")).ok();
                self.mdns_since = rx.browser.as_ref().map(|_| Instant::now());
            }
        }
        log::info!("engine: stopped");
    }

    // ---- đầu vào ----

    fn on_input(&mut self, input: Input) {
        match input {
            Input::Request { outbox, req, op } => self.on_request(outbox, req, op),
            Input::Subscribe { conn, outbox } => {
                // Trạng thái cửa sổ trước `Snapshot` đầu: UI dựng đúng trang ngay khung đầu (`ferry_core::view`).
                let cached = match self.view.as_ref().and_then(|v| v.browser.as_ref()) {
                    Some(b) => self.dir_cache.restore(b),
                    None => CachedData::default(),
                };
                outbox.push(DaemonToUi::Restore(Box::new(Restore { view: self.view.clone(), cached })));
                let snapshot = self.snapshot();
                let resync = !outbox.push(DaemonToUi::Snapshot(snapshot));
                self.subscribers.insert(conn, Subscriber { outbox, resync });
            }
            Input::Unsubscribe { conn } => {
                self.subscribers.remove(&conn);
            }
            Input::SaveView(mut view) => {
                view.clamp();
                let view = *view;
                self.view = Some(view);
            }
            Input::Shutdown { .. } => unreachable!("handled in run"),
        }
    }

    fn on_request(&mut self, outbox: Outbox, req: u64, op: Op) {
        let reply = match op {
            Op::Queue(op) => self.on_queue(&outbox, req, op),
            Op::Phone(op) => self.on_phone(&outbox, req, op),
            Op::Library(op) => self.on_library(&outbox, req, op),
            Op::Settings(op) => self.on_settings(&outbox, req, op),
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
                        let me = Identity { client_id: &client_id, client_name: &client_name };
                        let pin = Secret::new(pin);
                        if let Ok(mut c) = discovery::connect(&device, &addrs, None).await {
                            let p = pairing::pair(&mut c.client, Some(&device), &pin, me, &tokens).await;
                            return p.map(|p| (p, None)).map_err(pair_error);
                        }
                        // Địa chỉ cũ trả lời bằng mã khác: app điện thoại cài lại. PIN đọc trên máy đó đúng ⇒ nhận mã mới.
                        let (addr, info) =
                            discovery::find_replacement(&device, &addrs).await.ok_or(CommandError::Unreachable)?;
                        let mut client =
                            PhoneClient::new(addr, Some(&info.id), None).map_err(|_| CommandError::Unreachable)?;
                        let p =
                            pairing::pair(&mut client, Some(&info.id), &pin, me, &tokens).await.map_err(pair_error)?;
                        log::info!("engine: {device} was reinstalled as {} at {addr}, moving its queue", p.id);
                        Ok((p, Some(Replaced { old: device, info, addr })))
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
                // Không `config.forget_device` trước: `save_config` thấy config đã khớp sổ thì không ghi tệp ⇒ máy đã quên
                // sống lại ở lần chạy sau (gặp 11/11/2026). Bỏ khỏi sổ rồi để `save_config` tính và ghi.
                let ev = self.devices.forget(&device);
                self.on_device_events(ev);
                self.forget_view(&device);
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
            PhoneOp::AllowMdnsFirewall => {
                if self.fw_allowing {
                    return Some(Err(CommandError::InvalidInput));
                }
                self.fw_allowing = true;
                let (detected, tx, outbox) = (self.firewall.clone(), self.internal_tx.clone(), outbox.clone());
                // `pkexec` chờ người dùng xác nhận (có thể rất lâu) ⇒ luồng chặn riêng, không giữ vòng chính.
                tokio::task::spawn_blocking(move || {
                    let result = firewall::allow_with_prompt(&detected);
                    let _ = tx.send(Internal::FirewallAllowed { outbox, req, result });
                });
                None
            }
            PhoneOp::ScanLan => {
                match self.scan_running {
                    Some(true) => {}
                    Some(false) => self.full_scan_wanted = true,
                    None => {
                        let recent = self.last_scan.is_some_and(|t| {
                            Instant::now().saturating_duration_since(t) < Duration::from_secs(SCAN_MIN_INTERVAL_SECS)
                        });
                        if recent {
                            return Some(Ok(Outcome::Scanned { found: 0 }));
                        }
                        self.start_full_scan();
                    }
                }
                self.scan_waiters.push((outbox.clone(), req));
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
                self.refresh_info(&device, &client);
                let (outbox, tx) = (outbox.clone(), self.internal_tx.clone());
                tokio::spawn(async move {
                    let r = client.list().await;
                    if let Ok(items) = &r {
                        let _ = tx.send(Internal::LibraryRead { device: device.clone(), items: items.clone() });
                    }
                    let r = r.map(|items| Outcome::Library { items });
                    let r = r.map_err(|e| read_error(&tx, &device, &client, "list", e));
                    reply_to(&outbox, req, r);
                });
                None
            }
            LibraryOp::Dirs { device, path } => {
                let client = match self.online_client(&device) {
                    Ok(c) => c,
                    Err(e) => return Some(Err(e)),
                };
                self.refresh_info(&device, &client);
                let (outbox, tx) = (outbox.clone(), self.internal_tx.clone());
                tokio::spawn(async move {
                    let r = client.dirs(&path).await;
                    match &r {
                        Ok(l) => {
                            let _ =
                                tx.send(Internal::Listed { device: device.clone(), path, listing: Some(l.clone()) });
                        }
                        Err(ClientError::NotFound) => {
                            let _ = tx.send(Internal::Listed { device: device.clone(), path, listing: None });
                        }
                        Err(_) => {}
                    }
                    let r = r.map(|listing| Outcome::Dirs { listing });
                    let r = r.map_err(|e| match e {
                        ClientError::NotFound | ClientError::BadRequest => CommandError::InvalidInput,
                        // Công tắc tắt / mất quyền (§6.11): đọc lại `info` ngay để UI về chế độ dựng từ `list`.
                        ClientError::WrongPin => {
                            spawn_info_refresh(&tx, &device, &client);
                            CommandError::InvalidInput
                        }
                        e => read_error(&tx, &device, &client, "dirs", e),
                    });
                    reply_to(&outbox, req, r);
                });
                None
            }
            LibraryOp::Cached { device, path } => Some(
                self.dir_cache
                    .dir(&device, &path)
                    .map(|listing| Outcome::Dirs { listing })
                    .ok_or(CommandError::Missing),
            ),
            LibraryOp::MakeDir { device, path } => {
                self.edit_op(outbox, req, &device, feature::EDIT, move |c| async move {
                    c.make_dir(&path).await.map(|path| Outcome::Edited { path })
                })
            }
            LibraryOp::Rename { device, path, name } => {
                self.edit_op(outbox, req, &device, feature::EDIT, move |c| async move {
                    c.rename(&path, &name).await.map(|path| Outcome::Edited { path })
                })
            }
            LibraryOp::Delete { device, path } => {
                self.edit_op(outbox, req, &device, feature::EDIT, move |c| async move {
                    c.delete_entry(&path).await.map(|()| Outcome::Done)
                })
            }
            LibraryOp::Copy { device, path, dir } => {
                self.edit_op(outbox, req, &device, feature::COPY, move |c| async move {
                    c.copy_entry(&path, &dir).await.map(|path| Outcome::Edited { path })
                })
            }
            LibraryOp::Move { device, path, dir } => {
                self.edit_op(outbox, req, &device, feature::COPY, move |c| async move {
                    c.move_entry(&path, &dir).await.map(|path| Outcome::Edited { path })
                })
            }
            LibraryOp::Export { device, entries, cut } => Some(self.start_export(device, entries, cut)),
            LibraryOp::CancelExport => {
                self.drop_export(true);
                Some(Ok(Outcome::Done))
            }
            LibraryOp::Download { device, remote_ids } => {
                if remote_ids.is_empty() {
                    return Some(Err(CommandError::InvalidInput));
                }
                let client = match self.online_client(&device) {
                    Ok(c) => c,
                    Err(e) => return Some(Err(e)),
                };
                let (store, tx, outbox) = (self.store.clone(), self.internal_tx.clone(), outbox.clone());
                let download_dir = self.config.download_dir();
                tokio::spawn(async move {
                    // Hỏi lại `list`: tên / cỡ / thư mục mới nhất (UI có thể cầm danh sách cũ).
                    let outcome = match client.list().await {
                        Ok(items) => {
                            let r = tokio::task::spawn_blocking(move || {
                                add_downloads(&store, &device, &items, &remote_ids, &download_dir).map(|o| (device, o))
                            })
                            .await;
                            match r {
                                Ok(Ok((device, outcome))) => {
                                    let _ = tx.send(Internal::Added { outbox, req, device, outcome });
                                    return;
                                }
                                Ok(Err(e)) => Err(e),
                                Err(e) => Err(internal(e)),
                            }
                        }
                        Err(e) => {
                            log::info!("engine: list {device} for download: {e}");
                            Err(CommandError::Unreachable)
                        }
                    };
                    reply_to(&outbox, req, outcome);
                });
                None
            }
        }
    }

    /// W-8 (§6.12): một lệnh sửa bộ nhớ của máy Online, trả lời khi điện thoại trả. Không có `edit` ⇒ `Forbidden` khỏi
    /// gọi mạng; `403` ⇒ đọc lại `info` (công tắc vừa tắt).
    fn edit_op<F, Fut>(
        &mut self,
        outbox: &Outbox,
        req: u64,
        device: &DeviceId,
        // `edit` (§6.12) hoặc `copy` (§6.13, luôn đi cùng `edit`).
        need: &str,
        call: F,
    ) -> Option<Result<Outcome, CommandError>>
    where
        F: FnOnce(Arc<PhoneClient>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<Outcome, ClientError>> + Send + 'static,
    {
        let client = match self.online_client(device) {
            Ok(c) => c,
            Err(e) => return Some(Err(e)),
        };
        if !self
            .devices
            .get(device)
            .is_some_and(|d| d.features.iter().any(|f| f == feature::EDIT) && d.features.iter().any(|f| f == need))
        {
            return Some(Err(CommandError::Forbidden));
        }
        let (outbox, tx, device) = (outbox.clone(), self.internal_tx.clone(), device.clone());
        tokio::spawn(async move {
            let r = call(client.clone()).await.map_err(|e| match e {
                ClientError::Conflict => CommandError::Exists,
                ClientError::NotFound => CommandError::Missing,
                ClientError::BadRequest => CommandError::InvalidInput,
                ClientError::WrongPin => {
                    spawn_info_refresh(&tx, &device, &client);
                    CommandError::Forbidden
                }
                e => read_error(&tx, &device, &client, "edit", e),
            });
            reply_to(&outbox, req, r);
        });
        None
    }

    // ---- cài đặt (gd1_12, PROTOCOL 4.5) ----

    fn on_settings(&mut self, outbox: &Outbox, req: u64, op: SettingsOp) -> Option<Result<Outcome, CommandError>> {
        let internal_tx = self.internal_tx.clone();
        let blocking = |change: Box<dyn FnOnce() -> Settled + Send>| {
            let (tx, outbox) = (internal_tx.clone(), outbox.clone());
            tokio::spawn(async move {
                match tokio::task::spawn_blocking(change).await {
                    Ok(change) => {
                        let _ = tx.send(Internal::Settled { outbox, req, change });
                    }
                    Err(e) => {
                        log::warn!("engine: settings task: {e}");
                        reply_to(&outbox, req, Err(CommandError::Internal));
                    }
                }
            });
        };
        match op {
            SettingsOp::Get => {}
            SettingsOp::SetLanguage { language } => {
                if self.config.language != language {
                    self.config.language = language;
                    self.persist();
                    // Khay + thông báo đổi ngay: `TrayStatus` mang ngôn ngữ, luồng chính dựng lại menu theo nó.
                    self.i18n = I18n::new(language.resolve());
                    self.tray = None;
                    self.update_tray();
                }
            }
            SettingsOp::SetDownloadDir { path } => {
                if path.as_ref().is_some_and(|p| !p.is_absolute()) {
                    return Some(Err(CommandError::InvalidInput));
                }
                blocking(Box::new(move || {
                    let check = path.as_deref().map_or(Ok(()), check_writable_dir);
                    Settled::DownloadDir(path, check)
                }));
                return None;
            }
            SettingsOp::SetAutostart { enabled } => {
                let apply = self.autostart;
                blocking(Box::new(move || Settled::Autostart(enabled, apply(enabled))));
                return None;
            }
            SettingsOp::RenameDevice { device, name } => {
                if !self.config.devices.iter().any(|d| d.id == device) || !self.devices.set_alias(&device, Some(name)) {
                    return Some(Err(CommandError::UnknownDevice));
                }
                self.dirty = true;
                self.save_config();
            }
            SettingsOp::SetTheme { theme } => {
                if self.config.theme != theme {
                    self.config.theme = theme;
                    self.persist();
                }
            }
            SettingsOp::SetThemePreset { preset } => {
                let preset = preset.trim().to_owned();
                if self.config.theme_preset != preset {
                    self.config.theme_preset = preset;
                    self.persist();
                }
            }
        }
        Some(Ok(Outcome::Settings(self.settings_view())))
    }

    fn on_settled(&mut self, change: Settled) -> Result<Outcome, CommandError> {
        match change {
            Settled::DownloadDir(path, check) => {
                check?;
                self.config.download_dir = path;
            }
            Settled::Autostart(enabled, result) => {
                self.config.autostart = enabled;
                self.autostart_error = result.err();
            }
        }
        self.persist();
        Ok(Outcome::Settings(self.settings_view()))
    }

    fn settings_view(&self) -> SettingsView {
        let c = &self.config;
        SettingsView {
            language: c.language,
            download_dir: c.download_dir(),
            download_dir_custom: c.download_dir.as_ref().is_some_and(|d| !d.as_os_str().is_empty()),
            autostart: c.autostart,
            autostart_error: self.autostart_error.clone(),
            devices: c
                .devices
                .iter()
                .map(|p| {
                    let live = self.devices.get(&p.id);
                    DeviceSetting {
                        id: p.id.clone(),
                        name: live.map_or_else(|| p.name.clone(), |d| d.name.clone()),
                        alias: live.map_or_else(|| p.alias.clone(), |d| d.alias.clone()),
                        state: live.map_or(DeviceState::Paired, |d| d.state),
                    }
                })
                .collect(),
            theme: c.theme,
            theme_preset: c.theme_preset().to_owned(),
        }
    }

    /// IPv4 điện thoại từng dùng (sổ thiết bị: địa chỉ đã ghép, kể cả nhập tay ở màn Thêm điện thoại) — gợi ý dải quét (PROTOCOL 6.2 dải 2).
    fn phone_hints(&self, only_offline_paired: bool) -> Vec<std::net::IpAddr> {
        self.devices
            .iter()
            .filter(|d| !only_offline_paired || (d.state.has_token() && d.state != DeviceState::Online))
            .flat_map(|d| self.devices.addresses(&d.id))
            .map(|a| a.ip())
            .filter(std::net::IpAddr::is_ipv4)
            .collect()
    }

    /// Lượt quét đầy đủ cho màn Thêm điện thoại: subnet của PC + dải điện thoại từng dùng.
    fn start_full_scan(&mut self) {
        self.refresh_locals();
        let hints = self.phone_hints(false);
        let targets = scan::targets(&self.locals, true, &hints, address::DEFAULT_PORT);
        self.spawn_scan(targets, true);
    }

    /// Máy đã ghép nối hỏng ở mọi địa chỉ đã biết: thưa ([`OFFLINE_SCAN_EVERY`]) quét `/24` quanh các địa chỉ đó — điện
    /// thoại ở subnet khác (qua router, mDNS không tới) đổi IP do DHCP vẫn tìm lại được.
    fn maybe_offline_scan(&mut self) {
        if self.scan_running.is_some() || self.last_offline_scan.is_some_and(|t| t.elapsed() < OFFLINE_SCAN_EVERY) {
            return;
        }
        let hints = self.phone_hints(true);
        if hints.is_empty() {
            return;
        }
        self.last_offline_scan = Some(Instant::now());
        self.refresh_locals();
        let targets = scan::targets(&self.locals, false, &hints, address::DEFAULT_PORT);
        self.spawn_scan(targets, false);
    }

    fn spawn_scan(&mut self, targets: Vec<SocketAddr>, full: bool) {
        log::info!("engine: LAN scan ({}) of {} hosts", if full { "full" } else { "offline phones" }, targets.len());
        self.scan_running = Some(full);
        let tx = self.internal_tx.clone();
        tokio::spawn(async move {
            let found = scan::scan(targets).await;
            let _ = tx.send(Internal::Scanned { found, full });
        });
    }

    /// `info` hỏi không token (địa chỉ tay, quét LAN) trả lời từ `addr`.
    fn apply_untrusted_info(&mut self, info: &Info, addr: SocketAddr, now: Instant) {
        let has_token = self.devices.get(&info.id).is_some_and(|d| d.state.has_token());
        if has_token {
            // `info` không token luôn `paired: false` — không cho máy trạng thái nhầm thành NeedsRepair:
            // nối lại kèm token, địa chỉ mới thử trước.
            self.probing.remove(&info.id);
            self.probe_device(&info.id, Some(addr));
        } else {
            let ev = self.devices.info_ok(info, addr, now);
            self.on_device_events(ev);
        }
    }

    /// Ghi `config.json` (nguyên tử). Chỉ engine ghi — một nơi, tuần tự trong `select!`.
    fn persist(&self) {
        if let Err(e) = config::save(&self.config_path, &self.config) {
            log::warn!("engine: save {}: {e}", self.config_path.display());
        }
    }

    // ---- lệnh hàng đợi ----

    /// Mục upload hoặc tải về (`id` chung dãy, cùng bảng) — runner của thiết bị phân loại.
    fn item(&mut self, id: ItemId, action: Action) -> Result<Outcome, CommandError> {
        let device = {
            let store = lock(&self.store);
            match store.upload(id).map_err(internal)? {
                Some(u) => u.device_id,
                None => store.download(id).map_err(internal)?.ok_or(CommandError::InvalidInput)?.device_id,
            }
        };
        let runner = self.runners.get(&device).ok_or(CommandError::NotPaired)?;
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
        if matches!(action, Action::Cancel | Action::Retry) {
            return Err(CommandError::InvalidInput);
        }
        let (uploads, downloads) = {
            let store = lock(&self.store);
            (store.uploads().map_err(internal)?, store.downloads().map_err(internal)?)
        };
        let ups = uploads
            .into_iter()
            .map(|i| (i.id, i.device_id, matches!(i.state, UploadState::Paused { .. }), i.state.is_terminal()));
        let downs = downloads
            .into_iter()
            .map(|i| (i.id, i.device_id, matches!(i.state, DownloadState::Paused { .. }), i.state.is_terminal()));
        for (id, device, paused, terminal) in ups.chain(downs) {
            let hit = match action {
                Action::Pause => !paused && !terminal,
                _ => paused,
            };
            if hit && let Some(r) = self.runners.get(&device) {
                r.handle.send(Command::Item { id, action: item_action(action) });
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

    /// Đọc lại `info` của máy Online (nền, không ai chờ), mỗi máy cách nhau ít nhất [`INFO_REFRESH_EVERY`].
    fn refresh_info(&mut self, device: &DeviceId, client: &Arc<PhoneClient>) {
        let now = Instant::now();
        if self.info_refreshed.get(device).is_some_and(|t| now.duration_since(*t) < INFO_REFRESH_EVERY) {
            return;
        }
        self.info_refreshed.insert(device.clone(), now);
        spawn_info_refresh(&self.internal_tx, device, client);
    }

    fn online_client(&self, device: &DeviceId) -> Result<Arc<PhoneClient>, CommandError> {
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
        let (runner, handle) =
            UploadRunner::new(id.clone(), self.store.clone(), watch, self.transfer_tx.clone(), self.runner_cfg.clone());
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
                        self.maybe_offline_scan();
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
                let r = result.map(|(p, replaced)| {
                    if let Some(rp) = &replaced {
                        // Mã mới chưa có trong sổ (mDNS có thể bị chặn) ⇒ ghi từ `info` trước khi đánh dấu đã ghép.
                        let ev = self.devices.info_ok(&rp.info, rp.addr, now);
                        self.on_device_events(ev);
                    }
                    let ev = self.devices.paired(&p.id, now);
                    self.on_device_events(ev);
                    self.ensure_runner(&p.id);
                    // Ghép lại máy cũ (`replaced`), hoặc ghép "máy mới" mà thật ra là máy cũ đã cài lại app.
                    let old = replaced.map(|rp| rp.old).or_else(|| self.devices.superseded_by(&p.id));
                    if let Some(old) = old {
                        log::info!("engine: {old} is superseded by {}", p.id);
                        self.replace_device(&old, &p.id);
                    }
                    // Ghép lại sau 401: runner chờ `Link` mới — probe ngay đưa nó về Online.
                    self.save_config();
                    self.on_deadline();
                    Outcome::Paired { device: p.id, name: p.name }
                });
                reply_to(&outbox, req, r);
            }
            Internal::Scanned { found, full } => {
                self.scan_running = None;
                log::info!("engine: LAN scan found {} phone(s)", found.len());
                let count = u32::try_from(found.len()).unwrap_or(u32::MAX);
                for (addr, info) in found {
                    self.apply_untrusted_info(&info, addr, now);
                }
                if full {
                    self.last_scan = Some(Instant::now());
                    for (outbox, req) in std::mem::take(&mut self.scan_waiters) {
                        reply_to(&outbox, req, Ok(Outcome::Scanned { found: count }));
                    }
                }
                if std::mem::take(&mut self.full_scan_wanted) {
                    self.start_full_scan();
                }
            }
            Internal::Probed { outbox, req, addr, result } => {
                let r = result.map(|info| {
                    self.apply_untrusted_info(&info, addr, now);
                    Outcome::Found { device: info.id, name: info.name }
                });
                reply_to(&outbox, req, r);
            }
            Internal::Settled { outbox, req, change } => {
                let r = self.on_settled(change);
                reply_to(&outbox, req, r);
            }
            Internal::Added { outbox, req, device, outcome } => {
                let wake = matches!(outcome, Outcome::Added { batch: Some(_), .. })
                    || matches!(outcome, Outcome::DownloadsAdded { added, .. } if added > 0);
                if wake && let Some(r) = self.runners.get(&device) {
                    r.handle.send(Command::Wake);
                }
                self.dirty = true;
                reply_to(&outbox, req, Ok(outcome));
            }
            Internal::FirewallAllowed { outbox, req, result } => {
                self.fw_allowing = false;
                if result == AllowResult::Allowed {
                    let marker = self.firewall_marker();
                    if let Err(e) = std::fs::write(&marker, b"allowed by the user in ferry (W-6)\n") {
                        log::warn!("engine: write {}: {e}", marker.display());
                    }
                    self.firewall.may_block = false;
                    self.dirty = true;
                    self.restart_mdns = self.mdns_since.is_some();
                    log::info!("engine: mDNS allowed in the firewall, browsing again");
                }
                reply_to(&outbox, req, Ok(Outcome::Firewall { result }));
            }
            Internal::InfoRefreshed { device, addr, info } => {
                // Đã Offline / quên / đổi địa chỉ trong lúc hỏi ⇒ bỏ; `Connected` lần sau đọc `info` mới.
                let online = self.devices.get(&device).is_some_and(|d| d.state == DeviceState::Online);
                if !online || info.id != device {
                    return;
                }
                let before = self.devices.get(&device).map(|d| d.features.clone()).unwrap_or_default();
                let ev = self.devices.info_ok(&info, addr, now);
                self.on_device_events(ev);
                if before != info.features {
                    log::info!("engine: {device} features {:?} -> {:?}", before, info.features);
                    let dirs = info.has_feature(feature::DIRS);
                    if let Some(r) = self.runners.get(&device) {
                        r.link.send_if_modified(|l| match l {
                            Some(l) if l.dirs != dirs && l.addr() == addr => {
                                l.dirs = dirs;
                                true
                            }
                            _ => false,
                        });
                    }
                }
                self.dirty = true;
            }
            Internal::LinkFailed { device, addr } => {
                log::info!("engine: lost {device} at {addr} (library), reconnecting");
                self.devices.address_failed(&device, addr);
                self.probe_device(&device, None);
                self.dirty = true;
            }
            Internal::Moved { device } => {
                if let Some(r) = self.runners.get(&device) {
                    r.handle.send(Command::Wake);
                }
                self.dirty = true;
            }
            Internal::Forgot { outbox, req } => {
                self.dirty = true;
                reply_to(&outbox, req, Ok(Outcome::Done));
            }
            Internal::Export(msg) => self.on_export(msg),
            Internal::Listed { device, path, listing } => {
                // Máy vừa bị Quên trong lúc đọc ⇒ không nhận lại.
                if self.devices.get(&device).is_none() {
                    return;
                }
                match listing {
                    Some(l) => self.dir_cache.put_dir(&device, l),
                    None => self.dir_cache.remove_dir(&device, &path),
                }
            }
            Internal::LibraryRead { device, items } => {
                if self.devices.get(&device).is_some() {
                    self.dir_cache.put_library(&device, items);
                }
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
            TransferEvent::DownloadChanged { id, state } => {
                log::debug!("engine: download {id} -> {}", state.status());
                self.dirty = true;
                match &state {
                    DownloadState::Fetching { offset, .. } => {
                        if let Some(p) = self.progress.get_mut(&id) {
                            p.sent = p.sent.max(*offset);
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
        let store = lock(&self.store);
        match store.upload(id).ok().flatten() {
            Some(u) => Some(u.device_id),
            None => store.download(id).ok().flatten().map(|d| d.device_id),
        }
    }

    // ---- thông báo ----

    fn phone_name(&self, id: &DeviceId) -> String {
        self.devices
            .get(id)
            .map(|d| d.display_name().to_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.0.clone())
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
        let mut snapshot =
            snapshot_parts(&self.store, &self.devices, &self.locals, &self.summary, &self.progress, uploads);
        snapshot.mdns_firewall =
            firewall::hint_needed(self.firewall.may_block, self.mdns_since.is_some(), self.mdns_found)
                .then(|| self.firewall.hint());
        snapshot.export = self.export_view();
        snapshot
    }

    /// Dấu "người dùng đã cho phép mDNS" (W-6), cạnh `config.json`.
    fn firewall_marker(&self) -> PathBuf {
        self.config_path.with_file_name(firewall::USER_MARKER)
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
    /// `old` là bản cài trước của `new` (ghép lại bằng PIN, `Replaced`): bỏ `old` khỏi sổ, config, kho khoá như "Quên",
    /// nhưng chuyển hàng đợi của nó sang `new` thay vì xoá. Tên đặt trên PC đi theo.
    fn replace_device(&mut self, old: &DeviceId, new: &DeviceId) {
        let runner = self.runners.remove(old);
        if let Some(r) = &runner {
            r.handle.send(Command::Stop);
        }
        let alias = self.config.devices.iter().find(|d| d.id == *old).and_then(|d| d.alias.clone());
        // Bản ghi config của `old` do `save_config` (người gọi) bỏ và ghi tệp — xem `PhoneOp::Forget`.
        let ev = self.devices.forget(old);
        self.on_device_events(ev);
        self.forget_view(old);
        if let Some(alias) = alias {
            self.devices.set_alias(new, Some(alias));
        }
        self.told_repair.remove(old);
        self.told_no_space.remove(old);
        let (tokens, store, tx) = (self.tokens.clone(), self.store.clone(), self.internal_tx.clone());
        let (old, new) = (old.clone(), new.clone());
        tokio::spawn(async move {
            if let Some(r) = runner {
                let _ = tokio::time::timeout(STOP_TIMEOUT, r.task).await;
            }
            if let Err(e) = tokens.delete(&old).await {
                log::warn!("engine: delete token of replaced {old}: {e}");
            }
            let to = new.clone();
            match tokio::task::spawn_blocking(move || lock(&store).move_device(&old, &to)).await {
                Ok(Ok(n)) => log::info!("engine: moved {n} queued item(s) to {new}"),
                Ok(Err(e)) => log::warn!("engine: move queue to {new}: {e}"),
                Err(e) => log::warn!("engine: move queue task: {e}"),
            }
            let _ = tx.send(Internal::Moved { device: new });
        });
    }

    /// Máy bị Quên / thay mã ⇒ bỏ dữ liệu của nó ở bộ đệm thư mục và trạng thái cửa sổ đang giữ.
    fn forget_view(&mut self, device: &DeviceId) {
        self.dir_cache.forget_device(device);
        if let Some(v) = &mut self.view {
            v.forget_device(device);
        }
    }

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
            name: d.display_name().to_owned(),
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
    let downloads = lock(store).downloads().unwrap_or_else(|e| {
        log::warn!("engine: read downloads: {e}");
        Vec::new()
    });
    let (downloads, downloads_omitted) = cap_downloads(downloads);
    let dl_running =
        downloads.iter().any(|d| !d.state.is_terminal() && !matches!(d.state, DownloadState::Paused { .. }));
    let dl_paused = downloads.iter().any(|d| matches!(d.state, DownloadState::Paused { .. }));
    let ids = uploads.iter().map(|u| u.id).chain(downloads.iter().map(|d| d.id));
    let progress = ids.filter_map(|id| progress.get(&id).copied()).collect();
    let can_pause_all = summary.can_pause_all() || dl_running;
    Snapshot {
        devices,
        batches,
        uploads,
        progress,
        finished_omitted,
        queued_omitted,
        can_pause_all,
        can_resume_all: !can_pause_all && (summary.can_resume_all() || dl_paused),
        downloads,
        downloads_omitted,
        mdns_firewall: None,
        export: None,
    }
}

/// Mục tải về chưa xong (đầu hàng trước) + xong mới nhất, tối đa [`DOWNLOADS_IN_SNAPSHOT`], xếp theo `id`.
fn cap_downloads(all: Vec<DownloadItem>) -> (Vec<DownloadItem>, u64) {
    let total = all.len();
    let (mut live, mut finished): (Vec<DownloadItem>, Vec<DownloadItem>) =
        all.into_iter().partition(|d| !d.state.is_terminal());
    live.sort_by_key(|d| d.id);
    live.truncate(DOWNLOADS_IN_SNAPSHOT);
    finished.sort_by_key(|d| std::cmp::Reverse((d.updated, d.id)));
    finished.truncate(DOWNLOADS_IN_SNAPSHOT - live.len());
    let mut out = live;
    out.extend(finished);
    out.sort_by_key(|d| d.id);
    let omitted = (total - out.len()) as u64;
    (out, omitted)
}

/// Xếp `remote_ids` (theo `list` mới nhất) vào hàng tải về của `device`. Mục cùng `remoteId` đang có trong hàng (chưa
/// xong) bỏ qua; mục đã xong / lỗi / huỷ thì tải lại (người dùng bấm lần nữa).
fn add_downloads(
    store: &SharedStore,
    device: &DeviceId,
    items: &[ferry_core::protocol::LibraryItem],
    remote_ids: &[String],
    download_dir: &Path,
) -> Result<Outcome, CommandError> {
    let mut store = lock(store);
    let queued: HashSet<String> = store
        .downloads()
        .map_err(internal)?
        .into_iter()
        .filter(|d| d.device_id == *device && !d.state.is_terminal())
        .map(|d| d.remote_id)
        .collect();
    let (mut added, mut already_queued, mut missing) = (0, 0, 0);
    let mut seen = HashSet::new();
    let now = now_ms();
    for rid in remote_ids {
        if !seen.insert(rid.as_str()) || queued.contains(rid) {
            already_queued += 1;
            continue;
        }
        let Some(item) = items.iter().find(|i| i.id == *rid) else {
            missing += 1;
            continue;
        };
        let d = NewDownload {
            remote_id: item.id.clone(),
            name: item.name.clone(),
            remote_dir: item.dir.clone(),
            size: item.size,
            dest: download::default_dest(download_dir, &item.name),
        };
        store.add_download(device, &d, now).map_err(internal)?;
        added += 1;
    }
    Ok(Outcome::DownloadsAdded { added, already_queued, missing })
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

/// Thư mục tải về: tạo nếu chưa có, rồi thử ghi một tệp tạm (quyền, ổ chỉ đọc, ổ mạng rớt).
fn check_writable_dir(dir: &std::path::Path) -> Result<(), CommandError> {
    let probe = dir.join(format!(".ferry-write-test-{}", std::process::id()));
    std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&probe, b"")).map_err(|e| {
        log::info!("engine: download dir {} is not writable: {e}", dir.display());
        CommandError::NotWritable
    })?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// `GET /info` bằng client kèm token của máy Online ⇒ [`Internal::InfoRefreshed`]. Lỗi mạng bỏ qua: lệnh đang chạy cùng
/// lúc (`list` / `dirs`) tự báo mất kết nối.
fn spawn_info_refresh(tx: &mpsc::UnboundedSender<Internal>, device: &DeviceId, client: &Arc<PhoneClient>) {
    let (tx, device, client) = (tx.clone(), device.clone(), client.clone());
    tokio::spawn(async move {
        match client.info().await {
            Ok(info) => {
                let _ = tx.send(Internal::InfoRefreshed { device, addr: client.addr(), info });
            }
            Err(e) => log::debug!("engine: refresh info {device}: {e}"),
        }
    });
}

/// Lỗi đọc thư viện / thư mục điện thoại cho UI. Điện thoại tắt Sync WiFi giữa chừng (connection refused) ⇒ đừng để
/// UI tin "Đang kết nối": bỏ địa chỉ, nối lại; UI thấy Offline ngay thay vì "Không đọc được danh sách".
fn read_error(
    tx: &mpsc::UnboundedSender<Internal>,
    device: &DeviceId,
    client: &PhoneClient,
    what: &str,
    e: ClientError,
) -> CommandError {
    log::info!("engine: {what} {device}: {e}");
    if matches!(e, ClientError::Network(_)) {
        let _ = tx.send(Internal::LinkFailed { device: device.clone(), addr: client.addr() });
        CommandError::Offline
    } else {
        CommandError::Unreachable
    }
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

    use ferry_core::config::PairedDevice;
    use ferry_core::i18n::Language;
    use ferry_core::ipc::{ExportEntry, ExportState};
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

    /// `pace`: điện thoại giả nhận / gửi chậm (byte / s) để test bắt được lúc đang truyền.
    async fn start(dir: &Path, phone: &FakePhone, pace: Option<u64>) -> Rig {
        let token = phone.issue_token(CLIENT_ID);
        let tokens = Tokens::new(Arc::new(MemoryKeyring::new()));
        tokens.set(&device(), &Secret::new(token)).await.unwrap();
        start_with(dir, phone, tokens, pace)
    }

    fn start_with(dir: &Path, phone: &FakePhone, tokens: Tokens, pace: Option<u64>) -> Rig {
        phone.set_pace(pace);
        let config = Config {
            client_id: CLIENT_ID.into(),
            client_name: "Test PC".into(),
            devices: vec![PairedDevice {
                id: device(),
                name: fake_phone::NAME.into(),
                addresses: vec![phone.v4().to_string()],
                alias: None,
            }],
            download_dir: Some(dir.join("dl")),
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
            i18n: I18n::new(Language::En),
            autostart: |_| Ok(()),
            autostart_error: None,
            export_dir: dir.join("clipboard"),
            clipboard: crate::clipboard::Clipboard { set_files: fake_clipboard_set, clear: || {} },
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

    /// Clipboard giả: ghi lại thay vì đụng clipboard thật của máy chạy test.
    static CLIPBOARD: std::sync::Mutex<Vec<(Vec<PathBuf>, bool)>> = std::sync::Mutex::new(Vec::new());

    fn fake_clipboard_set(paths: &[PathBuf], cut: bool) -> Result<(), String> {
        CLIPBOARD.lock().unwrap().push((paths.to_vec(), cut));
        Ok(())
    }

    fn export_is(msg: &DaemonToUi, want: impl Fn(&ExportState) -> bool) -> bool {
        matches!(msg, DaemonToUi::Snapshot(s) if s.export.as_ref().is_some_and(|e| want(&e.state)))
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
        let (rx, first) = collect(rx, |m| matches!(m, DaemonToUi::Snapshot(_))).await;
        // Trạng thái cửa sổ (chưa có) rồi ảnh chụp đầy đủ.
        assert!(matches!(&first[0].1, DaemonToUi::Restore(r) if r.view.is_none()), "restore comes first");
        assert!(matches!(first[1].1, DaemonToUi::Snapshot(_)), "then a full snapshot");
        assert!(first[1].0 - t0 < Duration::from_millis(500), "snapshot arrives right away");
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

    /// gd1_10: `Library{Download}` ⇒ trả `DownloadsAdded` (trùng / mất được đếm), runner tải về `downloadDir`, ảnh chụp
    /// có `downloads` tới `Done`; tải lại khi đang có trong hàng thì bỏ qua.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn download_command_queues_and_fetches_into_download_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let phone = FakePhone::start().await;
        let data: Vec<u8> = (0..700_000u32).map(|i| (i % 251) as u8).collect();
        let rid = phone.add_file("clip.mp4", Some("Trips"), data.clone());
        let rig = start(tmp.path(), &phone, None).await;
        let (outbox, rx) = Outbox::new();
        rig.engine.send(Input::Subscribe { conn: 1, outbox: outbox.clone() });
        let (rx, _) = collect(rx, is_online).await;

        let op = LibraryOp::Download { device: device(), remote_ids: vec![rid.clone(), rid.clone(), "gone".into()] };
        rig.engine.send(Input::Request { outbox: outbox.clone(), req: 9, op: Op::Library(op) });
        let (rx, got) = collect(rx, |m| {
            matches!(m, DaemonToUi::Snapshot(s)
                if s.downloads.len() == 1 && s.downloads[0].state == DownloadState::Done)
        })
        .await;
        let reply = got.iter().find_map(|(_, m)| match m {
            DaemonToUi::Reply { req: 9, outcome } => Some(outcome.clone()),
            _ => None,
        });
        assert_eq!(reply, Some(Outcome::DownloadsAdded { added: 1, already_queued: 1, missing: 1 }));
        let dest = tmp.path().join("dl").join("clip.mp4");
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let DaemonToUi::Snapshot(snap) = &got.last().unwrap().1 else { unreachable!() };
        assert_eq!(snap.downloads[0].dest, dest);
        assert_eq!(snap.downloads[0].remote_dir.as_deref(), Some("Trips"));
        assert!(!snap.can_pause_all);

        // Danh sách rỗng ⇒ InvalidInput.
        let bad = LibraryOp::Download { device: device(), remote_ids: vec![] };
        rig.engine.send(Input::Request { outbox: outbox.clone(), req: 10, op: Op::Library(bad) });
        let (_rx, got) = collect(rx, |m| matches!(m, DaemonToUi::Reply { req: 10, .. })).await;
        assert!(matches!(
            &got.last().unwrap().1,
            DaemonToUi::Reply { outcome: Outcome::Failed { error: CommandError::InvalidInput }, .. }
        ));
        rig.engine.shutdown();
    }

    /// W-7: máy đã Online rồi mới bật "Cho máy tính duyệt bộ nhớ" ⇒ mở màn duyệt (`List`) đọc lại `info`, ảnh chụp có
    /// `browse`; tắt lại ⇒ `Dirs` nhận `403` ⇒ `browse` mất (gặp thật 11/11/2026: PC giữ `features` lúc ghép mãi).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn online_phone_features_refresh_when_browsing() {
        let has_browse = |m: &DaemonToUi, want: bool| {
            matches!(m, DaemonToUi::Snapshot(s) if s.devices.iter().any(|d| d.id == device()
                && d.state == DeviceState::Online && d.features.iter().any(|f| f == feature::BROWSE) == want))
        };
        let tmp = tempfile::tempdir().unwrap();
        let phone = FakePhone::start().await;
        let rig = start(tmp.path(), &phone, None).await;
        let (outbox, rx) = Outbox::new();
        rig.engine.send(Input::Subscribe { conn: 1, outbox: outbox.clone() });
        let (rx, _) = collect(rx, move |m| has_browse(m, false)).await;

        phone.set_features(&["ipv6", "dirs", "browse"]);
        let op = LibraryOp::List { device: device() };
        rig.engine.send(Input::Request { outbox: outbox.clone(), req: 3, op: Op::Library(op) });
        let (rx, _) = collect(rx, move |m| has_browse(m, true)).await;

        phone.set_features(&["ipv6", "dirs"]);
        // Còn trong `INFO_REFRESH_EVERY` của lần trên: chỉ `403` của `dirs` mới đọc lại `info`.
        let op = LibraryOp::Dirs { device: device(), path: String::new() };
        rig.engine.send(Input::Request { outbox: outbox.clone(), req: 4, op: Op::Library(op) });
        let (_rx, _) = collect(rx, move |m| has_browse(m, false)).await;
        rig.engine.shutdown();
    }

    /// Khôi phục cửa sổ: `SaveView` của cửa sổ trước + thư mục đã đọc quay lại trong `Restore` của lần nối sau (trước
    /// `Snapshot`); `Cached` đọc bộ đệm không gọi mạng; Quên máy ⇒ trang thư viện của nó lùi về Danh sách thiết bị, bộ đệm
    /// của nó mất.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn saved_view_and_cached_dirs_come_back_on_reconnect() {
        use ferry_core::view::{BrowserState, PageState};
        let tmp = tempfile::tempdir().unwrap();
        let phone = FakePhone::start().await;
        phone.add_file("a.mp4", Some("Download"), vec![1; 4]);
        phone.set_features(&["dirs", "browse"]);
        let rig = start(tmp.path(), &phone, None).await;
        let dev = device();
        let (outbox, rx) = Outbox::new();
        rig.engine.send(Input::Subscribe { conn: 1, outbox: outbox.clone() });
        let (rx, _) = collect(rx, is_online).await;

        let reply = |req: u64| move |m: &DaemonToUi| matches!(m, DaemonToUi::Reply { req: r, .. } if *r == req);
        let cached = |req| Input::Request {
            outbox: outbox.clone(),
            req,
            op: Op::Library(LibraryOp::Cached { device: device(), path: "Download".into() }),
        };
        rig.engine.send(cached(1));
        let (rx, got) = collect(rx, reply(1)).await;
        assert!(matches!(
            &got.last().unwrap().1,
            DaemonToUi::Reply { outcome: Outcome::Failed { error: CommandError::Missing }, .. }
        ));
        let op = LibraryOp::Dirs { device: dev.clone(), path: "Download".into() };
        rig.engine.send(Input::Request { outbox: outbox.clone(), req: 2, op: Op::Library(op) });
        let (rx, got) = collect(rx, reply(2)).await;
        let DaemonToUi::Reply { outcome: Outcome::Dirs { listing }, .. } = got.last().unwrap().1.clone() else {
            panic!("dirs failed: {got:?}")
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        rig.engine.send(cached(3));
        let (rx, got) = collect(rx, reply(3)).await;
        assert_eq!(
            got.last().unwrap().1,
            DaemonToUi::Reply { req: 3, outcome: Outcome::Dirs { listing: listing.clone() } }
        );

        let view = ViewState {
            page: PageState::Library { device: dev.clone() },
            history: vec![PageState::Main, PageState::Devices],
            browser: Some(BrowserState {
                device: dev.clone(),
                at: Some("Download".into()),
                selected: vec!["a.mp4".into()],
                table_top: 3,
                ..BrowserState::default()
            }),
            ..ViewState::default()
        };
        rig.engine.send(Input::SaveView(Box::new(view.clone())));
        rig.engine.send(Input::Unsubscribe { conn: 1 });
        drop(rx);

        let (outbox, rx) = Outbox::new();
        rig.engine.send(Input::Subscribe { conn: 2, outbox: outbox.clone() });
        let (rx, got) = collect(rx, |m| matches!(m, DaemonToUi::Snapshot(_))).await;
        let DaemonToUi::Restore(r) = &got[0].1 else { panic!("first is not restore: {got:?}") };
        assert_eq!(r.view.as_ref(), Some(&view));
        assert_eq!(r.cached.device.as_ref(), Some(&dev));
        assert!(r.cached.dirs.contains(&listing), "{:?}", r.cached.dirs);

        let op = PhoneOp::Forget { device: dev.clone() };
        rig.engine.send(Input::Request { outbox: outbox.clone(), req: 4, op: Op::Phone(op) });
        let (_rx, _) = collect(rx, reply(4)).await;
        rig.engine.send(Input::Unsubscribe { conn: 2 });
        let (outbox, rx) = Outbox::new();
        rig.engine.send(Input::Subscribe { conn: 3, outbox: outbox.clone() });
        let (_rx, got) = collect(rx, |m| matches!(m, DaemonToUi::Snapshot(_))).await;
        let DaemonToUi::Restore(r) = &got[0].1 else { panic!("first is not restore: {got:?}") };
        let v = r.view.as_ref().unwrap();
        assert_eq!(v.page, PageState::Devices);
        assert_eq!(v.history, [PageState::Main]);
        assert!(v.browser.is_none() && r.cached.dirs.is_empty());
        rig.engine.shutdown();
    }

    /// W-8: thư mục mới / đổi tên / xoá đi tới điện thoại; thiếu `edit` ⇒ `Forbidden` không gọi mạng; `409` ⇒ `Exists`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn edit_ops_reach_the_phone() {
        async fn call(
            engine: &EngineHandle,
            outbox: &Outbox,
            rx: OutboxReceiver,
            req: u64,
            op: LibraryOp,
        ) -> (OutboxReceiver, Outcome) {
            engine.send(Input::Request { outbox: outbox.clone(), req, op: Op::Library(op) });
            let (rx, got) = collect(rx, move |m| matches!(m, DaemonToUi::Reply { req: r, .. } if *r == req)).await;
            let DaemonToUi::Reply { outcome, .. } = got.last().unwrap().1.clone() else { unreachable!() };
            (rx, outcome)
        }
        let tmp = tempfile::tempdir().unwrap();
        let phone = FakePhone::start().await;
        phone.add_file("a.mp4", Some("Download"), vec![1; 4]);
        phone.set_features(&["dirs", "browse"]);
        let rig = start(tmp.path(), &phone, None).await;
        let (outbox, rx) = Outbox::new();
        rig.engine.send(Input::Subscribe { conn: 1, outbox: outbox.clone() });
        let (rx, _) = collect(rx, is_online).await;
        let dev = device();

        let (rx, out) =
            call(&rig.engine, &outbox, rx, 1, LibraryOp::MakeDir { device: dev.clone(), path: "Download/Mới".into() })
                .await;
        assert_eq!(out, Outcome::Failed { error: CommandError::Forbidden });
        assert!(phone.made_dirs().is_empty());

        phone.set_features(&["dirs", "browse", "edit"]);
        // Máy đang Online: `List` đọc lại `info` ⇒ có `edit`.
        let (rx, _) = call(&rig.engine, &outbox, rx, 2, LibraryOp::List { device: dev.clone() }).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (rx, out) =
            call(&rig.engine, &outbox, rx, 3, LibraryOp::MakeDir { device: dev.clone(), path: "Download/Mới".into() })
                .await;
        assert_eq!(out, Outcome::Edited { path: "Download/Mới".into() });
        let (rx, out) =
            call(&rig.engine, &outbox, rx, 4, LibraryOp::MakeDir { device: dev.clone(), path: "Download/Mới".into() })
                .await;
        assert_eq!(out, Outcome::Failed { error: CommandError::Exists });
        let rename = LibraryOp::Rename { device: dev.clone(), path: "Download/a.mp4".into(), name: "b.mp4".into() };
        let (rx, out) = call(&rig.engine, &outbox, rx, 5, rename).await;
        assert_eq!(out, Outcome::Edited { path: "Download/b.mp4".into() });
        let (rx, out) =
            call(&rig.engine, &outbox, rx, 6, LibraryOp::Delete { device: dev.clone(), path: "Download".into() }).await;
        assert_eq!(out, Outcome::Done);
        let (rx, out) =
            call(&rig.engine, &outbox, rx, 7, LibraryOp::Delete { device: dev.clone(), path: "Download".into() }).await;
        assert_eq!(out, Outcome::Failed { error: CommandError::Missing });
        assert!(phone.files().is_empty() && phone.made_dirs().is_empty());

        // W-9: chưa khai `copy` ⇒ Forbidden; có rồi thì chép / dời tới điện thoại.
        phone.add_file("c.mp4", Some("Movies"), vec![1; 3]);
        let copy = || LibraryOp::Copy { device: dev.clone(), path: "Movies/c.mp4".into(), dir: "Movies".into() };
        let (rx, out) = call(&rig.engine, &outbox, rx, 8, copy()).await;
        assert_eq!(out, Outcome::Failed { error: CommandError::Forbidden });
        phone.set_features(&["dirs", "browse", "edit", "copy"]);
        // Lần đọc `info` trước còn trong `INFO_REFRESH_EVERY`: chờ hết rồi `List` mới đọc lại.
        tokio::time::sleep(INFO_REFRESH_EVERY).await;
        let (rx, _) = call(&rig.engine, &outbox, rx, 9, LibraryOp::List { device: dev.clone() }).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (rx, out) = call(&rig.engine, &outbox, rx, 10, copy()).await;
        assert_eq!(out, Outcome::Edited { path: "Movies/c (2).mp4".into() });
        let mv = LibraryOp::Move { device: dev.clone(), path: "Movies/c.mp4".into(), dir: String::new() };
        let (rx, out) = call(&rig.engine, &outbox, rx, 11, mv).await;
        assert_eq!(out, Outcome::Edited { path: "c.mp4".into() });
        let mv = LibraryOp::Move { device: dev, path: "Movies".into(), dir: "Movies".into() };
        let (_rx, out) = call(&rig.engine, &outbox, rx, 12, mv).await;
        assert_eq!(out, Outcome::Failed { error: CommandError::InvalidInput });
        rig.engine.shutdown();
    }

    /// W-10: Sao chép tải vào thư mục tạm rồi mới đặt clipboard; Cắt xoá trên điện thoại chỉ sau khi mục tạm bị dời đi.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn export_copies_then_moves_after_paste() {
        async fn call(
            engine: &EngineHandle,
            outbox: &Outbox,
            rx: OutboxReceiver,
            req: u64,
            op: LibraryOp,
        ) -> (OutboxReceiver, Outcome) {
            engine.send(Input::Request { outbox: outbox.clone(), req, op: Op::Library(op) });
            let (rx, got) = collect(rx, move |m| matches!(m, DaemonToUi::Reply { req: r, .. } if *r == req)).await;
            let DaemonToUi::Reply { outcome, .. } = got.last().unwrap().1.clone() else { unreachable!() };
            (rx, outcome)
        }
        let tmp = tempfile::tempdir().unwrap();
        let phone = FakePhone::start().await;
        phone.add_file("a.mp4", Some("Download"), vec![3; 5000]);
        phone.add_file("x.mp4", Some("Phim/Sub"), vec![4; 7000]);
        phone.set_features(&["dirs", "browse", "fetch"]);
        let rig = start(tmp.path(), &phone, None).await;
        let (outbox, rx) = Outbox::new();
        rig.engine.send(Input::Subscribe { conn: 1, outbox: outbox.clone() });
        let (rx, _) = collect(rx, is_online).await;
        let dev = device();
        let entry = |path: &str, folder| ExportEntry { path: path.into(), folder };

        // Cắt cần `edit`.
        let cut = LibraryOp::Export { device: dev.clone(), entries: vec![entry("Download/a.mp4", false)], cut: true };
        let (rx, out) = call(&rig.engine, &outbox, rx, 1, cut).await;
        assert_eq!(out, Outcome::Failed { error: CommandError::Forbidden });

        let copy = LibraryOp::Export { device: dev.clone(), entries: vec![entry("Download/a.mp4", false)], cut: false };
        let (rx, out) = call(&rig.engine, &outbox, rx, 2, copy).await;
        assert_eq!(out, Outcome::Done);
        let (rx, _) = collect(rx, |m| export_is(m, |s| *s == ExportState::Ready)).await;
        let (paths, was_cut) = CLIPBOARD.lock().unwrap().last().cloned().unwrap();
        assert!(!was_cut);
        assert_eq!(std::fs::read(&paths[0]).unwrap(), vec![3; 5000]);
        let session = paths[0].parent().unwrap().to_owned();
        let (rx, out) = call(&rig.engine, &outbox, rx, 3, LibraryOp::CancelExport).await;
        assert_eq!(out, Outcome::Done);
        let (rx, _) = collect(rx, |m| matches!(m, DaemonToUi::Snapshot(s) if s.export.is_none())).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!session.exists(), "cancel removes the session dir");
        assert_eq!(phone.files().len(), 2, "copy never deletes on the phone");

        // Cắt: chưa dán ⇒ không xoá; dời mục tạm đi ⇒ xoá đúng phần đó.
        phone.set_features(&["dirs", "browse", "fetch", "edit"]);
        tokio::time::sleep(INFO_REFRESH_EVERY).await;
        let (rx, _) = call(&rig.engine, &outbox, rx, 4, LibraryOp::List { device: dev.clone() }).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let cut = LibraryOp::Export {
            device: dev.clone(),
            entries: vec![entry("Download/a.mp4", false), entry("Phim", true)],
            cut: true,
        };
        let (rx, out) = call(&rig.engine, &outbox, rx, 5, cut).await;
        assert_eq!(out, Outcome::Done);
        let (rx, _) = collect(rx, |m| export_is(m, |s| *s == ExportState::Ready)).await;
        let (paths, was_cut) = CLIPBOARD.lock().unwrap().last().cloned().unwrap();
        assert!(was_cut);
        assert_eq!(paths.len(), 2);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(phone.files().len(), 2, "nothing pasted yet");

        let dest = tmp.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        for p in &paths {
            std::fs::rename(p, dest.join(p.file_name().unwrap())).unwrap();
        }
        let (_rx, _) = collect(rx, |m| export_is(m, |s| matches!(s, ExportState::Moved { deleted: 4, kept: 0 }))).await;
        assert!(phone.files().is_empty());
        assert_eq!(std::fs::read(dest.join("Phim/Sub/x.mp4")).unwrap(), vec![4; 7000]);
        assert!(rig.captured.lock().unwrap().notices.iter().any(|n| n.body.contains("2")));
        rig.engine.shutdown();
    }

    /// Một lệnh cài đặt, chờ trả lời.
    async fn ask(engine: &EngineHandle, op: SettingsOp) -> Outcome {
        let (outbox, rx) = Outbox::new();
        engine.send(Input::Request { outbox, req: 42, op: Op::Settings(op) });
        let reply = tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(10))).await.unwrap();
        match reply {
            Some(DaemonToUi::Reply { req: 42, outcome }) => outcome,
            other => panic!("no reply: {other:?}"),
        }
    }

    fn view(outcome: Outcome) -> SettingsView {
        match outcome {
            Outcome::Settings(v) => v,
            other => panic!("expected settings, got {other:?}"),
        }
    }

    /// C1 (gd1_12, mức engine): đổi từng mục qua IPC ⇒ `config.json` đúng khoá camelCase, đọc lại còn; lỗi nói rõ.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn c1_settings_are_applied_and_saved() {
        let tmp = tempfile::tempdir().unwrap();
        let phone = FakePhone::start().await;
        let rig = start(tmp.path(), &phone, None).await;
        let e = &rig.engine;
        let first = view(ask(e, SettingsOp::Get).await);
        assert_eq!(first.devices.len(), 1);
        // Bộ thử đặt sẵn `downloadDir` = `<tmp>/dl` (gd1_10).
        assert!(first.download_dir_custom && !first.autostart);

        let dl = tmp.path().join("tải về").join("mới");
        let v = view(ask(e, SettingsOp::SetDownloadDir { path: Some(dl.clone()) }).await);
        assert!(v.download_dir_custom && dl.is_dir(), "created on demand");
        assert_eq!(v.download_dir, dl);
        let rel = ask(e, SettingsOp::SetDownloadDir { path: Some("relative".into()) }).await;
        assert_eq!(rel, Outcome::Failed { error: CommandError::InvalidInput });
        let blocker = tmp.path().join("a-file");
        std::fs::write(&blocker, b"x").unwrap();
        let bad = ask(e, SettingsOp::SetDownloadDir { path: Some(blocker.join("sub")) }).await;
        assert_eq!(bad, Outcome::Failed { error: CommandError::NotWritable });

        assert!(view(ask(e, SettingsOp::SetAutostart { enabled: true }).await).autostart);

        let v = view(ask(e, SettingsOp::Get).await);
        assert_eq!((v.theme, v.theme_preset.as_str()), (ferry_core::config::ThemeChoice::System, "default"));
        let v = view(ask(e, SettingsOp::SetTheme { theme: ferry_core::config::ThemeChoice::Dark }).await);
        assert_eq!(v.theme, ferry_core::config::ThemeChoice::Dark);
        let v = view(ask(e, SettingsOp::SetThemePreset { preset: "rose-red".into() }).await);
        assert_eq!((v.theme, v.theme_preset.as_str()), (ferry_core::config::ThemeChoice::Dark, "rose-red"));

        let v = view(ask(e, SettingsOp::RenameDevice { device: device(), name: "  Máy của mẹ ".into() }).await);
        assert_eq!(v.devices[0].display_name(), "Máy của mẹ");
        let r = ask(e, SettingsOp::RenameDevice { device: DeviceId("zz".into()), name: "x".into() }).await;
        assert_eq!(r, Outcome::Failed { error: CommandError::UnknownDevice });

        let v = view(ask(e, SettingsOp::SetLanguage { language: ferry_core::i18n::LanguagePref::Vi }).await);
        assert_eq!(v.language, ferry_core::i18n::LanguagePref::Vi);
        wait_for("tray in Vietnamese", || {
            rig.captured.lock().unwrap().tray.last().is_some_and(|s| s.language == Language::Vi)
        })
        .await;

        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(tmp.path().join("config.json")).unwrap()).unwrap();
        assert_eq!(json["downloadDir"], dl.to_string_lossy().as_ref());
        assert_eq!(json["autostart"], true);
        assert_eq!(json["devices"][0]["alias"], "Máy của mẹ");
        assert_eq!(json["language"], "vi");
        let reread = config::load(&tmp.path().join("config.json")).unwrap();
        assert_eq!(reread.devices[0].alias.as_deref(), Some("Máy của mẹ"));
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
        {
            let mut st = lock(&store);
            for i in 0..1000 {
                let d = NewDownload {
                    remote_id: format!("content://media/external/video/media/{i:08}"),
                    name: format!("VID_{i:08}_a rather long phone file name.mp4"),
                    remote_dir: Some(format!("DCIM/Camera/a rather long folder name {i}")),
                    size: 123_456_789,
                    dest: PathBuf::from(format!(
                        "/home/someone/Downloads/VID_{i:08}_a rather long phone file name.mp4"
                    )),
                };
                st.add_download(&device(), &d, i).unwrap();
            }
        }
        let snap = snapshot_parts(&store, &Devices::default(), &[], &Summary::default(), &HashMap::new(), uploads);
        assert_eq!(snap.uploads.len(), QUEUED_IN_SNAPSHOT + FINISHED_IN_SNAPSHOT);
        assert_eq!(snap.queued_omitted, 5000 - QUEUED_IN_SNAPSHOT as u64);
        assert_eq!(snap.finished_omitted, 1000 - FINISHED_IN_SNAPSHOT as u64);
        assert_eq!(snap.downloads.len(), DOWNLOADS_IN_SNAPSHOT);
        assert_eq!(snap.downloads_omitted, 1000 - DOWNLOADS_IN_SNAPSHOT as u64);
        assert!(snap.downloads.windows(2).all(|w| w[0].id < w[1].id));
        assert_eq!(snap.uploads[0].id, 1, "head of the queue first");
        assert_eq!(snap.uploads.last().unwrap().id, 6000, "newest finished kept");
        let line = serde_json::to_string(&DaemonToUi::Snapshot(snap)).unwrap();
        assert!((line.len() as u64) < ferry_core::ipc::MAX_LINE, "{} bytes", line.len());
    }
}
