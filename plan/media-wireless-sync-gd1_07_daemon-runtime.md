# media-wireless-sync-gd1_07 — daemon-runtime

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | opus |
| Cỡ | M — ~700 dòng |
| Phụ thuộc | gd1_04, gd1_05, gd1_06 |
| Quyết định áp dụng | W-1, D-13 |
| Phụ thuộc mới | thông báo hệ thống (`notify-rust` MIT/Apache hoặc API OS trực tiếp — soát Linux không kéo `libdbus`) |
| File đụng tới | MỚI `crates/daemon/src/{engine,tray_state,notify}.rs`; SỬA `crates/core/src/ipc.rs`, `crates/daemon/src/main.rs` |

## 1. Làm gì
- `mws` chạy runtime tokio đa luồng nhỏ (2 luồng) cho discovery + một `Runner` mỗi thiết bị; vòng sự kiện khay ở luồng chính.
- `Snapshot` (thiết bị + trạng thái, hàng đợi từng thiết bị, tiến độ, "Đã gửi") gửi qua IPC khi đổi, gộp tối đa 10 lần / s; UI mới nối nhận ngay bản đầy đủ.
- Lệnh IPC: `AddFiles{device, paths}` (lọc đuôi 6.6, thư mục đệ quy), `ItemAction{id, pause|resume|cancel|retry}`, `PauseAll/ResumeAll`, `Pair/Forget/AddManualAddress`, `ListLibrary`, `Download` — ≤ 4 biến thể mới mỗi lần thêm, tách nếu vượt trần.
- Khay 3 trạng thái (rảnh / đang gửi kèm % / cần chú ý), tooltip "Bật Sync WiFi trên điện thoại để gửi N video" khi có mục chờ mà Offline; menu: Mở cửa sổ, Tạm dừng tất cả, Thoát.
- Một thông báo hệ thống khi cả lô xong; lỗi cần người dùng làm (hết chỗ, cần ghép lại) báo một lần.
- Nợ từ gd1_01: **khay Linux** — bê StatusNotifierItem tự viết của fastcast (`fastcast/crates/daemon/src/platform/linux/sni.rs`,
  không GTK / libappindicator) vào `crates/daemon/src/platform/linux.rs` (hiện chỉ kênh chặn, không icon). Khung đã có:
  `Event`, `Daemon::handle`, `tray::{menu_action, phone_rgba}`, IPC `UiToDaemon` / `DaemonToUi` + `Hello`/`Ok`.
  Mốc RAM gd1_01: `mws` rảnh footprint 10 MB (RSS 44 MB) trên macOS — C4 so với số này.

## 6. Nghiệm thu
- C1. Client IPC giả nhận snapshot đầu đủ, rồi delta theo tiến độ ≤ 10 / s. `[opus-unit]`
- C2. Thả 3 tệp qua IPC với `fake_phone` ⇒ khay chuyển "đang gửi" ⇒ "rảnh" + một thông báo. `[opus-run]`
- C3. Tắt `mws` giữa khúc, mở lại ⇒ gửi tiếp. `[opus-run]`
- C4. RAM / CPU `mws` rảnh sau task, so với mốc gd1_01. `[opus-run]`
- C5. Icon khay ba trạng thái nhìn rõ ở nền sáng / tối ba OS. `[owner-qa]`

## 8. Nợ chuyển task khác
- Icon khay: vẽ tạm; chủ dự án chọn bộ icon sau (backlog nếu cần thiết kế).

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
