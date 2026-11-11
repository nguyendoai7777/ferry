//! Tiến trình nền `mws`: khay, IPC, bật cửa sổ `mws-ui`; sau này chạy hàng đợi gửi sang điện thoại (gd1_07). Không GPU,
//! không framework UI ("RAM là tính năng", `CLAUDE.md` mục Bố cục).
//!
//! `mws` lần hai (đã có instance giữ khoá) ⇒ nhờ instance đó mở cửa sổ rồi thoát.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod daemon;
mod instance;
mod ipc_server;
mod platform;
mod tray;
mod ui_process;

use std::process::ExitCode;
use std::time::Duration;

use mws_core::i18n::I18n;
use mws_core::ipc::{ClientConnection, DaemonToUi, Endpoint, Role, UiToDaemon};
use mws_core::{VERSION, config, logging, paths};

use crate::daemon::Daemon;

fn main() -> ExitCode {
    let log_file = logging::init("daemon");
    log::info!("mws {VERSION} starting (pid {}, log: {log_file:?})", std::process::id());

    let data_dir = match paths::data_dir() {
        Ok(d) => d,
        Err(e) => return fail(&format!("no data dir: {e}")),
    };
    let lock = match instance::acquire(&data_dir) {
        Ok(Some(lock)) => lock,
        Ok(None) => return activate_running(),
        Err(e) => return fail(&format!("cannot take instance lock: {e}")),
    };

    let config_path = match paths::config_file() {
        Ok(p) => p,
        Err(e) => return fail(&format!("no config dir: {e}")),
    };
    let loaded = match config::load_or_init(&config_path) {
        Ok(l) => l,
        Err(e) => return fail(&format!("cannot load {}: {e}", config_path.display())),
    };
    if let Some(bad) = &loaded.backup {
        log::warn!("config: broken file moved to {}", bad.display());
    }
    if loaded.written {
        log::info!("config: wrote {}", config_path.display());
    }
    let i18n = I18n::new(loaded.config.language.resolve());

    let listener = match Endpoint::daemon().and_then(|ep| ep.listen()) {
        Ok(l) => l,
        Err(e) => return fail(&format!("cannot open ipc endpoint: {e}")),
    };

    let result = platform::run_event_loop(move |waker| {
        ipc_server::spawn(listener, waker.clone()).map_err(|e| format!("ipc server: {e}"))?;
        Daemon::new(i18n, waker)
    });
    drop(lock);
    match result {
        Ok(()) => {
            log::info!("mws stopped");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

/// Đã có daemon: bảo nó mở cửa sổ. Daemon kia có thể vừa giữ khoá mà chưa mở socket ⇒ thử lại một lúc.
fn activate_running() -> ExitCode {
    log::info!("another mws is running, asking it to open the window");
    let mut last = None;
    for _ in 0..20 {
        match ClientConnection::open(Role::Cli).and_then(|mut c| c.request(&UiToDaemon::Focus)) {
            Ok(DaemonToUi::Ok) => return ExitCode::SUCCESS,
            Ok(other) => return fail(&format!("unexpected reply: {other:?}")),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    fail(&format!("cannot reach the running mws: {}", last.map_or_else(String::new, |e| e.to_string())))
}

fn fail(msg: &str) -> ExitCode {
    log::error!("{msg}");
    log::logger().flush();
    ExitCode::FAILURE
}
