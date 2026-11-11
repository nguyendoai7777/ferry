//! Nguồn asset của `ferry-ui`: icon của materiant + vài icon riêng (thư mục, video) cho trình duyệt điện thoại.
//! Icon riêng là SVG một màu `currentColor`, lưới 24×24 nét 1,5 như icon materiant.

use std::borrow::Cow;

use materiant::gpui_kit::{AssetSource, Result, SharedString};
use materiant::icon::IconSource;

const FOLDER: &[u8] = include_bytes!("../assets/icons/ferry-folder.svg");
const VIDEO: &[u8] = include_bytes!("../assets/icons/ferry-video.svg");

pub const FOLDER_PATH: &str = "icons/ferry-folder.svg";
pub const VIDEO_PATH: &str = "icons/ferry-video.svg";

pub fn folder() -> IconSource {
    IconSource::Asset(FOLDER_PATH.into())
}

pub fn video() -> IconSource {
    IconSource::Asset(VIDEO_PATH.into())
}

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match path {
            FOLDER_PATH => Ok(Some(Cow::Borrowed(FOLDER))),
            VIDEO_PATH => Ok(Some(Cow::Borrowed(VIDEO))),
            _ => materiant::icon::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        materiant::icon::Assets.list(path)
    }
}
