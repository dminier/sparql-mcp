//! Drives an external MCP server (e.g. semantic-code-mcp), turns its
//! responses into Endpoint records, and writes them into the store via SPARQL.

use std::sync::Arc;

use anyhow::Result;
use rmcp::model::CallToolRequestParams;
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::domain::SparqlStore;
use crate::mcp::client::McpService;
use crate::model::Endpoint;

const HKB_NS: &str = "https://sparql-mcp.dev/ns/hkb#";

pub async fn list_tools(service: &McpService) -> Result<Vec<String>> {
    let tools = service.list_all_tools().await?;
    Ok(tools.into_iter().map(|t| t.name.to_string()).collect())
}

pub async fn discover_endpoints(service: &McpService, patterns: &[&str]) -> Result<Vec<Endpoint>> {
    let mut endpoints = Vec::new();
    for pattern in patterns {
        let args = json!({ "name_pattern": pattern, "label": "Function" });
        let res = call_tool(service, "search_graph", args).await?;
        debug!(?res, "search_graph result for pattern {pattern}");
        endpoints.extend(parse_endpoints_from_search(&res));
    }
    Ok(endpoints)
}

async fn call_tool(service: &McpService, name: &str, args: Value) -> Result<Value> {
    let mut params = CallToolRequestParams::default();
    params.name = name.to_string().into();
    params.arguments = args.as_object().cloned();
    Ok(serde_json::to_value(service.call_tool(params).await?)?)
}

fn parse_endpoints_from_search(_raw: &Value) -> Vec<Endpoint> {
    warn!("parse_endpoints_from_search: stub — no parsing implemented yet");
    Vec::new()
}

/// Write endpoints into the store via SPARQL UPDATE INSERT DATA.
pub fn write_endpoints_sparql(
    store: &Arc<dyn SparqlStore>,
    app_id: &str,
    endpoints: &[Endpoint],
) -> Result<()> {
    let app_iri = format!("{HKB_NS}Application/{}", percent_encode(app_id));

    let mut triples = format!("<{app_iri}> a <{HKB_NS}Application> .\n");

    for (i, ep) in endpoints.iter().enumerate() {
        let ep_id = format!("{app_id}/{}/{i}", ep.method);
        let ep_iri = format!("{HKB_NS}Endpoint/{}", percent_encode(&ep_id));

        triples.push_str(&format!(
            "<{ep_iri}> a <{HKB_NS}Endpoint> ;\n\
                 <{HKB_NS}httpMethod> \"{}\" ;\n\
                 <{HKB_NS}path> \"{}\" .\n\
             <{app_iri}> <{HKB_NS}hasEndpoint> <{ep_iri}> .\n",
            escape_literal(&ep.method),
            escape_literal(&ep.path),
        ));

        for p in &ep.parameters {
            let param_iri = format!(
                "{HKB_NS}Parameter/{}",
                percent_encode(&format!("{ep_id}/{}", p.name))
            );
            triples.push_str(&format!(
                "<{param_iri}> a <{HKB_NS}Parameter> ;\n\
                     <http://www.w3.org/2000/01/rdf-schema#label> \"{}\" .\n\
                 <{ep_iri}> <{HKB_NS}hasParameter> <{param_iri}> .\n",
                escape_literal(&p.name),
            ));
        }
    }

    let sparql = format!("INSERT DATA {{\n{triples}}}");
    store.update(&sparql)?;
    info!(count = endpoints.len(), "wrote endpoints to store");
    Ok(())
}

fn percent_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}

fn escape_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
