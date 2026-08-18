use async_trait::async_trait;
use std::sync::Mutex;
use std::time::Duration;

use crate::error::{Error, Result};

/// A minimal HTTP response abstraction that does not depend on reqwest,
/// so tests can build canned responses without touching the network.
#[derive(Debug, Clone, Default)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn content_length(&self) -> Option<u64> {
        self.header("content-length")?.parse().ok()
    }

    pub fn text(&self) -> Result<String> {
        String::from_utf8(self.body.clone())
            .map_err(|e| Error::Parse(format!("invalid utf-8 body: {e}")))
    }

    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_slice(&self.body)
            .map_err(|e| Error::Parse(format!("invalid json body: {e}")))
    }

    pub fn ok(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }
}

/// Testable HTTP client abstraction. Real network goes through `ReqwestClient`,
/// tests inject `MockHttp`.
#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn get(&self, url: &str) -> Result<HttpResponse>;

    async fn get_with_headers(&self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse>;

    async fn head(&self, url: &str) -> Result<HttpResponse>;
}

/// Real reqwest-backed client.
pub struct ReqwestClient {
    inner: reqwest::Client,
}

impl ReqwestClient {
    pub fn new() -> Result<Self> {
        let inner = reqwest::Client::builder()
            .user_agent("VentoyPilot/0.1 (+https://github.com/Jvcon/VentoyPilot)")
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|e| Error::Http(e.to_string()))?;
        Ok(Self { inner })
    }
}

async fn to_response(resp: reqwest::Response) -> Result<HttpResponse> {
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                v.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let bytes = resp.bytes().await.map_err(|e| Error::Http(e.to_string()))?;
    Ok(HttpResponse {
        status,
        headers,
        body: bytes.to_vec(),
    })
}

#[async_trait]
impl HttpClient for ReqwestClient {
    async fn get(&self, url: &str) -> Result<HttpResponse> {
        self.get_with_headers(url, &[]).await
    }

    async fn get_with_headers(&self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse> {
        let mut req = self.inner.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::Http(format!("GET {url}: {e}")))?;
        to_response(resp).await
    }

    async fn head(&self, url: &str) -> Result<HttpResponse> {
        let resp = self
            .inner
            .head(url)
            .send()
            .await
            .map_err(|e| Error::Http(format!("HEAD {url}: {e}")))?;
        to_response(resp).await
    }
}

/// In-memory mock client for tests and offline debugging.
/// Routes are matched by URL prefix; the first matching route wins.
/// Requests are logged and can be asserted on.
#[derive(Default)]
#[allow(dead_code)] // used from integration tests
pub struct MockHttp {
    routes: Vec<(String, HttpResponse)>,
    requests: Mutex<Vec<String>>,
}

impl MockHttp {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a response for every URL starting with `prefix`.
    pub fn route(&mut self, prefix: &str, response: HttpResponse) {
        self.routes.push((prefix.to_string(), response));
    }

    /// Convenience: register a 200 JSON response.
    pub fn route_json(&mut self, prefix: &str, value: &serde_json::Value) {
        self.route(
            prefix,
            HttpResponse::ok(200, serde_json::to_vec(value).unwrap()),
        );
    }

    /// Convenience: register a 200 plain-text response.
    pub fn route_text(&mut self, prefix: &str, text: &str) {
        self.route(prefix, HttpResponse::ok(200, text.as_bytes().to_vec()));
    }

    /// URLs requested so far (in order).
    pub fn requested(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn dispatch(&self, url: &str) -> Result<HttpResponse> {
        self.requests.lock().unwrap().push(url.to_string());
        for (prefix, response) in &self.routes {
            if url.starts_with(prefix) {
                return Ok(response.clone());
            }
        }
        Err(Error::HttpStatus {
            status: 404,
            url: url.to_string(),
            body: "no mock route registered".to_string(),
        })
    }
}

#[async_trait]
impl HttpClient for MockHttp {
    async fn get(&self, url: &str) -> Result<HttpResponse> {
        self.dispatch(url)
    }

    async fn get_with_headers(&self, url: &str, _headers: &[(&str, &str)]) -> Result<HttpResponse> {
        self.dispatch(url)
    }

    async fn head(&self, url: &str) -> Result<HttpResponse> {
        self.dispatch(url)
    }
}
