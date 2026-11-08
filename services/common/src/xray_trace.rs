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
//! Beside the readings, this module MINTS the two ids that form takes
//! ([`mint_trace_id`], [`mint_span_id`]) and builds the header ONE HOP of a
//! request chain stamps on the call it makes ([`hop_tracing_header`]) — the
//! propagation every service that calls another on a caller's behalf performs, so
//! API Gateway handing a call to Lambda and Lambda handing the context to a
//! handler do not disagree about what a continued chain looks like.
//!
//! Every reading is a pure function of the text, so the same text is judged the
//! same way wherever it arrives; the two minters take their randomness from the
//! same maintained generator the rest of this crate mints identifiers with, and
//! the creation instant of a trace id from the caller rather than from a clock
//! read here.

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

/// The field of a tracing header that holds the span the NEXT hop's work is a child
/// of: "Parent – the segment ID of the request's parent segment".
const PARENT_FIELD: &str = "Parent";

/// The field of a tracing header that holds the sampling decision: "Sampled – the
/// sampling decision", `1` for a request being recorded and `0` for one that is not.
const SAMPLED_FIELD: &str = "Sampled";

/// How many hexadecimal digits a SPAN id carries: a segment id is "a 64-bit identifier
/// … unique among segments in the same trace, in 16 hexadecimal digits".
const SPAN_ID_DIGITS: usize = 16;

/// The `Sampled` value of a request being recorded, and of one that is not.
const SAMPLED_YES: &str = "1";
const SAMPLED_NO: &str = "0";

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

/// The value `text` carries in the field called `name`, reading each field on its own.
///
/// Unlike [`trace_root`] this does NOT require the whole text to be a well-formed
/// tracing header: a hop that is handed a header one field of which is malformed still
/// has to read the fields that are not, because what a tracer put beside the root is
/// the tracer's business. A field with no `=`, or a name `text` carries no field for,
/// gives nothing.
#[must_use]
pub fn trace_field<'t>(text: &'t str, name: &str) -> Option<&'t str> {
    text.split(FIELD_SEPARATOR)
        .filter_map(|field| field.split_once('='))
        .find(|(field, _)| *field == name)
        .map(|(_, value)| value)
}

/// Whether `value` is a well-formed SPAN id: [`SPAN_ID_DIGITS`] hexadecimal digits.
#[must_use]
pub fn is_span_id(value: &str) -> bool {
    value.len() == SPAN_ID_DIGITS && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The trace id `text` continues: the [`ROOT_FIELD`] field's value when that value IS a
/// trace id ([`is_trace_id`]), and `None` otherwise.
///
/// This is the question a hop asks of a header a CLIENT supplied: a chain is continued
/// when the client named one in the form X-Ray gives, and started afresh when it named
/// none or named something that is not a trace id at all — so a malformed value is
/// replaced rather than refused.
#[must_use]
pub fn continued_root(text: &str) -> Option<&str> {
    trace_field(text, ROOT_FIELD).filter(|value| is_trace_id(value))
}

/// The SPAN `text` names as the parent of the work it was handed to, when that value IS a
/// span id ([`is_span_id`]), and `None` otherwise.
///
/// This is the question a hop asks of a header a CALLER supplied: the caller named the span of
/// its own work where it named one, and that span is what the segment this hop records hangs
/// from. Text naming no span, or naming something that is not a span id, names no parent — so
/// the segment this hop records is the root of its trace.
#[must_use]
pub fn parent_span(text: &str) -> Option<&str> {
    trace_field(text, PARENT_FIELD).filter(|value| is_span_id(value))
}

/// The sampling decision `text` carries in its [`SAMPLED_FIELD`] field: `Some(true)` for
/// a request being recorded, `Some(false)` for one that is not, and `None` where the
/// header took no decision this reading recognises.
#[must_use]
pub fn sampling_decision(text: &str) -> Option<bool> {
    match trace_field(text, SAMPLED_FIELD)? {
        SAMPLED_YES => Some(true),
        SAMPLED_NO => Some(false),
        _ => None,
    }
}

/// A fresh trace id for a chain starting at `created`: `1-<8 hex>-<24 hex>`, the version,
/// "the time of the original request, in Unix epoch time, in 8 hexadecimal digits" and "a
/// 96-bit identifier for the trace, globally unique, in 24 hexadecimal digits".
///
/// The time is the instant the caller gives, never one read here, so the id a hop mints
/// carries the instant that hop is serving. The unique part is 96 opaque bits from the
/// same maintained generator the rest of this crate mints identifiers with.
#[must_use]
pub fn mint_trace_id(created: std::time::SystemTime) -> String {
    let seconds = created
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    // The time part is 8 hexadecimal digits wide, which is what a 32-bit epoch second
    // occupies; a clock past that width contributes its low 32 bits rather than a wider
    // field the form has no room for.
    let truncated = u32::try_from(seconds & u64::from(u32::MAX)).unwrap_or(u32::MAX);
    let unique = ::uuid::Uuid::new_v4().as_u128() & ((1u128 << 96) - 1);
    format!("{TRACE_ID_VERSION}-{truncated:08x}-{unique:024x}")
}

/// A fresh SPAN id: [`SPAN_ID_DIGITS`] hexadecimal digits of 64 opaque bits, which is the
/// form "a 64-bit identifier for the segment, unique among segments in the same trace"
/// takes on the wire.
#[must_use]
pub fn mint_span_id() -> String {
    let span = ::uuid::Uuid::new_v4().as_u128() as u64;
    format!("{span:016x}")
}

/// The tracing header ONE HOP of a request chain stamps on the call it makes onward, when
/// that hop RECORDS work of its own.
///
/// Three things are decided, and each is decided the same way wherever a hop performs
/// this propagation:
///
/// * the CHAIN is the one the client named, when it named one in X-Ray's own form
///   ([`continued_root`]), and a fresh chain starting `at` otherwise — so a client that
///   supplied no header, or supplied a root that is not a trace id, has the hop start the
///   chain rather than fail the call;
/// * the [`PARENT_FIELD`] is `span`, the id of the hop's OWN work, because that work is
///   what the next hop's work is a child of;
/// * the DECISION is the client's when it took one ([`sampling_decision`]), and otherwise
///   this hop's: a hop that records its own work records this request. So a request-level
///   decision a client made travels the whole chain, and a hop decides only for a request
///   that arrived undecided.
///
/// A hop that makes SEVERAL calls onward for one request decides the chain and the decision
/// once and then names a span per call: it builds each header with
/// [`stamped_tracing_header`], which is what this reading is built on, rather than calling
/// this one per call and starting a chain per call.
#[must_use]
pub fn hop_tracing_header(supplied: Option<&str>, span: &str, at: std::time::SystemTime) -> String {
    let root = supplied
        .and_then(continued_root)
        .map(str::to_string)
        .unwrap_or_else(|| mint_trace_id(at));
    let sampled = supplied.and_then(sampling_decision).unwrap_or(true);
    stamped_tracing_header(&root, span, sampled)
}

/// The tracing header for a chain, a span and a decision a hop has ALREADY decided — the one
/// rendering of the three fields, in the order X-Ray writes them.
#[must_use]
pub fn stamped_tracing_header(root: &str, span: &str, sampled: bool) -> String {
    let decision = if sampled { SAMPLED_YES } else { SAMPLED_NO };
    format!("{ROOT_FIELD}={root};{PARENT_FIELD}={span};{SAMPLED_FIELD}={decision}")
}

/// The tracing header the call a hop makes onward carries, whether or not that hop records
/// work of its own.
///
/// `span` is the id of the hop's own work where the hop RECORDS it, and `None` where it
/// records nothing. A recording hop stamps the header ([`hop_tracing_header`]); a hop that
/// records nothing has no span to name as a parent and no decision to take, so it passes the
/// client's own header on as written — and a call that arrived with no chain named carries
/// none onward either.
#[must_use]
pub fn onward_tracing_header(
    supplied: Option<&str>,
    span: Option<&str>,
    at: std::time::SystemTime,
) -> Option<String> {
    match span {
        Some(span) => Some(hop_tracing_header(supplied, span, at)),
        None => supplied.and_then(tracing_header).map(str::to_string),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{
        continued_root, hop_tracing_header, is_span_id, is_trace_id, mint_span_id, mint_trace_id,
        onward_tracing_header, parent_span, sampling_decision, trace_field, trace_root,
        tracing_header,
    };

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

    /// A field of a tracing header is read on its OWN — over a TABLE whose rows differ
    /// along which field is asked for, where it sits, and what else the text carries.
    #[test]
    fn a_field_of_a_tracing_header_is_read_on_its_own() {
        let root = "1-5759e988-bd862e3fe1be46a994272793";
        let rows: Vec<(&str, String, &str, Option<&str>)> = vec![
            ("the root of a sampled header", format!("Root={root};Sampled=1"), "Root", Some(root)),
            (
                "the parent of a continued header",
                format!("Root={root};Parent=53995c3f42cd8ad8;Sampled=1"),
                "Parent",
                Some("53995c3f42cd8ad8"),
            ),
            (
                "a field beside a root that is NOT a trace id",
                "Root=not-a-trace-id;Sampled=1".to_string(),
                "Sampled",
                Some("1"),
            ),
            (
                "a field the text does not carry",
                format!("Root={root}"),
                "Parent",
                None,
            ),
            ("a field with no value", "Sampled".to_string(), "Sampled", None),
            ("empty text", String::new(), "Root", None),
        ];
        for (case, text, name, expected) in rows {
            let actual = trace_field(&text, name);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {name} of {text:?} to be {expected:?}, got {actual:?}"
            );
        }
    }

    /// Which SPAN a hop reads as the parent of the work it was handed — over a TABLE whose
    /// rows differ along whether the supplied value is a span id, its width and its alphabet,
    /// and whether the header carries the field at all.
    #[test]
    fn a_hop_reads_the_span_its_caller_named_as_the_parent() {
        let root = "1-5759e988-bd862e3fe1be46a994272793";
        let span = "53995c3f42cd8ad8";
        let rows: Vec<(&str, String, Option<&str>)> = vec![
            (
                "a continued header",
                format!("Root={root};Parent={span};Sampled=1"),
                Some(span),
            ),
            ("the parent alone", format!("Parent={span}"), Some(span)),
            (
                "upper-case digits",
                "Parent=53995C3F42CD8AD8".to_string(),
                Some("53995C3F42CD8AD8"),
            ),
            (
                "a span one digit short",
                format!("Root={root};Parent=53995c3f42cd8ad"),
                None,
            ),
            (
                "a span one digit long",
                format!("Root={root};Parent=53995c3f42cd8ad8a"),
                None,
            ),
            (
                "a span that is not hexadecimal",
                format!("Root={root};Parent=zzzzzzzzzzzzzzzz"),
                None,
            ),
            ("no parent field at all", format!("Root={root};Sampled=1"), None),
            ("empty text", String::new(), None),
        ];
        for (case, text, expected) in rows {
            let actual = parent_span(&text);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the parent of {text:?} to be {expected:?}, got \
                 {actual:?}"
            );
        }
    }

    /// Which chain a hop CONTINUES, and which sampling decision it inherits — over a
    /// TABLE whose rows differ along whether the supplied root is a trace id, whether the
    /// rest of the text is well formed, and which decision the client took.
    #[test]
    fn a_hop_continues_a_supplied_root_and_inherits_a_supplied_decision() {
        let root = "1-5759e988-bd862e3fe1be46a994272793";
        let rows: Vec<(&str, String, Option<&str>, Option<bool>)> = vec![
            ("a sampled header", format!("Root={root};Sampled=1"), Some(root), Some(true)),
            ("an unsampled header", format!("Root={root};Sampled=0"), Some(root), Some(false)),
            ("the root alone", format!("Root={root}"), Some(root), None),
            (
                "a root that is not a trace id, beside a decision",
                "Root=not-a-trace-id;Sampled=1".to_string(),
                None,
                Some(true),
            ),
            ("no root at all", "Sampled=0".to_string(), None, Some(false)),
            (
                "a decision this reading does not recognise",
                format!("Root={root};Sampled=yes"),
                Some(root),
                None,
            ),
            ("empty text", String::new(), None, None),
        ];
        for (case, text, chain, decision) in rows {
            let continued = continued_root(&text);
            assert_eq!(
                continued, chain,
                "case {case:?}: expected {text:?} to continue {chain:?}, got {continued:?}"
            );
            let sampled = sampling_decision(&text);
            assert_eq!(
                sampled, decision,
                "case {case:?}: expected the decision of {text:?} to be {decision:?}, got \
                 {sampled:?}"
            );
        }
    }

    /// A minted trace id carries the instant it was minted for and is a trace id, and a
    /// minted span id is a span id — over a TABLE of instants whose rows differ along
    /// where each sits relative to the 8-digit epoch field (the epoch itself, a recorded
    /// instant, and one past the width that field has room for).
    #[test]
    fn a_minted_id_has_the_form_x_ray_gives_it() {
        let rows: Vec<(&str, SystemTime)> = vec![
            ("the epoch itself", UNIX_EPOCH),
            (
                "a recorded instant",
                UNIX_EPOCH + Duration::from_secs(0x5759_e988),
            ),
            (
                "the last instant the 8-digit field holds",
                UNIX_EPOCH + Duration::from_secs(u64::from(u32::MAX)),
            ),
            (
                "an instant past that width",
                UNIX_EPOCH + Duration::from_secs(u64::from(u32::MAX) + 1),
            ),
        ];
        for (case, at) in rows {
            let minted = mint_trace_id(at);
            assert!(
                is_trace_id(&minted),
                "case {case:?}: expected a minted trace id, got {minted:?}"
            );
            let seconds = at
                .duration_since(UNIX_EPOCH)
                .expect("an instant after the epoch")
                .as_secs();
            let expected = format!("{:08x}", seconds & u64::from(u32::MAX));
            let time = minted.split('-').nth(1).unwrap_or_default();
            assert_eq!(
                time, expected,
                "case {case:?}: expected the time part of {minted:?} to be {expected:?}, got \
                 {time:?}"
            );
            let span = mint_span_id();
            assert!(
                is_span_id(&span),
                "case {case:?}: expected a minted span id, got {span:?}"
            );
            assert_ne!(
                span,
                mint_span_id(),
                "case {case:?}: two minted span ids name two spans"
            );
        }
    }

    /// What a hop puts on the call it makes onward — over a TABLE whose rows differ along
    /// what the client supplied (a sampled chain, an unsampled one, a chain with no
    /// decision, a malformed root, no header at all) and whether the hop RECORDS work of
    /// its own.
    #[test]
    fn a_hop_stamps_the_chain_its_own_span_and_the_decision() {
        let root = "1-5759e988-bd862e3fe1be46a994272793";
        let span = "53995c3f42cd8ad8";
        let at = UNIX_EPOCH + Duration::from_secs(0x5759_e988);
        // (case, what the client supplied, the chain the header must name, the decision)
        let rows: Vec<(&str, Option<String>, Option<&str>, &str)> = vec![
            (
                "a sampled chain the client named",
                Some(format!("Root={root};Sampled=1")),
                Some(root),
                "1",
            ),
            (
                "an unsampled chain the client named",
                Some(format!("Root={root};Sampled=0")),
                Some(root),
                "0",
            ),
            (
                "a chain the client named with no decision",
                Some(format!("Root={root}")),
                Some(root),
                "1",
            ),
            (
                "a root that is not a trace id",
                Some("Root=not-a-trace-id;Sampled=1".to_string()),
                None,
                "1",
            ),
            ("no header at all", None, None, "1"),
        ];
        for (case, supplied, chain, decision) in rows {
            let stamped = hop_tracing_header(supplied.as_deref(), span, at);
            // A hop that records its own work takes the same decisions through the
            // whole-propagation reading, which is what every caller of it uses: the same
            // span, the same decision, and the same chain where the client named one (a
            // chain the hop STARTS is a fresh one each time, so only its form is the same).
            let onward = onward_tracing_header(supplied.as_deref(), Some(span), at)
                .unwrap_or_else(|| panic!("case {case:?}: a recording hop propagates a header"));
            for field in ["Parent", "Sampled"] {
                assert_eq!(
                    trace_field(&onward, field),
                    trace_field(&stamped, field),
                    "case {case:?}: a recording hop stamps the same {field} either way"
                );
            }
            assert!(
                is_trace_id(trace_field(&onward, "Root").unwrap_or_default()),
                "case {case:?}: a recording hop names a chain of X-Ray's form, got {onward:?}"
            );
            let fields: Vec<&str> = stamped.split(';').collect();
            assert_eq!(
                fields.len(),
                3,
                "case {case:?}: expected a Root, a Parent and a Sampled field, got {stamped:?}"
            );
            let stamped_root = trace_field(&stamped, "Root").unwrap_or_default();
            assert!(
                is_trace_id(stamped_root),
                "case {case:?}: expected a trace id as the Root of {stamped:?}, got \
                 {stamped_root:?}"
            );
            if let Some(continued) = chain {
                assert_eq!(
                    stamped_root, continued,
                    "case {case:?}: expected {stamped:?} to continue {continued:?}, got \
                     {stamped_root:?}"
                );
            } else {
                let time = stamped_root.split('-').nth(1).unwrap_or_default();
                assert_eq!(
                    time, "5759e988",
                    "case {case:?}: expected a chain minted for {at:?}, got {stamped_root:?}"
                );
            }
            assert_eq!(
                trace_field(&stamped, "Parent"),
                Some(span),
                "case {case:?}: expected the hop's own span as the Parent of {stamped:?}"
            );
            assert_eq!(
                trace_field(&stamped, "Sampled"),
                Some(decision),
                "case {case:?}: expected the decision {decision:?} in {stamped:?}"
            );
            // A hop that records NOTHING of its own has no span to name and no decision to
            // take: the client's own header travels on as written, and a call that named no
            // chain carries none onward.
            assert_eq!(
                onward_tracing_header(supplied.as_deref(), None, at),
                supplied.as_deref().and_then(tracing_header).map(str::to_string),
                "case {case:?}: a hop that records nothing passes the client's header on as \
                 written"
            );
        }
    }
}
