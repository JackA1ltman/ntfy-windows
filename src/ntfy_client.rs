//! Minimal asynchronous ntfy client.
//!
//! ntfy's subscribe API puts the topic **in the path**, for example:
//! * `GET /{topic}/json` – newline delimited JSON (NDJSON)
//! * `GET /{topic}/sse`  – Server-Sent Events
//! * `GET /{topic}/ws`   – WebSocket
//! * `GET /{topic}/json?poll=1&since=...` – polling
//!
//! Multiple topics are comma separated: `/{topic1,topic2}/json`.

use std::future::Future;
use std::time::Duration;

use anyhow::{bail, Result};
use base64::Engine as _;
use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::config::{AuthConfig, AuthMethod, Config, Transport};
use crate::models::NtfyMessage;

pub struct NtfyClient {
    http: reqwest::Client,
    base_url: String,
    auth: AuthConfig,
}

impl NtfyClient {
    pub fn new(config: &Config) -> Result<Self> {
        let mut builder = reqwest::Client::builder()
            .user_agent(format!("ntfy-windows/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15));
        if config.server.insecure_tls {
            builder = builder.danger_accept_invalid_certs(true);
        }
        let http = builder.build()?;
        let base_url = config.server.base_url.trim().trim_end_matches('/').to_string();
        let base_url = if base_url.is_empty() {
            "https://ntfy.sh".to_string()
        } else {
            base_url
        };
        Ok(Self {
            http,
            base_url,
            auth: config.auth.clone(),
        })
    }

    fn authorize(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.auth.method {
            AuthMethod::None => rb,
            AuthMethod::Basic => {
                rb.basic_auth(self.auth.username.clone(), Some(self.auth.password.clone()))
            }
            AuthMethod::Token => rb.bearer_auth(self.auth.token.clone()),
        }
    }

    /// Comma separated, non-empty topic path segment.
    fn topic_path(topics: &[String]) -> Result<String> {
        let clean: Vec<&str> = topics
            .iter()
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .collect();
        if clean.is_empty() {
            bail!("没有配置任何订阅主题");
        }
        Ok(clean.join(","))
    }

    /// Subscribe via `/json` or `/sse` and stream messages until disconnect.
    pub async fn run_stream<O, F, Fut>(
        &self,
        topics: &[String],
        transport: Transport,
        cancel: &CancellationToken,
        on_open: O,
        mut handler: F,
    ) -> Result<()>
    where
        O: FnOnce(),
        F: FnMut(NtfyMessage) -> Fut,
        Fut: Future<Output = ()>,
    {
        let topic = Self::topic_path(topics)?;
        let (segment, accept) = match transport {
            Transport::Sse => ("sse", "text/event-stream"),
            _ => ("json", "application/x-ndjson"),
        };
        let url = format!("{}/{}/{}", self.base_url, topic, segment);
        let transport_name = format!("{transport:?}");
        tracing::debug!(
            "{}",
            crate::i18n::tr_args(
                "订阅 {url} (transport={transport})",
                &[("url", url.as_str()), ("transport", transport_name.as_str())]
            )
        );

        let rb = self
            .http
            .get(&url)
            .header(reqwest::header::ACCEPT, accept)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .version(reqwest::Version::HTTP_11);
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            result = tokio::time::timeout(
                Duration::from_secs(15),
                self.authorize(rb).send(),
            ) => match result {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => {
                    let message = crate::i18n::tr_args(
                        "连接 {url} 超时（15 秒内未收到响应头）",
                        &[("url", url.as_str())],
                    );
                    bail!("{}", message)
                }
            },
        };

        let status = resp.status();
        let status_str = status.to_string();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let message = crate::i18n::tr_args(
                "ntfy 返回错误状态 {status}: {body}",
                &[("status", status_str.as_str()), ("body", body.trim())],
            );
            bail!("{}", message);
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string();
        if content_type.contains("text/html") {
            let message = crate::i18n::tr_args(
                "{url} 返回了 HTML 页面 ({status})；base_url 可能不正确（例如缺少子路径），或反向代理未把 API 请求转发到 ntfy",
                &[("url", url.as_str()), ("status", status_str.as_str())],
            );
            bail!("{}", message);
        }
        tracing::debug!(
            "{}",
            crate::i18n::tr_args(
                "ntfy 响应 {status} (content-type: {content_type})",
                &[("status", status_str.as_str()), ("content_type", content_type.as_str())]
            )
        );

        on_open();

        let mut stream = Box::pin(resp.bytes_stream());
        let mut buffer: Vec<u8> = Vec::with_capacity(8192);
        let mut warned_payload = false;

        loop {
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                chunk = stream.next() => {
                    match chunk {
                        Some(Ok(bytes)) => {
                            buffer.extend_from_slice(&bytes);
                            while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                                let mut line: Vec<u8> = buffer.drain(..=pos).collect();
                                line.pop();
                                if line.last() == Some(&b'\r') {
                                    line.pop();
                                }
                                let text = String::from_utf8_lossy(&line);
                                let text = text.trim();
                                if text.is_empty() || text.starts_with(':') {
                                    continue;
                                }
                                if text.starts_with("event:")
                                    || text.starts_with("id:")
                                    || text.starts_with("retry:")
                                {
                                    continue;
                                }
                                // A stream should never contain HTML.
                                if text.starts_with('<') {
                                    let message = crate::i18n::tr_args(
                                        "{url} 返回了 HTML 内容；请检查 base_url 是否正确",
                                        &[("url", url.as_str())],
                                    );
                                    bail!("{}", message);
                                }
                                let payload = text
                                    .strip_prefix("data:")
                                    .map(str::trim)
                                    .unwrap_or(text);
                                if payload.is_empty() {
                                    continue;
                                }
                                match serde_json::from_str::<NtfyMessage>(payload) {
                                    Ok(msg) => handler(msg).await,
                                    Err(e) => {
                                        let err = e.to_string();
                                        let raw = truncate(text, 200);
                                        let message = crate::i18n::tr_args(
                                            "无法解析 ntfy 消息: {err} ({raw})",
                                            &[("err", err.as_str()), ("raw", raw.as_str())],
                                        );
                                        if warned_payload {
                                            tracing::debug!("{}", message);
                                        } else {
                                            warned_payload = true;
                                            tracing::warn!("{}", message);
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => return Err(e.into()),
                        None => return Ok(()),
                    }
                }
            }
        }
    }

    /// Subscribe via WebSocket (`/{topic}/ws`).
    pub async fn run_ws<O, F, Fut>(
        &self,
        topics: &[String],
        cancel: &CancellationToken,
        on_open: O,
        mut handler: F,
    ) -> Result<()>
    where
        O: FnOnce(),
        F: FnMut(NtfyMessage) -> Fut,
        Fut: Future<Output = ()>,
    {
        let topic = Self::topic_path(topics)?;
        let url = self.ws_url(&topic)?;
        tracing::debug!(
            "{}",
            crate::i18n::tr_args("WebSocket 订阅 {url}", &[("url", url.as_str())])
        );

        let mut request = url.as_str().into_client_request()?;
        match self.auth.method {
            AuthMethod::None => {}
            AuthMethod::Basic => {
                let token = base64::engine::general_purpose::STANDARD
                    .encode(format!("{}:{}", self.auth.username, self.auth.password));
                let value = HeaderValue::from_str(&format!("Basic {token}"))?;
                request.headers_mut().insert(AUTHORIZATION, value);
            }
            AuthMethod::Token => {
                let value = HeaderValue::from_str(&format!("Bearer {}", self.auth.token))?;
                request.headers_mut().insert(AUTHORIZATION, value);
            }
        }

        let (mut ws, _response) = tokio::select! {
            _ = cancel.cancelled() => return Ok(()),
            result = tokio::time::timeout(
                Duration::from_secs(15),
                tokio_tungstenite::connect_async(request),
            ) => match result {
                Ok(Ok(pair)) => pair,
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => {
                    let message = crate::i18n::tr_args(
                        "WebSocket 连接 {url} 超时（15 秒）",
                        &[("url", url.as_str())],
                    );
                    bail!("{}", message)
                }
            },
        };
        on_open();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                message = ws.next() => {
                    match message {
                        Some(Ok(Message::Text(text))) => {
                            if let Ok(msg) = serde_json::from_str::<NtfyMessage>(text.as_str()) {
                                handler(msg).await;
                            }
                        }
                        Some(Ok(Message::Binary(bytes))) => {
                            if let Ok(msg) = serde_json::from_slice::<NtfyMessage>(&bytes) {
                                handler(msg).await;
                            }
                        }
                        Some(Ok(Message::Close(_))) => return Ok(()),
                        Some(Ok(_)) => {}
                        Some(Err(e)) => return Err(e.into()),
                        None => return Ok(()),
                    }
                }
            }
        }
    }

    /// Poll for messages newer than `since` (unix seconds). Returns the newest
    /// timestamp seen.
    pub async fn poll<F, Fut>(
        &self,
        topics: &[String],
        since: i64,
        cancel: &CancellationToken,
        mut handler: F,
    ) -> Result<i64>
    where
        F: FnMut(NtfyMessage) -> Fut,
        Fut: Future<Output = ()>,
    {
        let topic = Self::topic_path(topics)?;
        let since_str = since.to_string();
        let url = format!("{}/{}/json", self.base_url, topic);
        let rb = self
            .http
            .get(&url)
            .query(&[("poll", "1"), ("since", since_str.as_str())])
            .header(reqwest::header::ACCEPT, "application/x-ndjson")
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .header(reqwest::header::CACHE_CONTROL, "no-cache")
            .version(reqwest::Version::HTTP_11);
        let fetch = async {
            let resp = self.authorize(rb).send().await?;
            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                let status = status.to_string();
                let message = crate::i18n::tr_args(
                    "ntfy 返回错误状态 {status}: {body}",
                    &[("status", status.as_str()), ("body", body.trim())],
                );
                bail!("{}", message);
            }
            let body = resp.text().await?;
            Ok::<String, anyhow::Error>(body)
        };
        let body = match tokio::time::timeout(Duration::from_secs(30), fetch).await {
            Ok(Ok(body)) => body,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                let message = crate::i18n::tr_args(
                    "轮询请求 {url} 超时（30 秒）",
                    &[("url", url.as_str())],
                );
                bail!("{}", message)
            }
        };
        if body.trim_start().starts_with('<') {
            let message = crate::i18n::tr_args(
                "{url} 返回了 HTML 页面；base_url 可能不正确（例如缺少子路径），或反向代理未把 API 请求转发到 ntfy",
                &[("url", url.as_str())],
            );
            bail!("{}", message);
        }

        let mut newest = since;
        for line in body.lines() {
            if cancel.is_cancelled() {
                break;
            }
            let line = line.trim();
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            let payload = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
            if payload.is_empty() {
                continue;
            }
            if let Ok(msg) = serde_json::from_str::<NtfyMessage>(payload) {
                if msg.time > newest {
                    newest = msg.time;
                }
                handler(msg).await;
            }
        }
        Ok(newest)
    }

    fn ws_url(&self, topic: &str) -> Result<String> {
        let base = &self.base_url;
        let rest = if let Some(rest) = base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else if let Some(rest) = base.strip_prefix("wss://") {
            format!("wss://{rest}")
        } else if let Some(rest) = base.strip_prefix("ws://") {
            format!("ws://{rest}")
        } else {
            let message =
                crate::i18n::tr_args("无法识别服务器地址协议: {base}", &[("base", base.as_str())]);
            bail!("{}", message);
        };
        Ok(format!("{rest}/{topic}/ws"))
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}
