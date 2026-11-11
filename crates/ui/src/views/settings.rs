//! Cửa sổ cài đặt (gd1_12, PROTOCOL 4.5) — dựng theo `plan/ferry-gd1_12_settings.md` mục 7.
//!
//! Chỉ vẽ `SettingsView` nhận từ daemon và gửi lệnh qua [`SettingsClient`] (kết nối riêng, **chặn** ⇒ luôn chạy trong
//! `background_spawn`). Mỗi mục có cờ `busy` riêng: lệnh đang chạy thì khoá điều khiển của mục đó, không gửi chồng; lỗi
//! hiện dưới đúng mục bằng câu nói việc cần làm.

use std::time::Duration;

use ferry_core::config::{self, MAX_SPEED_LIMIT_MB_PER_SEC, WatchFolder};
use ferry_core::device::{DeviceId, DeviceState};
use ferry_core::i18n::{I18n, LanguagePref};
use ferry_core::ipc::{CommandError, DeviceSetting, PhoneOp, SettingsOp, SettingsView};
use ferry_core::paths;
use materiant::button::{Button, Size};
use materiant::card::{Card, CardGroup, CardItem};
use materiant::field::input::TextField;
use materiant::field::number::NumberField;
use materiant::gpui_kit::base::Disableable;
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{
    self, AnyWindowHandle, App, AppContext, Bounds, ClickEvent, Context, Entity, Focusable, Global, IntoElement,
    PathPromptOptions, Render, SharedString, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions, div, px,
    size,
};
use materiant::overlay::dialog::{Dialog, DialogHandle};
use materiant::scroll::ScrollArea;
use materiant::select::Select;
use materiant::switch::Switch;
use materiant::theme::ActiveTheme;
use std::path::PathBuf;

use crate::settings_link::{CallError, Request, SettingsClient, expect_settings};

const WINDOW_SIZE: (f32, f32) = (560., 640.);
const WINDOW_MIN_SIZE: (f32, f32) = (480., 420.);
/// Tốc độ gợi ý khi bật giới hạn mà ô số còn trống / sai.
const DEFAULT_SPEED_MB_PER_SEC: u32 = 10;
/// Gõ xong chừng này mới gửi giới hạn tốc độ (không gửi từng ký tự).
const SPEED_DEBOUNCE: Duration = Duration::from_millis(700);

/// Handle cửa sổ cài đặt đang mở — gọi `open` lần hai thì kích hoạt cửa sổ này thay vì mở thêm.
struct OpenWindow(AnyWindowHandle);
impl Global for OpenWindow {}

/// Mở cửa sổ cài đặt, hoặc đưa lên trước nếu đang mở.
pub fn open(cx: &mut App) {
    if let Some(handle) = cx.try_global::<OpenWindow>().map(|g| g.0)
        && cx.update_window(handle, |_, window, _| window.activate_window()).is_ok()
    {
        cx.activate(true);
        return;
    }
    let i18n = initial_i18n();
    let title: SharedString = i18n.t("settings.window_title").to_owned().into();
    let bounds = Bounds::centered(None, size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some(title), ..Default::default() }),
        window_min_size: Some(size(px(WINDOW_MIN_SIZE.0), px(WINDOW_MIN_SIZE.1))),
        app_id: Some("ferry".into()),
        window_decorations: super::title_bar::requested_decorations(),
        ..materiant::window::window_options()
    };
    match gpui_kit::open_window(options, cx, move |window, cx| cx.new(|cx| SettingsPage::new(i18n, window, cx))) {
        Ok((handle, _)) => {
            cx.set_global(OpenWindow(handle));
            cx.activate(true);
        }
        Err(err) => log::error!("cannot open the settings window: {err:#}"),
    }
}

/// Ngôn ngữ trước khi daemon trả lời: đọc `config.json` (chỉ đọc), giống cửa sổ chính.
fn initial_i18n() -> I18n {
    let language = paths::config_file()
        .and_then(|p| config::load(&p))
        .map_err(|e| log::warn!("settings: config: {e}; using defaults"))
        .unwrap_or_default()
        .language
        .resolve();
    I18n::new(language)
}

// ---------------------------------------------------------------------------------------------------------------
// Logic thuần (test được)

/// Mục của màn: mỗi mục có cờ `busy` và dòng lỗi riêng.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Devices,
    Files,
    Network,
    System,
    Advanced,
}

impl Section {
    const COUNT: usize = 5;

    fn ix(self) -> usize {
        self as usize
    }
}

/// Việc làm thêm khi lệnh thành công.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum After {
    Nothing,
    ClearAddress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickTarget {
    Download,
    Watch,
}

enum Load {
    Loading,
    Failed(SharedString),
    Ready,
}

/// Ô số → MB/s hợp lệ (`1..=MAX`, số nguyên).
fn parse_speed(value: Option<f64>) -> Option<u32> {
    let v = value?;
    if !v.is_finite() || v.fract() != 0. || v < 1. || v > f64::from(MAX_SPEED_LIMIT_MB_PER_SEC) {
        return None;
    }
    Some(v as u32)
}

/// Khoá i18n (+ tham số `{name}` nếu có) cho một lỗi, theo mục gây ra — không bao giờ nói mã lỗi.
fn error_key(section: Section, error: &CallError) -> (&'static str, Option<String>) {
    let command = match error {
        CallError::NoDaemon => return ("settings.error.no_daemon", None),
        CallError::Failed(c) => c,
    };
    let key = match command {
        CommandError::UnknownDevice | CommandError::NotPaired => "settings.error.unknown_device",
        CommandError::Offline => "settings.error.offline",
        CommandError::NotWritable => "settings.error.not_writable",
        CommandError::Unreachable | CommandError::OtherPhone => "settings.error.unreachable",
        CommandError::UnknownInterface { name } => return ("settings.error.unknown_interface", Some(name.clone())),
        CommandError::InvalidInput => match section {
            Section::Files => "settings.error.invalid_path",
            Section::Advanced => "settings.error.invalid_address",
            Section::Network => "settings.error.invalid_speed",
            Section::Devices => "settings.error.invalid_name",
            Section::System => "settings.error.internal",
        },
        _ => "settings.error.internal",
    };
    (key, None)
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

fn device_state_key(state: DeviceState) -> &'static str {
    match state {
        DeviceState::Online => "settings.devices.state_online",
        DeviceState::NeedsRepair => "settings.devices.state_needs_repair",
        DeviceState::Paired => "settings.devices.state_paired",
        DeviceState::Offline | DeviceState::Discovered | DeviceState::Unknown => "settings.devices.state_offline",
    }
}

// ---------------------------------------------------------------------------------------------------------------
// View

struct SettingsPage {
    client: SettingsClient,
    i18n: I18n,
    load: Load,
    data: Option<SettingsView>,
    busy: [bool; Section::COUNT],
    errors: [Option<SharedString>; Section::COUNT],
    speed: Entity<NumberField>,
    address: Entity<TextField>,
    rename_field: Entity<TextField>,
    rename_dialog: DialogHandle,
    rename_target: Option<DeviceId>,
    forget_dialog: DialogHandle,
    forget_target: Option<(DeviceId, String)>,
    /// Tăng mỗi lần gõ ô tốc độ — bộ đếm trễ chỉ chạy nếu còn là lần gõ cuối.
    speed_gen: u64,
    /// Ô tốc độ đổi lúc lệnh trước còn chạy ⇒ gửi lại ngay khi xong.
    speed_dirty: bool,
}

impl SettingsPage {
    fn new(i18n: I18n, window: &mut Window, cx: &mut Context<Self>) -> SettingsPage {
        let this: WeakEntity<SettingsPage> = cx.entity().downgrade();
        let speed = cx.new(|cx| {
            let this = this.clone();
            NumberField::new("settings-speed", window, cx)
                .range(1..=MAX_SPEED_LIMIT_MB_PER_SEC)
                .step(1)
                .default_value(f64::from(DEFAULT_SPEED_MB_PER_SEC))
                .on_change(move |value, window, cx| {
                    this.update(cx, |page, cx| page.speed_changed(value, window, cx)).ok();
                })
        });
        let address = cx.new(|cx| {
            let this = this.clone();
            TextField::new("settings-address", window, cx).placeholder("192.168.1.9:8080").on_submit(
                move |_, window, cx| {
                    this.update(cx, |page, cx| page.add_address(window, cx)).ok();
                },
            )
        });
        let rename_field = cx.new(|cx| {
            let this = this.clone();
            TextField::new("settings-rename", window, cx).on_submit(move |_, window, cx| {
                this.update(cx, |page, cx| page.confirm_rename(window, cx)).ok();
            })
        });
        let mut page = SettingsPage {
            client: SettingsClient::new(),
            i18n,
            load: Load::Loading,
            data: None,
            busy: [false; Section::COUNT],
            errors: Default::default(),
            speed,
            address,
            rename_field,
            rename_dialog: DialogHandle::new(cx),
            rename_target: None,
            forget_dialog: DialogHandle::new(cx),
            forget_target: None,
            speed_gen: 0,
            speed_dirty: false,
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
            this.update_in(cx, |page, window, cx| page.loaded(result, window, cx)).ok();
        })
        .detach();
    }

    fn loaded(&mut self, result: Result<SettingsView, CallError>, window: &mut Window, cx: &mut Context<Self>) {
        match result {
            Ok(view) => {
                self.apply_view(view, true, window, cx);
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

    /// `sync_inputs`: đặt lại ô số theo daemon (lúc nạp đầu) — không giật chữ người dùng đang gõ ở các lần sau.
    fn apply_view(&mut self, view: SettingsView, sync_inputs: bool, window: &mut Window, cx: &mut Context<Self>) {
        let language = view.language.resolve();
        if language != self.i18n.language() {
            self.i18n = I18n::new(language);
            window.set_window_title(self.i18n.t("settings.window_title"));
            self.errors = Default::default();
        }
        if sync_inputs {
            let value = f64::from(view.speed_limit_mb_per_sec.unwrap_or(DEFAULT_SPEED_MB_PER_SEC));
            self.speed.update(cx, |field, cx| {
                field.set_value(value, window, cx);
                field.set_error(None, cx);
            });
        }
        let limited = view.speed_limit_mb_per_sec.is_some();
        self.speed.update(cx, |field, cx| field.set_disabled(!limited, cx));
        self.data = Some(view);
        cx.notify();
    }

    // ----- chạy lệnh -----

    /// Chạy `job` (chặn) ở luồng nền; xong thì áp view mới hoặc ghi lỗi dưới `section`.
    fn run(
        &mut self,
        section: Section,
        after: After,
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
            this.update_in(cx, |page, window, cx| page.finished(section, after, result, window, cx)).ok();
        })
        .detach();
    }

    fn finished(
        &mut self,
        section: Section,
        after: After,
        result: Result<SettingsView, CallError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy[section.ix()] = false;
        match result {
            Ok(view) => {
                self.apply_view(view, false, window, cx);
                if after == After::ClearAddress {
                    self.address.update(cx, |field, cx| field.clear(window, cx));
                }
            }
            Err(err) => {
                log::info!("settings: {section:?} failed: {err:?}");
                let (key, name) = error_key(section, &err);
                let text = match name {
                    Some(name) => self.i18n.t_args(key, &[("name", &name)]),
                    None => self.i18n.t(key).to_owned(),
                };
                self.errors[section.ix()] = Some(text.into());
            }
        }
        if section == Section::Network && std::mem::take(&mut self.speed_dirty) {
            self.apply_speed(window, cx);
        }
        cx.notify();
    }

    fn run_op(&mut self, section: Section, op: SettingsOp, window: &mut Window, cx: &mut Context<Self>) {
        self.run(section, After::Nothing, move |c| c.call(Request::Settings(op)).and_then(expect_settings), window, cx);
    }

    // ----- thiết bị -----

    fn open_rename(&mut self, device: &DeviceSetting, window: &mut Window, cx: &mut Context<Self>) {
        self.rename_target = Some(device.id.clone());
        let name = device.display_name().to_owned();
        self.rename_field.update(cx, |field, cx| field.set_value(name, window, cx));
        self.rename_dialog.open(window, cx);
        window.focus(&self.rename_field.focus_handle(cx), cx);
        cx.notify();
    }

    fn confirm_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(device) = self.rename_target.take() else { return };
        let name = self.rename_field.read(cx).value(cx).trim().to_owned();
        self.rename_dialog.close(window, cx);
        self.run_op(Section::Devices, SettingsOp::RenameDevice { device, name }, window, cx);
    }

    fn open_forget(&mut self, device: &DeviceSetting, window: &mut Window, cx: &mut Context<Self>) {
        self.forget_target = Some((device.id.clone(), device.display_name().to_owned()));
        self.forget_dialog.open(window, cx);
        cx.notify();
    }

    fn confirm_forget(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((device, _)) = self.forget_target.take() else { return };
        self.forget_dialog.close(window, cx);
        self.run(
            Section::Devices,
            After::Nothing,
            move |c| {
                c.call(Request::Phone(PhoneOp::Forget { device }))?;
                c.get()
            },
            window,
            cx,
        );
    }

    // ----- tệp -----

    fn pick_folder(&mut self, target: PickTarget, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy[Section::Files.ix()] {
            return;
        }
        let picked =
            cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            let path = picked.await.ok().and_then(Result::ok).flatten().and_then(|p| p.into_iter().next());
            if let Some(path) = path {
                this.update_in(cx, |page, window, cx| page.folder_picked(target, path, window, cx)).ok();
            }
        })
        .detach();
    }

    fn folder_picked(&mut self, target: PickTarget, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let op = match target {
            PickTarget::Download => SettingsOp::SetDownloadDir { path: Some(path) },
            PickTarget::Watch => SettingsOp::SetWatchFolder { folder: WatchFolder { enabled: true, path: Some(path) } },
        };
        self.run_op(Section::Files, op, window, cx);
    }

    fn set_watch_enabled(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.data.as_ref().and_then(|d| d.watch_folder.path.clone());
        if on && path.is_none() {
            // Chưa có thư mục thì hỏi trước; chọn xong mới bật.
            self.pick_folder(PickTarget::Watch, window, cx);
            return;
        }
        self.run_op(
            Section::Files,
            SettingsOp::SetWatchFolder { folder: WatchFolder { enabled: on, path } },
            window,
            cx,
        );
    }

    // ----- mạng -----

    fn set_speed_enabled(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let mb_per_sec = if on {
            let n = parse_speed(self.speed.read(cx).value(cx)).unwrap_or(DEFAULT_SPEED_MB_PER_SEC);
            self.speed.update(cx, |field, cx| {
                field.set_value(f64::from(n), window, cx);
                field.set_error(None, cx);
            });
            Some(n)
        } else {
            self.speed.update(cx, |field, cx| field.set_error(None, cx));
            None
        };
        self.speed_gen += 1;
        self.run_op(Section::Network, SettingsOp::SetSpeedLimit { mb_per_sec }, window, cx);
    }

    fn speed_changed(&mut self, _value: Option<f64>, window: &mut Window, cx: &mut Context<Self>) {
        self.speed_gen += 1;
        let generation = self.speed_gen;
        let timer = cx.background_executor().timer(SPEED_DEBOUNCE);
        cx.spawn_in(window, async move |this, cx| {
            timer.await;
            this.update_in(cx, |page, window, cx| {
                if page.speed_gen == generation {
                    page.apply_speed(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Gửi số trong ô (nếu hợp lệ và giới hạn đang bật).
    fn apply_speed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let limited = self.data.as_ref().is_some_and(|d| d.speed_limit_mb_per_sec.is_some());
        if !limited {
            return;
        }
        let Some(n) = parse_speed(self.speed.read(cx).value(cx)) else {
            let text =
                self.i18n.t_args("settings.error.invalid_speed", &[("max", &MAX_SPEED_LIMIT_MB_PER_SEC.to_string())]);
            self.speed.update(cx, |field, cx| field.set_error(Some(text.into()), cx));
            return;
        };
        self.speed.update(cx, |field, cx| field.set_error(None, cx));
        if self.data.as_ref().and_then(|d| d.speed_limit_mb_per_sec) == Some(n) {
            return;
        }
        if self.busy[Section::Network.ix()] {
            self.speed_dirty = true;
            return;
        }
        self.run_op(Section::Network, SettingsOp::SetSpeedLimit { mb_per_sec: Some(n) }, window, cx);
    }

    // ----- nâng cao -----

    fn add_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let address = self.address.read(cx).value(cx).trim().to_owned();
        if address.is_empty() || self.busy[Section::Advanced.ix()] {
            return;
        }
        self.run(
            Section::Advanced,
            After::ClearAddress,
            move |c| c.call(Request::Settings(SettingsOp::AddAddress { address })).and_then(expect_settings),
            window,
            cx,
        );
    }

    // ----- vẽ -----

    fn error_line(&self, section: Section, cx: &App) -> Option<gpui_kit::Div> {
        let text = self.errors[section.ix()].clone()?;
        let theme = cx.theme();
        Some(div().py_2().text_size(theme.caption_size()).text_color(theme.colors().danger).child(text))
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

    fn render_devices(&self, data: &SettingsView, cx: &mut Context<Self>) -> Card {
        let busy = self.busy[Section::Devices.ix()];
        let muted = cx.theme().colors().muted;
        let mut card = Card::new("settings-devices")
            .title(self.t("settings.devices.title"))
            .description(self.t("settings.devices.description"));
        if data.devices.is_empty() {
            card = card.child(div().py_2().text_color(muted).child(self.t("settings.devices.empty")));
        } else {
            let items = data.devices.iter().enumerate().map(|(ix, device)| {
                let rename_device = device.clone();
                let forget_device = device.clone();
                let mut item =
                    CardItem::new(device.display_name().to_owned()).line(self.i18n.t(device_state_key(device.state)));
                if device.alias.is_some() {
                    item = item.line(self.i18n.t_args("settings.devices.phone_name", &[("name", &device.name)]));
                }
                item.trailing(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new(("settings-rename", ix), self.t("settings.devices.rename"))
                                .outlined()
                                .size(Size::Sm)
                                .disabled(busy)
                                .on_click(cx.listener(move |page, _: &ClickEvent, window, cx| {
                                    page.open_rename(&rename_device, window, cx);
                                })),
                        )
                        .child(
                            Button::new(("settings-forget", ix), self.t("settings.devices.forget"))
                                .text()
                                .danger()
                                .size(Size::Sm)
                                .disabled(busy)
                                .on_click(cx.listener(move |page, _: &ClickEvent, window, cx| {
                                    page.open_forget(&forget_device, window, cx);
                                })),
                        ),
                )
            });
            card = card.child(CardGroup::new().children(items));
        }
        card.children(self.error_line(Section::Devices, cx))
    }

    fn render_files(&self, data: &SettingsView, cx: &mut Context<Self>) -> Card {
        let busy = self.busy[Section::Files.ix()];
        let download = setting_row(
            cx,
            self.t("settings.files.download_dir"),
            Some(data.download_dir.display().to_string().into()),
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("settings-download-choose", self.t("settings.files.choose"))
                        .outlined()
                        .size(Size::Sm)
                        .disabled(busy)
                        .on_click(cx.listener(|page, _: &ClickEvent, window, cx| {
                            page.pick_folder(PickTarget::Download, window, cx);
                        })),
                )
                .child(
                    Button::new("settings-download-reset", self.t("settings.files.reset"))
                        .text()
                        .size(Size::Sm)
                        .disabled(busy || !data.download_dir_custom)
                        .on_click(cx.listener(|page, _: &ClickEvent, window, cx| {
                            page.run_op(Section::Files, SettingsOp::SetDownloadDir { path: None }, window, cx);
                        })),
                ),
        );
        let watch_hint = match &data.watch_folder.path {
            Some(path) => path.display().to_string().into(),
            None => self.t("settings.files.watch_folder_none"),
        };
        let watch = setting_row(
            cx,
            self.t("settings.files.watch_folder"),
            Some(watch_hint),
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new("settings-watch-choose", self.t("settings.files.choose"))
                        .outlined()
                        .size(Size::Sm)
                        .disabled(busy)
                        .on_click(cx.listener(|page, _: &ClickEvent, window, cx| {
                            page.pick_folder(PickTarget::Watch, window, cx);
                        })),
                )
                .child(
                    Switch::new("settings-watch")
                        .on(data.watch_folder.enabled)
                        .accessibility_label(self.t("settings.files.watch_folder"))
                        .disabled(busy)
                        .on_change(cx.listener(|page, on: &bool, window, cx| page.set_watch_enabled(*on, window, cx))),
                ),
        );
        Card::new("settings-files")
            .title(self.t("settings.files.title"))
            .child(download)
            .child(watch)
            .children(self.error_line(Section::Files, cx))
    }

    fn render_network(&self, data: &SettingsView, cx: &mut Context<Self>) -> Card {
        let busy = self.busy[Section::Network.ix()];
        let muted = cx.theme().colors().muted;
        let row = setting_row(
            cx,
            self.t("settings.network.speed_limit"),
            Some(self.t("settings.network.speed_limit_hint")),
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(96.)).child(self.speed.clone()))
                .child(div().text_color(muted).child(self.t("settings.network.speed_unit")))
                .child(
                    Switch::new("settings-speed-switch")
                        .on(data.speed_limit_mb_per_sec.is_some())
                        .accessibility_label(self.t("settings.network.speed_limit"))
                        .disabled(busy)
                        .on_change(cx.listener(|page, on: &bool, window, cx| page.set_speed_enabled(*on, window, cx))),
                ),
        );
        Card::new("settings-network")
            .title(self.t("settings.network.title"))
            .child(row)
            .children(self.error_line(Section::Network, cx))
    }

    fn render_system(&self, data: &SettingsView, cx: &mut Context<Self>) -> Card {
        let busy = self.busy[Section::System.ix()];
        let autostart = setting_row(
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
        let autostart_error = data.autostart_error.as_ref().map(|error| {
            let text = self.i18n.t_args("settings.system.autostart_error", &[("error", error)]);
            let theme = cx.theme();
            div()
                .py_2()
                .text_size(theme.caption_size())
                .text_color(theme.colors().danger)
                .child(SharedString::from(text))
        });
        let language = setting_row(
            cx,
            self.t("settings.system.language"),
            None,
            Select::new("settings-language")
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
                    page.run_op(Section::System, SettingsOp::SetLanguage { language }, window, cx);
                })),
        );
        Card::new("settings-system")
            .title(self.t("settings.system.title"))
            .child(autostart)
            .children(autostart_error)
            .child(language)
            .children(self.error_line(Section::System, cx))
    }

    fn render_advanced(&self, data: &SettingsView, cx: &mut Context<Self>) -> Card {
        let busy = self.busy[Section::Advanced.ix()];
        let muted = cx.theme().colors().muted;
        let head = setting_row(
            cx,
            self.t("settings.advanced.addresses"),
            Some(self.t("settings.advanced.addresses_hint")),
            div(),
        );
        let input = div()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .child(div().flex_1().min_w_0().child(self.address.clone()))
            .child(
                Button::new("settings-address-add", self.t("settings.advanced.add"))
                    .disabled(busy)
                    .on_click(cx.listener(|page, _: &ClickEvent, window, cx| page.add_address(window, cx))),
            );
        let list = if data.manual_addresses.is_empty() {
            div().py_2().text_color(muted).child(self.t("settings.advanced.empty"))
        } else {
            div().flex().flex_col().children(data.manual_addresses.iter().enumerate().map(|(ix, address)| {
                let target = address.clone();
                setting_row(
                    cx,
                    address.clone().into(),
                    None,
                    Button::new(("settings-address-remove", ix), self.t("settings.advanced.remove"))
                        .text()
                        .danger()
                        .size(Size::Sm)
                        .disabled(busy)
                        .on_click(cx.listener(move |page, _: &ClickEvent, window, cx| {
                            page.run_op(
                                Section::Advanced,
                                SettingsOp::RemoveAddress { address: target.clone() },
                                window,
                                cx,
                            );
                        })),
                )
            }))
        };
        Card::new("settings-advanced")
            .title(self.t("settings.advanced.title"))
            .child(head)
            .child(input)
            .child(list)
            .children(self.error_line(Section::Advanced, cx))
    }

    /// Linux (Mutter không có SSD): GPUI rơi về CSD ⇒ tự vẽ thanh tiêu đề kéo được + nút đóng.
    fn render_dialogs(&self, cx: &mut Context<Self>) -> (Dialog, Dialog) {
        let muted = cx.theme().colors().muted;
        let caption = cx.theme().caption_size();
        let rename = Dialog::new("settings-rename-dialog", &self.rename_dialog)
            .title(self.t("settings.rename.title"))
            .max_width(px(400.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().child(self.t("settings.rename.label")))
                    .child(self.rename_field.clone())
                    .child(div().text_size(caption).text_color(muted).child(self.t("settings.rename.hint"))),
            )
            .actions([
                Button::new("settings-rename-cancel", self.t("settings.rename.cancel"))
                    .text()
                    .base()
                    .on_click(cx.listener(|page, _: &ClickEvent, window, cx| {
                        page.rename_target = None;
                        page.rename_dialog.close(window, cx);
                    }))
                    .into_any_element(),
                Button::new("settings-rename-save", self.t("settings.rename.save"))
                    .on_click(cx.listener(|page, _: &ClickEvent, window, cx| page.confirm_rename(window, cx)))
                    .into_any_element(),
            ]);
        let name = self.forget_target.as_ref().map(|(_, n)| n.as_str()).unwrap_or_default();
        let forget = Dialog::new("settings-forget-dialog", &self.forget_dialog)
            .title(self.i18n.t_args("settings.forget.title", &[("name", name)]))
            .max_width(px(400.))
            .child(div().text_color(muted).child(self.t("settings.forget.body")))
            .actions([
                Button::new("settings-forget-cancel", self.t("settings.forget.cancel"))
                    .text()
                    .base()
                    .on_click(cx.listener(|page, _: &ClickEvent, window, cx| {
                        page.forget_target = None;
                        page.forget_dialog.close(window, cx);
                    }))
                    .into_any_element(),
                Button::new("settings-forget-confirm", self.t("settings.forget.confirm"))
                    .danger()
                    .on_click(cx.listener(|page, _: &ClickEvent, window, cx| page.confirm_forget(window, cx)))
                    .into_any_element(),
            ]);
        (rename, forget)
    }
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
    div().py_2().flex().items_center().justify_between().gap_3().child(text).child(div().flex_none().child(control))
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (colors, font_family, font_size) = {
            let theme = cx.theme();
            (theme.colors().clone(), theme.font_family(), theme.font_size())
        };
        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.surface)
            .text_color(colors.text)
            .font_family(font_family)
            .text_size(font_size);
        let body = match (&self.load, self.data.clone()) {
            (Load::Ready, Some(data)) => {
                // D-16: khung cuộn chạm mép cửa sổ; đệm ngang nằm trong nội dung, bằng nhau hai bên, ≥ thumb.
                let sections = div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .py_5()
                    .child(self.render_devices(&data, cx))
                    .child(self.render_files(&data, cx))
                    .child(self.render_network(&data, cx))
                    .child(self.render_system(&data, cx))
                    .child(self.render_advanced(&data, cx));
                ScrollArea::new("settings-scroll")
                    .vertical()
                    .flex_1()
                    .min_h_0()
                    .padding_x(px(24.))
                    .child(sections)
                    .into_any_element()
            }
            (Load::Failed(text), _) => self.render_message(text.clone(), true, cx).into_any_element(),
            _ => self.render_message(self.t("settings.loading"), false, cx).into_any_element(),
        };
        let (rename, forget) = self.render_dialogs(cx);
        let bar = super::title_bar::title_bar(
            "settings-close",
            self.t("settings.window_title"),
            self.t("settings.close"),
            window,
            cx,
        );
        root.children(bar).child(body).child(rename).child(forget)
    }
}

#[cfg(test)]
mod tests {
    use ferry_core::i18n::Language;

    use super::*;

    #[test]
    fn speed_accepts_only_whole_numbers_in_range() {
        assert_eq!(parse_speed(Some(1.)), Some(1));
        assert_eq!(parse_speed(Some(f64::from(MAX_SPEED_LIMIT_MB_PER_SEC))), Some(MAX_SPEED_LIMIT_MB_PER_SEC));
        assert_eq!(parse_speed(Some(0.)), None);
        assert_eq!(parse_speed(Some(f64::from(MAX_SPEED_LIMIT_MB_PER_SEC) + 1.)), None);
        assert_eq!(parse_speed(Some(2.5)), None);
        assert_eq!(parse_speed(Some(f64::NAN)), None);
        assert_eq!(parse_speed(None), None);
    }

    #[test]
    fn errors_become_actionable_keys_per_section() {
        use CommandError::*;
        let failed = |c| CallError::Failed(c);
        assert_eq!(error_key(Section::Files, &failed(InvalidInput)).0, "settings.error.invalid_path");
        assert_eq!(error_key(Section::Advanced, &failed(InvalidInput)).0, "settings.error.invalid_address");
        assert_eq!(error_key(Section::Files, &failed(NotWritable)).0, "settings.error.not_writable");
        assert_eq!(error_key(Section::Advanced, &failed(Unreachable)).0, "settings.error.unreachable");
        assert_eq!(
            error_key(Section::Advanced, &failed(UnknownInterface { name: "en9".into() })),
            ("settings.error.unknown_interface", Some("en9".into()))
        );
        assert_eq!(error_key(Section::Devices, &failed(UnknownDevice)).0, "settings.error.unknown_device");
        assert_eq!(error_key(Section::System, &failed(Internal)).0, "settings.error.internal");
        assert_eq!(error_key(Section::System, &CallError::NoDaemon).0, "settings.error.no_daemon");
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
        assert!(keys.len() > 40, "scan found only {} keys", keys.len());
        for lang in [Language::Vi, Language::En] {
            let i18n = I18n::new(lang);
            for key in &keys {
                assert_ne!(i18n.t(key), key.as_str(), "missing `{key}` for {lang:?}");
            }
        }
    }

    #[test]
    fn device_states_map_to_keys() {
        assert_eq!(device_state_key(DeviceState::Online), "settings.devices.state_online");
        assert_eq!(device_state_key(DeviceState::NeedsRepair), "settings.devices.state_needs_repair");
        assert_eq!(device_state_key(DeviceState::Offline), "settings.devices.state_offline");
    }
}
