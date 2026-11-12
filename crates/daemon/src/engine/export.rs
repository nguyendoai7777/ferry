//! W-10 (PROTOCOL §6.14): Cắt / Sao chép sang máy tính. Một việc mỗi lúc (lần mới thay lần cũ).
//!
//! Một task nền làm trọn đời việc: liệt kê cây (`export::plan`) → tải vào thư mục phiên (`export::download`, nối tiếp
//! khi đứt) → đặt danh sách tệp lên clipboard (luồng chặn) → Cắt: mỗi giây soát mục tạm nào đã bị trình quản lý tệp dời
//! đi rồi xoá đúng phần đó trên điện thoại (`export::remove_moved`); Sao chép: chỉ chờ hết hạn. Engine chỉ đổi trạng thái
//! theo [`ExportMsg`] và dọn khi huỷ / thay / hết hạn. Không bao giờ xoá trên điện thoại khi chưa thấy mục tạm biến mất.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferry_core::client::{ClientError, PhoneClient};
use ferry_core::device::DeviceId;
use ferry_core::export::{self, ExportEntry, ExportError, ExportPlan, Meter, Removal};
use ferry_core::ipc::{CommandError, ExportState, ExportView, Outcome};
use ferry_core::protocol::feature;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::{Engine, Internal, read_error, spawn_info_refresh};
use crate::clipboard::Clipboard;

/// Nhịp đẩy tiến độ lúc đang tải, và nhịp soát mục tạm đã bị dời (Cắt).
const TICK: Duration = Duration::from_millis(300);
const WATCH_EVERY: Duration = Duration::from_secs(1);
/// "Đã dời sang máy tính" hiện chừng này rồi tự ẩn.
const MOVED_SHOWN_FOR: Duration = Duration::from_secs(8);

pub(super) struct ExportJob {
    id: u64,
    device: DeviceId,
    cut: bool,
    paths: Vec<String>,
    root: PathBuf,
    meter: Arc<Meter>,
    /// `(byte, tệp)` khi đã liệt kê xong.
    totals: Option<(u64, u64)>,
    state: JobState,
    removal: Removal,
    task: JoinHandle<()>,
}

enum JobState {
    Preparing,
    Ready,
    Moved,
    Failed(CommandError),
}

pub(super) enum ExportMsg {
    Planned {
        id: u64,
        bytes: u64,
        files: u64,
    },
    Tick {
        id: u64,
    },
    Ready {
        id: u64,
    },
    Failed {
        id: u64,
        error: CommandError,
    },
    /// Đã xoá trên điện thoại phần của một mục cấp đầu vừa dán.
    Removed {
        id: u64,
        removal: Removal,
    },
    /// Cắt: mọi mục đã dán và xoá xong.
    AllMoved {
        id: u64,
    },
    /// Hết hạn giữ (`KEEP_FOR`), thư mục phiên mất, hoặc "Đã dời" hiện đủ lâu ⇒ dọn.
    Expired {
        id: u64,
    },
}

impl ExportJob {
    pub(super) fn view(&self) -> ExportView {
        let state = match &self.state {
            JobState::Preparing => {
                let (total_bytes, total_files) = self.totals.unwrap_or_default();
                ExportState::Preparing {
                    done_bytes: self.meter.bytes().min(total_bytes),
                    total_bytes,
                    done_files: self.meter.files(),
                    total_files,
                }
            }
            JobState::Ready => ExportState::Ready,
            JobState::Moved => ExportState::Moved { deleted: self.removal.deleted, kept: self.removal.kept },
            JobState::Failed(e) => ExportState::Failed { error: e.clone() },
        };
        ExportView { device: self.device.clone(), cut: self.cut, paths: self.paths.clone(), state }
    }
}

impl Engine {
    pub(super) fn start_export(
        &mut self,
        device: DeviceId,
        entries: Vec<ExportEntry>,
        cut: bool,
    ) -> Result<Outcome, CommandError> {
        if entries.is_empty() || entries.iter().any(|e| e.path.trim_matches('/').is_empty()) {
            return Err(CommandError::InvalidInput);
        }
        let client = self.online_client(&device)?;
        let has = |f: &str| self.devices.get(&device).is_some_and(|d| d.features.iter().any(|x| x == f));
        if !has(feature::FETCH) || (cut && !has(feature::EDIT)) {
            return Err(CommandError::Forbidden);
        }
        self.drop_export(false);
        self.export_seq += 1;
        let id = self.export_seq;
        let root = export::new_session_dir(&self.export_base);
        let meter = Arc::new(Meter::default());
        let paths = entries.iter().map(|e| e.path.clone()).collect();
        let ctx = TaskCtx {
            id,
            device: device.clone(),
            client,
            entries,
            root: root.clone(),
            cut,
            meter: meter.clone(),
            clipboard: self.clipboard,
            tx: self.internal_tx.clone(),
        };
        log::info!("engine: export #{id} from {device} ({} item(s), cut={cut})", ctx.entries.len());
        let task = tokio::spawn(run(ctx));
        self.export = Some(ExportJob {
            id,
            device,
            cut,
            paths,
            root,
            meter,
            totals: None,
            state: JobState::Preparing,
            removal: Removal::default(),
            task,
        });
        self.dirty = true;
        Ok(Outcome::Done)
    }

    /// Bỏ việc đang có: dừng task, xoá thư mục tạm, bỏ clipboard (`clear_clipboard`: lần mới sẽ đặt đè thì khỏi bỏ).
    pub(super) fn drop_export(&mut self, clear_clipboard: bool) {
        let Some(job) = self.export.take() else { return };
        job.task.abort();
        let had_clipboard = matches!(job.state, JobState::Ready);
        let (root, clear) = (job.root, self.clipboard.clear);
        tokio::task::spawn_blocking(move || {
            if clear_clipboard && had_clipboard {
                clear();
            }
            export::discard(&root);
        });
        self.dirty = true;
    }

    pub(super) fn on_export(&mut self, msg: ExportMsg) {
        let id = match &msg {
            ExportMsg::Planned { id, .. }
            | ExportMsg::Tick { id }
            | ExportMsg::Ready { id }
            | ExportMsg::Failed { id, .. }
            | ExportMsg::Removed { id, .. }
            | ExportMsg::AllMoved { id }
            | ExportMsg::Expired { id } => *id,
        };
        let Some(job) = self.export.as_mut().filter(|j| j.id == id) else { return };
        match msg {
            ExportMsg::Planned { bytes, files, .. } => job.totals = Some((bytes, files)),
            ExportMsg::Tick { .. } => {}
            ExportMsg::Ready { .. } => {
                log::info!("engine: export #{id} ready to paste");
                job.state = JobState::Ready;
            }
            ExportMsg::Failed { error, .. } => {
                log::info!("engine: export #{id} failed: {error:?}");
                job.state = JobState::Failed(error);
                let root = job.root.clone();
                tokio::task::spawn_blocking(move || export::discard(&root));
            }
            ExportMsg::Removed { removal, .. } => {
                job.removal.deleted += removal.deleted;
                job.removal.kept += removal.kept;
            }
            ExportMsg::AllMoved { .. } => {
                job.state = JobState::Moved;
                let (device, removal, n) = (job.device.clone(), job.removal, job.paths.len());
                log::info!(
                    "engine: export #{id} moved, deleted {} kept {} on the phone",
                    removal.deleted,
                    removal.kept
                );
                let phone = self.phone_name(&device);
                let count = n.to_string();
                let title = self.i18n.t_args("notice.moved_title", &[("phone", &phone)]);
                let mut body = self
                    .i18n
                    .t_args(if n == 1 { "notice.moved_body_one" } else { "notice.moved_body" }, &[("count", &count)]);
                if removal.kept > 0 {
                    body.push('\n');
                    body.push_str(&self.i18n.t_args("notice.moved_kept", &[("count", &removal.kept.to_string())]));
                }
                self.notify(title, body);
            }
            ExportMsg::Expired { .. } => {
                log::info!("engine: export #{id} expired");
                self.drop_export(true);
            }
        }
        self.dirty = true;
    }

    pub(super) fn export_view(&self) -> Option<Box<ExportView>> {
        self.export.as_ref().map(|j| Box::new(j.view()))
    }
}

struct TaskCtx {
    id: u64,
    device: DeviceId,
    client: Arc<PhoneClient>,
    entries: Vec<ExportEntry>,
    root: PathBuf,
    cut: bool,
    meter: Arc<Meter>,
    clipboard: Clipboard,
    tx: mpsc::UnboundedSender<Internal>,
}

impl TaskCtx {
    fn send(&self, msg: ExportMsg) {
        let _ = self.tx.send(Internal::Export(msg));
    }

    fn error(&self, e: ExportError) -> CommandError {
        match e {
            ExportError::Missing(path) => {
                log::info!("engine: export #{}: {path} is gone", self.id);
                CommandError::Missing
            }
            ExportError::TooMany => CommandError::TooMany,
            ExportError::Local(e) => {
                log::warn!("engine: export #{}: {e}", self.id);
                CommandError::NotWritable
            }
            ExportError::Client(e) => self.client_error(e),
        }
    }

    fn client_error(&self, e: ClientError) -> CommandError {
        match e {
            ClientError::NotFound => CommandError::Missing,
            ClientError::BadRequest => CommandError::InvalidInput,
            // Công tắc tắt / mất quyền (`403`): đọc lại `info` để UI làm mờ mục.
            ClientError::WrongPin => {
                spawn_info_refresh(&self.tx, &self.device, &self.client);
                CommandError::Forbidden
            }
            e => read_error(&self.tx, &self.device, &self.client, "fetch", e),
        }
    }
}

async fn run(ctx: TaskCtx) {
    let plan = match export::plan(&ctx.client, &ctx.entries, &ctx.root).await {
        Ok(p) => p,
        Err(e) => return ctx.send(ExportMsg::Failed { id: ctx.id, error: ctx.error(e) }),
    };
    ctx.send(ExportMsg::Planned { id: ctx.id, bytes: plan.total_bytes(), files: plan.files.len() as u64 });

    let download = export::download(&ctx.client, &plan, &ctx.meter);
    tokio::pin!(download);
    let mut tick = tokio::time::interval(TICK);
    let done = loop {
        tokio::select! {
            r = &mut download => break r,
            _ = tick.tick() => ctx.send(ExportMsg::Tick { id: ctx.id }),
        }
    };
    if let Err(e) = done {
        return ctx.send(ExportMsg::Failed { id: ctx.id, error: ctx.error(e) });
    }

    let (tops, cut, set_files) = (plan.top_paths(), ctx.cut, ctx.clipboard.set_files);
    let set = tokio::task::spawn_blocking(move || set_files(&tops, cut)).await;
    match set {
        Ok(Ok(())) => ctx.send(ExportMsg::Ready { id: ctx.id }),
        Ok(Err(e)) => {
            log::warn!("engine: export #{}: clipboard: {e}", ctx.id);
            return ctx.send(ExportMsg::Failed { id: ctx.id, error: CommandError::Internal });
        }
        Err(e) => {
            log::warn!("engine: export #{}: clipboard task: {e}", ctx.id);
            return ctx.send(ExportMsg::Failed { id: ctx.id, error: CommandError::Internal });
        }
    }

    let ready_at = Instant::now();
    if ctx.cut {
        if watch_cut(&ctx, &plan, ready_at).await {
            ctx.send(ExportMsg::AllMoved { id: ctx.id });
            let (root, clear) = (plan.root.clone(), ctx.clipboard.clear);
            let _ = tokio::task::spawn_blocking(move || {
                export::discard(&root);
                clear();
            })
            .await;
            tokio::time::sleep(MOVED_SHOWN_FOR).await;
        }
    } else {
        tokio::time::sleep(export::KEEP_FOR).await;
    }
    ctx.send(ExportMsg::Expired { id: ctx.id });
}

/// Soát mỗi giây mục tạm nào đã bị dời đi, xoá phần đó trên điện thoại. `true` khi mọi mục đã dán và xoá xong;
/// `false` khi hết hạn hoặc thư mục phiên mất (không rõ có phải dán không ⇒ không xoá gì nữa).
async fn watch_cut(ctx: &TaskCtx, plan: &ExportPlan, ready_at: Instant) -> bool {
    let mut removed = vec![false; plan.tops.len()];
    let mut tick = tokio::time::interval(WATCH_EVERY);
    loop {
        tick.tick().await;
        if ready_at.elapsed() >= export::KEEP_FOR {
            return false;
        }
        // Vài lệnh `stat` trên đĩa máy — khỏi `spawn_blocking`.
        let Some(moved) = export::moved_tops(plan) else {
            log::info!("engine: export #{}: {} vanished, nothing more is deleted", ctx.id, plan.root.display());
            return false;
        };
        for top in moved {
            if removed[top] {
                continue;
            }
            match export::remove_moved(&ctx.client, plan, top).await {
                Ok(removal) => {
                    removed[top] = true;
                    ctx.send(ExportMsg::Removed { id: ctx.id, removal });
                }
                // Mất mạng / điện thoại tắt: phần đã xoá vẫn xoá, thử lại nhịp sau (gọi lại an toàn).
                Err(e) => log::info!("engine: export #{}: delete {} on the phone: {e}", ctx.id, plan.tops[top].remote),
            }
        }
        if removed.iter().all(|r| *r) {
            return true;
        }
    }
}
