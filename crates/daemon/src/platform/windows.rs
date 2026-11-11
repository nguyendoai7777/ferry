//! Vòng sự kiện Win32 (bê từ fastcast). Đánh thức bằng `PostMessageW` tới một cửa sổ message-only (không dùng
//! `PostThreadMessageW`: thông điệp luồng bị vứt khi đang có vòng modal — vd. lúc menu khay đang mở).

use std::cell::RefCell;
use std::ptr::null_mut;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, Sender};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG, PostMessageW,
    PostQuitMessage, RegisterClassW, TranslateMessage, WM_APP, WNDCLASSW,
};

use crate::daemon::{Daemon, Event};

const WM_WAKE: u32 = WM_APP + 1;

#[derive(Clone)]
pub struct Waker {
    tx: Sender<Event>,
    /// HWND của cửa sổ message-only (lưu dạng số để `Waker` là Send).
    hwnd: isize,
}

impl Waker {
    pub fn send(&self, event: Event) {
        if self.tx.send(event).is_ok() {
            // SAFETY: hwnd là cửa sổ do `run_event_loop` tạo; PostMessageW vào cửa sổ đã huỷ chỉ trả lỗi.
            unsafe { PostMessageW(self.hwnd as HWND, WM_WAKE, 0, 0) };
        }
    }
}

struct LoopState {
    daemon: Daemon,
    rx: Receiver<Event>,
}

thread_local! {
    static STATE: RefCell<Option<LoopState>> = const { RefCell::new(None) };
}

static CTRL_WAKER: OnceLock<Waker> = OnceLock::new();

pub fn run_event_loop(init: impl FnOnce(Waker) -> Result<Daemon, String>) -> Result<(), String> {
    let hwnd = create_message_window()?;
    let (tx, rx) = mpsc::channel();
    let waker = Waker { tx, hwnd: hwnd as isize };
    let _ = CTRL_WAKER.set(waker.clone());
    // SAFETY: handler là hàm `extern "system"` tĩnh.
    unsafe { SetConsoleCtrlHandler(Some(on_console_ctrl), 1) };

    super::install_event_handlers(&waker);
    let daemon = init(waker)?;
    STATE.with(|s| *s.borrow_mut() = Some(LoopState { daemon, rx }));
    drain();

    // SAFETY: MSG là POD; GetMessageW / TranslateMessage / DispatchMessageW dùng đúng như mẫu vòng thông điệp Win32.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        loop {
            match GetMessageW(&mut msg, null_mut(), 0, 0) {
                0 => break,
                -1 => {
                    log::error!("GetMessageW failed: {}", std::io::Error::last_os_error());
                    break;
                }
                _ => {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }

    if let Some(mut state) = STATE.with(|s| s.borrow_mut().take()) {
        state.daemon.shutdown();
    }
    // SAFETY: hwnd do ta tạo, chưa huỷ.
    unsafe { DestroyWindow(hwnd) };
    Ok(())
}

/// Xử lý hết sự kiện đang chờ. Bị gọi lồng (handler bơm thông điệp) ⇒ bỏ qua, vòng ngoài sẽ lấy nốt.
fn drain() {
    STATE.with(|s| {
        let Ok(mut guard) = s.try_borrow_mut() else { return };
        let Some(state) = guard.as_mut() else { return };
        while let Ok(event) = state.rx.try_recv() {
            if !state.daemon.handle(event) {
                // SAFETY: gọi trên luồng vòng thông điệp.
                unsafe { PostQuitMessage(0) };
                break;
            }
        }
    });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_WAKE {
        drain();
        return 0;
    }
    // SAFETY: chuyển nguyên tham số Windows đưa cho wndproc.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Ctrl+C / đóng console khi chạy bản debug ⇒ thoát êm (gỡ icon khay) thay vì bị giết ngang.
unsafe extern "system" fn on_console_ctrl(_ctrl_type: u32) -> i32 {
    match CTRL_WAKER.get() {
        Some(w) => {
            w.send(Event::Quit);
            1
        }
        None => 0,
    }
}

fn create_message_window() -> Result<HWND, String> {
    let class: Vec<u16> = "media-wireless-sync-daemon\0".encode_utf16().collect();
    // SAFETY: chuỗi rộng kết thúc bằng NUL sống tới hết hàm; lớp cửa sổ đăng ký trùng lần hai chỉ trả lỗi (bỏ qua).
    unsafe {
        let instance = GetModuleHandleW(null_mut());
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(wndproc);
        wc.hInstance = instance;
        wc.lpszClassName = class.as_ptr();
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            class.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            instance,
            null_mut(),
        );
        if hwnd.is_null() {
            return Err(format!("CreateWindowExW failed: {}", std::io::Error::last_os_error()));
        }
        Ok(hwnd)
    }
}
