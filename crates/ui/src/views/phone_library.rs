//! Tab "Thư viện điện thoại" (PROTOCOL 4.3 / 4.6, W-3, W-4): bọc [`PhoneBrowser`] ở chế độ thư viện, thêm thanh trên
//! ("Quay lại", "Tải về máy" cho các video đang chọn) và nhận thả / dán tệp vào thư mục đang mở (gửi vào đúng thư mục
//! đó, §6.9). Không dựng danh sách riêng; mạng đi qua IPC của [`Model`].

use std::path::PathBuf;
use std::rc::Rc;

use ferry_core::device::DeviceId;
use ferry_core::i18n::I18n;
use ferry_core::ipc::{CommandError, LibraryOp, Outcome, QueueOp};
use materiant::button::{Button, IconButton, Size};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    AppContext, ClipboardEntry, Context, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, MouseButton, Render, SharedString, Subscription, Window, div, px,
};
use materiant::icon::IconName;
use materiant::theme::ActiveTheme;
use materiant::toast::{Toast, ToastKind, ToastSpec};

use super::phone_browser::{BrowserMode, PhoneBrowser};
use crate::fmt;
use crate::model::{Model, ModelEvent};

pub enum LibraryEvent {
    /// "Quay lại" về màn chính.
    Back,
}

pub struct PhoneLibrary {
    model: Entity<Model>,
    i18n: Rc<I18n>,
    device: DeviceId,
    browser: Entity<PhoneBrowser>,
    /// `req` của `Download` / `AddFiles` đang chờ trả lời.
    downloading: Option<u64>,
    sending: Option<u64>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<LibraryEvent> for PhoneLibrary {}

impl Focusable for PhoneLibrary {
    fn focus_handle(&self, _: &materiant::gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PhoneLibrary {
    pub fn new(
        model: Entity<Model>,
        i18n: Rc<I18n>,
        device: DeviceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let browser = {
            let (m, i, d) = (model.clone(), i18n.clone(), device.clone());
            cx.new(|cx| PhoneBrowser::new(m, i, d, BrowserMode::Library, String::new(), window, cx))
        };
        let subs = vec![
            // Chọn dòng / đổi thư mục chỉ `notify` ở browser; cần vẽ lại nút "Tải về máy".
            cx.observe(&browser, |_, _, cx| cx.notify()),
            cx.subscribe_in(&model, window, |this, _, ev: &ModelEvent, window, cx| this.on_model(ev, window, cx)),
        ];
        let focus = cx.focus_handle();
        PhoneLibrary { model, i18n, device, browser, downloading: None, sending: None, focus, _subs: subs }
    }

    fn toast(&self, window: &mut Window, cx: &mut Context<Self>, kind: ToastKind, text: String) {
        Toast::push(window, cx, ToastSpec::new(kind, text));
    }

    fn on_model(&mut self, ev: &ModelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ModelEvent::Reply { req, outcome } = ev else { return };
        if Some(*req) == self.downloading {
            self.downloading = None;
            match outcome {
                Outcome::DownloadsAdded { added, already_queued, missing } => {
                    let i = &self.i18n;
                    if *added > 0 {
                        let msg = fmt::plural(i, "library.queued", *added, &[("count", &added.to_string())]);
                        self.toast(window, cx, ToastKind::Success, msg);
                    }
                    if *already_queued > 0 {
                        let msg = fmt::plural(
                            i,
                            "library.already",
                            *already_queued,
                            &[("count", &already_queued.to_string())],
                        );
                        self.toast(window, cx, ToastKind::Info, msg);
                    }
                    if *missing > 0 {
                        let msg = fmt::plural(i, "library.missing", *missing, &[("count", &missing.to_string())]);
                        self.toast(window, cx, ToastKind::Warning, msg);
                    }
                }
                Outcome::Failed { error: CommandError::Offline } => {
                    self.toast(window, cx, ToastKind::Danger, self.i18n.t("state.offline").to_owned())
                }
                _ => self.toast(window, cx, ToastKind::Danger, self.i18n.t("library.failed").to_owned()),
            }
            cx.notify();
        } else if Some(*req) == self.sending {
            self.sending = None;
            match outcome {
                Outcome::Added { batch: Some(_), files, .. } => {
                    let msg = fmt::plural(&self.i18n, "main.added", *files, &[("count", &files.to_string())]);
                    self.toast(window, cx, ToastKind::Success, msg);
                }
                Outcome::Added { batch: None, .. } => {
                    self.toast(window, cx, ToastKind::Warning, self.i18n.t("main.nothing_to_send").to_owned())
                }
                Outcome::Failed { error: CommandError::Offline } => {
                    self.toast(window, cx, ToastKind::Danger, self.i18n.t("state.offline").to_owned())
                }
                _ => self.toast(window, cx, ToastKind::Danger, self.i18n.t("main.add_failed").to_owned()),
            }
            cx.notify();
        }
    }

    fn download(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.downloading.is_some() {
            return;
        }
        let remote_ids: Vec<String> = self.browser.read(cx).selected_items().iter().map(|i| i.id.clone()).collect();
        if remote_ids.is_empty() {
            return;
        }
        let device = self.device.clone();
        match self.model.update(cx, |m, _| m.library(LibraryOp::Download { device, remote_ids })) {
            Some(req) => self.downloading = Some(req),
            None => self.toast(window, cx, ToastKind::Danger, self.i18n.t("main.no_daemon").to_owned()),
        }
        cx.notify();
    }

    /// Thả / dán tệp: gửi vào thư mục đang mở (§6.9); nhóm "Khác" không phải đích.
    fn add_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() || self.sending.is_some() {
            return;
        }
        let Some(dest_dir) = self.browser.read(cx).destination().map(str::to_owned) else {
            self.toast(window, cx, ToastKind::Warning, self.i18n.t("library.drop_other").to_owned());
            return;
        };
        let op = QueueOp::AddFiles { device: self.device.clone(), paths, dest_dir };
        match self.model.update(cx, |m, _| m.queue(op)) {
            Some(req) => self.sending = Some(req),
            None => self.toast(window, cx, ToastKind::Danger, self.i18n.t("main.no_daemon").to_owned()),
        }
        cx.notify();
    }

    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else { return };
        let mut paths = Vec::new();
        for entry in item.entries() {
            if let ClipboardEntry::ExternalPaths(p) = entry {
                paths.extend(p.paths().iter().cloned());
            }
        }
        self.add_paths(paths, window, cx);
    }
}

impl Render for PhoneLibrary {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors().clone();
        let t = |k: &str| SharedString::from(self.i18n.t(k).to_owned());
        let (count, offline) = {
            let b = self.browser.read(cx);
            (b.selected_items().len(), b.offline)
        };
        let label = if count > 0 {
            SharedString::from(self.i18n.t_args("library.download_n", &[("count", &count.to_string())]))
        } else {
            t("library.download")
        };
        let bar = div()
            .flex()
            .items_center()
            .gap_2()
            // Nút Lùi có ô đệm riêng: mép trái 4 px để mũi tên thẳng hàng với nội dung bên dưới; mép phải giữ 16 px.
            .pl(px(4.))
            .pr_4()
            .h(px(44.))
            .border_b_1()
            .border_color(c.border)
            .child(
                IconButton::new("lib-back", IconName::ChevronLeft)
                    .accessibility_label(t("library.back"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(LibraryEvent::Back))),
            )
            .child(div().font_weight(cx.theme().title_weight()).child(t("library.title")))
            .child(div().flex_1().min_w_0().truncate().text_xs().text_color(c.muted).child(t("library.drop_hint")))
            .child(
                Button::new("lib-download", label)
                    .size(Size::Sm)
                    .disabled(count == 0 || offline)
                    .loading(self.downloading.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.download(window, cx))),
            );
        let weak = cx.weak_entity();
        let w_focus = weak.clone();
        div()
            .id("phone-library")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            // Pha bubble: ô con (ô tìm, ô đổi tên) vừa nhận focus thì giữ, chỉ lấy focus khi bấm vào chỗ trống.
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if let Ok(h) = w_focus.read_with(cx, |this, _| this.focus.clone())
                    && !h.contains_focused(window, cx)
                {
                    window.focus(&h, cx);
                }
            })
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.modifiers.secondary(cx) && ev.keystroke.key == "v" {
                    this.paste(window, cx);
                }
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.add_paths(paths.paths().to_vec(), window, cx);
            }))
            .child(bar)
            .child(div().flex_1().min_h(px(0.)).child(self.browser.clone()))
    }
}
