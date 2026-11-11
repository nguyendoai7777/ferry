//! Dò điện thoại qua mDNS `_vpsync._tcp` + địa chỉ nhập tay, IPv4 + IPv6 (`PROTOCOL.md` 6.2, 6.8) — task gd1_05.
//!
//! - [`address`]: thứ tự thử địa chỉ, đọc / ghi địa chỉ nhập tay (hàm thuần).
//! - [`mdns`]: [`mdns::Browser`] đổi sự kiện của `mdns-sd` thành [`crate::device::Announcement`] / gỡ instance.
//! - [`race`] + [`connect`] / [`probe`]: nối kiểu happy eyeballs (thử kế sau [`HAPPY_EYEBALLS_DELAY`]), soát `info.id`.
//!
//! Sổ thiết bị và máy trạng thái ở [`crate::device`]; daemon (gd1_07) nối các mảnh: tin của `Browser` ⇒
//! `Devices::announce` / `withdraw`, `Devices::next_deadline` ⇒ ngủ ⇒ `tick` / `due_probes` ⇒ [`connect`] ⇒ `info_ok`.

pub mod address;
pub mod mdns;

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::Duration;

use crate::client::{ClientError, PhoneClient};
use crate::device::DeviceId;
use crate::protocol::{Info, Secret};

/// Thử địa chỉ kế khi địa chỉ đang thử chưa nối được sau chừng này (`PROTOCOL.md` 6.8).
pub const HAPPY_EYEBALLS_DELAY: Duration = Duration::from_millis(250);
/// Trần cả cuộc đua `info` (mọi địa chỉ) — mỗi địa chỉ còn bị `client::CONNECT_TIMEOUT` chặn riêng.
pub const RACE_TIMEOUT: Duration = Duration::from_secs(12);

type Attempt<'a, T, E> = Pin<Box<dyn Future<Output = (SocketAddr, Result<T, E>)> + Send + 'a>>;

/// Happy eyeballs đơn giản: bắt đầu địa chỉ đầu; mỗi [`HAPPY_EYEBALLS_DELAY`] (hoặc ngay khi lần thử đang chạy hỏng)
/// thêm địa chỉ kế, chạy song song. `Ok` đầu tiên thắng, các lần thử còn lại bị huỷ. Mọi lần đều `Err` ⇒ danh sách lỗi
/// theo thứ tự xong. `addrs` rỗng ⇒ `Err(vec![])`.
pub async fn race<'a, T, E, F, Fut>(
    addrs: &[SocketAddr],
    delay: Duration,
    attempt: F,
) -> Result<(SocketAddr, T), Vec<(SocketAddr, E)>>
where
    F: Fn(SocketAddr) -> Fut,
    Fut: Future<Output = Result<T, E>> + Send + 'a,
    T: 'a,
    E: 'a,
{
    let mut pending: Vec<Attempt<'a, T, E>> = Vec::new();
    let mut next = 0usize;
    let mut errors = Vec::new();
    let start = |addr: SocketAddr| -> Attempt<'a, T, E> {
        let fut = attempt(addr);
        Box::pin(async move { (addr, fut.await) })
    };
    loop {
        if pending.is_empty() {
            match addrs.get(next) {
                Some(a) => {
                    pending.push(start(*a));
                    next += 1;
                }
                None => return Err(errors),
            }
        }
        let more = next < addrs.len();
        let done = std::future::poll_fn(|cx| {
            for (i, p) in pending.iter_mut().enumerate() {
                if let std::task::Poll::Ready(out) = p.as_mut().poll(cx) {
                    return std::task::Poll::Ready(Some((i, out)));
                }
            }
            std::task::Poll::Pending
        });
        let outcome = if more {
            tokio::select! {
                d = done => d,
                () = tokio::time::sleep(delay) => None,
            }
        } else {
            done.await
        };
        match outcome {
            Some((i, (addr, Ok(v)))) => {
                drop(pending.swap_remove(i));
                return Ok((addr, v));
            }
            Some((i, (addr, Err(e)))) => {
                drop(pending.swap_remove(i));
                errors.push((addr, e));
                // Hỏng sớm ⇒ thử kế ngay, không chờ hết nhịp.
                if let Some(a) = addrs.get(next) {
                    pending.push(start(*a));
                    next += 1;
                }
            }
            None => {
                if let Some(a) = addrs.get(next) {
                    pending.push(start(*a));
                    next += 1;
                }
            }
        }
    }
}

/// Nối thiết bị đã biết `id`: đua `GET /info` trên `addrs` (đã xếp — `Devices::addresses`).
#[derive(Debug)]
pub struct Connected {
    pub client: PhoneClient,
    pub info: Info,
    pub addr: SocketAddr,
}

/// Vì sao [`connect`] / [`probe`] không xong.
#[derive(Debug)]
pub enum ConnectError {
    /// Không địa chỉ nào trả lời (điện thoại tắt Sync WiFi / khác mạng) ⇒ `Devices::connect_failed`.
    /// Địa chỉ trả lời nhưng là điện thoại khác (IP cấp lại cho máy khác) nằm trong danh sách này dưới dạng
    /// `ClientError::InvalidResponse`. Rỗng ⇒ hết [`RACE_TIMEOUT`].
    Unreachable(Vec<(SocketAddr, ClientError)>),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(errs) => {
                f.write_str("no address answered")?;
                for (a, e) in errs {
                    write!(f, "; {a}: {e}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ConnectError {}

/// Đua `info` trên các địa chỉ của `id`, có token thì kèm token (`paired` cho biết token còn sống). Địa chỉ trả về
/// `info.id` khác bị coi như hỏng. Mã trạng thái lỗi của `info` (hiếm: `503` quá tải) cũng coi như hỏng ở địa chỉ đó.
pub async fn connect(id: &DeviceId, addrs: &[SocketAddr], token: Option<&Secret>) -> Result<Connected, ConnectError> {
    let attempt = |addr: SocketAddr| {
        let id = id.clone();
        let token = token.cloned();
        async move {
            let client = PhoneClient::new(addr, Some(&id), token)?;
            let info = client.info().await?;
            if info.id != id {
                log::info!("discovery: {addr} answers as {} instead of {id}", info.id);
                return Err(ClientError::InvalidResponse(format!("another phone ({})", info.id)));
            }
            Ok((client, info))
        }
    };
    match tokio::time::timeout(RACE_TIMEOUT, race(addrs, HAPPY_EYEBALLS_DELAY, attempt)).await {
        Ok(Ok((addr, (client, info)))) => Ok(Connected { client, info, addr }),
        Ok(Err(errs)) => Err(ConnectError::Unreachable(errs)),
        Err(_) => Err(ConnectError::Unreachable(Vec::new())),
    }
}

/// Không địa chỉ nào trả lời đúng `id`: hỏi `info` không token lần lượt trên các địa chỉ đó, trả máy *khác* đầu tiên
/// trả lời — app điện thoại cài lại (hoặc xoá dữ liệu) đổi `id` nhưng giữ IP. Chỉ dùng khi người dùng đang ghép lại bằng
/// PIN đọc trên chính máy đó: PIN đúng mới là bằng chứng, `name` trùng thì không.
pub async fn find_replacement(id: &DeviceId, addrs: &[SocketAddr]) -> Option<(SocketAddr, Info)> {
    for &addr in addrs {
        match probe(addr).await {
            Ok(info) if info.id != *id => return Some((addr, info)),
            Ok(_) => {}
            Err(e) => log::debug!("discovery: {addr} for replacement of {id}: {e}"),
        }
    }
    None
}

/// Địa chỉ nhập tay, chưa biết `id`: `GET /info` không token (`PROTOCOL.md` 6.2) ⇒ `id`, tên, các địa chỉ khác ⇒
/// `Devices::info_ok`.
pub async fn probe(addr: SocketAddr) -> Result<Info, ClientError> {
    let client = PhoneClient::new(addr, None, None)?;
    match tokio::time::timeout(RACE_TIMEOUT, client.info()).await {
        Ok(r) => r,
        Err(_) => Err(ClientError::Network(std::io::Error::from(std::io::ErrorKind::TimedOut))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addrs(n: u16) -> Vec<SocketAddr> {
        (0..n).map(|i| SocketAddr::from(([10, 0, 0, i as u8 + 1], 8080))).collect()
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn second_address_starts_after_delay_and_wins() {
        let list = addrs(3);
        let first = list[0];
        let t0 = tokio::time::Instant::now();
        let got = race(&list, HAPPY_EYEBALLS_DELAY, |a: SocketAddr| async move {
            if a == first {
                std::future::pending::<()>().await; // treo như địa chỉ chết không trả RST
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok::<_, ()>(a)
        })
        .await;
        assert_eq!(got.unwrap().0, list[1]);
        assert_eq!(t0.elapsed(), HAPPY_EYEBALLS_DELAY + Duration::from_millis(10));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn fast_failure_moves_on_immediately() {
        let list = addrs(2);
        let first = list[0];
        let t0 = tokio::time::Instant::now();
        let got = race(&list, HAPPY_EYEBALLS_DELAY, |a: SocketAddr| async move {
            if a == first { Err("refused") } else { Ok(a) }
        })
        .await;
        assert_eq!(got.unwrap().0, list[1]);
        assert_eq!(t0.elapsed(), Duration::ZERO);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn first_address_keeps_priority_when_it_answers_in_time() {
        let list = addrs(2);
        let first = list[0];
        let got = race(&list, HAPPY_EYEBALLS_DELAY, |a: SocketAddr| async move {
            tokio::time::sleep(Duration::from_millis(if a == first { 400 } else { 500 })).await;
            Ok::<_, ()>(a)
        })
        .await;
        // Đầu chạy từ 0 → 400 ms; kế bắt đầu ở 250 → 750 ms ⇒ đầu thắng.
        assert_eq!(got.unwrap().0, list[0]);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn all_fail_returns_every_error() {
        let list = addrs(3);
        let got = race(&list, HAPPY_EYEBALLS_DELAY, |a: SocketAddr| async move { Err::<(), _>(a) }).await;
        let errs = got.unwrap_err();
        assert_eq!(errs.iter().map(|(a, _)| *a).collect::<Vec<_>>(), list);
        let none = race(&[], HAPPY_EYEBALLS_DELAY, |_| async { Ok::<(), ()>(()) }).await;
        assert!(none.unwrap_err().is_empty());
    }
}
