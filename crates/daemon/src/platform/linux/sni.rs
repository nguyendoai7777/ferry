//! Khay Linux: StatusNotifierItem + DBusMenu tự viết trên `zbus` (bê từ fastcast; `ksni` Unlicense ngoài D-2,
//! `libappindicator` LGPL + GTK). Host trên Ubuntu là extension `ubuntu-appindicators`; không có host ⇒ daemon vẫn
//! chạy, tự đăng ký lại khi host xuất hiện (extension tắt lúc khoá màn, bật lại lúc mở).
//!
//! Chạy trên runtime tokio của engine (một task). Luồng chính đổi [`Content`] rồi gửi `Refresh`; task phát tín hiệu
//! `NewIcon` / `NewToolTip` / `LayoutUpdated` để host đọc lại thuộc tính.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use ferry_core::i18n::I18n;
use futures::StreamExt;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, Structure, Value};

use super::Waker;
use crate::daemon::Event;
use crate::tray::{self, LINUX, MenuEntry};
use crate::tray_state::{TrayMode, TrayStatus};

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_IFACE: &str = "org.kde.StatusNotifierWatcher";
/// Cỡ host hay xin (thanh GNOME 16–24 px, HiDPI ×2).
const SIZES: [u32; 5] = [16, 22, 24, 32, 48];

pub struct Content {
    title: String,
    tooltip: String,
    mode: TrayMode,
    menu: Vec<MenuEntry>,
    /// Tăng mỗi lần dựng lại menu; host so để biết layout cũ.
    revision: u32,
}

type Shared = Arc<Mutex<Content>>;

fn lock(shared: &Shared) -> std::sync::MutexGuard<'_, Content> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Phần nào đổi ⇒ tín hiệu nào.
#[derive(Debug, Clone, Copy, Default)]
struct Refresh {
    icon: bool,
    tooltip: bool,
    menu: bool,
}

pub struct Tray {
    shared: Shared,
    tx: UnboundedSender<Refresh>,
    status: Option<TrayStatus>,
}

impl Tray {
    /// Luôn `Ok`: có host hay không là chuyện của task D-Bus, biết sau và có thể đổi lúc chạy.
    pub fn new(i18n: &I18n, waker: Waker, rt: &tokio::runtime::Handle) -> Tray {
        let shared = Arc::new(Mutex::new(Content {
            title: i18n.t("app.name").to_owned(),
            tooltip: i18n.t("app.name").to_owned(),
            mode: TrayMode::Idle,
            menu: tray::menu_entries(i18n),
            revision: 1,
        }));
        let (tx, rx) = unbounded_channel();
        rt.spawn(run(shared.clone(), rx, waker));
        Tray { shared, tx, status: None }
    }

    pub fn apply(&mut self, status: &TrayStatus, i18n: &I18n) {
        if self.status.as_ref() == Some(status) {
            return;
        }
        let mut r = Refresh::default();
        {
            let mut c = lock(&self.shared);
            if tray::icon_key(c.mode) != tray::icon_key(status.mode) {
                r.icon = true;
            }
            c.mode = status.mode;
            if c.tooltip != status.tooltip {
                c.tooltip.clone_from(&status.tooltip);
                r.tooltip = true;
            }
            let menu = tray::menu_entries(i18n);
            if menu != c.menu {
                c.menu = menu;
                c.revision += 1;
                r.menu = true;
            }
        }
        self.status = Some(status.clone());
        if r.icon || r.tooltip || r.menu {
            let _ = self.tx.send(r);
        }
    }
}

/// RGBA thẳng ⇒ ARGB32 thứ tự mạng, định dạng `IconPixmap` của SNI.
fn argb(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect()
}

/// `a(iiay)`: rộng, cao, ARGB32.
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

struct Item {
    shared: Shared,
    waker: Waker,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    /// Chuột trái: mở cửa sổ.
    fn activate(&self, _x: i32, _y: i32) {
        self.waker.send(Event::TrayLeftClick);
    }

    fn secondary_activate(&self, _x: i32, _y: i32) {}

    /// `ubuntu-appindicators` không gọi: chuột phải mở thẳng `Menu` do Shell vẽ.
    fn context_menu(&self, _x: i32, _y: i32) {}

    fn scroll(&self, _delta: i32, _orientation: &str) {}

    fn provide_xdg_activation_token(&self, _token: String) {}

    #[zbus(signal)]
    async fn new_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property)]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property)]
    fn id(&self) -> &str {
        "ferry"
    }

    #[zbus(property)]
    fn title(&self) -> String {
        lock(&self.shared).title.clone()
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property)]
    fn window_id(&self) -> i32 {
        0
    }

    #[zbus(property)]
    fn icon_name(&self) -> &str {
        ""
    }

    /// Dựng khi host hỏi, không giữ trong RAM.
    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        let mode = lock(&self.shared).mode;
        SIZES.iter().map(|&s| (s as i32, s as i32, argb(&tray::icon_rgba(s, mode, LINUX)))).collect()
    }

    /// `false`: nhấn trái gọi `Activate` (mở cửa sổ), nhấn phải host mở `Menu`.
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(MENU_PATH).expect("valid path")
    }

    #[zbus(property, name = "ToolTip")]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        let c = lock(&self.shared);
        (String::new(), Vec::new(), c.title.clone(), c.tooltip.clone())
    }
}

struct Menu {
    shared: Shared,
    waker: Waker,
}

fn entry_props(entry: &MenuEntry) -> HashMap<String, Value<'static>> {
    let mut props = HashMap::new();
    match entry {
        MenuEntry::Item { label, enabled, .. } => {
            props.insert("label".to_owned(), Value::from(label.clone()));
            if !enabled {
                props.insert("enabled".to_owned(), Value::from(false));
            }
        }
        MenuEntry::Separator => {
            props.insert("type".to_owned(), Value::from("separator".to_owned()));
        }
    }
    props
}

/// Một nút `(ia{sv}av)`. Id trong DBusMenu = vị trí trong danh sách + 1, 0 là gốc.
type Node = (i32, HashMap<String, Value<'static>>, Vec<Value<'static>>);

fn layout(content: &Content, parent: i32) -> Node {
    if parent > 0 {
        let props = content.menu.get(parent as usize - 1).map(entry_props).unwrap_or_default();
        return (parent, props, Vec::new());
    }
    let children = content
        .menu
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let child: Node = (i as i32 + 1, entry_props(entry), Vec::new());
            Value::from(Structure::from(child))
        })
        .collect();
    let mut props = HashMap::new();
    props.insert("children-display".to_owned(), Value::from("submenu".to_owned()));
    (0, props, children)
}

/// Chỉ chạy đúng mục đã dựng và đang bật: id ngoài danh sách, separator, mục tắt, sự kiện khác `clicked` ⇒ bỏ.
fn clicked_id(menu: &[MenuEntry], id: i32, event_id: &str) -> Option<&'static str> {
    if event_id != "clicked" || id <= 0 {
        return None;
    }
    match menu.get(id as usize - 1)? {
        MenuEntry::Item { id, enabled: true, .. } => Some(id),
        _ => None,
    }
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(&self, parent_id: i32, _depth: i32, _names: Vec<String>) -> (u32, Node) {
        let content = lock(&self.shared);
        (content.revision, layout(&content, parent_id))
    }

    fn get_group_properties(&self, ids: Vec<i32>, _names: Vec<String>) -> Vec<(i32, HashMap<String, Value<'static>>)> {
        let content = lock(&self.shared);
        ids.into_iter()
            .filter(|&id| id > 0)
            .filter_map(|id| Some((id, entry_props(content.menu.get(id as usize - 1)?))))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> zbus::fdo::Result<Value<'static>> {
        let content = lock(&self.shared);
        let entry = usize::try_from(id - 1).ok().and_then(|i| content.menu.get(i));
        entry
            .and_then(|e| entry_props(e).remove(&name))
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("unknown menu property".to_owned()))
    }

    fn event(&self, id: i32, event_id: String, _data: Value<'_>, _timestamp: u32) {
        let picked = clicked_id(&lock(&self.shared).menu, id, &event_id);
        if let Some(picked) = picked {
            self.waker.send(Event::Menu(picked.to_owned()));
        }
    }

    fn event_group(&self, events: Vec<(i32, String, Value<'_>, u32)>) -> Vec<i32> {
        let content = lock(&self.shared);
        for (id, event_id, ..) in &events {
            if let Some(picked) = clicked_id(&content.menu, *id, event_id) {
                self.waker.send(Event::Menu(picked.to_owned()));
            }
        }
        Vec::new()
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(signal)]
    async fn layout_updated(emitter: &SignalEmitter<'_>, revision: u32, parent: i32) -> zbus::Result<()>;

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }
}

async fn register_item(conn: &zbus::Connection, name: &str) {
    let result = conn
        .call_method(Some(WATCHER_NAME), WATCHER_PATH, Some(WATCHER_IFACE), "RegisterStatusNotifierItem", &name)
        .await;
    match result {
        Ok(_) => log::info!("tray: registered with the status notifier host"),
        // Không có khay (GNOME tắt extension): mở cửa sổ bằng cách chạy `ferry` lần nữa.
        Err(e) => log::info!("tray: no status notifier host yet: {e}"),
    }
}

async fn serve(conn: &zbus::Connection, shared: Shared, waker: Waker) -> zbus::Result<String> {
    conn.object_server().at(ITEM_PATH, Item { shared: shared.clone(), waker: waker.clone() }).await?;
    conn.object_server().at(MENU_PATH, Menu { shared, waker }).await?;
    let name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
    conn.request_name(name.as_str()).await?;
    Ok(name)
}

async fn signal(conn: &zbus::Connection, shared: &Shared, r: Refresh) -> zbus::Result<()> {
    let item = conn.object_server().interface::<_, Item>(ITEM_PATH).await?;
    if r.icon {
        Item::new_icon(item.signal_emitter()).await?;
    }
    if r.tooltip {
        Item::new_tool_tip(item.signal_emitter()).await?;
    }
    if r.menu {
        let menu = conn.object_server().interface::<_, Menu>(MENU_PATH).await?;
        let revision = lock(shared).revision;
        Menu::layout_updated(menu.signal_emitter(), revision, 0).await?;
    }
    Ok(())
}

async fn run(shared: Shared, mut rx: UnboundedReceiver<Refresh>, waker: Waker) {
    let conn = match zbus::Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            log::error!("tray: no session bus: {e}");
            return;
        }
    };
    let name = match serve(&conn, shared.clone(), waker).await {
        Ok(name) => name,
        Err(e) => {
            log::error!("tray: cannot publish the item: {e}");
            return;
        }
    };
    // Theo dõi TRƯỚC khi đăng ký: host lên giữa hai bước vẫn được báo.
    let owners = match zbus::fdo::DBusProxy::new(&conn).await {
        Ok(dbus) => dbus.receive_name_owner_changed_with_args(&[(0, WATCHER_NAME)]).await.ok(),
        Err(_) => None,
    };
    let Some(mut owners) = owners else {
        log::warn!("tray: cannot watch for the status notifier host");
        return;
    };
    register_item(&conn, &name).await;
    loop {
        tokio::select! {
            r = rx.recv() => match r {
                Some(mut r) => {
                    // Gộp các lần đổi dồn lại (tiến độ) thành một lượt tín hiệu.
                    while let Ok(more) = rx.try_recv() {
                        r.icon |= more.icon;
                        r.tooltip |= more.tooltip;
                        r.menu |= more.menu;
                    }
                    if let Err(e) = signal(&conn, &shared, r).await {
                        log::warn!("tray: refresh failed: {e}");
                    }
                }
                None => return,
            },
            Some(change) = owners.next() => {
                if change.args().is_ok_and(|a| a.new_owner().is_some()) {
                    register_item(&conn, &name).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferry_core::i18n::Language;
    use zbus::zvariant::Type;

    /// Menu thật (Mở · Thoát) + một mục tắt đặt thêm để thử cờ `enabled` của dbusmenu.
    fn content() -> Content {
        let i18n = I18n::new(Language::En);
        let mut menu = tray::menu_entries(&i18n);
        menu.push(MenuEntry::Item { id: "off", label: "Off".into(), enabled: false });
        Content { title: "Ferry".into(), tooltip: String::new(), mode: TrayMode::Idle, menu, revision: 1 }
    }

    #[test]
    fn argb_moves_alpha_first() {
        assert_eq!(argb(&[1, 2, 3, 4, 5, 6, 7, 8]), [4, 1, 2, 3, 8, 5, 6, 7]);
    }

    #[test]
    fn only_a_click_on_an_enabled_item_counts() {
        let menu = content().menu;
        let pos = |want: &str| {
            menu.iter().position(|e| matches!(e, MenuEntry::Item { id, .. } if *id == want)).unwrap() as i32 + 1
        };
        let sep = menu.iter().position(|e| matches!(e, MenuEntry::Separator)).unwrap() as i32 + 1;
        assert_eq!(clicked_id(&menu, pos("quit"), "clicked"), Some("quit"));
        assert_eq!(clicked_id(&menu, pos("quit"), "hovered"), None);
        assert_eq!(clicked_id(&menu, pos("open"), "clicked"), Some("open"));
        assert_eq!(clicked_id(&menu, pos("off"), "clicked"), None, "disabled");
        assert_eq!(clicked_id(&menu, sep, "clicked"), None);
        assert_eq!(clicked_id(&menu, 0, "clicked"), None);
        assert_eq!(clicked_id(&menu, -1, "clicked"), None);
        assert_eq!(clicked_id(&menu, 9999, "clicked"), None);
    }

    #[test]
    fn layout_has_the_dbusmenu_signature_and_disabled_flag() {
        assert_eq!(Node::SIGNATURE.to_string(), "(ia{sv}av)");
        let c = content();
        let root = layout(&c, 0);
        assert_eq!(root.2.len(), c.menu.len());
        assert_eq!(root.2[0].value_signature().to_string(), "(ia{sv}av)");
        let off = layout(&c, 4); // Mở · — · Thoát · Off
        assert_eq!(off.1.get("enabled"), Some(&Value::from(false)));
        assert!(!layout(&c, 1).1.contains_key("enabled"));
    }
}
