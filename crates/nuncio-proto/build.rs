//! Build script for `nuncio-proto`.
//!
//! Compiles the versioned protobuf definitions under `proto/nuncio/v1` into
//! Rust client + server stubs via `tonic-prost-build`.
//!
//! `protoc` is vendored via the `protoc-bin-vendored` crate, which bundles
//! prebuilt `protoc` binaries for Linux, macOS, and Windows directly inside
//! the crate. This means the build never depends on a system-installed
//! protobuf compiler being present on PATH, nor on a C/C++ toolchain or
//! `cmake` to build one from source (unlike source-vendoring alternatives
//! such as `protobuf-src`). This is required for reproducible builds in CI
//! across all three platforms (see backlog story 1.A.1 / GH-148).

fn main() {
    let protoc_path = match protoc_bin_vendored::protoc_bin_path() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("failed to locate vendored protoc binary: {err}");
            std::process::exit(1);
        }
    };

    // Point prost-build at the vendored protoc binary instead of requiring
    // one on the host's PATH.
    std::env::set_var("PROTOC", protoc_path);

    let proto_file = "proto/nuncio/v1/nuncio.proto";

    println!("cargo:rerun-if-changed={proto_file}");

    if let Err(err) = tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&[proto_file], &["proto"])
    {
        eprintln!("failed to compile nuncio.proto: {err}");
        std::process::exit(1);
    }
}
