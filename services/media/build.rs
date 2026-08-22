// Compiles the gRPC contract (client only) into OUT_DIR at build time.
// Uses a vendored `protoc` (protoc-bin-vendored) so no system protobuf-compiler is required —
// keeps CI, Docker, and local builds hermetic.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", protoc);
    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(&["proto/voice.proto"], &["proto"])?;
    Ok(())
}
