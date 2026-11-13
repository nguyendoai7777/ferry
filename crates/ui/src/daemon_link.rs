//! Nối daemon `ferry` qua IPC. Một luồng chặn ở `recv` rồi chuyển thông điệp vào vòng GPUI qua kênh async — không chia sẻ
//! mutex với vòng vẽ. Daemon đóng kết nối (thoát từ khay) ⇒ cửa sổ thoát theo.
//!
//! Mở lại cửa sổ (12/11/2026): [`connect`] chờ tới [`FIRST_FRAME_WAIT`] cho `Restore` + `Snapshot` đầu (daemon đẩy ngay sau
//! `Hello`) rồi mới trả — `main` mở cửa sổ đã có trang, thư mục, hàng đợi lần trước ở khung đầu, không chớp màn rỗng.
//!
//! Không có daemon (chạy `ferry-ui` thẳng khi dev) ⇒ vẫn mở cửa sổ, chỉ log cảnh báo; nút gửi báo chưa nối được.

use std::time::Duration;

use ferry_core::ipc::{ClientConnection, DaemonToUi, Role, Sender, UiToDaemon};
use futures::StreamExt;
use futures::channel::mpsc;
use materiant::gpui_kit::{AnyWindowHandle, App, AppContext, Entity};

use crate::model::Model;

/// Trần chờ ảnh chụp đầu trước khi mở cửa sổ. Daemon trả trong vài ms; quá hạn (daemon bận / treo) thì mở cửa sổ như cũ,
/// dữ liệu tới sau.
const FIRST_FRAME_WAIT: Duration = Duration::from_millis(1500);

enum Incoming {
    // Hộp: `Snapshot` lớn, `Closed` không mang gì (clippy `large_enum_variant`).
    Message(Box<DaemonToUi>),
    Closed,
}

/// Kết nối đã mở + các thông điệp đầu đã nhận (`Restore`, `Snapshot`…).
pub struct Link {
    sender: Sender<UiToDaemon>,
    rx: mpsc::UnboundedReceiver<Incoming>,
    /// Thông điệp tới trước khi mở cửa sổ — `Model` áp hết trước khung đầu.
    pub initial: Vec<DaemonToUi>,
    closed: bool,
}

pub fn connect() -> Option<Link> {
    let mut conn = match ClientConnection::open(Role::Ui) {
        Ok(conn) => conn,
        Err(e) => {
            log::warn!("ipc: daemon not reachable, running alone: {e}");
            return None;
        }
    };
    let sender = conn.sender();
    let (tx, mut rx) = mpsc::unbounded();
    let (first_tx, first_rx) = std::sync::mpsc::channel::<()>();
    let spawned = std::thread::Builder::new().name("ipc-recv".into()).spawn(move || {
        let mut first = Some(first_tx);
        loop {
            match conn.recv() {
                Ok(Some(msg)) => {
                    let snapshot = matches!(msg, DaemonToUi::Snapshot(_));
                    if tx.unbounded_send(Incoming::Message(Box::new(msg))).is_err() {
                        return;
                    }
                    if snapshot && let Some(f) = first.take() {
                        let _ = f.send(());
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
        // `first` rơi ở đây ⇒ `recv_timeout` dưới thôi chờ.
        let _ = tx.unbounded_send(Incoming::Closed);
    });
    if let Err(e) = spawned {
        log::error!("ipc: cannot spawn reader thread: {e}");
        return None;
    }
    if first_rx.recv_timeout(FIRST_FRAME_WAIT).is_err() {
        log::warn!("ipc: no snapshot within {FIRST_FRAME_WAIT:?}, opening the window without it");
    }
    let (mut initial, mut closed) = (Vec::new(), false);
    while let Ok(incoming) = rx.try_recv() {
        match incoming {
            Incoming::Message(msg) => initial.push(*msg),
            Incoming::Closed => closed = true,
        }
    }
    Some(Link { sender, rx, initial, closed })
}

impl Link {
    /// Đầu ghi lệnh của kết nối (cho `Model`).
    pub fn sender(&self) -> Sender<UiToDaemon> {
        self.sender.clone()
    }
}

/// `window`: cửa sổ chính — đưa lên trước khi daemon gửi `Focus`. Thông điệp đầu (`Link::initial`) `main` đã áp.
pub fn listen(link: Link, model: Entity<Model>, window: AnyWindowHandle, cx: &mut App) {
    let Link { mut rx, closed, .. } = link;
    if closed {
        log::info!("ipc: daemon is gone, quitting");
        cx.quit();
        return;
    }
    cx.spawn(async move |cx| {
        while let Some(incoming) = rx.next().await {
            match incoming.map_msg() {
                Some(DaemonToUi::Focus) => {
                    let _ = cx.update_window(window, |_, window, cx| {
                        cx.activate(true);
                        window.activate_window();
                    });
                }
                Some(DaemonToUi::Error { message }) => log::warn!("ipc: daemon error: {message}"),
                Some(msg) => {
                    model.update(cx, |model, cx| model.handle(msg, cx));
                }
                None => {
                    log::info!("ipc: daemon is gone, quitting");
                    cx.update(|cx| cx.quit());
                    break;
                }
            }
        }
    })
    .detach();
}

impl Incoming {
    /// Mở hộp để `match` theo biến thể.
    fn map_msg(self) -> Option<DaemonToUi> {
        match self {
            Incoming::Message(msg) => Some(*msg),
            Incoming::Closed => None,
        }
    }
}
