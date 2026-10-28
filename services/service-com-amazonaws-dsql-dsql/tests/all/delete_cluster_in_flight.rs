// SPDX-License-Identifier: CC0-1.0

//! What a `DeleteCluster` that arrives while the deletion is already under way
//! answers, over the wire.
//!
//! RULE: a delete of a cluster whose deletion is already in flight is SERVED with
//! that deletion — 200, reporting the cluster's current status — in every leg the
//! deletion chain reports to a caller, and it starts nothing new: the transition
//! the clock already owes the cluster is left exactly as the call that began the
//! deletion set it. Only a cluster the clock owes some OTHER transition answers
//! the status gate's 409 `ConflictException`.
//!
//! The legs are the service's own chain: a cluster with a multi-Region peering to
//! unwind reports `PENDING_DELETE` and is then carried on to `DELETING`, while one
//! with no peering starts at `DELETING` — so the leg a retry lands in is what the
//! table spans, beside the cluster kind, how the retry carries a `clientToken`
//! (none at all, or a fresh one the ledger has not seen — `clientToken` is
//! optional, `smithy_query(query="operation", operation="DeleteCluster")`), and how
//! many retries there are.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::{
    ComAmazonawsDsqlDsqlState, STATUS_DELETING, STATUS_PENDING_DELETE,
};

use crate::cluster_policy::allow;
use crate::harness::{wait_for_the_clock, Answer, Dsql};
use crate::multi_region_cluster_status::{peered_pair, FIRST_REGION, WITNESS_REGION};

/// Which kind of cluster a case deletes: the two kinds differ only in whether the
/// deletion has a peering to unwind first, which is what gives the chain its
/// legs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// No multi-Region configuration at all.
    SingleRegion,
    /// Peered with a cluster in another Region, both naming each other.
    MultiRegion,
}

impl Kind {
    /// The statuses this kind's deletion reports to a caller, in order, before
    /// the cluster goes away.
    fn legs(self) -> &'static [&'static str] {
        match self {
            Kind::SingleRegion => &[STATUS_DELETING],
            Kind::MultiRegion => &[STATUS_PENDING_DELETE, STATUS_DELETING],
        }
    }
}

/// How a retry carries its `clientToken`: both forms reach the status gate, since
/// neither is a token the ledger has already served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carried {
    /// No `client-token` query parameter at all.
    NoToken,
    /// A token this request is the first to use, as an SDK mints one per call.
    FreshToken,
}

/// One case: the kind of cluster, how many legs of its chain the retries visit,
/// how many retries land in each leg, and what they carry.
#[derive(Debug)]
struct Case {
    name: &'static str,
    kind: Kind,
    legs: usize,
    repeats: usize,
    carried: Carried,
}

/// A settled cluster of `kind`, sharing `engine`, and the client that reaches it.
async fn settled_cluster(engine: &Arc<engine::InMemoryEngine>, kind: Kind) -> (Dsql, String) {
    match kind {
        Kind::SingleRegion => {
            let dsql = Dsql::sharing(engine, FIRST_REGION);
            let identifier = dsql.create_settled().await;
            (dsql, identifier)
        }
        Kind::MultiRegion => {
            let (first, one_id, _second, _two_id) = peered_pair(engine).await;
            (first, one_id)
        }
    }
}

/// The transition the clock owes the stored cluster, as a comparable value: the
/// instant it comes due and the status it leads to.
fn owed_transition(
    engine: &Arc<engine::InMemoryEngine>,
    identifier: &str,
) -> Option<(SystemTime, &'static str)> {
    let state = engine.state::<ComAmazonawsDsqlDsqlState>();
    let state = state.lock();
    let pending = state.clusters.get(identifier)?.pending.clone()?;
    Some((pending.due_at, pending.next_status))
}

/// The members of a `DeleteClusterOutput` that name the cluster rather than its
/// progress, as one comparable value (`smithy_query(query="operation",
/// operation="DeleteCluster")`: `arn`, `creationTime`, `identifier`, `status`, all
/// `@required`).
fn cluster_members(answer: &Answer) -> Value {
    json!({
        "arn": answer.get("arn"),
        "creationTime": answer.get("creationTime"),
        "identifier": answer.get("identifier"),
    })
}

/// RULE: a delete arriving while the deletion is already in flight answers 200
/// with the cluster's current status, whichever leg of the deletion chain it lands
/// in and whatever (or no) `clientToken` it carries — and it changes nothing: the
/// status stays on that leg, the transition the clock owes is untouched, and the
/// clock still carries the deletion to the cluster's removal.
#[tokio::test]
async fn a_delete_of_a_deletion_in_flight_is_served_on_every_leg() {
    let cases = [
        Case {
            name: "one tokenless retry while the removal is under way",
            kind: Kind::SingleRegion,
            legs: 1,
            repeats: 1,
            carried: Carried::NoToken,
        },
        Case {
            name: "three retries under fresh tokens while the removal is under way",
            kind: Kind::SingleRegion,
            legs: 1,
            repeats: 3,
            carried: Carried::FreshToken,
        },
        Case {
            name: "one tokenless retry while the peering is being unwound",
            kind: Kind::MultiRegion,
            legs: 1,
            repeats: 1,
            carried: Carried::NoToken,
        },
        Case {
            name: "a tokenless retry in each leg of a peered cluster's deletion",
            kind: Kind::MultiRegion,
            legs: 2,
            repeats: 1,
            carried: Carried::NoToken,
        },
        Case {
            name: "three retries under fresh tokens in each leg of a peered deletion",
            kind: Kind::MultiRegion,
            legs: 2,
            repeats: 3,
            carried: Carried::FreshToken,
        },
    ];

    for case in cases {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let (client, identifier) = settled_cluster(&engine, case.kind).await;
        let legs = case.kind.legs();

        let started = client.delete(&identifier).await;
        assert_eq!(
            (started.status, started.str("status")),
            (200, legs[0].to_string()),
            "case {:?}: the delete that starts the deletion must answer 200 {:?}, got {} {:?}",
            case.name,
            legs[0],
            started.status,
            started.body
        );
        let expected = cluster_members(&started);

        for (leg, status) in legs.iter().enumerate().take(case.legs) {
            if leg > 0 {
                wait_for_the_clock().await;
            }
            assert_eq!(
                client.get(&identifier).await.str("status"),
                *status,
                "case {:?}: the cluster must be on the {status:?} leg before the retries",
                case.name
            );
            let owed_before = owed_transition(&engine, &identifier);

            for repeat in 0..case.repeats {
                let answer = match case.carried {
                    Carried::NoToken => client.delete(&identifier).await,
                    Carried::FreshToken => {
                        client
                            .delete_with_token(&identifier, &format!("retry-{leg}-{repeat}"))
                            .await
                    }
                };
                assert_eq!(
                    answer.status, 200,
                    "case {:?}: retry #{repeat} on the {status:?} leg carrying \
                     {:?} must be served with the deletion in flight, got {} {:?}",
                    case.name, case.carried, answer.status, answer.body
                );
                assert_eq!(
                    answer.str("status"),
                    *status,
                    "case {:?}: retry #{repeat} must report the deletion's current status \
                     {status:?}, got {:?}",
                    case.name,
                    answer.str("status")
                );
                assert_eq!(
                    cluster_members(&answer),
                    expected,
                    "case {:?}: retry #{repeat} must answer for the same cluster {expected:?}, \
                     got {:?}",
                    case.name,
                    cluster_members(&answer)
                );
            }

            // The retries started nothing: the cluster is still on this leg and the
            // transition the clock owed it was neither replaced nor re-timed.
            assert_eq!(
                client.get(&identifier).await.str("status"),
                *status,
                "case {:?}: the retries must leave the cluster on the {status:?} leg, got {:?}",
                case.name,
                client.get(&identifier).await.str("status")
            );
            assert_eq!(
                owed_transition(&engine, &identifier),
                owed_before,
                "case {:?}: the retries on the {status:?} leg must leave the transition the \
                 clock owes untouched, got {:?} instead of {owed_before:?}",
                case.name,
                owed_transition(&engine, &identifier)
            );
        }

        // And the clock still finishes the deletion the first call started: the
        // remaining legs, then the removal.
        for _ in 0..=(legs.len() - case.legs) {
            wait_for_the_clock().await;
        }
        let gone = client.get(&identifier).await;
        assert_eq!(
            gone.status, 404,
            "case {:?}: the clock must still remove the cluster, got {} {:?}",
            case.name, gone.status, gone.body
        );
    }
}

/// RULE: the exemption is the DELETE of a deletion already in flight and nothing
/// else — every other write of a cluster whose peering is being unwound still
/// answers the status gate's 409 `ConflictException` naming that status.
#[tokio::test]
async fn only_the_delete_is_served_while_the_peering_is_being_unwound() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let (client, identifier) = settled_cluster(&engine, Kind::MultiRegion).await;

    let started = client.delete(&identifier).await;
    assert_eq!(
        (started.status, started.str("status")),
        (200, STATUS_PENDING_DELETE.to_string()),
        "the delete of a peered cluster must answer {STATUS_PENDING_DELETE}, got {} {:?}",
        started.status,
        started.body
    );

    for (case, refused) in [
        (
            "UpdateCluster",
            client
                .update(&identifier, json!({"deletionProtectionEnabled": true}))
                .await,
        ),
        (
            "UpdateCluster reconfiguring the peering",
            client
                .update(
                    &identifier,
                    json!({"multiRegionProperties": {"witnessRegion": WITNESS_REGION}}),
                )
                .await,
        ),
        (
            "PutClusterPolicy",
            client
                .put_policy(&identifier, json!({"policy": allow("dsql:DbConnect")}))
                .await,
        ),
        ("DeleteClusterPolicy", client.delete_policy(&identifier, "").await),
    ] {
        assert_eq!(
            (refused.status, refused.error.as_deref()),
            (409, Some("ConflictException")),
            "case {case:?}: a write that is not the delete must be refused while the cluster \
             is {STATUS_PENDING_DELETE}, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused.message().contains(STATUS_PENDING_DELETE)
                && refused.message().contains(&identifier),
            "case {case:?}: the refusal must name the cluster and the status that refused it, \
             got {:?}",
            refused.message()
        );
    }

    // The delete, by contrast, is served — and the cluster is still on the same leg.
    let repeated = client.delete(&identifier).await;
    assert_eq!(
        (repeated.status, repeated.str("status")),
        (200, STATUS_PENDING_DELETE.to_string()),
        "a repeated delete must be served with the deletion in flight, got {} {:?}",
        repeated.status,
        repeated.body
    );
}
