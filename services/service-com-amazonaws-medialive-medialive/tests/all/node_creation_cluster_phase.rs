// SPDX-License-Identifier: CC0-1.0

//! `CreateNode` against the LIFECYCLE PHASE of the cluster it is addressed to,
//! end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The node lifecycle suite beside this one spans the members a create carries;
//! this one spans the CLOCK: the phase the cluster is in when the create arrives
//! (a cluster still coming up, and one whose coming-up has come due) and the
//! phase the node is read back in. A cluster the account holds takes a node in
//! either phase, and the node it mints is terminal here.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::{cluster_state, node_state};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::node_lifecycle::{
    cluster, created, described, mapping, node_id, INTERFACE_MODES, ROLES,
};
use crate::wire::{call, medialive};

/// The `State` one cluster reports, read through `DescribeCluster`
/// (`smithy_query(query="operation", operation="DescribeCluster")`: `GET
/// /prod/clusters/{ClusterId}`).
async fn cluster_phase(router: &MultiServiceRouter, case: &str, cluster_id: &str) -> String {
    let (status, body) = call(router, "GET", &format!("/prod/clusters/{cluster_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeCluster({cluster_id}): expected 200, got {status}: {body}"
    );
    body["state"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the cluster reported no state: {body}"))
        .to_string()
}

/// One row: the case name, whether the clock is let settle the cluster before
/// the create, the cluster `State` the create is therefore addressed in, and the
/// request body.
struct Row {
    case: &'static str,
    settle_cluster_first: bool,
    phase: &'static str,
    body: fn() -> Value,
}

/// Rule: a node is created in any cluster the account HOLDS — the state that
/// cluster is in is not a precondition of the create, since it is the node that
/// has to register before either of them carries any media — and the node the
/// create mints starts in the state a create answers with and owes the clock
/// nothing, while the cluster's own clock-owed transition runs on untouched.
///
/// The rows differ along the cluster phase the create is addressed in — before
/// the transition the clock owes the cluster has come due and after it — and
/// along which optional members the create carries, because that is what the
/// rule generalises over; each row is then read back in both phases of the
/// node's own reading, before and after the clock.
#[tokio::test]
async fn a_cluster_takes_a_node_in_every_phase_of_its_own_lifecycle() {
    let rows: [Row; 4] = [
        Row {
            case: "a cluster still coming up, a create naming nothing",
            settle_cluster_first: false,
            phase: cluster_state::CREATING,
            body: || json!({}),
        },
        Row {
            case: "a cluster still coming up, a create naming every member",
            settle_cluster_first: false,
            phase: cluster_state::CREATING,
            body: || {
                json!({
                    "name": "coming-up",
                    "role": ROLES[1],
                    "nodeInterfaceMappings": [mapping("my-inputs", "eth0", INTERFACE_MODES[0])],
                })
            },
        },
        Row {
            case: "a cluster whose coming-up came due, a create naming nothing",
            settle_cluster_first: true,
            phase: cluster_state::ACTIVE,
            body: || json!({}),
        },
        Row {
            case: "a cluster whose coming-up came due, a create naming every member",
            settle_cluster_first: true,
            phase: cluster_state::ACTIVE,
            body: || {
                json!({
                    "name": "settled",
                    "role": ROLES[0],
                    "nodeInterfaceMappings": [
                        mapping("my-inputs", "eth0", INTERFACE_MODES[0]),
                        mapping("my-outputs", "eth1", INTERFACE_MODES[1]),
                    ],
                })
            },
        },
    ];

    for Row {
        case,
        settle_cluster_first,
        phase,
        body,
    } in rows
    {
        let router = medialive();
        let cluster_id = cluster(&router, "phase-cluster").await;
        if settle_cluster_first {
            wait_for_the_clock();
        }
        // The row is only a row if the cluster really is in the phase it claims.
        let before = cluster_phase(&router, case, &cluster_id).await;
        assert_eq!(
            before, phase,
            "case {case:?}: expected the create to be addressed to a {phase} cluster, got \
             {before:?}"
        );

        let request = body();
        let node = created(&router, case, &cluster_id, request.clone()).await;
        let id = node_id(case, &node);
        assert_eq!(
            node["clusterId"],
            json!(cluster_id),
            "case {case:?}: expected the cluster the create was addressed to, got {:?}",
            node["clusterId"]
        );
        assert_eq!(
            node["state"],
            json!(node_state::CREATED),
            "case {case:?}: expected a created node in {}, got {:?}",
            node_state::CREATED,
            node["state"]
        );
        // The members the request supplied are stored verbatim whatever phase
        // the cluster was in, so the phase changes nothing about the node.
        for supplied in ["name", "role"] {
            assert_eq!(
                node.get(supplied).cloned().unwrap_or(Value::Null),
                request.get(supplied).cloned().unwrap_or(Value::Null),
                "case {case:?}: expected the {supplied} the request supplied, got {:?}",
                node.get(supplied)
            );
        }
        assert_eq!(
            node["nodeInterfaceMappings"],
            request
                .get("nodeInterfaceMappings")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "case {case:?}: expected the requested interface mappings, got {:?}",
            node["nodeInterfaceMappings"]
        );

        // The create WROTE: the reader answers with that stored node.
        assert_eq!(
            described(&router, case, &cluster_id, &id).await,
            node,
            "case {case:?}: expected DescribeNode to report the created node"
        );

        wait_for_the_clock();

        // A created node owes the clock nothing — what moves it on is the
        // registration script run on its own hardware — so every member it
        // reported survives the instant at which a transient state would have
        // come due.
        assert_eq!(
            described(&router, case, &cluster_id, &id).await,
            node,
            "case {case:?}: expected the node to be unchanged once the clock had run on"
        );
        // ... and the cluster's own transition came due independently of the
        // node that was created in it.
        let after = cluster_phase(&router, case, &cluster_id).await;
        assert_eq!(
            after,
            cluster_state::ACTIVE,
            "case {case:?}: expected the cluster to report {}, got {after:?}",
            cluster_state::ACTIVE
        );
    }
}
