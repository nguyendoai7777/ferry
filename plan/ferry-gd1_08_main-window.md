# ferry-gd1_08 — main-window

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ✅ (11/11/2026) |
| Repo | ferry |
| Model | sonnet |
| Cỡ | L — ~1100 dòng (thêm `PhoneBrowser`, W-4) |
| Phụ thuộc | gd1_07 |
| Quyết định áp dụng | D-6, D-10, D-13, D-16, D-17, W-3, W-4 |
| Phụ thuộc mới | — (hộp chọn tệp: dùng của GPUI / materiant) |
| File đụng tới | MỚI `crates/ui/src/views/{main_window,device_header,drop_zone,send_tray,queue_list}.rs`, `crates/ui/src/views/phone_browser/{mod,tree,table,breadcrumb}.rs`, hàm thuần dựng cây `crates/core/src/browse.rs`; SỬA `i18n/{vi,en}.toml` |

## 0. Bàn giao từ ferry-gd1_07 (11/11/2026)
<!-- handoff: ferry-gd1_07 -->

- Hợp đồng IPC đã chốt ở `ferry_core::ipc` (doc module + test `command_and_reply_wire_format` là mẫu dây). Mỗi lệnh UI mang `req: u64` tự tăng, daemon trả đúng một `Reply{req, outcome}`; `outcome` = `done` · `added{…}` · `paired{device, name}` · `found{device, name}` · `library{items}` · `failed{error: CommandError}` (`code`).
- Sau `Hello{role: ui}` daemon đẩy ngay một `Snapshot` đầy đủ, rồi `Snapshot` mỗi khi mục / lô / thiết bị đổi, `Progress{items:[{id, sent, size, bytesPerSec}]}` khi chỉ tiến độ đổi; gộp ≤ 10 / s. Store UI: thay toàn bộ bằng `Snapshot`, vá tiến độ bằng `Progress` (id lạ ⇒ bỏ qua, chờ snapshot kế). `Snapshot.uploads` ≤ 1000 mục chưa xong đầu hàng + ≤ 300 mục xong mới nhất (`queuedOmitted`, `finishedOmitted` để hiện "… và N tệp nữa"); `batches` = `{info, progress}`; `canPauseAll` / `canResumeAll` cho nút chung.
- Lệnh của màn: `Queue{req, op}` với `add_files{device, paths, destDir}` (thả / dán / Chọn tệp / Chọn thư mục; daemon tự lọc không phải video, trả `added{batch?, files, skippedNonVideo, unreadable}`), `item{id, action}`, `batch{batch, action}`, `all{action}` — `action` = `pause|resume|cancel|retry` (`all` chỉ pause/resume). Chọn thư mục đích (W-4) dùng `Library{req, op: list{device}}` ⇒ `library{items}`.
- `crates/ui/src/daemon_link.rs` hiện chỉ log `Snapshot` / `Progress` / `Reply` — gd1_08 dựng store + bảng chờ `req` ở đó. Client đọc chậm (≥ 8 thông điệp chờ) bị bỏ lượt rồi nhận snapshot đầy đủ: UI không cần tự xin lại.

## 1. Làm gì (PROTOCOL mục 4.2)
- Đầu cửa sổ: thiết bị đang chọn (tên, chấm xanh / xám), nhiều máy ⇒ Select; chưa ghép máy nào ⇒ nút "Thêm điện thoại" (mở màn gd1_09).
- Vùng thả lớn + nút "Chọn tệp…" (chọn nhiều) + "Chọn thư mục…" ⇒ khay chờ gửi; nút "Gửi sang điện thoại…" mở component
  trình duyệt điện thoại (PROTOCOL 4.6, chế độ chọn đích) ⇒ "Gửi vào đây" ⇒ `AddFiles { paths, dest_dir }` (lọc đuôi ở
  daemon, UI chỉ báo số tệp bị bỏ). **Bổ sung W-4.**
- Hàng đợi dạng `VirtualList` (D-17): tên, cỡ, thanh tiến độ, tốc độ, trạng thái bằng chữ người dùng (§2.6), nút tạm dừng / huỷ / thử lại.
- Nhóm "Đã gửi" gập được.
- Soát theo `materiant/CLAUDE.md` mục "Soát trước khi xong một component / màn".
- **Bổ sung W-4 — component `PhoneBrowser`** (PROTOCOL 4.6) làm ở task này vì gửi cần nó trước; gd1_11 dùng lại ở chế
  độ thư viện. Breadcrumb + Lùi / Tiến / Lên, cây thư mục trái, bảng chi tiết phải (Tên · Ngày sửa · Loại · Cỡ, thư mục
  trước, sắp theo cột, nhấp đúp vào thư mục, chọn nhiều Ctrl/Cmd / Shift), "Thư mục mới" ảo, ô tìm; dựng từ snapshot
  danh sách điện thoại (daemon giữ bản `list` mới nhất + bản cache khi offline). Hàm dựng cây từ `dir` là hàm thuần có test.
  Nhớ thư mục đích lần trước theo từng điện thoại (config, theo `id`).
- Bổ sung W-3 (PROTOCOL 4.2, 6.9): nhận thả **và dán** (Ctrl+V / Cmd+V sau Copy trong Explorer / Finder) cả tệp lẫn thư mục; thư mục giữ cây (`<đích>/<tên thư mục>/<đường con>`); tệp không phải video bỏ qua, một dòng "Bỏ qua N tệp…"; hàng đợi hiện lô như dòng gộp mở ra được.

## 7. Ghi chú UI
```
┌──────────────────────────────────────────┐
│ ● Pixel 8 ▾                     ⚙       │
├──────────────────────────────────────────┤
│ Thả tệp / thư mục vào đây                │
│ [Chọn tệp…] [Chọn thư mục…]              │
│ Khay chờ: 3 tệp, 1 thư mục (4,2 GB)  ✕   │
│                 [Gửi sang điện thoại…]   │
├──────────────────────────────────────────┤
│ clip.mp4   1,2 GB  ▓▓▓▓░░ 42 %  18 MB/s ⏸✕│
│ ▸ Đã gửi (12)                            │
└──────────────────────────────────────────┘
```
Trình duyệt điện thoại (chế độ chọn đích):
```
┌────────────────────────────────────────────────────────────┐
│ ← → ↑  Pixel 8 › Chuyến đi › Ngày 1              ⟳  🔍     │
├───────────────┬────────────────────────────────────────────┤
│ ▾ Pixel 8     │ Tên            Ngày sửa       Loại   Cỡ    │
│   ▾ Chuyến đi │ 📁 Sáng        10/11 08:12    Thư mục       │
│     ▸ Ngày 1  │ 🎞 gộp.mp4     10/11 21:03    MP4    1,9 GB │
│ ▸ Khác        │                                            │
├───────────────┴────────────────────────────────────────────┤
│ [Thư mục mới]  Gửi 4 tệp (4,2 GB) vào: Chuyến đi/Ngày 1    │
│                                     [Huỷ] [Gửi vào đây]    │
└────────────────────────────────────────────────────────────┘
```
Khoá i18n `main.*`, `queue.*`, `state.*`, `browser.*`.

## 6. Nghiệm thu
- C1. Snapshot giả (test App ảo) ⇒ đúng số dòng, đúng chữ trạng thái vi / en. `[sonnet-test]`
- C2. Kéo thư mục có 3 video + 1 ảnh ⇒ 3 mục, báo "bỏ qua 1 tệp không phải video". `[owner-qa]`
- C-W4a. Hàm dựng cây từ `list` (dir lồng, `null` ⇒ Khác, thư mục ảo, sắp theo cột) có test. `[sonnet-test]`
- C-W4b. Chọn 3 tệp + 1 thư mục bằng hai nút ⇒ khay đúng; "Gửi sang điện thoại…" ⇒ trình duyệt mở ở đích lần trước; đi vào thư mục con bằng nhấp đúp + quay lại bằng breadcrumb; "Thư mục mới" ⇒ "Gửi vào đây" ⇒ lô có `dir` đúng (với `fake_phone`). `[sonnet-run]`
- C-W3. Thả một thư mục 2 cấp có cả `.txt` ⇒ một lô, đúng số video, dòng bỏ qua đúng số. `[sonnet-run]`

## 8. Nợ chuyển task khác
- gd1_09: nút "Thêm điện thoại" phát `MainEvent::AddPhone` (`views/main_window.rs`); `main.rs` chưa đăng ký nghe — task 09 nối vào màn của nó.
- gd1_11: chế độ `BrowserMode::Library` đã có (không thanh dưới), thiếu nút "Tải về máy" + lệnh `Download` (IPC của gd1_10), thả / dán vào thư mục đang mở.
- Daemon chưa giữ bản `list` cache khi offline (spec ghi "daemon giữ"): UI giữ bản đệm theo phiên trong `Model::libraries`, mở lại cửa sổ khi điện thoại offline thì trống. Cần lệnh/biến thể IPC đọc cache ⇒ để gd1_10/11 quyết (không tự thêm).
- Nút ⚙ cài đặt ở đầu cửa sổ: chưa có (gd1_12 sở hữu màn cài đặt).

## 9. Bàn giao thực tế
- Làm bằng Sonnet-session (D-14), không tách subagent. Soát xanh trên worktree w3: `cargo build`, `clippy --all-targets -D warnings`, `cargo test` (core 120 + ui 13 + tích hợp), `cargo deny check licenses` đều xanh.
- Mới: `core/src/browse.rs` (cây, thư mục ảo, sắp cột, breadcrumb; 8 test = C-W4a); ui: `model.rs` (kho + `apply_progress`), `daemon_link.rs` (đẩy vào `Entity<Model>`), `fmt.rs`, `ui_state.rs`, `assets.rs` (+2 icon `ferry-folder`, `ferry-video`), `views/{main_window,device_header,drop_zone,send_tray,queue_list,phone_browser/*}`. i18n thêm khoá `main.*`, `queue.*`, `state.*`, `browser.*`.
- Quyết định (Sonnet chọn 11/11/2026): (1) trình duyệt đích mở **thay trang chính trong cùng cửa sổ**, không dialog — đơn giản, không cần chồng overlay; (2) thư mục đích lần trước lưu ở `<data_dir>/ui-state.json` (UI tự ghi) thay vì `config.json` vì daemon là bên duy nhất ghi config và `config.rs` đang do gd1_12 sửa; (3) pause/resume dùng nút chữ (materiant chưa có icon tạm dừng / tiếp tục); (4) ngày sửa theo múi giờ máy nhờ `chrono` (đã có sẵn trong cây phụ thuộc của GPUI, `default-features=false`).
- Case: C1 ✓ (`queue_list::tests`), C-W4a ✓ (`browse::tests`), khay đúng số (`send_tray::tests`, một phần C-W4b), chọn nhiều Ctrl/Shift (`table::tests`). **Chưa chạy cửa sổ thật** (C2, C-W3, phần còn lại C-W4b với `fake_phone`, soát giao diện): Wayland không đặt được cửa sổ lên màn DELL và không muốn đè màn chủ dự án ⇒ `[owner-qa]`: `bash start.sh` rồi thả thư mục 3 video + 1 ảnh, bấm "Gửi sang điện thoại…" với điện thoại giả / thật.
- Nhớ: GPUI gốc Rust 2024 bắt buộc hàm view trả `AnyElement` khi mượn `cx` hai lần; `Window::focus(&h, cx)` cần `cx`.

## 10. Đề xuất cải tiến
1. [ux] Thả thư mục lớn: khay không biết số video / dung lượng thư mục (chỉ tên) — `send_tray.rs:known_size` — quét nền hoặc daemon trả `plan` trước khi gửi — người dùng thấy trước tổng cỡ — M — P3.
2. [ux] Icon tạm dừng / tiếp tục / thư mục cho materiant (đang dùng nút chữ + 2 SVG riêng ở `ui/assets`) — `queue_list.rs` — thêm vào `IconName` — đồng bộ với app khác — S — P3.
3. [ux] Daemon giữ cache `list` theo điện thoại để mở trình duyệt offline ngay từ đầu — xem mục 8 — M — P2 (gd1_10/11).
