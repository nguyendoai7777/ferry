//! Màn "Thêm điện thoại" (PROTOCOL 4.4, ghép PIN mục 6.3): hướng dẫn bật Sync WiFi → danh sách điện thoại dò thấy chưa
//! ghép (cập nhật trực tiếp từ snapshot) hoặc nhập địa chỉ tay → ô PIN 6 số → `Pair`. Xong ⇒ [`AddPhoneEvent::Paired`].
//!
//! View không tự nối IPC: chủ (cửa sổ chính) đưa vào `send` (gửi `PhoneOp`, trả `req`), gọi [`AddPhoneView::set_devices`]
//! mỗi khi có `Snapshot` mới và [`AddPhoneView::on_reply`] với mỗi `Reply`. PIN không giữ lại sau khi gửi (xoá ô ngay).

use std::rc::Rc;
use std::time::{Duration, Instant};

use ferry_core::device::{DeviceId, DeviceState};
use ferry_core::i18n::I18n;
use ferry_core::ipc::{CommandError, DeviceView, Outcome, PhoneOp, Pin};
use materiant::badge::Badge;
use materiant::button::{Button, IconButton, Size};
use materiant::card::{Card, CardGroup, CardItem};
use materiant::field::{FieldEvent, TextField};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    AnyElement, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, SharedString,
    Subscription, WeakEntity, Window, div, px,
};
use materiant::icon::IconName;
use materiant::progress::ProgressCircular;
use materiant::style::Accent;
use materiant::theme::ActiveTheme;

/// Số chữ số PIN (PROTOCOL 6.3).
pub const PIN_LEN: usize = 6;
/// Điện thoại không gửi `Retry-After` ⇒ daemon đã đổi thành 60 s; đây chỉ là trần chống số vô lý.
const MAX_LOCK_SECS: u64 = 3600;

pub enum AddPhoneEvent {
    /// Ghép xong: cửa sổ chính chọn máy này.
    Paired(DeviceId),
    /// Nút "Quay lại" ở bước đầu.
    Back,
}

/// Gửi một lệnh điện thoại sang daemon, trả `req` của lệnh (chủ tự đánh số); `None` = chưa nối được daemon.
pub type SendPhoneOp = Rc<dyn Fn(PhoneOp, &mut materiant::gpui_kit::App) -> Option<u64>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Pick,
    Manual,
    Pin,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    Address,
    Pair,
}

struct Pending {
    req: u64,
    kind: PendingKind,
}

pub struct AddPhoneView {
    i18n: Rc<I18n>,
    send: SendPhoneOp,
    devices: Vec<DeviceView>,
    step: Step,
    /// Máy đang nhập PIN: id + tên hiển thị.
    target: Option<(DeviceId, String)>,
    pending: Option<Pending>,
    error: Option<SharedString>,
    /// `RateLimited`: tới lúc này mới cho nhập lại.
    locked_until: Option<Instant>,
    pin: Entity<TextField>,
    address: Entity<TextField>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<AddPhoneEvent> for AddPhoneView {}

impl Focusable for AddPhoneView {
    fn focus_handle(&self, _cx: &materiant::gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

// ---- Hàm thuần (test được, không cần GPUI) ----

/// Ô PIN: giữ chữ số ASCII, bỏ khoảng trắng / dấu gạch / chữ, cắt còn [`PIN_LEN`] ("123 456" ⇒ "123456").
pub fn sanitize_pin(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).take(PIN_LEN).collect()
}

/// Điện thoại hiện ở danh sách: dò thấy chưa ghép, hoặc cần ghép lại; đang dò thấy (`visible`) xếp trước.
pub fn pairable(devices: &[DeviceView]) -> Vec<&DeviceView> {
    let mut list: Vec<&DeviceView> =
        devices.iter().filter(|d| matches!(d.state, DeviceState::Discovered | DeviceState::NeedsRepair)).collect();
    list.sort_by_key(|d| !d.visible);
    list
}

/// Số giây còn phải chờ, làm tròn lên (0 = hết khoá).
pub fn lock_remaining_secs(until: Instant, now: Instant) -> u64 {
    let left = until.saturating_duration_since(now);
    left.as_secs() + u64::from(left.subsec_nanos() > 0)
}

/// Lỗi lệnh ⇒ chữ nói việc cần làm (§2.6). `address_step`: lệnh là nhập địa chỉ tay (khác ghép PIN).
pub fn error_message(i18n: &I18n, error: &CommandError, address_step: bool, now_left_secs: u64) -> String {
    let key = |k: &str| i18n.t(k).to_owned();
    match error {
        CommandError::WrongPin => key("add_phone.err_wrong_pin"),
        CommandError::RateLimited { retry_after_secs } => {
            let secs = if now_left_secs > 0 { now_left_secs } else { (*retry_after_secs).min(MAX_LOCK_SECS) };
            i18n.t_args("add_phone.err_rate_limited", &[("seconds", &secs.to_string())])
        }
        CommandError::OtherPhone => key("add_phone.err_other_phone"),
        CommandError::Keyring => key("add_phone.err_keyring"),
        CommandError::UnknownInterface { name } => i18n.t_args("add_phone.err_unknown_interface", &[("name", name)]),
        CommandError::InvalidInput if address_step => key("add_phone.err_invalid_address"),
        CommandError::Offline => key("add_phone.err_offline"),
        CommandError::Unreachable | CommandError::NotPaired | CommandError::UnknownDevice => {
            key("add_phone.err_unreachable")
        }
        // `NotWritable` chỉ từ lệnh cài đặt (gd1_12), không tới màn này.
        CommandError::InvalidInput | CommandError::NotWritable | CommandError::Internal => {
            key("add_phone.err_internal")
        }
    }
}

impl AddPhoneView {
    /// `repair`: mở thẳng ở bước PIN cho máy `NeedsRepair` này (PROTOCOL 4.4).
    pub fn new(
        i18n: Rc<I18n>,
        send: SendPhoneOp,
        devices: Vec<DeviceView>,
        repair: Option<DeviceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let pin_placeholder = i18n.t("add_phone.pin_label").to_owned();
        let pin = cx.new(|cx| {
            TextField::new("add-phone-pin", window, cx)
                .placeholder(pin_placeholder)
                .size(materiant::field::FieldSize::Lg)
        });
        let address_placeholder = i18n.t("add_phone.manual_placeholder").to_owned();
        let address = cx.new(|cx| TextField::new("add-phone-address", window, cx).placeholder(address_placeholder));

        let subs = vec![
            cx.subscribe_in(&pin, window, |this, field, event: &FieldEvent, window, cx| match event {
                FieldEvent::Change(text) => {
                    let clean = sanitize_pin(text);
                    if clean.as_str() != text.as_ref() {
                        let clean = SharedString::from(clean);
                        field.update(cx, |f, cx| f.set_value(clean, window, cx));
                    }
                    this.error = None;
                    cx.notify();
                }
                FieldEvent::Submit(_) => this.submit_pin(window, cx),
            }),
            cx.subscribe_in(&address, window, |this, _, event: &FieldEvent, _, cx| match event {
                FieldEvent::Submit(_) => this.submit_address(cx),
                FieldEvent::Change(_) => {
                    this.error = None;
                    cx.notify();
                }
            }),
        ];

        let mut view = AddPhoneView {
            i18n,
            send,
            devices,
            step: Step::Pick,
            target: None,
            pending: None,
            error: None,
            locked_until: None,
            pin,
            address,
            focus: cx.focus_handle(),
            _subs: subs,
        };
        if let Some(id) = repair {
            let name = view.devices.iter().find(|d| d.id == id).map(|d| d.name.clone()).unwrap_or_default();
            view.go_pin(id, name, window, cx);
        }
        view
    }

    /// Snapshot mới: danh sách ở bước 2 cập nhật trực tiếp.
    pub fn set_devices(&mut self, devices: Vec<DeviceView>, cx: &mut Context<Self>) {
        if self.devices != devices {
            self.devices = devices;
            cx.notify();
        }
    }

    /// `Reply` từ daemon; bỏ qua nếu không phải của lệnh đang chờ.
    pub fn on_reply(&mut self, req: u64, outcome: &Outcome, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pending) = self.pending.as_ref().filter(|p| p.req == req) else { return };
        let kind = pending.kind;
        self.pending = None;
        match outcome {
            Outcome::Found { device, name } => self.go_pin(device.clone(), name.clone(), window, cx),
            Outcome::Paired { device, .. } => cx.emit(AddPhoneEvent::Paired(device.clone())),
            Outcome::Failed { error } => {
                if let CommandError::RateLimited { retry_after_secs } = error {
                    self.lock_for(Duration::from_secs((*retry_after_secs).min(MAX_LOCK_SECS)), cx);
                }
                let left = self.locked_until.map_or(0, |u| lock_remaining_secs(u, Instant::now()));
                self.error = Some(error_message(&self.i18n, error, kind == PendingKind::Address, left).into());
                if matches!(error, CommandError::WrongPin) {
                    {
                        let h = self.pin.focus_handle(cx);
                        window.focus(&h, cx);
                    }
                }
            }
            // Các kết quả khác không thuộc lệnh của màn này.
            _ => {}
        }
        cx.notify();
    }

    fn go_pin(&mut self, id: DeviceId, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.step = Step::Pin;
        self.target = Some((id, name));
        self.error = None;
        self.pin.update(cx, |f, cx| f.set_value("", window, cx));
        {
            let h = self.pin.focus_handle(cx);
            window.focus(&h, cx);
        }
        cx.notify();
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        match self.step {
            Step::Pick => cx.emit(AddPhoneEvent::Back),
            Step::Manual | Step::Pin => self.step = Step::Pick,
        }
        cx.notify();
    }

    /// Khoá nhập tới hết `wait`, đếm ngược mỗi giây rồi mở lại.
    fn lock_for(&mut self, wait: Duration, cx: &mut Context<Self>) {
        self.locked_until = Some(Instant::now() + wait);
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                executor.timer(Duration::from_secs(1)).await;
                let Ok(still) = this.update(cx, |this, cx| {
                    let Some(until) = this.locked_until else { return false };
                    let left = lock_remaining_secs(until, Instant::now());
                    if left == 0 {
                        this.locked_until = None;
                        this.error = None;
                    }
                    cx.notify();
                    left > 0
                }) else {
                    return;
                };
                if !still {
                    return;
                }
            }
        })
        .detach();
    }

    fn locked(&self) -> bool {
        self.locked_until.is_some_and(|u| lock_remaining_secs(u, Instant::now()) > 0)
    }

    fn submit_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((device, _)) = self.target.clone() else { return };
        if self.pending.is_some() || self.locked() {
            return;
        }
        let pin = sanitize_pin(self.pin.read(cx).value(cx).as_ref());
        if pin.len() != PIN_LEN {
            return;
        }
        // PIN không giữ lại sau khi gửi.
        self.pin.update(cx, |f, cx| f.set_value("", window, cx));
        let req = (self.send)(PhoneOp::Pair { device, pin: Pin(pin) }, cx);
        self.start(req, PendingKind::Pair);
        cx.notify();
    }

    fn submit_address(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let address = self.address.read(cx).value(cx).trim().to_owned();
        if address.is_empty() {
            return;
        }
        let req = (self.send)(PhoneOp::AddAddress { address }, cx);
        self.start(req, PendingKind::Address);
        cx.notify();
    }

    fn start(&mut self, req: Option<u64>, kind: PendingKind) {
        match req {
            Some(req) => {
                self.pending = Some(Pending { req, kind });
                self.error = None;
            }
            None => self.error = Some(self.i18n.t("add_phone.err_internal").to_owned().into()),
        }
    }

    fn pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = pairable(&self.devices).get(ix).map(|d| (d.id.clone(), d.name.clone())) else { return };
        self.go_pin(d.0, d.1, window, cx);
    }

    // ---- Vẽ ----

    fn render_pick(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors().clone();
        let t = |k: &str| SharedString::from(self.i18n.t(k).to_owned());
        let list = pairable(&self.devices);
        let body = if list.is_empty() {
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(ProgressCircular::new("add-phone-searching").scale(0.4))
                .child(div().text_color(c.muted).child(t("add_phone.empty")))
                .into_any_element()
        } else {
            CardGroup::new()
                .children(list.iter().enumerate().map(|(ix, d)| {
                    let mut item = CardItem::new(SharedString::from(d.name.clone()));
                    if d.state == DeviceState::NeedsRepair {
                        item = item.adornment(Badge::new(t("add_phone.repair")).tone(Accent::Warning));
                    }
                    if d.newer_api {
                        item = item.line(t("add_phone.newer_api"));
                    }
                    if let Some(addr) = &d.address {
                        item = item.line(SharedString::from(addr.clone()));
                    }
                    item.trailing(
                        Button::new(("add-phone-pick", ix), t("add_phone.select"))
                            .tonal()
                            .size(Size::Sm)
                            .on_click(cx.listener(move |this, _, window, cx| this.pick(ix, window, cx))),
                    )
                }))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                Card::new("add-phone-found")
                    .title(t("add_phone.found"))
                    .description(t("add_phone.step_enable"))
                    .child(body),
            )
            .child(div().flex().child(
                Button::new("add-phone-manual", t("add_phone.manual_link")).text().base().size(Size::Sm).on_click(
                    cx.listener(|this, _, window, cx| {
                        this.step = Step::Manual;
                        this.error = None;
                        {
                            let h = this.address.focus_handle(cx);
                            window.focus(&h, cx);
                        }
                        cx.notify();
                    }),
                ),
            ))
            .into_any_element()
    }

    fn render_manual(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = |k: &str| SharedString::from(self.i18n.t(k).to_owned());
        let busy = self.pending.is_some();
        let form = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_color(cx.theme().colors().muted).child(t("add_phone.manual_hint")))
            .child(
                div().flex().items_start().gap_2().child(div().flex_1().child(self.address.clone())).child(
                    Button::new("add-phone-add", t("add_phone.manual_add"))
                        .loading(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.submit_address(cx))),
                ),
            );
        Card::new("add-phone-manual-card").title(t("add_phone.manual_title")).child(form).into_any_element()
    }

    fn render_pin(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = |k: &str| SharedString::from(self.i18n.t(k).to_owned());
        let name = self.target.as_ref().map(|(_, n)| n.clone()).unwrap_or_default();
        let busy = self.pending.is_some();
        let typed = sanitize_pin(self.pin.read(cx).value(cx).as_ref()).len() == PIN_LEN;
        let locked = self.locked();
        let form = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_color(cx.theme().colors().muted).child(t("add_phone.pin_hint")))
            .child(
                div().flex().items_start().gap_2().child(div().w(px(180.)).child(self.pin.clone())).child(
                    Button::new("add-phone-pair", if busy { t("add_phone.pairing") } else { t("add_phone.pair") })
                        .size(Size::Lg)
                        .loading(busy)
                        .disabled(!typed || locked)
                        .on_click(cx.listener(|this, _, window, cx| this.submit_pin(window, cx))),
                ),
            );
        let title = self.i18n.t_args("add_phone.pin_title", &[("phone", &name)]);
        Card::new("add-phone-pin-card").title(SharedString::from(title)).child(form).into_any_element()
    }
}

impl Render for AddPhoneView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.theme().colors().clone();
        let title = SharedString::from(self.i18n.t("add_phone.title").to_owned());
        let back_label = SharedString::from(self.i18n.t("add_phone.back").to_owned());
        let content = match self.step {
            Step::Pick => self.render_pick(cx),
            Step::Manual => self.render_manual(cx),
            Step::Pin => self.render_pin(cx),
        };
        let error = self.error.clone().map(|e| div().text_color(c.danger).child(e));
        div()
            .id("add-phone")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .py_3()
            .px_4()
            .bg(c.surface)
            .text_color(c.text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        IconButton::new("add-phone-back", IconName::ChevronLeft)
                            .accessibility_label(back_label)
                            .on_click(cx.listener(|this, _, _, cx| this.go_back(cx))),
                    )
                    .child(div().text_xl().font_weight(cx.theme().title_weight()).child(title)),
            )
            .child(content)
            .children(error)
    }
}

#[cfg(test)]
mod tests {
    use ferry_core::i18n::{I18n, Language};

    use super::*;

    fn device(id: &str, state: DeviceState, visible: bool) -> DeviceView {
        DeviceView {
            id: DeviceId(id.into()),
            name: id.into(),
            state,
            visible,
            newer_api: false,
            features: Vec::new(),
            address: None,
        }
    }

    /// C1 — ô PIN nhận 6 số, từ chối chữ, dán "123 456" thành 123456.
    #[test]
    fn pin_accepts_six_digits_only() {
        assert_eq!(sanitize_pin("123 456"), "123456");
        assert_eq!(sanitize_pin("12-34-56"), "123456");
        assert_eq!(sanitize_pin("abc"), "");
        assert_eq!(sanitize_pin("1a2b3"), "123");
        assert_eq!(sanitize_pin("1234567890"), "123456");
        assert_eq!(sanitize_pin(""), "");
        // Chữ số Unicode (Ả Rập – Ấn) không phải PIN.
        assert_eq!(sanitize_pin("١٢٣456"), "456");
    }

    #[test]
    fn list_shows_only_unpaired_visible_first() {
        let devices = vec![
            device("a-online", DeviceState::Online, true),
            device("b-seen-off", DeviceState::Discovered, false),
            device("c-seen", DeviceState::Discovered, true),
            device("d-repair", DeviceState::NeedsRepair, true),
            device("e-paired", DeviceState::Paired, true),
        ];
        let ids: Vec<&str> = pairable(&devices).iter().map(|d| d.id.0.as_str()).collect();
        assert_eq!(ids, ["c-seen", "d-repair", "b-seen-off"]);
    }

    #[test]
    fn lock_countdown_rounds_up() {
        let now = Instant::now();
        assert_eq!(lock_remaining_secs(now + Duration::from_millis(1500), now), 2);
        assert_eq!(lock_remaining_secs(now + Duration::from_secs(3), now), 3);
        assert_eq!(lock_remaining_secs(now, now + Duration::from_secs(1)), 0);
    }

    #[test]
    fn errors_say_what_to_do_in_both_languages() {
        for lang in [Language::Vi, Language::En] {
            let i18n = I18n::new(lang);
            let all = [
                CommandError::WrongPin,
                CommandError::RateLimited { retry_after_secs: 42 },
                CommandError::OtherPhone,
                CommandError::Keyring,
                CommandError::Offline,
                CommandError::Unreachable,
                CommandError::UnknownInterface { name: "en9".into() },
                CommandError::InvalidInput,
                CommandError::Internal,
            ];
            for e in &all {
                for address_step in [false, true] {
                    let msg = error_message(&i18n, e, address_step, 0);
                    assert!(!msg.contains("add_phone."), "missing key for {e:?}: {msg}");
                    assert!(!msg.contains('{'), "unfilled placeholder for {e:?}: {msg}");
                }
            }
            assert!(error_message(&i18n, &all[1], false, 0).contains("42"));
            assert!(error_message(&i18n, &all[1], false, 7).contains('7'));
            assert!(error_message(&i18n, &all[6], false, 0).contains("en9"));
        }
    }
}
