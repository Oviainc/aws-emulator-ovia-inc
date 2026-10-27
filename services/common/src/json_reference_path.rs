// SPDX-License-Identifier: CC0-1.0

//! The ONE evaluation of a caller-supplied JSON path that has to name a SINGLE
//! value, for every service whose API takes such a path as a string.
//!
//! Several AWS APIs carry one: the JSON path an EventBridge target extracts its
//! Kinesis partition key with, the paths the Amazon States Language reads and
//! writes a state's data through. In each case the path names ONE place in a
//! document, so what a service needs is the value at that place — not a node
//! list, and not a language in which a caller can ask for the whole document
//! several times over.
//!
//! That is why the path is BOUNDED before anything evaluates it. A path is
//! accepted only as a REFERENCE path — `$`, `.name`, `['name']`, `[index]`, at
//! most [`MAX_SEGMENTS`] of them within [`MAX_PATH_BYTES`] — so its cost is its
//! segment count and does NOT grow with the size of the document: a descendant
//! (`..`), a wildcard, a union, a slice or a filter would walk the whole
//! document, or multiply the node list by its own length, and so is refused
//! here rather than evaluated and then rejected. The bound is STRUCTURAL: the
//! engine is handed [`canonical`]'s rendering of the segments this module
//! produced, never the caller's own text, so no syntax the validator does not
//! produce can reach it however the caller's text would otherwise have parsed.
//!
//! The evaluation itself is [`serde_json_path`], the maintained RFC 9535 engine
//! the workspace already depends on — this module restricts WHICH paths are
//! evaluated, it does not interpret the query language itself.
//!
//! NOTE: `services/service-com-amazonaws-sfn-awsstepfunctions/src/state/asl.rs`
//! holds an equivalent span of its own (its `select_path`, with the Amazon States
//! Language's extra `$$` context paths and its own refusal wording). It is to be
//! MOVED onto this module rather than copied again, and nothing new should write
//! another.

use serde_json::Value;
use serde_json_path::JsonPath;

/// The most bytes a reference path this module evaluates may carry.
///
/// A path names a place in a document, so it is SHORT: the API members that
/// carry one are modelled far below this (an EventBridge `PartitionKeyPath` at
/// 256 characters), and this is the bound that holds for a caller that reaches
/// the evaluation another way.
pub const MAX_PATH_BYTES: usize = 1024;

/// The most segments a reference path this module evaluates may carry.
///
/// Each segment is one lookup, so the segment count IS the cost of the
/// evaluation; a path naming a place deeper than this is refused rather than
/// walked.
pub const MAX_SEGMENTS: usize = 32;

/// One segment of a reference path: a member of an object, or an element of an
/// array.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// `.name` or `['name']` — the member `name` of an object.
    Key(String),
    /// `[index]` — the element at `index` of an array.
    Index(usize),
}

/// The segments `path` names, or `None` when `path` is not a reference path this
/// module evaluates: one past [`MAX_PATH_BYTES`] or [`MAX_SEGMENTS`], one not
/// rooted at `$`, and one carrying any selector that does not name a single
/// place (a descendant `..`, a wildcard, a union, a slice, a filter).
#[must_use]
pub fn segments(path: &str) -> Option<Vec<Segment>> {
    if path.len() > MAX_PATH_BYTES {
        return None;
    }
    // Every reference path is rooted at the document itself.
    let mut rest = path.strip_prefix('$')?;
    let mut named: Vec<Segment> = Vec::new();
    while !rest.is_empty() {
        if named.len() >= MAX_SEGMENTS {
            return None;
        }
        if let Some(after) = rest.strip_prefix('[') {
            let (inside, tail) = after.split_once(']')?;
            named.push(bracket_segment(inside)?);
            rest = tail;
            continue;
        }
        // The shorthand form. `..name` leaves an EMPTY name here, which is how a
        // descendant segment is refused: it selects every matching member at
        // every depth, which is a walk of the whole document.
        let after = rest.strip_prefix('.')?;
        let end = after.find(['.', '[']).unwrap_or(after.len());
        let (name, tail) = after.split_at(end);
        if !is_member_name(name) {
            return None;
        }
        named.push(Segment::Key(name.to_string()));
        rest = tail;
    }
    Some(named)
}

/// The segment a `[…]` selector names, or `None` for one that does not name a
/// single place.
fn bracket_segment(inside: &str) -> Option<Segment> {
    if let Some(quoted) = inside
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        // A quoted NAME selector, naming one member. A quote, a backslash or a
        // comma inside it is either an escape this module does not read or a
        // union of several names, so neither is one place.
        if quoted.is_empty() || quoted.contains(['\'', '\\', ',']) {
            return None;
        }
        return Some(Segment::Key(quoted.to_string()));
    }
    // An INDEX selector: digits only, which is what rules out a slice (`:`), a
    // union (`,`), a wildcard, a filter and a negative index (whose place
    // depends on the array's length).
    if inside.is_empty() || !inside.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    inside.parse::<usize>().ok().map(Segment::Index)
}

/// Whether `name` is a member name the shorthand `.name` form may carry.
///
/// Letters, digits, `_` and `-`: the last because an AWS event's own members
/// carry one (`detail-type`), and the canonical rendering below is what lets
/// such a name be evaluated at all.
fn is_member_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_alphanumeric() || character == '_' || character == '-')
}

/// `segments` rendered as the ONE path text the engine is handed.
///
/// Every member becomes a quoted name selector, whatever form it arrived in, so
/// the rendering is unambiguous for every name [`segments`] accepts and carries
/// no syntax beyond a name and an index selector.
#[must_use]
pub fn canonical(segments: &[Segment]) -> String {
    let mut rendered = String::from("$");
    for segment in segments {
        match segment {
            Segment::Key(name) => {
                rendered.push_str("['");
                rendered.push_str(name);
                rendered.push_str("']");
            }
            Segment::Index(index) => {
                rendered.push('[');
                rendered.push_str(&index.to_string());
                rendered.push(']');
            }
        }
    }
    rendered
}

/// The ONE value `path` names in `document`, or `None` when it names no single
/// value: a path this module does not evaluate ([`segments`]), and a place the
/// document does not carry.
#[must_use]
pub fn select<'document>(path: &str, document: &'document Value) -> Option<&'document Value> {
    select_segments(&segments(path)?, document)
}

/// [`select`] for a path whose [`segments`] the caller has already read — the
/// form a caller that judges the path BEFORE it builds the document needs, so a
/// path this module does not evaluate costs nothing but its own length.
#[must_use]
pub fn select_segments<'document>(
    segments: &[Segment],
    document: &'document Value,
) -> Option<&'document Value> {
    let parsed = JsonPath::parse(&canonical(segments)).ok()?;
    let nodes = parsed.query(document);
    // Each accepted segment selects at most one node, so a node list of any
    // other length is defence in depth rather than an expected answer.
    match nodes.len() {
        1 => nodes.first(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{canonical, segments, select, Segment, MAX_PATH_BYTES, MAX_SEGMENTS};

    use serde_json::{json, Value};

    /// The document the rows below select from: the members an event envelope
    /// carries, including a hyphenated one and an array.
    fn document() -> Value {
        json!({
            "id": "e-1",
            "detail-type": "Order Created",
            "detail": { "tenant": "alpha", "n": 7, "o": { "a": [1, 2] }, "tags": ["x", "y"] },
        })
    }

    /// Which paths name one place, and which value that place holds — over a
    /// TABLE whose rows differ along the FORM of each segment (shorthand,
    /// quoted name, index), along the DEPTH of the path, and along every
    /// selector that names more than one place or no place at all.
    #[test]
    fn a_reference_path_names_one_place_and_every_other_form_names_none() {
        // (case, path, the value it names)
        let rows: Vec<(&str, &str, Option<Value>)> = vec![
            ("the document itself", "$", Some(document())),
            ("a shorthand member", "$.id", Some(json!("e-1"))),
            (
                // RFC 9535's shorthand name excludes `-`, so this path is
                // evaluable only through the canonical rendering.
                "a hyphenated member",
                "$.detail-type",
                Some(json!("Order Created")),
            ),
            (
                "a nested member",
                "$.detail.tenant",
                Some(json!("alpha")),
            ),
            (
                "a quoted member",
                "$['detail']['n']",
                Some(json!(7)),
            ),
            ("an array element", "$.detail.tags[1]", Some(json!("y"))),
            (
                "a member and an element below it",
                "$.detail.o.a[0]",
                Some(json!(1)),
            ),
            ("a member the document does not carry", "$.detail.nope", None),
            ("an element past the array", "$.detail.tags[9]", None),
            // Each of these selects more than one place, or its cost grows with
            // the document rather than with the path.
            ("a descendant segment", "$..tenant", None),
            ("a descendant wildcard", "$..*", None),
            ("a wildcard member", "$.detail.*", None),
            ("a wildcard element", "$.detail.tags[*]", None),
            ("a union of names", "$['id','detail-type']", None),
            ("a union of indices", "$.detail.tags[0,1]", None),
            ("a slice", "$.detail.tags[0:2]", None),
            ("a negative index", "$.detail.tags[-1]", None),
            ("a filter", "$.detail.tags[?@=='x']", None),
            ("a function expression", "$.detail[?match(@.tenant,'a.*')]", None),
            // Not a path at all.
            ("text that is not rooted at the document", "detail.tenant", None),
            ("an empty path", "", None),
            ("a bracket that never closes", "$['detail'", None),
            ("an empty member name", "$..", None),
        ];

        let document = document();
        for (case, path, expected) in rows {
            let actual = select(path, &document).cloned();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {path} to name {expected:?}, got {actual:?}"
            );
        }
    }

    /// The two bounds: a path longer than [`MAX_PATH_BYTES`] and one carrying
    /// more than [`MAX_SEGMENTS`] segments name nothing, and each is accepted
    /// right up to its published bound.
    #[test]
    fn a_path_is_evaluated_up_to_its_bounds_and_not_past_them() {
        // (case, the segment count, whether it is within the bound)
        let rows = [
            ("one segment", 1, true),
            ("the deepest path evaluated", MAX_SEGMENTS, true),
            ("one segment past the bound", MAX_SEGMENTS + 1, false),
        ];
        for (case, count, within) in rows {
            let path = format!("$.{}", vec!["a"; count].join("."));
            let named = segments(&path);
            assert_eq!(
                named.is_some(),
                within,
                "case {case:?}: expected a path of {count} segments to be evaluated={within}, \
                 got {named:?}"
            );
            if within {
                assert_eq!(
                    named.as_deref().map(<[Segment]>::len),
                    Some(count),
                    "case {case:?}: expected {count} segments, got {named:?}"
                );
            }
        }

        // The length bound is charged on the text, whatever its segments are:
        // `$['…']` wraps a member name in five characters.
        let name = "a".repeat(MAX_PATH_BYTES);
        for (case, path, within) in [
            (
                "a path at the byte bound",
                format!("$['{}']", &name[..MAX_PATH_BYTES - 5]),
                true,
            ),
            (
                "a path one byte past it",
                format!("$['{}']", &name[..MAX_PATH_BYTES - 4]),
                false,
            ),
        ] {
            assert_eq!(
                segments(&path).is_some(),
                within,
                "case {case:?}: expected a path of {} bytes to be evaluated={within}",
                path.len()
            );
        }
    }

    /// Every accepted form renders to the same canonical text, so what the
    /// engine evaluates is this module's rendering and never the caller's text.
    #[test]
    fn every_form_of_one_place_renders_to_one_canonical_path() {
        let rows = [
            ("shorthand", "$.detail.tenant"),
            ("quoted names", "$['detail']['tenant']"),
            ("mixed forms", "$.detail['tenant']"),
        ];
        for (case, path) in rows {
            let rendered = segments(path).as_deref().map(canonical);
            assert_eq!(
                rendered.as_deref(),
                Some("$['detail']['tenant']"),
                "case {case:?}: expected one canonical rendering, got {rendered:?}"
            );
        }
        let indexed = segments("$.tags[2]").as_deref().map(canonical);
        assert_eq!(
            indexed.as_deref(),
            Some("$['tags'][2]"),
            "case \"an index\": expected the index to render as a bracket selector, got \
             {indexed:?}"
        );
    }
}
