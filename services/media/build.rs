// Compiles the gRPC contract (client only) into OUT_DIR at build time.
// Requires `protoc` on the build image (see services/media/Dockerfile).
fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(&["proto/voice.proto"], &["proto"])?;
    Ok(())
}
