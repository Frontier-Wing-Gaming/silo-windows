//! Thin client for the parts of Silo's native `/api/v2` that the desktop
//! player needs: device pairing, token refresh, catalog lookup, and the
//! playback start / progress / stop lifecycle.

use anyhow::{anyhow, bail, Context, Result};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use url::Url;

pub const DEVICE_PLATFORM: &str = "windows";
pub const CLIENT_NAME: &str = "Silolux for Windows";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The profile the web app is acting as, captured from its own requests.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Viewer {
    pub profile_id: String,
    #[serde(default)]
    pub profile_token: Option<String>,
}

/// A login session owned by the native player (never the web app's own).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tokens {
    pub account_id: String,
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub body: Value,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = self.body.get("detail").and_then(Value::as_str).unwrap_or("");
        write!(f, "{} {} {}", self.status.as_u16(), self.code, detail)
    }
}
impl std::error::Error for ApiError {}

#[derive(Clone)]
pub struct Client {
    pub base: Url,
    pub device_id: String,
    http: reqwest::Client,
}

impl Client {
    pub fn new(base: Url, device_id: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!("{CLIENT_NAME}/{APP_VERSION}"))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(45))
            .build()?;
        Ok(Self { base, device_id, http })
    }

    pub fn url(&self, path: &str) -> Result<Url> {
        Ok(self.base.join(path)?)
    }

    /// Whether `target` is on the configured server's origin. Account
    /// credentials are only ever sent there, never to a node or proxy origin
    /// named in a playback plan.
    pub fn same_origin(&self, target: &Url) -> bool {
        target.origin() == self.base.origin()
    }

    fn device_headers(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.header("X-Silo-Device-Id", &self.device_id)
            .header("X-Silo-Device-Name", device_name())
            .header("X-Silo-Device-Platform", DEVICE_PLATFORM)
            .header("X-Silo-Client", CLIENT_NAME)
            .header("X-Silo-Client-Version", APP_VERSION)
            .header("X-Device-ID", &self.device_id)
    }

    pub async fn send(
        &self,
        method: Method,
        path: &str,
        bearer: Option<&str>,
        viewer: Option<&Viewer>,
        body: Option<&Value>,
    ) -> Result<Value> {
        let mut rb = self
            .device_headers(self.http.request(method, self.url(path)?))
            .header("Accept", "application/json");
        if let Some(token) = bearer {
            rb = rb.bearer_auth(token);
        }
        if let Some(v) = viewer {
            rb = rb.header("X-Profile-Id", &v.profile_id);
            if let Some(t) = v.profile_token.as_deref().filter(|t| !t.is_empty()) {
                rb = rb.header("X-Profile-Token", t);
            }
        }
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let res = rb.send().await.with_context(|| format!("request to {path} failed"))?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        let value: Value = if text.is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::String(text)) };
        if status.is_success() {
            return Ok(value);
        }
        let code = value
            .get("code")
            .or_else(|| value.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("error")
            .to_string();
        Err(ApiError { status, code, body: value }.into())
    }

    // ---- discovery ---------------------------------------------------------

    pub async fn system_info(&self) -> Result<Value> {
        self.send(Method::GET, "/api/v2/system/info", None, None, None).await
    }

    // ---- auth --------------------------------------------------------------

    pub async fn account_id(&self, bearer: &str) -> Result<String> {
        let me = self.send(Method::GET, "/api/v2/account/me", Some(bearer), None, None).await?;
        me.get("id").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| anyhow!("account has no id"))
    }

    /// Opens a login session for the native player through device sign-in,
    /// approved on the spot with the web app's own session. The web app keeps
    /// its refresh token to itself; the two never share one.
    pub async fn pair(&self, web_bearer: &str) -> Result<Tokens> {
        let start = self
            .send(
                Method::POST,
                "/api/v2/auth/device/start",
                None,
                None,
                Some(&json!({ "device_name": device_name(), "device_platform": DEVICE_PLATFORM })),
            )
            .await?;
        let device_code = str_field(&start, "device_code")?;
        let user_code: String = str_field(&start, "user_code")?.chars().filter(char::is_ascii_digit).collect();
        self.send(Method::POST, "/api/v2/auth/device/approve", Some(web_bearer), None, Some(&json!({ "code": user_code })))
            .await
            .context("approving the player's sign-in")?;
        for _ in 0..10 {
            let poll = self
                .send(Method::POST, "/api/v2/auth/device/poll", None, None, Some(&json!({ "device_code": device_code })))
                .await?;
            match poll.get("status").and_then(Value::as_str) {
                Some("approved") => {
                    let t = poll.get("tokens").ok_or_else(|| anyhow!("approved without tokens"))?;
                    return Ok(Tokens {
                        account_id: t.pointer("/user/id").and_then(Value::as_str).unwrap_or_default().to_string(),
                        access_token: str_field(t, "access_token")?,
                        refresh_token: str_field(t, "refresh_token")?,
                    });
                }
                Some("pending") => {
                    let wait = poll.get("poll_after").and_then(Value::as_u64).unwrap_or(1).clamp(1, 5);
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                }
                other => bail!("device sign-in ended as {:?}", other),
            }
        }
        bail!("device sign-in was not collected in time")
    }

    pub async fn refresh(&self, tokens: &Tokens) -> Result<Tokens> {
        let r = self
            .send(Method::POST, "/api/v2/auth/refresh", None, None, Some(&json!({ "refresh_token": tokens.refresh_token })))
            .await?;
        Ok(Tokens {
            account_id: tokens.account_id.clone(),
            access_token: str_field(&r, "access_token")?,
            refresh_token: str_field(&r, "refresh_token")?,
        })
    }
}

pub fn str_field(v: &Value, key: &str) -> Result<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| anyhow!("missing `{key}`"))
}

pub fn device_name() -> String {
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows PC".into());
    format!("{CLIENT_NAME} ({host})")
}

pub fn is_status(err: &anyhow::Error, status: u16) -> bool {
    err.downcast_ref::<ApiError>().is_some_and(|e| e.status.as_u16() == status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(base: &str) -> Client {
        Client::new(Url::parse(base).unwrap(), "test-device".into()).unwrap()
    }

    #[test]
    fn credentials_stay_on_the_server_origin() {
        let c = client("http://192.168.0.10:8090/");
        assert!(c.same_origin(&Url::parse("http://192.168.0.10:8090/api/v2/stream/x").unwrap()));
        // Another port, scheme or host is another origin: a transcode node or a
        // proxy named in a playback plan must never get the account token.
        assert!(!c.same_origin(&Url::parse("http://192.168.0.10:9000/stream").unwrap()));
        assert!(!c.same_origin(&Url::parse("https://192.168.0.10:8090/stream").unwrap()));
        assert!(!c.same_origin(&Url::parse("http://node.example:8090/stream").unwrap()));
    }

    #[test]
    fn relative_paths_join_onto_the_server() {
        let c = client("https://media.example/silo/");
        assert_eq!(c.url("api/v2/system/info").unwrap().as_str(), "https://media.example/silo/api/v2/system/info");
        assert_eq!(c.url("/api/v2/system/info").unwrap().as_str(), "https://media.example/api/v2/system/info");
    }
}
