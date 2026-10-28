// SPDX-License-Identifier: CC0-1.0

//! The HTTP status and message DAX puts on a modeled fault, over the wire.
//!
//! DAX speaks `awsJson1_1` (`smithy_query(query="service_info")` reports
//! `protocols: ["awsJson1_1"]`), an RPC protocol on which the model's REST-shaped
//! `@httpError` is inert: the status line carries the fault's CLASS (400 for a client
//! fault) and the body's `__type` carries the fault's shape. So no fault of this
//! service renders the 404 its shape declares, whatever operation raises it.
//!
//! Every case drives `POST /` with `X-Amz-Target: AmazonDAXV3.<Operation>` through the
//! same `MultiServiceRouter` the production router builds, so what is asserted is the
//! HTTP behaviour a DAX SDK client sees.
//!
//! The tables span the CLASS each rule covers — every reachable client fault, across
//! the shapes whose model status is 404 and those whose model status is already 400,
//! and names differing in case, length and count; the one request a trace reported is
//! a single row.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_dax_amazondaxv3::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// A role ARN to pass as the required `IamRoleArn`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/DAXRole";

/// A node type to pass as the required `NodeType`.
const NODE_TYPE: &str = "dax.r4.large";

/// The status this service's wire protocol puts on EVERY client fault.
const CLIENT_FAULT_STATUS: u16 = 400;

/// A router carrying only this service, over its own engine, so each test starts from
/// an empty store.
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

/// The wire error message of a failed call, under either casing an error envelope uses.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
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

/// One row of the fault table: the call that raises a fault, the fault's wire code, and
/// the status its SHAPE declares in the model (`@httpError`) — which is what the wire
/// must NOT take when it is not the client-fault status.
struct FaultCase {
    operation: &'static str,
    request: Value,
    fault: &'static str,
    modeled: u16,
}

// ---------------------------------------------------------------------------
// Rule: on this service's `awsJson1_1` wire, every modeled CLIENT fault is answered
// with HTTP 400 and the fault's shape in the body's `__type`; the per-shape
// `@httpError` the model records (404 on the `*NotFoundFault` family) is inert.
// Source: smithy_query(service_info) -> protocols ["awsJson1_1"]; smithy_query(shape)
// -> ClusterNotFoundFault / ParameterGroupNotFoundFault / SubnetGroupNotFoundFault
// errorKind client, httpError 404 with an inert awsQueryError.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_modeled_client_fault_answers_the_json_protocol_client_status() {
    let router = emulator();

    // One stored cluster and one stored parameter group, so the "already exists" and
    // "invalid state" rows below have something to collide with. Nothing is settled:
    // the cluster is still `creating`, which is what makes its delete a state fault.
    let (status, body) = call(&router, "CreateCluster", create_cluster_body("faultbase", 1)).await;
    assert_eq!(status, 200, "CreateCluster faultbase: got {status} {body:?}");
    let (status, body) = call(
        &router,
        "CreateParameterGroup",
        json!({ "ParameterGroupName": "faultgroup" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateParameterGroup faultgroup: got {status} {body:?}"
    );

    // Rows span both halves of the class: the faults whose modeled status is 404 (every
    // resolve-by-name path, on each of the three resource families) and the faults whose
    // modeled status is already the client status, which must come through unchanged.
    let cases = [
        FaultCase {
            operation: "DescribeClusters",
            request: json!({ "ClusterNames": ["ghost"] }),
            fault: "ClusterNotFoundFault",
            modeled: 404,
        },
        FaultCase {
            operation: "DeleteCluster",
            request: json!({ "ClusterName": "ghost" }),
            fault: "ClusterNotFoundFault",
            modeled: 404,
        },
        FaultCase {
            operation: "UpdateCluster",
            request: json!({ "ClusterName": "ghost" }),
            fault: "ClusterNotFoundFault",
            modeled: 404,
        },
        FaultCase {
            operation: "IncreaseReplicationFactor",
            request: json!({ "ClusterName": "ghost", "NewReplicationFactor": 2 }),
            fault: "ClusterNotFoundFault",
            modeled: 404,
        },
        FaultCase {
            operation: "DescribeParameterGroups",
            request: json!({ "ParameterGroupNames": ["ghost-group"] }),
            fault: "ParameterGroupNotFoundFault",
            modeled: 404,
        },
        FaultCase {
            operation: "DeleteParameterGroup",
            request: json!({ "ParameterGroupName": "ghost-group" }),
            fault: "ParameterGroupNotFoundFault",
            modeled: 404,
        },
        FaultCase {
            operation: "DescribeSubnetGroups",
            request: json!({ "SubnetGroupNames": ["ghost-sg"] }),
            fault: "SubnetGroupNotFoundFault",
            modeled: 404,
        },
        FaultCase {
            operation: "CreateCluster",
            request: json!({
                "ClusterName": "needs-sg",
                "NodeType": NODE_TYPE,
                "ReplicationFactor": 1,
                "IamRoleArn": ROLE_ARN,
                "SubnetGroupName": "ghost-sg",
            }),
            fault: "SubnetGroupNotFoundFault",
            modeled: 404,
        },
        FaultCase {
            operation: "CreateCluster",
            request: create_cluster_body("faultbase", 1),
            fault: "ClusterAlreadyExistsFault",
            modeled: 400,
        },
        FaultCase {
            operation: "CreateParameterGroup",
            request: json!({ "ParameterGroupName": "faultgroup" }),
            fault: "ParameterGroupAlreadyExistsFault",
            modeled: 400,
        },
        FaultCase {
            operation: "CreateCluster",
            request: create_cluster_body(
                "too-many",
                i64::from(state::MAX_NODES_PER_CLUSTER) + 1,
            ),
            fault: "NodeQuotaForClusterExceededFault",
            modeled: 400,
        },
        FaultCase {
            operation: "DescribeClusters",
            request: json!({ "MaxResults": i64::from(state::MIN_MAX_RESULTS) - 1 }),
            fault: "InvalidParameterValueException",
            modeled: 400,
        },
        FaultCase {
            operation: "DeleteCluster",
            request: json!({ "ClusterName": "faultbase" }),
            fault: "InvalidClusterStateFault",
            modeled: 400,
        },
    ];

    for case in cases {
        let (status, body) = call(&router, case.operation, case.request.clone()).await;
        assert_eq!(
            error_code(&body),
            case.fault,
            "case {} {}: expected {}, got {status} {body:?}",
            case.operation,
            case.fault,
            case.fault
        );
        assert_eq!(
            status, CLIENT_FAULT_STATUS,
            "case {} {}: a client fault answers {CLIENT_FAULT_STATUS} whatever its \
             modeled @httpError ({}), got {status} {body:?}",
            case.operation, case.fault, case.modeled
        );
    }
}

// ---------------------------------------------------------------------------
// Rule: a not-found fault's message is built in one place per fault, and names the
// identifier that resolved to nothing exactly where it was raised while resolving one
// element of a requested LIST of names — the only case in which the request does not
// already say which name failed. The name it reports is the identity the resource is
// stored by (`CreateCluster$ClusterName` documents that it "is stored as a lowercase
// string"), not the spelling the request used.
// Source: the recorded DAX answers — "Cluster <name> not found." from the describe
// taking `ClusterNames`, "Cluster not found." from the single-`ClusterName` paths,
// "Subnet group <name> not found." from the describe taking `SubnetGroupNames`, and
// "Parameter Group not found." for the parameter-group family.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_not_found_fault_names_the_resource_its_template_names() {
    let router = emulator();
    let (status, body) = call(&router, "CreateCluster", create_cluster_body("kept", 1)).await;
    assert_eq!(status, 200, "CreateCluster kept: got {status} {body:?}");

    // Rows differ along the axes the rule generalises over: the length of the name, its
    // casing, the hyphens in it, and whether the request carries a list of names or a
    // single one.
    let names = [
        "ghost",
        "g",
        "MixedCaseGhost",
        "sim2real-dax-missing-5516309b87dc",
        "ghost-with-hyphens",
    ];
    for name in names {
        let listed = format!("Cluster {} not found.", name.to_lowercase());
        let cases: [(&str, Value, &str); 4] = [
            (
                "DescribeClusters",
                json!({ "ClusterNames": [name] }),
                listed.as_str(),
            ),
            (
                "DeleteCluster",
                json!({ "ClusterName": name }),
                "Cluster not found.",
            ),
            (
                "UpdateCluster",
                json!({ "ClusterName": name }),
                "Cluster not found.",
            ),
            (
                "IncreaseReplicationFactor",
                json!({ "ClusterName": name, "NewReplicationFactor": 2 }),
                "Cluster not found.",
            ),
        ];
        for (operation, request, expected) in cases {
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, CLIENT_FAULT_STATUS,
                "case {operation} {name:?}: expected {CLIENT_FAULT_STATUS}, got {status} {body:?}"
            );
            assert_eq!(
                error_message(&body),
                expected,
                "case {operation} {name:?}: wrong message, got {body:?}"
            );
        }
    }

    // A list mixing a stored name with an absent one is refused naming the ABSENT one,
    // which is the whole point of putting the name in the message.
    let (status, body) = call(
        &router,
        "DescribeClusters",
        json!({ "ClusterNames": ["kept", "Ghost-Two"] }),
    )
    .await;
    assert_eq!(
        status, CLIENT_FAULT_STATUS,
        "case a mixed list: expected {CLIENT_FAULT_STATUS}, got {status} {body:?}"
    );
    assert_eq!(
        error_message(&body),
        "Cluster ghost-two not found.",
        "case a mixed list: the fault must name the name that resolves to nothing, \
         got {body:?}"
    );

    // The subnet-group template names its group the same way.
    for asked in ["absent-sg", "Absent-SG"] {
        let (status, body) = call(
            &router,
            "DescribeSubnetGroups",
            json!({ "SubnetGroupNames": [asked] }),
        )
        .await;
        assert_eq!(
            status, CLIENT_FAULT_STATUS,
            "case DescribeSubnetGroups {asked:?}: expected {CLIENT_FAULT_STATUS}, \
             got {status} {body:?}"
        );
        assert_eq!(
            error_message(&body),
            format!("Subnet group {} not found.", asked.to_lowercase()),
            "case DescribeSubnetGroups {asked:?}: the fault must name the group asked for, \
             got {body:?}"
        );
    }

    // The parameter-group template names nothing, so the same text answers whichever
    // name was asked for and whichever operation resolved it.
    for asked in ["absent-group", "ABSENT-GROUP"] {
        for (operation, request) in [
            (
                "DescribeParameterGroups",
                json!({ "ParameterGroupNames": [asked] }),
            ),
            (
                "DeleteParameterGroup",
                json!({ "ParameterGroupName": asked }),
            ),
        ] {
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, CLIENT_FAULT_STATUS,
                "case {operation} {asked:?}: expected {CLIENT_FAULT_STATUS}, \
                 got {status} {body:?}"
            );
            assert_eq!(
                error_message(&body),
                "Parameter Group not found.",
                "case {operation} {asked:?}: this fault's template names no identifier, \
                 got {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule: the client-fault status covers MODELED faults only — the framework's own
// channels keep the status they carry, so an operation nobody has written yet still
// answers 501 and is not flattened into a client fault.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_scaffolded_operation_still_answers_not_implemented() {
    let router = emulator();
    let (status, body) = call(
        &router,
        "ListTags",
        json!({ "ResourceName": format!("arn:aws:dax:{REGION}:000000000000:cache/kept") }),
    )
    .await;
    assert_eq!(
        status, 501,
        "case ListTags: a scaffolded operation answers 501, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "NotImplemented",
        "case ListTags: expected NotImplemented, got {body:?}"
    );
}
