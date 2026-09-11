use std::{net::IpAddr, time::Duration};
use tonic::{
    metadata::{Ascii, MetadataValue},
    service::Interceptor,
    transport::{Channel, Endpoint},
    Request, Status,
};
use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("Endpoint must be an HTTP loopback address without credentials or a path")]
    InvalidEndpoint,
    #[error("Daemon is unavailable")]
    Unavailable,
    #[error("Profile authorization is unavailable")]
    Authorization,
}

pub fn loopback_endpoint(value: &str) -> Result<Endpoint, ClientError> {
    let uri: http::Uri = value.parse().map_err(|_| ClientError::InvalidEndpoint)?;
    let host = uri
        .host()
        .ok_or(ClientError::InvalidEndpoint)?
        .trim_matches(['[', ']']);
    let ip: IpAddr = host.parse().map_err(|_| ClientError::InvalidEndpoint)?;
    if !ip.is_loopback()
        || uri.scheme_str() != Some("http")
        || uri.query().is_some()
        || !matches!(uri.path(), "" | "/")
        || uri
            .authority()
            .is_some_and(|authority| authority.as_str().contains('@'))
    {
        return Err(ClientError::InvalidEndpoint);
    }
    Endpoint::from_shared(value.to_owned())
        .map(|endpoint| {
            endpoint
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(30))
        })
        .map_err(|_| ClientError::InvalidEndpoint)
}

pub async fn connect(value: &str) -> Result<Channel, ClientError> {
    connect_with_timeout(value, Duration::from_secs(30)).await
}

pub async fn connect_with_timeout(value: &str, timeout: Duration) -> Result<Channel, ClientError> {
    loopback_endpoint(value)?
        .timeout(timeout)
        .connect()
        .await
        .map_err(|_| ClientError::Unavailable)
}

#[derive(Clone)]
pub struct TokenInjector {
    value: MetadataValue<Ascii>,
}

impl TokenInjector {
    pub fn new(token: Zeroizing<String>) -> Result<Self, ClientError> {
        let mut value: MetadataValue<Ascii> =
            token.parse().map_err(|_| ClientError::Authorization)?;
        value.set_sensitive(true);
        Ok(Self { value })
    }
}

impl Interceptor for TokenInjector {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        request
            .metadata_mut()
            .insert("authorization", self.value.clone());
        Ok(request)
    }
}
