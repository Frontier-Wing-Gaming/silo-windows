//! The native player's own login session, kept per server and account.

use crate::api::{is_status, Client, Tokens, Viewer};
use anyhow::{Context, Result};
use reqwest::Method;
use serde_json::Value;
use std::path::PathBuf;
use tokio::sync::Mutex;

pub struct Session {
    path: PathBuf,
    tokens: Mutex<Option<Tokens>>,
}

impl Session {
    pub fn load(path: PathBuf) -> Self {
        let tokens = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok());
        Self { path, tokens: Mutex::new(tokens) }
    }

    fn save(&self, tokens: &Option<Tokens>) {
        let result = match tokens {
            Some(t) => serde_json::to_vec(t).map_err(anyhow::Error::from).and_then(|b| {
                if let Some(dir) = self.path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&self.path, b).map_err(Into::into)
            }),
            None => {
                let _ = std::fs::remove_file(&self.path);
                Ok(())
            }
        };
        if let Err(e) = result {
            log::warn!("could not persist the player session: {e}");
        }
    }


    /// Makes sure the player holds a session for the account the web app is
    /// signed in to, pairing a new one when it has none or holds another
    /// account's.
    pub async fn ensure(&self, client: &Client, web_bearer: &str) -> Result<()> {
        let account = client.account_id(web_bearer).await.context("reading the signed-in account")?;
        let mut guard = self.tokens.lock().await;
        if guard.as_ref().is_some_and(|t| t.account_id == account) {
            return Ok(());
        }
        let mut tokens = client.pair(web_bearer).await.context("pairing the player")?;
        if tokens.account_id.is_empty() {
            tokens.account_id = account;
        }
        *guard = Some(tokens);
        self.save(&guard);
        Ok(())
    }

    pub async fn access_token(&self) -> Option<String> {
        self.tokens.lock().await.as_ref().map(|t| t.access_token.clone())
    }

    /// Rotates the refresh token. `stale` is the access token that was
    /// refused, so a caller that lost the race reuses the winner's rotation
    /// instead of spending the refresh token twice.
    async fn refresh(&self, client: &Client, stale: &str) -> Result<()> {
        let mut guard = self.tokens.lock().await;
        let Some(current) = guard.clone() else { anyhow::bail!("the player is not signed in") };
        if current.access_token != stale {
            return Ok(());
        }
        match client.refresh(&current).await {
            Ok(next) => {
                *guard = Some(next);
                self.save(&guard);
                Ok(())
            }
            Err(e) => {
                if is_status(&e, 401) {
                    *guard = None;
                    self.save(&guard);
                }
                Err(e)
            }
        }
    }

    /// Sends an authenticated request, refreshing once on a 401.
    pub async fn send(&self, client: &Client, method: Method, path: &str, viewer: &Viewer, body: Option<&Value>) -> Result<Value> {
        let token = self.access_token().await.context("the player is not signed in")?;
        match client.send(method.clone(), path, Some(&token), Some(viewer), body).await {
            Err(e) if is_status(&e, 401) => {
                self.refresh(client, &token).await?;
                let token = self.access_token().await.context("the player is not signed in")?;
                client.send(method, path, Some(&token), Some(viewer), body).await
            }
            other => other,
        }
    }
}
