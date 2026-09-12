use std::{io, net::IpAddr, time::Duration};
use tokio::io::AsyncReadExt;
use tonic::{
    metadata::{Ascii, MetadataValue},
    Request,
};
use zeroize::Zeroizing;

mod v2 {
    tonic::include_proto!("nuncio.v2");
}

fn authorized<T>(body: T, token: &MetadataValue<Ascii>) -> Request<T> {
    let mut request = Request::new(body);
    request
        .metadata_mut()
        .insert("authorization", token.clone());
    request
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.len() != 1 {
        return Err(io::Error::other("Expected one loopback endpoint argument").into());
    }
    let uri: http::Uri = arguments[0].parse()?;
    let host: IpAddr = uri
        .host()
        .ok_or_else(|| io::Error::other("Missing host"))?
        .trim_matches(['[', ']'])
        .parse()?;
    if !host.is_loopback()
        || uri.scheme_str() != Some("http")
        || uri.query().is_some()
        || !matches!(uri.path(), "" | "/")
        || uri.authority().is_some_and(|a| a.as_str().contains('@'))
    {
        return Err(io::Error::other("Expected loopback HTTP endpoint").into());
    }
    let mut credential = Zeroizing::new(Vec::new());
    tokio::io::stdin()
        .take(1025)
        .read_to_end(&mut credential)
        .await?;
    if credential.is_empty() || credential.len() > 1024 {
        return Err(io::Error::other("Invalid authorization input").into());
    }
    let mut token: MetadataValue<Ascii> = std::str::from_utf8(&credential)?.trim().parse()?;
    token.set_sensitive(true);
    let channel = tonic::transport::Endpoint::from_shared(arguments[0].clone())?
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(10))
        .connect()
        .await?;
    let mut client = v2::system_client::SystemClient::new(channel);
    let status = client
        .get_status(authorized(v2::GetStatusRequest {}, &token))
        .await?
        .into_inner();
    let storage = status
        .storage
        .ok_or_else(|| io::Error::other("Missing storage status"))?;
    if status.api_version != "nuncio.v2" || storage.revision == 0 {
        return Err(io::Error::other("Expected v2 profile with a committed change").into());
    }
    let mut stream = client
        .watch_changes(authorized(
            v2::WatchChangesRequest { after_revision: 0 },
            &token,
        ))
        .await?
        .into_inner();
    let change = stream
        .message()
        .await?
        .ok_or_else(|| io::Error::other("Missing change"))?;
    println!(
        "{}",
        serde_json::json!({"api_version":status.api_version,"profile_id":status.profile_id,"revision":storage.revision,"change":{"revision":change.revision,"kind":change.kind,"account_id":change.account_id}})
    );
    Ok(())
}

#[tokio::main]
async fn main() {
    if !matches!(
        tokio::time::timeout(Duration::from_secs(15), run()).await,
        Ok(Ok(()))
    ) {
        eprintln!("Contract smoke failed");
        std::process::exit(1);
    }
}
