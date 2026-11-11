//! Kênh lệnh của cửa sổ cài đặt (gd1_12): kết nối IPC **riêng**, vai `Cli` — daemon không đẩy ảnh chụp vào đây, chỉ trả
//! `Reply` cho lệnh của nó. Tách khỏi `daemon_link` (kết nối vai `Ui` + kho trạng thái của cửa sổ chính, gd1_08) để màn
//! cài đặt không phụ thuộc kho đó.
//!
//! [`SettingsClient::call`] **chặn** tới khi có trả lời (lệnh đường dẫn / `AddAddress` có thể mất vài giây): gọi trong
//! `cx.background_executor().spawn(…)`, không gọi trên luồng vẽ. Mất daemon ⇒ lần gọi sau tự nối lại một lần.

use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use ferry_core::ipc::{ClientConnection, CommandError, DaemonToUi, Outcome, PhoneOp, Role, SettingsOp, SettingsView, UiToDaemon};

/// Lệnh màn cài đặt gửi được: cài đặt + quên điện thoại.
#[derive(Debug, Clone)]
pub enum Request {
    Settings(SettingsOp),
    Phone(PhoneOp),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// Không nối được daemon `ferry` (chưa chạy / vừa thoát) ⇒ "Mở Ferry rồi thử lại".
    NoDaemon,
    /// Daemon từ chối lệnh — UI đổi thành chữ nói việc cần làm (PROTOCOL 2.6).
    Failed(CommandError),
}

#[derive(Clone, Default)]
pub struct SettingsClient(Arc<Mutex<Inner>>);

#[derive(Default)]
struct Inner {
    conn: Option<ClientConnection>,
    next_req: u64,
}

impl SettingsClient {
    pub fn new() -> SettingsClient {
        SettingsClient::default()
    }

    /// Gửi một lệnh, chờ đúng `Reply` của nó. Chặn — xem doc module.
    pub fn call(&self, request: Request) -> Result<Outcome, CallError> {
        let mut inner = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        // Kết nối cũ có thể đã chết (daemon khởi động lại) ⇒ thử lại một lần trên kết nối mới.
        for attempt in 0..2 {
            if inner.conn.is_none() {
                match ClientConnection::open(Role::Cli) {
                    Ok(c) => inner.conn = Some(c),
                    Err(e) => {
                        log::info!("settings: daemon not reachable: {e}");
                        return Err(CallError::NoDaemon);
                    }
                }
            }
            inner.next_req += 1;
            let req = inner.next_req;
            let msg = match &request {
                Request::Settings(op) => UiToDaemon::Settings { req, op: op.clone() },
                Request::Phone(op) => UiToDaemon::Phone { req, op: op.clone() },
            };
            let conn = inner.conn.as_mut().expect("connected above");
            match exchange(conn, &msg, req) {
                Ok(Outcome::Failed { error }) => return Err(CallError::Failed(error)),
                Ok(outcome) => return Ok(outcome),
                Err(e) => {
                    log::info!("settings: ipc failed (attempt {}): {e}", attempt + 1);
                    inner.conn = None;
                }
            }
        }
        Err(CallError::NoDaemon)
    }

    /// `Get` rồi lấy [`SettingsView`].
    pub fn get(&self) -> Result<SettingsView, CallError> {
        self.call(Request::Settings(SettingsOp::Get)).and_then(expect_settings)
    }
}

/// Mọi lệnh `Settings` trả `Outcome::Settings`; khác ⇒ daemon lệch bản (coi như lỗi trong).
pub fn expect_settings(outcome: Outcome) -> Result<SettingsView, CallError> {
    match outcome {
        Outcome::Settings(view) => Ok(view),
        other => {
            log::warn!("settings: unexpected outcome {other:?}");
            Err(CallError::Failed(CommandError::Internal))
        }
    }
}

fn exchange(conn: &mut ClientConnection, msg: &UiToDaemon, req: u64) -> io::Result<Outcome> {
    conn.send(msg)?;
    loop {
        match conn.recv() {
            Ok(Some(DaemonToUi::Reply { req: r, outcome })) if r == req => return Ok(outcome),
            // Trả lời của lần gọi trước đã bỏ cuộc, `Error` của dòng lạ…: đọc tiếp.
            Ok(Some(other)) => log::debug!("settings: skip {other:?}"),
            Ok(None) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "daemon closed the connection")),
            Err(e) if e.kind() == io::ErrorKind::InvalidData => log::warn!("settings: {e}"),
            Err(e) => return Err(e),
        }
    }
}
