# ferry-gd1_14 — packaging

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | ferry (+ gốc: `build.*`, `tag.*`) |
| Model | opus |
| Cỡ | M |
| Phụ thuộc | gd1_08 … gd1_13 |
| Quyết định áp dụng | D-2, D-3, D-21, D-22 |
| Phụ thuộc mới | `cargo-about` (công cụ build) |
| File đụng tới | MỚI `build.sh`, `build.ps1`, `packaging/…`, `release-notes/TEMPLATE.md`; SỬA gốc `build.sh`, `build.ps1`, `tag.sh`, `tag.ps1` |

## 0. Bàn giao từ ferry-gd1_13 (11/11/2026)
<!-- handoff: ferry-gd1_13 -->

- Phụ thuộc mới `notify` 8 (CC0-1.0) trong `ferry-core` — trang giấy phép bên thứ ba có CC0; `cargo deny` đã qua.
- Daemon có thêm luồng `ferry-watch` + luồng inotify chỉ khi bật thư mục theo dõi; RSS release Linux 10,7 ⇒ 11,9 MB.
- Linux: theo dõi đệ quy tốn một inotify watch mỗi thư mục con (giới hạn `fs.inotify.max_user_watches`); hết thì màn cài đặt
  hiện lỗi theo dõi (`watchError`) — bản cài không cần chỉnh sysctl.
- Kiểm tay trên bản cài Windows / macOS: đổi tên tệp cũ trong thư mục theo dõi (gd1_13 mục 6 C5).

## 0. Bàn giao từ ferry-gd1_12 (11/11/2026)
<!-- handoff: ferry-gd1_12 -->

- Tự chạy cùng máy: `crates/daemon/src/autostart.rs` — Windows Task Scheduler, Linux `~/.config/autostart/ferry.desktop`,
  macOS LaunchAgent; lệnh ghi là `<exe> --autostart` (đường exe lúc bật). Bản debug dùng tên `ferry.dev`. Bản cài cần đường
  exe ổn định (đổi chỗ cài ⇒ daemon đồng bộ lại lúc khởi động vì `config.autostart` vẫn bật). Gỡ cài nên xoá mục tự chạy.
- `ferry-ui --settings` mở riêng cửa sổ cài đặt (khay gọi khi chưa có UI) — phải có trong bản cài cạnh `ferry`.
- Font nhúng Mona Sans (materiant feature `font-mona-sans`, OFL) ⇒ trang giấy phép bên thứ ba phải kèm `OFL.txt`
  (`materiant/assets/fonts/mona-sans/`); `cargo-about` không thấy font, thêm tay.
- Linux: cửa sổ xin CSD (`views/title_bar.rs`) vì GNOME Wayland không cấp viền; `app_id` = `ferry` ⇒ tệp `.desktop` của
  bản cài nên tên `ferry.desktop` (`StartupWMClass=ferry`) để dock gom đúng cửa sổ.

## 1. Làm gì
- Bản cài theo khuôn tên gốc: Windows NSIS, macOS `.app` + `.dmg`, Linux `.deb` (học `groove/build.*`, `groove/packaging`).
- Hai exe (`ferry`, `ferry-ui`) trong một gói; autostart trỏ `ferry`.
- Windows: luật tường lửa cho mDNS (UDP 5353 vào) lúc cài — không có thì dò không chạy trên mạng "Public".
- macOS: `NSLocalNetworkUsageDescription` + `NSBonjourServices` (`_vpsync._tcp`) trong `Info.plist` (macOS 15 chặn mạng LAN nếu thiếu).
- Trang giấy phép bên thứ ba; thông tin phát hành D-21; release note D-22; nối radio của `build.*` / `tag.*` gốc.

## 6. Nghiệm thu
- C1. Cài trên máy sạch mỗi OS ⇒ dò thấy điện thoại, gửi được 1 tệp. `[owner-qa]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
