use async_trait::async_trait;
use serde::Deserialize;

use super::resolver::{DownloadPlan, DownloadRequest, LinkResolver, ResolverContext};
use crate::catalog::{Catalog, Kind, WindowsEdition, WindowsRelease, WindowsVersion};
use crate::error::{Error, Result};
use crate::http::HttpClient;

const BASE: &str = "https://www.microsoft.com/software-download-connector/api";
const TAGS_URL: &str = "https://vlscppe.microsoft.com/tags";
const OVDF_MDT: &str = "https://ov-df.microsoft.com/mdt.js";
const OVDF_REPLY: &str = "https://ov-df.microsoft.com/";
const REFERER: &str = "https://www.microsoft.com/software-download/windows11";

/// Common shorthand aliases: "win11" -> Windows 11, "win10" -> Windows 10.
const WIN_ALIASES: &[(&str, &str)] = &[("win11", "windows11"), ("win10", "windows10")];

/// Fido-style Microsoft software-download connector resolver.
pub struct WindowsResolver<'a> {
    ctx: &'a ResolverContext<'a>,
    catalog: &'a Catalog,
}

impl<'a> WindowsResolver<'a> {
    pub fn new(ctx: &'a ResolverContext<'a>, catalog: &'a Catalog) -> Self {
        Self { ctx, catalog }
    }

    fn find_version(&self, name: &str) -> Result<&WindowsVersion> {
        let q = name.to_lowercase();
        let norm = |s: &str| s.to_lowercase().replace([' ', '_', '-', '(', ')', '.'], "");
        // exact slug / name match, then normalized containment, then aliases
        self.catalog
            .windows
            .iter()
            .find(|v| {
                v.slug.to_lowercase() == q
                    || v.name.to_lowercase() == q
                    || norm(&v.name).contains(&norm(name))
                    || WIN_ALIASES
                        .iter()
                        .any(|(k, slug)| *k == q && v.slug == *slug)
            })
            .ok_or_else(|| Error::NotFound(format!("unknown Windows version: {name}")))
    }

    fn find_release<'v>(
        &self,
        version: &'v WindowsVersion,
        release: Option<&str>,
    ) -> Result<&'v WindowsRelease> {
        let rel = release.unwrap_or(&self.ctx.config.default_release);
        if rel.is_empty() || rel == "latest" {
            return version
                .releases
                .first()
                .ok_or_else(|| Error::NotFound(format!("no releases for {}", version.name)));
        }
        let q = rel.to_lowercase();
        version
            .releases
            .iter()
            .find(|r| r.release.to_lowercase().contains(&q))
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "release '{rel}' not found for {} (use `vpilot search --kind windows` to list)",
                    version.name
                ))
            })
    }

    fn find_edition<'r>(
        &self,
        release: &'r WindowsRelease,
        edition: Option<&str>,
    ) -> Result<&'r WindowsEdition> {
        let ed = match edition {
            Some(e) if !e.is_empty() => e,
            _ => match &self.ctx.config.default_edition {
                Some(e) => e.as_str(),
                None => {
                    return release.editions.first().ok_or_else(|| {
                        Error::NotFound(format!("no editions for {}", release.release))
                    })
                }
            },
        };
        let q = ed.to_lowercase();
        release
            .editions
            .iter()
            .find(|e| {
                e.name.to_lowercase().contains(&q) || e.types.iter().any(|t| t.to_lowercase() == q)
            })
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "edition '{ed}' not found in {} (choices: {})",
                    release.release,
                    release
                        .editions
                        .iter()
                        .map(|e| e.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
    }
}

#[async_trait]
impl<'a> LinkResolver for WindowsResolver<'a> {
    fn kind(&self) -> Kind {
        Kind::Windows
    }

    fn find(&self, name: &str) -> Result<()> {
        self.find_version(name).map(|_| ())
    }

    async fn resolve(&self, req: &DownloadRequest, http: &dyn HttpClient) -> Result<DownloadPlan> {
        let version = self.find_version(&req.system)?;
        let release = self.find_release(version, req.release.as_deref())?;
        let edition = self.find_edition(release, req.edition.as_deref())?;
        let ms = &self.ctx.config.ms;

        // One session per productEditionId (Fido does the same).
        let mut skus: Vec<Sku> = Vec::new();
        let mut sessions: Vec<(u64, String)> = Vec::new();
        for peid in &edition.product_edition_id {
            let session_id = uuid::Uuid::new_v4().to_string();
            handshake(http, ms, &session_id).await?;
            let list = fetch_skus(http, ms, *peid, &session_id).await?;
            skus.extend(list);
            sessions.push((*peid, session_id));
        }
        if skus.is_empty() {
            return Err(Error::NotFound(format!(
                "no languages returned for {} / {} / {}",
                version.name, release.release, edition.name
            )));
        }

        // Pick language: exact display-name / locale-prefix / substring match.
        let lang = req
            .lang
            .as_deref()
            .filter(|l| !l.is_empty())
            .or(Some(self.ctx.config.default_lang.as_str()));
        let chosen = pick_language(&skus, lang)?;

        // Fetch download links for every SKU of the chosen language.
        let mut options: Vec<DownloadOption> = Vec::new();
        for sku in skus.iter().filter(|s| s.language == chosen.language) {
            let sid = sessions
                .iter()
                .find(|(peid, _)| *peid == sku.product_edition_id)
                .map(|(_, sid)| sid.clone())
                .ok_or_else(|| Error::Other("internal: session not found".into()))?;
            let mut opts = fetch_links(http, ms, &sku.id, &sid).await?;
            options.append(&mut opts);
        }
        if options.is_empty() {
            return Err(Error::NotFound(
                "no download links returned by Microsoft connector".into(),
            ));
        }

        // Filter by architecture (DownloadType: 0=x86, 1=x64, 2=arm64).
        let arch = req.arch.as_deref().unwrap_or(&self.ctx.config.default_arch);
        let arch_lower = arch.to_lowercase();
        let wanted: Option<u8> = match arch_lower.as_str() {
            "x64" | "amd64" | "x86_64" => Some(1),
            "arm64" | "aarch64" => Some(2),
            "x86" | "i386" | "i686" | "32" => Some(0),
            _ => None,
        };
        let pick = match wanted {
            Some(w) => options
                .iter()
                .find(|o| o.download_type == w)
                .or_else(|| options.first()),
            None => options.first(),
        }
        .ok_or_else(|| Error::NotFound("no matching download option".into()))?;

        let fallback_name = format!(
            "{}_{}_{}_{}.iso",
            version.slug,
            release.release.replace(' ', "_"),
            chosen.language,
            arch_lower
        );
        let file_name = super::resolver::file_name_from_url(&pick.uri, &fallback_name);
        let lang_display = if chosen.localized_language.is_empty() {
            chosen.language.as_str()
        } else {
            chosen.localized_language.as_str()
        };
        let source = format!(
            "{} / {} / {} / {} / {}",
            version.name, release.release, edition.name, lang_display, arch_lower
        );

        Ok(DownloadPlan::new(
            Kind::Windows,
            pick.uri.clone(),
            file_name,
            &req.target_dir,
            &arch_lower,
            &source,
        ))
    }
}

// ---------------------------------------------------------------------------
// Microsoft session handshake (mirrors Fido)
// ---------------------------------------------------------------------------

async fn handshake(
    http: &dyn HttpClient,
    ms: &crate::config::MsConfig,
    session_id: &str,
) -> Result<()> {
    // 1) Whitelist the session id.
    let tags_url = format!("{TAGS_URL}?org_id={}&session_id={session_id}", ms.org_id);
    let r = http.get(&tags_url).await?;
    if !(200..400).contains(&r.status) {
        return Err(Error::HttpStatus {
            status: r.status,
            url: tags_url,
            body: "vlscppe tags whitelist failed".into(),
        });
    }

    // 2) ov-df: get w + rticks.
    let mdt_url = format!(
        "{OVDF_MDT}?instanceId={}&PageId=si&session_id={session_id}",
        ms.instance_id
    );
    let body = http.get(&mdt_url).await?.text()?;
    let w = regex_match(&body, "[?&]w=([A-F0-9]+)")
        .ok_or_else(|| Error::Parse("ov-df: cannot extract w".into()))?;
    let rticks = regex_match(&body, "rticks=\\\\?\"\\+?(\\d+)")
        .ok_or_else(|| Error::Parse("ov-df: cannot extract rticks".into()))?;

    // 3) ov-df: reply with the tokens.
    let mdt = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| Error::Other(e.to_string()))?
        .as_millis();
    let reply = format!(
        "{OVDF_REPLY}?session_id={session_id}&CustomerId={}&PageId=si&w={w}&mdt={mdt}&rticks={rticks}",
        ms.instance_id
    );
    let r = http.get(&reply).await?;
    if !(200..400).contains(&r.status) {
        return Err(Error::HttpStatus {
            status: r.status,
            url: reply,
            body: "ov-df reply failed".into(),
        });
    }
    Ok(())
}

fn regex_match(text: &str, pattern: &str) -> Option<String> {
    let re = regex::Regex::new(pattern).ok()?;
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

#[derive(Debug, Clone)]
struct Sku {
    id: String,
    language: String,
    localized_language: String,
    product_edition_id: u64,
}

#[derive(Deserialize)]
struct SkuResponse {
    #[serde(rename = "Skus", default)]
    skus: Vec<SkuRaw>,
    #[serde(default)]
    errors: Vec<ApiError>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SkuRaw {
    id: Option<String>,
    language: Option<String>,
    localized_language: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ApiError {
    #[serde(rename = "type", default)]
    err_type: Option<u8>,
    #[serde(default)]
    value: Option<String>,
}

async fn fetch_skus(
    http: &dyn HttpClient,
    ms: &crate::config::MsConfig,
    product_edition_id: u64,
    session_id: &str,
) -> Result<Vec<Sku>> {
    let url = format!(
        "{BASE}/getskuinformationbyproductedition?profile={}&productEditionId={product_edition_id}&SKU=undefined&friendlyFileName=undefined&Locale=en-US&sessionID={session_id}",
        ms.profile_id
    );
    let resp = http.get(&url).await?;
    if !(200..400).contains(&resp.status) {
        return Err(Error::HttpStatus {
            status: resp.status,
            url,
            body: resp.text().unwrap_or_default(),
        });
    }
    let data: SkuResponse = resp.json()?;
    if let Some(err) = data.errors.first() {
        if err.err_type == Some(9) {
            return Err(Error::Other(
                "Microsoft has blocked this IP/region (error 715-123130). Try again later or use a different network.".into(),
            ));
        }
        return Err(Error::Other(format!(
            "Microsoft connector error: {}",
            err.value.clone().unwrap_or_default()
        )));
    }
    Ok(data
        .skus
        .into_iter()
        .filter_map(|s| {
            Some(Sku {
                id: s.id?,
                language: s.language?,
                localized_language: s.localized_language.unwrap_or_default(),
                product_edition_id,
            })
        })
        .collect())
}

#[derive(Debug, Clone)]
struct DownloadOption {
    download_type: u8,
    uri: String,
}

#[derive(Deserialize)]
struct LinksResponse {
    #[serde(rename = "ProductDownloadOptions", default)]
    product_download_options: Vec<LinkRaw>,
    #[serde(default)]
    errors: Vec<ApiError>,
}

#[derive(Deserialize)]
struct LinkRaw {
    #[serde(rename = "DownloadType", default)]
    download_type: Option<u8>,
    #[serde(rename = "Uri")]
    uri: Option<String>,
}

async fn fetch_links(
    http: &dyn HttpClient,
    ms: &crate::config::MsConfig,
    sku: &str,
    session_id: &str,
) -> Result<Vec<DownloadOption>> {
    let url = format!(
        "{BASE}/GetProductDownloadLinksBySku?profile={}&productEditionId=undefined&SKU={sku}&friendlyFileName=undefined&Locale=en-US&sessionID={session_id}",
        ms.profile_id
    );
    let resp = http.get_with_headers(&url, &[("Referer", REFERER)]).await?;
    if !(200..400).contains(&resp.status) {
        return Err(Error::HttpStatus {
            status: resp.status,
            url,
            body: resp.text().unwrap_or_default(),
        });
    }
    let data: LinksResponse = resp.json()?;
    if let Some(err) = data.errors.first() {
        if err.err_type == Some(9) {
            return Err(Error::Other(
                "Microsoft has blocked this IP/region (error 715-123130). Try again later or use a different network.".into(),
            ));
        }
        return Err(Error::Other(format!(
            "Microsoft connector error: {}",
            err.value.clone().unwrap_or_default()
        )));
    }
    Ok(data
        .product_download_options
        .into_iter()
        .filter_map(|l| {
            Some(DownloadOption {
                download_type: l.download_type.unwrap_or(1),
                uri: l.uri?,
            })
        })
        .collect())
}

/// Match the requested language against SKU language names.
/// Accepts display names ("Chinese (Simplified)"), codes ("zh-CN", "zh-cn")
/// and substrings ("Chin", "zh"). Falls back to the first SKU.
fn pick_language<'s>(skus: &'s [Sku], wanted: Option<&str>) -> Result<&'s Sku> {
    let Some(wanted) = wanted else {
        return skus
            .first()
            .ok_or_else(|| Error::NotFound("no languages".into()));
    };
    let w = wanted.trim().to_lowercase();

    // 1) exact match on code or display name
    for s in skus {
        if s.language.to_lowercase() == w || s.localized_language.to_lowercase() == w {
            return Ok(s);
        }
    }
    // 2) locale code prefix, e.g. zh-CN -> zh matches zh-cn
    if w.contains('-') {
        let primary = w.split('-').next().unwrap_or(&w);
        for s in skus {
            let lang = s.language.to_lowercase();
            if lang == primary || lang.starts_with(&format!("{primary}-")) {
                return Ok(s);
            }
        }
    }
    // 3) common locale -> display-name hints
    if let Some(hint) = LANG_HINTS.iter().find(|(k, _)| w.starts_with(k)) {
        for s in skus {
            let combined = format!("{} {}", s.language, s.localized_language).to_lowercase();
            if combined.contains(hint.1) {
                return Ok(s);
            }
        }
    }
    // 4) loose substring
    for s in skus {
        let combined = format!("{} {}", s.language, s.localized_language).to_lowercase();
        if combined.contains(&w) {
            return Ok(s);
        }
    }
    let first = skus
        .first()
        .ok_or_else(|| Error::NotFound("no languages".into()))?;
    Ok(first)
}

/// locale-prefix -> display-name fragment hints
const LANG_HINTS: &[(&str, &str)] = &[
    ("zh", "chinese"),
    ("en", "english"),
    ("ja", "japanese"),
    ("ko", "korean"),
    ("fr", "french"),
    ("de", "german"),
    ("es", "spanish"),
    ("it", "italian"),
    ("pt", "portuguese"),
    ("ru", "russian"),
    ("nl", "dutch"),
    ("sv", "swedish"),
    ("pl", "polish"),
    ("tr", "turkish"),
    ("ar", "arabic"),
    ("he", "hebrew"),
    ("th", "thai"),
    ("vi", "vietnamese"),
    ("id", "indonesian"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_exact_code() {
        let skus = vec![
            sku("Chinese (Simplified)", "简体中文"),
            sku("English (United States)", "English"),
        ];
        assert_eq!(
            pick_language(&skus, Some("zh-CN")).unwrap().language,
            "Chinese (Simplified)"
        );
        assert_eq!(
            pick_language(&skus, Some("english")).unwrap().language,
            "English (United States)"
        );
    }

    #[test]
    fn lang_substring_fallback() {
        let skus = vec![sku("Arabic", "العربية"), sku("Japanese", "日本語")];
        assert_eq!(
            pick_language(&skus, Some("jap")).unwrap().language,
            "Japanese"
        );
        assert_eq!(pick_language(&skus, None).unwrap().language, "Arabic");
    }

    fn sku(lang: &str, loc: &str) -> Sku {
        Sku {
            id: "1".into(),
            language: lang.into(),
            localized_language: loc.into(),
            product_edition_id: 3321,
        }
    }

    #[test]
    fn regex_extract() {
        // Real-world mdt.js carries the escaped quote: rticks=\"+12345678\"
        let real = "x = \"https://ov-df.microsoft.com/?w=ABCDEF&rticks=\\\"+12345678\\\"\"";
        assert_eq!(
            regex_match(real, "rticks=\\\\?\"\\+?(\\d+)").as_deref(),
            Some("12345678")
        );
        // Plain form (no backslash) also works.
        let body2 = "https://ov-df.microsoft.com/?w=ABCDEF&rticks=\"+12345678\"";
        assert_eq!(
            regex_match(body2, "rticks=\\\\?\"\\+?(\\d+)").as_deref(),
            Some("12345678")
        );
    }
}
