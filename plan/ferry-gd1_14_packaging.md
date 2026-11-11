# ferry-gd1_14 — packaging

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ 11/11/2026 |
| Repo | ferry (+ gốc: `build.*`, `tag.*`) |
| Model | opus |
| Cỡ | M |
| Phụ thuộc | gd1_08 … gd1_13 |
| Quyết định áp dụng | D-2, D-3, D-21, D-22 |
| Phụ thuộc mới | `cargo-about` (công cụ build) |
| File đụng tới | MỚI `build.sh`, `build.ps1`, `packaging/…`, `release-notes/TEMPLATE.md`; SỬA gốc `build.sh`, `build.ps1`, `tag.sh`, `tag.ps1`, `CLAUDE.md`; SỬA `crates/daemon/src/autostart.rs` (TryExec), `crates/core/src/logging.rs` (hạ log zbus); MỚI `crates/core/tests/packaging.rs` |

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

## 2b. Phân vai
- [opus] toàn bộ: script đóng gói ba OS, NSIS, Info.plist, trang giấy phép, icon, nối script gốc, test khớp tệp đóng gói ↔ mã.

## 6. Nghiệm thu
- C1. Cài trên máy sạch mỗi OS ⇒ dò thấy điện thoại, gửi được 1 tệp. `[owner-qa]`
- C2. `bash build.sh` trên Ubuntu ra `release/ferry/ferry_<ver>_linux_amd64.deb` + `.sha256`; `dpkg-deb -c` có
  `/usr/bin/{ferry,ferry-ui}`, `ferry.desktop` (qua `desktop-file-validate`), metainfo (qua `appstreamcli`), icon hicolor
  16…512, `third-party.html` + `copyright`. ✅
- C3. Exe release trong .deb chạy `ferry --autostart`: khay đăng ký, mDNS dò, RAM rảnh nhỏ. ✅ (số ở mục 9)
- C4. `ferry.nsi` biên dịch sạch `makensis /WX` (NSIS 3.10 trên Linux, tệp stage giả). ✅ — chạy bản cài thật `[owner-qa]`
- C5. `build.ps1`, `build-installer.ps1` chạy trên Windows (setup + portable) `[owner-qa]` (máy này không có pwsh).
- C6. `build.sh` trên macOS ra `Ferry.app` + `.dmg`; lần đầu dò hỏi quyền mạng LAN `[owner-qa]`.
- C7. `cargo test -p ferry-core --test packaging`: tên exe, `app_id`, `_vpsync._tcp`, UDP 5353, tên tác vụ tự chạy, targets
  giấy phép khớp mã. ✅

## 9. Bàn giao thực tế
- **Lệnh**: `bash build.sh` (ferry) hoặc gốc `bash build.sh --project ferry`; Windows `pwsh ./build.ps1 [-Package setup,portable]`.
  Ra `<gốc>/release/ferry/`. Cờ: `-f`, `--draft` (thiếu cargo-about), `--arch arm64|x64` / `--chip` (macOS), `--package`.
- **Đo trên máy này (Ubuntu 26.04)**: `ferry_0.1.0_linux_amd64.deb` 13 MB (Installed-Size ≈ 43 MB: `ferry` 12,3 MB,
  `ferry-ui` 32 MB); build release ≈ 3 phút, trang giấy phép ≈ 35 s (579 crate). Chạy `ferry --autostart` từ gói giải nén
  20 s: RSS 12,2 MB (đỉnh 12,4), 7 luồng, 0 % CPU; log: ghi config, hàng đợi lên v1, dò mDNS, engine chạy, khay đăng ký SNI.
- **Depends** = `dpkg-shlibdeps` + `libwayland-client0, libvulkan1 | libegl1` (GPUI `dlopen`, shlibdeps không thấy). Libc
  máy dựng là sàn: dựng trên 26.04 ⇒ `libc6 (>= 2.43)`, Ubuntu cũ hơn không cài được — muốn hỗ trợ 24.04 thì dựng trên 24.04.
- **Lựa chọn (D-9)**: bundle id `com.sandtechs.desktop.ferry`; bản cài Windows per-machine + admin vì luật tường lửa mDNS
  (UDP 5353 vào, `profile=any`, gắn `ferry.exe`) cần admin — bản portable không có luật (lần đầu Windows hỏi, phải tích cả
  Public); trang Finish chạy app qua `explorer` (không mang quyền admin). Gỡ cài xoá luật tường lửa + tác vụ `ferry-<user>`
  của người đang gỡ, hỏi xoá `%APPDATA%\ferry` / `%LOCALAPPDATA%\ferry`. Một script giấy phép Node cho cả ba OS (bê groove),
  thêm tay SQLite (blessing), Lucide (ISC), Mona Sans (OFL). Icon vẽ bằng mã (`packaging/icons/gen-icons.mjs`: ô vuông xanh,
  điện thoại trắng, mũi tên xuống) — thay khi chủ dự án chốt icon (backlog). macOS `LSUIElement` (chỉ khay, không Dock).
- **Sửa mã kèm**: mục tự chạy Linux thêm `TryExec` (gỡ .deb thì GNOME bỏ qua mục cũ, không báo lỗi); log `zbus` /
  `tracing` mức INFO hạ xuống DEBUG (12 / 20 dòng log rảnh là D-Bus). `cargo fmt --all` sửa định dạng tồn từ gd1_08–13.
- **Chưa kiểm được ở đây** `[owner-qa]`: C1, C5, C6; Windows: luật tường lửa trên mạng Public, SmartScreen (chưa ký);
  macOS: cửa sổ `ferry-ui` khi `LSUIElement` (có hiện Dock / lấy focus không), Gatekeeper với bản ký ad-hoc.
- Gỡ cài không xoá token trong kho khoá và tác vụ tự chạy của người dùng khác (chấp nhận, ghi đây).
- `tag.sh` tạo khung release note theo khuôn chung, `release-notes/TEMPLATE.md` để tham khảo.
- `plan/_handoff.md` giữ lại tới close-phase (tệp nói "Xong GĐ1 thì xoá" — chủ dự án còn kiểm tay).

## 10. Đề xuất cải tiến
- Nhúng icon vào exe Windows (`winres` / `embed-resource` trong build.rs) để Task Manager, Alt+Tab có icon — cần máy Windows
  kiểm (backlog).
- Dựng .deb trên Ubuntu LTS cũ hơn (container 24.04) để hạ sàn libc — khi chủ dự án muốn phát hành rộng.
