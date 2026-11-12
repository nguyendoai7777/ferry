//! Lõi ferry (`CLAUDE.md` mục Bố cục): mọi thứ không phải UI. Daemon `ferry` chạy nó; `ferry-ui` chỉ dùng
//! `i18n`, `config` (đọc), `paths`, `logging`, `ipc`.
//!
//! Mọi module đã khai sẵn từ gd1_01 để task sau chỉ thêm mã vào tệp của mình, không giẫm `lib.rs`. Chủ của từng module
//! ghi ở đầu tệp đó.

pub mod browse;
pub mod client;
pub mod config;
pub mod device;
pub mod discovery;
pub mod export;
pub mod firewall;
pub mod i18n;
pub mod ipc;
pub mod logging;
pub mod pairing;
pub mod paths;
pub mod protocol;
pub mod queue;
pub mod transfer;
pub mod view;

/// Phiên bản app (theo workspace), in vào log và `Hello`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
