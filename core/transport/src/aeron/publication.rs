//! Safe `AeronPublication` wrapper — outbound channel.
//!
//! Wraps `aeron_publication_t` and provides a safe `offer(data: &[u8])` method
//! that busy-spins on `BACK_PRESSURED` / `ADMIN_ACTION` up to a configurable
//! timeout.
//!
//! # Drop ordering
//!
//! Drop this *before* the [`super::context::AeronContext`] it was created from.

use std::ffi::CString;
use std::time::{Duration, Instant};

use blazil_aeron_sys as sys;
use blazil_common::error::{BlazerError, BlazerResult};

use super::context::AeronContext;
use super::driver::aeron_errmsg_string;

/// Maximum time to busy-spin waiting for back-pressure / admin-action to clear.
const BACK_PRESSURE_SPIN_TIMEOUT: Duration = Duration::from_millis(50);

// ── AeronPublication ──────────────────────────────────────────────────────────

/// Safe wrapper around `aeron_publication_t`.
///
/// **Not `Send` or `Sync`** — must be used from the thread that created it.
pub struct AeronPublication {
    /// Non-null while alive.
    ptr: *mut sys::aeron_publication_t,
}

impl AeronPublication {
    /// Create a publication on `channel` / `stream_id` using the given client context.
    ///
    /// Polls until the driver confirms the registration (busy-spin up to
    /// `timeout`).  Returns an error if the timeout elapses.
    pub fn new(
        ctx: &AeronContext,
        channel: &str,
        stream_id: i32,
        timeout: Duration,
    ) -> BlazerResult<Self> {
        let channel_c = CString::new(channel)
            .map_err(|e| BlazerError::Transport(format!("Invalid channel URI: {e}")))?;

        // ── Async add ─────────────────────────────────────────────────────────
        // SAFETY: aeron_async_add_publication writes into a valid stack variable;
        //         the client pointer is non-null for the duration of `ctx`.
        let mut async_ptr: *mut sys::aeron_async_add_publication_t = std::ptr::null_mut();
        let rc = unsafe {
            sys::aeron_async_add_publication(
                &mut async_ptr,
                ctx.client_ptr(),
                channel_c.as_ptr(),
                stream_id,
            )
        };
        if rc < 0 {
            return Err(BlazerError::Transport(format!(
                "aeron_async_add_publication failed ({rc}): {}",
                aeron_errmsg_string()
            )));
        }

        // ── Poll until ready ──────────────────────────────────────────────────
        let deadline = Instant::now() + timeout;
        let mut pub_ptr: *mut sys::aeron_publication_t = std::ptr::null_mut();
        loop {
            // SAFETY: async_ptr is non-null; pub_ptr is written on completion.
            let rc = unsafe { sys::aeron_async_poll_publication(&mut pub_ptr, async_ptr) };
            match rc {
                1 if !pub_ptr.is_null() => break,
                0 => {
                    // Still pending — busy spin.
                    std::hint::spin_loop();
                }
                _ => {
                    return Err(BlazerError::Transport(format!(
                        "aeron_async_poll_publication error ({rc}): {}",
                        aeron_errmsg_string()
                    )));
                }
            }
            if Instant::now() >= deadline {
                return Err(BlazerError::Transport(format!(
                    "Aeron publication `{channel}` stream {stream_id}: timeout waiting for registration"
                )));
            }
        }

        tracing::debug!(channel, stream_id, "Aeron publication registered");
        Ok(Self { ptr: pub_ptr })
    }

    /// Offer `data` to all connected subscribers.
    ///
    /// Busy-spins on `BACK_PRESSURED` and `ADMIN_ACTION` up to
    /// [`BACK_PRESSURE_SPIN_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// Returns [`BlazerError::Transport`] if:
    /// - No subscriber is connected (`AERON_PUBLICATION_NOT_CONNECTED`).
    /// - The publication is closed (`AERON_PUBLICATION_CLOSED`).
    /// - An unrecoverable error occurs (`AERON_PUBLICATION_ERROR`).
    /// - Back-pressure / admin-action persists past the timeout.
    pub fn offer(&self, data: &[u8]) -> BlazerResult<i64> {
        let deadline = Instant::now() + BACK_PRESSURE_SPIN_TIMEOUT;
        loop {
            // SAFETY: self.ptr is non-null (invariant maintained by `new` and `drop`).
            //         data is a valid slice; we pass its len and ptr together.
            let pos = unsafe {
                sys::aeron_publication_offer(
                    self.ptr,
                    data.as_ptr(),
                    data.len(),
                    None,
                    std::ptr::null_mut(),
                )
            };

            match pos {
                p if p >= 0 => return Ok(p),
                sys::AERON_PUBLICATION_BACK_PRESSURED | sys::AERON_PUBLICATION_ADMIN_ACTION => {
                    // Transient — spin and retry.
                    std::hint::spin_loop();
                    if Instant::now() >= deadline {
                        return Err(BlazerError::Transport(
                            "Aeron publication: back-pressure timeout".to_owned(),
                        ));
                    }
                }
                sys::AERON_PUBLICATION_NOT_CONNECTED => {
                    return Err(BlazerError::Transport(
                        "Aeron publication: no subscriber connected".to_owned(),
                    ));
                }
                sys::AERON_PUBLICATION_CLOSED => {
                    return Err(BlazerError::Transport(
                        "Aeron publication: publication closed".to_owned(),
                    ));
                }
                _ => {
                    return Err(BlazerError::Transport(format!(
                        "Aeron publication: unrecoverable error ({}): {}",
                        pos,
                        aeron_errmsg_string()
                    )));
                }
            }
        }
    }

    /// Returns `true` if at least one subscriber is connected.
    pub fn is_connected(&self) -> bool {
        // SAFETY: self.ptr is non-null.
        unsafe { sys::aeron_publication_is_connected(self.ptr) }
    }
}

impl Drop for AeronPublication {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: self.ptr is non-null and we hold the only reference.
            //         aeron_publication_close initiates an async close on the
            //         conductor thread; no notification callback needed here.
            unsafe {
                sys::aeron_publication_close(self.ptr, None, std::ptr::null_mut());
            }
            self.ptr = std::ptr::null_mut();
        }
    }
}
