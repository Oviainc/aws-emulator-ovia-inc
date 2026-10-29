// SPDX-License-Identifier: CC0-1.0

//! How `TagResource` WORDS a refusal of the tag map's per-entry bounds.
//!
//! RULE: when an entry of the tag map being added falls outside the modelled
//! `@length` of the shape that half of it is bound to — a key to `TagKey` (1..128), a
//! value to `TagValue` (0..256) — the tagging front end answers 400
//! `BadRequestException` whose message is itself a JSON DOCUMENT carrying the
//! validation envelope, in which each clause renders the whole offending ENTRY as
//! `{key=value}`, names the member the front end reports the added tags under, and
//! states the broken bound as a MAP constraint carrying both the modelled maximum and
//! minimum — not as the scalar-member clause a constrained leaf earns.
//!
//! Only the rendered entry and the two bounds vary; every other character of the
//! document is fixed text. So each row below computes the whole expected message from
//! that template and compares the answer against it in full, and the rows differ
//! along which half broke (key, value, both), how far past the bound it is, which end
//! of the bound it broke, how many entries offend, and what the entry's characters are.
//!
//! The wire harness (`Answer`, `emulator`, `error_code`) is the shared one in
//! `tests/all/wire.rs`; the tag helpers are `tag_contract`'s.

use crate::tag_contract::{stored, tag_map, taggable, Tags};
use crate::wire::{emulator, error_code};

/// The published character bounds of one tag key and one tag value, which are also
/// the bounds a clause states: `TagKey` is `@length {min: 1, max: 128}` and `TagValue`
/// is `@length {min: 0, max: 256}`
/// (`smithy_query(query="shape",shape="TagMap,TagKey,TagValue",
/// model_path="services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json")`).
const MIN_KEY_LENGTH: usize = 1;
/// See [`MIN_KEY_LENGTH`].
const MAX_KEY_LENGTH: usize = 128;
/// See [`MIN_KEY_LENGTH`].
const MIN_VALUE_LENGTH: usize = 0;
/// See [`MIN_KEY_LENGTH`].
const MAX_VALUE_LENGTH: usize = 256;

/// The member name the tagging front end reports the added tags under — its own name
/// for them, not the `tags` member the model declares on the input.
const MEMBER: &str = "addedTags";

/// The error code `TagResource` refuses a bad request with.
const BAD_REQUEST: &str = "BadRequestException";

/// How much of one half of an entry a clause carries before eliding it: twice the
/// maximum that half's shape allows, so a half that overran its bound is readable as
/// sent while the document stays bounded by the model rather than by how much the
/// caller sent.
fn echo_limit(half: Half) -> usize {
    half.bounds().1 * 2
}

/// Which half of an entry a clause is about: the two halves are bound to different
/// shapes, so each has its own bounds and its own name in the clause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Half {
    Key,
    Value,
}

impl Half {
    /// How a clause names this half.
    fn subject(self) -> &'static str {
        match self {
            Self::Key => "Map keys",
            Self::Value => "Map value",
        }
    }

    /// The modelled bounds a clause about this half states, as (min, max).
    fn bounds(self) -> (usize, usize) {
        match self {
            Self::Key => (MIN_KEY_LENGTH, MAX_KEY_LENGTH),
            Self::Value => (MIN_VALUE_LENGTH, MAX_VALUE_LENGTH),
        }
    }
}

/// One half of an entry as a clause echoes it.
fn echoed(value: &str, half: Half) -> String {
    let limit = echo_limit(half);
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let kept: String = value.chars().take(limit).collect();
    format!("{kept}…")
}

/// One half of an entry as the document carries it: echoed, then with every
/// character the document escapes escaped.
fn in_document(value: &str, half: Half) -> String {
    echoed(value, half)
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\'', "\\'")
}

/// One clause of the document: the offending entry, the member, and the broken
/// half's bounds.
fn clause(key: &str, value: &str, half: Half) -> String {
    let (min, max) = half.bounds();
    let subject = half.subject();
    let entry = format!(
        "{{{}={}}}",
        in_document(key, Half::Key),
        in_document(value, Half::Value)
    );
    format!(
        "Value \\'{entry}\\' at \\'{MEMBER}\\' failed to satisfy constraint: \
         {subject} must satisfy constraint: \
         [Member must have length less than or equal to {max}, \
         Member must have length greater than or equal to {min}]"
    )
}

/// The whole message a refusal carries: the validation envelope, as a JSON document.
fn document(clauses: &[String]) -> String {
    let detected = if clauses.len() == 1 {
        format!("1 validation error detected: {}", clauses[0])
    } else {
        format!(
            "{} validation errors detected: {}",
            clauses.len(),
            clauses.join("; ")
        )
    };
    format!("{{\"message\": \"{detected}\"}}")
}

// ---------------------------------------------------------------------------
// RULE: a per-entry bound broken by one entry earns the map-constraint document for
// the half that broke it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_broken_per_entry_bound_earns_the_map_constraint_document() {
    let router = emulator();

    // (case, key, value, the halves that broke, in the order the clauses appear)
    let rows: Vec<(&str, String, String, Vec<Half>)> = vec![
        (
            "a key one character past its maximum",
            "k".repeat(MAX_KEY_LENGTH + 1),
            "v".to_string(),
            vec![Half::Key],
        ),
        (
            "a key well past its maximum but inside twice it",
            "k".repeat(MAX_KEY_LENGTH + 40),
            String::new(),
            vec![Half::Key],
        ),
        (
            "a key past twice its maximum, so the entry is elided",
            "k".repeat(MAX_KEY_LENGTH * 3),
            "v".to_string(),
            vec![Half::Key],
        ),
        (
            "an empty key, one character under its minimum",
            String::new(),
            "v".to_string(),
            vec![Half::Key],
        ),
        (
            "a value one character past its maximum",
            "ok".to_string(),
            "v".repeat(MAX_VALUE_LENGTH + 1),
            vec![Half::Value],
        ),
        (
            "a value far past its maximum, past twice it",
            "ok".to_string(),
            "v".repeat(MAX_VALUE_LENGTH * 4),
            vec![Half::Value],
        ),
        (
            "a value past its maximum but inside twice it",
            "ok".to_string(),
            "v".repeat(MAX_VALUE_LENGTH + 100),
            vec![Half::Value],
        ),
        (
            "both halves past their maxima",
            "k".repeat(MAX_KEY_LENGTH + 1),
            "v".repeat(MAX_VALUE_LENGTH + 1),
            vec![Half::Key, Half::Value],
        ),
        (
            "an empty key beside an over-long value",
            String::new(),
            "v".repeat(MAX_VALUE_LENGTH + 9),
            vec![Half::Key, Half::Value],
        ),
        (
            "a key whose characters would break the document",
            format!("{}\"\\'", "k".repeat(MAX_KEY_LENGTH)),
            "v".to_string(),
            vec![Half::Key],
        ),
        (
            "a multi-byte key counted in characters, not bytes",
            "é".repeat(MAX_KEY_LENGTH + 1),
            "v".to_string(),
            vec![Half::Key],
        ),
    ];

    for (index, (case, key, value, halves)) in rows.iter().enumerate() {
        let arn = taggable(&router, &format!("entry-bound-{index}")).await;
        let mut map = Tags::new();
        map.insert(key.clone(), value.clone());
        let answer = tag_map(&router, &arn, &map).await;

        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            BAD_REQUEST,
            "case {case:?}: expected {BAD_REQUEST}, got {answer:?}"
        );
        let clauses: Vec<String> = halves
            .iter()
            .map(|half| clause(key, value, *half))
            .collect();
        let expected = document(&clauses);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the map-constraint document {expected:?}, got {:?}",
            answer.body["message"]
        );
        // The refusal is TOTAL: nothing of the request reached the record.
        assert_eq!(
            stored(&router, &arn).await,
            serde_json::json!({}),
            "case {case:?}: a refused request must write nothing"
        );
    }
}

/// One clause a row expects: the entry it is about and which half of it broke.
type ExpectedClause = (String, String, Half);

/// One row of the several-entries table: the case, the map sent, and the clauses it
/// must earn, in order.
type MapRow = (&'static str, Tags, Vec<ExpectedClause>);

// ---------------------------------------------------------------------------
// RULE (the same one, over a map of several entries): every offending entry earns its
// own clause, the envelope counts them, and an entry inside its bounds earns none.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_offending_entry_earns_its_own_clause() {
    let router = emulator();

    let long_key = "a".repeat(MAX_KEY_LENGTH + 1);
    let longer_key = "b".repeat(MAX_KEY_LENGTH + 2);
    let long_value = "v".repeat(MAX_VALUE_LENGTH + 1);

    // (case, the map, the (key, value, half) each clause is about, in order). The
    // clauses follow the map's key order, so one request always reads the same way.
    let rows: Vec<MapRow> = vec![
        (
            "one offending entry beside two well-formed ones",
            Tags::from([
                ("env".to_string(), "dev".to_string()),
                (long_key.clone(), "v".to_string()),
                ("zone".to_string(), "a".to_string()),
            ]),
            vec![(long_key.clone(), "v".to_string(), Half::Key)],
        ),
        (
            "two offending entries, each on a different half",
            Tags::from([
                (long_key.clone(), "v".to_string()),
                ("ok".to_string(), long_value.clone()),
            ]),
            vec![
                (long_key.clone(), "v".to_string(), Half::Key),
                ("ok".to_string(), long_value.clone(), Half::Value),
            ],
        ),
        (
            "three clauses from two entries",
            Tags::from([
                (long_key.clone(), long_value.clone()),
                (longer_key.clone(), "v".to_string()),
            ]),
            vec![
                (long_key.clone(), long_value.clone(), Half::Key),
                (long_key.clone(), long_value.clone(), Half::Value),
                (longer_key.clone(), "v".to_string(), Half::Key),
            ],
        ),
    ];

    for (index, (case, map, expected_clauses)) in rows.iter().enumerate() {
        let arn = taggable(&router, &format!("several-entries-{index}")).await;
        let answer = tag_map(&router, &arn, map).await;

        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            BAD_REQUEST,
            "case {case:?}: expected {BAD_REQUEST}, got {answer:?}"
        );
        let clauses: Vec<String> = expected_clauses
            .iter()
            .map(|(key, value, half)| clause(key, value, *half))
            .collect();
        let expected = document(&clauses);
        assert_eq!(
            answer.body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the map-constraint document {expected:?}, got {:?}",
            answer.body["message"]
        );
        assert_eq!(
            stored(&router, &arn).await,
            serde_json::json!({}),
            "case {case:?}: a refused request must write nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE (what the document is FOR): the message is a document a client can parse, and
// the member path it names is the one the front end reports the added tags under.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_message_is_a_document_a_client_can_parse() {
    let router = emulator();

    // (case, key, value) — the key leg, the value leg, and a key carrying the two
    // characters a JSON string cannot hold raw.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "the key leg",
            "k".repeat(MAX_KEY_LENGTH + 1),
            "v".to_string(),
        ),
        (
            "the value leg",
            "ok".to_string(),
            "v".repeat(MAX_VALUE_LENGTH + 1),
        ),
        (
            "a key carrying a quote and a backslash",
            format!("{}\"\\", "k".repeat(MAX_KEY_LENGTH)),
            "v".to_string(),
        ),
    ];

    for (index, (case, key, value)) in rows.iter().enumerate() {
        let arn = taggable(&router, &format!("parsed-{index}")).await;
        let mut map = Tags::new();
        map.insert(key.clone(), value.clone());
        let answer = tag_map(&router, &arn, &map).await;

        let message = answer.body["message"].as_str().unwrap_or_default();
        // The quotes inside the document are escaped, which is how it survives being
        // carried in the message field at all; a client unescapes them before parsing.
        let unescaped = message.replace("\\'", "'");
        let parsed: serde_json::Value = serde_json::from_str(&unescaped)
            .unwrap_or_else(|error| panic!("case {case:?}: {message:?} must parse: {error}"));
        let inner = parsed["message"].as_str().unwrap_or_default();
        assert!(
            inner.contains(&format!("at '{MEMBER}' failed to satisfy constraint:")),
            "case {case:?}: expected the document to name {MEMBER}, got {inner:?}"
        );
        assert!(
            inner.contains("must satisfy constraint: [Member must have length"),
            "case {case:?}: expected a map-constraint clause, got {inner:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE (the other side of the bound): an entry INSIDE both bounds is stored, so the
// refusal is the bound's and not the renderer's.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_entry_inside_both_bounds_is_stored() {
    let router = emulator();

    // (case, key length, value length) — each published bound's own ends, and a
    // mid-range entry.
    let rows: [(&str, usize, usize); 4] = [
        ("both bounds' low end", MIN_KEY_LENGTH, MIN_VALUE_LENGTH),
        ("both bounds' high end", MAX_KEY_LENGTH, MAX_VALUE_LENGTH),
        ("a long key with a short value", MAX_KEY_LENGTH, 1),
        ("a short key with a long value", 3, MAX_VALUE_LENGTH),
    ];

    for (index, (case, key_length, value_length)) in rows.iter().enumerate() {
        let arn = taggable(&router, &format!("inside-{index}")).await;
        let mut map = Tags::new();
        map.insert("k".repeat(*key_length), "v".repeat(*value_length));
        let answer = tag_map(&router, &arn, &map).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {answer:?}"
        );
        assert_eq!(
            stored(&router, &arn).await,
            serde_json::to_value(&map).expect("tags serialize"),
            "case {case:?}: expected the entry to round-trip"
        );
    }
}
