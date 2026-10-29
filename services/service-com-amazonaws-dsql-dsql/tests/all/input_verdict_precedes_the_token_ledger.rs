// SPDX-License-Identifier: CC0-1.0

//! What an operation that carries BOTH a check on its input and a `clientToken`
//! ledger answers a request whose input it refuses.
//!
//! RULE: an operation decides every check on the VALUES of the request it was
//! handed before it consults its idempotency ledger — the ledger decides what to
//! do with a request whose input is acceptable in the first place — so a request
//! this service refuses is refused with its input's own status, error code,
//! reason and sentence whether it carries no `clientToken` or one another request
//! already spent, it changes nothing, and it records no token, so the spent token
//! still serves the request it first served.
//!
//! The table spans every operation of this service that has both an input check
//! and a token ledger, and every input check they apply: `PutClusterPolicy`'s and
//! `CreateCluster`'s shared policy-document check (over both of its refusal
//! kinds, and over the JSON types an unacceptable document can be),
//! `CreateCluster`'s and `CreateStream`'s reserved tag-key check, and
//! `CreateCluster`'s and `UpdateCluster`'s witness-Region check. What each
//! refused input is refused WITH is pinned by the suites that own those checks
//! (`tests/all/inline_policy_validation.rs`, `tests/all/reserved_tag_keys.rs`,
//! `tests/all/multi_region_witness.rs`); here the refusal the same request gets
//! WITHOUT a token is the reference every token-carrying row is held to.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::{json, Value};

use crate::error_contract::stream_request;
use crate::harness::{Answer, Dsql};

/// The operations whose input check and token ledger are both in play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    CreateCluster,
    UpdateCluster,
    PutClusterPolicy,
    CreateStream,
}

/// One row: a request of `target` whose input this service refuses, and a request
/// of the SAME operation it serves, which is what spends the token first.
struct Row {
    name: String,
    target: Target,
    refused: Value,
    spender: Value,
}

/// A policy document wrapping `statement` in the policy envelope, serialized the
/// way the `policy` member carries it: a STRING holding a JSON document.
fn policy(statement: Value) -> String {
    serde_json::to_string(&json!({"Version": "2012-10-17", "Statement": statement}))
        .expect("a policy document serializes")
}

/// The shortest document this service holds, which is what a row's serving
/// request attaches.
fn acceptable_document() -> String {
    "{}".to_string()
}

/// Documents this service will not hold, spanning both refusal kinds — a value
/// that is not a JSON policy document at all, over several of the JSON types a
/// `policy` can be, and a document that would leave nobody able to replace it,
/// over two of the forms that does.
fn refused_documents() -> Vec<(&'static str, String)> {
    vec![
        ("plain text that is not JSON", "not a policy document".to_string()),
        ("a JSON array", "[]".to_string()),
        ("a JSON number", "7".to_string()),
        (
            "a deny of every principal on every action and resource",
            policy(json!([{
                "Effect": "Deny",
                "Principal": "*",
                "Action": "*",
                "Resource": "*",
            }])),
        ),
        (
            "a deny of every principal carrying no Resource, which is this cluster",
            policy(json!([{
                "Effect": "Deny",
                "Principal": "*",
                "Action": "dsql:*",
            }])),
        ),
    ]
}

/// Every row of the table, for a store whose requests are signed for `region`.
fn rows(region: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    for (document_name, document) in refused_documents() {
        rows.push(Row {
            name: format!("PutClusterPolicy: {document_name}"),
            target: Target::PutClusterPolicy,
            refused: json!({"policy": document.clone()}),
            spender: json!({"policy": acceptable_document()}),
        });
        rows.push(Row {
            name: format!("CreateCluster's inline policy: {document_name}"),
            target: Target::CreateCluster,
            refused: json!({"deletionProtectionEnabled": false, "policy": document}),
            spender: json!({"deletionProtectionEnabled": false, "policy": acceptable_document()}),
        });
    }
    rows.push(Row {
        name: "CreateCluster: a tag key in the namespace AWS reserves".to_string(),
        target: Target::CreateCluster,
        refused: json!({"deletionProtectionEnabled": false, "tags": {"aws:owner": "platform"}}),
        spender: json!({"deletionProtectionEnabled": false, "tags": {"owner": "platform"}}),
    });
    rows.push(Row {
        name: "CreateCluster: a witness Region that is the cluster's own".to_string(),
        target: Target::CreateCluster,
        refused: json!({
            "deletionProtectionEnabled": false,
            "multiRegionProperties": {"witnessRegion": region},
        }),
        spender: json!({
            "deletionProtectionEnabled": false,
            "multiRegionProperties": {"witnessRegion": "eu-west-1"},
        }),
    });
    rows.push(Row {
        name: "UpdateCluster: a witness Region that is the cluster's own".to_string(),
        target: Target::UpdateCluster,
        refused: json!({"multiRegionProperties": {"witnessRegion": region}}),
        spender: json!({"deletionProtectionEnabled": true}),
    });
    let mut tagged_stream = stream_request(engine::EMULATOR_ACCOUNT_ID, "dsql-cdc");
    tagged_stream["tags"] = json!({"aws:owner": "platform"});
    rows.push(Row {
        name: "CreateStream: a tag key in the namespace AWS reserves".to_string(),
        target: Target::CreateStream,
        refused: tagged_stream,
        spender: stream_request(engine::EMULATOR_ACCOUNT_ID, "dsql-cdc"),
    });
    rows
}

/// Send `body` to `target`, optionally carrying `token`, against `cluster` for the
/// cluster-addressed operations.
async fn send(
    dsql: &Dsql,
    target: Target,
    cluster: &str,
    body: &Value,
    token: Option<&str>,
) -> Answer {
    let mut body = body.clone();
    if let Some(token) = token {
        body["clientToken"] = json!(token);
    }
    match target {
        Target::CreateCluster => dsql.create(body).await,
        Target::UpdateCluster => dsql.update(cluster, body).await,
        Target::PutClusterPolicy => dsql.put_policy(cluster, body).await,
        Target::CreateStream => dsql.create_stream(cluster, body).await,
    }
}

/// The member of `target`'s answer that identifies what the FIRST call produced,
/// so a retry under the same token can be shown to answer with it.
fn replayed_member(target: Target) -> &'static str {
    match target {
        Target::CreateCluster => "identifier",
        Target::UpdateCluster => "arn",
        Target::PutClusterPolicy => "policyVersion",
        Target::CreateStream => "streamIdentifier",
    }
}

/// RULE: the verdict on a refused input does not depend on the token the request
/// carries — the answer to the same request with a `clientToken` another request
/// has already spent is the answer it gets with no token at all, down to the
/// reason and the sentence, and the refusal spends nothing.
#[tokio::test]
async fn a_spent_client_token_does_not_change_the_verdict_on_a_refused_input() {
    let dsql = Dsql::new();
    let rows = rows(dsql.region());
    // One settled cluster per row, all created before the single clock wait: a
    // cluster the clock still owes a transition takes no mutation, which is a
    // different rule (`tests/all/cluster_mutation_status_gate.rs`).
    let clusters = dsql.create_settled_many(rows.len()).await;

    for (index, (row, cluster)) in rows.iter().zip(&clusters).enumerate() {
        // The reference: the same refused request, carrying no token at all.
        let reference = send(&dsql, row.target, cluster, &row.refused, None).await;
        assert_eq!(
            reference.status, 400,
            "case {:?}: the refused input must answer 400 without a token, got {} {:?}",
            row.name, reference.status, reference.body
        );
        assert_eq!(
            reference.error.as_deref(),
            Some("ValidationException"),
            "case {:?}: the refusal must be a ValidationException, got {:?}",
            row.name,
            reference.error
        );
        assert!(
            !reference.str("reason").is_empty(),
            "case {:?}: the refusal must name a reason, got {:?}",
            row.name,
            reference.body
        );

        // A DIFFERENT request of the same operation spends the token.
        let token = format!("spent-{index}");
        let served = send(
            &dsql,
            row.target,
            cluster,
            &row.spender,
            Some(token.as_str()),
        )
        .await;
        assert_eq!(
            served.status, 200,
            "case {:?}: the serving request must spend the token, got {} {:?}",
            row.name, served.status, served.body
        );

        // The refused request, now wearing that spent token.
        let refused = send(
            &dsql,
            row.target,
            cluster,
            &row.refused,
            Some(token.as_str()),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error.clone()),
            (reference.status, reference.error.clone()),
            "case {:?}: expected the no-token answer {} {:?}, got {} {:?}",
            row.name,
            reference.status,
            reference.error,
            refused.status,
            refused.error
        );
        assert_eq!(
            refused.str("reason"),
            reference.str("reason"),
            "case {:?}: expected reason {:?}, got {:?}",
            row.name,
            reference.str("reason"),
            refused.str("reason")
        );
        assert_eq!(
            refused.message(),
            reference.message(),
            "case {:?}: expected the sentence {:?}, got {:?}",
            row.name,
            reference.message(),
            refused.message()
        );

        // The refusal recorded nothing against the token: it still names the
        // request it first served, which is answered with that call's own result.
        let replay = send(
            &dsql,
            row.target,
            cluster,
            &row.spender,
            Some(token.as_str()),
        )
        .await;
        assert_eq!(
            replay.status, 200,
            "case {:?}: the token must still serve its own request, got {} {:?}",
            row.name, replay.status, replay.body
        );
        let member = replayed_member(row.target);
        assert!(
            !served.str(member).is_empty(),
            "case {:?}: the serving request must answer a {member}, got {:?}",
            row.name,
            served.body
        );
        assert_eq!(
            replay.str(member),
            served.str(member),
            "case {:?}: the retry must answer with the first call's {member} ({:?}), got {:?}",
            row.name,
            served.str(member),
            replay.str(member)
        );
    }

    // Nothing the refusals asked for was created: the store holds one cluster per
    // row's fixture plus the one cluster each `CreateCluster` row's serving
    // request minted, and every retry and every refusal minted none.
    let minted = rows
        .iter()
        .filter(|row| row.target == Target::CreateCluster)
        .count();
    let listed = dsql
        .list("?max-results=100")
        .await
        .get("clusters")
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(
        listed,
        rows.len() + minted,
        "the refusals and retries must mint nothing: expected {} cluster(s), got {listed}",
        rows.len() + minted
    );
}

/// RULE, over the two entry points that write a cluster's policy: the verdict
/// belongs to the DOCUMENT, so a spent `clientToken` leaves both answering the
/// same refused document with the same status, error code, reason and sentence —
/// and the document is not attached.
#[tokio::test]
async fn both_policy_entry_points_refuse_the_same_document_alike_under_a_spent_token() {
    let dsql = Dsql::new();
    let documents = refused_documents();
    let clusters = dsql.create_settled_many(documents.len()).await;

    for (index, ((name, document), cluster)) in documents.iter().zip(&clusters).enumerate() {
        let token = format!("policy-{index}");
        // The put path: the token is spent by an acceptable document first.
        let attached = dsql
            .put_policy(
                cluster,
                json!({"policy": acceptable_document(), "clientToken": token.clone()}),
            )
            .await;
        assert_eq!(
            attached.status, 200,
            "case {name:?}: the serving put must spend the token, got {} {:?}",
            attached.status, attached.body
        );
        let put = dsql
            .put_policy(
                cluster,
                json!({"policy": document.clone(), "clientToken": token.clone()}),
            )
            .await;

        // The same document through the create path, in a store of its own, under
        // a token that store's own serving create has spent.
        let fresh = Dsql::new();
        let first = fresh
            .create(json!({"deletionProtectionEnabled": false, "clientToken": token.clone()}))
            .await;
        assert_eq!(
            first.status, 200,
            "case {name:?}: the serving create must spend the token, got {} {:?}",
            first.status, first.body
        );
        let created = fresh
            .create(json!({
                "deletionProtectionEnabled": false,
                "policy": document.clone(),
                "clientToken": token.clone(),
            }))
            .await;

        assert_eq!(
            (put.status, created.status),
            (400, 400),
            "case {name:?}: both entry points must refuse the document with 400, \
             got put {} {:?} and create {} {:?}",
            put.status,
            put.body,
            created.status,
            created.body
        );
        assert_eq!(
            (put.error.as_deref(), created.error.as_deref()),
            (Some("ValidationException"), Some("ValidationException")),
            "case {name:?}: both must answer a ValidationException, got {:?} and {:?}",
            put.error,
            created.error
        );
        assert_eq!(
            put.str("reason"),
            created.str("reason"),
            "case {name:?}: both must answer the same reason, got {:?} and {:?}",
            put.str("reason"),
            created.str("reason")
        );
        assert_eq!(
            put.message(),
            created.message(),
            "case {name:?}: both must answer the same sentence, got {:?} and {:?}",
            put.message(),
            created.message()
        );

        // Neither refusal took effect: the cluster still holds the document the
        // serving put attached, and the refused create minted no second cluster.
        let read = dsql.get_policy(cluster).await;
        assert_eq!(
            read.str("policy"),
            acceptable_document(),
            "case {name:?}: the refused put must leave the attached document alone, got {:?}",
            read.str("policy")
        );
        let listed = fresh
            .list("")
            .await
            .get("clusters")
            .as_array()
            .map_or(0, Vec::len);
        assert_eq!(
            listed, 1,
            "case {name:?}: the refused create must mint nothing, expected 1 cluster, got {listed}"
        );
    }
}
