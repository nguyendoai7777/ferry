//! Tường lửa chặn dò mDNS (W-6): Ubuntu bật `ufw` có thể chặn chiều vào UDP 5353 ⇒ không thấy điện thoại dù HTTP thông.
//! Lúc chạy daemon đoán (không cần root) rồi để màn "Thêm điện thoại" xin quyền qua `pkexec` (hộp mật khẩu của hệ thống);
//! bản cài `.deb` thêm cùng hai luật ở postinst (`packaging/linux/build-deb.sh`). Luật mang comment [`RULE_COMMENT`] để gỡ gói
//! xoá đúng nó. Không tắt tường lửa, không đổi luật nào khác, không bao giờ tự nhập mật khẩu.
//!
//! Hàm thuần (đọc `ufw.conf`, dựng câu lệnh, đổi mã thoát, tiêu chí gợi ý) có test; [`allow_with_prompt`] chạy `pkexec` thật.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Cổng mDNS (`PROTOCOL.md` 6.2).
pub const MDNS_PORT: u16 = 5353;
/// Nhóm multicast mDNS IPv4 + IPv6 — chỉ mở chiều vào tới hai địa chỉ này (W-6, không gắn tên interface).
pub const MDNS_GROUPS: [&str; 2] = ["224.0.0.251", "ff02::fb"];
/// Comment của luật `ufw` — postrm của `.deb` tìm theo chuỗi này (giữ khớp `packaging/linux/build-deb.sh`).
pub const RULE_COMMENT: &str = "ferry mDNS";
/// Cấu hình `ufw` (đọc được không cần root, `-rw-r--r--`).
pub const UFW_CONF: &str = "/etc/ufw/ufw.conf";
/// Postinst của `.deb` ghi tệp này khi đã thêm luật; postrm xoá.
pub const SYSTEM_MARKER: &str = "/var/lib/ferry/ufw-mdns-allowed";
/// Daemon ghi tệp này (cạnh `config.json`) sau khi người dùng cho phép trong app.
pub const USER_MARKER: &str = "mdns-firewall-allowed";
/// Nơi `ufw` thường nằm; `pkexec` chạy với PATH an toàn nên ghi đường đầy đủ.
const UFW_PATHS: [&str; 2] = ["/usr/sbin/ufw", "/sbin/ufw"];
const PKEXEC_PATHS: [&str; 2] = ["/usr/bin/pkexec", "/bin/pkexec"];

/// Gợi ý trong ảnh chụp (`Snapshot::mdns_firewall`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MdnsFirewallHint {
    /// Có `pkexec` ⇒ nút "Cho phép"; không ⇒ chỉ hiện [`Self::command`] để chép.
    pub can_prompt: bool,
    /// Câu lệnh chạy tay — phương án cuối (không `pkexec`, không có agent polkit).
    pub command: String,
}

/// Kết quả lệnh "Cho phép" (`Outcome::Firewall`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AllowResult {
    /// Đã thêm luật; daemon dò lại mDNS.
    Allowed,
    /// Người dùng đóng hộp xác nhận / sai mật khẩu.
    Cancelled,
    /// Không hỏi được (không `pkexec`, không agent polkit, không phải Linux) ⇒ UI hiện câu lệnh.
    NoPrompt,
    /// `ufw` báo lỗi — chi tiết ở log.
    Failed,
}

/// `ufw.conf` có `ENABLED=yes` (cho phép nháy kép / đơn, khoảng trắng).
pub fn ufw_enabled(conf: &str) -> bool {
    conf.lines().any(|line| {
        let line = line.trim();
        let Some(value) = line.strip_prefix("ENABLED") else { return false };
        let Some(value) = value.trim_start().strip_prefix('=') else { return false };
        value.trim().trim_matches(|c| c == '"' || c == '\'').eq_ignore_ascii_case("yes")
    })
}

/// Đối số `ufw` cho một nhóm: `allow in proto udp to <nhóm> port 5353 comment 'ferry mDNS'`.
pub fn ufw_rule_args(group: &str) -> Vec<String> {
    let port = MDNS_PORT.to_string();
    ["allow", "in", "proto", "udp", "to", group, "port", &port, "comment", RULE_COMMENT]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn shell_rule(ufw: &str, group: &str) -> String {
    let args = ufw_rule_args(group);
    let (comment, rest) = args.split_last().expect("rule has args");
    format!("{ufw} {} '{comment}'", rest.join(" "))
}

/// Kịch bản `sh -c` chạy dưới `pkexec`: hai luật trong một lần xác nhận (`pkexec` hỏi mỗi lần gọi). Luật IPv4 quyết định
/// kết quả; luật IPv6 lỗi (IPv6 tắt trong `/etc/default/ufw`) thì bỏ qua — giống postinst của `.deb`.
pub fn allow_script(ufw: &str) -> String {
    let [v4, v6] = MDNS_GROUPS;
    format!("{} && {{ {} || true; }}", shell_rule(ufw, v4), shell_rule(ufw, v6))
}

/// Câu lệnh để người dùng chép khi không hỏi được quyền.
pub fn manual_command() -> String {
    MDNS_GROUPS.iter().map(|g| shell_rule("sudo ufw", g)).collect::<Vec<_>>().join(" && ")
}

/// Mã thoát của `pkexec sh -c …` ⇒ kết quả. `pkexec`: 126 = người dùng đóng hộp / không được phép, 127 = không xác thực được
/// (không có agent polkit); mã khác ≠ 0 là của `ufw`.
pub fn result_from_exit(code: Option<i32>) -> AllowResult {
    match code {
        Some(0) => AllowResult::Allowed,
        Some(126) => AllowResult::Cancelled,
        Some(127) => AllowResult::NoPrompt,
        _ => AllowResult::Failed,
    }
}

/// Có xin quyền không (chủ dự án 11/11/2026: vào màn Thêm điện thoại là kiểm ngay, chưa có quyền thì hỏi luôn): tường lửa có
/// thể chặn, đang dò mDNS, và chưa thấy điện thoại nào qua mDNS (đã thấy ⇒ mDNS qua được, không cần hỏi).
pub fn hint_needed(may_block: bool, browsing: bool, found_any: bool) -> bool {
    may_block && browsing && !found_any
}

/// Tường lửa của máy, đoán không cần root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detected {
    /// `ufw` đang bật và chưa có dấu đã cho phép ⇒ có thể chặn mDNS.
    pub may_block: bool,
    /// Đường `ufw` (để chạy dưới `pkexec`).
    pub ufw: Option<PathBuf>,
    pub can_prompt: bool,
}

impl Detected {
    pub const NONE: Detected = Detected { may_block: false, ufw: None, can_prompt: false };

    pub fn hint(&self) -> MdnsFirewallHint {
        MdnsFirewallHint { can_prompt: self.can_prompt, command: manual_command() }
    }
}

/// Đoán trên máy này. `user_marker` = tệp [`USER_MARKER`] của người dùng. Ngoài Linux luôn [`Detected::NONE`] (Windows: bản
/// cài ghi luật; macOS: tường lửa ứng dụng tự hỏi).
pub fn detect(user_marker: &Path) -> Detected {
    if !cfg!(target_os = "linux") {
        return Detected::NONE;
    }
    let Some(ufw) = UFW_PATHS.iter().map(Path::new).find(|p| p.exists()) else { return Detected::NONE };
    let enabled = std::fs::read_to_string(UFW_CONF).is_ok_and(|c| ufw_enabled(&c));
    let allowed = user_marker.exists() || Path::new(SYSTEM_MARKER).exists();
    let can_prompt = PKEXEC_PATHS.iter().any(|p| Path::new(p).exists());
    Detected { may_block: enabled && !allowed, ufw: Some(ufw.to_owned()), can_prompt }
}

/// Chạy `pkexec /bin/sh -c "<ufw> allow …"` và chờ người dùng xác nhận (chặn — gọi trong `spawn_blocking`). Hệ thống tự
/// hiện hộp mật khẩu; app không chạm mật khẩu.
pub fn allow_with_prompt(detected: &Detected) -> AllowResult {
    let Some(ufw) = detected.ufw.as_ref().filter(|_| detected.can_prompt && cfg!(target_os = "linux")) else {
        return AllowResult::NoPrompt;
    };
    let Some(pkexec) = PKEXEC_PATHS.iter().find(|p| Path::new(p).exists()) else { return AllowResult::NoPrompt };
    let script = allow_script(&ufw.to_string_lossy());
    log::info!("firewall: asking to allow mDNS via pkexec");
    match std::process::Command::new(pkexec).arg("/bin/sh").arg("-c").arg(&script).output() {
        Ok(out) => {
            let result = result_from_exit(out.status.code());
            let log_out = |s: &[u8]| String::from_utf8_lossy(s).trim().to_owned();
            log::info!(
                "firewall: pkexec exit {:?} ⇒ {result:?}; stdout: {}; stderr: {}",
                out.status.code(),
                log_out(&out.stdout),
                log_out(&out.stderr)
            );
            result
        }
        Err(e) => {
            log::warn!("firewall: cannot run pkexec: {e}");
            AllowResult::NoPrompt
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ufw_enabled() {
        assert!(ufw_enabled("# comment\nENABLED=yes\nLOGLEVEL=low\n"));
        assert!(ufw_enabled("ENABLED = \"yes\""));
        assert!(ufw_enabled("  ENABLED='YES'"));
        assert!(!ufw_enabled("ENABLED=no\n"));
        assert!(!ufw_enabled("#ENABLED=yes\n"));
        assert!(!ufw_enabled("ENABLED_X=yes\n"));
        assert!(!ufw_enabled(""));
    }

    #[test]
    fn rules_open_only_mdns_groups_with_comment() {
        assert_eq!(
            ufw_rule_args("224.0.0.251").join(" "),
            "allow in proto udp to 224.0.0.251 port 5353 comment ferry mDNS"
        );
        let script = allow_script("/usr/sbin/ufw");
        assert_eq!(
            script,
            "/usr/sbin/ufw allow in proto udp to 224.0.0.251 port 5353 comment 'ferry mDNS' && \
             { /usr/sbin/ufw allow in proto udp to ff02::fb port 5353 comment 'ferry mDNS' || true; }"
        );
        // Không tắt tường lửa, không đụng luật khác.
        for bad in ["disable", "reset", "delete", "default", "--force"] {
            assert!(!script.split_whitespace().any(|w| w == bad), "{bad} in {script}");
        }
        assert!(manual_command().starts_with("sudo ufw allow in proto udp to 224.0.0.251"));
        assert!(manual_command().contains("&& sudo ufw allow in proto udp to ff02::fb port 5353"));
    }

    #[test]
    fn deb_scripts_use_the_same_comment() {
        let deb = include_str!("../../../packaging/linux/build-deb.sh");
        assert!(deb.contains(&format!("comment='{RULE_COMMENT}'")), "build-deb.sh phải dùng comment {RULE_COMMENT:?}");
        assert!(deb.contains(SYSTEM_MARKER));
    }

    #[test]
    fn pkexec_exit_codes() {
        assert_eq!(result_from_exit(Some(0)), AllowResult::Allowed);
        assert_eq!(result_from_exit(Some(126)), AllowResult::Cancelled);
        assert_eq!(result_from_exit(Some(127)), AllowResult::NoPrompt);
        assert_eq!(result_from_exit(Some(1)), AllowResult::Failed);
        assert_eq!(result_from_exit(None), AllowResult::Failed);
    }

    #[test]
    fn asks_right_away_unless_mdns_already_works() {
        assert!(hint_needed(true, true, false));
        assert!(!hint_needed(true, true, true), "đã thấy điện thoại qua mDNS ⇒ không chặn");
        assert!(!hint_needed(false, true, false), "tường lửa không chặn / đã cho phép");
        assert!(!hint_needed(true, false, false), "không dò mDNS");
    }
}
