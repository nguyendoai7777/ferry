//! Vòng `NSApplication` trên luồng chính, kiểu `Accessory` (không icon Dock) — bê từ fastcast. Đánh thức bằng hàng đợi
//! chính của libdispatch: hàng đợi này chạy cả lúc menu khay đang mở, nên sự kiện không bị giữ lại.
//!
//! `tray-icon` đòi luồng chính và vòng đã chạy trước khi tạo khay ⇒ `init` chạy trong khối đầu tiên của hàng đợi chính,
//! sau khi `run` đã vào vòng.

use std::cell::RefCell;
use std::sync::mpsc::{self, Receiver, Sender};

use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::NSPoint;

use crate::daemon::{Daemon, Event};

#[derive(Clone)]
pub struct Waker {
    tx: Sender<Event>,
}

impl Waker {
    pub fn send(&self, event: Event) {
        if self.tx.send(event).is_ok() {
            DispatchQueue::main().exec_async(drain);
        }
    }
}

struct LoopState {
    daemon: Daemon,
    rx: Receiver<Event>,
}

thread_local! {
    static STATE: RefCell<Option<LoopState>> = const { RefCell::new(None) };
    static START: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    static FAILURE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// `init` phải `'static` vì nó chờ trong `START` tới khi vòng chạy.
pub fn run_event_loop(init: impl FnOnce(Waker) -> Result<Daemon, String> + 'static) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("the daemon event loop must run on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let (tx, rx) = mpsc::channel();
    let waker = Waker { tx };
    let start = move || {
        super::install_event_handlers(&waker);
        match init(waker) {
            Ok(daemon) => {
                STATE.with(|s| *s.borrow_mut() = Some(LoopState { daemon, rx }));
                drain();
            }
            Err(e) => {
                FAILURE.with(|f| *f.borrow_mut() = Some(e));
                stop(mtm);
            }
        }
    };
    START.with(|s| *s.borrow_mut() = Some(Box::new(start)));
    DispatchQueue::main().exec_async(|| {
        if let Some(start) = START.with(|s| s.borrow_mut().take()) {
            autoreleasepool(|_| start());
        }
    });

    app.run();

    if let Some(mut state) = STATE.with(|s| s.borrow_mut().take()) {
        state.daemon.shutdown();
    }
    FAILURE.with(|f| f.borrow_mut().take()).map_or(Ok(()), Err)
}

/// Xử lý hết sự kiện đang chờ. Bị gọi lồng ⇒ bỏ qua, lượt ngoài sẽ lấy nốt.
fn drain() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    STATE.with(|s| {
        let Ok(mut guard) = s.try_borrow_mut() else { return };
        let Some(state) = guard.as_mut() else { return };
        while let Ok(event) = state.rx.try_recv() {
            if !autoreleasepool(|_| state.daemon.handle(event)) {
                stop(mtm);
                break;
            }
        }
    });
}

/// `stop` chỉ có hiệu lực sau khi vòng xử lý xong một sự kiện; đang ở khối dispatch thì chưa có sự kiện nào ⇒ tự gửi một
/// cái, không thì `run` không trả về tới khi người dùng chạm chuột.
fn stop(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    app.stop(None);
    let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        0,
        0,
        0,
    );
    if let Some(event) = event {
        app.postEvent_atStart(&event, true);
    }
}
