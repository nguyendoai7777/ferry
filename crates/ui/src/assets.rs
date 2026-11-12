//! Nguồn asset của `ferry-ui`: icon của materiant + icon riêng của app (thư mục, video cho trình duyệt điện thoại; nút
//! cửa sổ, cột điều hướng bê từ Groovex). Icon riêng là SVG một màu `currentColor`, lưới 24×24 như icon materiant; `send`,
//! `phone`, `settings`, `wifi` theo Lucide (ISC, có trong trang giấy phép).

use std::borrow::Cow;

use materiant::gpui_kit::{AssetSource, Result, SharedString};
use materiant::icon::IconSource;

macro_rules! ferry_icons {
    ($($name:ident => $file:literal,)*) => {
        /// Icon riêng của app. Đường dẫn asset: `icons/ferry-<tệp>.svg`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum FerryIcon {
            $($name,)*
        }

        impl FerryIcon {
            #[cfg(test)]
            pub const ALL: &'static [FerryIcon] = &[$(FerryIcon::$name,)*];

            pub fn path(self) -> &'static str {
                match self {
                    $(FerryIcon::$name => concat!("icons/ferry-", $file, ".svg"),)*
                }
            }
        }

        fn embedded(path: &str) -> Option<&'static [u8]> {
            match path {
                $(concat!("icons/ferry-", $file, ".svg") => {
                    Some(include_bytes!(concat!("../assets/icons/ferry-", $file, ".svg")))
                })*
                _ => None,
            }
        }
    };
}

ferry_icons! {
    Folder => "folder",
    Video => "video",
    File => "file",
    Close => "close",
    Minus => "minus",
    ArrowsExpand => "arrows-expand",
    ArrowsCompress => "arrows-compress",
    Settings => "settings",
    Send => "send",
    Phone => "phone",
    Wifi => "wifi",
}

impl From<FerryIcon> for IconSource {
    fn from(icon: FerryIcon) -> IconSource {
        IconSource::Asset(icon.path().into())
    }
}

pub fn folder() -> IconSource {
    FerryIcon::Folder.into()
}

pub fn video() -> IconSource {
    FerryIcon::Video.into()
}

pub fn file() -> IconSource {
    FerryIcon::File.into()
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match embedded(path) {
            Some(bytes) => Ok(Some(Cow::Borrowed(bytes))),
            None => materiant::icon::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        materiant::icon::Assets.list(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mọi icon khai báo đều nhúng, đúng lưới 24×24; không còn tệp mồ côi trong `assets/icons`.
    #[test]
    fn every_icon_is_embedded_and_declared() {
        for icon in FerryIcon::ALL {
            let bytes = embedded(icon.path()).unwrap_or_else(|| panic!("{icon:?} not embedded"));
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(text.contains(r#"viewBox="0 0 24 24""#), "{icon:?}");
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/icons");
        for entry in std::fs::read_dir(dir).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            let path = format!("icons/{name}");
            assert!(FerryIcon::ALL.iter().any(|i| i.path() == path), "{name} is not declared in ferry_icons!");
        }
    }
}
