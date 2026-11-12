//! macOS, bản debug: nhúng một `Info.plist` tối thiểu vào exe trần (`__TEXT,__info_plist`) để AppKit gọi app là "Ferry"
//! trên thanh menu thay vì tên tiến trình `ferry-ui` (bản dev chạy ngoài `Ferry.app`). Bản đóng gói lấy `CFBundleName` từ
//! `packaging/macos/Info.plist` của bundle nên bản release không nhúng (tránh lệch với plist của bundle khi ký).
//! Windows: nhúng `packaging/windows/ferry.ico` vào `ferry-ui.exe` (taskbar, Alt+Tab lấy icon của exe) — như Groovex.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_icon();
        return;
    }
    let macos = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos");
    let debug = std::env::var("PROFILE").as_deref() == Ok("debug");
    if !(macos && debug) {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let plist = out.join("Info.plist");
    let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>Ferry</string>
  <key>CFBundleDisplayName</key>
  <string>Ferry</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
"#;
    std::fs::write(&plist, body).expect("write Info.plist");
    println!("cargo:rustc-link-arg-bin=ferry-ui=-Wl,-sectcreate,__TEXT,__info_plist,{}", plist.display());
}

/// Icon của `ferry-ui.exe`: tài nguyên ICON đầu tiên của exe là icon cửa sổ trên taskbar.
fn windows_icon() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = manifest.join("../../packaging/windows/ferry.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let rc = out.join("ferry-ui.rc");
    // Đường dẫn trong .rc dùng `/`: `\\` là ký tự thoát của trình dịch tài nguyên.
    let script = format!("1 ICON \"{}\"\n", icon.to_string_lossy().replace("\\", "/"));
    std::fs::write(&rc, script).expect("write ferry-ui.rc");
    embed_resource::compile(&rc, embed_resource::NONE).manifest_optional().expect("compile the Windows resources");
}
