//! Mã theo OS của cửa sổ chính, bê từ Groovex (`groove/crates/groove-app/src/platform`): nút cửa sổ tự vẽ
//! ([`window_controls`]), kéo cửa sổ không vẽ lại mỗi nhịp trên Windows ([`move_loop`]), kéo mép đổi cỡ trên Linux
//! ([`resize_edges`]).

pub mod move_loop;
pub mod resize_edges;
pub mod window_controls;
