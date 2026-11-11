//! Hàng đợi bền trên SQLite (`PROTOCOL.md` 2.3, 5): `<data_dir>/queue.sqlite3`, WAL, migration theo `user_version`.
//!
//! Mỗi lần chuyển trạng thái ghi ngay một transaction (có `uid` ⇒ lưu liền; sau mỗi khúc ⇒ lưu offset). Trạng thái đầy
//! đủ nằm ở cột `state` (JSON của [`UploadState`] / [`DownloadState`]); `status`, `done_bytes`, `wake_ms` là bản sao để
//! truy vấn (đầu hàng theo thiết bị, tổng tiến độ lô, đồng hồ kế tiếp) không phải giải JSON.
//!
//! `synchronous = NORMAL` với WAL: app sập không hỏng DB và không mất transaction đã commit; mất điện có thể mất vài
//! transaction cuối — chấp nhận được vì offset được `HEAD` hỏi lại, phiên upload mất `uid` tự hết hạn trên điện thoại.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, Row, Transaction, params};
use serde::Serialize;

use super::batch::{self, BatchPlan};
use super::state::{
    DownloadEffect, DownloadEvent, DownloadMachine, DownloadState, FailReason, UploadEffect, UploadEvent,
    UploadMachine, UploadState,
};
use super::{BatchId, UnixMs};
use crate::device::DeviceId;
use crate::paths;

pub const DB_FILE: &str = "queue.sqlite3";
/// Mục "Đã gửi" (Done / Skipped / Cancelled) giữ bấy lâu rồi dọn (`PROTOCOL.md` 4.2).
pub const KEEP_FINISHED_MS: UnixMs = 7 * 24 * 60 * 60 * 1000;
/// Dọn lúc mở store + mỗi chừng này một lần (runner đặt đồng hồ, không timer dày).
pub const PURGE_INTERVAL_MS: UnixMs = 24 * 60 * 60 * 1000;

/// Migration theo thứ tự; phần tử `i` đưa `user_version` từ `i` lên `i + 1`. Chỉ thêm vào cuối, không sửa bản cũ.
const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE batches (
    id            TEXT PRIMARY KEY,
    device_id     TEXT NOT NULL,
    dest_dir      TEXT NOT NULL,
    label         TEXT,
    skipped_files INTEGER NOT NULL DEFAULT 0,
    created_ms    INTEGER NOT NULL
);
CREATE TABLE items (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    kind        TEXT NOT NULL CHECK (kind IN ('upload', 'download')),
    device_id   TEXT NOT NULL,
    batch_id    TEXT REFERENCES batches(id) ON DELETE CASCADE,
    local_path  TEXT NOT NULL,       -- upload: tệp nguồn; download: tệp đích
    name        TEXT NOT NULL,       -- upload: tên gửi; download: tên trên điện thoại
    dir         TEXT,                -- upload: dir đích chuẩn hoá; download: dir trên điện thoại
    remote_id   TEXT,                -- download: id của list
    size        INTEGER NOT NULL,
    mtime_ms    INTEGER,             -- upload: mtime nguồn lúc thả
    status      TEXT NOT NULL,
    state       TEXT NOT NULL,       -- JSON máy trạng thái
    failures    INTEGER NOT NULL DEFAULT 0,
    done_bytes  INTEGER NOT NULL DEFAULT 0,
    wake_ms     INTEGER,             -- Waiting.until
    created_ms  INTEGER NOT NULL,
    updated_ms  INTEGER NOT NULL,
    finished_ms INTEGER
);
CREATE INDEX items_head ON items(device_id, kind, id);
CREATE INDEX items_batch ON items(batch_id);
CREATE INDEX items_wake ON items(wake_ms) WHERE wake_ms IS NOT NULL;
CREATE INDEX items_finished ON items(finished_ms) WHERE finished_ms IS NOT NULL;
"#];

pub type ItemId = i64;

#[derive(Debug)]
pub enum StoreError {
    Sql(rusqlite::Error),
    Json(serde_json::Error),
    Io(io::Error),
    /// DB do bản app mới hơn tạo — không đụng.
    NewerSchema(i64),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Sql(e) => write!(f, "queue db: {e}"),
            StoreError::Json(e) => write!(f, "queue state json: {e}"),
            StoreError::Io(e) => write!(f, "queue io: {e}"),
            StoreError::NewerSchema(v) => write!(f, "queue db schema v{v} is newer than this app"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> StoreError {
        StoreError::Sql(e)
    }
}
impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> StoreError {
        StoreError::Json(e)
    }
}
impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> StoreError {
        StoreError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Một mục upload như store giữ (cũng là dữ liệu cho ảnh chụp trạng thái gửi UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadItem {
    pub id: ItemId,
    pub device_id: DeviceId,
    pub batch_id: Option<BatchId>,
    pub path: PathBuf,
    pub name: String,
    pub dir: String,
    pub size: u64,
    pub mtime: Option<UnixMs>,
    pub state: UploadState,
    pub failures: u32,
    pub created: UnixMs,
    pub updated: UnixMs,
}

impl UploadItem {
    pub fn machine(&self) -> UploadMachine {
        UploadMachine { state: self.state.clone(), failures: self.failures }
    }
}

/// Video trên điện thoại cần tải về (gd1_10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDownload {
    /// `id` mờ của `list`.
    pub remote_id: String,
    pub name: String,
    pub remote_dir: Option<String>,
    pub size: u64,
    /// Tệp đích cuối cùng (`.part` cạnh nó do runner quản).
    pub dest: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadItem {
    pub id: ItemId,
    pub device_id: DeviceId,
    pub remote_id: String,
    pub name: String,
    pub remote_dir: Option<String>,
    pub size: u64,
    pub dest: PathBuf,
    pub state: DownloadState,
    pub failures: u32,
    pub created: UnixMs,
    pub updated: UnixMs,
}

impl DownloadItem {
    pub fn machine(&self) -> DownloadMachine {
        DownloadMachine { state: self.state.clone(), failures: self.failures }
    }
}

/// Dòng gộp của một lô.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchInfo {
    pub id: BatchId,
    pub device_id: DeviceId,
    pub dest_dir: String,
    pub label: Option<String>,
    pub skipped_files: u64,
    pub created: UnixMs,
}

/// Tổng tiến độ một lô (`PROTOCOL.md` 5: dòng gộp "N tệp", tổng tiến độ).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchProgress {
    pub files: u64,
    /// Done + Skipped.
    pub finished_files: u64,
    pub failed_files: u64,
    pub paused_files: u64,
    pub cancelled_files: u64,
    /// Tổng byte của mục chưa huỷ.
    pub total_bytes: u64,
    pub done_bytes: u64,
}

impl BatchProgress {
    /// Không còn mục nào cần chạy (thông báo hệ thống "cả lô xong").
    pub fn is_settled(&self) -> bool {
        self.finished_files + self.failed_files + self.cancelled_files == self.files
    }
}

pub struct QueueStore {
    conn: Connection,
}

const UPLOAD_COLS: &str =
    "id, device_id, batch_id, local_path, name, dir, size, mtime_ms, state, failures, created_ms, updated_ms";
const DOWNLOAD_COLS: &str =
    "id, device_id, remote_id, name, dir, size, local_path, state, failures, created_ms, updated_ms";

impl QueueStore {
    /// `<data_dir>/queue.sqlite3`.
    pub fn default_path() -> io::Result<PathBuf> {
        paths::data_dir().map(|d| d.join(DB_FILE))
    }

    pub fn open_default(now: UnixMs) -> Result<QueueStore> {
        let path = QueueStore::default_path()?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        QueueStore::open(&path, now)
    }

    /// Mở (tạo nếu chưa có), migrate, đưa request đang bay lúc tắt app về `Waiting` / `Queued`, dọn mục cũ.
    pub fn open(path: &Path, now: UnixMs) -> Result<QueueStore> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // Tiến trình nền cả ngày (RAM là tính năng): hàng đợi vài nghìn dòng, 512 KB cache là đủ (mặc định 2 MB).
        conn.pragma_update(None, "cache_size", -512)?;
        let mut store = QueueStore { conn };
        store.migrate()?;
        store.restore(now)?;
        let purged = store.purge_finished(now)?;
        if purged > 0 {
            log::info!("queue: purged {purged} finished items older than 7 days");
        }
        Ok(store)
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let target = MIGRATIONS.len() as i64;
        if version > target {
            return Err(StoreError::NewerSchema(version));
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            tx.commit()?;
            log::info!("queue: migrated db to v{}", i + 1);
        }
        Ok(())
    }

    /// Phiên bản schema hiện tại.
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    fn restore(&mut self, now: UnixMs) -> Result<()> {
        let tx = self.conn.transaction()?;
        let ups: Vec<UploadItem> = query_uploads(&tx, "status IN ('checking', 'creating', 'sending')", [])?;
        for item in ups {
            let (m, _) = item.machine().next(&UploadEvent::Restore, now);
            save_upload(&tx, item.id, item.size, &m, now)?;
        }
        let downs: Vec<DownloadItem> = query_downloads(&tx, "status = 'fetching'", [])?;
        for item in downs {
            let (m, _) = item.machine().next(&DownloadEvent::Restore, now);
            save_download(&tx, item.id, item.size, &m, now)?;
        }
        tx.commit()?;
        Ok(())
    }

    // ------------------------------------------------------------ thêm

    /// Ghi một lô (một transaction). Lô rỗng (không có video) vẫn trả `BatchId` nhưng không ghi gì.
    pub fn add_batch(&mut self, device: &DeviceId, plan: &BatchPlan, now: UnixMs) -> Result<(BatchId, Vec<ItemId>)> {
        let batch_id = BatchId::new();
        if plan.files.is_empty() {
            return Ok((batch_id, Vec::new()));
        }
        let state = serde_json::to_string(&UploadState::Queued)?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO batches (id, device_id, dest_dir, label, skipped_files, created_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![batch_id.0, device.0, plan.dest_dir, plan.label, plan.skipped_non_video as i64, now],
        )?;
        let mut ids = Vec::with_capacity(plan.files.len());
        {
            let mut stmt = tx.prepare(
                "INSERT INTO items (kind, device_id, batch_id, local_path, name, dir, size, mtime_ms, status, state,
                                    created_ms, updated_ms)
                 VALUES ('upload', ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8, ?9, ?9)",
            )?;
            for f in &plan.files {
                stmt.execute(params![
                    device.0,
                    batch_id.0,
                    path_to_db(&f.path),
                    f.name,
                    f.dir,
                    f.size as i64,
                    f.mtime,
                    state,
                    now
                ])?;
                ids.push(tx.last_insert_rowid());
            }
        }
        tx.commit()?;
        Ok((batch_id, ids))
    }

    pub fn add_download(&mut self, device: &DeviceId, d: &NewDownload, now: UnixMs) -> Result<ItemId> {
        let state = serde_json::to_string(&DownloadState::Queued)?;
        self.conn.execute(
            "INSERT INTO items (kind, device_id, remote_id, name, dir, size, local_path, status, state, created_ms,
                                updated_ms)
             VALUES ('download', ?1, ?2, ?3, ?4, ?5, ?6, 'queued', ?7, ?8, ?8)",
            params![device.0, d.remote_id, d.name, d.remote_dir, d.size as i64, path_to_db(&d.dest), state, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    // ------------------------------------------------------------ đọc

    pub fn upload(&self, id: ItemId) -> Result<Option<UploadItem>> {
        Ok(query_uploads(&self.conn, "id = ?1", [id])?.pop())
    }

    pub fn download(&self, id: ItemId) -> Result<Option<DownloadItem>> {
        Ok(query_downloads(&self.conn, "id = ?1", [id])?.pop())
    }

    /// Mọi mục upload (cả "Đã gửi" chưa dọn), theo thứ tự thả vào.
    pub fn uploads(&self) -> Result<Vec<UploadItem>> {
        query_uploads(&self.conn, "1", [])
    }

    pub fn downloads(&self) -> Result<Vec<DownloadItem>> {
        query_downloads(&self.conn, "1", [])
    }

    pub fn batch_uploads(&self, batch: &BatchId) -> Result<Vec<UploadItem>> {
        query_uploads(&self.conn, "batch_id = ?1", [&batch.0])
    }

    pub fn batches(&self) -> Result<Vec<BatchInfo>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, device_id, dest_dir, label, skipped_files, created_ms FROM batches ORDER BY created_ms, rowid",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(BatchInfo {
                id: BatchId(r.get(0)?),
                device_id: DeviceId(r.get(1)?),
                dest_dir: r.get(2)?,
                label: r.get(3)?,
                skipped_files: r.get::<_, i64>(4)? as u64,
                created: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn batch_progress(&self, batch: &BatchId) -> Result<BatchProgress> {
        let p = self.conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(status IN ('done', 'skipped')), 0),
                    COALESCE(SUM(status = 'failed'), 0),
                    COALESCE(SUM(status = 'paused'), 0),
                    COALESCE(SUM(status = 'cancelled'), 0),
                    COALESCE(SUM(CASE WHEN status = 'cancelled' THEN 0 ELSE size END), 0),
                    COALESCE(SUM(CASE WHEN status = 'cancelled' THEN 0 ELSE done_bytes END), 0)
             FROM items WHERE batch_id = ?1",
            [&batch.0],
            |r| {
                let u = |i: usize| r.get::<_, i64>(i).map(|v| v as u64);
                Ok(BatchProgress {
                    files: u(0)?,
                    finished_files: u(1)?,
                    failed_files: u(2)?,
                    paused_files: u(3)?,
                    cancelled_files: u(4)?,
                    total_bytes: u(5)?,
                    done_bytes: u(6)?,
                })
            },
        )?;
        Ok(p)
    }

    /// Đầu hàng upload của một thiết bị (một tệp một lúc, `PROTOCOL.md` 2.8): mục chưa kết thúc, không tạm dừng, sớm
    /// nhất. `Waiting` chưa tới giờ ⇒ `None` (runner ngủ tới [`QueueStore::next_wake`]).
    pub fn next_upload(&self, device: &DeviceId, now: UnixMs) -> Result<Option<UploadItem>> {
        let item = self.pending_upload(device)?;
        Ok(item.filter(|i| !matches!(i.state, UploadState::Waiting { until, .. } if until > now)))
    }

    /// Đầu hàng upload của một thiết bị kể cả khi đang chờ backoff — runner (gd1_04) lấy `Waiting.until` của riêng
    /// thiết bị mình để ngủ, và đánh thức sớm khi điện thoại hiện lại ở địa chỉ mới.
    pub fn pending_upload(&self, device: &DeviceId) -> Result<Option<UploadItem>> {
        Ok(query_uploads(
            &self.conn,
            "device_id = ?1 AND status IN ('queued', 'checking', 'creating', 'sending', 'waiting') ORDER BY id LIMIT 1",
            [&device.0],
        )?
        .pop())
    }

    pub fn next_download(&self, device: &DeviceId, now: UnixMs) -> Result<Option<DownloadItem>> {
        let item = self.pending_download(device)?;
        Ok(item.filter(|i| !matches!(i.state, DownloadState::Waiting { until, .. } if until > now)))
    }

    /// Đầu hàng tải về của một thiết bị kể cả khi đang chờ backoff (như [`Self::pending_upload`]; runner gd1_10 chọn
    /// giữa hai đầu hàng theo `id`).
    pub fn pending_download(&self, device: &DeviceId) -> Result<Option<DownloadItem>> {
        Ok(query_downloads(
            &self.conn,
            "device_id = ?1 AND status IN ('queued', 'fetching', 'waiting') ORDER BY id LIMIT 1",
            [&device.0],
        )?
        .pop())
    }

    /// Tệp đích thật sau khi tải xong (tên từ `Content-Disposition`, đã làm sạch / thêm ` (2)` — gd1_10).
    pub fn set_download_dest(&mut self, id: ItemId, dest: &Path) -> Result<()> {
        self.conn.execute(
            "UPDATE items SET local_path = ?2 WHERE id = ?1 AND kind = 'download'",
            params![id, path_to_db(dest)],
        )?;
        Ok(())
    }

    /// Đồng hồ `Waiting` sớm nhất (mọi thiết bị, mọi chiều) — nguồn đánh thức của runner.
    pub fn next_wake(&self) -> Result<Option<UnixMs>> {
        Ok(self.conn.query_row("SELECT MIN(wake_ms) FROM items WHERE wake_ms IS NOT NULL", [], |r| r.get(0))?)
    }

    /// Thiết bị còn việc (chưa kết thúc, không tạm dừng).
    pub fn devices_with_work(&self) -> Result<Vec<DeviceId>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT device_id FROM items
             WHERE status IN ('queued', 'checking', 'creating', 'sending', 'waiting', 'fetching') ORDER BY device_id",
        )?;
        let rows = stmt.query_map([], |r| r.get(0).map(DeviceId))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ------------------------------------------------------------ chuyển trạng thái

    /// Áp một sự kiện lên mục upload và ghi ngay. `None` khi không có mục.
    pub fn apply_upload(&mut self, id: ItemId, event: &UploadEvent, now: UnixMs) -> Result<Option<UploadEffect>> {
        let tx = self.conn.transaction()?;
        let Some(item) = query_uploads(&tx, "id = ?1", [id])?.pop() else {
            return Ok(None);
        };
        let (m, effect) = item.machine().next(event, now);
        if m != item.machine() {
            save_upload(&tx, id, item.size, &m, now)?;
        }
        tx.commit()?;
        Ok(Some(effect))
    }

    /// "Thử lại" của người dùng: mục `Failed` vì tệp nguồn đổi / mất ⇒ đọc lại cỡ + mtime (người dùng đã sửa tệp và
    /// muốn gửi bản mới) rồi áp `Resume`.
    pub fn retry_upload(&mut self, id: ItemId, now: UnixMs) -> Result<Option<UploadEffect>> {
        if let Some(item) = self.upload(id)?
            && matches!(
                item.state,
                UploadState::Failed { reason: FailReason::SourceChanged | FailReason::SourceMissing }
            )
            && let Ok(meta) = std::fs::metadata(&item.path)
        {
            self.conn.execute(
                "UPDATE items SET size = ?2, mtime_ms = ?3 WHERE id = ?1",
                params![id, meta.len() as i64, batch::mtime_ms(&meta)],
            )?;
        }
        self.apply_upload(id, &UploadEvent::Resume, now)
    }

    pub fn apply_download(&mut self, id: ItemId, event: &DownloadEvent, now: UnixMs) -> Result<Option<DownloadEffect>> {
        let tx = self.conn.transaction()?;
        let Some(item) = query_downloads(&tx, "id = ?1", [id])?.pop() else {
            return Ok(None);
        };
        let (m, effect) = item.machine().next(event, now);
        if m != item.machine() {
            save_download(&tx, id, item.size, &m, now)?;
        }
        tx.commit()?;
        Ok(Some(effect))
    }

    /// Áp sự kiện người dùng (tạm dừng / chạy tiếp / huỷ) cho cả lô. Trả effect của từng mục (huỷ ⇒ `Delete`).
    pub fn apply_batch(
        &mut self,
        batch: &BatchId,
        event: &UploadEvent,
        now: UnixMs,
    ) -> Result<Vec<(ItemId, UploadEffect)>> {
        let tx = self.conn.transaction()?;
        let mut out = Vec::new();
        for item in query_uploads(&tx, "batch_id = ?1", [&batch.0])? {
            let (m, effect) = item.machine().next(event, now);
            if m != item.machine() {
                save_upload(&tx, item.id, item.size, &m, now)?;
                out.push((item.id, effect));
            }
        }
        tx.commit()?;
        Ok(out)
    }

    /// Xoá mục đã kết thúc (Done / Skipped / Cancelled) quá 7 ngày và lô không còn mục nào. Gọi lúc mở + mỗi
    /// [`PURGE_INTERVAL_MS`].
    pub fn purge_finished(&mut self, now: UnixMs) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let n = tx.execute(
            "DELETE FROM items WHERE status IN ('done', 'skipped', 'cancelled') AND finished_ms < ?1",
            [now - KEEP_FINISHED_MS],
        )?;
        tx.execute("DELETE FROM batches WHERE NOT EXISTS (SELECT 1 FROM items WHERE items.batch_id = batches.id)", [])?;
        tx.commit()?;
        Ok(n)
    }

    /// Bỏ mục khỏi danh sách (người dùng xoá dòng "Đã gửi" / mục Failed). Không đụng điện thoại.
    pub fn remove(&mut self, id: ItemId) -> Result<bool> {
        let tx = self.conn.transaction()?;
        let n = tx.execute("DELETE FROM items WHERE id = ?1", [id])?;
        tx.execute("DELETE FROM batches WHERE NOT EXISTS (SELECT 1 FROM items WHERE items.batch_id = batches.id)", [])?;
        tx.commit()?;
        Ok(n > 0)
    }

    /// Quên điện thoại (gd1_06, người dùng đã đồng ý xoá hàng đợi ở UI): bỏ mọi mục + lô của thiết bị. Gọi sau khi
    /// dừng runner của nó (mục đang bay không còn ai ghi). Trả số mục đã xoá.
    pub fn remove_device(&mut self, device: &DeviceId) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let n = tx.execute("DELETE FROM items WHERE device_id = ?1", [&device.0])?;
        tx.execute("DELETE FROM batches WHERE device_id = ?1", [&device.0])?;
        tx.commit()?;
        Ok(n)
    }
}

// ---------------------------------------------------------------- hàng ↔ kiểu

fn path_to_db(p: &Path) -> String {
    // Đường dẫn không phải UTF-8 (hiếm, chỉ Linux) — lưu lossy; tệp đó sẽ báo "SourceMissing" thay vì gửi nhầm.
    p.to_string_lossy().into_owned()
}

fn json_err(e: serde_json::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
}

fn upload_from_row(r: &Row<'_>) -> rusqlite::Result<UploadItem> {
    let state: String = r.get(8)?;
    Ok(UploadItem {
        id: r.get(0)?,
        device_id: DeviceId(r.get(1)?),
        batch_id: r.get::<_, Option<String>>(2)?.map(BatchId),
        path: PathBuf::from(r.get::<_, String>(3)?),
        name: r.get(4)?,
        dir: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        size: r.get::<_, i64>(6)? as u64,
        mtime: r.get(7)?,
        state: serde_json::from_str(&state).map_err(json_err)?,
        failures: r.get(9)?,
        created: r.get(10)?,
        updated: r.get(11)?,
    })
}

fn download_from_row(r: &Row<'_>) -> rusqlite::Result<DownloadItem> {
    let state: String = r.get(7)?;
    Ok(DownloadItem {
        id: r.get(0)?,
        device_id: DeviceId(r.get(1)?),
        remote_id: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        name: r.get(3)?,
        remote_dir: r.get(4)?,
        size: r.get::<_, i64>(5)? as u64,
        dest: PathBuf::from(r.get::<_, String>(6)?),
        state: serde_json::from_str(&state).map_err(json_err)?,
        failures: r.get(8)?,
        created: r.get(9)?,
        updated: r.get(10)?,
    })
}

fn query_uploads<P: rusqlite::Params>(conn: &Connection, filter: &str, p: P) -> Result<Vec<UploadItem>> {
    let order = if filter.contains("ORDER BY") { "" } else { " ORDER BY id" };
    let sql = format!("SELECT {UPLOAD_COLS} FROM items WHERE kind = 'upload' AND {filter}{order}");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(p, upload_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn query_downloads<P: rusqlite::Params>(conn: &Connection, filter: &str, p: P) -> Result<Vec<DownloadItem>> {
    let order = if filter.contains("ORDER BY") { "" } else { " ORDER BY id" };
    let sql = format!("SELECT {DOWNLOAD_COLS} FROM items WHERE kind = 'download' AND {filter}{order}");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(p, download_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn save_upload(tx: &Transaction<'_>, id: ItemId, size: u64, m: &UploadMachine, now: UnixMs) -> Result<()> {
    let wake = match m.state {
        UploadState::Waiting { until, .. } => Some(until),
        _ => None,
    };
    let finished = m.state.is_terminal().then_some(now);
    save_row(
        tx,
        id,
        m.state.status(),
        &serde_json::to_string(&m.state)?,
        m.failures,
        m.state.bytes_done(size),
        wake,
        finished,
        now,
    )
}

fn save_download(tx: &Transaction<'_>, id: ItemId, size: u64, m: &DownloadMachine, now: UnixMs) -> Result<()> {
    let wake = match m.state {
        DownloadState::Waiting { until, .. } => Some(until),
        _ => None,
    };
    let finished = m.state.is_terminal().then_some(now);
    save_row(
        tx,
        id,
        m.state.status(),
        &serde_json::to_string(&m.state)?,
        m.failures,
        m.state.bytes_done(size),
        wake,
        finished,
        now,
    )
}

#[allow(clippy::too_many_arguments)]
fn save_row(
    tx: &Transaction<'_>,
    id: ItemId,
    status: &str,
    state: &str,
    failures: u32,
    done_bytes: u64,
    wake: Option<UnixMs>,
    finished: Option<UnixMs>,
    now: UnixMs,
) -> Result<()> {
    tx.execute(
        "UPDATE items SET status = ?2, state = ?3, failures = ?4, done_bytes = ?5, wake_ms = ?6,
                          finished_ms = ?7, updated_ms = ?8
         WHERE id = ?1",
        params![id, status, state, failures, done_bytes as i64, wake, finished, now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::batch::{NewUpload, plan_batch};
    use crate::queue::state::{PauseReason, Resume};

    const NOW: UnixMs = 1_800_000_000_000;

    fn dev(s: &str) -> DeviceId {
        DeviceId(s.into())
    }

    fn plan(n: usize, dir: &str) -> BatchPlan {
        BatchPlan {
            dest_dir: dir.into(),
            files: (0..n)
                .map(|i| NewUpload {
                    path: PathBuf::from(format!("/videos/{i}.mp4")),
                    name: format!("{i}.mp4"),
                    dir: dir.into(),
                    size: 100,
                    mtime: Some(NOW - 5),
                })
                .collect(),
            ..BatchPlan::default()
        }
    }

    fn open(dir: &tempfile::TempDir, now: UnixMs) -> QueueStore {
        QueueStore::open(&dir.path().join(DB_FILE), now).unwrap()
    }

    fn integrity_ok(path: &Path) {
        let conn = Connection::open(path).unwrap();
        let ok: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
        assert_eq!(ok, "ok");
    }

    #[test]
    fn migrates_and_uses_wal() {
        let tmp = tempfile::tempdir().unwrap();
        let store = open(&tmp, NOW);
        assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len() as i64);
        let mode: String = store.conn.pragma_query_value(None, "journal_mode", |r| r.get(0)).unwrap();
        assert_eq!(mode, "wal");
        drop(store);
        // Mở lại không chạy lại migration.
        assert_eq!(open(&tmp, NOW).schema_version().unwrap(), MIGRATIONS.len() as i64);
    }

    #[test]
    fn refuses_newer_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(DB_FILE);
        Connection::open(&path).unwrap().pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(QueueStore::open(&path, NOW), Err(StoreError::NewerSchema(99))));
    }

    /// C2 — 1000 mục, đóng / mở lại: đủ mục, đúng trạng thái + offset; `Sending` thành `Waiting` (cần `HEAD`).
    #[test]
    fn thousand_items_survive_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = open(&tmp, NOW);
        let (_, ids) = store.add_batch(&dev("p1"), &plan(1000, "Phim"), NOW).unwrap();
        assert_eq!(ids.len(), 1000);
        let ev = |s: &mut QueueStore, id, e: UploadEvent| s.apply_upload(id, &e, NOW).unwrap().unwrap();
        // Mục 0: đang gửi ở offset 64. Mục 1: xong. Mục 2: hết chỗ. Mục 3: đang chờ backoff.
        ev(&mut store, ids[0], UploadEvent::Start);
        ev(&mut store, ids[0], UploadEvent::Checked { exists: false });
        assert_eq!(
            ev(&mut store, ids[0], UploadEvent::Created { uid: "u0".into() }),
            UploadEffect::Patch { uid: "u0".into(), offset: 0 }
        );
        ev(&mut store, ids[0], UploadEvent::ChunkAccepted { offset: 64 });
        ev(&mut store, ids[1], UploadEvent::Start);
        ev(&mut store, ids[1], UploadEvent::Checked { exists: false });
        ev(&mut store, ids[1], UploadEvent::Created { uid: "u1".into() });
        ev(
            &mut store,
            ids[1],
            UploadEvent::Committed { remote_name: "1 (1).mp4".into(), remote_dir: Some("Phim".into()) },
        );
        ev(&mut store, ids[2], UploadEvent::Start);
        ev(&mut store, ids[2], UploadEvent::Checked { exists: false });
        ev(&mut store, ids[2], UploadEvent::InsufficientStorage { short_by: Some(7) });
        ev(&mut store, ids[3], UploadEvent::Start);
        ev(&mut store, ids[3], UploadEvent::Transient { retry_after: None });
        drop(store);

        let later = NOW + 10_000;
        let store = open(&tmp, later);
        let all = store.uploads().unwrap();
        assert_eq!(all.len(), 1000);
        assert_eq!(
            all[0].state,
            UploadState::Waiting { attempt: 0, until: later, resume: Resume::Head { uid: "u0".into(), offset: 64 } }
        );
        assert_eq!(
            all[1].state,
            UploadState::Done { remote_name: "1 (1).mp4".into(), remote_dir: Some("Phim".into()) }
        );
        assert_eq!(
            all[2].state,
            UploadState::Paused { reason: PauseReason::NoSpace { short_by: Some(7) }, resume: Resume::Create }
        );
        assert!(
            matches!(all[3].state, UploadState::Waiting { attempt: 1, until, resume: Resume::Check } if until == NOW + 2_000)
        );
        assert_eq!(all[3].failures, 1);
        assert!(all[4..].iter().all(|i| i.state == UploadState::Queued && i.dir == "Phim" && i.mtime == Some(NOW - 5)));
        // Đầu hàng = mục 0, đánh thức ⇒ HEAD.
        let head = store.next_upload(&dev("p1"), later).unwrap().unwrap();
        assert_eq!(head.id, ids[0]);
        assert_eq!(head.machine().next(&UploadEvent::WakeUp, later).1, UploadEffect::Head { uid: "u0".into() });
        integrity_ok(&tmp.path().join(DB_FILE));
    }

    /// C-W3 — lô 3 tệp trong 2 thư mục con: `dir` đúng, tổng tiến độ đúng sau khởi động lại.
    #[test]
    fn batch_dirs_and_progress_survive_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("Chuyến đi");
        for (rel, n) in [("Ngày 1/a.mp4", 10), ("Ngày 1/b.mp4", 20), ("Ngày 2/c.mkv", 30)] {
            let p = src.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![0u8; n]).unwrap();
        }
        let plan = plan_batch(std::slice::from_ref(&src), "Phim").unwrap();
        let mut store = open(&tmp, NOW);
        let (batch, ids) = store.add_batch(&dev("p1"), &plan, NOW).unwrap();
        let ev = |s: &mut QueueStore, id, e: UploadEvent| s.apply_upload(id, &e, NOW).unwrap().unwrap();
        // a xong; b gửi được 5 byte; c đã có trên điện thoại.
        for e in [
            UploadEvent::Start,
            UploadEvent::Checked { exists: false },
            UploadEvent::Created { uid: "ua".into() },
            UploadEvent::Committed {
                remote_name: "a.mp4".into(), remote_dir: Some("Phim/Chuyến đi/Ngày 1".into())
            },
        ] {
            ev(&mut store, ids[0], e);
        }
        for e in [
            UploadEvent::Start,
            UploadEvent::Checked { exists: false },
            UploadEvent::Created { uid: "ub".into() },
            UploadEvent::ChunkAccepted { offset: 5 },
        ] {
            ev(&mut store, ids[1], e);
        }
        ev(&mut store, ids[2], UploadEvent::Start);
        ev(&mut store, ids[2], UploadEvent::Checked { exists: true });
        drop(store);

        let store = open(&tmp, NOW + 1);
        let items = store.batch_uploads(&batch).unwrap();
        let dirs: Vec<_> = items.iter().map(|i| (i.name.as_str(), i.dir.as_str())).collect();
        assert_eq!(
            dirs,
            [
                ("a.mp4", "Phim/Chuyến đi/Ngày 1"),
                ("b.mp4", "Phim/Chuyến đi/Ngày 1"),
                ("c.mkv", "Phim/Chuyến đi/Ngày 2")
            ]
        );
        let p = store.batch_progress(&batch).unwrap();
        assert_eq!(
            p,
            BatchProgress {
                files: 3,
                finished_files: 2,
                failed_files: 0,
                paused_files: 0,
                cancelled_files: 0,
                total_bytes: 60,
                done_bytes: 10 + 5 + 30,
            }
        );
        assert!(!p.is_settled());
        let b = &store.batches().unwrap()[0];
        assert_eq!((b.label.as_deref(), b.dest_dir.as_str()), (Some("Chuyến đi"), "Phim"));
    }

    #[test]
    fn one_queue_per_device_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = open(&tmp, NOW);
        let (_, a) = store.add_batch(&dev("p1"), &plan(2, ""), NOW).unwrap();
        let (_, b) = store.add_batch(&dev("p2"), &plan(1, ""), NOW).unwrap();
        assert_eq!(store.devices_with_work().unwrap(), [dev("p1"), dev("p2")]);
        assert_eq!(store.next_upload(&dev("p1"), NOW).unwrap().unwrap().id, a[0]);
        assert_eq!(store.next_upload(&dev("p2"), NOW).unwrap().unwrap().id, b[0]);
        // Đầu hàng đang chờ backoff ⇒ không ai chen lên; runner ngủ tới `next_wake`.
        store.apply_upload(a[0], &UploadEvent::Start, NOW).unwrap();
        store.apply_upload(a[0], &UploadEvent::Transient { retry_after: None }, NOW).unwrap();
        assert_eq!(store.next_upload(&dev("p1"), NOW + 1).unwrap(), None);
        assert_eq!(store.next_wake().unwrap(), Some(NOW + 2_000));
        assert_eq!(store.next_upload(&dev("p1"), NOW + 2_000).unwrap().unwrap().id, a[0]);
        // Tạm dừng đầu hàng ⇒ mục kế tiếp lên.
        store.apply_upload(a[0], &UploadEvent::Pause, NOW).unwrap();
        assert_eq!(store.next_wake().unwrap(), None);
        assert_eq!(store.next_upload(&dev("p1"), NOW).unwrap().unwrap().id, a[1]);
    }

    #[test]
    fn purges_finished_after_seven_days() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = open(&tmp, NOW);
        let (batch, ids) = store.add_batch(&dev("p1"), &plan(2, ""), NOW).unwrap();
        store.apply_upload(ids[0], &UploadEvent::Start, NOW).unwrap();
        store.apply_upload(ids[0], &UploadEvent::Checked { exists: true }, NOW).unwrap();
        store.apply_upload(ids[1], &UploadEvent::SourceMissing, NOW).unwrap();
        assert_eq!(store.purge_finished(NOW + KEEP_FINISHED_MS - 1).unwrap(), 0);
        drop(store);
        // Mở lại sau 7 ngày ⇒ mục Skipped bị dọn, mục Failed giữ (người dùng còn phải xử lý).
        let mut store = open(&tmp, NOW + KEEP_FINISHED_MS + 1);
        let left = store.uploads().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, ids[1]);
        assert_eq!(store.batches().unwrap().len(), 1);
        assert!(store.remove(ids[1]).unwrap());
        assert!(store.batches().unwrap().is_empty());
        assert_eq!(store.batch_progress(&batch).unwrap(), BatchProgress::default());
    }

    #[test]
    fn batch_cancel_returns_delete_effects() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = open(&tmp, NOW);
        let (batch, ids) = store.add_batch(&dev("p1"), &plan(2, ""), NOW).unwrap();
        for e in [UploadEvent::Start, UploadEvent::Checked { exists: false }, UploadEvent::Created { uid: "u".into() }]
        {
            store.apply_upload(ids[0], &e, NOW).unwrap();
        }
        let out = store.apply_batch(&batch, &UploadEvent::Cancel, NOW).unwrap();
        assert_eq!(out, [(ids[0], UploadEffect::Delete { uid: "u".into() }), (ids[1], UploadEffect::None)]);
        let p = store.batch_progress(&batch).unwrap();
        assert_eq!((p.cancelled_files, p.total_bytes), (2, 0));
        assert!(p.is_settled());
    }

    #[test]
    fn retry_after_source_change_takes_new_size() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("a.mp4");
        std::fs::write(&f, [1u8; 4]).unwrap();
        let plan = plan_batch(std::slice::from_ref(&f), "").unwrap();
        let mut store = open(&tmp, NOW);
        let (_, ids) = store.add_batch(&dev("p1"), &plan, NOW).unwrap();
        std::fs::write(&f, [1u8; 9]).unwrap();
        let item = store.upload(ids[0]).unwrap().unwrap();
        assert!(!batch::source_matches(&item.path, item.size, item.mtime).unwrap());
        store.apply_upload(ids[0], &UploadEvent::SourceChanged, NOW).unwrap();
        store.retry_upload(ids[0], NOW).unwrap();
        let item = store.upload(ids[0]).unwrap().unwrap();
        assert_eq!((item.state, item.size), (UploadState::Queued, 9));
        assert!(batch::source_matches(&item.path, item.size, item.mtime).unwrap());
    }

    #[test]
    fn downloads_persist_and_restore() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = open(&tmp, NOW);
        let d = NewDownload {
            remote_id: "r/1".into(),
            name: "x.mp4".into(),
            remote_dir: None,
            size: 50,
            dest: tmp.path().join("x.mp4"),
        };
        let id = store.add_download(&dev("p1"), &d, NOW).unwrap();
        assert_eq!(store.next_download(&dev("p1"), NOW).unwrap().unwrap().id, id);
        store.apply_download(id, &DownloadEvent::Start, NOW).unwrap();
        store.apply_download(id, &DownloadEvent::Responded { etag: Some("e".into()), full: false }, NOW).unwrap();
        store.apply_download(id, &DownloadEvent::Progress { offset: 20 }, NOW).unwrap();
        drop(store);
        let store = open(&tmp, NOW + 5);
        let item = store.download(id).unwrap().unwrap();
        assert_eq!(
            item.state,
            DownloadState::Waiting { attempt: 0, until: NOW + 5, offset: 20, etag: Some("e".into()) }
        );
        assert_eq!((item.remote_id.as_str(), item.remote_dir), ("r/1", None));
        // Upload và download không lẫn nhau.
        assert!(store.uploads().unwrap().is_empty());
    }

    #[test]
    fn remove_device_drops_only_that_device() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = open(&tmp, NOW);
        store.add_batch(&dev("p1"), &plan(3, "a"), NOW).unwrap();
        store.add_batch(&dev("p2"), &plan(2, "b"), NOW).unwrap();
        assert_eq!(store.remove_device(&dev("p1")).unwrap(), 3);
        assert_eq!(store.uploads().unwrap().len(), 2);
        assert!(store.batches().unwrap().iter().all(|b| b.device_id == dev("p2")), "batches of p1 are gone");
        assert!(store.next_upload(&dev("p1"), NOW).unwrap().is_none());
    }

    /// C3 — tiến trình con ghi liên tục, bị giết giữa chừng ⇒ DB không hỏng, mở lại được, mục đã commit còn đủ.
    #[test]
    fn killed_writer_leaves_db_intact() {
        const CHILD_VAR: &str = "FERRY_QUEUE_CRASH_CHILD";
        if let Some(path) = std::env::var_os(CHILD_VAR) {
            // Vai tiến trình con: ghi tới khi bị giết.
            let mut store = QueueStore::open(Path::new(&path), NOW).unwrap();
            let mut offset = 0u64;
            loop {
                let (_, ids) = store.add_batch(&dev("p1"), &plan(20, "a/b"), NOW).unwrap();
                for id in ids {
                    store.apply_upload(id, &UploadEvent::Start, NOW).unwrap();
                    store.apply_upload(id, &UploadEvent::Checked { exists: false }, NOW).unwrap();
                    store.apply_upload(id, &UploadEvent::Created { uid: format!("u{id}") }, NOW).unwrap();
                    offset += 1;
                    store.apply_upload(id, &UploadEvent::ChunkAccepted { offset }, NOW).unwrap();
                }
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(DB_FILE);
        let exe = std::env::current_exe().unwrap();
        for round in 0..3 {
            let mut child = std::process::Command::new(&exe)
                .args([
                    "--exact",
                    "queue::store::tests::killed_writer_leaves_db_intact",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(CHILD_VAR, &path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            // Chờ con ghi được ít nhất vài trăm mục mới, rồi giết (SIGKILL / TerminateProcess) — không cho dọn dẹp.
            let before = count(&path);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while count(&path) < before + 200 + round * 37 {
                assert!(std::time::Instant::now() < deadline, "child wrote nothing");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            child.kill().unwrap();
            child.wait().unwrap();
            integrity_ok(&path);
            let store = QueueStore::open(&path, NOW + 1).unwrap();
            let items = store.uploads().unwrap();
            assert!(items.len() as u64 >= before + 200);
            // Mỗi lô ghi trong một transaction ⇒ đủ 20 mục hoặc không có gì.
            assert_eq!(items.len() % 20, 0);
            assert!(items.iter().all(|i| !i.state.is_active()), "restore left in-flight items");
        }
    }

    fn count(path: &Path) -> u64 {
        let Ok(conn) = Connection::open(path) else { return 0 };
        conn.busy_timeout(std::time::Duration::from_secs(1)).ok();
        conn.query_row("SELECT COUNT(*) FROM items", [], |r| r.get::<_, i64>(0)).map_or(0, |n| n as u64)
    }
}
