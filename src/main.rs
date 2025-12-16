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
        description = "List all available dependencies as crate ids (name@version)"
    )]
    async fn cargo_dependencies(&self) -> Result<rmcp::Json<CargoDependenciesResponse>, String> {
        let metadata = self.get_metadata().await?;
        let root = metadata
            .root_package()
            .ok_or_else(|| "no root package found".to_string())?;

        let crates = self.get_dependencies(&metadata, root);
        Ok(rmcp::Json(CargoDependenciesResponse { crates }))
    }

    // Fetch cargo metadata in a blocking task and convert errors to String for the tool API
    async fn get_metadata(&self) -> Result<cargo_metadata::Metadata, String> {
        tokio::task::spawn_blocking(|| cargo_metadata::MetadataCommand::new().exec())
            .await
            .map_err(|e| format!("failed to run cargo metadata task: {}", e))?
            .map_err(|e| format!("cargo metadata error: {}", e))
    }

    // Collect crate info objects in a deterministic and readable way
    fn get_dependencies(
        &self,
        metadata: &cargo_metadata::Metadata,
        root: &cargo_metadata::Package,
    ) -> Vec<CrateInfo> {
        // Try to use the resolved dependency graph when available (gives exact package info)
        if let Some(node) = self.find_root_resolve_node(metadata, root) {
            let infos = self.resolved_dep_infos(node, metadata);
            if !infos.is_empty() {
                return self.unique_sorted_crates(infos);
            }
        }

        // Fallback: use declared dependencies and look up package info from `metadata.packages`
        let infos: Vec<CrateInfo> = root
            .dependencies
            .iter()
            .map(|d| {
                if let Some(p) = metadata.packages.iter().find(|p| p.name == d.name) {
                    CrateInfo {
                        crate_id: format!("{}@{}", p.name, p.version),
                        crate_name: p.name.clone(),
                        crate_version: p.version.to_string(),
                        crate_description: p.description.clone(),
                    }
                } else {
                    CrateInfo {
                        crate_id: d.name.clone(),
                        crate_name: d.name.clone(),
                        crate_version: String::new(),
                        crate_description: None,
                    }
                }
            })
            .collect();

        self.unique_sorted_crates(infos)
    }

    // Return the resolve node for the root package if available
    fn find_root_resolve_node<'a>(
        &self,
        metadata: &'a cargo_metadata::Metadata,
        root: &'a cargo_metadata::Package,
    ) -> Option<&'a cargo_metadata::Node> {
        metadata
            .resolve
            .as_ref()
            .and_then(move |r| r.nodes.iter().find(|n| n.id == root.id))
    }

    // Collect dep infos from a resolve node
    fn resolved_dep_infos(
        &self,
        node: &cargo_metadata::Node,
        metadata: &cargo_metadata::Metadata,
    ) -> Vec<CrateInfo> {
        node.deps
            .iter()
            .map(|d| self.format_dep_info(d, metadata))
            .collect()
    }

    // Format a NodeDep into a CrateInfo when possible, with fallbacks
    fn format_dep_info(
        &self,
        dep: &cargo_metadata::NodeDep,
        metadata: &cargo_metadata::Metadata,
    ) -> CrateInfo {
        if let Some(pkg) = metadata.packages.iter().find(|p| p.id == dep.pkg) {
            CrateInfo {
                crate_id: format!("{}@{}", pkg.name, pkg.version),
                crate_name: pkg.name.clone(),
                crate_version: pkg.version.to_string(),
                crate_description: pkg.description.clone(),
            }
        } else if let Some(pkg_by_name) = metadata.packages.iter().find(|p| p.name == dep.name) {
            CrateInfo {
                crate_id: format!("{}@{}", pkg_by_name.name, pkg_by_name.version),
                crate_name: pkg_by_name.name.clone(),
                crate_version: pkg_by_name.version.to_string(),
                crate_description: pkg_by_name.description.clone(),
            }
        } else {
            CrateInfo {
                crate_id: dep.name.clone(),
                crate_name: dep.name.clone(),
                crate_version: String::new(),
                crate_description: None,
            }
        }
    }

    fn unique_sorted_crates(&self, mut infos: Vec<CrateInfo>) -> Vec<CrateInfo> {
        infos.sort_by(|a, b| a.crate_id.cmp(&b.crate_id));
        infos.dedup_by(|a, b| a.crate_id == b.crate_id);
        infos
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
pub struct CrateInfo {
    /// id formatted as `name@version`
    pub crate_id: String,
    /// package name
    pub crate_name: String,
    /// package version string
    pub crate_version: String,
    /// optional package description from Cargo.toml
    pub crate_description: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CargoDependenciesResponse {
    /// list of crate info objects
    pub crates: Vec<CrateInfo>,
}

// npx @modelcontextprotocol/inspector cargo run
#[tokio::main]
async fn main() -> Result<()> {
    eprintln!("Starting cargo-copilot");
    let service = Copilot::new().serve(stdio()).await?;

    service.waiting().await?;
    Ok(())
}
