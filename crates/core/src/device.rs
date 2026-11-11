//! Thiết bị (điện thoại) và máy trạng thái của nó (`PROTOCOL.md` mục 5) — task gd1_05.
//!
//! Thiết bị nhận theo `id` (TXT `id=` / `info.id`), không theo IP; địa chỉ là một **tập** IPv4 + IPv6 đổi được
//! (`PROTOCOL.md` 6.8, W-3).
//!
//! - [`next_state`]: mũi tên của mục 5, hàm thuần.
//! - [`Devices`]: sổ thiết bị của daemon — nhận tin mDNS / kết quả `info` / lỗi nối, phát [`DeviceEvent`]. Không đồng hồ
//!   riêng, không luồng: mọi hàm nhận `now`, [`Devices::next_deadline`] cho biết lúc phải gọi [`Devices::tick`] (rảnh
//!   không giữ CPU — daemon ngủ tới mốc đó, không poll).

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::PairedDevice;
use crate::discovery::address::{self, LocalInterface};
use crate::protocol::{self, Info};

/// `id` của điện thoại (TXT `id=` / `info.id`). Trên dây là chuỗi JSON thường.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Mất mDNS quá chừng này ⇒ `Lost` (`PROTOCOL.md` mục 5).
pub const LOST_AFTER: Duration = Duration::from_secs(30);
/// Thiết bị đã ghép mà không thấy qua mDNS (mạng chặn multicast, địa chỉ nhập tay): thử `info` theo địa chỉ đã lưu
/// thưa cỡ này — không poll dày.
pub const PROBE_INTERVAL: Duration = Duration::from_secs(60);

/// Trạng thái thiết bị (`PROTOCOL.md` mục 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    /// Chưa biết (chưa thấy, chưa ghép) — thiết bị ở trạng thái này không nằm trong sổ.
    Unknown,
    /// Thấy qua mDNS / nhập tay, chưa có token ⇒ hiện ở "Thêm điện thoại".
    Discovered,
    /// Có token (vừa ghép, hoặc nạp từ config lúc khởi động), chưa xác nhận bằng `info`.
    Paired,
    /// `info` với token trả `paired: true` — gửi được.
    Online,
    /// Có token nhưng không nối được / mất mDNS — bình thường (điện thoại chưa bật Sync WiFi), hàng đợi chờ.
    Offline,
    /// Token chết (`paired: false` / `401`): điện thoại đã "Quên máy tính" ⇒ phải ghép lại.
    NeedsRepair,
}

impl DeviceState {
    pub fn has_token(self) -> bool {
        matches!(self, Self::Paired | Self::Online | Self::Offline | Self::NeedsRepair)
    }
}

/// Đầu vào của máy trạng thái.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceInput {
    /// mDNS thấy (lần đầu, hoặc thấy lại sau `Lost`).
    Seen,
    /// Mất mDNS [`LOST_AFTER`].
    Lost,
    /// `POST /pair` trả token (gd1_06).
    PinAccepted,
    /// `GET /info` trả lời; `paired` = token còn sống.
    InfoOk { paired: bool },
    /// Một request bất kỳ nhận `401`.
    Unauthorized,
    /// Mọi địa chỉ đều không nối được.
    ConnectFailed,
    /// Người dùng "Quên điện thoại" trên PC (token đã xoá).
    Forgotten,
}

/// Mũi tên của `PROTOCOL.md` mục 5. Ca không có mũi tên ⇒ giữ nguyên.
///
/// - "thấy lại → Online" đi qua `info`: `Seen` ở `Offline` không đổi trạng thái — sổ đánh dấu cần thử `info` ngay
///   ([`Devices::due_probes`]), `InfoOk { paired: true }` mới đưa về `Online` (khỏi báo Online cho token đã chết).
/// - `Paired` (chưa xác nhận) cũng đi `Offline` / `NeedsRepair` như `Online`: nạp từ config mà điện thoại đang tắt
///   hoặc đã quên PC.
/// - `NeedsRepair` chỉ thoát bằng ghép lại (`PinAccepted`) hoặc quên.
pub fn next_state(state: DeviceState, input: DeviceInput) -> DeviceState {
    use DeviceInput as I;
    use DeviceState as S;
    match (state, input) {
        (S::Unknown, I::Seen) => S::Discovered,
        (_, I::PinAccepted) => S::Paired,
        (S::Paired | S::Offline, I::InfoOk { paired: true }) => S::Online,
        (S::Paired | S::Online | S::Offline, I::InfoOk { paired: false } | I::Unauthorized) => S::NeedsRepair,
        (S::Paired | S::Online, I::Lost | I::ConnectFailed) => S::Offline,
        (S::Discovered, I::Lost) => S::Unknown,
        (S::Paired | S::Online | S::Offline | S::NeedsRepair, I::Forgotten) => S::Discovered,
        (s, _) => s,
    }
}

/// Sự kiện ra kênh của daemon (snapshot cho UI, đánh thức runner).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    /// Thấy qua mDNS (lần đầu hoặc sau `Lost`).
    Seen {
        id: DeviceId,
        name: String,
    },
    /// Mất mDNS [`LOST_AFTER`].
    Lost {
        id: DeviceId,
    },
    /// Tập địa chỉ đổi (IP mới — cập nhật, không hỏi). Đã xếp theo thứ tự thử.
    AddressChanged {
        id: DeviceId,
        addresses: Vec<SocketAddr>,
    },
    StateChanged {
        id: DeviceId,
        from: DeviceState,
        to: DeviceState,
    },
}

/// Một bản quảng bá mDNS đã đọc (`discovery::mdns`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// Tên đầy đủ của instance (`Pixel 8._vpsync._tcp.local.`) — `ServiceRemoved` chỉ mang tên này.
    pub fullname: String,
    pub id: DeviceId,
    /// Tên instance (tên điện thoại, có thể có hậu tố ` (2)`).
    pub name: String,
    /// TXT `api`; thiếu ⇒ 1.
    pub api: u32,
    /// A + AAAA + cổng SRV; link-local đã có `scope_id` của interface nhận.
    pub addresses: Vec<SocketAddr>,
}

/// Một thiết bị trong sổ.
#[derive(Debug, Clone)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub state: DeviceState,
    /// `api` của điện thoại lớn hơn bản PC hiểu ⇒ UI nhắc "cập nhật app trên máy tính"; vẫn gọi `info`.
    pub newer_api: bool,
    /// `info.features` lần cuối (vd `dirs`).
    pub features: Vec<String>,
    /// Tên người dùng đặt trên PC (cài đặt, gd1_12) — thắng `name` khi hiển thị, `info.name` không đè.
    pub alias: Option<String>,
    /// Địa chỉ biết được (mDNS, `info`, nhập tay, config), chưa xếp.
    known: Vec<SocketAddr>,
    /// Địa chỉ vừa nối được — thử trước tới khi lỗi (`PROTOCOL.md` 6.8).
    winner: Option<SocketAddr>,
    /// Instance mDNS đang thấy (`fullname`) — `None` khi chưa thấy qua mDNS.
    mdns: Option<String>,
    /// mDNS gỡ instance lúc này; quá [`LOST_AFTER`] mà không thấy lại ⇒ `Lost`.
    withdrawn_at: Option<Instant>,
    /// Lần thử `info` kế (thiết bị có token mà chưa `Online`).
    next_probe: Option<Instant>,
}

impl Device {
    fn new(id: DeviceId, name: String, state: DeviceState) -> Self {
        Self {
            id,
            name,
            state,
            newer_api: false,
            features: Vec::new(),
            alias: None,
            known: Vec::new(),
            winner: None,
            mdns: None,
            withdrawn_at: None,
            next_probe: None,
        }
    }

    /// Đang thấy qua mDNS (kể cả trong 30 s sau khi bị gỡ).
    pub fn visible(&self) -> bool {
        self.mdns.is_some()
    }

    pub fn winner(&self) -> Option<SocketAddr> {
        self.winner
    }

    /// Tên hiện cho người dùng: bí danh đặt trên PC, không thì tên điện thoại báo.
    pub fn display_name(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

/// Sổ thiết bị của daemon.
#[derive(Debug, Default)]
pub struct Devices {
    devices: BTreeMap<DeviceId, Device>,
    locals: Vec<LocalInterface>,
}

impl Devices {
    /// Nạp thiết bị đã ghép từ config (token ở kho khoá — gd1_06): trạng thái `Paired`, thử `info` ngay.
    pub fn from_config(paired: &[PairedDevice], locals: Vec<LocalInterface>, now: Instant) -> Self {
        let mut me = Self { devices: BTreeMap::new(), locals };
        for p in paired {
            let mut d = Device::new(p.id.clone(), p.name.clone(), DeviceState::Paired);
            d.alias = p.alias.clone().filter(|a| !a.trim().is_empty());
            for a in &p.addresses {
                match address::parse_address(a, &me.locals) {
                    Ok(addr) => d.known.push(addr),
                    Err(e) => log::warn!("device {}: skip saved address {a:?}: {e}", p.id),
                }
            }
            d.next_probe = Some(now);
            me.devices.insert(d.id.clone(), d);
        }
        me
    }

    /// Thiết bị đã ghép, dạng lưu config (địa chỉ đã xếp, link-local ghi tên interface).
    pub fn to_config(&self) -> Vec<PairedDevice> {
        self.devices
            .values()
            .filter(|d| d.state.has_token())
            .map(|d| PairedDevice {
                id: d.id.clone(),
                name: d.name.clone(),
                addresses: self.addresses(&d.id).iter().map(|a| address::format_address(a, &self.locals)).collect(),
                alias: d.alias.clone(),
            })
            .collect()
    }

    /// Interface của PC đổi (daemon đọc lại khi mạng đổi) ⇒ thứ tự địa chỉ đổi theo.
    pub fn set_local_interfaces(&mut self, locals: Vec<LocalInterface>) -> Vec<DeviceEvent> {
        let before: Vec<_> = self.devices.keys().map(|id| (id.clone(), self.addresses(id))).collect();
        self.locals = locals;
        before
            .into_iter()
            .filter_map(|(id, old)| {
                let new = self.addresses(&id);
                (new != old).then_some(DeviceEvent::AddressChanged { id, addresses: new })
            })
            .collect()
    }

    /// Đặt / bỏ bí danh (chuỗi trắng ⇒ bỏ). `false` ⇒ không có thiết bị này.
    pub fn set_alias(&mut self, id: &DeviceId, alias: Option<String>) -> bool {
        let Some(d) = self.devices.get_mut(id) else { return false };
        d.alias = alias.map(|a| a.trim().to_owned()).filter(|a| !a.is_empty());
        true
    }

    pub fn get(&self, id: &DeviceId) -> Option<&Device> {
        self.devices.get(id)
    }

    /// `new` vừa ghép mà trùng một máy đã ghép khác: cùng tên điện thoại, chung ít nhất một địa chỉ, máy cũ không Online
    /// (địa chỉ đó giờ trả lời bằng mã của `new`). Đó là cùng điện thoại đã cài lại app / xoá dữ liệu (đổi `id`) — người
    /// gọi thay máy cũ bằng `new` thay vì để hai dòng. Hai điện thoại thật cùng tên cùng IP không thể Online cùng lúc.
    pub fn superseded_by(&self, new: &DeviceId) -> Option<DeviceId> {
        let n = self.devices.get(new)?;
        self.devices
            .values()
            .find(|d| {
                d.id != *new
                    && d.state.has_token()
                    && d.state != DeviceState::Online
                    && d.name == n.name
                    && d.known.iter().any(|a| n.known.contains(a))
            })
            .map(|d| d.id.clone())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Device> {
        self.devices.values()
    }

    /// Địa chỉ theo thứ tự thử: địa chỉ thắng lần trước, rồi thứ tự `PROTOCOL.md` 6.2.
    pub fn addresses(&self, id: &DeviceId) -> Vec<SocketAddr> {
        let Some(d) = self.devices.get(id) else { return Vec::new() };
        let mut out = address::order_addresses(&d.known, &self.locals);
        if let Some(w) = d.winner
            && let Some(pos) = out.iter().position(|a| *a == w)
        {
            out.remove(pos);
            out.insert(0, w);
        }
        out
    }

    /// mDNS thấy / làm mới một instance.
    pub fn announce(&mut self, ann: Announcement, now: Instant) -> Vec<DeviceEvent> {
        let mut ev = Vec::new();
        let id = ann.id.clone();
        // Instance cũ của tên này thuộc id khác (cài lại app ⇒ id mới): gỡ khỏi máy cũ.
        let stale: Vec<DeviceId> = self
            .devices
            .values()
            .filter(|d| d.id != id && d.mdns.as_deref() == Some(ann.fullname.as_str()))
            .map(|d| d.id.clone())
            .collect();
        for other in stale {
            if let Some(d) = self.devices.get_mut(&other) {
                d.mdns = None;
                d.withdrawn_at = None;
            }
            ev.push(DeviceEvent::Lost { id: other.clone() });
            self.apply(&other, DeviceInput::Lost, &mut ev);
        }

        let before = self.addresses(&id);
        let d = self
            .devices
            .entry(id.clone())
            .or_insert_with(|| Device::new(id.clone(), ann.name.clone(), DeviceState::Unknown));
        let first_sight = d.mdns.is_none();
        d.mdns = Some(ann.fullname);
        d.withdrawn_at = None;
        d.newer_api = ann.api > protocol::API_VERSION;
        if !d.state.has_token() || d.name.is_empty() {
            d.name = ann.name.clone();
        }
        for a in ann.addresses {
            if !d.known.contains(&a) {
                d.known.push(a);
            }
        }
        if first_sight {
            ev.push(DeviceEvent::Seen { id: id.clone(), name: d.name.clone() });
            // Có token mà chưa Online ⇒ thử `info` ngay ("thấy lại → Online").
            if d.state.has_token() && d.state != DeviceState::Online {
                d.next_probe = Some(now);
            }
        }
        self.apply(&id, DeviceInput::Seen, &mut ev);
        self.push_address_change(&id, before, &mut ev);
        ev
    }

    /// mDNS gỡ instance (goodbye / hết TTL). Chưa phát gì: [`Self::tick`] phát `Lost` sau [`LOST_AFTER`] nếu không
    /// thấy lại.
    pub fn withdraw(&mut self, fullname: &str, now: Instant) {
        for d in self.devices.values_mut() {
            if d.mdns.as_deref() == Some(fullname) && d.withdrawn_at.is_none() {
                d.withdrawn_at = Some(now);
            }
        }
    }

    /// Mốc kế phải gọi [`Self::tick`] / [`Self::due_probes`]. `None` ⇒ ngủ tới tin mDNS / lệnh kế.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.devices.values().flat_map(|d| [d.withdrawn_at.map(|t| t + LOST_AFTER), d.next_probe]).flatten().min()
    }

    /// Phát `Lost` cho instance bị gỡ quá [`LOST_AFTER`].
    pub fn tick(&mut self, now: Instant) -> Vec<DeviceEvent> {
        let mut ev = Vec::new();
        let lost: Vec<DeviceId> = self
            .devices
            .values()
            .filter(|d| d.withdrawn_at.is_some_and(|t| now >= t + LOST_AFTER))
            .map(|d| d.id.clone())
            .collect();
        for id in lost {
            if let Some(d) = self.devices.get_mut(&id) {
                d.mdns = None;
                d.withdrawn_at = None;
            }
            ev.push(DeviceEvent::Lost { id: id.clone() });
            self.apply(&id, DeviceInput::Lost, &mut ev);
        }
        ev
    }

    /// Thiết bị có token, chưa `Online`, đến lượt thử `info` (mỗi [`PROBE_INTERVAL`]). Hẹn lượt kế luôn.
    pub fn due_probes(&mut self, now: Instant) -> Vec<DeviceId> {
        let mut out = Vec::new();
        for d in self.devices.values_mut() {
            if d.next_probe.is_some_and(|t| t <= now) {
                if matches!(d.state, DeviceState::Paired | DeviceState::Offline) && !d.known.is_empty() {
                    d.next_probe = Some(now + PROBE_INTERVAL);
                    out.push(d.id.clone());
                } else {
                    d.next_probe = None;
                }
            }
        }
        out
    }

    /// `info` trả lời từ `from` (đã soát `info.id` khớp — `discovery::connect`). Nhận tên, địa chỉ, `features`;
    /// `from` thành địa chỉ thắng. Thiết bị chưa trong sổ (nhập tay) ⇒ thêm ở `Discovered`.
    pub fn info_ok(&mut self, info: &Info, from: SocketAddr, now: Instant) -> Vec<DeviceEvent> {
        let mut ev = Vec::new();
        let id = info.id.clone();
        let before = self.addresses(&id);
        let d = self
            .devices
            .entry(id.clone())
            .or_insert_with(|| Device::new(id.clone(), info.name.clone(), DeviceState::Unknown));
        let new_device = d.state == DeviceState::Unknown;
        if !info.name.is_empty() {
            d.name = info.name.clone();
        }
        d.features = info.features.clone();
        d.newer_api = info.api > protocol::API_VERSION;
        d.winner = Some(from);
        let port = info.port.unwrap_or(from.port());
        for a in std::iter::once(from).chain(address::from_info(&info.addresses, port)) {
            if !d.known.contains(&a) {
                d.known.push(a);
            }
        }
        if new_device {
            // Nhập tay: coi như "thấy" (Unknown → Discovered).
            self.apply(&id, DeviceInput::Seen, &mut ev);
        }
        self.apply(&id, DeviceInput::InfoOk { paired: info.paired }, &mut ev);
        if let Some(d) = self.devices.get_mut(&id) {
            d.next_probe = (d.state == DeviceState::Paired).then_some(now + PROBE_INTERVAL);
        }
        self.push_address_change(&id, before, &mut ev);
        ev
    }

    /// Đã ghép (token vừa vào kho khoá).
    pub fn paired(&mut self, id: &DeviceId, now: Instant) -> Vec<DeviceEvent> {
        let mut ev = Vec::new();
        self.apply(id, DeviceInput::PinAccepted, &mut ev);
        if let Some(d) = self.devices.get_mut(id) {
            d.next_probe = Some(now);
        }
        ev
    }

    /// `401` ở một request bất kỳ.
    pub fn unauthorized(&mut self, id: &DeviceId) -> Vec<DeviceEvent> {
        let mut ev = Vec::new();
        self.apply(id, DeviceInput::Unauthorized, &mut ev);
        ev
    }

    /// Một địa chỉ hỏng giữa chừng (lỗi mạng): bỏ địa chỉ thắng, lần nối sau chọn lại từ đầu. Chưa đổi trạng thái.
    pub fn address_failed(&mut self, id: &DeviceId, addr: SocketAddr) {
        if let Some(d) = self.devices.get_mut(id)
            && d.winner == Some(addr)
        {
            d.winner = None;
        }
    }

    /// Mọi địa chỉ đều không nối được ⇒ `Offline` (thiết bị có token), thử lại thưa.
    pub fn connect_failed(&mut self, id: &DeviceId, now: Instant) -> Vec<DeviceEvent> {
        let mut ev = Vec::new();
        if let Some(d) = self.devices.get_mut(id) {
            d.winner = None;
            if d.state.has_token() {
                d.next_probe = Some(now + PROBE_INTERVAL);
            }
        }
        self.apply(id, DeviceInput::ConnectFailed, &mut ev);
        ev
    }

    /// Người dùng quên điện thoại trên PC: còn thấy qua mDNS ⇒ `Discovered`, không ⇒ ra khỏi sổ.
    pub fn forget(&mut self, id: &DeviceId) -> Vec<DeviceEvent> {
        let mut ev = Vec::new();
        self.apply(id, DeviceInput::Forgotten, &mut ev);
        if let Some(d) = self.devices.get_mut(id) {
            d.next_probe = None;
            d.winner = None;
        }
        ev
    }

    fn apply(&mut self, id: &DeviceId, input: DeviceInput, ev: &mut Vec<DeviceEvent>) {
        let Some(d) = self.devices.get_mut(id) else { return };
        let from = d.state;
        let mut to = next_state(from, input);
        // Discovered không còn thấy (quên khi điện thoại đã tắt) ⇒ ra khỏi sổ như chưa từng biết.
        if to == DeviceState::Discovered && !d.visible() && input == DeviceInput::Forgotten {
            to = DeviceState::Unknown;
        }
        if to != from {
            d.state = to;
            ev.push(DeviceEvent::StateChanged { id: id.clone(), from, to });
        }
        if to == DeviceState::Unknown {
            self.devices.remove(id);
        }
    }

    fn push_address_change(&self, id: &DeviceId, before: Vec<SocketAddr>, ev: &mut Vec<DeviceEvent>) {
        let after = self.addresses(id);
        if after != before && !after.is_empty() {
            ev.push(DeviceEvent::AddressChanged { id: id.clone(), addresses: after });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DeviceInput as I;
    use DeviceState as S;

    /// C1: mọi mũi tên của `PROTOCOL.md` mục 5 (+ các mũi tên bổ sung ghi ở doc của [`next_state`]).
    #[test]
    fn state_machine_arrows() {
        let arrows = [
            (S::Unknown, I::Seen, S::Discovered),
            (S::Discovered, I::PinAccepted, S::Paired),
            (S::Paired, I::InfoOk { paired: true }, S::Online),
            (S::Online, I::Lost, S::Offline),
            (S::Online, I::ConnectFailed, S::Offline),
            (S::Offline, I::InfoOk { paired: true }, S::Online),
            (S::Online, I::InfoOk { paired: false }, S::NeedsRepair),
            (S::Online, I::Unauthorized, S::NeedsRepair),
            (S::Offline, I::InfoOk { paired: false }, S::NeedsRepair),
            (S::Offline, I::Unauthorized, S::NeedsRepair),
            // Bổ sung.
            (S::Paired, I::Lost, S::Offline),
            (S::Paired, I::ConnectFailed, S::Offline),
            (S::Paired, I::InfoOk { paired: false }, S::NeedsRepair),
            (S::NeedsRepair, I::PinAccepted, S::Paired),
            (S::Offline, I::PinAccepted, S::Paired),
            (S::Discovered, I::Lost, S::Unknown),
            (S::Online, I::Forgotten, S::Discovered),
            (S::NeedsRepair, I::Forgotten, S::Discovered),
        ];
        for (from, input, to) in arrows {
            assert_eq!(next_state(from, input), to, "{from:?} --{input:?}-->");
        }
        // Không có mũi tên ⇒ giữ nguyên.
        let stays = [
            (S::Offline, I::Seen),
            (S::Online, I::Seen),
            (S::Discovered, I::InfoOk { paired: false }),
            (S::Discovered, I::InfoOk { paired: true }),
            (S::Discovered, I::ConnectFailed),
            (S::NeedsRepair, I::Lost),
            (S::NeedsRepair, I::InfoOk { paired: true }),
            (S::Offline, I::Lost),
            (S::Unknown, I::Lost),
            (S::Discovered, I::Forgotten),
        ];
        for (s, input) in stays {
            assert_eq!(next_state(s, input), s, "{s:?} --{input:?}--> (no arrow)");
        }
    }

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn locals() -> Vec<LocalInterface> {
        vec![
            LocalInterface { name: "en0".into(), index: 4, ip: "192.168.1.10".parse().unwrap(), prefix_len: 24 },
            LocalInterface { name: "en0".into(), index: 4, ip: "fd12::10".parse().unwrap(), prefix_len: 64 },
        ]
    }

    fn ann(id: &str, addrs: &[&str]) -> Announcement {
        Announcement {
            fullname: format!("Pixel {id}._vpsync._tcp.local."),
            id: DeviceId(id.into()),
            name: format!("Pixel {id}"),
            api: 1,
            addresses: addrs.iter().map(|a| sa(a)).collect(),
        }
    }

    fn info(id: &str, paired: bool) -> Info {
        Info {
            app: "xmobile.videoplayer".into(),
            api: 1,
            id: DeviceId(id.into()),
            name: "Pixel 8".into(),
            paired,
            port: Some(8080),
            addresses: vec!["192.168.1.23".into(), "fd12::23".into()],
            features: vec!["dirs".into()],
        }
    }

    fn states(ev: &[DeviceEvent]) -> Vec<(DeviceState, DeviceState)> {
        ev.iter()
            .filter_map(|e| match e {
                DeviceEvent::StateChanged { from, to, .. } => Some((*from, *to)),
                _ => None,
            })
            .collect()
    }

    /// Điện thoại cài lại app (mã mới) được ghép như máy mới ⇒ nhận ra bản ghi cũ cùng tên, cùng địa chỉ, không Online.
    #[test]
    fn new_pairing_supersedes_reinstalled_phone() {
        let t0 = Instant::now();
        let old = PairedDevice {
            id: DeviceId("old".into()),
            name: "Pixel 8".into(),
            addresses: vec!["192.168.1.23:8080".into()],
            alias: None,
        };
        let other = PairedDevice { id: DeviceId("other".into()), name: "Galaxy".into(), ..old.clone() };
        let mut devs = Devices::from_config(&[old, other], locals(), t0);
        devs.info_ok(&info("new", false), sa("192.168.1.23:8080"), t0);
        devs.paired(&DeviceId("new".into()), t0);
        assert_eq!(devs.superseded_by(&DeviceId("new".into())), Some(DeviceId("old".into())));
        // Máy cũ đang Online (trả lời đúng mã của nó) ⇒ là hai máy khác nhau.
        devs.info_ok(&info("old", true), sa("192.168.1.23:8080"), t0);
        assert_eq!(devs.superseded_by(&DeviceId("new".into())), None);
    }

    #[test]
    fn unpaired_phone_seen_then_lost_after_30s() {
        let t0 = Instant::now();
        let mut devs = Devices::from_config(&[], locals(), t0);
        let id = DeviceId("aa".into());
        let ev = devs.announce(ann("aa", &["[fd12::23]:8080", "192.168.1.23:8080"]), t0);
        assert!(matches!(&ev[0], DeviceEvent::Seen { id: i, .. } if *i == id));
        assert_eq!(states(&ev), vec![(S::Unknown, S::Discovered)]);
        assert!(ev.contains(&DeviceEvent::AddressChanged {
            id: id.clone(),
            addresses: vec![sa("192.168.1.23:8080"), sa("[fd12::23]:8080")]
        }));
        // Làm mới cùng địa chỉ ⇒ im lặng.
        assert!(devs.announce(ann("aa", &["192.168.1.23:8080"]), t0).is_empty());

        devs.withdraw("Pixel aa._vpsync._tcp.local.", t0);
        assert_eq!(devs.next_deadline(), Some(t0 + LOST_AFTER));
        assert!(devs.tick(t0 + LOST_AFTER - Duration::from_millis(1)).is_empty());
        let ev = devs.tick(t0 + LOST_AFTER);
        assert_eq!(ev[0], DeviceEvent::Lost { id: id.clone() });
        assert_eq!(states(&ev), vec![(S::Discovered, S::Unknown)]);
        assert!(devs.get(&id).is_none(), "unpaired phone leaves the list");
        assert_eq!(devs.next_deadline(), None, "idle: nothing to wake up for");
    }

    #[test]
    fn reannounce_within_grace_cancels_lost() {
        let t0 = Instant::now();
        let mut devs = Devices::default();
        devs.announce(ann("aa", &["192.168.1.23:8080"]), t0);
        devs.withdraw("Pixel aa._vpsync._tcp.local.", t0);
        assert!(devs.announce(ann("aa", &["192.168.1.23:8080"]), t0 + Duration::from_secs(10)).is_empty());
        assert!(devs.tick(t0 + Duration::from_secs(60)).is_empty());
    }

    #[test]
    fn paired_device_from_config_goes_online_then_offline_then_back() {
        let t0 = Instant::now();
        let saved = PairedDevice {
            id: DeviceId("aa".into()),
            name: "Pixel 8".into(),
            addresses: vec!["[fd12::23]:8080".into(), "192.168.1.23:8080".into()],
            alias: None,
        };
        let mut devs = Devices::from_config(&[saved], locals(), t0);
        let id = DeviceId("aa".into());
        assert_eq!(devs.get(&id).unwrap().state, S::Paired);
        assert_eq!(devs.due_probes(t0), vec![id.clone()]);
        assert!(devs.due_probes(t0).is_empty(), "next probe is PROBE_INTERVAL later");

        let ev = devs.info_ok(&info("aa", true), sa("[fd12::23]:8080"), t0);
        assert_eq!(states(&ev), vec![(S::Paired, S::Online)]);
        // Địa chỉ thắng lên đầu dù IPv4 cùng subnet xếp hạng cao hơn.
        assert_eq!(devs.addresses(&id), vec![sa("[fd12::23]:8080"), sa("192.168.1.23:8080")]);
        assert_eq!(devs.next_deadline(), None);

        // mDNS thấy rồi mất ⇒ Offline sau 30 s.
        devs.announce(ann("aa", &["192.168.1.23:8080"]), t0);
        devs.withdraw("Pixel aa._vpsync._tcp.local.", t0);
        let ev = devs.tick(t0 + LOST_AFTER);
        assert_eq!(states(&ev), vec![(S::Online, S::Offline)]);

        // Thấy lại ở IP mới ⇒ cập nhật địa chỉ, hẹn `info` ngay; `info` ok ⇒ Online.
        let t1 = t0 + Duration::from_secs(100);
        let ev = devs.announce(ann("aa", &["192.168.1.99:8080"]), t1);
        assert!(states(&ev).is_empty());
        assert!(ev.iter().any(|e| matches!(e, DeviceEvent::AddressChanged { .. })));
        assert_eq!(devs.due_probes(t1), vec![id.clone()]);
        let ev = devs.info_ok(&info("aa", true), sa("192.168.1.99:8080"), t1);
        assert_eq!(states(&ev), vec![(S::Offline, S::Online)]);

        // Địa chỉ thắng hỏng ⇒ thôi ưu tiên; mọi địa chỉ hỏng ⇒ Offline.
        devs.address_failed(&id, sa("192.168.1.99:8080"));
        assert_eq!(devs.get(&id).unwrap().winner(), None);
        let ev = devs.connect_failed(&id, t1);
        assert_eq!(states(&ev), vec![(S::Online, S::Offline)]);
        assert_eq!(devs.next_deadline(), Some(t1 + PROBE_INTERVAL));

        // Điện thoại đã quên PC ⇒ NeedsRepair; ghép lại ⇒ Paired.
        let ev = devs.info_ok(&info("aa", false), sa("192.168.1.99:8080"), t1);
        assert_eq!(states(&ev), vec![(S::Offline, S::NeedsRepair)]);
        assert_eq!(states(&devs.paired(&id, t1)), vec![(S::NeedsRepair, S::Paired)]);

        let saved = devs.to_config();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Pixel 8");
        assert_eq!(saved[0].addresses[0], "192.168.1.99:8080", "winner saved first");
    }

    #[test]
    fn manual_address_adds_device_and_forget_removes_invisible_one() {
        let t0 = Instant::now();
        let mut devs = Devices::from_config(&[], locals(), t0);
        let id = DeviceId("bb".into());
        let ev = devs.info_ok(&info("bb", false), sa("127.0.0.1:18080"), t0);
        assert_eq!(states(&ev), vec![(S::Unknown, S::Discovered)]);
        assert_eq!(devs.addresses(&id)[0], sa("127.0.0.1:18080"));
        assert!(devs.addresses(&id).contains(&sa("[fd12::23]:8080")), "info.addresses joined the set");
        assert_eq!(devs.get(&id).unwrap().features, vec!["dirs".to_string()]);
        devs.paired(&id, t0);
        let ev = devs.forget(&id);
        assert_eq!(states(&ev), vec![(S::Paired, S::Unknown)]);
        assert!(devs.get(&id).is_none());
    }

    #[test]
    fn newer_api_is_flagged_but_kept() {
        let mut devs = Devices::default();
        let mut a = ann("cc", &["192.168.1.5:8080"]);
        a.api = 2;
        devs.announce(a, Instant::now());
        assert!(devs.get(&DeviceId("cc".into())).unwrap().newer_api);
    }

    #[test]
    fn same_instance_name_with_new_id_retires_old_device() {
        let t0 = Instant::now();
        let mut devs = Devices::default();
        devs.announce(ann("aa", &["192.168.1.5:8080"]), t0);
        let mut again = ann("dd", &["192.168.1.5:8080"]);
        again.fullname = "Pixel aa._vpsync._tcp.local.".into();
        let ev = devs.announce(again, t0);
        assert_eq!(ev[0], DeviceEvent::Lost { id: DeviceId("aa".into()) });
        assert!(devs.get(&DeviceId("aa".into())).is_none());
        assert!(devs.get(&DeviceId("dd".into())).is_some());
    }

    #[test]
    fn interface_change_reorders_addresses() {
        let t0 = Instant::now();
        let mut devs = Devices::from_config(&[], locals(), t0);
        devs.announce(ann("aa", &["[fd12::23]:8080", "192.168.1.23:8080"]), t0);
        // PC rời WiFi v4, chỉ còn v6.
        let v6_only =
            vec![LocalInterface { name: "en0".into(), index: 4, ip: "fd12::10".parse().unwrap(), prefix_len: 64 }];
        let ev = devs.set_local_interfaces(v6_only);
        assert_eq!(
            ev,
            vec![DeviceEvent::AddressChanged {
                id: DeviceId("aa".into()),
                addresses: vec![sa("[fd12::23]:8080"), sa("192.168.1.23:8080")]
            }]
        );
    }

    /// gd1_12: bí danh đặt trên PC sống qua `info` (tên điện thoại đổi) và qua lần lưu / nạp config.
    #[test]
    fn alias_survives_info_and_config_round_trip() {
        let t0 = Instant::now();
        let saved = PairedDevice {
            id: DeviceId("aa".into()),
            name: "Pixel 8".into(),
            addresses: vec!["192.168.1.23:8080".into()],
            alias: Some("Máy của mẹ".into()),
        };
        let mut devs = Devices::from_config(&[saved], locals(), t0);
        let id = DeviceId("aa".into());
        assert_eq!(devs.get(&id).unwrap().display_name(), "Máy của mẹ");
        let mut renamed = info("aa", true);
        renamed.name = "Pixel 8 Pro".into();
        devs.info_ok(&renamed, sa("192.168.1.23:8080"), t0);
        let d = devs.get(&id).unwrap();
        assert_eq!((d.name.as_str(), d.display_name()), ("Pixel 8 Pro", "Máy của mẹ"));
        assert_eq!(devs.to_config()[0].alias.as_deref(), Some("Máy của mẹ"));
        assert!(devs.set_alias(&id, Some("   ".into())), "blank clears the alias");
        assert_eq!(devs.get(&id).unwrap().display_name(), "Pixel 8 Pro");
        assert_eq!(devs.to_config()[0].alias, None);
        assert!(!devs.set_alias(&DeviceId("zz".into()), Some("x".into())));
    }
}
