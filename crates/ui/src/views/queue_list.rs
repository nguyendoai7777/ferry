//! Hàng đợi (PROTOCOL 4.2): dòng gộp theo lô (mở ra được), dòng từng tệp, nhóm "Đã gửi" gập được. Dựng dòng là hàm
//! thuần ([`build_rows`], có test C1); vẽ bằng `VirtualList` (D-17) cao dòng cố định.

use std::collections::HashSet;
use std::rc::Rc;

use ferry_core::device::DeviceState;
use ferry_core::i18n::I18n;
use ferry_core::ipc::{Action, DeviceView, Snapshot};
use ferry_core::queue::state::{DownloadFailReason, DownloadState, FailReason, PauseReason, UploadState};
use ferry_core::queue::store::{DownloadItem, UploadItem};
use ferry_core::queue::{BatchId, ItemId};
use materiant::button::{Button, IconButton, Size};
use materiant::gpui_kit::prelude::*;
use materiant::gpui_kit::{App, ElementId, IntoElement, SharedString, Window, div, px};
use materiant::icon::IconName;
use materiant::list::VirtualList;
use materiant::progress::ProgressBar;
use materiant::style::Accent;
use materiant::theme::{ActiveTheme, Neutral};

use crate::fmt;

pub const ROW_HEIGHT: f32 = 60.;
/// Đệm ngang trong vùng cuộn, bằng nhau hai bên, chừa chỗ cho thumb (D-16).
const PAD_X: f32 = 16.;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Muted,
    Success,
    Warning,
    Danger,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    Batch { id: BatchId, open: bool },
    Item { id: ItemId, nested: bool },
    SentHeader { open: bool },
    More,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    pub title: String,
    pub meta: String,
    pub status: String,
    pub tone: Tone,
    pub progress: Option<f32>,
    pub speed: Option<String>,
    pub actions: Vec<Action>,
}

/// Lệnh người dùng bấm trên một dòng.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowCommand {
    Item(ItemId, Action),
    Batch(BatchId, Action),
    Toggle(RowKind),
}

fn is_sent(state: &UploadState) -> bool {
    matches!(state, UploadState::Done { .. } | UploadState::Skipped | UploadState::Cancelled)
}

fn sent_bytes(snapshot: &Snapshot, item: &UploadItem) -> u64 {
    let base = item.state.bytes_done(item.size);
    snapshot.progress.iter().find(|p| p.id == item.id).map_or(base, |p| p.sent.max(base)).min(item.size)
}

/// Chữ trạng thái cho người dùng (§2.6: nói việc cần làm, không nói mã HTTP).
pub fn item_status(i18n: &I18n, item: &UploadItem, device: DeviceState) -> (String, Tone) {
    let offline = device != DeviceState::Online;
    match &item.state {
        UploadState::Queued | UploadState::Waiting { .. } if offline => {
            (i18n.t("state.offline").to_owned(), Tone::Muted)
        }
        UploadState::Queued => (i18n.t("state.queued").to_owned(), Tone::Muted),
        UploadState::Checking => (i18n.t("state.checking").to_owned(), Tone::Normal),
        UploadState::Creating => (i18n.t("state.creating").to_owned(), Tone::Normal),
        UploadState::Sending { .. } => (i18n.t("state.sending").to_owned(), Tone::Normal),
        UploadState::Waiting { .. } => (i18n.t("state.waiting").to_owned(), Tone::Muted),
        UploadState::Paused { reason, .. } => match reason {
            PauseReason::User => (i18n.t("state.paused").to_owned(), Tone::Muted),
            PauseReason::NoSpace { short_by: Some(b) } => {
                (i18n.t_args("state.no_space_by", &[("size", &fmt::size(*b, i18n.language()))]), Tone::Warning)
            }
            PauseReason::NoSpace { short_by: None } => (i18n.t("state.no_space").to_owned(), Tone::Warning),
            PauseReason::NeedsPairing => (i18n.t("state.needs_pairing").to_owned(), Tone::Warning),
        },
        UploadState::Done { .. } => (i18n.t("state.done").to_owned(), Tone::Success),
        UploadState::Skipped => (i18n.t("state.skipped").to_owned(), Tone::Muted),
        UploadState::Cancelled => (i18n.t("state.cancelled").to_owned(), Tone::Muted),
        UploadState::Failed { reason } => {
            let key = match reason {
                FailReason::Unsupported => "state.failed_unsupported",
                FailReason::TooLarge => "state.failed_too_large",
                FailReason::BadRequest => "state.failed_bad_request",
                FailReason::SourceChanged => "state.failed_source_changed",
                FailReason::SourceMissing => "state.failed_source_missing",
            };
            (i18n.t(key).to_owned(), Tone::Danger)
        }
    }
}

fn item_actions(state: &UploadState) -> Vec<Action> {
    match state {
        UploadState::Paused { .. } => vec![Action::Resume, Action::Cancel],
        UploadState::Failed { .. } => vec![Action::Retry, Action::Cancel],
        s if s.is_terminal() => vec![],
        _ => vec![Action::Pause, Action::Cancel],
    }
}

fn item_row(i18n: &I18n, snapshot: &Snapshot, item: &UploadItem, device: DeviceState, nested: bool) -> Row {
    let (status, tone) = item_status(i18n, item, device);
    let sent = sent_bytes(snapshot, item);
    let show_bar =
        matches!(item.state, UploadState::Sending { .. } | UploadState::Waiting { .. } | UploadState::Paused { .. })
            && item.size > 0;
    let speed = match item.state {
        UploadState::Sending { .. } => snapshot
            .progress
            .iter()
            .find(|p| p.id == item.id)
            .filter(|p| p.bytes_per_sec > 0)
            .map(|p| i18n.t_args("queue.speed", &[("speed", &fmt::size(p.bytes_per_sec, i18n.language()))])),
        _ => None,
    };
    let title = if item.dir.is_empty() || nested { item.name.clone() } else { format!("{}/{}", item.dir, item.name) };
    Row {
        kind: RowKind::Item { id: item.id, nested },
        title,
        meta: fmt::size(item.size, i18n.language()),
        status: if show_bar { format!("{} · {} %", status, fmt::percent(sent, item.size)) } else { status },
        tone,
        progress: show_bar.then(|| fmt::fraction(sent, item.size)),
        speed,
        actions: item_actions(&item.state),
    }
}

/// Chữ trạng thái mục tải về (chiều điện thoại → PC, gd1_11).
pub fn download_status(i18n: &I18n, item: &DownloadItem, device: DeviceState) -> (String, Tone) {
    let offline = device != DeviceState::Online;
    match &item.state {
        DownloadState::Queued | DownloadState::Waiting { .. } if offline => {
            (i18n.t("state.offline_download").to_owned(), Tone::Muted)
        }
        DownloadState::Queued => (i18n.t("state.queued").to_owned(), Tone::Muted),
        DownloadState::Fetching { .. } => (i18n.t("state.downloading").to_owned(), Tone::Normal),
        DownloadState::Waiting { .. } => (i18n.t("state.waiting").to_owned(), Tone::Muted),
        DownloadState::Paused { .. } => (i18n.t("state.paused").to_owned(), Tone::Muted),
        DownloadState::Done => (i18n.t("state.downloaded").to_owned(), Tone::Success),
        DownloadState::Cancelled => (i18n.t("state.cancelled").to_owned(), Tone::Muted),
        DownloadState::Failed { reason } => {
            let key = match reason {
                DownloadFailReason::RemoteGone => "state.download_gone",
                DownloadFailReason::LocalWrite => "state.download_write",
            };
            (i18n.t(key).to_owned(), Tone::Danger)
        }
    }
}

fn download_row(i18n: &I18n, snapshot: &Snapshot, item: &DownloadItem, device: DeviceState) -> Row {
    let (status, tone) = download_status(i18n, item, device);
    let base = item.state.bytes_done(item.size);
    let prog = snapshot.progress.iter().find(|p| p.id == item.id);
    let got = prog.map_or(base, |p| p.sent.max(base)).min(item.size);
    let show_bar = matches!(
        item.state,
        DownloadState::Fetching { .. } | DownloadState::Waiting { .. } | DownloadState::Paused { .. }
    ) && item.size > 0;
    let speed = match item.state {
        DownloadState::Fetching { .. } => prog
            .filter(|p| p.bytes_per_sec > 0)
            .map(|p| i18n.t_args("queue.speed", &[("speed", &fmt::size(p.bytes_per_sec, i18n.language()))])),
        _ => None,
    };
    let actions = match &item.state {
        DownloadState::Paused { .. } => vec![Action::Resume, Action::Cancel],
        DownloadState::Failed { .. } => vec![Action::Retry, Action::Cancel],
        s if s.is_terminal() => vec![],
        _ => vec![Action::Pause, Action::Cancel],
    };
    Row {
        kind: RowKind::Item { id: item.id, nested: false },
        title: item.name.clone(),
        meta: format!("{} · {}", i18n.t("queue.download_tag"), fmt::size(item.size, i18n.language())),
        status: if show_bar { format!("{} · {} %", status, fmt::percent(got, item.size)) } else { status },
        tone,
        progress: show_bar.then(|| fmt::fraction(got, item.size)),
        speed,
        actions,
    }
}

/// Dựng các dòng của hàng đợi cho một điện thoại. `expanded`: lô đang mở; `sent_open`: nhóm "Đã gửi" đang mở.
pub fn build_rows(
    i18n: &I18n,
    snapshot: &Snapshot,
    device: &DeviceView,
    expanded: &HashSet<BatchId>,
    sent_open: bool,
) -> Vec<Row> {
    let items: Vec<&UploadItem> = snapshot.uploads.iter().filter(|u| u.device_id == device.id).collect();
    let lang = i18n.language();

    // Lô hiện thành dòng gộp: còn mục chưa "gửi xong" và (nhiều tệp hoặc thả cả thư mục).
    let live_batches: Vec<_> = snapshot
        .batches
        .iter()
        .filter(|b| b.info.device_id == device.id)
        .filter(|b| b.progress.files > 1 || b.info.label.is_some())
        .filter(|b| items.iter().any(|u| u.batch_id.as_ref() == Some(&b.info.id) && !is_sent(&u.state)))
        .collect();
    let in_live = |u: &UploadItem| u.batch_id.as_ref().is_some_and(|id| live_batches.iter().any(|b| &b.info.id == id));

    // (vị trí sắp xếp, dòng + dòng con)
    let mut live: Vec<(ItemId, Vec<Row>)> = Vec::new();
    for b in &live_batches {
        let members: Vec<&&UploadItem> = items.iter().filter(|u| u.batch_id.as_ref() == Some(&b.info.id)).collect();
        let first = members.iter().map(|u| u.id).min().unwrap_or(0);
        let p = &b.progress;
        let inflight: u64 =
            members.iter().map(|u| sent_bytes(snapshot, u).saturating_sub(u.state.bytes_done(u.size))).sum();
        let speed: u64 = members
            .iter()
            .filter(|u| matches!(u.state, UploadState::Sending { .. }))
            .filter_map(|u| snapshot.progress.iter().find(|q| q.id == u.id))
            .map(|q| q.bytes_per_sec)
            .sum();
        let remaining = p.files.saturating_sub(p.finished_files + p.failed_files + p.cancelled_files);
        let any_sending = members.iter().any(|u| u.state.is_active());
        let (status, tone) = if p.failed_files > 0 {
            (
                fmt::plural(i18n, "state.batch_failed", p.failed_files, &[("count", &p.failed_files.to_string())]),
                Tone::Danger,
            )
        } else if any_sending {
            (i18n.t("state.sending").to_owned(), Tone::Normal)
        } else if device.state != DeviceState::Online {
            (i18n.t("state.offline").to_owned(), Tone::Muted)
        } else if remaining > 0 && p.paused_files >= remaining {
            (i18n.t("state.batch_paused").to_owned(), Tone::Muted)
        } else {
            (i18n.t("state.batch_waiting").to_owned(), Tone::Muted)
        };
        let done = (p.done_bytes + inflight).min(p.total_bytes);
        let open = expanded.contains(&b.info.id);
        let mut actions = Vec::new();
        if remaining > 0 {
            actions.push(if p.paused_files >= remaining { Action::Resume } else { Action::Pause });
        }
        if p.failed_files > 0 {
            actions.push(Action::Retry);
        }
        if remaining > 0 || p.failed_files > 0 {
            actions.push(Action::Cancel);
        }
        let title = b
            .info
            .label
            .clone()
            .unwrap_or_else(|| fmt::plural(i18n, "queue.files", p.files, &[("count", &p.files.to_string())]));
        let mut rows = vec![Row {
            kind: RowKind::Batch { id: b.info.id.clone(), open },
            title,
            meta: format!(
                "{} · {}",
                i18n.t_args(
                    "queue.batch_progress",
                    &[("done", &p.finished_files.to_string()), ("total", &p.files.to_string())]
                ),
                fmt::size(p.total_bytes, lang)
            ),
            status: format!("{} · {} %", status, fmt::percent(done, p.total_bytes)),
            tone,
            progress: (p.total_bytes > 0 && (any_sending || done > 0)).then(|| fmt::fraction(done, p.total_bytes)),
            speed: (speed > 0).then(|| i18n.t_args("queue.speed", &[("speed", &fmt::size(speed, lang))])),
            actions,
        }];
        if open {
            rows.extend(members.iter().map(|u| item_row(i18n, snapshot, u, device.state, true)));
        }
        live.push((first, rows));
    }
    for u in items.iter().filter(|u| !is_sent(&u.state) && !in_live(u)) {
        live.push((u.id, vec![item_row(i18n, snapshot, u, device.state, false)]));
    }
    // Mục tải về còn sống chung hàng (id không trùng với upload).
    for d in snapshot.downloads.iter().filter(|d| {
        d.device_id == device.id && (!d.state.is_terminal() || matches!(d.state, DownloadState::Failed { .. }))
    }) {
        live.push((d.id, vec![download_row(i18n, snapshot, d, device.state)]));
    }
    live.sort_by_key(|(id, _)| *id);

    let mut out: Vec<Row> = live.into_iter().flat_map(|(_, rows)| rows).collect();
    if snapshot.queued_omitted > 0 {
        out.push(more_row(i18n, "queue.and_more", snapshot.queued_omitted));
    }

    // Nhóm "Đã gửi": mục xong / bỏ qua / huỷ không thuộc dòng gộp còn sống, mới nhất trước.
    let mut sent: Vec<&&UploadItem> = items.iter().filter(|u| is_sent(&u.state) && !in_live(u)).collect();
    sent.sort_by_key(|u| std::cmp::Reverse(u.id));
    let count = sent.len() as u64 + snapshot.finished_omitted;
    if count > 0 {
        out.push(Row {
            kind: RowKind::SentHeader { open: sent_open },
            title: i18n.t_args("queue.sent_group", &[("count", &count.to_string())]),
            meta: String::new(),
            status: String::new(),
            tone: Tone::Muted,
            progress: None,
            speed: None,
            actions: vec![],
        });
        if sent_open {
            out.extend(sent.iter().map(|u| item_row(i18n, snapshot, u, device.state, true)));
            if snapshot.finished_omitted > 0 {
                out.push(more_row(i18n, "queue.and_more", snapshot.finished_omitted));
            }
        }
    }
    out
}

fn more_row(i18n: &I18n, key: &str, n: u64) -> Row {
    Row {
        kind: RowKind::More,
        title: fmt::plural(i18n, key, n, &[("count", &n.to_string())]),
        meta: String::new(),
        status: String::new(),
        tone: Tone::Muted,
        progress: None,
        speed: None,
        actions: vec![],
    }
}

fn action_label(i18n: &I18n, action: Action) -> String {
    i18n.t(match action {
        Action::Pause => "queue.pause",
        Action::Resume => "queue.resume",
        Action::Cancel => "queue.cancel",
        Action::Retry => "queue.retry",
    })
    .to_owned()
}

fn command_for(kind: &RowKind, action: Action) -> Option<RowCommand> {
    match kind {
        RowKind::Batch { id, .. } => Some(RowCommand::Batch(id.clone(), action)),
        RowKind::Item { id, .. } => Some(RowCommand::Item(*id, action)),
        _ => None,
    }
}

/// Danh sách ảo của hàng đợi. `on_command` nhận nút bấm / mở-gập; chỉ số dòng chọn do app không giữ (không có chọn).
pub fn queue_list(
    rows: Rc<Vec<Row>>,
    i18n: Rc<I18n>,
    on_command: Rc<dyn Fn(RowCommand, &mut Window, &mut App)>,
) -> impl IntoElement {
    let count = rows.len();
    let activate_rows = rows.clone();
    let activate = on_command.clone();
    VirtualList::new("queue", count, px(ROW_HEIGHT), move |ix, _, cx| {
        let row = &rows[ix];
        render_row(row, ix, &i18n, &on_command, cx)
    })
    .padding_x(px(PAD_X))
    .on_activate(move |ix: &usize, window, cx| {
        if let Some(row) = activate_rows.get(*ix)
            && matches!(row.kind, RowKind::Batch { .. } | RowKind::SentHeader { .. })
        {
            activate(RowCommand::Toggle(row.kind.clone()), window, cx);
        }
    })
    .size_full()
}

fn render_row(
    row: &Row,
    ix: usize,
    i18n: &Rc<I18n>,
    on_command: &Rc<dyn Fn(RowCommand, &mut Window, &mut App)>,
    cx: &mut App,
) -> materiant::gpui_kit::AnyElement {
    let c = cx.theme().colors().clone();
    let tone = match row.tone {
        Tone::Normal => c.text,
        Tone::Muted => c.muted,
        Tone::Success => c.success,
        Tone::Warning => c.warning,
        Tone::Danger => c.danger,
    };
    let nested = matches!(row.kind, RowKind::Item { nested: true, .. });
    let header = matches!(row.kind, RowKind::Batch { .. } | RowKind::SentHeader { .. });
    let chevron = match &row.kind {
        RowKind::Batch { open, .. } | RowKind::SentHeader { open } => {
            let open = *open;
            let kind = row.kind.clone();
            let cmd = on_command.clone();
            Some(
                IconButton::new(
                    ElementId::Name(format!("q-toggle-{ix}").into()),
                    if open { IconName::ChevronDown } else { IconName::ChevronRight },
                )
                .size(Size::Sm)
                .accessibility_label(row.title.clone())
                .on_click(move |_, window, cx| cmd(RowCommand::Toggle(kind.clone()), window, cx)),
            )
        }
        _ => None,
    };

    let mut buttons = div().flex().flex_none().items_center().gap_1();
    for action in &row.actions {
        let Some(command) = command_for(&row.kind, *action) else { continue };
        let cmd = on_command.clone();
        let mut b = Button::new(ElementId::Name(format!("q-{ix}-{action:?}").into()), action_label(i18n, *action))
            .size(Size::Sm)
            .text();
        if *action == Action::Cancel {
            b = b.base_hover(Accent::Danger);
        }
        buttons = buttons.child(b.on_click(move |_, window, cx| cmd(command.clone(), window, cx)));
    }

    let title_weight = if header { cx.theme().title_weight() } else { cx.theme().label_weight() };
    let body = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .justify_center()
        .gap_1()
        .child(
            div()
                .flex()
                .items_baseline()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(title_weight)
                        .text_color(c.text)
                        .child(SharedString::from(row.title.clone())),
                )
                .when(!row.meta.is_empty(), |d| {
                    d.child(div().flex_none().text_xs().text_color(c.muted).child(SharedString::from(row.meta.clone())))
                }),
        )
        .when(!row.status.is_empty() || row.progress.is_some(), |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(260.))
                            .truncate()
                            .text_xs()
                            .text_color(tone)
                            .child(SharedString::from(row.status.clone())),
                    )
                    .when_some(row.progress, |d, value| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w(px(60.))
                                .child(ProgressBar::new(ElementId::Name(format!("q-bar-{ix}").into())).value(value)),
                        )
                    })
                    .when_some(row.speed.clone(), |d, speed| {
                        d.child(div().flex_none().text_xs().text_color(c.muted).child(SharedString::from(speed)))
                    }),
            )
        });

    div()
        .size_full()
        .flex()
        .items_center()
        .gap_2()
        .when(nested, |d| d.pl_6())
        .when(matches!(row.kind, RowKind::Item { nested: false, .. } | RowKind::More), |d| d.pl(px(32.)))
        .children(chevron)
        .child(body)
        .child(buttons)
        .border_b_1()
        .border_color(c.layer(Neutral::N100))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use ferry_core::device::DeviceId;
    use ferry_core::ipc::{BatchView, ItemProgress};
    use ferry_core::queue::store::{BatchInfo, BatchProgress};

    use ferry_core::i18n::Language;

    use super::*;

    fn device(state: DeviceState) -> DeviceView {
        DeviceView {
            id: DeviceId("p".into()),
            name: "Pixel 8".into(),
            state,
            visible: true,
            newer_api: false,
            features: vec!["dirs".into()],
            address: None,
        }
    }

    fn item(id: i64, name: &str, size: u64, state: UploadState, batch: Option<&str>) -> UploadItem {
        UploadItem {
            id,
            device_id: DeviceId("p".into()),
            batch_id: batch.map(|b| BatchId(b.into())),
            path: format!("/x/{name}").into(),
            name: name.into(),
            dir: String::new(),
            size,
            mtime: None,
            state,
            failures: 0,
            created: 0,
            updated: 0,
        }
    }

    fn snapshot() -> Snapshot {
        let sending = UploadState::Sending { uid: "u".into(), offset: 100 };
        let done = UploadState::Done { remote_name: "d.mp4".into(), remote_dir: Some(String::new()) };
        Snapshot {
            devices: vec![device(DeviceState::Online)],
            batches: vec![BatchView {
                info: BatchInfo {
                    id: BatchId("b1".into()),
                    device_id: DeviceId("p".into()),
                    dest_dir: String::new(),
                    label: Some("Chuyến đi".into()),
                    skipped_files: 1,
                    created: 0,
                },
                progress: BatchProgress {
                    files: 2,
                    finished_files: 0,
                    total_bytes: 2000,
                    done_bytes: 100,
                    ..Default::default()
                },
            }],
            uploads: vec![
                item(1, "a.mp4", 1000, sending, Some("b1")),
                item(2, "b.mp4", 1000, UploadState::Queued, Some("b1")),
                item(3, "solo.mp4", 500, UploadState::Failed { reason: FailReason::SourceMissing }, None),
                item(4, "old.mp4", 500, done, None),
            ],
            progress: vec![ItemProgress { id: 1, sent: 400, size: 1000, bytes_per_sec: 2048 }],
            ..Default::default()
        }
    }

    /// C1 — snapshot giả ⇒ đúng số dòng, đúng chữ trạng thái vi / en.
    #[test]
    fn rows_and_texts_vi_en() {
        let snap = snapshot();
        let dev = &snap.devices[0];
        let vi = I18n::new(Language::Vi);
        let rows = build_rows(&vi, &snap, dev, &HashSet::new(), false);
        // lô, solo lỗi, nhóm "Đã gửi" (đóng)
        assert_eq!(rows.len(), 3);
        assert!(matches!(rows[0].kind, RowKind::Batch { open: false, .. }));
        assert_eq!(rows[0].title, "Chuyến đi");
        assert!(rows[0].status.starts_with("Đang gửi"), "{}", rows[0].status);
        assert!(rows[0].speed.is_some());
        assert_eq!(rows[1].status, "Không tìm thấy tệp trên máy tính");
        assert_eq!(rows[1].tone, Tone::Danger);
        assert_eq!(rows[1].actions, vec![Action::Retry, Action::Cancel]);
        assert_eq!(rows[2].title, "Đã gửi (1)");

        let open: HashSet<_> = [BatchId("b1".into())].into();
        let rows = build_rows(&vi, &snap, dev, &open, true);
        // lô + 2 con + solo + header + 1 đã gửi
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[1].status, "Đang gửi · 40 %");
        assert_eq!(rows[2].status, "Đang chờ");
        assert_eq!(rows.last().unwrap().status, "Đã gửi");

        let en = I18n::new(Language::En);
        let rows = build_rows(&en, &snap, dev, &open, true);
        assert_eq!(rows[1].status, "Sending · 40 %");
        assert_eq!(rows[2].status, "Waiting");
        assert_eq!(rows[5].status, "Sent");
        assert_eq!(rows[2 + 1].status, "The file is no longer on this computer");
    }

    #[test]
    fn offline_phone_says_what_to_do() {
        let mut snap = snapshot();
        snap.devices[0].state = DeviceState::Offline;
        snap.uploads[0].state = UploadState::Queued;
        let vi = I18n::new(Language::Vi);
        let rows = build_rows(&vi, &snap, &snap.devices[0].clone(), &HashSet::new(), false);
        assert_eq!(rows[0].status.split(" · ").next().unwrap(), "Bật Sync WiFi trên điện thoại để gửi tiếp");
    }

    #[test]
    fn other_phones_items_are_hidden_and_single_file_batch_is_flat() {
        let mut snap = snapshot();
        snap.uploads[3].device_id = DeviceId("other".into());
        snap.batches[0].info.label = None;
        snap.batches[0].progress.files = 1;
        let vi = I18n::new(Language::Vi);
        let rows = build_rows(&vi, &snap, &snap.devices[0].clone(), &HashSet::new(), true);
        assert!(rows.iter().all(|r| !matches!(r.kind, RowKind::Batch { .. } | RowKind::SentHeader { .. })));
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn pause_resume_actions_follow_state() {
        let paused = UploadState::Paused { reason: PauseReason::User, resume: ferry_core::queue::state::Resume::Check };
        assert_eq!(item_actions(&paused), vec![Action::Resume, Action::Cancel]);
        assert_eq!(item_actions(&UploadState::Queued), vec![Action::Pause, Action::Cancel]);
        assert!(item_actions(&UploadState::Skipped).is_empty());
    }

    #[test]
    fn downloads_show_as_rows_with_progress_and_skip_finished() {
        let dl = |id: i64, state: DownloadState| DownloadItem {
            id,
            device_id: DeviceId("p".into()),
            remote_id: format!("v{id}"),
            name: format!("v{id}.mp4"),
            remote_dir: None,
            size: 100,
            dest: format!("/d/v{id}.mp4").into(),
            state,
            failures: 0,
            created: 0,
            updated: 0,
        };
        let mut snap = Snapshot::default();
        snap.downloads.push(dl(1, DownloadState::Fetching { offset: 10, etag: None }));
        snap.downloads.push(dl(2, DownloadState::Done));
        snap.downloads.push(dl(3, DownloadState::Failed { reason: DownloadFailReason::RemoteGone }));
        snap.progress.push(ItemProgress { id: 1, sent: 40, size: 100, bytes_per_sec: 5 });
        let vi = I18n::new(Language::Vi);
        let rows = build_rows(&vi, &snap, &device(DeviceState::Online), &HashSet::new(), false);
        assert_eq!(rows.len(), 2, "mục đã tải xong không nằm ở hàng sống");
        assert_eq!(rows[0].kind, RowKind::Item { id: 1, nested: false });
        assert!(rows[0].status.contains("40 %"));
        assert_eq!(rows[0].actions, vec![Action::Pause, Action::Cancel]);
        assert_eq!(rows[1].actions, vec![Action::Retry, Action::Cancel]);
        assert_eq!(rows[1].tone, Tone::Danger);
    }
}
