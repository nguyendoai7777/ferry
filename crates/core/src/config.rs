//! `config.json` (khoá camelCase, mọi trường có mặc định): định danh của PC với điện thoại (`PROTOCOL.md` 6.3), ngôn
//! ngữ, thiết bị đã ghép. Chỉ daemon `ferry` ghi; `ferry-ui` đọc ([`load`]), đổi gì thì gửi lệnh qua IPC.
//!
//! Token **không bao giờ** nằm ở đây (`PROTOCOL.md` 2.2): chỉ ở kho khoá OS (`pairing`, gd1_06).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::device::DeviceId;
use crate::i18n::LanguagePref;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// UUID v4 sinh lần chạy đầu, gửi khi ghép (`PROTOCOL.md` 6.3). Đổi ⇒ mọi điện thoại coi là PC mới.
    pub client_id: String,
    /// Tên PC hiện trên điện thoại; mặc định tên máy.
    pub client_name: String,
    pub language: LanguagePref,
    /// Điện thoại đã ghép (task gd1_06 thêm / xoá). Nhận theo `id`, địa chỉ chỉ là gợi ý lần nối sau.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<PairedDevice>,
    /// Thư mục nhận tệp "Tải về máy" (PROTOCOL 6.4, gd1_10). Vắng ⇒ [`default_download_dir`]; dùng
    /// [`Config::download_dir`] thay vì đọc thẳng.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_dir: Option<PathBuf>,
    /// Trần tốc độ chung mọi điện thoại, MB/s (1 MB = 1 048 576 byte, cùng đơn vị chữ hiển thị); vắng = không giới hạn
    /// (gd1_12). Dùng [`Config::speed_limit_bytes`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_limit_mb_per_sec: Option<u32>,
    /// Khởi động cùng hệ thống (gd1_12, `daemon/src/autostart.rs`). Mặc định tắt.
    pub autostart: bool,
    /// Thư mục theo dõi (gd1_12 lưu, gd1_13 chạy).
    pub watch_folder: WatchFolder,
    /// Địa chỉ nhập tay ở "Nâng cao" (`ip:port`, `[v6]:port`, `[fe80::x%iface]:port`): daemon hỏi `info` lúc khởi động
    /// để điện thoại vẫn hiện khi mạng chặn mDNS (gd1_12).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manual_addresses: Vec<String>,
}

/// Một thư mục theo dõi: tệp mới ổn định trong đó tự gửi sang điện thoại (gd1_13).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WatchFolder {
    pub enabled: bool,
    /// Vắng ⇒ chưa chọn; `enabled` không có nghĩa khi chưa có thư mục.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

/// Trần trên của giới hạn tốc độ (MB/s) — trên mức này coi như gõ nhầm.
pub const MAX_SPEED_LIMIT_MB_PER_SEC: u32 = 10_000;

/// Một điện thoại đã ghép (W-3, `PROTOCOL.md` 6.8): `id` + **tập** địa chỉ cuối cùng thấy được, IPv4 lẫn IPv6.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PairedDevice {
    pub id: DeviceId,
    /// Tên hiển thị (`info.name`).
    pub name: String,
    /// `ip:port`, `[v6]:port`, `[fe80::x%iface]:port` — dạng chuỗi để giữ được tên interface của địa chỉ link-local.
    pub addresses: Vec<String>,
    /// Tên người dùng đặt trên PC (cài đặt, gd1_12); vắng ⇒ hiện `name` của điện thoại. `info.name` đổi không đè cái này.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

impl Config {
    /// Thư mục tải về đang dùng: `downloadDir` khi đặt (không rỗng), không thì mặc định của OS.
    pub fn download_dir(&self) -> PathBuf {
        self.download_dir.clone().filter(|d| !d.as_os_str().is_empty()).unwrap_or_else(default_download_dir)
    }

    /// Trần tốc độ, byte / s; `None` = không giới hạn (`0` cũng là không giới hạn).
    pub fn speed_limit_bytes(&self) -> Option<u64> {
        self.speed_limit_mb_per_sec.filter(|n| *n > 0).map(|n| u64::from(n) * 1024 * 1024)
    }

    /// Thêm / cập nhật điện thoại vừa ghép (ghép lại ⇒ thay tên + địa chỉ, giữ vị trí trong danh sách). Không token.
    pub fn remember_device(&mut self, device: PairedDevice) {
        match self.devices.iter_mut().find(|d| d.id == device.id) {
            // Ghép lại không mất bí danh người dùng đã đặt.
            Some(d) => {
                let alias = d.alias.take();
                *d = PairedDevice { alias: device.alias.clone().or(alias), ..device };
            }
            None => self.devices.push(device),
        }
    }

    /// Bỏ điện thoại đã quên. `false` ⇒ không có trong danh sách.
    pub fn forget_device(&mut self, id: &DeviceId) -> bool {
        let before = self.devices.len();
        self.devices.retain(|d| d.id != *id);
        self.devices.len() != before
    }
}

/// Kết quả của [`load_or_init`].
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    /// Tệp vừa được tạo (lần chạy đầu) hoặc vừa được điền trường thiếu.
    pub written: bool,
    /// Tệp cũ hỏng, đã đổi tên thành đường dẫn này để người dùng còn gỡ lại được.
    pub backup: Option<PathBuf>,
}

/// Đọc cấu hình, không ghi gì (UI). Thiếu tệp ⇒ mặc định; hỏng ⇒ lỗi `InvalidData`.
pub fn load(path: &Path) -> io::Result<Config> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e),
    }
}

/// Daemon lúc khởi động: đọc, điền `clientId` / `clientName` nếu thiếu, ghi lại khi có đổi. Tệp hỏng ⇒ đổi tên thành
/// `config.json.bad` rồi bắt đầu lại (mất `clientId` ⇒ phải ghép lại, nhưng app vẫn chạy).
pub fn load_or_init(path: &Path) -> io::Result<Loaded> {
    let (mut config, mut written, backup) = match load(path) {
        Ok(c) => (c, !path.exists(), None),
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            let bad = path.with_extension("json.bad");
            fs::rename(path, &bad)?;
            (Config::default(), true, Some(bad))
        }
        Err(e) => return Err(e),
    };
    if config.client_id.trim().is_empty() {
        config.client_id = uuid::Uuid::new_v4().to_string();
        written = true;
    }
    if config.client_name.trim().is_empty() {
        config.client_name = default_client_name();
        written = true;
    }
    if written {
        save(path, &config)?;
    }
    Ok(Loaded { config, written, backup })
}

/// Ghi nguyên tử: tệp tạm cùng thư mục ⇒ `sync` ⇒ đổi tên đè. Mất điện giữa chừng thì còn bản cũ hoặc bản mới, không
/// bao giờ nửa tệp.
pub fn save(path: &Path, config: &Config) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut json = serde_json::to_vec_pretty(config).map_err(io::Error::other)?;
    json.push(b'\n');
    let tmp = dir.join(format!(".{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("config.json")));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Thư mục Downloads của OS (app gửi tệp bất kỳ — W-5 — nên không mặc định thư mục Video), thiếu thì Video, rồi thư
/// mục nhà, cuối cùng thư mục hiện tại.
pub fn default_download_dir() -> PathBuf {
    dirs::download_dir().or_else(dirs::video_dir).or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."))
}

/// Tên máy, bỏ đuôi `.local` của macOS; không đọc được ⇒ "PC".
pub fn default_client_name() -> String {
    let host = gethostname::gethostname().to_string_lossy().into_owned();
    let host = host.strip_suffix(".local").unwrap_or(&host).trim().to_owned();
    if host.is_empty() { "PC".to_owned() } else { host }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_creates_file_with_client_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("config.json");
        let loaded = load_or_init(&path).unwrap();
        assert!(loaded.written);
        assert!(uuid::Uuid::parse_str(&loaded.config.client_id).is_ok());
        assert!(!loaded.config.client_name.is_empty());
        let v: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["clientId"], loaded.config.client_id.as_str());
        assert_eq!(v["language"], "system");
        assert!(v.get("devices").is_none(), "empty list stays out of the file");

        let again = load_or_init(&path).unwrap();
        assert!(!again.written);
        assert_eq!(again.config, loaded.config, "clientId is stable across runs");
        assert_eq!(load(&path).unwrap(), loaded.config);
    }

    #[test]
    fn missing_fields_take_defaults_and_unknown_fields_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"clientName":"Bàn làm việc","fromTheFuture":1}"#).unwrap();
        let loaded = load_or_init(&path).unwrap();
        assert!(loaded.written, "clientId was filled in");
        assert_eq!(loaded.config.client_name, "Bàn làm việc");
        assert_eq!(loaded.config.language, LanguagePref::System);
    }

    #[test]
    fn devices_keep_ipv4_and_ipv6_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let config = Config {
            client_id: "c".into(),
            client_name: "n".into(),
            language: LanguagePref::Vi,
            devices: vec![PairedDevice {
                id: DeviceId("a1b2".into()),
                name: "Pixel".into(),
                addresses: vec!["192.168.1.20:8080".into(), "[fe80::1%en0]:8080".into()],
                alias: None,
            }],
            ..Config::default()
        };
        save(&path, &config).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["devices"][0]["id"], "a1b2");
        assert_eq!(v["devices"][0]["addresses"][1], "[fe80::1%en0]:8080");
        assert_eq!(v["language"], "vi");
        assert_eq!(load(&path).unwrap(), config);
        assert!(!dir.path().join(".config.json.tmp").exists());
    }

    #[test]
    fn remember_and_forget_device() {
        let mut c = Config::default();
        let dev = |id: &str, name: &str| PairedDevice { id: DeviceId(id.into()), name: name.into(), ..PairedDevice::default() };
        c.remember_device(dev("a", "A"));
        c.remember_device(dev("b", "B"));
        c.remember_device(dev("a", "A2"));
        assert_eq!(c.devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["A2", "B"]);
        assert!(c.forget_device(&DeviceId("a".into())));
        assert!(!c.forget_device(&DeviceId("a".into())));
        assert_eq!(c.devices.len(), 1);
    }

    #[test]
    fn download_dir_defaults_and_round_trips() {
        let mut c = Config::default();
        assert_eq!(c.download_dir(), default_download_dir());
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("downloadDir").is_none(), "unset stays out of the file");
        c.download_dir = Some(PathBuf::from(""));
        assert_eq!(c.download_dir(), default_download_dir(), "empty string means default");
        let back: Config = serde_json::from_str(r#"{"downloadDir":"/tmp/phim"}"#).unwrap();
        assert_eq!(back.download_dir(), PathBuf::from("/tmp/phim"));
    }

    /// C1 (gd1_12): mọi khoá cài đặt camelCase, ghi rồi đọc lại còn nguyên; vắng ⇒ mặc định, không ghi ra tệp.
    #[test]
    fn settings_keys_are_camel_case_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let defaults = serde_json::to_value(Config::default()).unwrap();
        for key in ["speedLimitMbPerSec", "manualAddresses", "downloadDir"] {
            assert!(defaults.get(key).is_none(), "{key} stays out of the file when unset");
        }
        assert_eq!(defaults["autostart"], false);
        assert_eq!(defaults["watchFolder"], serde_json::json!({ "enabled": false }));

        let config = Config {
            client_id: "c".into(),
            language: LanguagePref::En,
            download_dir: Some(PathBuf::from("/tmp/nhận")),
            speed_limit_mb_per_sec: Some(5),
            autostart: true,
            watch_folder: WatchFolder { enabled: true, path: Some(PathBuf::from("/tmp/theo dõi")) },
            manual_addresses: vec!["192.168.1.9:8080".into(), "[fd12::5]:8080".into()],
            devices: vec![PairedDevice {
                id: DeviceId("a1".into()),
                name: "Pixel 8".into(),
                addresses: vec![],
                alias: Some("Máy của mẹ".into()),
            }],
            ..Config::default()
        };
        save(&path, &config).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["speedLimitMbPerSec"], 5);
        assert_eq!(v["autostart"], true);
        assert_eq!(v["watchFolder"], serde_json::json!({ "enabled": true, "path": "/tmp/theo dõi" }));
        assert_eq!(v["manualAddresses"][1], "[fd12::5]:8080");
        assert_eq!(v["devices"][0]["alias"], "Máy của mẹ");
        assert_eq!(v["downloadDir"], "/tmp/nhận");
        assert_eq!(v["language"], "en");
        assert_eq!(load(&path).unwrap(), config);
        assert_eq!(config.speed_limit_bytes(), Some(5 * 1024 * 1024));
        assert_eq!(Config { speed_limit_mb_per_sec: Some(0), ..Config::default() }.speed_limit_bytes(), None);
    }

    #[test]
    fn broken_file_is_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, b"{not json").unwrap();
        assert_eq!(load(&path).unwrap_err().kind(), io::ErrorKind::InvalidData);
        let loaded = load_or_init(&path).unwrap();
        let bad = loaded.backup.expect("backup");
        assert_eq!(fs::read(&bad).unwrap(), b"{not json");
        assert!(load(&path).is_ok());
    }
}
