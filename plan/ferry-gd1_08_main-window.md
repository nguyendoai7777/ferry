# ferry-gd1_08 — main-window

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
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
## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
