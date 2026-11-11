//! Mã riêng macOS của cửa sổ chính (bê cách làm từ cửa sổ Cài đặt của fastcast, `fastcast/crates/ui/src/platform/macos.rs`):
//! cỡ đèn giao thông của hệ thống để căn chúng giữa thanh trên, bảng "Giới thiệu" chuẩn của AppKit, và icon Dock của bản dev.

#[cfg(debug_assertions)]
use objc2::AllocAnyThread;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion, NSAboutPanelOptionVersion, NSApplication,
    NSImage, NSWindow, NSWindowButton, NSWindowStyleMask,
};
use objc2_foundation::{NSData, NSDictionary, NSString};

/// Cạnh nút đèn giao thông, điểm logic: macOS 26 là 14, bản trước 12 ⇒ hỏi AppKit lúc chạy.
pub fn caption_button_size() -> f32 {
    let Some(mtm) = MainThreadMarker::new() else { return 12.0 };
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    NSWindow::standardWindowButton_forStyleMask(NSWindowButton::CloseButton, style, mtm)
        .map_or(12.0, |button| button.frame().size.height as f32)
}

/// Bảng "Giới thiệu" của AppKit. Truyền tên + phiên bản tường minh: exe trần (bản dev, ngoài `Ferry.app`) không có
/// `Info.plist` nên AppKit sẽ lấy tên tiến trình `ferry-ui`.
pub fn about_panel(name: &str, version: &str) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: ba khoá là hằng `NSString` của AppKit, giá trị là `NSString`; gọi trên luồng chính.
    unsafe {
        let name = NSString::from_str(name);
        let version = NSString::from_str(version);
        let empty = NSString::from_str("");
        let keys = [NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion, NSAboutPanelOptionVersion];
        let values: [&AnyObject; 3] = [&name, &version, &empty];
        let options: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(&keys, &values);
        app.orderFrontStandardAboutPanelWithOptions(&options);
    }
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
}

/// Icon Dock cho exe trần (bản dev, ngoài `Ferry.app`): không có bundle nên AppKit không đọc được `CFBundleIconFile` và
/// Dock hiện icon "exec". Bản release chạy trong `Ferry.app` lấy `ferry.icns` từ `Resources/` ⇒ chỉ nhúng ở bản debug.
#[cfg(debug_assertions)]
pub fn set_dev_dock_icon() {
    static ICNS: &[u8] = include_bytes!("../../../../packaging/macos/ferry.icns");
    let Some(mtm) = MainThreadMarker::new() else { return };
    let data = NSData::with_bytes(ICNS);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        log::warn!("ferry.icns: NSImage cannot decode the embedded icon");
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: gọi trên luồng chính (có `MainThreadMarker`), ảnh là `NSImage` hợp lệ.
    unsafe { app.setApplicationIconImage(Some(&image)) };
}
