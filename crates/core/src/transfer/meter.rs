//! Đếm byte + đo tốc độ trung bình trượt — task gd1_04. Không giới hạn băng thông: luôn gửi / tải hết cỡ (bỏ tuỳ chọn
//! giới hạn 12/11/2026, PROTOCOL 2.8).
//!
//! - [`MeteredReader`]: bọc `AsyncRead` của tệp nguồn — đếm byte đã đọc (tiến độ trong khúc).
//! - [`MeteredWriter`]: phía tải về (gd1_10) — bọc `AsyncWrite` của tệp `.part`, đếm byte đã ghi.
//! - [`SpeedMeter`]: tốc độ = byte trong cửa sổ trượt 5 s / thời gian, mẫu lấy theo nhịp tiến độ (không mỗi gói).

use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, ready};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Bộ đọc tệp nguồn cho thân `PATCH`: đếm byte vào `counter`.
pub struct MeteredReader<R> {
    inner: Pin<Box<R>>,
    counter: Arc<AtomicU64>,
}

impl<R: AsyncRead> MeteredReader<R> {
    pub fn new(inner: R, counter: Arc<AtomicU64>) -> MeteredReader<R> {
        MeteredReader { inner: Box::pin(inner), counter }
    }
}

impl<R: AsyncRead> AsyncRead for MeteredReader<R> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        ready!(self.inner.as_mut().poll_read(cx, buf))?;
        self.counter.fetch_add((buf.filled().len() - before) as u64, Ordering::Relaxed);
        Poll::Ready(Ok(()))
    }
}

/// Bộ ghi tệp `.part` khi tải về: đếm byte đã ghi vào `counter` (cộng dồn lên giá trị sẵn có — runner đặt sẵn offset
/// bắt đầu).
pub struct MeteredWriter<W> {
    inner: Pin<Box<W>>,
    counter: Arc<AtomicU64>,
}

impl<W: AsyncWrite> MeteredWriter<W> {
    pub fn new(inner: W, counter: Arc<AtomicU64>) -> MeteredWriter<W> {
        MeteredWriter { inner: Box::pin(inner), counter }
    }

    pub fn into_inner(self) -> Pin<Box<W>> {
        self.inner
    }
}

impl<W: AsyncWrite> AsyncWrite for MeteredWriter<W> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let n = ready!(self.inner.as_mut().poll_write(cx, buf))?;
        self.counter.fetch_add(n as u64, Ordering::Relaxed);
        Poll::Ready(Ok(n))
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
    async fn metered_reader_counts() {
        let data = vec![7u8; 600 * 1024];
        let counter = Arc::new(AtomicU64::new(0));
        let mut r = MeteredReader::new(&data[..], counter.clone());
        let mut out = Vec::new();
        r.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, data);
        assert_eq!(counter.load(Ordering::Relaxed), data.len() as u64);
    }

    #[tokio::test]
    async fn metered_writer_counts_from_base() {
        let data = vec![7u8; 600 * 1024];
        let counter = Arc::new(AtomicU64::new(100));
        let mut w = MeteredWriter::new(Vec::new(), counter.clone());
        w.write_all(&data).await.unwrap();
        assert_eq!(counter.load(Ordering::Relaxed), 100 + data.len() as u64);
        assert_eq!(w.into_inner().len(), data.len());
    }
}
