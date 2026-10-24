// SPDX-License-Identifier: CC0-1.0

//! The account-wide policy statistics `GetPoliciesStats` reports, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client. The harness
//! is the one `collection_lifecycle` uses; nothing is rebuilt here.
//!
//! The rule under test: `GetPoliciesStats` reports, for each policy/configuration TYPE the
//! response models a counter for, the number of records of that type the account has
//! stored, and `TotalPolicyCount` as the sum of those counters — so every create raises
//! exactly one counter and the total by one, every delete lowers them again, and a type the
//! response models no counter for is never counted under another type's counter. The tables
//! span what the rule generalises over: the type (every `SecurityPolicyType`, the one
//! `AccessPolicyType`, the `SecurityConfigType` values with and without a counter, the one
//! `LifecyclePolicyType`), the direction of the change (create, delete) and the COUNT, from
//! an empty account up to each resource's own quota.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{at, call, emulator, text, CLIENT_ERROR};

/// A minimal IdP metadata document, as the `security_configs` suite uses it: `samlMetadata`
/// is `@length(min: 1, max: 51200)` with no structural constraint in the model.
const SAML_METADATA: &str =
    r#"<EntityDescriptor entityID="https://idp.example.com/metadata"></EntityDescriptor>"#;

/// A well-formed `PolicyDocument` for a security policy of either `SecurityPolicyType`.
const SECURITY_POLICY_DOCUMENT: &str =
    r#"{"Rules":[{"ResourceType":"collection","Resource":["collection/*"]}]}"#;

/// A well-formed `PolicyDocument` for a data access policy.
const ACCESS_POLICY_DOCUMENT: &str = r#"{"Rules":[{"ResourceType":"index","Resource":["index/*/*"],"Permission":["aoss:ReadDocument"]}],"Principal":["arn:aws:iam::000000000000:role/Reader"]}"#;

/// A well-formed `PolicyDocument` for a retention lifecycle policy: a `MinIndexRetention`
/// window inside the 24h..3650d bounds `crate::state::validate_retention_policy` enforces.
const LIFECYCLE_POLICY_DOCUMENT: &str =
    r#"{"Rules":[{"ResourceType":"index","Resource":["index/logs/*"],"MinIndexRetention":"24h"}]}"#;

/// The values `SecurityPolicyType` accepts on the wire
/// (`smithy_query(query="shape", shape="SecurityPolicyType")` -> `values`), each of which
/// the response models its own counter for.
const SECURITY_POLICY_TYPES: [&str; 2] = ["encryption", "network"];

/// The account's quota for security policies of ONE type, as
/// `crate::state::MAX_SECURITY_POLICIES_PER_TYPE` records it from AWS's "Quotas for Amazon
/// OpenSearch Serverless": 20 per type.
const MAX_SECURITY_POLICIES_PER_TYPE: i64 = 20;

/// Every counter `GetPoliciesStatsResponse` carries, in the one struct a case compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Counts {
    /// `AccessPolicyStats$DataPolicyCount`.
    data: i64,
    /// `SecurityPolicyStats$EncryptionPolicyCount`.
    encryption: i64,
    /// `SecurityPolicyStats$NetworkPolicyCount`.
    network: i64,
    /// `SecurityConfigStats$SamlConfigCount`.
    saml: i64,
    /// `LifecyclePolicyStats$RetentionPolicyCount`.
    retention: i64,
    /// `TotalPolicyCount`.
    total: i64,
}

impl Counts {
    /// The counts an account holding these records reports, with the total derived from the
    /// parts the way the rule says the service derives it.
    fn of(data: i64, encryption: i64, network: i64, saml: i64, retention: i64) -> Self {
        Self {
            data,
            encryption,
            network,
            saml,
            retention,
            total: data + encryption + network + saml + retention,
        }
    }
}

/// Call `GetPoliciesStats` and answer every counter it reported, asserting the call was
/// accepted and that each counter is present as the `Long` the response models.
async fn stats(router: &MultiServiceRouter, case: &str) -> Counts {
    let (status, code, answer) = call(router, "GetPoliciesStats", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case}: expected GetPoliciesStats to answer 200, got {status} {code} with {answer}"
    );
    Counts {
        data: counter(&answer, "AccessPolicyStats.DataPolicyCount", case),
        encryption: counter(&answer, "SecurityPolicyStats.EncryptionPolicyCount", case),
        network: counter(&answer, "SecurityPolicyStats.NetworkPolicyCount", case),
        saml: counter(&answer, "SecurityConfigStats.SamlConfigCount", case),
        retention: counter(&answer, "LifecyclePolicyStats.RetentionPolicyCount", case),
        total: counter(&answer, "TotalPolicyCount", case),
    }
}

/// The integer counter at `path`, which the response must carry.
fn counter(answer: &Value, path: &str, case: &str) -> i64 {
    let value = at(answer, path);
    value.as_i64().unwrap_or_else(|| {
        panic!("case {case}: expected an integer at {path}, got {value:?} in {answer}")
    })
}

/// One change to the account's stored policies, applied between two reads of the statistics.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// `CreateSecurityPolicy` of `kind`.
    CreateSecurityPolicy { kind: &'static str, name: &'static str },
    /// `DeleteSecurityPolicy` of `kind`.
    DeleteSecurityPolicy { kind: &'static str, name: &'static str },
    /// `CreateAccessPolicy` of the one `AccessPolicyType`.
    CreateAccessPolicy { name: &'static str },
    /// `CreateLifecyclePolicy` of the one `LifecyclePolicyType`.
    CreateLifecyclePolicy { name: &'static str },
    /// `CreateSecurityConfig` of `kind`.
    CreateSecurityConfig { kind: &'static str, name: &'static str },
    /// `DeleteSecurityConfig`, addressed by the id its create reported.
    DeleteSecurityConfig { kind: &'static str, name: &'static str },
}

/// The `samlOptions`/`iamFederationOptions` member a `CreateSecurityConfig` of `kind`
/// carries, from `CreateSecurityConfigRequest`'s own per-type options members.
fn config_options(kind: &str) -> (&'static str, Value) {
    match kind {
        "saml" => ("samlOptions", json!({ "metadata": SAML_METADATA })),
        "iamfederation" => ("iamFederationOptions", json!({ "userAttribute": "Email" })),
        other => panic!("the suite drives no SecurityConfigType {other:?}"),
    }
}

/// Apply `step`, asserting it was accepted; `config_ids` remembers the id of each security
/// configuration created, so a later delete can address it the way the API does.
async fn apply(
    router: &MultiServiceRouter,
    step: Step,
    config_ids: &mut BTreeMap<(String, String), String>,
) {
    match step {
        Step::CreateSecurityPolicy { kind, name } => {
            let body = json!({ "type": kind, "name": name, "policy": SECURITY_POLICY_DOCUMENT });
            accept(router, "CreateSecurityPolicy", body).await;
        }
        Step::DeleteSecurityPolicy { kind, name } => {
            accept(
                router,
                "DeleteSecurityPolicy",
                json!({ "type": kind, "name": name }),
            )
            .await;
        }
        Step::CreateAccessPolicy { name } => {
            let body = json!({ "type": "data", "name": name, "policy": ACCESS_POLICY_DOCUMENT });
            accept(router, "CreateAccessPolicy", body).await;
        }
        Step::CreateLifecyclePolicy { name } => {
            let body = json!({
                "type": "retention",
                "name": name,
                "policy": LIFECYCLE_POLICY_DOCUMENT,
            });
            accept(router, "CreateLifecyclePolicy", body).await;
        }
        Step::CreateSecurityConfig { kind, name } => {
            let (member, options) = config_options(kind);
            let mut body = json!({ "type": kind, "name": name });
            body[member] = options;
            let answer = accept(router, "CreateSecurityConfig", body).await;
            config_ids.insert(
                (kind.to_string(), name.to_string()),
                text(&answer, "securityConfigDetail.id"),
            );
        }
        Step::DeleteSecurityConfig { kind, name } => {
            let id = config_ids
                .get(&(kind.to_string(), name.to_string()))
                .cloned()
                .unwrap_or_else(|| panic!("case {kind}/{name}: the create recorded no id"));
            accept(router, "DeleteSecurityConfig", json!({ "id": id })).await;
        }
    }
}

/// Call `operation` with `body`, asserting a 200, and answer the parsed response.
async fn accept(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}({case}): expected 200, got {status} {code} with {answer}"
    );
    answer
}

#[tokio::test]
async fn an_account_holding_nothing_reports_every_counter_at_zero() {
    let router = emulator();
    let observed = stats(&router, "an empty account").await;
    assert_eq!(
        observed,
        Counts::default(),
        "case an empty account: every counter and the total must be zero, got {observed:?}"
    );
}

#[tokio::test]
async fn every_counter_tracks_the_stored_records_of_its_own_type() {
    // Each row is one change and the counts the account reports once it is applied: the
    // table walks every type the response counts, in both directions, and includes a
    // security configuration of a type the response models NO counter for.
    let table: [(&str, Step, Counts); 14] = [
        (
            "the first encryption policy",
            Step::CreateSecurityPolicy { kind: "encryption", name: "enc-one" },
            Counts::of(0, 1, 0, 0, 0),
        ),
        (
            "a second encryption policy",
            Step::CreateSecurityPolicy { kind: "encryption", name: "enc-two" },
            Counts::of(0, 2, 0, 0, 0),
        ),
        (
            "a network policy, which raises only the network counter",
            Step::CreateSecurityPolicy { kind: "network", name: "net-one" },
            Counts::of(0, 2, 1, 0, 0),
        ),
        (
            "a data access policy",
            Step::CreateAccessPolicy { name: "data-one" },
            Counts::of(1, 2, 1, 0, 0),
        ),
        (
            "a second data access policy",
            Step::CreateAccessPolicy { name: "data-two" },
            Counts::of(2, 2, 1, 0, 0),
        ),
        (
            "a SAML security configuration",
            Step::CreateSecurityConfig { kind: "saml", name: "saml-one" },
            Counts::of(2, 2, 1, 1, 0),
        ),
        (
            "an IAM-federation configuration, a type with no counter of its own",
            Step::CreateSecurityConfig { kind: "iamfederation", name: "fed-one" },
            Counts::of(2, 2, 1, 1, 0),
        ),
        (
            "a second SAML security configuration",
            Step::CreateSecurityConfig { kind: "saml", name: "saml-two" },
            Counts::of(2, 2, 1, 2, 0),
        ),
        (
            "a retention lifecycle policy",
            Step::CreateLifecyclePolicy { name: "retain-one" },
            Counts::of(2, 2, 1, 2, 1),
        ),
        (
            "a second retention lifecycle policy",
            Step::CreateLifecyclePolicy { name: "retain-two" },
            Counts::of(2, 2, 1, 2, 2),
        ),
        (
            "deleting one SAML configuration",
            Step::DeleteSecurityConfig { kind: "saml", name: "saml-one" },
            Counts::of(2, 2, 1, 1, 2),
        ),
        (
            "deleting the IAM-federation configuration, which no counter followed",
            Step::DeleteSecurityConfig { kind: "iamfederation", name: "fed-one" },
            Counts::of(2, 2, 1, 1, 2),
        ),
        (
            "deleting the network policy",
            Step::DeleteSecurityPolicy { kind: "network", name: "net-one" },
            Counts::of(2, 2, 0, 1, 2),
        ),
        (
            "deleting one of the encryption policies",
            Step::DeleteSecurityPolicy { kind: "encryption", name: "enc-one" },
            Counts::of(2, 1, 0, 1, 2),
        ),
    ];

    let router = emulator();
    let mut config_ids = BTreeMap::new();
    for (what, step, expected) in table {
        apply(&router, step, &mut config_ids).await;
        let observed = stats(&router, what).await;
        assert_eq!(
            observed, expected,
            "case {what}: expected {expected:?}, got {observed:?}"
        );
    }
}

#[tokio::test]
async fn the_total_is_the_sum_of_the_counters_at_every_count_up_to_the_quota() {
    // Rows differ in the COUNT of each type, from none at all to each type's own quota
    // bound: the counters are counts of stored records, not a flag that something exists.
    let table: [(&str, i64, i64, i64, i64, i64); 5] = [
        ("nothing stored", 0, 0, 0, 0, 0),
        ("one record of one type", 0, 1, 0, 0, 0),
        ("a different count per type", 3, 1, 2, 4, 5),
        ("one of everything", 1, 1, 1, 1, 1),
        (
            "both security policy types at their quota",
            2,
            MAX_SECURITY_POLICIES_PER_TYPE,
            MAX_SECURITY_POLICIES_PER_TYPE,
            1,
            2,
        ),
    ];

    for (what, data, encryption, network, saml, retention) in table {
        let router = emulator();
        let mut config_ids = BTreeMap::new();
        for index in 0..data {
            let name = leaked(format!("data-{index}"));
            apply(&router, Step::CreateAccessPolicy { name }, &mut config_ids).await;
        }
        for (kind, wanted) in SECURITY_POLICY_TYPES
            .into_iter()
            .zip([encryption, network])
        {
            for index in 0..wanted {
                let name = leaked(format!("pol-{kind}-{index}"));
                apply(
                    &router,
                    Step::CreateSecurityPolicy { kind, name },
                    &mut config_ids,
                )
                .await;
            }
        }
        for index in 0..saml {
            let name = leaked(format!("saml-{index}"));
            apply(
                &router,
                Step::CreateSecurityConfig { kind: "saml", name },
                &mut config_ids,
            )
            .await;
        }
        for index in 0..retention {
            let name = leaked(format!("retain-{index}"));
            apply(&router, Step::CreateLifecyclePolicy { name }, &mut config_ids).await;
        }

        let observed = stats(&router, what).await;
        let expected = Counts::of(data, encryption, network, saml, retention);
        assert_eq!(
            observed, expected,
            "case {what}: expected {expected:?}, got {observed:?}"
        );
    }
}

#[tokio::test]
async fn reading_the_statistics_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let mut config_ids = BTreeMap::new();
    for step in [
        Step::CreateSecurityPolicy { kind: "encryption", name: "enc-stable" },
        Step::CreateSecurityPolicy { kind: "network", name: "net-stable" },
        Step::CreateAccessPolicy { name: "data-stable" },
        Step::CreateSecurityConfig { kind: "saml", name: "saml-stable" },
        Step::CreateLifecyclePolicy { name: "retain-stable" },
    ] {
        apply(&router, step, &mut config_ids).await;
    }

    let first = stats(&router, "the first read").await;
    for round in 1..=3 {
        let what = format!("read number {}", round + 1);
        let again = stats(&router, &what).await;
        assert_eq!(
            again, first,
            "case {what}: a read must not change the counters, expected {first:?}, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_call_leaves_every_counter_where_it_was() {
    let router = emulator();
    let mut config_ids = BTreeMap::new();
    for step in [
        Step::CreateSecurityPolicy { kind: "encryption", name: "enc-kept" },
        Step::CreateLifecyclePolicy { name: "retain-kept" },
    ] {
        apply(&router, step, &mut config_ids).await;
    }
    let before = stats(&router, "before any refused call").await;
    assert_eq!(
        before,
        Counts::of(0, 1, 0, 0, 1),
        "case before any refused call: expected one encryption and one retention policy, got {before:?}"
    );

    // A counter counts STORED records, so a call that is refused raises nothing: the rows
    // span a duplicate pair (`ConflictException`) and a request outside a modelled or
    // documented bound (`ValidationException`), over three of the counted resources. Both
    // answer this service's single client-error status.
    let refusals: [(&str, &str, Value, u16); 4] = [
        (
            "a duplicate retention policy",
            "CreateLifecyclePolicy",
            json!({ "type": "retention", "name": "retain-kept", "policy": LIFECYCLE_POLICY_DOCUMENT }),
            CLIENT_ERROR,
        ),
        (
            "a retention window below the documented 24h minimum",
            "CreateLifecyclePolicy",
            json!({
                "type": "retention",
                "name": "retain-too-short",
                "policy": r#"{"Rules":[{"ResourceType":"index","Resource":["index/logs/*"],"MinIndexRetention":"1h"}]}"#,
            }),
            400,
        ),
        (
            "a duplicate encryption policy",
            "CreateSecurityPolicy",
            json!({ "type": "encryption", "name": "enc-kept", "policy": SECURITY_POLICY_DOCUMENT }),
            CLIENT_ERROR,
        ),
        (
            "a data access policy whose document is not JSON",
            "CreateAccessPolicy",
            json!({ "type": "data", "name": "data-broken", "policy": "not a document" }),
            400,
        ),
    ];

    for (what, operation, body, expected_status) in refusals {
        let (status, code, answer) = call(&router, operation, body).await;
        assert_eq!(
            status, expected_status,
            "case {what}: expected {operation} to answer {expected_status}, got {status} {code} with {answer}"
        );
        let observed = stats(&router, what).await;
        assert_eq!(
            observed, before,
            "case {what}: a refused call must leave every counter where it was, expected {before:?}, got {observed:?}"
        );
    }
}

/// `Step` names its policies with `&'static str` so a table row is a plain constant; a
/// generated name is leaked so a loop can build one. Bounded by the quotas the loops above
/// stop at, so the leak is a handful of short strings per test.
fn leaked(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}
