// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of AWS X-Ray's TRACE ID and TRACING HEADER, for every service
//! that is handed one as a caller-supplied string.
//!
//! Two services take them, and must not disagree about what counts as one:
//!
//! * X-Ray itself indexes a stored trace under its `trace_id` and refuses a read
//!   naming anything that is not one;
//! * EventBridge takes an entry's `TraceHeader` — "an X-Ray trace header, which
//!   is an http header (`X-Amzn-Trace-Id`) that contains the trace-id associated
//!   with the event" — and carries it to the targets the event reaches.
//!
//! The forms are X-Ray's own. A trace id is `<version>-<time>-<unique>`: the
//! version, the time the id was created in 8 hexadecimal digits, and a 96-bit
//! random number in 24 hexadecimal digits. A tracing header is a `;`-separated
//! list of `name=value` fields of which `Root` holds that id, with any other
//! field beside it.
//!
//! Nothing here reaches the clock, the network or any store: both readings are
//! pure functions of the text, so the same text is judged the same way wherever
//! it arrives.

/// The version every trace id this module recognises begins with.
const TRACE_ID_VERSION: &str = "1";

/// How many hexadecimal digits a trace id's creation time carries.
const TRACE_ID_TIME_DIGITS: usize = 8;

/// How many hexadecimal digits a trace id's random part carries (96 bits).
const TRACE_ID_UNIQUE_DIGITS: usize = 24;

/// What separates the fields of a tracing header.
const FIELD_SEPARATOR: char = ';';

/// The field of a tracing header that holds the trace id.
const ROOT_FIELD: &str = "Root";

/// Whether `value` is a well-formed trace id: `1-<8 hex>-<24 hex>`, which
/// occupies exactly the 35 characters X-Ray's `TraceId` is modelled at.
#[must_use]
pub fn is_trace_id(value: &str) -> bool {
    let mut fields = value.split('-');
    let (Some(TRACE_ID_VERSION), Some(time), Some(unique), None) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    let hex = |field: &str, digits: usize| {
        field.len() == digits && field.bytes().all(|byte| byte.is_ascii_hexdigit())
    };
    hex(time, TRACE_ID_TIME_DIGITS) && hex(unique, TRACE_ID_UNIQUE_DIGITS)
}

/// `text` when it IS a tracing header, and `None` when it is not.
///
/// A tracing header is a list of `name=value` fields separated by
/// [`FIELD_SEPARATOR`], one of which is `Root` and holds a trace id
/// ([`is_trace_id`]); every other field is carried as written, because what a
/// field beside the root means is the tracer's business and not this reading's.
/// Text carrying no field, a field with no name, or a `Root` that is not a trace
/// id names no trace at all.
#[must_use]
pub fn tracing_header(text: &str) -> Option<&str> {
    let mut names_a_trace = false;
    for field in text.split(FIELD_SEPARATOR) {
        let (name, value) = field.split_once('=')?;
        if name.is_empty() {
            return None;
        }
        if name == ROOT_FIELD {
            if !is_trace_id(value) {
                return None;
            }
            names_a_trace = true;
        }
    }
    names_a_trace.then_some(text)
}

/// The trace id the tracing header `text` carries in its [`ROOT_FIELD`] field, when
/// `text` IS a tracing header ([`tracing_header`]).
///
/// That id is what every hop of ONE request chain carries — "the trace-id associated
/// with" the request — so it is how a service recognises two calls as belonging to the
/// same chain. Text that names no trace carries no root, and so names no chain.
#[must_use]
pub fn trace_root(text: &str) -> Option<&str> {
    tracing_header(text)?
        .split(FIELD_SEPARATOR)
        .filter_map(|field| field.split_once('='))
        .find(|(name, _)| *name == ROOT_FIELD)
        .map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    use super::{is_trace_id, trace_root, tracing_header};

    /// Which text is a trace id — over a TABLE whose rows differ along the
    /// version, the width of each part, the alphabet of each part, and how many
    /// parts the text carries.
    #[test]
    fn a_trace_id_is_a_version_a_time_and_a_unique_part() {
        let rows = [
            ("the documented form", "1-5759e988-bd862e3fe1be46a994272793", true),
            ("upper-case digits", "1-5759E988-BD862E3FE1BE46A994272793", true),
            ("all zeroes", "1-00000000-000000000000000000000000", true),
            ("another version", "2-5759e988-bd862e3fe1be46a994272793", false),
            ("no version", "5759e988-bd862e3fe1be46a994272793", false),
            ("a short time part", "1-5759e98-bd862e3fe1be46a994272793", false),
            ("a long time part", "1-5759e9881-bd862e3fe1be46a994272793", false),
            ("a short unique part", "1-5759e988-bd862e3fe1be46a9942727", false),
            ("a long unique part", "1-5759e988-bd862e3fe1be46a9942727931", false),
            ("a non-hexadecimal part", "1-5759e988-zzzzzzzzzzzzzzzzzzzzzzzz", false),
            ("a fourth part", "1-5759e988-bd862e3fe1be46a994272793-x", false),
            ("no parts at all", "1", false),
            ("empty text", "", false),
        ];
        for (case, value, expected) in rows {
            let actual = is_trace_id(value);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {value:?} to be a trace id={expected}, got {actual}"
            );
        }
    }

    /// Which text is a tracing header — over a TABLE whose rows differ along
    /// which fields the header carries, their order, and every shape that names
    /// no trace.
    #[test]
    fn a_tracing_header_carries_a_root_field_holding_a_trace_id() {
        let root = "1-5759e988-bd862e3fe1be46a994272793";
        let rows: Vec<(&str, String, bool)> = vec![
            ("the root alone", format!("Root={root}"), true),
            ("a sampled header", format!("Root={root};Sampled=1"), true),
            (
                "a parent segment beside it",
                format!("Root={root};Parent=53995c3f42cd8ad8"),
                true,
            ),
            (
                "a field of the tracer's own",
                format!("Root={root};Tenant=alpha"),
                true,
            ),
            (
                "the root after another field",
                format!("Sampled=1;Root={root}"),
                true,
            ),
            ("no field at all", "not-a-trace-header".to_string(), false),
            ("no root field", "Sampled=1".to_string(), false),
            ("a root that is not a trace id", "Root=whatever".to_string(), false),
            ("a field with no name", format!("=1;Root={root}"), false),
            (
                "a field that is not name=value",
                format!("Root={root};Sampled"),
                false,
            ),
            ("empty text", String::new(), false),
        ];
        for (case, text, expected) in rows {
            let actual = tracing_header(&text);
            assert_eq!(
                actual.is_some(),
                expected,
                "case {case:?}: expected {text:?} to be a tracing header={expected}, \
                 got {actual:?}"
            );
            if expected {
                assert_eq!(
                    actual,
                    Some(text.as_str()),
                    "case {case:?}: a tracing header is carried as written"
                );
            }
        }
    }

    /// Which trace id a tracing header names its chain by — over a TABLE whose rows
    /// differ along where the root field sits, which fields sit beside it, and every
    /// shape that names no chain at all.
    #[test]
    fn the_root_field_is_the_chain_a_tracing_header_names() {
        let root = "1-5759e988-bd862e3fe1be46a994272793";
        let other = "1-00000000-000000000000000000000000";
        let rows: Vec<(&str, String, Option<&str>)> = vec![
            ("the root alone", format!("Root={root}"), Some(root)),
            ("a sampled header", format!("Root={root};Sampled=1"), Some(root)),
            (
                "the root after another field",
                format!("Sampled=1;Parent=53995c3f42cd8ad8;Root={other}"),
                Some(other),
            ),
            (
                "a field of the tracer's own beside it",
                format!("Root={root};Lineage=1:11fb77a2:0"),
                Some(root),
            ),
            ("no root field", "Sampled=1".to_string(), None),
            ("a root that is not a trace id", "Root=whatever".to_string(), None),
            ("no field at all", "not-a-trace-header".to_string(), None),
            ("empty text", String::new(), None),
        ];
        for (case, text, expected) in rows {
            let actual = trace_root(&text);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the chain of {text:?} to be {expected:?}, got \
                 {actual:?}"
            );
        }
    }
}
