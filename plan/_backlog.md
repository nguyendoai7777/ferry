# Backlog — việc treo không làm được trong lượt

Mỗi gạch một việc + LÝ DO chưa làm được (thiếu máy, dịch vụ trả phí, chờ quyết định — D-8: đề xuất đã xác nhận thì làm
ngay, không vào đây). Có task thì trỏ tới tệp task, đừng chép lại.

## ferry

- **Remote git** — lý do: repo khởi tạo 11/11/2026 chỉ có ở máy; tạo repo trên GitHub là việc của chủ dự án. Có URL thì thêm
  vào bảng repo của `init.sh` / `init.ps1` gốc (`ferry` kéo `materiant`).
- **Tên hiển thị + icon của app** — lý do: chờ chủ dự án chốt. Tạm dùng "Ferry" qua khoá i18n `app.name`.
- **`507` không nói được "cần thêm N GB"** (gd1_04 mục 10) — lý do: `info` không có chỗ trống (PROTOCOL 6.3); thêm trường
  `free` hoặc header vào `507` là đổi hợp đồng (W-1) — chờ chủ dự án. PC đã sẵn `PauseReason::NoSpace { shortBy }`.
- **gd1_05 C4 — điện thoại thật hiện trong ≤ 5 s** — lý do: cần điện thoại Android thật cùng WiFi (emulator không quảng
  bá mDNS ra LAN). Bật Sync WiFi trên app `video-player` rồi `cargo run -p ferry-core --example discover`; xem mục 9 của
  `plan/ferry-gd1_05_discovery-devices.md`.
- **gd1_06 C3 — kho khoá thật sau khởi động lại máy** (Windows / macOS / GNOME) — lý do: cần daemon gd1_07 + UI ghép,
  và máy Windows / Ubuntu. Ghép một điện thoại, khởi động lại máy, mở `ferry` ⇒ thiết bị Online không hỏi PIN.
- **Keychain macOS hỏi quyền sau khi cập nhật bản chưa ký** (gd1_06 mục 10) — lý do: cần chứng chỉ ký (trả phí, D-21).
