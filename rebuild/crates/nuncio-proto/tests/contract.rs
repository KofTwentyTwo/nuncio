#![allow(clippy::unwrap_used)]
use prost::Message;
use prost_types::FileDescriptorSet;

#[test]
fn v2_contract_matches_reviewed_release_descriptor() {
    let released =
        FileDescriptorSet::decode(include_bytes!("../proto/nuncio.v2.bin").as_slice()).unwrap();
    let current = FileDescriptorSet::decode(nuncio_proto::DESCRIPTOR).unwrap();
    assert_eq!(released.file.len(), 6);
    assert!(released
        .file
        .iter()
        .all(|file| file.package.as_deref() == Some("nuncio.v2")));
    assert_eq!(current, released,
        "The released v2 descriptor changed. Review field numbers, types, reserved fields, RPC names and streaming semantics before deliberately updating the freeze.");
}
