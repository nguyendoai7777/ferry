//! Trạng thái của daemon trên luồng chính: mọi nguồn (khay, IPC, tiến trình UI, engine) đổi thành [`Event`] rồi xử lý
//! tuần tự ở [`Daemon::handle`] — không mutex chia sẻ với luồng khác. Hàng đợi, thiết bị, mạng sống ở engine (runtime
//! tokio, `engine.rs`); luồng chính chỉ giữ khay + tiến trình cửa sổ.

use ferry_core::i18n::I18n;

use ferry_core::ipc::UpdateOp;

use crate::engine::{EngineHandle, Input};
use crate::ipc_server::Outbox;
use crate::platform::Waker;
use crate::tray::{self, MenuAction};
use crate::tray_state::TrayStatus;
use crate::ui_process::UiProcess;
use crate::update::{self, Updater};

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
    /// ferry-gd2_01: kết quả của luồng soát / tải / cài bản mới.
    Update(update::Msg),
    /// ferry-gd2_01: lệnh cập nhật của cửa sổ (`UiToDaemon::Update`).
    UpdateOp(UpdateOp),
    /// Ctrl+C ở console (Windows, bản debug).
    #[cfg_attr(not(windows), allow(dead_code))]
    Quit,
}

pub struct Daemon {
    waker: Waker,
    i18n: I18n,
    engine: EngineHandle,
    ui: UiProcess,
    updater: Updater,
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
        let updater = Updater::start(Some(waker.clone()));
        Ok(Daemon { waker, i18n, engine, ui: UiProcess::default(), updater, tray })
    }

    /// `false` ⇒ thoát vòng sự kiện.
    pub fn handle(&mut self, event: Event) -> bool {
        match event {
            Event::Menu(id) => match tray::menu_action(&id) {
                Some(MenuAction::OpenWindow) => self.ui.open(&self.waker),
                Some(MenuAction::Quit) => return false,
                Some(MenuAction::Update) => {
                    let effect = self.updater.install();
                    return self.apply_update(effect);
                }
                None => log::debug!("tray: unknown menu id {id}"),
            },
            Event::TrayLeftClick | Event::OpenWindow => self.ui.open(&self.waker),
            Event::UiConnected { id, outbox } => self.ui.connected(id, outbox),
            Event::UiDisconnected { id } => self.ui.disconnected(id),
            Event::UiExited { pid } => self.ui.exited(pid),
            Event::Tray(status) => {
                // Cài đặt đổi ngôn ngữ: engine đã đổi chữ trong `status`, menu khay theo cùng ngôn ngữ.
                let language_changed = status.language != self.i18n.language();
                if language_changed {
                    self.i18n = I18n::new(status.language);
                }
                self.tray.apply(&status, &self.i18n);
                if language_changed {
                    self.tray.set_update(self.updater.version(), &self.i18n);
                }
            }
            Event::Update(msg) => {
                let effect = self.updater.handle(msg);
                return self.apply_update(effect);
            }
            Event::UpdateOp(op) => {
                let effect = match op {
                    UpdateOp::Check => self.updater.check(true),
                    UpdateOp::Install => self.updater.install(),
                };
                return self.apply_update(effect);
            }
            Event::Quit => return false,
        }
        true
    }

    /// Trạng thái cập nhật đổi ⇒ ảnh chụp cho cửa sổ + mục khay; cài xong ⇒ `false` (thoát để bản mới bật).
    fn apply_update(&mut self, effect: update::Effect) -> bool {
        if effect.changed {
            self.engine.send(Input::Update(self.updater.view().clone()));
            self.tray.set_update(self.updater.version(), &self.i18n);
        }
        !effect.quit
    }

    /// Trước khi thoát: dừng runner (request đang bay bị cắt, mở lại thì nối tiếp). Cửa sổ `ferry-ui` thấy IPC đóng (EOF)
    /// thì tự thoát theo.
    pub fn shutdown(&mut self) {
        log::info!("daemon: shutting down");
        self.engine.shutdown();
    }
}
