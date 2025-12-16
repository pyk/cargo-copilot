use rmcp::{
    Json, ServerHandler,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};

use crate::tools::cargo_dependencies;
use crate::tools::cargo_doc_get;
use crate::tools::cargo_doc_index;
use crate::tools::cargo_doc_overview;

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

    #[tool(
        name = "cargo_dependencies",
        description = "List all available dependencies as crate ids (name@version)"
    )]
    async fn cargo_dependencies(&self) -> Result<Json<cargo_dependencies::Response>, String> {
        let resp = cargo_dependencies::run().await?;
        Ok(Json(resp))
    }

    #[tool(
        name = "cargo_doc_overview",
        description = "Fetch the main documentation page for a crate from local `cargo doc` and return as markdown"
    )]
    async fn cargo_doc_overview(
        &self,
        Parameters(req): Parameters<cargo_doc_overview::Request>,
    ) -> Result<String, String> {
        cargo_doc_overview::run(&req).await
    }

    #[tool(
        name = "cargo_doc_index",
        description = "List symbols (modules, macros, structs, enums, functions, types) from a crate's generated docs"
    )]
    async fn cargo_doc_index(
        &self,
        Parameters(req): Parameters<cargo_doc_index::Request>,
    ) -> Result<Json<cargo_doc_index::Response>, String> {
        let resp = cargo_doc_index::run(&req).await?;
        Ok(Json(resp))
    }

    #[tool(
        name = "cargo_doc_get",
        description = "Get full documentation page for a symbol as markdown"
    )]
    async fn cargo_doc_get(
        &self,
        Parameters(req): Parameters<cargo_doc_get::Request>,
    ) -> Result<String, String> {
        let resp = cargo_doc_get::run(&req).await?;
        Ok(resp)
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
