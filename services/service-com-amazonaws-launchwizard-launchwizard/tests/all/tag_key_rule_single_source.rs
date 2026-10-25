// SPDX-License-Identifier: CC0-1.0

//! The modelled `TagKey` length/pattern rules have ONE statement in this crate —
//! `state::TAG_KEY_MIN_LEN` / `TAG_KEY_MAX_LEN` / `TAG_KEY_PATTERN`, pinned to
//! `smithy-model/` by `resource_tagging::the_tag_rules_the_handlers_enforce_are_the_models`
//! — and BOTH places that judge a tag key are built from it: the tag-MAP key check
//! `TagResource` runs, and the element constraint set `UntagResource`'s `tagKeys`
//! refusal is composed from.
//!
//! This suite drives the same offending key through an operation on each side of that
//! shared statement and requires both refusals to state the rule the statement carries.
//! Every expected clause below is BUILT from those three constants rather than written
//! out as literals, so a copy of the rule that drifted from them — stating a bound or a
//! pattern the handlers do not enforce — fails here.
//!
//! The rows differ along what the rule generalises over: WHICH of the three rules the
//! key breaks, how far past the length ceiling it is, the character set it uses, and the
//! operation (and therefore the protocol binding — a JSON map member versus a query
//! parameter) the key arrives on.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::deployment_lifecycle::create;
use crate::harness::Harness;
// The tagging suite owns the `/tags/{resourceArn}` request shape; reached from here
// rather than rebuilt, so the two suites cannot disagree about how a tagging call is
// addressed.
use crate::resource_tagging::{deployment_arn, encode_component};
use service_com_amazonaws_launchwizard_launchwizard::state;

/// The wire error code and HTTP status a refusal about the shape of a tag key carries
/// (`smithy_query(query="shape",shape="ValidationException")`: `errorKind: client`,
/// `httpError: 400`).
const CODE: &str = "ValidationException";
/// See [`CODE`].
const STATUS: u16 = 400;

/// The rule's length ceiling, as a validation report words the requirement.
fn length_ceiling_requirement() -> String {
    format!(
        "Member must have length less than or equal to {}",
        state::TAG_KEY_MAX_LEN
    )
}

/// The rule's length floor, as a validation report words the requirement.
fn length_floor_requirement() -> String {
    format!(
        "Member must have length greater than or equal to {}",
        state::TAG_KEY_MIN_LEN
    )
}

/// A `@pattern` requirement over `expression`.
fn pattern_requirement(expression: &str) -> String {
    format!("Member must satisfy regular expression pattern: {expression}")
}

/// The rule's pattern as a member reported on its own states it: without the `^`/`$`
/// the model spells "the whole value" with.
fn unanchored_pattern() -> String {
    let pattern = state::TAG_KEY_PATTERN;
    let without_caret = pattern.strip_prefix('^').unwrap_or(pattern);
    without_caret
        .strip_suffix('$')
        .unwrap_or(without_caret)
        .to_string()
}

/// The one clause `UntagResource` reports a bad `tagKeys` element with: the element
/// shape's COMPLETE constraint set, composed, whichever member of it the key broke —
/// so this clause is the same for every row below.
fn composed_tag_keys_clause() -> String {
    format!(
        "Value at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: \
         [{}, {}, {}]",
        length_ceiling_requirement(),
        length_floor_requirement(),
        pattern_requirement(&format!(".*{}.*", state::TAG_KEY_PATTERN)),
    )
}

/// The clause a tag-MAP key that broke `requirement` is reported with: the member the
/// map arrived on, and what the key failed.
fn tags_clause(requirement: &str) -> String {
    format!("Value at 'tags' failed to satisfy constraint: {requirement}")
}

/// The clauses of a validation report, as a SET: one key can break two of the rules at
/// once and a report states one clause per rule, in an order this suite does not pin.
/// The report's own count is required to agree with how many clauses it then states.
fn clauses(case: &str, message: &str) -> BTreeSet<String> {
    let (count, stated) = message
        .split_once(" detected: ")
        .unwrap_or_else(|| panic!("case {case:?}: not a validation report: {message:?}"));
    let clauses: BTreeSet<String> = stated.split("; ").map(ToString::to_string).collect();
    let noun = if clauses.len() == 1 { "error" } else { "errors" };
    let expected_count = format!("{} validation {noun}", clauses.len());
    assert_eq!(
        count, expected_count,
        "case {case:?}: expected the report to open with {expected_count:?}, got {count:?} \
         in {message:?}"
    );
    clauses
}

/// The `message` of a refusal, after requiring the modelled status and code.
async fn refusal_clauses(
    harness: &Harness,
    case: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> BTreeSet<String> {
    let reply = harness
        .expect_refused(case, method, path, body, STATUS, CODE)
        .await;
    let message = reply
        .get("message")
        .or_else(|| reply.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no message in the refusal body {reply}"))
        .to_string();
    clauses(case, &message)
}

/// The `/tags/{resourceArn}` path `TagResource` and `ListTagsForResource` are called on.
fn tags_path(arn: &str) -> String {
    format!("/tags/{}", encode_component(arn))
}

/// The `/tags/{resourceArn}?tagKeys=<key>` path `UntagResource` is called on for one
/// key: `tagKeys` is `@httpQuery`, so a key travels as a query parameter.
fn untag_path(arn: &str, key: &str) -> String {
    format!("{}?tagKeys={}", tags_path(arn), encode_component(key))
}

/// One offending tag key, and the clauses the tag-MAP key check must report it with.
struct Row {
    case: &'static str,
    key: String,
    /// The clauses `TagResource`'s key check states — the rules this key actually
    /// breaks. `UntagResource` states [`composed_tag_keys_clause`] for every row,
    /// because a list element is reported with its element shape's whole set.
    map_clauses: BTreeSet<String>,
}

/// A deployment to tag, so the tag key is judged at all: a tagging call reads its
/// `resourceArn` and resolves the resource before it judges the keys it was given.
async fn subject(harness: &Harness) -> String {
    let id = create(harness, "a deployment to tag", "SAP", "SapHanaSingle", "tag-key-rule").await;
    deployment_arn(&id)
}

#[tokio::test]
async fn both_tag_key_checks_refuse_by_the_same_statement_of_the_rule() {
    let harness = Harness::new();
    let arn = subject(&harness).await;

    let ceiling = state::TAG_KEY_MAX_LEN as usize;
    let rows = vec![
        Row {
            case: "a key one character past the length ceiling",
            key: "k".repeat(ceiling + 1),
            map_clauses: BTreeSet::from([tags_clause(&length_ceiling_requirement())]),
        },
        Row {
            case: "a key far past the length ceiling",
            key: "k".repeat(ceiling * 4),
            map_clauses: BTreeSet::from([tags_clause(&length_ceiling_requirement())]),
        },
        Row {
            // Below the length floor AND outside the pattern, which needs one
            // character: two rules at once, so two clauses.
            case: "a key below the length floor",
            key: String::new(),
            map_clauses: BTreeSet::from([
                tags_clause(&length_floor_requirement()),
                tags_clause(&pattern_requirement(&unanchored_pattern())),
            ]),
        },
        Row {
            case: "a key carrying one character the pattern excludes",
            key: "Owner\u{a3}".to_string(),
            map_clauses: BTreeSet::from([tags_clause(&pattern_requirement(&unanchored_pattern()))]),
        },
        Row {
            case: "a key of nothing but excluded characters",
            key: "\u{a3}\u{20ac}".to_string(),
            map_clauses: BTreeSet::from([tags_clause(&pattern_requirement(&unanchored_pattern()))]),
        },
    ];

    let composed = BTreeSet::from([composed_tag_keys_clause()]);
    for row in &rows {
        let case = row.case;

        // The tag-MAP key check, reached through `TagResource`.
        let reported = refusal_clauses(
            &harness,
            &format!("TagResource with {case}"),
            "POST",
            &tags_path(&arn),
            Some(&json!({"tags": {row.key.clone(): "v"}})),
        )
        .await;
        assert_eq!(
            reported, row.map_clauses,
            "case {case:?}: expected TagResource to report {:?}, got {reported:?}",
            row.map_clauses
        );

        // The `tagKeys` element check, reached through `UntagResource`.
        let reported = refusal_clauses(
            &harness,
            &format!("UntagResource with {case}"),
            "DELETE",
            &untag_path(&arn, &row.key),
            None,
        )
        .await;
        assert_eq!(
            reported, composed,
            "case {case:?}: expected UntagResource to report {composed:?}, got {reported:?}"
        );

        // Neither refusal replays the rejected key back to the caller.
        if !row.key.is_empty() {
            for clause in row.map_clauses.iter().chain(composed.iter()) {
                assert!(
                    !clause.contains(row.key.as_str()),
                    "case {case:?}: a clause echoes the rejected key: {clause:?}"
                );
            }
        }
    }

    // And a key the ONE statement admits is refused by neither check, so the rows above
    // are about the rule rather than about tag keys. The longest admitted key is exactly
    // at the ceiling the rows step one past.
    for (case, key) in [
        ("a plain key", "Owner".to_string()),
        ("a key of one character", "k".to_string()),
        ("a key at the length ceiling", "k".repeat(ceiling)),
    ] {
        harness
            .expect_ok(
                &format!("TagResource with {case}"),
                "POST",
                &tags_path(&arn),
                Some(&json!({"tags": {key.clone(): "v"}})),
            )
            .await;
        harness
            .expect_ok(
                &format!("UntagResource with {case}"),
                "DELETE",
                &untag_path(&arn, &key),
                None,
            )
            .await;
    }
}
