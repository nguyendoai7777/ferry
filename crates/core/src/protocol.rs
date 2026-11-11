//! Kiểu JSON + header của giao thức HTTP v1 (`PROTOCOL.md` mục 6) — task gd1_02.
//!
//! Chỉ dữ liệu trên dây, không mạng: `client` gửi / nhận, server giả `tests/fake_phone` dựng lại đúng các kiểu này.
//! Khoá JSON camelCase như điện thoại trả. Trường W-3 (`port`, `addresses`, `features`, `dir`) có mặc định để điện thoại
//! cũ (chưa có trường) vẫn parse được.

use std::fmt;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::device::DeviceId;

/// Phiên bản giao thức PC hiểu (`info.api`, TXT `api=`).
pub const API_VERSION: u32 = 1;

/// Đường dẫn route (PROTOCOL 6.3–6.5).
pub mod path {
    pub const INFO: &str = "/api/v1/info";
    pub const PAIR: &str = "/api/v1/pair";
    pub const LIST: &str = "/api/v1/list";
    /// `+ <id>` (id mờ, URL-encode bằng [`super::encode_path_segment`]).
    pub const FILE: &str = "/api/v1/file/";
    pub const UPLOADS: &str = "/api/v1/uploads";
    /// `?path=<URL-encoded>` — chỉ khi có [`super::feature::BROWSE`] (§6.11).
    pub const DIRS: &str = "/api/v1/dirs";
}

/// Tên header riêng của giao thức (PROTOCOL 6.3, 6.5).
pub mod header {
    pub const CLIENT_ID: &str = "x-client-id";
    pub const CLIENT_NAME: &str = "x-client-name";
    pub const FILE_NAME: &str = "x-file-name";
    pub const FILE_DIR: &str = "x-file-dir";
    pub const UPLOAD_LENGTH: &str = "upload-length";
    pub const UPLOAD_OFFSET: &str = "upload-offset";
    /// `Content-Type` của thân `PATCH`.
    pub const OFFSET_OCTET_STREAM: &str = "application/offset+octet-stream";
}

/// Giá trị trong `info.features` (PROTOCOL 6.3).
pub mod feature {
    /// Nhận `X-File-Dir` (§6.9). Thiếu ⇒ PC gửi phẳng.
    pub const DIRS: &str = "dirs";
    /// Server nghe IPv6 (§6.8).
    pub const IPV6: &str = "ipv6";
    /// Duyệt bộ nhớ trong thật qua `GET /dirs`; mọi `dir` tính từ gốc bộ nhớ dùng chung (§6.11, W-7).
    pub const BROWSE: &str = "browse";
}

/// Đuôi video điện thoại nhận (PROTOCOL 6.6), chữ thường.
pub const VIDEO_EXTENSIONS: [&str; 9] = ["mp4", "m4v", "mov", "mkv", "webm", "ts", "3gp", "avi", "flv"];

/// Tên tệp có đuôi video điện thoại nhận không (không phân biệt hoa thường). Lọc phía PC cho khỏi phí một request.
pub fn is_video_name(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else { return false };
    !stem.is_empty() && VIDEO_EXTENSIONS.iter().any(|v| ext.eq_ignore_ascii_case(v))
}

/// Byte giữ nguyên khi mã hoá: chỉ ký tự "unreserved" của RFC 3986 (`A-Z a-z 0-9 - . _ ~`). Mã hoá cả `+`, khoảng trắng
/// và `/` để phía nào giải mã kiểu form (`+` ⇒ khoảng trắng) cũng ra đúng chuỗi.
const HEADER_ENCODE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

/// Mã hoá giá trị header tên tệp / thư mục / tên máy (PROTOCOL 6.1): percent-encoding mọi byte UTF-8 ngoài "unreserved".
pub fn encode_header_value(value: &str) -> String {
    utf8_percent_encode(value, HEADER_ENCODE).to_string()
}

/// Giải mã header tên tệp / thư mục. `None` khi kết quả không phải UTF-8.
pub fn decode_header_value(value: &str) -> Option<String> {
    percent_decode_str(value).decode_utf8().ok().map(|s| s.into_owned())
}

/// Mã hoá một đoạn đường dẫn URL (id mờ của `list`, uid upload — PROTOCOL 6.4: "URL-encode khi ghép đường dẫn").
pub fn encode_path_segment(segment: &str) -> String {
    utf8_percent_encode(segment, HEADER_ENCODE).to_string()
}

/// Thư mục đích dạng điện thoại hiểu (§6.9): `\` ⇒ `/`, bỏ đoạn rỗng. Làm sạch thật là việc của điện thoại.
pub fn normalize_dir(dir: &str) -> String {
    dir.split(['/', '\\']).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/")
}

/// Bí mật (token lâu dài, PIN): không `Display`, `Debug` che giá trị, không `Serialize` — khỏi lọt vào log / IPC / config.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Giá trị thật — chỉ để dựng header / ghi kho khoá.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Secret)
    }
}

/// `GET /api/v1/info` (PROTOCOL 6.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub app: String,
    pub api: u32,
    pub id: DeviceId,
    pub name: String,
    /// `true` khi request kèm Bearer token còn sống.
    #[serde(default)]
    pub paired: bool,
    /// Cổng server (W-3). Điện thoại cũ: `None` ⇒ dùng cổng đang nối.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Địa chỉ LAN của điện thoại, không cổng, không link-local (W-3). Điện thoại cũ: rỗng.
    #[serde(default)]
    pub addresses: Vec<String>,
    /// Tính năng điện thoại có ([`feature`]). Điện thoại cũ: rỗng.
    #[serde(default)]
    pub features: Vec<String>,
}

impl Info {
    pub fn has_feature(&self, name: &str) -> bool {
        self.features.iter().any(|f| f == name)
    }
}

/// `200` của `POST /api/v1/pair` (PROTOCOL 6.3).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairResponse {
    pub token: Secret,
    pub id: DeviceId,
    pub name: String,
}

/// Một video trong thư viện điện thoại: phần tử của `list` và thân `201` của khúc upload cuối (PROTOCOL 6.4, 6.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryItem {
    /// Chuỗi mờ, dùng nguyên văn trong URL ([`encode_path_segment`]).
    pub id: String,
    /// Tên trên điện thoại — có thể khác tên gửi (điện thoại đổi khi trùng).
    pub name: String,
    pub size: u64,
    /// Mili giây Unix.
    pub modified: i64,
    #[serde(default)]
    pub mime: String,
    /// Thư mục chứa, tương đối so với nơi lưu, `/` phân cách (§6.9). `Some("")` = gốc (cả khi điện thoại cũ không gửi
    /// trường này); `None` (`null`) = ngoài nơi lưu — nhóm "Khác", không dùng làm đích gửi.
    #[serde(default = "root_dir", serialize_with = "serialize_dir")]
    pub dir: Option<String>,
}

fn root_dir() -> Option<String> {
    Some(String::new())
}

fn serialize_dir<S: Serializer>(dir: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
    match dir {
        Some(d) => s.serialize_str(d),
        None => s.serialize_none(),
    }
}

/// `200` của `GET /api/v1/list`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResponse {
    pub items: Vec<LibraryItem>,
}

/// Một thư mục con trong `GET /api/v1/dirs` (§6.11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteDir {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<i64>,
    /// Số mục trực tiếp bên trong; `None` = điện thoại không đếm được.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
}

/// Một tệp trong `GET /api/v1/dirs` (§6.11): mọi loại tệp; `id` chỉ khi nằm trong thư viện (tải về được).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFile {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub modified: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// `200` của `GET /api/v1/dirs?path=` (§6.11): con trực tiếp của `path` (đã làm sạch, `""` = gốc bộ nhớ).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListing {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub dirs: Vec<RemoteDir>,
    #[serde(default)]
    pub files: Vec<RemoteFile>,
}

/// `201` của `POST /api/v1/uploads` (PROTOCOL 6.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadCreated {
    /// `uid` của phiên upload.
    pub id: String,
    pub offset: u64,
    pub length: u64,
    /// Thư mục đích điện thoại đã làm sạch (§6.9); điện thoại cũ không gửi ⇒ `""`.
    #[serde(default)]
    pub dir: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_listing_parses_with_optional_fields() {
        let v: DirListing = serde_json::from_str(
            r#"{"path":"DCIM","dirs":[{"name":"Camera","modified":5,"count":4},{"name":"X"}],
                "files":[{"name":"a.txt","size":3,"modified":1},{"name":"c.mp4","size":9,"modified":2,"id":"v1"}]}"#,
        )
        .unwrap();
        assert_eq!(v.path, "DCIM");
        assert_eq!(v.dirs[0].count, Some(4));
        assert_eq!(v.dirs[1].modified, None);
        assert_eq!(v.files[1].id.as_deref(), Some("v1"));
        assert!(v.files[0].id.is_none());
    }

    #[test]
    fn video_names() {
        for n in ["a.mp4", "B.MKV", "x.y.Webm", "clip.ts", "Phim hè.3GP"] {
            assert!(is_video_name(n), "{n}");
        }
        for n in ["a.mp3", "mp4", ".mp4", "a.mp4.part", "a"] {
            assert!(!is_video_name(n), "{n}");
        }
    }

    #[test]
    fn header_encoding_round_trips_vietnamese_and_slash() {
        let dir = "Chuyến đi/Ngày 1+2";
        let enc = encode_header_value(dir);
        assert!(enc.is_ascii());
        assert!(!enc.contains(['/', ' ', '+']), "{enc}");
        assert_eq!(enc, "Chuy%E1%BA%BFn%20%C4%91i%2FNg%C3%A0y%201%2B2");
        assert_eq!(decode_header_value(&enc).as_deref(), Some(dir));
        assert_eq!(encode_header_value("My PC"), "My%20PC");
    }

    #[test]
    fn normalize_dir_uses_forward_slash() {
        assert_eq!(normalize_dir(r"Phim\Chuyến đi\\Ngày 1\"), "Phim/Chuyến đi/Ngày 1");
        assert_eq!(normalize_dir("/a//b/"), "a/b");
        assert_eq!(normalize_dir(""), "");
    }

    #[test]
    fn old_phone_info_parses() {
        let v: Info = serde_json::from_str(
            r#"{"app":"xmobile.videoplayer","api":1,"id":"3f9a0c1d2b4e5a67","name":"Pixel 8","paired":false}"#,
        )
        .unwrap();
        assert_eq!(v.id, DeviceId("3f9a0c1d2b4e5a67".into()));
        assert_eq!(v.port, None);
        assert!(v.addresses.is_empty() && v.features.is_empty());
        assert!(!v.has_feature(feature::DIRS));
    }

    #[test]
    fn new_phone_info_parses() {
        let v: Info = serde_json::from_str(
            r#"{"app":"xmobile.videoplayer","api":1,"id":"3f9a0c1d2b4e5a67","name":"Pixel 8","paired":true,
                "port":8080,"addresses":["192.168.1.23","fd12:3456:789a::23"],"features":["ipv6","dirs"],"future":1}"#,
        )
        .unwrap();
        assert_eq!(v.port, Some(8080));
        assert_eq!(v.addresses.len(), 2);
        assert!(v.paired && v.has_feature(feature::DIRS) && v.has_feature(feature::IPV6));
    }

    #[test]
    fn list_item_dir_absent_null_value() {
        let base = r#""id":"x","name":"a.mp4","size":1,"modified":2,"mime":"video/mp4""#;
        let absent: LibraryItem = serde_json::from_str(&format!("{{{base}}}")).unwrap();
        let null: LibraryItem = serde_json::from_str(&format!("{{{base},\"dir\":null}}")).unwrap();
        let set: LibraryItem = serde_json::from_str(&format!("{{{base},\"dir\":\"Chuyến đi\"}}")).unwrap();
        assert_eq!(absent.dir.as_deref(), Some(""));
        assert_eq!(null.dir, None);
        assert_eq!(set.dir.as_deref(), Some("Chuyến đi"));
        // Fake phone ghi lại: `None` ⇒ `null`.
        assert!(serde_json::to_string(&null).unwrap().contains("\"dir\":null"));
    }

    #[test]
    fn secret_debug_is_redacted() {
        let r: PairResponse = serde_json::from_str(r#"{"token":"abcdef0123","id":"1","name":"P"}"#).unwrap();
        assert_eq!(r.token.expose(), "abcdef0123");
        assert!(!format!("{r:?}").contains("abcdef"));
    }
}
