//! Linux: khay StatusNotifierItem tự viết (`linux/sni.rs`, bê từ fastcast — `tray-icon` đòi GTK + libappindicator). Vòng
//! của luồng chính chỉ là kênh chặn; D-Bus của khay chạy trên runtime tokio của engine. Không có host khay (GNOME tắt
//! extension) ⇒ mở cửa sổ bằng cách chạy `ferry` lần nữa (IPC `Focus`).

mod sni;

use std::sync::mpsc::{self, Sender};

pub use sni::Tray;

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
