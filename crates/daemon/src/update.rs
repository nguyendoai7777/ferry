//! ferry-gd2_01: tự cập nhật ở daemon (cửa sổ không gọi mạng — `CLAUDE.md`). Phần chung (manifest ký, sổ, tải, cài) ở crate
//! `updater` của materiant; ở đây chỉ lịch soát, trạng thái trên luồng chính và đường nối khay / ảnh chụp.
//!
//! - Luồng `update-timer` đọc sổ `<data_dir>/update/state.json` (bản đang chờ cài), ngủ tới hạn soát đầu (10 s sau khi bật,
//!   hoặc tới hạn 6 giờ tính từ lần soát cuối) rồi mỗi 6 giờ báo [`Msg::CheckDue`]. Không vòng bận: luồng chỉ ngủ.
//! - Mỗi lần soát / tải / cài chạy trên một luồng riêng (hàm chặn của `updater`), báo kết quả về luồng chính qua `Waker`.
//! - Có bản mới ⇒ menu khay thêm "Cập nhật lên v<x>", ảnh chụp mang [`UpdateView`] cho cửa sổ (nút Cập nhật, Cmd / Ctrl+D).
//!   Bấm ⇒ tải (soát cỡ + sha256) ⇒ giữ release note cho hộp "Có gì mới" ⇒ thay bản cài ⇒ daemon thoát (cửa sổ thoát theo);
//!   script của `updater` chờ daemon thoát rồi bật bản mới với `--updated` (= chạy tay ⇒ mở cửa sổ). Bản major / bản không
//!   tự cài được (dev, `.deb`, `.zip` chạy thẳng) ⇒ mở trang tải.
//!
//! Chạy thử: bản dev (`debug_assertions`) tin khoá dev + đọc `FERRY_CLOUD_URL` (server giả
//! `materiant/updater/tools/fake-server.mjs`); `FERRY_UPDATE_AUTOINSTALL=1` cài ngay khi thấy bản mới (E2E không bấm tay).

use std::path::PathBuf;
use std::sync::Arc;

use ferry_core::ipc::UpdateView;
use ferry_core::{VERSION, paths};
use updater::install::Layout;
use updater::{AppSpec, Manifest, Outcome, check, install, notes};

use crate::daemon::Event;
use crate::platform::Waker;

/// Bản build tin khoá dev + nhận `http://` + đọc biến môi trường chạy thử: bản debug, hoặc bản release build với
/// `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true` (gói `.app` chạy thử tự cập nhật — như groove).
pub const DEV_BUILD: bool = cfg!(debug_assertions);

/// Khoá công khai cập nhật prod của ferry (`cloud/secrets/ferry/update-prod-key.json`, sinh 13/11/2026).
const PROD_KEY: [u8; 32] = [
    0x65, 0x6e, 0x51, 0x7d, 0xf1, 0x43, 0x1b, 0x1e, 0xe6, 0x7f, 0x7b, 0x71, 0x35, 0x68, 0x66, 0x1f, //
    0x64, 0xbe, 0x63, 0x86, 0xc2, 0xd1, 0x64, 0xd3, 0x5e, 0x9f, 0x00, 0xd0, 0xb5, 0x57, 0x5f, 0xab,
];

pub const SPEC: AppSpec = AppSpec {
    id: "ferry",
    version: VERSION,
    prod_key: PROD_KEY,
    dev_build: DEV_BUILD,
    plan: "free",
    mac_bundle: "Ferry.app",
    exe: "ferry",
    linux_dir: "ferry",
    download_page: "https://ferry.sandtechs.com",
};

/// Server cập nhật (D-27: mỗi app một nhánh tên miền).
const PROD_SERVER: &str = "https://cloud.ferry.sandtechs.com";
const SERVER_VAR: &str = "FERRY_CLOUD_URL";
const AUTOINSTALL_VAR: &str = "FERRY_UPDATE_AUTOINSTALL";

fn server_url() -> String {
    if DEV_BUILD && let Ok(url) = std::env::var(SERVER_VAR) {
        return url;
    }
    PROD_SERVER.to_owned()
}

fn dev_flag(var: &str) -> bool {
    DEV_BUILD && std::env::var_os(var).is_some_and(|v| v == "1")
}

/// Kết quả các luồng nền, xử lý tuần tự ở [`Updater::handle`] trên luồng chính.
#[derive(Debug)]
pub enum Msg {
    /// Sổ đã đọc lúc bật: bản mới server báo lần trước (còn hợp lệ).
    Started(Option<Box<Manifest>>),
    CheckDue,
    Checked {
        manual: bool,
        result: Result<Outcome, String>,
    },
    Progress(u8),
    Installed,
    InstallFailed(String),
}

/// Việc luồng chính phải làm sau một bước.
#[derive(Debug, Default, PartialEq)]
pub struct Effect {
    /// Trạng thái đổi ⇒ đẩy cho engine (ảnh chụp) + khay.
    pub changed: bool,
    /// Đã cài xong ⇒ daemon thoát để bản mới bật.
    pub quit: bool,
}

pub struct Updater {
    view: UpdateView,
    manifest: Option<Arc<Manifest>>,
    dir: Option<PathBuf>,
    waker: Option<Waker>,
}

impl Updater {
    /// Bật lịch soát (nền không phát hành ⇒ không làm gì). `waker: None` (test) ⇒ không luồng nào chạy.
    pub fn start(waker: Option<Waker>) -> Updater {
        let dir = paths::data_dir().ok().map(|d| d.join("update"));
        let me = Updater { view: UpdateView::Idle, manifest: None, dir: dir.clone(), waker };
        if let (Some(waker), Some(dir), Some(_)) = (me.waker.clone(), dir, SPEC.target()) {
            let spawned = std::thread::Builder::new().name("update-timer".into()).spawn(move || {
                let start = check::startup(&SPEC, &dir);
                waker.send(Event::Update(Msg::Started(start.pending.map(Box::new))));
                std::thread::sleep(start.first_check);
                loop {
                    waker.send(Event::Update(Msg::CheckDue));
                    std::thread::sleep(check::CHECK_EVERY);
                }
            });
            if let Err(e) = spawned {
                log::warn!("update: cannot start the timer: {e}");
            }
        }
        me
    }

    pub fn view(&self) -> &UpdateView {
        &self.view
    }

    /// Bản mới có nút / mục menu (`None` ⇒ không có).
    pub fn version(&self) -> Option<&str> {
        self.view.version()
    }

    fn busy(&self) -> bool {
        matches!(self.view, UpdateView::Checking | UpdateView::Downloading { .. } | UpdateView::Installing { .. })
    }

    fn set(&mut self, view: UpdateView) -> Effect {
        let changed = self.view != view;
        self.view = view;
        Effect { changed, quit: false }
    }

    fn available(&mut self, manifest: Manifest) -> Effect {
        let opens_page = !manifest.kind.installs_in_app() || Layout::detect(&SPEC, None).is_none();
        let version = manifest.version.clone();
        self.manifest = Some(Arc::new(manifest));
        let effect = self.set(UpdateView::Available { version, opens_page });
        if dev_flag(AUTOINSTALL_VAR) {
            log::info!("update: {AUTOINSTALL_VAR}=1, installing now");
            let mut more = self.install();
            more.changed |= effect.changed;
            return more;
        }
        effect
    }

    pub fn handle(&mut self, msg: Msg) -> Effect {
        match msg {
            Msg::Started(Some(m)) if self.manifest.is_none() => {
                log::info!("update: {} pending from the last check", m.version);
                self.available(*m)
            }
            Msg::Started(_) => Effect::default(),
            Msg::CheckDue => self.check(false),
            Msg::Checked { manual, result } => match result {
                Ok(Outcome::UpToDate) => {
                    log::info!("update: up to date ({VERSION})");
                    self.manifest = None;
                    self.set(UpdateView::UpToDate)
                }
                Ok(Outcome::Available(m)) => {
                    log::info!("update: {} available ({:?})", m.version, m.kind);
                    self.available(*m)
                }
                Err(e) => {
                    log::warn!("update: check failed: {e}");
                    // Tự soát lỗi (mạng…) ⇒ giữ trạng thái cũ, im lặng; hỏi tay ⇒ báo.
                    match (&self.manifest, manual) {
                        (Some(m), _) => {
                            let opens_page = !m.kind.installs_in_app() || Layout::detect(&SPEC, None).is_none();
                            self.set(UpdateView::Available { version: m.version.clone(), opens_page })
                        }
                        (None, true) => self.set(UpdateView::Failed { version: None }),
                        (None, false) => self.set(UpdateView::Idle),
                    }
                }
            },
            Msg::Progress(percent) => match &self.view {
                UpdateView::Downloading { version, .. } => {
                    let version = version.clone();
                    self.set(UpdateView::Downloading { version, percent })
                }
                _ => Effect::default(),
            },
            Msg::Installed => {
                let version = self.manifest.as_ref().map(|m| m.version.clone()).unwrap_or_default();
                log::info!("update: {version} installed, quitting so the new version starts");
                let mut effect = self.set(UpdateView::Installing { version });
                effect.quit = true;
                effect
            }
            Msg::InstallFailed(e) => {
                log::error!("update: install failed: {e}");
                let version = self.manifest.as_ref().map(|m| m.version.clone());
                self.set(UpdateView::Failed { version })
            }
        }
    }

    /// Hỏi server ngay. `manual`: menu macOS / nút Kiểm tra — lỗi thì hiện.
    pub fn check(&mut self, manual: bool) -> Effect {
        if self.busy() || SPEC.target().is_none() {
            return Effect::default();
        }
        let Some(waker) = self.waker.clone() else { return Effect::default() };
        let dir = self.dir.clone();
        let spawned = std::thread::Builder::new().name("update-check".into()).spawn(move || {
            let result = check::check(&SPEC, &server_url(), dir.as_deref()).map_err(|e| e.to_string());
            waker.send(Event::Update(Msg::Checked { manual, result }));
        });
        if let Err(e) = spawned {
            log::warn!("update: cannot spawn check: {e}");
            return Effect::default();
        }
        // Tự soát khi đã có bản mới: giữ nút, không nháy "Đang kiểm tra".
        if manual || self.manifest.is_none() { self.set(UpdateView::Checking) } else { Effect::default() }
    }

    /// Mục khay "Cập nhật lên v<x>" / nút Cập nhật / Cmd+D: tải + cài, hoặc mở trang tải.
    pub fn install(&mut self) -> Effect {
        if self.busy() {
            return Effect::default();
        }
        let Some(manifest) = self.manifest.clone() else { return Effect::default() };
        let layout = Layout::detect(&SPEC, None);
        let (Some(layout), Some(dir), true) = (layout.clone(), self.dir.clone(), manifest.kind.installs_in_app())
        else {
            log::info!(
                "update: {} — open download page (kind {:?}, self-install: {})",
                manifest.version,
                manifest.kind,
                layout.is_some()
            );
            if self.waker.is_some() {
                open_url(SPEC.download_page);
            }
            return Effect::default();
        };
        let Some(waker) = self.waker.clone() else { return Effect::default() };
        let version = manifest.version.clone();
        let spawned = std::thread::Builder::new().name("update-install".into()).spawn(move || {
            let mut last = 0u8;
            let progress = |got: u64| {
                let percent = (got.saturating_mul(100) / manifest.size.max(1)).min(100) as u8;
                if percent != last {
                    last = percent;
                    waker.send(Event::Update(Msg::Progress(percent)));
                }
            };
            let result = install::download(&manifest, &dir, progress).and_then(|file| {
                notes::save_pending(&dir, &manifest.notes);
                install::apply(&SPEC, &layout, &file, &dir, std::process::id())
            });
            waker.send(Event::Update(match result {
                Ok(()) => Msg::Installed,
                Err(e) => Msg::InstallFailed(e),
            }));
        });
        if let Err(e) = spawned {
            log::warn!("update: cannot spawn install: {e}");
            return Effect::default();
        }
        log::info!("update: downloading {version}");
        self.set(UpdateView::Downloading { version, percent: 0 })
    }
}

/// Mở trang bằng trình duyệt mặc định của OS.
fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "linux")]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(windows)]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    match cmd.arg(url).spawn() {
        // Gặt tiến trình con ở luồng riêng, không để zombie.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => log::warn!("update: cannot open {url}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use updater::{Kind, testing};

    use super::*;

    fn manifest(version: &str, kind: Kind) -> Manifest {
        let target = SPEC.target().unwrap_or(updater::Target { os: "macos", arch: "arm64", plan: "free" });
        testing::manifest(&SPEC, target, VERSION, version, kind)
    }

    fn idle() -> Updater {
        Updater { view: UpdateView::Idle, manifest: None, dir: None, waker: None }
    }

    #[test]
    fn spec_matches_the_package() {
        assert_eq!(SPEC.current().to_string(), VERSION);
        assert_eq!(SPEC.keys(), vec![updater::DEV_KEY, PROD_KEY]);
    }

    /// Bản mới ⇒ có phiên bản cho khay / nút; bản dev (chạy từ `target/`) không tự cài ⇒ mở trang tải.
    #[test]
    fn available_update_shows_a_version_and_dev_builds_open_the_page() {
        let mut u = idle();
        let effect = u.handle(Msg::Checked {
            manual: false,
            result: Ok(Outcome::Available(Box::new(manifest("9.0.1", Kind::Minor)))),
        });
        assert!(effect.changed && !effect.quit);
        assert_eq!(u.version(), Some("9.0.1"));
        assert_eq!(u.view(), &UpdateView::Available { version: "9.0.1".into(), opens_page: true });
        // Bấm ở bản dev: không tải (không có bố cục cài), trạng thái giữ nguyên.
        assert_eq!(u.install(), Effect::default());
        assert_eq!(u.version(), Some("9.0.1"));
    }

    #[test]
    fn major_always_opens_the_page() {
        let mut u = idle();
        u.handle(Msg::Started(Some(Box::new(manifest("10.0.0", Kind::Major)))));
        assert!(matches!(u.view(), UpdateView::Available { opens_page: true, .. }));
    }

    #[test]
    fn failures_are_silent_unless_asked() {
        let mut u = idle();
        u.handle(Msg::Checked { manual: false, result: Err("offline".into()) });
        assert_eq!(u.view(), &UpdateView::Idle);
        u.handle(Msg::Checked { manual: true, result: Err("offline".into()) });
        assert_eq!(u.view(), &UpdateView::Failed { version: None });
        u.handle(Msg::Checked { manual: true, result: Ok(Outcome::UpToDate) });
        assert_eq!(u.view(), &UpdateView::UpToDate);
        assert_eq!(u.version(), None);
    }

    /// Đang tải: tiến độ vào ảnh chụp; cài xong ⇒ daemon thoát; cài lỗi ⇒ giữ bản để thử lại.
    #[test]
    fn download_progress_then_install_quits() {
        let mut u = idle();
        u.handle(Msg::Started(Some(Box::new(manifest("9.0.1", Kind::Patch)))));
        u.view = UpdateView::Downloading { version: "9.0.1".into(), percent: 0 };
        assert!(u.handle(Msg::Progress(42)).changed);
        assert_eq!(u.view(), &UpdateView::Downloading { version: "9.0.1".into(), percent: 42 });
        assert_eq!(u.check(true), Effect::default(), "no check while downloading");
        let failed = u.handle(Msg::InstallFailed("sha256 mismatch".into()));
        assert!(!failed.quit);
        assert_eq!(u.view(), &UpdateView::Failed { version: Some("9.0.1".into()) });
        assert_eq!(u.version(), Some("9.0.1"), "button stays to retry");
        let done = u.handle(Msg::Installed);
        assert!(done.quit);
        assert_eq!(u.view(), &UpdateView::Installing { version: "9.0.1".into() });
    }
}
