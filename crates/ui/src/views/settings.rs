//! Trang Cài đặt (gd1_12, PROTOCOL 4.5). Nằm trong cửa sổ chính, mở bằng mục Cài đặt ở đáy cột trái (hoặc menu khay
//! "Cài đặt…"). Bố cục theo Cài đặt của macOS / iOS (12/11/2026): các card `CardGroup::standalone` không tiêu đề, mỗi mục
//! một dòng có đường kẻ chia — Ngôn ngữ + khởi động cùng hệ thống; Giao diện (nền + bảng màu, như Groovex). Không
//! có giới hạn tốc độ, địa chỉ tay, thư mục theo dõi, thư mục tải về (gỡ 12/11/2026; thêm điện thoại bằng địa chỉ ở màn
//! Thêm điện thoại).
//!
//! Chỉ vẽ `SettingsView` nhận từ daemon và gửi lệnh qua [`SettingsClient`] (kết nối riêng, **chặn** ⇒ luôn chạy trong
//! `background_spawn`). Mỗi mục có cờ `busy` riêng: lệnh đang chạy thì khoá điều khiển của mục đó, không gửi chồng; lỗi
//! hiện dưới đúng mục bằng câu nói việc cần làm. Điện thoại đã ghép (đổi tên, xoá) ở màn Danh sách thiết bị
//! (`views/devices.rs`), không ở đây.

use ferry_core::config::ThemeChoice;
use ferry_core::i18n::{I18n, Language, LanguagePref};
use ferry_core::ipc::{SettingsOp, SettingsView};
use materiant::button::Button;
use materiant::button::segmented::SegmentedButtons;
use materiant::card::CardGroup;
use materiant::gpui_kit::base::Disableable;
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    self, App, AppContext, ClickEvent, Context, EventEmitter, IntoElement, Render, SharedString, Window, div, px,
};
use materiant::scroll::ScrollArea;
use materiant::select::Select;
use materiant::switch::Switch;
use materiant::theme::ActiveTheme;
use materiant::theme::picker::ThemePicker;

use crate::settings_link::{CallError, Request, SettingsClient, expect_settings};

// ---------------------------------------------------------------------------------------------------------------
// Logic thuần (test được)

/// Mục của màn: mỗi mục có cờ `busy` và dòng lỗi riêng.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Language,
    Appearance,
    System,
}

impl Section {
    const COUNT: usize = 3;

    fn ix(self) -> usize {
        self as usize
    }
}

enum Load {
    Loading,
    Failed(SharedString),
    Ready,
}

/// Khoá i18n cho một lỗi — không bao giờ nói mã lỗi. Lệnh của trang (ngôn ngữ, giao diện, khởi động) chỉ hỏng khi
/// daemon không chạy hoặc lỗi nội bộ.
fn error_key(error: &CallError) -> &'static str {
    match error {
        CallError::NoDaemon => "settings.error.no_daemon",
        CallError::Failed(_) => "settings.error.internal",
    }
}

fn language_index(language: LanguagePref) -> usize {
    match language {
        LanguagePref::System => 0,
        LanguagePref::Vi => 1,
        LanguagePref::En => 2,
    }
}

fn language_from_index(ix: usize) -> LanguagePref {
    match ix {
        1 => LanguagePref::Vi,
        2 => LanguagePref::En,
        _ => LanguagePref::System,
    }
}

fn theme_choice_key(choice: ThemeChoice) -> &'static str {
    match choice {
        ThemeChoice::Light => "settings.appearance.mode_light",
        ThemeChoice::Dark => "settings.appearance.mode_dark",
        ThemeChoice::System => "settings.appearance.mode_system",
    }
}

/// Trang báo cửa sổ chính: ngôn ngữ vừa đổi ⇒ dựng lại chữ của cột trái, thanh trên.
pub enum SettingsEvent {
    LanguageChanged(Language),
}

pub struct SettingsPage {
    client: SettingsClient,
    i18n: I18n,
    load: Load,
    data: Option<SettingsView>,
    busy: [bool; Section::COUNT],
    errors: [Option<SharedString>; Section::COUNT],
    /// Giao diện vừa đổi trên cửa sổ, chưa gửi daemon.
    appearance_dirty: bool,
}

impl EventEmitter<SettingsEvent> for SettingsPage {}

impl SettingsPage {
    pub fn new(i18n: I18n, window: &mut Window, cx: &mut Context<Self>) -> SettingsPage {
        let mut page = SettingsPage {
            client: SettingsClient::new(),
            i18n,
            load: Load::Loading,
            data: None,
            busy: [false; Section::COUNT],
            errors: Default::default(),
            appearance_dirty: false,
        };
        page.reload(window, cx);
        page
    }

    fn t(&self, key: &str) -> SharedString {
        self.i18n.t(key).to_owned().into()
    }

    // ----- nạp + áp dữ liệu -----

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load = Load::Loading;
        cx.notify();
        let client = self.client.clone();
        let task = cx.background_spawn(async move { client.get() });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update(cx, |page, cx| page.loaded(result, cx)).ok();
        })
        .detach();
    }

    fn loaded(&mut self, result: Result<SettingsView, CallError>, cx: &mut Context<Self>) {
        match result {
            Ok(view) => {
                self.apply_view(view, cx);
                self.load = Load::Ready;
            }
            Err(CallError::NoDaemon) => self.load = Load::Failed(self.t("settings.no_daemon")),
            Err(err) => {
                log::warn!("settings: load failed: {err:?}");
                self.load = Load::Failed(self.t("settings.load_failed"));
            }
        }
        cx.notify();
    }

    fn apply_view(&mut self, view: SettingsView, cx: &mut Context<Self>) {
        let language = view.language.resolve();
        if language != self.i18n.language() {
            self.i18n = I18n::new(language);
            self.errors = Default::default();
            cx.emit(SettingsEvent::LanguageChanged(language));
        }
        // Lựa chọn giao diện chưa lưu xong: giữ cái người dùng đang thấy, không giật về giá trị cũ của daemon.
        let mut view = view;
        if let Some(old) = self.data.as_ref().filter(|_| self.busy[Section::Appearance.ix()] || self.appearance_dirty) {
            view.theme = old.theme;
            view.theme_preset = old.theme_preset.clone();
        }
        self.data = Some(view);
        cx.notify();
    }

    // ----- chạy lệnh -----

    /// Chạy `job` (chặn) ở luồng nền; xong thì áp view mới hoặc ghi lỗi dưới `section`.
    fn run(
        &mut self,
        section: Section,
        job: impl FnOnce(&SettingsClient) -> Result<SettingsView, CallError> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy[section.ix()] {
            return;
        }
        self.busy[section.ix()] = true;
        self.errors[section.ix()] = None;
        cx.notify();
        let client = self.client.clone();
        let task = cx.background_spawn(async move { job(&client) });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |page, window, cx| page.finished(section, result, window, cx)).ok();
        })
        .detach();
    }

    fn finished(
        &mut self,
        section: Section,
        result: Result<SettingsView, CallError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy[section.ix()] = false;
        match result {
            Ok(view) => self.apply_view(view, cx),
            Err(err) => {
                log::info!("settings: {section:?} failed: {err:?}");
                self.errors[section.ix()] = Some(self.t(error_key(&err)));
            }
        }
        if section == Section::Appearance {
            self.flush_appearance(window, cx);
        }
        cx.notify();
    }

    fn run_op(&mut self, section: Section, op: SettingsOp, window: &mut Window, cx: &mut Context<Self>) {
        self.run(section, move |c| c.call(Request::Settings(op)).and_then(expect_settings), window, cx);
    }

    // ----- giao diện -----

    /// Áp ngay trên cửa sổ (không chờ daemon), rồi lưu qua IPC. Lưu hỏng thì vẫn giữ giao diện đã chọn tới khi đóng cửa sổ.
    fn set_appearance(&mut self, theme: ThemeChoice, preset: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data) = self.data.as_mut() else { return };
        if data.theme == theme && data.theme_preset == preset {
            return;
        }
        data.theme = theme;
        data.theme_preset = preset.clone();
        crate::appearance::apply(theme, &preset, cx);
        self.appearance_dirty = true;
        self.flush_appearance(window, cx);
    }

    /// Lưu lựa chọn giao diện đang hiện. Bấm lúc lệnh trước còn chạy ⇒ lưu tiếp khi nó xong (lựa chọn sau cùng thắng).
    fn flush_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy[Section::Appearance.ix()] || !self.appearance_dirty {
            return;
        }
        let Some(data) = self.data.as_ref() else { return };
        self.appearance_dirty = false;
        let (theme, preset) = (data.theme, data.theme_preset.clone());
        self.run(
            Section::Appearance,
            move |c| {
                c.call(Request::Settings(SettingsOp::SetTheme { theme }))?;
                c.call(Request::Settings(SettingsOp::SetThemePreset { preset })).and_then(expect_settings)
            },
            window,
            cx,
        );
    }

    // ----- vẽ -----

    fn error_line(&self, section: Section, cx: &App) -> Option<gpui_kit::Div> {
        let text = self.errors[section.ix()].clone()?;
        let theme = cx.theme();
        Some(div().pt_2().text_size(theme.caption_size()).text_color(theme.colors().danger).child(text))
    }

    fn render_message(&self, text: SharedString, retry: bool, cx: &mut Context<Self>) -> gpui_kit::Div {
        let muted = cx.theme().colors().muted;
        let mut column = div().flex().flex_col().items_center().gap_3().text_color(muted).child(text);
        if retry {
            let label = self.t("settings.retry");
            column = column.child(Button::new("settings-retry", label).outlined().on_click(cx.listener(
                |page, _: &ClickEvent, window, cx| {
                    page.reload(window, cx);
                },
            )));
        }
        div().flex_1().flex().items_center().justify_center().p_6().child(column)
    }

    /// Ngôn ngữ + khởi động cùng hệ thống — đầu trang, một card hai dòng có đường kẻ chia.
    fn render_language(&self, data: &SettingsView, cx: &mut Context<Self>) -> CardGroup {
        let busy = self.busy[Section::Language.ix()];
        let select = Select::new("settings-language")
            .items([
                self.t("settings.system.language_system"),
                self.t("settings.system.language_vi"),
                self.t("settings.system.language_en"),
            ])
            .selected(Some(language_index(data.language)))
            .disabled(busy)
            .w(px(180.))
            .on_change(cx.listener(|page, ix: &usize, window, cx| {
                let language = language_from_index(*ix);
                page.run_op(Section::Language, SettingsOp::SetLanguage { language }, window, cx);
            }));
        CardGroup::new()
            .standalone()
            .child(
                cell(setting_row(cx, self.t("settings.system.language"), None, select))
                    .children(self.error_line(Section::Language, cx)),
            )
            .child(self.render_autostart(data, cx))
    }

    /// Nền (Sáng / Tối / Theo hệ thống) + bảng màu (bốn preset của materiant), như Cài đặt → Chung của Groovex. Hai lựa
    /// chọn độc lập, áp ngay. Không khoá khi đang lưu: bấm liên tiếp chỉ đổi lựa chọn, lệnh lưu sau cùng thắng.
    fn render_appearance(&self, data: &SettingsView, cx: &mut Context<Self>) -> CardGroup {
        let presets = crate::appearance::presets(&self.i18n);
        let preset_ix = crate::appearance::preset_index(&data.theme_preset);
        let theme = data.theme;
        let mode = SegmentedButtons::new("settings-theme-mode")
            .items(ThemeChoice::ALL.map(|choice| self.t(theme_choice_key(choice))))
            .selected(ThemeChoice::ALL.iter().position(|c| *c == theme))
            .on_change(cx.listener(move |page, ix: &usize, window, cx| {
                let preset = page.data.as_ref().map(|d| d.theme_preset.clone()).unwrap_or_default();
                page.set_appearance(ThemeChoice::ALL[*ix], preset, window, cx);
            }));
        let keys: Vec<String> = presets.iter().map(|p| p.key.to_string()).collect();
        let picker = ThemePicker::new("settings-theme-preset").items(presets).selected(preset_ix).on_change(
            cx.listener(move |page, ix: &usize, window, cx| {
                let theme = page.data.as_ref().map_or(ThemeChoice::System, |d| d.theme);
                page.set_appearance(theme, keys[*ix].clone(), window, cx);
            }),
        );
        // Bảng màu là hàng thẻ rộng ⇒ nhãn ở trên, thẻ bên dưới (như ô ảnh nền trong Cài đặt của iOS).
        let palette = cell(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().child(self.t("settings.appearance.theme_preset")))
                .child(picker),
        )
        .children(self.error_line(Section::Appearance, cx));
        CardGroup::new()
            .standalone()
            .child(cell(setting_row(cx, self.t("settings.appearance.mode"), None, mode)))
            .child(palette)
    }

    fn render_autostart(&self, data: &SettingsView, cx: &mut Context<Self>) -> gpui_kit::Div {
        let busy = self.busy[Section::System.ix()];
        let row = setting_row(
            cx,
            self.t("settings.system.autostart"),
            None,
            Switch::new("settings-autostart")
                .on(data.autostart)
                .accessibility_label(self.t("settings.system.autostart"))
                .disabled(busy)
                .on_change(cx.listener(|page, on: &bool, window, cx| {
                    page.run_op(Section::System, SettingsOp::SetAutostart { enabled: *on }, window, cx);
                })),
        );
        let autostart_error = data
            .autostart_error
            .as_ref()
            .map(|error| danger_line(self.i18n.t_args("settings.system.autostart_error", &[("error", error)]), cx));
        cell(row).children(autostart_error).children(self.error_line(Section::System, cx))
    }
}

/// Đệm của một dòng trong card (một tầng đệm duy nhất; card không đặt đệm). Lỗi của dòng nằm ngay dưới, cùng ô.
const ROW_PAD_X: f32 = 16.;
const ROW_PAD_Y: f32 = 12.;

fn cell(content: impl IntoElement) -> gpui_kit::Div {
    div().w_full().flex().flex_col().px(px(ROW_PAD_X)).py(px(ROW_PAD_Y)).child(content)
}

fn danger_line(text: String, cx: &App) -> gpui_kit::Div {
    let theme = cx.theme();
    div().pt_2().text_size(theme.caption_size()).text_color(theme.colors().danger).child(SharedString::from(text))
}

/// Một dòng cài đặt: nhãn (+ dòng phụ mờ, cắt `…` khi dài) bên trái, điều khiển bên phải.
fn setting_row(cx: &App, label: SharedString, hint: Option<SharedString>, control: impl IntoElement) -> gpui_kit::Div {
    let theme = cx.theme();
    let (muted, caption) = (theme.colors().muted, theme.caption_size());
    let text = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap_0p5()
        .child(div().truncate().child(label))
        .children(hint.map(|h| div().truncate().text_size(caption).text_color(muted).child(h)));
    div()
        .min_h(px(32.))
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(text)
        .child(div().flex_none().child(control))
}

impl Render for SettingsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = div().size_full().flex().flex_col();
        let body = match (&self.load, self.data.clone()) {
            (Load::Ready, Some(data)) => {
                // D-16: khung cuộn chạm mép cửa sổ; đệm ngang nằm trong nội dung, bằng nhau hai bên, ≥ thumb.
                let sections = div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .py_4()
                    .child(self.render_language(&data, cx))
                    .child(self.render_appearance(&data, cx));
                ScrollArea::new("settings-scroll")
                    .vertical()
                    .flex_1()
                    .min_h_0()
                    .padding_x(px(16.))
                    .child(sections)
                    .into_any_element()
            }
            (Load::Failed(text), _) => self.render_message(text.clone(), true, cx).into_any_element(),
            _ => self.render_message(self.t("settings.loading"), false, cx).into_any_element(),
        };
        root.child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::ipc::CommandError;

    #[test]
    fn errors_become_actionable_keys() {
        assert_eq!(error_key(&CallError::Failed(CommandError::InvalidInput)), "settings.error.internal");
        assert_eq!(error_key(&CallError::NoDaemon), "settings.error.no_daemon");
    }

    #[test]
    fn language_index_round_trips() {
        for pref in [LanguagePref::System, LanguagePref::Vi, LanguagePref::En] {
            assert_eq!(language_from_index(language_index(pref)), pref);
        }
    }

    /// Mọi khoá `settings.*` viết trong mã (trừ phần test) phải có chữ ở cả vi và en.
    #[test]
    fn every_settings_key_is_translated() {
        let source = include_str!("settings.rs");
        let code = source.split_once("#[cfg(test)]").map_or(source, |(code, _)| code);
        let mut keys = Vec::new();
        for part in code.split("\"settings.").skip(1) {
            if let Some((rest, _)) = part.split_once('"') {
                keys.push(format!("settings.{rest}"));
            }
        }
        assert!(keys.len() > 10, "scan found only {} keys", keys.len());
        for lang in [Language::Vi, Language::En] {
            let i18n = I18n::new(lang);
            for key in &keys {
                assert_ne!(i18n.t(key), key.as_str(), "missing `{key}` for {lang:?}");
            }
        }
    }

    #[test]
    fn appearance_choices_have_keys() {
        let keys: Vec<_> = ThemeChoice::ALL.iter().map(|c| theme_choice_key(*c)).collect();
        assert_eq!(keys.len(), 3);
    }
}
