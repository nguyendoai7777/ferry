//! Tiến trình nền `ferry`: khay, IPC, bật cửa sổ `ferry-ui`, chạy hàng đợi gửi sang điện thoại (engine trên runtime tokio
//! 2 luồng). Không GPU, không framework UI ("RAM là tính năng", `CLAUDE.md` mục Bố cục).
//!
//! `ferry` chạy tay ⇒ mở cửa sổ ngay; lần hai (đã có instance giữ khoá) ⇒ nhờ instance đó mở cửa sổ rồi thoát.
//! `ferry --autostart` (OS bật lúc đăng nhập, `autostart.rs`) chỉ nằm ở khay, gặp instance đã chạy thì thoát im lặng.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod daemon;
mod engine;
mod instance;
mod ipc_server;
mod notify;
mod platform;
mod tray;
mod tray_state;
mod ui_process;

use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferry_core::i18n::I18n;
use ferry_core::ipc::{ClientConnection, DaemonToUi, Endpoint, Role, UiToDaemon};
use ferry_core::pairing::Tokens;
use ferry_core::queue::{QueueStore, now_ms};
use ferry_core::transfer::{RunnerConfig, Throttle};
use ferry_core::{VERSION, config, logging, paths};

use crate::daemon::{Daemon, Event};
use crate::engine::{EngineConfig, Sinks};

fn main() -> ExitCode {
    let log_file = logging::init("daemon");
    let from_autostart = std::env::args().skip(1).any(|a| a == "--autostart");
    log::info!("ferry {VERSION} starting (pid {}, autostart: {from_autostart}, log: {log_file:?})", std::process::id());

    let data_dir = match paths::data_dir() {
        Ok(d) => d,
        Err(e) => return fail(&format!("no data dir: {e}")),
    };
    let lock = match instance::acquire(&data_dir) {
        Ok(Some(lock)) => lock,
        Ok(None) if from_autostart => {
            log::info!("autostart: ferry is already running, exiting");
            return ExitCode::SUCCESS;
        }
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
    let language = loaded.config.language.resolve();
    let autostart_error = autostart::sync_on_start(loaded.config.autostart).err();
    let throttle = Throttle::new(loaded.config.speed_limit_bytes());

    let store = match QueueStore::default_path()
        .map_err(|e| e.to_string())
        .and_then(|p| QueueStore::open(&p, now_ms()).map_err(|e| format!("{}: {e}", p.display())))
    {
        Ok(s) => Arc::new(Mutex::new(s)),
        Err(e) => return fail(&format!("cannot open the queue: {e}")),
    };

    let listener = match Endpoint::daemon().and_then(|ep| ep.listen()) {
        Ok(l) => l,
        Err(e) => return fail(&format!("cannot open ipc endpoint: {e}")),
    };

    // Discovery + một runner mỗi điện thoại + khay D-Bus (Linux): việc chờ mạng là chính, 2 luồng đủ.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("ferry-rt")
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return fail(&format!("cannot start the runtime: {e}")),
    };
    let rt = runtime.handle().clone();

    let result = platform::run_event_loop(move |waker| {
        let tray_waker = waker.clone();
        let app_name = I18n::new(language).t("app.name").to_owned();
        let cfg = EngineConfig {
            config_path,
            config: loaded.config,
            store,
            tokens: Tokens::os(),
            mdns: true,
            runner: RunnerConfig::default(),
            throttle,
            i18n: I18n::new(language),
            autostart: autostart::apply,
            autostart_error,
            watch_stable_for: ferry_core::watch::STABLE_FOR,
        };
        let sinks = Sinks {
            tray: Box::new(move |status| tray_waker.send(Event::Tray(status))),
            notice: Box::new(move |n| notify::show(n, &app_name)),
        };
        let engine = engine::spawn(&rt, cfg, sinks);
        ipc_server::spawn(listener, waker.clone(), engine.clone()).map_err(|e| format!("ipc server: {e}"))?;
        // Người dùng tự mở app ⇒ hiện cửa sổ ngay (không thì tưởng app không chạy, icon khay dễ bị bỏ qua);
        // OS bật lúc đăng nhập (`--autostart`) ⇒ chỉ nằm ở khay.
        if !from_autostart {
            waker.send(Event::OpenWindow);
        }
        Daemon::new(I18n::new(language), waker, engine, &rt)
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    drop(lock);
    match result {
        Ok(()) => {
            log::info!("ferry stopped");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}

/// Đã có daemon: bảo nó mở cửa sổ. Daemon kia có thể vừa giữ khoá mà chưa mở socket ⇒ thử lại một lúc.
fn activate_running() -> ExitCode {
    log::info!("another ferry is running, asking it to open the window");
    let mut last = None;
    for _ in 0..20 {
        match ClientConnection::open(Role::Cli).and_then(|mut c| c.request(&UiToDaemon::Focus)) {
            Ok(DaemonToUi::Ok) => return ExitCode::SUCCESS,
            Ok(other) => return fail(&format!("unexpected reply: {other:?}")),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    fail(&format!("cannot reach the running ferry: {}", last.map_or_else(String::new, |e| e.to_string())))
}

fn fail(msg: &str) -> ExitCode {
    log::error!("{msg}");
    log::logger().flush();
    ExitCode::FAILURE
}
