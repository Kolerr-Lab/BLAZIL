//! Raw FFI bindings to the Aeron C Media Driver.
//!
//! These bindings are pre-generated from `real-logic/aeron` tag `1.44.1` via
//! bindgen and committed to the repository so that `cargo check` and
//! `cargo clippy` work without a C toolchain requirement.
//!
//! To regenerate (requires aeron submodule + cmake + bindgen):
//! ```bash
//! git submodule update --init --recursive
//! cargo build --features regenerate-bindings -p blazil-aeron-sys
//! ```
//!
//! # Safety
//!
//! All items here are raw C FFI.  Use the safe wrappers in
//! `blazil-transport::aeron` instead of calling these directly.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(clippy::all)]

include!("bindings.rs");
