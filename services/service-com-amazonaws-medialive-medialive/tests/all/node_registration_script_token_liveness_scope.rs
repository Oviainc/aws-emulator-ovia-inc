// SPDX-License-Identifier: CC0-1.0

//! WHICH resources the replay a spent `CreateNodeRegistrationScript`
//! `RequestId` answers with is resolved against — end to end over the restJson1
//! wire.
//!
//! RULE: the replay a spent idempotency token answers with resolves exactly the
//! resources the operation's own write resolves, through the same liveness, and
//! NO OTHERS — so a retry is refused exactly as a first attempt is once the
//! delete of a resource the artefact was issued for has been accepted, and a
//! delete accepted anywhere else in the account leaves the replay answering with
//! the script it issued.
//!
//! This suite is the COMPLEMENT of
//! `shared_token_replay_parent_liveness_rule::a_registration_script_retry_for_a_resource_whose_delete_was_accepted_is_refused`,
//! which pins the refusing half of that rule for each of the two resources a
//! script is issued for (the cluster it registers nodes in, and the node a
//! re-registration script names) over both halves of their delete lifecycles. No
//! row is repeated here. What these rows pin is the rule's SCOPE: the resolution
//! must not be WIDENED either, so a delete accepted elsewhere in the account —
//! another cluster, a node no script here was issued for — cannot quietly void a
//! token, and the one boundary row the sibling suite cannot carry (both the
//! cluster and the node the script was issued for gone at once) names the
//! cluster, which is the resolution the write itself performs first.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour rather than an internal function's return value.
//!
//! The suite is a TABLE: one row is one create-delete-retry sequence.

use serde_json::json;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::node_lifecycle::{cluster, created as created_node, delete_node, node_id};
use crate::node_registration_script::issued;
use crate::shared_token_replay_parent_liveness_rule::{
    create_script, delete_cluster, refusal_of, Refusal, NEVER_SPENT, TOKEN,
};
use crate::wire::medialive;

/// What the script the row's token spent was issued FOR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    /// A script for hardware units that have no node yet: it is issued for the
    /// cluster alone.
    ForTheCluster,
    /// A re-registration script, which the request's `Id` member makes: it is
    /// issued for the cluster AND for one node of it.
    ReRegisteringANode,
}

/// Whose delete the account has accepted before the row's retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deleted {
    /// Nobody's — the control rows, where the token is a plain replay.
    Nothing,
    /// Another cluster of the same account, which no script here was issued for.
    AnotherClusterOfTheAccount,
    /// A node of the very cluster the script registers nodes in, which the
    /// script was not issued for.
    AnotherNodeOfThisCluster,
    /// A node of the other cluster.
    ANodeOfAnotherCluster,
    /// Both resources the re-registration script was issued for: its node, and
    /// then the cluster the node's deregistration leaves idle enough to delete.
    TheClusterAndItsNode,
}

/// What the row expects the retry to be answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The script the first attempt issued, byte for byte.
    TheScriptItIssued,
    /// The modelled 400 naming the cluster the script registers nodes in.
    RefusedForTheCluster,
}

/// Rule: the replay a spent `RequestId` answers with is resolved against the
/// cluster the script registers nodes in and, for a re-registration, the node it
/// names — the same resolutions the write itself performs — and against NOTHING
/// ELSE. A retry sent after a delete accepted elsewhere in the account still
/// answers with the script it issued; one sent once the delete of what the script
/// was issued for has been accepted is refused with the modelled 400
/// `BadRequestException` naming that resource, exactly as a first attempt
/// carrying a token nothing ever spent is.
///
/// The rows differ along what the rule generalises over: WHOSE delete the account
/// accepted (nobody's, another cluster's, a node of this cluster, a node of the
/// other cluster, both resources the script was issued for), which of the two
/// shapes the script has (issued for the cluster alone, or for a node of it as
/// well), and which half of the delete lifecycle the retry lands in (the
/// transient window, or the terminal state the clock settles into).
#[tokio::test]
async fn a_spent_token_is_resolved_against_what_its_script_was_issued_for_and_nothing_else() {
    // (case, what the script was issued for, whose delete was accepted, whether
    //  the clock settles that delete, what the retry must be answered with)
    let rows: [(&str, Script, Deleted, bool, Outcome); 8] = [
        (
            "no delete at all, a script issued for the cluster",
            Script::ForTheCluster,
            Deleted::Nothing,
            false,
            Outcome::TheScriptItIssued,
        ),
        (
            "no delete at all, a re-registration script",
            Script::ReRegisteringANode,
            Deleted::Nothing,
            false,
            Outcome::TheScriptItIssued,
        ),
        (
            "another cluster of the account, inside its deleting window",
            Script::ForTheCluster,
            Deleted::AnotherClusterOfTheAccount,
            false,
            Outcome::TheScriptItIssued,
        ),
        (
            "another cluster of the account, once its delete settled",
            Script::ReRegisteringANode,
            Deleted::AnotherClusterOfTheAccount,
            true,
            Outcome::TheScriptItIssued,
        ),
        (
            "a node of this cluster the script was not issued for, inside its window",
            Script::ForTheCluster,
            Deleted::AnotherNodeOfThisCluster,
            false,
            Outcome::TheScriptItIssued,
        ),
        (
            "a node of this cluster the re-registration does not name, once settled",
            Script::ReRegisteringANode,
            Deleted::AnotherNodeOfThisCluster,
            true,
            Outcome::TheScriptItIssued,
        ),
        (
            "a node of the other cluster, once its deregistration settled",
            Script::ReRegisteringANode,
            Deleted::ANodeOfAnotherCluster,
            true,
            Outcome::TheScriptItIssued,
        ),
        (
            "both the cluster and the node the script re-registers",
            Script::ReRegisteringANode,
            Deleted::TheClusterAndItsNode,
            false,
            Outcome::RefusedForTheCluster,
        ),
    ];

    for (case, script, deleted, wait, outcome) in rows {
        let router = medialive();
        let cluster_id = cluster(&router, "scoped").await;
        let elsewhere = cluster(&router, "scoped-elsewhere").await;

        // The node a re-registration script is issued for, which only the rows
        // that ask for one create.
        let own_node = match script {
            Script::ReRegisteringANode => {
                let node =
                    created_node(&router, case, &cluster_id, json!({ "name": "unit-own" })).await;
                Some(node_id(case, &node))
            }
            Script::ForTheCluster => None,
        };
        // The node a row takes away without touching what the script was issued
        // for: either another node of this cluster, or one of the other cluster.
        let bystander = match deleted {
            Deleted::AnotherNodeOfThisCluster => {
                let node =
                    created_node(&router, case, &cluster_id, json!({ "name": "unit-extra" })).await;
                Some((cluster_id.clone(), node_id(case, &node)))
            }
            Deleted::ANodeOfAnotherCluster => {
                let node =
                    created_node(&router, case, &elsewhere, json!({ "name": "unit-elsewhere" }))
                        .await;
                Some((elsewhere.clone(), node_id(case, &node)))
            }
            Deleted::Nothing
            | Deleted::AnotherClusterOfTheAccount
            | Deleted::TheClusterAndItsNode => None,
        };

        let body = |token: &str| match own_node.as_deref() {
            Some(node) => json!({ "requestId": token, "id": node, "name": "anywhere-$hn" }),
            None => json!({ "requestId": token, "name": "anywhere-$hn" }),
        };

        // The script the row's token spends.
        let first = issued(&router, case, &cluster_id, body(TOKEN)).await;

        match deleted {
            Deleted::Nothing => {}
            Deleted::AnotherClusterOfTheAccount => {
                delete_cluster(&router, case, &elsewhere).await;
            }
            Deleted::AnotherNodeOfThisCluster | Deleted::ANodeOfAnotherCluster => {
                let (holder, node) = bystander
                    .as_ref()
                    .unwrap_or_else(|| panic!("case {case:?}: the row creates its bystander node"));
                let (status, deleting) = delete_node(&router, holder, node).await;
                assert_eq!(
                    status, 202,
                    "case {case:?}: DeleteNode({holder}/{node}): expected 202, got {status}: \
                     {deleting}"
                );
            }
            Deleted::TheClusterAndItsNode => {
                let node = own_node
                    .as_deref()
                    .unwrap_or_else(|| panic!("case {case:?}: the row re-registers a node"));
                let (status, deleting) = delete_node(&router, &cluster_id, node).await;
                assert_eq!(
                    status, 202,
                    "case {case:?}: DeleteNode({cluster_id}/{node}): expected 202, got {status}: \
                     {deleting}"
                );
                // "The Cluster must be idle": the node's deregistration is let
                // come due, so the cluster it was in holds nothing any more.
                wait_for_the_clock();
                delete_cluster(&router, case, &cluster_id).await;
            }
        }
        if wait {
            wait_for_the_clock();
        }

        match outcome {
            Outcome::TheScriptItIssued => {
                // A token this account has never spent still issues a script of
                // its own, so the replay below is the script the row's own token
                // issued rather than the newest one the cluster handed out.
                let control = issued(&router, case, &cluster_id, body(NEVER_SPENT)).await;
                assert_ne!(
                    control, first,
                    "case {case:?}: expected an unspent token to issue a script of its own"
                );
                let replayed = issued(&router, case, &cluster_id, body(TOKEN)).await;
                assert_eq!(
                    replayed, first,
                    "case {case:?}: expected the retry to answer with the script it issued, got \
                     {replayed:?}"
                );
            }
            Outcome::RefusedForTheCluster => {
                // The cluster is resolved before the node a re-registration
                // names, so when both are gone the refusal names the cluster.
                let expected = Refusal {
                    status: 400,
                    code: "BadRequestException".to_string(),
                    message: format!("Cluster {cluster_id} not found"),
                };

                // The first attempt: the same request, carrying a token this
                // account has never spent.
                let (status, control) = create_script(&router, &cluster_id, body(NEVER_SPENT)).await;
                assert!(
                    control.get("nodeRegistrationScript").is_none(),
                    "case {case:?}: expected no script on a refusal, got {control}"
                );
                let control = refusal_of(status, &control);
                assert_eq!(
                    control, expected,
                    "case {case:?}: expected the modelled 400 naming the cluster that is gone, \
                     got {control:?}"
                );

                let (status, refused) = create_script(&router, &cluster_id, body(TOKEN)).await;
                assert!(
                    refused.get("nodeRegistrationScript").is_none(),
                    "case {case:?}: expected the retry to hand out no script, got {refused}"
                );
                let refused = refusal_of(status, &refused);
                assert_eq!(
                    refused, control,
                    "case {case:?}: expected the retry to be refused exactly as a first attempt \
                     is, got {refused:?}"
                );
            }
        }
    }
}
