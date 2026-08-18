//! Integration test: Linux resolvers with mocked/offline behavior.
//! No real network, no file downloads — asserts the resolved link, target
//! path and file name for each resolver flavor.

use std::path::PathBuf;

use ventoypilot::catalog::{Catalog, Kind};
use ventoypilot::config::Config;
use ventoypilot::http::MockHttp;
use ventoypilot::source::linux::LinuxResolver;
use ventoypilot::source::resolver::{DownloadRequest, LinkResolver, ResolverContext};

fn cfg_with_dir() -> Config {
    Config {
        download_dir: PathBuf::from("/tmp/vpilot-linux-dl"),
        default_release: "latest".into(),
        default_arch: "x64".into(),
        default_lang: "en-US".into(),
        ..Config::default()
    }
}

fn req(cfg: &Config, system: &str) -> DownloadRequest {
    DownloadRequest {
        system: system.into(),
        release: None,
        edition: None,
        lang: None,
        arch: None,
        target_dir: cfg.download_dir(None),
    }
}

#[tokio::test]
async fn ubuntu_template_resolver() {
    let catalog = Catalog::load().unwrap();
    let cfg = cfg_with_dir();
    let ctx = ResolverContext {
        config: &cfg,
        kind: Kind::Linux,
    };
    let http = MockHttp::new();
    let resolver = LinuxResolver::new(&ctx, &catalog);

    let plan = resolver.resolve(&req(&cfg, "ubuntu"), &http).await.unwrap();

    assert!(plan.url.starts_with("https://releases.ubuntu.com/"));
    assert!(plan.url.ends_with("ubuntu-26.04-desktop-amd64.iso"));
    assert_eq!(plan.file_name, "ubuntu-26.04-desktop-amd64.iso");
    assert_eq!(
        plan.target_path,
        PathBuf::from("/tmp/vpilot-linux-dl/ubuntu-26.04-desktop-amd64.iso")
    );
    assert_eq!(plan.arch, "amd64");
    assert!(plan.downloadable);
}

#[tokio::test]
async fn ubuntu_arch_maps_to_arm64() {
    let catalog = Catalog::load().unwrap();
    let cfg = cfg_with_dir();
    let ctx = ResolverContext {
        config: &cfg,
        kind: Kind::Linux,
    };
    let http = MockHttp::new();
    let resolver = LinuxResolver::new(&ctx, &catalog);

    let mut r = req(&cfg, "ubuntu");
    r.arch = Some("arm64".into());
    let plan = resolver.resolve(&r, &http).await.unwrap();
    assert!(plan.url.ends_with("-arm64.iso"));
}

#[tokio::test]
async fn ubuntu_specific_release() {
    let catalog = Catalog::load().unwrap();
    let cfg = cfg_with_dir();
    let ctx = ResolverContext {
        config: &cfg,
        kind: Kind::Linux,
    };
    let http = MockHttp::new();
    let resolver = LinuxResolver::new(&ctx, &catalog);

    let mut r = req(&cfg, "ubuntu");
    r.release = Some("24.04".into());
    let plan = resolver.resolve(&r, &http).await.unwrap();
    assert!(plan.url.contains("ubuntu-24.04"));
}

#[tokio::test]
async fn fedora_arch_family_fallback() {
    let catalog = Catalog::load().unwrap();
    let cfg = cfg_with_dir();
    let ctx = ResolverContext {
        config: &cfg,
        kind: Kind::Linux,
    };
    let http = MockHttp::new();
    let resolver = LinuxResolver::new(&ctx, &catalog);

    let plan = resolver.resolve(&req(&cfg, "fedora"), &http).await.unwrap();
    assert!(plan.url.contains("Fedora-Workstation-Live-44-x86_64.iso"));
    assert_eq!(plan.arch, "x86_64");
}

#[tokio::test]
async fn alpine_version_uses_latest_patch() {
    let catalog = Catalog::load().unwrap();
    let cfg = cfg_with_dir();
    let ctx = ResolverContext {
        config: &cfg,
        kind: Kind::Linux,
    };
    let http = MockHttp::new();
    let resolver = LinuxResolver::new(&ctx, &catalog);

    let plan = resolver.resolve(&req(&cfg, "alpine"), &http).await.unwrap();
    assert!(plan
        .url
        .starts_with("https://dl-cdn.alpinelinux.org/alpine/v3.24/"));
    assert!(plan.url.ends_with("alpine-3.24.1-x86_64.iso"));
}

#[tokio::test]
async fn nixos_channel_url() {
    let catalog = Catalog::load().unwrap();
    let cfg = cfg_with_dir();
    let ctx = ResolverContext {
        config: &cfg,
        kind: Kind::Linux,
    };
    let http = MockHttp::new();
    let resolver = LinuxResolver::new(&ctx, &catalog);

    let plan = resolver.resolve(&req(&cfg, "nixos"), &http).await.unwrap();
    assert!(plan.url.starts_with("https://channels.nixos.org/nixos-"));
    assert!(plan.url.ends_with("latest-nixos-minimal-x86_64-linux.iso"));
}

#[tokio::test]
async fn proxmox_scrapes_iso_links() {
    let catalog = Catalog::load().unwrap();
    let cfg = cfg_with_dir();
    let ctx = ResolverContext {
        config: &cfg,
        kind: Kind::Linux,
    };
    let mut http = MockHttp::new();
    http.route_text(
        "https://www.proxmox.com/en/downloads/proxmox-virtual-environment/iso",
        r#"<a href="https://enterprise.proxmox.com/iso/proxmox-ve_8.3-1.iso">8.3</a>
           <a href="https://enterprise.proxmox.com/iso/proxmox-ve_8.4-2.iso">8.4</a>"#,
    );
    let resolver = LinuxResolver::new(&ctx, &catalog);

    let plan = resolver
        .resolve(&req(&cfg, "proxmox"), &http)
        .await
        .unwrap();
    assert_eq!(
        plan.url,
        "https://enterprise.proxmox.com/iso/proxmox-ve_8.4-2.iso"
    );
    assert_eq!(plan.file_name, "proxmox-ve_8.4-2.iso");

    let mut r = req(&cfg, "proxmox");
    r.release = Some("8.3".into());
    let plan = resolver.resolve(&r, &http).await.unwrap();
    assert_eq!(
        plan.url,
        "https://enterprise.proxmox.com/iso/proxmox-ve_8.3-1.iso"
    );
}

#[tokio::test]
async fn kde_plasma_release_page_not_downloadable() {
    let catalog = Catalog::load().unwrap();
    let cfg = cfg_with_dir();
    let ctx = ResolverContext {
        config: &cfg,
        kind: Kind::Linux,
    };
    let http = MockHttp::new();
    let resolver = LinuxResolver::new(&ctx, &catalog);

    let plan = resolver.resolve(&req(&cfg, "kde"), &http).await.unwrap();
    assert!(!plan.downloadable);
    assert_eq!(plan.url, "https://kde.org/plasma-desktop/");
}

#[tokio::test]
async fn linux_unknown_distro_errors() {
    let catalog = Catalog::load().unwrap();
    let cfg = cfg_with_dir();
    let ctx = ResolverContext {
        config: &cfg,
        kind: Kind::Linux,
    };
    let http = MockHttp::new();
    let resolver = LinuxResolver::new(&ctx, &catalog);

    let r = req(&cfg, "slackware");
    let err = resolver.resolve(&r, &http).await.unwrap_err();
    assert!(err.to_string().contains("unknown Linux distro"));
}
