//! Aeron client context — wraps `aeron_context_t` and `aeron_t`.
//!
//! [`AeronContext`] connects to the media driver over the IPC directory
//! established by [`super::driver::EmbeddedAeronDriver`].  It is the root from
//! which publications and subscriptions are created.
//!
//! # Drop ordering
//!
//! Drop all publications and subscriptions *before* dropping this context.
//! Dropping the context calls `aeron_close`, which unregisters all resources
//! with the media driver.

use std::ffi::CString;

use blazil_common::error::{BlazerError, BlazerResult};
use blazil_aeron_sys as sys;

use super::driver::aeron_errmsg_string;

// ── AeronContext ──────────────────────────────────────────────────────────────

/// Safe wrapper around `aeron_context_t` + `aeron_t`.
///
/// **Not `Send` or `Sync`** — the Aeron C client must be driven from a single
/// thread (the blocking transport thread created by `spawn_blocking`).
pub struct AeronContext {
    /// Raw context pointer.  Non-null while alive.
    ctx_ptr: *mut sys::aeron_context_t,
    /// Raw client pointer.  Non-null while alive.
    client_ptr: *mut sys::aeron_t,
}

impl AeronContext {
    /// Construct and start an Aeron client that connects to the driver at
    /// `aeron_dir` (must match the directory used by [`super::driver::EmbeddedAeronDriver`]).
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] if the C context or client cannot be
    /// initialised, or if the client fails to start its conductor thread.
    pub fn new(aeron_dir: &str) -> BlazerResult<Self> {
        let aeron_dir_c = CString::new(aeron_dir)
            .map_err(|e| BlazerError::Transport(format!("Invalid aeron dir: {e}")))?;

        // ── Initialise context ────────────────────────────────────────────────
        // SAFETY: aeron_context_init writes into a valid stack variable;
        //         we verify the return value and pointer before use.
        let mut ctx_ptr: *mut sys::aeron_context_t = std::ptr::null_mut();
        let rc = unsafe { sys::aeron_context_init(&mut ctx_ptr) };
        if rc < 0 || ctx_ptr.is_null() {
            return Err(BlazerError::Transport(format!(
                "aeron_context_init failed ({rc}): {}",
                aeron_errmsg_string()
            )));
        }

        // SAFETY: ctx_ptr is non-null (verified above).
        let rc =
            unsafe { sys::aeron_context_set_dir(ctx_ptr, aeron_dir_c.as_ptr()) };
        if rc < 0 {
            // SAFETY: error-path cleanup.
            unsafe { sys::aeron_context_close(ctx_ptr) };
            return Err(BlazerError::Transport(format!(
                "aeron_context_set_dir failed ({rc}): {}",
                aeron_errmsg_string()
            )));
        }

        // ── Initialise client ─────────────────────────────────────────────────
        // SAFETY: ctx_ptr is non-null and configured.
        let mut client_ptr: *mut sys::aeron_t = std::ptr::null_mut();
        let rc = unsafe { sys::aeron_init(&mut client_ptr, ctx_ptr) };
        if rc < 0 || client_ptr.is_null() {
            // SAFETY: error-path cleanup.
            unsafe { sys::aeron_context_close(ctx_ptr) };
            return Err(BlazerError::Transport(format!(
                "aeron_init failed ({rc}): {}",
                aeron_errmsg_string()
            )));
        }

        // SAFETY: client_ptr is non-null (verified above).
        let rc = unsafe { sys::aeron_start(client_ptr) };
        if rc < 0 {
            // SAFETY: error-path cleanup (close client before context).
            unsafe {
                sys::aeron_close(client_ptr);
                sys::aeron_context_close(ctx_ptr);
            }
            return Err(BlazerError::Transport(format!(
                "aeron_start failed ({rc}): {}",
                aeron_errmsg_string()
            )));
        }

        Ok(Self {
            ctx_ptr,
            client_ptr,
        })
    }

    /// Raw pointer to the `aeron_t` client.
    ///
    /// # Safety
    ///
    /// The returned pointer is valid as long as this [`AeronContext`] is alive.
    /// Callers must not retain the pointer beyond the lifetime of `self`.
    pub(crate) fn client_ptr(&self) -> *mut sys::aeron_t {
        self.client_ptr
    }
}

impl Drop for AeronContext {
    fn drop(&mut self) {
        if !self.client_ptr.is_null() {
            // SAFETY: client_ptr is non-null; we hold the only reference.
            //         aeron_close unregisters all streams with the driver.
            unsafe { sys::aeron_close(self.client_ptr) };
            self.client_ptr = std::ptr::null_mut();
        }
        if !self.ctx_ptr.is_null() {
            // SAFETY: ctx_ptr is non-null; client has been closed above.
            unsafe { sys::aeron_context_close(self.ctx_ptr) };
            self.ctx_ptr = std::ptr::null_mut();
        }
    }
}
