//! Màn "Danh sách thiết bị" (mục cột trái, thay tab "Thư viện điện thoại" cũ): mọi điện thoại đã ghép — tên, trạng thái,
//! địa chỉ đang dùng — và việc làm trên từng máy: "Duyệt tệp" (mở [`super::phone_library::PhoneLibrary`] của máy đó),
//! "Ghép lại" (máy cần ghép lại / chưa nối được ⇒ màn Thêm điện thoại ở thẳng bước PIN), "Đổi tên" (bí danh trên PC — cấu
//! hình theo thiết bị duy nhất daemon có, `SettingsOp::RenameDevice`) và "Xoá" (hộp xác nhận ⇒ `PhoneOp::Forget`: báo điện
//! thoại nếu đang nối, xoá token trong kho khoá, bản ghi trong `config.json` và hàng đợi của máy đó).
//!
//! Trạng thái sống (online / offline / cần ghép lại, địa chỉ) lấy từ ảnh chụp của [`Model`]; tên điện thoại gốc khi đã đặt
//! bí danh lấy từ `SettingsOp::Get` (ảnh chụp chỉ có tên hiển thị). Lệnh đi qua kết nối IPC chính của `Model`.

use std::collections::BTreeSet;
use std::rc::Rc;

use ferry_core::device::{DeviceId, DeviceState};
use ferry_core::i18n::I18n;
use ferry_core::ipc::{CommandError, DeviceView, Outcome, PhoneOp, SettingsOp, SettingsView};
use materiant::badge::Badge;
use materiant::button::{Button, Size};
use materiant::card::{Card, CardGroup, CardItem};
use materiant::field::{FieldEvent, TextField};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    AnyElement, AppContext, ClickEvent, Context, Entity, EventEmitter, Focusable, IntoElement, Render, SharedString,
    Subscription, Window, div, px,
};
use materiant::overlay::dialog::{Dialog, DialogHandle};
use materiant::scroll::ScrollArea;
use materiant::style::Accent;
use materiant::theme::ActiveTheme;
use materiant::toast::{Toast, ToastKind, ToastSpec};

use super::send_tray::lacks_dirs;
use crate::model::{Model, ModelEvent};

pub enum DevicesEvent {
    /// "Thêm điện thoại".
    AddPhone,
    /// "Ghép lại": màn Thêm điện thoại ở thẳng bước PIN cho máy này.
    Repair(DeviceId),
    /// "Duyệt tệp" trên điện thoại này.
    Browse(DeviceId),
}

// ---- Hàm thuần (test được, không cần GPUI) ----

/// Khoá i18n của nhãn trạng thái.
pub fn state_key(state: DeviceState) -> &'static str {
    match state {
        DeviceState::Online => "devices.state_online",
        DeviceState::NeedsRepair => "devices.state_needs_repair",
        DeviceState::Paired => "devices.state_checking",
        DeviceState::Offline | DeviceState::Discovered | DeviceState::Unknown => "devices.state_offline",
    }
}

/// Màu huy hiệu trạng thái; `None` ⇒ huy hiệu viền (trạng thái bình thường, không cần làm gì).
pub fn state_tone(state: DeviceState) -> Option<Accent> {
    match state {
        DeviceState::Online => Some(Accent::Success),
        DeviceState::NeedsRepair => Some(Accent::Warning),
        _ => None,
    }
}

/// Máy này nên được mời "Ghép lại" / "Kết nối lại": điện thoại đã từ chối token, hoặc đang không nối được (ghép lại bằng
/// PIN cũng là cách nối lại khi địa chỉ / token cũ hỏng).
pub fn needs_reconnect(state: DeviceState) -> bool {
    matches!(state, DeviceState::NeedsRepair | DeviceState::Offline)
}

/// Mã thiết bị rút gọn để phân biệt hai máy trùng tên (8 ký tự đầu, đủ khác nhau trong thực tế).
pub fn short_id(id: &DeviceId) -> String {
    id.0.chars().take(8).collect()
}

/// Lỗi lệnh ⇒ khoá i18n nói việc cần làm (PROTOCOL 2.6), không nói mã lỗi.
pub fn error_key(error: &CommandError) -> &'static str {
    match error {
        CommandError::InvalidInput => "devices.err_invalid_name",
        CommandError::UnknownDevice | CommandError::NotPaired => "devices.err_unknown",
        CommandError::Keyring => "devices.err_keyring",
        _ => "devices.err_internal",
    }
}

pub struct DevicesPage {
    model: Entity<Model>,
    i18n: Rc<I18n>,
    /// Cài đặt đọc lần cuối (bí danh + tên điện thoại báo).
    settings: Option<SettingsView>,
    /// Bộ máy đã ghép lúc đọc `settings` — đổi (ghép mới, xoá) ⇒ đọc lại.
    known: BTreeSet<DeviceId>,
    get_req: Option<u64>,
    rename_req: Option<u64>,
    forget_req: Option<(u64, String)>,
    rename_field: Entity<TextField>,
    rename_dialog: DialogHandle,
    rename_target: Option<DeviceId>,
    forget_dialog: DialogHandle,
    forget_target: Option<(DeviceId, String)>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<DevicesEvent> for DevicesPage {}

impl DevicesPage {
    pub fn new(model: Entity<Model>, i18n: Rc<I18n>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let label = i18n.t("devices.rename_label").to_owned();
        let rename_field = cx.new(|cx| TextField::new("devices-rename", window, cx).placeholder(label));
        let subs = vec![
            cx.subscribe_in(&model, window, |this, _, ev: &ModelEvent, window, cx| this.on_model(ev, window, cx)),
            cx.subscribe_in(&rename_field, window, |this, _, ev: &FieldEvent, window, cx| {
                if let FieldEvent::Submit(_) = ev {
                    this.confirm_rename(window, cx);
                }
            }),
        ];
        let mut page = DevicesPage {
            model,
            i18n,
            settings: None,
            known: BTreeSet::new(),
            get_req: None,
            rename_req: None,
            forget_req: None,
            rename_field,
            rename_dialog: DialogHandle::new(cx),
            rename_target: None,
            forget_dialog: DialogHandle::new(cx),
            forget_target: None,
            _subs: subs,
        };
        page.reload(cx);
        page
    }

    fn t(&self, key: &str) -> SharedString {
        SharedString::from(self.i18n.t(key).to_owned())
    }

    fn paired_ids(&self, cx: &Context<Self>) -> BTreeSet<DeviceId> {
        self.model.read(cx).paired().into_iter().map(|d| d.id.clone()).collect()
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.known = self.paired_ids(cx);
        self.get_req = self.model.update(cx, |m, _| m.settings(SettingsOp::Get));
    }

    fn toast(&self, kind: ToastKind, text: String, window: &mut Window, cx: &mut Context<Self>) {
        Toast::push(window, cx, ToastSpec::new(kind, text));
    }

    fn on_model(&mut self, ev: &ModelEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            ModelEvent::Changed => {
                if self.get_req.is_none() && self.paired_ids(cx) != self.known {
                    self.reload(cx);
                }
                cx.notify();
            }
            ModelEvent::Reply { req, outcome } => {
                let req = Some(*req);
                if req == self.get_req {
                    self.get_req = None;
                    if let Outcome::Settings(view) = outcome {
                        self.settings = Some(view.clone());
                    }
                } else if req == self.rename_req {
                    self.rename_req = None;
                    match outcome {
                        Outcome::Settings(view) => self.settings = Some(view.clone()),
                        Outcome::Failed { error } => {
                            self.toast(ToastKind::Danger, self.i18n.t(error_key(error)).to_owned(), window, cx)
                        }
                        _ => {}
                    }
                } else if req == self.forget_req.as_ref().map(|(r, _)| *r) {
                    let Some((_, name)) = self.forget_req.take() else { return };
                    match outcome {
                        Outcome::Failed { error } => {
                            self.toast(ToastKind::Danger, self.i18n.t(error_key(error)).to_owned(), window, cx)
                        }
                        _ => {
                            let text = self.i18n.t_args("devices.removed", &[("name", &name)]);
                            self.toast(ToastKind::Success, text, window, cx);
                            self.reload(cx);
                        }
                    }
                } else {
                    return;
                }
                cx.notify();
            }
        }
    }

    fn no_daemon(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.toast(ToastKind::Danger, self.i18n.t("main.no_daemon").to_owned(), window, cx);
    }

    fn open_rename(&mut self, device: &DeviceView, window: &mut Window, cx: &mut Context<Self>) {
        self.rename_target = Some(device.id.clone());
        let name = device.name.clone();
        self.rename_field.update(cx, |field, cx| field.set_value(name, window, cx));
        self.rename_dialog.open(window, cx);
        window.focus(&self.rename_field.focus_handle(cx), cx);
        cx.notify();
    }

    fn confirm_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(device) = self.rename_target.take() else { return };
        let name = self.rename_field.read(cx).value(cx).trim().to_owned();
        self.rename_dialog.close(window, cx);
        self.rename_req = self.model.update(cx, |m, _| m.settings(SettingsOp::RenameDevice { device, name }));
        if self.rename_req.is_none() {
            self.no_daemon(window, cx);
        }
        cx.notify();
    }

    fn open_forget(&mut self, device: &DeviceView, window: &mut Window, cx: &mut Context<Self>) {
        self.forget_target = Some((device.id.clone(), device.name.clone()));
        self.forget_dialog.open(window, cx);
        cx.notify();
    }

    fn confirm_forget(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((device, name)) = self.forget_target.take() else { return };
        self.forget_dialog.close(window, cx);
        match self.model.update(cx, |m, _| m.phone(PhoneOp::Forget { device })) {
            Some(req) => self.forget_req = Some((req, name)),
            None => self.no_daemon(window, cx),
        }
        cx.notify();
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.theme().colors().clone();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_4()
            .h(px(44.))
            .border_b_1()
            .border_color(c.border)
            .child(div().flex_none().font_weight(cx.theme().title_weight()).child(self.t("devices.title")))
            .child(div().flex_1().min_w_0().truncate().text_xs().text_color(c.muted).child(self.t("devices.hint")))
            .child(
                Button::new("devices-add", self.i18n.t("main.add_phone"))
                    .size(Size::Sm)
                    .tonal()
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(DevicesEvent::AddPhone))),
            )
            .into_any_element()
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().colors().muted;
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .px_6()
            .child(div().font_weight(cx.theme().title_weight()).child(self.t("main.no_phone")))
            .child(
                div()
                    .w_full()
                    .max_w(px(380.))
                    .text_center()
                    .text_sm()
                    .text_color(muted)
                    .child(self.t("devices.empty_hint")),
            )
            .child(
                div().pt_2().child(
                    Button::new("devices-empty-add", self.i18n.t("main.add_phone"))
                        .size(Size::Sm)
                        .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(DevicesEvent::AddPhone))),
                ),
            )
            .into_any_element()
    }

    /// `same_name`: có máy khác cùng tên hiển thị ⇒ thêm dòng mã thiết bị để phân biệt (vd. điện thoại cài lại app nên
    /// ghép thành máy mới, bản ghi cũ còn đó).
    fn render_item(&self, ix: usize, device: &DeviceView, same_name: bool, cx: &mut Context<Self>) -> CardItem {
        let setting = self.settings.as_ref().and_then(|s| s.devices.iter().find(|d| d.id == device.id));
        let busy = self.forget_req.is_some() || self.rename_req.is_some();
        let badge = {
            let b = Badge::new(self.i18n.t(state_key(device.state)).to_owned());
            match state_tone(device.state) {
                Some(tone) => b.tone(tone),
                None => b.outline(),
            }
        };
        let mut item = CardItem::new(device.name.clone()).adornment(badge);
        if let Some(s) = setting.filter(|s| s.alias.is_some()) {
            item = item.line(self.i18n.t_args("devices.phone_name", &[("name", &s.name)]));
        }
        item = item.line(match &device.address {
            Some(a) => self.i18n.t_args("devices.address", &[("address", a)]),
            None => self.i18n.t("devices.no_address").to_owned(),
        });
        if same_name {
            item = item.line(self.i18n.t_args("devices.device_id", &[("id", &short_id(&device.id))]));
        }
        if device.state == DeviceState::NeedsRepair {
            item = item.line(self.i18n.t("devices.needs_repair_hint").to_owned());
        } else if lacks_dirs(device) {
            item = item.line(self.i18n.t("devices.update_phone_app").to_owned());
        }

        let (id_browse, id_repair) = (device.id.clone(), device.id.clone());
        let (rename_device, forget_device) = (device.clone(), device.clone());
        let actions = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .when(needs_reconnect(device.state), |d| {
                d.child(
                    Button::new(("devices-repair", ix), self.i18n.t("devices.repair")).size(Size::Sm).tonal().on_click(
                        cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(DevicesEvent::Repair(id_repair.clone()))),
                    ),
                )
            })
            .child(
                Button::new(("devices-browse", ix), self.i18n.t("devices.browse")).size(Size::Sm).outlined().on_click(
                    cx.listener(move |_, _: &ClickEvent, _, cx| cx.emit(DevicesEvent::Browse(id_browse.clone()))),
                ),
            )
            .child(
                Button::new(("devices-rename", ix), self.i18n.t("devices.rename"))
                    .size(Size::Sm)
                    .text()
                    .disabled(busy)
                    .on_click(cx.listener(move |page, _: &ClickEvent, window, cx| {
                        page.open_rename(&rename_device, window, cx)
                    })),
            )
            .child(
                Button::new(("devices-remove", ix), self.i18n.t("devices.remove"))
                    .size(Size::Sm)
                    .text()
                    .danger()
                    .disabled(busy)
                    .on_click(cx.listener(move |page, _: &ClickEvent, window, cx| {
                        page.open_forget(&forget_device, window, cx)
                    })),
            );
        item.trailing(actions)
    }

    fn render_dialogs(&self, cx: &mut Context<Self>) -> (Dialog, Dialog) {
        let muted = cx.theme().colors().muted;
        let caption = cx.theme().caption_size();
        let rename = Dialog::new("devices-rename-dialog", &self.rename_dialog)
            .title(self.t("devices.rename_title"))
            .max_width(px(400.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().child(self.t("devices.rename_label")))
                    .child(self.rename_field.clone())
                    .child(div().text_size(caption).text_color(muted).child(self.t("devices.rename_hint"))),
            )
            .actions([
                Button::new("devices-rename-cancel", self.i18n.t("devices.cancel"))
                    .text()
                    .base()
                    .on_click(cx.listener(|page, _: &ClickEvent, window, cx| {
                        page.rename_target = None;
                        page.rename_dialog.close(window, cx);
                    }))
                    .into_any_element(),
                Button::new("devices-rename-save", self.i18n.t("devices.save"))
                    .on_click(cx.listener(|page, _: &ClickEvent, window, cx| page.confirm_rename(window, cx)))
                    .into_any_element(),
            ]);
        let name = self.forget_target.as_ref().map(|(_, n)| n.as_str()).unwrap_or_default();
        let forget = Dialog::new("devices-remove-dialog", &self.forget_dialog)
            .title(self.i18n.t_args("devices.remove_title", &[("name", name)]))
            .max_width(px(420.))
            .child(div().text_color(muted).child(self.t("devices.remove_body")))
            .actions([
                Button::new("devices-remove-cancel", self.i18n.t("devices.cancel"))
                    .text()
                    .base()
                    .on_click(cx.listener(|page, _: &ClickEvent, window, cx| {
                        page.forget_target = None;
                        page.forget_dialog.close(window, cx);
                    }))
                    .into_any_element(),
                Button::new("devices-remove-confirm", self.i18n.t("devices.remove_confirm"))
                    .danger()
                    .on_click(cx.listener(|page, _: &ClickEvent, window, cx| page.confirm_forget(window, cx)))
                    .into_any_element(),
            ]);
        (rename, forget)
    }
}

impl Render for DevicesPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let devices: Vec<DeviceView> = self.model.read(cx).paired().into_iter().cloned().collect();
        let body = if devices.is_empty() {
            self.render_empty(cx)
        } else {
            let items: Vec<CardItem> = devices
                .iter()
                .enumerate()
                .map(|(ix, d)| {
                    let same_name = devices.iter().filter(|o| o.name == d.name).count() > 1;
                    self.render_item(ix, d, same_name, cx)
                })
                .collect();
            let card = Card::new("devices-card")
                .title(self.t("devices.paired_title"))
                .description(self.t("devices.paired_description"))
                .child(CardGroup::new().children(items));
            // D-16: khung cuộn chạm mép; đệm ngang nằm trong nội dung, bằng nhau hai bên.
            ScrollArea::new("devices-scroll")
                .vertical()
                .flex_1()
                .min_h_0()
                .padding_x(px(24.))
                .child(div().py_5().child(card))
                .into_any_element()
        };
        let (rename, forget) = self.render_dialogs(cx);
        div()
            .id("devices-page")
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_header(cx))
            .child(body)
            .child(rename)
            .child(forget)
    }
}

#[cfg(test)]
mod tests {
    use ferry_core::i18n::Language;

    use super::*;

    #[test]
    fn states_map_to_translated_keys() {
        let all = [
            DeviceState::Online,
            DeviceState::Offline,
            DeviceState::NeedsRepair,
            DeviceState::Paired,
            DeviceState::Discovered,
            DeviceState::Unknown,
        ];
        for lang in [Language::Vi, Language::En] {
            let i18n = I18n::new(lang);
            for s in all {
                assert_ne!(i18n.t(state_key(s)), state_key(s), "{s:?} {lang:?}");
            }
        }
        assert_eq!(state_tone(DeviceState::NeedsRepair), Some(Accent::Warning));
        assert_eq!(state_tone(DeviceState::Offline), None);
    }

    #[test]
    fn short_id_keeps_eight_chars() {
        assert_eq!(short_id(&DeviceId("cdf3e727f2603654".into())), "cdf3e727");
        assert_eq!(short_id(&DeviceId("ab".into())), "ab");
    }

    #[test]
    fn reconnect_only_when_broken() {
        assert!(needs_reconnect(DeviceState::NeedsRepair));
        assert!(needs_reconnect(DeviceState::Offline));
        assert!(!needs_reconnect(DeviceState::Online));
        assert!(!needs_reconnect(DeviceState::Paired));
    }

    #[test]
    fn every_devices_key_is_translated() {
        let source = include_str!("devices.rs");
        let code = source.split_once("#[cfg(test)]").map_or(source, |(code, _)| code);
        let keys: Vec<String> = code
            .split("\"devices.")
            .skip(1)
            .filter_map(|p| p.split_once('"').map(|(k, _)| format!("devices.{k}")))
            .collect();
        assert!(keys.len() > 15, "scan found only {} keys", keys.len());
        for lang in [Language::Vi, Language::En] {
            let i18n = I18n::new(lang);
            for key in &keys {
                assert_ne!(i18n.t(key), key.as_str(), "missing `{key}` for {lang:?}");
            }
        }
    }
}
