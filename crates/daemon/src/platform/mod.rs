//! Vòng sự kiện theo OS (bê từ fastcast) + nối sự kiện của `tray-icon` vào [`Event`] của daemon.
//!
//! `Waker` (Send + Clone) đẩy `Event` vào kênh rồi đánh thức luồng chính; luồng chính xử lý tuần tự bằng
//! `Daemon::handle`. Rảnh thì luồng chính ngủ trong vòng của OS, không poll (`CLAUDE.md`: "Rảnh không giữ CPU").

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{Waker, run_event_loop};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{Waker, run_event_loop};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{Waker, run_event_loop};

#[cfg(not(target_os = "linux"))]
use tray_icon::menu::MenuEvent;

#[cfg(not(target_os = "linux"))]
use crate::daemon::Event;

/// Handler của `tray-icon` chạy trên luồng vòng sự kiện, trong lúc crate đang giữ khoá — chỉ đẩy sự kiện đi, không xử lý
/// tại chỗ.
#[cfg(not(target_os = "linux"))]
pub fn install_event_handlers(waker: &Waker) {
    let w = waker.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| w.send(Event::Menu(e.id().0.clone()))));
    #[cfg(windows)]
    {
        use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};
        let w = waker.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                w.send(Event::TrayLeftClick);
            }
        }));
    }
}
