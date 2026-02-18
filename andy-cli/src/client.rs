use std::path::PathBuf;

use anyhow::{Result, bail};
use bytes::Bytes;
use reqwest::Client as ReqwestClient;

use crate::a11y::A11yTree;
use crate::types::*;

pub struct Client {
    http: ReqwestClient,
}

impl Client {
    pub fn new(socket_path: PathBuf) -> Self {
        let http = ReqwestClient::builder()
            .unix_socket(socket_path)
            .build()
            .expect("build reqwest client");
        Self { http }
    }

    async fn get(&self, path: &str) -> Result<Bytes> {
        let resp = self
            .http
            .get(format!("http://localhost{path}"))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("{path}: {status} {text}");
        }
        Ok(resp.bytes().await?)
    }


    async fn post_json(&self, path: &str, json: &impl serde::Serialize) -> Result<()> {
        let resp = self
            .http
            .post(format!("http://localhost{path}"))
            .json(json)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("{path}: {status} {text}");
        }
        Ok(())
    }

    pub async fn ensure_screen(&self, name: &str, package: &str) -> Result<()> {
        self.post_json(
            "/screens",
            &CreateScreenRequest {
                name: name.to_string(),
                width: 1080,
                height: 1920,
                dpi: 240,
                timeout_secs: 300,
                package: package.to_string(),
            },
        )
        .await
    }

    pub async fn info(&self, screen: &str) -> Result<ScreenInfo> {
        let body = self.get(&format!("/screens/{screen}/info")).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    pub async fn screenshot(&self, screen: &str, no_wait: bool) -> Result<(Bytes, Option<u64>)> {
        let mut url = format!("/screens/{screen}/screenshot");
        if no_wait {
            url.push_str("?no_wait=true");
        }
        let resp = self
            .http
            .get(format!("http://localhost{url}"))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("{url}: {status} {text}");
        }
        let wait_ms = resp
            .headers()
            .get("X-Wait-Ms")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        Ok((resp.bytes().await?, wait_ms))
    }

    pub async fn a11y(&self, screen: &str, no_wait: bool) -> Result<(A11yTree, Option<u64>)> {
        let mut url = format!("/screens/{screen}/a11y");
        if no_wait {
            url.push_str("?no_wait=true");
        }
        let resp = self
            .http
            .get(format!("http://localhost{url}"))
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("{url}: {status} {text}");
        }
        let wait_ms = resp
            .headers()
            .get("X-Wait-Ms")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let body = resp.bytes().await?;
        Ok((serde_json::from_slice(&body)?, wait_ms))
    }

    pub async fn tap(&self, screen: &str, x: f32, y: f32, no_wait: bool) -> Result<Option<u64>> {
        let mut url = format!("/screens/{screen}/tap");
        if no_wait {
            url.push_str("?no_wait=true");
        }
        let resp = self
            .http
            .post(format!("http://localhost{url}"))
            .json(&TapRequest { x, y })
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("{url}: {status} {text}");
        }
        let wait_ms = resp
            .headers()
            .get("X-Wait-Ms")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        Ok(wait_ms)
    }

    pub async fn swipe(
        &self,
        screen: &str,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        duration_ms: i64,
    ) -> Result<()> {
        self.post_json(
            &format!("/screens/{screen}/swipe"),
            &SwipeRequest {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            },
        )
        .await
    }

    pub async fn type_text(&self, screen: &str, text: &str) -> Result<()> {
        self.post_json(
            &format!("/screens/{screen}/type"),
            &TypeRequest {
                text: text.to_string(),
            },
        )
        .await
    }

    pub async fn key(&self, screen: &str, keycode: i32) -> Result<()> {
        self.post_json(&format!("/screens/{screen}/key"), &KeyRequest { keycode })
            .await
    }

    pub async fn launch(&self, screen: &str, no_wait: bool) -> Result<Option<u64>> {
        let mut url = format!("/screens/{screen}/launch");
        if no_wait {
            url.push_str("?no_wait=true");
        }
        let resp = self.http.post(format!("http://localhost{url}")).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            bail!("{url}: {status} {text}");
        }
        let wait_ms = resp
            .headers()
            .get("X-Wait-Ms")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        Ok(wait_ms)
    }

    pub async fn stop(&self, screen: &str) -> Result<()> {
        let _ = self.http.post(format!("http://localhost/screens/{screen}/stop")).send().await?;
        Ok(())
    }

    pub async fn reset(&self, screen: &str) -> Result<()> {
        let _ = self.http.post(format!("http://localhost/screens/{screen}/reset")).send().await?;
        Ok(())
    }

    pub async fn open_url(&self, screen: &str, url: &str) -> Result<()> {
        self.post_json(
            &format!("/screens/{screen}/open-url"),
            &OpenUrlRequest { url: url.to_string() },
        )
        .await
    }

    pub async fn wait_for_idle(
        &self,
        screen: &str,
        idle_timeout_ms: i64,
        global_timeout_ms: i64,
    ) -> Result<()> {
        self.post_json(
            &format!("/screens/{screen}/wait-for-idle"),
            &WaitForIdleRequest {
                idle_timeout_ms,
                global_timeout_ms,
            },
        )
        .await
    }

}
