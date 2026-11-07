// SPDX-License-Identifier: CC0-1.0

//! What becomes of an idempotency record when the clock retires the cluster the
//! call it describes acted on.
//!
//! RULE: a `clientToken` record is keyed by the resource its call acted on, and it
//! is dropped when that resource ceases to exist — including when it ceases to
//! exist because the cluster holding it was removed — so the token stands for no
//! earlier request afterwards and serves a fresh one; only the ledger of an
//! operation that REMOVES a resource keeps its record, which is what a retry
//! arriving after the removal is answered from.
//!
//! The first table walks every ledger against both kinds of resource a record is
//! keyed by — a cluster, and a stream stored on a cluster's record — with a second,
//! untouched cluster as the control. The second table drives the whole class of
//! token-carrying operations over the wire through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::ops::types::{KinesisTargetDefinition, TargetDefinition};
use service_com_amazonaws_dsql_dsql::state::{
    ComAmazonawsDsqlDsqlState, RecordedCall, Stream, TokenScope, TokenVerdict, STATUS_ACTIVE,
    STREAM_STATUS_ACTIVE,
};

use crate::cluster_lifecycle::{cluster_id_for, retiring_cluster};
use crate::cluster_policy::allow;
use crate::harness::{expected_stream_arn, wait_for_the_clock, Answer, Dsql};
use crate::stream_lifecycle::kinesis_target;

/// A `StreamId`-shaped identifier for case `n`. `StreamId` and `ClusterId` have
/// the same shape (`^[a-z0-9]{26}$`, `smithy_query(query="shape",
/// shape="StreamId,ClusterId")`), so one builder serves both.
fn stream_id_for(n: usize) -> String {
    cluster_id_for(1_000 + n)
}

/// A stored, settled stream of `cluster`, as `CreateStream` persists one.
fn stored_stream(region: &str, cluster: &str, stream: &str) -> Stream {
    Stream {
        stream_identifier: stream.to_string(),
        arn: expected_stream_arn(region, cluster, stream),
        status: STREAM_STATUS_ACTIVE.to_string(),
        creation_time: SystemTime::now(),
        ordering: "UNORDERED".to_string(),
        format: "JSON".to_string(),
        target_definition: TargetDefinition::Kinesis(KinesisTargetDefinition {
            stream_arn: format!(
                "arn:aws:kinesis:{region}:{}:stream/changes",
                engine::EMULATOR_ACCOUNT_ID
            ),
            role_arn: format!("arn:aws:iam::{}:role/dsql-cdc", engine::EMULATOR_ACCOUNT_ID),
        }),
        tags: Default::default(),
        pending: None,
        failure: None,
        status_updated_at: SystemTime::now(),
    }
}

/// The token a row records in `scope` against `resource`. Its fingerprint is the
/// same string, so a record that is still there answers `Replay` and one that is
/// gone answers `Unused`.
fn token_for(scope: TokenScope, resource: &str) -> String {
    format!("{scope:?}-on-{resource}")
}

/// RULE (at the ledger): retiring a cluster drops every record keyed by a resource
/// that retirement removes — the cluster itself and each stream stored on its
/// record — in every ledger but those belonging to a call that removes a resource,
/// and leaves the records of a cluster that is still there alone.
///
/// Driven against the state directly: a ledger is per-operation, so covering
/// every one over the wire would need one suite per operation — the wire-level
/// class is the test below.
#[test]
fn retiring_a_cluster_drops_every_token_keyed_by_what_it_removes() {
    let already_due = SystemTime::now() - Duration::from_secs(1);
    let region = "us-east-1";

    let retiring = cluster_id_for(0);
    let surviving = cluster_id_for(1);
    // Two streams on the cluster on its way out, so the assertion is about every
    // stream it carries rather than one of them, and one on the cluster that stays.
    let retiring_streams = [stream_id_for(0), stream_id_for(1)];
    let surviving_stream = stream_id_for(2);

    let mut state = ComAmazonawsDsqlDsqlState::default();

    let mut doomed = retiring_cluster(&retiring, already_due);
    for stream in &retiring_streams {
        doomed
            .streams
            .insert(stream.clone(), stored_stream(region, &retiring, stream));
    }
    state.clusters.insert(retiring.clone(), doomed);

    // The control: the same stored record with nothing due, so this cluster and
    // its stream are still there when `settle` returns.
    let mut survivor = retiring_cluster(&surviving, already_due);
    survivor.status = STATUS_ACTIVE.to_string();
    survivor.pending = None;
    survivor.streams.insert(
        surviving_stream.clone(),
        stored_stream(region, &surviving, &surviving_stream),
    );
    state.clusters.insert(surviving.clone(), survivor);

    let removed: Vec<String> = std::iter::once(retiring.clone())
        .chain(retiring_streams.iter().cloned())
        .collect();
    let kept = [surviving.clone(), surviving_stream.clone()];

    for scope in TokenScope::ALL {
        for resource in removed.iter().chain(kept.iter()) {
            let token = token_for(scope, resource);
            state
                .tokens(scope)
                .record(region, token.clone(), token, RecordedCall::new(resource));
        }
    }

    state.settle(SystemTime::now());

    assert!(
        !state.clusters.contains_key(&retiring),
        "the cluster whose deletion was due must be retired, got {:?}",
        state.clusters.keys().collect::<Vec<_>>()
    );
    assert!(
        state.clusters.contains_key(&surviving),
        "the cluster with nothing due must still be there, got {:?}",
        state.clusters.keys().collect::<Vec<_>>()
    );

    for scope in TokenScope::ALL {
        // The exemption: the record of a call that REMOVES a resource has to
        // outlive it, because that is when a retry of it arrives — the delete of
        // the cluster, and the delete of a stream the cluster carried.
        let outlives = matches!(scope, TokenScope::DeleteCluster | TokenScope::DeleteStream);
        for resource in &removed {
            let token = token_for(scope, resource);
            let verdict = state.tokens_of(scope).verdict(region, &token, &token);
            let case = format!("{scope:?} keyed by the removed {resource}");
            if outlives {
                assert_eq!(
                    verdict,
                    TokenVerdict::Replay(RecordedCall::new(resource)),
                    "case {case:?}: the record of the removing call must stand, got {verdict:?}"
                );
            } else {
                assert_eq!(
                    verdict,
                    TokenVerdict::Unused,
                    "case {case:?}: the record must go with the resource, got {verdict:?}"
                );
            }
        }
        for resource in &kept {
            let token = token_for(scope, resource);
            let verdict = state.tokens_of(scope).verdict(region, &token, &token);
            let case = format!("{scope:?} keyed by the surviving {resource}");
            assert_eq!(
                verdict,
                TokenVerdict::Replay(RecordedCall::new(resource)),
                "case {case:?}: a record of a resource that is still there must stand, \
                 got {verdict:?}"
            );
        }
    }
}

/// Which operation first used the row's token — that is, which ledger holds the
/// record and which resource the record is keyed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FirstUse {
    CreateCluster,
    UpdateCluster,
    PutClusterPolicy,
    DeleteClusterPolicy,
    /// The one writer whose record is keyed by a STREAM rather than by the
    /// cluster; `streams` is how many streams the retired cluster carried, each
    /// under its own token.
    CreateStream {
        streams: usize,
    },
    /// The exemption: the record of the call that removes the cluster.
    DeleteCluster,
}

/// Assert `answer` was served, and hand it back for the row's own assertions.
fn served<'a>(case: &str, which: &str, answer: &'a Answer) -> &'a Answer {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: {which} must answer 200, got {} {:?}",
        answer.status, answer.body
    );
    answer
}

/// A `CreateStream` request under `token`, the same one every row sends: what
/// differs between the two uses of the token is the cluster it is sent to, which
/// is part of what the token is bound to.
fn stream_request(token: &str) -> Value {
    json!({
        "format": "JSON",
        "ordering": "UNORDERED",
        "targetDefinition": kinesis_target("us-east-1", "changes", "dsql-cdc"),
        "clientToken": token,
    })
}

/// RULE (on the wire): once the clock has retired a cluster, a `clientToken` that
/// named a resource that retirement removed serves a fresh request rather than
/// being refused as a conflicting reuse — whichever operation recorded it and
/// whether it was recorded against the cluster or against a stream of it — while
/// the token of the delete that removed the cluster still names that delete.
#[tokio::test]
async fn a_token_of_a_retired_clusters_resource_serves_a_fresh_request() {
    let rows = [
        FirstUse::CreateCluster,
        FirstUse::UpdateCluster,
        FirstUse::PutClusterPolicy,
        FirstUse::DeleteClusterPolicy,
        FirstUse::CreateStream { streams: 1 },
        FirstUse::CreateStream { streams: 3 },
        FirstUse::DeleteCluster,
    ];

    for row in rows {
        let case = format!("{row:?}");
        let dsql = Dsql::new();
        let tokens: Vec<String> = match row {
            FirstUse::CreateStream { streams } => {
                (0..streams).map(|n| format!("token-{n}")).collect()
            }
            _ => vec!["token-0".to_string()],
        };

        // The cluster the clock will retire. For the create row the token's first
        // use IS this creation.
        let doomed = match row {
            FirstUse::CreateCluster => {
                let created = dsql
                    .create(json!({
                        "deletionProtectionEnabled": false,
                        "tags": {"use": "first"},
                        "clientToken": tokens[0],
                    }))
                    .await;
                let identifier = served(&case, "the first CreateCluster", &created).str("identifier");
                wait_for_the_clock().await;
                identifier
            }
            _ => dsql.create_settled().await,
        };

        // The token's first use, on a resource this cluster's retirement removes.
        let mut first_streams = Vec::new();
        match row {
            FirstUse::CreateCluster | FirstUse::DeleteCluster => {}
            FirstUse::UpdateCluster => {
                let updated = dsql
                    .update(
                        &doomed,
                        json!({"deletionProtectionEnabled": false, "clientToken": tokens[0]}),
                    )
                    .await;
                served(&case, "the first UpdateCluster", &updated);
                wait_for_the_clock().await;
            }
            FirstUse::PutClusterPolicy => {
                let attached = dsql
                    .put_policy(
                        &doomed,
                        json!({"policy": allow("dsql:DbConnect"), "clientToken": tokens[0]}),
                    )
                    .await;
                served(&case, "the first PutClusterPolicy", &attached);
            }
            FirstUse::DeleteClusterPolicy => {
                let attached = dsql
                    .put_policy(&doomed, json!({"policy": allow("dsql:DbConnect")}))
                    .await;
                served(&case, "attaching the policy the delete removes", &attached);
                let removed = dsql
                    .delete_policy(&doomed, &format!("?client-token={}", tokens[0]))
                    .await;
                served(&case, "the first DeleteClusterPolicy", &removed);
            }
            FirstUse::CreateStream { .. } => {
                for token in &tokens {
                    let created = dsql.create_stream(&doomed, stream_request(token)).await;
                    first_streams
                        .push(served(&case, "the first CreateStream", &created).str("streamIdentifier"));
                }
            }
        }

        // The clock retires the cluster, which takes its streams with it.
        let deleted = match row {
            FirstUse::DeleteCluster => dsql.delete_with_token(&doomed, &tokens[0]).await,
            _ => dsql.delete(&doomed).await,
        };
        served(&case, "DeleteCluster", &deleted);
        wait_for_the_clock().await;
        let gone = dsql.get(&doomed).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: the cluster must be retired before the token is reused, got {} {:?}",
            gone.status, gone.body
        );

        // The second use of each token, on a cluster that is still there.
        let live = dsql.create_settled().await;
        match row {
            FirstUse::CreateCluster => {
                let again = dsql
                    .create(json!({
                        "deletionProtectionEnabled": false,
                        "tags": {"use": "second"},
                        "clientToken": tokens[0],
                    }))
                    .await;
                let minted = served(&case, "the reused CreateCluster", &again).str("identifier");
                assert_ne!(
                    minted, doomed,
                    "case {case:?}: the reused token must mint a new cluster, got {minted:?}"
                );
                let fetched = dsql.get(&minted).await;
                assert_eq!(
                    fetched.get("tags"),
                    json!({"use": "second"}),
                    "case {case:?}: the fresh request's own parameters must be the ones stored, \
                     got {:?}",
                    fetched.get("tags")
                );
            }
            FirstUse::UpdateCluster => {
                let again = dsql
                    .update(
                        &live,
                        json!({"deletionProtectionEnabled": true, "clientToken": tokens[0]}),
                    )
                    .await;
                served(&case, "the reused UpdateCluster", &again);
                wait_for_the_clock().await;
                let fetched = dsql.get(&live).await;
                assert_eq!(
                    fetched.get("deletionProtectionEnabled"),
                    json!(true),
                    "case {case:?}: the fresh update must be the one that took effect, got {:?}",
                    fetched.get("deletionProtectionEnabled")
                );
            }
            FirstUse::PutClusterPolicy => {
                let document = allow("dsql:DbConnectAdmin");
                let again = dsql
                    .put_policy(
                        &live,
                        json!({"policy": document.clone(), "clientToken": tokens[0]}),
                    )
                    .await;
                served(&case, "the reused PutClusterPolicy", &again);
                let fetched = dsql.get_policy(&live).await;
                assert_eq!(
                    fetched.str("policy"),
                    document,
                    "case {case:?}: the fresh write's document must be the one stored, got {:?}",
                    fetched.str("policy")
                );
            }
            FirstUse::DeleteClusterPolicy => {
                let attached = dsql
                    .put_policy(&live, json!({"policy": allow("dsql:DbConnect")}))
                    .await;
                let version = served(&case, "attaching a policy to the live cluster", &attached)
                    .str("policyVersion");
                let again = dsql
                    .delete_policy(&live, &format!("?client-token={}", tokens[0]))
                    .await;
                assert_eq!(
                    served(&case, "the reused DeleteClusterPolicy", &again).str("policyVersion"),
                    version,
                    "case {case:?}: the fresh delete must report the version IT removed, got {:?}",
                    again.str("policyVersion")
                );
                let fetched = dsql.get_policy(&live).await;
                assert_eq!(
                    fetched.status, 404,
                    "case {case:?}: the fresh delete must have removed the policy, got {} {:?}",
                    fetched.status, fetched.body
                );
            }
            FirstUse::CreateStream { .. } => {
                for (n, token) in tokens.iter().enumerate() {
                    let again = dsql.create_stream(&live, stream_request(token)).await;
                    let minted = served(&case, &format!("the reused CreateStream #{n}"), &again)
                        .str("streamIdentifier");
                    assert!(
                        !first_streams.contains(&minted),
                        "case {case:?}: the reused token must mint a new stream, got {minted:?}"
                    );
                    let fetched = dsql.get_stream(&live, &minted).await;
                    assert_eq!(
                        fetched.str("streamIdentifier"),
                        minted,
                        "case {case:?}: the fresh stream must read back from the live cluster, \
                         got {} {:?}",
                        fetched.status,
                        fetched.body
                    );
                }
            }
            FirstUse::DeleteCluster => {
                let again = dsql.delete_with_token(&live, &tokens[0]).await;
                assert_eq!(
                    again.status, 409,
                    "case {case:?}: the record of the removing call stands, so the same token \
                     carried by another delete is a conflict, got {} {:?}",
                    again.status, again.body
                );
                assert_eq!(
                    again.error.as_deref(),
                    Some("ConflictException"),
                    "case {case:?}: the refusal must be a ConflictException, got {:?}",
                    again.error
                );
                let fetched = dsql.get(&live).await;
                assert_eq!(
                    served(&case, "GetCluster on the live cluster", &fetched).str("status"),
                    STATUS_ACTIVE,
                    "case {case:?}: the refused delete must have deleted nothing, got {:?}",
                    fetched.str("status")
                );
            }
        }
    }
}
