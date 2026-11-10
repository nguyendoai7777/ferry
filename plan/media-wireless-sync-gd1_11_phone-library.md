# media-wireless-sync-gd1_11 — phone-library

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | media-wireless-sync |
| Model | sonnet |
| Cỡ | S — ~350 dòng |
| Phụ thuộc | gd1_10 |
| Quyết định áp dụng | D-13, D-17 |
| Phụ thuộc mới | — |
| File đụng tới | MỚI `crates/ui/src/views/phone_library.rs`; SỬA `crates/ui/src/views/main_window.rs` (tab), `i18n/{vi,en}.toml` |

## 1. Làm gì (PROTOCOL mục 4.3)
- Tab phụ "Thư viện điện thoại": `VirtualList` tên, cỡ, ngày (mới nhất trước), nút làm mới.
- Chọn một / nhiều ⇒ "Tải về máy" ⇒ lệnh `Download`; mục hiện trong hàng đợi chiều ngược lại.
- Không có nút xoá (v1 không có). Offline ⇒ hiện danh sách lần cuối + ghi chú.

## 6. Nghiệm thu
- C1. Danh sách 2000 mục cuộn mượt (dựng ảo). `[sonnet-test]` + `[owner-qa]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
