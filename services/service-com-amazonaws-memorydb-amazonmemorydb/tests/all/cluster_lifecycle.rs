// SPDX-License-Identifier: CC0-1.0

//! The MemoryDB cluster lifecycle, end to end over the wire.
//!
//! Every case here drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router, so what
//! is asserted is the emulator's HTTP behaviour and not a handler called directly.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a create persists the resource and a later describe answers from that state —
//!   same ARN, endpoint, node type, shard layout and tags;
//! * a cluster's shard layout follows `NumShards` / `NumReplicasPerShard`: `NumShards`
//!   shards covering the protocol's 16384 hash slots contiguously, each with one
//!   primary plus `NumReplicasPerShard` replicas;
//! * an update patches only the members it supplies;
//! * a resource AWS reports in a transient status settles to its terminal status once
//!   the clock owes it, without any caller doing anything;
//! * a listing pages over the whole set in name order for any `MaxResults`;
//! * naming a resource that is not in state is the modelled not-found fault, and
//!   re-creating one that is is the modelled already-exists fault.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_memorydb_amazonmemorydb::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for. Every ARN and endpoint the emulator
/// mints must carry it, rather than a constant baked into the service.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Longer than `crate`-side `SETTLE_AFTER`, so a transient status is certainly due by
/// the time the test looks again. The emulator stores the due instant and never
/// sleeps; only the suites that assert a settled status wait, and they block the test
/// thread rather than awaiting, because the workspace's `tokio` is declared without
/// the `time` feature (`#[tokio::test]` / `#[tokio::main]` only).
const PAST_SETTLE: Duration = Duration::from_millis(700);

/// The number of hash slots a cluster spreads over its shards (Redis OSS / Valkey
/// cluster protocol: slots `0..=16383`).
const TOTAL_SLOTS: i64 = 16_384;

/// The service's published layout bounds, mirrored from
/// `crate::state::MAX_SHARDS_PER_CLUSTER` (MemoryDB's per-cluster shard quota) and
/// `MAX_REPLICAS_PER_SHARD` (`NumReplicasPerShard`: "The maximum is 5"). Each is
/// exercised AT the bound and one past it.
const MAX_SHARDS: i64 = 500;
const MAX_REPLICAS: i64 = 5;

/// A router with just this service mounted, on one shared engine — what
/// `core/router` builds, narrowed to the service under test.
///
/// `pub(crate)` so the other suites in this test crate drive the SAME harness rather
/// than standing up a second one.
pub(crate) fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation, signed for `region`, and return `(status, body)`.
///
/// The `Authorization` header carries a SigV4 credential scope naming `memorydb` in
/// that region: that is how the router picks this service, and how the handler learns
/// the region a resource it mints belongs to.
pub(crate) async fn call_signed(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://memory-db.{region}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonMemoryDB.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/memorydb/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({ "raw": String::from_utf8_lossy(&bytes) }))
    };
    (status, parsed)
}

/// Call one operation signed for [`REGION`], and return `(status, body)`.
pub(crate) async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    call_signed(router, REGION, operation, body).await
}

/// Call an operation that must succeed, returning its body.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value, case: &str) -> Value {
    let (status, body) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {body}"
    );
    body
}

/// The wire error code of a failure body (`__type`, with any namespace prefix dropped).
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Create an ACL, the step a client takes before it creates clusters. No wait: an
/// ACL's status gates nothing, and only the suite that asserts the settle pays for it.
async fn acl(router: &MultiServiceRouter, name: &str) {
    ok(router, "CreateACL", &json!({ "ACLName": name }), name).await;
}

/// `Shards[*].Slots` of one cluster, as `(first, last)` pairs in shard order.
fn slot_ranges(cluster: &Value, case: &str) -> Vec<(i64, i64)> {
    cluster
        .get("Shards")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected Shards in {cluster}"))
        .iter()
        .map(|shard| {
            let slots = shard
                .get("Slots")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: expected Slots in {shard}"));
            let (first, last) = slots
                .split_once('-')
                .unwrap_or_else(|| panic!("case {case:?}: malformed Slots {slots:?}"));
            (
                first.parse().expect("a slot number"),
                last.parse().expect("a slot number"),
            )
        })
        .collect()
}

/// A resource belongs to the region the request that created it was signed for, and it
/// keeps that region for the rest of its life: a later call signed elsewhere reports
/// the same ARN, endpoints and zones rather than rewriting them.
#[tokio::test]
async fn a_resource_keeps_the_region_it_was_created_in() {
    const OTHER_REGION: &str = "eu-west-1";
    let router = router();

    // The region comes from the request, not from a constant in the service: the same
    // create signed in two regions mints two differently-scoped ARNs.
    for region in [REGION, OTHER_REGION] {
        let case = format!("created in {region}");
        let (status, body) = call_signed(
            &router,
            region,
            "CreateACL",
            &json!({ "ACLName": format!("acl-{region}") }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} with {body}");
        assert_eq!(
            body["ACL"]["ARN"],
            json!(format!("arn:aws:memorydb:{region}:{ACCOUNT}:acl/acl-{region}")),
            "case {case:?}: the ARN must carry the request's region, got {body}"
        );
    }

    let case = "cross-region resize";
    let created = call_signed(
        &router,
        REGION,
        "CreateCluster",
        &json!({
            "ClusterName": "homed",
            "NodeType": "db.t4g.small",
            "ACLName": format!("acl-{REGION}"),
            "NumShards": 2,
        }),
    )
    .await
    .1;
    let created_arn = created["Cluster"]["ARN"].clone();
    let created_endpoint = created["Cluster"]["ClusterEndpoint"].clone();

    // The resize is signed for ANOTHER region. It must not move the cluster.
    let (status, resized) = call_signed(
        &router,
        OTHER_REGION,
        "UpdateCluster",
        &json!({ "ClusterName": "homed", "ShardConfiguration": { "ShardCount": 3 } }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: got {status} with {resized}");
    let resized = &resized["Cluster"];
    assert_eq!(
        resized["ARN"], created_arn,
        "case {case:?}: the ARN must not follow the caller's region, got {resized}"
    );
    assert_eq!(
        resized["ClusterEndpoint"], created_endpoint,
        "case {case:?}: the configuration endpoint must not move, got {resized}"
    );
    let shards = resized["Shards"].as_array().expect("Shards is a list");
    assert_eq!(
        shards.len(),
        3,
        "case {case:?}: the resize must still apply, got {resized}"
    );
    for shard in shards {
        for node in shard["Nodes"].as_array().expect("Nodes is a list") {
            let address = node["Endpoint"]["Address"].as_str().unwrap_or_default();
            assert!(
                address.ends_with(&format!("memorydb.{REGION}.amazonaws.com")),
                "case {case:?}: a re-laid node must stay in the cluster's region, got {node}"
            );
            assert!(
                node["AvailabilityZone"]
                    .as_str()
                    .is_some_and(|zone| zone.starts_with(REGION)),
                "case {case:?}: a re-laid node's zone must stay in the cluster's region, \
                 got {node}"
            );
        }
    }
}

/// A resource AWS reports in a transient status settles to its terminal status once
/// the clock owes it — the ACL half of the rule the lifecycle suite checks on clusters.
#[tokio::test]
async fn a_new_acl_settles_from_creating_to_active() {
    let router = router();
    let case = "acl settle";
    let created = ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": "settling-acl", "UserNames": ["app-user"] }),
        case,
    )
    .await;
    assert_eq!(
        created["ACL"]["Status"], "creating",
        "case {case:?}: a just-created ACL is not active yet, got {created}"
    );
    assert_eq!(
        created["ACL"]["ARN"],
        json!(format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:acl/settling-acl")),
        "case {case:?}: expected a region- and account-correct ARN, got {created}"
    );

    std::thread::sleep(PAST_SETTLE);
    let described = ok(
        &router,
        "DescribeACLs",
        &json!({ "ACLName": "settling-acl" }),
        case,
    )
    .await;
    assert_eq!(
        described["ACLs"][0]["Status"], "active",
        "case {case:?}: expected the ACL to have settled, got {described}"
    );
    assert_eq!(
        described["ACLs"][0]["UserNames"],
        json!(["app-user"]),
        "case {case:?}: the stored user names must round-trip, got {described}"
    );
}

/// THE WHOLE WORKFLOW the issue's "done when" names, on one shared store:
/// CreateACL → CreateCluster → DescribeClusters → DescribeACLs → UpdateCluster →
/// DescribeClusters → DeleteCluster → DescribeClusters.
#[tokio::test]
async fn the_cluster_lifecycle_is_served_from_persisted_state() {
    let router = router();
    let case = "lifecycle";
    acl(&router, "demo-acl").await;
    let created = ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": "demo",
            "NodeType": "db.t4g.small",
            "ACLName": "demo-acl",
            "NumShards": 2,
            "Description": "created",
            "Tags": [{ "Key": "team", "Value": "platform" }],
        }),
        case,
    )
    .await;
    let created = &created["Cluster"];
    assert_eq!(
        created["Status"], "creating",
        "case {case:?}: a just-created cluster is not serving yet, got {created}"
    );
    assert_eq!(
        created["ARN"],
        json!(format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:cluster/demo")),
        "case {case:?}: expected a region- and account-correct ARN, got {created}"
    );
    assert_eq!(
        created["ClusterEndpoint"]["Port"],
        json!(6379),
        "case {case:?}: expected the default port, got {created}"
    );
    let endpoint = created["ClusterEndpoint"]["Address"].clone();
    assert!(
        endpoint
            .as_str()
            .is_some_and(|address| address.starts_with("demo.")
                && address.ends_with(&format!("memorydb.{REGION}.amazonaws.com"))),
        "case {case:?}: expected a MemoryDB endpoint host, got {endpoint}"
    );

    // The cluster settles to `available` because the clock owed it, with no caller
    // doing anything but looking.
    std::thread::sleep(PAST_SETTLE);
    let described = ok(
        &router,
        "DescribeClusters",
        &json!({ "ClusterName": "demo" }),
        case,
    )
    .await;
    let described = &described["Clusters"][0];
    assert_eq!(
        described["Status"], "available",
        "case {case:?}: expected the cluster to have settled, got {described}"
    );
    for member in [
        "Name",
        "NodeType",
        "NumberOfShards",
        "ACLName",
        "ARN",
        "Description",
        "ClusterEndpoint",
    ] {
        assert_eq!(
            described[member], created[member],
            "case {case:?}: {member} must round-trip through state, got {described}"
        );
    }
    assert!(
        described.get("Shards").is_none(),
        "case {case:?}: Shards is reported only when ShowShardDetails is set, got {described}"
    );

    // The ACL reports the cluster that references it.
    let acls = ok(
        &router,
        "DescribeACLs",
        &json!({ "ACLName": "demo-acl" }),
        case,
    )
    .await;
    assert_eq!(
        acls["ACLs"][0]["Clusters"],
        json!(["demo"]),
        "case {case:?}: an ACL lists the clusters using it, got {acls}"
    );

    // The update is visible in the next describe, and only the supplied members move.
    let updated = ok(
        &router,
        "UpdateCluster",
        &json!({
            "ClusterName": "demo",
            "Description": "updated",
            "SnapshotRetentionLimit": 3,
        }),
        case,
    )
    .await;
    assert_eq!(
        updated["Cluster"]["Status"], "updating",
        "case {case:?}: an accepted update leaves the cluster updating, got {updated}"
    );
    std::thread::sleep(PAST_SETTLE);
    let after = ok(
        &router,
        "DescribeClusters",
        &json!({ "ClusterName": "demo" }),
        case,
    )
    .await;
    let after = &after["Clusters"][0];
    assert_eq!(
        after["Description"], "updated",
        "case {case:?}: expected the update to be visible, got {after}"
    );
    assert_eq!(
        after["SnapshotRetentionLimit"],
        json!(3),
        "case {case:?}: expected the update to be visible, got {after}"
    );
    assert_eq!(
        after["NodeType"], created["NodeType"],
        "case {case:?}: an absent member must not change, got {after}"
    );
    assert_eq!(
        after["Status"], "available",
        "case {case:?}: expected the update to have settled, got {after}"
    );

    // The delete answers with the cluster as it was, and removes it from state.
    let deleted = ok(
        &router,
        "DeleteCluster",
        &json!({ "ClusterName": "demo" }),
        case,
    )
    .await;
    assert_eq!(
        deleted["Cluster"]["Status"], "deleting",
        "case {case:?}: a deleted cluster reports deleting, got {deleted}"
    );
    assert_eq!(
        deleted["Cluster"]["Description"], "updated",
        "case {case:?}: the delete answers with the stored cluster, got {deleted}"
    );
    let (status, body) = call(
        &router,
        "DescribeClusters",
        &json!({ "ClusterName": "demo" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ClusterNotFoundFault"),
        "case {case:?}: the cluster must be gone from state, got {status} with {body}"
    );
    // And the ACL no longer lists it.
    let acls = ok(&router, "DescribeACLs", &json!({}), case).await;
    assert_eq!(
        acls["ACLs"][0]["Clusters"],
        json!([]),
        "case {case:?}: a deleted cluster leaves its ACL, got {acls}"
    );
}

/// The shard layout follows `NumShards` / `NumReplicasPerShard` for ANY combination:
/// `NumShards` shards (default 1) covering `0..=16383` contiguously, each with one
/// primary plus `NumReplicasPerShard` replicas (default 1), on the requested port
/// (default 6379). The layout is persisted, so a describe repeats it exactly.
#[tokio::test]
async fn the_shard_layout_follows_the_requested_shape() {
    let router = router();
    acl(&router, "shape-acl").await;

    // One row per requested shape. The defaults come from the model's own docs:
    // "NumShards ... The default value is 1", "NumReplicasPerShard ... The default
    // value is 1"; 6379 is the Redis OSS / Valkey port MemoryDB serves on.
    struct Shape {
        num_shards: Option<i64>,
        replicas: Option<i64>,
        port: Option<i64>,
        want_shards: usize,
        want_nodes: usize,
        want_port: i64,
        want_mode: &'static str,
    }
    const fn shape(
        num_shards: Option<i64>,
        replicas: Option<i64>,
        port: Option<i64>,
        want_shards: usize,
        want_nodes: usize,
        want_port: i64,
        want_mode: &'static str,
    ) -> Shape {
        Shape {
            num_shards,
            replicas,
            port,
            want_shards,
            want_nodes,
            want_port,
            want_mode,
        }
    }
    let cases = &[
        shape(None, None, None, 1, 2, 6379, "multiaz"),
        shape(Some(1), Some(0), None, 1, 1, 6379, "singleaz"),
        shape(Some(2), Some(1), Some(6380), 2, 2, 6380, "multiaz"),
        shape(Some(3), Some(2), None, 3, 3, 6379, "multiaz"),
        shape(Some(5), Some(0), Some(11_211), 5, 1, 11_211, "singleaz"),
        shape(Some(7), Some(3), None, 7, 4, 6379, "multiaz"),
        // Both published bounds: the largest shard count and the largest replica
        // count the service accepts (MAX_SHARDS_PER_CLUSTER / MAX_REPLICAS_PER_SHARD;
        // one past each is refused in the fault table below).
        shape(Some(MAX_SHARDS), Some(1), None, MAX_SHARDS as usize, 2, 6379, "multiaz"),
        shape(Some(2), Some(MAX_REPLICAS), None, 2, MAX_REPLICAS as usize + 1, 6379, "multiaz"),
    ];

    for (index, expected) in cases.iter().enumerate() {
        let Shape {
            num_shards: shards,
            replicas,
            port,
            want_shards,
            want_nodes,
            want_port,
            want_mode,
        } = expected;
        let name = format!("shape-{index}");
        let case = format!("NumShards={shards:?} NumReplicasPerShard={replicas:?} Port={port:?}");
        let mut request = json!({
            "ClusterName": name,
            "NodeType": "db.r6g.large",
            "ACLName": "shape-acl",
        });
        if let Some(shards) = shards {
            request["NumShards"] = json!(shards);
        }
        if let Some(replicas) = replicas {
            request["NumReplicasPerShard"] = json!(replicas);
        }
        if let Some(port) = port {
            request["Port"] = json!(port);
        }

        let created = ok(&router, "CreateCluster", &request, &case).await;
        let created = created["Cluster"].clone();
        assert_eq!(
            created["NumberOfShards"],
            json!(*want_shards as i64),
            "case {case:?}: expected {want_shards} shards, got {created}"
        );
        assert_eq!(
            created["ClusterEndpoint"]["Port"],
            json!(*want_port),
            "case {case:?}: expected port {want_port}, got {created}"
        );
        assert_eq!(
            created["AvailabilityMode"],
            json!(*want_mode),
            "case {case:?}: expected availability mode {want_mode}, got {created}"
        );

        let ranges = slot_ranges(&created, &case);
        assert_eq!(
            ranges.len(),
            *want_shards,
            "case {case:?}: expected {want_shards} shard entries, got {ranges:?}"
        );
        let mut expected_first = 0;
        for (shard_index, (first, last)) in ranges.iter().enumerate() {
            assert_eq!(
                *first, expected_first,
                "case {case:?}: shard {shard_index} must continue where the previous one \
                 ended, got {ranges:?}"
            );
            assert!(
                last >= first,
                "case {case:?}: shard {shard_index} has an empty slot range, got {ranges:?}"
            );
            expected_first = last + 1;
        }
        assert_eq!(
            expected_first, TOTAL_SLOTS,
            "case {case:?}: the shards must cover every hash slot exactly once, got {ranges:?}"
        );

        let shard_list = created["Shards"].as_array().expect("Shards is a list");
        for shard in shard_list {
            let nodes = shard["Nodes"].as_array().expect("Nodes is a list");
            assert_eq!(
                nodes.len(),
                *want_nodes,
                "case {case:?}: expected {want_nodes} nodes per shard, got {shard}"
            );
            assert_eq!(
                shard["NumberOfNodes"],
                json!(*want_nodes as i64),
                "case {case:?}: NumberOfNodes must match the nodes reported, got {shard}"
            );
            for node in nodes {
                assert_eq!(
                    node["Endpoint"]["Port"],
                    json!(*want_port),
                    "case {case:?}: a node serves the cluster's port, got {node}"
                );
                assert!(
                    node["AvailabilityZone"]
                        .as_str()
                        .is_some_and(|zone| zone.starts_with(REGION)),
                    "case {case:?}: a node is in the request's region, got {node}"
                );
            }
        }

        // The layout is read back from state, not recomputed per call.
        let described = ok(
            &router,
            "DescribeClusters",
            &json!({ "ClusterName": name, "ShowShardDetails": true }),
            &case,
        )
        .await;
        assert_eq!(
            described["Clusters"][0]["Shards"], created["Shards"],
            "case {case:?}: the stored shard layout must be reported unchanged"
        );
    }
}

/// A `ShardConfiguration` or `ReplicaConfiguration` update re-lays the cluster out:
/// the new shard count still covers every hash slot, and each shard carries the new
/// number of nodes. The new layout is persisted, so the next describe repeats it.
#[tokio::test]
async fn an_update_relays_out_the_shards_it_resizes() {
    let router = router();
    acl(&router, "reshape-acl").await;

    // One row per resize. A create without `NumReplicasPerShard` has the modelled
    // default of 1 replica, so a row that supplies no `ReplicaCount` keeps two nodes
    // per shard.
    struct Resize {
        initial: i64,
        shard_count: Option<i64>,
        replica_count: Option<i64>,
        want_shards: usize,
        want_nodes: usize,
    }
    const fn resize(
        initial: i64,
        shard_count: Option<i64>,
        replica_count: Option<i64>,
        want_shards: usize,
        want_nodes: usize,
    ) -> Resize {
        Resize {
            initial,
            shard_count,
            replica_count,
            want_shards,
            want_nodes,
        }
    }
    let cases = &[
        resize(1, Some(3), None, 3, 2),
        resize(4, Some(2), Some(0), 2, 1),
        resize(2, None, Some(2), 2, 3),
        resize(3, Some(1), Some(4), 1, 5),
    ];

    for (index, expected) in cases.iter().enumerate() {
        let Resize {
            initial,
            shard_count,
            replica_count,
            want_shards,
            want_nodes,
        } = expected;
        let name = format!("reshape-{index}");
        let case =
            format!("{initial} shards -> ShardCount={shard_count:?} ReplicaCount={replica_count:?}");
        ok(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": name,
                "NodeType": "db.r6g.large",
                "ACLName": "reshape-acl",
                "NumShards": initial,
            }),
            &case,
        )
        .await;

        let mut request = json!({ "ClusterName": name });
        if let Some(shard_count) = shard_count {
            request["ShardConfiguration"] = json!({ "ShardCount": shard_count });
        }
        if let Some(replica_count) = replica_count {
            request["ReplicaConfiguration"] = json!({ "ReplicaCount": replica_count });
        }
        let updated = ok(&router, "UpdateCluster", &request, &case).await;
        let updated = updated["Cluster"].clone();
        assert_eq!(
            updated["NumberOfShards"],
            json!(*want_shards as i64),
            "case {case:?}: expected {want_shards} shards after the update, got {updated}"
        );
        let ranges = slot_ranges(&updated, &case);
        assert_eq!(
            ranges.len(),
            *want_shards,
            "case {case:?}: expected {want_shards} shard entries, got {ranges:?}"
        );
        assert_eq!(
            ranges.first().map(|(first, _)| *first),
            Some(0),
            "case {case:?}: the re-laid shards must start at slot 0, got {ranges:?}"
        );
        assert_eq!(
            ranges.last().map(|(_, last)| *last),
            Some(TOTAL_SLOTS - 1),
            "case {case:?}: the re-laid shards must still cover every slot, got {ranges:?}"
        );
        for shard in updated["Shards"].as_array().expect("Shards is a list") {
            assert_eq!(
                shard["Nodes"].as_array().map(Vec::len),
                Some(*want_nodes),
                "case {case:?}: expected {want_nodes} nodes per shard, got {shard}"
            );
        }

        let described = ok(
            &router,
            "DescribeClusters",
            &json!({ "ClusterName": name, "ShowShardDetails": true }),
            &case,
        )
        .await;
        assert_eq!(
            described["Clusters"][0]["Shards"], updated["Shards"],
            "case {case:?}: the re-laid layout must be the one stored"
        );
    }
}

/// Every listing pages over its whole set, in name order, for any `MaxResults`: each
/// page is at most `MaxResults` long, the token resumes exactly where the page ended,
/// and the pages concatenated are the full set with nothing repeated or dropped.
#[tokio::test]
async fn listings_page_over_the_whole_set_for_any_page_size() {
    let router = router();
    acl(&router, "page-acl").await;

    let names: Vec<String> = (0..7).map(|index| format!("page-{index}")).collect();
    for name in &names {
        ok(
            &router,
            "CreateCluster",
            &json!({
                "ClusterName": name,
                "NodeType": "db.t4g.small",
                "ACLName": "page-acl",
            }),
            name,
        )
        .await;
    }

    for max_results in [1_i64, 2, 3, 6, 7, 20] {
        let case = format!("MaxResults={max_results}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": max_results });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let body = ok(&router, "DescribeClusters", &request, &case).await;
            let page = body["Clusters"].as_array().expect("Clusters is a list");
            assert!(
                page.len() <= max_results as usize,
                "case {case:?}: a page may hold at most {max_results} items, got {}",
                page.len()
            );
            seen.extend(
                page.iter()
                    .map(|cluster| cluster["Name"].as_str().unwrap_or_default().to_string()),
            );
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages, saw {seen:?}"
            );
            match body.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, names,
            "case {case:?}: paging must yield every cluster once, in name order"
        );
    }

    // The same rule on the other paginated listing in this slice.
    for name in ["page-acl-b", "page-acl-c"] {
        ok(&router, "CreateACL", &json!({ "ACLName": name }), name).await;
    }
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({ "MaxResults": 2 });
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let body = ok(&router, "DescribeACLs", &request, "acl paging").await;
        seen.extend(
            body["ACLs"]
                .as_array()
                .expect("ACLs is a list")
                .iter()
                .map(|acl| acl["Name"].as_str().unwrap_or_default().to_string()),
        );
        match body.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        seen,
        // The built-in `open-access` ACL every MemoryDB account already has pages with
        // the created ones, and sorts first.
        vec!["open-access", "page-acl", "page-acl-b", "page-acl-c"],
        "case {:?}: paging must yield every ACL once, in name order",
        "acl paging"
    );
}

/// An `ACL`'s `Clusters` is DERIVED from the clusters referencing it, so it follows
/// creates, updates that repoint a cluster, and deletes.
#[tokio::test]
async fn an_acl_lists_the_clusters_that_reference_it() {
    let router = router();
    acl(&router, "left-acl").await;
    acl(&router, "right-acl").await;

    for (name, acl) in [("one", "left-acl"), ("two", "left-acl"), ("three", "right-acl")] {
        ok(
            &router,
            "CreateCluster",
            &json!({ "ClusterName": name, "NodeType": "db.t4g.small", "ACLName": acl }),
            name,
        )
        .await;
    }

    let membership = |body: &Value, acl: &str| -> Vec<String> {
        body["ACLs"]
            .as_array()
            .expect("ACLs is a list")
            .iter()
            .find(|entry| entry["Name"] == json!(acl))
            .map(|entry| {
                entry["Clusters"]
                    .as_array()
                    .expect("Clusters is a list")
                    .iter()
                    .map(|name| name.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    };

    let body = ok(&router, "DescribeACLs", &json!({}), "initial").await;
    assert_eq!(
        membership(&body, "left-acl"),
        vec!["one", "two"],
        "case {:?}: expected both clusters on the ACL they name, got {body}",
        "initial"
    );
    assert_eq!(
        membership(&body, "right-acl"),
        vec!["three"],
        "case {:?}: expected only its own cluster, got {body}",
        "initial"
    );

    // Repointing a cluster at another ACL moves it between the two lists.
    ok(
        &router,
        "UpdateCluster",
        &json!({ "ClusterName": "two", "ACLName": "right-acl" }),
        "repointed",
    )
    .await;
    let body = ok(&router, "DescribeACLs", &json!({}), "repointed").await;
    assert_eq!(
        membership(&body, "left-acl"),
        vec!["one"],
        "case {:?}: the moved cluster must leave its old ACL, got {body}",
        "repointed"
    );
    assert_eq!(
        membership(&body, "right-acl"),
        vec!["three", "two"],
        "case {:?}: the moved cluster must join its new ACL, got {body}",
        "repointed"
    );
}

/// Naming a resource state does not hold is the modelled not-found fault; re-creating
/// one it does hold is the modelled already-exists fault; an update that asks for
/// nothing is `NoOperationFault`. One table, one refusal rule per row.
#[tokio::test]
async fn resolution_and_conflict_faults_come_from_state() {
    let router = router();
    acl(&router, "fault-acl").await;
    ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": "taken",
            "NodeType": "db.t4g.small",
            "ACLName": "fault-acl",
        }),
        "setup",
    )
    .await;

    let cases: &[(&str, Value, u16, &str)] = &[
        (
            "CreateCluster",
            json!({ "ClusterName": "taken", "NodeType": "db.t4g.small", "ACLName": "fault-acl" }),
            400,
            "ClusterAlreadyExistsFault",
        ),
        (
            "CreateCluster",
            json!({ "ClusterName": "fresh", "NodeType": "db.t4g.small", "ACLName": "no-such-acl" }),
            404,
            "ACLNotFoundFault",
        ),
        ("CreateACL", json!({ "ACLName": "fault-acl" }), 400, "ACLAlreadyExistsFault"),
        (
            "DescribeClusters",
            json!({ "ClusterName": "nope" }),
            404,
            "ClusterNotFoundFault",
        ),
        ("DescribeACLs", json!({ "ACLName": "nope" }), 404, "ACLNotFoundFault"),
        (
            "UpdateCluster",
            json!({ "ClusterName": "nope", "Description": "x" }),
            404,
            "ClusterNotFoundFault",
        ),
        (
            "UpdateCluster",
            json!({ "ClusterName": "taken", "ACLName": "no-such-acl" }),
            404,
            "ACLNotFoundFault",
        ),
        ("UpdateCluster", json!({ "ClusterName": "taken" }), 400, "NoOperationFault"),
        (
            "DeleteCluster",
            json!({ "ClusterName": "nope" }),
            404,
            "ClusterNotFoundFault",
        ),
        // Asked for behaviour this slice cannot perform: told so, rather than a 200
        // that silently took no snapshot / touched no multi-Region cluster.
        (
            "DeleteCluster",
            json!({ "ClusterName": "taken", "FinalSnapshotName": "farewell" }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "DeleteCluster",
            json!({ "ClusterName": "taken", "MultiRegionClusterName": "global" }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "DescribeClusters",
            json!({ "MaxResults": 0 }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "CreateCluster",
            json!({
                "ClusterName": "zero-shards",
                "NodeType": "db.t4g.small",
                "ACLName": "fault-acl",
                "NumShards": 0,
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "CreateCluster",
            json!({
                "ClusterName": "negative-replicas",
                "NodeType": "db.t4g.small",
                "ACLName": "fault-acl",
                "NumReplicasPerShard": -1,
            }),
            400,
            "InvalidParameterValueException",
        ),
        // One past each published bound: the layout a cluster may have is capped from
        // ABOVE as well as below, so no request can size the stored shard/node lists.
        (
            "CreateCluster",
            json!({
                "ClusterName": "too-many-shards",
                "NodeType": "db.t4g.small",
                "ACLName": "fault-acl",
                "NumShards": MAX_SHARDS + 1,
            }),
            400,
            "ShardsPerClusterQuotaExceededFault",
        ),
        (
            "CreateCluster",
            json!({
                "ClusterName": "huge-shards",
                "NodeType": "db.t4g.small",
                "ACLName": "fault-acl",
                "NumShards": 2_000_000_000_i64,
                "NumReplicasPerShard": 5,
            }),
            400,
            "ShardsPerClusterQuotaExceededFault",
        ),
        (
            "CreateCluster",
            json!({
                "ClusterName": "too-many-replicas",
                "NodeType": "db.t4g.small",
                "ACLName": "fault-acl",
                "NumReplicasPerShard": MAX_REPLICAS + 1,
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "UpdateCluster",
            json!({
                "ClusterName": "taken",
                "ShardConfiguration": { "ShardCount": MAX_SHARDS + 1 },
            }),
            400,
            "ShardsPerClusterQuotaExceededFault",
        ),
        (
            "UpdateCluster",
            json!({
                "ClusterName": "taken",
                "ReplicaConfiguration": { "ReplicaCount": MAX_REPLICAS + 1 },
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "UpdateCluster",
            json!({ "ClusterName": "taken", "ShardConfiguration": { "ShardCount": 0 } }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "DescribeClusters",
            json!({ "NextToken": "zzz-no-such-position" }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "DescribeACLs",
            json!({ "NextToken": "zzz-no-such-position" }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "DescribeACLs",
            json!({ "MaxResults": 0 }),
            400,
            "InvalidParameterValueException",
        ),
    ];

    for (operation, request, want_status, want_code) in cases {
        let case = format!("{operation} {request}");
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (*want_status, *want_code),
            "case {case:?}: expected {want_status} {want_code}, got {status} with {body}"
        );
    }

    // The refused calls changed nothing: the one cluster and one ACL are still there.
    let clusters = ok(&router, "DescribeClusters", &json!({}), "unchanged").await;
    assert_eq!(
        clusters["Clusters"]
            .as_array()
            .expect("Clusters is a list")
            .len(),
        1,
        "case {:?}: a refused request must not change state, got {clusters}",
        "unchanged"
    );
}
