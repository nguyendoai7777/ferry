//! Cửa sổ chính, vỏ như Groovex: cột điều hướng trái ([`SideNav`]: Gửi, Danh sách thiết bị, Cài đặt ở đáy), thanh trên
//! kéo được với ba nút cửa sổ ([`AppBar`]), nội dung bên phải. Trang Gửi (PROTOCOL 4.2): đầu thiết bị + vùng thả + khay chờ +
//! hàng đợi; "Gửi sang điện thoại…" chuyển sang trình duyệt điện thoại (4.6) ở chế độ chọn đích rồi quay lại; "Thêm điện
//! thoại" (4.4) là trang con của trang Gửi / Danh sách thiết bị. Danh sách thiết bị và Cài đặt (4.5) là mục của cột trái;
//! thư viện của một điện thoại (4.3) là trang con của Danh sách thiết bị ("Duyệt tệp"). Giữ trạng thái người dùng
//! chọn (khay, lô đang mở); trạng thái daemon nằm ở [`Model`].
//!
//! Mở lại như cũ (12/11/2026, `ferry_core::view`): [`MainWindow::restore`] dựng trang, lịch sử Lùi, khay, trình duyệt từ
//! `ViewState` daemon giữ; mỗi lần vẽ / cuộn / dời cửa sổ hẹn một lần gửi lại sau [`SAVE_DELAY`] (chỉ gửi khi khác bản
//! trước), và gửi ngay trước khi cửa sổ đóng (`window_controls::before_close`). Không giữ hộp thoại.

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use ferry_core::device::{DeviceId, DeviceState};
use ferry_core::i18n::I18n;
use ferry_core::ipc::{Action, CommandError, Outcome, PhoneOp, QueueOp};
use ferry_core::queue::BatchId;
use ferry_core::view::{BrowserState, PageState, TRAY_MAX, ViewState, WindowState};
use materiant::button::{Button, Size};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    AnyElement, App, AppContext, ClipboardEntry, Context, Entity, ExternalPaths, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, PathPromptOptions, Render, SharedString, Subscription, Task, WeakEntity, Window, WindowBounds, div,
    px,
};
use materiant::theme::ActiveTheme;
use materiant::toast::{Toast, ToastKind, ToastSpec, ToastViewport};
use materiant::window::DragRegionExt;

use super::add_phone::{AddPhoneEvent, AddPhoneView};
use super::app_bar::{self, AppBar, CaptionLabels};
use super::device_header::{Reconnect, device_header};
use super::devices::{DevicesEvent, DevicesPage};
use super::drop_zone::drop_zone;
use super::phone_browser::{BrowserEvent, BrowserMode, BrowserStart, PhoneBrowser};
use super::phone_library::PhoneLibrary;
use super::queue_list::{self, RowCommand, RowKind, queue_list};
use super::send_tray::{self, TrayEntry, lacks_dirs, send_tray};
use super::settings::{SettingsEvent, SettingsPage};
use super::side_nav::{self, Active, NavItem, SideNav};
use crate::fmt;
use crate::model::{Model, ModelEvent};

enum Page {
    Main,
    Browse {
        browser: Entity<PhoneBrowser>,
        _sub: Subscription,
        _watch: Subscription,
    },
    /// Danh sách thiết bị (mục cột trái).
    Devices {
        view: Entity<DevicesPage>,
        _sub: Subscription,
    },
    /// Thư viện của một điện thoại — trang con của Danh sách thiết bị.
    Library {
        library: Entity<PhoneLibrary>,
        device: DeviceId,
        _watch: Subscription,
    },
    /// `origin`: trang mở luồng này (Gửi hoặc Danh sách thiết bị) — xong / quay lại thì về đó.
    AddPhone {
        view: Entity<AddPhoneView>,
        origin: NavItem,
        _sub: Subscription,
    },
    Settings {
        view: Entity<SettingsPage>,
        _sub: Subscription,
    },
}

/// Trang ghi được vào lịch sử Lùi (như lịch sử của trình duyệt). Luồng tạm (chọn nơi gửi, Thêm điện thoại) không vào
/// lịch sử: xong / huỷ là về trang trước nó.
#[derive(Clone, Debug, PartialEq)]
enum Route {
    Main,
    Devices,
    Library(DeviceId),
    Settings,
}

/// Trần lịch sử Lùi.
const HISTORY_MAX: usize = ferry_core::view::HISTORY_MAX;
/// Gộp các lần đổi (cuộn, dời cửa sổ…) thành một lần gửi `SaveView`.
const SAVE_DELAY: Duration = Duration::from_millis(500);

impl Route {
    fn to_state(&self) -> PageState {
        match self {
            Route::Main => PageState::Main,
            Route::Devices => PageState::Devices,
            Route::Library(device) => PageState::Library { device: device.clone() },
            Route::Settings => PageState::Settings,
        }
    }

    fn from_state(p: &PageState) -> Option<Route> {
        match p {
            PageState::Main => Some(Route::Main),
            PageState::Devices => Some(Route::Devices),
            PageState::Library { device } => Some(Route::Library(device.clone())),
            PageState::Settings => Some(Route::Settings),
            PageState::Browse | PageState::AddPhone { .. } => None,
        }
    }
}

/// Trang mở lại + lịch sử Lùi: thư viện của máy không còn ghép ⇒ Danh sách thiết bị (và bỏ khỏi lịch sử); bước chọn
/// nơi gửi chỉ mở lại khi `can_browse` (còn khay chờ, đúng máy đang chọn), không thì trang Gửi.
fn plan_restore(view: &ViewState, paired: &HashSet<DeviceId>, can_browse: bool) -> (PageState, Vec<Route>) {
    let usable = |p: &PageState| !matches!(p, PageState::Library { device } if !paired.contains(device));
    let mut page = if usable(&view.page) { view.page.clone() } else { PageState::Devices };
    if page == PageState::Browse && !can_browse {
        page = PageState::Main;
    }
    let mut history: Vec<Route> = view.history.iter().filter(|p| usable(p)).filter_map(Route::from_state).collect();
    history.dedup();
    let current = Route::from_state(&page);
    while current.is_some() && history.last() == current.as_ref() {
        history.pop();
    }
    (page, history)
}

/// Khung cửa sổ hiện tại (phóng to ⇒ khung khi khôi phục). Wayland không cho biết / đặt vị trí: chỉ cỡ có nghĩa.
pub fn window_state(window: &Window) -> WindowState {
    let (b, maximized) = match window.window_bounds() {
        WindowBounds::Windowed(b) => (b, false),
        WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, true),
    };
    WindowState {
        x: b.origin.x.into(),
        y: b.origin.y.into(),
        width: b.size.width.into(),
        height: b.size.height.into(),
        maximized,
    }
}

impl Page {
    fn route(&self) -> Option<Route> {
        match self {
            Page::Main => Some(Route::Main),
            Page::Devices { .. } => Some(Route::Devices),
            Page::Library { device, .. } => Some(Route::Library(device.clone())),
            Page::Settings { .. } => Some(Route::Settings),
            Page::Browse { .. } | Page::AddPhone { .. } => None,
        }
    }

    fn to_state(&self) -> PageState {
        match self {
            Page::Browse { .. } => PageState::Browse,
            Page::AddPhone { origin, .. } => PageState::AddPhone { from_devices: *origin == NavItem::Devices },
            page => page.route().map_or(PageState::Main, |r| r.to_state()),
        }
    }

    fn active(&self) -> Active {
        match self {
            Page::Main | Page::Browse { .. } => Active::Item(NavItem::Send),
            Page::AddPhone { origin, .. } => Active::Item(*origin),
            Page::Devices { .. } | Page::Library { .. } => Active::Item(NavItem::Devices),
            Page::Settings { .. } => Active::Settings,
        }
    }
}

pub struct MainWindow {
    model: Entity<Model>,
    i18n: Rc<I18n>,
    tray: Vec<TrayEntry>,
    expanded: HashSet<BatchId>,
    page: Page,
    /// Trang đã đi qua, cuối là trang ngay trước — nút Lùi của thanh trên.
    history: Vec<Route>,
    /// Đang Lùi: đổi trang không ghi trang đang rời vào lịch sử.
    going_back: bool,
    focus: FocusHandle,
    /// `req` của `AddFiles` đang chờ trả lời.
    sending: Option<u64>,
    /// Thư mục đích đang gửi (nhớ khi daemon nhận).
    sending_dest: Option<(DeviceId, String)>,
    /// Tên cửa sổ đã đặt cho OS (taskbar, Alt+Tab) — không có thanh tiêu đề OS nên đặt bằng `set_window_title`.
    window_title: SharedString,
    /// Bản `ViewState` gửi daemon lần cuối — chỉ gửi lại khi khác.
    saved: Option<ViewState>,
    /// Lần gửi đã hẹn (`SAVE_DELAY`); `Some` thì lần đổi kế không hẹn thêm.
    save_task: Option<Task<()>>,
    _sub: Subscription,
    _bounds: Subscription,
}

impl Focusable for MainWindow {
    fn focus_handle(&self, _: &materiant::gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl MainWindow {
    pub fn new(model: Entity<Model>, i18n: Rc<I18n>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe_in(&model, window, |this, _, ev: &ModelEvent, window, cx| this.on_model(ev, window, cx));
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let bounds = cx.observe_window_bounds(window, |this, window, cx| this.schedule_save(window, cx));
        MainWindow {
            model,
            i18n,
            tray: Vec::new(),
            expanded: HashSet::new(),
            page: Page::Main,
            history: Vec::new(),
            going_back: false,
            focus,
            sending: None,
            sending_dest: None,
            window_title: SharedString::default(),
            saved: None,
            save_task: None,
            _sub: sub,
            _bounds: bounds,
        }
    }

    /// Dựng lại cửa sổ như lần trước (`DaemonToUi::Restore`, gọi trước khung đầu). Máy đã Quên / không còn ghép ⇒ trang
    /// Danh sách thiết bị thay cho thư viện của nó; nơi gửi chỉ mở lại khi còn khay chờ và đúng máy đang chọn.
    pub fn restore(&mut self, view: ViewState, window: &mut Window, cx: &mut Context<Self>) {
        let paired: HashSet<DeviceId> = self.model.read(cx).paired().into_iter().map(|d| d.id.clone()).collect();
        send_tray::add_paths(&mut self.tray, &view.tray);
        self.expanded = view.expanded.iter().cloned().collect();
        let current = self.model.read(cx).current().map(|d| d.id.clone());
        let can_browse =
            !self.tray.is_empty() && view.browser.as_ref().is_some_and(|b| Some(&b.device) == current.as_ref());
        let (page, history) = plan_restore(&view, &paired, can_browse);
        let browser = view.browser.clone();
        self.going_back = true;
        match page {
            PageState::Main => {}
            PageState::Browse => self.open_browser_with(browser, window, cx),
            PageState::Devices => self.open_devices(window, cx),
            PageState::Library { device } => self.open_library(device, browser, window, cx),
            PageState::Settings => self.open_settings(window, cx),
            PageState::AddPhone { from_devices } => {
                let origin = if from_devices { NavItem::Devices } else { NavItem::Send };
                self.open_add_phone(None, origin, window, cx);
            }
        }
        self.going_back = false;
        self.history = history;
        // Mốc so sánh là bản daemon đang giữ: trang đã lùi (máy bị Quên…) khác bản đó ⇒ lần lưu đầu gửi lại.
        self.saved = Some(view);
        cx.notify();
    }

    /// Trạng thái hiện tại để daemon giữ hộ.
    fn view_state(&self, window: Option<WindowState>, cx: &App) -> ViewState {
        let m = self.model.read(cx);
        let browser = match &self.page {
            Page::Browse { browser, .. } => Some(browser.read(cx).state()),
            Page::Library { library, .. } => Some(library.read(cx).browser().read(cx).state()),
            _ => None,
        };
        let mut expanded: Vec<BatchId> = self.expanded.iter().cloned().collect();
        expanded.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        ViewState {
            page: self.page.to_state(),
            history: self.history.iter().map(Route::to_state).collect(),
            selected: m.selected.clone(),
            tray: self.tray.iter().take(TRAY_MAX).map(|e| e.path.clone()).collect(),
            expanded,
            browser,
            clip: m.clip.as_ref().map(|c| c.to_state()),
            window,
        }
    }

    /// Gửi trạng thái cho daemon nếu khác lần trước. `closing`: cửa sổ sắp đóng ⇒ ghi cả khung cửa sổ vào `ui-state.json`.
    pub fn save_view(&mut self, window: &Window, closing: bool, cx: &mut Context<Self>) {
        self.save_task = None;
        let ws = window_state(window);
        let view = self.view_state(Some(ws), cx);
        if self.saved.as_ref() != Some(&view) {
            self.model.read(cx).save_view(view.clone());
            self.saved = Some(view);
        }
        if closing {
            self.model.update(cx, |m, _| m.remember_window(ws));
        }
    }

    /// Hẹn một lần gửi sau `SAVE_DELAY` (đã hẹn thì thôi). Không đổi gì ⇒ không vẽ ⇒ không hẹn: rảnh không tốn CPU.
    fn schedule_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.save_task.is_some() || !self.model.read(cx).connected() {
            return;
        }
        let timer = cx.background_executor().timer(SAVE_DELAY);
        self.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            timer.await;
            let _ = this.update_in(cx, |this, window, cx| this.save_view(window, false, cx));
        }));
    }

    /// Đổi trang: trang đang rời vào lịch sử (trừ luồng tạm, trừ lúc Lùi); về đúng trang ở đỉnh lịch sử (huỷ / xong luồng
    /// tạm) thì bỏ đỉnh đó để Lùi không về chính nó.
    fn show(&mut self, page: Page) {
        if !self.going_back
            && let Some(route) = self.page.route()
            && page.route().as_ref() != Some(&route)
        {
            self.history.push(route);
            if self.history.len() > HISTORY_MAX {
                self.history.remove(0);
            }
        }
        self.page = page;
        let current = self.page.route();
        while current.is_some() && self.history.last() == current.as_ref() {
            self.history.pop();
        }
    }

    /// Nút Lùi của thanh trên: về trang trước trong lịch sử.
    fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(route) = self.history.pop() else { return };
        self.going_back = true;
        match route {
            Route::Main => self.back_to_main(window, cx),
            Route::Devices => self.open_devices(window, cx),
            Route::Library(device) => self.open_library(device, None, window, cx),
            Route::Settings => self.open_settings(window, cx),
        }
        self.going_back = false;
        cx.notify();
    }

    /// Mục của cột trái.
    fn navigate(&mut self, item: NavItem, window: &mut Window, cx: &mut Context<Self>) {
        if self.page.active() == Active::Item(item)
            && !matches!(self.page, Page::Browse { .. } | Page::AddPhone { .. } | Page::Library { .. })
        {
            return;
        }
        match item {
            NavItem::Send => self.back_to_main(window, cx),
            NavItem::Devices => self.open_devices(window, cx),
        }
    }

    /// Trang Cài đặt (mục đáy cột trái, menu khay "Cài đặt…", `ferry-ui --settings`).
    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.page, Page::Settings { .. }) {
            return;
        }
        let i18n = I18n::new(self.i18n.language());
        let view = cx.new(|cx| SettingsPage::new(i18n, window, cx));
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &SettingsEvent, _, cx| match ev {
            SettingsEvent::LanguageChanged(language) => {
                this.i18n = Rc::new(I18n::new(*language));
                if cfg!(target_os = "macos") {
                    crate::app_menu::install(&this.i18n, cx);
                }
                cx.notify();
            }
        });
        self.show(Page::Settings { view, _sub: sub });
        cx.notify();
    }

    fn toast(&self, window: &mut Window, cx: &mut Context<Self>, kind: ToastKind, key: &str) {
        let text = self.i18n.t(key).to_owned();
        Toast::push(window, cx, ToastSpec::new(kind, text));
    }

    fn on_model(&mut self, ev: &ModelEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            ModelEvent::Changed => {
                if let Page::AddPhone { view, .. } = &self.page {
                    let devices = self.model.read(cx).snapshot.devices.clone();
                    let firewall = self.model.read(cx).snapshot.mdns_firewall.clone();
                    view.update(cx, |v, cx| {
                        v.set_devices(devices, cx);
                        v.set_firewall(firewall, window, cx);
                    });
                }
                cx.notify();
            }
            ModelEvent::Reply { req, outcome } if Some(*req) == self.sending => {
                self.sending = None;
                let dest = self.sending_dest.take();
                self.show(Page::Main);
                match outcome {
                    Outcome::Added { batch, files, skipped_non_video, unreadable } => {
                        if batch.is_some() {
                            let msg = fmt::plural(&self.i18n, "main.added", *files, &[("count", &files.to_string())]);
                            Toast::push(window, cx, ToastSpec::new(ToastKind::Success, msg));
                            if let Some((device, dir)) = dest {
                                self.model.update(cx, |m, _| m.remember_dest(&device, &dir));
                            }
                        } else {
                            self.toast(window, cx, ToastKind::Warning, "main.nothing_to_send");
                        }
                        if *skipped_non_video > 0 {
                            let msg = fmt::plural(
                                &self.i18n,
                                "main.skipped",
                                *skipped_non_video,
                                &[("count", &skipped_non_video.to_string())],
                            );
                            Toast::push(window, cx, ToastSpec::new(ToastKind::Info, msg));
                        }
                        if *unreadable > 0 {
                            let msg = fmt::plural(
                                &self.i18n,
                                "main.unreadable",
                                *unreadable,
                                &[("count", &unreadable.to_string())],
                            );
                            Toast::push(window, cx, ToastSpec::new(ToastKind::Warning, msg));
                        }
                        self.tray.clear();
                    }
                    Outcome::Failed { error } => {
                        let key = match error {
                            CommandError::Offline => "state.offline",
                            _ => "main.add_failed",
                        };
                        self.toast(window, cx, ToastKind::Danger, key);
                    }
                    _ => {}
                }
                cx.notify();
            }
            ModelEvent::Reply { req, outcome } => {
                if let Page::AddPhone { view, .. } = &self.page {
                    view.update(cx, |v, cx| v.on_reply(*req, outcome, window, cx));
                }
            }
        }
    }

    /// Danh sách thiết bị (mục cột trái).
    fn open_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (model, i18n) = (self.model.clone(), self.i18n.clone());
        let view = cx.new(|cx| DevicesPage::new(model, i18n, window, cx));
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &DevicesEvent, window, cx| match ev {
            DevicesEvent::AddPhone => this.open_add_phone(None, NavItem::Devices, window, cx),
            DevicesEvent::Repair(id) => {
                let state = this.model.read(cx).snapshot.devices.iter().find(|d| &d.id == id).map(|d| d.state);
                if let Some(state) = state {
                    this.reconnect(id.clone(), state, NavItem::Devices, window, cx);
                }
            }
            DevicesEvent::Browse(id) => this.open_library(id.clone(), None, window, cx),
        });
        self.show(Page::Devices { view, _sub: sub });
        cx.notify();
    }

    /// Thư viện của một điện thoại (PROTOCOL 4.3), mở từ Danh sách thiết bị. Chọn luôn máy đó: bộ nhớ đệm danh sách và
    /// trang Gửi theo máy đang chọn.
    fn open_library(
        &mut self,
        device: DeviceId,
        restore: Option<BrowserState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.model.update(cx, |m, cx| m.select(device.clone(), cx));
        let (model, i18n) = (self.model.clone(), self.i18n.clone());
        let library = cx.new(|cx| PhoneLibrary::new(model, i18n, device.clone(), restore, window, cx));
        // Cuộn / chọn / đổi thư mục trong trình duyệt chỉ vẽ lại nó ⇒ nghe nó để hẹn lưu.
        let browser = library.read(cx).browser().clone();
        let watch = cx.observe_in(&browser, window, |this, _, window, cx| this.schedule_save(window, cx));
        self.show(Page::Library { library, device, _watch: watch });
        cx.notify();
    }

    /// "Thêm điện thoại" (PROTOCOL 4.4). `repair`: mở thẳng bước PIN cho máy cần ghép lại / kết nối lại. `origin`: trang
    /// về khi xong hoặc quay lại.
    /// "Kết nối lại" / "Ghép lại" của máy đang chọn: cần ghép lại ⇒ bước PIN; chỉ Offline ⇒ daemon thử nối + quét ngay.
    fn reconnect_current(&mut self, origin: NavItem, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, state)) = self.model.read(cx).current().map(|d| (d.id.clone(), d.state)) else { return };
        self.reconnect(id, state, origin, window, cx);
    }

    fn reconnect(
        &mut self,
        id: DeviceId,
        state: DeviceState,
        origin: NavItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if state == DeviceState::NeedsRepair {
            self.open_add_phone(Some(id), origin, window, cx);
        } else {
            self.model.update(cx, |m, cx| m.reconnect(id, cx));
        }
    }

    fn open_add_phone(
        &mut self,
        repair: Option<DeviceId>,
        origin: NavItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.clone();
        let send: super::add_phone::SendPhoneOp =
            Rc::new(move |op: PhoneOp, cx: &mut materiant::gpui_kit::App| model.update(cx, |m, _| m.phone(op)));
        let devices = self.model.read(cx).snapshot.devices.clone();
        let i18n = self.i18n.clone();
        let firewall = self.model.read(cx).snapshot.mdns_firewall.clone();
        let view = cx.new(|cx| {
            let mut v = AddPhoneView::new(i18n, send, devices, repair, window, cx);
            v.set_firewall(firewall, window, cx);
            v
        });
        let sub = cx.subscribe_in(&view, window, move |this, _, ev: &AddPhoneEvent, window, cx| {
            if let AddPhoneEvent::Paired(id) = ev {
                let id = id.clone();
                this.model.update(cx, |m, cx| m.select(id, cx));
            }
            match origin {
                NavItem::Devices => this.open_devices(window, cx),
                NavItem::Send => this.back_to_main(window, cx),
            }
        });
        self.show(Page::AddPhone { view, origin, _sub: sub });
        cx.notify();
    }

    fn back_to_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show(Page::Main);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn add_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        if send_tray::add_paths(&mut self.tray, paths) > 0 {
            cx.notify();
        }
    }

    fn pick(&mut self, directories: bool, cx: &mut Context<Self>) {
        let picked =
            cx.prompt_for_paths(PathPromptOptions { files: !directories, directories, multiple: true, prompt: None });
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                let _ = this.update(cx, |this, cx| this.add_paths(&paths, cx));
            }
        })
        .detach();
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else { return };
        let mut paths = Vec::new();
        for entry in item.entries() {
            if let ClipboardEntry::ExternalPaths(p) = entry {
                paths.extend(p.paths().iter().cloned());
            }
        }
        self.add_paths(&paths, cx);
    }

    fn open_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_browser_with(None, window, cx);
    }

    /// `restore`: trình duyệt lần trước (mở lại cửa sổ đang ở bước chọn nơi gửi).
    fn open_browser_with(&mut self, restore: Option<BrowserState>, window: &mut Window, cx: &mut Context<Self>) {
        let (device, start) = {
            let m = self.model.read(cx);
            let Some(d) = m.current() else { return };
            let start = match restore {
                Some(b) if b.device == d.id => BrowserStart::Restore(b),
                _ => BrowserStart::Dir(m.last_dest(&d.id)),
            };
            (d.id.clone(), start)
        };
        if self.tray.is_empty() {
            return;
        }
        // App điện thoại chưa có `dirs` (§6.9): không chọn nơi lưu được, mọi thứ vào gốc ⇒ gửi luôn, khay đã báo.
        if self.model.read(cx).current().is_some_and(lacks_dirs) {
            self.send(String::new(), window, cx);
            return;
        }
        let mode = BrowserMode::Destination {
            what: send_tray::summary(&self.i18n, &self.tray),
            bytes: send_tray::known_size(&self.tray),
        };
        let (model, i18n) = (self.model.clone(), self.i18n.clone());
        let browser = cx.new(|cx| PhoneBrowser::new(model, i18n, device, mode, start, window, cx));
        let watch = cx.observe_in(&browser, window, |this, _, window, cx| this.schedule_save(window, cx));
        let sub = cx.subscribe_in(&browser, window, |this, _, ev: &BrowserEvent, window, cx| match ev {
            BrowserEvent::Cancel => {
                this.show(Page::Main);
                window.focus(&this.focus, cx);
                cx.notify();
            }
            BrowserEvent::Send { dest_dir } => this.send(dest_dir.clone(), window, cx),
            // Trang chọn đích không nhận dán tệp của máy (`computer_files` rỗng ngoài trang thư viện).
            BrowserEvent::PasteFiles { .. } => {}
        });
        self.show(Page::Browse { browser, _sub: sub, _watch: watch });
        cx.notify();
    }

    fn send(&mut self, dest_dir: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending.is_some() {
            return;
        }
        let Some(device) = self.model.read(cx).current().map(|d| d.id.clone()) else { return };
        let paths: Vec<PathBuf> = self.tray.iter().map(|e| e.path.clone()).collect();
        let op = QueueOp::AddFiles { device: device.clone(), paths, dest_dir: dest_dir.clone() };
        match self.model.update(cx, |m, _| m.queue(op)) {
            Some(req) => {
                self.sending = Some(req);
                self.sending_dest = Some((device, dest_dir));
            }
            None => {
                self.show(Page::Main);
                self.toast(window, cx, ToastKind::Danger, "main.no_daemon");
            }
        }
        cx.notify();
    }

    fn command(&mut self, cmd: RowCommand, cx: &mut Context<Self>) {
        match cmd {
            RowCommand::Item(id, action) => {
                self.model.update(cx, |m, _| m.queue(QueueOp::Item { id, action }));
            }
            RowCommand::Batch(batch, action) => {
                self.model.update(cx, |m, _| m.queue(QueueOp::Batch { batch, action }));
            }
            RowCommand::Toggle(RowKind::Batch { id, .. }) => {
                if !self.expanded.remove(&id) {
                    self.expanded.insert(id);
                }
            }
            RowCommand::Toggle(_) => {}
        }
        cx.notify();
    }
}

impl MainWindow {
    fn t(&self, key: &str) -> SharedString {
        SharedString::from(self.i18n.t(key).to_owned())
    }

    /// Chưa có điện thoại: chữ giữa vùng (trang Gửi).
    fn no_phone(&self, cx: &Context<Self>) -> AnyElement {
        let muted = cx.theme().colors().muted;
        div()
            .flex_1()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_1()
            .child(div().font_weight(cx.theme().title_weight()).child(self.t("main.no_phone")))
            .child(div().text_sm().text_color(muted).child(self.t("main.no_phone_hint")))
            .into_any_element()
    }

    /// Cột trái. Vùng kéo cửa sổ của đầu cột nằm ở [`Render::render`] (dải sau cột), không trong cột.
    fn render_nav(&self, cx: &mut Context<Self>) -> AnyElement {
        let (w1, w2) = (cx.weak_entity(), cx.weak_entity());
        SideNav::new(
            self.t("app.name"),
            self.page.active(),
            NavItem::ALL.iter().map(|item| self.t(item.label_key())).collect(),
            self.t("nav.settings"),
            move |item, window, cx| {
                let _ = w1.update(cx, |this, cx| this.navigate(item, window, cx));
            },
            move |window, cx| {
                let _ = w2.update(cx, |this, cx| this.open_settings(window, cx));
            },
        )
        .into_any_element()
    }

    /// Trang Gửi: đầu thiết bị + vùng thả + khay chờ + hàng đợi.
    fn render_send(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors().clone();
        let weak = cx.weak_entity();
        let i18n = self.i18n.clone();
        let (paired, selected, connected, device) = {
            let m = self.model.read(cx);
            let paired: Vec<_> = m.paired().into_iter().cloned().collect();
            let selected = m.selected.as_ref().and_then(|id| paired.iter().position(|d| &d.id == id));
            (paired, selected, m.connected(), m.current().cloned())
        };
        let has_phone = device.is_some();
        let no_dirs = device.as_ref().is_some_and(lacks_dirs);

        let header = {
            let w1 = weak.clone();
            let w2 = weak.clone();
            let w3 = weak.clone();
            let ids: Vec<DeviceId> = paired.iter().map(|d| d.id.clone()).collect();
            let reconnecting =
                selected.and_then(|i| paired.get(i)).is_some_and(|d| self.model.read(cx).is_reconnecting(&d.id));
            device_header(
                &i18n,
                &paired,
                selected,
                Rc::new(move |ix, _, cx| {
                    if let Some(id) = ids.get(ix).cloned() {
                        let _ = w1.update(cx, |this, cx| this.model.update(cx, |m, cx| m.select(id, cx)));
                    }
                }),
                Rc::new(move |window, cx| {
                    let _ = w2.update(cx, |this, cx| this.open_add_phone(None, NavItem::Send, window, cx));
                }),
                Reconnect {
                    busy: reconnecting,
                    on_click: Rc::new(move |window, cx| {
                        let _ = w3.update(cx, |this, cx| this.reconnect_current(NavItem::Send, window, cx));
                    }),
                },
                cx,
            )
        };

        let zone = {
            let (w1, w2, w3) = (weak.clone(), weak.clone(), weak.clone());
            drop_zone(
                &i18n,
                has_phone,
                Rc::new(move |_, cx| {
                    let _ = w1.update(cx, |this, cx| this.pick(false, cx));
                }),
                Rc::new(move |_, cx| {
                    let _ = w2.update(cx, |this, cx| this.pick(true, cx));
                }),
                Rc::new(move |paths: &ExternalPaths, _, cx| {
                    let paths = paths.paths().to_vec();
                    let _ = w3.update(cx, |this, cx| this.add_paths(&paths, cx));
                }),
                cx,
            )
        };

        let tray = (!self.tray.is_empty()).then(|| {
            let (w1, w2, w3) = (weak.clone(), weak.clone(), weak.clone());
            send_tray(
                &i18n,
                &self.tray,
                has_phone && connected,
                no_dirs,
                Rc::new(move |ix, _, cx| {
                    let _ = w1.update(cx, |this, cx| {
                        if ix < this.tray.len() {
                            this.tray.remove(ix);
                            cx.notify();
                        }
                    });
                }),
                Rc::new(move |_, cx| {
                    let _ = w2.update(cx, |this, cx| {
                        this.tray.clear();
                        cx.notify();
                    });
                }),
                Rc::new(move |window, cx| {
                    let _ = w3.update(cx, |this, cx| this.open_browser(window, cx));
                }),
                cx,
            )
        });

        let queue = match &device {
            None => self.no_phone(cx),
            Some(dev) => {
                let (can_pause, can_resume) = {
                    let s = &self.model.read(cx).snapshot;
                    (s.can_pause_all, s.can_resume_all)
                };
                let rows = {
                    let snap = &self.model.read(cx).snapshot;
                    queue_list::build_rows(&i18n, snap, dev, &self.expanded)
                };
                let empty = rows.is_empty();
                let w = weak.clone();
                let w_all = weak.clone();
                let on_command: Rc<dyn Fn(RowCommand, &mut Window, &mut materiant::gpui_kit::App)> =
                    Rc::new(move |cmd, _, cx| {
                        let _ = w.update(cx, |this, cx| this.command(cmd, cx));
                    });
                let all = |action: Action, id: &'static str, key: &str| {
                    let w = w_all.clone();
                    Button::new(id, i18n.t(key)).size(Size::Sm).text().on_click(move |_, _, cx| {
                        let _ = w.update(cx, |this, cx| {
                            this.model.update(cx, |m, _| m.queue(QueueOp::All { action }));
                        });
                    })
                };
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_4()
                            .h(px(32.))
                            .child(
                                div()
                                    .font_weight(cx.theme().title_weight())
                                    .child(SharedString::from(i18n.t("queue.title").to_owned())),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .when(can_pause, |d| d.child(all(Action::Pause, "all-pause", "queue.pause_all")))
                                    .when(can_resume, |d| {
                                        d.child(all(Action::Resume, "all-resume", "queue.resume_all"))
                                    }),
                            ),
                    )
                    .child(if empty {
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_sm()
                            .text_color(c.muted)
                            .child(SharedString::from(i18n.t("queue.empty").to_owned()))
                            .into_any_element()
                    } else {
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .child(queue_list(Rc::new(rows), i18n.clone(), on_command))
                            .into_any_element()
                    })
                    .into_any_element()
            }
        };

        let w_paste = weak.clone();
        let w_focus = weak;
        div()
            .id("main-page")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .on_mouse_down(materiant::gpui_kit::MouseButton::Left, move |_, window, cx| {
                // Ô con vừa nhận focus (pha bubble) thì giữ: chỉ lấy focus khi bấm vào chỗ trống.
                let handle = w_focus.read_with(cx, |this, _| this.focus.clone()).ok();
                if let Some(h) = handle
                    && !h.contains_focused(window, cx)
                {
                    window.focus(&h, cx);
                }
            })
            .on_key_down(move |ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.modifiers.secondary(cx) && ev.keystroke.key == "v" {
                    let _ = w_paste.update(cx, |this, cx| this.paste(cx));
                }
            })
            .on_drop({
                let w = cx.weak_entity();
                move |paths: &ExternalPaths, _, cx| {
                    let paths = paths.paths().to_vec();
                    let _ = w.update(cx, |this, cx| this.add_paths(&paths, cx));
                }
            })
            .child(header)
            .child(zone)
            .children(tray)
            .child(queue)
            .into_any_element()
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.schedule_save(window, cx);
        let title = self.t("app.name");
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
        let c = cx.theme().colors().clone();
        let content = match &self.page {
            Page::Main => self.render_send(cx),
            Page::Browse { browser, .. } => browser.clone().into_any_element(),
            Page::Devices { view, .. } => view.clone().into_any_element(),
            Page::Library { library, .. } => library.clone().into_any_element(),
            Page::AddPhone { view, .. } => view.clone().into_any_element(),
            Page::Settings { view, .. } => view.clone().into_any_element(),
        };
        // Vùng kéo cửa sổ của đầu cột trái: dải phủ sau cột (như Groovex), mục bấm được nằm dưới dải nên không bị che.
        let nav = div()
            .relative()
            .w(px(side_nav::WIDTH))
            .h_full()
            .flex_none()
            .child(div().absolute().top_0().left_0().w_full().h(px(side_nav::HEADER_HEIGHT)).drag_region())
            .child(div().size_full().child(self.render_nav(cx)));
        let labels = CaptionLabels {
            minimize: self.t("app_bar.minimize"),
            maximize: self.t("app_bar.maximize"),
            restore: self.t("app_bar.restore"),
            close: self.t("app_bar.close"),
        };
        // Thanh trên: khung bọc là vùng kéo (nhấp đúp phóng to), ba nút `occlude` chặn kéo.
        let bar = div().w_full().h(px(app_bar::HEIGHT)).flex_none().drag_region().child(
            AppBar::new(window.is_maximized(), labels).back(
                !self.history.is_empty(),
                self.t("app_bar.back"),
                cx.listener(|this, _, window, cx| this.go_back(window, cx)),
            ),
        );
        div()
            .id("main-window")
            .relative()
            .size_full()
            .flex()
            .bg(c.surface)
            .text_color(c.text)
            .font_family(cx.theme().font_family())
            .text_size(cx.theme().font_size())
            .child(nav)
            // Kiểu lưới của Groovex: các vùng sát nhau, ngăn bằng đường kẻ 1 px.
            .child(div().w(px(1.)).h_full().flex_none().bg(c.border))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(bar)
                    .child(div().w_full().h(px(1.)).flex_none().bg(c.border))
                    .child(
                        div()
                            .id("content")
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .child(div().size_full().flex().flex_col().child(content))
                            .child(ToastViewport::new()),
                    ),
            )
            // Linux: viền tự vẽ không tự đổi cỡ được — dải kéo mép, trên cùng mọi lớp.
            .children(crate::platform::resize_edges::overlay(window))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(s: &str) -> DeviceId {
        DeviceId(s.into())
    }

    #[test]
    fn restore_keeps_pages_of_paired_phones() {
        let paired = HashSet::from([dev("p")]);
        let view = ViewState {
            page: PageState::Library { device: dev("p") },
            history: vec![PageState::Main, PageState::Settings, PageState::Devices],
            ..ViewState::default()
        };
        let (page, history) = plan_restore(&view, &paired, false);
        assert_eq!(page, PageState::Library { device: dev("p") });
        assert_eq!(history, [Route::Main, Route::Settings, Route::Devices]);
        for r in &history {
            assert_eq!(Route::from_state(&r.to_state()).as_ref(), Some(r));
        }
    }

    #[test]
    fn forgotten_phone_falls_back_to_the_devices_page() {
        let view = ViewState {
            page: PageState::Library { device: dev("gone") },
            history: vec![PageState::Main, PageState::Library { device: dev("gone") }, PageState::Devices],
            ..ViewState::default()
        };
        let (page, history) = plan_restore(&view, &HashSet::new(), false);
        assert_eq!(page, PageState::Devices);
        // Lùi không về chính trang đang đứng, không về thư viện đã mất.
        assert_eq!(history, [Route::Main]);
    }

    #[test]
    fn choosing_a_destination_reopens_only_with_a_tray() {
        let view = ViewState { page: PageState::Browse, history: vec![PageState::Main], ..ViewState::default() };
        assert_eq!(plan_restore(&view, &HashSet::new(), true).0, PageState::Browse);
        let (page, history) = plan_restore(&view, &HashSet::new(), false);
        assert_eq!(page, PageState::Main);
        assert!(history.is_empty());
    }
}
