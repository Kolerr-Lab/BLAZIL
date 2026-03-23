//! Safe `AeronSubscription` wrapper — inbound channel.
//!
//! Wraps `aeron_subscription_t` and provides a safe `poll_fragments` method
//! that collects received raw bytes into a `Vec<Vec<u8>>`, avoiding any
//! closure-in-C-callback complexity through a trampoline function.
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

// ── Trampoline ────────────────────────────────────────────────────────────────

/// C-compatible fragment handler that appends each received fragment to a
/// `Vec<Vec<u8>>` whose address is passed as `clientd`.
///
/// # Safety
///
/// The caller must ensure `clientd` is a valid `*mut Vec<Vec<u8>>` for the
/// duration of the `aeron_subscription_poll` call.
unsafe extern "C" fn collect_fragment(
    clientd: *mut std::ffi::c_void,
    buffer: *const u8,
    length: usize,
    _header: *mut sys::aeron_header_t,
) {
    // SAFETY: clientd is always &mut Vec<Vec<u8>> — see poll_fragments below.
    let out = &mut *(clientd as *mut Vec<Vec<u8>>);
    let data = std::slice::from_raw_parts(buffer, length);
    out.push(data.to_vec());
}

// ── AeronSubscription ─────────────────────────────────────────────────────────

/// Safe wrapper around `aeron_subscription_t`.
///
/// **Not `Send` or `Sync`** — must be used from the thread that created it.
pub struct AeronSubscription {
    /// Non-null while alive.
    ptr: *mut sys::aeron_subscription_t,
}

impl AeronSubscription {
    /// Create a subscription on `channel` / `stream_id` using the given client context.
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
        // SAFETY: aeron_async_add_subscription writes into stack variables; all
        //         pointer arguments are either non-null (ctx, channel_c) or
        //         explicitly null (no image handlers needed).
        let mut async_ptr: *mut sys::aeron_async_add_subscription_t = std::ptr::null_mut();
        let rc = unsafe {
            sys::aeron_async_add_subscription(
                &mut async_ptr,
                ctx.client_ptr(),
                channel_c.as_ptr(),
                stream_id,
                None,                 // on_available_image
                std::ptr::null_mut(), // on_available_image clientd
                None,                 // on_unavailable_image
                std::ptr::null_mut(), // on_unavailable_image clientd
            )
        };
        if rc < 0 {
            return Err(BlazerError::Transport(format!(
                "aeron_async_add_subscription failed ({rc}): {}",
                aeron_errmsg_string()
            )));
        }

        // ── Poll until ready ──────────────────────────────────────────────────
        let deadline = Instant::now() + timeout;
        let mut sub_ptr: *mut sys::aeron_subscription_t = std::ptr::null_mut();
        loop {
            // SAFETY: async_ptr is non-null; sub_ptr is set on completion.
            let rc = unsafe { sys::aeron_async_poll_subscription(&mut sub_ptr, async_ptr) };
            match rc {
                1 if !sub_ptr.is_null() => break,
                0 => {
                    std::hint::spin_loop();
                }
                _ => {
                    return Err(BlazerError::Transport(format!(
                        "aeron_async_poll_subscription error ({rc}): {}",
                        aeron_errmsg_string()
                    )));
                }
            }
            if Instant::now() >= deadline {
                return Err(BlazerError::Transport(format!(
                    "Aeron subscription `{channel}` stream {stream_id}: timeout waiting for registration"
                )));
            }
        }

        tracing::debug!(channel, stream_id, "Aeron subscription registered");
        Ok(Self { ptr: sub_ptr })
    }

    /// Poll for up to `fragment_limit` received fragments per call.
    ///
    /// Appends each fragment's bytes (copied) to `out`.  Returns the number of
    /// fragments received in this call (may be 0 if no messages are available).
    pub fn poll_fragments(&self, out: &mut Vec<Vec<u8>>, fragment_limit: usize) -> i32 {
        // SAFETY:
        //   * self.ptr is non-null (invariant).
        //   * collect_fragment only accesses `clientd` as *mut Vec<Vec<u8>>;
        //     `out` outlives this call and is on the current thread's stack.
        //   * The poll does not move `out`; only push is called inside the callback.
        unsafe {
            sys::aeron_subscription_poll(
                self.ptr,
                Some(collect_fragment),
                out as *mut Vec<Vec<u8>> as *mut std::ffi::c_void,
                fragment_limit,
            )
        }
    }

    /// Returns `true` if at least one publisher image is connected.
    pub fn is_connected(&self) -> bool {
        // SAFETY: self.ptr is non-null.
        unsafe { sys::aeron_subscription_is_connected(self.ptr) }
    }
}

impl Drop for AeronSubscription {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: self.ptr is non-null and we hold the only reference.
            unsafe {
                sys::aeron_subscription_close(self.ptr, None, std::ptr::null_mut());
            }
            self.ptr = std::ptr::null_mut();
        }
    }
}
