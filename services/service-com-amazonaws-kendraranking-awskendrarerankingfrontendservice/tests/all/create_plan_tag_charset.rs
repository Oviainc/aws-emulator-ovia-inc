// SPDX-License-Identifier: CC0-1.0

//! The character set bounding the tags a CREATE carries, end to end over the wire.
//!
//! The rule: a tag key or value in a write request may consist only of Unicode
//! letters, digits, white space and the symbols `_ . : / = + - @`, so
//! `CreateRescoreExecutionPlan` — which takes the same `TagList` the tagging
//! operations take — refuses a create carrying one that does not with a 400
//! `ValidationException` and persists NOTHING: no plan, no id, no ARN, no tag and no
//! `ClientToken` registration. Neither `TagKey` nor `TagValue` carries a `@pattern`
//! (verified with smithy_query query=shape shape=Tag,TagKey,TagValue), so the rule
//! comes from the `Tag` documentation ("A tag key and value can consist of Unicode
//! letters, digits, white space, and any of the following symbols: _ . : / = + - @.")
//! rather than from a modelled constraint.
//!
//! `tests/all/tag_character_set.rs` asserts the same rule across the classes of
//! character on every operation that takes a tag. This suite pins what is specific to
//! the CREATE path, which mints a resource rather than editing one: the refusal
//! reaches it BEFORE anything is minted, over a table spanning the offending value's
//! position in the request, the other members a create may carry, the scripts and
//! digit sets a legal tag may be written in, the reserved key prefix, and the
//! idempotency token a refused create must leave unused.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    assert_tag_rejection, call, emulator, error_message, list_plan_summaries, list_tags,
};

/// Text OUTSIDE the documented set, one row per class of character, chosen so no row
/// is a character `tests/all/tag_character_set.rs` already cites: each holds at least
/// one character that is no Unicode letter, digit or white space and none of the eight
/// symbols the `Tag` doc names (the source of this catalog).
const OUTSIDE_THE_SET: &[(&str, &str)] = &[
    ("square brackets", "cost[center]"),
    ("a plus-minus sign", "delta\u{b1}5"),
    ("a section sign", "clause\u{a7}3"),
    ("a middle dot", "a\u{b7}b"),
    ("a bullet", "a\u{2022}b"),
    ("an em dash", "cost\u{2014}center"),
    ("a right single quotation mark", "team\u{2019}s"),
    ("a degree sign", "30\u{b0}c"),
    ("a non-breaking hyphen", "cost\u{2011}center"),
    ("a zero-width space", "a\u{200b}b"),
];

/// Text INSIDE the documented set: Unicode letters in scripts and digits in digit sets
/// the issue did not cite, white space, and every one of the eight symbols the `Tag`
/// doc names (this catalog's source). Every row is built only from characters whose
/// Unicode category is a letter, a digit or a separator — the three classes the doc
/// names beside those symbols.
const INSIDE_THE_SET: &[(&str, &str)] = &[
    ("Cyrillic letters", "\u{43f}\u{43e}\u{438}\u{441}\u{43a}"),
    ("Hebrew letters", "\u{5d7}\u{5d9}\u{5e4}\u{5d5}\u{5e9}"),
    ("Thai letters", "\u{e04}\u{e19}\u{e2b}\u{e32}"),
    ("Devanagari digits", "\u{966}\u{967}\u{968}"),
    ("Thai digits", "\u{e51}\u{e52}\u{e53}"),
    ("an ideographic space", "cost\u{3000}center"),
    ("the micro sign, a Unicode lowercase letter", "5\u{b5}s"),
    ("every documented symbol at once", "_.:/=+-@"),
    ("digits and the hyphen-minus", "2026-10-24"),
    ("the empty value", ""),
];

/// Every `(Name, Id)` pair `ListRescoreExecutionPlans` reports for this emulator,
/// projected out of the shared summary listing.
async fn listed_plans(router: &MultiServiceRouter) -> Vec<(String, String)> {
    list_plan_summaries(router)
        .await
        .iter()
        .map(|item| {
            (
                item["Name"].as_str().unwrap_or_default().to_string(),
                item["Id"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Assert `answer` is the refusal this rule produces and that it names `member` — the
/// position of the offending value — without echoing the value itself. The check is the
/// shared [`super::harness::assert_tag_rejection`]; every row of this suite knows the
/// value it sent, so the no-echo arm is always exercised here.
fn assert_refused(case: &str, status: u16, answer: &Value, member: &str, offender: &str) {
    assert_tag_rejection(case, status, answer, member, Some(offender));
}

#[tokio::test]
async fn a_create_carrying_a_character_outside_the_documented_set_mints_nothing() {
    let router = emulator();

    for (class, text) in OUTSIDE_THE_SET {
        // The same character is refused wherever it sits in the request, and whichever
        // other members the create carries: the rule bounds the tag content, not the
        // shape of the call around it.
        let rows: Vec<(String, Value, &str)> = vec![
            (
                format!("{class} in the key"),
                json!({ "Name": "charset-create", "Tags": [{ "Key": text, "Value": "v" }] }),
                "tags.1.member.key",
            ),
            (
                format!("{class} in the value"),
                json!({ "Name": "charset-create", "Tags": [{ "Key": "k", "Value": text }] }),
                "tags.1.member.value",
            ),
            (
                format!("{class} in the third tag of the list"),
                json!({
                    "Name": "charset-create",
                    "Tags": [
                        { "Key": "team", "Value": "search" },
                        { "Key": "env", "Value": "prod" },
                        { "Key": text, "Value": "v" },
                    ],
                }),
                "tags.3.member.key",
            ),
            (
                format!("{class} beside a Description and CapacityUnits"),
                json!({
                    "Name": "charset-create",
                    "Description": "a plan the service must not mint",
                    "CapacityUnits": { "RescoreCapacityUnits": 2 },
                    "Tags": [{ "Key": "k", "Value": text }],
                }),
                "tags.1.member.value",
            ),
            (
                format!("{class} beside a ClientToken"),
                json!({
                    "Name": "charset-create",
                    "ClientToken": "token-for-a-refused-create",
                    "Tags": [{ "Key": text, "Value": "v" }],
                }),
                "tags.1.member.key",
            ),
        ];

        for (case, request, member) in rows {
            let (status, answer) = call(&router, "CreateRescoreExecutionPlan", request).await;
            assert_refused(&case, status, &answer, member, text);

            // Refused WHOLE: no plan was minted, so not even the legal tags of the
            // mixed rows above can be read back anywhere.
            let plans = listed_plans(&router).await;
            assert!(
                plans.is_empty(),
                "case {case:?}: a refused create must mint no plan, got {plans:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_create_whose_tags_use_only_documented_characters_round_trips_them() {
    let router = emulator();

    for (class, text) in INSIDE_THE_SET {
        // A key must be 1..128 characters (`Tag$Key`), so the empty row is exercised as
        // a VALUE only — the one place the model admits it ("The value can be an empty
        // string but it can't be null." — `Tag$Value`).
        let key = if text.is_empty() { "owner" } else { *text };
        let (status, answer) = call(
            &router,
            "CreateRescoreExecutionPlan",
            json!({
                "Name": "charset-accepted",
                "Tags": [{ "Key": key, "Value": text }],
            }),
        )
        .await;
        assert_eq!(status, 200, "case {class:?}: unexpected answer {answer}");
        let id = answer["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {class:?}: no Id in {answer}"))
            .to_string();
        let arn = answer["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {class:?}: no Arn in {answer}"))
            .to_string();

        // The plan the create minted reads back through the implemented readers, with
        // the tag exactly as it was written.
        let (describe_status, described) =
            call(&router, "DescribeRescoreExecutionPlan", json!({ "Id": id })).await;
        assert_eq!(
            describe_status, 200,
            "case {class:?}: describe {id} answered {described}"
        );
        assert_eq!(
            described["Arn"].as_str().unwrap_or_default(),
            arn,
            "case {class:?}: describe must report the minted ARN, got {described}"
        );
        let listed = list_tags(&router, &arn).await;
        let expected = vec![(key.to_string(), (*text).to_string())];
        assert_eq!(
            listed, expected,
            "case {class:?}: expected {expected:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_create_naming_a_reserved_tag_key_prefix_mints_nothing() {
    let router = emulator();

    // The other half of the rule bounds the tag KEY alone: a caller may not write a key
    // under the prefix AWS reserves for itself, whatever case it spells it in (a tag key
    // is case-insensitive — "Keys are not case sensitive and must be unique." —
    // `Tag$Key`), while a key that merely starts with the same letters, the prefix away
    // from the key's start, and the same text in a VALUE are all admitted.
    let cases: Vec<(&str, &str, &str, bool)> = vec![
        ("the reserved prefix", "aws:createdBy", "me", false),
        ("the reserved prefix in upper case", "AWS:Owner", "me", false),
        ("the reserved prefix in mixed case", "Aws:cost", "me", false),
        ("the reserved letters without the colon", "aws", "v", true),
        ("a longer word starting with them", "awsome:team", "v", true),
        ("the prefix away from the key's start", "my-aws:key", "v", true),
        ("the prefix in a value, which it does not bound", "origin", "aws:cfn", true),
    ];

    for (index, (case, key, value, accepted)) in cases.into_iter().enumerate() {
        // One name per row, so the "no plan was minted" check of a refused row cannot be
        // confused by a plan an accepted row created.
        let name = format!("reserved-prefix-create-{index}");
        let (status, answer) = call(
            &router,
            "CreateRescoreExecutionPlan",
            json!({
                "Name": name,
                "Tags": [{ "Key": key, "Value": value }],
            }),
        )
        .await;
        if accepted {
            assert_eq!(status, 200, "case {case:?}: unexpected answer {answer}");
            let arn = answer["Arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no Arn in {answer}"));
            let listed = list_tags(&router, arn).await;
            let expected = vec![(key.to_string(), value.to_string())];
            assert_eq!(
                listed, expected,
                "case {case:?}: expected {expected:?}, got {listed:?}"
            );
        } else {
            assert_refused(case, status, &answer, "tags.1.member.key", key);
            let names: Vec<String> = listed_plans(&router)
                .await
                .into_iter()
                .map(|(name, _)| name)
                .collect();
            assert!(
                !names.iter().any(|minted| minted == &name),
                "case {case:?}: a refused create must mint no plan, got {names:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_refused_create_leaves_its_client_token_free_to_mint_a_plan() {
    let router = emulator();
    let token = "charset-idempotency-token";

    // The refusal reaches the request before anything is persisted, so the token it
    // carried was never registered against a plan.
    let (status, answer) = call(
        &router,
        "CreateRescoreExecutionPlan",
        json!({
            "Name": "token-create",
            "ClientToken": token,
            "Tags": [{ "Key": "cost[center]", "Value": "42" }],
        }),
    )
    .await;
    assert_refused(
        "a refused create carrying a client token",
        status,
        &answer,
        "tags.1.member.key",
        "cost[center]",
    );

    // The same token now mints a plan, which is what a free token does...
    let (status, minted) = call(
        &router,
        "CreateRescoreExecutionPlan",
        json!({
            "Name": "token-create",
            "ClientToken": token,
            "Tags": [{ "Key": "cost-center", "Value": "42" }],
        }),
    )
    .await;
    assert_eq!(status, 200, "the legal retry answered {status} {minted}");
    let id = minted["Id"].as_str().unwrap_or_default().to_string();
    assert!(
        !id.is_empty(),
        "the legal retry must mint an Id, got {minted}"
    );

    // ...and repeating it returns that same plan rather than a second one, so the
    // refused call consumed no slot in the token's history either.
    let (status, repeated) = call(
        &router,
        "CreateRescoreExecutionPlan",
        json!({
            "Name": "token-create",
            "ClientToken": token,
            "Tags": [{ "Key": "cost-center", "Value": "42" }],
        }),
    )
    .await;
    assert_eq!(status, 200, "the repeated token answered {status} {repeated}");
    assert_eq!(
        repeated["Id"].as_str().unwrap_or_default(),
        id,
        "the repeated token must return the plan it already created, got {repeated}"
    );

    let plans = listed_plans(&router).await;
    assert_eq!(
        plans.len(),
        1,
        "exactly the one legal create may have minted a plan, got {plans:?}"
    );
}

#[tokio::test]
async fn a_create_report_states_every_offending_position_at_once() {
    let router = emulator();

    // The standard AWS report states the violation count and one clause per offending
    // position, so a caller is told about all of them in one answer.
    let (status, answer) = call(
        &router,
        "CreateRescoreExecutionPlan",
        json!({
            "Name": "report-create",
            "Tags": [
                { "Key": "legal", "Value": "v" },
                { "Key": "cost[center]", "Value": "delta\u{b1}5" },
                { "Key": "env", "Value": "a\u{2022}b" },
            ],
        }),
    )
    .await;
    assert_eq!(status, 400, "the mixed create answered {status} {answer}");
    let message = error_message(&answer);
    for expected in [
        "3 validation errors detected",
        "tags.2.member.key",
        "tags.2.member.value",
        "tags.3.member.value",
    ] {
        assert!(
            message.contains(expected),
            "the report must state {expected:?}, got {message:?}"
        );
    }
    assert!(
        !message.contains("tags.1."),
        "the report must not fault the legal tag, got {message:?}"
    );

    let plans = listed_plans(&router).await;
    assert!(
        plans.is_empty(),
        "the refused create must mint no plan, got {plans:?}"
    );
}
