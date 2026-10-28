// SPDX-License-Identifier: CC0-1.0

//! What a deletion leaves behind on the clusters that stay, over the wire.
//!
//! RULE under test: a multi-Region peering is a relationship BOTH clusters carry,
//! so retiring a cluster unwinds the other side of it — every surviving cluster's
//! `multiRegionProperties.clusters` stops naming the retired cluster, a survivor
//! left naming no peer at all keeps no multi-Region configuration (the peer list
//! and the witness Region that held the quorum vote for it go together), and the
//! status a survivor settles into is recomputed from the configuration it is left
//! with: one whose remaining peers all name it back is usable, one that still names
//! a peer which does not is still waiting.
//!
//! So no read reports a peer this service no longer holds, and no later call on a
//! survivor recomputes its status from a reference the deletion took away.
//!
//! The table spans the class rather than the one reported pair: the retired cluster
//! is multi-Region (naming the survivor back) or single-Region (merely named BY the
//! survivor); the survivor is left with no peer, with another live peer that names
//! it back, or with a peer this store does not hold; and it enters the deletion
//! `ACTIVE` or waiting in `PENDING_SETUP`.
//!
//! `multiRegionProperties.clusters` is modelled as the set of peered clusters
//! forming the configuration (`smithy_query(query="shape",
//! shape="MultiRegionProperties")`), and `UpdateCluster` documents removing a peer
//! as a change of the member — "you need dsql:RemovePeerCluster only for the peer
//! clusters that the update removes" (`smithy_query(query="doc",
//! shape="UpdateCluster")`) — so a peer entry names a cluster that is expected to be
//! there.
//!
//! Every case drives the real request path through the `Dsql` harness in
//! `tests/all/harness.rs` — this crate's one test client.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::{
    STATUS_ACTIVE, STATUS_DELETING, STATUS_PENDING_DELETE, STATUS_PENDING_SETUP, STATUS_UPDATING,
};

use crate::cluster_lifecycle::cluster_id_for;
use crate::harness::{expected_arn, wait_for_the_clock, Dsql};
use crate::multi_region_cluster_status::{FIRST_REGION, SECOND_REGION, WITNESS_REGION};

/// The Region the third cluster of a case lives in: a fourth Region, so that no
/// cluster of the fixture names its own Region as the witness Region (the
/// constraint both writing operations document).
const THIRD_REGION: &str = "eu-west-1";

/// A peer ARN naming a cluster this store does not hold, as `ClusterArn`'s pattern
/// requires one (`^arn:aws(-[^:]+)?:dsql:[a-z0-9-]{1,20}:[0-9]{12}:cluster/[a-z0-9]{26}$`).
fn absent_peer_arn() -> String {
    expected_arn(THIRD_REGION, &cluster_id_for(7_700))
}

/// One deletion and the configuration and status it must leave the survivor with.
struct UnwindCase {
    name: &'static str,
    /// Whether the cluster being deleted names the survivor back — a multi-Region
    /// cluster — or is a single-Region cluster the survivor merely names.
    mutual: bool,
    /// Whether the survivor also names a THIRD live cluster, which names it back.
    live_peer: bool,
    /// Whether the survivor also names a peer ARN this store does not hold.
    absent_peer: bool,
    /// The status the survivor reports going into the deletion.
    before: &'static str,
    /// The status the survivor reports once the deletion is through.
    after: &'static str,
}

fn unwind_cases() -> Vec<UnwindCase> {
    vec![
        UnwindCase {
            name: "a peered pair, so the survivor is left with no peer at all",
            mutual: true,
            live_peer: false,
            absent_peer: false,
            before: STATUS_ACTIVE,
            after: STATUS_ACTIVE,
        },
        UnwindCase {
            name: "a survivor peered with a third cluster too",
            mutual: true,
            live_peer: true,
            absent_peer: false,
            before: STATUS_ACTIVE,
            after: STATUS_ACTIVE,
        },
        UnwindCase {
            name: "a single-Region cluster the survivor names",
            mutual: false,
            live_peer: false,
            absent_peer: false,
            before: STATUS_PENDING_SETUP,
            after: STATUS_ACTIVE,
        },
        UnwindCase {
            name: "a survivor also naming a peer this store does not hold",
            mutual: true,
            live_peer: false,
            absent_peer: true,
            before: STATUS_PENDING_SETUP,
            after: STATUS_PENDING_SETUP,
        },
        UnwindCase {
            name: "a single-Region cluster beside a live peer",
            mutual: false,
            live_peer: true,
            absent_peer: false,
            before: STATUS_PENDING_SETUP,
            after: STATUS_ACTIVE,
        },
    ]
}

/// The fixture one case runs against: the client and identifier of the cluster to
/// delete, those of the cluster that survives it, and the peer ARNs the survivor
/// must still name afterwards, in the order it named them.
struct Fixture {
    doomed: Dsql,
    doomed_id: String,
    doomed_arn: String,
    survivor: Dsql,
    survivor_id: String,
    remaining_peers: Vec<String>,
}

/// Build `case`'s topology in one store, the way a client builds it: each cluster
/// is created in its own Region, and a peering is completed by configuring the
/// other side.
async fn fixture(engine: &Arc<engine::InMemoryEngine>, case: &UnwindCase) -> Fixture {
    let doomed = Dsql::sharing(engine, FIRST_REGION);
    let survivor = Dsql::sharing(engine, SECOND_REGION);
    let third = Dsql::sharing(engine, THIRD_REGION);

    // The cluster to be deleted: multi-Region when the case peers it back, a
    // plain single-Region cluster otherwise.
    let mut request = json!({"deletionProtectionEnabled": false});
    if case.mutual {
        request["multiRegionProperties"] = json!({"witnessRegion": WITNESS_REGION});
    }
    let created = doomed.create(request).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: the cluster to delete must be created, got {} {:?}",
        case.name, created.status, created.body
    );
    let (doomed_id, doomed_arn) = (created.str("identifier"), created.str("arn"));

    let mut remaining_peers: Vec<String> = Vec::new();
    let third_cluster = if case.live_peer {
        let created = third
            .create(json!({
                "deletionProtectionEnabled": false,
                "multiRegionProperties": {"witnessRegion": WITNESS_REGION},
            }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the third cluster must be created, got {} {:?}",
            case.name, created.status, created.body
        );
        remaining_peers.push(created.str("arn"));
        Some(created.str("identifier"))
    } else {
        None
    };
    if case.absent_peer {
        remaining_peers.push(absent_peer_arn());
    }

    // The survivor names the cluster to be deleted first, then every peer it keeps.
    let mut peers = vec![doomed_arn.clone()];
    peers.extend(remaining_peers.iter().cloned());
    let created = survivor
        .create(json!({
            "deletionProtectionEnabled": false,
            "multiRegionProperties": {"witnessRegion": WITNESS_REGION, "clusters": peers},
        }))
        .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: the surviving cluster must be created, got {} {:?}",
        case.name, created.status, created.body
    );
    let (survivor_id, survivor_arn) = (created.str("identifier"), created.str("arn"));

    // The creations settle before anything is reconfigured: a cluster the clock
    // still owes `CREATING` takes no configuration change.
    wait_for_the_clock().await;

    // The calls that configure the other side of each peering the case asks for.
    for (name, client, identifier) in [
        ("the cluster to delete", &doomed, case.mutual.then_some(&doomed_id)),
        ("the third cluster", &third, third_cluster.as_ref()),
    ] {
        let Some(identifier) = identifier else {
            continue;
        };
        let configured = client
            .update(
                identifier,
                json!({
                    "multiRegionProperties": {
                        "witnessRegion": WITNESS_REGION,
                        "clusters": [survivor_arn.clone()],
                    },
                }),
            )
            .await;
        assert_eq!(
            (configured.status, configured.str("status")),
            (200, STATUS_UPDATING.to_string()),
            "case {:?}: configuring {name} must be accepted, got {} {:?}",
            case.name,
            configured.status,
            configured.body
        );
    }
    wait_for_the_clock().await;

    Fixture {
        doomed,
        doomed_id,
        doomed_arn,
        survivor,
        survivor_id,
        remaining_peers,
    }
}

/// Wait for the clock until the deleted cluster is gone, which is at most the legs
/// of the deletion chain plus one.
async fn wait_until_gone(case: &str, client: &Dsql, identifier: &str) {
    for _ in 0..4 {
        wait_for_the_clock().await;
        if client.get(identifier).await.status == 404 {
            return;
        }
    }
    panic!("case {case:?}: the clock must remove the deleted cluster {identifier}");
}

/// The `clusters` list a `multiRegionProperties` reports, or an empty list when it
/// reports none.
fn reported_peers(properties: &Value) -> Vec<String> {
    properties
        .get("clusters")
        .and_then(Value::as_array)
        .map(|peers| {
            peers
                .iter()
                .map(|peer| peer.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: retiring a cluster unwinds the other side of every peering it was part of
/// — each survivor stops naming it, a survivor left with no peer keeps no
/// multi-Region configuration, and the survivor's status follows the configuration
/// it is left with.
#[tokio::test]
async fn a_deletion_unwinds_the_peering_on_the_clusters_that_stay() {
    for case in unwind_cases() {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let fixture = fixture(&engine, &case).await;

        let entering = fixture.survivor.get(&fixture.survivor_id).await;
        assert_eq!(
            entering.str("status"),
            case.before,
            "case {:?}: the survivor must enter the deletion {:?}, got {:?}",
            case.name,
            case.before,
            entering.str("status")
        );
        assert!(
            reported_peers(&entering.get("multiRegionProperties")).contains(&fixture.doomed_arn),
            "case {:?}: the survivor must name the cluster to be deleted, got {:?}",
            case.name,
            entering.get("multiRegionProperties")
        );

        let deleted = fixture.doomed.delete(&fixture.doomed_id).await;
        let starts_with = if case.mutual {
            STATUS_PENDING_DELETE
        } else {
            STATUS_DELETING
        };
        assert_eq!(
            (deleted.status, deleted.str("status")),
            (200, starts_with.to_string()),
            "case {:?}: the delete must be accepted and report {starts_with:?}, got {} {:?}",
            case.name,
            deleted.status,
            deleted.body
        );

        // The peering is unwound as the removal starts, so the survivor stops
        // naming the retiring cluster straight away — and keeps every other peer.
        let unwound = fixture.survivor.get(&fixture.survivor_id).await;
        assert_eq!(
            reported_peers(&unwound.get("multiRegionProperties")),
            fixture.remaining_peers,
            "case {:?}: the survivor must name exactly the peers the deletion leaves it, \
             got {:?}",
            case.name,
            unwound.get("multiRegionProperties")
        );

        wait_until_gone(case.name, &fixture.doomed, &fixture.doomed_id).await;

        let settled = fixture.survivor.get(&fixture.survivor_id).await;
        let properties = settled.get("multiRegionProperties");
        assert_eq!(
            reported_peers(&properties),
            fixture.remaining_peers,
            "case {:?}: a removed cluster must be named by no survivor, got {properties:?}",
            case.name
        );
        if fixture.remaining_peers.is_empty() {
            assert_eq!(
                properties,
                Value::Null,
                "case {:?}: a survivor left with no peer keeps no multi-Region \
                 configuration, got {properties:?}",
                case.name
            );
        } else {
            assert_eq!(
                properties.get("witnessRegion").cloned().unwrap_or(Value::Null),
                json!(WITNESS_REGION),
                "case {:?}: a survivor that still names a peer keeps its witness Region, \
                 got {properties:?}",
                case.name
            );
        }
        assert_eq!(
            settled.str("status"),
            case.after,
            "case {:?}: the survivor must settle into {:?}, got {:?}",
            case.name,
            case.after,
            settled.str("status")
        );

        // The point of the unwinding: a survivor whose peering is gone is a cluster
        // like any other, so an update that carries nothing about multi-Region
        // settles it back where it was rather than into a wait for a peer that is
        // gone — and one still naming a peer which does not name it back is still
        // waiting, and refuses that update exactly as any waiting cluster does.
        let unrelated = fixture
            .survivor
            .update(&fixture.survivor_id, json!({"deletionProtectionEnabled": false}))
            .await;
        if case.after == STATUS_PENDING_SETUP {
            assert_eq!(
                (unrelated.status, unrelated.error.as_deref()),
                (409, Some("ConflictException")),
                "case {:?}: a survivor still waiting for a peer refuses an unrelated \
                 mutation, got {} {:?}",
                case.name,
                unrelated.status,
                unrelated.body
            );
            continue;
        }
        assert_eq!(
            (unrelated.status, unrelated.str("status")),
            (200, STATUS_UPDATING.to_string()),
            "case {:?}: an unrelated update of the survivor must be accepted, got {} {:?}",
            case.name,
            unrelated.status,
            unrelated.body
        );
        wait_for_the_clock().await;
        let after_update = fixture.survivor.get(&fixture.survivor_id).await;
        assert_eq!(
            after_update.str("status"),
            case.after,
            "case {:?}: an unrelated update must not leave the survivor waiting for a peer \
             the deletion took away, got {:?}",
            case.name,
            after_update.str("status")
        );

        // And the survivor is still deletable, which a cluster waiting for a
        // peering is not.
        let retired = fixture.survivor.delete(&fixture.survivor_id).await;
        assert_eq!(
            retired.status, 200,
            "case {:?}: the survivor must still accept its own deletion, got {} {:?}",
            case.name,
            retired.status,
            retired.body
        );
    }
}
