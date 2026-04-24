use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub root_path: String,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: i64,
    pub label: String,
    pub name: String,
    pub qualified_name: String,
    pub file_path: String,
    pub start_line: i32,
    pub end_line: i32,
    pub properties: NodeProperties,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeProperties {
    pub complexity: Option<i32>,
    pub lines: Option<i32>,
    pub is_exported: Option<bool>,
    pub is_test: Option<bool>,
    pub is_entry_point: Option<bool>,
    pub signature: Option<String>,
    pub docstring: Option<String>,
    pub extension: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub source_id: i64,
    pub target_id: i64,
    pub edge_type: String,
}

#[derive(Debug, Clone)]
pub struct KnowledgeGraph {
    pub project: Project,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}
