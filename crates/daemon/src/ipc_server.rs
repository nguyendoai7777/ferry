//! Server IPC của daemon: một luồng nhận kết nối, mỗi kết nối một luồng đọc + một luồng ghi (`ipc` đồng bộ, kết nối ít
//! và sống lâu — chỉ `ferry-ui` và lần chạy `ferry` thứ hai). Luồng đọc không xử lý gì: `Focus` đổi thành [`Event`] cho
//! luồng chính, lệnh `Queue` / `Phone` / `Library` chuyển thẳng sang engine (runtime tokio). Mọi thứ gửi ra client đi qua
//! [`Outbox`] của kết nối: engine không bao giờ chặn vì client ghi chậm.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;

use ferry_core::ipc::{DaemonToUi, Role, ServerConnection, UiToDaemon};
use interprocess::local_socket::Listener;
use interprocess::local_socket::prelude::*;

use crate::daemon::Event;
use crate::engine::{EngineHandle, Input, Op};
use crate::platform::Waker;

/// Thông điệp đang chờ ghi quá chừng này ⇒ client đọc chậm: engine bỏ ảnh chụp / tiến độ (gửi bản đầy đủ khi client
/// đọc kịp), trả lời lệnh vẫn đi.
pub const BACKLOG: usize = 8;

/// Hàng ghi của một kết nối. Clone rẻ; luồng ghi thoát khi mọi bản sao bị bỏ hoặc client đóng.
#[derive(Clone)]
pub struct Outbox {
    tx: mpsc::Sender<DaemonToUi>,
    queued: Arc<AtomicUsize>,
}

impl Outbox {
    /// Hàng + đầu nhận cho luồng ghi (test dùng thẳng đầu nhận).
    pub fn new() -> (Outbox, OutboxReceiver) {
        let (tx, rx) = mpsc::channel();
        let queued = Arc::new(AtomicUsize::new(0));
        (Outbox { tx, queued: queued.clone() }, OutboxReceiver { rx, queued })
    }

    /// `false` ⇒ kết nối đã đóng.
    pub fn push(&self, msg: DaemonToUi) -> bool {
        self.queued.fetch_add(1, Ordering::Relaxed);
        let ok = self.tx.send(msg).is_ok();
        if !ok {
            self.queued.fetch_sub(1, Ordering::Relaxed);
        }
        ok
    }

    pub fn backed_up(&self) -> bool {
        self.queued.load(Ordering::Relaxed) >= BACKLOG
    }
}

pub struct OutboxReceiver {
    rx: mpsc::Receiver<DaemonToUi>,
    queued: Arc<AtomicUsize>,
}

impl OutboxReceiver {
    /// Chặn tới thông điệp kế; `None` khi mọi [`Outbox`] đã bỏ.
    pub fn recv(&self) -> Option<DaemonToUi> {
        let msg = self.rx.recv().ok()?;
        self.queued.fetch_sub(1, Ordering::Relaxed);
        Some(msg)
    }

    #[cfg(test)]
    pub fn recv_timeout(&self, d: std::time::Duration) -> Option<DaemonToUi> {
        let msg = self.rx.recv_timeout(d).ok()?;
        self.queued.fetch_sub(1, Ordering::Relaxed);
        Some(msg)
    }
}

pub fn spawn(listener: Listener, waker: Waker, engine: EngineHandle) -> io::Result<()> {
    std::thread::Builder::new().name("ipc-accept".into()).spawn(move || {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                    let (waker, engine) = (waker.clone(), engine.clone());
                    let spawned = std::thread::Builder::new()
                        .name(format!("ipc-{id}"))
                        .spawn(move || serve(id, ServerConnection::new(stream), &waker, &engine));
                    if let Err(e) = spawned {
                        log::warn!("ipc: cannot spawn connection thread: {e}");
                    }
                }
                Err(e) => log::warn!("ipc: accept failed: {e}"),
            }
        }
    })?;
    Ok(())
}

fn serve(id: u64, mut conn: ServerConnection, waker: &Waker, engine: &EngineHandle) {
    let role = match conn.recv() {
        Ok(Some(UiToDaemon::Hello { role, pid, protocol })) => {
            log::info!("ipc: #{id} hello from {role:?} pid {pid} (protocol {protocol})");
            role
        }
        Ok(Some(other)) => {
            log::warn!("ipc: #{id} first message is not hello: {other:?}");
            let _ = conn.send(&DaemonToUi::Error { message: "expected hello".into() });
            return;
        }
        Ok(None) => return,
        Err(e) => {
            log::warn!("ipc: #{id} bad hello: {e}");
            return;
        }
    };
    let (outbox, rx) = Outbox::new();
    let sender = conn.sender();
    let writer = std::thread::Builder::new().name(format!("ipc-{id}-w")).spawn(move || {
        while let Some(msg) = rx.recv() {
            if let Err(e) = sender.send(&msg) {
                log::info!("ipc: #{id} write failed: {e}");
                break;
            }
        }
    });
    if let Err(e) = writer {
        log::warn!("ipc: #{id} cannot spawn writer: {e}");
        return;
    }
    if !outbox.push(DaemonToUi::Ok) {
        return;
    }
    if role == Role::Ui {
        waker.send(Event::UiConnected { id, outbox: outbox.clone() });
        engine.send(Input::Subscribe { conn: id, outbox: outbox.clone() });
    }
    loop {
        let request = |req, op| Input::Request { outbox: outbox.clone(), req, op };
        match conn.recv() {
            Ok(Some(UiToDaemon::Focus)) => {
                waker.send(Event::OpenWindow);
                if !outbox.push(DaemonToUi::Ok) {
                    break;
                }
            }
            Ok(Some(UiToDaemon::Queue { req, op })) => engine.send(request(req, Op::Queue(op))),
            Ok(Some(UiToDaemon::Phone { req, op })) => engine.send(request(req, Op::Phone(op))),
            Ok(Some(UiToDaemon::Library { req, op })) => engine.send(request(req, Op::Library(op))),
            Ok(Some(UiToDaemon::Settings { req, op })) => engine.send(request(req, Op::Settings(op))),
            // Chỉ cửa sổ (`Ui`) giữ trạng thái; lần chạy `ferry` thứ hai không có gì để giữ.
            Ok(Some(UiToDaemon::SaveView { view })) if role == Role::Ui => engine.send(Input::SaveView(view)),
            Ok(Some(UiToDaemon::SaveView { .. })) => {}
            Ok(Some(UiToDaemon::Update { op })) => waker.send(Event::UpdateOp(op)),
            Ok(Some(UiToDaemon::Hello { .. })) => {
                outbox.push(DaemonToUi::Error { message: "already said hello".into() });
            }
            Ok(None) => break,
            // Thông điệp của bản mới hơn / dòng hỏng: báo rồi đọc tiếp.
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                log::warn!("ipc: #{id} {e}");
                if !outbox.push(DaemonToUi::Error { message: "unknown message".into() }) {
                    break;
                }
            }
            Err(e) => {
                log::info!("ipc: #{id} closed: {e}");
                break;
            }
        }
    }
    if role == Role::Ui {
        waker.send(Event::UiDisconnected { id });
        engine.send(Input::Unsubscribe { conn: id });
    }
}
