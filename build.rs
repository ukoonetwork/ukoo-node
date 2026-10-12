fn main() {
    // protox compiles the proto in pure Rust, so building needs no system `protoc`.
    let fds =
        protox::compile(["proto/node.proto"], ["proto"]).expect("failed to compile node.proto");
    tonic_prost_build::configure()
        .build_server(false)
        // The generated `connect` constructor would clash with the `Connect` RPC; we build the channel ourselves.
        .build_transport(false)
        .compile_fds(fds)
        .expect("failed to generate gRPC code");
    println!("cargo:rerun-if-changed=proto/node.proto");
}
