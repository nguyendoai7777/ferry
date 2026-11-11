//! Thông báo hệ thống (PROTOCOL 2.6: một cái khi cả lô xong; lỗi người dùng phải làm báo một lần). `notify-rust`:
//! Linux nói thẳng D-Bus `org.freedesktop.Notifications` qua `zbus` thuần Rust (không `libdbus`), Windows toast WinRT,
//! macOS `NSUserNotification`. Gọi chặn (Linux mở kết nối D-Bus mỗi lần) ⇒ một luồng ngắn mỗi thông báo — hiếm, không
//! giữ luồng nền.

/// Chữ đã dịch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

pub fn show(notice: Notice, app_name: &str) {
    let app_name = app_name.to_owned();
    let spawned = std::thread::Builder::new().name("notify".into()).spawn(move || {
        let mut n = notify_rust::Notification::new();
        n.appname(&app_name).summary(&notice.title).body(&notice.body);
        // Không đọc kết quả: thiếu dịch vụ thông báo (máy chủ không phiên đồ hoạ) thì chỉ log.
        if let Err(e) = n.show() {
            log::warn!("notify: {e}");
        }
    });
    if let Err(e) = spawned {
        log::warn!("notify: cannot spawn thread: {e}");
    }
}
