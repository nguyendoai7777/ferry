//! Trạng thái khay (PROTOCOL 4.1, 5): rảnh / đang gửi kèm % / cần chú ý, chữ tooltip. Hàm thuần
//! trên danh sách mục của store + trạng thái thiết bị — engine gọi sau mỗi ảnh chụp, gửi về luồng chính khi đổi.

use std::collections::HashMap;

use ferry_core::device::{DeviceId, DeviceState};
use ferry_core::i18n::{I18n, Language};
use ferry_core::ipc::ItemProgress;
use ferry_core::queue::state::PauseReason;
use ferry_core::queue::store::UploadItem;
use ferry_core::queue::{ItemId, UploadState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayMode {
    Idle,
    /// `percent` = byte đã tới điện thoại / tổng byte các mục đang chạy.
    Sending {
        percent: u8,
    },
    /// Người dùng phải làm gì đó (ghép lại, giải phóng bộ nhớ).
    Attention,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayStatus {
    pub mode: TrayMode,
    pub tooltip: String,
    /// Ngôn ngữ chữ của bản này: đổi trong cài đặt (gd1_12) ⇒ luồng chính dựng lại menu khay theo nó.
    pub language: Language,
}

/// Việc người dùng phải làm, theo thứ tự ưu tiên (ghép lại trước hết chỗ).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attention {
    NeedsRepair { phone: String },
    NoSpace { phone: String },
}

/// Tổng hợp hàng đợi cho khay + cờ `canPauseAll` / `canResumeAll` của ảnh chụp.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    /// Mục chưa kết thúc, không tạm dừng, điện thoại Online: `(id, cỡ, byte đã tới)`.
    pub running: Vec<(ItemId, u64, u64)>,
    /// Mục chờ vì điện thoại không Online ("Bật Sync WiFi trên điện thoại").
    pub waiting_offline: usize,
    pub paused: usize,
    pub attention: Option<Attention>,
}

impl Summary {
    pub fn can_pause_all(&self) -> bool {
        !self.running.is_empty() || self.waiting_offline > 0
    }

    pub fn can_resume_all(&self) -> bool {
        self.paused > 0
    }
}

/// `device(id)` ⇒ trạng thái + tên hiển thị; `None` ⇒ thiết bị không còn trong sổ (coi như Offline).
pub fn summarize<'a>(
    uploads: impl IntoIterator<Item = &'a UploadItem>,
    device: impl Fn(&DeviceId) -> Option<(DeviceState, String)>,
) -> Summary {
    let mut s = Summary::default();
    let mut repair: Option<String> = None;
    let mut no_space: Option<String> = None;
    let name = |id: &DeviceId| device(id).map_or_else(|| id.0.clone(), |(_, n)| n);
    for item in uploads {
        if item.state.is_terminal() {
            continue;
        }
        let state = device(&item.device_id).map(|(st, _)| st);
        match &item.state {
            UploadState::Paused { reason, .. } => {
                s.paused += 1;
                match reason {
                    PauseReason::NeedsPairing => {
                        repair.get_or_insert_with(|| name(&item.device_id));
                    }
                    PauseReason::NoSpace { .. } => {
                        no_space.get_or_insert_with(|| name(&item.device_id));
                    }
                    PauseReason::User => {}
                }
            }
            _ if state == Some(DeviceState::NeedsRepair) => {
                repair.get_or_insert_with(|| name(&item.device_id));
                s.waiting_offline += 1;
            }
            st if state == Some(DeviceState::Online) => s.running.push((item.id, item.size, st.bytes_done(item.size))),
            _ => s.waiting_offline += 1,
        }
    }
    s.attention = repair
        .map(|phone| Attention::NeedsRepair { phone })
        .or_else(|| no_space.map(|phone| Attention::NoSpace { phone }));
    s
}

/// % của mục đang chạy; tiến độ trong khúc (`progress`) mới hơn `offset` của store.
pub fn percent(running: &[(ItemId, u64, u64)], progress: &HashMap<ItemId, ItemProgress>) -> u8 {
    let (mut done, mut total) = (0u128, 0u128);
    for &(id, size, base) in running {
        let sent = progress.get(&id).map_or(base, |p| p.sent.max(base)).min(size);
        done += u128::from(sent);
        total += u128::from(size);
    }
    (done * 100).checked_div(total).map_or(0, |p| p.min(100) as u8)
}

pub fn status(s: &Summary, progress: &HashMap<ItemId, ItemProgress>, i18n: &I18n) -> TrayStatus {
    let count = |n: usize| n.to_string();
    let (mode, tooltip) = match &s.attention {
        Some(Attention::NeedsRepair { phone }) => {
            (TrayMode::Attention, i18n.t_args("tray.needs_repair", &[("phone", phone)]))
        }
        Some(Attention::NoSpace { phone }) => (TrayMode::Attention, i18n.t_args("tray.no_space", &[("phone", phone)])),
        None if !s.running.is_empty() => {
            let p = percent(&s.running, progress);
            let key = plural("tray.sending", s.running.len());
            (
                TrayMode::Sending { percent: p },
                i18n.t_args(&key, &[("count", &count(s.running.len())), ("percent", &p.to_string())]),
            )
        }
        None if s.waiting_offline > 0 => {
            let key = plural("tray.waiting_offline", s.waiting_offline);
            (TrayMode::Idle, i18n.t_args(&key, &[("count", &count(s.waiting_offline))]))
        }
        None if s.paused > 0 => {
            (TrayMode::Idle, i18n.t_args(&plural("tray.paused", s.paused), &[("count", &count(s.paused))]))
        }
        None => (TrayMode::Idle, i18n.t("app.name").to_owned()),
    };
    TrayStatus { mode, tooltip, language: i18n.language() }
}

/// Khoá i18n theo số: `<key>_one` khi `n == 1` (tiếng Anh "1 file"), không thì `<key>`. Hai catalog có đủ cả hai khoá.
pub fn plural(key: &str, n: impl TryInto<u64>) -> String {
    if n.try_into().ok() == Some(1) { format!("{key}_one") } else { key.to_owned() }
}

/// Cỡ cho người đọc: "1,2 GB" (vi) / "1.2 GB" (en), một chữ số thập phân từ MB trở lên.
pub fn format_size(bytes: u64, lang: Language) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let text = if unit < 2 { format!("{}", value.round() as u64) } else { format!("{value:.1}") };
    let text = match lang {
        Language::Vi => text.replace('.', ","),
        Language::En => text,
    };
    format!("{text} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::queue::state::Resume;

    fn item(id: ItemId, device: &str, size: u64, state: UploadState) -> UploadItem {
        UploadItem {
            id,
            device_id: DeviceId(device.into()),
            batch_id: None,
            path: format!("/v/{id}.mp4").into(),
            name: format!("{id}.mp4"),
            dir: String::new(),
            size,
            mtime: None,
            state,
            failures: 0,
            created: 0,
            updated: 0,
        }
    }

    fn devices(online: &[&str], repair: &[&str]) -> impl Fn(&DeviceId) -> Option<(DeviceState, String)> {
        let online: Vec<String> = online.iter().map(|s| s.to_string()).collect();
        let repair: Vec<String> = repair.iter().map(|s| s.to_string()).collect();
        move |id: &DeviceId| {
            let st = if online.contains(&id.0) {
                DeviceState::Online
            } else if repair.contains(&id.0) {
                DeviceState::NeedsRepair
            } else {
                DeviceState::Offline
            };
            Some((st, format!("Phone {}", id.0)))
        }
    }

    fn en() -> I18n {
        I18n::new(Language::En)
    }

    #[test]
    fn empty_queue_is_idle() {
        let s = summarize(&[], devices(&[], &[]));
        let st = status(&s, &HashMap::new(), &en());
        assert_eq!(st.mode, TrayMode::Idle);
        assert_eq!(st.tooltip, "Ferry");
    }

    #[test]
    fn sending_reports_percent_from_store_and_live_progress() {
        let items = [
            item(1, "a", 100, UploadState::Sending { uid: "u".into(), offset: 40 }),
            item(2, "a", 100, UploadState::Queued),
            item(3, "a", 50, UploadState::Done { remote_name: "x".into(), remote_dir: None }),
        ];
        let s = summarize(&items, devices(&["a"], &[]));
        assert_eq!(s.running.len(), 2);
        let mut progress = HashMap::new();
        assert_eq!(status(&s, &progress, &en()).mode, TrayMode::Sending { percent: 20 });
        progress.insert(1, ItemProgress { id: 1, sent: 90, size: 100, bytes_per_sec: 1 });
        let st = status(&s, &progress, &en());
        assert_eq!(st.mode, TrayMode::Sending { percent: 45 });
        assert_eq!(st.tooltip, "Sending 2 files — 45%");
    }

    #[test]
    fn offline_phone_waits_quietly_with_a_hint() {
        let items = [item(1, "a", 10, UploadState::Queued)];
        let st = status(&summarize(&items, devices(&[], &[])), &HashMap::new(), &en());
        assert_eq!(st.mode, TrayMode::Idle);
        assert_eq!(st.tooltip, "Turn on WiFi Sync on your phone to send 1 file");
        let vi = status(&summarize(&items, devices(&[], &[])), &HashMap::new(), &I18n::new(Language::Vi));
        assert_eq!(vi.tooltip, "Bật Đồng bộ WiFi trên điện thoại để gửi 1 tệp");
    }

    #[test]
    fn repair_and_no_space_need_attention_repair_first() {
        let full = UploadState::Paused { reason: PauseReason::NoSpace { short_by: None }, resume: Resume::Create };
        let items = [item(1, "a", 10, full), item(2, "b", 10, UploadState::Queued)];
        let st = status(&summarize(&items, devices(&["a"], &[])), &HashMap::new(), &en());
        assert_eq!(st.mode, TrayMode::Attention);
        assert!(st.tooltip.contains("Phone a"), "{}", st.tooltip);
        let st = status(&summarize(&items, devices(&["a"], &["b"])), &HashMap::new(), &en());
        assert_eq!(st.mode, TrayMode::Attention);
        assert!(st.tooltip.contains("Phone b") && st.tooltip.contains("again"), "{}", st.tooltip);
    }

    #[test]
    fn only_paused_items_offer_resume() {
        let paused = UploadState::Paused { reason: PauseReason::User, resume: Resume::Check };
        let items = [item(1, "a", 10, paused.clone()), item(2, "a", 10, paused)];
        let s = summarize(&items, devices(&["a"], &[]));
        assert!(!s.can_pause_all() && s.can_resume_all());
        let st = status(&s, &HashMap::new(), &en());
        assert_eq!(st.mode, TrayMode::Idle);
        assert_eq!(st.tooltip, "2 files paused");
    }

    #[test]
    fn sizes_read_like_the_protocol_examples() {
        assert_eq!(format_size(1_288_490_189, Language::Vi), "1,2 GB");
        assert_eq!(format_size(1_288_490_189, Language::En), "1.2 GB");
        assert_eq!(format_size(512, Language::En), "512 B");
        assert_eq!(format_size(2048, Language::En), "2 KB");
    }

    #[test]
    fn percent_never_exceeds_100_and_handles_empty_files() {
        assert_eq!(percent(&[(1, 0, 0)], &HashMap::new()), 0);
        let mut p = HashMap::new();
        p.insert(1, ItemProgress { id: 1, sent: 500, size: 10, bytes_per_sec: 0 });
        assert_eq!(percent(&[(1, 10, 0)], &p), 100);
    }
}
