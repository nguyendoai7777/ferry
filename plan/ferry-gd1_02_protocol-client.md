# ferry-gd1_02 — protocol-client

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ (11/11/2026) |
| Repo | ferry |
| Model | opus |
| Cỡ | M — ~700 dòng + test |
| Phụ thuộc | gd1_01 |
| Quyết định áp dụng | W-1, W-3, D-2 |
| Phụ thuộc mới | `tokio`, `reqwest` (`default-features = false`, `stream`; không TLS) — MIT/Apache; test: `hyper` server hoặc `tokio` TCP thô cho server giả |
| File đụng tới | MỚI `crates/core/src/{protocol,client}.rs`, `crates/core/tests/fake_phone/mod.rs`, `crates/core/tests/client.rs` |

## 0. Bàn giao từ gd1_01 (11/11/2026)

<!-- handoff: ferry-gd1_01 -->

- **Đã có sẵn**: `ferry-core` khai sẵn mọi module (tệp rỗng ghi task chủ) — chỉ thêm mã vào tệp của mình, không sửa `lib.rs`.
  `queue` và `transfer` là thư mục (`queue/mod.rs`, `transfer/mod.rs`).
- **Kiểu dùng chung W-3**: `device::DeviceId` (chuỗi trong suốt), `config::PairedDevice { id, name, addresses: Vec<String> }`
  (địa chỉ dạng chuỗi giữ `%iface`), `queue::BatchId::new()` (UUID v4). Dùng lại, đừng định nghĩa song song.
- **Phụ thuộc đã có trong `ferry-core`**: `serde`, `serde_json`, `uuid` (v4), `log`, `dirs`, `toml`, `interprocess`,
  `gethostname`, `sys-locale`; dev: `tempfile`. Thêm crate ⇒ khai ở `crates/core/Cargo.toml` (crate dùng chung nhiều
  crate thì vào `[workspace.dependencies]`), soát `cargo deny check licenses`.
- **Đường dẫn**: `paths::data_dir()` (ghi đè `FERRY_DATA_DIR`) cho SQLite / tệp tạm; test dùng `tempfile`, không đụng thư mục thật.
- **Soát lại khi làm**: build ở `<gốc>/ferry/target/w<N>`; đổi mã theo OS ⇒ thêm clippy `--target
  x86_64-pc-windows-msvc` / `x86_64-unknown-linux-gnu` (lệnh ở `CLAUDE.md` mục Lệnh).
- **Bẫy**: socket Unix ≤ 104 byte (đã tự xử lý trong `ipc`); số RAM trên macOS đo bằng `footprint`, không RSS.

## 1. Làm gì
- `protocol`: kiểu JSON của PROTOCOL 6.3–6.5 (`Info`, `PairResponse`, `LibraryItem`, `UploadCreated`), header, đuôi video 6.6 (`is_video_name`), mã hoá tên tệp percent-encoding UTF-8.
- `client::PhoneClient { base: SocketAddr, token: Option<Secret> }`: `info`, `pair(pin, client_id, client_name)`, `unpair`, `list`, `upload_create`, `upload_patch(uid, offset, reader, len)` stream từ tệp, `upload_head`, `upload_delete`, `file_head`, `file_get_range` stream ra writer.
- Mỗi request một kết nối (pool 0), timeout đọc 60 s, không đọc cả tệp vào RAM.
- Mọi phản hồi map vào `ClientError` theo **mã trạng thái** (`Unauthorized`, `NotFound`, `OffsetMismatch(u64)`, `Locked`, `TooLarge`, `Unsupported`, `InsufficientStorage`, `Busy { retry_after }`, `RateLimited { retry_after }`, `WrongPin`, `Network(io)`, `Server(u16)`), thân lỗi chỉ vào log.
- Server giả `fake_phone` trong tiến trình theo đúng §6 (PIN, token, phiên upload trong RAM, giữ byte đã tới khi đứt giữa khúc, `Connection: close`), có nút điều khiển: đứt sau N byte, trả `507`, `503`, quên mọi token, tắt server (mất phiên).
- Kiểu §6 bổ sung W-3: `Info` có `port`, `addresses`, `features` (thiếu = rỗng); `ListItem.dir: Option<String>` (thiếu = `""`, `null` = ngoài nơi lưu); `create_upload(name, length, dir: Option<&str>)` gửi `X-File-Dir` URL-encoded (PROTOCOL 6.5, 6.9); phản hồi có `dir`.
- **Không ghép IP vào URL** (PROTOCOL 6.8 bẫy): client nhận `SocketAddr` (v4 / v6 / v6 + `scope_id`), dựng URL với host giả `<id>.vpsync.invalid` + `ClientBuilder::resolve`. `fake_phone` nghe cả `127.0.0.1` và `[::1]`.

## 2. Không làm trong task này
- Không máy trạng thái hàng đợi / retry (03, 04). Không mDNS (05). Không kho khoá (06).

## 6. Nghiệm thu
- C1. info / pair đúng PIN / sai PIN (`403`) / 5 lần sai (`429` + `Retry-After`) với server giả. `[opus-unit]`
- C2. Upload 3 khúc ⇒ `204, 204, 201` + `LibraryItem` có `name` server đặt. `[opus-unit]`
- C3. Đứt giữa khúc ⇒ `upload_head` trả offset đã tới; `PATCH` sai offset ⇒ `OffsetMismatch(đúng)`. `[opus-unit]`
- C4. Tải về `Range` + `If-Range` khớp ⇒ `206`; ETag đổi ⇒ `200` toàn tệp. `[opus-unit]`
- C5. Upload 1 GB tệp tạm: RAM tiến trình test không tăng quá ~2 khúc. `[opus-run]`
- C6. `curl` mẫu ở PROTOCOL mục 7 chạy được với server giả (hợp đồng khớp). `[opus-run]`
- C-W3a. Cùng một bộ test chạy qua `127.0.0.1` và `[::1]`; `SocketAddrV6` có `scope_id` dựng được request (không panic `url`). `[opus-unit]`
- C-W3b. `X-File-Dir` mã hoá đúng tên tiếng Việt + `/`; `info` thiếu `features`/`addresses` (điện thoại cũ) vẫn parse. `[opus-unit]`

## 8. Nợ chuyển task khác

- Không còn nợ trong phạm vi. `fake_phone` chưa có: đổi cổng / chỉ nghe `[::1]` (gd1_04 C7, gd1_05 C-W3b tự thêm nút vào
  `tests/fake_phone/mod.rs`), `423` thật do hai PATCH chồng (hiện ép bằng `fail_next(423)`).

## 9. Bàn giao thực tế

**Đã làm (11/11/2026, Opus)** — nhánh `claude/gd1_02-protocol-client`, worktree `ferry-w1`, build `target/w1`.
- `protocol.rs`: `API_VERSION`, hằng `path::*`, `header::*`, `feature::{DIRS, IPV6}`; `Info` (W-3: `port: Option<u16>`,
  `addresses`, `features` mặc định rỗng, `has_feature`), `PairResponse`, `LibraryItem` (tên này thay cả `ListItem` của spec;
  `dir: Option<String>` — vắng ⇒ `Some("")`, `null` ⇒ `None`), `ListResponse`, `UploadCreated { id, offset, length, dir }`;
  `is_video_name` (§6.6), `encode_header_value` / `decode_header_value` (percent-encoding mọi byte ngoài "unreserved", kể cả
  `+`, khoảng trắng, `/` ⇒ phía giải mã kiểu form vẫn đúng), `encode_path_segment`, `normalize_dir` (`\` ⇒ `/`).
  `Secret` (token / PIN) ở đây: `Debug` che, không `Display` / `Serialize`, `expose()` lấy giá trị.
- `client.rs`: `PhoneClient::new(addr: SocketAddr, device: Option<&DeviceId>, token: Option<Secret>)`, `set_token`,
  `info`, `pair(&Secret, client_id, client_name)` (tên cắt 64 ký tự, URL-encode), `unpair`, `list`,
  `upload_create(name, length, dir: Option<&str>)` (tên spec `create_upload` đổi theo nhóm `upload_*`), `upload_patch(uid,
  offset, reader: AsyncRead, len) -> PatchOutcome::{Partial(offset), Done(LibraryItem)}`, `upload_head -> UploadStatus`,
  `upload_delete`, `file_head -> FileMeta { length, etag, mime, file_name }`, `file_get_range(id, from, if_range) ->
  FileDownload { partial, start, total, length, etag }` rồi `write_to(writer)` (hai bước: tầng trên xem `partial` để cắt
  `.part` về 0 trước khi ghi). Host giả `<id>.vpsync.invalid` (`fake_host`; id không hợp lệ làm nhãn DNS / chưa biết ⇒
  `phone.vpsync.invalid`) + `resolve(host, addr)`, cổng nằm trong URL. `no_proxy`, `pool_max_idle_per_host(0)`,
  `Connection: close`, `connect_timeout` 10 s, `read_timeout` 60 s. Thân PATCH: `ReaderStream` 64 KB trên bộ đọc đúng `len`
  byte (`ExactReader`).
- `ClientError` theo mã: `BadRequest` 400, `Unauthorized` 401, `WrongPin` 403, `NotFound` 404, `OffsetMismatch(u64)` 409,
  `TooLarge` 411/413, `Unsupported` 415, `RangeNotSatisfiable` 416, `Locked` 423, `RateLimited { retry_after }` 429,
  `Busy { retry_after }` 503, `InsufficientStorage` 507, `Server(u16)`, `Network(io::Error)` (`kind`: `ConnectionRefused`
  nối hỏng, `TimedOut`, `Other` đứt giữa chừng), thêm `Local(io::Error)` (đọc tệp nguồn / ghi tệp đích hỏng, tệp nguồn ngắn
  hơn khúc ⇒ `UnexpectedEof` — thử lại vô ích, runner nên Failed) và `InvalidResponse(String)` (JSON hỏng, thiếu header).
  Thân lỗi: `log::debug!` ≤ 1 KB, không bao giờ log token.
- `tests/fake_phone/mod.rs`: HTTP/1.1 tự viết trên `tokio::net` (không thêm crate server), nghe `127.0.0.1` + `[::1]` cùng
  cổng; PIN `123456`, khoá 30 s sau 5 lần sai (lần sai thứ 5 trả `429`); phiên upload trong RAM giữ byte đã tới khi đứt;
  làm sạch `X-File-Dir` như §6.9; đổi tên `a (1).mp4` khi trùng; `Range` / `If-Range` / `416`; đếm kết nối. Nút:
  `issue_token`, `forget_tokens`, `restart` (mất phiên), `stop`, `cut_next_patch_after(n)`, `fail_next(status, retry_after)`,
  `set_free_space`, `set_features`, `set_legacy_info`, `set_discard_data`, `add_file`, `replace_file` (ETag đổi), `files`,
  `sessions`, `connections`. Phản hồi lỗi sớm (409 trước khi đọc thân) đóng chiều ghi rồi đọc bỏ phần thân còn lại — đóng
  khi còn byte chưa đọc làm OS gửi RST, client mất phản hồi.
- `tests/client.rs` (11 ca, mỗi ca qua `127.0.0.1` và `[::1]`), `tests/real_phone.rs` (ignored, app thật qua env).
- Phụ thuộc mới ở `crates/core/Cargo.toml` (không đụng `[workspace.dependencies]`): `reqwest 0.13` (`default-features =
  false`, `stream` — không TLS, HTTP/2, proxy hệ thống), `tokio` (`io-util`), `tokio-util` (`io`), `percent-encoding`; dev:
  `tokio` (`macros`, `net`, `process`, `rt-multi-thread`, `fs`, `sync`, `time`). `cargo deny check licenses` ✓.

**Nghiệm thu**: C1 ✓ C2 ✓ C3 ✓ C4 ✓ C-W3a ✓ (`::1` qua `SocketAddrV6` scope 0; `fe80::1234` scope 1 dựng request, không panic)
C-W3b ✓ (`Chuyến đi\Đà Lạt/ngày 1` ⇒ `Chuyến đi/Đà Lạt/ngày 1`; `info` thiếu trường parse). C6 ✓ — 4 lệnh `curl` của PROTOCOL
mục 7 chạy với server giả (test tự bỏ qua khi máy không có `curl`). C5 ✓ (`--release -- --ignored c5`, macOS arm64): upload
1 GB tệp thưa, khúc 32 MB, RSS 7,7 → đỉnh 9,8 MB (+2,1 MB), 3,7 GB/s qua loopback.

**Thử với app Android thật** (emulator `Medium_Phone`, `adb forward tcp:18080 tcp:8080`, master xmobile): `real_phone_round_trip`
xanh — info (`features: ipv6, dirs`), sai PIN `403`, ghép, `list`, tạo với `X-File-Dir` tiếng Việt, `204`, PATCH sai offset ⇒
`409` + offset đúng (điện thoại trả lỗi sớm vẫn tới client), HEAD, `201` + `dir`, `HEAD file` + `Range` 206 khớp byte,
If-Range lệch ⇒ 200, `.mp3` ⇒ `415`, `a/../b` ⇒ `400`, huỷ phiên ⇒ HEAD `404`, `unpair` ⇒ `401`. Để lại trên emulator tệp
`ferry-gd1_02/Thử nghiệm/gd1_02 thử + nghiệm.mp4` (300 KB). `adb forward` không nghe `[::1]` ⇒ IPv6 thật chưa thử (cần điện
thoại thật cùng WiFi — gd1_05). Điện thoại trả `id` thư viện chứa `%2F` và khoảng trắng nguyên văn ⇒ `encode_path_segment`
mã hoá `%` thành `%25` là đúng (đã qua `HEAD file` / `GET`).

**Opus chọn 11/11/2026** (D-9):
- `Secret` nằm ở `protocol` (dùng cho `PairResponse.token`); gd1_06 dùng lại, không định nghĩa thêm.
- `upload_patch` nhận `AsyncRead` đã seek sẵn (không nhận đường dẫn): runner gd1_04 giữ tệp mở, kiểm `size`/`mtime` trước
  khúc, bọc token bucket quanh reader.
- `file_get_range` hai bước (phản hồi đã kiểm mã ⇒ `write_to`) thay vì một hàm ghi thẳng: `200` khi đang tải tiếp phải cắt
  tệp `.part` trước khi ghi.
- Mã `403` luôn map `WrongPin` (chỉ `pair` trả `403` theo hợp đồng).
- `connect_timeout` 10 s — happy eyeballs 250 ms là việc của gd1_05 (đua nhiều `PhoneClient`).

**Soát xanh** (cwd = worktree, `CARGO_TARGET_DIR=…/target/w1`): `cargo build` ✓, `cargo clippy --all-targets -- -D warnings` ✓,
`cargo clippy -p ferry-core --all-targets --target {x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu} -- -D warnings` ✓,
`cargo test` ✓ (core lib 30, client 11 + 1 ignored, real_phone 1 ignored, daemon 3), `cargo deny check licenses` ✓.

**Rebase lên gd1_03 (`1397fb8`)**: xung đột `crates/core/Cargo.toml` (`rusqlite` cạnh `percent-encoding` / `reqwest`) và khối
bàn giao trong gd1_04 (khối của 02 và 03 cạnh nhau) — giữ cả hai; `Cargo.lock` dựng lại từ master. Gộp đuôi video về một
nguồn: `queue::batch::VIDEO_EXTENSIONS` thành `pub use crate::protocol::VIDEO_EXTENSIONS`. Soát xanh lại sau rebase ✓
(core lib 76, client 11, daemon 3; clippy Windows chéo với `LIBSQLITE3_SYS_USE_PKG_CONFIG=1`).

## 10. Đề xuất cải tiến

Soát 4 lăng kính: **bảo mật** — token chỉ qua `Secret::expose` vào header, `Debug` của `PhoneClient` / `PairResponse` che,
thân lỗi log ≤ 1 KB, không proxy hệ thống (token không đi qua proxy lạ) — không phát hiện thêm. **RAM** — C5 +2,1 MB / 1 GB;
reqwest không TLS, không HTTP/2 kéo thêm `hyper` + `tower` (≈ 60 crate, đã có trong `Cargo.lock`, chỉ nạp ở `ferry`) — gd1_07
đo lại footprint daemon khi có runtime tokio. **UI/UX**, **Pro** — không áp (lõi, không UI).
