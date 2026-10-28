// SPDX-License-Identifier: CC0-1.0

//! The OpenSearch Serverless **lifecycle policy** delete path, end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body (the wire
//! contract in this crate's `OPERATIONS.md`) — so the boundary validator, the protocol
//! codec and the handler all take part, exactly as they do for an SDK client. The harness
//! is the one `collection_lifecycle` owns; nothing is rebuilt here.
//!
//! The rule under test: deleting a policy removes the record its `(type, name)` pair
//! identifies and nothing else, answers the empty modelled body, and is what every later
//! read reports — the batch read answers the modelled `NOT_FOUND` detail for that pair, the
//! account's policy counters fall by exactly one, the freed pair takes a new policy, and a
//! pair that names no stored record is the modelled not-found refusal rather than a
//! silent success.
//!
//! The tables span what the rule generalises over: the whole `LifecyclePolicyType` value
//! list, the `PolicyName` length bounds and a name in between, a described / undescribed /
//! emptily-described policy, the retention window's form (hours, days, unlimited, several
//! rules), and whether the delete request carries a `clientToken` — at both `ClientToken`
//! length bounds — not one request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::collection_lifecycle::{at, call, emulator, is_policy_version, text, CLIENT_ERROR};

/// The values `LifecyclePolicyType` accepts on the wire
/// (`smithy_query(query="shape", shape="LifecyclePolicyType")` -> `values`): the single
/// value `retention`. Every case below is driven over this whole list.
const LIFECYCLE_POLICY_TYPES: [&str; 1] = ["retention"];

/// A `LifecyclePolicyType` value the model's list does not carry, so the handler is what
/// refuses it.
const UNMODELED_TYPE: &str = "expiration";

/// The description a row carries when it carries one, so a delete that drops the record is
/// visible against a record that really held every member.
const DESCRIPTION: &str = "the description the create stored";

/// One row of the delete table: the policy name, the document text the create carries and
/// the description it carries (or none), plus the `clientToken` the DELETE request carries
/// (or none).
struct Row {
    what: &'static str,
    name: String,
    policy: &'static str,
    description: Option<String>,
    delete_token: Option<String>,
}

/// The delete table. Rows differ along what the rule generalises over: the `PolicyName`
/// length bounds and a name in between, whether the stored policy carried a description,
/// the retention window's form and rule count, and whether the delete request carries a
/// `clientToken` (`ClientToken` is `@length(min: 1, max: 512)`, so both bounds appear).
fn rows() -> Vec<Row> {
    vec![
        Row {
            what: "a described policy at the 24h floor, deleted without a token",
            // `PolicyName` is @length(min: 3, max: 32): this is the minimum.
            name: "abc".to_string(),
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"MinIndexRetention":"24h"}]}"#,
            description: Some(DESCRIPTION.to_string()),
            delete_token: None,
        },
        Row {
            what: "an undescribed policy with a window in days, deleted with a 1-character token",
            name: "retain-seven-days".to_string(),
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/orders"],"MinIndexRetention":"7d"}]}"#,
            description: None,
            // `ClientToken` is @length(min: 1, max: 512): the minimum.
            delete_token: Some("t".to_string()),
        },
        Row {
            what: "an emptily-described policy with unlimited retention, deleted with a 512-character token",
            // `PolicyName`'s maximum.
            name: format!("d{}", "e".repeat(31)),
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*"],"NoMinIndexRetention":true}]}"#,
            description: Some(String::new()),
            // `ClientToken`'s maximum.
            delete_token: Some("z".repeat(512)),
        },
        Row {
            what: "a policy with several rules over several resources",
            name: "mixed-rules-policy".to_string(),
            policy: r#"{"Rules":[{"ResourceType":"index","Resource":["index/demo/*","index/other/*"],"MinIndexRetention":"30d"},{"ResourceType":"index","Resource":["index/logs/*"],"NoMinIndexRetention":true}]}"#,
            description: Some("two rules".to_string()),
            delete_token: Some("mixed-rules-token".to_string()),
        },
    ]
}

/// The document a recreated pair is given, so a read after the recreate cannot be answering
/// the record the delete removed.
const RECREATED_POLICY: &str =
    r#"{"Rules":[{"ResourceType":"index","Resource":["index/fresh/*"],"MinIndexRetention":"48h"}]}"#;

/// Call `operation` with `body`, asserting it was accepted, and answer the member of the
/// response named by `detail`.
async fn accepted(
    router: &MultiServiceRouter,
    operation: &str,
    detail: &str,
    body: Value,
) -> Value {
    let case = body.to_string();
    let (status, code, answer) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation}({case}): expected 200, got {status} {code} with {answer}"
    );
    at(&answer, detail).clone()
}

/// The `LifecyclePolicyDetail` `BatchGetLifecyclePolicy` reports for `(kind, name)`, or
/// `Value::Null` when that pair resolves to no record.
async fn read_back(router: &MultiServiceRouter, kind: &str, name: &str) -> Value {
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
        .and_then(|details| details.first().cloned())
        .unwrap_or(Value::Null)
}

/// The whole `BatchGetLifecyclePolicy` answer for `(kind, name)`, for the cases that assert
/// on the error details as well as the details.
async fn batch_get(router: &MultiServiceRouter, kind: &str, name: &str) -> Value {
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
    read
}

/// The account's stored lifecycle-policy count for `kind`, as `GetPoliciesStats` reports it,
/// and the total policy count beside it.
async fn policy_counts(router: &MultiServiceRouter) -> (i64, i64) {
    let (status, code, answer) = call(router, "GetPoliciesStats", json!({})).await;
    assert_eq!(
        status, 200,
        "case GetPoliciesStats: expected 200, got {status} {code} with {answer}"
    );
    (
        at(&answer, "LifecyclePolicyStats.RetentionPolicyCount")
            .as_i64()
            .unwrap_or_default(),
        at(&answer, "TotalPolicyCount").as_i64().unwrap_or_default(),
    )
}

// ---------------------------------------------------------------------------
// A delete removes the record its `(type, name)` pair names and nothing else, so every
// later read of that pair is the modelled not-found answer — and the freed pair may be
// created again.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_lifecycle_policy_is_absent_from_every_later_read() {
    let router = emulator();

    for kind in LIFECYCLE_POLICY_TYPES {
        // One policy nothing below deletes: a delete must remove the record it names and no
        // other.
        let kept = format!("kept-{kind}-policy");
        let kept_detail = accepted(
            &router,
            "CreateLifecyclePolicy",
            "lifecyclePolicyDetail",
            json!({
                "type": kind,
                "name": kept,
                "policy": rows()[0].policy,
                "description": DESCRIPTION,
            }),
        )
        .await;

        for row in rows() {
            let what = row.what;
            let name = row.name.clone();
            let mut request = json!({ "type": kind, "name": name, "policy": row.policy });
            if let Some(description) = row.description.clone() {
                request["description"] = Value::String(description);
            }
            let created = accepted(
                &router,
                "CreateLifecyclePolicy",
                "lifecyclePolicyDetail",
                request,
            )
            .await;
            assert!(
                is_policy_version(&text(&created, "policyVersion")),
                "case {what}: the create must mint a modelled policyVersion, got {created}"
            );

            let (before_retention, before_total) = policy_counts(&router).await;

            // The delete itself: 200 and the empty modelled body.
            let mut delete = json!({ "type": kind, "name": name });
            if let Some(token) = row.delete_token.clone() {
                delete["clientToken"] = Value::String(token);
            }
            let (status, code, answer) = call(&router, "DeleteLifecyclePolicy", delete).await;
            assert_eq!(
                status, 200,
                "case {what}: expected DeleteLifecyclePolicy({kind}/{name}) to answer 200, got {status} {code} with {answer}"
            );
            assert_eq!(
                answer,
                json!({}),
                "case {what}: the model declares no output members for DeleteLifecyclePolicy, got {answer}"
            );

            // The read of a deleted pair is the modelled NOT_FOUND error detail, and it
            // stays that way however often it is polled — a read changes nothing.
            for attempt in 0..3 {
                let read = batch_get(&router, kind, &name).await;
                assert_eq!(
                    at(&read, "lifecyclePolicyDetails"),
                    &json!([]),
                    "case {what} poll {attempt}: a deleted policy has no detail, got {read}"
                );
                assert_eq!(
                    at(&read, "lifecyclePolicyErrorDetails"),
                    &json!([{
                        "type": kind,
                        "name": name,
                        "errorCode": "NOT_FOUND",
                        "errorMessage": "The specified Lifecycle Policy is not found.",
                    }]),
                    "case {what} poll {attempt}: the unresolved identifier is the modelled NOT_FOUND detail, got {read}"
                );
            }

            // The account's counters fell by exactly one.
            let (after_retention, after_total) = policy_counts(&router).await;
            assert_eq!(
                (after_retention, after_total),
                (before_retention - 1, before_total - 1),
                "case {what}: one delete removes exactly one policy, expected {:?}, got {:?}",
                (before_retention - 1, before_total - 1),
                (after_retention, after_total)
            );

            // Deleting the same pair again names no record: the modelled not-found refusal,
            // naming the pair the request carried.
            let (status, code, answer) = call(
                &router,
                "DeleteLifecyclePolicy",
                json!({ "type": kind, "name": name }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ResourceNotFoundException"),
                "case {what} (deleted twice): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
            );
            assert_eq!(
                text(&answer, "message"),
                format!("Policy with name {name} and type {kind} is not found"),
                "case {what} (deleted twice): the refusal must name the pair the request carried, got {answer}"
            );

            // The freed pair takes a new policy, whose own document is what the read
            // reports — so the recreate is not answering the removed record.
            let recreated = accepted(
                &router,
                "CreateLifecyclePolicy",
                "lifecyclePolicyDetail",
                json!({ "type": kind, "name": name, "policy": RECREATED_POLICY }),
            )
            .await;
            let read = read_back(&router, kind, &name).await;
            assert_eq!(
                read, recreated,
                "case {what}: the read after the recreate must answer the new record, got {read}"
            );
            assert_ne!(
                at(&recreated, "policy"),
                at(&created, "policy"),
                "case {what}: the recreated policy carries its own document, got {recreated}"
            );
            assert_eq!(
                at(&recreated, "description"),
                &Value::Null,
                "case {what}: the recreated policy carries only what its own request named, got {recreated}"
            );
        }

        // The policy no delete named is untouched, down to its version and timestamps.
        let kept_read = read_back(&router, kind, &kept).await;
        assert_eq!(
            kept_read, kept_detail,
            "case kept({kind}/{kept}): a delete must remove only the record it named, got {kept_read}"
        );
    }
}

// ---------------------------------------------------------------------------
// A delete that names no stored policy refuses and writes nothing, and a `type` outside the
// modelled value list is the validation refusal.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_that_names_no_lifecycle_policy_changes_nothing() {
    let router = emulator();

    for kind in LIFECYCLE_POLICY_TYPES {
        let stored = format!("stored-{kind}-policy");
        let detail = accepted(
            &router,
            "CreateLifecyclePolicy",
            "lifecyclePolicyDetail",
            json!({
                "type": kind,
                "name": stored,
                "policy": rows()[0].policy,
                "description": DESCRIPTION,
            }),
        )
        .await;
        let before = policy_counts(&router).await;

        // Names spanning `PolicyName`'s published bounds that no create ever used.
        let absent = [
            "xyz".to_string(),
            "never-created-policy".to_string(),
            format!("n{}", "o".repeat(31)),
        ];
        for name in &absent {
            let (status, code, answer) = call(
                &router,
                "DeleteLifecyclePolicy",
                json!({ "type": kind, "name": name }),
            )
            .await;
            assert_eq!(
                (status, code.as_str()),
                (CLIENT_ERROR, "ResourceNotFoundException"),
                "case DeleteLifecyclePolicy({kind}/{name}, never created): expected {CLIENT_ERROR} ResourceNotFoundException, got {status} {code} with {answer}"
            );
            assert_eq!(
                text(&answer, "message"),
                format!("Policy with name {name} and type {kind} is not found"),
                "case DeleteLifecyclePolicy({kind}/{name}, never created): the refusal must name the pair the request carried, got {answer}"
            );
        }

        // A `type` the model's own value list does not carry names no policy type at all.
        let (status, code, answer) = call(
            &router,
            "DeleteLifecyclePolicy",
            json!({ "type": UNMODELED_TYPE, "name": stored }),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case DeleteLifecyclePolicy({UNMODELED_TYPE}/{stored}): expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );

        // None of the refusals above touched the stored record or the counters.
        let read = read_back(&router, kind, &stored).await;
        assert_eq!(
            read, detail,
            "case BatchGetLifecyclePolicy({kind}/{stored}): a refused delete must write nothing, got {read}"
        );
        let after = policy_counts(&router).await;
        assert_eq!(
            after, before,
            "case GetPoliciesStats(after the refused deletes): expected {before:?}, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A delete drops the `clientToken` replay entries that named the policy with it, so the
// token the create spent no longer resurrects the removed record.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_a_lifecycle_policy_frees_the_token_that_created_it() {
    let router = emulator();

    for kind in LIFECYCLE_POLICY_TYPES {
        let name = format!("token-{kind}-policy");
        let token = format!("a-token-whose-{kind}-policy-is-deleted");
        let request = json!({
            "type": kind,
            "name": name,
            "policy": rows()[0].policy,
            "clientToken": token,
        });

        let first = accepted(
            &router,
            "CreateLifecyclePolicy",
            "lifecyclePolicyDetail",
            request.clone(),
        )
        .await;
        // The token replays while the record is stored.
        let replayed = accepted(
            &router,
            "CreateLifecyclePolicy",
            "lifecyclePolicyDetail",
            request.clone(),
        )
        .await;
        assert_eq!(
            replayed, first,
            "case CreateLifecyclePolicy({kind}/{name}, replayed token): the replay answers the first record, got {replayed}"
        );

        let (status, code, answer) = call(
            &router,
            "DeleteLifecyclePolicy",
            json!({ "type": kind, "name": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case DeleteLifecyclePolicy({kind}/{name}): expected 200, got {status} {code} with {answer}"
        );

        // The same token spent again creates a FRESH policy rather than replaying the
        // deleted one.
        let again = accepted(
            &router,
            "CreateLifecyclePolicy",
            "lifecyclePolicyDetail",
            request,
        )
        .await;
        assert_ne!(
            text(&again, "policyVersion"),
            text(&first, "policyVersion"),
            "case CreateLifecyclePolicy({kind}/{name}, a token whose policy was deleted): expected a freshly created policy, got the deleted one's record {again}"
        );
        let read = read_back(&router, kind, &name).await;
        assert_eq!(
            read, again,
            "case BatchGetLifecyclePolicy({kind}/{name}, after the token was respent): the read answers the fresh record, got {read}"
        );
    }
}
