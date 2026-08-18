use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::catalog::Kind;
use crate::config::Config;
use crate::error::Result;
use crate::http::{HttpClient, HttpResponse};

/// What the user asked to download.
#[derive(Debug, Clone, Default)]
pub struct DownloadRequest {
    /// System name as typed, e.g. "win11", "ubuntu".
    pub system: String,
    /// Release (Fido -Rel), e.g. "25H2", or "latest".
    pub release: Option<String>,
    /// Edition (Fido -Ed), e.g. "Pro", "desktop".
    pub edition: Option<String>,
    /// Language (Fido -Lang), e.g. "zh-CN".
    pub lang: Option<String>,
    /// Architecture (Fido -Arch): x64 | arm64 | x86.
    pub arch: Option<String>,
    /// Effective download directory (already resolved from config/CLI).
    pub target_dir: PathBuf,
}

/// The result of link resolution: everything the debug/dry-run mode prints.
#[derive(Debug, Clone)]
pub struct DownloadPlan {
    pub kind: Kind,
    /// The concrete download URL (or official page for release-page systems).
    pub url: String,
    /// Filename to save as, e.g. "Win11_25H2_x64.iso".
    pub file_name: String,
    /// Full destination path = target_dir / file_name.
    pub target_path: PathBuf,
    pub arch: String,
    /// Size in bytes, when known (filled by availability check).
    pub size_hint: Option<u64>,
    /// SHA-256 hex, when known.
    pub checksum: Option<String>,
    /// Whether a bootable file can actually be downloaded (false for
    /// DE-type systems like KDE Plasma where we only have a page).
    pub downloadable: bool,
    /// Human-readable source description.
    pub source: String,
}

impl DownloadPlan {
    pub fn new(
        kind: Kind,
        url: String,
        file_name: String,
        target_dir: &Path,
        arch: &str,
        source: &str,
    ) -> Self {
        let target_path = target_dir.join(&file_name);
        Self {
            kind,
            url,
            file_name,
            target_path,
            arch: arch.to_string(),
            size_hint: None,
            checksum: None,
            downloadable: true,
            source: source.to_string(),
        }
    }
}

/// Result of the availability probe (HEAD request) used by --dry-run.
#[derive(Debug, Clone)]
pub struct LinkCheck {
    pub url: String,
    pub status: u16,
    pub size_hint: Option<u64>,
    pub content_type: Option<String>,
    pub ok: bool,
}

impl LinkCheck {
    pub fn from_response(url: &str, resp: &HttpResponse) -> Self {
        let ok = (200..400).contains(&resp.status);
        Self {
            url: url.to_string(),
            status: resp.status,
            size_hint: resp.content_length(),
            content_type: resp.header("content-type").map(|s| s.to_string()),
            ok,
        }
    }
}

/// Resolves a concrete download link for a given system, live at download time.
#[async_trait]
pub trait LinkResolver: Send + Sync {
    fn kind(&self) -> Kind;

    /// Find the system in the catalog; error with `NotFound` when unknown.
    fn find(&self, name: &str) -> Result<()>;

    async fn resolve(&self, req: &DownloadRequest, http: &dyn HttpClient) -> Result<DownloadPlan>;
}

/// Probe the resolved URL (HEAD) for availability + size. Used by --dry-run.
pub async fn check_link(http: &dyn HttpClient, plan: &DownloadPlan) -> Result<LinkCheck> {
    let resp = http.head(&plan.url).await?;
    Ok(LinkCheck::from_response(&plan.url, &resp))
}

/// Derive a safe filename from a download URL: the last path segment without
/// query string, falling back to `fallback` when nothing sensible is found.
pub fn file_name_from_url(url: &str, fallback: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return fallback.to_string();
    };
    let mut segments = match parsed.path_segments() {
        Some(s) => s,
        None => return fallback.to_string(),
    };
    let Some(seg) = segments.rfind(|s| !s.is_empty()) else {
        return fallback.to_string();
    };
    // A bare host (trailing slash) is not a filename.
    if seg.trim().is_empty() || !seg.contains('.') {
        return fallback.to_string();
    }
    let decoded = percent_decode(seg);
    if decoded.is_empty() {
        fallback.to_string()
    } else {
        decoded
    }
}

fn percent_decode(s: &str) -> String {
    // Minimal percent-decoding for common cases.
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// The per-source config bundle handed to resolvers.
pub struct ResolverContext<'a> {
    pub config: &'a Config,
    pub kind: Kind,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_from_url_basic() {
        assert_eq!(
            file_name_from_url(
                "https://software.download.prss.microsoft.com/db/Win11_25H2_x64.iso?t=123&e=456",
                "fallback.iso"
            ),
            "Win11_25H2_x64.iso"
        );
        assert_eq!(
            file_name_from_url("https://example.com/foo/Ubuntu%2026.04.iso", "fb.iso"),
            "Ubuntu 26.04.iso"
        );
        assert_eq!(
            file_name_from_url("https://example.com/", "fb.iso"),
            "fb.iso"
        );
    }
}
