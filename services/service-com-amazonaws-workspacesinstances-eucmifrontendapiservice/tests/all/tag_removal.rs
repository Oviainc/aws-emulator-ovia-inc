// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` across the class of key lists a caller can send.
//!
//! The rule: untagging a resource REMOVES the named keys from the tag set the resource
//! has stored and leaves every other tag of that resource alone — the stored value of a
//! key plays no part in whether it goes, a key the resource does not carry is nothing
//! to remove rather than a failure, and the call therefore lands on the same tag set
//! however many times it is repeated. A resource no record of the request's Region
//! holds is reported absent and nothing is removed anywhere.
//!
//! Every row drives the real wire (awsJson1_0 through the router) and reads the result
//! back out of the store through `ListTagsForResource`, so no canned answer can pass:
//! the expected tag set of each row is COMPUTED from what the row stored minus the keys
//! it asked to remove, never written out by hand.
//!
//! The wire helpers come from the sibling `workspace_instance_lifecycle` suite.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_workspacesinstances_eucmifrontendapiservice as wsinst;

use crate::workspace_instance_lifecycle::{
    call, call_in_region, create, error_code, launchable_managed_instance, ok, router, stored_tags,
    DEFAULT_TEST_REGION,
};

/// One row of the table: the tag set the instance is created with, and the `TagKeys`
/// list the untag names.
struct Row {
    case: &'static str,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
}

/// A row from borrowed literals.
fn row(case: &'static str, initial: &[(&str, &str)], remove: &[&str]) -> Row {
    Row {
        case,
        initial: initial
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        remove: remove.iter().map(|key| key.to_string()).collect(),
    }
}

/// The modelled maximum length of a `TagKey`: `len 1..128`
/// (`smithy_query(query="shape",shape="TagKey")`).
const TAG_KEY_MAX_LENGTH: usize = 128;

/// A key of exactly `length` characters, inside the modelled `TagKey` pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`.
fn key_of_length(length: usize) -> String {
    "k".repeat(length)
}

/// The table: rows differ in how many keys are named, whether each names a stored key,
/// whether a key repeats, the case and script of the keys, the length of a key at the
/// modelled bounds, and the stored value a removed key carried.
fn rows() -> Vec<Row> {
    let three = [("env", "dev"), ("team", "euc"), ("owner", "ana")];
    let mut table = vec![
        row("one key of several", &three, &["team"]),
        row("every key the resource carries", &three, &["env", "team", "owner"]),
        row("a key the resource does not carry", &[("env", "dev")], &["absent"]),
        row(
            "stored and absent keys in one list",
            &three,
            &["team", "absent", "owner"],
        ),
        row("the same key named twice", &three, &["env", "env"]),
        // Tag keys are case-sensitive, so a key differing only in case is a DIFFERENT
        // key and survives the removal of its neighbour.
        row(
            "a key differing from a stored one only in case",
            &[("Env", "dev"), ("env", "prod")],
            &["Env"],
        ),
        // The modelled `TagValue` is `len 0..256`, so a stored value can be empty: the
        // value plays no part in whether the key goes.
        row(
            "a key whose stored value is empty",
            &[("env", ""), ("team", "euc")],
            &["env"],
        ),
        row(
            "a key at the modelled 1-character minimum",
            &[("a", "1"), ("ab", "2")],
            &["a"],
        ),
        // Every character class the modelled `TagKey` pattern admits, on a key the
        // issue did not cite.
        row(
            "a key spanning the pattern's character classes",
            &[("Kéy.1:a/b=c+d-e@f", "v"), ("env", "dev")],
            &["Kéy.1:a/b=c+d-e@f"],
        ),
        row(
            "a non-ASCII key",
            &[("étiquette", "oui"), ("env", "dev")],
            &["étiquette"],
        ),
    ];

    // A key at the modelled maximum length, and one a character past it would be
    // refused at the boundary (the `validation_messages` suite owns that refusal), so
    // the maximum is the longest key this operation ever reaches.
    let longest = key_of_length(TAG_KEY_MAX_LENGTH);
    table.push(Row {
        case: "a key at the modelled 128-character maximum",
        initial: vec![
            (longest.clone(), "v".to_string()),
            ("env".to_string(), "dev".to_string()),
        ],
        remove: vec![longest],
    });

    // The `TagKeys` list at its modelled `len 1..50` maximum
    // (`smithy_query(query="shape",shape="TagKeyList")`) against a resource holding the
    // most tags it can hold: every stored key goes in one call.
    let full: Vec<(String, String)> = (0..wsinst::state::MAX_TAGS_PER_INSTANCE)
        .map(|n| (format!("key-{n:02}"), format!("value-{n:02}")))
        .collect();
    table.push(Row {
        case: "a full tag set removed by a list at the modelled 50 maximum",
        initial: full.clone(),
        remove: full.iter().map(|(key, _)| key.clone()).collect(),
    });
    // The same maximum-length list, half of it naming keys the resource does not carry.
    let mut half_and_half: Vec<String> = full
        .iter()
        .take(wsinst::state::MAX_TAGS_PER_INSTANCE / 2)
        .map(|(key, _)| key.clone())
        .collect();
    half_and_half.extend((0..wsinst::state::MAX_TAGS_PER_INSTANCE / 2).map(|n| format!("gone-{n}")));
    table.push(Row {
        case: "a maximum-length list half of whose keys are not stored",
        initial: full,
        remove: half_and_half,
    });

    table
}

/// A `Tags` list body for `CreateWorkspaceInstance`.
fn tags_body(pairs: &[(String, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// What the row's tag set must be after the removal: what it stored, minus every key
/// the request named.
fn expected_after(row: &Row) -> BTreeMap<String, String> {
    let mut expected: BTreeMap<String, String> = row.initial.iter().cloned().collect();
    for key in &row.remove {
        expected.remove(key);
    }
    expected
}

#[tokio::test]
async fn untag_removes_exactly_the_named_keys_and_leaves_the_rest() {
    let router = router();

    for row in rows() {
        let case = row.case;
        let id = create(
            &router,
            json!({
                "ManagedInstance": launchable_managed_instance(),
                "Tags": tags_body(&row.initial),
            }),
        )
        .await;
        let stored: BTreeMap<String, String> = row.initial.iter().cloned().collect();
        let before = stored_tags(&router, &id).await;
        assert_eq!(
            before, stored,
            "case {case:?}: the create did not store the row's tags, got {before:?}"
        );

        let answered = ok(
            &router,
            "UntagResource",
            json!({ "WorkspaceInstanceId": &id, "TagKeys": &row.remove }),
        )
        .await;
        // `UntagResourceResponse` models no members, so the body carries none.
        assert_eq!(
            answered,
            json!({}),
            "case {case:?}: expected an empty response body, got {answered}"
        );

        let expected = expected_after(&row);
        let after = stored_tags(&router, &id).await;
        assert_eq!(
            after, expected,
            "case {case:?}: expected the tag set {expected:?}, got {after:?}"
        );

        // Removal is by key against what is stored, so repeating the same call lands on
        // the same tag set.
        ok(
            &router,
            "UntagResource",
            json!({ "WorkspaceInstanceId": &id, "TagKeys": &row.remove }),
        )
        .await;
        let repeated = stored_tags(&router, &id).await;
        assert_eq!(
            repeated, expected,
            "case {case:?} repeated: expected the tag set {expected:?}, got {repeated:?}"
        );
    }
}

#[tokio::test]
async fn untag_leaves_another_instances_tags_alone() {
    let router = router();
    let shared = [("env", "dev"), ("team", "euc")];
    let body = json!({
        "ManagedInstance": launchable_managed_instance(),
        "Tags": tags_body(
            &shared
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<Vec<_>>(),
        ),
    });
    let first = create(&router, body.clone()).await;
    let second = create(&router, body).await;

    ok(
        &router,
        "UntagResource",
        json!({ "WorkspaceInstanceId": &first, "TagKeys": ["env", "team"] }),
    )
    .await;

    let untagged = stored_tags(&router, &first).await;
    assert!(
        untagged.is_empty(),
        "case \"the instance named\": expected every key removed, got {untagged:?}"
    );
    let untouched = stored_tags(&router, &second).await;
    assert_eq!(
        untouched,
        shared
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<BTreeMap<String, String>>(),
        "case \"another instance carrying the same keys\": expected its tags untouched, \
         got {untouched:?}"
    );
}

#[tokio::test]
async fn untag_reports_an_instance_this_region_does_not_hold_as_absent() {
    let router = router();
    let stored: BTreeMap<String, String> = [("env".to_string(), "dev".to_string())]
        .into_iter()
        .collect();
    let id = create(
        &router,
        json!({
            "ManagedInstance": launchable_managed_instance(),
            "Tags": tags_body(&stored.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>()),
        }),
    )
    .await;

    // An id nobody minted, and the Region-foreign one below, are the same condition to
    // a caller: the instance is not one this endpoint holds.
    let foreign_region = wsinst::state::SUPPORTED_REGIONS
        .iter()
        .find(|region| **region != DEFAULT_TEST_REGION)
        .expect("the service supports more than one Region");
    let cases: Vec<(String, String, String)> = vec![
        (
            "an id nobody minted".to_string(),
            DEFAULT_TEST_REGION.to_string(),
            "wsinst-00000000".to_string(),
        ),
        (
            "an instance of another Region".to_string(),
            (*foreign_region).to_string(),
            id.clone(),
        ),
    ];

    for (case, region, target) in cases {
        let (status, payload) = call_in_region(
            &router,
            &region,
            "UntagResource",
            json!({ "WorkspaceInstanceId": &target, "TagKeys": ["env"] }),
        )
        .await;
        assert_eq!(
            status,
            wsinst::state::RESOURCE_NOT_FOUND_STATUS,
            "case {case:?}: expected HTTP {}, got {status} with {payload}",
            wsinst::state::RESOURCE_NOT_FOUND_STATUS
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {payload}"
        );
    }

    // The refused call removed nothing: the instance still carries what it was created
    // with in the Region that holds it.
    let after = stored_tags(&router, &id).await;
    assert_eq!(
        after, stored,
        "case \"a refused untag\": expected the stored tags untouched, got {after:?}"
    );
}

#[tokio::test]
async fn untag_refuses_a_reserved_key_and_removes_nothing() {
    let router = router();
    let stored: BTreeMap<String, String> = [
        ("env".to_string(), "dev".to_string()),
        ("team".to_string(), "euc".to_string()),
    ]
    .into_iter()
    .collect();
    let id = create(
        &router,
        json!({
            "ManagedInstance": launchable_managed_instance(),
            "Tags": tags_body(&stored.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>()),
        }),
    )
    .await;

    // The reserved namespace is AWS's own: a caller may neither set nor take away a key
    // in it. Each row satisfies the modelled `TagKey` constraints, so only the
    // handler's rule can refuse it — and the ordinary key sent beside it stays stored.
    for key in [
        "aws:cloudformation:stack-name",
        "AWS:Region",
        "Aws:CreatedBy",
        wsinst::state::RESERVED_TAG_KEY_PREFIX,
    ] {
        let (status, payload) = call(
            &router,
            "UntagResource",
            json!({ "WorkspaceInstanceId": &id, "TagKeys": [key, "env"] }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {key:?}: expected HTTP 400 for a reserved tag key, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {key:?}: expected ValidationException, got {payload}"
        );
        let named: Vec<&str> = payload["FieldList"]
            .as_array()
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(|field| field["Name"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            named.contains(&"TagKeys"),
            "case {key:?}: expected FieldList to name TagKeys, got {named:?} in {payload}"
        );

        let after = stored_tags(&router, &id).await;
        assert_eq!(
            after, stored,
            "case {key:?}: a refused untag removed a tag, got {after:?}"
        );
    }
}
