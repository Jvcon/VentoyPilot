use clap::Parser;

use ventoypilot::catalog::{Catalog, Kind};
use ventoypilot::cli::{normalize_args, Cli, Command, ConfigAction};
use ventoypilot::config::Config;
use ventoypilot::error::{Error, Result};
use ventoypilot::source::{LinkResolver, ResolverContext};

#[tokio::main]
async fn main() {
    let args = normalize_args(std::env::args().collect());
    let cli = Cli::parse_from(args);
    if let Err(e) = run(cli).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Some(Command::Config(args)) => run_config(args.action),
        Some(Command::Search(args)) => {
            run_search(&args.query, args.kind.map(Into::into), args.json)
        }
        None => {
            if cli.system.is_none() {
                return Err(Error::Other(
                    "no system specified. Try `vpilot search` or `vpilot --help`.".into(),
                ));
            }
            run_download(cli).await
        }
    }
}

fn run_config(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Path => {
            println!("{}", Config::config_path().display());
            Ok(())
        }
        ConfigAction::List => {
            let cfg = Config::load()?;
            for key in Config::KEYS {
                if let Some(v) = cfg.get(key) {
                    println!("{key} = {v}");
                }
            }
            Ok(())
        }
        ConfigAction::Get { key } => {
            let cfg = Config::load()?;
            match cfg.get(&key) {
                Some(v) => {
                    println!("{v}");
                    Ok(())
                }
                None => Err(Error::Config(format!("unknown config key '{key}'"))),
            }
        }
        ConfigAction::Set { key, value } => {
            let mut cfg = Config::load()?;
            cfg.set(&key, &value)?;
            cfg.save()?;
            println!(
                "{} = {} (saved to {})",
                key,
                value,
                Config::config_path().display()
            );
            Ok(())
        }
    }
}

fn run_search(query: &Option<String>, kind: Option<Kind>, json: bool) -> Result<()> {
    let catalog = Catalog::load()?;
    let matches = catalog.search(kind, query.as_deref());
    if json {
        let rows: Vec<serde_json::Value> = matches
            .iter()
            .map(|m| {
                serde_json::json!({
                    "kind": m.kind.as_str(),
                    "name": m.name,
                    "slug": m.slug,
                    "versions": m.versions,
                    "detail": m.detail,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows).unwrap());
        return Ok(());
    }
    if matches.is_empty() {
        println!("no systems found");
        return Ok(());
    }
    let mut last_kind: Option<&str> = None;
    for m in &matches {
        if last_kind != Some(m.kind.as_str()) {
            println!("\n[{}]", m.kind.as_str());
            last_kind = Some(m.kind.as_str());
        }
        println!("  {}", m.name);
        for v in &m.versions {
            println!("    - {v}");
        }
    }
    println!("\nHint: `vpilot <name> --dry-run` resolves the download link without downloading.");
    Ok(())
}

async fn run_download(cli: Cli) -> Result<()> {
    let cfg = Config::load()?;
    let catalog = Catalog::load()?;
    let system = cli.system.clone().unwrap();

    let ctx = ResolverContext {
        config: &cfg,
        kind: Kind::Windows,
    };
    let resolvers: Vec<Box<dyn LinkResolver>> = vec![
        Box::new(ventoypilot::source::windows::WindowsResolver::new(
            &ctx, &catalog,
        )),
        Box::new(ventoypilot::source::linux::LinuxResolver::new(
            &ctx, &catalog,
        )),
        Box::new(ventoypilot::source::tools::ToolsResolver::new(
            &ctx, &catalog,
        )),
    ];

    let mut found: Option<(Kind, Box<dyn LinkResolver>)> = None;
    for r in resolvers {
        if r.find(&system).is_ok() {
            found = Some((r.kind(), r));
            break;
        }
    }
    let (_kind, resolver) = found.ok_or_else(|| {
        let all = catalog
            .search(None, None)
            .iter()
            .map(|m| format!("{} ({})", m.name, m.kind.as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        Error::NotFound(format!(
            "unknown system '{system}'. Available: {all}. Try `vpilot search`."
        ))
    })?;

    let target_dir = cfg.download_dir(cli.download.output.as_deref());
    let req = ventoypilot::source::resolver::DownloadRequest {
        system,
        release: cli.download.release.clone(),
        edition: cli.download.edition.clone(),
        lang: cli.download.lang.clone(),
        arch: cli.download.arch.clone(),
        target_dir,
    };

    let http = ventoypilot::http::ReqwestClient::new()?;
    let plan = resolver.resolve(&req, &http).await?;

    if cli.download.print_url {
        ventoypilot::download::print_url(&plan);
        return Ok(());
    }
    if cli.download.dry_run {
        ventoypilot::download::dry_run(&http, &plan).await?;
        return Ok(());
    }
    if !plan.downloadable {
        return Err(Error::Other(format!(
            "{} has no bootable ISO; open {} manually",
            plan.source, plan.url
        )));
    }

    let client = reqwest::Client::builder()
        .user_agent("VentoyPilot/0.1 (+https://github.com/Jvcon/VentoyPilot)")
        .build()
        .map_err(|e| Error::Http(e.to_string()))?;
    ventoypilot::download::download_file(&client, &plan, cli.download.progress).await
}
