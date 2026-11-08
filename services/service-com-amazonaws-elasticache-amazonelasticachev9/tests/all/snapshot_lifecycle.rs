// SPDX-License-Identifier: CC0-1.0

//! The snapshot rules no other suite of this target pins: the INSTANTS a
//! snapshot copies out of its source cluster, the VPC it reports, and that a
//! manual snapshot never expires on the retention limit it carries.
//!
//! Every assertion goes over the wire through this target's harness — the
//! router-backed `Emulator` and `Reply` that live in
//! `tests/all/cache_cluster_lifecycle.rs` — so only the HTTP status and the XML
//! body are inspected: a response that is not computed from persisted state
//! cannot pass. The `snapshots` suite beside this one covers the create/describe
//! round trip, the filters, the pages and the refusals; this one does not repeat
//! them.
//!
//! The rule under test is the one that makes a snapshot a copy of the SOURCE
//! CLUSTER AS IT STOOD when the snapshot was taken — so each value it reports is
//! compared against what the cluster itself reports, never against a second
//! hand-written expectation. The tables are inputs along the axes that rule
//! generalises over: the engine of the source cluster, whether it sits in a
//! subnet group, and the retention limit it carries.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

// The harness every suite of this target shares; the cluster helpers live in
// the cluster suite and the wire harness plus this suite's snapshot drivers in
// `crate::harness`.
use crate::cache_cluster_lifecycle::{
    account_subnet_ids, create_available_cluster, describe_cluster, subnet_group_params,
};
use crate::harness::{describe_snapshot, snapshot, Emulator, PAST_TRANSITION};

#[tokio::test]
async fn a_snapshot_reports_the_source_clusters_own_instants() {
    // The rule: `Snapshot$CacheClusterCreateTime` is "the date and time when the
    // source cluster was created" and `NodeSnapshot$CacheNodeCreateTime` is
    // "when the cache node was created in the source cluster" — the same
    // instants the cluster reports, copied rather than re-read from a clock, so
    // neither drifts or loses precision on the way into the snapshot.
    //
    // The axis: the engine of the source cluster whose instants are copied.
    // Memcached cannot be snapshotted and a Valkey / Redis OSS cluster has one
    // node, so that is the axis a cluster snapshot has.
    for (case, extra) in [
        ("instants-redis", &[("Engine", "redis")][..]),
        (
            "instants-valkey",
            &[("Engine", "valkey"), ("CacheNodeType", "cache.m7g.large")][..],
        ),
    ] {
        let emulator = Emulator::new();
        create_available_cluster(&emulator, case, extra).await;
        // Read the cluster BEFORE the snapshot: taking one moves it to
        // `snapshotting`, and these are the instants the snapshot must copy.
        let cluster = describe_cluster(&emulator, case).await;
        let cluster_created = cluster
            .text("CacheClusterCreateTime")
            .expect("the cluster reports its creation instant")
            .to_string();
        let node_created = cluster
            .all("CacheNodeCreateTime")
            .iter()
            .map(|text| (*text).to_string())
            .collect::<Vec<String>>();
        assert!(
            !node_created.is_empty(),
            "case {case}: the cluster must report its nodes' creation instants, got {}",
            cluster.body
        );

        snapshot(&emulator, "instants", case).await;
        std::thread::sleep(PAST_TRANSITION);
        let described = describe_snapshot(&emulator, "instants").await;

        assert_eq!(
            described.text("Snapshot/CacheClusterCreateTime"),
            Some(cluster_created.as_str()),
            "case {case}: expected the cluster's creation instant {cluster_created}, got {:?}",
            described.text("Snapshot/CacheClusterCreateTime")
        );
        assert_eq!(
            described.all("CacheNodeCreateTime"),
            node_created,
            "case {case}: expected the source nodes' creation instants {node_created:?}, got {:?}",
            described.all("CacheNodeCreateTime")
        );
        // `NodeSnapshot$SnapshotCreateTime` is "when the source node's metadata
        // and cache data set was obtained for the snapshot": one per node, in
        // the same date-time form, and never before the cluster existed.
        let captured = described.all("SnapshotCreateTime");
        assert_eq!(
            captured.len(),
            node_created.len(),
            "case {case}: every node snapshot carries its capture instant, got {captured:?}"
        );
        for instant in captured {
            assert!(
                instant.contains('T') && instant.ends_with('Z'),
                "case {case}: expected an ISO-8601 UTC instant, got {instant}"
            );
            assert!(
                instant >= cluster_created.as_str(),
                "case {case}: the capture instant {instant} cannot precede the cluster's creation \
                 {cluster_created}"
            );
        }
    }
}

#[tokio::test]
async fn a_snapshot_reports_the_vpc_of_the_source_clusters_subnet_group() {
    // The rule: `Snapshot$VpcId` is "the VPC ID of the cache subnet group for
    // the source cluster", so it is that stored group's VPC — and a cluster
    // placed in no subnet group reports neither member.
    let emulator = Emulator::new();
    let account_subnets = account_subnet_ids(&emulator).await;
    let group = emulator
        .call(
            "CreateCacheSubnetGroup",
            &subnet_group_params("snap-subnets", "for snapshots", &account_subnets[..2]),
        )
        .await;
    assert_eq!(
        group.status, 200,
        "the subnet group must be created, got {} {}",
        group.status, group.body
    );
    let vpc_id = group
        .text("VpcId")
        .expect("the subnet group reports its VPC")
        .to_string();

    create_available_cluster(
        &emulator,
        "in-a-vpc",
        &[("Engine", "redis"), ("CacheSubnetGroupName", "snap-subnets")],
    )
    .await;
    snapshot(&emulator, "vpc-snap", "in-a-vpc").await;
    std::thread::sleep(PAST_TRANSITION);
    let described = describe_snapshot(&emulator, "vpc-snap").await;
    assert_eq!(
        described.text("Snapshot/CacheSubnetGroupName"),
        Some("snap-subnets"),
        "case in-a-vpc: the snapshot must name the source cluster's subnet group, got {}",
        described.body
    );
    assert_eq!(
        described.text("Snapshot/VpcId"),
        Some(vpc_id.as_str()),
        "case in-a-vpc: expected the group's VPC {vpc_id}, got {}",
        described.body
    );

    create_available_cluster(&emulator, "no-vpc", &[("Engine", "redis")]).await;
    snapshot(&emulator, "plain-snap", "no-vpc").await;
    std::thread::sleep(PAST_TRANSITION);
    let plain = describe_snapshot(&emulator, "plain-snap").await;
    for element in ["Snapshot/CacheSubnetGroupName", "Snapshot/VpcId"] {
        assert_eq!(
            plain.text(element),
            None,
            "case no-vpc: a cluster in no subnet group reports no {element}, got {}",
            plain.body
        );
    }
}

#[tokio::test]
async fn a_manual_snapshot_does_not_expire_on_the_retention_limit_it_carries() {
    // The rule: for a manual snapshot `SnapshotRetentionLimit` "is otherwise
    // ignored: Manual snapshots do not expire, and can only be deleted using the
    // DeleteSnapshot operation" — so no limit the source cluster carries ever
    // removes one.
    //
    // The axis: the limit itself, from the documented default of 0 ("automatic
    // backups are disabled for this cache cluster") through the shortest
    // retention a cluster can carry to the longest ElastiCache publishes. A
    // limit of 1 is the one that would bite first if anything expired a manual
    // snapshot.
    for limit in ["0", "1", "35"] {
        let emulator = Emulator::new();
        let cluster = format!("retain-{limit}");
        create_available_cluster(
            &emulator,
            &cluster,
            &[("Engine", "redis"), ("SnapshotRetentionLimit", limit)],
        )
        .await;
        snapshot(&emulator, "retained", &cluster).await;

        // Past the one span this emulator owes a snapshot — its transition to
        // `available` — the snapshot is still there, and nothing else will ever
        // remove it: `SnapshotRetentionLimit` governs the automatic backups
        // ElastiCache takes on a schedule this emulator does not run.
        std::thread::sleep(PAST_TRANSITION);
        let described = describe_snapshot(&emulator, "retained").await;
        assert_eq!(
            described.all("SnapshotName"),
            vec!["retained"],
            "case {limit}: a manual snapshot does not expire, got {}",
            described.body
        );
        assert_eq!(
            described.text("Snapshot/SnapshotStatus"),
            Some(state::STATUS_AVAILABLE),
            "case {limit}: the snapshot must be {} once the clock owes it, got {}",
            state::STATUS_AVAILABLE,
            described.body
        );
        // And the source cluster the snapshot came from settles back to
        // `available`, so taking one holds nothing open.
        let source = describe_cluster(&emulator, &cluster).await;
        assert_eq!(
            source.text("CacheClusterStatus"),
            Some(state::STATUS_AVAILABLE),
            "case {limit}: the source cluster must settle back to {}, got {}",
            state::STATUS_AVAILABLE,
            source.body
        );
    }
}
