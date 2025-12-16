use schemars::JsonSchema;
use serde::Serialize;

use crate::cargo;

/// Response for `cargo_dependencies` tool
#[derive(Debug, Serialize, JsonSchema)]
pub struct Response {
    pub crates: Vec<cargo::CrateInfo>,
}

/// Logic for the `cargo_dependencies` tool (self-contained)
pub async fn run() -> Result<Response, String> {
    let metadata = cargo::get_metadata().await?;
    let root = metadata
        .root_package()
        .ok_or_else(|| "no root package found".to_string())?;

    let crates = cargo::get_dependencies(&metadata, root);
    Ok(Response { crates })
}
