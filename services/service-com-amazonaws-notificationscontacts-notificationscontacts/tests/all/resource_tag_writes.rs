// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the tag WRITES — `TagResource` and `UntagResource`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes, and every tag set is read
//! back through `ListTagsForResource` rather than out of the store.
//!
//! The rules under test:
//!
//! * `TagResource` MERGES the request's pairs into the tag set already persisted on
//!   the email contact its `arn` names — a key the request repeats takes the request's
//!   value, a key it does not mention keeps the value it had — so tag calls naming
//!   different keys all survive, and the stored set never grows past the modelled
//!   `TagMap` maximum.
//! * `UntagResource` leaves exactly the keys the request does NOT name, so a key the
//!   contact never carried removes nothing and still succeeds.
//! * Both are writes to the contact's tag set only: the members the `EmailContact`
//!   shape reports — including `updateTime` — are untouched, and a well-formed ARN
//!   naming no stored contact is the modelled 404.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::email_contacts::{
    as_millis, call, create_ok, emulator, error_code, get_ok, label, unknown_arn, Emulator,
};
use crate::resource_tags::{
    create_tagged, pair, tag_map, tags_ok, MAX_TAGS, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN,
};

/// Percent-encode one `@httpQuery` value: every byte outside the unreserved set
/// travels as `%XX`, which is what an SDK writes for a `tagKeys` element.
fn query_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// `POST /tags/{arn}` with the tag map in the body — `TagResource`.
async fn tag(em: &Emulator, arn: &str, tags: &BTreeMap<String, String>) -> (u16, Value) {
    let body = json!({
        "tags": Value::Object(
            tags.iter()
                .map(|(key, value)| (key.clone(), json!(value)))
                .collect(),
        )
    });
    call(em, "POST", &format!("/tags/{}", label(arn)), Some(&body)).await
}

/// `DELETE /tags/{arn}?tagKeys=…` — `UntagResource`. `tagKeys` is an `@httpQuery`
/// list, so each key travels as its own repeated `tagKeys=` parameter.
async fn untag(em: &Emulator, arn: &str, keys: &[String]) -> (u16, Value) {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key)))
        .collect();
    call(
        em,
        "DELETE",
        &format!("/tags/{}?{}", label(arn), query.join("&")),
        None,
    )
    .await
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// A tag set of `count` distinct pairs, named off `prefix` so two sets never collide.
fn generated(prefix: &str, count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|n| pair(&format!("{prefix}-{n:03}"), &format!("value-{n:03}")))
        .collect()
}

/// Every row is one `TagResource` call against one starting set, differing along what
/// the rule generalises over: how many pairs the request carries, whether its keys are
/// new or already present, how long its keys and values are, and which characters they
/// carry.
#[allow(clippy::type_complexity)]
fn merge_table() -> Vec<(
    &'static str,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
)> {
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let long_value = "v".repeat(MAX_TAG_VALUE_LEN);
    vec![
        (
            "one pair onto an untagged contact",
            BTreeMap::new(),
            tag_map([pair("team", "infra")]),
            tag_map([pair("team", "infra")]),
        ),
        (
            "several pairs onto an untagged contact",
            BTreeMap::new(),
            tag_map([pair("team", "infra"), pair("env", "prod")]),
            tag_map([pair("team", "infra"), pair("env", "prod")]),
        ),
        (
            "a distinct key beside the ones already stored",
            tag_map([pair("team", "infra")]),
            tag_map([pair("env", "prod")]),
            tag_map([pair("team", "infra"), pair("env", "prod")]),
        ),
        (
            "a repeated key takes the request's value",
            tag_map([pair("team", "infra")]),
            tag_map([pair("team", "platform")]),
            tag_map([pair("team", "platform")]),
        ),
        (
            "one request both replacing and adding",
            tag_map([pair("team", "infra"), pair("env", "dev")]),
            tag_map([pair("env", "prod"), pair("owner", "on-call")]),
            tag_map([
                pair("team", "infra"),
                pair("env", "prod"),
                pair("owner", "on-call"),
            ]),
        ),
        (
            "an empty tag map changes nothing",
            tag_map([pair("team", "infra")]),
            BTreeMap::new(),
            tag_map([pair("team", "infra")]),
        ),
        (
            "an empty value, the TagValue minimum",
            tag_map([pair("retired", "no")]),
            tag_map([pair("retired", "")]),
            tag_map([pair("retired", "")]),
        ),
        (
            "a key at the TagKey maximum length",
            tag_map([pair("team", "infra")]),
            tag_map([(long_key.clone(), "at-the-bound".to_string())]),
            tag_map([pair("team", "infra"), (long_key, "at-the-bound".to_string())]),
        ),
        (
            "a value at the TagValue maximum length",
            BTreeMap::new(),
            tag_map([("long".to_string(), long_value.clone())]),
            tag_map([("long".to_string(), long_value)]),
        ),
        (
            "keys and values carrying punctuation and non-ascii text",
            tag_map([pair("cost:center", "eu-west/1")]),
            tag_map([
                pair("cost:center", "eu-west/42"),
                pair("équipe", "naïve value"),
                pair("spaced key", "spaced value"),
            ]),
            tag_map([
                pair("cost:center", "eu-west/42"),
                pair("équipe", "naïve value"),
                pair("spaced key", "spaced value"),
            ]),
        ),
        (
            "a request filling the contact to the TagMap maximum",
            generated("stored", MAX_TAGS / 2),
            generated("added", MAX_TAGS / 2),
            generated("stored", MAX_TAGS / 2)
                .into_iter()
                .chain(generated("added", MAX_TAGS / 2))
                .collect(),
        ),
    ]
}

#[tokio::test]
async fn tag_resource_merges_the_request_into_the_persisted_tag_set() {
    let em = emulator();
    let table = merge_table();
    assert!(
        table.len() >= 5,
        "the merge table must span the class, got {} rows",
        table.len()
    );

    for (index, (case, initial, request, expected)) in table.iter().enumerate() {
        let arn = create_tagged(&em, &format!("contact-{index}"), Some(initial)).await;
        let before = get_ok(&em, &arn).await;

        let (status, answer) = tag(&em, &arn, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: TagResource must answer 200, got {status} / {answer:?}"
        );

        let reported = tags_ok(&em, &arn).await;
        assert_eq!(
            &reported, expected,
            "case {case:?}: the merged tag set must be persisted, got {reported:?}"
        );

        // `@idempotent`: repeating the very same request lands the same set.
        let (status, answer) = tag(&em, &arn, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: repeating TagResource must answer 200, got {status} / {answer:?}"
        );
        let again = tags_ok(&em, &arn).await;
        assert_eq!(
            &again, expected,
            "case {case:?}: repeating the request must leave the same set, got {again:?}"
        );

        // Tags are metadata beside the contact: the members `EmailContact` reports,
        // `updateTime` included, are the ones the create stored.
        let after = get_ok(&em, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: tagging must not touch the contact's own members, got {after:?}"
        );
        assert_eq!(
            as_millis(&after, "updateTime"),
            as_millis(&before, "updateTime"),
            "case {case:?}: tagging must not move updateTime, got {after:?}"
        );
    }
}

/// Every row removes one key list from one starting set, differing along how many keys
/// it names, whether they are present, and what characters they carry.
#[allow(clippy::type_complexity)]
fn removal_table() -> Vec<(
    &'static str,
    BTreeMap<String, String>,
    Vec<String>,
    BTreeMap<String, String>,
)> {
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    vec![
        (
            "one key out of several",
            tag_map([pair("team", "infra"), pair("env", "prod")]),
            keys(&["team"]),
            tag_map([pair("env", "prod")]),
        ),
        (
            "a key the contact never carried removes nothing",
            tag_map([pair("team", "infra")]),
            keys(&["never-set"]),
            tag_map([pair("team", "infra")]),
        ),
        (
            "a key naming nothing on an untagged contact",
            BTreeMap::new(),
            keys(&["never-set"]),
            BTreeMap::new(),
        ),
        (
            "a mix of present and absent keys",
            tag_map([pair("team", "infra"), pair("env", "prod")]),
            keys(&["team", "never-set"]),
            tag_map([pair("env", "prod")]),
        ),
        (
            "every key the contact carries",
            tag_map([pair("team", "infra"), pair("env", "prod"), pair("tier", "1")]),
            keys(&["team", "env", "tier"]),
            BTreeMap::new(),
        ),
        (
            "a key whose value is empty",
            tag_map([pair("retired", ""), pair("team", "infra")]),
            keys(&["retired"]),
            tag_map([pair("team", "infra")]),
        ),
        (
            "keys carrying punctuation and non-ascii text",
            tag_map([
                pair("cost:center", "eu-west/42"),
                pair("équipe", "naïve value"),
                pair("spaced key", "spaced value"),
                pair("team", "infra"),
            ]),
            keys(&["cost:center", "équipe", "spaced key"]),
            tag_map([pair("team", "infra")]),
        ),
        (
            "a key at the TagKey maximum length",
            tag_map([(long_key.clone(), "at-the-bound".to_string()), pair("team", "infra")]),
            vec![long_key],
            tag_map([pair("team", "infra")]),
        ),
        (
            "every key of a contact holding the TagMap maximum",
            generated("stored", MAX_TAGS),
            generated("stored", MAX_TAGS).into_keys().collect(),
            BTreeMap::new(),
        ),
    ]
}

#[tokio::test]
async fn untag_resource_leaves_exactly_the_keys_it_does_not_name() {
    let em = emulator();
    let table = removal_table();
    assert!(
        table.len() >= 5,
        "the removal table must span the class, got {} rows",
        table.len()
    );

    for (index, (case, initial, removed, expected)) in table.iter().enumerate() {
        let arn = create_tagged(&em, &format!("contact-{index}"), Some(initial)).await;
        let before = get_ok(&em, &arn).await;

        let (status, answer) = untag(&em, &arn, removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UntagResource must answer 200, got {status} / {answer:?}"
        );

        let reported = tags_ok(&em, &arn).await;
        assert_eq!(
            &reported, expected,
            "case {case:?}: only the keys not named may survive, got {reported:?}"
        );

        // `@idempotent`: the second removal has nothing left to remove and still
        // succeeds with the same set.
        let (status, answer) = untag(&em, &arn, removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: repeating UntagResource must answer 200, got {status} / {answer:?}"
        );
        let again = tags_ok(&em, &arn).await;
        assert_eq!(
            &again, expected,
            "case {case:?}: repeating the removal must leave the same set, got {again:?}"
        );

        let after = get_ok(&em, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: untagging must not touch the contact's own members, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_write_lands_on_the_named_contact_only() {
    let em = emulator();
    // Three contacts created with the same starting set, so a handler writing to a
    // single shared map — or to the wrong record — cannot pass.
    let start = tag_map([pair("team", "shared"), pair("env", "shared")]);
    let mut arns = Vec::new();
    for name in ["alpha", "beta", "gamma"] {
        arns.push(create_tagged(&em, name, Some(&start)).await);
    }

    assert_eq!(
        tag(&em, &arns[0], &tag_map([pair("team", "alpha-only")]))
            .await
            .0,
        200,
        "tagging alpha must succeed"
    );
    assert_eq!(
        untag(&em, &arns[1], &keys(&["env"])).await.0,
        200,
        "untagging beta must succeed"
    );

    let wanted = [
        (
            "the contact that was tagged",
            tag_map([pair("team", "alpha-only"), pair("env", "shared")]),
        ),
        ("the contact that was untagged", tag_map([pair("team", "shared")])),
        (
            "the contact neither call named",
            tag_map([pair("team", "shared"), pair("env", "shared")]),
        ),
    ];
    for (arn, (case, expected)) in arns.iter().zip(wanted.iter()) {
        let reported = tags_ok(&em, arn).await;
        assert_eq!(
            &reported, expected,
            "case {case:?}: a write must land on the named contact only, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_write_on_an_arn_naming_no_contact_is_a_modelled_404() {
    let em = emulator();
    let survivor = create_tagged(&em, "survivor", Some(&tag_map([pair("keep", "me")]))).await;
    let deleted = create_ok(&em, "doomed", "doomed@example.com").await;
    assert_eq!(
        call(&em, "DELETE", &format!("/emailcontacts/{}", label(&deleted)), None)
            .await
            .0,
        200,
        "the delete must succeed"
    );

    for (case, arn) in [
        ("an arn this store never minted", unknown_arn('q')),
        ("another well-formed unknown arn", unknown_arn('7')),
        ("a deleted contact's arn", deleted.clone()),
    ] {
        for (operation, (status, body)) in [
            ("TagResource", tag(&em, &arn, &tag_map([pair("team", "ghost")])).await),
            ("UntagResource", untag(&em, &arn, &keys(&["team"])).await),
        ] {
            assert_eq!(
                status, 404,
                "case {case:?}/{operation}: an arn naming nothing must answer 404, \
                 got {status} / {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}/{operation}: expected ResourceNotFoundException, got {body:?}"
            );
        }
    }

    // `arn`'s own modelled pattern is refused at the boundary, before either handler.
    let account = engine::EMULATOR_ACCOUNT_ID;
    for (case, arn) in [
        (
            "an id that is too short",
            format!("arn:aws:notifications-contacts::{account}:emailcontact/abc"),
        ),
        (
            "an arn carrying a region",
            format!(
                "arn:aws:notifications-contacts:us-west-2:{account}:emailcontact/{}",
                "a".repeat(27)
            ),
        ),
        ("not an arn at all", "just-a-name".to_string()),
    ] {
        for (operation, (status, body)) in [
            ("TagResource", tag(&em, &arn, &tag_map([pair("team", "ghost")])).await),
            ("UntagResource", untag(&em, &arn, &keys(&["team"])).await),
        ] {
            assert_eq!(
                status, 400,
                "case {case:?}/{operation}: a malformed arn must be refused, \
                 got {status} / {body:?}"
            );
        }
    }

    // None of the refusals disturbed the contact that does exist.
    let reported = tags_ok(&em, &survivor).await;
    assert_eq!(
        reported,
        tag_map([pair("keep", "me")]),
        "the surviving contact must still report its own tags, got {reported:?}"
    );
}

#[tokio::test]
async fn a_merge_past_the_modelled_tag_map_maximum_is_refused_and_persists_nothing() {
    let em = emulator();
    // At the bound: the contact already holds the modelled `TagMap` maximum, which
    // `ListTagsForResource` reports through a member carrying that same maximum.
    let full = generated("stored", MAX_TAGS);
    let arn = create_tagged(&em, "full", Some(&full)).await;

    // Replacing values at the bound does not grow the set, so it is accepted.
    let replacement = tag_map([pair("stored-000", "replaced"), pair("stored-199", "replaced")]);
    let (status, answer) = tag(&em, &arn, &replacement).await;
    assert_eq!(
        status, 200,
        "case \"replacing values at the bound\": must answer 200, got {status} / {answer:?}"
    );
    let mut expected = full.clone();
    expected.extend(replacement);
    let reported = tags_ok(&em, &arn).await;
    assert_eq!(
        reported, expected,
        "case \"replacing values at the bound\": the values must be replaced, got {} pairs",
        reported.len()
    );

    // One past the bound: a single new key would take the stored set to 201.
    for (case, request) in [
        ("one new key past the bound", tag_map([pair("one-too-many", "x")])),
        (
            "a new key beside a replaced one",
            tag_map([pair("stored-000", "again"), pair("one-too-many", "x")]),
        ),
        ("several new keys past the bound", generated("extra", 5)),
    ] {
        let (status, body) = tag(&em, &arn, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a merge past the maximum must be refused, got {status} / {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        let reported = tags_ok(&em, &arn).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: a refused merge must persist nothing, got {} pairs",
            reported.len()
        );
    }

    // Freeing one slot lets exactly one new key in.
    assert_eq!(
        untag(&em, &arn, &keys(&["stored-000"])).await.0,
        200,
        "the removal must succeed"
    );
    let (status, answer) = tag(&em, &arn, &tag_map([pair("one-too-many", "x")])).await;
    assert_eq!(
        status, 200,
        "case \"a new key into the freed slot\": must answer 200, got {status} / {answer:?}"
    );
    let reported = tags_ok(&em, &arn).await;
    assert_eq!(
        reported.len(),
        MAX_TAGS,
        "case \"a new key into the freed slot\": the set must hold the maximum, got {} pairs",
        reported.len()
    );
    assert!(
        reported.contains_key("one-too-many") && !reported.contains_key("stored-000"),
        "case \"a new key into the freed slot\": the freed key must be gone and the new one \
         present, got {reported:?}"
    );
}
