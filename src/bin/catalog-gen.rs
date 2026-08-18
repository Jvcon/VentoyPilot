//! catalog-gen: generates the static version list `catalog.json`.
//!
//! Data sources:
//!   - Linux distros: endoflife.date API (cycles/latest/lts/eol), with
//!     `manual_cycles` fallback from sources/linux.toml.
//!   - Windows: Fido-style table from sources/windows.toml (hand-maintained).
//!   - Tools: sources/tools.toml (hand-maintained).
//!
//! The output contains NO concrete download links.
//!
//! Run by GitHub Actions on a schedule; committed back to the repo.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use serde::Deserialize;

mod schema {
    include!("../catalog_schema.rs");
}

use schema::*;

#[derive(Debug, Parser)]
struct Args {
    #[arg(short, long, default_value = "sources")]
    sources_dir: PathBuf,
    #[arg(short, long, default_value = "catalog.json")]
    output: PathBuf,
    /// Do not touch the network; use only manual_cycles.
    #[arg(long)]
    offline: bool,
}

fn main() {
    let args = Args::parse();
    if let Err(e) = run(args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    let linux_src: LinuxSources = toml::from_str(&std::fs::read_to_string(
        args.sources_dir.join("linux.toml"),
    )?)?;
    let windows_src: WindowsSources = toml::from_str(&std::fs::read_to_string(
        args.sources_dir.join("windows.toml"),
    )?)?;
    let tools_src: ToolsSources = toml::from_str(&std::fs::read_to_string(
        args.sources_dir.join("tools.toml"),
    )?)?;

    let runtime = tokio::runtime::Runtime::new()?;
    let mut linux = Vec::new();
    for distro in &linux_src.distro {
        let manual: Vec<LinuxCycle> = distro
            .manual_cycles
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(Into::into)
            .collect();
        let cycles = if args.offline {
            manual
        } else {
            match fetch_cycles(&runtime, &distro.slug) {
                Ok(cycles) if !cycles.is_empty() => cycles,
                Ok(_) => {
                    eprintln!(
                        "warning: {}: endoflife returned no cycles, using manual",
                        distro.slug
                    );
                    manual
                }
                Err(e) => {
                    eprintln!("warning: {}: {e}; using manual cycles", distro.slug);
                    manual
                }
            }
        };
        linux.push(LinuxDistro {
            name: distro.name.clone(),
            slug: distro.slug.clone(),
            resolver: distro.resolver.clone(),
            url_template: distro.url_template.clone(),
            page_url: distro.page_url.clone(),
            arch: distro.arch.clone(),
            cycles,
        });
    }

    let catalog = Catalog {
        generated_at: today_iso(),
        windows: windows_src.version,
        linux,
        tools: tools_src.tool,
    };
    let json = serde_json::to_string_pretty(&catalog)?;
    std::fs::write(&args.output, format!("{json}\n"))?;
    println!("wrote {}", args.output.display());
    Ok(())
}

#[derive(Deserialize)]
struct WindowsSources {
    version: Vec<WindowsVersion>,
}

#[derive(Deserialize)]
struct ToolsSources {
    tool: Vec<Tool>,
}

/// sources/linux.toml schema (has manual_cycles which the runtime schema lacks).
#[derive(Deserialize)]
struct LinuxSources {
    distro: Vec<LinuxSourceToml>,
}

#[derive(Deserialize)]
struct LinuxSourceToml {
    name: String,
    slug: String,
    resolver: String,
    url_template: Option<String>,
    page_url: Option<String>,
    #[serde(default)]
    arch: Vec<String>,
    #[serde(default)]
    manual_cycles: Option<Vec<ManualCycle>>,
}

#[derive(Deserialize, Clone)]
struct ManualCycle {
    cycle: String,
    latest: Option<String>,
    lts: Option<bool>,
    release_date: Option<String>,
    eol: Option<String>,
}

impl From<ManualCycle> for LinuxCycle {
    fn from(c: ManualCycle) -> Self {
        LinuxCycle {
            cycle: c.cycle,
            latest: c.latest,
            lts: c.lts,
            release_date: c.release_date,
            eol: c.eol,
        }
    }
}

fn fetch_cycles(
    runtime: &tokio::runtime::Runtime,
    slug: &str,
) -> Result<Vec<LinuxCycle>, Box<dyn std::error::Error>> {
    runtime.block_on(async move {
        let url = format!("https://endoflife.date/api/{slug}.json");
        let resp = reqwest::get(&url).await?;
        if !resp.status().is_success() {
            return Err(format!("endoflife.date returned {}", resp.status()).into());
        }
        let raw: Vec<EndOfLifeCycle> = resp.json().await?;
        Ok(raw
            .into_iter()
            .map(|c| LinuxCycle {
                cycle: c.cycle,
                latest: c.latest,
                lts: c.lts,
                release_date: c.release_date,
                eol: match c.eol {
                    serde_json::Value::String(s) if !s.is_empty() => Some(s),
                    _ => None,
                },
            })
            .collect())
    })
}

#[derive(Deserialize)]
struct EndOfLifeCycle {
    cycle: String,
    latest: Option<String>,
    #[serde(default)]
    lts: Option<bool>,
    #[serde(rename = "releaseDate")]
    release_date: Option<String>,
    #[serde(default)]
    eol: serde_json::Value,
}

fn today_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs / 86400;
    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_date_ok() {
        assert_eq!(today_iso().len(), 10);
    }
}
