//! Embedded Aeron C Media Driver — in-process lifecycle management.
//!
//! Spins up the C media driver in a dedicated Rust thread using a manual
//! `do_work` busy-spin loop for minimum latency.  No `aeronmd` subprocess
//! or external sidecar is required.
//!
//! # Drop ordering
//!
//! Drop all [`super::publication::AeronPublication`]s and
//! [`super::subscription::AeronSubscription`]s *before* dropping the
//! [`super::context::AeronContext`], and drop the context *before* dropping
//! this [`EmbeddedAeronDriver`].

use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use blazil_common::error::{BlazerError, BlazerResult};
use blazil_aeron_sys as sys;

/// IPC shared-memory directory for the embedded driver on Linux (tmpfs).
#[cfg(target_os = "linux")]
const DEFAULT_AERON_DIR: &str = "/dev/shm/aeron-blazil";

/// Fallback IPC directory on non-Linux platforms (macOS dev workstations).
#[cfg(not(target_os = "linux"))]
const DEFAULT_AERON_DIR: &str = "/tmp/aeron-blazil";

// ── EmbeddedAeronDriver ───────────────────────────────────────────────────────

/// In-process Aeron C Media Driver backed by a dedicated busy-spin thread.
///
/// The driver thread calls `aeron_driver_main_do_work` in a tight loop,
/// applying `aeron_driver_main_idle_strategy` to back off when idle.
/// This gives minimum latency without burning 100 % CPU under light load.
pub struct EmbeddedAeronDriver {
    aeron_dir: String,
    stop: Arc<AtomicBool>,
    /// Populated after `start()`; `None` before or after the thread has joined.
    join_handle: Mutex<Option<JoinHandle<()>>>,
}

impl EmbeddedAeronDriver {
    /// Create a new driver handle.  The driver thread is **not** started yet.
    ///
    /// `aeron_dir` is the shared-memory IPC path both the driver and all
    /// clients must agree on.  Defaults to `DEFAULT_AERON_DIR` when `None`.
    pub fn new(aeron_dir: Option<&str>) -> Self {
        Self {
            aeron_dir: aeron_dir.unwrap_or(DEFAULT_AERON_DIR).to_owned(),
            stop: Arc::new(AtomicBool::new(false)),
            join_handle: Mutex::new(None),
        }
    }

    /// The IPC directory string clients must use when constructing their
    /// [`super::context::AeronContext`].
    pub fn aeron_dir(&self) -> &str {
        &self.aeron_dir
    }

    /// Initialise the C driver and spawn the driver thread.
    ///
    /// Blocks for ~100 ms to give the driver time to write its lock file
    /// before any client attempts to connect.
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] if C driver initialisation fails.
    pub fn start(&self) -> BlazerResult<()> {
        let aeron_dir = self.aeron_dir.clone();
        let stop = Arc::clone(&self.stop);

        // Ensure the IPC directory exists before the C driver tries to lock it.
        std::fs::create_dir_all(&aeron_dir).map_err(|e| {
            BlazerError::Transport(format!("Cannot create aeron dir `{aeron_dir}`: {e}"))
        })?;

        let aeron_dir_c = CString::new(aeron_dir.as_str())
            .map_err(|e| BlazerError::Transport(format!("Invalid aeron dir: {e}")))?;

        // ── Initialise driver context ─────────────────────────────────────────
        // SAFETY: aeron_driver_context_init writes into a valid stack variable;
        //         we verify the return value and pointer before use.
        let mut driver_ctx_ptr: *mut sys::aeron_driver_context_t = std::ptr::null_mut();
        let rc = unsafe { sys::aeron_driver_context_init(&mut driver_ctx_ptr) };
        if rc < 0 || driver_ctx_ptr.is_null() {
            return Err(BlazerError::Transport(format!(
                "aeron_driver_context_init failed ({rc}): {}",
                aeron_errmsg_string()
            )));
        }

        // SAFETY: driver_ctx_ptr is non-null (verified above).
        let rc = unsafe {
            sys::aeron_driver_context_set_dir(driver_ctx_ptr, aeron_dir_c.as_ptr())
        };
        if rc < 0 {
            // SAFETY: driver_ctx_ptr is non-null; error-path cleanup.
            unsafe { sys::aeron_driver_context_close(driver_ctx_ptr) };
            return Err(BlazerError::Transport(format!(
                "aeron_driver_context_set_dir failed ({rc}): {}",
                aeron_errmsg_string()
            )));
        }

        // Delete and recreate the IPC directory on start for a clean slate.
        // SAFETY: driver_ctx_ptr is non-null.
        unsafe {
            sys::aeron_driver_context_set_dir_delete_on_start(driver_ctx_ptr, true);
        }

        // ── Initialise driver struct ──────────────────────────────────────────
        // SAFETY: driver_ctx_ptr is non-null and fully configured.
        let mut driver_ptr: *mut sys::aeron_driver_t = std::ptr::null_mut();
        let rc = unsafe { sys::aeron_driver_init(&mut driver_ptr, driver_ctx_ptr) };
        if rc < 0 || driver_ptr.is_null() {
            // SAFETY: error-path cleanup.
            unsafe { sys::aeron_driver_context_close(driver_ctx_ptr) };
            return Err(BlazerError::Transport(format!(
                "aeron_driver_init failed ({rc}): {}",
                aeron_errmsg_string()
            )));
        }

        // ── Start the driver (manual main loop) ───────────────────────────────
        // SAFETY: driver_ptr is non-null (verified above).
        let rc = unsafe { sys::aeron_driver_start(driver_ptr, true) };
        if rc < 0 {
            // SAFETY: error-path cleanup.
            unsafe {
                sys::aeron_driver_close(driver_ptr);
                sys::aeron_driver_context_close(driver_ctx_ptr);
            }
            return Err(BlazerError::Transport(format!(
                "aeron_driver_start failed ({rc}): {}",
                aeron_errmsg_string()
            )));
        }

        // ── Spawn dedicated driver thread ─────────────────────────────────────
        // Convert raw pointers to usize so they can cross the thread boundary.
        // This is safe because:
        //   • The C driver structs live until the driver thread exits (which
        //     happens before the Rust driver thread is joined in `stop()`).
        //   • Only this thread ever dereferences these pointers after the move.
        //   • The driver_ctx is only freed by the thread after driver_close().
        let driver_usize = driver_ptr as usize;
        let driver_ctx_usize = driver_ctx_ptr as usize;

        let join_handle = thread::Builder::new()
            .name("aeron-driver".to_owned())
            .spawn(move || {
                tracing::info!("Aeron embedded driver thread started");
                driver_thread_body(driver_usize, driver_ctx_usize, stop);
                tracing::info!("Aeron embedded driver thread exited");
            })
            .map_err(|e| {
                BlazerError::Transport(format!("Failed to spawn aeron-driver thread: {e}"))
            })?;

        *self.join_handle.lock().unwrap() = Some(join_handle);

        // Allow the driver 100 ms to write its lock file before clients connect.
        std::thread::sleep(std::time::Duration::from_millis(100));

        tracing::info!(dir = %self.aeron_dir, "Aeron embedded driver started");
        Ok(())
    }

    /// Signal the driver thread to stop and wait for it to join.
    ///
    /// Call this **after** all clients, publications, and subscriptions have
    /// been dropped.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.join_handle.lock().unwrap().take() {
            let _ = handle.join();
        }
        tracing::info!("Aeron embedded driver stopped");
    }
}

impl Drop for EmbeddedAeronDriver {
    fn drop(&mut self) {
        // Idempotent — safe to call even if `stop()` was already called.
        if !self.stop.load(Ordering::Acquire) {
            self.stop();
        }
    }
}

// ── Driver thread body ────────────────────────────────────────────────────────

/// Entry point for the dedicated driver thread.
///
/// Runs `aeron_driver_main_do_work` in a busy-spin loop, applying
/// `aeron_driver_main_idle_strategy` to avoid wasting CPU when idle.
fn driver_thread_body(driver_usize: usize, driver_ctx_usize: usize, stop: Arc<AtomicBool>) {
    // SAFETY: These pointers were valid when sent here and remain alive for
    //         the full lifetime of this function.  No other thread accesses them.
    let driver = driver_usize as *mut sys::aeron_driver_t;
    let driver_ctx = driver_ctx_usize as *mut sys::aeron_driver_context_t;

    while !stop.load(Ordering::Acquire) {
        // SAFETY: driver is non-null and properly initialised.
        let work_count = unsafe { sys::aeron_driver_main_do_work(driver) };
        // SAFETY: driver is non-null.
        unsafe { sys::aeron_driver_main_idle_strategy(driver, work_count) };
    }

    tracing::debug!("Aeron driver thread: performing shutdown cleanup");

    // SAFETY: driver is non-null; no other code accesses it after this point.
    unsafe { sys::aeron_driver_close(driver) };

    // SAFETY: driver_ctx is non-null; driver has been closed above.
    unsafe { sys::aeron_driver_context_close(driver_ctx) };
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Read the last Aeron error message from thread-local C storage.
pub(crate) fn aeron_errmsg_string() -> String {
    // SAFETY: aeron_errmsg() always returns a non-null, thread-local static
    //         C string owned by the Aeron library.  We copy it to a Rust
    //         String immediately and do not retain the pointer.
    unsafe {
        let ptr = sys::aeron_errmsg();
        if ptr.is_null() {
            return "(null error message)".to_owned();
        }
        std::ffi::CStr::from_ptr(ptr)
            .to_string_lossy()
            .into_owned()
    }
}
