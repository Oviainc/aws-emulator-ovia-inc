// SPDX-License-Identifier: CC0-1.0

//! `DescribeNode` as a REPORTING operation, end to end over the restJson1 wire.
//!
//! The rule this suite pins: `DescribeNode` answers the node the store holds for
//! the `(ClusterId, NodeId)` pair its path names — the one
//! `DescribeNodeSummary` projection every node writer and `ListNodes` answer
//! from, whichever operation wrote the node last — and it writes nothing of its
//! own, so any number of reads leaves the node, the other nodes of its cluster,
//! the cluster's listing and the id the next `CreateNode` mints exactly as they
//! were.
//!
//! The lifecycle suite beside this one asserts what each WRITER stores; this one
//! asserts that the READER reports that stored node and only reports it. The
//! rows span the writer that last touched the node (the create alone, each value
//! `UpdateNodeState` accepts, a deregistration inside its window and one the
//! clock has settled) and the shape of the node itself, because that is what the
//! rule generalises over.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::state::node_state;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
// The node harness is the lifecycle suite's and the listing walk is the listing
// suite's, so a describe and a listing here reach the emulator exactly as they
// do there — and `MEMBERS_BESIDE_STATE` is the one list of the members no
// writer in either suite is asked to change.
use crate::node_lifecycle::{
    cluster, created, delete_node, described, mapping, node_id, state_updated, INTERFACE_MODES,
    MEMBERS_BESIDE_STATE, ROLES,
};
use crate::node_listing::{list, summaries};
use crate::wire::medialive;

/// How many reads a row takes of the same node: more than one, so "reading
/// changes nothing" is asserted over repeated reads rather than a single one.
const READS: usize = 4;

/// The operation that last wrote the node a row reads back.
#[derive(Debug, Clone, Copy)]
enum LastWriter {
    /// Nothing but the create itself.
    Create,
    /// `UpdateNodeState` asking for one of the values its `State` member accepts.
    RequestedState(&'static str),
    /// `DeleteNode`, read either inside the deregistration window the clock owes
    /// or once it has come due.
    Deregistration { settled: bool },
}

impl LastWriter {
    /// The `NodeState` the node is in by the time the row reads it.
    fn expected_state(self) -> &'static str {
        match self {
            // `smithy_query(query="shape", shape="NodeState")`: a created node
            // starts in `CREATED`.
            Self::Create => node_state::CREATED,
            Self::RequestedState(state) => state,
            Self::Deregistration { settled: false } => node_state::DEREGISTERING,
            Self::Deregistration { settled: true } => node_state::DEREGISTERED,
        }
    }
}

/// The node summary `ListNodes` reports for `node` in `cluster_id`, or `None`
/// when the listing does not report it at all.
async fn listed(
    router: &MultiServiceRouter,
    case: &str,
    cluster_id: &str,
    node: &str,
) -> Option<Value> {
    let (status, body) = list(router, cluster_id, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListNodes: expected 200, got {status}: {body}"
    );
    summaries(case, &body)
        .into_iter()
        .find(|summary| summary["id"] == json!(node))
}

/// A node id read as the number it is, so the row can assert what the NEXT
/// create mints (`CreateNode` mints MediaLive ids as a numeric string).
fn id_number(case: &str, id: &str) -> u64 {
    id.parse()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a numeric node id, got {id:?}"))
}

/// The node bodies the rows are built over: the bare create, and creates
/// carrying each of the members `CreateNode` accepts, spanning the `NodeRole`
/// enum and the `NetworkInterfaceMode` enum ([`ROLES`], [`INTERFACE_MODES`]).
fn node_bodies() -> Vec<(&'static str, Value)> {
    vec![
        ("a bare node", json!({})),
        (
            "a named ACTIVE node with one mapping",
            json!({
                "name": "reported",
                "role": ROLES[0],
                "nodeInterfaceMappings": [mapping("my-inputs", "eth1", INTERFACE_MODES[0])],
            }),
        ),
        (
            "a named BACKUP node with two mappings",
            json!({
                "name": "reported-spare",
                "role": ROLES[1],
                "nodeInterfaceMappings": [
                    mapping("my-inputs", "eth1", INTERFACE_MODES[0]),
                    mapping("my-outputs", "ENO1234EXAMPLE", INTERFACE_MODES[1]),
                ],
            }),
        ),
    ]
}

/// Rule: `DescribeNode` reports the stored node of the `(ClusterId, NodeId)`
/// pair its path names, in the state whichever operation wrote it last left it
/// and with every other member as the create stored it, agreeing member for
/// member with the summary `ListNodes` reports for the same node; and it is a
/// READ — repeated describes report the same node, leave the other nodes of the
/// cluster alone and consume no node id, so the next create mints the id that
/// was next before them.
#[tokio::test]
async fn describe_node_reports_the_stored_node_and_writes_nothing() {
    let writers = [
        LastWriter::Create,
        // The two values `UpdateNodeStateShape` declares
        // (`smithy_query(query="shape", shape="UpdateNodeStateShape")`).
        LastWriter::RequestedState(node_state::ACTIVE),
        LastWriter::RequestedState(node_state::DRAINING),
        LastWriter::Deregistration { settled: false },
        LastWriter::Deregistration { settled: true },
    ];

    for writer in writers {
        for (shape, body) in node_bodies() {
            let case = format!("{shape} after {writer:?}");
            let router = medialive();
            // Two clusters, so what the reader answers is the node of the
            // cluster the path names rather than the only one that exists.
            let cluster_id = cluster(&router, "reporting-cluster").await;
            let elsewhere = cluster(&router, "other-reporting-cluster").await;

            let node = created(&router, &case, &cluster_id, body.clone()).await;
            let id = node_id(&case, &node);
            // A bystander, so "reading changes nothing" covers the rest of the
            // cluster and not only the node that was read.
            let bystander =
                created(&router, &case, &cluster_id, json!({ "name": "bystander" })).await;
            let bystander_id = node_id(&case, &bystander);

            let written = match writer {
                LastWriter::Create => node.clone(),
                LastWriter::RequestedState(state) => {
                    state_updated(&router, &case, &cluster_id, &id, Some(state)).await
                }
                LastWriter::Deregistration { settled } => {
                    let (status, accepted) = delete_node(&router, &cluster_id, &id).await;
                    assert_eq!(
                        status, 202,
                        "case {case:?}: DeleteNode: expected 202, got {status}: {accepted}"
                    );
                    if settled {
                        wait_for_the_clock();
                    }
                    accepted
                }
            };

            let first = described(&router, &case, &cluster_id, &id).await;
            assert_eq!(
                first["state"],
                json!(writer.expected_state()),
                "case {case:?}: expected the reader to report state {:?}, got {:?}",
                writer.expected_state(),
                first["state"]
            );
            // Every member but the state is the one the create stored: a write
            // the caller did not ask for moves nothing else, and the reader
            // invents nothing.
            for member in MEMBERS_BESIDE_STATE {
                assert_eq!(
                    first[member], node[member],
                    "case {case:?}: expected {member:?} as the create stored it, got {:?}",
                    first[member]
                );
            }
            // The writer's own response and the read that follows it are the
            // same projection of the same record, so they cannot disagree about
            // the members the clock has not moved since.
            for member in MEMBERS_BESIDE_STATE {
                assert_eq!(
                    first[member], written[member],
                    "case {case:?}: expected the reader to agree with the writer at {member:?}, \
                     got {:?} against {:?}",
                    first[member], written[member]
                );
            }

            // A node is reported through its OWN cluster: the pair the path
            // names is what resolves it, so the other cluster's collection
            // holds nothing of it.
            assert_eq!(
                first["clusterId"],
                json!(cluster_id),
                "case {case:?}: expected the node of the cluster the path named, got {:?}",
                first["clusterId"]
            );
            assert_eq!(
                listed(&router, &case, &elsewhere, &id).await,
                None,
                "case {case:?}: expected cluster {elsewhere} to report nothing of node {id}"
            );

            // The reader and the listing answer from one projection.
            let summary = listed(&router, &case, &cluster_id, &id)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: ListNodes did not report node {id}"));
            assert_eq!(
                summary, first,
                "case {case:?}: expected the listed summary to equal the described node, got {summary}"
            );

            // Reading is a READ: repeated describes report the same node, and
            // nothing else of the cluster moves.
            for read in 1..=READS {
                let again = described(&router, &case, &cluster_id, &id).await;
                assert_eq!(
                    again, first,
                    "case {case:?}: read {read}: expected the same node, got {again}"
                );
            }
            assert_eq!(
                described(&router, &case, &cluster_id, &bystander_id).await,
                bystander,
                "case {case:?}: expected the bystander node {bystander_id} to be untouched"
            );
            assert_eq!(
                listed(&router, &case, &cluster_id, &id).await.as_ref(),
                Some(&first),
                "case {case:?}: expected the listing to be unchanged by the reads"
            );

            // And the reads consumed no node id: the next create mints the one
            // that was next before them.
            let next = created(&router, &case, &cluster_id, json!({ "name": "after-the-reads" }))
                .await;
            let next_id = node_id(&case, &next);
            assert_eq!(
                id_number(&case, &next_id),
                id_number(&case, &bystander_id) + 1,
                "case {case:?}: expected the create after {READS} reads to mint the id after \
                 {bystander_id}, got {next_id}"
            );
        }
    }
}
