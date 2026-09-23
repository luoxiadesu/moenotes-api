# Protocol Provenance

`descriptors.pb` is a recovered FileDescriptorSet from the Android
`com.bilibili.sirius` 1.0.1 sample (Unity 6000.3.12f1). It contains 123 files,
56 services and 261 unary methods, including Google protobuf descriptors.

SHA-256: `d07bd23de3553f71caec0830668bf4f5196fe33575a265ed0489f73ffb60cf20`

The snapshot is not an official published API specification. Its third-party
contents and generated protocol definitions are NOT claimed as original MIT
licensed work. Original build and reflection helpers are covered by the root
license. No credentials, account captures, master tables, executable samples,
or private analysis artifacts are included.

Cargo generates Rust message types directly from this snapshot with prost-build;
no proprietary tools, game installation, protoc or live server is required.
Future snapshot changes require a hash/provenance update and compatibility review.
