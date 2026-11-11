//! Trạng thái của daemon trên luồng chính: mọi nguồn (khay, IPC, tiến trình UI, engine) đổi thành [`Event`] rồi xử lý
//! tuần tự ở [`Daemon::handle`] — không mutex chia sẻ với luồng khác. Hàng đợi, thiết bị, mạng sống ở engine (runtime
//! tokio, `engine.rs`); luồng chính chỉ giữ khay + tiến trình cửa sổ.

use ferry_core::i18n::I18n;

use crate::engine::{EngineHandle, Input};
use crate::ipc_server::Outbox;
use crate::platform::Waker;
use crate::tray::{self, MenuAction};
use crate::tray_state::TrayStatus;
use crate::ui_process::UiProcess;

pub enum Event {
    /// Mục menu khay (id của `tray`).
    Menu(String),
    /// Chuột trái vào icon khay (Windows; Linux `Activate` của SNI).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    TrayLeftClick,
    /// IPC `Focus`: lần chạy `ferry` thứ hai, hoặc lệnh dòng lệnh.
    OpenWindow,
    UiConnected {
        id: u64,
        outbox: Outbox,
    },
    UiDisconnected {
        id: u64,
    },
    UiExited {
        pid: u32,
    },
    /// Engine tính lại trạng thái khay (chỉ khi đổi).
    Tray(TrayStatus),
    /// Ctrl+C ở console (Windows, bản debug).
    #[cfg_attr(not(windows), allow(dead_code))]
    Quit,
}

pub struct Daemon {
    waker: Waker,
    i18n: I18n,
    engine: EngineHandle,
    ui: UiProcess,
    #[cfg(target_os = "linux")]
    tray: crate::platform::Tray,
    #[cfg(not(target_os = "linux"))]
    tray: tray::Tray,
}

impl Daemon {
    pub fn new(i18n: I18n, waker: Waker, engine: EngineHandle, rt: &tokio::runtime::Handle) -> Result<Daemon, String> {
        #[cfg(target_os = "linux")]
        let tray = crate::platform::Tray::new(&i18n, waker.clone(), rt);
        #[cfg(not(target_os = "linux"))]
        let tray = {
            let _ = rt;
            tray::Tray::new(&i18n).map_err(|e| format!("tray: {e}"))?
        };
        Ok(Daemon { waker, i18n, engine, ui: UiProcess::default(), tray })
    }

    /// `false` ⇒ thoát vòng sự kiện.
    pub fn handle(&mut self, event: Event) -> bool {
        match event {
            Event::Menu(id) => match tray::menu_action(&id) {
                Some(MenuAction::OpenWindow) => self.ui.open(&self.waker),
                Some(MenuAction::TogglePause) => self.engine.send(Input::TogglePauseAll),
                Some(MenuAction::Quit) => return false,
                None => log::debug!("tray: unknown menu id {id}"),
            },
            Event::TrayLeftClick | Event::OpenWindow => self.ui.open(&self.waker),
            Event::UiConnected { id, outbox } => self.ui.connected(id, outbox),
            Event::UiDisconnected { id } => self.ui.disconnected(id),
            Event::UiExited { pid } => self.ui.exited(pid),
            Event::Tray(status) => self.tray.apply(&status, &self.i18n),
            Event::Quit => return false,
        }
        true
    }

    /// Trước khi thoát: dừng runner (request đang bay bị cắt, mở lại thì nối tiếp). Cửa sổ `ferry-ui` thấy IPC đóng (EOF)
    /// thì tự thoát theo.
    pub fn shutdown(&mut self) {
        log::info!("daemon: shutting down");
        self.engine.shutdown();
    }
}
