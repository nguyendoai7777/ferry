# ferry-gd1_13 — watch-folder

| Thuộc tính | Giá trị |
|---|---|
| Trạng thái | ☐ |
| Repo | ferry |
| Model | opus |
| Cỡ | S — ~300 dòng |
| Phụ thuộc | gd1_04, gd1_12 |
| Quyết định áp dụng | W-1, W-3 |
| Phụ thuộc mới | `notify` (CC0-1.0, groove đã dùng; soát `cargo deny`) |
| File đụng tới | MỚI `crates/core/src/watch.rs`; SỬA `crates/daemon/src/engine.rs` |

## 1. Làm gì
- Theo dõi một thư mục (không đệ quy sâu tuỳ ý, không quét ổ — §2.8): tệp video mới **ổn định** (cỡ + mtime không đổi 5 s, mở ghi được) ⇒ `AddFiles` cho thiết bị mặc định.
- Tệp đã có trong hàng đợi / "Đã gửi" không thêm lại (khoá: đường dẫn + cỡ + mtime).
- Không giữ CPU khi rảnh: chỉ sự kiện OS, đồng hồ ổn định chạy khi có tệp chờ.
- Bổ sung W-3: tệp mới trong thư mục theo dõi gửi với `dir = <tên thư mục theo dõi>/<đường con>` (giữ cây như copy/paste).

## 6. Nghiệm thu
- C1. Chép tệp 2 GB vào thư mục ⇒ chỉ vào hàng đợi sau khi chép xong. `[opus-unit]`
- C2. Đổi tên tệp trong thư mục ⇒ không thêm trùng. `[opus-unit]`
- C-W3. Tệp mới ở thư mục con của thư mục theo dõi ⇒ `dir` đúng. `[opus-unit]`

## 9. Bàn giao thực tế
## 10. Đề xuất cải tiến
