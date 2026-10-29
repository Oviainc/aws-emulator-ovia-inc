// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a `CreatePrivateGraphEndpoint` for a graph and VPC that
//! already hold an endpoint, driven over the wire (`restJson1`) through the same router
//! the emulator serves from.
//!
//! One rule, asserted over a set of inputs rather than one request: a 409 raised because
//! a stored record held the call back states WHAT THE CALL COULD NOT DO and THE CONDITION
//! that held it back — `Unable to <action> because <the record> <condition>.` — and
//! nothing else. The caller addressed the endpoint by the two identifiers it sent (the
//! graph identifier and the `vpcId`), so the refusal fronts neither of them, and it
//! reports nothing of the stored record it collided with: not the `vpcEndpointId` this
//! service minted for it, nor the subnets or security groups it was created with.
//!
//! Source for the template: the recorded AWS response for `StartGraph` on an `AVAILABLE`
//! graph, "Unable to start the graph because it is in the AVAILABLE state.", the grammar
//! every other 409 of this service is worded by (`tests/all/precondition_refusal_grammar.rs`).
//!
//! The rows span what the rule generalises over:
//!
//! * the status the stored endpoint had settled into when the second create arrived —
//!   still being attached (`CREATING`), serving (`AVAILABLE`), or on its way out
//!   (`DELETING`), the three statuses an endpoint of this service is ever read in;
//! * the form of the `vpcId` the pair is keyed on, across what the modelled
//!   `^vpc-[a-z0-9]+$` pattern admits;
//! * the subnets and security groups of the stored record and of the colliding request,
//!   which differ from each other on every row, since the conflict is keyed on the pair
//!   alone;
//! * which graph holds the endpoint, so the pair and not the VPC id is what collides.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    ENDPOINT_DELETION_SPAN, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// The most subnets one endpoint may name: `SubnetIds` is `@length(1..6)`
/// (`smithy_query(query="shape",shape="SubnetIds")`).
const MAX_SUBNET_IDS: usize = 6;

/// An emulator holding only this service, on its own engine, so each test starts from an
/// empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` member of a failure body.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The whole message this refusal must carry, written out here rather than reached from
/// the service's own helper so the test states the expectation independently of the code
/// under test.
fn expected_message() -> String {
    let action = "create the private graph endpoint";
    format!("Unable to {action} because it already exists.")
}

/// `n` distinct subnet ids matching the modelled `SubnetId` pattern `^subnet-[a-z0-9]+$`.
fn subnets(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("subnet-0a1b2c3d4e5f{i:05}")).collect()
}

/// `n` distinct security group ids matching the modelled `SecurityGroupId` pattern
/// `^sg-[a-z0-9]+$`, offset by `from` so two requests can name disjoint sets.
fn security_groups(from: usize, n: usize) -> Vec<String> {
    (from..from + n)
        .map(|i| format!("sg-0f1e2d3c4b5a{i:05}"))
        .collect()
}

/// The request body of one create: the VPC, its subnets, and the security groups when
/// the row attaches any.
fn endpoint_body(vpc_id: &str, subnet_ids: &[String], security_group_ids: &[String]) -> Value {
    let mut body = json!({"vpcId": vpc_id, "subnetIds": subnet_ids});
    if !security_group_ids.is_empty() {
        body["vpcSecurityGroupIds"] = json!(security_group_ids);
    }
    body
}

/// `POST /graphs/{graphIdentifier}/endpoints/`.
async fn create_endpoint(router: &MultiServiceRouter, graph_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/graphs/{graph_id}/endpoints/"),
        Some(body),
    )
    .await
}

/// `GET /graphs/{graphIdentifier}/endpoints/{vpcId}`.
async fn get_endpoint(router: &MultiServiceRouter, graph_id: &str, vpc_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/graphs/{graph_id}/endpoints/{vpc_id}"),
        None,
    )
    .await
}

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this test
/// does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, case: &str, path: &str) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read of {path}, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {case:?}: {path} never became AVAILABLE, last status {}",
        body["status"]
    );
}

/// Create a graph and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/graphs",
        Some(json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no graph id in {created}"))
        .to_string();
    wait_until_available(router, case, &format!("/graphs/{id}")).await;
    id
}

/// Assert one refusal: the modelled status and code, the whole message the rule words,
/// and that nothing of the record it collided with — nor of the identifiers the caller
/// addressed that record by — reached the message.
fn assert_endpoint_conflict(case: &str, status: u16, body: &Value, undisclosed: &[(&str, &str)]) {
    assert_eq!(
        status, 409,
        "case {case:?}: expected 409, got {status}: {body}"
    );
    assert_eq!(
        error_code(body),
        "ConflictException",
        "case {case:?}: expected ConflictException, got {body}"
    );
    let actual = message(body);
    let expected = expected_message();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected message {expected:?}, got {actual:?}"
    );
    for (what, value) in undisclosed {
        assert!(
            !actual.contains(value),
            "case {case:?}: the refusal must not disclose {what} ({value}), got {actual:?}"
        );
    }
}

/// One row of the table below: the case name, the `vpcId` the pair is keyed on, the
/// status the stored endpoint is in when the second create arrives, and the subnet counts
/// the stored record and the colliding request name.
type ConflictCase = (&'static str, &'static str, &'static str, usize, usize);

#[tokio::test]
async fn a_second_endpoint_in_a_pair_that_already_holds_one_is_refused_without_disclosing_it() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "the holding graph", "endpoint-conflict-holder").await;
    let other_graph =
        graph_in_service(&router, "another graph", "endpoint-conflict-bystander").await;

    // (case, vpcId, the status the stored endpoint is read in, stored subnet count,
    // colliding subnet count) — the VPC id spans the forms `^vpc-[a-z0-9]+$` admits, and
    // the two creates of a row never name the same subnets or security groups, so the
    // refusal cannot be reading the request's other members.
    let cases: Vec<ConflictCase> = vec![
        (
            "an endpoint still being attached",
            "vpc-creating01",
            "CREATING",
            1,
            3,
        ),
        (
            "a serving endpoint, long VPC id, subnets at the published bound",
            "vpc-0a1b2c3d4e5f67890",
            "AVAILABLE",
            MAX_SUBNET_IDS,
            2,
        ),
        ("an endpoint on its way out", "vpc-9", "DELETING", 2, 1),
    ];

    for (case, vpc_id, stored_status, stored_subnets, colliding_subnets) in cases {
        let stored_subnet_ids = subnets(stored_subnets);
        let stored_groups = security_groups(0, 2);
        let (status, stored) = create_endpoint(
            &router,
            &graph_id,
            endpoint_body(vpc_id, &stored_subnet_ids, &stored_groups),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from the first create, got {status}: {stored}"
        );
        let vpc_endpoint_id = stored["vpcEndpointId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no vpcEndpointId in {stored}"))
            .to_string();

        // Bring the stored record into the status this row is about. `CREATING` is where
        // the create left it, so that row waits for nothing.
        match stored_status {
            "CREATING" => {}
            "AVAILABLE" => {
                wait_until_available(
                    &router,
                    case,
                    &format!("/graphs/{graph_id}/endpoints/{vpc_id}"),
                )
                .await;
            }
            "DELETING" => {
                wait_until_available(
                    &router,
                    case,
                    &format!("/graphs/{graph_id}/endpoints/{vpc_id}"),
                )
                .await;
                let (delete_status, deleted) = call(
                    &router,
                    "DELETE",
                    &format!("/graphs/{graph_id}/endpoints/{vpc_id}"),
                    None,
                )
                .await;
                assert_eq!(
                    (delete_status, &deleted["status"]),
                    (200, &json!("DELETING")),
                    "case {case:?}: expected a 200 DELETING from the delete, got \
                     {delete_status}: {deleted}"
                );
            }
            other => panic!("case {case:?}: no row puts an endpoint in {other:?}"),
        }

        // The colliding create: the same pair, every other member different.
        let colliding_subnet_ids = subnets(colliding_subnets);
        let colliding_groups = security_groups(5, 1);
        let (status, refused) = create_endpoint(
            &router,
            &graph_id,
            endpoint_body(vpc_id, &colliding_subnet_ids, &colliding_groups),
        )
        .await;
        let mut undisclosed = vec![
            ("the stored endpoint's minted id", vpc_endpoint_id.as_str()),
            ("the graph identifier the caller sent", graph_id.as_str()),
            ("the VPC id the caller sent", vpc_id),
        ];
        for subnet in &stored_subnet_ids {
            undisclosed.push(("a subnet of the stored endpoint", subnet.as_str()));
        }
        for group in &stored_groups {
            undisclosed.push(("a security group of the stored endpoint", group.as_str()));
        }
        assert_endpoint_conflict(case, status, &refused, &undisclosed);

        // The refused call reported a collision; it changed nothing. A record on its way
        // out is reclaimed by the clock, so that row's read is allowed to find it gone.
        let (read_status, read) = get_endpoint(&router, &graph_id, vpc_id).await;
        if stored_status == "DELETING" && read_status == 404 {
            continue;
        }
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from GetPrivateGraphEndpoint, got {read_status}: \
             {read}"
        );
        for member in ["vpcEndpointId", "subnetIds"] {
            assert_eq!(
                read[member], stored[member],
                "case {case:?}: the refused create must not change {member}, expected {}, \
                 got {}",
                stored[member], read[member]
            );
        }
    }

    // The boundary of the same rule: the conflict is keyed on the PAIR, so every VPC id
    // the rows above collided on is free on another graph.
    for vpc_id in ["vpc-creating01", "vpc-0a1b2c3d4e5f67890", "vpc-9"] {
        let case = format!("{vpc_id} on a graph that holds no endpoint in it");
        let (status, created) = create_endpoint(
            &router,
            &other_graph,
            endpoint_body(vpc_id, &subnets(1), &[]),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status}: {created}"
        );
    }
}

#[tokio::test]
async fn the_pair_takes_a_new_endpoint_once_the_stored_one_is_reclaimed() {
    let router = emulator();
    let case = "a reclaimed pair";
    let graph_id = graph_in_service(&router, case, "endpoint-conflict-reclaim").await;
    let vpc_id = "vpc-reclaim01";

    let (status, created) =
        create_endpoint(&router, &graph_id, endpoint_body(vpc_id, &subnets(1), &[])).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from the first create, got {status}: {created}"
    );
    let first_id = created["vpcEndpointId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no vpcEndpointId in {created}"))
        .to_string();
    wait_until_available(
        &router,
        case,
        &format!("/graphs/{graph_id}/endpoints/{vpc_id}"),
    )
    .await;
    let (delete_status, deleted) = call(
        &router,
        "DELETE",
        &format!("/graphs/{graph_id}/endpoints/{vpc_id}"),
        None,
    )
    .await;
    assert_eq!(
        delete_status, 200,
        "case {case:?}: expected 200 from the delete, got {delete_status}: {deleted}"
    );

    // Once the clock has made the removal due the pair holds no record, so there is
    // nothing left to conflict with: the refusal was about the stored endpoint and not
    // about the identifiers the request named.
    for _ in 0..40 {
        let (read_status, _) = get_endpoint(&router, &graph_id, vpc_id).await;
        if read_status == 404 {
            break;
        }
        tokio::time::sleep(ENDPOINT_DELETION_SPAN / 4).await;
    }
    let (status, again) =
        create_endpoint(&router, &graph_id, endpoint_body(vpc_id, &subnets(2), &[])).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 once the endpoint was reclaimed, got {status}: {again}"
    );
    assert_ne!(
        again["vpcEndpointId"],
        json!(first_id),
        "case {case:?}: the new endpoint must carry its own minted id, got {first_id:?} \
         again"
    );
}
