// SPDX-License-Identifier: CC0-1.0

//! `BatchGetEffectiveLifecyclePolicy`: the lifecycle policy in EFFECT for an index
//! resource, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client. The harness
//! is the one `collection_lifecycle` uses; nothing is rebuilt here.
//!
//! The rule under test: the effective lifecycle policy of an index resource is decided by
//! the MOST SPECIFIC retention rule, among the account's stored lifecycle policies of the
//! requested type, whose `Resource` patterns match that resource name — the rule's
//! `MinIndexRetention` window is reported as `retentionPeriod`, a rule naming no minimum
//! (and a resource no rule matches at all) reports `noMinRetentionPeriod` instead — so the
//! answer follows the stored policies through every create, update and delete and nothing
//! about the read changes them.
//!
//! The tables span what the rule generalises over: the shape of the matching pattern (a
//! collection-wide wildcard, an index-name prefix, an exact index name, an account-wide
//! wildcard), the number of policies and of rules inside one policy, both spellings of the
//! retention setting, several collections, the identifier COUNT up to the modelled maximum
//! and one past it, and the whole `LifecyclePolicyType` value list.

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};

/// The values `LifecyclePolicyType` accepts on the wire
/// (`smithy_query(query="shape", shape="LifecyclePolicyType")` -> `values`): the single
/// value `retention`.
const LIFECYCLE_POLICY_TYPES: [&str; 1] = ["retention"];

/// The one value `ResourceType` accepts on the wire
/// (`smithy_query(query="shape", shape="ResourceType")` -> `values`), which is what an
/// index resource's own name is prefixed with.
const RESOURCE_TYPE: &str = "index";

/// The most identifiers one request may carry, from
/// `BatchGetEffectiveLifecyclePolicyRequest$resourceIdentifiers`' own
/// `@length(min: 1, max: 100)`.
const MAX_RESOURCE_IDENTIFIERS: usize = 100;

/// One stored lifecycle policy the tables below resolve against: its `PolicyName` and the
/// document text the create carries.
struct StoredPolicy {
    name: &'static str,
    policy: &'static str,
}

/// The policies every precedence row is resolved against. They differ along the shape of
/// the `Resource` pattern their rules name — an account-wide wildcard, a collection-wide
/// wildcard, an index-name prefix, an exact index name — and along the number of rules one
/// document carries and the retention spelling each rule uses.
fn stored_policies() -> Vec<StoredPolicy> {
    vec![
        StoredPolicy {
            name: "retain-everything",
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/*"],"NoMinIndexRetention":true}]}"#,
        },
        StoredPolicy {
            name: "logs-collection",
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/logs-prod/*"],"MinIndexRetention":"30d"}]}"#,
        },
        StoredPolicy {
            name: "logs-app-prefix",
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/logs-prod/app-*"],"MinIndexRetention":"7d"}]}"#,
        },
        StoredPolicy {
            name: "logs-app-exact",
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/logs-prod/app-2026"],"MinIndexRetention":"24h"}]}"#,
        },
        StoredPolicy {
            name: "metrics-collection",
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/metrics/*"],"MinIndexRetention":"3650d"}]}"#,
        },
        StoredPolicy {
            name: "audit-rules",
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/audit/*"],"MinIndexRetention":"48h"},{"ResourceType":"index","Resource":["index/audit/secret"],"NoMinIndexRetention":true}]}"#,
        },
    ]
}

/// One precedence row: the resource asked about, and the policy and retention setting that
/// must decide it.
struct Row {
    what: &'static str,
    resource: &'static str,
    policy_name: Option<&'static str>,
    /// The `MinIndexRetention` window the deciding rule names, or `None` when it names no
    /// minimum at all.
    retention_period: Option<&'static str>,
}

/// The precedence table. Each row names a resource several stored rules match and the one
/// that must decide it, so the row fails if any less specific match wins.
fn rows() -> Vec<Row> {
    vec![
        Row {
            what: "an exact index name beats a prefix and two wildcards",
            resource: "index/logs-prod/app-2026",
            policy_name: Some("logs-app-exact"),
            retention_period: Some("24h"),
        },
        Row {
            what: "an index-name prefix beats the collection-wide and account-wide wildcards",
            resource: "index/logs-prod/app-2025",
            policy_name: Some("logs-app-prefix"),
            retention_period: Some("7d"),
        },
        Row {
            what: "the collection-wide wildcard beats the account-wide one",
            resource: "index/logs-prod/orders",
            policy_name: Some("logs-collection"),
            retention_period: Some("30d"),
        },
        Row {
            what: "another collection's wildcard decides its own indexes",
            resource: "index/metrics/cpu",
            policy_name: Some("metrics-collection"),
            retention_period: Some("3650d"),
        },
        Row {
            what: "the first rule of a two-rule document",
            resource: "index/audit/debug",
            policy_name: Some("audit-rules"),
            retention_period: Some("48h"),
        },
        Row {
            what: "the more specific rule of the same document, naming no minimum",
            resource: "index/audit/secret",
            policy_name: Some("audit-rules"),
            retention_period: None,
        },
        Row {
            what: "an unmentioned collection, left to the account-wide wildcard",
            resource: "index/other/thing",
            policy_name: Some("retain-everything"),
            retention_period: None,
        },
    ]
}

/// The `EffectiveLifecyclePolicyDetail` a row must be answered with.
///
/// `retentionPeriod` "will return only if it's set" and `noMinRetentionPeriod` "will return
/// as `true` if the minimum number of days or hours is not set to a index resource"
/// (`smithy_query(query="doc", shape="EffectiveLifecyclePolicyDetail", member="...")`), so a
/// decided window is reported beside a `noMinRetentionPeriod` of `false` and no window is
/// reported at all when none is set.
fn expected_detail(kind: &str, row: &Row) -> Value {
    let mut detail = json!({
        "type": kind,
        "resource": row.resource,
        "resourceType": RESOURCE_TYPE,
        "noMinRetentionPeriod": row.retention_period.is_none(),
    });
    if let Some(name) = row.policy_name {
        detail["policyName"] = json!(name);
    }
    if let Some(window) = row.retention_period {
        detail["retentionPeriod"] = json!(window);
    }
    detail
}

/// Create every policy of [`stored_policies`] under `kind`, answering each one's
/// `policyVersion` beside its name.
async fn create_stored_policies(
    router: &smithy_server_runtime::MultiServiceRouter,
    kind: &str,
) -> Vec<(&'static str, String)> {
    let mut created = Vec::new();
    for stored in stored_policies() {
        let (status, code, answer) = call(
            router,
            "CreateLifecyclePolicy",
            json!({ "type": kind, "name": stored.name, "policy": stored.policy }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case CreateLifecyclePolicy({kind}/{}): expected 200, got {status} {code} with {answer}",
            stored.name
        );
        created.push((
            stored.name,
            text(&answer, "lifecyclePolicyDetail.policyVersion"),
        ));
    }
    created
}

/// The `effectiveLifecyclePolicyDetails` of `answer`, or the empty list.
fn details(answer: &Value) -> Vec<Value> {
    at(answer, "effectiveLifecyclePolicyDetails")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The most specific matching rule decides, one identifier at a time and for the whole
// table in one request.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_effective_policy_is_the_most_specific_matching_rule() {
    for kind in LIFECYCLE_POLICY_TYPES {
        let router = emulator();
        create_stored_policies(&router, kind).await;

        for row in rows() {
            let label = format!(
                "BatchGetEffectiveLifecyclePolicy({kind}, {}, {})",
                row.resource, row.what
            );
            let (status, code, answer) = call(
                &router,
                "BatchGetEffectiveLifecyclePolicy",
                json!({ "resourceIdentifiers": [{ "type": kind, "resource": row.resource }] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label}: expected 200, got {status} {code} with {answer}"
            );
            assert_eq!(
                details(&answer),
                vec![expected_detail(kind, &row)],
                "case {label}: the most specific matching rule must decide, got {answer}"
            );
            assert_eq!(
                at(&answer, "effectiveLifecyclePolicyErrorDetails"),
                &json!([]),
                "case {label}: a resource the service resolves carries no error, got {answer}"
            );
        }

        // One request carrying the whole table is answered in request order, and the same
        // identifier twice is answered twice: the read is per entry.
        let mut identifiers: Vec<Value> = rows()
            .iter()
            .map(|row| json!({ "type": kind, "resource": row.resource }))
            .collect();
        identifiers.push(json!({ "type": kind, "resource": rows()[0].resource }));
        let (status, code, answer) = call(
            &router,
            "BatchGetEffectiveLifecyclePolicy",
            json!({ "resourceIdentifiers": identifiers }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case BatchGetEffectiveLifecyclePolicy({kind}, the whole table): expected 200, got {status} {code} with {answer}"
        );
        let mut expected: Vec<Value> = rows()
            .iter()
            .map(|row| expected_detail(kind, row))
            .collect();
        expected.push(expected_detail(kind, &rows()[0]));
        assert_eq!(
            details(&answer),
            expected,
            "case BatchGetEffectiveLifecyclePolicy({kind}, the whole table): every identifier is answered, in request order, got {answer}"
        );

        // A read changes nothing a later caller sees: the same request answers the same
        // details, and every stored policy still reads back as it was created.
        let (_, _, again) = call(
            &router,
            "BatchGetEffectiveLifecyclePolicy",
            json!({ "resourceIdentifiers": identifiers }),
        )
        .await;
        assert_eq!(
            details(&again),
            expected,
            "case BatchGetEffectiveLifecyclePolicy({kind}, repeated): a read must not change the answer, got {again}"
        );
        let names: Vec<Value> = stored_policies()
            .iter()
            .map(|stored| json!({ "type": kind, "name": stored.name }))
            .collect();
        let (status, code, stored_read) = call(
            &router,
            "BatchGetLifecyclePolicy",
            json!({ "identifiers": names }),
        )
        .await;
        assert_eq!(
            (status, at(&stored_read, "lifecyclePolicyErrorDetails")),
            (200, &json!([])),
            "case BatchGetLifecyclePolicy({kind}, after the effective reads): every policy is still stored, got {status} {code} with {stored_read}"
        );
    }
}

// ---------------------------------------------------------------------------
// A resource no stored rule applies to has no minimum retention set.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_resource_no_rule_applies_to_has_no_minimum_retention() {
    for kind in LIFECYCLE_POLICY_TYPES {
        // An empty store, and a store whose only policy names other resources: neither
        // sets a minimum for the resource asked about.
        let empty = emulator();
        let narrow = emulator();
        let (status, code, answer) = call(
            &narrow,
            "CreateLifecyclePolicy",
            json!({
                "type": kind,
                "name": "metrics-collection",
                "policy": r#"{"Rules":[{"ResourceType":"index","Resource":["index/metrics/*"],"MinIndexRetention":"30d"}]}"#,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case CreateLifecyclePolicy({kind}/metrics-collection): expected 200, got {status} {code} with {answer}"
        );

        let unmatched = [
            ("an empty store", &empty, "index/logs-prod/orders"),
            ("a store of other resources", &narrow, "index/logs-prod/orders"),
            // The same collection, an index its rules do not name, and a name that only
            // shares a prefix with the pattern's collection half.
            ("another index of a covered collection", &narrow, "index/metrics-staging/cpu"),
        ];
        for (what, router, resource) in unmatched {
            let label = format!("BatchGetEffectiveLifecyclePolicy({kind}, {resource}, {what})");
            let (status, code, answer) = call(
                router,
                "BatchGetEffectiveLifecyclePolicy",
                json!({ "resourceIdentifiers": [{ "type": kind, "resource": resource }] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label}: expected 200, got {status} {code} with {answer}"
            );
            assert_eq!(
                details(&answer),
                vec![json!({
                    "type": kind,
                    "resource": resource,
                    "resourceType": RESOURCE_TYPE,
                    "noMinRetentionPeriod": true,
                })],
                "case {label}: a resource no rule applies to has no minimum retention and no policy name, got {answer}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The effective policy follows the stored policies: an update, a more specific create and
// a delete each move it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_effective_policy_follows_the_stored_policies() {
    let kind = LIFECYCLE_POLICY_TYPES[0];
    let router = emulator();

    /// The detail answered for `resource`, which every step below compares against.
    async fn effective(
        router: &smithy_server_runtime::MultiServiceRouter,
        kind: &str,
        resource: &str,
        label: &str,
    ) -> Value {
        let (status, code, answer) = call(
            router,
            "BatchGetEffectiveLifecyclePolicy",
            json!({ "resourceIdentifiers": [{ "type": kind, "resource": resource }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: expected 200, got {status} {code} with {answer}"
        );
        details(&answer).into_iter().next().unwrap_or(Value::Null)
    }

    let (status, code, created) = call(
        &router,
        "CreateLifecyclePolicy",
        json!({
            "type": kind,
            "name": "demo-collection",
            "policy": r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"30d"}]}"#,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateLifecyclePolicy(demo-collection): expected 200, got {status} {code} with {created}"
    );
    let version = text(&created, "lifecyclePolicyDetail.policyVersion");

    let after_create = effective(&router, kind, "index/demo/orders", "after the create").await;
    assert_eq!(
        after_create,
        json!({
            "type": kind,
            "resource": "index/demo/orders",
            "resourceType": RESOURCE_TYPE,
            "policyName": "demo-collection",
            "retentionPeriod": "30d",
            "noMinRetentionPeriod": false,
        }),
        "case after the create: the stored window decides, got {after_create}"
    );

    // An update to the window moves the effective answer with it.
    let (status, code, updated) = call(
        &router,
        "UpdateLifecyclePolicy",
        json!({
            "type": kind,
            "name": "demo-collection",
            "policyVersion": version,
            "policy": r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"48h"}]}"#,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case UpdateLifecyclePolicy(demo-collection): expected 200, got {status} {code} with {updated}"
    );
    let after_update = effective(&router, kind, "index/demo/orders", "after the update").await;
    assert_eq!(
        text(&after_update, "retentionPeriod"),
        "48h",
        "case after the update: the rewritten window decides, got {after_update}"
    );

    // A second, more specific policy takes precedence for the index it names, and leaves
    // the collection's other indexes where they were.
    let (status, code, specific) = call(
        &router,
        "CreateLifecyclePolicy",
        json!({
            "type": kind,
            "name": "demo-orders",
            "policy": r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/orders"],"NoMinIndexRetention":true}]}"#,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateLifecyclePolicy(demo-orders): expected 200, got {status} {code} with {specific}"
    );
    let narrowed = effective(&router, kind, "index/demo/orders", "after the narrow create").await;
    assert_eq!(
        narrowed,
        json!({
            "type": kind,
            "resource": "index/demo/orders",
            "resourceType": RESOURCE_TYPE,
            "policyName": "demo-orders",
            "noMinRetentionPeriod": true,
        }),
        "case after the narrow create: the more specific policy decides, got {narrowed}"
    );
    let sibling = effective(&router, kind, "index/demo/carts", "the sibling index").await;
    assert_eq!(
        text(&sibling, "policyName"),
        "demo-collection",
        "case the sibling index: a narrower policy decides only what it names, got {sibling}"
    );

    // Deleting the narrow policy falls back to the collection-wide one...
    let (status, code, answer) = call(
        &router,
        "DeleteLifecyclePolicy",
        json!({ "type": kind, "name": "demo-orders" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteLifecyclePolicy(demo-orders): expected 200, got {status} {code} with {answer}"
    );
    let fallen_back = effective(&router, kind, "index/demo/orders", "after the narrow delete").await;
    assert_eq!(
        text(&fallen_back, "retentionPeriod"),
        "48h",
        "case after the narrow delete: the remaining policy decides, got {fallen_back}"
    );

    // ... and deleting the last applying policy leaves the resource with no minimum at all.
    let (status, code, answer) = call(
        &router,
        "DeleteLifecyclePolicy",
        json!({ "type": kind, "name": "demo-collection" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteLifecyclePolicy(demo-collection): expected 200, got {status} {code} with {answer}"
    );
    let unmatched = effective(&router, kind, "index/demo/orders", "after the last delete").await;
    assert_eq!(
        unmatched,
        json!({
            "type": kind,
            "resource": "index/demo/orders",
            "resourceType": RESOURCE_TYPE,
            "noMinRetentionPeriod": true,
        }),
        "case after the last delete: no stored rule applies, so no minimum is set, got {unmatched}"
    );
}

// ---------------------------------------------------------------------------
// The refusals this read owes: the identifier count on the modelled bound and past it, a
// `type` outside the value list, and a `resource` outside `ResourceName`'s pattern.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_effective_read_refuses_what_the_model_and_the_enum_do_not_admit() {
    let kind = LIFECYCLE_POLICY_TYPES[0];
    let router = emulator();
    create_stored_policies(&router, kind).await;

    // The maximum identifier count is SERVED: every entry is answered.
    let at_bound: Vec<Value> = (0..MAX_RESOURCE_IDENTIFIERS)
        .map(|index| json!({ "type": kind, "resource": format!("index/logs-prod/app-{index}") }))
        .collect();
    let (status, code, answer) = call(
        &router,
        "BatchGetEffectiveLifecyclePolicy",
        json!({ "resourceIdentifiers": at_bound }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case BatchGetEffectiveLifecyclePolicy({MAX_RESOURCE_IDENTIFIERS} identifiers): expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        details(&answer).len(),
        MAX_RESOURCE_IDENTIFIERS,
        "case BatchGetEffectiveLifecyclePolicy({MAX_RESOURCE_IDENTIFIERS} identifiers): every identifier is answered, got {answer}"
    );

    let refused: Vec<(&str, Value)> = vec![
        (
            "no identifiers at all, one below the modelled minimum",
            json!({ "resourceIdentifiers": [] }),
        ),
        (
            "resourceIdentifiers absent",
            json!({}),
        ),
        (
            "one identifier past the modelled maximum",
            json!({
                "resourceIdentifiers": (0..=MAX_RESOURCE_IDENTIFIERS)
                    .map(|index| json!({ "type": kind, "resource": format!("index/logs-prod/app-{index}") }))
                    .collect::<Vec<Value>>(),
            }),
        ),
        (
            "type outside LifecyclePolicyType's value list",
            json!({ "resourceIdentifiers": [{ "type": "data", "resource": "index/logs-prod/orders" }] }),
        ),
        (
            "type in the wrong case",
            json!({ "resourceIdentifiers": [{ "type": "Retention", "resource": "index/logs-prod/orders" }] }),
        ),
        (
            "type absent",
            json!({ "resourceIdentifiers": [{ "resource": "index/logs-prod/orders" }] }),
        ),
        (
            "resource absent",
            json!({ "resourceIdentifiers": [{ "type": kind }] }),
        ),
        (
            "a resource that names no index at all",
            json!({ "resourceIdentifiers": [{ "type": kind, "resource": "collection/logs-prod" }] }),
        ),
        (
            "a resource outside ResourceName's pattern",
            json!({ "resourceIdentifiers": [{ "type": kind, "resource": "index/Logs-Prod/orders" }] }),
        ),
        (
            // One bad entry refuses the whole request rather than half-answering it.
            "a good identifier beside a bad one",
            json!({
                "resourceIdentifiers": [
                    { "type": kind, "resource": "index/logs-prod/orders" },
                    { "type": "data", "resource": "index/logs-prod/orders" },
                ],
            }),
        ),
    ];
    for (what, request) in refused {
        let label = format!("BatchGetEffectiveLifecyclePolicy({what})");
        let (status, code, answer) =
            call(&router, "BatchGetEffectiveLifecyclePolicy", request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case {label}: expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
    }

    // Every refusal left the store alone: the table's rows still resolve as they did.
    for row in rows() {
        let label = format!(
            "BatchGetEffectiveLifecyclePolicy({}, after the refusals)",
            row.resource
        );
        let (status, code, answer) = call(
            &router,
            "BatchGetEffectiveLifecyclePolicy",
            json!({ "resourceIdentifiers": [{ "type": kind, "resource": row.resource }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: expected 200, got {status} {code} with {answer}"
        );
        assert_eq!(
            details(&answer),
            vec![expected_detail(kind, &row)],
            "case {label}: a refused read must not change what a later one answers, got {answer}"
        );
    }
}
