//! ferry-gd1_14: tệp đóng gói (`packaging/`) khớp mã — tên exe, `app_id` cửa sổ, dịch vụ mDNS, tường lửa. Đọc tệp, không dựng
//! bản cài (việc đó ở `build.sh` / `build.ps1`).

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn linux_desktop_entry_matches_window_app_id_and_binaries() {
    let entry = read("packaging/linux/ferry.desktop");
    // Cửa sổ `ferry-ui` bản phát hành đặt `app_id: "ferry"` (crates/ui/src/main.rs) ⇒ tệp tên `ferry.desktop` + `StartupWMClass=ferry`
    // để dock gom đúng cửa sổ (bàn giao gd1_12).
    let ui_main = read("crates/ui/src/main.rs");
    assert!(ui_main.contains(r#"{ "ferry.dev" } else { "ferry" }"#), "app_id của cửa sổ đổi ⇒ sửa ferry.desktop");
    for line in ["StartupWMClass=ferry\n", "Exec=ferry\n", "Icon=ferry\n", "Exec=ferry-ui --settings\n"] {
        assert!(entry.contains(line), "ferry.desktop thiếu {line:?}");
    }
    // Cờ của ferry-ui phải còn (`ui_process::SETTINGS_ARG`).
    assert!(read("crates/daemon/src/ui_process.rs").contains(r#""--settings""#));
    let deb = read("packaging/linux/build-deb.sh");
    assert!(deb.contains("bins=(ferry ferry-ui)"));
    assert!(deb.contains("usr/share/applications/ferry.desktop"));
}

#[test]
fn macos_bundle_declares_the_bonjour_service_it_browses() {
    let plist = read("packaging/macos/Info.plist");
    // macOS 15 chặn mạng LAN nếu loại dịch vụ không khai trong NSBonjourServices.
    let service = ferry_core::discovery::mdns::SERVICE_TYPE.trim_end_matches(".local.");
    assert!(plist.contains(&format!("<string>{service}</string>")), "Info.plist thiếu {service}");
    assert!(plist.contains("<key>NSLocalNetworkUsageDescription</key>"));
    assert!(plist.contains("<key>CFBundleExecutable</key>\n  <string>ferry</string>"));
    assert!(read("packaging/macos/build-macos.sh").contains(r#"cp "$release/ferry" "$release/ferry-ui""#));
}

#[test]
fn windows_installer_ships_both_exes_and_opens_mdns() {
    let nsi = read("packaging/windows/ferry.nsi");
    assert!(nsi.contains(r#"File "${STAGE}\ferry.exe""#));
    assert!(nsi.contains(r#"File "${STAGE}\ferry-ui.exe""#));
    assert!(nsi.contains("protocol=UDP localport=5353"));
    assert!(nsi.contains("RequestExecutionLevel admin"), "luật tường lửa cần admin");
    // Gỡ cài xoá đúng tác vụ tự chạy `autostart::task_name` = `ferry-<user>`.
    assert!(read("crates/daemon/src/autostart.rs").contains(r#"format!("{BASE_NAME}-{user}")"#));
    assert!(nsi.contains("('ferry-' + $$n)"));
}

#[test]
fn license_page_and_deny_check_the_same_targets() {
    let block = |text: &str| -> Vec<String> {
        let start = text.find("targets = [").expect("targets");
        let end = start + text[start..].find(']').expect("]");
        let mut t: Vec<String> = text[start..end].split('"').skip(1).step_by(2).map(str::to_owned).collect();
        t.sort();
        t
    };
    assert_eq!(block(&read("packaging/licenses/about.toml")), block(&read("deny.toml")));
}
