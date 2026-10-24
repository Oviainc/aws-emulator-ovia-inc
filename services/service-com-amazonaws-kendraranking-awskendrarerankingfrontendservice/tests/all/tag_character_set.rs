// SPDX-License-Identifier: CC0-1.0

//! The character set a tag key or value may be built from, end to end over the wire.
//!
//! The rule: a tag key or value the request carries may consist only of Unicode
//! letters, digits, white space and the symbols `_ . : / = + - @`, and a tag key may
//! not lie under the `aws:` prefix AWS reserves for itself; a request carrying one
//! that does not is refused WHOLE with a 400 `ValidationException` and persists
//! nothing. Neither `TagKey` nor `TagValue` carries a `@pattern`, so the rule comes
//! from the `Tag` documentation ("A tag key and value can consist of Unicode letters,
//! digits, white space, and any of the following symbols: _ . : / = + - @.") rather
//! than from a modelled constraint.
//!
//! Each suite asserts the rule over a TABLE spanning the classes of character — the
//! documented symbols, Unicode letters in several scripts, Unicode digits, white
//! space, and punctuation, currency, pictographic and control characters outside the
//! set — on every operation that takes a tag key or value from the request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, create_plan_with_tags, emulator, error_code, error_message};

/// Text OUTSIDE the documented set, one row per class of character. Every row holds at
/// least one character that is not a Unicode letter, digit or white space and is none
/// of the eight symbols `Tag` names (the source of the catalog below).
const OUTSIDE_THE_SET: &[(&str, &str)] = &[
    ("an exclamation mark", "bad!key"),
    ("a percent sign", "100%"),
    ("a comma", "a,b"),
    ("a number sign", "#tag"),
    ("a dollar sign", "v$v"),
    ("an ampersand", "a&b"),
    ("a semicolon", "a;b"),
    ("a quotation mark", "say\"what\""),
    ("a reverse solidus", "a\\b"),
    ("parentheses", "a(b)"),
    ("an asterisk", "a*b"),
    ("curly brackets", "a{b}"),
    ("angle brackets", "a<b>"),
    ("a question mark", "a?b"),
    ("a vertical line", "a|b"),
    ("a circumflex accent", "a^b"),
    ("a tilde", "a~b"),
    ("a grave accent", "a`b"),
    ("a currency symbol", "10\u{20ac}"),
    ("a pictographic character", "ship \u{1f680}"),
    ("a copyright sign", "\u{a9}acme"),
    ("a control character", "a\u{1}b"),
];

/// Text INSIDE the documented set: Unicode letters, digits, white space, and each of
/// the eight symbols `Tag` names (`_ . : / = + - @` — the catalog's source), including
/// a script and a digit set the issue did not cite.
const INSIDE_THE_SET: &[(&str, &str)] = &[
    ("ASCII letters and digits", "team42"),
    ("white space", "cost center"),
    ("the low line", "cost_center"),
    ("the full stop", "cost.center"),
    ("the colon", "cost:center"),
    ("the solidus", "cost/center"),
    ("the equals sign", "cost=center"),
    ("the plus sign", "cost+center"),
    ("the hyphen-minus", "cost-center"),
    ("the commercial at", "cost@center"),
    ("every documented symbol at once", "_.:/=+-@"),
    ("Latin letters with diacritics", "\u{e9}quipe-recherche"),
    ("Greek letters", "\u{3bf}\u{3bc}\u{3ac}\u{3b4}\u{3b1}"),
    ("CJK letters", "\u{691c}\u{7d22}\u{30c1}\u{30fc}\u{30e0}"),
    ("Arabic-Indic digits", "\u{663}\u{664}\u{665}"),
    ("a non-breaking space", "cost\u{a0}center"),
];

/// The tags `ListTagsForResource` reports for `arn`, as `(Key, Value)` pairs.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    let (status, answer) = call(router, "ListTagsForResource", json!({ "ResourceARN": arn })).await;
    assert_eq!(status, 200, "list tags for {arn}: unexpected answer {answer}");
    answer["Tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The names `ListRescoreExecutionPlans` reports for every plan this emulator holds.
async fn listed_plan_names(router: &MultiServiceRouter) -> Vec<String> {
    let (status, answer) = call(router, "ListRescoreExecutionPlans", json!({})).await;
    assert_eq!(status, 200, "list plans: unexpected answer {answer}");
    answer["SummaryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assert `answer` is the refusal this rule produces, and that it names `member` —
/// the position of the offending value — without echoing the value itself.
fn assert_refused(case: &str, status: u16, answer: &Value, member: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {answer}"
    );
    assert_eq!(
        error_code(answer),
        "ValidationException",
        "case {case:?}: got {answer}"
    );
    let message = error_message(answer);
    assert!(
        message.contains(member),
        "case {case:?}: the report must name {member}, got {message:?}"
    );
}

#[tokio::test]
async fn tag_resource_refuses_a_key_or_value_outside_the_documented_character_set() {
    let router = emulator();
    let baseline = [("team", "search")];
    let (_, arn) = create_plan_with_tags(
        &router,
        "charset-target",
        json!([{ "Key": "team", "Value": "search" }]),
    )
    .await;

    for (case, text) in OUTSIDE_THE_SET {
        // The same character is refused wherever it sits: in the key, in the value,
        // and in a tag that shares its request with a legal one.
        let rows: Vec<(String, Value, &str)> = vec![
            (
                format!("{case} in the key"),
                json!([{ "Key": text, "Value": "v" }]),
                "tags.1.member.key",
            ),
            (
                format!("{case} in the value"),
                json!([{ "Key": "k", "Value": text }]),
                "tags.1.member.value",
            ),
            (
                format!("{case} beside a legal tag"),
                json!([{ "Key": "legal", "Value": "v" }, { "Key": text, "Value": "v" }]),
                "tags.2.member.key",
            ),
        ];

        for (row, tags, member) in rows {
            let (status, answer) =
                call(&router, "TagResource", json!({ "ResourceARN": arn, "Tags": tags })).await;
            assert_refused(&row, status, &answer, member);

            // Refused WHOLE: the plan still carries exactly the tags it started with,
            // so not even the legal tag of the mixed request was written.
            let listed = list_tags(&router, &arn).await;
            assert_eq!(
                listed.len(),
                baseline.len(),
                "case {row:?}: a refused write must persist nothing, got {listed:?}"
            );
            assert_eq!(
                listed[0],
                ("team".to_string(), "search".to_string()),
                "case {row:?}: a refused write must persist nothing, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn tag_resource_accepts_every_character_the_service_documents() {
    let router = emulator();

    for (case, text) in INSIDE_THE_SET {
        let (_, arn) = create_plan_with_tags(&router, "charset-accepted", Value::Null).await;
        let (status, answer) = call(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Key": text, "Value": text }] }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: unexpected answer {answer}");

        let listed = list_tags(&router, &arn).await;
        let expected = vec![((*text).to_string(), (*text).to_string())];
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn the_reserved_prefix_is_refused_on_a_tag_key_only() {
    let router = emulator();

    // The `aws:` prefix is AWS's own, so a caller may not write a key under it,
    // whatever case it spells it in (a tag key is case-insensitive: "Keys are not case
    // sensitive and must be unique." — `Tag$Key`). The rows span the prefix's casing,
    // a key that merely starts with the same letters, the prefix inside a key rather
    // than at its start, and the same text in a VALUE, which the prefix does not bound.
    struct PrefixCase {
        case: &'static str,
        key: &'static str,
        value: &'static str,
        accepted: bool,
    }

    let cases = &[
        PrefixCase {
            case: "the reserved prefix",
            key: "aws:createdBy",
            value: "me",
            accepted: false,
        },
        PrefixCase {
            case: "the reserved prefix in upper case",
            key: "AWS:CreatedBy",
            value: "me",
            accepted: false,
        },
        PrefixCase {
            case: "the reserved prefix in mixed case",
            key: "Aws:cloudformation:stack",
            value: "me",
            accepted: false,
        },
        PrefixCase {
            case: "the bare reserved prefix",
            key: "aws:",
            value: "",
            accepted: false,
        },
        PrefixCase {
            case: "the reserved letters without the colon",
            key: "aws",
            value: "v",
            accepted: true,
        },
        PrefixCase {
            case: "a longer word starting with the reserved letters",
            key: "awsome:team",
            value: "v",
            accepted: true,
        },
        PrefixCase {
            case: "the prefix inside the key rather than at its start",
            key: "my-aws:key",
            value: "v",
            accepted: true,
        },
        PrefixCase {
            case: "the prefix in a value, which it does not bound",
            key: "origin",
            value: "aws:cloudformation",
            accepted: true,
        },
    ];

    for PrefixCase {
        case,
        key,
        value,
        accepted,
    } in cases
    {
        let (_, arn) = create_plan_with_tags(&router, "reserved-prefix-target", Value::Null).await;
        let (status, answer) = call(
            &router,
            "TagResource",
            json!({ "ResourceARN": arn, "Tags": [{ "Key": key, "Value": value }] }),
        )
        .await;
        let listed = list_tags(&router, &arn).await;
        if *accepted {
            assert_eq!(status, 200, "case {case:?}: unexpected answer {answer}");
            let expected = vec![((*key).to_string(), (*value).to_string())];
            assert_eq!(
                listed, expected,
                "case {case:?}: expected {expected:?}, got {listed:?}"
            );
        } else {
            assert_refused(case, status, &answer, "tags.1.member.key");
            assert!(
                listed.is_empty(),
                "case {case:?}: a refused write must persist nothing, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn creating_a_plan_with_an_illegal_tag_is_refused_and_mints_no_plan() {
    let router = emulator();
    let (_, kept) = create_plan_with_tags(&router, "kept-plan", Value::Null).await;

    // The same rule bounds the tags a CREATE carries: the request is refused and no
    // plan is minted for it. One row per class of offending tag, plus a legal exotic
    // one that must round-trip.
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "a character outside the set in the key",
            json!([{ "Key": "bad!key", "Value": "v" }]),
            Some("tags.1.member.key"),
        ),
        (
            "a character outside the set in the value",
            json!([{ "Key": "k", "Value": "v$v" }]),
            Some("tags.1.member.value"),
        ),
        (
            "the reserved key prefix",
            json!([{ "Key": "aws:createdBy", "Value": "me" }]),
            Some("tags.1.member.key"),
        ),
        (
            "an illegal tag beside a legal one",
            json!([{ "Key": "legal", "Value": "v" }, { "Key": "a,b", "Value": "v" }]),
            Some("tags.2.member.key"),
        ),
        (
            "the documented symbols and Unicode letters",
            json!([{ "Key": "co\u{fc}t_centre", "Value": "a_b:c/d=e+f-g@h" }]),
            None,
        ),
    ];

    for (case, tags, member) in cases {
        let (status, answer) = call(
            &router,
            "CreateRescoreExecutionPlan",
            json!({ "Name": "charset-create", "Tags": tags.clone() }),
        )
        .await;
        let names = listed_plan_names(&router).await;
        match member {
            Some(member) => {
                assert_refused(case, status, &answer, member);
                assert!(
                    !names.iter().any(|name| name == "charset-create"),
                    "case {case:?}: a refused create must mint no plan, got {names:?}"
                );
            }
            None => {
                assert_eq!(status, 200, "case {case:?}: unexpected answer {answer}");
                let arn = answer["Arn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no Arn in {answer}"));
                let listed = list_tags(&router, arn).await;
                let expected: Vec<(String, String)> = tags
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|tag| {
                        (
                            tag["Key"].as_str().unwrap_or_default().to_string(),
                            tag["Value"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect();
                assert_eq!(
                    listed, expected,
                    "case {case:?}: expected {expected:?}, got {listed:?}"
                );
            }
        }
    }

    // Nothing the refusals did touched the plan that was already there.
    let kept_tags = list_tags(&router, &kept).await;
    assert!(
        kept_tags.is_empty(),
        "the refused creates must not touch another plan, got {kept_tags:?}"
    );
}

#[tokio::test]
async fn untagging_a_key_outside_the_character_set_is_refused_and_removes_nothing() {
    let router = emulator();

    // `UntagResource` takes `TagKey` values too, so the same character set bounds
    // them; a refused request removes nothing.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "one key outside the set",
            json!(["bad!key"]),
            "tagKeys.1.member",
        ),
        (
            "an illegal key beside a legal one",
            json!(["team", "100%"]),
            "tagKeys.2.member",
        ),
        (
            "a pictographic key",
            json!(["ship \u{1f680}"]),
            "tagKeys.1.member",
        ),
    ];

    for (case, keys, member) in cases {
        let (_, arn) = create_plan_with_tags(
            &router,
            "untag-charset-target",
            json!([{ "Key": "team", "Value": "search" }]),
        )
        .await;
        let (status, answer) = call(
            &router,
            "UntagResource",
            json!({ "ResourceARN": arn, "TagKeys": keys }),
        )
        .await;
        assert_refused(case, status, &answer, member);

        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed,
            vec![("team".to_string(), "search".to_string())],
            "case {case:?}: a refused removal must change nothing, got {listed:?}"
        );
    }

    // A legal key list still removes exactly what it names.
    let (_, arn) = create_plan_with_tags(
        &router,
        "untag-charset-legal",
        json!([{ "Key": "cost-center", "Value": "42" }, { "Key": "team", "Value": "search" }]),
    )
    .await;
    let (status, answer) = call(
        &router,
        "UntagResource",
        json!({ "ResourceARN": arn, "TagKeys": ["cost-center"] }),
    )
    .await;
    assert_eq!(status, 200, "a legal removal: got {status} {answer}");
    let listed = list_tags(&router, &arn).await;
    assert_eq!(
        listed,
        vec![("team".to_string(), "search".to_string())],
        "a legal removal takes only the key it names, got {listed:?}"
    );
}

#[tokio::test]
async fn every_offending_position_is_reported_at_once() {
    let router = emulator();
    let (_, arn) = create_plan_with_tags(&router, "report-target", Value::Null).await;

    // The standard AWS report states the violation count and one clause per offending
    // position, and a clause never renders the caller's value.
    let (status, answer) = call(
        &router,
        "TagResource",
        json!({
            "ResourceARN": arn,
            "Tags": [
                { "Key": "legal", "Value": "v" },
                { "Key": "bad!key", "Value": "also,bad" },
            ],
        }),
    )
    .await;
    assert_eq!(status, 400, "the mixed request: got {status} {answer}");
    let message = error_message(&answer);
    for expected in [
        "2 validation errors detected",
        "tags.2.member.key",
        "tags.2.member.value",
    ] {
        assert!(
            message.contains(expected),
            "the report must state {expected:?}, got {message:?}"
        );
    }
    for value in ["bad!key", "also,bad"] {
        assert!(
            !message.contains(value),
            "the report must not echo the offending value {value:?}, got {message:?}"
        );
    }

    let listed = list_tags(&router, &arn).await;
    assert!(
        listed.is_empty(),
        "the refused request must persist nothing, got {listed:?}"
    );
}
