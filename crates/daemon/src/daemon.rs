//! Trạng thái của daemon trên luồng chính: mọi nguồn (khay, IPC, tiến trình UI) đổi thành [`Event`] rồi xử lý tuần tự ở
//! [`Daemon::handle`] — không mutex chia sẻ với luồng khác. gd1_07 thêm runtime tokio + hàng đợi vào đây.

use mws_core::i18n::I18n;
use mws_core::ipc::{DaemonToUi, Sender};

use crate::platform::Waker;
use crate::tray::{self, MenuAction};
use crate::ui_process::UiProcess;

pub enum Event {
    /// Mục menu khay (id của `tray`). Linux chưa có khay.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    Menu(String),
    /// Chuột trái vào icon khay (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    TrayLeftClick,
    /// IPC `Focus`: lần chạy `mws` thứ hai, hoặc lệnh dòng lệnh.
    OpenWindow,
    UiConnected {
        id: u64,
        sender: Sender<DaemonToUi>,
    },
    UiDisconnected {
        id: u64,
    },
    UiExited {
        pid: u32,
    },
    /// Ctrl+C ở console (Windows, bản debug).
    #[cfg_attr(not(windows), allow(dead_code))]
    Quit,
}

pub struct Daemon {
    waker: Waker,
    ui: UiProcess,
    #[cfg(not(target_os = "linux"))]
    _tray: tray::Tray,
}

impl Daemon {
    pub fn new(i18n: I18n, waker: Waker) -> Result<Daemon, String> {
        #[cfg(not(target_os = "linux"))]
        let tray = tray::Tray::new(&i18n).map_err(|e| format!("tray: {e}"))?;
        #[cfg(target_os = "linux")]
        let _ = i18n;
        Ok(Daemon {
            waker,
            ui: UiProcess::default(),
            #[cfg(not(target_os = "linux"))]
            _tray: tray,
        })
    }

    /// `false` ⇒ thoát vòng sự kiện.
    pub fn handle(&mut self, event: Event) -> bool {
        match event {
            Event::Menu(id) => match tray::menu_action(&id) {
                Some(MenuAction::OpenWindow) => self.ui.open(&self.waker),
                Some(MenuAction::Quit) => return false,
                None => log::debug!("tray: unknown menu id {id}"),
            },
            Event::TrayLeftClick | Event::OpenWindow => self.ui.open(&self.waker),
            Event::UiConnected { id, sender } => self.ui.connected(id, sender),
            Event::UiDisconnected { id } => self.ui.disconnected(id),
            Event::UiExited { pid } => self.ui.exited(pid),
            Event::Quit => return false,
        }
        true
    }

    /// Trước khi thoát. Cửa sổ `mws-ui` thấy IPC đóng (EOF) thì tự thoát theo.
    pub fn shutdown(&mut self) {
        log::info!("daemon: shutting down");
    }
}
