//! Cửa sổ chính (PROTOCOL 4.2): đầu thiết bị + vùng thả + khay chờ + hàng đợi; "Gửi sang điện thoại…" chuyển sang
//! trình duyệt điện thoại (4.6) ở chế độ chọn đích rồi quay lại; "Thư viện điện thoại" (4.3) và "Thêm điện thoại" (4.4)
//! là trang con thay nội dung cửa sổ. Giữ trạng thái người dùng chọn (khay, lô đang mở);
//! trạng thái daemon nằm ở [`Model`].

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use ferry_core::device::DeviceId;
use ferry_core::i18n::I18n;
use ferry_core::ipc::{Action, CommandError, Outcome, PhoneOp, QueueOp};
use ferry_core::queue::BatchId;
use materiant::button::{Button, Size};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    AppContext, ClipboardEntry, Context, Entity, ExternalPaths, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    PathPromptOptions, Render, SharedString, Subscription, WeakEntity, Window, div, px,
};
use materiant::theme::ActiveTheme;
use materiant::toast::{Toast, ToastKind, ToastSpec, ToastViewport};

use super::add_phone::{AddPhoneEvent, AddPhoneView};
use super::device_header::device_header;
use super::drop_zone::drop_zone;
use super::phone_browser::{BrowserEvent, BrowserMode, PhoneBrowser};
use super::phone_library::{LibraryEvent, PhoneLibrary};
use super::queue_list::{self, RowCommand, RowKind, queue_list};
use super::send_tray::{self, TrayEntry, send_tray};
use crate::fmt;
use crate::model::{Model, ModelEvent};

enum Page {
    Main,
    Browse { browser: Entity<PhoneBrowser>, _sub: Subscription },
    Library { library: Entity<PhoneLibrary>, _sub: Subscription },
    AddPhone { view: Entity<AddPhoneView>, _sub: Subscription },
}

pub struct MainWindow {
    model: Entity<Model>,
    i18n: Rc<I18n>,
    tray: Vec<TrayEntry>,
    expanded: HashSet<BatchId>,
    sent_open: bool,
    page: Page,
    focus: FocusHandle,
    /// `req` của `AddFiles` đang chờ trả lời.
    sending: Option<u64>,
    /// Thư mục đích đang gửi (nhớ khi daemon nhận).
    sending_dest: Option<(DeviceId, String)>,
    _sub: Subscription,
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
        MainWindow {
            model,
            i18n,
            tray: Vec::new(),
            expanded: HashSet::new(),
            sent_open: false,
            page: Page::Main,
            focus,
            sending: None,
            sending_dest: None,
            _sub: sub,
        }
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
                    view.update(cx, |v, cx| v.set_devices(devices, cx));
                }
                cx.notify();
            }
            ModelEvent::Reply { req, outcome } if Some(*req) == self.sending => {
                self.sending = None;
                let dest = self.sending_dest.take();
                self.page = Page::Main;
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

    /// Mở thư viện của điện thoại đang chọn (PROTOCOL 4.3).
    fn open_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(device) = self.model.read(cx).current().map(|d| d.id.clone()) else { return };
        let (model, i18n) = (self.model.clone(), self.i18n.clone());
        let library = cx.new(|cx| PhoneLibrary::new(model, i18n, device, window, cx));
        let sub = cx.subscribe_in(&library, window, |this, _, ev: &LibraryEvent, window, cx| match ev {
            LibraryEvent::Back => this.back_to_main(window, cx),
        });
        self.page = Page::Library { library, _sub: sub };
        cx.notify();
    }

    /// "Thêm điện thoại" (PROTOCOL 4.4). `repair`: mở thẳng bước PIN cho máy cần ghép lại.
    fn open_add_phone(&mut self, repair: Option<DeviceId>, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let send: super::add_phone::SendPhoneOp =
            Rc::new(move |op: PhoneOp, cx: &mut materiant::gpui_kit::App| model.update(cx, |m, _| m.phone(op)));
        let devices = self.model.read(cx).snapshot.devices.clone();
        let i18n = self.i18n.clone();
        let view = cx.new(|cx| AddPhoneView::new(i18n, send, devices, repair, window, cx));
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &AddPhoneEvent, window, cx| {
            if let AddPhoneEvent::Paired(id) = ev {
                let id = id.clone();
                this.model.update(cx, |m, cx| m.select(id, cx));
            }
            this.back_to_main(window, cx);
        });
        self.page = Page::AddPhone { view, _sub: sub };
        cx.notify();
    }

    fn back_to_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.page = Page::Main;
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
        let (device, start) = {
            let m = self.model.read(cx);
            let Some(d) = m.current() else { return };
            (d.id.clone(), m.last_dest(&d.id))
        };
        if self.tray.is_empty() {
            return;
        }
        let mode = BrowserMode::Destination {
            what: send_tray::summary(&self.i18n, &self.tray),
            bytes: send_tray::known_size(&self.tray),
        };
        let (model, i18n) = (self.model.clone(), self.i18n.clone());
        let browser = cx.new(|cx| PhoneBrowser::new(model, i18n, device, mode, start, window, cx));
        let sub = cx.subscribe_in(&browser, window, |this, _, ev: &BrowserEvent, window, cx| match ev {
            BrowserEvent::Cancel => {
                this.page = Page::Main;
                window.focus(&this.focus, cx);
                cx.notify();
            }
            BrowserEvent::Send { dest_dir } => this.send(dest_dir.clone(), window, cx),
        });
        self.page = Page::Browse { browser, _sub: sub };
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
                self.page = Page::Main;
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
            RowCommand::Toggle(RowKind::SentHeader { .. }) => self.sent_open = !self.sent_open,
            RowCommand::Toggle(_) => {}
        }
        cx.notify();
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors().clone();
        let root = div()
            .size_full()
            .relative()
            .bg(c.surface)
            .text_color(c.text)
            .font_family(cx.theme().font_family())
            .child(ToastViewport::new());
        match &self.page {
            Page::Browse { browser, .. } => {
                return root.child(div().size_full().child(browser.clone())).into_any_element();
            }
            Page::Library { library, .. } => {
                return root.child(div().size_full().child(library.clone())).into_any_element();
            }
            Page::AddPhone { view, .. } => return root.child(div().size_full().child(view.clone())).into_any_element(),
            Page::Main => {}
        }
        let weak = cx.weak_entity();
        let i18n = self.i18n.clone();
        let (paired, selected, connected, device) = {
            let m = self.model.read(cx);
            let paired: Vec<_> = m.paired().into_iter().cloned().collect();
            let selected = m.selected.as_ref().and_then(|id| paired.iter().position(|d| &d.id == id));
            (paired, selected, m.connected(), m.current().cloned())
        };
        let has_phone = device.is_some();

        let header = {
            let w1 = weak.clone();
            let w2 = weak.clone();
            let ids: Vec<DeviceId> = paired.iter().map(|d| d.id.clone()).collect();
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
                    let _ = w2.update(cx, |this, cx| this.open_add_phone(None, window, cx));
                }),
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
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_1()
                .child(
                    div()
                        .font_weight(cx.theme().title_weight())
                        .child(SharedString::from(i18n.t("main.no_phone").to_owned())),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(c.muted)
                        .child(SharedString::from(i18n.t("main.no_phone_hint").to_owned())),
                )
                .into_any_element(),
            Some(dev) => {
                let (can_pause, can_resume) = {
                    let s = &self.model.read(cx).snapshot;
                    (s.can_pause_all, s.can_resume_all)
                };
                let rows = {
                    let snap = &self.model.read(cx).snapshot;
                    queue_list::build_rows(&i18n, snap, dev, &self.expanded, self.sent_open)
                };
                let empty = rows.is_empty();
                let w = weak.clone();
                let w_all = weak.clone();
                let w_lib = weak.clone();
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
                                    .child(
                                        Button::new("open-library", i18n.t("main.library"))
                                            .size(Size::Sm)
                                            .text()
                                            .on_click(move |_, window, cx| {
                                                let _ = w_lib.update(cx, |this, cx| this.open_library(window, cx));
                                            }),
                                    )
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
        root.child(
            div()
                .id("main-page")
                .track_focus(&self.focus)
                .size_full()
                .flex()
                .flex_col()
                .on_mouse_down(materiant::gpui_kit::MouseButton::Left, move |_, window, cx| {
                    let handle = w_focus.read_with(cx, |this, _| this.focus.clone()).ok();
                    if let Some(h) = handle {
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
                .child(queue),
        )
        .into_any_element()
    }
}
