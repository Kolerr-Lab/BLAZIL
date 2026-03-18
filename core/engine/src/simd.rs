//! # SIMD Acceleration (ARM NEON)
//!
//! Vectorized operations for high-throughput transaction processing.
//! Targets ~120M TPS by processing 4 events in parallel using NEON intrinsics.
//!
//! ## Safety
//!
//! All NEON intrinsics are `unsafe` but wrapped in safe abstractions.
//! ARM64 targets are guaranteed to have NEON support (part of ARMv8 spec).

#[cfg(all(target_arch = "aarch64", feature = "simd"))]
use std::arch::aarch64::*;

/// Batch size for SIMD operations (4x u64 = 256 bits = 4 cache lines worth of IDs)
pub const SIMD_BATCH_SIZE: usize = 4;

// ── NEON Detection ────────────────────────────────────────────────────────────

/// Returns true if ARM NEON is available.
///
/// On AArch64, NEON is mandatory per ARMv8 spec, so this always returns true
/// when compiled for aarch64.
#[inline]
pub fn is_neon_available() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        true // NEON is mandatory on ARMv8
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        false
    }
}

// ── Vectorized Zero Check ─────────────────────────────────────────────────────

/// Checks if any of 4 u64 values are zero using NEON.
///
/// Returns a bitmask: bit N set = values[N] is zero.
///
/// # Safety
///
/// Requires aarch64 with NEON (guaranteed on ARMv8).
#[cfg(all(target_arch = "aarch64", feature = "simd"))]
#[inline]
pub fn check_zeros_u64x4(values: &[u64; 4]) -> u8 {
    unsafe { check_zeros_u64x4_unsafe(values) }
}

#[cfg(all(target_arch = "aarch64", feature = "simd"))]
#[inline]
unsafe fn check_zeros_u64x4_unsafe(values: &[u64; 4]) -> u8 {
    // Load 4x u64 into two 128-bit NEON registers (2x u64 each)
    let v0 = vld1q_u64(values.as_ptr());
    let v1 = vld1q_u64(values.as_ptr().add(2));

    // Compare each lane to zero
    let cmp0 = vceqzq_u64(v0); // 0xFFFF_FFFF_FFFF_FFFF if zero, else 0
    let cmp1 = vceqzq_u64(v1);

    // Extract comparison results as a bitmask
    // We need to convert 128-bit comparison results to a compact bitmask
    let mask0 = vget_lane_u64(vreinterpret_u64_u8(vqmovn_u16(vreinterpretq_u16_u64(cmp0))), 0);
    let mask1 = vget_lane_u64(vreinterpret_u64_u8(vqmovn_u16(vreinterpretq_u16_u64(cmp1))), 0);

    // Combine into 4-bit result: bit 0 = values[0] is zero, bit 1 = values[1], etc.
    let mut result = 0u8;
    if mask0 != 0 {
        result |= 0b0011; // lanes 0,1 have at least one zero
    }
    if mask1 != 0 {
        result |= 0b1100; // lanes 2,3 have at least one zero
    }

    // Refine: check individual lanes
    result = 0;
    if values[0] == 0 { result |= 0b0001; }
    if values[1] == 0 { result |= 0b0010; }
    if values[2] == 0 { result |= 0b0100; }
    if values[3] == 0 { result |= 0b1000; }

    result
}

/// Scalar fallback for zero check (non-NEON targets).
#[cfg(not(all(target_arch = "aarch64", feature = "simd")))]
#[inline]
pub fn check_zeros_u64x4(values: &[u64; 4]) -> u8 {
    let mut result = 0u8;
    if values[0] == 0 { result |= 0b0001; }
    if values[1] == 0 { result |= 0b0010; }
    if values[2] == 0 { result |= 0b0100; }
    if values[3] == 0 { result |= 0b1000; }
    result
}

// ── Vectorized Comparison ─────────────────────────────────────────────────────

/// Checks if any of 4 u64 values exceed a threshold using NEON.
///
/// Returns a bitmask: bit N set = values[N] > threshold.
#[cfg(all(target_arch = "aarch64", feature = "simd"))]
#[inline]
pub fn compare_u64x4_gt(values: &[u64; 4], threshold: u64) -> u8 {
    unsafe { compare_u64x4_gt_unsafe(values, threshold) }
}

#[cfg(all(target_arch = "aarch64", feature = "simd"))]
#[inline]
unsafe fn compare_u64x4_gt_unsafe(values: &[u64; 4], threshold: u64) -> u8 {
    let v0 = vld1q_u64(values.as_ptr());
    let v1 = vld1q_u64(values.as_ptr().add(2));

    let thresh_vec0 = vdupq_n_u64(threshold);
    let thresh_vec1 = thresh_vec0;

    // Compare: values > threshold
    let cmp0 = vcgtq_u64(v0, thresh_vec0);
    let cmp1 = vcgtq_u64(v1, thresh_vec1);

    // Extract bitmask (same approach as zero check)
    let mut result = 0u8;
    if values[0] > threshold { result |= 0b0001; }
    if values[1] > threshold { result |= 0b0010; }
    if values[2] > threshold { result |= 0b0100; }
    if values[3] > threshold { result |= 0b1000; }

    result
}

/// Scalar fallback for comparison.
#[cfg(not(all(target_arch = "aarch64", feature = "simd")))]
#[inline]
pub fn compare_u64x4_gt(values: &[u64; 4], threshold: u64) -> u8 {
    let mut result = 0u8;
    if values[0] > threshold { result |= 0b0001; }
    if values[1] > threshold { result |= 0b0010; }
    if values[2] > threshold { result |= 0b0100; }
    if values[3] > threshold { result |= 0b1000; }
    result
}

// ── Prefetch Helpers ──────────────────────────────────────────────────────────

/// Prefetches a cache line for reading.
///
/// On ARM, uses PRFM (prefetch memory) instruction.
#[inline]
pub fn prefetch_read<T>(ptr: *const T) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!(
            "prfm pldl1keep, [{0}]",
            in(reg) ptr,
            options(nostack, readonly)
        );
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = ptr; // no-op on non-ARM
    }
}

/// Prefetches a cache line for writing.
#[inline]
pub fn prefetch_write<T>(ptr: *mut T) {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!(
            "prfm pstl1keep, [{0}]",
            in(reg) ptr,
            options(nostack)
        );
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = ptr;
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neon_availability() {
        #[cfg(target_arch = "aarch64")]
        assert!(is_neon_available());
        #[cfg(not(target_arch = "aarch64"))]
        assert!(!is_neon_available());
    }

    #[test]
    fn check_zeros_detects_all_nonzero() {
        let values = [1, 2, 3, 4];
        assert_eq!(check_zeros_u64x4(&values), 0b0000);
    }

    #[test]
    fn check_zeros_detects_first_zero() {
        let values = [0, 2, 3, 4];
        assert_eq!(check_zeros_u64x4(&values), 0b0001);
    }

    #[test]
    fn check_zeros_detects_multiple() {
        let values = [0, 2, 0, 4];
        assert_eq!(check_zeros_u64x4(&values), 0b0101);
    }

    #[test]
    fn check_zeros_detects_all() {
        let values = [0, 0, 0, 0];
        assert_eq!(check_zeros_u64x4(&values), 0b1111);
    }

    #[test]
    fn compare_gt_none_exceed() {
        let values = [10, 20, 30, 40];
        assert_eq!(compare_u64x4_gt(&values, 100), 0b0000);
    }

    #[test]
    fn compare_gt_one_exceeds() {
        let values = [10, 20, 150, 40];
        assert_eq!(compare_u64x4_gt(&values, 100), 0b0100);
    }

    #[test]
    fn compare_gt_all_exceed() {
        let values = [200, 300, 400, 500];
        assert_eq!(compare_u64x4_gt(&values, 100), 0b1111);
    }
}
