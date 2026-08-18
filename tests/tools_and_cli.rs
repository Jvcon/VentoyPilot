//! Integration tests: tools resolver + catalog search + CLI contract.
//! No real network, no file downloads.

use std::path::PathBuf;

use clap::Parser;
use ventoypilot::catalog::{Catalog, Kind};
use ventoypilot::cli::Cli;
use ventoypilot::config::Config;
use ventoypilot::http::MockHttp;
use ventoypilot::source::resolver::{DownloadRequest, LinkResolver, ResolverContext};
use ventoypilot::source::tools::ToolsResolver;

// ---------------------------------------------------------------------------
// Tools resolver
// ---------------------------------------------------------------------------

#[tokio::test]
async fn uefi_shell_latest_builds_github_url() {
    let catalog = Catalog::load().unwrap();
    let cfg = Config {
        download_dir: PathBuf::from("/tmp/vpilot-tools-dl"),
        ..Config::default()
    };
    let ctx = ResolverContext {
        config: &cfg,
        kind: Kind::Tools,
    };
    let http = MockHttp::new();
    let resolver = ToolsResolver::new(&ctx, &catalog);

    let req = DownloadRequest {
        system: "uefi-shell".into(),
        release: None,
        edition: None,
        lang: None,
        arch: None,
        target_dir: cfg.download_dir(None),
    };
    let plan = resolver.resolve(&req, &http).await.unwrap();

    assert_eq!(
        plan.url,
        "https://github.com/pbatard/UEFI-Shell/releases/download/26H1/UEFI-Shell-2.2-26H1-RELEASE.iso"
    );
    assert_eq!(plan.file_name, "UEFI-Shell-2.2-26H1-RELEASE.iso");
    assert_eq!(
        plan.target_path,
        PathBuf::from("/tmp/vpilot-tools-dl/UEFI-Shell-2.2-26H1-RELEASE.iso")
    );
}

#[tokio::test]
async fn uefi_shell_specific_tag() {
    let catalog = Catalog::load().unwrap();
    let cfg = Config::default();
    let ctx = ResolverContext {
        config: &cfg,
        kind: Kind::Tools,
    };
    let http = MockHttp::new();
    let resolver = ToolsResolver::new(&ctx, &catalog);

    let req = DownloadRequest {
        system: "uefi-shell".into(),
        release: Some("24H2".into()),
        edition: None,
        lang: None,
        arch: None,
        target_dir: cfg.download_dir(None),
    };
    let plan = resolver.resolve(&req, &http).await.unwrap();
    assert!(plan
        .url
        .contains("download/24H2/UEFI-Shell-2.2-24H2-RELEASE.iso"));
}

// ---------------------------------------------------------------------------
// Catalog search (offline, deterministic)
// ---------------------------------------------------------------------------

#[test]
fn search_lists_all_kinds() {
    let catalog = Catalog::load().unwrap();
    let all = catalog.search(None, None);
    let kinds: Vec<&str> = all.iter().map(|m| m.kind.as_str()).collect();
    assert!(kinds.contains(&"windows"));
    assert!(kinds.contains(&"linux"));
    assert!(kinds.contains(&"tools"));

    let win11 = all.iter().find(|m| m.slug == "windows11").unwrap();
    assert!(win11.versions.iter().any(|v| v.contains("25H2")));
}

#[test]
fn search_filters_by_kind_and_query() {
    let catalog = Catalog::load().unwrap();
    let linux = catalog.search(Some(Kind::Linux), None);
    assert!(linux.iter().all(|m| m.kind == Kind::Linux));
    assert!(linux.iter().any(|m| m.slug == "nixos"));
    assert!(linux.iter().any(|m| m.slug == "proxmox-ve"));
    assert!(linux.iter().any(|m| m.slug == "kde-plasma"));

    let ubuntu = catalog.search(Some(Kind::Linux), Some("ubuntu"));
    assert_eq!(ubuntu.len(), 1);
    assert_eq!(ubuntu[0].slug, "ubuntu");
}

#[test]
fn catalog_has_no_download_links() {
    // The whole point: the static catalog must not embed concrete ISO links.
    // URL *templates* with `{placeholders}` and release *pages* are allowed.
    let raw = ventoypilot::catalog::EMBEDDED_CATALOG;
    let re = regex::Regex::new(r#""(https?://[^"]*)""#).unwrap();
    for cap in re.captures_iter(raw) {
        let url = &cap[1];
        if url.contains('{') {
            continue; // template
        }
        assert!(
            !url.to_lowercase().contains(".iso"),
            "catalog must not embed concrete iso links: {url}"
        );
    }
}

// ---------------------------------------------------------------------------
// CLI contract (Fido-aligned parameters)
// ---------------------------------------------------------------------------

#[test]
fn cli_direct_system_download_contract() {
    // `vpilot win11 -v 25H2 -lang zh-CN -type desktop`
    let raw = [
        "vpilot", "win11", "-v", "25H2", "-lang", "zh-CN", "-type", "desktop",
    ]
    .map(String::from)
    .to_vec();
    let cli = Cli::parse_from(ventoypilot::cli::normalize_args(raw));
    assert!(cli.command.is_none());
    assert_eq!(cli.system.as_deref(), Some("win11"));
    assert_eq!(cli.download.release.as_deref(), Some("25H2"));
    assert_eq!(cli.download.edition.as_deref(), Some("desktop"));
    assert_eq!(cli.download.lang.as_deref(), Some("zh-CN"));
}

#[test]
fn cli_fido_style_flags() {
    // `vpilot win11 --release 25H2 --edition Pro --lang en-US --arch arm64 --dry-run`
    let cli = Cli::parse_from([
        "vpilot",
        "win11",
        "--release",
        "25H2",
        "--edition",
        "Pro",
        "--lang",
        "en-US",
        "--arch",
        "arm64",
        "--dry-run",
    ]);
    assert_eq!(cli.system.as_deref(), Some("win11"));
    assert_eq!(cli.download.release.as_deref(), Some("25H2"));
    assert_eq!(cli.download.edition.as_deref(), Some("Pro"));
    assert_eq!(cli.download.lang.as_deref(), Some("en-US"));
    assert_eq!(cli.download.arch.as_deref(), Some("arm64"));
    assert!(cli.download.dry_run);
}

#[test]
fn cli_subcommand_wins_over_positional() {
    let cli = Cli::parse_from(["vpilot", "search", "ubuntu"]);
    assert!(cli.command.is_some());
    assert!(cli.system.is_none());
}

#[test]
fn cli_print_url_flag() {
    let cli = Cli::parse_from(["vpilot", "ubuntu", "--print-url"]);
    assert!(cli.download.print_url);
}

#[test]
fn cli_config_subcommand() {
    let cli = Cli::parse_from(["vpilot", "config", "set", "download_dir", "/tmp/iso"]);
    assert!(matches!(
        cli.command,
        Some(ventoypilot::cli::Command::Config(_))
    ));
}
