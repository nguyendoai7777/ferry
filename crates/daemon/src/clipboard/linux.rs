//! Làm chủ selection `CLIPBOARD` của X11 (XWayland trên GNOME Wayland). Một luồng nền giữ kết nối và phục vụ
//! `SelectionRequest`; đổi nội dung chỉ thay `Payload` dùng chung rồi nhận lại quyền sở hữu.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, SELECTION_NOTIFY_EVENT,
    SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{CURRENT_TIME, NONE};

#[derive(Clone, Copy)]
struct Atoms {
    clipboard: Atom,
    targets: Atom,
    utf8: Atom,
    text_plain: Atom,
    uri_list: Atom,
    gnome: Atom,
}

#[derive(Default)]
struct Payload {
    uri_list: Vec<u8>,
    gnome: Vec<u8>,
    text: Vec<u8>,
}

struct Owner {
    conn: Arc<RustConnection>,
    window: Window,
    atoms: Atoms,
    data: Arc<Mutex<Option<Payload>>>,
}

static OWNER: OnceLock<Result<Owner, String>> = OnceLock::new();

fn owner() -> Result<&'static Owner, String> {
    OWNER.get_or_init(start).as_ref().map_err(Clone::clone)
}

fn start() -> Result<Owner, String> {
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("X11 connect: {e}"))?;
    let conn = Arc::new(conn);
    let root = conn.setup().roots[screen].root;
    let window = conn.generate_id().map_err(|e| e.to_string())?;
    conn.create_window(0, window, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, 0, &CreateWindowAux::new())
        .map_err(|e| e.to_string())?;
    let intern = |name: &str| -> Result<Atom, String> {
        Ok(conn
            .intern_atom(false, name.as_bytes())
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .atom)
    };
    let atoms = Atoms {
        clipboard: intern("CLIPBOARD")?,
        targets: intern("TARGETS")?,
        utf8: intern("UTF8_STRING")?,
        text_plain: intern("text/plain;charset=utf-8")?,
        uri_list: intern("text/uri-list")?,
        gnome: intern("x-special/gnome-copied-files")?,
    };
    let data: Arc<Mutex<Option<Payload>>> = Arc::default();
    let served = atoms;
    let (c, d) = (conn.clone(), data.clone());
    std::thread::Builder::new()
        .name("ferry-clipboard".into())
        .spawn(move || serve(&c, &served, &d))
        .map_err(|e| e.to_string())?;
    Ok(Owner { conn, window, atoms, data })
}

fn serve(conn: &RustConnection, atoms: &Atoms, data: &Mutex<Option<Payload>>) {
    loop {
        let event = match conn.wait_for_event() {
            Ok(e) => e,
            Err(e) => {
                log::warn!("clipboard X11 connection lost: {e}");
                *data.lock().unwrap_or_else(|p| p.into_inner()) = None;
                return;
            }
        };
        match event {
            Event::SelectionRequest(req) => {
                if let Err(e) = answer(conn, atoms, data, &req) {
                    log::debug!("clipboard answer: {e}");
                }
            }
            // Ứng dụng khác chép đè ⇒ không còn là của ta.
            Event::SelectionClear(e) if e.selection == atoms.clipboard => {
                *data.lock().unwrap_or_else(|p| p.into_inner()) = None;
            }
            _ => {}
        }
    }
}

fn answer(
    conn: &RustConnection,
    atoms: &Atoms,
    data: &Mutex<Option<Payload>>,
    req: &SelectionRequestEvent,
) -> Result<(), Box<dyn std::error::Error>> {
    // Client cũ (ICCCM) để property NONE ⇒ dùng tên target.
    let mut property = if req.property == NONE { req.target } else { req.property };
    let guard = data.lock().unwrap_or_else(|p| p.into_inner());
    match guard.as_ref() {
        Some(_) if req.selection == atoms.clipboard && req.target == atoms.targets => {
            let list = [atoms.targets, atoms.uri_list, atoms.gnome, atoms.utf8, atoms.text_plain];
            conn.change_property32(PropMode::REPLACE, req.requestor, property, AtomEnum::ATOM, &list)?;
        }
        Some(p) if req.selection == atoms.clipboard => {
            let bytes = if req.target == atoms.uri_list {
                Some(&p.uri_list)
            } else if req.target == atoms.gnome {
                Some(&p.gnome)
            } else if req.target == atoms.utf8 || req.target == atoms.text_plain {
                Some(&p.text)
            } else {
                None
            };
            // Danh sách mục cấp trên nhỏ; quá giới hạn một request (không làm INCR) ⇒ từ chối.
            let max = conn.maximum_request_bytes().saturating_sub(64);
            match bytes {
                Some(b) if b.len() <= max => {
                    conn.change_property8(PropMode::REPLACE, req.requestor, property, req.target, b)?;
                }
                _ => property = NONE,
            }
        }
        _ => property = NONE,
    }
    drop(guard);
    let notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: req.time,
        requestor: req.requestor,
        selection: req.selection,
        target: req.target,
        property,
    };
    conn.send_event(false, req.requestor, EventMask::NO_EVENT, notify)?;
    conn.flush()?;
    Ok(())
}

pub(super) fn set_files(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    let o = owner()?;
    let uris: Vec<String> = paths.iter().map(|p| super::file_uri(p)).collect();
    let payload = Payload {
        uri_list: uris.iter().map(|u| format!("{u}\r\n")).collect::<String>().into_bytes(),
        gnome: format!("{}\n{}", if cut { "cut" } else { "copy" }, uris.join("\n")).into_bytes(),
        text: paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>().join("\n").into_bytes(),
    };
    *o.data.lock().unwrap_or_else(|p| p.into_inner()) = Some(payload);
    let err = |e: &dyn std::fmt::Display| e.to_string();
    o.conn.set_selection_owner(o.window, o.atoms.clipboard, CURRENT_TIME).map_err(|e| err(&e))?;
    let now = o.conn.get_selection_owner(o.atoms.clipboard).map_err(|e| err(&e))?.reply().map_err(|e| err(&e))?;
    if now.owner != o.window {
        *o.data.lock().unwrap_or_else(|p| p.into_inner()) = None;
        return Err("could not take the X11 clipboard".into());
    }
    Ok(())
}

pub(super) fn clear() -> Result<(), String> {
    let Some(Ok(o)) = OWNER.get() else { return Ok(()) };
    let had = o.data.lock().unwrap_or_else(|p| p.into_inner()).take().is_some();
    if had {
        let now = o.conn.get_selection_owner(o.atoms.clipboard).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        if now.owner == o.window {
            o.conn.set_selection_owner(NONE, o.atoms.clipboard, CURRENT_TIME).map_err(|e| e.to_string())?;
            o.conn.flush().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Thử tay trên máy có phiên đồ hoạ (đụng clipboard thật): `cargo test -p ferry-daemon x11_clipboard -- --ignored`.
    #[test]
    #[ignore = "touches the real clipboard"]
    fn x11_clipboard_offers_file_lists() {
        let dir = std::env::temp_dir().join("ferry clip test");
        std::fs::create_dir_all(&dir).unwrap();
        super::set_files(std::slice::from_ref(&dir), true).unwrap();
        let read = |target: &str| {
            let out = std::process::Command::new("xclip")
                .args(["-selection", "clipboard", "-o", "-t", target])
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap()
        };
        let targets = read("TARGETS");
        assert!(targets.contains("x-special/gnome-copied-files"), "{targets}");
        let gnome = read("x-special/gnome-copied-files");
        assert!(gnome.starts_with("cut\nfile://") && gnome.contains("ferry%20clip%20test"), "{gnome}");
        super::clear().unwrap();
    }
}
