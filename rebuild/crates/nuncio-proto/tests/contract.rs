#![allow(clippy::unwrap_used)]
use prost::Message;
use prost_types::FileDescriptorSet;

#[test]
fn account_extension_preserves_every_previous_field_and_rpc() {
    fn message(old: &prost_types::DescriptorProto, new: &prost_types::DescriptorProto) {
        for field in &old.field {
            assert_eq!(
                new.field.iter().find(|f| f.number == field.number),
                Some(field)
            );
        }
        for nested in &old.nested_type {
            message(
                nested,
                new.nested_type
                    .iter()
                    .find(|m| m.name == nested.name)
                    .unwrap(),
            );
        }
        for enumeration in &old.enum_type {
            let current = new
                .enum_type
                .iter()
                .find(|e| e.name == enumeration.name)
                .unwrap();
            for value in &enumeration.value {
                assert!(current.value.contains(value));
            }
        }
        assert!(old
            .oneof_decl
            .iter()
            .zip(&new.oneof_decl)
            .all(|(a, b)| a == b));
        assert!(new.oneof_decl.len() >= old.oneof_decl.len());
    }
    let prior = FileDescriptorSet::decode(
        include_bytes!("../proto/nuncio.v2.pre-account-management.bin").as_slice(),
    )
    .unwrap();
    let current = FileDescriptorSet::decode(nuncio_proto::DESCRIPTOR).unwrap();
    for old in prior.file {
        let new = current.file.iter().find(|f| f.name == old.name).unwrap();
        assert_eq!(old.package, new.package);
        for previous in &old.message_type {
            message(
                previous,
                new.message_type
                    .iter()
                    .find(|m| m.name == previous.name)
                    .unwrap(),
            );
        }
        for previous in &old.enum_type {
            let enumeration = new
                .enum_type
                .iter()
                .find(|e| e.name == previous.name)
                .unwrap();
            for value in &previous.value {
                assert!(enumeration.value.contains(value));
            }
        }
        for service in &old.service {
            let updated = new.service.iter().find(|s| s.name == service.name).unwrap();
            for method in &service.method {
                assert!(updated.method.contains(method));
            }
        }
    }
}

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
