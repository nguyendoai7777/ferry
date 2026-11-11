//! Ghép đôi bằng PIN, token lâu dài trong kho khoá OS (`PROTOCOL.md` 6.3, 2.2) — task gd1_06.
//!
//! - [`TokenStore`]: nơi cất token — [`OsKeyring`] (Keychain macOS, Credential Manager Windows, Secret Service Linux
//!   qua `zbus`, thuần Rust) hoặc [`MemoryKeyring`] (test). Khoá: dịch vụ [`KEYRING_SERVICE`], tài khoản = `id` điện
//!   thoại.
//! - [`Tokens`]: kho khoá + bộ đệm trong RAM cho daemon — `discovery::connect` / `Link` lấy token ở đây, không đọc kho
//!   khoá mỗi lần thử `info`.
//! - [`pair`]: `POST /api/v1/pair` ⇒ token vào kho khoá ⇒ client mang token. Ghép lại ⇒ token mới đè token cũ.
//! - [`forget`]: `DELETE /api/v1/pair` nếu còn nối được (lỗi mạng vẫn quên phía PC) ⇒ xoá token.
//!
//! Token **không bao giờ** vào `config.json`, log, IPC: kiểu [`Secret`] (`Debug` che, không `Serialize`), lỗi kho khoá
//! chỉ mang thông điệp của OS. Config chỉ giữ `id`, tên, địa chỉ (`Config::remember_device`).
//!
//! Gọi kho khoá là I/O chặn (Secret Service: D-Bus; Keychain có thể chờ mở khoá) ⇒ hàm async ở đây chạy nó trên
//! `spawn_blocking`.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::client::{ClientError, PhoneClient};
use crate::device::DeviceId;
use crate::protocol::Secret;

/// Tên dịch vụ trong kho khoá. Bản debug (`start.sh`, dữ liệu dev riêng) dùng tên khác để không đè token của bản
/// đã cài (hai bản có `clientId` khác nhau ⇒ hai token khác nhau trên cùng một điện thoại).
pub const KEYRING_SERVICE: &str =
    if cfg!(debug_assertions) { "media-wireless-sync.dev" } else { "media-wireless-sync" };

/// Chờ `DELETE /pair` khi quên điện thoại tối đa chừng này rồi quên phía PC luôn.
pub const UNPAIR_TIMEOUT: Duration = Duration::from_secs(5);

/// Lỗi kho khoá (không mang token).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyringError {
    /// Không có kho khoá dùng được (Linux không chạy Secret Service: máy không desktop, phiên SSH).
    Unavailable(String),
    /// Kho khoá từ chối / bị khoá / lỗi OS khác.
    Platform(String),
}

impl fmt::Display for KeyringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "no credential store: {m}"),
            Self::Platform(m) => write!(f, "credential store: {m}"),
        }
    }
}

impl std::error::Error for KeyringError {}

/// Nơi cất token theo `id` điện thoại. Hàm chặn — gọi từ `spawn_blocking` hoặc qua [`Tokens`].
pub trait TokenStore: Send + Sync + 'static {
    /// `None` ⇒ không có (chưa ghép, hoặc người dùng đã xoá trong kho khoá).
    fn get(&self, device: &DeviceId) -> Result<Option<Secret>, KeyringError>;
    /// Ghi đè nếu đã có.
    fn set(&self, device: &DeviceId, token: &Secret) -> Result<(), KeyringError>;
    /// Không có thì coi như xong.
    fn delete(&self, device: &DeviceId) -> Result<(), KeyringError>;
}

/// Kho khoá của OS (crate `keyring`, bản `v1`).
#[derive(Debug, Default, Clone, Copy)]
pub struct OsKeyring;

impl OsKeyring {
    fn entry(device: &DeviceId) -> Result<keyring::Entry, KeyringError> {
        keyring::Entry::new(KEYRING_SERVICE, &device.0).map_err(map_keyring)
    }
}

fn map_keyring(e: keyring::Error) -> KeyringError {
    match e {
        keyring::Error::NoDefaultStore | keyring::Error::NoStorageAccess(_) => KeyringError::Unavailable(e.to_string()),
        other => KeyringError::Platform(other.to_string()),
    }
}

impl TokenStore for OsKeyring {
    fn get(&self, device: &DeviceId) -> Result<Option<Secret>, KeyringError> {
        match Self::entry(device)?.get_password() {
            Ok(t) => Ok(Some(Secret::new(t))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_keyring(e)),
        }
    }

    fn set(&self, device: &DeviceId, token: &Secret) -> Result<(), KeyringError> {
        Self::entry(device)?.set_password(token.expose()).map_err(map_keyring)
    }

    fn delete(&self, device: &DeviceId) -> Result<(), KeyringError> {
        match Self::entry(device)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_keyring(e)),
        }
    }
}

/// Kho khoá giả trong RAM (test, và nơi không có kho khoá khi chạy thử).
#[derive(Debug, Default)]
pub struct MemoryKeyring {
    tokens: Mutex<HashMap<DeviceId, Secret>>,
    fail: Mutex<Option<KeyringError>>,
}

impl MemoryKeyring {
    pub fn new() -> Self {
        Self::default()
    }

    /// Số token đang giữ (test soát xoá).
    pub fn len(&self) -> usize {
        self.tokens.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Mọi lệnh sau trả lỗi này (`None` ⇒ hết lỗi) — thử nhánh kho khoá hỏng.
    pub fn fail_with(&self, err: Option<KeyringError>) {
        *self.fail.lock().unwrap() = err;
    }

    fn check(&self) -> Result<(), KeyringError> {
        self.fail.lock().unwrap().clone().map_or(Ok(()), Err)
    }
}

impl TokenStore for MemoryKeyring {
    fn get(&self, device: &DeviceId) -> Result<Option<Secret>, KeyringError> {
        self.check()?;
        Ok(self.tokens.lock().unwrap().get(device).cloned())
    }

    fn set(&self, device: &DeviceId, token: &Secret) -> Result<(), KeyringError> {
        self.check()?;
        self.tokens.lock().unwrap().insert(device.clone(), token.clone());
        Ok(())
    }

    fn delete(&self, device: &DeviceId) -> Result<(), KeyringError> {
        self.check()?;
        self.tokens.lock().unwrap().remove(device);
        Ok(())
    }
}

/// Kho khoá + bộ đệm RAM của daemon. Clone rẻ (chia sẻ).
#[derive(Clone)]
pub struct Tokens {
    store: Arc<dyn TokenStore>,
    /// `Some(None)` = đã hỏi kho khoá, không có token (khỏi hỏi lại mỗi lần thử `info`).
    cache: Arc<Mutex<HashMap<DeviceId, Option<Secret>>>>,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cached = self.cache.lock().map(|c| c.len()).unwrap_or(0);
        f.debug_struct("Tokens").field("cached", &cached).finish_non_exhaustive()
    }
}

impl Tokens {
    pub fn new(store: Arc<dyn TokenStore>) -> Self {
        Self { store, cache: Arc::default() }
    }

    /// Kho khoá của OS.
    pub fn os() -> Self {
        Self::new(Arc::new(OsKeyring))
    }

    /// Token của thiết bị (đệm sau lần đọc đầu). `Ok(None)` ⇒ không có token ⇒ thiết bị phải ghép lại
    /// (`Devices::unauthorized`). Lỗi kho khoá không đệm — lần sau thử lại (kho khoá có thể vừa mở khoá).
    pub async fn get(&self, device: &DeviceId) -> Result<Option<Secret>, KeyringError> {
        if let Some(hit) = self.cache.lock().unwrap().get(device) {
            return Ok(hit.clone());
        }
        let store = self.store.clone();
        let id = device.clone();
        let got = blocking(move || store.get(&id)).await?;
        self.cache.lock().unwrap().insert(device.clone(), got.clone());
        Ok(got)
    }

    /// Cất token (ghép / ghép lại).
    pub async fn set(&self, device: &DeviceId, token: &Secret) -> Result<(), KeyringError> {
        let store = self.store.clone();
        let (id, t) = (device.clone(), token.clone());
        blocking(move || store.set(&id, &t)).await?;
        self.cache.lock().unwrap().insert(device.clone(), Some(token.clone()));
        Ok(())
    }

    /// Xoá token (quên). Bộ đệm bỏ dù kho khoá lỗi: phía PC coi như đã quên.
    pub async fn delete(&self, device: &DeviceId) -> Result<(), KeyringError> {
        self.cache.lock().unwrap().insert(device.clone(), None);
        let store = self.store.clone();
        let id = device.clone();
        blocking(move || store.delete(&id)).await
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, KeyringError> + Send + 'static,
) -> Result<T, KeyringError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| KeyringError::Platform(format!("keyring task: {e}")))?
}

/// PC tự giới thiệu với điện thoại (`Config::client_id` / `client_name`).
#[derive(Debug, Clone, Copy)]
pub struct Identity<'a> {
    pub client_id: &'a str,
    pub client_name: &'a str,
}

impl<'a> From<&'a crate::config::Config> for Identity<'a> {
    fn from(c: &'a crate::config::Config) -> Self {
        Self { client_id: &c.client_id, client_name: &c.client_name }
    }
}

/// Ghép xong: điện thoại đã cấp token, token đã nằm trong kho khoá và trong client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    pub id: DeviceId,
    pub name: String,
}

/// Vì sao [`pair`] không xong. UI nói việc cần làm, không nói mã HTTP (`PROTOCOL.md` 2.6).
#[derive(Debug)]
pub enum PairError {
    /// Sai PIN (`403`) ⇒ "PIN không đúng, xem lại trên điện thoại".
    WrongPin,
    /// Sai quá nhiều lần (`429`) ⇒ UI đếm ngược `retry_after` (thiếu ⇒ 60 s) rồi mới cho nhập lại.
    RateLimited { retry_after: Duration },
    /// Điện thoại trả lời là một máy khác `id` đang ghép (IP cấp lại giữa chừng) — token đã bỏ, không cất.
    OtherPhone { got: DeviceId },
    /// Không cất được token ⇒ "Không mở được kho khoá của hệ thống". Điện thoại đã cấp token nhưng PC bỏ nó.
    Keyring(KeyringError),
    /// Lỗi mạng / lỗi khác của client (`Network` ⇒ "Không nối được điện thoại").
    Client(ClientError),
}

impl fmt::Display for PairError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongPin => f.write_str("wrong PIN"),
            Self::RateLimited { retry_after } => write!(f, "too many attempts, retry in {}s", retry_after.as_secs()),
            Self::OtherPhone { got } => write!(f, "address now answers as another phone ({got})"),
            Self::Keyring(e) => write!(f, "{e}"),
            Self::Client(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for PairError {}

/// Điện thoại không gửi `Retry-After` kèm `429` ⇒ đếm ngược chừng này.
pub const DEFAULT_PIN_LOCKOUT: Duration = Duration::from_secs(60);

/// `POST /api/v1/pair` với PIN người dùng đọc trên điện thoại ⇒ token vào kho khoá ⇒ `client` mang token.
///
/// `expected`: `id` của điện thoại người dùng chọn (`None` khi ghép thẳng theo địa chỉ nhập tay). Ghép lại thiết bị
/// `NeedsRepair` ⇒ token mới đè token cũ. Xong ⇒ người gọi ghi `Config::remember_device` + `Devices::paired`.
pub async fn pair(
    client: &mut PhoneClient,
    expected: Option<&DeviceId>,
    pin: &Secret,
    me: Identity<'_>,
    tokens: &Tokens,
) -> Result<Paired, PairError> {
    let resp = client.pair(pin, me.client_id, me.client_name).await.map_err(|e| match e {
        ClientError::WrongPin => PairError::WrongPin,
        ClientError::RateLimited { retry_after } => {
            PairError::RateLimited { retry_after: retry_after.unwrap_or(DEFAULT_PIN_LOCKOUT) }
        }
        other => PairError::Client(other),
    })?;
    if let Some(want) = expected
        && *want != resp.id
    {
        log::warn!("pairing: {} answered as {} instead of {want}", client.addr(), resp.id);
        return Err(PairError::OtherPhone { got: resp.id });
    }
    tokens.set(&resp.id, &resp.token).await.map_err(PairError::Keyring)?;
    client.set_token(Some(resp.token));
    log::info!("pairing: paired with {} ({}) at {}", resp.id, resp.name, client.addr());
    Ok(Paired { id: resp.id, name: resp.name })
}

/// Kết quả của [`forget`] — phía PC luôn coi là đã quên.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forgotten {
    /// Điện thoại đã huỷ token (`DELETE /pair` trả lời). `false` ⇒ Offline / lỗi mạng — điện thoại còn giữ token
    /// tới khi người dùng "Quên mọi máy tính" trên đó (token vô hại: PC đã xoá bản của mình).
    pub phone_notified: bool,
    /// Xoá trong kho khoá hỏng (hiếm) — token còn nằm trong kho khoá; UI không cần báo, log có ghi.
    pub keyring_error: Option<KeyringError>,
}

/// Quên điện thoại: `DELETE /api/v1/pair` qua `online` (client mang token, chỉ khi Online) trong [`UNPAIR_TIMEOUT`],
/// rồi xoá token. Người gọi gỡ `Config::forget_device`, `QueueStore::remove_device` (sau khi dừng runner),
/// `Devices::forget`.
pub async fn forget(device: &DeviceId, online: Option<&PhoneClient>, tokens: &Tokens) -> Forgotten {
    let phone_notified = match online {
        Some(c) => match tokio::time::timeout(UNPAIR_TIMEOUT, c.unpair()).await {
            Ok(Ok(())) => true,
            // Token đã chết phía điện thoại: điện thoại đã quên trước.
            Ok(Err(ClientError::Unauthorized)) => true,
            Ok(Err(e)) => {
                log::info!("pairing: unpair {device} failed ({e}), forgetting on PC only");
                false
            }
            Err(_) => {
                log::info!("pairing: unpair {device} timed out, forgetting on PC only");
                false
            }
        },
        None => false,
    };
    let keyring_error = tokens.delete(device).await.err();
    if let Some(e) = &keyring_error {
        log::warn!("pairing: forget {device}: {e}");
    }
    log::info!("pairing: forgot {device} (phone notified: {phone_notified})");
    Forgotten { phone_notified, keyring_error }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tokens_cache_and_overwrite() {
        let mem = Arc::new(MemoryKeyring::new());
        let tokens = Tokens::new(mem.clone());
        let id = DeviceId("abc".into());
        assert_eq!(tokens.get(&id).await.unwrap(), None);
        tokens.set(&id, &Secret::new("t1")).await.unwrap();
        tokens.set(&id, &Secret::new("t2")).await.unwrap();
        assert_eq!(tokens.get(&id).await.unwrap(), Some(Secret::new("t2")));
        assert_eq!(mem.get(&id).unwrap(), Some(Secret::new("t2")));

        // Lỗi kho khoá không đệm.
        let other = DeviceId("zzz".into());
        mem.fail_with(Some(KeyringError::Unavailable("locked".into())));
        assert!(tokens.get(&other).await.is_err());
        mem.fail_with(None);
        mem.set(&other, &Secret::new("t3")).unwrap();
        assert_eq!(tokens.get(&other).await.unwrap(), Some(Secret::new("t3")));

        tokens.delete(&id).await.unwrap();
        assert_eq!(tokens.get(&id).await.unwrap(), None);
        assert_eq!(mem.len(), 1);
        assert!(!format!("{tokens:?}").contains("t3"));
    }

    #[test]
    #[cfg(debug_assertions)]
    fn dev_build_uses_its_own_service() {
        assert_eq!(KEYRING_SERVICE, "media-wireless-sync.dev");
    }
}
