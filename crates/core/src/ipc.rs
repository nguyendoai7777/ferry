//! IPC daemon `ferry` ↔ cửa sổ `ferry-ui` (↔ lần chạy `ferry` thứ hai): JSON một dòng mỗi thông điệp qua local socket — named
//! pipe trên Windows, Unix socket trên macOS / Linux (crate `interprocess`, đồng bộ, không async runtime). Bê từ
//! `fastcast_core::ipc`.
//!
//! Daemon là server duy nhất. Client mở đầu bằng [`UiToDaemon::Hello`] (vai + phiên bản), daemon trả `Ok`. Thông điệp lạ
//! (bản mới hơn) ⇒ `InvalidData`, bên nhận log rồi đọc tiếp, không cắt kết nối.
//!
//! Luật (`CLAUDE.md`): token không bao giờ đi qua IPC (PIN chỉ đi một chiều UI → daemon, kiểu [`Pin`] che khi log); UI
//! không gọi mạng — mọi lệnh đi đây.
//!
//! Hợp đồng gd1_07 (task 08 / 09 / 10 / 12 dùng; thêm biến thể ⇒ tối đa 4 mỗi task, ghi mục 9 của task):
//! - UI ⇒ daemon: `Queue` / `Phone` / `Library`, mỗi lệnh mang `req` (số UI tự tăng); daemon trả đúng một
//!   [`DaemonToUi::Reply`] cùng `req` với [`Outcome`] (`failed` + [`CommandError`] khi không làm được). Lệnh chạy nền
//!   (thêm tệp, ghép, đọc thư viện) trả lời khi xong, không chặn lệnh khác.
//! - Daemon ⇒ UI (role `ui` sau `Hello`): [`DaemonToUi::Snapshot`] đầy đủ ngay khi nối, rồi mỗi khi đổi; chỉ tiến độ
//!   trong khúc đổi ⇒ [`DaemonToUi::Progress`]. Gộp tối đa [`MAX_PUSH_PER_SEC`] lần / s. UI thay toàn bộ trạng thái bằng
//!   `Snapshot`, cộng `Progress` lên trên (`sent` mới hơn `offset` của mục).
//! - `Cli` (lần chạy `ferry` thứ hai) không nhận đẩy.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{ListenerOptions, Name, Stream};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::device::{DeviceId, DeviceState};
use crate::protocol::LibraryItem;
use crate::queue::store::{BatchInfo, BatchProgress, UploadItem};
use crate::queue::{BatchId, ItemId};

/// Một dòng JSON tối đa — chặn client hỏng / độc gửi dòng vô tận làm daemon phình RAM. Tệp không đi qua IPC (chỉ đường
/// dẫn), 1 MiB là dư.
pub const MAX_LINE: u64 = 1 << 20;

/// Phiên bản thông điệp IPC (khác `api` của giao thức HTTP với điện thoại). UI và daemon cùng một bản cài nên chỉ để log.
pub const PROTOCOL_VERSION: u32 = 1;

/// Client → daemon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum UiToDaemon {
    /// Thông điệp đầu tiên của mọi kết nối.
    Hello { role: Role, pid: u32, protocol: u32 },
    /// Đưa cửa sổ chính lên trước (chưa mở thì daemon bật `ferry-ui`). Lần chạy `ferry` thứ hai gửi cái này rồi thoát.
    Focus,
    /// Lệnh hàng đợi (gd1_07). Mọi lệnh có `req` (client tự đánh số, tăng dần theo kết nối): daemon trả đúng một
    /// [`DaemonToUi::Reply`] cùng `req`, xen giữa các ảnh chụp trạng thái.
    Queue { req: u64, op: QueueOp },
    /// Lệnh điện thoại: ghép, quên, nhập địa chỉ tay (gd1_07).
    Phone { req: u64, op: PhoneOp },
    /// Thư viện điện thoại (gd1_07: `List`; gd1_10 / gd1_11 thêm `Download`).
    Library { req: u64, op: LibraryOp },
}

/// Thao tác người dùng trên mục / lô (nút ở hàng đợi). Ánh xạ sang `transfer::ItemAction` ở daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Pause,
    Resume,
    Cancel,
    Retry,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum QueueOp {
    /// Thả / dán / chọn tệp + thư mục (W-3, W-4) ⇒ một lô. Daemon lọc đuôi (§6.6), duyệt thư mục đệ quy. Trả
    /// [`Outcome::Added`]. `destDir` rỗng = gốc nơi lưu; có `..` ⇒ `InvalidInput`.
    AddFiles {
        device: DeviceId,
        paths: Vec<PathBuf>,
        dest_dir: String,
    },
    Item {
        id: ItemId,
        action: Action,
    },
    Batch {
        batch: BatchId,
        action: Action,
    },
    /// "Tạm dừng tất cả" / "Tiếp tục tất cả" (khay, cửa sổ). Chỉ nhận `pause` / `resume`.
    All {
        action: Action,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum PhoneOp {
    /// Ghép thiết bị đang thấy (`Discovered`) hoặc ghép lại (`NeedsRepair`) bằng PIN 6 số đọc trên điện thoại. Trả
    /// [`Outcome::Paired`]; sai ⇒ `WrongPin` / `RateLimited`.
    Pair { device: DeviceId, pin: Pin },
    /// Quên trên PC: báo điện thoại (nếu Online), xoá token, **xoá hàng đợi của máy đó**. Trả `Done`.
    Forget { device: DeviceId },
    /// Địa chỉ nhập tay (mọi dạng của `discovery::address::parse_address`) ⇒ hỏi `info` ⇒ thiết bị vào danh sách. Trả
    /// [`Outcome::Found`].
    AddAddress { address: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum LibraryOp {
    /// `GET /list` của điện thoại đang Online. Trả [`Outcome::Library`]; Offline ⇒ `Offline`.
    List { device: DeviceId },
}

/// PIN ghép: chỉ đi UI → daemon, không bao giờ vào snapshot / log (`Debug` che). Daemon bọc ngay `protocol::Secret`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Pin(pub String);

impl std::fmt::Debug for Pin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pin(***)")
    }
}

/// Daemon → client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum DaemonToUi {
    /// Trả lời `Hello` / lệnh thành công.
    Ok,
    Error {
        message: String,
    },
    /// Daemon → UI: đưa cửa sổ lên trước (người dùng chọn "Mở cửa sổ" khi UI đang chạy).
    Focus,
    /// Ảnh chụp đầy đủ (gd1_07): ngay sau `Hello` của vai `Ui`, rồi mỗi khi thiết bị / mục / lô đổi — gộp tối đa
    /// [`MAX_PUSH_PER_SEC`] lần / s. UI thay toàn bộ trạng thái bằng bản này.
    Snapshot(Snapshot),
    /// Tiến độ giữa hai ảnh chụp (chỉ mục đang gửi), cùng nhịp gộp. UI áp lên bản đầy đủ gần nhất.
    Progress(ProgressDelta),
    /// Trả lời đúng một lần cho lệnh có `req`.
    Reply {
        req: u64,
        outcome: Outcome,
    },
}

/// Trần nhịp đẩy `Snapshot` + `Progress` cho mỗi client.
pub const MAX_PUSH_PER_SEC: u32 = 10;

/// Số mục đã kết thúc (Đã gửi / Bỏ qua / Huỷ / Lỗi) tối đa trong một ảnh chụp — mới nhất trước; phần còn lại chỉ đếm
/// ([`Snapshot::finished_omitted`]). Giữ dòng JSON xa [`MAX_LINE`].
pub const FINISHED_IN_SNAPSHOT: usize = 300;

/// Số mục chưa kết thúc tối đa trong một ảnh chụp — đầu hàng đợi (gửi trước) trước; lô rất lớn (thả cả ổ) thì phần
/// sau chỉ hiện qua tổng của [`BatchView`] ([`Snapshot::queued_omitted`]). Cùng [`FINISHED_IN_SNAPSHOT`] giữ dòng JSON
/// dưới [`MAX_LINE`] (~1 300 mục × vài trăm byte).
pub const QUEUED_IN_SNAPSHOT: usize = 1000;

/// Trạng thái daemon cho cửa sổ (PROTOCOL 4.2, 4.4, 4.5). Không token, không PIN.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// Mọi thiết bị trong sổ: đã ghép (mọi trạng thái) + đang thấy chưa ghép (`Discovered`, màn "Thêm điện thoại").
    pub devices: Vec<DeviceView>,
    /// Lô còn mục trong `uploads` (dòng gộp "N tệp" / tên thư mục, tổng tiến độ).
    pub batches: Vec<BatchView>,
    /// Mục chưa kết thúc (≤ [`QUEUED_IN_SNAPSHOT`], đầu hàng đợi) và mục đã kết thúc mới nhất
    /// (≤ [`FINISHED_IN_SNAPSHOT`]), xếp theo `id` (thứ tự thêm).
    pub uploads: Vec<UploadItem>,
    /// Tiến độ trong khúc của mục đang gửi (byte đã đi trong khúc hiện tại cộng dồn — số mới hơn `offset` của store).
    pub progress: Vec<ItemProgress>,
    /// Mục đã kết thúc không đưa vào `uploads` (cũ hơn).
    pub finished_omitted: u64,
    /// Mục chưa kết thúc không có trong `uploads` (sau [`QUEUED_IN_SNAPSHOT`]).
    pub queued_omitted: u64,
    /// Có mục đang chạy ⇒ khay / nút hiện "Tạm dừng tất cả"; không mà có mục tạm dừng ⇒ "Tiếp tục tất cả".
    pub can_pause_all: bool,
    pub can_resume_all: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    pub id: DeviceId,
    pub name: String,
    pub state: DeviceState,
    /// Đang thấy qua mDNS (chấm xanh khi cùng `Online`).
    pub visible: bool,
    /// `api` điện thoại mới hơn bản PC ⇒ "Cập nhật app trên máy tính" (gd1_09).
    pub newer_api: bool,
    /// `info.features` lần cuối; thiếu `dirs` ⇒ gửi phẳng, "Cập nhật app trên điện thoại để giữ thư mục".
    pub features: Vec<String>,
    /// Địa chỉ đang dùng (`ip:port`, `[v6]:port`, `[fe80::x%iface]:port`) — chỉ để hiện ở cài đặt nâng cao.
    pub address: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchView {
    pub info: BatchInfo,
    pub progress: BatchProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemProgress {
    pub id: ItemId,
    /// Byte điện thoại đã nhận (gồm phần đang bay của khúc hiện tại).
    pub sent: u64,
    pub size: u64,
    /// Trung bình trượt 5 s.
    pub bytes_per_sec: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressDelta {
    pub items: Vec<ItemProgress>,
}

/// Kết quả một lệnh.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum Outcome {
    Done,
    /// `AddFiles`: `batch = None` khi không có tệp nào gửi được (toàn tệp không phải video / không đọc được).
    Added {
        batch: Option<BatchId>,
        files: u64,
        skipped_non_video: u64,
        unreadable: u64,
    },
    Paired {
        device: DeviceId,
        name: String,
    },
    Found {
        device: DeviceId,
        name: String,
    },
    Library {
        items: Vec<LibraryItem>,
    },
    Failed {
        error: CommandError,
    },
}

/// Vì sao lệnh không xong — UI đổi thành chữ nói việc cần làm (§2.6), không hiện mã HTTP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum CommandError {
    /// Không có thiết bị này trong sổ.
    UnknownDevice,
    /// Thiết bị chưa ghép / cần ghép lại.
    NotPaired,
    /// Điện thoại không Online ("Bật Sync WiFi trên điện thoại").
    Offline,
    /// Tham số sai (đích có `..`, hành động không hợp lệ, địa chỉ không đọc được, PIN không phải 6 số).
    InvalidInput,
    /// Địa chỉ link-local ghi card mạng không có trên máy (`[fe80::5%en9]`).
    UnknownInterface {
        name: String,
    },
    WrongPin,
    /// Sai PIN quá nhiều: UI đếm ngược rồi mới cho nhập lại.
    RateLimited {
        retry_after_secs: u64,
    },
    /// Địa chỉ giờ là một điện thoại khác.
    OtherPhone,
    /// Không mở được kho khoá của hệ thống.
    Keyring,
    /// Không nối được điện thoại.
    Unreachable,
    /// Lỗi trong daemon (đĩa, SQLite) — chi tiết ở log.
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Cửa sổ `ferry-ui`: nhận thông điệp đẩy suốt đời kết nối.
    Ui,
    /// Lần chạy `ferry` thứ hai / lệnh dòng lệnh: gửi một lệnh rồi đóng.
    Cli,
}

/// Ghi một thông điệp + `\n`, flush.
pub fn send<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(msg).map_err(io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

/// Đọc một thông điệp. Hết luồng ⇒ `Ok(None)`. Dòng quá [`MAX_LINE`] hoặc JSON lạ ⇒ `InvalidData` (dòng hỏng đã bị đọc
/// hết, lần đọc sau lấy dòng kế).
pub fn recv<T: DeserializeOwned>(r: &mut impl BufRead) -> io::Result<Option<T>> {
    let mut line = String::new();
    let n = r.take(MAX_LINE + 1).read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    if n as u64 > MAX_LINE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "ipc message too long"));
    }
    serde_json::from_str(line.trim_end()).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Địa chỉ server của daemon cho người dùng hiện tại.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Windows: tên pipe; Unix: đường dẫn tệp socket.
    id: String,
    /// Endpoint thật: client kiểm exe của server (Windows), socket nằm trong thư mục riêng 0700 (Unix).
    guarded: bool,
}

impl Endpoint {
    /// Endpoint thật của daemon. Windows: pipe theo tên người dùng (pipe là toàn máy, không theo phiên); có
    /// `FERRY_DATA_DIR` thì thêm hậu tố theo thư mục đó (hai bản dev không chung pipe). Unix: `<runtime_dir>/daemon.sock`.
    pub fn daemon() -> io::Result<Endpoint> {
        #[cfg(windows)]
        {
            let mut tag: String = std::env::var("USERNAME").unwrap_or_default();
            if let Some(dir) = std::env::var_os(crate::paths::DATA_DIR_VAR) {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                dir.hash(&mut h);
                tag = format!("{tag}-{:x}", h.finish());
            }
            let tag: String = tag
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c.to_ascii_lowercase() } else { '_' })
                .collect();
            Ok(Endpoint { id: format!("ferry-{tag}"), guarded: true })
        }
        #[cfg(unix)]
        {
            let path = crate::paths::runtime_dir()?.join("daemon.sock");
            Ok(Endpoint { id: short_socket_path(path).to_string_lossy().into_owned(), guarded: true })
        }
    }

    /// Endpoint riêng cho test. Client không kiểm exe của server.
    pub fn custom(tag: &str) -> Endpoint {
        #[cfg(windows)]
        {
            Endpoint { id: format!("ferry-{tag}"), guarded: false }
        }
        #[cfg(unix)]
        {
            let path = std::env::temp_dir().join(format!("ferry-{tag}.sock"));
            Endpoint { id: path.to_string_lossy().into_owned(), guarded: false }
        }
    }

    /// Như [`Endpoint::custom`] nhưng đi đường kiểm của endpoint thật (thư mục 0700 trên Unix).
    pub fn guarded(tag: &str) -> Endpoint {
        Endpoint { guarded: true, ..Endpoint::custom_dir(tag) }
    }

    fn custom_dir(tag: &str) -> Endpoint {
        #[cfg(windows)]
        {
            Endpoint::custom(tag)
        }
        #[cfg(unix)]
        {
            let path = std::env::temp_dir().join(format!("ferry-{tag}")).join("daemon.sock");
            Endpoint { id: path.to_string_lossy().into_owned(), guarded: false }
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> io::Result<Name<'static>> {
        #[cfg(windows)]
        {
            use interprocess::local_socket::GenericNamespaced;
            self.id.clone().to_ns_name::<GenericNamespaced>()
        }
        #[cfg(unix)]
        {
            use interprocess::local_socket::GenericFilePath;
            std::path::PathBuf::from(&self.id).to_fs_name::<GenericFilePath>()
        }
    }

    /// Mở server, chỉ người dùng hiện tại nối được. Windows: instance đầu tiên + DACL một người; tiến trình khác giữ tên
    /// trước ⇒ `AddrInUse`. Unix: xoá tệp socket cũ còn sót — chỉ gọi khi đã giữ khoá một-instance.
    pub fn listen(&self) -> io::Result<interprocess::local_socket::Listener> {
        #[cfg(unix)]
        {
            if self.guarded {
                sys::prepare_private_dir(&self.id)?;
            }
            if let Err(e) = std::fs::remove_file(&self.id)
                && e.kind() != io::ErrorKind::NotFound
            {
                return Err(e);
            }
        }
        let options = ListenerOptions::new().name(self.name()?);
        #[cfg(windows)]
        let options = {
            use interprocess::os::windows::local_socket::ListenerOptionsExt;
            options.security_descriptor(sys::owner_only_descriptor()?)
        };
        let listener = options.create_sync().map_err(|e| sys::listen_error(&self.id, e))?;
        #[cfg(unix)]
        sys::restrict_socket(&self.id)?;
        Ok(listener)
    }

    /// Nối server. Endpoint thật trên Windows: server phải chạy từ cùng thư mục với exe này, không thì `PermissionDenied`
    /// trước khi gửi byte nào.
    pub fn connect<Out: Serialize, In: DeserializeOwned>(&self) -> io::Result<Connection<Out, In>> {
        let stream = Stream::connect(self.name()?)?;
        #[cfg(windows)]
        if self.guarded {
            sys::check_server(stream.peer_creds()?.pid())?;
        }
        Ok(Connection::new(stream))
    }
}

/// `sockaddr_un.sun_path` chỉ 104 byte (macOS) / 108 byte (Linux), tính cả NUL: thư mục dữ liệu dài (tên người dùng dài,
/// `FERRY_DATA_DIR` sâu) thì `bind` hỏng. Quá trần ⇒ `<thư mục tạm>/ferry-<băm đường dẫn gốc>/daemon.sock` — thư mục tạm của
/// macOS đã riêng từng người dùng; Linux `/tmp` chung nhưng thư mục con 0700 (`prepare_private_dir`), của người khác thì
/// đổi quyền hỏng ⇒ lỗi, không chạy mù. Băm theo đường dẫn gốc để hai bản dev khác `FERRY_DATA_DIR` không chung socket.
#[cfg(unix)]
fn short_socket_path(path: std::path::PathBuf) -> std::path::PathBuf {
    const MAX: usize = 100;
    if path.as_os_str().len() <= MAX {
        return path;
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    std::env::temp_dir().join(format!("ferry-{:016x}", h.finish())).join("daemon.sock")
}

/// Kết nối phía client: gửi [`UiToDaemon`], nhận [`DaemonToUi`].
pub type ClientConnection = Connection<UiToDaemon, DaemonToUi>;
/// Kết nối phía daemon: gửi [`DaemonToUi`], nhận [`UiToDaemon`].
pub type ServerConnection = Connection<DaemonToUi, UiToDaemon>;

/// Một kết nối hai chiều đã bọc đệm đọc, có kiểu theo chiều. Đọc ở một luồng; ghi được từ luồng khác qua
/// [`Connection::sender`] (daemon đẩy thông điệp trong lúc luồng kết nối chờ đọc).
pub struct Connection<Out, In> {
    reader: BufReader<SharedStream>,
    sender: Sender<Out>,
    _in: PhantomData<fn() -> In>,
}

#[derive(Clone)]
struct SharedStream(Arc<Stream>);

impl Read for SharedStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self.0).read(buf)
    }
}

impl Write for SharedStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self.0).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self.0).flush()
    }
}

/// Đầu ghi của một [`Connection`], nhân bản được sang luồng khác; mỗi thông điệp ghi liền một mạch dưới khoá.
pub struct Sender<Out>(Arc<Mutex<SharedStream>>, PhantomData<fn(Out)>);

impl<Out> Clone for Sender<Out> {
    fn clone(&self) -> Self {
        Sender(self.0.clone(), PhantomData)
    }
}

impl<Out: Serialize> Sender<Out> {
    pub fn send(&self, msg: &Out) -> io::Result<()> {
        let mut w = self.0.lock().map_err(|_| io::Error::other("ipc sender poisoned"))?;
        send(&mut *w, msg)
    }
}

impl<Out: Serialize, In: DeserializeOwned> Connection<Out, In> {
    pub fn new(stream: Stream) -> Self {
        let stream = SharedStream(Arc::new(stream));
        Connection {
            reader: BufReader::new(stream.clone()),
            sender: Sender(Arc::new(Mutex::new(stream)), PhantomData),
            _in: PhantomData,
        }
    }

    pub fn send(&mut self, msg: &Out) -> io::Result<()> {
        self.sender.send(msg)
    }

    pub fn sender(&self) -> Sender<Out> {
        self.sender.clone()
    }

    pub fn recv(&mut self) -> io::Result<Option<In>> {
        recv(&mut self.reader)
    }

    /// Gửi rồi chờ đúng một thông điệp.
    pub fn request(&mut self, msg: &Out) -> io::Result<In> {
        self.send(msg)?;
        self.recv()?.ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "peer closed the connection"))
    }
}

impl ClientConnection {
    /// Nối daemon thật rồi chào ([`ClientConnection::hello`]).
    pub fn open(role: Role) -> io::Result<ClientConnection> {
        let mut conn = Endpoint::daemon()?.connect()?;
        conn.hello(role)?;
        Ok(conn)
    }

    /// Mở đầu phiên: `Hello` + chờ `Ok`.
    pub fn hello(&mut self, role: Role) -> io::Result<()> {
        let hello = UiToDaemon::Hello { role, pid: std::process::id(), protocol: PROTOCOL_VERSION };
        match self.request(&hello)? {
            DaemonToUi::Ok => Ok(()),
            other => Err(io::Error::other(format!("unexpected reply to hello: {other:?}"))),
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::Path;
    use std::ptr;

    use interprocess::os::windows::security_descriptor::{
        AsSecurityDescriptorExt, BorrowedSecurityDescriptor, SecurityDescriptor,
    };
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_PIPE_BUSY, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    /// `CreateNamedPipeW` với `FILE_FLAG_FIRST_PIPE_INSTANCE` khi tên đã có chủ.
    pub fn listen_error(id: &str, e: io::Error) -> io::Error {
        match e.raw_os_error().map(|c| c as u32) {
            Some(ERROR_ACCESS_DENIED | ERROR_PIPE_BUSY) => {
                io::Error::new(io::ErrorKind::AddrInUse, format!("ipc endpoint {id} is held by another process"))
            }
            _ => e,
        }
    }

    fn current_user_sid() -> io::Result<String> {
        let mut token = ptr::null_mut();
        // SAFETY: tham số ra hợp lệ; `OwnedHandle` đóng handle token.
        let token = unsafe {
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            OwnedHandle::from_raw_handle(token)
        };
        let mut size = 0;
        // SAFETY: lần đầu chỉ hỏi cỡ (đệm rỗng); lần sau đệm `u64` đủ cỡ, căn hơn `TOKEN_USER`.
        let mut buf = unsafe {
            GetTokenInformation(token.as_raw_handle(), TokenUser, ptr::null_mut(), 0, &mut size);
            vec![0u64; (size as usize).div_ceil(8)]
        };
        // SAFETY: như trên.
        let ok =
            unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, buf.as_mut_ptr().cast(), size, &mut size) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut text = ptr::null_mut();
        // SAFETY: `buf` là `TOKEN_USER` OS vừa ghi; `text` cấp bằng `LocalAlloc`, kết thúc bằng NUL.
        unsafe {
            let sid = (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid;
            if ConvertSidToStringSidW(sid, &mut text) == 0 {
                return Err(io::Error::last_os_error());
            }
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text.cast());
            Ok(sid)
        }
    }

    /// DACL có bảo vệ (không thừa kế), một ACE: toàn quyền cho người dùng hiện tại.
    pub fn owner_only_descriptor() -> io::Result<SecurityDescriptor> {
        let sddl = format!("D:P(A;;GA;;;{})", current_user_sid()?);
        let wide: Vec<u16> = std::ffi::OsStr::new(&sddl).encode_wide().chain([0]).collect();
        let mut raw = ptr::null_mut();
        // SAFETY: `wide` kết thúc bằng NUL; `raw` cấp bằng `LocalAlloc`, chép xong thì trả.
        unsafe {
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut raw,
                ptr::null_mut(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            let owned = BorrowedSecurityDescriptor::from_ptr(raw).to_owned_sd();
            LocalFree(raw);
            owned
        }
    }

    /// Exe của tiến trình `pid`. Không đọc được ⇒ lỗi: bên gọi từ chối, không bỏ qua.
    fn process_image(pid: u32) -> io::Result<OsString> {
        // SAFETY: handle hợp lệ khi khác null, `OwnedHandle` đóng; đệm + độ dài đúng cặp.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let handle = OwnedHandle::from_raw_handle(handle);
            let mut buf = vec![0u16; 32 * 1024];
            let mut len = buf.len() as u32;
            if QueryFullProcessImageNameW(handle.as_raw_handle(), PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(OsString::from_wide(&buf[..len as usize]))
        }
    }

    /// Cùng thư mục sau khi giải đường dẫn thật. Giải không được ⇒ `false`.
    fn same_directory(a: &Path, b: &Path) -> bool {
        match (a.parent().map(Path::canonicalize), b.parent().map(Path::canonicalize)) {
            (Some(Ok(a)), Some(Ok(b))) => a == b,
            _ => false,
        }
    }

    pub fn check_server(pid: Option<u32>) -> io::Result<()> {
        let denied = |message: String| io::Error::new(io::ErrorKind::PermissionDenied, message);
        let pid = pid.ok_or_else(|| denied("ipc server has no pid".into()))?;
        let server = process_image(pid).map_err(|e| denied(format!("ipc server pid {pid}: {e}")))?;
        if same_directory(&std::env::current_exe()?, Path::new(&server)) {
            Ok(())
        } else {
            // Không ghi đường dẫn: có tên người dùng, và lỗi này vào log.
            Err(denied(format!("ipc server pid {pid} runs from another directory")))
        }
    }
}

#[cfg(unix)]
mod sys {
    use std::io;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::path::Path;

    /// Thư mục chứa socket: 0700, thuộc người dùng này.
    pub fn prepare_private_dir(socket: &str) -> io::Result<()> {
        let Some(dir) = Path::new(socket).parent() else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "socket path has no parent"));
        };
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }

    /// `ListenerOptionsExt::mode` của `interprocess` không có trên macOS ⇒ chmod sau `bind`.
    pub fn restrict_socket(socket: &str) -> io::Result<()> {
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
    }

    pub fn listen_error(_id: &str, e: io::Error) -> io::Error {
        e
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn wire_format_is_tagged_snake_case_with_camel_case_fields() {
        let hello = UiToDaemon::Hello { role: Role::Ui, pid: 7, protocol: 1 };
        assert_eq!(serde_json::to_string(&hello).unwrap(), r#"{"type":"hello","role":"ui","pid":7,"protocol":1}"#);
        assert_eq!(serde_json::to_string(&UiToDaemon::Focus).unwrap(), r#"{"type":"focus"}"#);
        assert_eq!(serde_json::to_string(&DaemonToUi::Focus).unwrap(), r#"{"type":"focus"}"#);
        assert_eq!(serde_json::to_string(&DaemonToUi::Ok).unwrap(), r#"{"type":"ok"}"#);
    }

    /// Mẫu dây của các lệnh / trả lời gd1_07 — cửa sổ (gd1_08) và điện thoại giả của test đọc đúng dạng này.
    #[test]
    fn command_and_reply_wire_format() {
        let dev = DeviceId("3f9a".into());
        let cases: Vec<(UiToDaemon, &str)> = vec![
            (
                UiToDaemon::Queue {
                    req: 1,
                    op: QueueOp::AddFiles {
                        device: dev.clone(),
                        paths: vec!["/v/a.mp4".into()],
                        dest_dir: "Trips".into(),
                    },
                },
                r#"{"type":"queue","req":1,"op":{"op":"add_files","device":"3f9a","paths":["/v/a.mp4"],"destDir":"Trips"}}"#,
            ),
            (
                UiToDaemon::Queue { req: 2, op: QueueOp::Item { id: 5, action: Action::Retry } },
                r#"{"type":"queue","req":2,"op":{"op":"item","id":5,"action":"retry"}}"#,
            ),
            (
                UiToDaemon::Queue { req: 3, op: QueueOp::All { action: Action::Pause } },
                r#"{"type":"queue","req":3,"op":{"op":"all","action":"pause"}}"#,
            ),
            (
                UiToDaemon::Phone { req: 4, op: PhoneOp::Pair { device: dev.clone(), pin: Pin("123456".into()) } },
                r#"{"type":"phone","req":4,"op":{"op":"pair","device":"3f9a","pin":"123456"}}"#,
            ),
            (
                UiToDaemon::Phone { req: 5, op: PhoneOp::AddAddress { address: "[fe80::1%en0]:8080".into() } },
                r#"{"type":"phone","req":5,"op":{"op":"add_address","address":"[fe80::1%en0]:8080"}}"#,
            ),
            (
                UiToDaemon::Library { req: 6, op: LibraryOp::List { device: dev.clone() } },
                r#"{"type":"library","req":6,"op":{"op":"list","device":"3f9a"}}"#,
            ),
        ];
        for (msg, wire) in cases {
            assert_eq!(serde_json::to_string(&msg).unwrap(), wire);
            assert_eq!(serde_json::from_str::<UiToDaemon>(wire).unwrap(), msg);
        }
        let replies: Vec<(DaemonToUi, &str)> = vec![
            (
                DaemonToUi::Reply { req: 1, outcome: Outcome::Done },
                r#"{"type":"reply","req":1,"outcome":{"kind":"done"}}"#,
            ),
            (
                DaemonToUi::Reply {
                    req: 4,
                    outcome: Outcome::Failed { error: CommandError::RateLimited { retry_after_secs: 30 } },
                },
                r#"{"type":"reply","req":4,"outcome":{"kind":"failed","error":{"code":"rate_limited","retryAfterSecs":30}}}"#,
            ),
            (
                DaemonToUi::Progress(ProgressDelta {
                    items: vec![ItemProgress { id: 5, sent: 10, size: 20, bytes_per_sec: 3 }],
                }),
                r#"{"type":"progress","items":[{"id":5,"sent":10,"size":20,"bytesPerSec":3}]}"#,
            ),
        ];
        for (msg, wire) in replies {
            assert_eq!(serde_json::to_string(&msg).unwrap(), wire);
            assert_eq!(serde_json::from_str::<DaemonToUi>(wire).unwrap(), msg);
        }
        assert_eq!(format!("{:?}", Pin("123456".into())), "Pin(***)", "PIN never reaches the log");
    }

    #[test]
    fn snapshot_round_trips() {
        let snap = Snapshot {
            devices: vec![DeviceView {
                id: DeviceId("3f9a".into()),
                name: "Pixel".into(),
                state: DeviceState::Online,
                visible: true,
                newer_api: false,
                features: vec!["dirs".into()],
                address: Some("192.168.1.5:8080".into()),
            }],
            can_pause_all: true,
            ..Snapshot::default()
        };
        let wire = serde_json::to_string(&DaemonToUi::Snapshot(snap.clone())).unwrap();
        assert!(
            wire.starts_with(r#"{"type":"snapshot","devices":[{"id":"3f9a","name":"Pixel","state":"online""#),
            "{wire}"
        );
        assert!(wire.contains(r#""canPauseAll":true"#) && wire.contains(r#""finishedOmitted":0"#), "{wire}");
        assert_eq!(serde_json::from_str::<DaemonToUi>(&wire).unwrap(), DaemonToUi::Snapshot(snap));
    }

    #[test]
    fn framing_roundtrip() {
        let msgs = [DaemonToUi::Ok, DaemonToUi::Focus, DaemonToUi::Error { message: "dòng có\nxuống dòng".into() }];
        let mut buf = Vec::new();
        for m in &msgs {
            send(&mut buf, m).unwrap();
        }
        assert_eq!(buf.iter().filter(|&&b| b == b'\n').count(), msgs.len(), "one line per message");
        let mut r = Cursor::new(buf);
        for m in &msgs {
            assert_eq!(recv::<DaemonToUi>(&mut r).unwrap().as_ref(), Some(m));
        }
        assert_eq!(recv::<DaemonToUi>(&mut r).unwrap(), None);
    }

    #[test]
    fn unknown_message_is_invalid_data_and_stream_continues() {
        let mut r = Cursor::new(b"{\"type\":\"from_the_future\"}\n{\"type\":\"focus\"}\n".to_vec());
        assert_eq!(recv::<UiToDaemon>(&mut r).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(recv::<UiToDaemon>(&mut r).unwrap(), Some(UiToDaemon::Focus));
    }

    #[test]
    fn oversized_line_is_rejected() {
        let mut r = Cursor::new(vec![b'x'; MAX_LINE as usize + 10]);
        assert_eq!(recv::<UiToDaemon>(&mut r).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn long_socket_path_falls_back_to_temp_dir() {
        let short = std::path::PathBuf::from("/run/user/1000/ferry/daemon.sock");
        assert_eq!(short_socket_path(short.clone()), short);
        let long = std::path::PathBuf::from(format!("/{}/daemon.sock", "x".repeat(120)));
        let a = short_socket_path(long.clone());
        assert!(a.as_os_str().len() <= 100 || a.starts_with(std::env::temp_dir()));
        assert!(a.ends_with("daemon.sock") && a != long);
        assert_eq!(a, short_socket_path(long), "stable");
        let other = short_socket_path(std::path::PathBuf::from(format!("/{}/daemon.sock", "y".repeat(120))));
        assert_ne!(a, other, "different data dirs never share a socket");
    }

    #[cfg(unix)]
    #[test]
    fn guarded_socket_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let ep = Endpoint::guarded(&format!("test-private-{}", std::process::id()));
        let _listener = ep.listen().unwrap();
        let path = std::path::Path::new(ep.id());
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    /// Chào, rồi daemon đẩy `Focus` từ luồng khác trong lúc luồng kết nối đang chờ đọc.
    #[test]
    fn hello_then_server_pushes_from_another_thread() {
        let ep = Endpoint::custom(&format!("test-{}", std::process::id()));
        let listener = ep.listen().unwrap();
        let server = std::thread::spawn(move || {
            let mut conn = ServerConnection::new(listener.accept().unwrap());
            assert!(matches!(conn.recv().unwrap(), Some(UiToDaemon::Hello { role: Role::Ui, .. })));
            conn.send(&DaemonToUi::Ok).unwrap();
            let sender = conn.sender();
            let pusher = std::thread::spawn(move || sender.send(&DaemonToUi::Focus).unwrap());
            assert_eq!(conn.recv().unwrap(), Some(UiToDaemon::Focus));
            pusher.join().unwrap();
            assert_eq!(conn.recv().unwrap(), None, "client hung up");
        });
        let mut client: ClientConnection = ep.connect().unwrap();
        client.hello(Role::Ui).unwrap();
        assert_eq!(client.recv().unwrap(), Some(DaemonToUi::Focus));
        client.send(&UiToDaemon::Focus).unwrap();
        drop(client);
        server.join().unwrap();
    }
}
