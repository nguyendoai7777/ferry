# media-wireless-sync-gd1_02 — protocol-client

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | opus |
| Cỡ | M — ~700 dòng + test |
| Phụ thuộc | gd1_01 |
| Quyết định áp dụng | W-1, W-3, D-2 |
| Phụ thuộc mới | `tokio`, `reqwest` (`default-features = false`, `stream`; không TLS) — MIT/Apache; test: `hyper` server hoặc `tokio` TCP thô cho server giả |
| File đụng tới | MỚI `crates/core/src/{protocol,client}.rs`, `crates/core/tests/fake_phone/mod.rs`, `crates/core/tests/client.rs` |

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
## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
