//! Điện thoại giả trong tiến trình, đúng hợp đồng `PROTOCOL.md` mục 6 (v1 + W-3) — task gd1_02.
//!
//! HTTP/1.1 tự viết trên `tokio::net` (không thêm crate server): mỗi kết nối một request, luôn `Connection: close`.
//! Nghe `127.0.0.1` và `[::1]` (cùng cổng nếu được). Phiên upload trong RAM, giữ byte đã tới khi đứt giữa khúc.
//! Nút điều khiển cho test: đứt sau N byte, ép mã lỗi kế tiếp (`507`, `503`…), quên mọi token, "tắt server" (mất phiên),
//! dừng hẳn (không nhận kết nối), bỏ dữ liệu (đo RAM khi upload lớn), đổi `features`.
//!
//! Dùng chung cho test của task sau (`mod fake_phone;` trong `tests/<tên>.rs`).

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

pub const PIN: &str = "123456";
pub const ID: &str = "3f9a0c1d2b4e5a67";
pub const NAME: &str = "Fake Pixel";
/// Cần thêm ngoài cỡ tệp khi tạo upload (PROTOCOL 6.5).
pub const STORAGE_MARGIN: u64 = 16 * 1024 * 1024;
const MAX_SESSIONS: usize = 8;
const MAX_FAILS: u32 = 5;
const LOCK_SECS: u64 = 30;
const HEADER_MAX: usize = 16 * 1024;
const READ_BUF: usize = 64 * 1024;

/// Họ địa chỉ để chạy cùng một test qua IPv4 và IPv6 (C-W3a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
}

#[derive(Debug, Clone)]
pub struct StoredFile {
    pub id: String,
    pub name: String,
    pub dir: Option<String>,
    pub data: Arc<Vec<u8>>,
    pub size: u64,
    pub modified: i64,
    pub version: u32,
}

impl StoredFile {
    fn etag(&self) -> String {
        format!("\"{}-{}\"", self.id, self.version)
    }

    fn json(&self) -> serde_json::Value {
        json!({"id": self.id, "name": self.name, "size": self.size, "modified": self.modified,
               "mime": "video/mp4", "dir": self.dir})
    }
}

#[derive(Debug)]
struct Session {
    name: String,
    dir: String,
    length: u64,
    offset: u64,
    data: Vec<u8>,
    busy: bool,
}

#[derive(Debug)]
struct State {
    /// `client_id` ⇒ token.
    tokens: HashMap<String, String>,
    fails: u32,
    locked_until: Option<Instant>,
    sessions: HashMap<String, Session>,
    files: Vec<StoredFile>,
    next_id: u64,
    clock: i64,
    free_space: u64,
    features: Vec<String>,
    legacy_info: bool,
    discard_data: bool,
    cut_after: Option<u64>,
    fail_next: Option<(u16, Option<u64>)>,
    port: u16,
}

impl State {
    fn token_ok(&self, req: &Request) -> bool {
        let Some(auth) = req.header("authorization") else { return false };
        let Some(t) = auth.strip_prefix("Bearer ") else { return false };
        self.tokens.values().any(|v| v == t.trim())
    }

    fn used(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum::<u64>() + self.sessions.values().map(|s| s.length).sum::<u64>()
    }

    fn new_id(&mut self) -> String {
        self.next_id += 1;
        format!("f{:04}", self.next_id)
    }

    fn unique_name(&self, dir: &str, name: &str) -> String {
        let taken = |n: &str| self.files.iter().any(|f| f.dir.as_deref() == Some(dir) && f.name == n);
        if !taken(name) {
            return name.to_owned();
        }
        let (stem, ext) = name.rsplit_once('.').map_or((name, ""), |(s, e)| (s, e));
        (1..).map(|i| format!("{stem} ({i}).{ext}")).find(|n| !taken(n)).unwrap()
    }
}

pub struct FakePhone {
    state: Arc<Mutex<State>>,
    v4: SocketAddr,
    v6: Option<SocketAddr>,
    tasks: Vec<JoinHandle<()>>,
    connections: Arc<AtomicU64>,
}

impl Drop for FakePhone {
    fn drop(&mut self) {
        self.stop();
    }
}

impl FakePhone {
    pub async fn start() -> Self {
        let (l4, l6) = bind_pair().await;
        let v4 = l4.local_addr().unwrap();
        let v6 = l6.as_ref().map(|l| l.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            tokens: HashMap::new(),
            fails: 0,
            locked_until: None,
            sessions: HashMap::new(),
            files: Vec::new(),
            next_id: 0,
            clock: 1_762_761_600_000,
            free_space: u64::MAX / 2,
            features: vec!["ipv6".into(), "dirs".into()],
            legacy_info: false,
            discard_data: false,
            cut_after: None,
            fail_next: None,
            port: v4.port(),
        }));
        let connections = Arc::new(AtomicU64::new(0));
        let mut tasks = vec![tokio::spawn(accept_loop(l4, state.clone(), connections.clone()))];
        if let Some(l6) = l6 {
            tasks.push(tokio::spawn(accept_loop(l6, state.clone(), connections.clone())));
        }
        Self { state, v4, v6, tasks, connections }
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    pub fn v4(&self) -> SocketAddr {
        self.v4
    }

    /// `None` khi máy không có IPv6 loopback.
    pub fn v6(&self) -> Option<SocketAddr> {
        self.v6
    }

    pub fn addr(&self, family: Family) -> Option<SocketAddr> {
        match family {
            Family::V4 => Some(self.v4),
            Family::V6 => self.v6,
        }
    }

    /// Số kết nối TCP đã nhận (kiểm "mỗi request một kết nối").
    pub fn connections(&self) -> u64 {
        self.connections.load(Ordering::SeqCst)
    }

    /// Đăng ký thẳng một token (bỏ qua bước PIN) — cho test không quan tâm ghép.
    pub fn issue_token(&self, client_id: &str) -> String {
        let t = random_token();
        self.st().tokens.insert(client_id.into(), t.clone());
        t
    }

    /// "Quên các máy tính đã ghép" — mọi token chết ⇒ `401`.
    pub fn forget_tokens(&self) {
        self.st().tokens.clear();
    }

    /// Tắt rồi bật Sync WiFi: mất mọi phiên upload dở (token lâu dài còn), khoá PIN reset.
    pub fn restart(&self) {
        let mut s = self.st();
        s.sessions.clear();
        s.fails = 0;
        s.locked_until = None;
    }

    /// Dừng hẳn: không nhận kết nối mới (client thấy nối hỏng).
    pub fn stop(&self) {
        for t in &self.tasks {
            t.abort();
        }
    }

    /// Request `PATCH` kế tiếp: nhận đúng `n` byte thân rồi cắt kết nối (không phản hồi).
    pub fn cut_next_patch_after(&self, n: u64) {
        self.st().cut_after = Some(n);
    }

    /// Request kế tiếp (trừ `info`) trả `status` (+ `Retry-After`).
    pub fn fail_next(&self, status: u16, retry_after: Option<u64>) {
        self.st().fail_next = Some((status, retry_after));
    }

    pub fn set_free_space(&self, bytes: u64) {
        self.st().free_space = bytes;
    }

    pub fn set_features(&self, features: &[&str]) {
        self.st().features = features.iter().map(|s| s.to_string()).collect();
    }

    /// `info` kiểu điện thoại cũ: không `port`, `addresses`, `features`.
    pub fn set_legacy_info(&self, on: bool) {
        self.st().legacy_info = on;
    }

    /// Không giữ byte upload (chỉ đếm offset) — upload lớn không tốn RAM của tiến trình test.
    pub fn set_discard_data(&self, on: bool) {
        self.st().discard_data = on;
    }

    pub fn sessions(&self) -> usize {
        self.st().sessions.len()
    }

    pub fn files(&self) -> Vec<StoredFile> {
        self.st().files.clone()
    }

    pub fn add_file(&self, name: &str, dir: Option<&str>, data: Vec<u8>) -> String {
        let mut s = self.st();
        let id = s.new_id();
        s.clock += 1000;
        let file = StoredFile {
            id: id.clone(),
            name: name.into(),
            dir: dir.map(str::to_owned),
            size: data.len() as u64,
            data: Arc::new(data),
            modified: s.clock,
            version: 1,
        };
        s.files.insert(0, file);
        id
    }

    /// Ghi đè nội dung tệp ⇒ `ETag` đổi.
    pub fn replace_file(&self, id: &str, data: Vec<u8>) {
        let mut s = self.st();
        let f = s.files.iter_mut().find(|f| f.id == id).unwrap();
        f.size = data.len() as u64;
        f.data = Arc::new(data);
        f.version += 1;
    }
}

/// Listener phụ dùng chung trạng thái điện thoại (gd1_04 C7: "đổi cổng / IP giữa chừng"). Bỏ ⇒ ngừng nghe.
pub struct ExtraListener {
    pub addr: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for ExtraListener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakePhone {
    /// Nghe thêm ở một cổng mới (cùng phiên upload, token, thư viện). Kết hợp [`FakePhone::stop`] để địa chỉ cũ chết.
    pub async fn listen_extra(&self, family: Family) -> Option<ExtraListener> {
        let ip: IpAddr = match family {
            Family::V4 => Ipv4Addr::LOCALHOST.into(),
            Family::V6 => Ipv6Addr::LOCALHOST.into(),
        };
        let l = TcpListener::bind((ip, 0)).await.ok()?;
        let addr = l.local_addr().ok()?;
        let task = tokio::spawn(accept_loop(l, self.state.clone(), self.connections.clone()));
        Some(ExtraListener { addr, task })
    }

    /// Phiên upload đang mở: `(uid, offset)` — kiểm "không tạo phiên mới" khi đổi địa chỉ.
    pub fn uploads_in_progress(&self) -> Vec<(String, u64)> {
        self.st().sessions.iter().map(|(k, v)| (k.clone(), v.offset)).collect()
    }
}

async fn bind_pair() -> (TcpListener, Option<TcpListener>) {
    for _ in 0..20 {
        let l4 = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = l4.local_addr().unwrap().port();
        if let Ok(l6) = TcpListener::bind((Ipv6Addr::LOCALHOST, port)).await {
            return (l4, Some(l6));
        }
    }
    let l4 = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let l6 = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).await.ok();
    (l4, l6)
}

async fn accept_loop(listener: TcpListener, state: Arc<Mutex<State>>, conns: Arc<AtomicU64>) {
    while let Ok((sock, _)) = listener.accept().await {
        conns.fetch_add(1, Ordering::SeqCst);
        tokio::spawn(handle(sock, state.clone()));
    }
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    /// Byte thân đã đọc lẫn cùng header.
    rest: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn header_u64(&self, name: &str) -> Option<u64> {
        self.header(name)?.trim().parse().ok()
    }
}

async fn read_request(sock: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        if buf.len() > HEADER_MAX {
            return None;
        }
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = std::str::from_utf8(&buf[..end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let path = first.next()?.to_owned();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    Some(Request { method, path, headers, rest: buf[end + 4..].to_vec() })
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// `HEAD`: gửi `Content-Length` của thân nhưng không gửi thân.
    head_only: bool,
}

impl Reply {
    fn status(status: u16) -> Self {
        Self { status, headers: Vec::new(), body: format!("error {status}").into_bytes(), head_only: false }
    }

    fn empty(status: u16) -> Self {
        Self { status, headers: Vec::new(), body: Vec::new(), head_only: false }
    }

    fn json(status: u16, v: serde_json::Value) -> Self {
        let mut r = Self::empty(status);
        r.headers.push(("Content-Type".into(), "application/json; charset=utf-8".into()));
        r.body = serde_json::to_vec(&v).unwrap();
        r
    }

    fn with(mut self, k: &str, v: impl ToString) -> Self {
        self.headers.push((k.into(), v.to_string()));
        self
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        206 => "Partial Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        423 => "Locked",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        507 => "Insufficient Storage",
        _ => "Status",
    }
}

async fn send(sock: &mut TcpStream, r: Reply) {
    let mut head = format!("HTTP/1.1 {} {}\r\nConnection: close\r\n", r.status, reason(r.status));
    let has_len = r.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-length"));
    for (k, v) in &r.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if !has_len && r.status != 204 {
        head.push_str(&format!("Content-Length: {}\r\n", r.body.len()));
    }
    head.push_str("\r\n");
    let _ = sock.write_all(head.as_bytes()).await;
    if !r.head_only && r.status != 204 {
        let _ = sock.write_all(&r.body).await;
    }
    let _ = sock.flush().await;
    // Đóng chiều ghi rồi đọc bỏ phần thân client còn gửi (vd `409` trước khi đọc thân `PATCH`) — đóng khi còn byte
    // chưa đọc làm OS gửi RST, client mất phản hồi.
    let _ = sock.shutdown().await;
    let mut sink = vec![0u8; READ_BUF];
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while let Ok(n) = sock.read(&mut sink).await {
            if n == 0 {
                break;
            }
        }
    })
    .await;
}

async fn handle(mut sock: TcpStream, state: Arc<Mutex<State>>) {
    let Some(req) = read_request(&mut sock).await else { return };
    let path = req.path.split('?').next().unwrap_or("").to_owned();

    if path == "/api/v1/info" && req.method == "GET" {
        let r = info(&state.lock().unwrap(), &req, sock.local_addr().ok());
        return send(&mut sock, r).await;
    }
    let forced = state.lock().unwrap().fail_next.take();
    if let Some((status, retry)) = forced {
        let mut r = Reply::status(status);
        if let Some(s) = retry {
            r = r.with("Retry-After", s);
        }
        return send(&mut sock, r).await;
    }

    let reply = match (req.method.as_str(), path.as_str()) {
        ("POST", "/api/v1/pair") => pair(&mut sock, &state, &req).await,
        (_, p) if p.starts_with("/api/v1/") && !authorized(&state, &req) => Reply::status(401),
        ("DELETE", "/api/v1/pair") => {
            let mut s = state.lock().unwrap();
            let token = req.header("authorization").unwrap_or("").trim_start_matches("Bearer ").trim().to_owned();
            s.tokens.retain(|_, v| *v != token);
            Reply::empty(204)
        }
        ("GET", "/api/v1/list") => {
            let s = state.lock().unwrap();
            Reply::json(200, json!({"items": s.files.iter().map(StoredFile::json).collect::<Vec<_>>()}))
        }
        ("POST", "/api/v1/uploads") => create_upload(&state, &req),
        (m, p) if p.starts_with("/api/v1/uploads/") => {
            let uid = decode(&p["/api/v1/uploads/".len()..]);
            match m {
                "PATCH" => match patch(&mut sock, &state, &req, &uid).await {
                    Some(r) => r,
                    None => return, // đứt có chủ ý (`cut_next_patch_after`) hoặc client đứt
                },
                "HEAD" => {
                    let s = state.lock().unwrap();
                    match s.sessions.get(&uid) {
                        Some(sess) => Reply::empty(200)
                            .with("Upload-Offset", sess.offset)
                            .with("Upload-Length", sess.length)
                            .with("Cache-Control", "no-store")
                            .with("Content-Length", 0),
                        None => Reply::status(404),
                    }
                }
                "DELETE" => {
                    let removed = state.lock().unwrap().sessions.remove(&uid).is_some();
                    Reply::empty(if removed { 204 } else { 404 })
                }
                _ => Reply::status(405),
            }
        }
        (m @ ("GET" | "HEAD"), p) if p.starts_with("/api/v1/file/") => {
            let id = decode(&p["/api/v1/file/".len()..]);
            let file = state.lock().unwrap().files.iter().find(|f| f.id == id).cloned();
            match file {
                Some(f) => file_reply(&f, &req, m == "HEAD"),
                None => Reply::status(404),
            }
        }
        _ => Reply::status(404),
    };
    send(&mut sock, reply).await;
}

fn authorized(state: &Mutex<State>, req: &Request) -> bool {
    state.lock().unwrap().token_ok(req)
}

fn info(s: &State, req: &Request, local: Option<SocketAddr>) -> Reply {
    let mut v = json!({"app": "xmobile.videoplayer", "api": 1, "id": ID, "name": NAME, "paired": s.token_ok(req)});
    if !s.legacy_info {
        v["port"] = json!(local.map_or(s.port, |a| a.port()));
        v["addresses"] = json!(["127.0.0.1", "::1"]);
        v["features"] = json!(s.features);
    }
    Reply::json(200, v)
}

async fn read_body(sock: &mut TcpStream, req: &Request, max: usize) -> Result<Vec<u8>, u16> {
    let Some(len) = req.header_u64("content-length") else { return Err(411) };
    if len as usize > max {
        return Err(413);
    }
    let mut body = req.rest.clone();
    let mut tmp = [0u8; 256];
    while body.len() < len as usize {
        let n = sock.read(&mut tmp).await.map_err(|_| 400u16)?;
        if n == 0 {
            return Err(400);
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(len as usize);
    Ok(body)
}

async fn pair(sock: &mut TcpStream, state: &Mutex<State>, req: &Request) -> Reply {
    let client_id = req.header("x-client-id").unwrap_or("").to_owned();
    let id_ok = (8..=64).contains(&client_id.len())
        && client_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !id_ok {
        return Reply::status(400);
    }
    if let Some(name) = req.header("x-client-name") {
        match decode_opt(name) {
            Some(n) if n.chars().count() <= 64 => {}
            _ => return Reply::status(400),
        }
    }
    let body = match read_body(sock, req, 64).await {
        Ok(b) => b,
        Err(code) => return Reply::status(code),
    };
    let mut s = state.lock().unwrap();
    if let Some(until) = s.locked_until {
        let now = Instant::now();
        if until > now {
            return Reply::status(429).with("Retry-After", (until - now).as_secs().max(1));
        }
        s.locked_until = None;
    }
    if String::from_utf8_lossy(&body).trim() != PIN {
        s.fails += 1;
        if s.fails >= MAX_FAILS {
            s.fails = 0;
            s.locked_until = Some(Instant::now() + Duration::from_secs(LOCK_SECS));
            return Reply::status(429).with("Retry-After", LOCK_SECS);
        }
        return Reply::status(403);
    }
    s.fails = 0;
    let token = random_token();
    s.tokens.insert(client_id, token.clone());
    Reply::json(200, json!({"token": token, "id": ID, "name": NAME}))
}

fn create_upload(state: &Mutex<State>, req: &Request) -> Reply {
    let Some(name) = req.header("x-file-name").and_then(decode_opt) else { return Reply::status(400) };
    let name = name.rsplit(['/', '\\']).next().unwrap_or("").to_owned();
    let Some(length) = req.header_u64("upload-length").filter(|l| *l > 0) else { return Reply::status(400) };
    let dir = match req.header("x-file-dir") {
        None => String::new(),
        Some(raw) => match decode_opt(raw).and_then(|d| sanitize_dir(&d)) {
            Some(d) => d,
            None => return Reply::status(400),
        },
    };
    if !is_video(&name) {
        return Reply::status(415);
    }
    let mut s = state.lock().unwrap();
    if s.sessions.len() >= MAX_SESSIONS {
        return Reply::status(503).with("Retry-After", 2);
    }
    if s.used() + length + STORAGE_MARGIN > s.free_space {
        return Reply::status(507);
    }
    let uid = uuid::Uuid::new_v4().simple().to_string();
    s.sessions
        .insert(uid.clone(), Session { name, dir: dir.clone(), length, offset: 0, data: Vec::new(), busy: false });
    Reply::json(201, json!({"id": uid, "offset": 0, "length": length, "dir": dir}))
        .with("Location", format!("/api/v1/uploads/{uid}"))
}

/// `None` = không phản hồi (kết nối đứt).
async fn patch(sock: &mut TcpStream, state: &Mutex<State>, req: &Request, uid: &str) -> Option<Reply> {
    let Some(offset) = req.header_u64("upload-offset") else { return Some(Reply::status(400)) };
    let Some(len) = req.header_u64("content-length").filter(|l| *l > 0) else { return Some(Reply::status(400)) };
    let cut = {
        let mut s = state.lock().unwrap();
        let Some(sess) = s.sessions.get_mut(uid) else { return Some(Reply::status(404)) };
        if sess.busy {
            return Some(Reply::status(423));
        }
        if offset != sess.offset {
            return Some(Reply::status(409).with("Upload-Offset", sess.offset));
        }
        if offset + len > sess.length {
            return Some(Reply::status(413));
        }
        sess.busy = true;
        s.cut_after.take()
    };
    let discard = state.lock().unwrap().discard_data;

    // Nhận thân, ghi vào phiên ngay từng mẩu (đứt giữa chừng vẫn giữ byte đã tới).
    let mut received = 0u64;
    let limit = cut.map_or(len, |c| c.min(len));
    let mut pending = req.rest.clone();
    let mut buf = vec![0u8; READ_BUF];
    let mut broken = false;
    while received < limit {
        let chunk: &[u8] = if !pending.is_empty() {
            &pending
        } else {
            match sock.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    broken = true;
                    break;
                }
                Ok(n) => &buf[..n],
            }
        };
        let take = chunk.len().min((limit - received) as usize);
        {
            let mut s = state.lock().unwrap();
            let sess = s.sessions.get_mut(uid)?;
            if !discard {
                sess.data.extend_from_slice(&chunk[..take]);
            }
            sess.offset += take as u64;
        }
        received += take as u64;
        pending.clear();
    }

    let mut s = state.lock().unwrap();
    let sess = s.sessions.get_mut(uid)?;
    sess.busy = false;
    if broken || received < len {
        // Cắt có chủ ý / client đứt: đóng kết nối không phản hồi (drop socket).
        return None;
    }
    if sess.offset < sess.length {
        return Some(Reply::empty(204).with("Upload-Offset", sess.offset));
    }
    let sess = s.sessions.remove(uid).unwrap();
    let id = s.new_id();
    s.clock += 1000;
    let name = s.unique_name(&sess.dir, &sess.name);
    let file = StoredFile {
        id,
        name,
        dir: Some(sess.dir),
        size: sess.length,
        data: Arc::new(sess.data),
        modified: s.clock,
        version: 1,
    };
    let body = file.json();
    s.files.insert(0, file);
    Some(Reply::json(201, body))
}

fn file_reply(f: &StoredFile, req: &Request, head_only: bool) -> Reply {
    let size = f.size;
    let etag = f.etag();
    let disposition = format!(
        "attachment; filename=\"{}\"; filename*=UTF-8''{}",
        f.name.replace(|c: char| !c.is_ascii() || c == '"', "_"),
        utf8_percent_encode(&f.name, ENCODE)
    );
    let range = req.header("range").and_then(|r| parse_range(r, size));
    let range_ok = req.header("if-range").is_none_or(|v| v == etag);
    let mut r = match (req.header("range"), range, range_ok) {
        (Some(_), None, true) => return Reply::status(416).with("Content-Range", format!("bytes */{size}")),
        (Some(_), Some((a, b)), true) => {
            let mut r = Reply::empty(206).with("Content-Range", format!("bytes {a}-{b}/{size}"));
            r.body = f.data[a as usize..=b as usize].to_vec();
            r
        }
        _ => {
            let mut r = Reply::empty(200);
            r.body = f.data.to_vec();
            r
        }
    };
    r = r
        .with("Content-Type", "video/mp4")
        .with("ETag", etag)
        .with("Accept-Ranges", "bytes")
        .with("Content-Disposition", disposition);
    if head_only {
        let len = r.body.len();
        r = r.with("Content-Length", len);
        r.head_only = true;
    }
    r
}

/// `bytes=a-` / `bytes=a-b` (một khoảng).
fn parse_range(v: &str, size: u64) -> Option<(u64, u64)> {
    let spec = v.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split_once('-')?;
    let a: u64 = a.trim().parse().ok()?;
    let b: u64 = if b.trim().is_empty() { size.checked_sub(1)? } else { b.trim().parse::<u64>().ok()?.min(size - 1) };
    (a < size && a <= b).then_some((a, b))
}

/// Làm sạch `X-File-Dir` như điện thoại (PROTOCOL 6.9). `None` ⇒ `400`.
fn sanitize_dir(raw: &str) -> Option<String> {
    let mut out = Vec::new();
    for seg in raw.split(['/', '\\']) {
        if seg == ".." {
            return None;
        }
        let cleaned: String =
            seg.chars().map(|c| if c.is_control() || "\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
        let cleaned = cleaned.trim().trim_start_matches('.').trim().to_owned();
        if cleaned.is_empty() {
            continue;
        }
        out.push(cleaned);
    }
    let joined = out.join("/");
    (out.len() <= 8 && joined.chars().count() <= 255).then_some(joined)
}

fn is_video(name: &str) -> bool {
    mws_core::protocol::is_video_name(name)
}

const ENCODE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

fn decode_opt(v: &str) -> Option<String> {
    percent_decode_str(v).decode_utf8().ok().map(|s| s.into_owned())
}

fn decode(v: &str) -> String {
    decode_opt(v).unwrap_or_default()
}

fn random_token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// Địa chỉ loopback theo họ — test dùng khi cần dựng `SocketAddr` khác cổng.
pub fn loopback(family: Family, port: u16) -> SocketAddr {
    match family {
        Family::V4 => SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        Family::V6 => SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
    }
}
