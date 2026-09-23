use prost::Message;

fn main() {
    println!("cargo:rerun-if-changed=descriptors.pb");
    let bytes = std::fs::read("descriptors.pb").expect("read checked-in protocol snapshot");
    let set =
        prost_types::FileDescriptorSet::decode(bytes.as_slice()).expect("valid descriptor set");
    prost_build::Config::new()
        .include_file("generated.rs")
        .compile_fds(set)
        .expect("generate Rust types without protoc or network access");
}
