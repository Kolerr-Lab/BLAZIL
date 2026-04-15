//! Build script for `blazil-aeron-sys`.
//!
//! # What this does
//!
//! 1. If the `aeron/` git submodule is initialised on disk, it compiles
//!    `libaeron_static.a` from source via CMake and emits the necessary
//!    `cargo:rustc-link-*` directives.
//!
//! 2. If the submodule is **not** initialised (developer workstation without
//!    a full checkout, or the primary `rust-ci` job which does not need the C
//!    library), the build script emits a `cargo:warning` and **returns without
//!    panicking**.  This means `cargo check` and `cargo clippy` succeed against
//!    the pre-generated `src/bindings.rs`, while `cargo build` / `cargo test`
//!    will fail at the **linker** stage with a clear "library not found" error.
//!
//! 3. If the `regenerate-bindings` feature is enabled, bindgen re-generates
//!    `src/bindings.rs` from the C headers in the submodule.

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=aeron/CMakeLists.txt");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let aeron_src = manifest_dir.join("aeron");

    if !aeron_src.join("CMakeLists.txt").exists() {
        // ── Submodule not initialised ────────────────────────────────────────
        // Allow `cargo check` / `cargo clippy` to type-check the Rust wrapper
        // code against the pre-generated bindings.  Actual linking will fail
        // cleanly with a "library not found" error, prompting the developer.
        println!(
            "cargo:warning=blazil-aeron-sys: Aeron C submodule absent at `{}`.",
            aeron_src.display()
        );
        println!("cargo:warning=Run `git submodule update --init --recursive` to enable linking.");
        return;
    }

    // ── Regenerate bindings if requested ─────────────────────────────────────
    #[cfg(feature = "regenerate-bindings")]
    regenerate_bindings(&aeron_src, &manifest_dir);

    // ── Build libaeron_static.a via CMake ────────────────────────────────────
    build_aeron_static(&aeron_src);
}

fn build_aeron_static(aeron_src: &std::path::Path) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let build_dir = out_dir.join("aeron-build");
    std::fs::create_dir_all(&build_dir).expect("create build dir");

    let nproc = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    // CMake configure — Release build, driver + C client only.
    // Archive API is disabled because it requires a JDK on the build machine.
    let status = std::process::Command::new("cmake")
        .current_dir(&build_dir)
        .arg(aeron_src)
        .args([
            "-DCMAKE_BUILD_TYPE=Release",
            "-DAERON_BUILD_SAMPLES=OFF",
            "-DAERON_BUILD_TESTS=OFF",
            "-DAERON_BUILD_BENCHMARKS=OFF",
            "-DAERON_ENABLE_DRIVER_EXT=OFF",
            "-DAERON_BUILD_TOOLS=OFF",
            "-DBUILD_AERON_DRIVER=ON",
            "-DAERON_DISABLE_BOUNDS_CHECKS=ON",
            // Disable the archive API — avoids requiring a JDK on every build
            // machine.  Blazil only uses the C client and media driver.
            "-DBUILD_AERON_ARCHIVE_API=OFF",
        ])
        .status()
        .expect("cmake not found — install: apt-get install -y cmake g++");
    assert!(status.success(), "cmake configuration failed");

    // CMake build — C client + embedded media driver.
    // aeron_static        = C client library (aeron_context, publication, subscription …)
    // aeron_driver_static = embedded media driver (aeron_driver_start/close …)
    for target in &["aeron_static", "aeron_driver_static"] {
        let status = std::process::Command::new("cmake")
            .current_dir(&build_dir)
            .args([
                "--build",
                ".",
                "--parallel",
                &nproc.to_string(),
                "--target",
                target,
            ])
            .status()
            .expect("cmake --build failed");
        assert!(
            status.success(),
            "Aeron cmake target `{}` build failed",
            target
        );
    }

    // Emit linker search paths and library names.
    // Order matters: driver_static depends on aeron_static, so list driver first.
    println!("cargo:rustc-link-search=native={}", build_dir.display());
    println!(
        "cargo:rustc-link-search=native={}",
        build_dir.join("lib").display()
    );
    // Link order: driver_static depends on aeron_static, so link driver first.
    println!("cargo:rustc-link-lib=static=aeron_driver_static");
    println!("cargo:rustc-link-lib=static=aeron_static");

    // Platform deps for libaeron_static.a.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "linux" => {
            println!("cargo:rustc-link-lib=dylib=dl");
            println!("cargo:rustc-link-lib=dylib=pthread");
            println!("cargo:rustc-link-lib=dylib=rt");
            println!("cargo:rustc-link-lib=dylib=stdc++");
        }
        "macos" => {
            println!("cargo:rustc-link-lib=dylib=c++");
        }
        _ => {}
    }
}

#[cfg(feature = "regenerate-bindings")]
fn regenerate_bindings(aeron_src: &std::path::Path, manifest_dir: &std::path::Path) {
    let headers = aeron_src.join("aeron-client/src/main/c");
    let out = manifest_dir.join("src/bindings.rs");

    let bindings = bindgen::Builder::default()
        .header(headers.join("aeronc.h").to_str().unwrap())
        // Allowlist: only what Blazil uses.
        .allowlist_type("aeron_context_t")
        .allowlist_type("aeron_t")
        .allowlist_type("aeron_publication_t")
        .allowlist_type("aeron_subscription_t")
        .allowlist_type("aeron_driver_context_t")
        .allowlist_type("aeron_driver_t")
        .allowlist_type("aeron_header_t")
        .allowlist_type("aeron_image_t")
        .allowlist_type("aeron_async_add_publication_t")
        .allowlist_type("aeron_async_add_subscription_t")
        .allowlist_type("aeron_fragment_handler_t")
        .allowlist_type("aeron_error_handler_t")
        .allowlist_type("aeron_on_available_image_t")
        .allowlist_type("aeron_on_unavailable_image_t")
        .allowlist_type("aeron_notification_t")
        .allowlist_type("aeron_reserved_value_supplier_t")
        .allowlist_function("aeron_context_init")
        .allowlist_function("aeron_context_close")
        .allowlist_function("aeron_context_set_dir")
        .allowlist_function("aeron_context_set_error_handler")
        .allowlist_function("aeron_init")
        .allowlist_function("aeron_start")
        .allowlist_function("aeron_close")
        .allowlist_function("aeron_driver_context_init")
        .allowlist_function("aeron_driver_context_close")
        .allowlist_function("aeron_driver_context_set_dir")
        .allowlist_function("aeron_driver_context_set_dir_delete_on_start")
        .allowlist_function("aeron_driver_init")
        .allowlist_function("aeron_driver_start")
        .allowlist_function("aeron_driver_main_do_work")
        .allowlist_function("aeron_driver_main_idle_strategy")
        .allowlist_function("aeron_driver_close")
        .allowlist_function("aeron_async_add_publication")
        .allowlist_function("aeron_async_poll_publication")
        .allowlist_function("aeron_publication_offer")
        .allowlist_function("aeron_publication_is_connected")
        .allowlist_function("aeron_publication_close")
        .allowlist_function("aeron_async_add_subscription")
        .allowlist_function("aeron_async_poll_subscription")
        .allowlist_function("aeron_subscription_poll")
        .allowlist_function("aeron_subscription_is_connected")
        .allowlist_function("aeron_subscription_close")
        .allowlist_function("aeron_errmsg")
        .allowlist_function("aeron_errcode")
        .allowlist_var("AERON_PUBLICATION_.*")
        .clang_arg(format!("-I{}", headers.display()))
        .generate()
        .expect("bindgen failed to generate bindings");

    bindings.write_to_file(&out).expect("write bindings.rs");
    println!("cargo:warning=bindings.rs regenerated at {}", out.display());
}
