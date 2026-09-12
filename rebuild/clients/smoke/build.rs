fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = "../../crates/nuncio-proto/proto";
    let path = format!("{directory}/nuncio/v2/system.proto");
    let mut config = tonic_prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);
    tonic_prost_build::configure()
        .build_server(false)
        .compile_with_config(config, &[path.as_str()], &[directory])?;
    println!("cargo:rerun-if-changed={path}");
    Ok(())
}
