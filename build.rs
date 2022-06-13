fn main() {
    let proto_files = ["proto/multiplexer.proto3", "proto/socks.proto3"];

    // Compile & gen protobuf code
    prost_build::compile_protos(&proto_files, &["proto"]).expect("Couldn't build protobuf files");

    proto_files
        .iter()
        .for_each(|x| println!("cargo:rerun-if-changed={}", x));
}
