use async_trait::async_trait;

use super::resolver::{DownloadPlan, DownloadRequest, LinkResolver, ResolverContext};
use crate::catalog::{Catalog, Kind, Tool};
use crate::error::{Error, Result};
use crate::http::HttpClient;

/// Utility / rescue disk resolver (UEFI Shell etc.): versions come from the
/// static catalog, the concrete asset URL is built at download time.
pub struct ToolsResolver<'a> {
    ctx: &'a ResolverContext<'a>,
    catalog: &'a Catalog,
}

impl<'a> ToolsResolver<'a> {
    pub fn new(ctx: &'a ResolverContext<'a>, catalog: &'a Catalog) -> Self {
        Self { ctx, catalog }
    }

    fn find_tool(&self, name: &str) -> Result<&'a Tool> {
        let q = name.to_lowercase();
        self.catalog
            .tools
            .iter()
            .find(|t| t.slug.to_lowercase() == q || t.name.to_lowercase().contains(&q))
            .ok_or_else(|| Error::NotFound(format!("unknown tool: {name}")))
    }
}

#[async_trait]
impl<'a> LinkResolver for ToolsResolver<'a> {
    fn kind(&self) -> Kind {
        Kind::Tools
    }

    fn find(&self, name: &str) -> Result<()> {
        self.find_tool(name).map(|_| ())
    }

    async fn resolve(&self, req: &DownloadRequest, _http: &dyn HttpClient) -> Result<DownloadPlan> {
        let tool = self.find_tool(&req.system)?;
        if tool.cycles.is_empty() {
            return Err(Error::NotFound(format!(
                "no version info for {} in catalog",
                tool.name
            )));
        }
        let rel = req
            .release
            .as_deref()
            .unwrap_or(&self.ctx.config.default_release);
        let cycle = if rel.is_empty() || rel == "latest" {
            &tool.cycles[0]
        } else {
            let q = rel.to_lowercase();
            tool.cycles
                .iter()
                .find(|c| c.tag.to_lowercase().contains(&q))
                .ok_or_else(|| {
                    Error::NotFound(format!(
                        "release '{rel}' not found for {} (available: {})",
                        tool.name,
                        tool.cycles
                            .iter()
                            .map(|c| c.tag.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })?
        };

        let tpl = tool
            .url_template
            .as_ref()
            .ok_or_else(|| Error::Other(format!("{}: no url_template in catalog", tool.name)))?;
        let url = tpl
            .replace("{tag}", &cycle.tag)
            .replace("{release}", cycle.release.as_deref().unwrap_or(""));
        let fallback = format!("{}-{}.iso", tool.slug, cycle.tag);
        let file_name = super::resolver::file_name_from_url(&url, &fallback);
        let source = format!(
            "{} {} ({})",
            tool.name,
            cycle.tag,
            tool.repo.as_deref().unwrap_or("")
        );
        Ok(DownloadPlan::new(
            Kind::Tools,
            url,
            file_name,
            &req.target_dir,
            "x86_64",
            &source,
        ))
    }
}
