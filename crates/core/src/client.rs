//! Client HTTP của giao thức v1 (`PROTOCOL.md` 6.3–6.5, 6.9): mỗi request một kết nối, không TLS — task gd1_02.
//!
//! - Một [`PhoneClient`] = một địa chỉ (`SocketAddr` v4 / v6 / v6 + `scope_id`). Chọn địa chỉ, happy eyeballs, nhớ
//!   địa chỉ thắng là việc của tầng trên (gd1_05); IP đổi ⇒ dựng client mới.
//! - Không ghép IP vào URL (PROTOCOL 6.8 bẫy: `url` không nhận `%en0`): URL dùng host giả `<id>.vpsync.invalid`,
//!   `ClientBuilder::resolve` trỏ host đó về `SocketAddr` — một đường code cho IPv4, IPv6, link-local.
//! - Quyết theo **mã trạng thái** ([`ClientError`]); thân lỗi chỉ vào log (debug, cắt ngắn).
//! - Không đọc cả tệp vào RAM: `PATCH` đọc dần từ `AsyncRead`, tải về ghi dần ra `AsyncWrite`.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use reqwest::header::{self as h, HeaderMap};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio_util::io::ReaderStream;

use crate::device::DeviceId;
use crate::protocol::{
    self, DirListing, EntryPath, LibraryItem, ListResponse, PairResponse, Secret, UploadCreated, decode_header_value,
    encode_header_value, encode_path_segment, header, path,
};

/// Server đóng kết nối sau 60 s không có byte (PROTOCOL 6.1) — client chờ đọc cùng mức.
pub const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Nối TCP trên LAN: quá mức này là địa chỉ chết (tầng trên thử địa chỉ khác).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Khúc `PATCH` gợi ý (PROTOCOL 6.5).
pub const CHUNK_SIZE: u64 = 32 * 1024 * 1024;
/// Đuôi host giả (PROTOCOL 6.8).
pub const FAKE_HOST_SUFFIX: &str = ".vpsync.invalid";

/// Bộ đệm đọc tệp cho thân `PATCH`: RAM của một khúc đang gửi chỉ cỡ này, không phải 32 MB.
const BODY_BUF: usize = 64 * 1024;
/// Thân lỗi chỉ để log: đọc tối đa chừng này.
const ERROR_BODY_MAX: usize = 1024;
/// `X-Client-Name` tối đa 64 ký tự (PROTOCOL 6.3).
const CLIENT_NAME_MAX: usize = 64;

/// Lỗi theo mã trạng thái (PROTOCOL 6.3–6.5). Không mang thân lỗi (chỉ log).
#[derive(Debug)]
pub enum ClientError {
    /// `400` — thiếu / sai header (vd `X-File-Dir` có `..`, quá sâu — §6.9).
    BadRequest,
    /// `401` — token chết (điện thoại "Quên các máy tính đã ghép") ⇒ `NeedsRepair`.
    Unauthorized,
    /// `403` — sai PIN (`pair`); ở route khác: không có quyền (`dirs` khi thiếu `browse`, vùng cấm sửa §6.12).
    WrongPin,
    /// `404` — không có upload / tệp này (server đã tắt, phiên quá hạn) ⇒ tạo lại.
    NotFound,
    /// `409` — offset lệch; giá trị = offset đúng theo điện thoại.
    OffsetMismatch(u64),
    /// `409` không kèm `Upload-Offset` — đã có mục cùng tên / đang bận (§6.12).
    Conflict,
    /// `411` / `413` — thân thiếu / quá dài, khúc vượt `Upload-Length`.
    TooLarge,
    /// `415` — không phải đuôi video (§6.6).
    Unsupported,
    /// `416` — `Range` ngoài cỡ tệp.
    RangeNotSatisfiable,
    /// `423` — đang có PATCH khác cho cùng upload ⇒ chờ rồi `HEAD`.
    Locked,
    /// `429` — sai PIN 5 lần, khoá tạm.
    RateLimited { retry_after: Option<Duration> },
    /// `503` — quá nhiều kết nối / phiên upload dở.
    Busy { retry_after: Option<Duration> },
    /// `507` — điện thoại không đủ chỗ (cần tổng + 16 MB).
    InsufficientStorage,
    /// Mã khác.
    Server(u16),
    /// Không nối được / đứt / quá thời gian. `kind()`: `ConnectionRefused` (nối hỏng), `TimedOut`, `Other` (đứt giữa chừng).
    Network(io::Error),
    /// Lỗi đọc tệp nguồn / ghi tệp đích phía PC — không phải lỗi mạng, thử lại không ích.
    Local(io::Error),
    /// Phản hồi sai hợp đồng (JSON hỏng, thiếu header bắt buộc).
    InvalidResponse(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest => f.write_str("bad request (400)"),
            Self::Unauthorized => f.write_str("unauthorized (401)"),
            Self::WrongPin => f.write_str("wrong PIN (403)"),
            Self::NotFound => f.write_str("not found (404)"),
            Self::OffsetMismatch(o) => write!(f, "offset mismatch (409), phone has {o}"),
            Self::Conflict => f.write_str("conflict (409)"),
            Self::TooLarge => f.write_str("payload too large (413)"),
            Self::Unsupported => f.write_str("unsupported media type (415)"),
            Self::RangeNotSatisfiable => f.write_str("range not satisfiable (416)"),
            Self::Locked => f.write_str("upload locked (423)"),
            Self::RateLimited { retry_after } => write!(f, "rate limited (429), retry after {retry_after:?}"),
            Self::Busy { retry_after } => write!(f, "busy (503), retry after {retry_after:?}"),
            Self::InsufficientStorage => f.write_str("insufficient storage (507)"),
            Self::Server(c) => write!(f, "HTTP {c}"),
            Self::Network(e) => write!(f, "network: {e}"),
            Self::Local(e) => write!(f, "local file: {e}"),
            Self::InvalidResponse(m) => write!(f, "invalid response: {m}"),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Network(e) | Self::Local(e) => Some(e),
            _ => None,
        }
    }
}

impl ClientError {
    /// Map mã trạng thái lỗi (không 2xx) — bảng PROTOCOL 6.3–6.5.
    fn from_status(status: StatusCode, headers: &HeaderMap) -> Self {
        match status.as_u16() {
            400 => Self::BadRequest,
            401 => Self::Unauthorized,
            403 => Self::WrongPin,
            404 => Self::NotFound,
            409 => match header_u64(headers, header::UPLOAD_OFFSET) {
                Some(o) => Self::OffsetMismatch(o),
                None => Self::Conflict,
            },
            411 | 413 => Self::TooLarge,
            415 => Self::Unsupported,
            416 => Self::RangeNotSatisfiable,
            423 => Self::Locked,
            429 => Self::RateLimited { retry_after: retry_after(headers) },
            503 => Self::Busy { retry_after: retry_after(headers) },
            507 => Self::InsufficientStorage,
            c => Self::Server(c),
        }
    }

    fn network(e: reqwest::Error) -> Self {
        let kind = if e.is_timeout() {
            io::ErrorKind::TimedOut
        } else if e.is_connect() {
            io::ErrorKind::ConnectionRefused
        } else {
            io::ErrorKind::Other
        };
        // `reqwest` chỉ in "error sending request for url (…)": nối cả chuỗi nguyên nhân (timeout, connection refused,
        // no route…) để log daemon nói được vì sao.
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(&e);
        while let Some(cause) = src {
            msg.push_str(": ");
            msg.push_str(&cause.to_string());
            src = cause.source();
        }
        Self::Network(io::Error::new(kind, msg))
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;

/// Kết quả một khúc `PATCH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchOutcome {
    /// `204` — chưa đủ, offset mới.
    Partial(u64),
    /// `201` — khúc cuối, tệp đã vào thư viện (`name` có thể khác tên gửi).
    Done(LibraryItem),
}

/// `HEAD /api/v1/uploads/<uid>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadStatus {
    pub offset: u64,
    pub length: u64,
}

/// `HEAD /api/v1/file/<id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta {
    pub length: u64,
    pub etag: Option<String>,
    pub mime: Option<String>,
    /// Từ `Content-Disposition` (`filename*` UTF-8, rồi `filename`).
    pub file_name: Option<String>,
}

/// Client của một điện thoại tại một địa chỉ.
pub struct PhoneClient {
    http: reqwest::Client,
    base: String,
    addr: SocketAddr,
    token: Option<Secret>,
}

impl std::fmt::Debug for PhoneClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhoneClient")
            .field("base", &self.base)
            .field("addr", &self.addr)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .finish()
    }
}

impl PhoneClient {
    /// `device` = `id` đã biết (TXT `id=` / `info.id`) để đặt tên host giả; chưa biết (nhập tay, trước `info`) ⇒ `None`.
    pub fn new(addr: SocketAddr, device: Option<&DeviceId>, token: Option<Secret>) -> Result<Self> {
        let host = fake_host(device);
        let http = reqwest::Client::builder()
            .no_proxy()
            .pool_max_idle_per_host(0)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            // `resolve` bỏ qua cổng của `addr` — cổng nằm trong URL. `scope_id` của link-local được giữ.
            .resolve(&host, addr)
            .build()
            .map_err(|e| ClientError::Local(io::Error::other(e)))?;
        Ok(Self { http, base: format!("http://{host}:{}", addr.port()), addr, token })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    /// Đặt / bỏ token (sau `pair` / `unpair`).
    pub fn set_token(&mut self, token: Option<Secret>) {
        self.token = token;
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let mut rb = self.http.request(method, format!("{}{path}", self.base)).header(h::CONNECTION, "close");
        if let Some(t) = &self.token {
            rb = rb.bearer_auth(t.expose());
        }
        rb
    }

    async fn send(&self, rb: RequestBuilder) -> Result<Response> {
        let resp = rb.send().await.map_err(ClientError::network)?;
        check(resp).await
    }

    /// `GET /api/v1/info` — `paired` cho biết token còn sống.
    pub async fn info(&self) -> Result<protocol::Info> {
        json(self.send(self.request(Method::GET, path::INFO)).await?).await
    }

    /// `POST /api/v1/pair` — không tự giữ token: gọi [`Self::set_token`] sau khi cất vào kho khoá.
    pub async fn pair(&self, pin: &Secret, client_id: &str, client_name: &str) -> Result<PairResponse> {
        let name: String = client_name.chars().take(CLIENT_NAME_MAX).collect();
        let rb = self
            .http
            .post(format!("{}{}", self.base, path::PAIR))
            .header(h::CONNECTION, "close")
            .header(header::CLIENT_ID, client_id)
            .header(header::CLIENT_NAME, encode_header_value(&name))
            .header(h::CONTENT_TYPE, "text/plain")
            .body(pin.expose().to_owned());
        json(self.send(rb).await?).await
    }

    /// `DELETE /api/v1/pair` — huỷ token trên điện thoại.
    pub async fn unpair(&self) -> Result<()> {
        self.send(self.request(Method::DELETE, path::PAIR)).await.map(drop)
    }

    /// `GET /api/v1/list` — mới nhất trước.
    pub async fn list(&self) -> Result<Vec<LibraryItem>> {
        let r: ListResponse = json(self.send(self.request(Method::GET, path::LIST)).await?).await?;
        Ok(r.items)
    }

    /// `GET /api/v1/dirs?path=` (§6.11) — chỉ gọi khi `info.features` có `browse`. `path` rỗng = gốc bộ nhớ.
    pub async fn dirs(&self, dir: &str) -> Result<DirListing> {
        let q = format!("{}?path={}", path::DIRS, encode_path_segment(&protocol::normalize_dir(dir)));
        json(self.send(self.request(Method::GET, &q)).await?).await
    }

    /// `POST /api/v1/dirs` (§6.12) — tạo thư mục (cả cha còn thiếu). Trả đường điện thoại đã làm sạch.
    pub async fn make_dir(&self, dir: &str) -> Result<String> {
        let body = serde_json::json!({ "path": protocol::normalize_dir(dir) });
        let r: EntryPath = json(self.send(self.json_request(Method::POST, path::DIRS, &body)).await?).await?;
        Ok(r.path)
    }

    /// `POST /api/v1/rename` (§6.12) — đổi tên tệp / thư mục `entry` thành `name` trong cùng thư mục. Trả đường mới.
    pub async fn rename(&self, entry: &str, name: &str) -> Result<String> {
        let body = serde_json::json!({ "path": protocol::normalize_dir(entry), "name": name });
        let r: EntryPath = json(self.send(self.json_request(Method::POST, path::RENAME, &body)).await?).await?;
        Ok(r.path)
    }

    /// `DELETE /api/v1/entries?path=` (§6.12) — xoá tệp, hoặc thư mục cùng cây con. Không hoàn tác.
    pub async fn delete_entry(&self, entry: &str) -> Result<()> {
        let q = format!("{}?path={}", path::ENTRIES, encode_path_segment(&protocol::normalize_dir(entry)));
        self.send(self.request(Method::DELETE, &q)).await.map(drop)
    }

    fn json_request(&self, method: Method, path: &str, body: &serde_json::Value) -> RequestBuilder {
        let bytes = serde_json::to_vec(body).unwrap_or_default();
        self.request(method, path)
            .header(h::CONTENT_TYPE, "application/json")
            .header(h::CONTENT_LENGTH, bytes.len())
            .body(bytes)
    }

    /// `POST /api/v1/uploads`. `name` chỉ phần tên. `dir` (thư mục đích, `/` hoặc `\` phân cách) chỉ gửi khi điện
    /// thoại có `features: dirs` — tầng trên quyết; `None` = không gửi `X-File-Dir` (gốc).
    pub async fn upload_create(&self, name: &str, length: u64, dir: Option<&str>) -> Result<UploadCreated> {
        let mut rb = self
            .request(Method::POST, path::UPLOADS)
            .header(header::FILE_NAME, encode_header_value(name))
            .header(header::UPLOAD_LENGTH, length)
            .header(h::CONTENT_LENGTH, 0);
        if let Some(d) = dir {
            rb = rb.header(header::FILE_DIR, encode_header_value(&protocol::normalize_dir(d)));
        }
        json(self.send(rb).await?).await
    }

    /// `PATCH /api/v1/uploads/<uid>`: gửi đúng `len` byte (> 0) đọc dần từ `reader` (đã đặt ở `offset` của tệp).
    /// Đứt giữa khúc ⇒ `Network`; byte đã tới được điện thoại giữ — [`Self::upload_head`] để biết offset thật.
    pub async fn upload_patch<R>(&self, uid: &str, offset: u64, reader: R, len: u64) -> Result<PatchOutcome>
    where
        R: AsyncRead + Send + 'static,
    {
        let local_err = Arc::new(Mutex::new(None));
        let body = ExactReader { inner: Box::pin(reader), left: len, error: local_err.clone() };
        let rb = self
            .request(Method::PATCH, &upload_path(uid))
            .header(header::UPLOAD_OFFSET, offset)
            .header(h::CONTENT_LENGTH, len)
            .header(h::CONTENT_TYPE, header::OFFSET_OCTET_STREAM)
            .body(reqwest::Body::wrap_stream(ReaderStream::with_capacity(body, BODY_BUF)));
        let resp = match rb.send().await {
            Ok(r) => r,
            Err(e) => {
                let local = local_err.lock().unwrap_or_else(|p| p.into_inner()).take();
                return Err(local.map_or_else(|| ClientError::network(e), ClientError::Local));
            }
        };
        let resp = check(resp).await?;
        match resp.status() {
            StatusCode::CREATED => Ok(PatchOutcome::Done(json(resp).await?)),
            _ => header_u64(resp.headers(), header::UPLOAD_OFFSET)
                .map(PatchOutcome::Partial)
                .ok_or_else(|| ClientError::InvalidResponse("PATCH without Upload-Offset".into())),
        }
    }

    /// `HEAD /api/v1/uploads/<uid>` — offset điện thoại đang giữ.
    pub async fn upload_head(&self, uid: &str) -> Result<UploadStatus> {
        let resp = self.send(self.request(Method::HEAD, &upload_path(uid))).await?;
        let hs = resp.headers();
        match (header_u64(hs, header::UPLOAD_OFFSET), header_u64(hs, header::UPLOAD_LENGTH)) {
            (Some(offset), Some(length)) => Ok(UploadStatus { offset, length }),
            _ => Err(ClientError::InvalidResponse("HEAD without Upload-Offset / Upload-Length".into())),
        }
    }

    /// `DELETE /api/v1/uploads/<uid>` — huỷ, điện thoại xoá tệp tạm.
    pub async fn upload_delete(&self, uid: &str) -> Result<()> {
        self.send(self.request(Method::DELETE, &upload_path(uid))).await.map(drop)
    }

    /// `HEAD /api/v1/file/<id>`.
    pub async fn file_head(&self, id: &str) -> Result<FileMeta> {
        let resp = self.send(self.request(Method::HEAD, &file_path(id))).await?;
        let hs = resp.headers();
        let length = header_u64(hs, h::CONTENT_LENGTH.as_str())
            .ok_or_else(|| ClientError::InvalidResponse("HEAD file without Content-Length".into()))?;
        Ok(FileMeta {
            length,
            etag: header_str(hs, h::ETAG.as_str()).map(str::to_owned),
            mime: header_str(hs, h::CONTENT_TYPE.as_str()).map(str::to_owned),
            file_name: header_str(hs, h::CONTENT_DISPOSITION.as_str()).and_then(disposition_file_name),
        })
    }

    /// `GET /api/v1/file/<id>` từ byte `from` (`Range: bytes=<from>-`), kèm `If-Range: <etag>` khi tải tiếp.
    /// Trả phản hồi đã kiểm mã: xem [`FileDownload::partial`] **trước** khi ghi (`false` ⇒ tệp đã đổi / server gửi cả
    /// tệp từ byte 0, cắt tệp `.part` về 0) rồi [`FileDownload::write_to`].
    pub async fn file_get_range(&self, id: &str, from: u64, if_range: Option<&str>) -> Result<FileDownload> {
        let mut rb = self.request(Method::GET, &file_path(id));
        if from > 0 {
            rb = rb.header(h::RANGE, format!("bytes={from}-"));
            if let Some(etag) = if_range {
                rb = rb.header(h::IF_RANGE, etag);
            }
        }
        let resp = self.send(rb).await?;
        let hs = resp.headers();
        let etag = header_str(hs, h::ETAG.as_str()).map(str::to_owned);
        let length = header_u64(hs, h::CONTENT_LENGTH.as_str());
        let file_name = header_str(hs, h::CONTENT_DISPOSITION.as_str()).and_then(disposition_file_name);
        if resp.status() == StatusCode::PARTIAL_CONTENT {
            let (start, total) = header_str(hs, h::CONTENT_RANGE.as_str())
                .and_then(parse_content_range)
                .ok_or_else(|| ClientError::InvalidResponse("206 without valid Content-Range".into()))?;
            Ok(FileDownload { resp, partial: true, start, total, length, etag, file_name })
        } else {
            Ok(FileDownload { resp, partial: false, start: 0, total: length, length, etag, file_name })
        }
    }
}

/// Phản hồi tải về đang chờ đọc thân.
#[derive(Debug)]
pub struct FileDownload {
    resp: Response,
    /// `206` — thân bắt đầu ở [`Self::start`]. `false` (`200`) ⇒ thân là cả tệp từ byte 0.
    pub partial: bool,
    pub start: u64,
    /// Cỡ cả tệp (nếu server cho biết).
    pub total: Option<u64>,
    /// Cỡ thân phản hồi.
    pub length: Option<u64>,
    /// Lưu lại để `If-Range` lần tải tiếp.
    pub etag: Option<String>,
    /// Từ `Content-Disposition` (`filename*` UTF-8, rồi `filename`) — chưa làm sạch, đừng ghép thẳng vào đường dẫn.
    pub file_name: Option<String>,
}

impl FileDownload {
    /// Ghi dần thân ra `writer` (không gom vào RAM), trả số byte đã ghi. Đứt ⇒ `Network` — byte đã ghi vẫn ở `writer`,
    /// tải tiếp từ cỡ tệp `.part`.
    pub async fn write_to<W: AsyncWrite + Unpin>(mut self, writer: &mut W) -> Result<u64> {
        let mut written = 0u64;
        while let Some(chunk) = self.resp.chunk().await.map_err(ClientError::network)? {
            writer.write_all(&chunk).await.map_err(ClientError::Local)?;
            written += chunk.len() as u64;
        }
        writer.flush().await.map_err(ClientError::Local)?;
        if let Some(len) = self.length
            && written != len
        {
            return Err(ClientError::Network(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "body shorter than Content-Length",
            )));
        }
        Ok(written)
    }
}

/// Host giả `<id>.vpsync.invalid`: `id` hợp lệ làm nhãn DNS (chữ thường, số, `-`, ≤ 63) thì dùng, không thì `phone`.
pub fn fake_host(device: Option<&DeviceId>) -> String {
    let label = device
        .map(|d| d.0.to_ascii_lowercase())
        .filter(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| "phone".into());
    format!("{label}{FAKE_HOST_SUFFIX}")
}

fn upload_path(uid: &str) -> String {
    format!("{}/{}", path::UPLOADS, encode_path_segment(uid))
}

fn file_path(id: &str) -> String {
    format!("{}{}", path::FILE, encode_path_segment(id))
}

/// 2xx ⇒ giữ nguyên; lỗi ⇒ log thân (cắt ngắn) rồi map theo mã.
async fn check(mut resp: Response) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let headers = resp.headers().clone();
    let mut body = Vec::new();
    while body.len() < ERROR_BODY_MAX {
        match resp.chunk().await {
            Ok(Some(c)) => body.extend_from_slice(&c),
            _ => break,
        }
    }
    body.truncate(ERROR_BODY_MAX);
    log::debug!("phone {} {}: {}", resp.url().path(), status.as_u16(), String::from_utf8_lossy(&body).trim());
    Err(ClientError::from_status(status, &headers))
}

async fn json<T: serde::de::DeserializeOwned>(resp: Response) -> Result<T> {
    let bytes = resp.bytes().await.map_err(ClientError::network)?;
    serde_json::from_slice(&bytes).map_err(|e| ClientError::InvalidResponse(format!("JSON: {e}")))
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim)
}

fn header_u64(headers: &HeaderMap, name: &str) -> Option<u64> {
    header_str(headers, name)?.parse().ok()
}

/// `Retry-After: <giây>` (dạng ngày HTTP không dùng trong giao thức).
fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    header_u64(headers, h::RETRY_AFTER.as_str()).map(Duration::from_secs)
}

/// `bytes a-b/total` ⇒ `(a, Some(total))`; `total` = `*` ⇒ `None`.
fn parse_content_range(v: &str) -> Option<(u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (start, _) = range.split_once('-')?;
    Some((start.trim().parse().ok()?, total.trim().parse().ok()))
}

/// `attachment; filename="a.mp4"; filename*=UTF-8''Chuy%E1%BA%BFn.mp4` ⇒ ưu tiên `filename*`.
fn disposition_file_name(v: &str) -> Option<String> {
    let mut plain = None;
    for part in v.split(';').map(str::trim) {
        if let Some(enc) = part.strip_prefix("filename*=") {
            let enc = enc.trim_matches('"');
            let value = enc.split_once("''").map_or(enc, |(_, val)| val);
            if let Some(name) = decode_header_value(value) {
                return Some(name);
            }
        } else if let Some(p) = part.strip_prefix("filename=") {
            plain = Some(p.trim_matches('"').to_owned());
        }
    }
    plain
}

/// Đọc đúng `left` byte; ghi lại lỗi đọc tệp để phân biệt với lỗi mạng (reqwest gói lỗi thân vào lỗi của nó).
struct ExactReader {
    inner: Pin<Box<dyn AsyncRead + Send>>,
    left: u64,
    error: Arc<Mutex<Option<io::Error>>>,
}

impl ExactReader {
    fn fail(&self, e: io::Error) -> io::Error {
        let copy = io::Error::new(e.kind(), e.to_string());
        *self.error.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
        copy
    }
}

impl AsyncRead for ExactReader {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if self.left == 0 {
            return Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        let room = buf.remaining() as u64;
        if room > self.left {
            // Đọc thẳng vào `buf` nhưng không quá `left`: vùng con có trần, rồi báo lại số byte đã điền.
            let max = self.left as usize;
            let mut sub = ReadBuf::new(buf.initialize_unfilled_to(max));
            let res = self.inner.as_mut().poll_read(cx, &mut sub);
            let n = sub.filled().len();
            buf.advance(n);
            return self.finish(res, n);
        }
        let res = self.inner.as_mut().poll_read(cx, buf);
        let n = buf.filled().len() - before;
        self.finish(res, n)
    }
}

impl ExactReader {
    fn finish(&mut self, res: Poll<io::Result<()>>, n: usize) -> Poll<io::Result<()>> {
        match res {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Err(self.fail(e))),
            Poll::Ready(Ok(())) if n == 0 => {
                let e = io::Error::new(io::ErrorKind::UnexpectedEof, "source file shorter than chunk");
                Poll::Ready(Err(self.fail(e)))
            }
            Poll::Ready(Ok(())) => {
                self.left -= n as u64;
                Poll::Ready(Ok(()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_host_label() {
        assert_eq!(fake_host(Some(&DeviceId("3F9A0c1d2b4e5a67".into()))), "3f9a0c1d2b4e5a67.vpsync.invalid");
        assert_eq!(fake_host(None), "phone.vpsync.invalid");
        assert_eq!(fake_host(Some(&DeviceId("a.b/c".into()))), "phone.vpsync.invalid");
    }

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range("bytes 100-199/1000"), Some((100, Some(1000))));
        assert_eq!(parse_content_range("bytes 5-9/*"), Some((5, None)));
        assert_eq!(parse_content_range("items 1-2/3"), None);
    }

    #[test]
    fn disposition() {
        assert_eq!(
            disposition_file_name("attachment; filename=\"Chuyen.mp4\"; filename*=UTF-8''Chuy%E1%BA%BFn.mp4")
                .as_deref(),
            Some("Chuyến.mp4")
        );
        assert_eq!(disposition_file_name("attachment; filename=\"a.mp4\"").as_deref(), Some("a.mp4"));
    }

    #[test]
    fn link_local_v6_with_scope_builds() {
        let addr: SocketAddr = "[fe80::1%1]:8080".parse().unwrap();
        let c = PhoneClient::new(addr, Some(&DeviceId("abc".into())), Some(Secret::new("t"))).unwrap();
        assert_eq!(c.base, "http://abc.vpsync.invalid:8080");
        assert!(!format!("{c:?}").contains("\"t\""));
    }
}
