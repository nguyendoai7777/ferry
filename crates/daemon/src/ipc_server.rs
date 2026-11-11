//! Server IPC của daemon: một luồng nhận kết nối, mỗi kết nối một luồng đọc (`ipc` đồng bộ, kết nối ít và sống lâu — chỉ
//! `ferry-ui` và lần chạy `ferry` thứ hai). Luồng đọc không xử lý gì: đổi thông điệp thành [`Event`] rồi đẩy về luồng chính.

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

use ferry_core::ipc::{DaemonToUi, Role, ServerConnection, UiToDaemon};
use interprocess::local_socket::Listener;
use interprocess::local_socket::prelude::*;

use crate::daemon::Event;
use crate::platform::Waker;

pub fn spawn(listener: Listener, waker: Waker) -> io::Result<()> {
    std::thread::Builder::new().name("ipc-accept".into()).spawn(move || {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                    let waker = waker.clone();
                    let spawned = std::thread::Builder::new()
                        .name(format!("ipc-{id}"))
                        .spawn(move || serve(id, ServerConnection::new(stream), &waker));
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

fn serve(id: u64, mut conn: ServerConnection, waker: &Waker) {
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
    if conn.send(&DaemonToUi::Ok).is_err() {
        return;
    }
    if role == Role::Ui {
        waker.send(Event::UiConnected { id, sender: conn.sender() });
    }
    loop {
        match conn.recv() {
            Ok(Some(UiToDaemon::Focus)) => {
                waker.send(Event::OpenWindow);
                if conn.send(&DaemonToUi::Ok).is_err() {
                    break;
                }
            }
            Ok(Some(UiToDaemon::Hello { .. })) => {
                let _ = conn.send(&DaemonToUi::Error { message: "already said hello".into() });
            }
            Ok(None) => break,
            // Thông điệp của bản mới hơn / dòng hỏng: báo rồi đọc tiếp.
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                log::warn!("ipc: #{id} {e}");
                if conn.send(&DaemonToUi::Error { message: "unknown message".into() }).is_err() {
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
    }
}
