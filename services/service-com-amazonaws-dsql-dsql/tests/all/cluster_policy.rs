// SPDX-License-Identifier: CC0-1.0

//! The `@idempotencyToken` and version-token rules of a cluster's resource-based
//! policy, end to end over the wire.
//!
//! The round trip, the version conflict, the lockout safety check and the
//! malformed-document refusal are pinned by `tests/all/error_contract.rs`; this
//! suite covers the three rules that sit beside them:
//!
//! * a policy write's `clientToken` serves ONE request — a replay of that request
//!   answers from it, a different request under the same token is refused;
//! * a version token this service has issued is never issued again, so a token
//!   held from before a `DeleteClusterPolicy` selects nothing afterwards;
//! * a policy attached inline by `CreateCluster` is the same stored record the
//!   policy operations read, replace and remove.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::{json, Value};

use crate::harness::{wait_for_the_clock, Dsql};

/// A policy document as the wire carries it: `policy` is a STRING member holding a
/// JSON document, so the document is serialized into it.
fn document(value: &Value) -> String {
    serde_json::to_string(value).expect("a policy document serializes")
}

/// A policy that allows one named principal one action — a document the lockout
/// safety check has no reason to refuse.
fn allow(action: &str) -> String {
    document(&json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::EMULATOR_ACCOUNT_ID)},
            "Action": action,
            "Resource": "*",
        }],
    }))
}

/// One settled cluster per case, all created up front so the clock's
/// `CREATING -> ACTIVE` transition is waited for once rather than per case.
async fn settled_clusters(dsql: &Dsql, count: usize) -> Vec<String> {
    let mut identifiers = Vec::with_capacity(count);
    for _ in 0..count {
        let created = dsql.create(json!({"deletionProtectionEnabled": false})).await;
        assert_eq!(
            created.status, 200,
            "CreateCluster must succeed, got {} {:?}",
            created.status, created.body
        );
        identifiers.push(created.str("identifier"));
    }
    wait_for_the_clock().await;
    identifiers
}

/// One replay of a policy write under a `clientToken` already used, and what the
/// service must answer: the first call's result, or a conflict.
struct ReplayCase {
    name: &'static str,
    /// The second request's body, beside the token.
    second: Value,
    /// Whether the second call is the SAME request as the first.
    identical: bool,
}

fn put_replay_cases() -> Vec<ReplayCase> {
    vec![
        ReplayCase {
            name: "the identical request",
            second: json!({"policy": allow("dsql:DbConnect")}),
            identical: true,
        },
        ReplayCase {
            name: "a different policy document",
            second: json!({"policy": allow("dsql:DbConnectAdmin")}),
            identical: false,
        },
        ReplayCase {
            name: "the same document with the lockout check bypassed",
            second: json!({
                "policy": allow("dsql:DbConnect"),
                "bypassPolicyLockoutSafetyCheck": true,
            }),
            identical: false,
        },
        ReplayCase {
            name: "the same document conditioned on a version",
            second: json!({"policy": allow("dsql:DbConnect"), "expectedPolicyVersion": "1"}),
            identical: false,
        },
    ]
}

/// RULE: `PutClusterPolicy`'s `clientToken` is an `@idempotencyToken`, so it stands
/// for ONE request: replaying that request answers with the version the first call
/// stored and attaches nothing further, while any other request under the same
/// token is refused 409 `ConflictException` and changes nothing.
#[tokio::test]
async fn a_put_policy_client_token_serves_one_request() {
    let dsql = Dsql::new();
    let cases = put_replay_cases();
    let clusters = settled_clusters(&dsql, cases.len()).await;
    let first_document = allow("dsql:DbConnect");

    for (index, case) in cases.iter().enumerate() {
        let identifier = &clusters[index];
        let token = format!("put-token-{index}");
        let mut first = json!({"policy": first_document.clone()});
        first["clientToken"] = json!(token.clone());
        let put = dsql.put_policy(identifier, first).await;
        assert_eq!(
            put.status, 200,
            "case {:?}: the first write must answer 200, got {} {:?}",
            case.name, put.status, put.body
        );
        let version = put.str("policyVersion");

        let mut second = case.second.clone();
        second["clientToken"] = json!(token.clone());
        let replay = dsql.put_policy(identifier, second).await;

        if case.identical {
            assert_eq!(
                replay.status, 200,
                "case {:?}: a replay of the same request must answer 200, got {} {:?}",
                case.name, replay.status, replay.body
            );
            assert_eq!(
                replay.str("policyVersion"),
                version,
                "case {:?}: a replay must answer with the first call's version, got {:?}",
                case.name,
                replay.str("policyVersion")
            );
        } else {
            assert_eq!(
                replay.status, 409,
                "case {:?}: a different request under one token must answer 409, got {} {:?}",
                case.name, replay.status, replay.body
            );
            assert_eq!(
                replay.error.as_deref(),
                Some("ConflictException"),
                "case {:?}: that 409 must be a ConflictException, got {:?}",
                case.name,
                replay.error
            );
        }

        // Either way exactly one document is attached, at the first version.
        let read = dsql.get_policy(identifier).await;
        assert_eq!(
            read.str("policy"),
            first_document,
            "case {:?}: the stored document must still be the first one, got {:?}",
            case.name,
            read.str("policy")
        );
        assert_eq!(
            read.str("policyVersion"),
            version,
            "case {:?}: no second version may have been written, got {:?}",
            case.name,
            read.str("policyVersion")
        );
    }
}

/// RULE: `DeleteClusterPolicy`'s `clientToken` is an `@idempotencyToken` too, so a
/// retry of the SAME delete answers with the version it removed instead of the 404
/// an independent second delete gets, and a different delete under that token is
/// refused 409.
#[tokio::test]
async fn a_delete_policy_client_token_serves_one_request() {
    let dsql = Dsql::new();
    let clusters = settled_clusters(&dsql, 2).await;
    let policy = allow("dsql:DbConnect");

    // (case name, the query the delete carries beside the token, whether that
    // delete is the same request as the first one)
    for (index, (name, query, identical)) in [
        ("the identical delete", String::new(), true),
        (
            "a delete conditioned on a version the first one did not name",
            "&expected-policy-version=1".to_string(),
            false,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let identifier = &clusters[index];
        let attached = dsql
            .put_policy(identifier, json!({"policy": policy.clone()}))
            .await;
        let version = attached.str("policyVersion");
        let token = format!("delete-token-{index}");

        let deleted = dsql
            .delete_policy(identifier, &format!("?client-token={token}"))
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: the first delete must answer 200, got {} {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.str("policyVersion"),
            version,
            "case {name:?}: the delete must report the version it removed, got {:?}",
            deleted.str("policyVersion")
        );

        let retried = dsql
            .delete_policy(identifier, &format!("?client-token={token}{query}"))
            .await;
        if identical {
            assert_eq!(
                retried.status, 200,
                "case {name:?}: a retry under the same token must answer 200, got {} {:?}",
                retried.status, retried.body
            );
            assert_eq!(
                retried.str("policyVersion"),
                version,
                "case {name:?}: a retry must report the version the first delete removed, got {:?}",
                retried.str("policyVersion")
            );
        } else {
            assert_eq!(
                retried.status, 409,
                "case {name:?}: a different delete under one token must answer 409, got {} {:?}",
                retried.status, retried.body
            );
            assert_eq!(
                retried.error.as_deref(),
                Some("ConflictException"),
                "case {name:?}: that 409 must be a ConflictException, got {:?}",
                retried.error
            );
        }

        // Neither answer resurrects the policy.
        assert_eq!(
            dsql.get_policy(identifier).await.status,
            404,
            "case {name:?}: the policy must stay removed"
        );
    }
}

/// RULE: a version token this service has issued is never issued again — removing
/// the policy does not rewind the sequence — so a token held from before the
/// removal selects nothing afterwards: conditioning a write on it is refused, and
/// the write that does land carries a version no earlier policy had.
#[tokio::test]
async fn a_version_token_is_never_reissued() {
    let dsql = Dsql::new();
    // One cluster per round-count, so the rule is asserted over several numbers of
    // attach/remove cycles rather than one.
    let cycles = [1_usize, 2, 3];
    let clusters = settled_clusters(&dsql, cycles.len()).await;

    for (rounds, identifier) in cycles.iter().zip(&clusters) {
        let mut issued: Vec<String> = Vec::new();
        for round in 0..*rounds {
            let policy = allow(if round % 2 == 0 {
                "dsql:DbConnect"
            } else {
                "dsql:DbConnectAdmin"
            });
            let put = dsql
                .put_policy(identifier, json!({"policy": policy.clone()}))
                .await;
            assert_eq!(
                put.status, 200,
                "case {rounds} cycles, round {round}: the write must answer 200, got {} {:?}",
                put.status, put.body
            );
            let version = put.str("policyVersion");
            assert!(
                !issued.contains(&version),
                "case {rounds} cycles, round {round}: version {version:?} was already issued, \
                 seen {issued:?}"
            );
            issued.push(version.clone());

            let removed = dsql.delete_policy(identifier, "").await;
            assert_eq!(
                removed.str("policyVersion"),
                version,
                "case {rounds} cycles, round {round}: the delete must report {version:?}, got {:?}",
                removed.str("policyVersion")
            );

            // The token the caller still holds names a policy that is gone.
            let stale = dsql
                .put_policy(
                    identifier,
                    json!({"policy": policy, "expectedPolicyVersion": version.clone()}),
                )
                .await;
            assert_eq!(
                stale.status, 404,
                "case {rounds} cycles, round {round}: a write conditioned on a removed version \
                 must answer 404, got {} {:?}",
                stale.status, stale.body
            );
            assert_eq!(
                stale.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {rounds} cycles, round {round}: that 404 must be a \
                 ResourceNotFoundException, got {:?}",
                stale.error
            );
            assert_eq!(
                dsql.get_policy(identifier).await.status,
                404,
                "case {rounds} cycles, round {round}: the refused write must attach nothing"
            );
        }
        assert_eq!(
            issued.len(),
            *rounds,
            "case {rounds} cycles: every cycle must issue its own version, got {issued:?}"
        );
    }
}

/// RULE: a policy attached inline by `CreateCluster` is the cluster's policy, not a
/// separate record — the reader reports it with a version, a conditional write
/// replaces it from that version, and the delete removes it — so both ways of
/// attaching a policy converge on one stored record.
#[tokio::test]
async fn an_inline_policy_from_create_is_the_same_record_the_policy_operations_read() {
    let dsql = Dsql::new();
    let inline = allow("dsql:DbConnect");
    let replacement = allow("dsql:DbConnectAdmin");

    let created = dsql
        .create(json!({"deletionProtectionEnabled": false, "policy": inline.clone()}))
        .await;
    assert_eq!(
        created.status, 200,
        "CreateCluster with an inline policy must answer 200, got {} {:?}",
        created.status, created.body
    );
    let identifier = created.str("identifier");
    wait_for_the_clock().await;

    let read = dsql.get_policy(&identifier).await;
    assert_eq!(
        read.status, 200,
        "case inline: the inline policy must be readable, got {} {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.str("policy"),
        inline,
        "case inline: the reader must report the document CreateCluster was given, got {:?}",
        read.str("policy")
    );
    let version = read.str("policyVersion");
    assert!(
        !version.is_empty(),
        "case inline: the inline policy must carry a version, got {:?}",
        read.body
    );

    let replaced = dsql
        .put_policy(
            &identifier,
            json!({"policy": replacement.clone(), "expectedPolicyVersion": version.clone()}),
        )
        .await;
    assert_eq!(
        replaced.status, 200,
        "case inline: a write conditioned on the inline version must be applied, got {} {:?}",
        replaced.status, replaced.body
    );
    assert_ne!(
        replaced.str("policyVersion"),
        version,
        "case inline: the replacement must carry a version other than {version:?}"
    );
    assert_eq!(
        dsql.get_policy(&identifier).await.str("policy"),
        replacement,
        "case inline: the replacement must be the stored document"
    );

    let removed = dsql.delete_policy(&identifier, "").await;
    assert_eq!(
        removed.str("policyVersion"),
        replaced.str("policyVersion"),
        "case inline: the delete must report the version it removed, got {:?}",
        removed.str("policyVersion")
    );
    assert_eq!(
        dsql.get_policy(&identifier).await.status,
        404,
        "case inline: once removed the policy must read as 404"
    );
    assert_eq!(
        dsql.get(&identifier).await.status,
        200,
        "case inline: removing a policy must not retire the cluster"
    );
}
