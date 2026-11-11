//! Bật / theo dõi tiến trình cửa sổ `ferry-ui` (exe nằm cạnh `ferry`). Đóng cửa sổ ⇒ `ferry-ui` thoát, nhả GPU / RAM; truyền
//! vẫn chạy ở `ferry` (`CLAUDE.md` mục Bố cục). Mỗi lúc tối đa một `ferry-ui` do daemon bật.

use std::io;
use std::path::PathBuf;
use std::process::Command;

use ferry_core::ipc::DaemonToUi;

use crate::daemon::Event;
use crate::ipc_server::Outbox;
use crate::platform::Waker;

#[derive(Default)]
pub struct UiProcess {
    /// Đã bật mà chưa thoát (kể cả lúc chưa kịp nối IPC).
    running: Option<u32>,
    /// Kết nối IPC của UI đang mở: `(id kết nối, đầu ghi)`.
    conn: Option<(u64, Outbox)>,
    /// "Cài đặt…" bấm lúc UI đang bật dở ⇒ gửi `OpenSettings` khi nó nối.
    settings_pending: bool,
}

impl UiProcess {
    /// "Mở cửa sổ": UI đang nối ⇒ bảo nó lên trước; đang bật dở ⇒ chờ; chưa có ⇒ bật.
    pub fn open(&mut self, waker: &Waker) {
        if let Some((id, outbox)) = &self.conn {
            if outbox.push(DaemonToUi::Focus) {
                return;
            }
            log::warn!("ui: #{id} focus failed: connection closed");
            self.conn = None;
        }
        if let Some(pid) = self.running {
            log::info!("ui: pid {pid} is still starting");
            return;
        }
        self.start(waker, false);
    }

    /// Menu khay "Cài đặt…" (gd1_12): UI đang nối ⇒ bảo nó mở cửa sổ cài đặt; chưa có ⇒ bật `ferry-ui --settings`.
    pub fn open_settings(&mut self, waker: &Waker) {
        if let Some((id, outbox)) = &self.conn {
            if outbox.push(DaemonToUi::OpenSettings) {
                return;
            }
            log::warn!("ui: #{id} open settings failed: connection closed");
            self.conn = None;
        }
        if let Some(pid) = self.running {
            log::info!("ui: pid {pid} is still starting, settings open once it connects");
            self.settings_pending = true;
            return;
        }
        self.start(waker, true);
    }

    fn start(&mut self, waker: &Waker, settings: bool) {
        match spawn(waker.clone(), settings) {
            Ok(pid) => {
                log::info!("ui: started pid {pid}");
                self.running = Some(pid);
            }
            Err(e) => log::error!("ui: cannot start ferry-ui: {e}"),
        }
    }

    pub fn connected(&mut self, id: u64, outbox: Outbox) {
        if std::mem::take(&mut self.settings_pending) {
            outbox.push(DaemonToUi::OpenSettings);
        }
        self.conn = Some((id, outbox));
    }

    pub fn disconnected(&mut self, id: u64) {
        if self.conn.as_ref().is_some_and(|(c, _)| *c == id) {
            self.conn = None;
        }
    }

    pub fn exited(&mut self, pid: u32) {
        if self.running == Some(pid) {
            self.running = None;
            self.settings_pending = false;
        }
    }
}

/// Đường dẫn `ferry-ui` cạnh exe đang chạy.
fn ui_exe() -> io::Result<PathBuf> {
    Ok(std::env::current_exe()?.with_file_name(format!("ferry-ui{}", std::env::consts::EXE_SUFFIX)))
}

/// Đối số dòng lệnh của `ferry-ui`: mở thẳng cửa sổ cài đặt thay vì cửa sổ chính.
pub const SETTINGS_ARG: &str = "--settings";

/// Bật `ferry-ui` (`settings` ⇒ kèm [`SETTINGS_ARG`]); một luồng chờ nó thoát (gặt tiến trình, không để zombie) rồi báo [`Event::UiExited`].
fn spawn(waker: Waker, settings: bool) -> io::Result<u32> {
    let mut command = Command::new(ui_exe()?);
    if settings {
        command.arg(SETTINGS_ARG);
    }
    let mut child = command.spawn()?;
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
