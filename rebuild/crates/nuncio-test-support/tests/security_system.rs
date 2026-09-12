#![allow(clippy::unwrap_used)]
#[path = "support/security_inputs.rs"]
mod security_inputs;
pub mod support;

use nuncio_engine::secrets::SecretStore;
use nuncio_proto::v2::GetStatusRequest;
use nuncio_test_support::{google::Seed, TestError};
use prost::Message;
use serde_json::json;
use std::collections::BTreeSet;
use support::system::SystemHarness;
use tonic::{Code, Request};
use zeroize::Zeroizing;

#[derive(Clone, PartialEq, Message)]
struct Empty {}

#[tokio::test]
async fn every_descriptor_rpc_rejects_missing_wrong_and_retired_profile_authorization(
) -> Result<(), TestError> {
    let mut h = SystemHarness::start(Seed::TwoAccounts).await?;
    let status = h
        .authenticated()
        .get_status(GetStatusRequest {})
        .await?
        .into_inner();
    let name = format!("{}/profile/api", status.profile_id);
    let old = h.secrets.get(&name)?.unwrap();
    let retired = Zeroizing::new(format!("Bearer {}", hex::encode(old.as_slice())));
    h.shutdown().await?;
    h.secrets.put(&name, &[0x71; 32])?;
    h.restart().await?;
    assert_eq!(
        h.authenticated()
            .get_status(GetStatusRequest {})
            .await?
            .into_inner()
            .profile_id,
        status.profile_id
    );
    let before = serde_json::to_value(h.google.control().snapshot().await)?;
    let descriptor = prost_types::FileDescriptorSet::decode(nuncio_proto::DESCRIPTOR)?;
    let mut methods = Vec::new();
    let mut services = BTreeSet::new();
    for file in descriptor.file {
        let package = file.package.unwrap();
        for service in file.service {
            let service_name = format!("{}.{}", package, service.name.unwrap());
            services.insert(service_name.clone());
            for method in service.method {
                methods.push(format!("/{}/{}", service_name, method.name.unwrap()));
            }
        }
    }
    assert_eq!(services.len(), 6);
    assert!(
        methods.len() >= 49,
        "descriptor must cover the complete public API"
    );
    let mut evidence = Vec::new();
    for (case, token) in [
        ("missing", None),
        ("wrong", Some("Bearer synthetic-wrong-profile-token")),
        ("retired", Some(retired.as_str())),
        ("wrong-scheme", Some("Basic synthetic-wrong-scheme")),
    ] {
        for path in &methods {
            let mut client = tonic::client::Grpc::new(h.channel());
            client.ready().await?;
            let mut request = Request::new(Empty {});
            if let Some(token) = token {
                request
                    .metadata_mut()
                    .insert("authorization", token.parse()?);
            }
            // Auth must reject before any payload or stream-shape validation.
            let result: Result<tonic::Response<Empty>, _> = client
                .unary(
                    request,
                    http::uri::PathAndQuery::try_from(path.as_str())?,
                    tonic_prost::ProstCodec::default(),
                )
                .await;
            let error = result.unwrap_err();
            assert_eq!(error.code(), Code::Unauthenticated, "{case} {path}");
            assert_eq!(error.message(), "Profile authorization required");
            evidence.push(json!({"case":case,"rpc":path,"status":"unauthenticated"}));
        }
    }
    assert_eq!(
        serde_json::to_value(h.google.control().snapshot().await)?,
        before
    );
    assert_eq!(
        h.authenticated()
            .get_status(GetStatusRequest {})
            .await?
            .into_inner()
            .profile_id,
        status.profile_id
    );
    if let Some(directory) = std::env::var_os("NUNCIO_TEST_ARTIFACTS") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join("all-rpc-auth.json"),
            serde_json::to_vec_pretty(&evidence)?,
        )?;
    }
    h.shutdown().await?;
    Ok(())
}
