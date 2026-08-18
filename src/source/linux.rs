use async_trait::async_trait;

use super::resolver::{DownloadPlan, DownloadRequest, LinkResolver, ResolverContext};
use crate::catalog::{Catalog, Kind, LinuxCycle, LinuxDistro};
use crate::error::{Error, Result};
use crate::http::HttpClient;

/// Linux resolver: version list comes from the static catalog (endoflife.date),
/// the concrete ISO link is produced at download time.
pub struct LinuxResolver<'a> {
    ctx: &'a ResolverContext<'a>,
    catalog: &'a Catalog,
}

impl<'a> LinuxResolver<'a> {
    pub fn new(ctx: &'a ResolverContext<'a>, catalog: &'a Catalog) -> Self {
        Self { ctx, catalog }
    }

    fn find_distro(&self, name: &str) -> Result<&'a LinuxDistro> {
        let q = name.to_lowercase();
        self.catalog
            .linux
            .iter()
            .find(|d| d.slug.to_lowercase() == q || d.name.to_lowercase().contains(&q))
            .ok_or_else(|| Error::NotFound(format!("unknown Linux distro: {name}")))
    }

    fn find_cycle<'d>(
        &self,
        distro: &'d LinuxDistro,
        release: Option<&str>,
    ) -> Result<&'d LinuxCycle> {
        if distro.cycles.is_empty() {
            return Err(Error::NotFound(format!(
                "no version info for {} in catalog (check `vpilot search --kind linux`)",
                distro.name
            )));
        }
        let rel = release.unwrap_or(&self.ctx.config.default_release);
        if rel.is_empty() || rel == "latest" {
            return Ok(&distro.cycles[0]);
        }
        let q = rel.to_lowercase();
        distro
            .cycles
            .iter()
            .find(|c| c.cycle.to_lowercase().contains(&q))
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "release '{rel}' not found for {} (available: {})",
                    distro.name,
                    distro
                        .cycles
                        .iter()
                        .map(|c| c.cycle.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
    }

    fn map_arch(&self, distro: &LinuxDistro, arch: Option<&str>) -> String {
        if distro.arch.is_empty() {
            return arch.unwrap_or(&self.ctx.config.default_arch).to_lowercase();
        }
        let requested = arch.unwrap_or(&self.ctx.config.default_arch);
        let norm = |s: &str| s.to_lowercase().replace(['_', '-'], "");
        let want = norm(requested);

        for a in &distro.arch {
            if norm(a) == want {
                return a.clone();
            }
        }
        let want_arm = want.contains("arm");
        let want_x86 = want.contains("86") || want.contains("64");
        for a in &distro.arch {
            let na = norm(a);
            if want_arm && na.contains("arm") {
                return a.clone();
            }
            if want_x86 && (na.contains("86") || na.contains("64")) {
                return a.clone();
            }
        }
        distro.arch[0].clone()
    }
}

#[async_trait]
impl<'a> LinkResolver for LinuxResolver<'a> {
    fn kind(&self) -> Kind {
        Kind::Linux
    }

    fn find(&self, name: &str) -> Result<()> {
        self.find_distro(name).map(|_| ())
    }

    async fn resolve(&self, req: &DownloadRequest, http: &dyn HttpClient) -> Result<DownloadPlan> {
        let distro = self.find_distro(&req.system)?;
        // Proxmox scrapes live links and matches the release there; the other
        // resolvers pick a version from the catalog first.
        if distro.resolver != "proxmox" {
            let cycle = self.find_cycle(distro, req.release.as_deref())?;
            let arch = self.map_arch(distro, req.arch.as_deref());
            let version = cycle
                .latest
                .as_ref()
                .filter(|v| !v.is_empty())
                .unwrap_or(&cycle.cycle);
            return self
                .resolve_with_version(req, http, distro, cycle, arch, version)
                .await;
        }
        self.resolve_proxmox(req, http, distro).await
    }
}

impl<'a> LinuxResolver<'a> {
    async fn resolve_with_version(
        &self,
        req: &DownloadRequest,
        _http: &dyn HttpClient,
        distro: &'a LinuxDistro,
        cycle: &'a LinuxCycle,
        arch: String,
        version: &str,
    ) -> Result<DownloadPlan> {
        match distro.resolver.as_str() {
            "template" => {
                let tpl = distro.url_template.as_ref().ok_or_else(|| {
                    Error::Other(format!("{}: no url_template in catalog", distro.name))
                })?;
                let url = tpl
                    .replace("{version}", version)
                    .replace("{cycle}", &cycle.cycle)
                    .replace("{arch}", &arch);
                let fallback = format!(
                    "{}-{}-{}.iso",
                    distro.slug,
                    cycle.cycle.replace(' ', "_"),
                    arch
                );
                let file_name = super::resolver::file_name_from_url(&url, &fallback);
                let source = format!(
                    "{} {} {} ({})",
                    distro.name,
                    cycle.cycle,
                    cycle.latest.as_deref().unwrap_or(""),
                    distro.resolver
                );
                Ok(DownloadPlan::new(
                    Kind::Linux,
                    url,
                    file_name,
                    &req.target_dir,
                    &arch,
                    &source,
                ))
            }
            "nixos" => {
                // Channel URL that redirects to the concrete ISO; fine to download directly.
                let nix_arch = match arch.as_str() {
                    "amd64" | "x86_64" => "x86_64",
                    "arm64" | "aarch64" => "aarch64",
                    other => other,
                };
                let url = format!(
                    "https://channels.nixos.org/nixos-{}/latest-nixos-minimal-{}-linux.iso",
                    cycle.cycle, nix_arch
                );
                let file_name = format!("nixos-minimal-{}-{}.iso", cycle.cycle, nix_arch);
                Ok(DownloadPlan::new(
                    Kind::Linux,
                    url,
                    file_name,
                    &req.target_dir,
                    &arch,
                    &format!("NixOS {} channel", cycle.cycle),
                ))
            }
            "proxmox" => Err(Error::Other(
                "proxmox resolver should not reach this path".into(),
            )),
            "release-page" => {
                // No bootable ISO (e.g. KDE Plasma is a desktop environment):
                // we only resolve to the official page.
                let page = distro
                    .page_url
                    .as_deref()
                    .unwrap_or("https://kde.org/plasma-desktop/");
                let file_name = format!("{}-{}.txt", distro.slug, cycle.cycle);
                let mut plan = DownloadPlan::new(
                    Kind::Linux,
                    page.to_string(),
                    file_name,
                    &req.target_dir,
                    &arch,
                    &format!("{} release page (no bootable ISO)", distro.name),
                );
                plan.downloadable = false;
                Ok(plan)
            }
            other => Err(Error::Other(format!(
                "{}: unknown resolver '{other}'",
                distro.name
            ))),
        }
    }

    async fn resolve_proxmox(
        &self,
        req: &DownloadRequest,
        http: &dyn HttpClient,
        distro: &'a LinuxDistro,
    ) -> Result<DownloadPlan> {
        const PAGE: &str = "https://www.proxmox.com/en/downloads/proxmox-virtual-environment/iso";
        let resp = http.get(PAGE).await?;
        let html = resp.text()?;
        let re = regex::Regex::new(r#"href="(https?://[^"]*proxmox-ve[^"]*\.iso)"#)
            .map_err(|e| Error::Parse(e.to_string()))?;
        let mut links: Vec<String> = re.captures_iter(&html).map(|c| c[1].to_string()).collect();
        links.sort();
        links.dedup();
        if links.is_empty() {
            return Err(Error::NotFound(
                "no proxmox-ve*.iso links found on the Proxmox page (page structure may have changed; open it manually)".into(),
            ));
        }
        let rel = req
            .release
            .as_deref()
            .filter(|r| !r.is_empty() && *r != "latest")
            .unwrap_or(&self.ctx.config.default_release);
        let link = if rel.is_empty() || rel == "latest" {
            links.last().cloned().unwrap()
        } else {
            links
                .iter()
                .rev()
                .find(|l| l.contains(&format!("proxmox-ve_{rel}-")))
                .or_else(|| links.iter().find(|l| l.contains(rel)))
                .cloned()
                .ok_or_else(|| {
                    Error::NotFound(format!(
                        "no Proxmox ISO for release '{rel}' (found: {})",
                        links.join(", ")
                    ))
                })?
        };
        let file_name = super::resolver::file_name_from_url(&link, "proxmox-ve.iso");
        let arch = self.map_arch(distro, req.arch.as_deref());
        Ok(DownloadPlan::new(
            Kind::Linux,
            link,
            file_name,
            &req.target_dir,
            &arch,
            "Proxmox VE (scraped)",
        ))
    }
}
