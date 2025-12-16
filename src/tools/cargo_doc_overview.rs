use schemars::JsonSchema;
use serde::Deserialize;

use crate::cargo;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Request {
    /// crate id in the form `name@version` or just `name`
    pub crate_id: String,
}

pub async fn run(req: &Request) -> Result<String, String> {
    let crate_name = req.crate_id.split('@').next().unwrap_or(&req.crate_id);
    cargo::doc(crate_name).await?;
    let html = cargo::read_doc_index_html(crate_name).await?;
    let docblock_html = cargo::extract_docblock(&html)
        .ok_or_else(|| "no <div \"docblock\"> found in index.html".to_string())?;

    Ok(html2md::parse_html(&docblock_html))
}
