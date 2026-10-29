// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateCluster` that rewrites a peer list leaves behind on the clusters
//! the new list no longer names, over the wire.
//!
//! RULE under test: `multiRegionProperties.clusters` is the SET of peered clusters
//! forming the configuration (`smithy_query(query="shape",
//! shape="MultiRegionProperties")`), and an update REPLACES it, so every current peer
//! the new list omits is a peering the call removes — the member `UpdateCluster`
//! documents `dsql:RemovePeerCluster` for ("you need dsql:RemovePeerCluster only for
//! the peer clusters that the update removes", `smithy_query(query="doc",
//! shape="UpdateCluster")`). A peering is a relationship BOTH clusters carry, so a
//! removed peer stops naming this cluster too, keeps the peerings it still has, and
//! has its status recomputed from the configuration the removal leaves it with; the
//! peers the new list KEEPS are untouched.
//!
//! So no read reports a peer that is no longer peered back, and no later call on a
//! removed peer recomputes its status from a peering this update dismantled — the
//! same end state the removal reaches through `DeleteCluster`
//! (`tests/all/peering_unwound_by_deletion.rs`).
//!
//! The table spans the class rather than the one reported pair: the new configuration
//! drops one peer of two, drops every peer, carries no peer list at all, keeps every
//! peer, or is not carried at all; the dropped peer has a peering of its own to keep
//! or none, and names this cluster back or never did; and the list that remains names
//! a peer this store does not hold.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs` — this crate's one test client.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::{
    STATUS_ACTIVE, STATUS_PENDING_SETUP, STATUS_UPDATING,
};

use crate::harness::{wait_for_the_clock, Dsql};
use crate::multi_region_cluster_status::{FIRST_REGION, SECOND_REGION, WITNESS_REGION};
use crate::peering_unwound_by_deletion::{absent_peer_arn, reported_peers, THIRD_REGION};

/// One rewritten peer list, and what the clusters it stops naming must be left with.
///
/// The fixture is always the same three clusters, each in its own Region: `A` names
/// `B` and `C`, `C` names `A`, and the flags say what else is configured. The update
/// under test rewrites `A`'s list.
struct RemovalCase {
    name: &'static str,
    /// Whether `B` names `A` back, so the peering between them is established on
    /// both sides rather than only on `A`'s.
    b_names_a: bool,
    /// Whether `B` and `C` are also peered with each OTHER — a relationship this
    /// update must leave alone.
    b_peers_c: bool,
    /// Whether the update carries a `multiRegionProperties` at all; a request that
    /// carries none rewrites no list, so it removes no peer.
    carries_configuration: bool,
    /// Whether the rewritten list still names `B`.
    keeps_b: bool,
    /// Whether the rewritten list still names `C`.
    keeps_c: bool,
    /// Whether the rewritten list also names a peer ARN this store does not hold.
    keeps_absent: bool,
    /// Whether the configuration the update carries OMITS the `clusters` member
    /// altogether rather than carrying the rewritten list: a configuration that
    /// carries no peer list names no peer, so every current peer is one it omits.
    omits_clusters_member: bool,
}

fn removal_cases() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            name: "one peer dropped from a list that keeps the other",
            b_names_a: true,
            b_peers_c: false,
            carries_configuration: true,
            keeps_b: false,
            keeps_c: true,
            keeps_absent: false,
            omits_clusters_member: false,
        },
        RemovalCase {
            name: "every peer dropped, the list left naming none",
            b_names_a: true,
            b_peers_c: false,
            carries_configuration: true,
            keeps_b: false,
            keeps_c: false,
            keeps_absent: false,
            omits_clusters_member: false,
        },
        RemovalCase {
            name: "a configuration carrying no peer list at all",
            b_names_a: true,
            b_peers_c: false,
            carries_configuration: true,
            keeps_b: false,
            keeps_c: false,
            keeps_absent: false,
            omits_clusters_member: true,
        },
        RemovalCase {
            name: "a dropped peer that keeps a peering of its own",
            b_names_a: true,
            b_peers_c: true,
            carries_configuration: true,
            keeps_b: false,
            keeps_c: true,
            keeps_absent: false,
            omits_clusters_member: false,
        },
        RemovalCase {
            name: "a dropped peer that never named this cluster back",
            b_names_a: false,
            b_peers_c: false,
            carries_configuration: true,
            keeps_b: false,
            keeps_c: true,
            keeps_absent: false,
            omits_clusters_member: false,
        },
        RemovalCase {
            name: "a dropped peer beside a kept peer this store does not hold",
            b_names_a: true,
            b_peers_c: false,
            carries_configuration: true,
            keeps_b: false,
            keeps_c: true,
            keeps_absent: true,
            omits_clusters_member: false,
        },
        RemovalCase {
            name: "a list that keeps every peer removes none",
            b_names_a: true,
            b_peers_c: false,
            carries_configuration: true,
            keeps_b: true,
            keeps_c: true,
            keeps_absent: false,
            omits_clusters_member: false,
        },
        RemovalCase {
            name: "an update carrying no configuration rewrites no list",
            b_names_a: true,
            b_peers_c: false,
            carries_configuration: false,
            keeps_b: true,
            keeps_c: true,
            keeps_absent: false,
            omits_clusters_member: false,
        },
    ]
}

/// One cluster of the fixture: the client signed for its Region, its identifier and
/// ARN, and the peers it names.
struct Peer {
    client: Dsql,
    identifier: String,
    arn: String,
    peers: Vec<String>,
}

/// The status the RULE says a cluster settles into, stated here independently of the
/// service: a cluster with no multi-Region configuration is usable, and one that has
/// a configuration is usable only once every peer it names is a cluster this store
/// holds whose own list names it back — otherwise it waits in `PENDING_SETUP`.
fn settles_to(arn: &str, peers: &[String], multi_region: bool, topology: &Topology) -> &'static str {
    let mutual = !peers.is_empty()
        && peers.iter().all(|peer| {
            topology
                .get(peer)
                .is_some_and(|theirs| theirs.iter().any(|back| back == arn))
        });
    if !multi_region || mutual {
        STATUS_ACTIVE
    } else {
        STATUS_PENDING_SETUP
    }
}

/// Which peers each cluster this store holds names, keyed by ARN — the end state a
/// case expects, which is what [`settles_to`] reads a status off.
type Topology = BTreeMap<String, Vec<String>>;

/// Build `case`'s starting topology the way a client builds one: each cluster is
/// created in its own Region with the witness Region alone, and every peering is
/// then configured from both sides.
async fn fixture(engine: &Arc<engine::InMemoryEngine>, case: &RemovalCase) -> [Peer; 3] {
    let clients = [
        Dsql::sharing(engine, FIRST_REGION),
        Dsql::sharing(engine, SECOND_REGION),
        Dsql::sharing(engine, THIRD_REGION),
    ];
    let mut minted: Vec<(String, String)> = Vec::new();
    for (named, client) in ["A", "B", "C"].into_iter().zip(clients.iter()) {
        let created = client
            .create(json!({
                "deletionProtectionEnabled": false,
                "multiRegionProperties": {"witnessRegion": WITNESS_REGION},
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: cluster {named} must be created, got {} {:?}",
            case.name, created.status, created.body
        );
        minted.push((created.str("identifier"), created.str("arn")));
    }
    // The creations settle before anything is reconfigured: a cluster the clock
    // still owes `CREATING` takes no configuration change.
    wait_for_the_clock().await;

    let (a_id, a_arn) = minted[0].clone();
    let (b_id, b_arn) = minted[1].clone();
    let (c_id, c_arn) = minted[2].clone();

    // A names both the others; what they name back is the case's.
    let mut b_peers = Vec::new();
    if case.b_names_a {
        b_peers.push(a_arn.clone());
    }
    if case.b_peers_c {
        b_peers.push(c_arn.clone());
    }
    let mut c_peers = vec![a_arn.clone()];
    if case.b_peers_c {
        c_peers.push(b_arn.clone());
    }
    let [a_client, b_client, c_client] = clients;
    let fixture = [
        Peer {
            client: a_client,
            identifier: a_id,
            arn: a_arn,
            peers: vec![b_arn.clone(), c_arn.clone()],
        },
        Peer {
            client: b_client,
            identifier: b_id,
            arn: b_arn,
            peers: b_peers,
        },
        Peer {
            client: c_client,
            identifier: c_id,
            arn: c_arn,
            peers: c_peers,
        },
    ];

    // The calls that configure each cluster's side of the peerings the case asks
    // for; a cluster the case leaves naming nobody is configured by none.
    for (named, peer) in ["A", "B", "C"].into_iter().zip(fixture.iter()) {
        if peer.peers.is_empty() {
            continue;
        }
        let configured = peer
            .client
            .update(
                &peer.identifier,
                json!({
                    "multiRegionProperties": {
                        "witnessRegion": WITNESS_REGION,
                        "clusters": peer.peers.clone(),
                    },
                }),
            )
            .await;
        assert_eq!(
            (configured.status, configured.str("status")),
            (200, STATUS_UPDATING.to_string()),
            "case {:?}: configuring cluster {named} must be accepted, got {} {:?}",
            case.name,
            configured.status,
            configured.body
        );
    }
    // Two spans: the configuration changes settle, and a cluster whose peering the
    // LAST of them completed is carried on to `ACTIVE` a span after that.
    wait_for_the_clock().await;
    wait_for_the_clock().await;

    fixture
}

/// The peers and the witness Region a cluster reports, read over the wire.
async fn reported(client: &Dsql, identifier: &str) -> (Vec<String>, Value) {
    let fetched = client.get(identifier).await;
    let properties = fetched.get("multiRegionProperties");
    (reported_peers(&properties), properties)
}

/// RULE: an update that rewrites a cluster's peer list removes the peerings the new
/// list omits — on BOTH sides. Each removed peer stops naming the updated cluster,
/// keeps every other peering it has, loses its whole multi-Region configuration when
/// the removal leaves it naming no peer at all, and settles into the status the
/// configuration it is left with asks for; the peers the list keeps are untouched.
#[tokio::test]
async fn an_update_that_drops_a_peer_unwinds_the_peering_on_the_removed_peer() {
    for case in removal_cases() {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let [a, b, c] = fixture(&engine, &case).await;

        // Where the case starts: every cluster reports the configuration it was
        // given, in the status the rule says that configuration asks for.
        let entering: Topology = [&a, &b, &c]
            .iter()
            .map(|peer| (peer.arn.clone(), peer.peers.clone()))
            .collect();
        for (named, peer) in [("A", &a), ("B", &b), ("C", &c)] {
            let (peers, _) = reported(&peer.client, &peer.identifier).await;
            assert_eq!(
                peers, peer.peers,
                "case {:?}: cluster {named} must enter the update naming {:?}, got {peers:?}",
                case.name, peer.peers
            );
            let expected = settles_to(&peer.arn, &peer.peers, true, &entering);
            let status = peer.client.get(&peer.identifier).await.str("status");
            assert_eq!(
                status, expected,
                "case {:?}: cluster {named} must enter the update {expected:?}, got {status:?}",
                case.name
            );
        }

        // The update under test: the configuration A is rewritten to — with the peer
        // list the case keeps, with no peer list at all, or no configuration at all.
        let mut rewritten: Vec<String> = Vec::new();
        if case.keeps_b {
            rewritten.push(b.arn.clone());
        }
        if case.keeps_c {
            rewritten.push(c.arn.clone());
        }
        if case.keeps_absent {
            rewritten.push(absent_peer_arn());
        }
        let request = match (case.carries_configuration, case.omits_clusters_member) {
            (true, true) => json!({"multiRegionProperties": {"witnessRegion": WITNESS_REGION}}),
            (true, false) => json!({
                "multiRegionProperties": {
                    "witnessRegion": WITNESS_REGION,
                    "clusters": rewritten.clone(),
                },
            }),
            _ => json!({"deletionProtectionEnabled": false}),
        };
        let updated = a.client.update(&a.identifier, request.clone()).await;
        assert_eq!(
            (updated.status, updated.str("status")),
            (200, STATUS_UPDATING.to_string()),
            "case {:?}: the update {request} must be accepted, got {} {:?}",
            case.name,
            updated.status,
            updated.body
        );

        // What each cluster must be left naming: A the list it was given, a removed
        // peer its own list minus A, and a kept peer exactly what it had.
        let a_after = if case.carries_configuration {
            rewritten.clone()
        } else {
            a.peers.clone()
        };
        let mut expected: Topology = BTreeMap::new();
        expected.insert(a.arn.clone(), a_after.clone());
        // A cluster the update UNWOUND is one it dropped and that was naming it
        // back; one that never named A has nothing of A's to take out.
        let mut unwound: Vec<&str> = Vec::new();
        for (named, peer, kept) in [("B", &b, case.keeps_b), ("C", &c, case.keeps_c)] {
            let dropped = case.carries_configuration && !kept;
            let was_mutual = peer.peers.contains(&a.arn);
            let after: Vec<String> = if dropped && was_mutual {
                unwound.push(named);
                peer.peers
                    .iter()
                    .filter(|named_peer| **named_peer != a.arn)
                    .cloned()
                    .collect()
            } else {
                peer.peers.clone()
            };
            expected.insert(peer.arn.clone(), after);
        }

        // The removal takes effect with the call, as the deletion's does: no clock
        // span stands between the update and the removed peer's half going away.
        for (named, peer) in [("A", &a), ("B", &b), ("C", &c)] {
            let wanted = expected.get(&peer.arn).expect("every cluster is expected");
            let (peers, properties) = reported(&peer.client, &peer.identifier).await;
            assert_eq!(
                &peers, wanted,
                "case {:?}: cluster {named} must name exactly {wanted:?} once the update is \
                 through, got {peers:?}",
                case.name
            );
            // A cluster the unwinding left naming no peer is no longer part of a
            // multi-Region cluster, so its whole configuration goes with the
            // peering; every other cluster keeps the witness Region it was given.
            if peers.is_empty() && unwound.contains(&named) {
                assert_eq!(
                    properties,
                    Value::Null,
                    "case {:?}: cluster {named}, left naming no peer, keeps no multi-Region \
                     configuration, got {properties:?}",
                    case.name
                );
            } else {
                assert_eq!(
                    properties.get("witnessRegion").cloned().unwrap_or(Value::Null),
                    json!(WITNESS_REGION),
                    "case {:?}: cluster {named} must keep its witness Region, got {properties:?}",
                    case.name
                );
            }
        }

        // And the status each cluster settles into follows the configuration it is
        // left with — a removed peer is not left waiting for a peering this update
        // dismantled.
        wait_for_the_clock().await;
        wait_for_the_clock().await;
        for (named, peer) in [("A", &a), ("B", &b), ("C", &c)] {
            let wanted = expected.get(&peer.arn).expect("every cluster is expected");
            let multi_region = !wanted.is_empty() || !unwound.contains(&named);
            let expected_status = settles_to(&peer.arn, wanted, multi_region, &expected);
            let status = peer.client.get(&peer.identifier).await.str("status");
            assert_eq!(
                status, expected_status,
                "case {:?}: cluster {named} must settle into {expected_status:?}, got {status:?}",
                case.name
            );
        }
    }
}

/// RULE: a cluster the update removed from a peer list is a cluster like any other,
/// so the next call that recomputes its status — an update carrying nothing about
/// multi-Region at all — settles it back where it was rather than into a wait for a
/// peering that was dismantled.
#[tokio::test]
async fn a_removed_peer_is_not_left_waiting_for_the_dismantled_peering() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let (first, one_id, second, two_id) =
        crate::multi_region_cluster_status::peered_pair(&engine).await;

    // The cluster that drops its peer, and the peer it drops.
    let one_arn = first.get(&one_id).await.str("arn");
    let dropped = first
        .update(
            &one_id,
            json!({"multiRegionProperties": {"witnessRegion": WITNESS_REGION, "clusters": []}}),
        )
        .await;
    assert_eq!(
        (dropped.status, dropped.str("status")),
        (200, STATUS_UPDATING.to_string()),
        "the update dropping the peer must be accepted, got {} {:?}",
        dropped.status,
        dropped.body
    );
    let properties = second.get(&two_id).await.get("multiRegionProperties");
    assert!(
        !reported_peers(&properties).contains(&one_arn),
        "case dropped peer: the removed peer must stop naming the cluster that dropped it, \
         got {properties:?}"
    );

    wait_for_the_clock().await;
    let settled = second.get(&two_id).await.str("status");
    assert_eq!(
        settled, STATUS_ACTIVE,
        "case dropped peer: a cluster whose only peering the update dismantled is usable, \
         got {settled:?}"
    );

    // The call the issue is about: an unrelated mutation recomputes the status, and a
    // dangling half would make it wait for a peering nobody asked for.
    let unrelated = second
        .update(&two_id, json!({"deletionProtectionEnabled": false}))
        .await;
    assert_eq!(
        (unrelated.status, unrelated.str("status")),
        (200, STATUS_UPDATING.to_string()),
        "case unrelated update: the removed peer must accept an ordinary mutation, got {} {:?}",
        unrelated.status,
        unrelated.body
    );
    wait_for_the_clock().await;
    let after = second.get(&two_id).await.str("status");
    assert_eq!(
        after, STATUS_ACTIVE,
        "case unrelated update: a later call must not leave the removed peer waiting for the \
         peering the update dismantled, got {after:?}"
    );
}
