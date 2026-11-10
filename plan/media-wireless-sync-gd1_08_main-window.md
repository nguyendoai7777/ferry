# media-wireless-sync-gd1_08 — main-window

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | sonnet |
| Cỡ | M — ~600 dòng |
| Phụ thuộc | gd1_07 |
| Quyết định áp dụng | D-6, D-10, D-13, D-16, D-17 |
| Phụ thuộc mới | — (hộp chọn tệp: dùng của GPUI / materiant) |
| File đụng tới | MỚI `crates/ui/src/views/{main_window,device_header,drop_zone,queue_list}.rs`; SỬA `i18n/{vi,en}.toml` |

## 1. Làm gì (PROTOCOL mục 4.2)
- Đầu cửa sổ: thiết bị đang chọn (tên, chấm xanh / xám), nhiều máy ⇒ Select; chưa ghép máy nào ⇒ nút "Thêm điện thoại" (mở màn gd1_09).
- Vùng thả lớn + nút "Chọn video…": gửi `AddFiles` (lọc đuôi ở daemon, UI chỉ báo số tệp bị bỏ).
- Hàng đợi dạng `VirtualList` (D-17): tên, cỡ, thanh tiến độ, tốc độ, trạng thái bằng chữ người dùng (§2.6), nút tạm dừng / huỷ / thử lại.
- Nhóm "Đã gửi" gập được.
- Soát theo `materiant/CLAUDE.md` mục "Soát trước khi xong một component / màn".

## 7. Ghi chú UI
```
┌──────────────────────────────────────────┐
│ ● Pixel 8 ▾                     ⚙       │
├──────────────────────────────────────────┤
│   Thả video vào đây   [Chọn video…]      │
├──────────────────────────────────────────┤
│ clip.mp4   1,2 GB  ▓▓▓▓░░ 42 %  18 MB/s ⏸✕│
│ ▸ Đã gửi (12)                            │
└──────────────────────────────────────────┘
```
Khoá i18n `main.*`, `queue.*`, `state.*`.

## 6. Nghiệm thu
- C1. Snapshot giả (test App ảo) ⇒ đúng số dòng, đúng chữ trạng thái vi / en. `[sonnet-test]`
- C2. Kéo thư mục có 3 video + 1 ảnh ⇒ 3 mục, báo "bỏ qua 1 tệp không phải video". `[owner-qa]`

## 8. Nợ chuyển task khác
## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
