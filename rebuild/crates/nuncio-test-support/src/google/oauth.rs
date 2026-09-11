use super::{
    state::Model,
    wire::{Input, Query, Reply, Result},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::json;
use sha2::{Digest, Sha256};
use url::Url;

pub(super) struct Code {
    account: String,
    redirect: String,
    challenge: String,
    scopes: String,
    expires: u64,
    offline: bool,
}
#[derive(Clone)]
pub(super) struct Grant {
    pub account: String,
    pub scopes: String,
    pub expires: u64,
}

fn error(name: &str) -> Reply {
    Reply {
        status: 400,
        body: json!({"error":name}),
        location: None,
    }
}

pub(super) fn route(model: &mut Model, input: &Input) -> Result<Reply> {
    if input.path == "/o/oauth2/v2/auth" && input.method == "GET" {
        return authorize(model, &input.query);
    }
    if input.path != "/token" || input.method != "POST" {
        return Err(Reply::error(405, "methodNotAllowed"));
    }
    if !input
        .headers
        .get("content-type")
        .and_then(|s| s.to_str().ok())
        .is_some_and(|s| s.starts_with("application/x-www-form-urlencoded"))
    {
        return Err(error("invalid_request"));
    }
    let form = Query::parse(&input.body);
    form.validate(
        &[
            "client_id",
            "client_secret",
            "grant_type",
            "redirect_uri",
            "code",
            "code_verifier",
            "refresh_token",
        ],
        &[],
    )
    .map_err(|_| error("invalid_request"))?;
    if form.get("client_id") != Some("nuncio-test-client")
        || form
            .get("client_secret")
            .is_some_and(|secret| secret != "synthetic-client-secret")
    {
        return Err(error("invalid_client"));
    }
    let (grant, offline) = match form.get("grant_type") {
        Some("authorization_code") => {
            let code = model
                .codes
                .remove(form.get("code").unwrap_or(""))
                .ok_or_else(|| error("invalid_grant"))?;
            let verifier = form.get("code_verifier").unwrap_or("");
            if code.expires <= model.now
                || form.get("redirect_uri") != Some(&code.redirect)
                || !(43..=128).contains(&verifier.len())
                || !verifier
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
                || URL_SAFE_NO_PAD.encode(Sha256::digest(verifier)) != code.challenge
            {
                return Err(error("invalid_grant"));
            }
            (
                Grant {
                    account: code.account,
                    scopes: code.scopes,
                    expires: model.now + 3600,
                },
                code.offline,
            )
        }
        Some("refresh_token") => {
            let presented = form.get("refresh_token").unwrap_or("");
            let mut grant = model
                .refresh
                .get(presented)
                .cloned()
                .ok_or_else(|| error("invalid_grant"))?;
            grant.expires = model.now + 3600;
            if model.rotate_refresh {
                model.refresh.remove(presented);
            }
            (grant, model.rotate_refresh)
        }
        _ => return Err(error("unsupported_grant_type")),
    };
    let access = model.next("access");
    let response_scopes = grant
        .scopes
        .split_whitespace()
        .map(|scope| {
            if scope == "email" {
                "https://www.googleapis.com/auth/userinfo.email"
            } else {
                scope
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let mut body = json!({"access_token":access,"token_type":"Bearer","expires_in":3600,"scope":response_scopes});
    model.record_credential(&access, &grant.account);
    model.access.insert(access, grant.clone());
    if offline {
        let refresh = model.next("refresh");
        model.record_credential(&refresh, &grant.account);
        model.refresh.insert(refresh.clone(), grant);
        body["refresh_token"] = json!(refresh);
    }
    Ok(Reply::json(body))
}

pub(super) fn userinfo(input: &Input, account: &str) -> Result<Reply> {
    if input.method != "GET" {
        return Err(Reply::error(405, "methodNotAllowed"));
    }
    input.query.validate(&[], &[])?;
    let subject = match account {
        "alpha@example.test" => "100000000000000000001",
        "beta@example.test" => "100000000000000000002",
        _ => return Err(Reply::error(404, "notFound")),
    };
    Ok(Reply::json(
        json!({"sub":subject,"email":account,"email_verified":true}),
    ))
}

fn authorize(model: &mut Model, query: &Query) -> Result<Reply> {
    query
        .validate(
            &[
                "client_id",
                "redirect_uri",
                "response_type",
                "state",
                "code_challenge_method",
                "code_challenge",
                "scope",
                "access_type",
                "login_hint",
                "prompt",
                "include_granted_scopes",
            ],
            &[],
        )
        .map_err(|_| error("invalid_request"))?;
    if query.get("client_id") != Some("nuncio-test-client") {
        return Err(error("invalid_client"));
    }
    let redirect = query
        .required("redirect_uri")
        .map_err(|_| error("invalid_request"))?;
    let mut location = Url::parse(redirect).map_err(|_| error("invalid_request"))?;
    if location.scheme() != "http"
        || !location.username().is_empty()
        || location.password().is_some()
        || location.query().is_some()
        || location.fragment().is_some()
        || !matches!(location.host(),Some(url::Host::Ipv4(ip)) if ip.is_loopback())
            && !matches!(location.host(),Some(url::Host::Ipv6(ip)) if ip.is_loopback())
    {
        return Err(error("invalid_request"));
    }
    let state = query
        .required("state")
        .map_err(|_| error("invalid_request"))?;
    let challenge = query
        .required("code_challenge")
        .map_err(|_| error("invalid_request"))?;
    if query.get("response_type") != Some("code")
        || query.get("code_challenge_method") != Some("S256")
        || URL_SAFE_NO_PAD
            .decode(challenge)
            .map_or(true, |bytes| bytes.len() != 32)
    {
        return Err(error("invalid_request"));
    }
    let account = query.get("login_hint").unwrap_or("alpha@example.test");
    if !model.accounts.contains(account) {
        return Err(error("invalid_request"));
    }
    let requested = query
        .required("scope")
        .map_err(|_| error("invalid_scope"))?;
    if requested.split_whitespace().any(|scope| {
        ![
            "https://mail.google.com/",
            "https://www.googleapis.com/auth/gmail.modify",
            "https://www.googleapis.com/auth/gmail.readonly",
            "https://www.googleapis.com/auth/gmail.metadata",
            "https://www.googleapis.com/auth/gmail.send",
            "https://www.googleapis.com/auth/calendar",
            "https://www.googleapis.com/auth/calendar.readonly",
            "https://www.googleapis.com/auth/calendar.events",
            "openid",
            "email",
            "profile",
        ]
        .contains(&scope)
    }) {
        return Err(error("invalid_scope"));
    }
    let scopes = requested
        .split_whitespace()
        .filter(|scope| !model.denied_scopes.contains(*scope))
        .collect::<Vec<_>>()
        .join(" ");
    location.query_pairs_mut().append_pair("state", state);
    if model.deny_consent || scopes.is_empty() {
        location
            .query_pairs_mut()
            .append_pair("error", "access_denied");
    } else {
        let code = model.next("code");
        model.record_credential(&code, account);
        model.codes.insert(
            code.clone(),
            Code {
                account: account.into(),
                redirect: redirect.into(),
                challenge: challenge.into(),
                scopes,
                expires: model.now + 300,
                offline: query.get("access_type") == Some("offline"),
            },
        );
        location.query_pairs_mut().append_pair("code", &code);
    }
    Ok(Reply {
        status: 302,
        body: json!(null),
        location: Some(location.into()),
    })
}
