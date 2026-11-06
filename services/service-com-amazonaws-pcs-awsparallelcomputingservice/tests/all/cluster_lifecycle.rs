// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the PCS cluster lifecycle, driven over the awsJson1_0 wire
//! (`POST /` with `X-Amz-Target: AWSParallelComputingService.<Operation>`) through the
//! same router the emulator serves from — so these exercise routing, deserialization,
//! constraint validation, the handler and serialization, not a handler in isolation.
//!
//! Each test builds its own engine, so the state one test writes is invisible to the
//! others.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_pcs_awsparallelcomputingservice::state::{
    MAX_CLUSTERS, MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER, MAX_QUEUES_PER_CLUSTER,
    MAX_STORED_LIST_ENTRIES, TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header scoped to `pcs` in `us-east-1`. The router resolves a
/// request's service from the credential scope, and the handlers read the region out of
/// it to build an ARN. The signature is never checked: request authentication is off.
const SIGNED_FOR_PCS: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/pcs/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The region [`SIGNED_FOR_PCS`] signs for, which every ARN in these tests carries.
const REGION: &str = "us-east-1";

/// An emulator with only PCS mounted, on its own engine.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_pcs_awsparallelcomputingservice::EmulatedService::new(engine),
    );
    router
}

/// Call one operation and return its HTTP status and JSON body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("AWSParallelComputingService.{operation}"),
        )
        .header("authorization", SIGNED_FOR_PCS)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.body().clone();
    let parsed = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered {status} with a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Wait out the span the service stores for a lifecycle transition the clock owes, so
/// the next call sees it materialised. The service never sleeps for it — this is the
/// CALLER waiting, as a caller of the real service would.
pub(crate) async fn wait_past_the_owed_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(250)).await;
}

/// A `CreateCluster` request carrying every `@required` member.
fn create_cluster(name: &str, size: &str) -> Value {
    json!({
        "clusterName": name,
        "scheduler": {"type": "SLURM", "version": "24.11"},
        "size": size,
        "networking": {
            "subnetIds": ["subnet-0123456789abcdef0"],
            "securityGroupIds": ["sg-0123456789abcdef0"],
            "networkType": "IPV4",
        },
    })
}

/// A `CreateComputeNodeGroup` request carrying every `@required` member. The instance
/// profile ARN satisfies `InstanceProfileArn`'s `@pattern`, which the boundary enforces.
fn create_compute_node_group(cluster: &str, name: &str) -> Value {
    json!({
        "clusterIdentifier": cluster,
        "computeNodeGroupName": name,
        "subnetIds": ["subnet-0123456789abcdef0"],
        "customLaunchTemplate": {"id": "lt-0123456789abcdef0", "version": "1"},
        "iamInstanceProfileArn": format!(
            "arn:aws:iam::{}:instance-profile/AWSPCS-demo",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "scalingConfiguration": {"minInstanceCount": 0, "maxInstanceCount": 4},
        "instanceConfigs": [{"instanceType": "c6i.large"}],
    })
}

/// A `CreateQueue` request, assigning the queue to `groups`.
fn create_queue(cluster: &str, name: &str, groups: &[&str]) -> Value {
    json!({
        "clusterIdentifier": cluster,
        "queueName": name,
        "computeNodeGroupConfigurations": groups
            .iter()
            .map(|id| json!({"computeNodeGroupId": id}))
            .collect::<Vec<Value>>(),
    })
}

/// Create a cluster and wait until the clock has made it ACTIVE, answering its id.
async fn active_cluster(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "CreateCluster", &create_cluster(name, "SMALL")).await;
    assert_eq!(status, 200, "case create {name}: got {body}");
    let id = body["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    wait_past_the_owed_transition().await;
    id
}

#[tokio::test]
async fn a_cluster_round_trips_through_create_get_list_and_delete() {
    let router = emulator();

    let (status, created) = call(
        &router,
        "CreateCluster",
        &create_cluster("demo-cluster", "SMALL"),
    )
    .await;
    assert_eq!(status, 200, "case CreateCluster: got {created}");
    let cluster = created["cluster"].clone();
    let id = cluster["id"].as_str().unwrap_or_default().to_string();

    assert!(
        id.starts_with("pcs_") && id.len() > "pcs_".len(),
        "case CreateCluster: expected a generated pcs_ id, got {cluster}"
    );
    assert_eq!(
        cluster["arn"],
        json!(format!(
            "arn:aws:pcs:{REGION}:{}:cluster/{id}",
            engine::EMULATOR_ACCOUNT_ID
        )),
        "case CreateCluster: expected the cluster ARN to carry the signed region, the \
         emulator account and the generated id, got {cluster}"
    );
    assert_eq!(
        cluster["status"],
        json!("CREATING"),
        "case CreateCluster: expected a new cluster to be reported CREATING, got {cluster}"
    );
    assert!(
        !cluster["createdAt"].is_null() && !cluster["modifiedAt"].is_null(),
        "case CreateCluster: expected both required timestamps, got {cluster}"
    );

    // The identifier resolves by either of the two forms `ClusterIdentifier` allows, and
    // both answer the record the create stored.
    for identifier in [id.as_str(), "demo-cluster"] {
        let (status, got) = call(
            &router,
            "GetCluster",
            &json!({"clusterIdentifier": identifier}),
        )
        .await;
        assert_eq!(status, 200, "case GetCluster {identifier}: got {got}");
        assert_eq!(
            got["cluster"], cluster,
            "case GetCluster {identifier}: expected exactly what CreateCluster stored, got {got}"
        );
    }

    // A child create needs an ACTIVE cluster, and this one is still CREATING.
    let (status, refused) = call(
        &router,
        "CreateQueue",
        &create_queue("demo-cluster", "demo-queue", &[]),
    )
    .await;
    assert_eq!(
        status, 409,
        "case CreateQueue while CREATING: expected a conflict, got {refused}"
    );
    assert_eq!(
        refused["__type"],
        json!("ConflictException"),
        "case CreateQueue while CREATING: got {refused}"
    );

    wait_past_the_owed_transition().await;

    let (status, settled) = call(&router, "GetCluster", &json!({"clusterIdentifier": &id})).await;
    assert_eq!(status, 200, "case GetCluster after the span: got {settled}");
    assert_eq!(
        settled["cluster"]["status"],
        json!("ACTIVE"),
        "case GetCluster after the span: expected the owed CREATING -> ACTIVE transition, \
         got {settled}"
    );
    assert_eq!(
        settled["cluster"]["createdAt"], cluster["createdAt"],
        "case GetCluster after the span: createdAt must be stable across reads, got {settled}"
    );

    // A second cluster of the same name conflicts; the name stays bound to the first id.
    let (status, conflict) = call(
        &router,
        "CreateCluster",
        &create_cluster("demo-cluster", "LARGE"),
    )
    .await;
    assert_eq!(
        status, 409,
        "case duplicate CreateCluster: expected a conflict, got {conflict}"
    );
    assert_eq!(
        conflict["__type"],
        json!("ConflictException"),
        "case duplicate CreateCluster: got {conflict}"
    );

    let (status, missing) = call(
        &router,
        "GetCluster",
        &json!({"clusterIdentifier": "no-such-cluster"}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case GetCluster on an unknown identifier: got {missing}"
    );
    assert_eq!(
        missing["__type"],
        json!("ResourceNotFoundException"),
        "case GetCluster on an unknown identifier: got {missing}"
    );

    let (status, listed) = call(&router, "ListClusters", &json!({})).await;
    assert_eq!(status, 200, "case ListClusters: got {listed}");
    let summaries = listed["clusters"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        summaries.len(),
        1,
        "case ListClusters: expected the one persisted cluster, got {listed}"
    );
    assert_eq!(
        summaries[0]["id"],
        json!(&id),
        "case ListClusters: expected the summary to carry the stored id, got {listed}"
    );

    // Delete reports DELETING straight away and the clock owes the removal.
    let (status, deleted) = call(
        &router,
        "DeleteCluster",
        &json!({"clusterIdentifier": "demo-cluster"}),
    )
    .await;
    assert_eq!(status, 200, "case DeleteCluster: got {deleted}");

    let (status, deleting) = call(&router, "GetCluster", &json!({"clusterIdentifier": &id})).await;
    assert_eq!(status, 200, "case GetCluster while DELETING: got {deleting}");
    assert_eq!(
        deleting["cluster"]["status"],
        json!("DELETING"),
        "case GetCluster while DELETING: expected the transient state, got {deleting}"
    );

    wait_past_the_owed_transition().await;

    for identifier in [id.as_str(), "demo-cluster"] {
        let (status, gone) = call(
            &router,
            "GetCluster",
            &json!({"clusterIdentifier": identifier}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case GetCluster {identifier} after the delete came due: got {gone}"
        );
    }
    let (status, empty) = call(&router, "ListClusters", &json!({})).await;
    assert_eq!(status, 200, "case ListClusters after delete: got {empty}");
    assert_eq!(
        empty["clusters"],
        json!([]),
        "case ListClusters after delete: expected no clusters, got {empty}"
    );
}

#[tokio::test]
async fn a_compute_node_group_and_a_queue_round_trip_through_their_readers() {
    let router = emulator();
    let cluster_id = active_cluster(&router, "demo-cluster").await;

    let (status, created) = call(
        &router,
        "CreateComputeNodeGroup",
        &create_compute_node_group("demo-cluster", "demo-cng"),
    )
    .await;
    assert_eq!(status, 200, "case CreateComputeNodeGroup: got {created}");
    let group = created["computeNodeGroup"].clone();
    let group_id = group["id"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        group["clusterId"],
        json!(&cluster_id),
        "case CreateComputeNodeGroup: expected the parent cluster's id, got {group}"
    );
    assert_eq!(
        group["arn"],
        json!(format!(
            "arn:aws:pcs:{REGION}:{}:cluster/{cluster_id}/computenodegroup/{group_id}",
            engine::EMULATOR_ACCOUNT_ID
        )),
        "case CreateComputeNodeGroup: got {group}"
    );
    assert_eq!(
        group["scalingConfiguration"],
        json!({"minInstanceCount": 0, "maxInstanceCount": 4}),
        "case CreateComputeNodeGroup: expected the requested scaling bounds, got {group}"
    );

    // Read it back through its own reader: the create persisted it.
    for identifier in [group_id.as_str(), "demo-cng"] {
        let (status, got) = call(
            &router,
            "GetComputeNodeGroup",
            &json!({
                "clusterIdentifier": &cluster_id,
                "computeNodeGroupIdentifier": identifier,
            }),
        )
        .await;
        assert_eq!(status, 200, "case GetComputeNodeGroup {identifier}: got {got}");
        assert_eq!(
            got["computeNodeGroup"], group,
            "case GetComputeNodeGroup {identifier}: expected what the create stored, got {got}"
        );
    }

    let (status, created) = call(
        &router,
        "CreateQueue",
        &create_queue("demo-cluster", "demo-queue", &[group_id.as_str()]),
    )
    .await;
    assert_eq!(status, 200, "case CreateQueue: got {created}");
    let queue = created["queue"].clone();
    let queue_id = queue["id"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        queue["clusterId"],
        json!(&cluster_id),
        "case CreateQueue: expected the parent cluster's id, got {queue}"
    );
    assert_eq!(
        queue["arn"],
        json!(format!(
            "arn:aws:pcs:{REGION}:{}:cluster/{cluster_id}/queue/{queue_id}",
            engine::EMULATOR_ACCOUNT_ID
        )),
        "case CreateQueue: got {queue}"
    );
    assert_eq!(
        queue["computeNodeGroupConfigurations"],
        json!([{"computeNodeGroupId": &group_id}]),
        "case CreateQueue: expected the requested compute node groups, got {queue}"
    );

    for identifier in [queue_id.as_str(), "demo-queue"] {
        let (status, got) = call(
            &router,
            "GetQueue",
            &json!({"clusterIdentifier": "demo-cluster", "queueIdentifier": identifier}),
        )
        .await;
        assert_eq!(status, 200, "case GetQueue {identifier}: got {got}");
        assert_eq!(
            got["queue"], queue,
            "case GetQueue {identifier}: expected what the create stored, got {got}"
        );
    }

    let (status, listed) = call(
        &router,
        "ListQueues",
        &json!({"clusterIdentifier": "demo-cluster"}),
    )
    .await;
    assert_eq!(status, 200, "case ListQueues: got {listed}");
    assert_eq!(
        listed["queues"].as_array().map(Vec::len),
        Some(1),
        "case ListQueues: expected the one persisted queue, got {listed}"
    );
    assert_eq!(
        listed["queues"][0]["id"],
        json!(&queue_id),
        "case ListQueues: got {listed}"
    );

    // A cluster that still holds either child cannot be deleted.
    let (status, refused) = call(
        &router,
        "DeleteCluster",
        &json!({"clusterIdentifier": &cluster_id}),
    )
    .await;
    assert_eq!(
        status, 409,
        "case DeleteCluster with children: expected a conflict, got {refused}"
    );
}

#[tokio::test]
async fn a_child_create_refuses_a_cluster_it_cannot_find() {
    let router = emulator();
    active_cluster(&router, "demo-cluster").await;

    // Rows: the operation, the request, and the identifier it names. Every row names a
    // cluster that does not exist, in each of the two forms `ClusterIdentifier` allows.
    let cases: Vec<(&str, Value)> = vec![
        (
            "CreateComputeNodeGroup",
            create_compute_node_group("no-such-cluster", "demo-cng"),
        ),
        (
            "CreateComputeNodeGroup",
            create_compute_node_group("pcs_0000000000000000", "demo-cng"),
        ),
        (
            "CreateQueue",
            create_queue("no-such-cluster", "demo-queue", &[]),
        ),
        (
            "CreateQueue",
            create_queue("pcs_0000000000000000", "demo-queue", &[]),
        ),
        ("GetQueue", json!({
            "clusterIdentifier": "no-such-cluster",
            "queueIdentifier": "demo-queue",
        })),
        ("ListQueues", json!({"clusterIdentifier": "no-such-cluster"})),
        ("GetComputeNodeGroup", json!({
            "clusterIdentifier": "no-such-cluster",
            "computeNodeGroupIdentifier": "demo-cng",
        })),
    ];
    for (operation, request) in cases {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 404,
            "case {operation} on an unknown cluster: expected 404, got {status} {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ResourceNotFoundException"),
            "case {operation} on an unknown cluster: got {body}"
        );
    }

    // A queue cannot be assigned a compute node group its own cluster does not hold.
    let (status, body) = call(
        &router,
        "CreateQueue",
        &create_queue("demo-cluster", "demo-queue", &["pcs_0000000000000000"]),
    )
    .await;
    assert_eq!(
        status, 404,
        "case CreateQueue naming an absent compute node group: got {status} {body}"
    );

    // A scaling range the cluster could never satisfy is refused. Rows: an inverted
    // range, and the degenerate-but-legal equal one.
    for (min, max, expected) in [(4, 0, 400_u16), (0, 0, 200), (2, 2, 200)] {
        let mut request = create_compute_node_group("demo-cluster", &format!("scale-{min}-{max}"));
        request["scalingConfiguration"] =
            json!({"minInstanceCount": min, "maxInstanceCount": max});
        let (status, body) = call(&router, "CreateComputeNodeGroup", &request).await;
        assert_eq!(
            status, expected,
            "case scalingConfiguration {min}..{max}: expected {expected}, got {status} {body}"
        );
    }

    // A queue or compute node group the cluster does not hold is not found either.
    for (operation, request) in [
        (
            "GetQueue",
            json!({"clusterIdentifier": "demo-cluster", "queueIdentifier": "no-such-queue"}),
        ),
        (
            "GetComputeNodeGroup",
            json!({
                "clusterIdentifier": "demo-cluster",
                "computeNodeGroupIdentifier": "no-such-cng",
            }),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 404,
            "case {operation} on an unknown child: expected 404, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_resource_name_is_accepted_exactly_when_it_satisfies_its_pattern_and_length() {
    let router = emulator();

    // `ClusterName` (and `ComputeNodeGroupName`, and `QueueName`) is
    // `@length(min: 3, max: 40)` with `@pattern("^(?!pcs_)^[A-Za-z][A-Za-z0-9-]+$")` —
    // from `smithy_query(query="operation", operation="CreateCluster")`. Rows span the
    // class the rule generalises over: each published length bound and one past it, and
    // each character the class admits or refuses.
    let at_max = "a".repeat(40);
    let over_max = "a".repeat(41);
    let cases: Vec<(&str, u16)> = vec![
        ("demo-cluster", 200),      // the common form
        ("Demo-Cluster-9", 200),    // upper case and digits are in the class
        ("abc", 200),               // the @length minimum
        (at_max.as_str(), 200),     // the @length maximum
        ("ab", 400),                // one under the @length minimum
        (over_max.as_str(), 400),   // one over the @length maximum
        ("pcs_demo", 400),          // the prefix reserved for generated ids
        ("1demo", 400),             // must start with a letter
        ("-demo", 400),             // ... a letter, not a hyphen
        ("demo_cluster", 400),      // `_` is not in the class
        ("demo cluster", 400),      // nor is a space
        ("démo", 400),              // nor is a non-ASCII letter
    ];
    for (name, expected) in cases {
        let (status, body) = call(&router, "CreateCluster", &create_cluster(name, "SMALL")).await;
        assert_eq!(
            status, expected,
            "case clusterName={name:?}: expected {expected}, got {status} {body}"
        );
        if expected == 400 {
            assert_eq!(
                body["__type"],
                json!("ValidationException"),
                "case clusterName={name:?}: got {body}"
            );
        }
    }

    // The same rule holds for the other two name members, on a value neither the issue
    // nor the rows above cite.
    let cluster_id = active_cluster(&router, "names-cluster").await;
    for (operation, request, member) in [
        (
            "CreateComputeNodeGroup",
            create_compute_node_group(&cluster_id, "pcs_group"),
            "computeNodeGroupName",
        ),
        (
            "CreateQueue",
            create_queue(&cluster_id, "queue.one", &[]),
            "queueName",
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {member} outside its pattern: expected 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case {member} outside its pattern: got {body}"
        );
    }
}

#[tokio::test]
async fn a_cluster_size_is_accepted_exactly_when_the_size_enum_declares_it() {
    let router = emulator();

    // The `Size` enum's members, from `SIZE_VALUES` in the service's generated types
    // (`smithy_query(query="shape", shape="Size")`), plus values outside it.
    let cases: Vec<(&str, u16)> = vec![
        ("SMALL", 200),
        ("MEDIUM", 200),
        ("LARGE", 200),
        ("small", 400),      // the members are upper case
        ("EXTRALARGE", 400), // not a member
        ("", 400),           // not a member
    ];
    for (index, (size, expected)) in cases.iter().enumerate() {
        let name = format!("size-cluster-{index}");
        let (status, body) = call(&router, "CreateCluster", &create_cluster(&name, size)).await;
        assert_eq!(
            status, *expected,
            "case size={size:?}: expected {expected}, got {status} {body}"
        );
        if *expected == 200 {
            assert_eq!(
                body["cluster"]["size"],
                json!(size),
                "case size={size:?}: expected the stored size to be echoed, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn list_clusters_pages_through_every_cluster_at_every_page_size() {
    let router = emulator();

    let names = ["alpha-cluster", "beta-cluster", "gamma-cluster"];
    let mut created = Vec::new();
    for name in names {
        let (status, body) = call(&router, "CreateCluster", &create_cluster(name, "SMALL")).await;
        assert_eq!(status, 200, "case create {name}: got {body}");
        created.push(
            body["cluster"]["id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    // Rows: the page size, spanning one item per page up to more than the whole set.
    for page_size in [1_usize, 2, 3, 4] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({"maxResults": page_size});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListClusters", &request).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size} page {pages}: got {body}"
            );
            let page = body["clusters"].as_array().cloned().unwrap_or_default();
            pages += 1;
            assert!(
                !page.is_empty() && page.len() <= page_size,
                "case maxResults={page_size} page {pages}: expected 1..={page_size} rows, \
                 got {} in {body}",
                page.len()
            );
            seen.extend(
                page.iter()
                    .map(|row| row["id"].as_str().unwrap_or_default().to_string()),
            );
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= names.len(),
                "case maxResults={page_size}: pagination did not terminate"
            );
        }
        assert_eq!(
            seen, created,
            "case maxResults={page_size}: expected every cluster exactly once in creation \
             order, got {seen:?}"
        );
        let expected_pages = created.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case maxResults={page_size}: expected {expected_pages} pages, got {pages}"
        );
    }

    // A token this service did not issue names no page, and is refused rather than
    // served as a first page.
    for token in ["not-hex", "6162", ""] {
        let (status, body) = call(
            &router,
            "ListClusters",
            &json!({"nextToken": token, "maxResults": 1}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case nextToken={token:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"],
            json!("ValidationException"),
            "case nextToken={token:?}: got {body}"
        );
    }

    // A token is scoped to the listing that issued it: the creation sequence is
    // service-wide, so a `ListClusters` token must not resume a `ListQueues` page.
    let (status, page) = call(&router, "ListClusters", &json!({"maxResults": 1})).await;
    assert_eq!(status, 200, "case ListClusters page one: got {page}");
    let cluster_token = page["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !cluster_token.is_empty(),
        "case ListClusters page one: expected a nextToken, got {page}"
    );
    let (status, refused) = call(
        &router,
        "ListQueues",
        &json!({"clusterIdentifier": &created[0], "nextToken": &cluster_token}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a ListClusters token replayed into ListQueues: expected 400, got \
         {status} {refused}"
    );
    assert_eq!(
        refused["__type"],
        json!("ValidationException"),
        "case a ListClusters token replayed into ListQueues: got {refused}"
    );
}

#[tokio::test]
async fn an_idempotency_token_replays_the_first_create_instead_of_conflicting() {
    let router = emulator();

    // Rows: the operation, its first request, and a retry of it. A retry carrying the
    // same `clientToken` returns the first call's resource and has no further effect.
    let cluster_token = "cluster-token-0001";
    let mut first_cluster = create_cluster("demo-cluster", "SMALL");
    first_cluster["clientToken"] = json!(cluster_token);

    let (status, created) = call(&router, "CreateCluster", &first_cluster).await;
    assert_eq!(status, 200, "case first CreateCluster: got {created}");
    let (status, replayed) = call(&router, "CreateCluster", &first_cluster).await;
    assert_eq!(status, 200, "case CreateCluster retry: got {replayed}");
    assert_eq!(
        replayed["cluster"]["id"], created["cluster"]["id"],
        "case CreateCluster retry: expected the first call's cluster, got {replayed}"
    );

    // A DIFFERENT token on the same name is a genuine second create, so it conflicts.
    let mut other = create_cluster("demo-cluster", "SMALL");
    other["clientToken"] = json!("cluster-token-0002");
    let (status, conflict) = call(&router, "CreateCluster", &other).await;
    assert_eq!(
        status, 409,
        "case CreateCluster with a new token on a taken name: got {status} {conflict}"
    );

    wait_past_the_owed_transition().await;
    let cluster_id = created["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let mut first_group = create_compute_node_group(&cluster_id, "demo-cng");
    first_group["clientToken"] = json!("group-token-0001");
    let (status, group) = call(&router, "CreateComputeNodeGroup", &first_group).await;
    assert_eq!(status, 200, "case first CreateComputeNodeGroup: got {group}");
    let (status, group_retry) = call(&router, "CreateComputeNodeGroup", &first_group).await;
    assert_eq!(
        status, 200,
        "case CreateComputeNodeGroup retry: got {group_retry}"
    );
    assert_eq!(
        group_retry["computeNodeGroup"]["id"], group["computeNodeGroup"]["id"],
        "case CreateComputeNodeGroup retry: expected the first call's group, got {group_retry}"
    );

    let mut first_queue = create_queue(&cluster_id, "demo-queue", &[]);
    first_queue["clientToken"] = json!("queue-token-0001");
    let (status, queue) = call(&router, "CreateQueue", &first_queue).await;
    assert_eq!(status, 200, "case first CreateQueue: got {queue}");
    let (status, queue_retry) = call(&router, "CreateQueue", &first_queue).await;
    assert_eq!(status, 200, "case CreateQueue retry: got {queue_retry}");
    assert_eq!(
        queue_retry["queue"]["id"], queue["queue"]["id"],
        "case CreateQueue retry: expected the first call's queue, got {queue_retry}"
    );

    // One cluster, one group, one queue — the retries created nothing.
    let (status, listed) = call(&router, "ListClusters", &json!({})).await;
    assert_eq!(status, 200, "case ListClusters: got {listed}");
    assert_eq!(
        listed["clusters"].as_array().map(Vec::len),
        Some(1),
        "case ListClusters: expected exactly one cluster, got {listed}"
    );
    let (status, queues) = call(
        &router,
        "ListQueues",
        &json!({"clusterIdentifier": &cluster_id}),
    )
    .await;
    assert_eq!(status, 200, "case ListQueues: got {queues}");
    assert_eq!(
        queues["queues"].as_array().map(Vec::len),
        Some(1),
        "case ListQueues: expected exactly one queue, got {queues}"
    );
}

#[tokio::test]
async fn a_missing_required_member_is_refused_before_any_record_is_written() {
    let router = emulator();

    // Rows: each `@required` member of `CreateCluster`, dropped in turn.
    for member in ["clusterName", "scheduler", "size", "networking"] {
        let mut request = create_cluster("demo-cluster", "SMALL");
        request
            .as_object_mut()
            .expect("the request is a JSON object")
            .remove(member);
        let (status, body) = call(&router, "CreateCluster", &request).await;
        assert_eq!(
            status, 400,
            "case {member} absent: expected 400, got {status} {body}"
        );
    }

    let (status, listed) = call(&router, "ListClusters", &json!({})).await;
    assert_eq!(status, 200, "case ListClusters: got {listed}");
    assert_eq!(
        listed["clusters"],
        json!([]),
        "case ListClusters after refused creates: expected no cluster to have been \
         written, got {listed}"
    );
}

#[tokio::test]
async fn an_idempotency_token_cannot_be_replayed_into_another_cluster() {
    let router = emulator();
    let first_cluster = active_cluster(&router, "first-cluster").await;
    let (status, body) = call(
        &router,
        "CreateCluster",
        &create_cluster("second-cluster", "SMALL"),
    )
    .await;
    assert_eq!(status, 200, "case create second-cluster: got {body}");
    let second_cluster = body["cluster"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    wait_past_the_owed_transition().await;

    let token = "shared-token-0001";
    let mut request = create_queue(&first_cluster, "demo-queue", &[]);
    request["clientToken"] = json!(token);
    let (status, created) = call(&router, "CreateQueue", &request).await;
    assert_eq!(status, 200, "case first CreateQueue: got {created}");

    // The token names the first call's queue, in the cluster that call created it in.
    // Replaying it against another cluster must not create a second queue.
    let mut elsewhere = create_queue(&second_cluster, "demo-queue", &[]);
    elsewhere["clientToken"] = json!(token);
    let (status, refused) = call(&router, "CreateQueue", &elsewhere).await;
    assert_eq!(
        status, 409,
        "case replay into another cluster: expected a conflict, got {status} {refused}"
    );
    let (status, queues) = call(
        &router,
        "ListQueues",
        &json!({"clusterIdentifier": &second_cluster}),
    )
    .await;
    assert_eq!(status, 200, "case ListQueues on the other cluster: {queues}");
    assert_eq!(
        queues["queues"],
        json!([]),
        "case replay into another cluster: expected no queue to have been written there, \
         got {queues}"
    );

    // A queue page token is scoped to ITS cluster: the creation sequence is
    // service-wide, so one cluster's token must not resume another cluster's queues.
    let (status, body) = call(
        &router,
        "CreateQueue",
        &create_queue(&first_cluster, "second-queue", &[]),
    )
    .await;
    assert_eq!(status, 200, "case create second-queue: got {body}");
    let (status, page) = call(
        &router,
        "ListQueues",
        &json!({"clusterIdentifier": &first_cluster, "maxResults": 1}),
    )
    .await;
    assert_eq!(status, 200, "case ListQueues page one: got {page}");
    let queue_token = page["nextToken"].as_str().unwrap_or_default().to_string();
    assert!(
        !queue_token.is_empty(),
        "case ListQueues page one: expected a nextToken, got {page}"
    );
    let (status, refused) = call(
        &router,
        "ListQueues",
        &json!({"clusterIdentifier": &second_cluster, "nextToken": &queue_token}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one cluster's queue token replayed into another cluster: expected 400, \
         got {status} {refused}"
    );
}

#[tokio::test]
async fn a_create_past_a_store_ceiling_is_refused_with_the_modelled_quota_error() {
    let router = emulator();

    // The cluster store's ceiling: every create up to it succeeds, the next is refused.
    for index in 0..MAX_CLUSTERS {
        let name = format!("cap-cluster-{index}");
        let (status, body) = call(&router, "CreateCluster", &create_cluster(&name, "SMALL")).await;
        assert_eq!(
            status, 200,
            "case cluster {index} of {MAX_CLUSTERS}: expected 200, got {status} {body}"
        );
    }
    let (status, refused) = call(
        &router,
        "CreateCluster",
        &create_cluster("cap-cluster-over", "SMALL"),
    )
    .await;
    assert_eq!(
        status, 402,
        "case one cluster past the ceiling: expected the modelled quota error, got \
         {status} {refused}"
    );
    assert_eq!(
        refused["__type"],
        json!("ServiceQuotaExceededException"),
        "case one cluster past the ceiling: got {refused}"
    );
}

#[tokio::test]
async fn a_child_create_past_its_cluster_ceiling_is_refused_with_the_modelled_quota_error() {
    let router = emulator();
    let cluster = active_cluster(&router, "cap-cluster").await;

    // Rows: each per-cluster ceiling, with the create that fills it and the create that
    // goes one past.
    for index in 0..MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER {
        let name = format!("cap-cng-{index}");
        let (status, body) = call(
            &router,
            "CreateComputeNodeGroup",
            &create_compute_node_group(&cluster, &name),
        )
        .await;
        assert_eq!(
            status, 200,
            "case compute node group {index} of {MAX_COMPUTE_NODE_GROUPS_PER_CLUSTER}: \
             expected 200, got {status} {body}"
        );
    }
    let (status, refused) = call(
        &router,
        "CreateComputeNodeGroup",
        &create_compute_node_group(&cluster, "cap-cng-over"),
    )
    .await;
    assert_eq!(
        status, 402,
        "case one compute node group past the ceiling: got {status} {refused}"
    );

    for index in 0..MAX_QUEUES_PER_CLUSTER {
        let name = format!("cap-queue-{index}");
        let (status, body) = call(&router, "CreateQueue", &create_queue(&cluster, &name, &[])).await;
        assert_eq!(
            status, 200,
            "case queue {index} of {MAX_QUEUES_PER_CLUSTER}: expected 200, got {status} {body}"
        );
    }
    let (status, refused) = call(
        &router,
        "CreateQueue",
        &create_queue(&cluster, "cap-queue-over", &[]),
    )
    .await;
    assert_eq!(
        status, 402,
        "case one queue past the ceiling: got {status} {refused}"
    );
    assert_eq!(
        refused["__type"],
        json!("ServiceQuotaExceededException"),
        "case one queue past the ceiling: got {refused}"
    );
}

#[tokio::test]
async fn a_persisted_list_is_accepted_at_its_bound_and_refused_past_it() {
    let router = emulator();

    // Rows: the published bound and one past it, on a list the model itself does not
    // bound, so the ceiling is the one `crate::state::MAX_STORED_LIST_ENTRIES` sets.
    let cases: [(usize, u16); 2] = [
        (MAX_STORED_LIST_ENTRIES, 200),
        (MAX_STORED_LIST_ENTRIES + 1, 400),
    ];
    for (entries, expected) in cases {
        let subnets: Vec<String> = (0..entries)
            .map(|index| format!("subnet-{index:017}"))
            .collect();
        let mut request = create_cluster(&format!("list-cluster-{entries}"), "SMALL");
        request["networking"]["subnetIds"] = json!(subnets);
        let (status, body) = call(&router, "CreateCluster", &request).await;
        assert_eq!(
            status, expected,
            "case {entries} subnetIds: expected {expected}, got {status} {body}"
        );
        if expected == 200 {
            assert_eq!(
                body["cluster"]["networking"]["subnetIds"]
                    .as_array()
                    .map(Vec::len),
                Some(entries),
                "case {entries} subnetIds: expected every entry to be stored, got {body}"
            );
        } else {
            assert_eq!(
                body["__type"],
                json!("ValidationException"),
                "case {entries} subnetIds: got {body}"
            );
        }
    }
}
