//! Khởi động cùng hệ thống (gd1_12, bê `fastcast/crates/daemon/src/autostart.rs`), mặc định tắt. Khoá `autostart` của
//! `config.json` là nguồn sự thật; mục đăng ký của OS kéo về khớp nó lúc daemon khởi động ([`sync_on_start`]) và mỗi
//! lần màn cài đặt đổi ([`apply`]).
//!
//! - Windows: tác vụ Task Scheduler "At log on" của chính người dùng (không cần admin), `Priority 7` (dưới thường),
//!   không giới hạn thời gian chạy. Không dùng khoá `Run`: Task Scheduler bị chặn ⇒ báo lỗi cho màn cài đặt.
//!   `tray-icon` tự thêm lại icon khi thanh tác vụ lên sau (`TaskbarCreated`) nên không cần trễ.
//! - Linux (GNOME): `~/.config/autostart/ferry.desktop`, GNOME chạy nó sau khi Shell lên.
//! - macOS: LaunchAgent `~/Library/LaunchAgents/<LABEL>.plist` (`Aqua`); trong `.app` agent gọi `open -g -a` để app đi
//!   qua LaunchServices.
//!
//! Lệnh chạy: `ferry --autostart` — đã có daemon thì thoát im lặng, không mở cửa sổ (`main.rs`).
//!
//! Bản debug (`start.sh`) dùng tên mục riêng có `.dev` (như kho khoá `ferry.dev`, W-5) để không đè mục của bản cài; daemon
//! chỉ tự đồng bộ lúc khởi động khi là bản release nằm ngoài thư mục `target/` của cargo — chạy thử không tự đăng ký exe
//! tạm. Bật / tắt từ màn cài đặt thì luôn áp.

use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub enum State {
    /// Đã đăng ký, trỏ đúng exe này, đúng nội dung bản này ghi.
    Current,
    /// Đã đăng ký nhưng exe / nội dung khác (app dời chỗ, bản cũ).
    Stale,
    Absent,
}

/// Tên mục đăng ký cơ sở: `ferry` (bản cài) / `ferry.dev` (bản debug).
const BASE_NAME: &str = if cfg!(debug_assertions) { "ferry.dev" } else { "ferry" };

/// Lúc daemon khởi động: kéo đăng ký của OS về khớp cấu hình. `Err` đã log; trả về để màn cài đặt hiện.
pub fn sync_on_start(enabled: bool) -> Result<(), String> {
    match std::env::current_exe() {
        Ok(exe) if self_managed(&exe) => sync(&exe, enabled),
        Ok(_) => {
            log::debug!("autostart: dev build, not syncing");
            Ok(())
        }
        Err(e) => {
            log::warn!("autostart: no current exe: {e}");
            Ok(())
        }
    }
}

/// Màn cài đặt đổi công tắc. `Err`: chữ lỗi để báo người dùng, đã ghi log.
pub fn apply(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| {
        log::warn!("autostart: no current exe: {e}");
        e.to_string()
    })?;
    sync(&exe, enabled)
}

fn self_managed(exe: &Path) -> bool {
    !cfg!(debug_assertions) && !exe.components().any(|c| c.as_os_str() == "target")
}

fn sync(exe: &Path, enabled: bool) -> Result<(), String> {
    let result = sync_with(
        enabled,
        sys::query(exe),
        || sys::register(exe).map(|how| log::info!("autostart: registered {} via {how}", exe.display())),
        || sys::unregister().map(|()| log::info!("autostart: removed")),
    );
    if let Err(e) = &result {
        log::warn!("autostart: {e}");
    }
    result
}

/// Tách khỏi `sys` để test không đụng Task Scheduler / thư mục autostart thật của máy.
fn sync_with(
    enabled: bool,
    state: Result<State, String>,
    register: impl FnOnce() -> Result<(), String>,
    unregister: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    match (enabled, state?) {
        (true, State::Current) | (false, State::Absent) => Ok(()),
        (true, _) => register(),
        (false, _) => unregister(),
    }
}

/// Tên tác vụ Task Scheduler: theo người dùng, vì Task Scheduler là chung cả máy.
#[cfg_attr(not(windows), allow(dead_code))]
fn task_name() -> String {
    let user: String = std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" })
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    format!("{BASE_NAME}-{user}")
}

/// Nội dung tác vụ Task Scheduler — hàm thuần, test chạy trên mọi OS.
mod task {
    #![cfg_attr(not(windows), allow(dead_code))]

    use std::path::Path;

    pub fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
    }

    /// Tác vụ Task Scheduler (lược đồ 1.2 — Windows 7 trở lên).
    pub fn task_xml(exe: &Path, user: &str) -> String {
        let exe = xml_escape(&exe.display().to_string());
        let user = xml_escape(user);
        format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Ferry: send files to your phone over WiFi (tray)</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>--autostart</Arguments>
    </Exec>
  </Actions>
</Task>
"#
        )
    }

    /// Đối chiếu tác vụ đang có (XML `schtasks /Query /XML`) với exe mong muốn. Đường dẫn Windows không phân biệt hoa
    /// thường.
    pub fn task_matches(xml: &str, exe: &Path) -> bool {
        let command = format!("<Command>{}</Command>", xml_escape(&exe.display().to_string()));
        xml.to_lowercase().contains(&command.to_lowercase()) && xml.contains("<Arguments>--autostart</Arguments>")
    }
}

/// Nội dung tệp XDG autostart — hàm thuần, test chạy trên mọi OS.
mod xdg {
    #![cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]

    use std::path::{Path, PathBuf};

    /// Tệp `.desktop` theo chuẩn XDG Autostart.
    pub fn desktop_entry(exe: &Path) -> String {
        format!(
            "[Desktop Entry]\nType=Application\nName=Ferry\nComment=Send files to your phone over WiFi\n\
             Exec={} --autostart\nTerminal=false\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
            desktop_exec_quote(&exe.display().to_string())
        )
    }

    /// Trích dẫn đối số `Exec=` theo Desktop Entry Spec: bọc `"…"`, thoát `"` `` ` `` `$` `\`, `%` thành `%%`.
    pub fn desktop_exec_quote(arg: &str) -> String {
        let mut out = String::from("\"");
        for c in arg.chars() {
            match c {
                '"' | '`' | '$' | '\\' => {
                    out.push('\\');
                    out.push(c);
                }
                '%' => out.push_str("%%"),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// `$XDG_CONFIG_HOME/autostart/<tên>.desktop` (mặc định `~/.config`).
    pub fn entry_file(config_home: &Path) -> PathBuf {
        config_home.join("autostart").join(format!("{}.desktop", super::BASE_NAME))
    }
}

/// Nội dung LaunchAgent + nhận biết `.app` — hàm thuần, test chạy trên mọi OS.
mod launchd {
    #![cfg_attr(not(target_os = "macos"), allow(dead_code))]

    use std::path::{Path, PathBuf};

    use super::task::xml_escape;

    pub const LABEL: &str = if cfg!(debug_assertions) { "com.sandtechs.ferry.dev" } else { "com.sandtechs.ferry" };

    /// Lệnh agent chạy lúc đăng nhập. Trong `.app`: `open -g -a <app> --args …` (không giành focus); exe rời: chạy thẳng.
    pub fn program_arguments(exe: &Path) -> Vec<String> {
        let mut args = match exe.ancestors().nth(3).filter(|_| in_app_bundle(exe)) {
            Some(app) => {
                vec!["/usr/bin/open".into(), "-g".into(), "-a".into(), app.display().to_string(), "--args".into()]
            }
            None => vec![exe.display().to_string()],
        };
        args.push("--autostart".into());
        args
    }

    /// `LimitLoadToSessionType = Aqua`: chỉ chạy trong phiên đồ hoạ.
    pub fn plist(exe: &Path) -> String {
        let args: String =
            program_arguments(exe).iter().map(|a| format!("    <string>{}</string>\n", xml_escape(a))).collect();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
{args}  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
</dict>
</plist>
"#
        )
    }

    pub fn agent_file(home: &Path) -> PathBuf {
        home.join("Library").join("LaunchAgents").join(format!("{LABEL}.plist"))
    }

    /// `<tên>.app/Contents/MacOS/<exe>`.
    pub fn in_app_bundle(exe: &Path) -> bool {
        let mut up = exe.ancestors().skip(1);
        let mut name = || up.next().and_then(Path::file_name);
        name().is_some_and(|n| n == "MacOS")
            && name().is_some_and(|n| n == "Contents")
            && name().is_some_and(|n| Path::new(n).extension().is_some_and(|e| e == "app"))
    }
}

/// Tệp đăng ký dạng văn bản (XDG, LaunchAgent): có ⇒ so nội dung, ghi / xoá nguyên tệp.
#[cfg(unix)]
mod file_entry {
    use std::path::Path;

    use super::State;

    pub fn query(path: &Path, want: &str) -> Result<State, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(if text == want { State::Current } else { State::Stale }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::Absent),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn write(path: &Path, text: &str) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn remove(path: &Path) -> Result<(), String> {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", path.display())),
            _ => Ok(()),
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Command, Output};

    use super::task::{task_matches, task_xml};
    use super::{State, task_name};

    /// Daemon là app GUI — gọi `schtasks` không được loé cửa sổ console.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn run(args: &[&str]) -> Result<Output, String> {
        Command::new("schtasks")
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|e| format!("cannot run schtasks: {e}"))
    }

    fn stderr(o: &Output) -> String {
        String::from_utf8_lossy(&o.stderr).trim().to_owned()
    }

    pub fn query(exe: &Path) -> Result<State, String> {
        let task = run(&["/Query", "/TN", &task_name(), "/XML"])?;
        if !task.status.success() {
            return Ok(State::Absent);
        }
        let xml = String::from_utf8_lossy(&task.stdout);
        Ok(if task_matches(&xml, exe) { State::Current } else { State::Stale })
    }

    /// Trả cách đã đăng ký.
    pub fn register(exe: &Path) -> Result<&'static str, String> {
        let name = task_name();
        let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
            (Ok(d), Ok(u)) => format!(r"{d}\{u}"),
            (_, Ok(u)) => u,
            _ => return Err("USERNAME is not set".into()),
        };
        let xml_path = std::env::temp_dir().join(format!("{name}-task.xml"));
        // schtasks đọc XML khai báo UTF-16 ⇒ ghi UTF-16 LE có BOM.
        let bytes: Vec<u8> =
            std::iter::once(0xFEFF).chain(task_xml(exe, &user).encode_utf16()).flat_map(u16::to_le_bytes).collect();
        std::fs::write(&xml_path, bytes).map_err(|e| format!("cannot write {}: {e}", xml_path.display()))?;
        let created = run(&["/Create", "/TN", &name, "/XML", &xml_path.to_string_lossy(), "/F"]);
        let _ = std::fs::remove_file(&xml_path);
        let o = created?;
        if !o.status.success() {
            return Err(format!("Task Scheduler refused: {}", stderr(&o)));
        }
        Ok("Task Scheduler")
    }

    pub fn unregister() -> Result<(), String> {
        let task = run(&["/Delete", "/TN", &task_name(), "/F"])?;
        // Xoá thất bại vì vốn không có gì để xoá thì vẫn là thành công.
        if task.status.success() || matches!(query(Path::new("")), Ok(State::Absent)) {
            Ok(())
        } else {
            Err(format!("cannot remove the log-on task: {}", stderr(&task)))
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod sys {
    use std::path::{Path, PathBuf};

    use super::xdg::{desktop_entry, entry_file};
    use super::{State, file_entry};

    fn file() -> Result<PathBuf, String> {
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .ok_or("HOME is not set")?;
        Ok(entry_file(&config_home))
    }

    pub fn query(exe: &Path) -> Result<State, String> {
        file_entry::query(&file()?, &desktop_entry(exe))
    }

    pub fn register(exe: &Path) -> Result<&'static str, String> {
        file_entry::write(&file()?, &desktop_entry(exe)).map(|()| "XDG autostart")
    }

    pub fn unregister() -> Result<(), String> {
        file_entry::remove(&file()?)
    }
}

#[cfg(target_os = "macos")]
mod sys {
    use std::path::{Path, PathBuf};

    use super::launchd::{agent_file, plist};
    use super::{State, file_entry};

    fn file() -> Result<PathBuf, String> {
        let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
        Ok(agent_file(Path::new(&home)))
    }

    pub fn query(exe: &Path) -> Result<State, String> {
        file_entry::query(&file()?, &plist(exe))
    }

    pub fn register(exe: &Path) -> Result<&'static str, String> {
        file_entry::write(&file()?, &plist(exe)).map(|()| "LaunchAgent")
    }

    /// Không `launchctl bootout`: với exe rời agent đang nạp chính là daemon này, gỡ là giết nó. Bỏ tệp là lần đăng nhập
    /// sau hết chạy.
    pub fn unregister() -> Result<(), String> {
        file_entry::remove(&file()?)
    }
}

#[cfg(test)]
mod tests {
    use super::task::{task_matches, task_xml};
    use super::xdg::{desktop_entry, entry_file};
    use super::*;

    #[test]
    fn task_xml_runs_at_log_on_with_escaped_path() {
        let exe = Path::new(r"C:\Program Files\R&D <x>\ferry.exe");
        let xml = task_xml(exe, r"PC\Đoài");
        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.contains("<Priority>7</Priority>"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains(r"<Command>C:\Program Files\R&amp;D &lt;x&gt;\ferry.exe</Command>"));
        assert!(xml.contains(r"<UserId>PC\Đoài</UserId>"));
        assert!(task_matches(&xml, exe));
        assert!(!task_matches(&xml, Path::new(r"D:\other\ferry.exe")), "moved exe ⇒ stale");
        assert!(task_matches(&xml, Path::new(r"c:\program files\r&d <x>\FERRY.EXE")), "path match ignores case");
        assert!(!task_matches(&xml.replace("--autostart", ""), exe), "no --autostart ⇒ stale (would open the window)");
    }

    /// Lỗi của OS phải về tới người gọi: màn cài đặt hiện nó dưới công tắc.
    #[test]
    fn sync_returns_the_os_error_and_skips_no_ops() {
        let failed = || Err("denied".to_owned());
        let never = || -> Result<(), String> { panic!("must not touch the OS") };
        assert_eq!(sync_with(true, Ok(State::Absent), failed, never), Err("denied".into()));
        assert_eq!(sync_with(true, Ok(State::Stale), failed, never), Err("denied".into()));
        assert_eq!(sync_with(false, Ok(State::Current), never, failed), Err("denied".into()));
        assert_eq!(sync_with(false, Ok(State::Stale), never, || Ok(())), Ok(()));
        assert_eq!(sync_with(true, Err("no query".into()), never, never), Err("no query".into()));
        assert_eq!(sync_with(true, Ok(State::Current), never, never), Ok(()));
        assert_eq!(sync_with(false, Ok(State::Absent), never, never), Ok(()));
        assert_eq!(sync_with(true, Ok(State::Absent), || Ok(()), never), Ok(()));
    }

    #[test]
    fn desktop_entry_quotes_exec_and_has_autostart_flag() {
        let entry = desktop_entry(Path::new("/opt/my apps/$x/fer\"ry%"));
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("Exec=\"/opt/my apps/\\$x/fer\\\"ry%%\" --autostart\n"));
        assert!(entry.contains("Name=Ferry\n"));
        let file = entry_file(Path::new("/home/u/.config"));
        assert_eq!(file, Path::new("/home/u/.config/autostart").join(format!("{BASE_NAME}.desktop")));
    }

    #[test]
    fn launch_agent_has_args_and_escaped_path() {
        let text = launchd::plist(Path::new("/Users/a b/R&D/ferry"));
        assert!(text.contains("<string>/Users/a b/R&amp;D/ferry</string>\n    <string>--autostart</string>\n  </array>"));
        assert!(text.contains(&format!("<key>Label</key>\n  <string>{}</string>", launchd::LABEL)));
        assert!(text.contains("<key>RunAtLoad</key>\n  <true/>"));
        let exe = Path::new("/Applications/Ferry.app/Contents/MacOS/ferry");
        assert_eq!(
            launchd::program_arguments(exe),
            ["/usr/bin/open", "-g", "-a", "/Applications/Ferry.app", "--args", "--autostart"]
        );
        assert!(!launchd::in_app_bundle(Path::new("/x/Contents/MacOS/ferry")), "Contents không nằm trong .app");
    }

    #[test]
    fn debug_entries_do_not_clash_with_the_installed_app() {
        assert_eq!(BASE_NAME, "ferry.dev", "tests run as debug builds");
        assert!(launchd::LABEL.ends_with(".dev"));
        let n = task_name();
        assert!(n.starts_with("ferry.dev-"));
        assert!(n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')));
    }

    #[test]
    fn dev_builds_do_not_self_register() {
        assert!(!self_managed(Path::new("/src/ferry/target/release/ferry")));
        assert_eq!(self_managed(Path::new("/opt/ferry/ferry")), !cfg!(debug_assertions));
    }
}
