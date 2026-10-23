// SPDX-License-Identifier: CC0-1.0

//! The OpenSearch Serverless **lifecycle policy** create/read path, end to end over the
//! wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client. The harness
//! is the one `collection_lifecycle` uses; nothing is rebuilt here.
//!
//! The rule under test: `CreateLifecyclePolicy` accepts a `retention` policy document that
//! follows the documented rule syntax, stores it under its `(type, name)` pair with a
//! server-minted `policyVersion` and the document held PARSED, and answers from that stored
//! record — so `BatchGetLifecyclePolicy` reports exactly what the create reported, each
//! identifier it cannot resolve is the modelled `NOT_FOUND` error detail, a second create
//! for a pair already stored is the conflict, and a replayed `clientToken` answers the
//! record the first call created. The tables span what the rule generalises over — the
//! window's unit (hours, days) and both published bounds plus one past each, unlimited
//! retention, the rule and resource COUNT, the document's whitespace, the `PolicyName`
//! length bounds, a described and an undescribed policy, and the whole
//! `LifecyclePolicyType` value list — not the one request the issue quoted.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, is_policy_version, text};

/// The values `LifecyclePolicyType` accepts on the wire
/// (`smithy_query(query="shape", shape="LifecyclePolicyType")` -> `values`): the single
/// value `retention`. Every case below is driven over this whole list.
const LIFECYCLE_POLICY_TYPES: [&str; 1] = ["retention"];

/// The account's lifecycle-policy cap, as `crate::state::MAX_LIFECYCLE_POLICIES` records it.
const MAX_LIFECYCLE_POLICIES: usize = 100;

/// The most identifiers one `BatchGetLifecyclePolicy` request may carry, from
/// `BatchGetLifecyclePolicyRequest$identifiers`' own `@length(min: 1, max: 40)`.
const MAX_IDENTIFIERS: usize = 40;

/// Now, in the epoch milliseconds `createdDate` / `lastModifiedDate` are modelled with.
///
/// The test keeps its own clock on purpose: the window a stamp is asserted inside has to
/// come from outside the implementation that produced the stamp.
fn now_epoch_millis() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the test clock is after the epoch")
            .as_millis(),
    )
    .expect("epoch milliseconds fit in an i64")
}

/// One create case: the policy name, the `policy` TEXT the request carries, and the
/// description it carries (or none).
struct Case {
    what: &'static str,
    name: String,
    policy: String,
    description: Option<String>,
}

/// The create table. Rows differ along what the rule generalises over: the retention
/// window's unit and magnitude (including both published bounds), unlimited retention, the
/// number of rules and of resources a rule names, the whitespace of the request text, the
/// `PolicyName` length bounds and whether a description is carried.
fn cases() -> Vec<Case> {
    vec![
        Case {
            what: "a window on the 24h floor, over a wildcard resource",
            name: "retain-one-day".to_string(),
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"24h"}]}"#.to_string(),
            description: Some("the shortest window the service serves".to_string()),
        },
        Case {
            what: "a window in days, over one named index, and no description",
            // `PolicyName` is @length(min: 3, max: 32): this is the minimum.
            name: "abc".to_string(),
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/orders"],"MinIndexRetention":"7d"}]}"#.to_string(),
            description: None,
        },
        Case {
            what: "a window on the 3650d ceiling",
            name: format!("a{}", "b".repeat(31)),
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"3650d"}]}"#.to_string(),
            description: Some(String::new()),
        },
        Case {
            what: "the same ceiling written in hours",
            name: "retain-87600-hours".to_string(),
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"87600h"}]}"#.to_string(),
            description: Some("d".repeat(1000)),
        },
        Case {
            what: "unlimited retention",
            name: "retain-forever".to_string(),
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"NoMinIndexRetention":true}]}"#.to_string(),
            description: Some("no minimum retention at all".to_string()),
        },
        Case {
            what: "two rules, one windowed and one unlimited, over several resources",
            name: "mixed-rules".to_string(),
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/logs-a","index/demo/logs-b"],"MinIndexRetention":"100d"},{"ResourceType":"index","Resource":["index/demo/audit"],"NoMinIndexRetention":true}]}"#.to_string(),
            description: Some("café au lait, and a number: 3".to_string()),
        },
        Case {
            what: "a request text carrying tabs, newlines and spaces",
            name: "whitespace-text".to_string(),
            policy: "{\n\t\"Rules\" : [\n\t\t{ \"ResourceType\" : \"index\",\n\t\t  \"Resource\" : [ \"index/demo/*\" ],\n\t\t  \"MinIndexRetention\" : \"48h\" }\n\t]\n}".to_string(),
            description: Some("the document is reported without the whitespace".to_string()),
        },
    ]
}

// ---------------------------------------------------------------------------
// CreateLifecyclePolicy + BatchGetLifecyclePolicy: the create persists, and the read
// answers the stored record.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_lifecycle_policy_create_and_read_are_served_from_state() {
    let router = emulator();
    let mut versions: BTreeSet<String> = BTreeSet::new();

    for kind in LIFECYCLE_POLICY_TYPES {
        for case in cases() {
            let label = format!(
                "CreateLifecyclePolicy({kind}/{}, {})",
                case.name, case.what
            );
            let mut request = json!({ "type": kind, "name": case.name, "policy": case.policy });
            if let Some(description) = &case.description {
                request["description"] = json!(description);
            }

            let before = now_epoch_millis();
            let (status, code, answer) = call(&router, "CreateLifecyclePolicy", request).await;
            let after = now_epoch_millis();
            assert_eq!(
                status, 200,
                "case {label}: expected 200, got {status} {code} with {answer}"
            );

            let detail = at(&answer, "lifecyclePolicyDetail").clone();
            assert_eq!(
                (text(&detail, "type"), text(&detail, "name")),
                (kind.to_string(), case.name.clone()),
                "case {label}: the detail names the stored policy, got {detail}"
            );

            let version = text(&detail, "policyVersion");
            assert!(
                is_policy_version(&version),
                "case {label}: policyVersion must match PolicyVersion's length and pattern, got {version:?}"
            );
            assert!(
                versions.insert(version.clone()),
                "case {label}: every policy write mints a fresh policyVersion, got a repeat of {version:?}"
            );

            // The document is reported PARSED — `LifecyclePolicyDetail$policy` is "the JSON
            // policy document without any whitespaces" — so it equals the JSON the request
            // text carried, whatever whitespace that text had.
            let expected: Value =
                serde_json::from_str(&case.policy).expect("the case policy is JSON");
            assert_eq!(
                at(&detail, "policy"),
                &expected,
                "case {label}: the policy document is the parsed request document, got {detail}"
            );

            assert_eq!(
                text(&detail, "description"),
                case.description.clone().unwrap_or_default(),
                "case {label}: the stored description is reported, got {detail}"
            );

            // A just-created policy was last modified when it was created, and both stamps
            // are the epoch milliseconds of the call that stored it.
            let created = at(&detail, "createdDate").as_i64().unwrap_or_default();
            let modified = at(&detail, "lastModifiedDate").as_i64().unwrap_or_default();
            assert_eq!(
                created, modified,
                "case {label}: a just-created policy's lastModifiedDate is its createdDate, got {detail}"
            );
            assert!(
                (before..=after).contains(&created),
                "case {label}: createdDate must be the epoch millis of the create, expected {before}..={after}, got {created}"
            );

            // The read-back reports exactly what the create reported, down to the version
            // and the timestamps: both answer the one stored record.
            let (status, code, read) = call(
                &router,
                "BatchGetLifecyclePolicy",
                json!({ "identifiers": [{ "type": kind, "name": case.name }] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case BatchGetLifecyclePolicy({kind}/{}): expected 200, got {status} {code} with {read}",
                case.name
            );
            assert_eq!(
                at(&read, "lifecyclePolicyDetails"),
                &json!([detail]),
                "case BatchGetLifecyclePolicy({kind}/{}): the read must answer the created record, got {read}",
                case.name
            );
            assert_eq!(
                at(&read, "lifecyclePolicyErrorDetails"),
                &json!([]),
                "case BatchGetLifecyclePolicy({kind}/{}): a resolved identifier carries no error, got {read}",
                case.name
            );
        }

        // Every row is still readable after the whole table ran, so a create never
        // overwrote a record it did not name. One request addresses them all: `identifiers`
        // is a list, and the details come back in request order.
        let identifiers: Vec<Value> = cases()
            .iter()
            .map(|case| json!({ "type": kind, "name": case.name }))
            .collect();
        let (status, code, read) = call(
            &router,
            "BatchGetLifecyclePolicy",
            json!({ "identifiers": identifiers }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case BatchGetLifecyclePolicy(the whole table): expected 200, got {status} {code} with {read}"
        );
        let names: Vec<String> = at(&read, "lifecyclePolicyDetails")
            .as_array()
            .map(|details| details.iter().map(|detail| text(detail, "name")).collect())
            .unwrap_or_default();
        assert_eq!(
            names,
            cases()
                .iter()
                .map(|case| case.name.clone())
                .collect::<Vec<String>>(),
            "case BatchGetLifecyclePolicy(the whole table): every row is answered, in request order, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// BatchGetLifecyclePolicy: an identifier that names no stored policy is the modelled
// NOT_FOUND error detail, beside the details of the ones that resolved.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_get_lifecycle_policy_reports_the_identifiers_it_cannot_resolve() {
    let router = emulator();

    let (status, code, answer) = call(
        &router,
        "CreateLifecyclePolicy",
        json!({
            "type": "retention",
            "name": "stored-policy",
            "policy": r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"30d"}]}"#,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateLifecyclePolicy(stored-policy): expected 200, got {status} {code} with {answer}"
    );
    let stored = at(&answer, "lifecyclePolicyDetail").clone();

    // A request mixing stored and unknown identifiers is a PARTIAL success: 200, the
    // details of what resolved and an error detail per identifier that did not.
    let (status, code, read) = call(
        &router,
        "BatchGetLifecyclePolicy",
        json!({
            "identifiers": [
                { "type": "retention", "name": "never-created" },
                { "type": "retention", "name": "stored-policy" },
                { "type": "retention", "name": "also-absent" },
                // The same identifier twice is answered twice: the read is per entry.
                { "type": "retention", "name": "stored-policy" },
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case BatchGetLifecyclePolicy(mixed identifiers): expected 200, got {status} {code} with {read}"
    );
    assert_eq!(
        at(&read, "lifecyclePolicyDetails"),
        &json!([stored, stored]),
        "case BatchGetLifecyclePolicy(mixed identifiers): the stored policy is answered once per entry, got {read}"
    );
    assert_eq!(
        at(&read, "lifecyclePolicyErrorDetails"),
        &json!([
            {
                "type": "retention",
                "name": "never-created",
                "errorCode": "NOT_FOUND",
                "errorMessage": "The specified Lifecycle Policy is not found.",
            },
            {
                "type": "retention",
                "name": "also-absent",
                "errorCode": "NOT_FOUND",
                "errorMessage": "The specified Lifecycle Policy is not found.",
            },
        ]),
        "case BatchGetLifecyclePolicy(mixed identifiers): each unresolved identifier is the modelled NOT_FOUND detail, got {read}"
    );

    // `identifiers` is @length(min: 1, max: 40): the maximum is served, and each bound is
    // probed on the bound and one past it.
    let full: Vec<Value> = (0..MAX_IDENTIFIERS)
        .map(|index| json!({ "type": "retention", "name": format!("absent-{index}") }))
        .collect();
    let (status, code, read) = call(
        &router,
        "BatchGetLifecyclePolicy",
        json!({ "identifiers": full }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case BatchGetLifecyclePolicy({MAX_IDENTIFIERS} identifiers): expected 200, got {status} {code} with {read}"
    );
    let errors = at(&read, "lifecyclePolicyErrorDetails")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        errors, MAX_IDENTIFIERS,
        "case BatchGetLifecyclePolicy({MAX_IDENTIFIERS} identifiers): one error detail per identifier, got {errors} in {read}"
    );

    let past_bound: Vec<Value> = (0..=MAX_IDENTIFIERS)
        .map(|index| json!({ "type": "retention", "name": format!("absent-{index}") }))
        .collect();
    let refusals: Vec<(&str, Value)> = vec![
        ("identifiers empty", json!({ "identifiers": [] })),
        (
            "identifiers one past the 40-entry maximum",
            json!({ "identifiers": past_bound }),
        ),
        (
            "an identifier type outside LifecyclePolicyType",
            json!({ "identifiers": [{ "type": "data", "name": "stored-policy" }] }),
        ),
        (
            "an identifier type in the wrong case",
            json!({ "identifiers": [{ "type": "Retention", "name": "stored-policy" }] }),
        ),
        (
            "an identifier name outside PolicyName's ^[a-z][a-z0-9-]+$",
            json!({ "identifiers": [{ "type": "retention", "name": "Bad_Name" }] }),
        ),
        (
            "an identifier missing its name",
            json!({ "identifiers": [{ "type": "retention" }] }),
        ),
        ("identifiers absent", json!({})),
    ];
    for (case, request) in refusals {
        let (status, code, answer) = call(&router, "BatchGetLifecyclePolicy", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case BatchGetLifecyclePolicy({case}): expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }

    // And the read changed nothing: the stored policy still reports what the create did.
    let (status, code, read) = call(
        &router,
        "BatchGetLifecyclePolicy",
        json!({ "identifiers": [{ "type": "retention", "name": "stored-policy" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case BatchGetLifecyclePolicy(stored-policy, after the reads): expected 200, got {status} {code} with {read}"
    );
    assert_eq!(
        at(&read, "lifecyclePolicyDetails"),
        &json!([stored]),
        "case BatchGetLifecyclePolicy(stored-policy, after the reads): a read must not change the record, got {read}"
    );
}

// ---------------------------------------------------------------------------
// CreateLifecyclePolicy: the refusals it owes — the duplicate pair, the modelled bounds,
// and the retention rule syntax, on each published window bound and one past it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_lifecycle_policy_refuses_a_duplicate_pair_and_an_invalid_input() {
    let router = emulator();
    let valid = r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"24h"}]}"#;

    let (status, code, first) = call(
        &router,
        "CreateLifecyclePolicy",
        json!({ "type": "retention", "name": "demo-retention", "policy": valid }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateLifecyclePolicy(demo-retention): expected 200, got {status} {code} with {first}"
    );
    let stored = at(&first, "lifecyclePolicyDetail").clone();

    // A second create for a pair already stored is the conflict the model declares...
    let (status, code, answer) = call(
        &router,
        "CreateLifecyclePolicy",
        json!({ "type": "retention", "name": "demo-retention", "policy": valid, "description": "a second create" }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case CreateLifecyclePolicy(duplicate pair): expected 409 ConflictException, got {status} {code} with {answer}"
    );

    // ... and it leaves the stored policy exactly as it was.
    let (status, code, read) = call(
        &router,
        "BatchGetLifecyclePolicy",
        json!({ "identifiers": [{ "type": "retention", "name": "demo-retention" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case BatchGetLifecyclePolicy(demo-retention, after the conflict): expected 200, got {status} {code} with {read}"
    );
    assert_eq!(
        at(&read, "lifecyclePolicyDetails"),
        &json!([stored]),
        "case BatchGetLifecyclePolicy(demo-retention, after the conflict): a refused create must not rewrite the record, got {read}"
    );

    // The modelled refusals: `type` outside `LifecyclePolicyType`'s value list, and each
    // modelled bound, on one past it.
    let modelled: Vec<(&str, Value)> = vec![
        (
            "type outside LifecyclePolicyType",
            json!({ "type": "data", "name": "bad-policy", "policy": valid }),
        ),
        (
            "type in the wrong case",
            json!({ "type": "Retention", "name": "bad-policy", "policy": valid }),
        ),
        (
            "name one short of the 3-character minimum",
            json!({ "type": "retention", "name": "ab", "policy": valid }),
        ),
        (
            "name one past the 32-character maximum",
            json!({ "type": "retention", "name": format!("a{}", "b".repeat(32)), "policy": valid }),
        ),
        (
            "name outside PolicyName's ^[a-z][a-z0-9-]+$",
            json!({ "type": "retention", "name": "Bad_Name", "policy": valid }),
        ),
        (
            "description one past the 1000-character maximum",
            json!({ "type": "retention", "name": "bad-policy", "policy": valid, "description": "d".repeat(1001) }),
        ),
        (
            "clientToken below the 1-character minimum",
            json!({ "type": "retention", "name": "bad-policy", "policy": valid, "clientToken": "" }),
        ),
        (
            "policy absent",
            json!({ "type": "retention", "name": "bad-policy" }),
        ),
        (
            "name absent",
            json!({ "type": "retention", "policy": valid }),
        ),
    ];

    // The document refusals: the `retention` rule syntax AWS documents, each dimension on
    // its own, and the window range on each published bound's far side — the bounds
    // THEMSELVES are accepted in the create table above.
    let documents: Vec<(&str, Value)> = vec![
        ("not JSON at all", json!("not json at all")),
        ("a document with no Rules", json!({ "Policy": "retain" })),
        ("Rules that is not a list", json!({ "Rules": "all of them" })),
        ("Rules with no rule in it", json!({ "Rules": [] })),
        ("a rule that is not an object", json!({ "Rules": ["index/demo/*"] })),
        (
            "a rule with no ResourceType",
            json!({ "Rules": [{ "Resource": ["index/demo/*"], "MinIndexRetention": "24h" }] }),
        ),
        (
            "a ResourceType outside the model's value list",
            json!({ "Rules": [{ "ResourceType": "collection", "Resource": ["index/demo/*"], "MinIndexRetention": "24h" }] }),
        ),
        (
            "a rule with no Resource",
            json!({ "Rules": [{ "ResourceType": "index", "MinIndexRetention": "24h" }] }),
        ),
        (
            "a rule whose Resource names nothing",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": [], "MinIndexRetention": "24h" }] }),
        ),
        (
            "a Resource entry that is not a resource name",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": [7], "MinIndexRetention": "24h" }] }),
        ),
        (
            "a rule naming both a window and unlimited retention",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": ["index/demo/*"], "MinIndexRetention": "24h", "NoMinIndexRetention": true }] }),
        ),
        (
            "a rule naming neither a window nor unlimited retention",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": ["index/demo/*"] }] }),
        ),
        (
            "NoMinIndexRetention that is not a boolean",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": ["index/demo/*"], "NoMinIndexRetention": "yes" }] }),
        ),
        (
            "a window that is not a string",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": ["index/demo/*"], "MinIndexRetention": 24 }] }),
        ),
        (
            "a window with no unit",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": ["index/demo/*"], "MinIndexRetention": "24" }] }),
        ),
        (
            "a window in an unsupported unit",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": ["index/demo/*"], "MinIndexRetention": "4w" }] }),
        ),
        (
            "a window with no count",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": ["index/demo/*"], "MinIndexRetention": "h" }] }),
        ),
        (
            "a window whose count carries a sign",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": ["index/demo/*"], "MinIndexRetention": "+24h" }] }),
        ),
        (
            "a window one hour short of the 24h floor",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": ["index/demo/*"], "MinIndexRetention": "23h" }] }),
        ),
        (
            "a window one day past the 3650d ceiling",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": ["index/demo/*"], "MinIndexRetention": "3651d" }] }),
        ),
        (
            "a window one hour past the same ceiling",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": ["index/demo/*"], "MinIndexRetention": "87601h" }] }),
        ),
        (
            "a second rule that is invalid",
            json!({ "Rules": [
                { "ResourceType": "index", "Resource": ["index/demo/a"], "MinIndexRetention": "24h" },
                { "ResourceType": "index", "Resource": ["index/demo/b"], "MinIndexRetention": "1h" },
            ] }),
        ),
    ];

    let refusals = modelled.into_iter().chain(documents.into_iter().map(
        |(case, document)| {
            let policy = match document.as_str() {
                Some(text) => text.to_string(),
                None => document.to_string(),
            };
            (
                case,
                json!({ "type": "retention", "name": "bad-policy", "policy": policy }),
            )
        },
    ));
    for (case, request) in refusals {
        let (status, code, answer) = call(&router, "CreateLifecyclePolicy", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case CreateLifecyclePolicy({case}): expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }

    // A refused create persists nothing: the name every refusal above used names no policy.
    let (status, code, read) = call(
        &router,
        "BatchGetLifecyclePolicy",
        json!({ "identifiers": [{ "type": "retention", "name": "bad-policy" }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case BatchGetLifecyclePolicy(bad-policy): expected 200, got {status} {code} with {read}"
    );
    assert_eq!(
        at(&read, "lifecyclePolicyDetails"),
        &json!([]),
        "case BatchGetLifecyclePolicy(bad-policy): a refused create must persist nothing, got {read}"
    );

    // The modelled maxima themselves are accepted: a 20480-character document and a
    // 1000-character description.
    let filler = 20480 - r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/"],"MinIndexRetention":"24h"}]}"#.len();
    let at_bounds = json!({
        "type": "retention",
        "name": "bound-policy",
        "policy": format!(
            r#"{{"Rules":[{{"ResourceType":"index","Resource":["index/demo/{}"],"MinIndexRetention":"24h"}}]}}"#,
            "x".repeat(filler)
        ),
        "description": "d".repeat(1000),
    });
    let policy_len = at_bounds["policy"].as_str().unwrap_or_default().len();
    assert_eq!(
        policy_len, 20480,
        "case CreateLifecyclePolicy(policy at the 20480-character maximum): the case itself must sit on the bound, got {policy_len}"
    );
    let (status, code, answer) = call(&router, "CreateLifecyclePolicy", at_bounds).await;
    assert_eq!(
        status, 200,
        "case CreateLifecyclePolicy(policy and description on their maxima): expected 200, got {status} {code} with {answer}"
    );
}

// ---------------------------------------------------------------------------
// CreateLifecyclePolicy: the `@idempotencyToken` replay.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_answers_the_lifecycle_policy_it_first_created() {
    let router = emulator();
    let request = json!({
        "type": "retention",
        "name": "token-policy",
        "policy": r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"24h"}]}"#,
        "clientToken": "a-caller-supplied-token",
    });

    let (status, code, first) = call(&router, "CreateLifecyclePolicy", request.clone()).await;
    assert_eq!(
        status, 200,
        "case CreateLifecyclePolicy(token-policy): expected 200, got {status} {code} with {first}"
    );
    let stored = at(&first, "lifecyclePolicyDetail").clone();

    // The same token again is a RETRY of that call: the same record, version and stamps —
    // not a second policy and not a conflict.
    for (case, body) in [
        ("the identical retry", request.clone()),
        (
            "a retry whose body differs",
            json!({
                "type": "retention",
                "name": "token-policy",
                "policy": r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"NoMinIndexRetention":true}]}"#,
                "description": "a different body under the same token",
                "clientToken": "a-caller-supplied-token",
            }),
        ),
    ] {
        let (status, code, answer) = call(&router, "CreateLifecyclePolicy", body).await;
        assert_eq!(
            status, 200,
            "case CreateLifecyclePolicy({case}): expected 200, got {status} {code} with {answer}"
        );
        assert_eq!(
            at(&answer, "lifecyclePolicyDetail"),
            &stored,
            "case CreateLifecyclePolicy({case}): a replayed clientToken answers the first record, got {answer}"
        );
    }

    // A DIFFERENT token for the same pair is a new request for a name already taken.
    let (status, code, answer) = call(
        &router,
        "CreateLifecyclePolicy",
        json!({
            "type": "retention",
            "name": "token-policy",
            "policy": r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"24h"}]}"#,
            "clientToken": "another-token",
        }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case CreateLifecyclePolicy(a fresh token for a stored pair): expected 409 ConflictException, got {status} {code} with {answer}"
    );

    // The token index is keyed by operation as well, so the same token spent on another
    // create is an independent resource rather than a replay of this one.
    let (status, code, answer) = call(
        &router,
        "CreateAccessPolicy",
        json!({
            "type": "data",
            "name": "token-policy",
            "policy": r#"{"Rules":[]}"#,
            "clientToken": "a-caller-supplied-token",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateAccessPolicy(the lifecycle policy's token): a token is per-operation, got {status} {code} with {answer}"
    );
    assert_eq!(
        text(at(&answer, "accessPolicyDetail"), "type"),
        "data",
        "case CreateAccessPolicy(the lifecycle policy's token): the answer must be the access policy it created, got {answer}"
    );
}

// ---------------------------------------------------------------------------
// CreateLifecyclePolicy: the account cap, answered with the modelled quota error.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_lifecycle_policy_refuses_the_request_past_the_account_cap() {
    let router = emulator();
    let policy = r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"24h"}]}"#;

    for index in 0..MAX_LIFECYCLE_POLICIES {
        let name = format!("cap-policy-{index}");
        let (status, code, answer) = call(
            &router,
            "CreateLifecyclePolicy",
            json!({ "type": "retention", "name": name, "policy": policy }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case CreateLifecyclePolicy({name}): expected 200 inside the cap, got {status} {code} with {answer}"
        );
    }

    let (status, code, answer) = call(
        &router,
        "CreateLifecyclePolicy",
        json!({ "type": "retention", "name": "cap-policy-one-too-many", "policy": policy }),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (402, "ServiceQuotaExceededException"),
        "case CreateLifecyclePolicy(one past the {MAX_LIFECYCLE_POLICIES}-policy cap): expected 402 ServiceQuotaExceededException, got {status} {code} with {answer}"
    );
}
