// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DAX control-plane slice: the subnet group a cluster needs,
//! and the cluster lifecycle (create, describe, grow, update, delete) over the wire.
//!
//! Every case drives `POST /` with `X-Amz-Target: AmazonDAXV3.<Operation>` through the
//! same `MultiServiceRouter` the production router builds, so what is asserted is the
//! HTTP behaviour a DAX SDK client sees — not an internal function call.
//!
//! The tables below span the CLASS each documented rule covers; the names and values the
//! bootstrap issue happened to cite are one row each.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dax_amazondaxv3::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for; it must show up in the ARNs and the
/// endpoint hostnames the service mints.
const REGION: &str = "us-west-2";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A role ARN to pass as the required `IamRoleArn`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/DAXRole";

/// A node type to pass as the required `NodeType`.
const NODE_TYPE: &str = "dax.r4.large";

/// Wait until every clock-owed status move started before the call is due.
async fn settle() {
    tokio::time::sleep(state::STATUS_SETTLE_DELAY + Duration::from_millis(400)).await;
}

/// A router carrying only this service, over its own engine, so each test starts from an
/// empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and parsed JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonDAXV3.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/dax/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failed call: the `__type` member, without any namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A `CreateCluster` body with the required members filled in.
fn create_cluster_body(name: &str, replication_factor: i64) -> Value {
    json!({
        "ClusterName": name,
        "NodeType": NODE_TYPE,
        "ReplicationFactor": replication_factor,
        "IamRoleArn": ROLE_ARN,
    })
}

/// The subnets that EXIST in `REGION`'s network — the only identifiers a subnet group
/// may name (`state::region_subnets`).
fn existing_subnets() -> Vec<String> {
    state::region_subnets(REGION)
        .into_iter()
        .map(|subnet| subnet.subnet_id)
        .collect()
}

/// One subnet that exists in `REGION`.
fn a_subnet() -> String {
    state::region_subnets(REGION)
        .first()
        .expect("a Region has subnets")
        .subnet_id
        .clone()
}

/// Create a cluster and drive it to `available`, returning its describe view.
async fn available_cluster(router: &MultiServiceRouter, name: &str, nodes: i64) -> Value {
    let (status, body) = call(router, "CreateCluster", create_cluster_body(name, nodes)).await;
    assert_eq!(status, 200, "CreateCluster {name}: got {body:?}");
    settle().await;
    let (status, body) = call(
        router,
        "DescribeClusters",
        json!({ "ClusterNames": [name] }),
    )
    .await;
    assert_eq!(status, 200, "DescribeClusters {name}: got {body:?}");
    body["Clusters"][0].clone()
}

/// The `NodeId`s a describe view reports.
fn node_ids(cluster: &Value) -> Vec<String> {
    cluster["Nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .map(|node| node["NodeId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The whole happy path, as one round trip through persisted state.
// ---------------------------------------------------------------------------

/// Every value the lifecycle reports is read back out of the store: the subnet group
/// resolves the cluster's placement, the cluster's ARN/endpoints/nodes survive a
/// describe, a grow and an update are visible to the next describe, and a delete
/// eventually frees the name.
#[tokio::test]
async fn the_cluster_lifecycle_round_trips_through_persisted_state() {
    let router = emulator();

    // 1. CreateSubnetGroup, read back through DescribeSubnetGroups.
    let subnets = existing_subnets();
    let (status, created) = call(
        &router,
        "CreateSubnetGroup",
        json!({
            "SubnetGroupName": "dax-subnets",
            "Description": "two subnets",
            "SubnetIds": [subnets[0], subnets[1]],
        }),
    )
    .await;
    assert_eq!(status, 200, "CreateSubnetGroup: got {created:?}");
    let (status, described) = call(
        &router,
        "DescribeSubnetGroups",
        json!({ "SubnetGroupNames": ["dax-subnets"] }),
    )
    .await;
    assert_eq!(status, 200, "DescribeSubnetGroups: got {described:?}");
    assert_eq!(
        described["SubnetGroups"][0], created["SubnetGroup"],
        "the stored subnet group must read back exactly as created, got {described:?}"
    );
    let subnets = created["SubnetGroup"]["Subnets"]
        .as_array()
        .expect("Subnets is a list");
    assert_eq!(
        subnets.len(),
        2,
        "both subnets must be reported, got {subnets:?}"
    );

    // 2. CreateCluster against that subnet group.
    let mut body = create_cluster_body("demo", 3);
    body["SubnetGroupName"] = json!("dax-subnets");
    body["Description"] = json!("initial");
    body["SecurityGroupIds"] = json!(["sg-1"]);
    let (status, created) = call(&router, "CreateCluster", body).await;
    assert_eq!(status, 200, "CreateCluster: got {created:?}");
    let cluster = &created["Cluster"];
    assert_eq!(
        cluster["ClusterArn"],
        json!(format!("arn:aws:dax:{REGION}:{ACCOUNT}:cache/demo")),
        "the ARN must name the signed region and the emulator account, got {cluster:?}"
    );
    assert_eq!(
        cluster["Status"],
        json!(state::CLUSTER_CREATING),
        "a freshly created cluster is not yet available, got {cluster:?}"
    );
    assert_eq!(
        cluster["TotalNodes"],
        json!(3),
        "TotalNodes must equal ReplicationFactor, got {cluster:?}"
    );
    assert_eq!(
        cluster["ActiveNodes"],
        json!(0),
        "no node serves requests while the cluster is creating, got {cluster:?}"
    );
    assert_eq!(
        cluster["SubnetGroup"],
        json!("dax-subnets"),
        "the cluster must report the subnet group it was placed in, got {cluster:?}"
    );
    assert_eq!(
        cluster["SecurityGroups"][0]["SecurityGroupIdentifier"],
        json!("sg-1"),
        "the requested security group must be reported, got {cluster:?}"
    );
    assert_eq!(
        cluster["IamRoleArn"],
        json!(ROLE_ARN),
        "the role DAX assumes must be reported, got {cluster:?}"
    );
    let created_nodes = node_ids(cluster);
    assert_eq!(
        created_nodes.len(),
        3,
        "one node per replication factor, got {created_nodes:?}"
    );
    assert_eq!(
        created_nodes.iter().collect::<BTreeSet<_>>().len(),
        3,
        "node ids must be distinct, got {created_nodes:?}"
    );

    // 3. While creating, DescribeClusters reports cluster-level detail only.
    let (status, described) = call(&router, "DescribeClusters", json!({})).await;
    assert_eq!(status, 200, "DescribeClusters: got {described:?}");
    let creating = &described["Clusters"][0];
    assert_eq!(
        creating["TotalNodes"],
        json!(3),
        "TotalNodes is cluster-level and stays reported, got {creating:?}"
    );
    assert_eq!(
        node_ids(creating),
        Vec::<String>::new(),
        "a creating cluster shows only cluster level information, got {creating:?}"
    );

    // 4. The clock settles it: available, with every node and its endpoint.
    settle().await;
    let (status, described) = call(&router, "DescribeClusters", json!({})).await;
    assert_eq!(status, 200, "DescribeClusters: got {described:?}");
    let available = &described["Clusters"][0];
    assert_eq!(
        available["Status"],
        json!(state::CLUSTER_AVAILABLE),
        "the clock owes the cluster its move to available, got {available:?}"
    );
    assert_eq!(
        available["ActiveNodes"],
        json!(3),
        "every settled node serves requests, got {available:?}"
    );
    assert_eq!(
        available["ClusterArn"], cluster["ClusterArn"],
        "the describe must report the stored ARN, got {available:?}"
    );
    assert_eq!(
        available["NodeType"],
        json!(NODE_TYPE),
        "the describe must report the stored node type, got {available:?}"
    );
    assert_eq!(
        node_ids(available),
        created_nodes,
        "the describe must report the stored nodes, got {available:?}"
    );
    for node in available["Nodes"].as_array().expect("Nodes is a list") {
        assert!(
            node["Endpoint"]["Address"]
                .as_str()
                .unwrap_or_default()
                .contains(REGION),
            "a settled node must carry an endpoint in the signed region, got {node:?}"
        );
        assert!(
            node["AvailabilityZone"]
                .as_str()
                .unwrap_or_default()
                .starts_with(REGION),
            "a node must be placed in a zone of the signed region, got {node:?}"
        );
    }

    // 5. IncreaseReplicationFactor grows the stored cluster.
    let (status, grown) = call(
        &router,
        "IncreaseReplicationFactor",
        json!({ "ClusterName": "demo", "NewReplicationFactor": 5 }),
    )
    .await;
    assert_eq!(status, 200, "IncreaseReplicationFactor: got {grown:?}");
    settle().await;
    let (status, described) = call(&router, "DescribeClusters", json!({})).await;
    assert_eq!(status, 200, "DescribeClusters: got {described:?}");
    let grown = &described["Clusters"][0];
    assert_eq!(
        grown["TotalNodes"],
        json!(5),
        "the grown cluster must report the new node count, got {grown:?}"
    );
    let grown_nodes = node_ids(grown);
    assert_eq!(
        grown_nodes.iter().collect::<BTreeSet<_>>().len(),
        5,
        "the added node ids must be distinct from the existing ones, got {grown_nodes:?}"
    );
    assert!(
        created_nodes.iter().all(|id| grown_nodes.contains(id)),
        "growing a cluster must keep its existing nodes, got {grown_nodes:?}"
    );

    // 6. UpdateCluster mutates only what it was given.
    let (status, updated) = call(
        &router,
        "UpdateCluster",
        json!({
            "ClusterName": "demo",
            "Description": "updated",
            "PreferredMaintenanceWindow": "sun:01:00-sun:09:00",
        }),
    )
    .await;
    assert_eq!(status, 200, "UpdateCluster: got {updated:?}");
    let (status, described) = call(&router, "DescribeClusters", json!({})).await;
    assert_eq!(status, 200, "DescribeClusters: got {described:?}");
    let modified = &described["Clusters"][0];
    assert_eq!(
        modified["Description"],
        json!("updated"),
        "the new description must be persisted, got {modified:?}"
    );
    assert_eq!(
        modified["PreferredMaintenanceWindow"],
        json!("sun:01:00-sun:09:00"),
        "the new maintenance window must be persisted, got {modified:?}"
    );
    assert_eq!(
        modified["SubnetGroup"],
        json!("dax-subnets"),
        "an unsupplied member must keep its stored value, got {modified:?}"
    );
    assert_eq!(
        modified["TotalNodes"],
        json!(5),
        "an update must not change the node count, got {modified:?}"
    );

    // 7. DeleteCluster: deleting first, then gone, then the name is free again.
    settle().await;
    let (status, deleted) = call(&router, "DeleteCluster", json!({ "ClusterName": "demo" })).await;
    assert_eq!(status, 200, "DeleteCluster: got {deleted:?}");
    assert_eq!(
        deleted["Cluster"]["Status"],
        json!(state::CLUSTER_DELETING),
        "a deleted cluster is reported in a deleting status, got {deleted:?}"
    );
    let (status, described) = call(
        &router,
        "DescribeClusters",
        json!({ "ClusterNames": ["demo"] }),
    )
    .await;
    assert_eq!(status, 200, "DescribeClusters while deleting: got {described:?}");
    assert_eq!(
        described["Clusters"][0]["Status"],
        json!(state::CLUSTER_DELETING),
        "the cluster is still reported until the clock removes it, got {described:?}"
    );
    settle().await;
    let (status, gone) = call(
        &router,
        "DescribeClusters",
        json!({ "ClusterNames": ["demo"] }),
    )
    .await;
    assert_eq!(status, 404, "DescribeClusters after deletion: got {gone:?}");
    assert_eq!(
        error_code(&gone),
        "ClusterNotFoundFault",
        "a removed cluster is not found, got {gone:?}"
    );
    let (status, recreated) = call(&router, "CreateCluster", create_cluster_body("demo", 1)).await;
    assert_eq!(
        status, 200,
        "the name must be reusable once the deletion settled, got {recreated:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule: a DAX resource name is stored, and reported, lower-cased.
// Source: `CreateCluster$ClusterName` ("This parameter is stored as a lowercase
// string") and `CreateSubnetGroup$SubnetGroupName` ("This value is stored as a
// lowercase string").
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_resource_name_is_stored_lower_cased_and_resolves_case_insensitively() {
    // Rows differ in WHERE the upper case sits: all of it, the first letter, the middle,
    // and a name that is already lower case (the identity row).
    let cases = [
        ("MYCLUSTER", "mycluster"),
        ("Demo", "demo"),
        ("mixedCASEname", "mixedcasename"),
        ("already-lower", "already-lower"),
    ];

    for (given, expected) in cases {
        let router = emulator();

        let (status, body) = call(
            &router,
            "CreateSubnetGroup",
            json!({ "SubnetGroupName": given, "SubnetIds": [a_subnet()] }),
        )
        .await;
        assert_eq!(status, 200, "case {given:?}: CreateSubnetGroup got {body:?}");
        assert_eq!(
            body["SubnetGroup"]["SubnetGroupName"],
            json!(expected),
            "case {given:?}: expected subnet group name {expected:?}, got {body:?}"
        );

        let (status, body) = call(&router, "CreateCluster", create_cluster_body(given, 1)).await;
        assert_eq!(status, 200, "case {given:?}: CreateCluster got {body:?}");
        assert_eq!(
            body["Cluster"]["ClusterName"],
            json!(expected),
            "case {given:?}: expected cluster name {expected:?}, got {body:?}"
        );
        assert_eq!(
            body["Cluster"]["ClusterArn"],
            json!(format!("arn:aws:dax:{REGION}:{ACCOUNT}:cache/{expected}")),
            "case {given:?}: the ARN must carry the lower-cased name, got {body:?}"
        );

        // And the lower-cased resource is the one a differently-cased request resolves.
        let (status, body) = call(
            &router,
            "DescribeClusters",
            json!({ "ClusterNames": [given.to_uppercase()] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {given:?}: a describe by any casing must resolve the cluster, got {body:?}"
        );
        assert_eq!(
            body["Clusters"][0]["ClusterName"],
            json!(expected),
            "case {given:?}: expected {expected:?}, got {body:?}"
        );
        let (status, body) = call(
            &router,
            "DescribeSubnetGroups",
            json!({ "SubnetGroupNames": [given.to_uppercase()] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {given:?}: a describe by any casing must resolve the subnet group, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: a `ClusterName` must be 1..=20 alphanumeric characters or hyphens, start
// with a letter, not end with a hyphen and not contain two consecutive hyphens;
// anything else is refused with `InvalidParameterValueException` (HTTP 400) and
// nothing is stored.
// Source: `CreateCluster$ClusterName`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_name_is_accepted_only_when_it_satisfies_the_documented_grammar() {
    // One row per clause of the rule, each at the boundary it tests: the published
    // length bound (20) and one past it (21), the first-character clause, the
    // trailing-hyphen clause, the consecutive-hyphen clause and the character set.
    let accepted = ["a", "abcdefghij1234567890", "my-cluster-1", "a-1"];
    let refused = [
        "abcdefghij12345678901",
        "1abc",
        "-abc",
        "abc-",
        "a--b",
        "ab_c",
        "ab.c",
    ];

    let router = emulator();
    for name in accepted {
        let (status, body) = call(&router, "CreateCluster", create_cluster_body(name, 1)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the name to be accepted, got {status} {body:?}"
        );
    }
    for name in refused {
        let (status, body) = call(&router, "CreateCluster", create_cluster_body(name, 1)).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected the name to be refused, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {name:?}: expected InvalidParameterValueException, got {body:?}"
        );
        let (status, described) = call(
            &router,
            "DescribeClusters",
            json!({ "ClusterNames": [name.to_lowercase()] }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {name:?}: a refused create must store nothing, got {status} {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: a cluster holds one node per replication factor, from 1 up to the
// documented maximum of 10; a factor below 1 is refused with
// `InvalidParameterValueException` and one above the maximum with
// `NodeQuotaForClusterExceededFault`.
// Source: `CreateCluster$ReplicationFactor` ("A replication factor of 1 will create a
// single-node cluster ... a number between 3 (one primary and two read replicas) and 10
// (one primary and nine read replicas)").
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_cluster_holds_one_distinct_node_per_replication_factor() {
    // Rows: the published lower bound, the doc's own example span, the published upper
    // bound, and one past each bound.
    let accepted = [1_i64, 2, 3, 10];
    let refused: [(i64, &str); 3] = [
        (0, "InvalidParameterValueException"),
        (-1, "InvalidParameterValueException"),
        (11, "NodeQuotaForClusterExceededFault"),
    ];

    let router = emulator();
    for factor in accepted {
        let name = format!("cluster-{factor}");
        let cluster = available_cluster(&router, &name, factor).await;
        assert_eq!(
            cluster["TotalNodes"],
            json!(factor),
            "case {factor}: expected TotalNodes {factor}, got {cluster:?}"
        );
        assert_eq!(
            cluster["ActiveNodes"],
            json!(factor),
            "case {factor}: every settled node is active, got {cluster:?}"
        );
        let ids = node_ids(&cluster);
        assert_eq!(
            ids.len(),
            usize::try_from(factor).unwrap(),
            "case {factor}: expected {factor} nodes, got {ids:?}"
        );
        assert_eq!(
            ids.iter().collect::<BTreeSet<_>>().len(),
            ids.len(),
            "case {factor}: node ids must be distinct, got {ids:?}"
        );
    }
    for (factor, expected) in refused {
        let (status, body) = call(
            &router,
            "CreateCluster",
            create_cluster_body("refused-factor", factor),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {factor}: expected a refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {factor}: expected {expected}, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: an endpoint's URL scheme and port follow the cluster's endpoint encryption
// type — `dax` on 8111 without TLS, `daxs` on 9111 with it — for the cluster's
// discovery endpoint and for every node endpoint alike.
// Source: `Endpoint$URL` ("The default ports are 8111 for the \"dax\" protocol and 9111
// for the \"daxs\" protocol") and `ClusterEndpointEncryptionType`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_endpoint_speaks_the_protocol_its_encryption_type_selects() {
    // Rows: the member omitted (the default), each of the two values the enum lists.
    let cases: [(Option<&str>, &str, i64); 3] = [
        (None, state::DAX_SCHEME, state::DAX_PORT as i64),
        (Some("NONE"), state::DAX_SCHEME, state::DAX_PORT as i64),
        (Some("TLS"), state::DAXS_SCHEME, state::DAXS_PORT as i64),
    ];

    let router = emulator();
    for (index, (encryption, scheme, port)) in cases.into_iter().enumerate() {
        let name = format!("enc-{index}");
        let mut body = create_cluster_body(&name, 2);
        if let Some(encryption) = encryption {
            body["ClusterEndpointEncryptionType"] = json!(encryption);
        }
        let (status, created) = call(&router, "CreateCluster", body).await;
        assert_eq!(
            status, 200,
            "case {encryption:?}: CreateCluster got {created:?}"
        );
        settle().await;
        let (status, described) = call(
            &router,
            "DescribeClusters",
            json!({ "ClusterNames": [name] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {encryption:?}: DescribeClusters got {described:?}"
        );
        let cluster = &described["Clusters"][0];

        let discovery = &cluster["ClusterDiscoveryEndpoint"];
        assert_eq!(
            discovery["Port"],
            json!(port),
            "case {encryption:?}: expected discovery port {port}, got {discovery:?}"
        );
        assert!(
            discovery["URL"]
                .as_str()
                .unwrap_or_default()
                .starts_with(&format!("{scheme}://")),
            "case {encryption:?}: expected a {scheme} discovery URL, got {discovery:?}"
        );
        for node in cluster["Nodes"].as_array().expect("Nodes is a list") {
            assert_eq!(
                node["Endpoint"]["Port"],
                json!(port),
                "case {encryption:?}: expected node port {port}, got {node:?}"
            );
            assert!(
                node["Endpoint"]["URL"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with(&format!("{scheme}://")),
                "case {encryption:?}: expected a {scheme} node URL, got {node:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule: a paginated DAX describe accepts a `MaxResults` inside the documented
// 20..=100 window and refuses anything outside it with HTTP 400; a page it truncates
// carries a continuation token that returns the rest exactly once.
// Source: `DescribeClustersRequest$MaxResults` and
// `DescribeSubnetGroupsRequest$MaxResults` ("The value for MaxResults must be between 20
// and 100").
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_paginated_describe_honours_the_documented_max_results_window() {
    let router = emulator();
    // Rows for the window: each published bound, one past each bound.
    let accepted = [20_i64, 21, 99, 100];
    let refused = [19_i64, 0, -1, 101, 1000];

    for operation in ["DescribeClusters", "DescribeSubnetGroups"] {
        for max_results in accepted {
            let (status, body) = call(&router, operation, json!({ "MaxResults": max_results })).await;
            assert_eq!(
                status, 200,
                "case {operation} MaxResults={max_results}: expected it accepted, got {status} {body:?}"
            );
        }
        for max_results in refused {
            let (status, body) = call(&router, operation, json!({ "MaxResults": max_results })).await;
            assert_eq!(
                status, 400,
                "case {operation} MaxResults={max_results}: expected a refusal, got {status} {body:?}"
            );
        }
    }
}

/// Rule: a `NextToken` this service did not issue is refused with HTTP 400, never
/// answered with some arbitrary page and a 200.
#[tokio::test]
async fn a_continuation_token_this_service_did_not_issue_is_refused() {
    let router = emulator();
    // Rows: not base64, base64 of non-UTF-8 bytes, base64 of the empty string.
    let tokens = ["not-a-token!!", "/w==", ""];

    for operation in ["DescribeClusters", "DescribeSubnetGroups"] {
        for token in tokens {
            let (status, body) = call(
                &router,
                operation,
                json!({ "MaxResults": 20, "NextToken": token }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {operation} NextToken={token:?}: expected a refusal, got {status} {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule: each store this service keeps is BOUNDED — a create past the bound is refused
// with the modelled quota error for that resource, so no sequence of requests grows the
// emulator's memory without limit.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_store_is_bounded_and_a_create_past_the_bound_is_refused() {
    // Rows: the cluster count bound, the subnet group count bound, the subnets-per-group
    // bound and the tags-per-cluster bound. Each is filled to its bound (accepted) and
    // then pushed one past it (refused with that resource's quota error).
    let router = emulator();

    for index in 0..state::MAX_CLUSTERS {
        let name = format!("quota-{index:02}");
        let (status, body) = call(&router, "CreateCluster", create_cluster_body(&name, 1)).await;
        assert_eq!(
            status, 200,
            "case cluster {index}: a create inside the bound is accepted, got {status} {body:?}"
        );
    }
    let (status, body) = call(
        &router,
        "CreateCluster",
        create_cluster_body("quota-over", 1),
    )
    .await;
    assert_eq!(
        status, 400,
        "case cluster bound: expected a refusal past {}, got {status} {body:?}",
        state::MAX_CLUSTERS
    );
    assert_eq!(
        error_code(&body),
        "ClusterQuotaForCustomerExceededFault",
        "case cluster bound: expected ClusterQuotaForCustomerExceededFault, got {body:?}"
    );

    for index in 0..state::MAX_SUBNET_GROUPS {
        let name = format!("sgquota-{index:02}");
        let (status, body) = call(
            &router,
            "CreateSubnetGroup",
            json!({ "SubnetGroupName": name, "SubnetIds": [a_subnet()] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case subnet group {index}: a create inside the bound is accepted, got {status} {body:?}"
        );
    }
    let (status, body) = call(
        &router,
        "CreateSubnetGroup",
        json!({ "SubnetGroupName": "sgquota-over", "SubnetIds": [a_subnet()] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case subnet group bound: expected a refusal past {}, got {status} {body:?}",
        state::MAX_SUBNET_GROUPS
    );
    assert_eq!(
        error_code(&body),
        "SubnetGroupQuotaExceededFault",
        "case subnet group bound: expected SubnetGroupQuotaExceededFault, got {body:?}"
    );

    // The subnets-per-group and tags-per-cluster bounds, on a fresh store. Every
    // identifier must name a subnet that EXISTS, so the rows fill the quota by
    // repeating one rather than by inventing identifiers.
    let router = emulator();
    let at_bound: Vec<String> = (0..state::MAX_SUBNETS_PER_GROUP)
        .map(|_| a_subnet())
        .collect();
    let (status, body) = call(
        &router,
        "CreateSubnetGroup",
        json!({ "SubnetGroupName": "at-bound", "SubnetIds": at_bound }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {} subnets: expected it accepted at the bound, got {status} {body:?}",
        state::MAX_SUBNETS_PER_GROUP
    );
    let past_bound: Vec<String> = (0..=state::MAX_SUBNETS_PER_GROUP)
        .map(|_| a_subnet())
        .collect();
    let (status, body) = call(
        &router,
        "CreateSubnetGroup",
        json!({ "SubnetGroupName": "past-bound", "SubnetIds": past_bound }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {} subnets: expected a refusal past the bound, got {status} {body:?}",
        state::MAX_SUBNETS_PER_GROUP + 1
    );
    assert_eq!(
        error_code(&body),
        "SubnetQuotaExceededFault",
        "case subnets-per-group bound: expected SubnetQuotaExceededFault, got {body:?}"
    );

    let tags = |count: usize| -> Value {
        json!((0..count)
            .map(|index| json!({ "Key": format!("k{index}"), "Value": "v" }))
            .collect::<Vec<Value>>())
    };
    let mut body = create_cluster_body("tags-at-bound", 1);
    body["Tags"] = tags(state::MAX_TAGS_PER_RESOURCE);
    let (status, answered) = call(&router, "CreateCluster", body).await;
    assert_eq!(
        status, 200,
        "case {} tags: expected it accepted at the bound, got {status} {answered:?}",
        state::MAX_TAGS_PER_RESOURCE
    );
    let mut body = create_cluster_body("tags-past-bound", 1);
    body["Tags"] = tags(state::MAX_TAGS_PER_RESOURCE + 1);
    let (status, answered) = call(&router, "CreateCluster", body).await;
    assert_eq!(
        status, 400,
        "case {} tags: expected a refusal past the bound, got {status} {answered:?}",
        state::MAX_TAGS_PER_RESOURCE + 1
    );
    assert_eq!(
        error_code(&answered),
        "TagQuotaPerResourceExceeded",
        "case tags-per-cluster bound: expected TagQuotaPerResourceExceeded, got {answered:?}"
    );
}

#[tokio::test]
async fn a_truncated_page_continues_through_its_next_token() {
    let router = emulator();
    let total = 25_usize;
    for index in 0..total {
        let name = format!("page-{index:02}");
        let (status, body) = call(&router, "CreateCluster", create_cluster_body(&name, 1)).await;
        assert_eq!(status, 200, "case {name}: CreateCluster got {body:?}");
        let (status, body) = call(
            &router,
            "CreateSubnetGroup",
            json!({ "SubnetGroupName": name, "SubnetIds": [a_subnet()] }),
        )
        .await;
        assert_eq!(status, 200, "case {name}: CreateSubnetGroup got {body:?}");
    }

    // Rows: both paginated describes, each over the same 25-resource store.
    let cases = [
        ("DescribeClusters", "Clusters", "ClusterName"),
        ("DescribeSubnetGroups", "SubnetGroups", "SubnetGroupName"),
    ];
    for (operation, list, name_member) in cases {
        let (status, first) = call(&router, operation, json!({ "MaxResults": 20 })).await;
        assert_eq!(status, 200, "case {operation}: first page got {first:?}");
        let page = first[list].as_array().expect("a list of resources");
        assert_eq!(
            page.len(),
            20,
            "case {operation}: expected a full first page, got {first:?}"
        );
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {operation}: a truncated page must carry a token, got {first:?}"))
            .to_string();

        let (status, second) = call(
            &router,
            operation,
            json!({ "MaxResults": 20, "NextToken": token }),
        )
        .await;
        assert_eq!(status, 200, "case {operation}: second page got {second:?}");
        let rest = second[list].as_array().expect("a list of resources");
        assert_eq!(
            rest.len(),
            total - 20,
            "case {operation}: expected the remainder, got {second:?}"
        );
        assert!(
            second["NextToken"].is_null(),
            "case {operation}: the last page carries no token, got {second:?}"
        );

        let seen: BTreeSet<String> = page
            .iter()
            .chain(rest.iter())
            .map(|item| item[name_member].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            seen.len(),
            total,
            "case {operation}: the pages must cover every resource exactly once, got {seen:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: a cluster modification is accepted only while the cluster is `available`;
// otherwise it is refused with `InvalidClusterStateFault` ("The requested DAX cluster
// is not in the available state") and the store is left alone.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_modification_is_refused_while_the_cluster_is_not_available() {
    // Rows: every operation that modifies a stored cluster, over both non-available
    // statuses a cluster passes through.
    let modifications: [(&str, Value); 3] = [
        ("UpdateCluster", json!({ "Description": "x" })),
        ("DeleteCluster", json!({})),
        (
            "IncreaseReplicationFactor",
            json!({ "NewReplicationFactor": 4 }),
        ),
    ];

    for (index, (operation, extra)) in modifications.into_iter().enumerate() {
        for transient in [state::CLUSTER_CREATING, state::CLUSTER_DELETING] {
            let router = emulator();
            let name = format!("state-{index}");
            let (status, body) = call(&router, "CreateCluster", create_cluster_body(&name, 3)).await;
            assert_eq!(status, 200, "case {operation}/{transient}: got {body:?}");
            if transient == state::CLUSTER_DELETING {
                settle().await;
                let (status, body) =
                    call(&router, "DeleteCluster", json!({ "ClusterName": name })).await;
                assert_eq!(
                    status, 200,
                    "case {operation}/{transient}: DeleteCluster got {body:?}"
                );
            }

            let mut request = extra.clone();
            request["ClusterName"] = json!(name);
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {operation}/{transient}: expected a refusal, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidClusterStateFault",
                "case {operation}/{transient}: expected InvalidClusterStateFault, got {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule: a create of a name that already names a resource is refused as already
// existing, and a create that references a resource that does not exist is refused as
// not found; neither changes the store.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_name_collision_and_a_dangling_reference_are_refused() {
    let router = emulator();

    // Rows: each of the two resource kinds collides on its own name.
    let collisions: [(&str, Value, &str); 2] = [
        (
            "CreateSubnetGroup",
            json!({ "SubnetGroupName": "collide", "SubnetIds": [a_subnet()] }),
            "SubnetGroupAlreadyExistsFault",
        ),
        (
            "CreateCluster",
            create_cluster_body("collide", 1),
            "ClusterAlreadyExistsFault",
        ),
    ];
    for (operation, body, expected) in collisions {
        let (status, first) = call(&router, operation, body.clone()).await;
        assert_eq!(status, 200, "case {operation}: first create got {first:?}");
        let (status, again) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {operation}: expected a refusal, got {status} {again:?}"
        );
        assert_eq!(
            error_code(&again),
            expected,
            "case {operation}: expected {expected}, got {again:?}"
        );
    }

    // A cluster may only be placed in a subnet group that exists.
    let mut body = create_cluster_body("dangling", 1);
    body["SubnetGroupName"] = json!("no-such-group");
    let (status, refused) = call(&router, "CreateCluster", body).await;
    assert_eq!(
        status, 404,
        "an unknown subnet group is not found, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "SubnetGroupNotFoundFault",
        "expected SubnetGroupNotFoundFault, got {refused:?}"
    );
    let (status, described) = call(
        &router,
        "DescribeClusters",
        json!({ "ClusterNames": ["dangling"] }),
    )
    .await;
    assert_eq!(
        status, 404,
        "a refused create must store nothing, got {status} {described:?}"
    );

    // A group must name at least one subnet.
    let (status, refused) = call(
        &router,
        "CreateSubnetGroup",
        json!({ "SubnetGroupName": "empty", "SubnetIds": [] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a subnet group with no subnet is refused, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidSubnet",
        "expected InvalidSubnet, got {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule: a subnet group reports, for every subnet it names, the VPC and the
// Availability Zone the REGION'S NETWORK holds for that subnet — so the group's VpcId
// is the VPC its subnets belong to, and one identifier always reports one zone,
// whatever the order of the request or the repeats in it.
// Source: `state::region_subnets` / `state::default_vpc_id` (the Region's default-VPC
// subnets), reported through `SubnetGroup$VpcId` and `Subnet$SubnetAvailabilityZone`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_subnet_groups_vpc_and_zones_come_from_the_regions_network() {
    let router = emulator();
    let known = state::region_subnets(REGION);
    let ids: Vec<String> = known
        .iter()
        .map(|subnet| subnet.subnet_id.clone())
        .collect();

    // Rows: every subnet of the Region, the same set in the opposite order, a set with
    // a repeated identifier, and a single subnet.
    let cases: [(&str, Vec<String>); 4] = [
        ("group-a", ids.clone()),
        ("group-b", ids.iter().rev().cloned().collect()),
        (
            "group-c",
            vec![ids[0].clone(), ids[1].clone(), ids[1].clone()],
        ),
        ("group-d", vec![ids[2].clone()]),
    ];

    for (name, subnet_ids) in cases {
        let (status, body) = call(
            &router,
            "CreateSubnetGroup",
            json!({ "SubnetGroupName": name, "SubnetIds": subnet_ids }),
        )
        .await;
        assert_eq!(status, 200, "case {name}: CreateSubnetGroup got {body:?}");
        let group = &body["SubnetGroup"];
        assert_eq!(
            group["VpcId"],
            json!(state::default_vpc_id(REGION)),
            "case {name}: the group reports the VPC its subnets belong to, got {group:?}"
        );
        for subnet in group["Subnets"].as_array().expect("Subnets is a list") {
            let id = subnet["SubnetIdentifier"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let expected = known
                .iter()
                .find(|candidate| candidate.subnet_id == id)
                .map(|candidate| candidate.availability_zone.clone());
            assert_eq!(
                subnet["SubnetAvailabilityZone"].as_str().map(str::to_string),
                expected,
                "case {name}: subnet {id} must report its own zone, got {subnet:?}"
            );
        }
    }

    // The zone one identifier reports is the same in every group that names it.
    let zone_of = |group: &Value, id: &str| -> Option<String> {
        group["Subnets"].as_array()?.iter().find_map(|subnet| {
            if subnet["SubnetIdentifier"] == json!(id) {
                Some(subnet["SubnetAvailabilityZone"].as_str()?.to_string())
            } else {
                None
            }
        })
    };
    let (_, a) = call(
        &router,
        "DescribeSubnetGroups",
        json!({ "SubnetGroupNames": ["group-a"] }),
    )
    .await;
    let (_, b) = call(
        &router,
        "DescribeSubnetGroups",
        json!({ "SubnetGroupNames": ["group-b"] }),
    )
    .await;
    assert_eq!(
        zone_of(&a["SubnetGroups"][0], &ids[0]),
        zone_of(&b["SubnetGroups"][0], &ids[0]),
        "one subnet identifier always reports one zone, got {a:?} and {b:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule: an operation naming a cluster that does not exist is answered with
// `ClusterNotFoundFault` (HTTP 404), whatever the operation.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unknown_cluster_is_not_found_by_every_operation_that_names_one() {
    let router = emulator();
    // Rows: every implemented operation that resolves a cluster by name.
    let cases: [(&str, Value); 4] = [
        ("DescribeClusters", json!({ "ClusterNames": ["ghost"] })),
        ("UpdateCluster", json!({ "ClusterName": "ghost" })),
        ("DeleteCluster", json!({ "ClusterName": "ghost" })),
        (
            "IncreaseReplicationFactor",
            json!({ "ClusterName": "ghost", "NewReplicationFactor": 3 }),
        ),
    ];

    for (operation, body) in cases {
        let (status, answered) = call(&router, operation, body).await;
        assert_eq!(
            status, 404,
            "case {operation}: expected 404, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "ClusterNotFoundFault",
            "case {operation}: expected ClusterNotFoundFault, got {answered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: `IncreaseReplicationFactor` only ADDS nodes — a factor at or below the
// cluster's current node count is refused with `InvalidParameterValueException`, and one
// past the documented maximum with `NodeQuotaForClusterExceededFault`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn increasing_the_replication_factor_only_grows_the_cluster() {
    let router = emulator();
    let cluster = available_cluster(&router, "grow", 3).await;
    assert_eq!(
        cluster["TotalNodes"],
        json!(3),
        "the cluster must start with three nodes, got {cluster:?}"
    );

    // Rows: below the current count, equal to it, and past the documented maximum.
    let refused: [(i64, &str); 3] = [
        (2, "InvalidParameterValueException"),
        (3, "InvalidParameterValueException"),
        (11, "NodeQuotaForClusterExceededFault"),
    ];
    for (factor, expected) in refused {
        let (status, body) = call(
            &router,
            "IncreaseReplicationFactor",
            json!({ "ClusterName": "grow", "NewReplicationFactor": factor }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {factor}: expected a refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {factor}: expected {expected}, got {body:?}"
        );
    }

    // Rows: every accepted step up to the documented maximum, each growing the store.
    for factor in [4_i64, 7, 10] {
        let (status, body) = call(
            &router,
            "IncreaseReplicationFactor",
            json!({ "ClusterName": "grow", "NewReplicationFactor": factor }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {factor}: expected the growth accepted, got {status} {body:?}"
        );
        settle().await;
        let (status, described) = call(
            &router,
            "DescribeClusters",
            json!({ "ClusterNames": ["grow"] }),
        )
        .await;
        assert_eq!(status, 200, "case {factor}: DescribeClusters got {described:?}");
        let grown = &described["Clusters"][0];
        assert_eq!(
            grown["TotalNodes"],
            json!(factor),
            "case {factor}: expected TotalNodes {factor}, got {grown:?}"
        );
        let ids = node_ids(grown);
        assert_eq!(
            ids.iter().collect::<BTreeSet<_>>().len(),
            usize::try_from(factor).unwrap(),
            "case {factor}: expected {factor} distinct node ids, got {ids:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: `AvailabilityZones`, when given to `CreateCluster`, must have exactly
// `ReplicationFactor` entries, and the nodes are placed in them.
// Source: `CreateCluster$AvailabilityZones` ("If provided, the length of this list must
// equal the ReplicationFactor parameter").
// ---------------------------------------------------------------------------

#[tokio::test]
async fn availability_zones_must_match_the_replication_factor_and_place_the_nodes() {
    let router = emulator();
    let zones = [
        format!("{REGION}a"),
        format!("{REGION}b"),
        format!("{REGION}c"),
    ];

    // Rows: one entry too few, one too many, and the matching length.
    let refused: [usize; 2] = [2, 3];
    for (index, count) in refused.into_iter().enumerate() {
        let factor = if count == 2 { 3 } else { 2 };
        let mut body = create_cluster_body(&format!("az-bad-{index}"), factor);
        body["AvailabilityZones"] = json!(zones[..count]);
        let (status, answered) = call(&router, "CreateCluster", body).await;
        assert_eq!(
            status, 400,
            "case {count} zones for factor {factor}: expected a refusal, got {status} {answered:?}"
        );
        assert_eq!(
            error_code(&answered),
            "InvalidParameterCombinationException",
            "case {count} zones for factor {factor}: expected InvalidParameterCombinationException, \
             got {answered:?}"
        );
    }

    let mut body = create_cluster_body("az-good", 3);
    body["AvailabilityZones"] = json!(zones);
    let (status, created) = call(&router, "CreateCluster", body).await;
    assert_eq!(status, 200, "a matching zone list is accepted, got {created:?}");
    settle().await;
    let (status, described) = call(
        &router,
        "DescribeClusters",
        json!({ "ClusterNames": ["az-good"] }),
    )
    .await;
    assert_eq!(status, 200, "DescribeClusters got {described:?}");
    let placed: BTreeSet<String> = described["Clusters"][0]["Nodes"]
        .as_array()
        .expect("Nodes is a list")
        .iter()
        .map(|node| {
            node["AvailabilityZone"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        placed,
        zones.iter().cloned().collect::<BTreeSet<String>>(),
        "the nodes must be placed in the requested zones, got {placed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule: server-side encryption is reported as the state the request asked for.
// Source: `SSEDescription$Status` / `SSESpecification$Enabled`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn server_side_encryption_is_reported_as_requested() {
    // Rows: the specification omitted, disabled, and enabled.
    let cases: [(Option<bool>, &str); 3] = [
        (None, state::SSE_DISABLED),
        (Some(false), state::SSE_DISABLED),
        (Some(true), state::SSE_ENABLED),
    ];

    let router = emulator();
    for (index, (enabled, expected)) in cases.into_iter().enumerate() {
        let name = format!("sse-{index}");
        let mut body = create_cluster_body(&name, 1);
        if let Some(enabled) = enabled {
            body["SSESpecification"] = json!({ "Enabled": enabled });
        }
        let (status, created) = call(&router, "CreateCluster", body).await;
        assert_eq!(status, 200, "case {enabled:?}: CreateCluster got {created:?}");
        assert_eq!(
            created["Cluster"]["SSEDescription"]["Status"],
            json!(expected),
            "case {enabled:?}: expected {expected}, got {created:?}"
        );
        let (status, described) = call(
            &router,
            "DescribeClusters",
            json!({ "ClusterNames": [name] }),
        )
        .await;
        assert_eq!(status, 200, "case {enabled:?}: DescribeClusters got {described:?}");
        assert_eq!(
            described["Clusters"][0]["SSEDescription"]["Status"],
            json!(expected),
            "case {enabled:?}: the stored state must read back, got {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: `UpdateCluster` applies only the members it was given; `NotificationTopicStatus`
// is one of `active` / `inactive` and needs a topic.
// Source: `UpdateCluster$NotificationTopicStatus`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_applies_only_the_members_it_was_given() {
    let router = emulator();
    let mut body = create_cluster_body("patch", 1);
    body["Description"] = json!("first");
    body["PreferredMaintenanceWindow"] = json!("mon:05:00-mon:06:00");
    body["ParameterGroupName"] = json!("group-one");
    let (status, created) = call(&router, "CreateCluster", body).await;
    assert_eq!(status, 200, "CreateCluster got {created:?}");

    // Rows: one member at a time, each leaving the others as they were stored.
    let patches: [(&str, Value); 3] = [
        ("Description", json!("second")),
        ("PreferredMaintenanceWindow", json!("tue:07:00-tue:08:00")),
        ("ParameterGroupName", json!("group-two")),
    ];
    let mut expected = serde_json::Map::new();
    expected.insert("Description".into(), json!("first"));
    expected.insert(
        "PreferredMaintenanceWindow".into(),
        json!("mon:05:00-mon:06:00"),
    );
    expected.insert("ParameterGroupName".into(), json!("group-one"));

    for (member, value) in patches {
        settle().await;
        let (status, updated) = call(
            &router,
            "UpdateCluster",
            json!({ "ClusterName": "patch", member: value }),
        )
        .await;
        assert_eq!(status, 200, "case {member}: UpdateCluster got {updated:?}");
        expected.insert(member.into(), value);

        let (status, described) = call(
            &router,
            "DescribeClusters",
            json!({ "ClusterNames": ["patch"] }),
        )
        .await;
        assert_eq!(status, 200, "case {member}: DescribeClusters got {described:?}");
        let cluster = &described["Clusters"][0];
        assert_eq!(
            cluster["Description"], expected["Description"],
            "case {member}: expected description {:?}, got {cluster:?}",
            expected["Description"]
        );
        assert_eq!(
            cluster["PreferredMaintenanceWindow"], expected["PreferredMaintenanceWindow"],
            "case {member}: expected window {:?}, got {cluster:?}",
            expected["PreferredMaintenanceWindow"]
        );
        assert_eq!(
            cluster["ParameterGroup"]["ParameterGroupName"], expected["ParameterGroupName"],
            "case {member}: expected parameter group {:?}, got {cluster:?}",
            expected["ParameterGroupName"]
        );
    }

    // Rows for the topic status: the two documented values, and one that is neither.
    settle().await;
    let (status, answered) = call(
        &router,
        "UpdateCluster",
        json!({
            "ClusterName": "patch",
            "NotificationTopicArn": "arn:aws:sns:us-west-2:000000000000:dax",
            "NotificationTopicStatus": "active",
        }),
    )
    .await;
    assert_eq!(status, 200, "case active: UpdateCluster got {answered:?}");
    assert_eq!(
        answered["Cluster"]["NotificationConfiguration"]["TopicStatus"],
        json!("active"),
        "case active: expected the topic reported active, got {answered:?}"
    );
    settle().await;
    let (status, answered) = call(
        &router,
        "UpdateCluster",
        json!({ "ClusterName": "patch", "NotificationTopicStatus": "inactive" }),
    )
    .await;
    assert_eq!(status, 200, "case inactive: UpdateCluster got {answered:?}");
    assert_eq!(
        answered["Cluster"]["NotificationConfiguration"]["TopicStatus"],
        json!("inactive"),
        "case inactive: expected the topic reported inactive, got {answered:?}"
    );
    settle().await;
    let (status, answered) = call(
        &router,
        "UpdateCluster",
        json!({ "ClusterName": "patch", "NotificationTopicStatus": "paused" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case paused: expected a refusal, got {status} {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "InvalidParameterValueException",
        "case paused: expected InvalidParameterValueException, got {answered:?}"
    );
}

// ---------------------------------------------------------------------------
// The operations this slice deliberately leaves unwritten still answer 501, so a
// client can tell "not written yet" from "implemented".
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_operations_outside_this_slice_still_answer_not_implemented() {
    let router = emulator();
    // Rows: one operation from each family left to its own gap issue.
    let cases: [(&str, Value); 5] = [
        (
            "UpdateParameterGroup",
            json!({ "ParameterGroupName": "pg", "ParameterNameValues": [] }),
        ),
        (
            "DecreaseReplicationFactor",
            json!({ "ClusterName": "demo", "NewReplicationFactor": 1 }),
        ),
        (
            "RebootNode",
            json!({ "ClusterName": "demo", "NodeId": "demo-1" }),
        ),
        (
            "TagResource",
            json!({ "ResourceName": "arn:aws:dax:us-west-2:000000000000:cache/demo", "Tags": [] }),
        ),
        ("DeleteSubnetGroup", json!({ "SubnetGroupName": "sg" })),
    ];

    for (operation, body) in cases {
        let (status, answered) = call(&router, operation, body).await;
        assert_eq!(
            status, 501,
            "case {operation}: an unwritten operation answers 501, got {status} {answered:?}"
        );
    }
}
