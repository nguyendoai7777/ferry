//! macOS, bản debug: nhúng một `Info.plist` tối thiểu vào exe trần (`__TEXT,__info_plist`) để AppKit gọi app là "Ferry"
//! trên thanh menu thay vì tên tiến trình `ferry-ui` (bản dev chạy ngoài `Ferry.app`). Bản đóng gói lấy `CFBundleName` từ
//! `packaging/macos/Info.plist` của bundle nên bản release không nhúng (tránh lệch với plist của bundle khi ký).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
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
