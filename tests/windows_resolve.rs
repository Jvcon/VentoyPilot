//! Integration test: full Windows resolve flow with a mocked Microsoft
//! connector. No real network, no file downloads.
//!
//! Mirrors Fido's session handshake: tags whitelist -> ov-df (w/rticks) ->
//! ov-df reply -> getskuinformationbyproductedition -> GetProductDownloadLinksBySku.

use std::path::PathBuf;

use ventoypilot::catalog::{Catalog, Kind};
use ventoypilot::config::Config;
use ventoypilot::http::MockHttp;
use ventoypilot::source::resolver::{DownloadRequest, LinkResolver, ResolverContext};
use ventoypilot::source::windows::WindowsResolver;

fn mock_connector() -> MockHttp {
    let mut http = MockHttp::new();
    // Session whitelist + ov-df handshake.
    http.route_text("https://vlscppe.microsoft.com/tags", "OK");
    http.route_text(
        "https://ov-df.microsoft.com/mdt.js",
        "x = \"https://ov-df.microsoft.com/?w=ABCDEF&rticks=\\\"+9876543210\\\"\"",
    );
    http.route_text("https://ov-df.microsoft.com/?session_id=", "OK");

    // Languages for productEditionId 3321.
    http.route_json(
        "https://www.microsoft.com/software-download-connector/api/getskuinformationbyproductedition",
        &serde_json::json!({
            "Skus": [
                { "Id": "1449", "Language": "zh-cn", "LocalizedLanguage": "Chinese (Simplified)" },
                { "Id": "1450", "Language": "en-us", "LocalizedLanguage": "English (United States)" }
            ],
            "Errors": []
        }),
    );

    // Download links per SKU.
    http.route_json(
        "https://www.microsoft.com/software-download-connector/api/GetProductDownloadLinksBySku",
        &serde_json::json!({
            "ProductDownloadOptions": [
                { "DownloadType": 0, "Uri": "https://dl.example/x86/Win11_x86.iso?t=1" },
                { "DownloadType": 1, "Uri": "https://dl.example/x64/Win11_25H2_Chinese_Simplified_x64.iso?t=1&e=2" },
                { "DownloadType": 2, "Uri": "https://dl.example/arm64/Win11_25H2_Chinese_Simplified_arm64.iso" }
            ],
            "Errors": []
        }),
    );
    http
}

fn ctx<'a>(cfg: &'a Config, _catalog: &'a Catalog) -> ResolverContext<'a> {
    ResolverContext {
        config: cfg,
        kind: Kind::Windows,
    }
}

fn req(cfg: &Config) -> DownloadRequest {
    DownloadRequest {
        system: "win11".into(),
        release: Some("25H2".into()),
        edition: Some("desktop".into()),
        lang: Some("zh-CN".into()),
        arch: Some("x64".into()),
        target_dir: cfg.download_dir(None),
    }
}

#[tokio::test]
async fn windows_full_flow_resolves_link_path_and_name() {
    let catalog = Catalog::load().unwrap();
    let cfg = Config {
        download_dir: PathBuf::from("/tmp/vpilot-test-dl"),
        ..Config::default()
    };
    let http = mock_connector();
    let c = ctx(&cfg, &catalog);
    let resolver = WindowsResolver::new(&c, &catalog);

    let plan = resolver.resolve(&req(&cfg), &http).await.unwrap();

    // The debug targets: download link, target path, target file name, arch.
    assert_eq!(
        plan.url,
        "https://dl.example/x64/Win11_25H2_Chinese_Simplified_x64.iso?t=1&e=2"
    );
    assert_eq!(plan.file_name, "Win11_25H2_Chinese_Simplified_x64.iso");
    assert_eq!(
        plan.target_path,
        PathBuf::from("/tmp/vpilot-test-dl/Win11_25H2_Chinese_Simplified_x64.iso")
    );
    assert_eq!(plan.arch, "x64");
    assert!(plan.downloadable);
    assert!(plan.source.contains("Windows 11"));
    assert!(plan.source.contains("25H2"));
    assert!(plan.source.contains("Chinese (Simplified)"));

    // Handshake requests actually went out in order.
    let requested = http.requested();
    assert!(requested
        .iter()
        .any(|u| u.starts_with("https://vlscppe.microsoft.com/tags")));
    assert!(requested
        .iter()
        .any(|u| u.starts_with("https://ov-df.microsoft.com/mdt.js")));
    assert!(requested
        .iter()
        .any(|u| u.starts_with("https://ov-df.microsoft.com/?session_id=")));
    let sku_call = requested
        .iter()
        .find(|u| u.contains("getskuinformationbyproductedition"))
        .expect("sku call made");
    assert!(sku_call.contains("productEditionId=3321"));
    let links_call = requested
        .iter()
        .find(|u| u.contains("GetProductDownloadLinksBySku"))
        .expect("links call made");
    assert!(links_call.contains("SKU=1449"));
}

#[tokio::test]
async fn windows_arm64_selected_by_arch() {
    let catalog = Catalog::load().unwrap();
    let cfg = Config::default();
    let http = mock_connector();
    let c = ctx(&cfg, &catalog);
    let resolver = WindowsResolver::new(&c, &catalog);

    let mut r = req(&cfg);
    r.arch = Some("arm64".into());
    let plan = resolver.resolve(&r, &http).await.unwrap();

    assert!(plan
        .url
        .ends_with("Win11_25H2_Chinese_Simplified_arm64.iso"));
    assert_eq!(plan.arch, "arm64");
}

#[tokio::test]
async fn windows_language_fallback_english() {
    let catalog = Catalog::load().unwrap();
    let cfg = Config::default();
    let http = mock_connector();
    let c = ctx(&cfg, &catalog);
    let resolver = WindowsResolver::new(&c, &catalog);

    let mut r = req(&cfg);
    r.lang = Some("en-US".into());
    let plan = resolver.resolve(&r, &http).await.unwrap();

    assert!(plan.source.contains("English (United States)"));
    assert_eq!(plan.arch, "x64");
}

#[tokio::test]
async fn windows_edition_china() {
    let catalog = Catalog::load().unwrap();
    let cfg = Config::default();
    let http = mock_connector();
    let c = ctx(&cfg, &catalog);
    let resolver = WindowsResolver::new(&c, &catalog);

    let mut r = req(&cfg);
    r.edition = Some("home-china".into());
    let plan = resolver.resolve(&r, &http).await.unwrap();
    assert!(plan.source.contains("Home China"));
}

#[tokio::test]
async fn windows_unknown_system_errors() {
    let catalog = Catalog::load().unwrap();
    let cfg = Config::default();
    let http = mock_connector();
    let c = ctx(&cfg, &catalog);
    let resolver = WindowsResolver::new(&c, &catalog);

    let mut r = req(&cfg);
    r.system = "macos".into();
    let err = resolver.resolve(&r, &http).await.unwrap_err();
    assert!(err.to_string().contains("unknown Windows version"));
}

#[tokio::test]
async fn windows_link_availability_probe() {
    let catalog = Catalog::load().unwrap();
    let cfg = Config::default();
    let mut http = mock_connector();
    // HEAD probe for availability + size.
    let mut resp = ventoypilot::http::HttpResponse::ok(200, b"".to_vec());
    resp.headers
        .push(("Content-Length".into(), "5864800256".into()));
    resp.headers
        .push(("Content-Type".into(), "application/octet-stream".into()));
    http.route("https://dl.example/x64/", resp);

    let c = ctx(&cfg, &catalog);
    let resolver = WindowsResolver::new(&c, &catalog);
    let plan = resolver.resolve(&req(&cfg), &http).await.unwrap();

    let check = ventoypilot::source::resolver::check_link(&http, &plan)
        .await
        .unwrap();
    assert!(check.ok);
    assert_eq!(check.status, 200);
    assert_eq!(check.size_hint, Some(5_864_800_256));
    assert_eq!(
        http.requested()
            .iter()
            .filter(|u| u.starts_with("https://dl.example/x64/"))
            .count(),
        1
    );
}
