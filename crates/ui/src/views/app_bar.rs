//! Thanh trên cao 36 px của cửa sổ chính, bê từ Groovex (`groove/crates/groove-app/src/views/app_bar.rs`): thu nhỏ,
//! phóng to / khôi phục, đóng ở góc phải ([`caption`]: icon `Minus`, `ArrowsExpand` / `ArrowsCompress`, `Close` 20 px trong
//! ô 36 px; rê xám nhạt, Đóng rê đỏ `red-600` icon trắng). Vùng trống kéo được cửa sổ, nhấp đúp phóng to / khôi phục: vùng
//! kéo là khung bọc ở `MainWindow` nằm sau thanh; nút `occlude` để không kéo theo.
//!
//! Windows / Linux: cửa sổ không có viền / thanh tiêu đề của OS (như Groovex), ba nút này là nút cửa sổ duy nhất, ô 36 px
//! sát mép phải. macOS ([`window_controls::NATIVE_CAPTION_BUTTONS`]): thêm đèn giao thông của hệ thống ở góc trái (đầu cột
//! trái chừa chỗ), ba nút này thành ô vuông 28 px bo 7 px cách nhau 4 px, chừa 8 px mép phải cho góc bo của cửa sổ, Đóng rê
//! xám như hai nút kia — y như thanh tiêu đề cửa sổ Cài đặt của fastcast (`fastcast/crates/ui/src/settings/title_bar.rs`).

use materiant::effect::ripple::{RippleConfig, RippleExt};
// `#[derive(IntoElement)]` trỏ crate `gpui`; bản không test chỉ có `materiant::gpui_kit`.
use materiant::gpui_kit as gpui;
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    App, Hsla, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, px, svg, white,
};
use materiant::overlay::tooltip::TooltipExt;
use materiant::theme::ActiveTheme;
use materiant::theme::color::hex;

use crate::assets::FerryIcon;
use crate::platform::window_controls;

pub const HEIGHT: f32 = 36.;
/// Nút cửa sổ: ô 36 px, icon 20 px.
const CONTROL_WIDTH: f32 = 36.;
const CAPTION_ICON: f32 = 20.;
/// macOS (như cửa sổ Cài đặt của fastcast / app Claude desktop): nút là ô vuông bo góc nhỏ hơn thanh, có khe giữa các nút,
/// chừa mép phải để góc bo của cửa sổ không cắt nút Đóng; Đóng rê xám (đỏ là quy ước của Windows).
const MAC_CONTROL: f32 = 28.;
const MAC_CONTROL_RADIUS: f32 = 7.;
const MAC_CONTROL_ICON: f32 = 18.;
const MAC_CONTROL_GAP: f32 = 4.;
const MAC_CONTROL_TRAIL: f32 = 8.;

/// Nhãn trợ năng / tooltip của ba nút (đã dịch).
pub struct CaptionLabels {
    pub minimize: SharedString,
    pub maximize: SharedString,
    pub restore: SharedString,
    pub close: SharedString,
}

#[derive(IntoElement)]
pub struct AppBar {
    maximized: bool,
    labels: CaptionLabels,
}

impl AppBar {
    pub fn new(maximized: bool, labels: CaptionLabels) -> AppBar {
        AppBar { maximized, labels }
    }
}

/// Ba nút cửa sổ ở góc phải.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caption {
    Minimize,
    Maximize,
    Close,
}

impl Caption {
    /// Phóng to `ArrowsExpand`, đang phóng to thì `ArrowsCompress` (khôi phục).
    pub fn icon(self, maximized: bool) -> FerryIcon {
        match self {
            Caption::Minimize => FerryIcon::Minus,
            Caption::Maximize if maximized => FerryIcon::ArrowsCompress,
            Caption::Maximize => FerryIcon::ArrowsExpand,
            Caption::Close => FerryIcon::Close,
        }
    }
}

/// Đỏ của nút Đóng khi rê chuột (`red-600`), icon trắng.
pub const CLOSE_HOVER: u32 = 0xdc2626;

/// Màu một nút cửa sổ như Windows: thu nhỏ / phóng to rê là lớp xám mờ theo màu chữ, nhấn nhạt hơn; Đóng rê nền đỏ, icon
/// trắng, nhấn đỏ hơi trong.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptionPaint {
    pub icon: Hsla,
    pub hover_bg: Hsla,
    pub hover_icon: Hsla,
    pub active_bg: Hsla,
}

impl CaptionPaint {
    pub fn resolve(caption: Caption, text: Hsla) -> CaptionPaint {
        match caption {
            Caption::Close if window_controls::NATIVE_CAPTION_BUTTONS => CaptionPaint::neutral(text),
            Caption::Close => {
                let red: Hsla = hex(CLOSE_HOVER).into();
                CaptionPaint { icon: text, hover_bg: red, hover_icon: white(), active_bg: red.opacity(0.9) }
            }
            _ => CaptionPaint::neutral(text),
        }
    }

    fn neutral(text: Hsla) -> CaptionPaint {
        CaptionPaint { icon: text, hover_bg: text.opacity(0.09), hover_icon: text, active_bg: text.opacity(0.06) }
    }
}

/// Nút cửa sổ rộng 36 px, cao bằng thanh, icon 20 px (macOS: ô 28 px bo góc, icon 18 px), ripple từ tâm. Bấm gọi [`window_controls`].
fn caption(id: &'static str, caption: Caption, maximized: bool, label: SharedString, cx: &App) -> impl IntoElement {
    let paint = CaptionPaint::resolve(caption, cx.theme().colors().text);
    let compact = window_controls::NATIVE_CAPTION_BUTTONS;
    let (w, h, icon) =
        if compact { (MAC_CONTROL, MAC_CONTROL, MAC_CONTROL_ICON) } else { (CONTROL_WIDTH, HEIGHT, CAPTION_ICON) };
    div()
        .id(id)
        .group(id)
        .occlude()
        .flex_none()
        .w(px(w))
        .h(px(h))
        // Trước `.ripple`: ripple chép bo góc để cắt vòng.
        .when(compact, |d| d.rounded(px(MAC_CONTROL_RADIUS)))
        .flex()
        .items_center()
        .justify_center()
        .hover(move |s| s.bg(paint.hover_bg))
        .active(move |s| s.bg(paint.active_bg))
        .ripple(RippleConfig::centered().color(paint.hover_icon))
        .on_click(move |_, window, _| match caption {
            Caption::Minimize => window_controls::minimize(window),
            Caption::Maximize => window_controls::toggle_maximize(window),
            Caption::Close => window_controls::close(window),
        })
        .child(
            svg()
                .path(caption.icon(maximized).path())
                .size(px(icon))
                .flex_none()
                .text_color(paint.icon)
                .group_hover(id, move |s| s.text_color(paint.hover_icon)),
        )
        .with_tooltip((id, 0usize), label)
}

impl RenderOnce for AppBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let CaptionLabels { minimize, maximize, restore, close } = self.labels;
        let toggle_label = if self.maximized { restore } else { maximize };
        div().id("app-bar").h(px(HEIGHT)).w_full().flex_none().flex().items_center().child(
            div()
                .ml_auto()
                .flex()
                .items_center()
                .h_full()
                .occlude()
                .when(window_controls::NATIVE_CAPTION_BUTTONS, |row| {
                    row.gap(px(MAC_CONTROL_GAP)).pr(px(MAC_CONTROL_TRAIL))
                })
                .child(caption("app-bar-minimize", Caption::Minimize, self.maximized, minimize, cx))
                .child(caption("app-bar-maximize", Caption::Maximize, self.maximized, toggle_label, cx))
                .child(caption("app-bar-close", Caption::Close, self.maximized, close, cx)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Đóng rê đỏ icon trắng; thu nhỏ / phóng to rê xám nhạt, giữ màu icon, nhấn nhạt hơn rê (như Groovex).
    #[test]
    fn caption_paint_matches_groovex() {
        let text: Hsla = hex(0xf5f5f5).into();
        let close = CaptionPaint::resolve(Caption::Close, text);
        assert_eq!(close.icon, text);
        if window_controls::NATIVE_CAPTION_BUTTONS {
            assert_eq!(close, CaptionPaint::neutral(text), "macOS: Đóng rê xám như cửa sổ Cài đặt của fastcast");
        } else {
            assert_eq!(close.hover_bg, Hsla::from(hex(CLOSE_HOVER)));
            assert_eq!(close.hover_icon, white());
        }
        for caption in [Caption::Minimize, Caption::Maximize] {
            let p = CaptionPaint::resolve(caption, text);
            assert_eq!(p.hover_icon, text);
            assert!(p.hover_bg.a > 0. && p.hover_bg.a < 0.15, "xám nhạt: {:?}", p.hover_bg);
            assert!(p.active_bg.a < p.hover_bg.a);
        }
    }

    #[test]
    fn caption_icons_follow_maximized_state() {
        for maximized in [false, true] {
            assert_eq!(Caption::Minimize.icon(maximized), FerryIcon::Minus);
            assert_eq!(Caption::Close.icon(maximized), FerryIcon::Close);
        }
        assert_eq!(Caption::Maximize.icon(false), FerryIcon::ArrowsExpand);
        assert_eq!(Caption::Maximize.icon(true), FerryIcon::ArrowsCompress);
    }
}
