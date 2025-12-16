use anyhow::Result;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::io::stdio,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone)]
pub struct Copilot {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Copilot {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Calculate the sum of two numbers")]
    fn sum(&self, Parameters(SumRequest { a, b }): Parameters<SumRequest>) -> String {
        (a + b).to_string()
    }

    #[tool(description = "Calculate the difference of two numbers")]
    fn sub(&self, Parameters(SubRequest { a, b }): Parameters<SubRequest>) -> String {
        (a - b).to_string()
    }

    #[tool(
        name = "cargo_dependencies",
        description = "List all available dependencies (including dev dependencies)"
    )]
    async fn cargo_dependencies(&self) -> Result<rmcp::Json<CargoDependenciesResponse>, String> {
        let metadata =
            tokio::task::spawn_blocking(|| cargo_metadata::MetadataCommand::new().exec())
                .await
                .map_err(|e| format!("failed to run cargo metadata task: {}", e))?
                .map_err(|e| format!("cargo metadata error: {}", e))?;

        if let Some(root) = metadata.root_package() {
            let mut ids = root
                .dependencies
                .iter()
                .map(|d| d.name.clone())
                .collect::<Vec<_>>();
            ids.sort();
            ids.dedup();
            Ok(rmcp::Json(CargoDependenciesResponse { package_ids: ids }))
        } else {
            Err("no root package found".into())
        }
    }
}

#[tool_handler]
impl ServerHandler for Copilot {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("MCP server for Cargo".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SumRequest {
    #[schemars(description = "the left hand side number")]
    pub a: i32,
    pub b: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SubRequest {
    #[schemars(description = "the left hand side number")]
    pub a: i32,
    #[schemars(description = "the right hand side number")]
    pub b: i32,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CargoDependenciesResponse {
    /// list of package ids (package names)
    pub package_ids: Vec<String>,
}

// npx @modelcontextprotocol/inspector cargo run
#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("Starting cargo-copilot");
    let service = Copilot::new().serve(stdio()).await?;

    service.waiting().await?;
    Ok(())
}
