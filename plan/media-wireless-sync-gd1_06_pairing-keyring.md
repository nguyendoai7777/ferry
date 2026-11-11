# media-wireless-sync-gd1_06 — pairing-keyring

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | opus |
| Cỡ | S — ~350 dòng |
| Phụ thuộc | gd1_02, gd1_05 |
| Quyết định áp dụng | W-1 |
| Phụ thuộc mới | `keyring` (MIT/Apache; Linux qua Secret Service — soát cây phụ thuộc không kéo `libdbus` LGPL, dùng backend thuần Rust nếu có) |
| File đụng tới | MỚI `crates/core/src/pairing.rs`; SỬA `crates/core/src/config.rs` (danh sách thiết bị đã ghép: `id`, tên hiển thị, địa chỉ cuối) |

## 0. Bàn giao từ media-wireless-sync-gd1_05 (11/11/2026)

<!-- handoff: media-wireless-sync-gd1_05 -->

- **Đã có sẵn**: `device::Devices` (sổ thiết bị) — `paired(id, now)` sau khi token vào kho khoá (⇒ `Paired`, hẹn `info`
  ngay), `forget(id)` (⇒ `Discovered` nếu còn thấy qua mDNS, không thì ra khỏi sổ), `unauthorized(id)` (⇒ `NeedsRepair`),
  `to_config()` ⇒ `Vec<PairedDevice>` (địa chỉ đã xếp, link-local ghi `%en0`) để ghi `config.devices`; `from_config` nạp
  lại ở `Paired`. Trạng thái nằm ở `Device::state`, không ở config.
- **Địa chỉ để ghép**: `Devices::addresses(id)` (đã xếp, địa chỉ thắng lên đầu) ⇒ `discovery::connect(id, &addrs, None)`
  trả `Connected { client, info, addr }` — dùng `client.pair(..)` trên chính client thắng. Nhập tay: `address::parse_address`
  ⇒ `discovery::probe(addr)` ⇒ `Devices::info_ok(&info, addr, now)` (vào sổ ở `Discovered`).
- **Bẫy**: `Devices` không giữ token (chỉ biết có / không qua trạng thái) — token ở `pairing` + kho khoá; `connect` nhận
  `Option<&Secret>`.
- **Soát lại**: `cargo test -p mws-core device::` và `--test discovery`; mục 9 của `plan/media-wireless-sync-gd1_05_discovery-devices.md`.

## 0. Bàn giao từ media-wireless-sync-gd1_02 (11/11/2026)

<!-- handoff: media-wireless-sync-gd1_02 -->

- **`Secret` đã có ở `protocol::Secret`** (`new`, `expose`, `Debug` che, không `Display` / `Serialize`) — dùng lại, đừng định
  nghĩa thêm.
- **Đã có sẵn**: `PhoneClient::pair(&Secret pin, client_id, client_name) -> PairResponse { token: Secret, id, name }` (không tự
  giữ token: cất kho khoá rồi `set_token(Some(..))`), `unpair()`, `info().paired`. `client_name` tự cắt 64 ký tự + URL-encode.
- **Lỗi**: `WrongPin` (403), `RateLimited { retry_after: Option<Duration> }` (429 — lần sai thứ 5 đã trả 429), `Unauthorized`
  (401 ⇒ `NeedsRepair`), `BadRequest` (client id sai dạng: 8–64 ký tự `[A-Za-z0-9_-]`).
- **`fake_phone`**: `PIN = "123456"`, `forget_tokens()` (C2), `issue_token(client_id)` bỏ qua bước PIN, `restart()` reset khoá PIN.
- **Soát lại**: `cargo test -p mws-core --test client c1` (luồng ghép), `tests/real_phone.rs` (app thật, PIN lấy trên màn Sync WiFi).

## 1. Làm gì
- `pair(device, pin)` ⇒ `POST /api/v1/pair` với `clientId` / `clientName` ⇒ token vào kho khoá OS (khoá `media-wireless-sync/<device id>`), thiết bị vào config (không token).
- Sai PIN / `429` trả lỗi có `retry_after` cho UI đếm ngược.
- `forget(device)` ⇒ `DELETE /api/v1/pair` nếu Online (lỗi mạng thì vẫn quên phía PC), xoá token + mục config + hàng đợi của thiết bị (hỏi ở UI).
- `401` / `info.paired == false` ⇒ `NeedsRepair`; ghép lại ⇒ token mới thay token cũ.
- Token kiểu `Secret` (không `Debug` / `Serialize` lộ giá trị), log che.

## 6. Nghiệm thu
- C1. Ghép với `fake_phone` ⇒ token có trong kho khoá (backend giả trong test), không có trong `config.json`, log. `[opus-unit]`
- C2. Server "quên mọi máy" ⇒ `NeedsRepair`; ghép lại ⇒ Online. `[opus-unit]`
- C3. Kho khoá thật trên Windows / macOS / GNOME: ghép, khởi động lại máy, vẫn Online. `[owner-qa]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
