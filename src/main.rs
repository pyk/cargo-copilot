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

    #[tool(
        name = "crate_overview",
        description = "Fetch the main documentation page for a crate from local `cargo doc` and return as markdown"
    )]
    async fn crate_overview(
        &self,
        Parameters(CrateOverviewRequest { crate_id }): Parameters<CrateOverviewRequest>,
    ) -> Result<String, String> {
        // Accept `name@version` or `name`
        let crate_name = crate_id.split('@').next().unwrap_or(&crate_id);

        // Build docs (stable `cargo doc`); use --no-deps to reduce work
        self.run_cargo_doc(crate_name).await?;

        // Read the generated HTML and extract the first `div.docblock` content
        let html = self.read_doc_index_html(crate_name).await?;
        let docblock_html = self
            .extract_docblock(&html)
            .ok_or_else(|| "no <div class=\"docblock\"> found in index.html".to_string())?;

        // Convert HTML to Markdown and return
        let md = html2md::parse_html(&docblock_html);
        Ok(md)
    }

    #[tool(
        name = "crate_symbol_list",
        description = "List symbols (modules, macros, structs, enums, functions, types) from a crate's generated docs"
    )]
    async fn crate_symbol_list(
        &self,
        Parameters(CrateSymbolListRequest { crate_id }): Parameters<CrateSymbolListRequest>,
    ) -> Result<rmcp::Json<CrateSymbolListResponse>, String> {
        let crate_name = crate_id.split('@').next().unwrap_or(&crate_id);
        self.run_cargo_doc(crate_name).await?;
        let html = self.read_doc_index_html(crate_name).await?;
        let symbols = self.extract_symbols(&html, crate_name).await?;
        Ok(rmcp::Json(CrateSymbolListResponse { symbols }))
    }

    #[tool(
        name = "crate_symbol_get",
        description = "Get full documentation page for a symbol as markdown"
    )]
    async fn crate_symbol_get(
        &self,
        Parameters(CrateSymbolGetRequest {
            crate_id,
            symbol_path,
        }): Parameters<CrateSymbolGetRequest>,
    ) -> Result<String, String> {
        let crate_name = crate_id.split('@').next().unwrap_or(&crate_id);
        self.run_cargo_doc(crate_name).await?;

        // Normalize symbol_path: remove leading '/', append .html if missing
        let mut rel = symbol_path.trim().trim_start_matches('/').to_string();
        if !rel.ends_with(".html") {
            rel.push_str(".html");
        }

        let html = self.read_doc_html_by_rel_path(crate_name, &rel).await?;

        // Parse and extract <section id="main-content"> synchronously inside blocking task
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

    // Run `cargo doc --package <crate> --no-deps` to generate stable HTML docs
    async fn run_cargo_doc(&self, crate_name: &str) -> Result<(), String> {
        let status = tokio::process::Command::new("cargo")
            .arg("doc")
            .arg("--package")
            .arg(crate_name)
            .arg("--no-deps")
            .status()
            .await
            .map_err(|e| format!("failed to spawn cargo doc: {}", e))?;

        if !status.success() {
            return Err(format!(
                "cargo doc failed with status: {}. Ensure the package exists locally",
                status
            ));
        }

        Ok(())
    }

    // Read `target/doc/<crate>/index.html`
    async fn read_doc_index_html(&self, crate_name: &str) -> Result<String, String> {
        let path = std::path::Path::new("target")
            .join("doc")
            .join(crate_name)
            .join("index.html");
        let contents = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
        Ok(contents)
    }

    // Extract inner HTML of the first `div.docblock` in the page
    fn extract_docblock(&self, html: &str) -> Option<String> {
        let document = scraper::Html::parse_document(html);
        let selector = match scraper::Selector::parse("div.docblock") {
            Ok(s) => s,
            Err(_) => return None,
        };
        let mut iter = document.select(&selector);
        iter.next().map(|el| el.inner_html())
    }

    // Extract symbol listings (modules, macros, structs, enums, functions, types) from index.html
    async fn extract_symbols(
        &self,
        html: &str,
        crate_name: &str,
    ) -> Result<Vec<SymbolInfo>, String> {
        use std::collections::VecDeque;

        // queue of (html_string, base_dir)
        let mut queue: VecDeque<(String, std::path::PathBuf)> = VecDeque::new();
        queue.push_back((html.to_string(), std::path::PathBuf::from("")));

        let mut symbols: Vec<SymbolInfo> = Vec::new();
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();

        while let Some((page_html, base_dir)) = queue.pop_front() {
            // process page synchronously
            let (page_symbols, modules) = self.process_page(&page_html, &base_dir);
            symbols.extend(page_symbols);

            // schedule modules to visit
            for module_path in modules {
                if visited.contains(&module_path) {
                    continue;
                }
                // attempt to read module html
                match self
                    .read_doc_html_by_rel_path(crate_name, &module_path)
                    .await
                {
                    Ok(module_html) => {
                        let parent = std::path::Path::new(&module_path)
                            .parent()
                            .unwrap_or(std::path::Path::new(""))
                            .to_path_buf();
                        queue.push_back((module_html, parent));
                        visited.insert(module_path);
                    }
                    Err(_) => {
                        // ignore missing module page
                    }
                }
            }
        }

        Ok(symbols)
    }

    // Process a single page synchronously and extract SymbolInfo entries and module links to visit
    fn process_page(
        &self,
        html: &str,
        base_dir: &std::path::Path,
    ) -> (Vec<SymbolInfo>, Vec<String>) {
        let document = scraper::Html::parse_document(html);

        // Mapping of section id -> symbol_type string
        let sections = vec![
            ("modules", "module"),
            ("macros", "macro"),
            ("structs", "struct"),
            ("enums", "enum"),
            ("functions", "function"),
            ("types", "type_alias"),
        ];

        let mut out = Vec::new();
        let mut modules_to_visit = Vec::new();

        for (section_id, symbol_type) in sections {
            let selector_str = format!("h2#{} + dl.item-table", section_id);
            let dl_selector = match scraper::Selector::parse(&selector_str) {
                Ok(s) => s,
                Err(_) => continue,
            };

            for dl in document.select(&dl_selector) {
                let pair_selector = match scraper::Selector::parse("dt, dd") {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                let mut iter = dl.select(&pair_selector);
                while let Some(item) = iter.next() {
                    if item.value().name() == "dt"
                        && let Some(a) = item.select(&scraper::Selector::parse("a").unwrap()).next()
                    {
                        let symbol_id = a.text().collect::<Vec<_>>().join("").trim().to_string();
                        let href = a.value().attr("href").unwrap_or("").to_string();

                        // Compute full relative path (normalize)
                        let full_path = if base_dir.as_os_str().is_empty() {
                            std::path::Path::new(&href).to_path_buf()
                        } else {
                            base_dir.join(&href)
                        };
                        let full_path = Self::normalize_rel_path(&full_path);
                        let full_path_str = full_path.to_string_lossy().replace("\\", "/");

                        // next should be dd (optional)
                        let mut desc: Option<String> = None;
                        if let Some(next_item) = iter.next()
                            && next_item.value().name() == "dd"
                        {
                            let dd_html = next_item.inner_html();
                            let dd_md = html2md::parse_html(&dd_html);
                            let dd_trim = dd_md.trim().to_string();
                            if !dd_trim.is_empty() {
                                desc = Some(dd_trim);
                            }
                        }

                        out.push(SymbolInfo {
                            symbol_id: symbol_id.clone(),
                            symbol_path: full_path_str.clone(),
                            symbol_type: symbol_type.to_string(),
                            symbol_description: desc,
                        });

                        if symbol_type == "module" {
                            modules_to_visit.push(full_path_str.clone());
                        }
                    }
                }
            }
        }

        (out, modules_to_visit)
    }

    // Read an arbitrary doc HTML file relative to the crate doc dir, e.g., "de/index.html" or "struct.Error.html"
    async fn read_doc_html_by_rel_path(
        &self,
        crate_name: &str,
        rel_path: &str,
    ) -> Result<String, String> {
        let path = std::path::Path::new("target")
            .join("doc")
            .join(crate_name)
            .join(rel_path);
        let contents = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| format!("failed to read {}: {}", path.display(), e))?;
        Ok(contents)
    }

    // Normalize a relative path, removing `./` and resolving `..` segments
    fn normalize_rel_path(p: &std::path::Path) -> std::path::PathBuf {
        let mut out = std::path::PathBuf::new();
        for comp in p.components() {
            match comp {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    let _ = out.pop();
                }
                std::path::Component::Normal(s) => out.push(s),
                std::path::Component::RootDir => out.push(std::path::Path::new("/")),
                std::path::Component::Prefix(_) => out.push(comp.as_os_str()),
            }
        }
        out
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
pub struct CrateOverviewRequest {
    /// crate id in the form `name@version` or just `name`
    pub crate_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CrateSymbolListRequest {
    /// crate id in the form `name@version` or just `name`
    pub crate_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CrateSymbolGetRequest {
    /// crate id in the form `name@version` or just `name`
    pub crate_id: String,
    /// symbol path relative to crate docs, e.g. `macro.anyhow` or `de/struct.Deserializer`
    pub symbol_path: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SymbolInfo {
    /// anchor text (symbol identifier)
    pub symbol_id: String,
    /// path/href to the symbol page from the crate docs (e.g., `macro.anyhow.html`)
    pub symbol_path: String,
    /// type of symbol: module|macro|struct|enum|function|type_alias
    pub symbol_type: String,
    /// optional description (converted to markdown)
    pub symbol_description: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CrateSymbolListResponse {
    pub symbols: Vec<SymbolInfo>,
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
