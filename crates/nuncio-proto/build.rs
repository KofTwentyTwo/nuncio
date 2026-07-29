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
//! across all three platforms.
//!
//! Alongside code generation, this also emits a compiled `FileDescriptorSet`
//! for the `nuncio.v1` package to `$OUT_DIR`. The crate's own tests compare
//! that freshly generated descriptor against the one committed at
//! `proto/nuncio/v1/descriptor.bin` -- the published wire contract -- so any
//! accidental breaking change to the `.proto` sources fails the test suite
//! instead of silently shipping.

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

    let out_dir = match std::env::var("OUT_DIR") {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("OUT_DIR was not set by cargo: {err}");
            std::process::exit(1);
        }
    };
    let descriptor_path = std::path::Path::new(&out_dir).join("nuncio_v1_descriptor.bin");

    let mut config = tonic_prost_build::Config::new();
    // `SourceCodeInfo` records comment text and source byte offsets, which
    // are irrelevant to the wire contract. Excluding it keeps the emitted
    // descriptor -- and the golden file it is checked against -- focused
    // purely on the structural contract (fields, numbers, types, RPCs), so
    // editing a doc comment alone never perturbs the committed golden.
    config.skip_source_info();
    config.file_descriptor_set_path(&descriptor_path);

    if let Err(err) = tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_with_config(config, &[proto_file], &["proto"])
    {
        eprintln!("failed to compile nuncio.proto: {err}");
        std::process::exit(1);
    }
}
