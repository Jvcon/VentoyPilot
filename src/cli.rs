use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::catalog::Kind;

#[derive(Debug, Parser)]
#[command(
    name = "vpilot",
    version,
    about = "VentoyPilot: rule-driven toolbox to manage assets for Ventoy/iVentoy",
    subcommand_precedence_over_arg = true
)]
pub struct Cli {
    /// Manage configuration (config.toml).
    #[command(subcommand)]
    pub command: Option<Command>,

    /// System name to download, e.g. win11 / win10 / ubuntu / debian / nixos / uefi-shell.
    /// (Fido -Win)
    pub system: Option<String>,

    #[command(flatten)]
    pub download: DownloadArgs,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage configuration
    Config(ConfigArgs),
    /// Search available systems from the embedded catalog
    Search(SearchArgs),
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Set a config value, e.g. `vpilot config set download_dir ~/iso`
    Set { key: String, value: String },
    /// Print a config value
    Get { key: String },
    /// List all config values
    List,
    /// Print the config file path
    Path,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    /// Optional query to filter systems by name/slug
    pub query: Option<String>,
    /// Only search one kind of system
    #[arg(long, value_enum)]
    pub kind: Option<KindArg>,
    /// Print machine-readable JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum KindArg {
    Windows,
    Linux,
    Tools,
}

impl From<KindArg> for Kind {
    fn from(k: KindArg) -> Kind {
        match k {
            KindArg::Windows => Kind::Windows,
            KindArg::Linux => Kind::Linux,
            KindArg::Tools => Kind::Tools,
        }
    }
}

/// Normalize single-dash multi-char compat flags (`-lang`/`-type` as in the
/// Fido-style examples) into their long form before clap parsing.
pub fn normalize_args(args: Vec<String>) -> Vec<String> {
    args.into_iter()
        .map(|a| match a.as_str() {
            "-lang" => "--lang".to_string(),
            "-type" => "--type".to_string(),
            _ => a,
        })
        .collect()
}

/// Download options. Mirrors Fido's -Rel/-Ed/-Lang/-Arch/-GetUrl.
#[derive(Debug, Default, Args)]
pub struct DownloadArgs {
    /// Windows release (Fido -Rel), e.g. 25H2 / 24H2 / latest
    #[arg(
        short = 'v',
        long = "release",
        visible_short_alias = 'r',
        value_name = "RELEASE"
    )]
    pub release: Option<String>,

    /// Windows edition (Fido -Ed), e.g. Pro / Home / desktop
    #[arg(
        short = 'e',
        long = "edition",
        alias = "type",
        visible_short_alias = 't',
        value_name = "EDITION"
    )]
    pub edition: Option<String>,

    /// Language (Fido -Lang), e.g. zh-CN
    #[arg(short = 'l', long = "lang", value_name = "LANG")]
    pub lang: Option<String>,

    /// Architecture (Fido -Arch): x64 | arm64 | x86
    #[arg(short = 'a', long = "arch", value_name = "ARCH")]
    pub arch: Option<String>,

    /// Output directory (overrides config download_dir)
    #[arg(short = 'o', long = "output", value_name = "DIR")]
    pub output: Option<PathBuf>,

    /// Only print the final download URL (Fido -GetUrl)
    #[arg(long = "print-url")]
    pub print_url: bool,

    /// Debug: resolve link, probe availability and show target path/name; download nothing
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Show progress bar during the real download
    #[arg(long, default_value_t = true)]
    pub progress: bool,
}
