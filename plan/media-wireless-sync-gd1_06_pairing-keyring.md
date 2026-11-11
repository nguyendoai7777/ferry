# media-wireless-sync-gd1_06 — pairing-keyring

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ (11/11/2026) |
| Repo | media-wireless-sync |
| Model | opus |
| Cỡ | S — ~350 dòng |
| Phụ thuộc | gd1_02, gd1_05 |
| Quyết định áp dụng | W-1 |
| Phụ thuộc mới | `keyring` 4.2 (`v1`: Keychain / Credential Manager / Secret Service qua `zbus` thuần Rust — không `libdbus`; MIT/Apache, `cargo deny` xanh cả 6 target) |
| File đụng tới | MỚI `crates/core/src/pairing.rs`, `crates/core/tests/pairing.rs`; SỬA `crates/core/src/config.rs` (`remember_device` / `forget_device`), `crates/core/src/queue/store.rs` (`remove_device`), `crates/core/tests/real_phone.rs` |

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

## 2b. Phân vai

| Phần | Vai |
|---|---|
| `pairing.rs` (kho khoá, `Tokens`, `pair`, `forget`), `config` / `queue` thêm hàm, test | `[opus]` |
| Thử app Android thật + Keychain thật | `[opus-run]` |
| Kho khoá thật Windows / GNOME, khởi động lại máy | `[owner-qa]` |

## 6. Nghiệm thu
- C1. Cho `fake_phone` + `MemoryKeyring` / Khi ghép đúng PIN / Thì token có trong kho khoá, client mang token, `config.json`
  chỉ có `id` + tên + địa chỉ, log (mức trace) không có token lẫn PIN; sai PIN ⇒ `WrongPin`; sai quá 5 ⇒ `RateLimited`
  có thời gian chờ; máy khác `id` trả lời ⇒ `OtherPhone`, không cất; kho khoá hỏng ⇒ `Keyring`, client không mang token.
  `[opus-unit]` — `tests/pairing.rs` `c1_*` ✅
- C2. Cho thiết bị Online / Khi điện thoại "quên mọi máy" / Thì `401` ⇒ `NeedsRepair`, `info.paired=false` lúc khởi động
  lại ⇒ `NeedsRepair`; ghép lại ⇒ token mới đè cũ ⇒ `Paired` ⇒ Online. `[opus-unit]` — `c2_phone_forgets_then_repair` ✅;
  thêm `missing_token_needs_repair`, `forget_online_and_offline` ✅
- C2-real. App Android thật (emulator) + Keychain macOS thật: ghép ⇒ token trong Keychain, đọc lại bằng `Tokens` mới ⇒ Online;
  bấm "Forget all computers" ⇒ `NeedsRepair` + `401`; quên phía PC ⇒ Keychain sạch. `[opus-run]` — `real_phone_keyring` ✅
- C3. Kho khoá thật trên Windows / macOS / GNOME: ghép, khởi động lại máy, vẫn Online. `[owner-qa]` — chờ gd1_07 (daemon
  thật mới ghép được từ UI)

## 9. Bàn giao thực tế

- **API** (`mws_core::pairing`): `TokenStore` (chặn: `get` / `set` / `delete` theo `DeviceId`) — `OsKeyring` (crate
  `keyring` 4 `v1`), `MemoryKeyring` (test, `fail_with`). `Tokens` = kho khoá + đệm RAM, async qua `spawn_blocking`, clone
  rẻ — daemon giữ một cái, `discovery::connect(id, addrs, tokens.get(id).await?.as_ref())`. `pair(&mut client, expected,
  &pin, Identity::from(&config), &tokens) -> Paired { id, name }` (token đã vào kho khoá + client). `forget(id,
  online_client, &tokens) -> Forgotten { phone_notified, keyring_error }` (`DELETE /pair` chờ ≤ 5 s, lỗi mạng / `401` vẫn
  quên phía PC). `Config::remember_device` / `forget_device`, `QueueStore::remove_device`.
- **Khoá kho khoá**: dịch vụ `media-wireless-sync` (bản debug `media-wireless-sync.dev` — khỏi đè token của bản đã cài, hai
  bản khác `clientId`), tài khoản = `id` điện thoại.
- Opus chọn 11/11/2026: `keyring` 4.2 thay 3.x — vì bản `v1` của 4 dùng `zbus` thuần Rust trên Linux (3.x mặc định
  `dbus-secret-service` kéo `libdbus`, giấy phép AFL/GPL), có mã hoá phiên `crypto-rust`; không thêm feature nào.
  `NeedsRepair` không xoá token chết (ghép lại đè lên); thiếu token trong kho khoá ⇒ `Devices::unauthorized` ⇒ `NeedsRepair`.
  Không có kho khoá (Linux không Secret Service) ⇒ `KeyringError::Unavailable`, **không** rơi về tệp (PROTOCOL 2.2).
- Ngoài phạm vi đã sửa (D-11): `cargo fmt` cho `transfer/{mod,upload}.rs`, `tests/upload.rs` (nợ fmt của gd1_04).
- Thử thật: emulator `sdk_gphone64_arm64` qua `adb forward tcp:18080 tcp:8080`, client id `mws-gd1-06-test`; đã bấm
  "Forget all computers" (xoá cả máy `w2test` của session w2 đã xong). Keychain không hỏi quyền (cùng một exe ghi rồi đọc).
- Soát xanh: `cargo build`, `clippy --all-targets -D warnings` (+ chéo `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`),
  `cargo test` (pairing 6, store +1, config +1, pairing unit 2), `cargo deny check licenses` xanh. RAM: chưa đo được — `mws`
  chưa gọi `pairing` (gd1_07); Linux sẽ thêm luồng `async-io` của `zbus` khi chạm kho khoá lần đầu ⇒ gd1_07 C4 đo lại.
- Chờ kiểm tay: C3 (`_backlog.md`).

## 10. Đề xuất cải tiến

- [bảo mật|ux] macOS: exe chưa ký (D-21) mà đổi bản ⇒ mục Keychain do bản cũ tạo có thể hỏi "mws muốn dùng thông tin bí
  mật" — bằng chứng: ACL Keychain gắn chữ ký exe; chưa tái hiện vì chưa có hai bản cài — đề xuất: ký ad-hoc ổn định /
  Developer ID khi phát hành — cỡ S — P2. Không làm được trong lượt: chứng chỉ ký là dịch vụ trả phí ⇒ `_backlog.md`.
- [ux] Linux không Secret Service (KDE thiếu `kwalletd` bridge, phiên tối giản) ⇒ không ghép được — đề xuất: UI gd1_09 nói
  "Cần kho khoá của hệ thống (GNOME Keyring / KWallet)" khi `PairError::Keyring(Unavailable)` — cỡ S — P2 ⇒ ghi vào gd1_09.
