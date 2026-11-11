//! Linux: chưa có khay. `tray-icon` đòi GTK + libappindicator (nặng, LGPL) nên sẽ bê bản StatusNotifierItem tự viết của
//! fastcast (`fastcast/crates/daemon/src/platform/linux/sni.rs`) — nợ của gd1_07. Tới lúc đó daemon chạy không icon:
//! mở cửa sổ bằng cách chạy `ferry` lần nữa (IPC `Focus`). Vòng ở đây chỉ là kênh chặn.

use std::sync::mpsc::{self, Sender};

use crate::daemon::{Daemon, Event};

#[derive(Clone)]
pub struct Waker {
    tx: Sender<Event>,
}

impl Waker {
    pub fn send(&self, event: Event) {
        let _ = self.tx.send(event);
    }
}

pub fn run_event_loop(init: impl FnOnce(Waker) -> Result<Daemon, String>) -> Result<(), String> {
    log::warn!("tray: not available on Linux yet; run `ferry` again to open the window");
    let (tx, rx) = mpsc::channel();
    let mut daemon = init(Waker { tx })?;
    while let Ok(event) = rx.recv() {
        if !daemon.handle(event) {
            break;
        }
    }
    daemon.shutdown();
    Ok(())
}
