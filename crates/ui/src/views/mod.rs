//! Màn hình của `ferry-ui`. Mỗi màn một tệp / thư mục; `main.rs` chỉ dựng `MainWindow` (đăng ký view ở đây).
#![allow(clippy::type_complexity)] // callback `Rc<dyn Fn(..)>` của các view con: một kiểu mỗi chỗ, alias không làm rõ hơn

pub mod add_phone;
pub mod device_header;
pub mod drop_zone;
pub mod main_window;
pub mod phone_browser;
pub mod phone_library;
pub mod queue_list;
pub mod send_tray;
/// Cửa sổ cài đặt riêng (gd1_12).
pub mod settings;
/// Thanh tiêu đề tự vẽ khi hệ thống không cấp viền (GNOME Wayland).
pub mod title_bar;
