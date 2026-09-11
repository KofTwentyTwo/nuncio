use nuncio_engine::engine::{Engine, EngineConfig, EngineError};
use nuncio_proto::{
    client::{connect, TokenInjector},
    v2::{system_client::SystemClient, ShutdownRequest},
};
use nuncio_test_support::{
    google::{MockGoogle, Seed},
    TestError,
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tonic::{service::interceptor::InterceptedService, transport::Channel};

pub struct SystemHarness {
    clock_origin: std::time::Instant,
    pub google: MockGoogle,
    pub secrets: Arc<super::secrets::TestSecrets>,
    pub secrets_file: PathBuf,
    pub directory: PathBuf,
    temporary: Option<tempfile::TempDir>,
    channel: Channel,
    injector: TokenInjector,
    server: Option<tokio::task::JoinHandle<Result<(), EngineError>>>,
    finished: bool,
    payload_limit: u64,
    polling: Option<u64>,
}
impl SystemHarness {
    pub async fn start(seed: Seed) -> Result<Self, TestError> {
        Self::with_payload_limit(seed, 64 * 1024 * 1024).await
    }
    pub async fn with_payload_limit(seed: Seed, payload_limit: u64) -> Result<Self, TestError> {
        Self::configured(seed, payload_limit, None).await
    }
    pub async fn with_polling(seed: Seed, interval_ms: u64) -> Result<Self, TestError> {
        Self::configured(seed, 64 * 1024 * 1024, Some(interval_ms)).await
    }
    async fn configured(
        seed: Seed,
        payload_limit: u64,
        polling: Option<u64>,
    ) -> Result<Self, TestError> {
        let temporary = tempfile::Builder::new()
            .prefix("nuncio-system-")
            .tempdir()?;
        let directory = temporary.path().join("profile");
        let secrets_file = temporary.path().join("synthetic-secrets.json");
        let secrets = Arc::new(super::secrets::TestSecrets::new(secrets_file.clone()));
        let google = MockGoogle::start(seed).await?;
        let clock_origin = std::time::Instant::now();
        let (channel, injector, server) = start_engine(
            &google,
            &directory,
            secrets.clone(),
            payload_limit,
            polling,
            clock_origin,
        )
        .await?;
        Ok(Self {
            google,
            secrets,
            secrets_file,
            directory,
            temporary: Some(temporary),
            channel,
            injector,
            server: Some(server),
            finished: false,
            payload_limit,
            polling,
            clock_origin,
        })
    }
    pub async fn restart(&mut self) -> Result<(), TestError> {
        if self.server.is_some() {
            return Err(std::io::Error::other("shutdown the system harness before restart").into());
        }
        let (channel, injector, server) = start_engine(
            &self.google,
            &self.directory,
            self.secrets.clone(),
            self.payload_limit,
            self.polling,
            self.clock_origin,
        )
        .await?;
        self.channel = channel;
        self.injector = injector;
        self.server = Some(server);
        self.finished = false;
        Ok(())
    }
    pub fn arm(&self, name: &str) -> Result<(), TestError> {
        let directory = self.directory.with_extension("test-barriers");
        for extension in ["entered", "release"] {
            match std::fs::remove_file(directory.join(format!("{name}.{extension}"))) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        std::fs::write(directory.join(format!("{name}.arm")), [])?;
        Ok(())
    }
    pub async fn wait(&self, name: &str) -> Result<(), TestError> {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !self
                .directory
                .with_extension("test-barriers")
                .join(format!("{name}.entered"))
                .exists()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        Ok(())
    }
    pub fn release(&self, name: &str) -> Result<(), TestError> {
        std::fs::write(
            self.directory
                .with_extension("test-barriers")
                .join(format!("{name}.release")),
            [],
        )?;
        Ok(())
    }
    pub fn maintenance(
        &self,
    ) -> nuncio_proto::v2::maintenance_client::MaintenanceClient<
        InterceptedService<Channel, TokenInjector>,
    > {
        nuncio_proto::v2::maintenance_client::MaintenanceClient::with_interceptor(
            self.channel.clone(),
            self.injector.clone(),
        )
    }
    pub fn anonymous_maintenance(
        &self,
    ) -> nuncio_proto::v2::maintenance_client::MaintenanceClient<Channel> {
        nuncio_proto::v2::maintenance_client::MaintenanceClient::new(self.channel.clone())
    }
    pub fn authenticated(&self) -> SystemClient<InterceptedService<Channel, TokenInjector>> {
        SystemClient::with_interceptor(self.channel.clone(), self.injector.clone())
    }
    pub fn unauthenticated(&self) -> SystemClient<Channel> {
        SystemClient::new(self.channel.clone())
    }
    pub fn accounts(
        &self,
    ) -> nuncio_proto::v2::accounts_client::AccountsClient<InterceptedService<Channel, TokenInjector>>
    {
        nuncio_proto::v2::accounts_client::AccountsClient::with_interceptor(
            self.channel.clone(),
            self.injector.clone(),
        )
    }
    pub fn anonymous_accounts(&self) -> nuncio_proto::v2::accounts_client::AccountsClient<Channel> {
        nuncio_proto::v2::accounts_client::AccountsClient::new(self.channel.clone())
    }
    pub fn mail(
        &self,
    ) -> nuncio_proto::v2::mail_client::MailClient<InterceptedService<Channel, TokenInjector>> {
        nuncio_proto::v2::mail_client::MailClient::with_interceptor(
            self.channel.clone(),
            self.injector.clone(),
        )
    }
    pub fn anonymous_mail(&self) -> nuncio_proto::v2::mail_client::MailClient<Channel> {
        nuncio_proto::v2::mail_client::MailClient::new(self.channel.clone())
    }
    pub fn operations(
        &self,
    ) -> nuncio_proto::v2::operations_client::OperationsClient<
        InterceptedService<Channel, TokenInjector>,
    > {
        nuncio_proto::v2::operations_client::OperationsClient::with_interceptor(
            self.channel.clone(),
            self.injector.clone(),
        )
        .max_decoding_message_size(8 * 1024 * 1024)
    }
    pub fn anonymous_operations(
        &self,
    ) -> nuncio_proto::v2::operations_client::OperationsClient<Channel> {
        nuncio_proto::v2::operations_client::OperationsClient::new(self.channel.clone())
    }
    pub fn calendar(
        &self,
    ) -> nuncio_proto::v2::calendar_client::CalendarClient<InterceptedService<Channel, TokenInjector>>
    {
        nuncio_proto::v2::calendar_client::CalendarClient::with_interceptor(
            self.channel.clone(),
            self.injector.clone(),
        )
        .max_decoding_message_size(16 * 1024 * 1024)
    }
    pub fn anonymous_calendar(&self) -> nuncio_proto::v2::calendar_client::CalendarClient<Channel> {
        nuncio_proto::v2::calendar_client::CalendarClient::new(self.channel.clone())
    }
    pub async fn shutdown(&mut self) -> Result<(), TestError> {
        self.authenticated().shutdown(ShutdownRequest {}).await?;
        if let Some(mut server) = self.server.take() {
            match tokio::time::timeout(Duration::from_secs(5), &mut server).await {
                Ok(result) => {
                    result??;
                }
                Err(error) => {
                    server.abort();
                    let _ = server.await;
                    return Err(error.into());
                }
            }
        }
        self.finished = true;
        Ok(())
    }
}

async fn start_engine(
    google: &MockGoogle,
    directory: &std::path::Path,
    secrets: Arc<super::secrets::TestSecrets>,
    payload_limit: u64,
    polling: Option<u64>,
    clock_origin: std::time::Instant,
) -> Result<
    (
        Channel,
        TokenInjector,
        tokio::task::JoinHandle<Result<(), EngineError>>,
    ),
    TestError,
> {
    let mut settings = nuncio_engine::test_controls::TestConfig::google(google.base_url());
    settings.background_sync = polling.is_some();
    settings.poll_interval_ms = polling.unwrap_or(60000);
    settings.request_timeout_ms = 1000;
    let barriers = directory.with_extension("test-barriers");
    std::fs::create_dir_all(&barriers)?;
    settings.barriers_directory = Some(barriers);
    settings.max_payload_bytes = payload_limit;
    settings.now_unix_ms = Some(1772895600000 + i64::try_from(clock_origin.elapsed().as_millis())?);
    let engine = Engine::open_for_test(
        EngineConfig {
            directory: directory.into(),
            secrets,
        },
        settings,
    )
    .await?;
    let injector = TokenInjector::new(engine.authorization())?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let mut server = tokio::spawn(nunciod::serve(engine, listener));
    let channel = match tokio::time::timeout(Duration::from_secs(5), connect(&endpoint)).await {
        Ok(Ok(channel)) => channel,
        _ => {
            server.abort();
            let _ = (&mut server).await;
            return Err(std::io::Error::other("system harness RPC readiness failed").into());
        }
    };
    Ok((channel, injector, server))
}
impl Drop for SystemHarness {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
        if !self.finished || std::thread::panicking() {
            if let Some(temporary) = self.temporary.take() {
                let path: PathBuf = temporary.keep();
                eprintln!("System test profile preserved at {}", path.display());
            }
        }
    }
}
