use schemars::JsonSchema;
use serde::Deserialize;

use crate::cargo;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Request {
    /// crate id in the form `name@version` or just `name`
    pub crate_id: String,
    /// symbol path relative to crate docs, e.g. `macro.anyhow` or `de/struct.Deserializer`
    pub symbol_path: String,
}

pub async fn run(req: &Request) -> Result<String, String> {
    let crate_name = req.crate_id.split('@').next().unwrap_or(&req.crate_id);
    cargo::doc(crate_name).await?;

    let mut rel = req.symbol_path.trim().trim_start_matches('/').to_string();
    if !rel.ends_with(".html") {
        rel.push_str(".html");
    }

    let html = cargo::read_doc_html_by_rel_path(crate_name, &rel).await?;

    let md = tokio::task::spawn_blocking(move || {
        let document = scraper::Html::parse_document(&html);
        let selector = scraper::Selector::parse("section#main-content").ok()?;
        let content = document.select(&selector).next()?.inner_html();
        Some(html2md::parse_html(&content))
    })
    .await
    .map_err(|e| format!("task join error: {}", e))?
    .ok_or_else(|| "section#main-content not found".to_string())?;

    Ok(md)
}
