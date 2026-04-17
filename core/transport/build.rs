// build.rs — blazil-transport
//
// When the `af-xdp` feature is enabled, compile the XDP BPF C program
// (ebpf/blazil_xdp.bpf.c) into a BPF ELF object that is `include_bytes!`'d
// at compile time by the Aya loader in src/ebpf/mod.rs.
//
// Requirements:
//   - clang with BPF target support (`clang -target bpf`)
//   - libbpf headers (usually in /usr/include via `libbpf-dev` or `libbpf-devel`)
//   - Linux kernel headers in the default include path
//
// On Ubuntu/Debian:
//   sudo apt-get install -y clang llvm libbpf-dev linux-headers-$(uname -r)
//
// On Amazon Linux 2023 (i4i.metal):
//   sudo dnf install -y clang llvm libbpf-devel kernel-devel

fn main() {
    // Only compile BPF when the af-xdp feature is active AND we are on Linux.
    // On macOS / CI hosts without kernel headers, skip gracefully — the
    // src/ebpf/mod.rs module is gated with
    //   #[cfg(all(target_os = "linux", feature = "af-xdp"))]
    // so the `include_bytes!` never executes on non-Linux even if --all-features
    // is passed.
    let is_linux = std::env::var("CARGO_CFG_TARGET_OS")
        .map(|os| os == "linux")
        .unwrap_or(false);

    if std::env::var("CARGO_FEATURE_AF_XDP").is_ok() && is_linux {
        compile_xdp_program();
    } else if std::env::var("CARGO_FEATURE_AF_XDP").is_ok() && !is_linux {
        println!(
            "cargo:warning=af-xdp feature is enabled but target OS is not Linux; \
             BPF compilation skipped (safe: ebpf module is cfg-gated to Linux)"
        );
    }
}

fn compile_xdp_program() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set by cargo");
    let src = "ebpf/blazil_xdp.bpf.c";
    let out = format!("{out_dir}/blazil_xdp.bpf.o");

    // Detect target architecture for the include path.
    // On most Linux distros, kernel uapi headers live under:
    //   /usr/include/<arch>-linux-gnu/
    // Fallback to /usr/include if the arch-specific path is absent.
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "x86_64".into());
    let triple = match arch.as_str() {
        "x86_64" => "x86_64-linux-gnu",
        "aarch64" => "aarch64-linux-gnu",
        other => other,
    };
    let inc_arch = format!("/usr/include/{triple}");
    let inc_flag = if std::path::Path::new(&inc_arch).exists() {
        format!("-I{inc_arch}")
    } else {
        "-I/usr/include".into()
    };

    let status = std::process::Command::new("clang")
        .args([
            "-target",
            "bpf",
            "-O2",
            "-g", // Keep BTF debug info for bpftool / libbpf
            "-Wall",
            "-Wno-unused-value",
            "-Wno-pointer-sign",
            "-Wno-compare-distinct-pointer-types",
            &inc_flag,
            "-c",
            src,
            "-o",
            &out,
        ])
        .status()
        .unwrap_or_else(|e| {
            panic!(
                "clang not found or failed to run.\n\
                 Install: apt-get install -y clang libbpf-dev \
                 (Ubuntu) or dnf install -y clang libbpf-devel (AL2023)\n\
                 Error: {e}"
            )
        });

    assert!(
        status.success(),
        "BPF compilation failed — check ebpf/blazil_xdp.bpf.c for errors"
    );

    // Rebuild if the C source changes.
    println!("cargo:rerun-if-changed=ebpf/blazil_xdp.bpf.c");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_AF_XDP");
}
