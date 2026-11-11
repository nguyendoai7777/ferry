//! Bê từ Groovex (linux-resize 09/11/2026): kéo mép đổi cỡ cửa sổ chính trên Linux.
//!
//! Cửa sổ chính tự vẽ viền (`WindowDecorations::Client`; GNOME / Mutter còn không có viền server cho Wayland). gpui-pre-linux
//! 0.3.10 không tự hit-test mép cho viền tự vẽ: app phải tự bắt chuột ở mép rồi gọi `Window::start_window_resize` (Wayland
//! `xdg_toplevel.resize`, X11 `_NET_WM_MOVERESIZE`). Thiếu dải này ⇒ cả bản debug lẫn bản cài không kéo mép
//! được. Dải [`EDGE`] px mép trong (không có bóng / viền vô hình bên ngoài như Zed) phủ lên nội dung, `occlude` để nút / thanh
//! cuộn sát mép không nhận lần bấm đó; góc dài [`CORNER`] px theo hai cạnh. Mép đang dính (phóng to, ghép nửa màn) thì bỏ.
//! Windows: GPUI tự hit-test mép cho viền tự vẽ; macOS viền do OS ⇒ không vẽ gì.

use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    AnyElement, CursorStyle, MouseButton, Pixels, Point, ResizeEdge, Size, Tiling, Window, div, px,
};

/// Bề dày dải kéo mép (px logic), như 6 px mép trong của Windows trừ một px để thumb thanh cuộn sát mép phải còn bấm được.
pub const EDGE: f32 = 5.;

/// Độ dài vùng góc dọc theo mỗi cạnh: góc dễ trúng hơn chỉ một ô vuông [`EDGE`].
pub const CORNER: f32 = 12.;

/// Mép đổi cỡ ở `pos` (toạ độ trong cửa sổ cỡ `size`), bỏ mép đang dính. `None` = không ở mép.
pub fn edge_at(pos: Point<Pixels>, size: Size<Pixels>, tiling: Tiling) -> Option<ResizeEdge> {
    let (x, y, w, h) = (f32::from(pos.x), f32::from(pos.y), f32::from(size.width), f32::from(size.height));
    if x < 0. || y < 0. || x >= w || y >= h {
        return None;
    }
    let top = !tiling.top && y < EDGE;
    let bottom = !tiling.bottom && y >= h - EDGE;
    let left = !tiling.left && x < EDGE;
    let right = !tiling.right && x >= w - EDGE;
    // Góc: ở dải của một cạnh và trong CORNER px của cạnh kề (cạnh kề không dính).
    let near_top = !tiling.top && y < CORNER;
    let near_bottom = !tiling.bottom && y >= h - CORNER;
    let near_left = !tiling.left && x < CORNER;
    let near_right = !tiling.right && x >= w - CORNER;
    let corner = |a: bool, b: bool, near_a: bool, near_b: bool| (a && near_b) || (b && near_a);
    if corner(top, left, near_top, near_left) {
        Some(ResizeEdge::TopLeft)
    } else if corner(top, right, near_top, near_right) {
        Some(ResizeEdge::TopRight)
    } else if corner(bottom, left, near_bottom, near_left) {
        Some(ResizeEdge::BottomLeft)
    } else if corner(bottom, right, near_bottom, near_right) {
        Some(ResizeEdge::BottomRight)
    } else if top {
        Some(ResizeEdge::Top)
    } else if bottom {
        Some(ResizeEdge::Bottom)
    } else if left {
        Some(ResizeEdge::Left)
    } else if right {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

fn cursor(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}

/// Mép nào đang dính, hoặc `None` khi cửa sổ không tự vẽ viền (không cần dải kéo).
fn tiling(window: &Window) -> Option<Tiling> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    match window.window_decorations() {
        materiant::gpui_kit::Decorations::Client { .. } if window.is_maximized() => None,
        materiant::gpui_kit::Decorations::Client { tiling } => Some(tiling),
        materiant::gpui_kit::Decorations::Server => None,
    }
}

/// Dải kéo mép phủ trên cùng khung gốc `relative` của cửa sổ chính. Mỗi dải chỉ lo con trỏ và chặn bấm; mép thật lấy lại từ
/// [`edge_at`] lúc bấm để góc trùng đúng vùng tính toán.
pub fn overlay(window: &Window) -> Option<AnyElement> {
    let tiling = tiling(window)?;
    if tiling == Tiling::tiled() {
        return None;
    }
    let strip = |id: &'static str, edge: ResizeEdge| {
        div().id(id).absolute().occlude().cursor(cursor(edge)).on_mouse_down(MouseButton::Left, move |e, window, cx| {
            if let Some(edge) = edge_at(e.position, window.viewport_size(), tiling) {
                cx.stop_propagation();
                window.start_window_resize(edge);
            }
        })
    };
    let (edge, corner) = (px(EDGE), px(CORNER));
    let root = div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .when(!tiling.top, |d| d.child(strip("resize-top", ResizeEdge::Top).top_0().left_0().right_0().h(edge)))
        .when(!tiling.bottom, |d| {
            d.child(strip("resize-bottom", ResizeEdge::Bottom).bottom_0().left_0().right_0().h(edge))
        })
        .when(!tiling.left, |d| d.child(strip("resize-left", ResizeEdge::Left).left_0().top_0().bottom_0().w(edge)))
        .when(!tiling.right, |d| d.child(strip("resize-right", ResizeEdge::Right).right_0().top_0().bottom_0().w(edge)))
        // Góc vẽ sau cạnh để con trỏ chéo thắng ở chỗ chồng; ô vuông CORNER phủ cả hai nhánh chữ L (phần ở giữa ô không thuộc
        // mép nào thì `edge_at` trả `None`, bấm không làm gì).
        .when(!tiling.top && !tiling.left, |d| {
            d.child(strip("resize-tl", ResizeEdge::TopLeft).top_0().left_0().size(corner))
        })
        .when(!tiling.top && !tiling.right, |d| {
            d.child(strip("resize-tr", ResizeEdge::TopRight).top_0().right_0().size(corner))
        })
        .when(!tiling.bottom && !tiling.left, |d| {
            d.child(strip("resize-bl", ResizeEdge::BottomLeft).bottom_0().left_0().size(corner))
        })
        .when(!tiling.bottom && !tiling.right, |d| {
            d.child(strip("resize-br", ResizeEdge::BottomRight).bottom_0().right_0().size(corner))
        });
    Some(root.into_any_element())
}

#[cfg(test)]
mod tests {
    use materiant::gpui_kit::{ResizeEdge, Tiling, point, px, size};

    use super::edge_at;

    fn at(x: f32, y: f32, tiling: Tiling) -> Option<ResizeEdge> {
        edge_at(point(px(x), px(y)), size(px(800.), px(600.)), tiling)
    }

    #[test]
    fn edges_and_corners_of_a_floating_window() {
        let free = Tiling::default();
        assert_eq!(at(400., 300., free), None, "giữa cửa sổ");
        assert_eq!(at(400., 6., free), None, "ngay dưới dải mép trên");
        assert_eq!(at(400., 0., free), Some(ResizeEdge::Top));
        assert_eq!(at(400., 599., free), Some(ResizeEdge::Bottom));
        assert_eq!(at(0., 300., free), Some(ResizeEdge::Left));
        assert_eq!(at(796., 300., free), Some(ResizeEdge::Right));
        assert_eq!(at(1., 1., free), Some(ResizeEdge::TopLeft));
        assert_eq!(at(10., 2., free), Some(ResizeEdge::TopLeft), "góc kéo dài dọc cạnh trên");
        assert_eq!(at(2., 10., free), Some(ResizeEdge::TopLeft), "góc kéo dài dọc cạnh trái");
        assert_eq!(at(10., 10., free), None, "trong ô góc nhưng không ở dải mép nào");
        assert_eq!(at(799., 0., free), Some(ResizeEdge::TopRight));
        assert_eq!(at(0., 599., free), Some(ResizeEdge::BottomLeft));
        assert_eq!(at(790., 597., free), Some(ResizeEdge::BottomRight));
        assert_eq!(at(800., 300., free), None, "ngoài cửa sổ");
    }

    #[test]
    fn tiled_edges_do_not_resize() {
        // Ghép nửa trái màn: trên, dưới, trái dính — chỉ còn mép phải, góc thành mép phải.
        let left_half = Tiling { top: true, bottom: true, left: true, right: false };
        assert_eq!(at(400., 0., left_half), None);
        assert_eq!(at(0., 300., left_half), None);
        assert_eq!(at(799., 300., left_half), Some(ResizeEdge::Right));
        assert_eq!(at(799., 0., left_half), Some(ResizeEdge::Right));
        assert_eq!(at(0., 0., Tiling::tiled()), None);
    }
}
