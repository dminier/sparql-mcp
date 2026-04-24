use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use std::path::Path;

use super::model::*;

pub fn load_graph(db_path: &Path, project_name: Option<&str>) -> Result<KnowledgeGraph> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("cannot open CBM database: {}", db_path.display()))?;

    let name = match project_name {
        Some(n) => n.to_string(),
        None => auto_detect_project(&conn)?,
    };

    let project = conn
        .query_row(
            "SELECT name, root_path FROM projects WHERE name = ?1",
            params![name],
            |row| {
                Ok(Project {
                    name: row.get(0)?,
                    root_path: row.get(1)?,
                })
            },
        )
        .with_context(|| format!("project '{name}' not found in database"))?;

    let mut stmt = conn.prepare(
        "SELECT id, label, name, qualified_name, file_path, start_line, end_line, properties \
         FROM nodes WHERE project = ?1",
    )?;
    let nodes: Vec<Node> = stmt
        .query_map(params![name], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i32>(5)?,
                row.get::<_, i32>(6)?,
                row.get::<_, String>(7)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .map(|(id, label, name, qn, fp, sl, el, props_json)| {
            let properties: NodeProperties = serde_json::from_str(&props_json).unwrap_or_default();
            Node {
                id,
                label,
                name,
                qualified_name: qn,
                file_path: fp,
                start_line: sl,
                end_line: el,
                properties,
            }
        })
        .collect();

    let mut stmt =
        conn.prepare("SELECT source_id, target_id, type FROM edges WHERE project = ?1")?;
    let edges: Vec<Edge> = stmt
        .query_map(params![name], |row| {
            Ok(Edge {
                source_id: row.get(0)?,
                target_id: row.get(1)?,
                edge_type: row.get(2)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(KnowledgeGraph {
        project,
        nodes,
        edges,
    })
}

pub fn list_projects(db_path: &Path) -> Result<Vec<String>> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("cannot open CBM database: {}", db_path.display()))?;
    let mut stmt = conn.prepare("SELECT name FROM projects ORDER BY name")?;
    let names: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(names)
}

fn auto_detect_project(conn: &Connection) -> Result<String> {
    let mut stmt = conn.prepare("SELECT name FROM projects")?;
    let names: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    match names.len() {
        0 => bail!("no projects found in database"),
        1 => Ok(names.into_iter().next().unwrap()),
        _ => bail!(
            "multiple projects in DB: {}. Use --cbm-project",
            names.join(", ")
        ),
    }
}
