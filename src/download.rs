use std::io::Write;

use futures_util::StreamExt;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::source::resolver::{DownloadPlan, LinkCheck};

/// --print-url: only the URL, nothing else.
pub fn print_url(plan: &DownloadPlan) {
    println!("{}", plan.url);
}

/// --dry-run: resolve + probe availability + show target path/name. No bytes are written.
pub async fn dry_run(http: &dyn HttpClient, plan: &DownloadPlan) -> Result<LinkCheck> {
    println!("system:      {} ({})", plan.source, plan.kind.as_str());
    println!("url:         {}", plan.url);
    if !plan.downloadable {
        println!("downloadable: no (release page only)");
        return Ok(LinkCheck {
            url: plan.url.clone(),
            status: 0,
            size_hint: None,
            content_type: None,
            ok: false,
        });
    }
    let check = crate::source::resolver::check_link(http, plan).await?;
    let size = check
        .size_hint
        .map(human_size)
        .unwrap_or_else(|| "unknown".to_string());
    println!("availability: HTTP {} ({size})", check.status);
    println!("file name:   {}", plan.file_name);
    println!("target path: {}", plan.target_path.display());
    if let Some(hash) = &plan.checksum {
        println!("sha256:      {hash}");
    }
    if let Some(ct) = &check.content_type {
        println!("content-type: {ct}");
    }
    Ok(check)
}

/// Real download: stream to `<file>.part`, verify sha256, rename into place.
pub async fn download_file(
    client: &reqwest::Client,
    plan: &DownloadPlan,
    progress: bool,
) -> Result<()> {
    if !plan.downloadable {
        return Err(Error::Other(format!(
            "{} has no bootable ISO; open {} manually",
            plan.source, plan.url
        )));
    }
    if let Some(parent) = plan.target_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let resp = client
        .get(&plan.url)
        .send()
        .await
        .map_err(|e| Error::Http(format!("GET {}: {e}", plan.url)))?;
    let total = resp.content_length();
    let mut stream = resp.bytes_stream();

    let part_path = plan.target_path.with_extension("iso.part");
    let mut file = std::fs::File::create(&part_path)?;
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;

    let bar = progress.then(|| {
        indicatif::ProgressBar::new(total.unwrap_or(0)).with_style(
            indicatif::ProgressStyle::with_template(
                "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total} ({percent}%) {eta}",
            )
            .unwrap()
            .progress_chars("#>-"),
        )
    });

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| Error::Http(format!("stream: {e}")))?;
        hasher.update(&chunk);
        file.write_all(&chunk)?;
        written += chunk.len() as u64;
        if let Some(b) = &bar {
            b.set_position(written);
        }
    }
    file.flush()?;

    let digest = hasher.finalize();
    let hex = hex::encode(digest);
    if let Some(b) = bar {
        b.finish_and_clear();
    }

    std::fs::rename(&part_path, &plan.target_path)?;
    println!("done: {}", plan.target_path.display());
    println!("sha256: {hex}  {}", plan.file_name);
    Ok(())
}

fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_format() {
        assert_eq!(human_size(0), "0.0 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(5_864_800_256), "5.5 GiB");
    }
}
