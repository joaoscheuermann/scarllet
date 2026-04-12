fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fds = protox::compile(["proto/orchestrator.proto"], ["proto/"])?;
    tonic_prost_build::configure().compile_fds(fds)?;
    Ok(())
}
