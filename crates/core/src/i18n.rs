//! Catalog chữ hiển thị `vi` / `en` (`i18n/*.toml` ở gốc repo, nhúng vào exe lúc build) — cùng định dạng fastcast.
//!
//! TOML, bảng lồng nhau làm phần trước dấu chấm (`[tray] quit` ⇒ khoá `tray.quit`), biến `{name}`. TOML thay Fluent vì
//! chỉ cần chuỗi + biến (tiếng Việt không có số nhiều), không thêm phụ thuộc nặng vào tiến trình nền. Chữ viết hoa chữ
//! cái đầu (D-13), không toàn chữ hoa; "Gửi sang điện thoại" / "Tải về máy", không "upload / sync" (`CLAUDE.md`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

const VI: &str = include_str!("../../../i18n/vi.toml");
const EN: &str = include_str!("../../../i18n/en.toml");

/// Ngôn ngữ đã giải (không còn "theo hệ thống").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Vi,
    En,
}

impl Language {
    /// `vi…` ⇒ tiếng Việt, mọi locale khác ⇒ tiếng Anh.
    pub fn from_locale(locale: &str) -> Language {
        if locale.to_ascii_lowercase().starts_with("vi") { Language::Vi } else { Language::En }
    }

    pub fn code(self) -> &'static str {
        match self {
            Language::Vi => "vi",
            Language::En => "en",
        }
    }
}

/// Lựa chọn ngôn ngữ trong `config.json` (`"language": "system" | "vi" | "en"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguagePref {
    #[default]
    System,
    Vi,
    En,
}

impl LanguagePref {
    /// `System` ⇒ theo locale của OS (không đọc được ⇒ tiếng Anh).
    pub fn resolve(self) -> Language {
        self.resolve_with(os_locale().as_deref())
    }

    pub fn resolve_with(self, locale: Option<&str>) -> Language {
        match self {
            LanguagePref::Vi => Language::Vi,
            LanguagePref::En => Language::En,
            LanguagePref::System => locale.map_or(Language::En, Language::from_locale),
        }
    }
}

/// Locale của OS (`vi-VN`, `en-US`…).
pub fn os_locale() -> Option<String> {
    sys_locale::get_locale()
}

/// Chữ cho một ngôn ngữ; thiếu khoá thì lấy tiếng Anh, thiếu nữa thì trả chính khoá (dễ thấy khi test tay).
pub struct I18n {
    lang: Language,
    primary: HashMap<String, String>,
    fallback: HashMap<String, String>,
}

impl I18n {
    pub fn new(lang: Language) -> I18n {
        let primary = match lang {
            Language::Vi => parse(VI),
            Language::En => HashMap::new(),
        };
        I18n { lang, primary, fallback: parse(EN) }
    }

    pub fn language(&self) -> Language {
        self.lang
    }

    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.primary.get(key).or_else(|| self.fallback.get(key)).map_or(key, String::as_str)
    }

    /// `t` rồi thay `{name}` bằng giá trị.
    pub fn t_args(&self, key: &str, args: &[(&str, &str)]) -> String {
        let mut s = self.t(key).to_owned();
        for (name, value) in args {
            s = s.replace(&format!("{{{name}}}"), value);
        }
        s
    }
}

fn parse(src: &str) -> HashMap<String, String> {
    let table: toml::Table = src.parse().expect("embedded i18n catalog is valid TOML");
    let mut out = HashMap::new();
    flatten("", &table, &mut out);
    out
}

fn flatten(prefix: &str, table: &toml::Table, out: &mut HashMap<String, String>) {
    for (k, v) in table {
        let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        match v {
            toml::Value::String(s) => {
                out.insert(key, s.clone());
            }
            toml::Value::Table(t) => flatten(&key, t, out),
            other => panic!("i18n key `{key}` must be a string, got {}", other.type_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn placeholders(s: &str) -> BTreeSet<&str> {
        s.split('{').skip(1).filter_map(|p| p.split_once('}').map(|(name, _)| name)).collect()
    }

    /// C2 — hai catalog cùng bộ khoá, cùng biến.
    #[test]
    fn catalogs_have_same_keys_and_placeholders() {
        let (vi, en) = (parse(VI), parse(EN));
        let vi_keys: BTreeSet<_> = vi.keys().collect();
        let en_keys: BTreeSet<_> = en.keys().collect();
        assert_eq!(vi_keys, en_keys, "vi.toml and en.toml must have the same keys");
        for (k, v) in &vi {
            assert_eq!(placeholders(v), placeholders(&en[k]), "placeholders differ for `{k}`");
        }
    }

    /// Từ viết tắt được phép toàn chữ hoa.
    const ACRONYMS: [&str; 6] = ["PC", "PIN", "IP", "ID", "USB", "OK"];

    /// C2 — không chữ toàn hoa (D-13): mỗi từ từ hai chữ cái trở lên (trừ [`ACRONYMS`]) phải có chữ thường.
    #[test]
    fn no_all_caps_words() {
        for (lang, src) in [("vi", VI), ("en", EN)] {
            for (k, v) in parse(src) {
                for word in v.split(|c: char| !c.is_alphabetic()) {
                    let letters = word.chars().count();
                    assert!(
                        letters < 2 || ACRONYMS.contains(&word) || word.chars().any(char::is_lowercase),
                        "{lang} `{k}`: all-caps word `{word}` (D-13)"
                    );
                }
                // Mở đầu bằng tham số (`{count} files paused`, `{phone} hết chỗ`): số / tên điện thoại đứng đầu.
                let first = v.chars().find(|c| c.is_alphabetic() || *c == '{');
                assert!(
                    first.is_none_or(|c| c == '{' || c.is_uppercase()),
                    "{lang} `{k}`: must start with a capital letter (D-13)"
                );
            }
        }
    }

    #[test]
    fn lookup_fallback_and_args() {
        let vi = I18n::new(Language::Vi);
        assert_eq!(vi.t("tray.quit"), "Thoát");
        assert_eq!(I18n::new(Language::En).t("tray.quit"), "Quit");
        assert_eq!(vi.t("no.such.key"), "no.such.key");
        assert_eq!(vi.t_args("x", &[("a", "b")]), "x");
    }

    #[test]
    fn language_pref_resolves() {
        assert_eq!(LanguagePref::System.resolve_with(Some("vi-VN")), Language::Vi);
        assert_eq!(LanguagePref::System.resolve_with(Some("en-US")), Language::En);
        assert_eq!(LanguagePref::System.resolve_with(None), Language::En);
        assert_eq!(LanguagePref::Vi.resolve_with(Some("en-US")), Language::Vi);
        assert_eq!(serde_json::to_string(&LanguagePref::System).unwrap(), r#""system""#);
    }
}
