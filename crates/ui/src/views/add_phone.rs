//! Màn "Thêm điện thoại" (PROTOCOL 4.4, ghép PIN mục 6.3): hướng dẫn bật Sync WiFi → danh sách điện thoại dò thấy chưa
//! ghép (cập nhật trực tiếp từ snapshot) hoặc nhập địa chỉ tay → ô PIN 6 số → `Pair`. Xong ⇒ [`AddPhoneEvent::Paired`].
//!
//! View không tự nối IPC: chủ (cửa sổ chính) đưa vào `send` (gửi `PhoneOp`, trả `req`), gọi [`AddPhoneView::set_devices`]
//! mỗi khi có `Snapshot` mới và [`AddPhoneView::on_reply`] với mỗi `Reply`. PIN không giữ lại sau khi gửi (xoá ô ngay).
//!
//! W-6: vào màn mà `Snapshot::mdns_firewall` có (tường lửa bật, chưa cho phép — daemon biết ngay lúc khởi động) ⇒ mở luôn hộp
//! thoại xin quyền (qua [`AddPhoneView::set_firewall`], một lần mỗi lần mở màn); "Cho phép" ⇒ `PhoneOp::AllowMdnsFirewall`,
//! hệ thống hỏi mật khẩu; không hỏi được thì hộp hiện câu lệnh để chép. "Để sau" ⇒ còn nút mở lại ở bước đầu; đường nhập
//! địa chỉ tay vẫn ở đó.

use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::assets::FerryIcon;
use ferry_core::device::{DeviceId, DeviceState};
use ferry_core::firewall::{AllowResult, MdnsFirewallHint};
use ferry_core::i18n::I18n;
use ferry_core::ipc::{CommandError, DeviceView, Outcome, PhoneOp, Pin};
use materiant::badge::Badge;
use materiant::button::{Button, IconButton, Size};
use materiant::card::{Card, CardGroup, CardItem};
use materiant::field::{FieldEvent, TextField};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    AnyElement, AppContext, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render,
    SharedString, Subscription, WeakEntity, Window, div, px, svg,
};
use materiant::icon::IconName;
use materiant::overlay::dialog::{Dialog, DialogHandle};
use materiant::progress::ProgressCircular;
use materiant::style::Accent;
use materiant::theme::ActiveTheme;
use materiant::toast::{Toast, ToastKind, ToastSpec};

/// Dò chừng này chưa thấy điện thoại nào ⇒ chỉ sang đường nhập địa chỉ tay (WiFi công ty / AP isolation hay lọc mDNS: chờ
/// thêm cũng không thấy, kể cả sau khi đã mở tường lửa).
pub const SLOW_SEARCH: Duration = Duration::from_secs(20);
/// Danh sách còn trống ⇒ quét LAN dự phòng lại sau chừng này kể từ lượt trước xong (`PROTOCOL.md` 6.2) — mDNS bị mạng chặn
/// vẫn thấy điện thoại vừa bật Sync WiFi; không dày hơn để "rảnh không giữ CPU".
pub const RESCAN_EVERY: Duration = Duration::from_secs(15);

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

/// Trạng thái nút "Cho phép" (W-6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirewallStatus {
    Idle,
    /// Hộp xác nhận của hệ thống đang mở (lệnh `req`).
    Waiting(u64),
    Done(AllowResult),
}

/// Khoá i18n cho dòng trạng thái dưới nút "Cho phép"; `None` ⇒ không có dòng.
pub fn firewall_status_key(status: FirewallStatus) -> Option<&'static str> {
    match status {
        FirewallStatus::Idle => None,
        FirewallStatus::Waiting(_) => Some("add_phone.firewall_waiting"),
        FirewallStatus::Done(AllowResult::Allowed) => Some("add_phone.firewall_allowed"),
        FirewallStatus::Done(AllowResult::Cancelled) => Some("add_phone.firewall_cancelled"),
        FirewallStatus::Done(AllowResult::NoPrompt) => Some("add_phone.firewall_no_prompt"),
        FirewallStatus::Done(AllowResult::Failed) => Some("add_phone.firewall_failed"),
    }
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
    /// W-6: gợi ý từ `Snapshot::mdns_firewall`.
    firewall: Option<MdnsFirewallHint>,
    firewall_status: FirewallStatus,
    /// Câu lệnh của lần gợi ý gần nhất — giữ để hiện khi không hỏi được quyền (gợi ý có thể đã tắt).
    firewall_command: Option<String>,
    /// Mở màn lúc này — dò quá [`SLOW_SEARCH`] mà danh sách trống ⇒ gợi ý nhập tay.
    opened_at: Instant,
    /// Hộp thoại xin quyền tường lửa (W-6).
    firewall_dialog: DialogHandle,
    /// Đã tự mở hộp một lần trong lần mở màn này.
    firewall_asked: bool,
    /// Lệnh `ScanLan` đang chờ trả lời.
    scan_req: Option<u64>,
    /// Mở để ghép lại một máy đã biết (`repair`): "Quay lại" ở bước PIN thoát hẳn màn, không tự mở hộp tường lửa (máy đã có
    /// địa chỉ, không cần dò mDNS).
    repair: bool,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<AddPhoneEvent> for AddPhoneView {}

impl Focusable for AddPhoneView {
    fn focus_handle(&self, _cx: &materiant::gpui_kit::App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Vòng "đang dò" 24 px: vòng nền mờ cố định (`value(0)` chỉ vẽ track) + cung quay chồng lên. Cung không xác định có khung
/// chỉ dài ~10° — đứng một mình ở cỡ này (hoặc khi cửa sổ không vẽ lại) trông như một gạch lệch, có vòng nền mới đọc ra
/// là đang quay.
fn searching_spinner() -> impl IntoElement {
    const SCALE: f32 = 24.0 / 64.0;
    div()
        .relative()
        .flex_none()
        .size(px(24.))
        .child(ProgressCircular::new("add-phone-searching-track").value(0.0).scale(SCALE))
        .child(div().absolute().top_0().left_0().child(ProgressCircular::new("add-phone-searching").scale(SCALE)))
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

/// Đã dò quá [`SLOW_SEARCH`] kể từ `since`.
pub fn search_slow(since: Instant, now: Instant) -> bool {
    now.saturating_duration_since(since) >= SLOW_SEARCH
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
            firewall: None,
            firewall_status: FirewallStatus::Idle,
            firewall_command: None,
            opened_at: Instant::now(),
            firewall_dialog: DialogHandle::new(cx),
            firewall_asked: repair.is_some(),
            repair: repair.is_some(),
            scan_req: None,
            focus: cx.focus_handle(),
            _subs: subs,
        };
        view.redraw_after(SLOW_SEARCH, cx);
        if repair.is_none() {
            view.scan_lan(cx);
        }
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

    /// Snapshot mới: tường lửa cần cho phép (W-6). Lần đầu thấy trong lần mở màn này ⇒ mở hộp thoại xin quyền ngay.
    pub fn set_firewall(&mut self, hint: Option<MdnsFirewallHint>, window: &mut Window, cx: &mut Context<Self>) {
        if self.firewall == hint {
            return;
        }
        if let Some(h) = &hint {
            self.firewall_command = Some(h.command.clone());
            if !std::mem::replace(&mut self.firewall_asked, true) {
                // Hộp cần cây phần tử của view: mở sau lượt dựng hiện tại.
                cx.defer_in(window, |this, window, cx| this.firewall_dialog.open(window, cx));
            }
        }
        self.firewall = hint;
        cx.notify();
    }

    fn allow_firewall(&mut self, cx: &mut Context<Self>) {
        if matches!(self.firewall_status, FirewallStatus::Waiting(_)) {
            return;
        }
        self.firewall_status = match (self.send)(PhoneOp::AllowMdnsFirewall, cx) {
            Some(req) => FirewallStatus::Waiting(req),
            None => FirewallStatus::Done(AllowResult::Failed),
        };
        cx.notify();
    }

    /// Quét LAN dự phòng (PROTOCOL 6.2) — máy trả lời tới qua snapshot như máy mDNS thấy. Không chồng lệnh.
    fn scan_lan(&mut self, cx: &mut Context<Self>) {
        if self.scan_req.is_none() {
            self.scan_req = (self.send)(PhoneOp::ScanLan, cx);
        }
    }

    /// Hẹn lượt quét kế sau [`RESCAN_EVERY`]: tới giờ mà còn ở bước đầu và danh sách trống thì quét, không thì hẹn tiếp.
    /// Màn đóng ⇒ view bị thả, hẹn tự huỷ.
    fn schedule_rescan(&self, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            executor.timer(RESCAN_EVERY).await;
            let _ = this.update(cx, |this, cx| {
                if this.step == Step::Pick && pairable(&this.devices).is_empty() {
                    this.scan_lan(cx);
                } else {
                    // Đang nhập tay / PIN, hoặc đã thấy máy: chưa quét, hẹn lại (chỉ một đồng hồ, không tốn CPU).
                    this.schedule_rescan(cx);
                }
            });
        })
        .detach();
    }

    /// `Reply` từ daemon; bỏ qua nếu không phải của lệnh đang chờ.
    pub fn on_reply(&mut self, req: u64, outcome: &Outcome, window: &mut Window, cx: &mut Context<Self>) {
        if self.scan_req == Some(req) {
            self.scan_req = None;
            if !matches!(outcome, Outcome::Failed { .. }) {
                self.schedule_rescan(cx);
            }
            return;
        }
        if self.firewall_status == FirewallStatus::Waiting(req) {
            self.firewall_status = match outcome {
                Outcome::Firewall { result } => {
                    if *result == AllowResult::Allowed {
                        self.firewall_dialog.close(window, cx);
                        let text = self.i18n.t("add_phone.firewall_allowed").to_owned();
                        Toast::push(window, cx, ToastSpec::new(ToastKind::Success, text));
                    }
                    FirewallStatus::Done(*result)
                }
                _ => FirewallStatus::Done(AllowResult::Failed),
            };
            cx.notify();
            return;
        }
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

    /// Vẽ lại một lần sau `wait` (hết hạn chờ dò ⇒ hiện gợi ý nhập tay).
    fn redraw_after(&self, wait: Duration, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            executor.timer(wait).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn go_manual(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step = Step::Manual;
        self.error = None;
        {
            let h = self.address.focus_handle(cx);
            window.focus(&h, cx);
        }
        cx.notify();
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        match self.step {
            Step::Pick => cx.emit(AddPhoneEvent::Back),
            Step::Pin if self.repair => cx.emit(AddPhoneEvent::Back),
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

    /// Hộp thoại xin quyền W-6 (luôn trong cây, đóng thì không vẽ gì).
    fn render_firewall_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors().clone();
        let t = |k: &str| SharedString::from(self.i18n.t(k).to_owned());
        let status = self.firewall_status;
        let waiting = matches!(status, FirewallStatus::Waiting(_));
        let can_prompt = self.firewall.as_ref().is_none_or(|h| h.can_prompt);
        let show_command =
            !can_prompt || matches!(status, FirewallStatus::Done(AllowResult::NoPrompt | AllowResult::Failed));
        let status_line = firewall_status_key(status).filter(|_| can_prompt).map(|k| {
            let color = match status {
                FirewallStatus::Done(AllowResult::Allowed) => c.success,
                FirewallStatus::Done(AllowResult::Failed | AllowResult::NoPrompt) => c.danger,
                _ => c.muted,
            };
            div().text_color(color).child(t(k))
        });
        let command = self.firewall_command.clone().filter(|_| show_command).map(|cmd| {
            let copy = cmd.clone();
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_color(c.muted).child(t("add_phone.firewall_command_hint")))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .bg(c.surface)
                                .font_family("monospace")
                                .text_sm()
                                .child(SharedString::from(cmd)),
                        )
                        .child(
                            IconButton::new("add-phone-firewall-copy", IconName::Copy)
                                .accessibility_label(t("add_phone.firewall_copy"))
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                                }),
                        ),
                )
        });
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().child(t("add_phone.firewall_body")))
            .children(status_line)
            .children(command);
        let close = {
            let h = self.firewall_dialog.clone();
            let label = if can_prompt { t("add_phone.firewall_later") } else { t("add_phone.firewall_close") };
            Button::new("add-phone-firewall-later", label)
                .text()
                .base()
                .on_click(move |_, window, cx| h.close(window, cx))
                .into_any_element()
        };
        let allow = can_prompt.then(|| {
            Button::new("add-phone-firewall-allow", t("add_phone.firewall_allow"))
                .loading(waiting)
                .disabled(waiting)
                .on_click(cx.listener(|this, _, _, cx| this.allow_firewall(cx)))
                .into_any_element()
        });
        Dialog::new("add-phone-firewall", &self.firewall_dialog)
            .title(t("add_phone.firewall_title"))
            .max_width(px(480.))
            .child(body)
            .actions(std::iter::once(close).chain(allow))
            .into_any_element()
    }

    fn render_pick(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors().clone();
        let t = |k: &str| SharedString::from(self.i18n.t(k).to_owned());
        let list = pairable(&self.devices);
        // Gợi ý "vẫn chưa thấy" đã có nút nhập tay ⇒ không vẽ thêm nút cùng chữ bên dưới.
        let slow_shown = list.is_empty() && search_slow(self.opened_at, Instant::now());
        let body = if list.is_empty() {
            let searching = div()
                .flex()
                .items_center()
                .gap_3()
                .child(searching_spinner())
                .child(div().text_color(c.muted).child(t("add_phone.empty")));
            let slow = slow_shown.then(|| {
                div().flex().flex_col().items_start().gap_2().child(div().child(t("add_phone.slow_hint"))).child(
                    Button::new("add-phone-manual-slow", t("add_phone.manual_link"))
                        .tonal()
                        .size(Size::Sm)
                        .on_click(cx.listener(|this, _, window, cx| this.go_manual(window, cx))),
                )
            });
            div().flex().flex_col().gap_3().child(searching).children(slow).into_any_element()
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
                    .child(div().flex().flex_col().gap_3().child(self.render_step_enable(cx)).child(body)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .when(!slow_shown, |row| {
                        row.child(
                            Button::new("add-phone-manual", t("add_phone.manual_link"))
                                .text()
                                .base()
                                .size(Size::Sm)
                                .on_click(cx.listener(|this, _, window, cx| this.go_manual(window, cx))),
                        )
                    })
                    .when(self.firewall.is_some(), |row| {
                        let h = self.firewall_dialog.clone();
                        row.child(
                            Button::new("add-phone-firewall-open", t("add_phone.firewall_open"))
                                .text()
                                .base()
                                .size(Size::Sm)
                                .on_click(move |_, window, cx| h.open(window, cx)),
                        )
                    }),
            )
            .into_any_element()
    }

    /// "Trên điện thoại: mở app → [⋮] → Sync WiFi → Bật": nút menu của app điện thoại vẽ thành phím 32×32 (viền + bo góc)
    /// để người dùng nhận ra đúng nút cần bấm.
    fn render_step_enable(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors().clone();
        let t = |k: &str| SharedString::from(self.i18n.t(k).to_owned());
        let key = div()
            .flex_none()
            .size(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .border_1()
            .border_b_2()
            .border_color(c.border)
            .bg(c.surface)
            .child(svg().path(FerryIcon::MoreVertical.path()).size(px(18.)).text_color(c.text));
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .text_color(c.muted)
            .child(t("add_phone.step_enable_before"))
            .child(key)
            .child(t("add_phone.step_enable_after"))
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
        let title_key = if self.repair { "add_phone.title_repair" } else { "add_phone.title" };
        let title = SharedString::from(self.i18n.t(title_key).to_owned());
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
            .child(self.render_firewall_dialog(cx))
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
    fn manual_hint_after_slow_search() {
        let t0 = Instant::now();
        assert!(!search_slow(t0, t0 + SLOW_SEARCH - Duration::from_millis(1)));
        assert!(search_slow(t0, t0 + SLOW_SEARCH));
    }

    #[test]
    fn lock_countdown_rounds_up() {
        let now = Instant::now();
        assert_eq!(lock_remaining_secs(now + Duration::from_millis(1500), now), 2);
        assert_eq!(lock_remaining_secs(now + Duration::from_secs(3), now), 3);
        assert_eq!(lock_remaining_secs(now, now + Duration::from_secs(1)), 0);
    }

    /// W-6: mọi trạng thái nút "Cho phép" có chữ ở cả hai ngôn ngữ.
    #[test]
    fn firewall_texts_in_both_languages() {
        let statuses = [
            FirewallStatus::Waiting(1),
            FirewallStatus::Done(AllowResult::Allowed),
            FirewallStatus::Done(AllowResult::Cancelled),
            FirewallStatus::Done(AllowResult::NoPrompt),
            FirewallStatus::Done(AllowResult::Failed),
        ];
        assert_eq!(firewall_status_key(FirewallStatus::Idle), None);
        for lang in [Language::Vi, Language::En] {
            let i18n = I18n::new(lang);
            let keys = statuses.iter().filter_map(|s| firewall_status_key(*s)).chain([
                "add_phone.firewall_title",
                "add_phone.firewall_body",
                "add_phone.firewall_allow",
                "add_phone.firewall_command_hint",
                "add_phone.firewall_copy",
                "add_phone.firewall_later",
                "add_phone.firewall_close",
                "add_phone.firewall_open",
                "add_phone.slow_hint",
            ]);
            for k in keys {
                assert!(!i18n.t(k).contains("add_phone."), "{lang:?} missing {k}");
            }
        }
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
