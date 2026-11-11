//! Định dạng số cho chữ hiển thị: cỡ tệp, tốc độ, ngày, khoá số ít / nhiều. Thuần, có test.

use ferry_core::i18n::{I18n, Language};

/// `1536` ⇒ `1,5 KB` (vi) / `1.5 KB` (en). Đơn vị thập phân 1024, một chữ số lẻ từ MB trở lên.
pub fn size(bytes: u64, lang: Language) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    let text = if unit < 2 { format!("{value:.0}") } else { format!("{value:.1}") };
    let text = if lang == Language::Vi { text.replace('.', ",") } else { text };
    format!("{text} {}", UNITS[unit])
}

/// Phần trăm 0–100, kẹp.
pub fn percent(done: u64, total: u64) -> u32 {
    if total == 0 { 0 } else { (done.min(total) as u128 * 100 / total as u128) as u32 }
}

/// Tỉ lệ 0–1 cho thanh tiến độ.
pub fn fraction(done: u64, total: u64) -> f32 {
    if total == 0 { 0. } else { (done.min(total) as f64 / total as f64) as f32 }
}

/// `<key>_one` khi `n == 1`, không thì `<key>` (cùng quy ước khay).
pub fn plural(i18n: &I18n, key: &str, n: u64, args: &[(&str, &str)]) -> String {
    let key = if n == 1 { format!("{key}_one") } else { key.to_owned() };
    i18n.t_args(&key, args)
}

/// Ngày sửa theo giờ máy: `10/11 21:03` (trong năm nay) hoặc `10/11/2025`. `modified` = ms Unix.
pub fn date(modified_ms: i64, now_ms: i64) -> String {
    date_in(modified_ms, now_ms, &chrono::Local)
}

pub fn date_in<Tz: chrono::TimeZone>(modified_ms: i64, now_ms: i64, tz: &Tz) -> String {
    use chrono::{Datelike, Timelike};
    let Some(at) = tz.timestamp_millis_opt(modified_ms).single() else { return String::new() };
    let this_year = tz.timestamp_millis_opt(now_ms).single().map(|n| n.year());
    if Some(at.year()) == this_year {
        format!("{:02}/{:02} {:02}:{:02}", at.day(), at.month(), at.hour(), at.minute())
    } else {
        format!("{:02}/{:02}/{}", at.day(), at.month(), at.year())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(0, Language::En), "0 B");
        assert_eq!(size(1536, Language::En), "2 KB");
        assert_eq!(size(1_288_490_189, Language::Vi), "1,2 GB");
        assert_eq!(size(1_288_490_189, Language::En), "1.2 GB");
    }

    #[test]
    fn percents() {
        assert_eq!(percent(1, 4), 25);
        assert_eq!(percent(9, 4), 100);
        assert_eq!(percent(1, 0), 0);
        assert!((fraction(1, 2) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn civil_dates() {
        // 2026-11-10 21:03:00 UTC
        let ms = 1_794_344_580_000;
        assert_eq!(date_in(ms, ms, &chrono::Utc), "10/11 21:03");
        assert_eq!(date_in(ms, ms + 400 * 86_400_000, &chrono::Utc), "10/11/2026");
    }
}
