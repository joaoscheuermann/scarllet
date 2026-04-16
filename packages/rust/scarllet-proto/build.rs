/// Compiles the Scarllet orchestrator `.proto` definitions into Rust types
/// and tonic service stubs, making them available via `tonic::include_proto!`.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fds = protox::compile(["proto/orchestrator.proto"], ["proto/"])?;
    tonic_prost_build::configure().compile_fds(fds)?;
    Ok(())
}
