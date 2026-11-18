//! `ingest_validate`: run named read-only checks against a staged job's data
//! before it is committed, and record whether they passed.
//!
//! A check is a `SELECT` or `ASK` in which `{{staging}}` and `{{target}}` stand
//! for the job's two graphs. The record (`smj:validationPassed`,
//! `smj:validatedAtBatches`) is what `ingest_commit` with `require_validation`
//! looks at: it is only written if no batch arrived while the checks ran, so a
//! recorded pass always covers exactly the batches that were there.

use std::time::Duration;

use super::*;
use crate::domain::BoundedOutcome;

/// Most checks in one call.
pub const MAX_CHECKS: usize = 16;
/// Largest check query, in bytes.
pub const MAX_CHECK_BYTES: usize = 16 * 1024;
/// Rows returned per check (the evaluation stops reading there).
pub const MAX_ROWS: usize = 20;

#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    pub query: String,
    /// `empty`, `non_empty` (SELECT) or `true`, `false` (ASK). A SELECT
    /// defaults to `empty` (it looks for violations), an ASK to `true`.
    pub expect: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ValidateRequest {
    pub job_id: String,
    pub checks: Vec<Check>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    Empty,
    NonEmpty,
    True,
    False,
}

impl Expect {
    fn name(self) -> &'static str {
        match self {
            Expect::Empty => "empty",
            Expect::NonEmpty => "non_empty",
            Expect::True => "true",
            Expect::False => "false",
        }
    }
}

/// A check that has been parsed and found acceptable.
struct Prepared<'a> {
    check: &'a Check,
    /// The query with `{{staging}}` and `{{target}}` replaced.
    query: String,
    expect: Expect,
}

fn invalid(job: &str, message: String) -> anyhow::Error {
    coded("not_a_query", message, Some(job))
}

/// Parse a check and decide what it expects. Refuses anything but a `SELECT`
/// or `ASK` without `SERVICE`.
fn prepare<'a>(job: &str, check: &'a Check, staging: &str, target: &str) -> Result<Prepared<'a>> {
    let name = &check.name;
    let query = check
        .query
        .replace("{{staging}}", staging)
        .replace("{{target}}", target);
    let parsed = spargebra::SparqlParser::new()
        .parse_query(&query)
        .map_err(|e| invalid(job, format!("check '{name}' is not a valid query: {e}")))?;
    // Display renders every SERVICE (also inside FILTER EXISTS). A string
    // literal that happens to contain the word is refused too: over-refusing
    // is the safe side here.
    if parsed.to_string().contains("SERVICE") {
        return Err(invalid(
            job,
            format!("check '{name}' uses SERVICE, which is not allowed"),
        ));
    }
    let is_ask = match parsed {
        spargebra::Query::Select { .. } => false,
        spargebra::Query::Ask { .. } => true,
        _ => {
            return Err(invalid(
                job,
                format!("check '{name}' must be a SELECT or an ASK"),
            ))
        }
    };
    let expect = match (check.expect.as_deref(), is_ask) {
        (None, false) | (Some("empty"), false) => Expect::Empty,
        (Some("non_empty"), false) => Expect::NonEmpty,
        (None, true) | (Some("true"), true) => Expect::True,
        (Some("false"), true) => Expect::False,
        (Some(other), is_ask) => {
            return Err(invalid(
                job,
                format!(
                    "check '{name}': expect '{other}' does not fit {}",
                    if is_ask {
                        "an ASK (use true or false)"
                    } else {
                        "a SELECT (use empty or non_empty)"
                    }
                ),
            ))
        }
    };
    Ok(Prepared {
        check,
        query,
        expect,
    })
}

impl IngestService {
    /// Run the checks of `req` against the job's staging graph.
    pub fn validate(&self, req: &ValidateRequest) -> Result<Value> {
        let id = req.job_id.as_str();
        if req.checks.is_empty() || req.checks.len() > MAX_CHECKS {
            return Err(ToolError::LimitExceeded {
                limit_key: "checks per ingest_validate call".into(),
                limit: MAX_CHECKS as u64,
                value: req.checks.len() as u64,
            }
            .into_anyhow());
        }
        for c in &req.checks {
            if c.query.len() > MAX_CHECK_BYTES {
                return Err(ToolError::LimitExceeded {
                    limit_key: format!("bytes of check '{}'", c.name),
                    limit: MAX_CHECK_BYTES as u64,
                    value: c.query.len() as u64,
                }
                .into_anyhow());
            }
        }
        let store: &dyn SparqlStore = self.privileged.as_ref();
        let Some(m) = self.manifest(store, id)? else {
            return Err(coded("job_not_found", format!("no job {id}"), Some(id)));
        };
        let state = m.get("state").and_then(|s| JobState::from_iri(s));
        if state != Some(JobState::Open) {
            return Err(job_not_open(id, state));
        }
        if m.get("mode").and_then(|s| Mode::parse(s)) == Some(Mode::Direct) {
            return Err(coded(
                "wrong_mode",
                "a direct job writes to the target as it goes; there is no staged data to validate",
                Some(id),
            ));
        }
        let (batches, _) = self.totals(&m);
        let staging = format!("<{}>", m.get("staging").cloned().unwrap_or_default());
        let target = format!("<{}>", m.get("target").cloned().unwrap_or_default());
        let timeout = Duration::from_secs(self.cfg.validate_timeout_secs.max(1));
        // Refuse every bad check before running any.
        let prepared = req
            .checks
            .iter()
            .map(|c| prepare(id, c, &staging, &target))
            .collect::<Result<Vec<_>>>()?;

        let mut all_passed = true;
        let mut results = Vec::new();
        for p in &prepared {
            let outcome = store.query_bounded(&p.query, MAX_ROWS, timeout)?;
            let (passed, detail) = match outcome {
                BoundedOutcome::TimedOut => {
                    return Err(coded(
                        "validate_timeout",
                        format!(
                            "check '{}' did not finish within {} s; nothing was recorded",
                            p.check.name,
                            timeout.as_secs()
                        ),
                        Some(id),
                    ))
                }
                BoundedOutcome::Answer(b) => {
                    (b == (p.expect == Expect::True), json!({ "answer": b }))
                }
                BoundedOutcome::Rows {
                    rows, truncated, ..
                } => {
                    let non_empty = !rows.is_empty();
                    let shown: Vec<Value> = rows
                        .iter()
                        .map(|r| {
                            let mut o = serde_json::Map::new();
                            for (k, v) in r {
                                o.insert(k.clone(), json!(v.to_ntriples()));
                            }
                            Value::Object(o)
                        })
                        .collect();
                    (
                        non_empty == (p.expect == Expect::NonEmpty),
                        json!({ "rows": shown, "truncated": truncated }),
                    )
                }
            };
            all_passed &= passed;
            let mut entry = json!({
                "name": p.check.name,
                "passed": passed,
                "expect": p.expect.name(),
            });
            if let (Some(e), Some(d)) = (entry.as_object_mut(), detail.as_object()) {
                e.extend(d.clone());
            }
            results.push(entry);
        }

        let recorded = self.record_validation(id, batches, all_passed)?;
        self.touch(id);
        Ok(json!({
            "job_id": id,
            "passed": all_passed,
            "batches_validated": batches,
            "recorded": recorded,
            "results": results,
        }))
    }

    /// Store the outcome, unless a batch arrived (or the job closed) while the
    /// checks ran: a record must describe exactly the data that was checked.
    fn record_validation(&self, id: &str, batches: u64, passed: bool) -> Result<bool> {
        let job = job_iri(id);
        let (p_passed, p_at) = (smj("validationPassed"), smj("validatedAtBatches"));
        self.privileged
            .exclusive("ingest_validate", |s| -> Result<bool> {
                let Some(m) = self.manifest(s.as_ref(), id)? else {
                    return Ok(false);
                };
                let open = m.get("state").and_then(|x| JobState::from_iri(x)) == Some(JobState::Open);
                if !open || self.totals(&m).0 != batches {
                    return Ok(false);
                }
                s.update(&format!(
                    "DELETE WHERE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {p_passed} ?o }} }} ; \
                     DELETE WHERE {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {p_at} ?o }} }} ; \
                     INSERT DATA {{ GRAPH <{JOBS_GRAPH}> {{ <{job}> {p_passed} {} ; {p_at} {} }} }}",
                    typed(if passed { "true" } else { "false" }, "boolean"),
                    typed(&batches.to_string(), "integer"),
                ))?;
                Ok(true)
            })?
    }
}
