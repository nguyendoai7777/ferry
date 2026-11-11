//! Bật / theo dõi tiến trình cửa sổ `mws-ui` (exe nằm cạnh `mws`). Đóng cửa sổ ⇒ `mws-ui` thoát, nhả GPU / RAM; truyền
//! vẫn chạy ở `mws` (`CLAUDE.md` mục Bố cục). Mỗi lúc tối đa một `mws-ui` do daemon bật.

use std::io;
use std::path::PathBuf;
use std::process::Command;

use mws_core::ipc::{DaemonToUi, Sender};

use crate::daemon::Event;
use crate::platform::Waker;

#[derive(Default)]
pub struct UiProcess {
    /// Đã bật mà chưa thoát (kể cả lúc chưa kịp nối IPC).
    running: Option<u32>,
    /// Kết nối IPC của UI đang mở: `(id kết nối, đầu ghi)`.
    conn: Option<(u64, Sender<DaemonToUi>)>,
}

impl UiProcess {
    /// "Mở cửa sổ": UI đang nối ⇒ bảo nó lên trước; đang bật dở ⇒ chờ; chưa có ⇒ bật.
    pub fn open(&mut self, waker: &Waker) {
        if let Some((id, sender)) = &self.conn {
            match sender.send(&DaemonToUi::Focus) {
                Ok(()) => return,
                Err(e) => {
                    log::warn!("ui: #{id} focus failed: {e}");
                    self.conn = None;
                }
            }
        }
        if let Some(pid) = self.running {
            log::info!("ui: pid {pid} is still starting");
            return;
        }
        match spawn(waker.clone()) {
            Ok(pid) => {
                log::info!("ui: started pid {pid}");
                self.running = Some(pid);
            }
            Err(e) => log::error!("ui: cannot start mws-ui: {e}"),
        }
    }

    pub fn connected(&mut self, id: u64, sender: Sender<DaemonToUi>) {
        self.conn = Some((id, sender));
    }

    pub fn disconnected(&mut self, id: u64) {
        if self.conn.as_ref().is_some_and(|(c, _)| *c == id) {
            self.conn = None;
        }
    }

    pub fn exited(&mut self, pid: u32) {
        if self.running == Some(pid) {
            self.running = None;
        }
    }
}

/// Đường dẫn `mws-ui` cạnh exe đang chạy.
fn ui_exe() -> io::Result<PathBuf> {
    Ok(std::env::current_exe()?.with_file_name(format!("mws-ui{}", std::env::consts::EXE_SUFFIX)))
}

/// Bật `mws-ui`; một luồng chờ nó thoát (gặt tiến trình, không để zombie) rồi báo [`Event::UiExited`].
fn spawn(waker: Waker) -> io::Result<u32> {
    let mut child = Command::new(ui_exe()?).spawn()?;
    let pid = child.id();
    std::thread::Builder::new().name("ui-wait".into()).spawn(move || {
        match child.wait() {
            Ok(status) => log::info!("ui: pid {pid} exited ({status})"),
            Err(e) => log::warn!("ui: wait for pid {pid} failed: {e}"),
        }
        waker.send(Event::UiExited { pid });
    })?;
    Ok(pid)
}
