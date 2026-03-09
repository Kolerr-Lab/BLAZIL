//! Nanosecond-precision timestamp for all Blazil temporal operations.
//!
//! [`Timestamp`] wraps a `u64` representing nanoseconds since the Unix epoch
//! (1970-01-01T00:00:00Z). Using an integer representation avoids floating-
//! point imprecision and makes serialization deterministic.
//!
//! # Examples
//!
//! ```rust
//! use blazil_common::timestamp::Timestamp;
//!
//! let ts = Timestamp::now();
//! assert!(ts.as_nanos() > 0);
//!
//! let fixed = Timestamp::from_nanos(1_741_564_800_000_000_000); // 2026-03-10
//! println!("{}", fixed); // 2026-03-10T00:00:00.000000000Z
//! ```

use serde::{Deserialize, Serialize};
use std::fmt;

/// A nanosecond-precision timestamp representing an instant since the Unix epoch.
///
/// Stored as a `u64` count of nanoseconds since 1970-01-01T00:00:00Z.
/// This representation is:
/// - Free of floating-point imprecision
/// - Trivially comparable and sortable
/// - Cheaply copyable (8 bytes)
/// - Deterministically serialisable
///
/// # Examples
///
/// ```rust
/// use blazil_common::timestamp::Timestamp;
///
/// let t1 = Timestamp::from_nanos(1_000);
/// let t2 = Timestamp::from_nanos(2_000);
/// assert!(t1 < t2);
/// assert_eq!(t1.as_nanos(), 1_000);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(u64);

impl Timestamp {
    /// Returns the current wall-clock time as a [`Timestamp`].
    ///
    /// Uses [`std::time::SystemTime`] for nanosecond resolution.
    ///
    /// # Panics
    ///
    /// Panics if the system clock reports a time before the Unix epoch
    /// (1970-01-01T00:00:00Z). This condition cannot occur on any
    /// correctly configured production machine.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::timestamp::Timestamp;
    ///
    /// let ts = Timestamp::now();
    /// assert!(ts.as_nanos() > 0);
    /// ```
    #[must_use]
    pub fn now() -> Self {
        let dur = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect(
                "System clock is before UNIX epoch (1970-01-01T00:00:00Z). \
                 This should never happen on a correctly configured machine.",
            );
        Self(dur.as_nanos() as u64)
    }

    /// Constructs a [`Timestamp`] directly from a nanosecond count.
    ///
    /// Useful for replaying historical events or in tests where a
    /// deterministic timestamp is required.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::timestamp::Timestamp;
    ///
    /// let ts = Timestamp::from_nanos(1_741_564_800_000_000_000);
    /// assert_eq!(ts.as_nanos(), 1_741_564_800_000_000_000);
    /// ```
    #[must_use]
    pub fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Returns the raw nanoseconds since the Unix epoch.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::timestamp::Timestamp;
    ///
    /// let ts = Timestamp::from_nanos(42_000);
    /// assert_eq!(ts.as_nanos(), 42_000);
    /// ```
    #[must_use]
    pub fn as_nanos(&self) -> u64 {
        self.0
    }

    /// Returns the number of nanoseconds elapsed since this timestamp.
    ///
    /// Returns `0` if the current time is before `self` (clock skew protection).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::timestamp::Timestamp;
    ///
    /// let ts = Timestamp::now();
    /// let elapsed = ts.elapsed_nanos();
    /// // elapsed is 0 or very small (sub-microsecond)
    /// assert!(elapsed < 1_000_000_000); // less than 1 second
    /// ```
    #[must_use]
    pub fn elapsed_nanos(&self) -> u64 {
        Self::now().0.saturating_sub(self.0)
    }

    /// Returns microseconds elapsed since this timestamp.
    ///
    /// Convenience wrapper around [`elapsed_nanos`](Self::elapsed_nanos).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::timestamp::Timestamp;
    ///
    /// let ts = Timestamp::now();
    /// let _ = ts.elapsed_micros(); // typically 0 immediately after creation
    /// ```
    #[must_use]
    pub fn elapsed_micros(&self) -> u64 {
        self.elapsed_nanos() / 1_000
    }

    /// Returns milliseconds elapsed since this timestamp.
    ///
    /// Convenience wrapper around [`elapsed_nanos`](Self::elapsed_nanos).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::timestamp::Timestamp;
    ///
    /// let ts = Timestamp::now();
    /// let _ = ts.elapsed_millis(); // typically 0 immediately after creation
    /// ```
    #[must_use]
    pub fn elapsed_millis(&self) -> u64 {
        self.elapsed_nanos() / 1_000_000
    }
}

impl fmt::Display for Timestamp {
    /// Formats the timestamp as ISO 8601 with nanosecond precision.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use blazil_common::timestamp::Timestamp;
    ///
    /// let ts = Timestamp::from_nanos(0);
    /// assert_eq!(ts.to_string(), "1970-01-01T00:00:00.000000000Z");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let total_nanos = self.0;
        let secs = total_nanos / 1_000_000_000;
        let nanos_part = (total_nanos % 1_000_000_000) as u32;

        let (year, month, day, hour, min, sec) = unix_secs_to_ymd_hms(secs);
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
            year, month, day, hour, min, sec, nanos_part
        )
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timestamp({})", self)
    }
}

/// Converts a Unix timestamp (seconds since epoch) to calendar components.
///
/// Uses Howard Hinnant's civil-from-days algorithm, which correctly handles
/// the full range of the Gregorian calendar including leap years.
///
/// Returns `(year, month, day, hour, minute, second)`.
fn unix_secs_to_ymd_hms(secs: u64) -> (u32, u8, u8, u8, u8, u8) {
    let days = secs / 86_400;
    let time_of_day = secs % 86_400;

    let h = (time_of_day / 3_600) as u8;
    let m = ((time_of_day % 3_600) / 60) as u8;
    let s = (time_of_day % 60) as u8;

    // Howard Hinnant's civil_from_days algorithm
    // https://howardhinnant.github.io/date_algorithms.html#civil_from_days
    // Shift era so that March 1, 0000 is day 0 of a 400-year cycle.
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // day-of-era  [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // year-of-era [0, 399]
    let y = yoe as i64 + era * 400; // proleptic year (March-based)
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day-of-year [0, 365]
    let mp = (5 * doy + 2) / 153; // month-prime [0, 11] (0 = March)
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8; // day [1, 31]
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u8; // month [1, 12]
    let yr = (if mo <= 2 { y + 1 } else { y }) as u32; // adjust for Jan/Feb

    (yr, mo, d, h, m, s)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_returns_nonzero_timestamp() {
        let ts = Timestamp::now();
        assert!(ts.as_nanos() > 0, "Timestamp::now() returned zero");
    }

    #[test]
    fn from_nanos_round_trips_correctly() {
        let nanos: u64 = 1_741_564_800_123_456_789;
        let ts = Timestamp::from_nanos(nanos);
        assert_eq!(ts.as_nanos(), nanos);
    }

    #[test]
    fn elapsed_nanos_is_nonzero_after_sleep() {
        let ts = Timestamp::now();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let elapsed = ts.elapsed_nanos();
        assert!(elapsed > 0, "elapsed_nanos should be > 0 after a 2ms sleep");
    }

    #[test]
    fn elapsed_micros_nonzero_after_sleep() {
        let ts = Timestamp::now();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(ts.elapsed_micros() > 0);
    }

    #[test]
    fn elapsed_millis_nonzero_after_sleep() {
        let ts = Timestamp::now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(ts.elapsed_millis() > 0);
    }

    #[test]
    fn clock_skew_protection_returns_zero() {
        // A timestamp far in the future — elapsed should be 0, not underflow
        let future = Timestamp::from_nanos(u64::MAX);
        assert_eq!(future.elapsed_nanos(), 0);
    }

    #[test]
    fn ordering_earlier_less_than_later() {
        let t1 = Timestamp::from_nanos(1_000);
        let t2 = Timestamp::from_nanos(2_000);
        assert!(t1 < t2);
        assert!(t2 > t1);
        assert!(t1 <= t1);
        assert_eq!(t1, t1);
    }

    #[test]
    fn display_unix_epoch_is_correct() {
        let ts = Timestamp::from_nanos(0);
        assert_eq!(ts.to_string(), "1970-01-01T00:00:00.000000000Z");
    }

    #[test]
    fn display_known_date_2026_03_09() {
        // 2026-03-09T00:00:00Z = days since epoch:
        // 1970→2026 = 56 years, 14 leap years → 20454 days
        // + Jan (31) + Feb (28) + 8 days = 67 days extra
        // Unix seconds = (20454 + 67) * 86400 = 20521 * 86400 = 1773014400
        let ts = Timestamp::from_nanos(1_773_014_400_000_000_000);
        assert_eq!(ts.to_string(), "2026-03-09T00:00:00.000000000Z");
    }

    #[test]
    fn display_nanosecond_part_is_padded() {
        // 1 nanosecond past epoch
        let ts = Timestamp::from_nanos(1);
        assert_eq!(ts.to_string(), "1970-01-01T00:00:00.000000001Z");
    }

    #[test]
    fn display_leap_day_2000_02_29() {
        // 2000-02-29T00:00:00Z
        // Days from epoch to 2000-01-01: 30 years, 7 leap years (72,76,80,84,88,92,96) = 10957
        // + Jan (31) + Feb 1-28 (28) = 59 more days → day 11016
        let ts = Timestamp::from_nanos(11_016 * 86_400 * 1_000_000_000);
        assert_eq!(ts.to_string(), "2000-02-29T00:00:00.000000000Z");
    }

    #[test]
    fn serde_roundtrip_as_u64() {
        let ts = Timestamp::from_nanos(1_741_564_800_123_456_789);
        let json = serde_json::to_string(&ts).unwrap();
        // Must serialize as a plain u64, not an object
        assert_eq!(json, "1741564800123456789");
        let back: Timestamp = serde_json::from_str(&json).unwrap();
        assert_eq!(ts, back);
    }
}
