//! Giao diện của cửa sổ: nền (sáng / tối / hệ thống) + bảng màu (preset của materiant), như Groovex
//! (`groove/crates/groove-app/src/app.rs::apply_theme`). Đọc `theme` / `themePreset` của `config.json` lúc mở, áp lại ngay khi
//! người dùng chọn ở Cài đặt → Giao diện (lưu thì gửi lệnh qua IPC: daemon là bên ghi `config.json`).

use ferry_core::config::ThemeChoice;
use ferry_core::i18n::I18n;
use materiant::gpui_kit::App;
use materiant::theme::picker::{ThemePreset, builtin_presets};
use materiant::theme::{MateriantTheme, ThemeMode, set_palette, set_theme};

fn mode(choice: ThemeChoice) -> ThemeMode {
    match choice {
        ThemeChoice::Light => ThemeMode::Light,
        ThemeChoice::Dark => ThemeMode::Dark,
        ThemeChoice::System => ThemeMode::System,
    }
}

/// Áp nền + bảng màu. `set_palette` chỉ thay màu (không sót màu preset trước); bản vá sau đặt nền + font nhúng (D-20).
pub fn apply(choice: ThemeChoice, preset: &str, cx: &mut App) {
    set_palette(&preset_theme(preset), cx);
    set_theme(
        &MateriantTheme {
            mode: Some(mode(choice)),
            font_family: Some(materiant::font::MONA_SANS.into()),
            ..Default::default()
        },
        cx,
    );
}

/// Chỉ số preset theo khoá đã lưu; khoá lạ / rỗng ⇒ 0 (mặc định).
pub fn preset_index(key: &str) -> usize {
    builtin_presets().iter().position(|p| p.key == key).unwrap_or(0)
}

fn preset_theme(key: &str) -> MateriantTheme {
    let mut presets = builtin_presets();
    let ix = preset_index(key);
    presets.swap_remove(ix).theme
}

/// Bốn preset có sẵn của materiant, nhãn đã dịch; thứ tự như gallery. Không có bảng màu tự chỉnh.
pub fn presets(i18n: &I18n) -> Vec<ThemePreset> {
    builtin_presets()
        .into_iter()
        .map(|preset| {
            let key = match preset.key.as_ref() {
                "default" => "theme_preset.default",
                "deeppink" => "theme_preset.deeppink",
                "rose-red" => "theme_preset.rose_red",
                "rebecca-purple" => "theme_preset.rebecca_purple",
                // Preset mới của materiant chưa có bản dịch: hiện khoá.
                _ => return preset,
            };
            preset.label(i18n.t(key).to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use ferry_core::i18n::Language;

    use super::*;

    #[test]
    fn four_builtin_presets_all_translated() {
        let keys: Vec<String> = builtin_presets().iter().map(|p| p.key.to_string()).collect();
        assert_eq!(keys, ["default", "deeppink", "rose-red", "rebecca-purple"]);
        for lang in [Language::Vi, Language::En] {
            let i18n = I18n::new(lang);
            for (preset, key) in presets(&i18n).iter().zip(&keys) {
                assert_ne!(preset.label.as_ref(), key.as_str(), "{key} chưa có chữ ({lang:?})");
            }
        }
    }

    #[test]
    fn unknown_preset_falls_back_to_default() {
        assert_eq!(preset_index("rose-red"), 2);
        assert_eq!(preset_index(""), 0);
        assert_eq!(preset_index("custom"), 0);
    }
}
