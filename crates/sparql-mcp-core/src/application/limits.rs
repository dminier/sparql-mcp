//! Typed refusals raised below the tool layer.
//!
//! Tools map store errors to `McpError`, which would turn a refusal such as
//! `write_busy` into a JSON-RPC error string. [`ToolError`] keeps refusals
//! structured: the store layer raises one through [`ToolError::into_anyhow`],
//! and the dispatcher turns it into an `isError: true` tool result with a JSON
//! payload (see `docs/designs/sparql-mcp-write-path-hardening.md`, *Limits*).
//!
//! Tool functions run synchronously on one blocking thread, so the raised
//! error is also recorded in a thread-local slot. That is how it survives the
//! tools' own `map_err(|e| McpError…)` calls without editing each one: the
//! dispatcher calls [`take_captured`] after the tool returns an error.

use std::cell::RefCell;
use std::fmt;

use rmcp::model::{CallToolResult, Content};
use serde_json::{json, Value};

/// A refusal with a stable machine-readable code.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolError {
    LimitExceeded {
        limit_key: String,
        limit: u64,
        value: u64,
    },
    ParseError {
        message: String,
        line: Option<u64>,
        column: Option<u64>,
    },
    GraphProtected {
        graph: String,
    },
    ReservedIri {
        iri: String,
    },
    DestructiveUpdateForbidden,
    LoadForbidden,
    PathOutsideRoots {
        path: String,
    },
    PathRejected {
        path: String,
    },
    /// The write gate was not available within `write_wait_timeout_secs`.
    /// `holder` is the label of the store method that holds it.
    WriteBusy {
        holder: String,
        waited_secs: u64,
    },
    /// A refusal specific to one tool family, with its own stable code (for
    /// example `job_not_found` from the ingestion tools). `job_id` is echoed
    /// in the payload when the refusal is about one job.
    Coded {
        code: &'static str,
        message: String,
        job_id: Option<String>,
    },
}

impl ToolError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::LimitExceeded { .. } => "limit_exceeded",
            Self::ParseError { .. } => "parse_error",
            Self::GraphProtected { .. } => "graph_protected",
            Self::ReservedIri { .. } => "reserved_iri",
            Self::DestructiveUpdateForbidden => "destructive_update_forbidden",
            Self::LoadForbidden => "load_forbidden",
            Self::PathOutsideRoots { .. } => "path_outside_roots",
            Self::PathRejected { .. } => "path_rejected",
            Self::WriteBusy { .. } => "write_busy",
            Self::Coded { code, .. } => code,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::LimitExceeded {
                limit_key,
                limit,
                value,
            } => format!("{limit_key} exceeded: {value} > {limit}"),
            Self::ParseError { message, .. } => message.clone(),
            Self::GraphProtected { graph } => format!("graph <{graph}> is protected"),
            Self::ReservedIri { iri } => format!("IRI <{iri}> is in a reserved namespace"),
            Self::DestructiveUpdateForbidden => {
                "destructive update refused; set [ingest] allow_destructive_updates = true to allow it"
                    .to_string()
            }
            Self::LoadForbidden => "SPARQL LOAD is not allowed".to_string(),
            Self::PathOutsideRoots { path } => {
                format!("path {path} is outside every [ingest] file_roots entry")
            }
            Self::PathRejected { path } => format!("path {path} is not allowed"),
            Self::Coded { message, .. } => message.clone(),
            Self::WriteBusy {
                holder,
                waited_secs,
            } => format!(
                "another write ({holder}) held the write gate for more than {waited_secs} s"
            ),
        }
    }

    /// The error payload: `{"code", "message", …}`.
    pub fn to_json(&self) -> Value {
        let mut v = json!({ "code": self.code(), "message": self.message() });
        let obj = v.as_object_mut().expect("object literal");
        match self {
            Self::LimitExceeded {
                limit_key,
                limit,
                value,
            } => {
                obj.insert("limit_key".into(), json!(limit_key));
                obj.insert("limit".into(), json!(limit));
                obj.insert("value".into(), json!(value));
            }
            Self::ParseError { line, column, .. } => {
                if let Some(line) = line {
                    obj.insert("line".into(), json!(line));
                }
                if let Some(column) = column {
                    obj.insert("column".into(), json!(column));
                }
            }
            Self::WriteBusy { holder, .. } => {
                obj.insert("holder".into(), json!(holder));
            }
            Self::Coded {
                job_id: Some(id), ..
            } => {
                obj.insert("job_id".into(), json!(id));
            }
            _ => {}
        }
        v
    }

    /// The tool result for this refusal: `isError: true` with the JSON payload.
    pub fn to_result(&self) -> CallToolResult {
        CallToolResult::error(vec![Content::text(
            serde_json::to_string_pretty(&self.to_json()).unwrap_or_default(),
        )])
    }

    /// Wrap as an `anyhow::Error` and record it for [`take_captured`].
    pub fn into_anyhow(self) -> anyhow::Error {
        CAPTURED.with(|slot| *slot.borrow_mut() = Some(self.clone()));
        anyhow::Error::new(self)
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for ToolError {}

thread_local! {
    static CAPTURED: RefCell<Option<ToolError>> = const { RefCell::new(None) };
}

/// Refuse from inside a tool: records `error` for the dispatcher (which turns
/// it into an `isError` result with the JSON payload) and returns the
/// `McpError` to propagate with `?`.
pub fn refuse(error: ToolError) -> rmcp::ErrorData {
    let message = error.to_string();
    drop(error.into_anyhow());
    rmcp::ErrorData::invalid_params(message, None)
}

/// Forget any refusal recorded on this thread. Call before running a tool.
pub fn clear_captured() {
    CAPTURED.with(|slot| *slot.borrow_mut() = None);
}

/// Take the refusal recorded on this thread, if any. Call after a tool
/// returned an error.
pub fn take_captured() -> Option<ToolError> {
    CAPTURED.with(|slot| slot.borrow_mut().take())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_busy_payload_names_the_holder() {
        let e = ToolError::WriteBusy {
            holder: "update_with_outcome".into(),
            waited_secs: 30,
        };
        let v = e.to_json();
        assert_eq!(v["code"], "write_busy");
        assert_eq!(v["holder"], "update_with_outcome");
        assert_eq!(e.to_result().is_error, Some(true));
    }

    #[test]
    fn raised_error_is_captured_and_downcastable() {
        clear_captured();
        let err = ToolError::GraphProtected {
            graph: "urn:meta".into(),
        }
        .into_anyhow();
        assert!(err.downcast_ref::<ToolError>().is_some());
        assert_eq!(take_captured().unwrap().code(), "graph_protected");
        assert!(take_captured().is_none());
    }
}
