//! Trang "Thư viện điện thoại" (PROTOCOL 4.3 / 4.6, W-3, W-4): bọc [`PhoneBrowser`] ở chế độ thư viện và nhận thả / dán
//! tệp vào thư mục đang mở (gửi vào đúng thư mục đó, §6.9). Không thanh trên riêng: về Danh sách thiết bị bằng cột trái.
//! Không dựng danh sách riêng; mạng đi qua IPC của [`Model`].

use std::path::PathBuf;
use std::rc::Rc;

use ferry_core::device::DeviceId;
use ferry_core::i18n::I18n;
use ferry_core::ipc::{CommandError, Outcome, QueueOp};
use ferry_core::view::BrowserState;
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    AppContext, ClipboardEntry, Context, Entity, ExternalPaths, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    MouseButton, Render, Subscription, Window, div, px,
};
use materiant::toast::{Toast, ToastKind, ToastSpec};

use super::phone_browser::{BrowserEvent, BrowserMode, BrowserStart, PhoneBrowser};
use crate::fmt;
use crate::model::{Model, ModelEvent};

pub struct PhoneLibrary {
    model: Entity<Model>,
    i18n: Rc<I18n>,
    device: DeviceId,
    browser: Entity<PhoneBrowser>,
    /// `req` của `AddFiles` đang chờ trả lời.
    sending: Option<u64>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

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
        restore: Option<BrowserState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let start = restore.map_or(BrowserStart::Dir(String::new()), BrowserStart::Restore);
        let browser = {
            let (m, i, d) = (model.clone(), i18n.clone(), device.clone());
            cx.new(|cx| PhoneBrowser::new(m, i, d, BrowserMode::Library, start, window, cx))
        };
        let subs = vec![
            cx.subscribe_in(&model, window, |this, _, ev: &ModelEvent, window, cx| this.on_model(ev, window, cx)),
            // Dán (menu / phím tắt) tệp sao chép ở trình quản lý tệp của máy ⇒ gửi vào thư mục đã chọn.
            cx.subscribe_in(&browser, window, |this, _, ev: &BrowserEvent, window, cx| {
                if let BrowserEvent::PasteFiles { paths, dest_dir } = ev {
                    this.send_to(paths.clone(), dest_dir.clone(), window, cx);
                }
            }),
        ];
        let focus = cx.focus_handle();
        PhoneLibrary { model, i18n, device, browser, sending: None, focus, _subs: subs }
    }

    /// Trình duyệt bên trong (đọc `state()` khi lưu trạng thái cửa sổ).
    pub fn browser(&self) -> &Entity<PhoneBrowser> {
        &self.browser
    }

    fn toast(&self, window: &mut Window, cx: &mut Context<Self>, kind: ToastKind, text: String) {
        Toast::push(window, cx, ToastSpec::new(kind, text));
    }

    fn on_model(&mut self, ev: &ModelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ModelEvent::Reply { req, outcome } = ev else { return };
        if Some(*req) == self.sending {
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

    /// Thả / dán tệp: gửi vào thư mục đang mở (§6.9); nhóm "Khác" không phải đích.
    fn add_paths(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() || self.sending.is_some() {
            return;
        }
        let Some(dest_dir) = self.browser.read(cx).destination().map(str::to_owned) else {
            self.toast(window, cx, ToastKind::Warning, self.i18n.t("library.drop_other").to_owned());
            return;
        };
        self.send_to(paths, dest_dir, window, cx);
    }

    /// Gửi `paths` vào `dest_dir` trên điện thoại (thả / dán).
    fn send_to(&mut self, paths: Vec<PathBuf>, dest_dir: String, window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() || self.sending.is_some() {
            return;
        }
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
            .child(div().flex_1().min_h(px(0.)).child(self.browser.clone()))
    }
}
