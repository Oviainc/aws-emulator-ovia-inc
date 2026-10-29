// SPDX-License-Identifier: CC0-1.0

//! The OpenSearch Serverless **lifecycle policy write** path: `UpdateLifecyclePolicy`,
//! proved through the read that follows it, end to end over the wire.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body, the wire
//! contract in this crate's `OPERATIONS.md`), through the harness this crate's
//! `collection_lifecycle` suite already owns, so the boundary validator, the protocol codec
//! and the handler all take part exactly as they do for an SDK client.
//!
//! The rule under test: a policy write replaces the members the request names, leaves the
//! ones it does not at their stored values, mints a fresh `policyVersion`, stamps
//! `lastModifiedDate` and answers from the record it stored — with the request's
//! `policyVersion` as the write's precondition, so a version that is not the stored one is
//! refused and writes nothing. The tables span what the rule generalises over: the whole
//! `LifecyclePolicyType` value list, which members the request names (both, each alone,
//! neither, an empty description), the retention window's unit and magnitude including both
//! published bounds and one past each, unlimited retention, the rule and resource COUNT, the
//! document's whitespace, the document's published length maximum and the `ClientToken`
//! length bounds.

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, is_policy_version, text, CLIENT_ERROR};

/// The values `LifecyclePolicyType` accepts on the wire
/// (`smithy_query(query="shape", shape="LifecyclePolicyType")` -> `values`): the single
/// value `retention`. Every case below is driven over this whole list.
const LIFECYCLE_POLICY_TYPES: [&str; 1] = ["retention"];

/// The document every policy below is created with, so a row that names no `policy` proves
/// the stored document survived the write.
const CREATED_POLICY: &str = r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"30d"}]}"#;

/// The description every policy below is created with, so a row that names no `description`
/// proves the stored description survived the write.
const CREATED_DESCRIPTION: &str = "the description the create stored";

/// A syntactically valid `PolicyVersion` that no write ever mints: 24 characters out of
/// `[0-9a-f]`, inside `PolicyVersion`'s `@length(min: 20, max: 36)` and its
/// `^([0-9a-zA-Z+/]{4})*...$` pattern, so the boundary validator accepts it and the handler
/// is what refuses it.
const STALE_VERSION: &str = "000000000000000000000000";

/// `PolicyDocument` is `@length(min: 1, max: 20480)`: the maximum a request may carry.
const MAX_POLICY_DOCUMENT: usize = 20480;

/// One write row: which members the request names, and the document text it carries when it
/// names one. A member this row leaves `None` is one the request does not name.
struct Row {
    what: &'static str,
    policy: Option<String>,
    description: Option<String>,
}

/// A `retention` document whose single rule's `Resource` entry is padded until the whole
/// text is exactly `MAX_POLICY_DOCUMENT` characters, so one row sits on the published
/// maximum.
fn policy_on_the_length_maximum() -> String {
    let skeleton =
        r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/"],"MinIndexRetention":"24h"}]}"#;
    let filler = MAX_POLICY_DOCUMENT - skeleton.len();
    format!(
        r#"{{"Rules":[{{"ResourceType":"index","Resource":["index/demo/{}"],"MinIndexRetention":"24h"}}]}}"#,
        "x".repeat(filler)
    )
}

/// The write table. Rows differ along what the rule generalises over: which members the
/// request names, the retention window's unit and magnitude (both published bounds
/// included), unlimited retention, the number of rules and of resources a rule names, the
/// whitespace of the request text, the document's length maximum and whether the description
/// is emptied.
fn rows() -> Vec<Row> {
    vec![
        Row {
            what: "a window on the 24h floor, and a new description",
            policy: Some(
                r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"24h"}]}"#
                    .to_string(),
            ),
            description: Some("the shortest window the service serves".to_string()),
        },
        Row {
            what: "a window on the 3650d ceiling, leaving the stored description",
            policy: Some(
                r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"3650d"}]}"#
                    .to_string(),
            ),
            description: None,
        },
        Row {
            what: "the same ceiling written in hours",
            policy: Some(
                r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"87600h"}]}"#
                    .to_string(),
            ),
            // `PolicyDescription` is @length(min: 0, max: 1000): the upper bound.
            description: Some("d".repeat(1000)),
        },
        Row {
            what: "unlimited retention",
            policy: Some(
                r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"NoMinIndexRetention":true}]}"#
                    .to_string(),
            ),
            description: Some("no minimum retention at all".to_string()),
        },
        Row {
            what: "two rules, one windowed and one unlimited, over several resources",
            policy: Some(
                r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/logs-a","index/demo/logs-b"],"MinIndexRetention":"100d"},{"ResourceType":"index","Resource":["index/demo/audit"],"NoMinIndexRetention":true}]}"#
                    .to_string(),
            ),
            // `é` and `—` are inside `PolicyDescription`'s own character range.
            description: Some("naïve café — non-ASCII text".to_string()),
        },
        Row {
            what: "a request text carrying tabs, newlines and spaces",
            policy: Some(
                "{\n\t\"Rules\" : [\n\t\t{ \"ResourceType\" : \"index\",\n\t\t  \"Resource\" : [ \"index/demo/*\" ],\n\t\t  \"MinIndexRetention\" : \"48h\" }\n\t]\n}"
                    .to_string(),
            ),
            description: Some("the document is reported without the whitespace".to_string()),
        },
        Row {
            what: "a document on the 20480-character maximum",
            policy: Some(policy_on_the_length_maximum()),
            description: None,
        },
        Row {
            what: "a description only, leaving the stored document",
            policy: None,
            description: Some("updated description".to_string()),
        },
        Row {
            what: "an empty description",
            policy: None,
            // `PolicyDescription` is @length(min: 0, max: 1000): the lower bound.
            description: Some(String::new()),
        },
        Row {
            what: "neither member named, so only the version moves",
            policy: None,
            description: None,
        },
    ]
}

/// Call `operation` with `body`, asserting it was accepted, and answer its
/// `lifecyclePolicyDetail`.
async fn accepted(
    router: &smithy_server_runtime::MultiServiceRouter,
    operation: &str,
    body: Value,
) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, "lifecyclePolicyDetail").clone()
}

/// The one lifecycle policy `(kind, name)` names, read back through
/// `BatchGetLifecyclePolicy`.
async fn read_back(
    router: &smithy_server_runtime::MultiServiceRouter,
    kind: &str,
    name: &str,
) -> Value {
    let (status, code, read) = call(
        router,
        "BatchGetLifecyclePolicy",
        json!({ "identifiers": [{ "type": kind, "name": name }] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case BatchGetLifecyclePolicy({kind}/{name}): expected 200, got {status} {code} with {read}"
    );
    at(&read, "lifecyclePolicyDetails")
        .as_array()
        .and_then(|details| details.first())
        .cloned()
        .unwrap_or(Value::Null)
}

/// Create one lifecycle policy with the shared document and description, answering its
/// detail.
async fn create(
    router: &smithy_server_runtime::MultiServiceRouter,
    kind: &str,
    name: &str,
) -> Value {
    accepted(
        router,
        "CreateLifecyclePolicy",
        json!({
            "type": kind,
            "name": name,
            "policy": CREATED_POLICY,
            "description": CREATED_DESCRIPTION,
        }),
    )
    .await
}

// ---------------------------------------------------------------------------
// A lifecycle policy write replaces the members the request names, mints a fresh version,
// and is exactly what the following read reports.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_lifecycle_policy_is_what_the_following_read_reports() {
    let router = emulator();

    for kind in LIFECYCLE_POLICY_TYPES {
        for (index, row) in rows().into_iter().enumerate() {
            // `PolicyName` is @length(min: 3, max: 32) with `^[a-z][a-z0-9-]+$`.
            let name = format!("upd-{kind}-{index}");
            let created = create(&router, kind, &name).await;
            let created_version = text(&created, "policyVersion");
            let created_date = at(&created, "createdDate").as_i64().unwrap_or_default();

            let case = format!("UpdateLifecyclePolicy({kind}/{name}, {})", row.what);
            let mut request = json!({
                "type": kind,
                "name": name,
                "policyVersion": created_version,
            });
            if let Some(policy) = &row.policy {
                request["policy"] = json!(policy);
            }
            if let Some(description) = &row.description {
                request["description"] = json!(description);
            }
            let written = accepted(&router, "UpdateLifecyclePolicy", request).await;

            // A member the request named is the stored value now; one it did not name kept
            // what the create stored. The document is reported PARSED —
            // `LifecyclePolicyDetail$policy` is "the JSON policy document without any
            // whitespaces" — whatever whitespace the request text had.
            let expected_policy: Value = serde_json::from_str(
                row.policy.as_deref().unwrap_or(CREATED_POLICY),
            )
            .expect("every row's document text is JSON");
            assert_eq!(
                at(&written, "policy"),
                &expected_policy,
                "case {case}: the stored document must be the written one, parsed, got {written}"
            );
            assert_eq!(
                at(&written, "description"),
                &json!(row
                    .description
                    .clone()
                    .unwrap_or_else(|| CREATED_DESCRIPTION.to_string())),
                "case {case}: the stored description must be the written one, got {written}"
            );

            // Every write mints a fresh `policyVersion` and stamps `lastModifiedDate`,
            // while the identity and the creation stamp of the record stay put.
            let written_version = text(&written, "policyVersion");
            assert!(
                is_policy_version(&written_version),
                "case {case}: policyVersion must match PolicyVersion's length and pattern, got {written_version:?}"
            );
            assert_ne!(
                written_version, created_version,
                "case {case}: a write mints a NEW policyVersion, got the creation version {written_version:?}"
            );
            assert_eq!(
                (text(&written, "type"), text(&written, "name")),
                (kind.to_string(), name.clone()),
                "case {case}: the write answers the policy it was addressed to, got {written}"
            );
            assert_eq!(
                at(&written, "createdDate").as_i64(),
                Some(created_date),
                "case {case}: a write leaves createdDate where the create put it, got {written}"
            );
            let modified = at(&written, "lastModifiedDate").as_i64().unwrap_or_default();
            assert!(
                modified >= created_date,
                "case {case}: lastModifiedDate must be the write's own instant, expected >= {created_date}, got {modified}"
            );

            // The round trip: the read answers the record the write stored, member for
            // member — the version and both stamps included.
            let read = read_back(&router, kind, &name).await;
            assert_eq!(
                read, written,
                "case {case}: the read must answer the written record, got {read}"
            );
        }

        // Every row's policy is still its own: a write never touched a record it did not
        // name. One request addresses them all, and the details come back in request order.
        let names: Vec<String> = (0..rows().len())
            .map(|index| format!("upd-{kind}-{index}"))
            .collect();
        let identifiers: Vec<Value> = names
            .iter()
            .map(|name| json!({ "type": kind, "name": name }))
            .collect();
        let (status, code, read) = call(
            &router,
            "BatchGetLifecyclePolicy",
            json!({ "identifiers": identifiers }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case BatchGetLifecyclePolicy(the whole write table): expected 200, got {status} {code} with {read}"
        );
        let answered: Vec<String> = at(&read, "lifecyclePolicyDetails")
            .as_array()
            .map(|details| details.iter().map(|detail| text(detail, "name")).collect())
            .unwrap_or_default();
        assert_eq!(
            answered, names,
            "case BatchGetLifecyclePolicy(the whole write table): every written row is answered, in request order, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// `policyVersion` is the write's precondition: the version chains forward, and a version
// that is not the stored one is refused and writes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_lifecycle_policy_chains_the_version_and_refuses_a_stale_one() {
    let router = emulator();

    for kind in LIFECYCLE_POLICY_TYPES {
        let name = format!("chain-{kind}");
        let created = create(&router, kind, &name).await;

        // Each write is addressed with the version the write before it returned, and every
        // one of them mints a version no earlier write used.
        let mut versions = vec![text(&created, "policyVersion")];
        for step in 0..3 {
            let current = versions.last().cloned().expect("a version to write against");
            let written = accepted(
                &router,
                "UpdateLifecyclePolicy",
                json!({
                    "type": kind,
                    "name": name,
                    "policyVersion": current,
                    "description": format!("write {step}"),
                }),
            )
            .await;
            let version = text(&written, "policyVersion");
            assert!(
                !versions.contains(&version),
                "case UpdateLifecyclePolicy({kind}/{name}) write {step}: every write mints a fresh version, got a repeat of {version:?}"
            );
            versions.push(version);
        }

        let stale: Vec<(&str, String)> = vec![
            ("the creation version, long since superseded", versions[0].clone()),
            (
                "the version before the last write",
                versions[versions.len() - 2].clone(),
            ),
            (
                "a well-formed version nothing ever minted",
                STALE_VERSION.to_string(),
            ),
        ];
        for (what, version) in stale {
            let (status, code, answer) = call(
                &router,
                "UpdateLifecyclePolicy",
                json!({
                    "type": kind,
                    "name": name,
                    "policyVersion": version,
                    "description": "this write must not land",
                }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ValidationException"),
                "case UpdateLifecyclePolicy({kind}/{name}, {what}): expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
            );
        }

        // A refused write stored nothing: the policy still carries the last write that was
        // accepted, at the version that write minted.
        let read = read_back(&router, kind, &name).await;
        assert_eq!(
            (text(&read, "policyVersion"), text(&read, "description")),
            (
                versions.last().cloned().unwrap_or_default(),
                "write 2".to_string()
            ),
            "case UpdateLifecyclePolicy({kind}/{name}): a refused write must leave the record as the last accepted write left it, got {read}"
        );

        // A `(type, name)` pair that names no stored policy is the modelled not-found
        // refusal, whatever version the request carries.
        for (what, version) in [
            ("a version nothing minted", STALE_VERSION.to_string()),
            (
                "the version of another policy",
                versions.last().cloned().unwrap_or_default(),
            ),
        ] {
            let (status, code, answer) = call(
                &router,
                "UpdateLifecyclePolicy",
                json!({
                    "type": kind,
                    "name": format!("absent-{kind}"),
                    "policyVersion": version,
                    "description": "there is nothing to write",
                }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ResourceNotFoundException"),
                "case UpdateLifecyclePolicy({kind}/absent, {what}): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The refusals a write owes on its own input: the modelled bounds, and the `retention`
// document syntax on each published window bound's far side.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_lifecycle_policy_refuses_an_invalid_input_and_writes_nothing() {
    let router = emulator();
    let created = create(&router, "retention", "refusal-policy").await;
    let version = text(&created, "policyVersion");

    // The modelled refusals: `type` outside `LifecyclePolicyType`'s value list, and each
    // modelled bound, one past it.
    let modelled: Vec<(&str, Value)> = vec![
        (
            "type outside LifecyclePolicyType",
            json!({ "type": "data", "name": "refusal-policy", "policyVersion": version }),
        ),
        (
            "type in the wrong case",
            json!({ "type": "Retention", "name": "refusal-policy", "policyVersion": version }),
        ),
        (
            "name one short of the 3-character minimum",
            json!({ "type": "retention", "name": "ab", "policyVersion": version }),
        ),
        (
            "name one past the 32-character maximum",
            json!({ "type": "retention", "name": format!("a{}", "b".repeat(32)), "policyVersion": version }),
        ),
        (
            "name outside PolicyName's ^[a-z][a-z0-9-]+$",
            json!({ "type": "retention", "name": "Bad_Name", "policyVersion": version }),
        ),
        (
            "policyVersion one short of its 20-character minimum",
            json!({ "type": "retention", "name": "refusal-policy", "policyVersion": "0".repeat(19) }),
        ),
        (
            "policyVersion one past its 36-character maximum",
            json!({ "type": "retention", "name": "refusal-policy", "policyVersion": "0".repeat(37) }),
        ),
        (
            "policyVersion absent",
            json!({ "type": "retention", "name": "refusal-policy" }),
        ),
        (
            "name absent",
            json!({ "type": "retention", "policyVersion": version }),
        ),
        (
            "description one past the 1000-character maximum",
            json!({ "type": "retention", "name": "refusal-policy", "policyVersion": version, "description": "d".repeat(1001) }),
        ),
        (
            "clientToken below the 1-character minimum",
            json!({ "type": "retention", "name": "refusal-policy", "policyVersion": version, "clientToken": "" }),
        ),
        (
            "policy one past the 20480-character maximum",
            json!({ "type": "retention", "name": "refusal-policy", "policyVersion": version, "policy": "x".repeat(MAX_POLICY_DOCUMENT + 1) }),
        ),
    ];

    // The document refusals: the `retention` rule syntax AWS documents, each dimension on
    // its own, and the window range on each published bound's far side — the bounds
    // THEMSELVES are written in the table above.
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
            "a rule whose Resource names nothing",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": [], "MinIndexRetention": "24h" }] }),
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
            "a window in an unsupported unit",
            json!({ "Rules": [{ "ResourceType": "index", "Resource": ["index/demo/*"], "MinIndexRetention": "4w" }] }),
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

    let refusals = modelled
        .into_iter()
        .chain(documents.into_iter().map(|(case, document)| {
            let policy = match document.as_str() {
                Some(text) => text.to_string(),
                None => document.to_string(),
            };
            (
                case,
                json!({
                    "type": "retention",
                    "name": "refusal-policy",
                    "policyVersion": version,
                    "policy": policy,
                }),
            )
        }));
    for (case, request) in refusals {
        let (status, code, answer) = call(&router, "UpdateLifecyclePolicy", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case UpdateLifecyclePolicy({case}): expected 400 ValidationException, got {status} {code} with {answer}"
        );
    }

    // A refused write persists nothing: the policy still reports what the create stored,
    // version and all.
    let read = read_back(&router, "retention", "refusal-policy").await;
    assert_eq!(
        read, created,
        "case UpdateLifecyclePolicy(after every refusal): a refused write must leave the created record untouched, got {read}"
    );
}

// ---------------------------------------------------------------------------
// A replayed `clientToken` on a write is a RETRY of that write: the same record, the same
// version, no second write.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_on_a_lifecycle_policy_update_answers_the_write_it_made() {
    let router = emulator();

    // `ClientToken` is @length(min: 1, max: 512): both published bounds, and one token in
    // between.
    let tokens = [
        "t".to_string(),
        "retry-token-retention".to_string(),
        "r".repeat(512),
    ];
    for (row, token) in tokens.iter().enumerate() {
        let name = format!("retry-retention-{row}");
        let created = create(&router, "retention", &name).await;
        let request = json!({
            "type": "retention",
            "name": name,
            "policyVersion": text(&created, "policyVersion"),
            "description": "written once",
            "clientToken": token,
        });

        let first = accepted(&router, "UpdateLifecyclePolicy", request.clone()).await;
        // The retry carries the version the FIRST call carried, which that call has since
        // superseded: without the token it would be the stale-version conflict.
        let replay = accepted(&router, "UpdateLifecyclePolicy", request).await;
        assert_eq!(
            replay, first,
            "case UpdateLifecyclePolicy(retention/{name}, a replayed clientToken of {} characters): the retry must answer the write it already made, got {replay}",
            token.len()
        );

        let read = read_back(&router, "retention", &name).await;
        assert_eq!(
            read, first,
            "case UpdateLifecyclePolicy(retention/{name}): the record must carry the single write the token names, got {read}"
        );
    }

    // The token index is keyed by operation as well, so a token first spent on the CREATE
    // is no replay of anything here: this write lands on its own.
    let name = "token-scope";
    let created = accepted(
        &router,
        "CreateLifecyclePolicy",
        json!({
            "type": "retention",
            "name": name,
            "policy": CREATED_POLICY,
            "description": CREATED_DESCRIPTION,
            "clientToken": "a-caller-supplied-token",
        }),
    )
    .await;
    let written = accepted(
        &router,
        "UpdateLifecyclePolicy",
        json!({
            "type": "retention",
            "name": name,
            "policyVersion": text(&created, "policyVersion"),
            "description": "the create's token is not this write's",
            "clientToken": "a-caller-supplied-token",
        }),
    )
    .await;
    assert_eq!(
        text(&written, "description"),
        "the create's token is not this write's",
        "case UpdateLifecyclePolicy(retention/{name}, the create's token): a token is per-operation, so this write must land, got {written}"
    );
    assert_ne!(
        text(&written, "policyVersion"),
        text(&created, "policyVersion"),
        "case UpdateLifecyclePolicy(retention/{name}, the create's token): the write must mint its own version, got {written}"
    );
}
