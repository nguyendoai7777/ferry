//! Cột điều hướng trái của cửa sổ chính, theo cột trái của Groovex (`groove/crates/groove-app/src/views/side_nav.rs`):
//! đầu cột cao 36 px có tên app (vùng kéo cửa sổ nằm ở `MainWindow`, sau cột), các mục cao 42 px với vạch trái 3 px trượt
//! theo mục đang chọn (`materiant::tabs::InkBar`), Cài đặt ở đáy cột. Mỗi hàng là một nút: Tab đi qua từng hàng, Enter /
//! Space chọn, vòng focus khi đi bằng phím. Không có nút thu cột như Groovex: ferry chỉ có hai mục, cửa sổ nhỏ.

use std::rc::Rc;

use materiant::effect::ripple::{RippleConfig, RippleExt};
use materiant::gpui_kit::base::Button;
// `#[derive(IntoElement)]` trỏ crate `gpui`; bản không test chỉ có `materiant::gpui_kit`.
use materiant::gpui_kit as gpui;
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    AnyElement, App, ClickEvent, ElementId, Hsla, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window,
    div, px, svg, transparent_black,
};
use materiant::style::ActiveTokens;
use materiant::tabs::InkBar;
use materiant::theme::color::hex;
use materiant::theme::{ActiveTheme, Neutral};

use crate::assets::FerryIcon;

pub const WIDTH: f32 = 200.;
pub const HEADER_HEIGHT: f32 = 36.;
pub const ITEM_HEIGHT: f32 = 42.;
pub const ACTIVE_BAR: f32 = 3.;
const ICON_SIZE: f32 = 18.;

/// Mục điều hướng chính. Thêm điện thoại / chọn thư mục đích là luồng con mở từ trang Gửi (có nút lùi riêng) nên không
/// thành mục ở đây.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavItem {
    /// Vùng thả + khay chờ + hàng đợi (PROTOCOL 4.2).
    Send,
    /// Thư viện điện thoại (4.3).
    Library,
}

impl NavItem {
    pub const ALL: [NavItem; 2] = [NavItem::Send, NavItem::Library];

    pub fn icon(self) -> FerryIcon {
        match self {
            NavItem::Send => FerryIcon::Send,
            NavItem::Library => FerryIcon::Phone,
        }
    }

    pub fn label_key(self) -> &'static str {
        match self {
            NavItem::Send => "nav.send",
            NavItem::Library => "nav.library",
        }
    }
}

/// Mục đang tô sáng.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Active {
    Item(NavItem),
    Settings,
}

type Handler<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct SideNav {
    title: SharedString,
    active: Active,
    /// Nhãn đã dịch theo [`NavItem::ALL`], rồi nhãn Cài đặt.
    labels: Vec<SharedString>,
    settings_label: SharedString,
    on_select: Handler<NavItem>,
    on_settings: Handler<()>,
}

impl SideNav {
    pub fn new(
        title: SharedString,
        active: Active,
        labels: Vec<SharedString>,
        settings_label: SharedString,
        on_select: impl Fn(NavItem, &mut Window, &mut App) + 'static,
        on_settings: impl Fn(&mut Window, &mut App) + 'static,
    ) -> SideNav {
        SideNav {
            title,
            active,
            labels,
            settings_label,
            on_select: Rc::new(on_select),
            on_settings: Rc::new(move |(), window, cx| on_settings(window, cx)),
        }
    }
}

impl RenderOnce for SideNav {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let c = cx.theme().colors().clone();
        let header = div()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .px_4()
            .child(div().flex_1().min_w_0().truncate().text_color(c.text).child(self.title));

        let items: Vec<AnyElement> = NavItem::ALL
            .iter()
            .enumerate()
            .map(|(ix, &item)| {
                let on_select = self.on_select.clone();
                nav_button(
                    ("side-nav-item", ix),
                    item.icon(),
                    self.labels.get(ix).cloned().unwrap_or_default(),
                    self.active == Active::Item(item),
                    Rc::new(move |(), window, cx| on_select(item, window, cx)),
                    window,
                    cx,
                )
            })
            .collect();
        // Vạch của mục đang chọn: một phần tử trượt giữa các hàng (hàng cao đều nên khỏi đo).
        let ink = NavItem::ALL.iter().position(|&item| self.active == Active::Item(item)).map(|ix| {
            InkBar::new("side-nav-ink", ix as f32 * ITEM_HEIGHT, ITEM_HEIGHT)
                .vertical()
                .thickness(ACTIVE_BAR)
                .color(c.primary)
                .build(window, cx)
        });
        let settings = nav_button(
            ("side-nav-settings", 0),
            FerryIcon::Settings,
            self.settings_label,
            self.active == Active::Settings,
            self.on_settings,
            window,
            cx,
        );
        // Cài đặt ở đáy không dùng chung vạch trượt với các mục trên: tự vẽ vạch khi đang mở.
        let settings_bar = (self.active == Active::Settings)
            .then(|| div().absolute().top_0().left_0().bottom_0().w(px(ACTIVE_BAR)).bg(c.primary));

        div()
            .id("side-nav")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(side_nav_surface(cx))
            .child(header)
            .child(div().relative().flex_1().flex().flex_col().overflow_hidden().children(items).children(ink))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .border_t_1()
                    .border_color(c.border.opacity(0.3))
                    .child(settings)
                    .children(settings_bar),
            )
    }
}

/// Nền cột trái: một bậc tối / sáng hơn nội dung (cửa sổ đục, không mờ như Groovex).
pub fn side_nav_surface(cx: &App) -> Hsla {
    cx.theme().colors().layer(Neutral::N50)
}

/// Màu một hàng như Groovex: đang chọn = icon màu nhấn + chữ chính; còn lại = `muted`, rê sáng lên, nền và vạch trái nhạt.
#[derive(Clone, Copy)]
struct NavInk {
    icon: Hsla,
    label: Hsla,
    hover: Hsla,
    hover_bar: Hsla,
    active_bg: Hsla,
    hover_bg: Hsla,
}

impl NavInk {
    fn resolve(active: bool, cx: &App) -> NavInk {
        let theme = cx.theme();
        let c = theme.colors();
        let secondary: Hsla = if theme.is_dark() { hex(0xd4d4d8).into() } else { c.text };
        NavInk {
            icon: if active { c.primary } else { c.muted },
            label: if active { c.text } else { c.muted },
            hover: secondary,
            hover_bar: c.border.opacity(0.7),
            active_bg: c.layer(Neutral::N200),
            hover_bg: c.layer(Neutral::N100),
        }
    }
}

/// Một hàng cao 42 px toàn bề ngang, vạch trái 3 px: icon + chữ.
fn nav_button(
    id: (&'static str, usize),
    icon: FerryIcon,
    label: SharedString,
    active: bool,
    on_click: Handler<()>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let ink = NavInk::resolve(active, cx);
    let element_id = ElementId::from((SharedString::from(id.0), id.1));
    let focus = window
        .use_keyed_state(ElementId::from((SharedString::from(format!("{}-focus", id.0)), id.1)), cx, |_, cx| {
            cx.focus_handle()
        })
        .read(cx)
        .clone();
    let ring = materiant::style::focus_visible(&focus, window, cx);
    let group = SharedString::from(format!("{}-{}-group", id.0, id.1));
    let glyph = svg()
        .path(icon.path())
        .flex_none()
        .size(px(ICON_SIZE))
        .text_color(ink.icon)
        .when(!active, |s| s.group_hover(group.clone(), move |s| s.text_color(ink.hover)));
    Button::new(element_id)
        .track_focus(&focus)
        .selected(active)
        .accessibility_label(label.clone())
        .relative()
        .group(group.clone())
        .w_full()
        .h(px(ITEM_HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .cursor_pointer()
        .border_l(px(ACTIVE_BAR))
        .border_color(transparent_black())
        .justify_start()
        .gap(px(12.))
        .pl(px(13.))
        .pr_4()
        .when(active, |d| d.bg(ink.active_bg))
        .when(!active, |d| d.hover(move |s| s.bg(ink.hover_bg).border_color(ink.hover_bar)))
        .on_click(move |_: &ClickEvent, window, cx| on_click((), window, cx))
        .child(
            div()
                .id(ElementId::from((SharedString::from(format!("{}-ripple", id.0)), id.1)))
                .absolute()
                .inset_0()
                .ripple(RippleConfig::default()),
        )
        .child(glyph)
        .when(ring, |d| {
            d.child(
                div()
                    .absolute()
                    .top(px(2.))
                    .bottom(px(2.))
                    .left(px(2.))
                    .right(px(2.))
                    .border_2()
                    .rounded(px(4.))
                    .border_color(cx.focus_ring_color()),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_sm()
                .line_height(px(20.))
                .text_color(ink.label)
                .when(!active, |t| t.group_hover(group.clone(), move |s| s.text_color(ink.hover)))
                .child(label),
        )
        .into_any_element()
}
