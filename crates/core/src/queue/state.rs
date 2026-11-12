//! Máy trạng thái mục upload / download (`PROTOCOL.md` 5) dưới dạng hàm thuần: `(trạng thái, sự kiện, giờ) ⇒ (trạng
//! thái mới, effect)`. Không gọi mạng, không đọc đĩa: effect chỉ mô tả việc phải làm, runner (gd1_04 / gd1_10) thực
//! thi rồi báo kết quả bằng sự kiện kế tiếp. Sự kiện không hợp với trạng thái hiện tại ⇒ giữ nguyên, effect `None`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{UnixMs, backoff};

// ---------------------------------------------------------------- upload

/// Chỗ chạy tiếp sau `Waiting` / `Paused`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Resume {
    /// Chưa có phiên upload, chưa hỏi `list` (Checking).
    Check,
    /// Chưa có phiên upload, đã qua Checking (Creating).
    Create,
    /// Đã có phiên upload: `HEAD` lấy offset thật rồi gửi tiếp.
    Head { uid: String, offset: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum PauseReason {
    /// `507`: điện thoại hết chỗ. `shortBy` = số byte thiếu khi runner biết (từ `info`).
    NoSpace { short_by: Option<u64> },
    /// `401`: điện thoại đã "Quên máy tính" — ghép lại rồi bấm thử lại.
    NeedsPairing,
    /// Người dùng bấm tạm dừng.
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailReason {
    /// `415`: không phải đuôi video.
    Unsupported,
    /// `413`: khúc vượt `Upload-Length`.
    TooLarge,
    /// `400`: `X-File-Dir` / header sai (§6.9).
    BadRequest,
    /// Cỡ / mtime của tệp nguồn khác lúc thả vào.
    SourceChanged,
    /// Tệp nguồn không còn.
    SourceMissing,
    /// `410`: người dùng từ chối lô trên điện thoại (§6.5, §6.10) — cả lô dừng, không tự thử lại.
    Declined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum UploadState {
    Queued,
    Checking,
    Creating,
    Sending {
        uid: String,
        offset: u64,
    },
    Waiting {
        attempt: u32,
        until: UnixMs,
        resume: Resume,
    },
    Paused {
        reason: PauseReason,
        resume: Resume,
    },
    /// `remoteName` / `remoteDir`: tên + `dir` thật trên điện thoại (có thể khác tên gửi).
    Done {
        remote_name: String,
        remote_dir: Option<String>,
    },
    /// Điện thoại đã có tệp trùng `dir` + tên + cỡ.
    Skipped,
    Failed {
        reason: FailReason,
    },
    Cancelled,
}

impl UploadState {
    /// Tên biến thể (cột `status` của store, khoá i18n phía UI).
    pub fn status(&self) -> &'static str {
        match self {
            UploadState::Queued => "queued",
            UploadState::Checking => "checking",
            UploadState::Creating => "creating",
            UploadState::Sending { .. } => "sending",
            UploadState::Waiting { .. } => "waiting",
            UploadState::Paused { .. } => "paused",
            UploadState::Done { .. } => "done",
            UploadState::Skipped => "skipped",
            UploadState::Failed { .. } => "failed",
            UploadState::Cancelled => "cancelled",
        }
    }

    /// Không còn việc gì nữa (chỉ chờ dọn hoặc người dùng thử lại).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            UploadState::Done { .. } | UploadState::Skipped | UploadState::Failed { .. } | UploadState::Cancelled
        )
    }

    /// Đang có request bay (Checking / Creating / Sending).
    pub fn is_active(&self) -> bool {
        matches!(self, UploadState::Checking | UploadState::Creating | UploadState::Sending { .. })
    }

    /// Phiên upload đang mở trên điện thoại (nếu có).
    pub fn uid(&self) -> Option<&str> {
        match self {
            UploadState::Sending { uid, .. }
            | UploadState::Waiting { resume: Resume::Head { uid, .. }, .. }
            | UploadState::Paused { resume: Resume::Head { uid, .. }, .. } => Some(uid),
            _ => None,
        }
    }

    /// Số byte đã tới điện thoại, để tính tiến độ (mục + lô).
    pub fn bytes_done(&self, size: u64) -> u64 {
        match self {
            UploadState::Sending { offset, .. }
            | UploadState::Waiting { resume: Resume::Head { offset, .. }, .. }
            | UploadState::Paused { resume: Resume::Head { offset, .. }, .. } => (*offset).min(size),
            UploadState::Done { .. } | UploadState::Skipped => size,
            _ => 0,
        }
    }

    fn resume_point(&self) -> Resume {
        match self {
            UploadState::Sending { uid, offset } => Resume::Head { uid: uid.clone(), offset: *offset },
            UploadState::Creating => Resume::Create,
            UploadState::Waiting { resume, .. } | UploadState::Paused { resume, .. } => resume.clone(),
            _ => Resume::Check,
        }
    }
}

/// Kết quả runner báo về (mỗi biến thể = một mũi tên của PROTOCOL 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadEvent {
    /// Runner nhận mục ở đầu hàng: Queued ⇒ Checking.
    Start,
    /// `GET /list` xong: `exists` = trùng `dir` + tên + cỡ.
    Checked {
        exists: bool,
    },
    /// `POST /uploads` ⇒ `201` + `uid`.
    Created {
        uid: String,
    },
    /// `PATCH` ⇒ `204` + `Upload-Offset`.
    ChunkAccepted {
        offset: u64,
    },
    /// `PATCH` khúc cuối ⇒ `201` + mục thư viện.
    Committed {
        remote_name: String,
        remote_dir: Option<String>,
    },
    /// `PATCH` ⇒ `409`: offset lệch, hỏi lại bằng `HEAD`.
    OffsetConflict,
    /// `HEAD` ⇒ `200` + `Upload-Offset`.
    OffsetReported {
        offset: u64,
    },
    /// `PATCH` ⇒ `423`: đang có PATCH khác — chờ rồi `HEAD`.
    Locked {
        retry_after: Option<Duration>,
    },
    /// Lỗi mạng / `5xx` / `503`.
    Transient {
        retry_after: Option<Duration>,
    },
    /// `PATCH` / `HEAD` ⇒ `404`: phiên upload đã mất, tạo lại từ 0.
    UploadGone,
    /// `415` / `413` / `400`: không thử lại.
    Rejected(FailReason),
    /// `507`.
    InsufficientStorage {
        short_by: Option<u64>,
    },
    /// `401`.
    Unauthorized,
    /// Cỡ / mtime nguồn khác lúc thả vào.
    SourceChanged,
    /// Tệp nguồn không còn.
    SourceMissing,
    /// `410 declined` (§6.10): mục đang bay và mọi mục chưa xong của lô ⇒ Failed, không HEAD / PATCH / DELETE thêm.
    Declined,
    /// Đồng hồ `Waiting.until` đã tới.
    WakeUp,
    /// Người dùng: tạm dừng / chạy tiếp (cả "thử lại" sau Failed) / huỷ.
    Pause,
    Resume,
    Cancel,
    /// Store mở lại sau khi app tắt: request đang bay coi như đứt.
    Restore,
}

/// Việc runner phải làm sau chuyển trạng thái.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadEffect {
    None,
    /// `GET /list`, so `dir` + tên + cỡ.
    CheckRemote,
    /// `POST /uploads`.
    Create,
    /// `PATCH` một khúc từ `offset`.
    Patch {
        uid: String,
        offset: u64,
    },
    /// `HEAD` lấy offset thật.
    Head {
        uid: String,
    },
    /// `DELETE` phiên upload (huỷ).
    Delete {
        uid: String,
    },
    /// Đặt đồng hồ tới `until` rồi gửi `WakeUp`.
    Wait {
        until: UnixMs,
    },
}

/// Máy trạng thái một mục upload: trạng thái + số lần lỗi tạm liên tiếp (cho backoff; về 0 khi có tiến triển thật).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadMachine {
    pub state: UploadState,
    pub failures: u32,
}

impl UploadMachine {
    pub fn new() -> UploadMachine {
        UploadMachine { state: UploadState::Queued, failures: 0 }
    }

    /// Một bước. Trả máy mới + effect; sự kiện không hợp lệ ⇒ giữ nguyên, `UploadEffect::None`.
    pub fn next(&self, event: &UploadEvent, now: UnixMs) -> (UploadMachine, UploadEffect) {
        use UploadEffect as E;
        use UploadEvent as Ev;
        use UploadState as S;
        let keep = || (self.clone(), E::None);
        let to = |state: S, failures: u32, effect: E| (UploadMachine { state, failures }, effect);
        let f = self.failures;
        let wait = |retry_after: Option<Duration>, resume: Resume| {
            let attempt = f.saturating_add(1);
            let until = now.saturating_add(backoff::delay(attempt, retry_after).as_millis() as UnixMs);
            to(S::Waiting { attempt, until, resume }, attempt, E::Wait { until })
        };

        if self.state.is_terminal() {
            return match (&self.state, event) {
                // "Thử lại" sau Failed: chạy lại từ đầu (Checking hỏi lại `list`).
                (S::Failed { .. }, Ev::Resume) => to(S::Queued, 0, E::None),
                _ => keep(),
            };
        }

        match (&self.state, event) {
            // Người dùng — áp cho mọi trạng thái chưa kết thúc.
            (_, Ev::Cancel) => {
                let effect = self.state.uid().map_or(E::None, |uid| E::Delete { uid: uid.to_owned() });
                to(S::Cancelled, f, effect)
            }
            (S::Paused { .. }, Ev::Pause) => keep(),
            (_, Ev::Pause) => {
                to(S::Paused { reason: PauseReason::User, resume: self.state.resume_point() }, f, E::None)
            }
            (S::Paused { resume, .. }, Ev::Resume) => match resume {
                Resume::Check => to(S::Queued, 0, E::None),
                _ => to(S::Waiting { attempt: 0, until: now, resume: resume.clone() }, 0, E::Wait { until: now }),
            },
            (_, Ev::SourceChanged) => to(S::Failed { reason: FailReason::SourceChanged }, f, self.delete_effect()),
            (_, Ev::SourceMissing) => to(S::Failed { reason: FailReason::SourceMissing }, f, self.delete_effect()),
            (_, Ev::Declined) => to(S::Failed { reason: FailReason::Declined }, f, E::None),

            // Mở lại sau khi tắt app.
            (S::Checking | S::Creating, Ev::Restore) => to(S::Queued, f, E::None),
            (S::Sending { uid, offset }, Ev::Restore) => {
                let resume = Resume::Head { uid: uid.clone(), offset: *offset };
                to(S::Waiting { attempt: f, until: now, resume }, f, E::Wait { until: now })
            }

            // Đường chính.
            (S::Queued, Ev::Start) => to(S::Checking, f, E::CheckRemote),
            (S::Checking, Ev::Checked { exists: true }) => to(S::Skipped, 0, E::None),
            (S::Checking, Ev::Checked { exists: false }) => to(S::Creating, f, E::Create),
            (S::Creating, Ev::Created { uid }) => {
                to(S::Sending { uid: uid.clone(), offset: 0 }, 0, E::Patch { uid: uid.clone(), offset: 0 })
            }
            (S::Sending { uid, .. }, Ev::ChunkAccepted { offset }) => {
                to(S::Sending { uid: uid.clone(), offset: *offset }, 0, E::Patch { uid: uid.clone(), offset: *offset })
            }
            (S::Sending { .. }, Ev::Committed { remote_name, remote_dir }) => {
                to(S::Done { remote_name: remote_name.clone(), remote_dir: remote_dir.clone() }, 0, E::None)
            }
            (S::Sending { uid, offset }, Ev::OffsetConflict) => {
                to(S::Sending { uid: uid.clone(), offset: *offset }, f, E::Head { uid: uid.clone() })
            }
            (S::Sending { uid, .. }, Ev::OffsetReported { offset }) => {
                to(S::Sending { uid: uid.clone(), offset: *offset }, f, E::Patch { uid: uid.clone(), offset: *offset })
            }
            (S::Sending { .. }, Ev::UploadGone) => to(S::Creating, f, E::Create),

            // Lỗi.
            (S::Sending { .. }, Ev::Locked { retry_after }) => wait(*retry_after, self.state.resume_point()),
            (S::Checking | S::Creating | S::Sending { .. }, Ev::Transient { retry_after }) => {
                wait(*retry_after, self.state.resume_point())
            }
            (S::Checking | S::Creating | S::Sending { .. }, Ev::Rejected(reason)) => {
                to(S::Failed { reason: *reason }, f, self.delete_effect())
            }
            (S::Checking | S::Creating | S::Sending { .. }, Ev::InsufficientStorage { short_by }) => to(
                S::Paused { reason: PauseReason::NoSpace { short_by: *short_by }, resume: self.state.resume_point() },
                f,
                E::None,
            ),
            (S::Checking | S::Creating | S::Sending { .. }, Ev::Unauthorized) => {
                to(S::Paused { reason: PauseReason::NeedsPairing, resume: self.state.resume_point() }, f, E::None)
            }

            // Hết giờ chờ.
            (S::Waiting { resume, .. }, Ev::WakeUp) => match resume {
                Resume::Check => to(S::Checking, f, E::CheckRemote),
                Resume::Create => to(S::Creating, f, E::Create),
                Resume::Head { uid, offset } => {
                    to(S::Sending { uid: uid.clone(), offset: *offset }, f, E::Head { uid: uid.clone() })
                }
            },

            _ => keep(),
        }
    }

    /// Bỏ phiên upload dở trên điện thoại khi mục kết thúc mà không xong.
    fn delete_effect(&self) -> UploadEffect {
        self.state.uid().map_or(UploadEffect::None, |uid| UploadEffect::Delete { uid: uid.to_owned() })
    }
}

impl Default for UploadMachine {
    fn default() -> UploadMachine {
        UploadMachine::new()
    }
}

// ---------------------------------------------------------------- download (PROTOCOL 6.4)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadFailReason {
    /// `404`: video không còn trên điện thoại.
    RemoteGone,
    /// Không ghi được tệp đích (đầy đĩa, quyền).
    LocalWrite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum DownloadState {
    Queued,
    Fetching {
        offset: u64,
        etag: Option<String>,
    },
    Waiting {
        attempt: u32,
        until: UnixMs,
        offset: u64,
        etag: Option<String>,
    },
    /// Người dùng bấm tạm dừng (gd1_10). Giữ `offset` + `etag` để chạy tiếp bằng `Range` + `If-Range`.
    Paused {
        offset: u64,
        etag: Option<String>,
    },
    Done,
    Failed {
        reason: DownloadFailReason,
    },
    Cancelled,
}

impl DownloadState {
    pub fn status(&self) -> &'static str {
        match self {
            DownloadState::Queued => "queued",
            DownloadState::Fetching { .. } => "fetching",
            DownloadState::Waiting { .. } => "waiting",
            DownloadState::Paused { .. } => "paused",
            DownloadState::Done => "done",
            DownloadState::Failed { .. } => "failed",
            DownloadState::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, DownloadState::Done | DownloadState::Failed { .. } | DownloadState::Cancelled)
    }

    pub fn is_active(&self) -> bool {
        matches!(self, DownloadState::Fetching { .. })
    }

    pub fn bytes_done(&self, size: u64) -> u64 {
        match self {
            DownloadState::Fetching { offset, .. }
            | DownloadState::Waiting { offset, .. }
            | DownloadState::Paused { offset, .. } => (*offset).min(size),
            DownloadState::Done => size,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadEvent {
    Start,
    /// Đã nhận header: `full` = `200` (không phải `206`) ⇒ tệp đã đổi / server bỏ `Range`, ghi lại từ 0.
    Responded {
        etag: Option<String>,
        full: bool,
    },
    /// Đã ghi tới `offset` vào `.part`.
    Progress {
        offset: u64,
    },
    Finished,
    Transient {
        retry_after: Option<Duration>,
    },
    /// `416`: khoảng không hợp lệ ⇒ tải lại từ 0.
    RangeInvalid,
    /// `404`.
    Gone,
    /// Lỗi ghi đĩa.
    WriteFailed,
    WakeUp,
    /// Thử lại sau Failed.
    Retry,
    /// Người dùng tạm dừng (gd1_10): Queued / Fetching / Waiting ⇒ Paused.
    Pause,
    /// Chạy tiếp sau Paused ⇒ Waiting tới hạn ngay (runner `WakeUp` ⇒ `Get` từ offset đã có).
    Resume,
    Cancel,
    Restore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadEffect {
    None,
    /// `GET` với `Range: bytes=<offset>-` (+ `If-Range: <etag>` khi có, `offset > 0`).
    Get {
        offset: u64,
        etag: Option<String>,
    },
    /// Cắt `.part` về 0 rồi ghi tiếp từ thân `200` đang mở.
    Truncate,
    /// Xoá `.part`.
    DiscardPartial,
    Wait {
        until: UnixMs,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadMachine {
    pub state: DownloadState,
    pub failures: u32,
}

impl DownloadMachine {
    pub fn new() -> DownloadMachine {
        DownloadMachine { state: DownloadState::Queued, failures: 0 }
    }

    pub fn next(&self, event: &DownloadEvent, now: UnixMs) -> (DownloadMachine, DownloadEffect) {
        use DownloadEffect as E;
        use DownloadEvent as Ev;
        use DownloadState as S;
        let keep = || (self.clone(), E::None);
        let to = |state: S, failures: u32, effect: E| (DownloadMachine { state, failures }, effect);
        let f = self.failures;

        match (&self.state, event) {
            (S::Failed { .. }, Ev::Retry) => to(S::Queued, 0, E::None),
            (s, _) if s.is_terminal() => keep(),
            (_, Ev::Cancel) => to(S::Cancelled, f, E::DiscardPartial),
            (S::Queued, Ev::Pause) => to(S::Paused { offset: 0, etag: None }, f, E::None),
            (S::Fetching { offset, etag } | S::Waiting { offset, etag, .. }, Ev::Pause) => {
                to(S::Paused { offset: *offset, etag: etag.clone() }, f, E::None)
            }
            (S::Paused { offset, etag }, Ev::Resume) => to(
                S::Waiting { attempt: 0, until: now, offset: *offset, etag: etag.clone() },
                0,
                E::Wait { until: now },
            ),

            (S::Queued, Ev::Start) => to(S::Fetching { offset: 0, etag: None }, f, E::Get { offset: 0, etag: None }),
            (S::Fetching { offset, .. }, Ev::Responded { etag, full }) => {
                if *full && *offset > 0 {
                    to(S::Fetching { offset: 0, etag: etag.clone() }, f, E::Truncate)
                } else {
                    to(S::Fetching { offset: *offset, etag: etag.clone() }, f, E::None)
                }
            }
            (S::Fetching { etag, .. }, Ev::Progress { offset }) => {
                to(S::Fetching { offset: *offset, etag: etag.clone() }, 0, E::None)
            }
            (S::Fetching { .. }, Ev::Finished) => to(S::Done, 0, E::None),
            (S::Fetching { .. }, Ev::RangeInvalid) => {
                to(S::Fetching { offset: 0, etag: None }, f, E::Get { offset: 0, etag: None })
            }
            (S::Fetching { .. }, Ev::Gone) => {
                to(S::Failed { reason: DownloadFailReason::RemoteGone }, f, E::DiscardPartial)
            }
            (S::Fetching { .. }, Ev::WriteFailed) => {
                to(S::Failed { reason: DownloadFailReason::LocalWrite }, f, E::None)
            }
            (S::Fetching { offset, etag }, Ev::Transient { retry_after }) => {
                let attempt = f.saturating_add(1);
                let until = now.saturating_add(backoff::delay(attempt, *retry_after).as_millis() as UnixMs);
                to(S::Waiting { attempt, until, offset: *offset, etag: etag.clone() }, attempt, E::Wait { until })
            }
            (S::Fetching { offset, etag }, Ev::Restore) => to(
                S::Waiting { attempt: f, until: now, offset: *offset, etag: etag.clone() },
                f,
                E::Wait { until: now },
            ),
            (S::Waiting { offset, etag, .. }, Ev::WakeUp) => to(
                S::Fetching { offset: *offset, etag: etag.clone() },
                f,
                E::Get { offset: *offset, etag: etag.clone() },
            ),
            _ => keep(),
        }
    }
}

impl Default for DownloadMachine {
    fn default() -> DownloadMachine {
        DownloadMachine::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use UploadEffect as E;
    use UploadEvent as Ev;
    use UploadState as S;

    const NOW: UnixMs = 1_000_000;

    fn m(state: S) -> UploadMachine {
        UploadMachine { state, failures: 0 }
    }
    fn sending(offset: u64) -> S {
        S::Sending { uid: "u1".into(), offset }
    }
    fn step(state: S, ev: Ev) -> (S, E) {
        let (n, e) = m(state).next(&ev, NOW);
        (n.state, e)
    }

    // C1 — mỗi mũi tên của PROTOCOL 5 một test.

    #[test]
    fn queued_to_checking() {
        assert_eq!(step(S::Queued, Ev::Start), (S::Checking, E::CheckRemote));
    }

    #[test]
    fn checking_duplicate_is_skipped() {
        assert_eq!(step(S::Checking, Ev::Checked { exists: true }), (S::Skipped, E::None));
    }

    #[test]
    fn checking_new_goes_creating() {
        assert_eq!(step(S::Checking, Ev::Checked { exists: false }), (S::Creating, E::Create));
    }

    #[test]
    fn created_starts_sending_at_zero() {
        assert_eq!(
            step(S::Creating, Ev::Created { uid: "u1".into() }),
            (sending(0), E::Patch { uid: "u1".into(), offset: 0 })
        );
    }

    #[test]
    fn chunk_accepted_advances_offset() {
        assert_eq!(
            step(sending(0), Ev::ChunkAccepted { offset: 32 }),
            (sending(32), E::Patch { uid: "u1".into(), offset: 32 })
        );
    }

    #[test]
    fn committed_is_done_with_remote_name() {
        let ev = Ev::Committed { remote_name: "a (1).mp4".into(), remote_dir: Some("Trip/Day 1".into()) };
        assert_eq!(
            step(sending(32), ev),
            (S::Done { remote_name: "a (1).mp4".into(), remote_dir: Some("Trip/Day 1".into()) }, E::None)
        );
    }

    #[test]
    fn transient_while_sending_waits_then_heads() {
        let (n, e) = m(sending(64)).next(&Ev::Transient { retry_after: None }, NOW);
        let resume = Resume::Head { uid: "u1".into(), offset: 64 };
        assert_eq!(n.state, S::Waiting { attempt: 1, until: NOW + 2_000, resume });
        assert_eq!(e, E::Wait { until: NOW + 2_000 });
        // Lỗi tiếp ⇒ 4 s.
        let (n2, _) = n.next(&Ev::WakeUp, NOW + 2_000);
        assert_eq!(n2.state, sending(64));
        let (n3, e3) = n2.next(&Ev::Transient { retry_after: None }, NOW + 3_000);
        assert_eq!(n3.failures, 2);
        assert_eq!(e3, E::Wait { until: NOW + 3_000 + 4_000 });
        // Có tiến triển ⇒ về 0.
        let (n4, _) = n3.next(&Ev::WakeUp, NOW + 7_000);
        let (n5, _) = n4.next(&Ev::OffsetReported { offset: 64 }, NOW + 7_000);
        let (n6, _) = n5.next(&Ev::ChunkAccepted { offset: 96 }, NOW + 8_000);
        assert_eq!(n6.failures, 0);
    }

    #[test]
    fn wakeup_sending_heads() {
        let w = S::Waiting { attempt: 1, until: NOW, resume: Resume::Head { uid: "u1".into(), offset: 64 } };
        assert_eq!(step(w, Ev::WakeUp), (sending(64), E::Head { uid: "u1".into() }));
    }

    #[test]
    fn head_reports_offset_then_patch() {
        assert_eq!(
            step(sending(64), Ev::OffsetReported { offset: 50 }),
            (sending(50), E::Patch { uid: "u1".into(), offset: 50 })
        );
    }

    #[test]
    fn transient_before_uid_resumes_same_phase() {
        let (s, _) = step(S::Checking, Ev::Transient { retry_after: None });
        assert!(matches!(&s, S::Waiting { resume: Resume::Check, .. }));
        assert_eq!(step(s, Ev::WakeUp), (S::Checking, E::CheckRemote));
        let (s, _) = step(S::Creating, Ev::Transient { retry_after: None });
        assert!(matches!(&s, S::Waiting { resume: Resume::Create, .. }));
        assert_eq!(step(s, Ev::WakeUp), (S::Creating, E::Create));
    }

    #[test]
    fn service_unavailable_honors_retry_after() {
        let (s, e) = step(S::Creating, Ev::Transient { retry_after: Some(Duration::from_secs(30)) });
        assert!(matches!(s, S::Waiting { until, .. } if until == NOW + 30_000));
        assert_eq!(e, E::Wait { until: NOW + 30_000 });
    }

    #[test]
    fn not_found_recreates_from_zero() {
        assert_eq!(step(sending(64), Ev::UploadGone), (S::Creating, E::Create));
        assert_eq!(S::Creating.bytes_done(100), 0);
    }

    #[test]
    fn conflict_asks_head() {
        assert_eq!(step(sending(64), Ev::OffsetConflict), (sending(64), E::Head { uid: "u1".into() }));
    }

    #[test]
    fn locked_waits_then_heads() {
        let (s, e) = step(sending(64), Ev::Locked { retry_after: None });
        assert!(matches!(&s, S::Waiting { resume: Resume::Head { offset: 64, .. }, .. }));
        assert!(matches!(e, E::Wait { .. }));
        assert_eq!(step(s, Ev::WakeUp), (sending(64), E::Head { uid: "u1".into() }));
    }

    #[test]
    fn unsupported_and_too_large_fail_without_retry() {
        assert_eq!(
            step(S::Creating, Ev::Rejected(FailReason::Unsupported)),
            (S::Failed { reason: FailReason::Unsupported }, E::None)
        );
        assert_eq!(
            step(sending(64), Ev::Rejected(FailReason::TooLarge)),
            (S::Failed { reason: FailReason::TooLarge }, E::Delete { uid: "u1".into() })
        );
        assert_eq!(
            step(S::Creating, Ev::Rejected(FailReason::BadRequest)),
            (S::Failed { reason: FailReason::BadRequest }, E::None)
        );
        // Kết thúc ⇒ đồng hồ không đánh thức lại.
        assert_eq!(step(S::Failed { reason: FailReason::TooLarge }, Ev::WakeUp).1, E::None);
    }

    #[test]
    fn insufficient_storage_pauses_until_user_retries() {
        let (s, e) = step(S::Creating, Ev::InsufficientStorage { short_by: Some(1_200_000_000) });
        assert_eq!(
            s,
            S::Paused { reason: PauseReason::NoSpace { short_by: Some(1_200_000_000) }, resume: Resume::Create }
        );
        assert_eq!(e, E::None);
        // WakeUp không đánh thức Paused.
        assert_eq!(step(s.clone(), Ev::WakeUp).0, s);
        // Người dùng bấm thử lại ⇒ chờ 0 s rồi tạo lại.
        let (s, e) = step(s, Ev::Resume);
        assert_eq!(s, S::Waiting { attempt: 0, until: NOW, resume: Resume::Create });
        assert_eq!(e, E::Wait { until: NOW });
    }

    #[test]
    fn unauthorized_pauses_for_pairing() {
        let (s, _) = step(sending(64), Ev::Unauthorized);
        assert!(matches!(s, S::Paused { reason: PauseReason::NeedsPairing, resume: Resume::Head { offset: 64, .. } }));
    }

    #[test]
    fn source_changed_fails_and_drops_session() {
        assert_eq!(
            step(sending(64), Ev::SourceChanged),
            (S::Failed { reason: FailReason::SourceChanged }, E::Delete { uid: "u1".into() })
        );
        assert_eq!(step(S::Queued, Ev::SourceMissing), (S::Failed { reason: FailReason::SourceMissing }, E::None));
        // §6.10: từ chối lô ⇒ Failed ở mọi trạng thái chưa xong, không DELETE phiên (điện thoại trả 410 cho mọi thứ).
        assert_eq!(step(S::Queued, Ev::Declined), (S::Failed { reason: FailReason::Declined }, E::None));
        let sending = S::Sending { uid: "u".into(), offset: 9 };
        assert_eq!(step(sending, Ev::Declined), (S::Failed { reason: FailReason::Declined }, E::None));
    }

    #[test]
    fn user_pause_resume_cancel() {
        let (s, _) = step(sending(64), Ev::Pause);
        assert!(matches!(&s, S::Paused { reason: PauseReason::User, resume: Resume::Head { offset: 64, .. } }));
        assert_eq!(s.bytes_done(100), 64);
        assert_eq!(step(S::Queued, Ev::Pause).0, S::Paused { reason: PauseReason::User, resume: Resume::Check });
        assert_eq!(
            step(S::Paused { reason: PauseReason::User, resume: Resume::Check }, Ev::Resume),
            (S::Queued, E::None)
        );
        assert_eq!(step(s, Ev::Cancel), (S::Cancelled, E::Delete { uid: "u1".into() }));
        assert_eq!(step(S::Queued, Ev::Cancel), (S::Cancelled, E::None));
        assert_eq!(step(S::Cancelled, Ev::Resume).0, S::Cancelled);
    }

    #[test]
    fn failed_retry_requeues() {
        let mut machine = m(S::Failed { reason: FailReason::SourceChanged });
        machine.failures = 4;
        let (n, _) = machine.next(&Ev::Resume, NOW);
        assert_eq!(n, UploadMachine { state: S::Queued, failures: 0 });
    }

    #[test]
    fn restore_turns_inflight_into_waiting() {
        assert_eq!(
            step(sending(64), Ev::Restore),
            (
                S::Waiting { attempt: 0, until: NOW, resume: Resume::Head { uid: "u1".into(), offset: 64 } },
                E::Wait { until: NOW }
            )
        );
        assert_eq!(step(S::Checking, Ev::Restore).0, S::Queued);
        assert_eq!(step(S::Creating, Ev::Restore).0, S::Queued);
        assert_eq!(step(S::Queued, Ev::Restore).0, S::Queued);
    }

    #[test]
    fn stray_events_are_ignored() {
        assert_eq!(step(S::Queued, Ev::ChunkAccepted { offset: 5 }), (S::Queued, E::None));
        assert_eq!(step(S::Checking, Ev::Created { uid: "x".into() }), (S::Checking, E::None));
        let done = S::Done { remote_name: "a.mp4".into(), remote_dir: None };
        assert_eq!(step(done.clone(), Ev::Cancel), (done, E::None));
    }

    #[test]
    fn state_json_is_camel_case() {
        let s = S::Waiting { attempt: 2, until: 5, resume: Resume::Head { uid: "u".into(), offset: 9 } };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(
            json,
            r#"{"status":"waiting","attempt":2,"until":5,"resume":{"phase":"head","uid":"u","offset":9}}"#
        );
        assert_eq!(serde_json::from_str::<S>(&json).unwrap(), s);
        let p = S::Paused { reason: PauseReason::NoSpace { short_by: Some(3) }, resume: Resume::Create };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains(r#""shortBy":3"#), "{json}");
        assert_eq!(serde_json::from_str::<S>(&json).unwrap(), p);
    }

    // Download.

    use DownloadEffect as DE;
    use DownloadEvent as DEv;
    use DownloadState as DS;

    fn dstep(state: DS, ev: DEv) -> (DS, DE) {
        let (n, e) = DownloadMachine { state, failures: 0 }.next(&ev, NOW);
        (n.state, e)
    }

    #[test]
    fn download_happy_path() {
        let (s, e) = dstep(DS::Queued, DEv::Start);
        assert_eq!(e, DE::Get { offset: 0, etag: None });
        let (s, _) = dstep(s, DEv::Responded { etag: Some("\"e1\"".into()), full: true });
        let (s, _) = dstep(s, DEv::Progress { offset: 10 });
        assert_eq!(s, DS::Fetching { offset: 10, etag: Some("\"e1\"".into()) });
        assert_eq!(dstep(s, DEv::Finished).0, DS::Done);
    }

    #[test]
    fn download_resumes_with_range_and_etag() {
        let f = DS::Fetching { offset: 10, etag: Some("e".into()) };
        let (s, e) = dstep(f.clone(), DEv::Transient { retry_after: None });
        assert_eq!(e, DE::Wait { until: NOW + 2_000 });
        assert_eq!(dstep(s, DEv::WakeUp), (f.clone(), DE::Get { offset: 10, etag: Some("e".into()) }));
        let (s, _) = dstep(f.clone(), DEv::Restore);
        assert!(matches!(s, DS::Waiting { offset: 10, .. }));
    }

    #[test]
    fn download_full_response_restarts_from_zero() {
        let f = DS::Fetching { offset: 10, etag: Some("old".into()) };
        assert_eq!(
            dstep(f, DEv::Responded { etag: Some("new".into()), full: true }),
            (DS::Fetching { offset: 0, etag: Some("new".into()) }, DE::Truncate)
        );
        let f = DS::Fetching { offset: 10, etag: Some("e".into()) };
        assert_eq!(
            dstep(f, DEv::RangeInvalid),
            (DS::Fetching { offset: 0, etag: None }, DE::Get { offset: 0, etag: None })
        );
    }

    #[test]
    fn download_failures_and_cancel() {
        let f = DS::Fetching { offset: 10, etag: None };
        assert_eq!(
            dstep(f.clone(), DEv::Gone),
            (DS::Failed { reason: DownloadFailReason::RemoteGone }, DE::DiscardPartial)
        );
        let (s, _) = dstep(f.clone(), DEv::WriteFailed);
        assert_eq!(dstep(s, DEv::Retry).0, DS::Queued);
        assert_eq!(dstep(f, DEv::Cancel), (DS::Cancelled, DE::DiscardPartial));
        assert_eq!(dstep(DS::Done, DEv::Cancel), (DS::Done, DE::None));
    }

    #[test]
    fn download_pause_keeps_offset_and_resumes() {
        let f = DS::Fetching { offset: 10, etag: Some("e".into()) };
        let (s, e) = dstep(f, DEv::Pause);
        assert_eq!((s.clone(), e), (DS::Paused { offset: 10, etag: Some("e".into()) }, DE::None));
        assert_eq!(s.status(), "paused");
        assert_eq!(s.bytes_done(100), 10);
        let (s, e) = dstep(s, DEv::Resume);
        assert_eq!(e, DE::Wait { until: NOW });
        assert_eq!(dstep(s, DEv::WakeUp).1, DE::Get { offset: 10, etag: Some("e".into()) });
        assert_eq!(dstep(DS::Queued, DEv::Pause).0, DS::Paused { offset: 0, etag: None });
        assert_eq!(dstep(DS::Paused { offset: 3, etag: None }, DEv::Cancel), (DS::Cancelled, DE::DiscardPartial));
    }
}
