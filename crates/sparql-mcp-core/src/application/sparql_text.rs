//! Building SPARQL text safely.
//!
//! Anything that comes from outside (a label, a graph IRI, a job id) must go
//! through these two functions before it is put in a query or update. Never
//! format raw input into SPARQL.

/// A quoted SPARQL string literal: `"…"` with `\`, `"` and control characters
/// escaped.
pub fn string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// An IRI reference `<…>`, or `None` when `iri` could not be written as one:
/// empty, or containing a character SPARQL forbids inside `<>` (space,
/// control characters, `< > " { } | \ ^` and the backtick).
pub fn iri(iri: &str) -> Option<String> {
    let bad = |c: char| {
        c.is_control()
            || c == ' '
            || matches!(c, '<' | '>' | '"' | '{' | '}' | '|' | '\\' | '^' | '`')
    };
    if iri.is_empty() || iri.chars().any(bad) {
        None
    } else {
        Some(format!("<{iri}>"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_literals_escape_what_could_break_out() {
        assert_eq!(string_literal("plain"), "\"plain\"");
        assert_eq!(string_literal("a\"b"), "\"a\\\"b\"");
        assert_eq!(string_literal("a\\b"), "\"a\\\\b\"");
        assert_eq!(string_literal("l1\nl2\tx\r"), "\"l1\\nl2\\tx\\r\"");
        assert_eq!(string_literal("\u{1}"), "\"\\u0001\"");
        // Non-ASCII passes through.
        assert_eq!(string_literal("é→"), "\"é→\"");
    }

    #[test]
    fn an_injection_attempt_stays_inside_the_literal() {
        let lit = string_literal("x\" . } ; DROP ALL ; INSERT DATA { <a> <b> \"");
        // Between the outer quotes, every `"` is escaped.
        let inner = &lit[1..lit.len() - 1];
        let mut escaped = true;
        let mut prev = ' ';
        for c in inner.chars() {
            if c == '"' && prev != '\\' {
                escaped = false;
            }
            prev = c;
        }
        assert!(escaped, "{lit}");
    }

    #[test]
    fn iris_reject_what_cannot_be_written_inside_angle_brackets() {
        assert_eq!(iri("urn:project:p").as_deref(), Some("<urn:project:p>"));
        for bad in [
            "", "a b", "a>b", "a<b", "a\"b", "a{b", "a\\b", "a\nb", "a`b",
        ] {
            assert!(iri(bad).is_none(), "{bad:?}");
        }
    }
}
