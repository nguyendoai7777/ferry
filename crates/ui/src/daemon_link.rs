//! Nối daemon `mws` qua IPC. Một luồng chặn ở `recv` rồi chuyển thông điệp vào vòng GPUI qua kênh async — không chia sẻ
//! mutex với vòng vẽ. Daemon đóng kết nối (thoát từ khay) ⇒ cửa sổ thoát theo.
//!
//! Không có daemon (chạy `mws-ui` thẳng khi dev) ⇒ vẫn mở cửa sổ, chỉ log cảnh báo.

use futures::StreamExt;
use futures::channel::mpsc;
use materiant::gpui_kit::{AnyWindowHandle, App, AppContext};
use mws_core::ipc::{ClientConnection, DaemonToUi, Role};

pub fn connect() -> Option<ClientConnection> {
    match ClientConnection::open(Role::Ui) {
        Ok(conn) => Some(conn),
        Err(e) => {
            log::warn!("ipc: daemon not reachable, running alone: {e}");
            None
        }
    }
}

enum Incoming {
    Message(DaemonToUi),
    Closed,
}

pub fn listen(mut conn: ClientConnection, window: AnyWindowHandle, cx: &mut App) {
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
                Incoming::Message(DaemonToUi::Ok) => {}
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
