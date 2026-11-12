//! Cắt / Sao chép sang máy tính (W-10, PROTOCOL §6.14): cùng "Cắt" / "Sao chép" của W-9, thêm phần dán ở trình quản lý
//! tệp của máy (Files, Explorer, Finder).
//!
//! UI không chạm mạng hay clipboard của máy: gửi `LibraryOp::Export`, daemon tải vào thư mục tạm, đặt clipboard (tiến
//! trình nền sống lâu — cửa sổ đóng vẫn dán được) và báo tiến độ ở `Snapshot::export`. Thanh dưới bảng hiện "Đang chuẩn
//! bị để dán…" ⇒ "Sẵn sàng dán" + nút Bỏ. Cắt: dán ở máy xong daemon xoá trên điện thoại ⇒ ở đây bỏ bảng nháp + đọc lại
//! thư mục cha. Phím: Ctrl+C / Ctrl+X / Ctrl+V (⌘ trên macOS) trong cây / bảng.

use ferry_core::browse::Location;
use ferry_core::ipc::{CommandError, ExportEntry, ExportState, ExportView, LibraryOp, Outcome};
use materiant::button::{Button, Size};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{AnyElement, App, Context, KeyDownEvent, SharedString, Window, div};
use materiant::theme::ActiveTheme;

use super::edit::{Pending, Target, error_key};
use super::{BrowserMode, PhoneBrowser};
use crate::fmt;

impl PhoneBrowser {
    /// Điện thoại khai `fetch` (§6.14) và đang duyệt bộ nhớ thật ở trang thư viện.
    pub(crate) fn export_known(&self, cx: &App) -> bool {
        self.browses_storage() && self.mode == BrowserMode::Library && self.model.read(cx).fetches_storage(&self.device)
    }

    /// "Sao chép" bật được: chép trong điện thoại (W-9) hoặc sang máy (W-10).
    pub(crate) fn can_copy_out(&self, cx: &App) -> bool {
        self.copy_known(cx) || self.export_known(cx)
    }

    /// "Cắt" bật được: dời trong điện thoại, hoặc sang máy (cần thêm `edit` để xoá sau khi dán).
    pub(crate) fn can_cut_out(&self, cx: &App) -> bool {
        self.copy_known(cx) || (self.export_known(cx) && self.edit_known(cx))
    }

    /// Bắt đầu tải các mục vào thư mục tạm để dán ở máy (thay lần trước).
    pub(crate) fn start_export(&mut self, targets: &[Target], cut: bool, cx: &mut Context<Self>) {
        if self.offline || self.mode != BrowserMode::Library || !self.browses_storage() {
            return;
        }
        if !self.model.read(cx).fetches_storage(&self.device) {
            // App điện thoại chưa có §6.14 ⇒ chỉ dán được trong điện thoại (W-9). Nói rõ thay vì im lặng: người dùng
            // tưởng đã chép sang máy rồi dán ở Files không ra gì.
            self.notice = Some(self.i18n.t("browser.export_unsupported").to_owned());
            cx.notify();
            return;
        }
        if cut && !self.edit_known(cx) {
            return;
        }
        let entries = targets.iter().map(|t| ExportEntry { path: t.path.clone(), folder: t.folder }).collect();
        let op = LibraryOp::Export { device: self.device.clone(), entries, cut };
        if let Some(req) = self.model.update(cx, |m, _| m.library(op)) {
            self.edit_req.insert(req, Pending::Export);
        }
    }

    pub(crate) fn on_export_reply(&mut self, outcome: &Outcome, cx: &mut Context<Self>) {
        if let Outcome::Failed { error } = outcome {
            let why = self.i18n.t(export_error_key(error)).to_owned();
            self.notice = Some(self.i18n.t_args("browser.export_failed", &[("why", &why)]));
            cx.notify();
        }
    }

    /// Lần chép sang máy của điện thoại này.
    fn export<'a>(&self, cx: &'a App) -> Option<&'a ExportView> {
        self.model.read(cx).snapshot.export.as_deref().filter(|e| e.device == self.device)
    }

    pub(crate) fn cancel_export(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |m, _| m.library(LibraryOp::CancelExport));
    }

    /// Dán Cắt trong điện thoại (W-9) thay cho dán ở máy ⇒ bỏ danh sách tệp đang chờ trên clipboard của máy (mục tạm sắp
    /// lỗi thời; dán nhầm ở máy sau đó cũng không xoá gì vì đường cũ không còn).
    pub(crate) fn drop_export_of(&mut self, paths: &[String], cx: &mut Context<Self>) {
        if self.export(cx).is_some_and(|e| e.cut && e.paths.iter().any(|p| paths.contains(p))) {
            self.cancel_export(cx);
        }
    }

    /// Ảnh chụp mới: Cắt đã dán ở máy (daemon xoá xong trên điện thoại) ⇒ bỏ bảng nháp trùng mục, đọc lại thư mục cha.
    pub(crate) fn sync_export(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.export(cx) else { return };
        if !matches!(view.state, ExportState::Moved { .. }) {
            return;
        }
        let paths = view.paths.clone();
        let device = self.device.clone();
        let cleared = self.model.update(cx, |m, _| {
            let same = m.clip.as_ref().is_some_and(|c| {
                c.device == device && c.cut && c.entries.iter().all(|e| paths.contains(&e.path))
            });
            if same {
                m.clip = None;
            }
            same
        });
        if !cleared {
            return;
        }
        let moved: Vec<Target> = paths.iter().map(|p| Target::of_path(p)).collect();
        if let Some(fs) = self.storage.as_mut() {
            for t in &moved {
                fs.remove(&t.path);
            }
        }
        self.leave_deleted(&moved, cx);
        let parents: std::collections::BTreeSet<String> = moved.iter().map(Target::parent).collect();
        for p in parents {
            self.fetch_dir(p, cx);
        }
        self.publish_storage(cx);
        self.recompute(cx);
    }

    /// Ctrl+C / Ctrl+X / Ctrl+V (⌘ trên macOS) khi focus ở cây / bảng.
    pub(crate) fn on_browser_key(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        if !k.modifiers.secondary(cx) || k.modifiers.shift || k.modifiers.alt {
            return;
        }
        match k.key.as_str() {
            "c" | "x" => {
                let cut = k.key == "x";
                let targets: Vec<Target> =
                    self.selected.iter().filter_map(|&i| self.entries.get(i)).map(Target::of).collect();
                if targets.is_empty() || targets.iter().any(|t| t.is_virtual) {
                    return;
                }
                if if cut { self.can_cut_out(cx) } else { self.can_copy_out(cx) } {
                    self.set_clip(targets, cut, cx);
                    cx.stop_propagation();
                }
            }
            "v" => {
                if let Location::Dir(p) = self.at.clone()
                    && self.can_paste(cx)
                {
                    self.paste_into(p, cx);
                    cx.stop_propagation();
                }
            }
            _ => {}
        }
    }

    /// Thanh dưới bảng: tiến độ / sẵn sàng / đã dời / lỗi của lần chép sang máy, nút Bỏ.
    pub(crate) fn render_export_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let view = self.export(cx)?.clone();
        let c = cx.theme().colors().clone();
        let lang = self.i18n.language();
        let n = view.paths.len().to_string();
        let (text, color, cancel) = match &view.state {
            ExportState::Preparing { total_files: 0, .. } => (self.i18n.t("browser.export_preparing").to_owned(), c.muted, true),
            ExportState::Preparing { done_bytes, total_bytes, .. } => {
                let pct = (done_bytes.saturating_mul(100)).checked_div(*total_bytes).unwrap_or(100).to_string();
                let text = self.i18n.t_args(
                    "browser.export_progress",
                    &[("done", &fmt::size(*done_bytes, lang)), ("total", &fmt::size(*total_bytes, lang)), ("pct", &pct)],
                );
                (text, c.muted, true)
            }
            ExportState::Ready => {
                let key = match (view.cut, cfg!(target_os = "macos")) {
                    (false, _) => "browser.export_ready_copy",
                    (true, false) => "browser.export_ready_cut",
                    (true, true) => "browser.export_ready_cut_mac",
                };
                (self.i18n.t_args(key, &[("n", &n)]), c.text, true)
            }
            ExportState::Moved { kept, .. } => {
                let mut text = self.i18n.t_args("browser.export_moved", &[("n", &n)]);
                if *kept > 0 {
                    text.push_str(" · ");
                    text.push_str(&self.i18n.t_args("browser.export_kept", &[("n", &kept.to_string())]));
                }
                (text, c.muted, false)
            }
            ExportState::Failed { error } => {
                let why = self.i18n.t(export_error_key(error)).to_owned();
                (self.i18n.t_args("browser.export_failed", &[("why", &why)]), c.danger, true)
            }
        };
        let bar = div()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .py_2()
            .border_t_1()
            .border_color(c.border)
            .child(div().flex_1().min_w_0().truncate().text_sm().text_color(color).child(SharedString::from(text)))
            .when(cancel, |d| {
                d.child(
                    Button::new("br-export-cancel", self.i18n.t("browser.export_cancel"))
                        .size(Size::Sm)
                        .outlined()
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_export(cx))),
                )
            });
        Some(bar.into_any_element())
    }
}

/// Lỗi của lần chép sang máy ⇒ chữ (khác `error_key`: không có chuyện tên sai).
fn export_error_key(e: &CommandError) -> &'static str {
    match e {
        CommandError::TooMany => "browser.err_too_many",
        CommandError::NotWritable => "browser.err_not_writable",
        CommandError::Forbidden => "browser.err_forbidden_fetch",
        CommandError::InvalidInput => "browser.err_failed",
        e => error_key(e),
    }
}

impl Target {
    fn of_path(path: &str) -> Target {
        let name = path.rsplit('/').next().unwrap_or(path).to_owned();
        // Thư mục hay tệp không quan trọng ở đây: `fs.remove` bỏ cả nhánh nếu là thư mục, không có thì thôi.
        Target { path: path.to_owned(), name, folder: true, is_virtual: false }
    }
}
