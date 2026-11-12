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
        #[cfg(target_os = "macos")]
        macos::ensure_application();
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

/// `mac-notification-sys` chưa được khai app gửi thì tự dò bằng AppleScript `get id of application "use_default"` —
/// không có app tên đó ⇒ macOS bật hộp "Choose Application — Where is use_default?" mỗi lần (gặp 12/11/2026 với bản
/// dev). Khai trước một lần: `Ferry.app` dùng mã bundle của chính nó, exe chưa đóng gói (`start.sh ferry`) dùng Finder
/// như mặc định của thư viện — không AppleScript nào chạy.
#[cfg(target_os = "macos")]
mod macos {
    use std::sync::Once;

    use objc2_foundation::NSBundle;

    const FALLBACK: &str = "com.apple.Finder";

    pub fn ensure_application() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let own = NSBundle::mainBundle().bundleIdentifier().map(|s| s.to_string());
            let id = own.unwrap_or_else(|| FALLBACK.to_owned());
            // Thư viện chỉ nhận lần khai đầu (`Once`), hỏng cũng không dò lại bằng AppleScript — chỉ log.
            if let Err(e) = notify_rust::set_application(&id) {
                log::warn!("notify: set application {id}: {e}");
            }
        });
    }
}
