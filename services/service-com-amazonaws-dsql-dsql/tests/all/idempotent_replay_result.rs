// SPDX-License-Identifier: CC0-1.0

//! What a `clientToken` retry ANSWERS, end to end over the wire.
//!
//! RULE: a retry the service's idempotency ledger recognises is served from the
//! record of the original call — it answers with the result that call produced
//! and has no effect of its own — so a `policyVersion` the first call issued is
//! what the retry reports whatever calls landed in between, and the retry
//! neither attaches nor removes a document.
//!
//! The table spans what happens to the cluster's policy BETWEEN the first call
//! and its retry, which is what the rule generalises over: nothing, one or
//! several further writes, a removal, a removal and a re-attachment, a write
//! under another token, a write conditioned on the version the first call
//! issued. The case the simple replay in `tests/all/cluster_policy.rs` covers
//! (nothing in between, so the original answer and the current state coincide)
//! is one row of it.
//!
//! The same rule over the resource-minting calls is the third suite here: the
//! `status` a `CreateCluster`, `UpdateCluster` or `CreateStream` retry answers
//! is the one its first call answered, not the one the clock has moved the
//! resource to since.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::json;

use crate::error_contract::stream_request;
use crate::harness::{wait_for_the_clock, Dsql};
use crate::policy_version_conflict_and_lockout::benign_document;

/// What happens to a cluster's policy between the first `PutClusterPolicy` and
/// its retry under the same `clientToken`.
#[derive(Debug, Clone, Copy)]
enum Intervening {
    /// Nothing at all.
    Nothing,
    /// That many further tokenless writes, each attaching another document.
    Writes(usize),
    /// The policy removed.
    Removed,
    /// The policy removed, and another attached in its place.
    RemovedAndReattached,
    /// A write carrying a DIFFERENT `clientToken`.
    WriteUnderAnotherToken,
    /// A write conditioned on the version the first call issued.
    ConditionalWrite,
}

/// One row: what happened in between, under a name the assertions quote.
struct ReplayCase {
    name: &'static str,
    intervening: Intervening,
}

fn replay_cases() -> Vec<ReplayCase> {
    vec![
        ReplayCase {
            name: "nothing in between",
            intervening: Intervening::Nothing,
        },
        ReplayCase {
            name: "one tokenless write in between",
            intervening: Intervening::Writes(1),
        },
        ReplayCase {
            name: "three tokenless writes in between",
            intervening: Intervening::Writes(3),
        },
        ReplayCase {
            name: "the policy removed in between",
            intervening: Intervening::Removed,
        },
        ReplayCase {
            name: "the policy removed and another attached in between",
            intervening: Intervening::RemovedAndReattached,
        },
        ReplayCase {
            name: "a write under another clientToken in between",
            intervening: Intervening::WriteUnderAnotherToken,
        },
        ReplayCase {
            name: "a conditional write in between",
            intervening: Intervening::ConditionalWrite,
        },
    ]
}

/// What the policy reader must report, as a case's intervening calls left it:
/// the document and the version attached, or `None` when nothing is attached.
type Attached = Option<(String, String)>;

/// Attach `document` to `identifier` with whatever extra members `extra`
/// carries, and answer with the version it got.
async fn attach(
    dsql: &Dsql,
    case: &str,
    identifier: &str,
    document: &str,
    extra: &[(&str, &str)],
) -> String {
    let mut body = json!({"policy": document});
    for (member, value) in extra {
        body[*member] = json!(*value);
    }
    let answer = dsql.put_policy(identifier, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: an intervening write must answer 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.str("policyVersion")
}

/// Run a case's intervening calls and answer with what the policy reader must
/// report once they are done.
async fn intervene(
    dsql: &Dsql,
    case: &ReplayCase,
    identifier: &str,
    first_document: &str,
    first_version: &str,
) -> Attached {
    match case.intervening {
        Intervening::Nothing => Some((first_document.to_string(), first_version.to_string())),
        Intervening::Writes(count) => {
            let mut attached = (first_document.to_string(), first_version.to_string());
            for round in 1..=count {
                let document = benign_document(round);
                let version = attach(dsql, case.name, identifier, &document, &[]).await;
                attached = (document, version);
            }
            Some(attached)
        }
        Intervening::Removed => {
            remove(dsql, case.name, identifier).await;
            None
        }
        Intervening::RemovedAndReattached => {
            remove(dsql, case.name, identifier).await;
            let document = benign_document(1);
            let version = attach(dsql, case.name, identifier, &document, &[]).await;
            Some((document, version))
        }
        Intervening::WriteUnderAnotherToken => {
            let document = benign_document(1);
            let version = attach(
                dsql,
                case.name,
                identifier,
                &document,
                &[("clientToken", "another-token")],
            )
            .await;
            Some((document, version))
        }
        Intervening::ConditionalWrite => {
            let document = benign_document(1);
            let version = attach(
                dsql,
                case.name,
                identifier,
                &document,
                &[("expectedPolicyVersion", first_version)],
            )
            .await;
            Some((document, version))
        }
    }
}

/// Remove `identifier`'s policy with no `clientToken`, asserting it was there.
async fn remove(dsql: &Dsql, case: &str, identifier: &str) {
    let removed = dsql.delete_policy(identifier, "").await;
    assert_eq!(
        removed.status, 200,
        "case {case:?}: an intervening removal must answer 200, got {} {:?}",
        removed.status, removed.body
    );
}

/// Assert the policy reader reports exactly `expected`.
async fn assert_attached(dsql: &Dsql, case: &str, identifier: &str, expected: &Attached) {
    let read = dsql.get_policy(identifier).await;
    match expected {
        Some((document, version)) => {
            assert_eq!(
                read.status, 200,
                "case {case:?}: the policy left in place must still be readable, got {} {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.str("policy"),
                *document,
                "case {case:?}: the stored document must be the one the intervening calls left, \
                 got {:?}",
                read.str("policy")
            );
            assert_eq!(
                read.str("policyVersion"),
                *version,
                "case {case:?}: the stored version must be the one the intervening calls left, \
                 got {:?}",
                read.str("policyVersion")
            );
        }
        None => assert_eq!(
            read.status, 404,
            "case {case:?}: no policy may be attached, got {} {:?}",
            read.status, read.body
        ),
    }
}

/// RULE: a `PutClusterPolicy` retry under a `clientToken` the service already
/// served answers with the `policyVersion` the FIRST call issued — not the
/// cluster's current one — and attaches nothing, whatever writes, removals and
/// re-attachments landed in between.
#[tokio::test]
async fn a_put_policy_retry_answers_the_version_the_first_call_issued() {
    let dsql = Dsql::new();
    let cases = replay_cases();
    let clusters = dsql.create_settled_many(cases.len()).await;
    let first_document = benign_document(0);

    for (index, case) in cases.iter().enumerate() {
        let identifier = &clusters[index];
        let token = format!("put-replay-{index}");
        let first_version = attach(
            &dsql,
            case.name,
            identifier,
            &first_document,
            &[("clientToken", token.as_str())],
        )
        .await;

        let attached = intervene(&dsql, case, identifier, &first_document, &first_version).await;
        // Every row but the first must have moved the cluster off that version,
        // so answering the current state is distinguishable from answering the
        // first call's result.
        if !matches!(case.intervening, Intervening::Nothing) {
            assert_ne!(
                attached.as_ref().map(|(_, version)| version.clone()),
                Some(first_version.clone()),
                "case {:?}: the intervening calls must leave a version other than {:?}, got {:?}",
                case.name,
                first_version,
                attached
            );
        }

        let replay = dsql
            .put_policy(
                identifier,
                json!({"policy": first_document.clone(), "clientToken": token.clone()}),
            )
            .await;
        assert_eq!(
            replay.status, 200,
            "case {:?}: a retry of the same request must answer 200, got {} {:?}",
            case.name, replay.status, replay.body
        );
        assert_eq!(
            replay.str("policyVersion"),
            first_version,
            "case {:?}: a retry must answer the version the first call issued ({:?}), got {:?}",
            case.name,
            first_version,
            replay.str("policyVersion")
        );

        // The retry had no effect of its own: the reader still reports exactly
        // what the intervening calls left.
        assert_attached(&dsql, case.name, identifier, &attached).await;
    }
}

/// What happens to a cluster's policy between the first `DeleteClusterPolicy`
/// and its retry under the same `clientToken`.
#[derive(Debug, Clone, Copy)]
enum AfterDelete {
    /// Nothing at all.
    Nothing,
    /// Another policy attached since.
    Reattached,
    /// Another policy attached and removed again since.
    ReattachedAndRemoved,
}

/// RULE: the same record serves the removal side — a `DeleteClusterPolicy` retry
/// answers the `policyVersion` the FIRST delete removed and removes nothing
/// further, so a policy attached since the first delete is still attached
/// afterwards.
#[tokio::test]
async fn a_delete_policy_retry_answers_the_version_it_removed_and_removes_nothing_further() {
    let dsql = Dsql::new();
    let rows = [
        ("nothing in between", AfterDelete::Nothing),
        ("another policy attached in between", AfterDelete::Reattached),
        (
            "another policy attached and removed in between",
            AfterDelete::ReattachedAndRemoved,
        ),
    ];
    let clusters = dsql.create_settled_many(rows.len()).await;

    for (index, (name, after)) in rows.iter().enumerate() {
        let identifier = &clusters[index];
        let token = format!("delete-replay-{index}");
        let document = benign_document(0);
        let removed_version = attach(&dsql, name, identifier, &document, &[]).await;

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
            removed_version,
            "case {name:?}: the delete must report the version it removed, got {:?}",
            deleted.str("policyVersion")
        );

        let attached: Attached = match after {
            AfterDelete::Nothing => None,
            AfterDelete::Reattached => {
                let later = benign_document(1);
                let version = attach(&dsql, name, identifier, &later, &[]).await;
                Some((later, version))
            }
            AfterDelete::ReattachedAndRemoved => {
                let later = benign_document(1);
                attach(&dsql, name, identifier, &later, &[]).await;
                remove(&dsql, name, identifier).await;
                None
            }
        };

        let retried = dsql
            .delete_policy(identifier, &format!("?client-token={token}"))
            .await;
        assert_eq!(
            retried.status, 200,
            "case {name:?}: a retry of the same delete must answer 200, got {} {:?}",
            retried.status, retried.body
        );
        assert_eq!(
            retried.str("policyVersion"),
            removed_version,
            "case {name:?}: a retry must answer the version the first delete removed ({:?}), \
             got {:?}",
            removed_version,
            retried.str("policyVersion")
        );

        assert_attached(&dsql, name, identifier, &attached).await;
    }
}

/// Which resource-minting call a row retries once the clock has settled what it
/// minted.
#[derive(Debug, Clone, Copy)]
enum Minting {
    /// `CreateCluster`: its first answer is a cluster the clock still owes
    /// `ACTIVE`.
    CreateCluster,
    /// `UpdateCluster`: its first answer is a cluster it put into `UPDATING`.
    UpdateCluster,
    /// `CreateStream`: its first answer is a stream the clock still owes
    /// `ACTIVE`.
    CreateStream,
}

/// One row's two answers, the resource's live status, and how many resources of
/// that kind exist afterwards.
struct Round {
    first_status: String,
    replay_status: String,
    live_status: String,
    first_identity: String,
    replay_identity: String,
    minted: usize,
}

/// RULE: the same record serves the resource-minting calls — a retry answers the
/// `status` the FIRST call answered, not the one the clock or a later call has
/// moved the resource to since, and mints nothing second.
#[tokio::test]
async fn a_minting_call_retry_answers_the_status_the_first_call_answered() {
    let rows = [
        ("CreateCluster", Minting::CreateCluster),
        ("UpdateCluster", Minting::UpdateCluster),
        ("CreateStream", Minting::CreateStream),
    ];

    for (name, minting) in rows {
        // A store of its own per row, so the count of minted resources is the
        // row's own.
        let dsql = Dsql::new();
        let token = format!("minting-{name}");
        let round = match minting {
            Minting::CreateCluster => {
                let request =
                    json!({"deletionProtectionEnabled": false, "clientToken": token.clone()});
                let first = dsql.create(request.clone()).await;
                assert_eq!(
                    first.status, 200,
                    "case {name}: the first call must answer 200, got {} {:?}",
                    first.status, first.body
                );
                wait_for_the_clock().await;
                let replay = dsql.create(request).await;
                assert_eq!(
                    replay.status, 200,
                    "case {name}: the retry must answer 200, got {} {:?}",
                    replay.status, replay.body
                );
                let identifier = first.str("identifier");
                Round {
                    first_status: first.str("status"),
                    replay_status: replay.str("status"),
                    live_status: dsql.get(&identifier).await.str("status"),
                    first_identity: identifier,
                    replay_identity: replay.str("identifier"),
                    minted: dsql
                        .list("?max-results=100")
                        .await
                        .get("clusters")
                        .as_array()
                        .map_or(0, Vec::len),
                }
            }
            Minting::UpdateCluster => {
                let identifier = dsql.create_settled().await;
                let request =
                    json!({"deletionProtectionEnabled": true, "clientToken": token.clone()});
                let first = dsql.update(&identifier, request.clone()).await;
                assert_eq!(
                    first.status, 200,
                    "case {name}: the first call must answer 200, got {} {:?}",
                    first.status, first.body
                );
                wait_for_the_clock().await;
                let replay = dsql.update(&identifier, request).await;
                assert_eq!(
                    replay.status, 200,
                    "case {name}: the retry must answer 200, got {} {:?}",
                    replay.status, replay.body
                );
                Round {
                    first_status: first.str("status"),
                    replay_status: replay.str("status"),
                    live_status: dsql.get(&identifier).await.str("status"),
                    first_identity: first.str("identifier"),
                    replay_identity: replay.str("identifier"),
                    minted: dsql
                        .list("?max-results=100")
                        .await
                        .get("clusters")
                        .as_array()
                        .map_or(0, Vec::len),
                }
            }
            Minting::CreateStream => {
                let cluster = dsql.create_settled().await;
                let mut request = stream_request(engine::EMULATOR_ACCOUNT_ID, "dsql-cdc");
                request["clientToken"] = json!(token.clone());
                let first = dsql.create_stream(&cluster, request.clone()).await;
                assert_eq!(
                    first.status, 200,
                    "case {name}: the first call must answer 200, got {} {:?}",
                    first.status, first.body
                );
                wait_for_the_clock().await;
                let replay = dsql.create_stream(&cluster, request).await;
                assert_eq!(
                    replay.status, 200,
                    "case {name}: the retry must answer 200, got {} {:?}",
                    replay.status, replay.body
                );
                let stream = first.str("streamIdentifier");
                Round {
                    first_status: first.str("status"),
                    replay_status: replay.str("status"),
                    live_status: dsql.get_stream(&cluster, &stream).await.str("status"),
                    first_identity: stream,
                    replay_identity: replay.str("streamIdentifier"),
                    minted: dsql
                        .list_streams(&cluster, "?max-results=100")
                        .await
                        .get("streams")
                        .as_array()
                        .map_or(0, Vec::len),
                }
            }
        };

        assert_eq!(
            round.replay_identity, round.first_identity,
            "case {name}: the retry must answer with the resource the first call named ({:?}), \
             got {:?}",
            round.first_identity, round.replay_identity
        );
        assert_eq!(
            round.replay_status, round.first_status,
            "case {name}: the retry must answer the status the first call answered ({:?}), \
             got {:?}",
            round.first_status, round.replay_status
        );
        // The row is only a test of that if the clock has moved the resource on
        // in the meantime, so the recorded answer is distinguishable from the
        // live one.
        assert_ne!(
            round.live_status, round.first_status,
            "case {name}: the clock must have moved the resource off {:?} before the retry",
            round.first_status
        );
        assert_eq!(
            round.minted, 1,
            "case {name}: the retry must mint nothing second, got {} resource(s)",
            round.minted
        );
    }
}
