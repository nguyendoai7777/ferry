//! Màn hình của `ferry-ui`. Mỗi màn một tệp / thư mục; `main.rs` chỉ dựng `MainWindow` (đăng ký view ở đây).
#![allow(clippy::type_complexity)] // callback `Rc<dyn Fn(..)>` của các view con: một kiểu mỗi chỗ, alias không làm rõ hơn

pub mod add_phone;
/// Thanh trên: vùng kéo + ba nút cửa sổ (bê từ Groovex).
pub mod app_bar;
pub mod device_header;
/// Màn Danh sách thiết bị (điện thoại đã ghép: ghép lại, đổi tên, xoá, duyệt tệp).
pub mod devices;
pub mod drop_zone;
pub mod main_window;
pub mod phone_browser;
pub mod phone_library;
pub mod queue_list;
pub mod quiet_button;
pub mod send_tray;
/// Trang Cài đặt trong cửa sổ chính (gd1_12; vỏ Groovex).
pub mod settings;
/// Cột điều hướng trái (theo Groovex).
pub mod side_nav;
