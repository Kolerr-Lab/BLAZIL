// Pre-generated Aeron C API bindings for Blazil.
// Source: real-logic/aeron tag 1.44.1
// Generated: 2026-03-24 via bindgen 0.70 with allowlist pattern.
// Re-generate: cargo build --features regenerate-bindings -p blazil-aeron-sys

use std::ffi::{c_char, c_int, c_void};

// ── Opaque C types ─────────────────────────────────────────────────────────────

/// Aeron client context (configuration before connecting to the driver).
#[repr(C)]
pub struct aeron_context_stct {
    _private: [u8; 0],
}
pub type aeron_context_t = aeron_context_stct;

/// Connected Aeron client instance.
#[repr(C)]
pub struct aeron_stct {
    _private: [u8; 0],
}
pub type aeron_t = aeron_stct;

/// Aeron publication (outbound channel to subscribers).
#[repr(C)]
pub struct aeron_publication_stct {
    _private: [u8; 0],
}
pub type aeron_publication_t = aeron_publication_stct;

/// Aeron subscription (inbound channel from publishers).
#[repr(C)]
pub struct aeron_subscription_stct {
    _private: [u8; 0],
}
pub type aeron_subscription_t = aeron_subscription_stct;

/// Media driver context (driver configuration before initialisation).
#[repr(C)]
pub struct aeron_driver_context_stct {
    _private: [u8; 0],
}
pub type aeron_driver_context_t = aeron_driver_context_stct;

/// Aeron C Media Driver instance.
#[repr(C)]
pub struct aeron_driver_stct {
    _private: [u8; 0],
}
pub type aeron_driver_t = aeron_driver_stct;

/// Per-fragment metadata header.
#[repr(C)]
pub struct aeron_header_stct {
    _private: [u8; 0],
}
pub type aeron_header_t = aeron_header_stct;

/// A single publisher's stream as seen by a subscriber.
#[repr(C)]
pub struct aeron_image_stct {
    _private: [u8; 0],
}
pub type aeron_image_t = aeron_image_stct;

/// Async handle returned while a publication registration is in progress.
#[repr(C)]
pub struct aeron_async_add_publication_stct {
    _private: [u8; 0],
}
pub type aeron_async_add_publication_t = aeron_async_add_publication_stct;

/// Async handle returned while a subscription registration is in progress.
#[repr(C)]
pub struct aeron_async_add_subscription_stct {
    _private: [u8; 0],
}
pub type aeron_async_add_subscription_t = aeron_async_add_subscription_stct;

// ── Publication offer sentinel values ─────────────────────────────────────────

/// No subscriber is currently connected to this publication.
pub const AERON_PUBLICATION_NOT_CONNECTED: i64 = -1;
/// The publication's term buffer is full; retry after yielding.
pub const AERON_PUBLICATION_BACK_PRESSURED: i64 = -2;
/// Admin action in progress; retry is safe.
pub const AERON_PUBLICATION_ADMIN_ACTION: i64 = -3;
/// The publication has been closed.
pub const AERON_PUBLICATION_CLOSED: i64 = -4;
/// The maximum stream position has been exceeded.
pub const AERON_PUBLICATION_MAX_POSITION_EXCEEDED: i64 = -5;
/// An unrecoverable error occurred.
pub const AERON_PUBLICATION_ERROR: i64 = -6;

// ── Callback / handler types ───────────────────────────────────────────────────

/// Called once per received fragment from `aeron_subscription_poll`.
pub type aeron_fragment_handler_t = ::std::option::Option<
    unsafe extern "C" fn(
        clientd: *mut c_void,
        buffer: *const u8,
        length: usize,
        header: *mut aeron_header_t,
    ),
>;

/// Error notification callback on the client context.
pub type aeron_error_handler_t = ::std::option::Option<
    unsafe extern "C" fn(clientd: *mut c_void, errcode: c_int, message: *const c_char),
>;

/// Async completion notification (e.g. publication/subscription close).
pub type aeron_notification_t =
    ::std::option::Option<unsafe extern "C" fn(clientd: *mut c_void)>;

/// Called when a new publisher image becomes available to a subscriber.
pub type aeron_on_available_image_t = ::std::option::Option<
    unsafe extern "C" fn(
        clientd: *mut c_void,
        subscription: *mut aeron_subscription_t,
        image: *mut aeron_image_t,
    ),
>;

/// Called when a publisher image becomes unavailable (publisher disconnected).
pub type aeron_on_unavailable_image_t = ::std::option::Option<
    unsafe extern "C" fn(
        clientd: *mut c_void,
        subscription: *mut aeron_subscription_t,
        image: *mut aeron_image_t,
    ),
>;

/// Optional supplier for the reserved-value field in the frame header.
pub type aeron_reserved_value_supplier_t = ::std::option::Option<
    unsafe extern "C" fn(clientd: *mut c_void, buffer: *mut u8, frame_length: usize) -> i64,
>;

// ── Context (aeron_context_t) ──────────────────────────────────────────────────

extern "C" {
    /// Initialise a client context with default values.
    /// Writes the new pointer into `*context`.  Returns 0 on success, -1 on error.
    pub fn aeron_context_init(context: *mut *mut aeron_context_t) -> c_int;

    /// Close and free a client context previously created by `aeron_context_init`.
    pub fn aeron_context_close(context: *mut aeron_context_t) -> c_int;

    /// Set the Aeron IPC directory that the client uses to find the driver.
    pub fn aeron_context_set_dir(
        context: *mut aeron_context_t,
        value: *const c_char,
    ) -> c_int;

    /// Register an error-handler callback on the client context.
    pub fn aeron_context_set_error_handler(
        context: *mut aeron_context_t,
        handler: aeron_error_handler_t,
        clientd: *mut c_void,
    ) -> c_int;
}

// ── Client (aeron_t) ───────────────────────────────────────────────────────────

extern "C" {
    /// Construct an Aeron client from a configured context.
    /// The client does not start its conductor thread until `aeron_start` is called.
    pub fn aeron_init(client: *mut *mut aeron_t, context: *mut aeron_context_t) -> c_int;

    /// Start the Aeron client's internal conductor thread.
    pub fn aeron_start(client: *mut aeron_t) -> c_int;

    /// Close and free the Aeron client, including all associated resources.
    pub fn aeron_close(client: *mut aeron_t) -> c_int;
}

// ── Driver context (aeron_driver_context_t) ────────────────────────────────────

extern "C" {
    /// Initialise a driver context with default values.
    pub fn aeron_driver_context_init(context: *mut *mut aeron_driver_context_t) -> c_int;

    /// Close and free a driver context.
    pub fn aeron_driver_context_close(context: *mut aeron_driver_context_t) -> c_int;

    /// Set the Aeron IPC directory for the driver.
    pub fn aeron_driver_context_set_dir(
        context: *mut aeron_driver_context_t,
        value: *const c_char,
    ) -> c_int;

    /// When `true`, the driver deletes the IPC directory on start for a clean slate.
    pub fn aeron_driver_context_set_dir_delete_on_start(
        context: *mut aeron_driver_context_t,
        value: bool,
    ) -> c_int;

    /// Set the term buffer length for UDP channels (bytes, must be power of two).
    /// Default is 16 MB; reducing to 1 MB cuts /dev/shm usage 16× per channel.
    pub fn aeron_driver_context_set_term_buffer_length(
        context: *mut aeron_driver_context_t,
        value: usize,
    ) -> c_int;

    /// Set the term buffer length for IPC channels (bytes, must be power of two).
    /// Default is 64 MB; reducing to 1 MB cuts in-process shared-memory usage.
    pub fn aeron_driver_context_set_ipc_term_buffer_length(
        context: *mut aeron_driver_context_t,
        value: usize,
    ) -> c_int;

    /// Set the OS socket receive buffer size (bytes).
    /// Larger values reduce packet loss under burst traffic.
    pub fn aeron_driver_context_set_socket_so_rcvbuf(
        context: *mut aeron_driver_context_t,
        value: usize,
    ) -> c_int;

    /// Set the OS socket send buffer size (bytes).
    /// Larger values allow the kernel to absorb bursts without back-pressure.
    pub fn aeron_driver_context_set_socket_so_sndbuf(
        context: *mut aeron_driver_context_t,
        value: usize,
    ) -> c_int;
}

// ── Driver (aeron_driver_t) ────────────────────────────────────────────────────

extern "C" {
    /// Construct a media driver from a fully configured driver context.
    pub fn aeron_driver_init(
        driver: *mut *mut aeron_driver_t,
        context: *mut aeron_driver_context_t,
    ) -> c_int;

    /// Start the media driver.
    ///
    /// When `manual_main_loop` is `true` the caller must drive the event loop
    /// by calling `aeron_driver_main_do_work` repeatedly.  Blazil always uses
    /// `true` so it can run the loop in its own thread with full control over
    /// the idle strategy.
    pub fn aeron_driver_start(driver: *mut aeron_driver_t, manual_main_loop: bool) -> c_int;

    /// Execute one iteration of the media driver event loop.
    /// Returns the number of work items performed; 0 means the driver is idle.
    pub fn aeron_driver_main_do_work(driver: *mut aeron_driver_t) -> c_int;

    /// Apply the driver's configured idle strategy after a `do_work` pass.
    /// `work_count` should be the value returned by `aeron_driver_main_do_work`.
    pub fn aeron_driver_main_idle_strategy(driver: *mut aeron_driver_t, work_count: c_int);

    /// Shut down and free the media driver.
    pub fn aeron_driver_close(driver: *mut aeron_driver_t) -> c_int;
}

// ── Async publication ──────────────────────────────────────────────────────────

extern "C" {
    /// Begin adding a publication to the client asynchronously.
    /// Returns immediately; poll with `aeron_async_add_publication_poll` until done.
    pub fn aeron_async_add_publication(
        async_: *mut *mut aeron_async_add_publication_t,
        client: *mut aeron_t,
        uri: *const c_char,
        stream_id: i32,
    ) -> c_int;

    /// Poll for completion of an async publication add.
    ///
    /// Returns:  0 while pending  |  1 when `*publication` is ready  |  -1 on error.
    pub fn aeron_async_add_publication_poll(
        publication: *mut *mut aeron_publication_t,
        async_: *mut aeron_async_add_publication_t,
    ) -> c_int;

    /// Offer a message buffer to all connected subscribers.
    ///
    /// Returns the new stream position (≥ 0) on success, or a negative
    /// `AERON_PUBLICATION_*` sentinel on failure.
    pub fn aeron_publication_offer(
        publication: *mut aeron_publication_t,
        buffer: *const u8,
        length: usize,
        reserved_value_supplier: aeron_reserved_value_supplier_t,
        clientd: *mut c_void,
    ) -> i64;

    /// Returns `true` if at least one subscriber is connected to this publication.
    pub fn aeron_publication_is_connected(publication: *mut aeron_publication_t) -> bool;

    /// Initiate an async close of the publication.
    /// `on_close_complete` is called (on the conductor thread) when complete.
    pub fn aeron_publication_close(
        publication: *mut aeron_publication_t,
        on_close_complete: aeron_notification_t,
        on_close_complete_clientd: *mut c_void,
    ) -> c_int;
}

// ── Async subscription ─────────────────────────────────────────────────────────

extern "C" {
    /// Begin adding a subscription asynchronously.
    pub fn aeron_async_add_subscription(
        async_: *mut *mut aeron_async_add_subscription_t,
        client: *mut aeron_t,
        uri: *const c_char,
        stream_id: i32,
        on_available_image_handler: aeron_on_available_image_t,
        on_available_image_clientd: *mut c_void,
        on_unavailable_image_handler: aeron_on_unavailable_image_t,
        on_unavailable_image_clientd: *mut c_void,
    ) -> c_int;

    /// Poll for completion of an async subscription add.
    ///
    /// Returns:  0 while pending  |  1 when `*subscription` is ready  |  -1 on error.
    pub fn aeron_async_add_subscription_poll(
        subscription: *mut *mut aeron_subscription_t,
        async_: *mut aeron_async_add_subscription_t,
    ) -> c_int;

    /// Poll for available fragments on a subscription.
    ///
    /// Invokes `fragment_handler` once per fragment (up to `fragment_limit`).
    /// Returns the total number of fragments dispatched.
    pub fn aeron_subscription_poll(
        subscription: *mut aeron_subscription_t,
        fragment_handler: aeron_fragment_handler_t,
        clientd: *mut c_void,
        fragment_limit: usize,
    ) -> c_int;

    /// Returns `true` if at least one publisher image is connected.
    pub fn aeron_subscription_is_connected(subscription: *mut aeron_subscription_t) -> bool;

    /// Initiate an async close of the subscription.
    pub fn aeron_subscription_close(
        subscription: *mut aeron_subscription_t,
        on_close_complete: aeron_notification_t,
        on_close_complete_clientd: *mut c_void,
    ) -> c_int;
}

// ── Error API ──────────────────────────────────────────────────────────────────

extern "C" {
    /// Returns a pointer to the last Aeron error message (thread-local storage).
    /// Always non-null; the string is owned by Aeron and must not be freed.
    pub fn aeron_errmsg() -> *const c_char;

    /// Returns the last Aeron error code (thread-local storage).
    pub fn aeron_errcode() -> c_int;
}
