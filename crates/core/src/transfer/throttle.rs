//! Giới hạn băng thông (token bucket bọc quanh bộ đọc tệp) + đo tốc độ trung bình trượt — task gd1_04.
//!
//! - [`Throttle`]: một xô dùng chung (`Arc`) cho mọi runner ⇒ giới hạn là tổng của cả app (PROTOCOL 2.8: "Băng thông:
//!   mặc định hết cỡ, có tuỳ chọn giới hạn"). Đổi mức lúc đang chạy bằng [`Throttle::set_rate`]. Không giới hạn ⇒ đường
//!   đọc thẳng, không đụng khoá.
//! - [`MeteredReader`]: bọc `AsyncRead` của tệp nguồn — đếm byte đã đọc (tiến độ trong khúc) và xin token trước khi đọc.
//!   Không có vòng chờ bận: thiếu token ⇒ đặt `tokio::time::Sleep` tới lúc đủ rồi trả `Pending`.
//! - [`MeteredWriter`]: phía tải về (gd1_10) — bọc `AsyncWrite` của tệp `.part`, cùng xô: ghi chậm ⇒ đọc socket chậm
//!   (backpressure TCP), không gom vào RAM.
//! - [`SpeedMeter`]: tốc độ = byte trong cửa sổ trượt 5 s / thời gian, mẫu lấy theo nhịp tiến độ (không mỗi gói).

use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

/// Xô chứa tối đa chừng này thời gian của mức giới hạn (cú dồn sau khi rảnh ≤ 100 ms dữ liệu).
const BURST: Duration = Duration::from_millis(100);
/// Xô tối thiểu — mức giới hạn rất thấp vẫn đọc được từng mẩu hợp lý.
const MIN_BURST: f64 = 16.0 * 1024.0;
/// Chờ tới khi xô có ít nhất chừng này (hoặc đủ cả yêu cầu) để khỏi thức dậy cho vài byte.
const MIN_GRANT: f64 = 16.0 * 1024.0;
/// Bộ đệm trung gian khi bị giới hạn (đọc đúng số byte được cấp).
const SCRATCH: usize = 64 * 1024;

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Token bucket theo byte / giây. `0` = không giới hạn.
#[derive(Debug)]
pub struct Throttle {
    rate: AtomicU64,
    bucket: Mutex<Bucket>,
}

impl Throttle {
    /// `None` = không giới hạn (mặc định).
    pub fn new(bytes_per_sec: Option<u64>) -> Arc<Throttle> {
        Arc::new(Throttle {
            rate: AtomicU64::new(bytes_per_sec.unwrap_or(0)),
            bucket: Mutex::new(Bucket { tokens: 0.0, last: Instant::now() }),
        })
    }

    pub fn unlimited() -> Arc<Throttle> {
        Throttle::new(None)
    }

    pub fn rate(&self) -> Option<u64> {
        Some(self.rate.load(Ordering::Relaxed)).filter(|r| *r > 0)
    }

    /// Đổi mức (cài đặt gd1_12). Xô về rỗng để mức mới không bị cú dồn của mức cũ.
    pub fn set_rate(&self, bytes_per_sec: Option<u64>) {
        self.rate.store(bytes_per_sec.unwrap_or(0), Ordering::Relaxed);
        let mut b = self.lock();
        b.tokens = 0.0;
        b.last = Instant::now();
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Bucket> {
        self.bucket.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Xin tối đa `want` byte. `Ok(n)` = được đọc `n` (> 0); `Err(d)` = chờ `d` rồi xin lại.
    fn take(&self, want: usize) -> Result<usize, Duration> {
        let rate = self.rate.load(Ordering::Relaxed);
        if rate == 0 || want == 0 {
            return Ok(want);
        }
        let rate = rate as f64;
        let cap = (rate * BURST.as_secs_f64()).max(MIN_BURST);
        let mut b = self.lock();
        let now = Instant::now();
        b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * rate).min(cap);
        b.last = now;
        let need = (want as f64).min(MIN_GRANT);
        if b.tokens >= need {
            let n = (want as f64).min(b.tokens.floor()) as usize;
            b.tokens -= n as f64;
            Ok(n)
        } else {
            Err(Duration::from_secs_f64((need - b.tokens) / rate).max(Duration::from_millis(1)))
        }
    }

    /// Trả lại token đã xin mà không dùng (bộ đọc trả ít byte hơn / `Pending`).
    fn refund(&self, n: usize) {
        if n > 0 && self.rate.load(Ordering::Relaxed) > 0 {
            self.lock().tokens += n as f64;
        }
    }
}

/// Bộ đọc tệp nguồn cho thân `PATCH`: đếm byte vào `counter`, tôn trọng [`Throttle`].
pub struct MeteredReader<R> {
    inner: Pin<Box<R>>,
    throttle: Arc<Throttle>,
    counter: Arc<AtomicU64>,
    sleep: Option<Pin<Box<Sleep>>>,
    scratch: Vec<u8>,
}

impl<R: AsyncRead> MeteredReader<R> {
    pub fn new(inner: R, throttle: Arc<Throttle>, counter: Arc<AtomicU64>) -> MeteredReader<R> {
        MeteredReader { inner: Box::pin(inner), throttle, counter, sleep: None, scratch: Vec::new() }
    }
}

impl<R: AsyncRead> AsyncRead for MeteredReader<R> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if this.throttle.rate().is_none() {
            let before = buf.filled().len();
            ready!(this.inner.as_mut().poll_read(cx, buf))?;
            this.counter.fetch_add((buf.filled().len() - before) as u64, Ordering::Relaxed);
            return Poll::Ready(Ok(()));
        }
        loop {
            if let Some(sleep) = this.sleep.as_mut() {
                ready!(sleep.as_mut().poll(cx));
                this.sleep = None;
            }
            let want = buf.remaining().min(SCRATCH);
            match this.throttle.take(want) {
                Ok(granted) => {
                    if this.scratch.len() < granted {
                        this.scratch.resize(SCRATCH, 0);
                    }
                    let mut sub = ReadBuf::new(&mut this.scratch[..granted]);
                    match this.inner.as_mut().poll_read(cx, &mut sub) {
                        Poll::Pending => {
                            this.throttle.refund(granted);
                            return Poll::Pending;
                        }
                        Poll::Ready(Err(e)) => {
                            this.throttle.refund(granted);
                            return Poll::Ready(Err(e));
                        }
                        Poll::Ready(Ok(())) => {
                            let got = sub.filled().len();
                            this.throttle.refund(granted - got);
                            buf.put_slice(&this.scratch[..got]);
                            this.counter.fetch_add(got as u64, Ordering::Relaxed);
                            return Poll::Ready(Ok(()));
                        }
                    }
                }
                Err(wait) => this.sleep = Some(Box::pin(tokio::time::sleep(wait))),
            }
        }
    }
}

/// Bộ ghi tệp `.part` khi tải về: đếm byte đã ghi vào `counter` (cộng dồn lên giá trị sẵn có — runner đặt sẵn offset
/// bắt đầu), tôn trọng [`Throttle`].
pub struct MeteredWriter<W> {
    inner: Pin<Box<W>>,
    throttle: Arc<Throttle>,
    counter: Arc<AtomicU64>,
    sleep: Option<Pin<Box<Sleep>>>,
}

impl<W: AsyncWrite> MeteredWriter<W> {
    pub fn new(inner: W, throttle: Arc<Throttle>, counter: Arc<AtomicU64>) -> MeteredWriter<W> {
        MeteredWriter { inner: Box::pin(inner), throttle, counter, sleep: None }
    }

    pub fn into_inner(self) -> Pin<Box<W>> {
        self.inner
    }
}

impl<W: AsyncWrite> AsyncWrite for MeteredWriter<W> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        let granted = if this.throttle.rate().is_none() {
            buf.len()
        } else {
            loop {
                if let Some(sleep) = this.sleep.as_mut() {
                    ready!(sleep.as_mut().poll(cx));
                    this.sleep = None;
                }
                match this.throttle.take(buf.len().min(SCRATCH)) {
                    Ok(n) => break n,
                    Err(wait) => this.sleep = Some(Box::pin(tokio::time::sleep(wait))),
                }
            }
        };
        match this.inner.as_mut().poll_write(cx, &buf[..granted]) {
            Poll::Ready(Ok(n)) => {
                this.throttle.refund(granted - n);
                this.counter.fetch_add(n as u64, Ordering::Relaxed);
                Poll::Ready(Ok(n))
            }
            other => {
                this.throttle.refund(granted);
                other
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.as_mut().poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.inner.as_mut().poll_shutdown(cx)
    }
}

/// Cửa sổ của trung bình trượt.
pub const SPEED_WINDOW: Duration = Duration::from_secs(5);

/// Tốc độ trung bình trượt từ các mẫu `(giờ, tổng byte)`.
#[derive(Debug, Default)]
pub struct SpeedMeter {
    samples: VecDeque<(Instant, u64)>,
}

impl SpeedMeter {
    pub fn new() -> SpeedMeter {
        SpeedMeter::default()
    }

    /// Ghi mẫu. `total` lùi (gửi lại từ offset thấp hơn) ⇒ bắt đầu cửa sổ mới.
    pub fn record(&mut self, at: Instant, total: u64) {
        if self.samples.back().is_some_and(|&(_, t)| total < t) {
            self.samples.clear();
        }
        self.samples.push_back((at, total));
        while self.samples.len() > 2 && self.samples.front().is_some_and(|&(t, _)| at.duration_since(t) > SPEED_WINDOW)
        {
            self.samples.pop_front();
        }
    }

    /// Byte / giây; `0` khi chưa đủ hai mẫu.
    pub fn bytes_per_sec(&self) -> u64 {
        match (self.samples.front(), self.samples.back()) {
            (Some(&(t0, b0)), Some(&(t1, b1))) if t1 > t0 => {
                ((b1 - b0) as f64 / t1.duration_since(t0).as_secs_f64()) as u64
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn unlimited_grants_everything() {
        let t = Throttle::unlimited();
        assert_eq!(t.take(1 << 20), Ok(1 << 20));
    }

    #[test]
    fn limited_starts_empty_and_waits() {
        let t = Throttle::new(Some(1_000_000));
        let Err(wait) = t.take(64 * 1024) else { panic!("bucket must start empty") };
        // 16 KB ở 1 MB/s ≈ 16 ms.
        assert!(wait >= Duration::from_millis(10) && wait <= Duration::from_millis(20), "{wait:?}");
    }

    #[test]
    fn speed_meter_window() {
        let mut m = SpeedMeter::new();
        let t0 = Instant::now();
        assert_eq!(m.bytes_per_sec(), 0);
        m.record(t0, 0);
        m.record(t0 + Duration::from_secs(1), 1_000);
        m.record(t0 + Duration::from_secs(2), 2_000);
        assert_eq!(m.bytes_per_sec(), 1_000);
        // Mẫu cũ hơn 5 s rơi khỏi cửa sổ: tốc độ theo phần gần đây.
        m.record(t0 + Duration::from_secs(8), 32_000);
        assert_eq!(m.bytes_per_sec(), 5_000);
        // Gửi lại từ offset thấp ⇒ cửa sổ mới.
        m.record(t0 + Duration::from_secs(9), 10);
        assert_eq!(m.bytes_per_sec(), 0);
    }

    #[tokio::test]
    async fn metered_reader_counts_and_limits() {
        let data = vec![7u8; 600 * 1024];
        let counter = Arc::new(AtomicU64::new(0));
        let throttle = Throttle::new(Some(2 * 1024 * 1024));
        let mut r = MeteredReader::new(&data[..], throttle, counter.clone());
        let start = Instant::now();
        let mut out = Vec::new();
        r.read_to_end(&mut out).await.unwrap();
        let elapsed = start.elapsed().as_secs_f64();
        assert_eq!(out, data);
        assert_eq!(counter.load(Ordering::Relaxed), data.len() as u64);
        // 600 KB ở 2 MB/s ≈ 0,29 s.
        assert!((0.25..0.40).contains(&elapsed), "{elapsed}");
    }

    #[tokio::test]
    async fn metered_writer_counts_from_base_and_limits() {
        use tokio::io::AsyncWriteExt;
        let data = vec![7u8; 600 * 1024];
        let counter = Arc::new(AtomicU64::new(100));
        let mut w = MeteredWriter::new(Vec::new(), Throttle::new(Some(2 * 1024 * 1024)), counter.clone());
        let start = Instant::now();
        w.write_all(&data).await.unwrap();
        let elapsed = start.elapsed().as_secs_f64();
        assert_eq!(counter.load(Ordering::Relaxed), 100 + data.len() as u64);
        assert!((0.25..0.40).contains(&elapsed), "{elapsed}");
        let out = w.into_inner();
        assert_eq!(out.len(), data.len());
    }
}
