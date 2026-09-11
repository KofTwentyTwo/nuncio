use super::system::SystemHarness;
use nuncio_proto::v2::{BeginGoogleAuthRequest, GetAuthStatusRequest};

pub fn browser() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap()
}

pub async fn begin(
    h: &SystemHarness,
    address: &str,
    account: Option<String>,
) -> nuncio_proto::v2::AuthSession {
    h.accounts()
        .begin_google_auth(BeginGoogleAuthRequest {
            client_id: "nuncio-test-client".into(),
            client_secret: None,
            login_hint: Some(address.into()),
            account_id: account,
        })
        .await
        .unwrap()
        .into_inner()
}
pub async fn consent(url: &str) -> url::Url {
    let response = browser().get(url).send().await.unwrap();
    assert_eq!(response.status(), 302);
    url::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap()
}
pub async fn finish(
    h: &SystemHarness,
    session: &nuncio_proto::v2::AuthSession,
    expected: u16,
) -> nuncio_proto::v2::AuthSession {
    let callback = consent(&session.browser_url).await;
    assert_eq!(
        browser().get(callback).send().await.unwrap().status(),
        expected
    );
    h.accounts()
        .get_auth_status(GetAuthStatusRequest {
            session_id: session.session_id.clone(),
        })
        .await
        .unwrap()
        .into_inner()
}
