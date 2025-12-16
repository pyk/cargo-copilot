use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::cargo;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Request {
    /// crate id in the form `name@version` or just `name`
    pub crate_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct Response {
    pub symbols: Vec<cargo::SymbolInfo>,
}

pub async fn run(req: &Request) -> Result<Response, String> {
    let crate_name = req.crate_id.split('@').next().unwrap_or(&req.crate_id);
    cargo::doc(crate_name).await?;
    let html = cargo::read_doc_index_html(crate_name).await?;
    let symbols = cargo::extract_symbols(&html, crate_name).await?;
    Ok(Response { symbols })
}
