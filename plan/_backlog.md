# Backlog — việc treo không làm được trong lượt

Mỗi gạch một việc + LÝ DO chưa làm được (thiếu máy, dịch vụ trả phí, chờ quyết định — D-8: đề xuất đã xác nhận thì làm
ngay, không vào đây). Có task thì trỏ tới tệp task, đừng chép lại.

## ferry

- **Tên hiển thị của app** — lý do: chờ chủ dự án chốt. Tạm dùng "Ferry" qua khoá i18n `app.name`. Icon đã chốt 11/11/2026: máy bay giấy ba mặt (`packaging/icons/gen-icons.mjs`, khay `crates/daemon/src/tray.rs`).
- **`507` không nói được "cần thêm N GB"** (gd1_04 mục 10) — lý do: `info` không có chỗ trống (PROTOCOL 6.3); thêm trường
  `free` hoặc header vào `507` là đổi hợp đồng (W-1) — chờ chủ dự án. PC đã sẵn `PauseReason::NoSpace { shortBy }`.
- **gd1_05 C4 — điện thoại thật hiện trong ≤ 5 s** — lý do: cần điện thoại Android thật cùng WiFi (emulator không quảng
  bá mDNS ra LAN). Bật Sync WiFi trên app `video-player` rồi `cargo run -p ferry-core --example discover`; xem mục 9 của
  `plan/ferry-gd1_05_discovery-devices.md`.
- **gd1_06 C3 — kho khoá thật sau khởi động lại máy** (Windows / macOS / GNOME) — lý do: cần daemon gd1_07 + UI ghép,
  và máy Windows / Ubuntu. Ghép một điện thoại, khởi động lại máy, mở `ferry` ⇒ thiết bị Online không hỏi PIN.
- **Keychain macOS hỏi quyền sau khi cập nhật bản chưa ký** (gd1_06 mục 10) — lý do: cần chứng chỉ ký (trả phí, D-21).
- **gd1_07 C5 `[owner-qa]` — icon khay trên nền sáng / tối cả 3 OS** — lý do: cần mắt chủ dự án + máy Windows / macOS (KDE
  nền sáng). Icon Linux gần trắng (`#f2f2f2`) có thể mất trên thanh sáng; Windows xanh + chấm cam; macOS template. Xem 3
  trạng thái Rảnh / Đang gửi / Cần chú ý. Icon khay đã là máy bay giấy (11/11/2026), đang gửi = đậm dần đuôi ⇒ mũi.
- **gd1_07 C4 trên macOS** — đo `footprint -p <pid>` lúc rảnh (mốc gd1_01 ≈ 10 MB); Linux đã đo: RssAnon 1,9 MB, 0 CPU.
- **gd1_12 C2 `[owner-qa]` — bật "Khởi động cùng hệ thống" rồi đăng nhập lại ⇒ có khay** — lý do: cần đăng xuất / đăng
  nhập máy thật (Windows / macOS / GNOME). Bản debug ghi mục `ferry.dev`; bản cài ghi `ferry`. Tắt lại ⇒ mục bị gỡ.
- **gd1_14 `[owner-qa]` — bản cài Windows / macOS** — lý do: cần máy Windows (`pwsh ./build.ps1 -Package setup,portable`,
  cài, xem luật tường lửa "Ferry (mDNS)" trên mạng Public, gỡ cài) và Mac (`bash build.sh`, `Ferry.app` hỏi quyền mạng
  LAN, cửa sổ `ferry-ui` có lấy focus khi `LSUIElement`). Cả ba OS: cài trên máy sạch ⇒ dò điện thoại, gửi 1 tệp (gd1_14 C1).
- **gd1_15 C9 `[owner-qa]` — dán ở Explorer / Finder** — lý do: cần máy Windows / Mac (Ubuntu Files chủ dự án đã thử
  ổn 12/11/2026 với điện thoại thật). Sao chép một tệp + một thư mục ⇒ dán; Cắt ⇒ dán (Finder: ⌥⌘V) ⇒ mục biến khỏi
  điện thoại. macOS: `clipboard/macos.rs` chưa biên dịch lần nào (không check chéo được từ Linux).
- **Icon nhúng trong exe Windows** (gd1_14 mục 10) — lý do: cần máy Windows kiểm resource; shortcut đã dùng `ferry.ico`.
- **Chờ app điện thoại `video-player`** — lý do: việc ở repo điện thoại (session mobile), PC đã xong phần mình. Khi điện
  thoại có thì thử thật trên vivo V2425A (`adb connect <ip>:5555`, `~/Android/Sdk/platform-tools/adb`):
  - **W-7 duyệt bộ nhớ** (PROTOCOL §6.11): công tắc "Cho máy tính duyệt bộ nhớ" + quyền "Truy cập mọi tệp", khai `browse`,
    route `GET /api/v1/dirs`, gốc `dir` = `/storage/emulated/0`. PC dựng cây từ `dirs` (`browse::FsCache`).
  - **Server Sync WiFi tự bật lại**: lưu `sync_enabled`, bật lại service ở `Application.onCreate`,
    `MY_PACKAGE_REPLACED`, `BOOT_COMPLETED`, `START_STICKY`; log tag `VpSync`. Gặp thật 11/11/2026: cài đè apk giết
    tiến trình, mở lại app không mở cổng 8080 ⇒ PC báo lỗi mạng.
  - **§6.10 danh sách cả lô** (`batch`, `X-Batch-*`, `503 awaiting_approval`, `410 declined`): phía PC chưa gửi
    `X-Batch-*` / chưa xử lý `410` — làm khi điện thoại khai `batch`.
  - **W-11 — `id` + token bền qua tắt / bật "Cho phép nhận"** (PROTOCOL §6.3; gặp 12/11/2026 trên vivo: mỗi lần bật lại
    sync `id` đổi `e5427876…` ⇒ `a002ad7a…` và token cũ thành `paired:false` ⇒ PC bắt ghép lại PIN). Lưu bền `id` + danh
    sách token; soát cả iPhone. Thử: ghi `id` từ `curl <ip>:8080/api/v1/info`, tắt / bật sync, `id` phải giữ.
  - **W-11 — mục ẩn** (§6.11–6.14): `dirs` liệt kê tên bắt đầu bằng `.` (vẫn giấu `Android/data`, `Android/obb`); `dirs`
    `POST`, `rename`, `copy`, `move`, `fetch`, `DELETE entries` cho phép mục ẩn (vẫn cấm gốc + `Android/`).
  - **Mã thiết bị bền qua cài lại app** (HMAC của `ANDROID_ID`): cài lại app đổi `id` ⇒ PC đang dò thay thế qua địa chỉ
    (`discovery::find_replacement`).
  - **Điện thoại không quảng bá `_vpsync._tcp`** (gặp 11/11/2026, vivo V2425A, Sync WiFi bật, HTTP `:8080` trả `info`):
    hỏi mDNS kiểu legacy unicast (cổng nguồn ngẫu nhiên, điện thoại trả unicast) thì `_services._dns-sd._udp` của máy
    chỉ có `_adb._tcp` ⇒ `NsdManager.registerService` của app chưa chạy / hỏng (soát log `VpSync`, `onRegistrationFailed`,
    service đăng ký lại sau khi server bật lại). PC đã có quét LAN dự phòng (PROTOCOL §6.2) nên vẫn thấy máy trên mạng
    cùng /24; mDNS vẫn cần cho mạng rộng hơn /24 và IPv6-only.
