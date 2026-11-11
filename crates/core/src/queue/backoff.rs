//! Backoff khi lỗi tạm (`PROTOCOL.md` 5: "Waiting (backoff 2 s, 4 s … tối đa 60 s)"). `Retry-After` của server thắng.

use std::time::Duration;

/// Lần chờ đầu tiên.
pub const BASE: Duration = Duration::from_secs(2);
/// Trần backoff tự tính.
pub const CAP: Duration = Duration::from_secs(60);
/// `Retry-After` nhỏ hơn mức này bị nâng lên: server trả `0` liên tục không được biến thành vòng lặp bận.
pub const RETRY_AFTER_MIN: Duration = Duration::from_secs(1);
/// `Retry-After` lớn hơn mức này bị hạ xuống: một phản hồi lạ không được treo hàng đợi cả ngày.
pub const RETRY_AFTER_MAX: Duration = Duration::from_secs(60 * 60);

/// Thời gian chờ trước lần thử `attempt` (1 = lần lỗi đầu tiên): 2 s, 4 s, 8 s … trần 60 s. `retry_after` (header
/// `Retry-After` của server) thắng, kẹp trong [`RETRY_AFTER_MIN`, `RETRY_AFTER_MAX`].
pub fn delay(attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(ra) = retry_after {
        return ra.clamp(RETRY_AFTER_MIN, RETRY_AFTER_MAX);
    }
    let shift = attempt.saturating_sub(1).min(16);
    BASE.saturating_mul(1u32 << shift).min(CAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: fn(u64) -> Duration = Duration::from_secs;

    #[test]
    fn doubles_from_two_seconds() {
        assert_eq!(delay(1, None), S(2));
        assert_eq!(delay(2, None), S(4));
        assert_eq!(delay(3, None), S(8));
        assert_eq!(delay(5, None), S(32));
    }

    #[test]
    fn hits_cap_and_stays() {
        assert_eq!(delay(6, None), S(60)); // 64 ⇒ 60
        assert_eq!(delay(7, None), S(60));
        assert_eq!(delay(u32::MAX, None), S(60));
        // Lần 0 (chưa lỗi) coi như lần 1.
        assert_eq!(delay(0, None), S(2));
    }

    #[test]
    fn retry_after_wins() {
        assert_eq!(delay(1, Some(S(30))), S(30));
        assert_eq!(delay(10, Some(S(5))), S(5)); // nhỏ hơn backoff tự tính vẫn thắng
        assert_eq!(delay(1, Some(S(120))), S(120)); // lớn hơn trần 60 s vẫn thắng
    }

    #[test]
    fn retry_after_zero_and_huge_are_clamped() {
        assert_eq!(delay(1, Some(Duration::ZERO)), RETRY_AFTER_MIN);
        assert_eq!(delay(1, Some(S(u64::MAX / 4))), RETRY_AFTER_MAX);
    }
}
