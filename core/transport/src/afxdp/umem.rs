//! AF_XDP UMEM — shared DMA memory region between the NIC and userspace.
//!
//! # What is UMEM?
//!
//! UMEM is a large, contiguous, page-aligned memory region that is:
//!   1. `mmap`-allocated in userspace
//!   2. `mlock`-pinned to prevent swap (required for AF_XDP zero-copy)
//!   3. Registered with the kernel via `setsockopt(XDP_UMEM_REG)`
//!   4. DMA-mapped by the NIC driver (zero-copy mode)
//!
//! The region is divided into equal-sized frames.  The NIC writes received
//! data directly into UMEM frames — **no copy from NIC to kernel, no copy
//! from kernel to userspace** (XDP_COPY mode would copy; we use `XDP_ZEROCOPY`).
//!
//! # Frame lifecycle
//!
//! ```text
//! 1. Startup: all frames → FillQueue (tell kernel "these are free")
//! 2. NIC receives pkt → DMA into a free UMEM frame
//! 3. Kernel writes frame descriptor → RxQueue (available to userspace)
//! 4. Userspace reads descriptor, reads frame bytes (zero-copy!)
//! 5. Userspace returns descriptor → FillQueue  (frame is free again)
//! ```
//!
//! # Safety
//!
//! - UMEM memory is owned by `OwnedUmem` via `xsk_rs::Umem`.
//! - Frame data is only accessed while the descriptor is "owned" by userspace
//!   (between steps 3 and 5 above).
//! - `OwnedUmem` is `!Send` intentionally: the raw frame slices produced by
//!   `frame_data()` borrow from the UMEM mmap region.  Callers that need to
//!   share UMEM across threads must wrap in `Arc<OwnedUmem>` and ensure the
//!   borrowed slice is consumed before returning the descriptor to the fill queue.

use std::num::NonZeroU32;

use xsk_rs::{
    config::{FrameSize, UmemConfig},
    umem::Umem,
    FrameDesc,
};

use blazil_common::error::{BlazerError, BlazerResult};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Each UMEM frame holds one Ethernet frame.  2048 bytes covers the common
/// 1500-byte MTU plus all headers.  Must be a power of 2 ≥ 2048 per the
/// kernel AF_XDP ABI.
pub const FRAME_SIZE: u32 = 2048;

/// Total UMEM frames = 64 K.  64K × 2048 B = 128 MiB.
/// On i4i.metal (1.5 TiB RAM) this is negligible.
/// Divided across queues: 8 K fill + 8 K comp + extras per queue.
pub const FRAME_COUNT: u32 = 65_536;

/// Fill-ring entries per UMEM: tells the kernel how many frames we can offer
/// at once.  Must divide FRAME_COUNT and be a power of 2.
pub const FILL_RING_SIZE: u32 = 4096;

/// Completion-ring entries per UMEM (TX path).  Same power-of-2 constraint.
pub const COMP_RING_SIZE: u32 = 4096;

// ── OwnedUmem ─────────────────────────────────────────────────────────────────

/// Owner of the AF_XDP UMEM region.
///
/// Wrap in `Arc<OwnedUmem>` and share across queue worker threads.
/// Frames produced by `frame_data()` are valid until the descriptor is
/// returned to the fill queue.
pub struct OwnedUmem {
    inner: Umem,
    /// All frame descriptors for this UMEM.  On startup the caller pops them
    /// in batches and pushes to the FillQueue to hand them to the kernel.
    pub free_descs: Vec<FrameDesc>,
}

impl OwnedUmem {
    /// Allocate and register a new UMEM region.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - `mmap` fails (unlikely: we only need 128 MiB)
    /// - `mlock` fails — ensure `ulimit -l unlimited` before calling
    ///   (`v0.4_aws_setup.sh` sets this)
    /// - Kernel rejects the UMEM registration (needs kernel 4.18+
    ///   with `CONFIG_XDP_SOCKETS=y`)
    pub fn new() -> BlazerResult<Self> {
        let frame_count = NonZeroU32::new(FRAME_COUNT)
            .expect("FRAME_COUNT > 0");

        let config = UmemConfig::builder()
            .frame_size(FrameSize::TwoKiloBytes)
            .fill_queue_size(NonZeroU32::new(FILL_RING_SIZE).unwrap())
            .comp_queue_size(NonZeroU32::new(COMP_RING_SIZE).unwrap())
            .build()
            .map_err(|e| BlazerError::Transport(format!("UMEM config: {e}")))?;

        // SAFETY: Umem::new mmap-allocates and mlocks the region.
        // The bool argument (use_huge_pages) is false here; we configure
        // huge pages at the OS level via v0.4_aws_setup.sh so the kernel's
        // THP daemon handles it transparently.
        let (inner, free_descs) = unsafe {
            Umem::new(config, frame_count, false)
                .map_err(|e| BlazerError::Transport(format!("UMEM alloc: {e}")))?
        };

        Ok(Self { inner, free_descs })
    }

    /// Return a read-only byte slice for the given frame descriptor.
    ///
    /// # Safety
    ///
    /// The caller must guarantee:
    /// - `desc` was received from the RX ring (kernel gave it to userspace).
    /// - No other thread is simultaneously writing to this frame.
    /// - The slice is not held past the point where `desc` is returned to
    ///   the fill queue.
    ///
    /// These invariants are upheld by the single-threaded RX cursor in
    /// `rx.rs`: each desc is processed then immediately re-queued.
    #[inline]
    pub unsafe fn frame_data(&self, desc: &FrameDesc) -> &[u8] {
        self.inner.data(desc)
    }

    /// Expose the inner [`Umem`] for socket binding.
    pub fn inner(&self) -> &Umem {
        &self.inner
    }
}
