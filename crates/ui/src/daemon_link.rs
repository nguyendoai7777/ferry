//! Nối daemon `ferry` qua IPC. Một luồng chặn ở `recv` rồi chuyển thông điệp vào vòng GPUI qua kênh async — không chia sẻ
//! mutex với vòng vẽ. Daemon đóng kết nối (thoát từ khay) ⇒ cửa sổ thoát theo.
//!
//! Không có daemon (chạy `ferry-ui` thẳng khi dev) ⇒ vẫn mở cửa sổ, chỉ log cảnh báo; nút gửi báo chưa nối được.

use ferry_core::ipc::{ClientConnection, DaemonToUi, Role, Sender, UiToDaemon};
use futures::StreamExt;
use futures::channel::mpsc;
use materiant::gpui_kit::{AnyWindowHandle, App, AppContext, Entity};

use crate::model::Model;

pub fn connect() -> Option<ClientConnection> {
    match ClientConnection::open(Role::Ui) {
        Ok(conn) => Some(conn),
        Err(e) => {
            log::warn!("ipc: daemon not reachable, running alone: {e}");
            None
        }
    }
}

/// Đầu ghi lệnh của kết nối (cho `Model`), lấy trước khi `listen` nhận quyền sở hữu phần đọc.
pub fn sender(conn: &ClientConnection) -> Sender<UiToDaemon> {
    conn.sender()
}

enum Incoming {
    Message(DaemonToUi),
    Closed,
}

pub fn listen(mut conn: ClientConnection, model: Entity<Model>, window: AnyWindowHandle, cx: &mut App) {
    let (tx, mut rx) = mpsc::unbounded();
    let spawned = std::thread::Builder::new().name("ipc-recv".into()).spawn(move || {
        loop {
            match conn.recv() {
                Ok(Some(msg)) => {
                    if tx.unbounded_send(Incoming::Message(msg)).is_err() {
                        return;
                    }
                }
                Ok(None) => break,
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => log::warn!("ipc: {e}"),
                Err(e) => {
                    log::info!("ipc: closed: {e}");
                    break;
                }
            }
        }
        let _ = tx.unbounded_send(Incoming::Closed);
    });
    if let Err(e) = spawned {
        log::error!("ipc: cannot spawn reader thread: {e}");
        return;
    }
    cx.spawn(async move |cx| {
        while let Some(incoming) = rx.next().await {
            match incoming {
                Incoming::Message(DaemonToUi::Focus) => {
                    let _ = cx.update_window(window, |_, window, cx| {
                        cx.activate(true);
                        window.activate_window();
                    });
                }
                Incoming::Message(DaemonToUi::Error { message }) => log::warn!("ipc: daemon error: {message}"),
                Incoming::Message(msg) => {
                    model.update(cx, |model, cx| model.handle(msg, cx));
                }
                Incoming::Closed => {
                    log::info!("ipc: daemon is gone, quitting");
                    cx.update(|cx| cx.quit());
                    break;
                }
            }
        }
    })
    .detach();
}
