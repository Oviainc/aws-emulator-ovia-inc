// SPDX-License-Identifier: CC0-1.0

//! RULE: a call that mutates a cluster's stored record is refused with 409
//! `ConflictException` unless the cluster's status accepts a mutation — a cluster
//! the clock still owes a transition, or one already on its way out, accepts none.
//! Attaching a resource-based policy and detaching it are two mutations of the
//! SAME field of that record, so both policy writers follow the gate every other
//! mutation of a cluster follows, and at any one instant the two answer alike.
//!
//! The table spans the cluster's status (the terminal one that accepts a mutation,
//! and the three transient ones the clock owes after a create, after an update and
//! after a delete), the DIRECTION of the policy mutation (`PutClusterPolicy`
//! attaches, `DeleteClusterPolicy` detaches) and whether a policy is attached at
//! all — so a refusal that is really the "nothing attached" 404 cannot pass for the
//! status refusal. `GetClusterPolicy` reads the record back in every case: a
//! refused mutation leaves the stored policy exactly as it was, and the refusal
//! lifts by itself once the transition the clock owes comes due.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs`, this crate's one test client.

use serde_json::json;

use crate::harness::{wait_for_the_clock, Answer, Dsql};
use crate::policy_version_conflict_and_lockout::benign_document;

/// The status a case's cluster holds when its policy mutations are issued, and how
/// the case gets it there. The names are the wire spellings of `ClusterStatus`
/// (`smithy_query(query="shape", shape="ClusterStatus")`: CREATING | ACTIVE | IDLE
/// | INACTIVE | UPDATING | DELETING | DELETED | FAILED | PENDING_SETUP |
/// PENDING_DELETE).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Settled after the create: a status that accepts a mutation.
    Active,
    /// Straight out of `CreateCluster`, inside the span the clock owes `ACTIVE`.
    Creating,
    /// Settled, then driven back into a transient status by `UpdateCluster`.
    Updating,
    /// Settled, then started on its way out by `DeleteCluster`.
    Deleting,
}

impl Phase {
    /// The `ClusterStatus` `GetCluster` reports for a cluster in this phase.
    fn status(self) -> &'static str {
        match self {
            Phase::Active => "ACTIVE",
            Phase::Creating => "CREATING",
            Phase::Updating => "UPDATING",
            Phase::Deleting => "DELETING",
        }
    }

    /// Whether a cluster in this status accepts a mutation at all.
    fn accepts_mutation(self) -> bool {
        self == Phase::Active
    }

    /// Whether the case has to create its cluster at assertion time: a `CREATING`
    /// cluster is one the clock has not settled yet, so it cannot be prepared
    /// before the suite's one wait.
    fn is_fresh(self) -> bool {
        self == Phase::Creating
    }
}

/// One row: the status the cluster is in, and whether it already carries a policy.
#[derive(Clone, Copy, Debug)]
struct Case {
    phase: Phase,
    attached: bool,
}

/// The whole class: every status crossed with both states of the policy field.
fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for phase in [
        Phase::Active,
        Phase::Creating,
        Phase::Updating,
        Phase::Deleting,
    ] {
        for attached in [true, false] {
            cases.push(Case { phase, attached });
        }
    }
    cases
}

/// A label naming the row, carried into every assertion of it.
fn label(case: Case) -> String {
    format!(
        "{} cluster with {} policy",
        case.phase.status(),
        if case.attached { "an attached" } else { "no" }
    )
}

/// Create a cluster, with a policy already attached by `CreateCluster` itself when
/// the row wants one — the only way to get a policy onto a cluster that is still
/// `CREATING`, since attaching one through `PutClusterPolicy` is the very mutation
/// under test. `round` varies the document so no two rows attach the same one.
async fn create(dsql: &Dsql, case: Case, round: usize) -> String {
    let body = if case.attached {
        json!({"deletionProtectionEnabled": false, "policy": benign_document(round)})
    } else {
        json!({"deletionProtectionEnabled": false})
    };
    let created = dsql.create(body).await;
    assert_eq!(
        created.status,
        200,
        "case {}: CreateCluster must succeed, got {} {:?}",
        label(case),
        created.status,
        created.body
    );
    created.str("identifier")
}

/// Every refusal the status gate answers: 409 `ConflictException`, naming the
/// cluster the call addressed and the status that refused it, so a caller can tell
/// waiting for the clock from giving up.
fn assert_refused(case: Case, operation: &str, answer: &Answer, identifier: &str) {
    let case = label(case);
    assert_eq!(
        answer.status, 409,
        "case {case}: {operation} must be refused with 409, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error.as_deref(),
        Some("ConflictException"),
        "case {case}: {operation} must be refused with a ConflictException, got {:?}",
        answer.error
    );
    let message = answer.message();
    assert!(
        message.contains(identifier),
        "case {case}: {operation}'s refusal must name the cluster {identifier}, got {message:?}"
    );
}

#[tokio::test]
async fn a_cluster_whose_status_refuses_a_mutation_refuses_both_policy_writers() {
    let dsql = Dsql::new();
    let cases = cases();

    // Every row but the `CREATING` ones needs a settled cluster, so they are all
    // created before the suite's one wait rather than one wait per row.
    let mut prepared: Vec<Option<String>> = Vec::with_capacity(cases.len());
    for (round, &case) in cases.iter().enumerate() {
        prepared.push(if case.phase.is_fresh() {
            None
        } else {
            Some(create(&dsql, case, round).await)
        });
    }
    wait_for_the_clock().await;

    // What each row left behind, checked once the clock has settled every pending
    // transition.
    let mut settled_checks: Vec<(Case, String)> = Vec::with_capacity(cases.len());

    for (round, &case) in cases.iter().enumerate() {
        let name = label(case);
        let identifier = match &prepared[round] {
            Some(identifier) => identifier.clone(),
            // Created here, so the assertions below run inside the span the clock
            // owes this cluster's `CREATING -> ACTIVE` transition.
            None => create(&dsql, case, round).await,
        };

        // Drive a settled cluster into the transient status the row is about.
        match case.phase {
            Phase::Updating => {
                let driven = dsql
                    .update(&identifier, json!({"deletionProtectionEnabled": true}))
                    .await;
                assert_eq!(
                    driven.status, 200,
                    "case {name}: UpdateCluster must start the transition, got {} {:?}",
                    driven.status, driven.body
                );
            }
            Phase::Deleting => {
                let driven = dsql.delete(&identifier).await;
                assert_eq!(
                    driven.status, 200,
                    "case {name}: DeleteCluster must start the removal, got {} {:?}",
                    driven.status, driven.body
                );
            }
            Phase::Active | Phase::Creating => {}
        }

        let reported = dsql.get(&identifier).await.str("status");
        assert_eq!(
            reported,
            case.phase.status(),
            "case {name}: the cluster must be in the status the row is about, got {reported:?}"
        );

        // The policy the record holds before either mutation is attempted.
        let before = dsql.get_policy(&identifier).await;
        let version_before = before.str("policyVersion");
        if case.attached {
            assert_eq!(
                before.status, 200,
                "case {name}: the attached policy must be readable, got {} {:?}",
                before.status, before.body
            );
            assert!(
                !version_before.is_empty(),
                "case {name}: GetClusterPolicy must report a policyVersion, got {:?}",
                before.body
            );
        } else {
            assert_eq!(
                before.status, 404,
                "case {name}: a cluster with no policy must answer 404, got {} {:?}",
                before.status, before.body
            );
        }

        // Detaching: the mutation this suite was written for.
        let detached = dsql.delete_policy(&identifier, "").await;
        if case.phase.accepts_mutation() {
            if case.attached {
                assert_eq!(
                    detached.status, 200,
                    "case {name}: DeleteClusterPolicy must be served, got {} {:?}",
                    detached.status, detached.body
                );
                assert_eq!(
                    detached.str("policyVersion"),
                    version_before,
                    "case {name}: DeleteClusterPolicy must answer the version it removed"
                );
            } else {
                assert_eq!(
                    detached.status, 404,
                    "case {name}: with nothing attached DeleteClusterPolicy must answer 404, \
                     got {} {:?}",
                    detached.status, detached.body
                );
                assert_eq!(
                    detached.error.as_deref(),
                    Some("ResourceNotFoundException"),
                    "case {name}: the nothing-attached refusal must stay a \
                     ResourceNotFoundException, got {:?}",
                    detached.error
                );
            }
        } else {
            // Including the row with nothing attached: the status refuses the call
            // before the state of the policy itself is read, so this is the 409 and
            // not the nothing-attached 404.
            assert_refused(case, "DeleteClusterPolicy", &detached, &identifier);
        }

        // Attaching: the sibling mutation of the same field, at the same instant.
        let put = dsql
            .put_policy(&identifier, json!({"policy": benign_document(round + 1)}))
            .await;
        if case.phase.accepts_mutation() {
            assert_eq!(
                put.status, 200,
                "case {name}: PutClusterPolicy must be served, got {} {:?}",
                put.status, put.body
            );
            assert!(
                !put.str("policyVersion").is_empty(),
                "case {name}: PutClusterPolicy must answer a policyVersion, got {:?}",
                put.body
            );
            assert_ne!(
                put.str("policyVersion"),
                version_before,
                "case {name}: a fresh attachment must not reuse the version it replaced"
            );
        } else {
            assert_refused(case, "PutClusterPolicy", &put, &identifier);
        }

        // The record read back: a refused mutation changed nothing.
        let after = dsql.get_policy(&identifier).await;
        if case.phase.accepts_mutation() {
            assert_eq!(
                after.status, 200,
                "case {name}: the policy just attached must be readable, got {} {:?}",
                after.status, after.body
            );
            assert_eq!(
                after.str("policyVersion"),
                put.str("policyVersion"),
                "case {name}: GetClusterPolicy must report the version PutClusterPolicy issued"
            );
        } else if case.attached {
            assert_eq!(
                after.status, 200,
                "case {name}: a refused mutation must leave the policy attached, got {} {:?}",
                after.status, after.body
            );
            assert_eq!(
                after.str("policyVersion"),
                version_before,
                "case {name}: a refused mutation must leave the stored policy as it was"
            );
        } else {
            assert_eq!(
                after.status, 404,
                "case {name}: a refused attachment must leave the cluster without a policy, \
                 got {} {:?}",
                after.status, after.body
            );
        }

        settled_checks.push((case, identifier));
    }

    // The refusal is about the STATUS, and the clock moves it on: a transition the
    // clock owed comes due and the very same attachment is served, while a cluster
    // whose removal settled is gone.
    wait_for_the_clock().await;
    for (case, identifier) in settled_checks {
        let name = label(case);
        let again = dsql
            .put_policy(&identifier, json!({"policy": benign_document(0)}))
            .await;
        if case.phase == Phase::Deleting {
            assert_eq!(
                again.status, 404,
                "case {name}: once the removal settled the cluster is gone, got {} {:?}",
                again.status, again.body
            );
            assert_eq!(
                again.error.as_deref(),
                Some("ResourceNotFoundException"),
                "case {name}: a retired cluster must answer ResourceNotFoundException, got {:?}",
                again.error
            );
        } else {
            assert_eq!(
                again.status, 200,
                "case {name}: once the cluster is ACTIVE the same attachment must be served, \
                 got {} {:?}",
                again.status, again.body
            );
        }
    }
}
