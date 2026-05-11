use std::{fs, path::PathBuf, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use axum::http::{HeaderMap, header};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rand::{Rng, distributions::Alphanumeric};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use serde_yaml::Value as YamlValue;
use sha2::{Digest, Sha256};
use tracing::{error, info, warn};
use url::Url;

use crate::config::{resolve_config_path, yaml_string};

const TOKEN_SAFETY_MARGIN_SECONDS: i64 = 15 * 60;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SsoSession {
    pub authenticated: bool,
    pub user: Option<SsoUser>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SsoUser {
    pub email: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct OAuthState {
    state: String,
    environment: String,
    auth_mode: String,
    client_id: String,
    redirect_uri: String,
    code_verifier: Option<String>,
    return_to: String,
    created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SaxoSessionCache {
    environment: String,
    auth_mode: String,
    client_id: String,
    redirect_uri: String,
    code_verifier: Option<String>,
    client_key: Option<String>,
    account_key: Option<String>,
    default_account_id: Option<String>,
    client_id_display: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    token_type: String,
    access_token_expires_at: Option<String>,
    refresh_token_expires_at: Option<String>,
    created_at: Option<String>,
    last_refreshed_at: Option<String>,
    refresh_token_invalid_at: Option<String>,
    refresh_error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<i64>,
    refresh_token_expires_in: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ClientMeResponse {
    client_key: Option<String>,
    default_account_key: Option<String>,
    default_account_id: Option<String>,
    client_id: Option<String>,
}

pub struct SaxoAuthStart {
    pub authorize_url: String,
    pub redirect_uri: String,
    pub environment: String,
    pub auth_mode: String,
}

impl SsoSession {
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let email = first_header(headers, "x-daytrader-user-email");
        let name = first_header(headers, "x-daytrader-user-name");
        let user = email.map(|email| SsoUser {
            name: name.unwrap_or_else(|| email.clone()),
            email,
        });
        Self {
            authenticated: user.is_some(),
            user,
        }
    }
}

pub async fn start_saxo_auth(
    config: &YamlValue,
    config_path: &PathBuf,
    headers: &HeaderMap,
) -> Result<SaxoAuthStart> {
    let environment = saxo_environment(config);
    let auth_mode = saxo_auth_mode(config);
    let client_id = yaml_string(config, &["saxo", "client_id"])
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("SAXO_CLIENT_ID is missing."))?;
    let state = random_url_token(32);
    let (code_verifier, code_challenge) = if auth_mode == "pkce" {
        let (verifier, challenge) = build_pkce_pair();
        (Some(verifier), Some(challenge))
    } else {
        (None, None)
    };
    let public_base_url = public_base_url(headers);
    let redirect_uri = format!("{public_base_url}/api/saxo/auth/callback");
    let return_to =
        first_header(headers, header::REFERER.as_str()).unwrap_or_else(|| "/".to_string());
    let authorize_url = build_authorize_url(
        &environment,
        &client_id,
        &redirect_uri,
        &state,
        &auth_mode,
        code_challenge.as_deref(),
    )?;
    let oauth_state = OAuthState {
        state: state.clone(),
        environment: environment.clone(),
        auth_mode: auth_mode.clone(),
        client_id,
        redirect_uri: redirect_uri.clone(),
        code_verifier,
        return_to,
        created_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    write_oauth_state(config, config_path, &state, &oauth_state)?;
    info!(
        environment = %environment,
        auth_mode = %auth_mode,
        redirect_uri = %redirect_uri,
        "created Saxo OAuth state"
    );

    Ok(SaxoAuthStart {
        authorize_url,
        redirect_uri,
        environment,
        auth_mode,
    })
}

pub async fn finish_saxo_auth(
    config: &YamlValue,
    config_path: &PathBuf,
    code: &str,
    state: &str,
) -> Result<String> {
    let oauth_state = pop_oauth_state(config, config_path, state)?;
    if oauth_state.state != state {
        bail!("Saxo OAuth state mismatch.");
    }
    let token_response = exchange_authorization_code(
        config,
        &oauth_state.environment,
        &oauth_state.auth_mode,
        &oauth_state.client_id,
        &oauth_state.redirect_uri,
        code,
        oauth_state.code_verifier.as_deref(),
    )
    .await?;
    let session_context =
        fetch_initial_session_context(&oauth_state.environment, &token_response.access_token)
            .await?;
    let session = SaxoSessionCache {
        environment: oauth_state.environment,
        auth_mode: oauth_state.auth_mode,
        client_id: oauth_state.client_id,
        redirect_uri: oauth_state.redirect_uri,
        code_verifier: oauth_state.code_verifier,
        client_key: session_context.client_key,
        account_key: session_context.default_account_key,
        default_account_id: session_context.default_account_id,
        client_id_display: session_context.client_id,
        access_token: Some(token_response.access_token),
        refresh_token: token_response.refresh_token,
        token_type: token_response
            .token_type
            .unwrap_or_else(|| "Bearer".to_string()),
        access_token_expires_at: Some(expires_at(token_response.expires_in.unwrap_or(0))),
        refresh_token_expires_at: token_response.refresh_token_expires_in.map(expires_at),
        created_at: Some(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        last_refreshed_at: Some(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        refresh_token_invalid_at: None,
        refresh_error: None,
    };
    save_session(&session_path(config, config_path), &session)?;
    info!(
        environment = %session.environment,
        auth_mode = %session.auth_mode,
        client_key_present = session.client_key.as_ref().is_some_and(|value| !value.is_empty()),
        account_key_present = session.account_key.as_ref().is_some_and(|value| !value.is_empty()),
        "stored Saxo OAuth session"
    );
    Ok(oauth_state.return_to)
}

pub async fn auth_status(
    config: &YamlValue,
    config_path: &PathBuf,
    auto_refresh: bool,
) -> JsonValue {
    let path = session_path(config, config_path);
    if auto_refresh {
        if let Err(err) = ensure_access_token(config, config_path).await {
            warn!(
                session_path = %path.display(),
                "Saxo auto-refresh skipped while building auth status: {err:#}"
            );
        }
    }
    match load_session(&path) {
        Ok(session) => session_status(config, &path, &session),
        Err(err) => base_status(config, &path, Some(err.to_string())),
    }
}

pub async fn session_api(config: &YamlValue, config_path: &PathBuf) -> JsonValue {
    let path = session_path(config, config_path);
    match load_session(&path) {
        Ok(session) => {
            let mut status = session_status(config, &path, &session);
            if let Some(obj) = status.as_object_mut() {
                obj.insert("auth_mode".to_string(), json!(session.auth_mode));
                obj.insert(
                    "client_key_present".to_string(),
                    json!(session.client_key.as_ref().is_some_and(|v| !v.is_empty())),
                );
                obj.insert(
                    "account_key_present".to_string(),
                    json!(session.account_key.as_ref().is_some_and(|v| !v.is_empty())),
                );
                obj.insert(
                    "default_account_id".to_string(),
                    json!(session.default_account_id),
                );
                obj.insert(
                    "client_id_display".to_string(),
                    json!(session.client_id_display),
                );
            }
            status
        }
        Err(err) => base_status(config, &path, Some(err.to_string())),
    }
}

pub async fn refresh_session(config: &YamlValue, config_path: &PathBuf) -> Result<JsonValue> {
    ensure_access_token(config, config_path).await?;
    Ok(auth_status(config, config_path, false).await)
}

pub fn logout_session(config: &YamlValue, config_path: &PathBuf) -> Result<JsonValue> {
    let path = session_path(config, config_path);
    if path.exists() {
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(base_status(config, &path, None))
}

pub fn export_session_json(config: &YamlValue, config_path: &PathBuf) -> Result<JsonValue> {
    let session = load_session(&session_path(config, config_path))?;
    Ok(serde_json::to_value(session)?)
}

pub fn import_session_json(
    config: &YamlValue,
    config_path: &PathBuf,
    value: &JsonValue,
) -> Result<()> {
    let session = serde_json::from_value::<SaxoSessionCache>(value.clone())
        .context("decoding Saxo session JSON from database")?;
    save_session(&session_path(config, config_path), &session)
}

async fn ensure_access_token(
    config: &YamlValue,
    config_path: &PathBuf,
) -> Result<SaxoSessionCache> {
    let path = session_path(config, config_path);
    let mut session = load_session(&path)?;
    if access_token_valid(&session) {
        info!(
            environment = %session.environment,
            session_path = %path.display(),
            "Saxo access token is still within the safety window"
        );
        return Ok(session);
    }
    if !refresh_token_valid(&session) {
        warn!(
            environment = %session.environment,
            session_path = %path.display(),
            "Saxo refresh token is missing, expired, or already marked invalid"
        );
        bail!("No valid Saxo refresh token is available. Re-authentication is required.");
    }

    info!(
        environment = %session.environment,
        auth_mode = %session.auth_mode,
        session_path = %path.display(),
        "refreshing Saxo access token"
    );
    match refresh_access_token(config, &session).await {
        Ok(refreshed) => {
            save_session(&path, &refreshed)?;
            info!(
                environment = %refreshed.environment,
                session_path = %path.display(),
                "Saxo access token refreshed"
            );
            Ok(refreshed)
        }
        Err(err) => {
            let message = err.to_string();
            if message.contains("HTTP 400") || message.contains("HTTP 401") {
                session.refresh_token_invalid_at =
                    Some(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
                session.refresh_error = Some(message);
                let _ = save_session(&path, &session);
                warn!(
                    environment = %session.environment,
                    session_path = %path.display(),
                    "Saxo refresh token marked invalid after authorization failure"
                );
            } else {
                error!(
                    environment = %session.environment,
                    session_path = %path.display(),
                    "Saxo token refresh failed: {err:#}"
                );
            }
            Err(err)
        }
    }
}

async fn refresh_access_token(
    config: &YamlValue,
    session: &SaxoSessionCache,
) -> Result<SaxoSessionCache> {
    let environment = session.environment.to_lowercase();
    let client_id = yaml_string(config, &["saxo", "client_id"])
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| session.client_id.clone());
    let client_secret = yaml_string(config, &["saxo", "client_secret"]).unwrap_or_default();
    let token_url = format!("{}/token", auth_base_url(&environment)?);

    let mut form = vec![
        ("grant_type", "refresh_token".to_string()),
        (
            "refresh_token",
            session.refresh_token.clone().unwrap_or_default(),
        ),
    ];
    let client = http_client()?;
    let mut request = client.post(token_url).form(&form);
    if session.auth_mode == "pkce" {
        form.push(("client_id", client_id));
        if let Some(code_verifier) = &session.code_verifier {
            form.push(("code_verifier", code_verifier.clone()));
        }
        form.push(("redirect_uri", session.redirect_uri.clone()));
        request = client
            .post(format!("{}/token", auth_base_url(&environment)?))
            .form(&form);
    } else {
        if client_secret.trim().is_empty() {
            bail!("SAXO_CLIENT_SECRET is missing for secret-based Saxo token refresh.");
        }
        request = request.basic_auth(client_id, Some(client_secret));
    }
    let token_response = send_token_request(request).await?;

    let mut refreshed = SaxoSessionCache {
        access_token: Some(token_response.access_token),
        refresh_token: token_response
            .refresh_token
            .or_else(|| session.refresh_token.clone()),
        token_type: token_response
            .token_type
            .unwrap_or_else(|| session.token_type.clone()),
        access_token_expires_at: Some(expires_at(token_response.expires_in.unwrap_or(0))),
        refresh_token_expires_at: token_response
            .refresh_token_expires_in
            .map(expires_at)
            .or_else(|| session.refresh_token_expires_at.clone()),
        last_refreshed_at: Some(Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        refresh_token_invalid_at: None,
        refresh_error: None,
        environment: session.environment.clone(),
        auth_mode: session.auth_mode.clone(),
        client_id: session.client_id.clone(),
        redirect_uri: session.redirect_uri.clone(),
        code_verifier: session.code_verifier.clone(),
        client_key: session.client_key.clone(),
        account_key: session.account_key.clone(),
        default_account_id: session.default_account_id.clone(),
        client_id_display: session.client_id_display.clone(),
        created_at: session.created_at.clone(),
    };

    if refreshed.client_key.is_none() || refreshed.account_key.is_none() {
        if let Some(access_token) = &refreshed.access_token {
            let context = fetch_initial_session_context(&environment, access_token).await?;
            refreshed.client_key = context.client_key;
            refreshed.account_key = context.default_account_key;
            refreshed.default_account_id = context.default_account_id;
            refreshed.client_id_display = context.client_id;
        }
    }
    Ok(refreshed)
}

async fn exchange_authorization_code(
    config: &YamlValue,
    environment: &str,
    auth_mode: &str,
    client_id: &str,
    redirect_uri: &str,
    code: &str,
    code_verifier: Option<&str>,
) -> Result<TokenResponse> {
    let token_url = format!("{}/token", auth_base_url(environment)?);
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", redirect_uri.to_string()),
    ];
    let client = http_client()?;
    let mut request = client.post(token_url).form(&form);
    if auth_mode == "pkce" {
        form.push(("client_id", client_id.to_string()));
        form.push(("code_verifier", code_verifier.unwrap_or("").to_string()));
        request = client
            .post(format!("{}/token", auth_base_url(environment)?))
            .form(&form);
    } else {
        let client_secret = yaml_string(config, &["saxo", "client_secret"]).unwrap_or_default();
        if client_secret.trim().is_empty() {
            bail!("SAXO_CLIENT_SECRET is missing for secret-based Saxo authorization.");
        }
        request = request.basic_auth(client_id.to_string(), Some(client_secret));
    }
    send_token_request(request).await
}

async fn fetch_initial_session_context(
    environment: &str,
    access_token: &str,
) -> Result<ClientMeResponse> {
    let url = format!("{}/port/v1/clients/me", openapi_base_url(environment)?);
    let response = http_client()?
        .get(url)
        .bearer_auth(access_token)
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .context("fetching Saxo client context")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        bail!("Saxo client context request failed with HTTP {status}: {body}");
    }
    Ok(response.json::<ClientMeResponse>().await?)
}

async fn send_token_request(request: reqwest::RequestBuilder) -> Result<TokenResponse> {
    let response = request.send().await.context("sending Saxo token request")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let marker = if status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED {
            format!("HTTP {}", status.as_u16())
        } else {
            status.to_string()
        };
        bail!("Saxo token request failed with {marker}: {body}");
    }
    Ok(response.json::<TokenResponse>().await?)
}

fn session_status(config: &YamlValue, path: &PathBuf, session: &SaxoSessionCache) -> JsonValue {
    let access_expires_at = parse_iso(session.access_token_expires_at.as_deref());
    let refresh_expires_at = parse_iso(session.refresh_token_expires_at.as_deref());
    let expires_in_minutes = minutes_until(access_expires_at);
    let refresh_expires_in_minutes = minutes_until(refresh_expires_at);
    let token_valid = access_token_valid(session);
    let refresh_token_valid = refresh_token_valid(session);
    let needs_reauth = !token_valid && !refresh_token_valid;
    let (status, status_text, connected) = if token_valid && expires_in_minutes.unwrap_or(60) >= 10
    {
        ("healthy", "Connected to Saxo.", true)
    } else if token_valid {
        ("expiring_soon", "Saxo access token is expiring soon.", true)
    } else if refresh_token_valid {
        (
            "refresh_available",
            "Access token expired, refresh token is still valid.",
            false,
        )
    } else {
        (
            "needs_reauth",
            session
                .refresh_error
                .as_deref()
                .unwrap_or("Saxo session expired. Re-authentication is required."),
            false,
        )
    };

    json!({
        "connected": connected,
        "environment": session.environment,
        "configured_environment": saxo_environment(config),
        "token_valid": token_valid,
        "refresh_token_valid": refresh_token_valid,
        "expires_at": access_expires_at.map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        "expires_in_minutes": expires_in_minutes,
        "refresh_expires_at": refresh_expires_at.map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        "refresh_expires_in_minutes": refresh_expires_in_minutes,
        "last_refreshed_at": session.last_refreshed_at,
        "refreshing": false,
        "needs_reauth": needs_reauth,
        "status": status,
        "status_text": status_text,
        "session_path": path.display().to_string(),
        "error": if needs_reauth { session.refresh_error.clone() } else { None },
    })
}

fn base_status(config: &YamlValue, path: &PathBuf, error: Option<String>) -> JsonValue {
    json!({
        "connected": false,
        "environment": saxo_environment(config),
        "configured_environment": saxo_environment(config),
        "token_valid": false,
        "refresh_token_valid": false,
        "expires_at": null,
        "expires_in_minutes": null,
        "refresh_expires_at": null,
        "refresh_expires_in_minutes": null,
        "last_refreshed_at": null,
        "refreshing": false,
        "needs_reauth": true,
        "status": "missing_session",
        "status_text": "Saxo session file is missing.",
        "session_path": path.display().to_string(),
        "error": error,
    })
}

fn access_token_valid(session: &SaxoSessionCache) -> bool {
    let Some(expires_at) = parse_iso(session.access_token_expires_at.as_deref()) else {
        return false;
    };
    session
        .access_token
        .as_ref()
        .is_some_and(|value| !value.is_empty())
        && expires_at > Utc::now() + ChronoDuration::seconds(TOKEN_SAFETY_MARGIN_SECONDS)
}

fn refresh_token_valid(session: &SaxoSessionCache) -> bool {
    if session.refresh_token_invalid_at.is_some() {
        return false;
    }
    let Some(expires_at) = parse_iso(session.refresh_token_expires_at.as_deref()) else {
        return false;
    };
    session
        .refresh_token
        .as_ref()
        .is_some_and(|value| !value.is_empty())
        && expires_at > Utc::now() + ChronoDuration::seconds(TOKEN_SAFETY_MARGIN_SECONDS)
}

fn minutes_until(value: Option<DateTime<Utc>>) -> Option<i64> {
    value.map(|value| ((value - Utc::now()).num_seconds() / 60).max(0))
}

fn parse_iso(value: Option<&str>) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value?.replace('Z', "+00:00").as_str())
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn expires_at(seconds: i64) -> String {
    (Utc::now() + ChronoDuration::seconds(seconds.max(0)))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn build_pkce_pair() -> (String, String) {
    let verifier = random_url_token(64);
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(digest);
    (verifier, challenge)
}

fn random_url_token(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

fn build_authorize_url(
    environment: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    auth_mode: &str,
    code_challenge: Option<&str>,
) -> Result<String> {
    let mut url = Url::parse(&format!("{}/authorize", auth_base_url(environment)?))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code");
        query.append_pair("client_id", client_id);
        query.append_pair("state", state);
        query.append_pair("redirect_uri", redirect_uri);
        if auth_mode == "pkce" {
            let challenge = code_challenge
                .ok_or_else(|| anyhow!("PKCE authorization requires a code challenge."))?;
            query.append_pair("code_challenge", challenge);
            query.append_pair("code_challenge_method", "S256");
        }
    }
    Ok(url.to_string())
}

fn public_base_url(headers: &HeaderMap) -> String {
    if let Some(host) = first_header(headers, "x-forwarded-host") {
        let proto =
            first_header(headers, "x-forwarded-proto").unwrap_or_else(|| "https".to_string());
        return format!("{}://{}", first_csv_value(&proto), first_csv_value(&host));
    }
    if let Some(origin) = first_header(headers, header::ORIGIN.as_str()) {
        return origin.trim_end_matches('/').to_string();
    }
    if let Some(referer) = first_header(headers, header::REFERER.as_str()) {
        if let Ok(url) = Url::parse(&referer) {
            if let Some(host) = url.host_str() {
                let port = url
                    .port()
                    .map(|port| format!(":{port}"))
                    .unwrap_or_default();
                return format!("{}://{}{}", url.scheme(), host, port);
            }
        }
    }
    let host = first_header(headers, header::HOST.as_str())
        .unwrap_or_else(|| "localhost:8000".to_string());
    format!("http://{host}")
}

fn first_header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)?
        .to_str()
        .ok()
        .map(first_csv_value)
        .filter(|value| !value.trim().is_empty())
}

fn first_csv_value(value: &str) -> String {
    value.split(',').next().unwrap_or(value).trim().to_string()
}

fn session_path(config: &YamlValue, config_path: &PathBuf) -> PathBuf {
    let configured = yaml_string(config, &["saxo", "session_path"])
        .unwrap_or_else(|| "/tmp/daytrader/saxo_session.json".to_string());
    resolve_config_path(config_path, &configured)
}

fn oauth_state_path(config: &YamlValue, config_path: &PathBuf, state: &str) -> PathBuf {
    session_path(config, config_path)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(format!("saxo_oauth_state_{state}.json"))
}

fn write_oauth_state(
    config: &YamlValue,
    config_path: &PathBuf,
    state: &str,
    payload: &OAuthState,
) -> Result<()> {
    let path = oauth_state_path(config, config_path, state);
    write_private_json(&path, payload)
}

fn pop_oauth_state(config: &YamlValue, config_path: &PathBuf, state: &str) -> Result<OAuthState> {
    let path = oauth_state_path(config, config_path, state);
    let text = fs::read_to_string(&path).with_context(|| {
        format!(
            "Saxo OAuth state was not found or has expired: {}",
            path.display()
        )
    })?;
    let payload = serde_json::from_str::<OAuthState>(&text)?;
    let _ = fs::remove_file(&path);
    let created_at = parse_iso(Some(&payload.created_at))
        .ok_or_else(|| anyhow!("Saxo OAuth state has an invalid timestamp."))?;
    if created_at < Utc::now() - ChronoDuration::minutes(10) {
        bail!("Saxo OAuth state has expired. Start re-authentication again.");
    }
    Ok(payload)
}

fn load_session(path: &PathBuf) -> Result<SaxoSessionCache> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("Saxo session file is missing: {}", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

fn save_session(path: &PathBuf, payload: &SaxoSessionCache) -> Result<()> {
    write_private_json(path, payload)
}

fn write_private_json<T: Serialize>(path: &PathBuf, payload: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(payload)? + "\n";
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn saxo_environment(config: &YamlValue) -> String {
    yaml_string(config, &["saxo", "environment"])
        .unwrap_or_else(|| "sim".to_string())
        .to_lowercase()
}

fn saxo_auth_mode(config: &YamlValue) -> String {
    match yaml_string(config, &["saxo", "auth_mode"])
        .unwrap_or_default()
        .to_lowercase()
        .as_str()
    {
        "pkce" => "pkce".to_string(),
        "secret" => "secret".to_string(),
        _ if saxo_environment(config) == "live" => "secret".to_string(),
        _ => "pkce".to_string(),
    }
}

fn auth_base_url(environment: &str) -> Result<&'static str> {
    match environment.to_lowercase().as_str() {
        "sim" => Ok("https://sim.logonvalidation.net"),
        "live" => Ok("https://live.logonvalidation.net"),
        _ => bail!("Unsupported Saxo environment: {environment}"),
    }
}

fn openapi_base_url(environment: &str) -> Result<&'static str> {
    match environment.to_lowercase().as_str() {
        "sim" => Ok("https://gateway.saxobank.com/sim/openapi"),
        "live" => Ok("https://gateway.saxobank.com/openapi"),
        _ => bail!("Unsupported Saxo environment: {environment}"),
    }
}

fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?)
}

pub fn oauth_callback_html(ok: bool, title: &str, message: &str, return_to: &str) -> String {
    let color = if ok { "#0a7f39" } else { "#b42318" };
    let safe_title = html_escape(title);
    let safe_message = html_escape(message);
    let safe_return_to = html_escape(return_to);
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta http-equiv="refresh" content="2; url={safe_return_to}" />
    <title>{safe_title}</title>
    <style>
      body {{ font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; margin: 4rem; color: #111827; }}
      main {{ max-width: 44rem; padding: 2rem; border: 1px solid #d8e0ea; border-radius: 8px; box-shadow: 0 18px 60px rgba(15, 23, 42, 0.08); background: white; }}
      h1 {{ color: {color}; margin-top: 0; }}
      a {{ color: #2563eb; }}
    </style>
  </head>
  <body>
    <main>
      <h1>{safe_title}</h1>
      <p>{safe_message}</p>
      <p>Returning to the dashboard...</p>
      <p><a href="{safe_return_to}">Continue now</a></p>
    </main>
  </body>
</html>"#
    )
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    #[test]
    fn reads_ngrok_sso_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-daytrader-user-email",
            HeaderValue::from_static("user@example.com"),
        );
        headers.insert(
            "x-daytrader-user-name",
            HeaderValue::from_static("User Name"),
        );

        let session = SsoSession::from_headers(&headers);

        assert!(session.authenticated);
        assert_eq!(session.user.unwrap().email, "user@example.com");
    }

    #[test]
    fn builds_public_base_url_from_forwarded_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        headers.insert(
            "x-forwarded-host",
            HeaderValue::from_static("example.ngrok-free.dev"),
        );

        assert_eq!(public_base_url(&headers), "https://example.ngrok-free.dev");
    }
}
