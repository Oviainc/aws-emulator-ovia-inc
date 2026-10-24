// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the private graph endpoint resource:
//! `CreatePrivateGraphEndpoint`, the `GetPrivateGraphEndpoint` read it round-trips
//! through, and `DeletePrivateGraphEndpoint`, driven over the wire (`restJson1`)
//! through the same router the emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * A private graph endpoint is the private route into ONE graph from ONE VPC, so
//!   `(graphIdentifier, vpcId)` is its identity: a second endpoint in a VPC that
//!   already has one conflicts (409), the same `vpcId` on another graph names another
//!   endpoint, and an endpoint cannot outlive the graph it reaches.
//! * `CreatePrivateGraphEndpoint` persists the record and reports it — the VPC and
//!   subnets the request named, a minted `^vpce-[0-9a-f]{17}$` endpoint id, and a
//!   `CREATING` status the clock settles to `AVAILABLE`.
//! * `DeletePrivateGraphEndpoint` reports the stored record on its way out with the
//!   modelled `DELETING` status, keeps answering reads with it until the clock makes
//!   the removal due, and then the endpoint is gone: a read answers
//!   `ResourceNotFoundException` (404) and the VPC is free to take a new endpoint.
//! * An identifier naming no stored resource is refused 404, an endpoint with a
//!   transition still outstanding (`CREATING`, `DELETING`) is refused 409, and a value
//!   outside the modelled `VpcId` / `SubnetIds` / `SecurityGroupIds` constraints is
//!   refused 400 rather than served.
//!
//! So each table below spans what its rule generalises over — the subnet count at the
//! published bound and one past it, the presence and count of security groups, the form
//! of the VPC id, which graph an endpoint was created on, and which identifier of the
//! addressing pair names nothing — rather than re-running one recorded request.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    DELETION_SPAN, ENDPOINT_DELETION_SPAN, PROVISIONING_SPAN,
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

/// The most security groups one endpoint may name: `SecurityGroupIds` is
/// `@length(1..10)` (`smithy_query(query="shape",shape="SecurityGroupIds")`).
const MAX_SECURITY_GROUP_IDS: usize = 10;

/// An emulator holding only this service, on its own engine, so each test starts from
/// an empty store.
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

/// Whether `value` matches the modelled `VpcEndpointId` pattern `^vpce-[0-9a-f]{17}$`.
fn is_vpc_endpoint_id(value: &str) -> bool {
    value.strip_prefix("vpce-").is_some_and(|suffix| {
        suffix.len() == 17 && suffix.chars().all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f'))
    })
}

/// `POST /graphs/{graphIdentifier}/endpoints/`.
async fn create_endpoint(
    router: &MultiServiceRouter,
    graph_id: &str,
    body: Value,
) -> (u16, Value) {
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

/// `DELETE /graphs/{graphIdentifier}/endpoints/{vpcId}`.
async fn delete_endpoint(
    router: &MultiServiceRouter,
    graph_id: &str,
    vpc_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/graphs/{graph_id}/endpoints/{vpc_id}"),
        None,
    )
    .await
}

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this
/// test does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, path: &str) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {path}: expected 200 from the read, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {path}: never became AVAILABLE, last status {}",
        body["status"]
    );
}

/// Poll one endpoint until the read no longer finds it, and report the last body.
async fn wait_until_gone(router: &MultiServiceRouter, graph_id: &str, vpc_id: &str) -> Value {
    for _ in 0..40 {
        let (status, body) = get_endpoint(router, graph_id, vpc_id).await;
        if status == 404 {
            return body;
        }
        assert_eq!(
            status, 200,
            "case {graph_id}/{vpc_id}: expected 200 or 404 while the deletion is \
             outstanding, got {status}: {body}"
        );
        tokio::time::sleep(ENDPOINT_DELETION_SPAN / 4).await;
    }
    panic!("case {graph_id}/{vpc_id}: the deleted endpoint was never reclaimed");
}

/// Create a graph and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "graphName": name,
        "provisionedMemory": MIN_PROVISIONED_MEMORY,
    });
    let (status, created) = call(router, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: no id in {created}"))
        .to_string();
    wait_until_available(router, &format!("/graphs/{id}")).await;
    id
}

/// `n` distinct subnet ids matching the modelled `SubnetId` pattern
/// `^subnet-[a-z0-9]+$`.
fn subnets(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("subnet-0a1b2c3d4e5f{i:05}")).collect()
}

/// `n` distinct security group ids matching the modelled `SecurityGroupId` pattern
/// `^sg-[a-z0-9]+$`.
fn security_groups(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("sg-0f1e2d3c4b5a{i:05}")).collect()
}

/// One row of the create-and-read-back table: the case name, the `vpcId` the request
/// carries, the subnets it names, and the security groups it attaches.
type EndpointCase = (&'static str, &'static str, Vec<String>, Vec<String>);

/// One row of the modelled-constraint table: [`EndpointCase`] plus whether the request
/// it describes is accepted.
type ConstraintCase = (
    &'static str,
    &'static str,
    Vec<String>,
    Vec<String>,
    bool,
);

#[tokio::test]
async fn create_private_graph_endpoint_persists_the_record_and_reads_it_back() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "endpoint-source").await;

    // (case, vpcId, subnetIds, vpcSecurityGroupIds) — the rows differ in the form of
    // the VPC id, the subnet count from one up to the published bound, and whether (and
    // how many) security groups are attached.
    let cases: Vec<EndpointCase> = vec![
        (
            "one subnet, no security group",
            "vpc-abc123",
            subnets(1),
            Vec::new(),
        ),
        (
            "three subnets, one security group",
            "vpc-0a1b2c3d4e5f67890",
            subnets(3),
            security_groups(1),
        ),
        (
            "subnets at the published bound, security groups at theirs",
            "vpc-9",
            subnets(MAX_SUBNET_IDS),
            security_groups(MAX_SECURITY_GROUP_IDS),
        ),
    ];

    let mut minted = Vec::new();
    for (case, vpc_id, subnet_ids, security_group_ids) in cases {
        let mut body = json!({"vpcId": vpc_id, "subnetIds": subnet_ids});
        if !security_group_ids.is_empty() {
            body["vpcSecurityGroupIds"] = json!(security_group_ids);
        }
        let (status, created) = create_endpoint(&router, &graph_id, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status}: {created}"
        );
        assert_eq!(
            created["vpcId"],
            json!(vpc_id),
            "case {case:?}: expected vpcId {vpc_id:?}, got {}",
            created["vpcId"]
        );
        assert_eq!(
            created["subnetIds"],
            json!(subnet_ids),
            "case {case:?}: expected subnetIds {}, got {}",
            json!(subnet_ids),
            created["subnetIds"]
        );
        assert_eq!(
            created["status"],
            json!("CREATING"),
            "case {case:?}: an endpoint is not reachable when the call returns, expected \
             CREATING, got {}",
            created["status"]
        );
        let vpc_endpoint_id = created["vpcEndpointId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no vpcEndpointId in {created}"))
            .to_string();
        assert!(
            is_vpc_endpoint_id(&vpc_endpoint_id),
            "case {case:?}: expected an id matching ^vpce-[0-9a-f]{{17}}$, got \
             {vpc_endpoint_id:?}"
        );
        assert!(
            !minted.contains(&vpc_endpoint_id),
            "case {case:?}: vpcEndpointId {vpc_endpoint_id:?} was minted twice, already \
             in {minted:?}"
        );
        minted.push(vpc_endpoint_id.clone());

        // The round trip: an independent read reports the record that was stored, not
        // the response that was rendered.
        let (read_status, read) = get_endpoint(&router, &graph_id, vpc_id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from GetPrivateGraphEndpoint, got {read_status}: \
             {read}"
        );
        for member in ["vpcId", "subnetIds", "vpcEndpointId"] {
            assert_eq!(
                read[member], created[member],
                "case {case:?}: GetPrivateGraphEndpoint's {member} must be the stored \
                 value {}, got {}",
                created[member], read[member]
            );
        }

        // The clock owes the move to AVAILABLE; nothing else about the record changes.
        let settled =
            wait_until_available(&router, &format!("/graphs/{graph_id}/endpoints/{vpc_id}")).await;
        for member in ["vpcId", "subnetIds", "vpcEndpointId"] {
            assert_eq!(
                settled[member], created[member],
                "case {case:?}: settling must not change {member}, expected {}, got {}",
                created[member], settled[member]
            );
        }
    }
}

#[tokio::test]
async fn a_second_endpoint_in_a_vpc_that_already_has_one_conflicts() {
    let router = emulator();
    let first_graph = graph_in_service(&router, "conflict-graph-a").await;
    let second_graph = graph_in_service(&router, "conflict-graph-b").await;

    let (status, created) = create_endpoint(
        &router,
        &first_graph,
        json!({"vpcId": "vpc-taken1", "subnetIds": subnets(2)}),
    )
    .await;
    assert_eq!(
        status, 201,
        "case the first endpoint: expected 201, got {status}: {created}"
    );

    // (case, the graph the next endpoint is created on, its vpcId, expected status)
    let cases: Vec<(&str, &str, &str, u16)> = vec![
        (
            "the same vpc on the same graph",
            &first_graph,
            "vpc-taken1",
            409,
        ),
        (
            "the same vpc id on another graph",
            &second_graph,
            "vpc-taken1",
            201,
        ),
        ("a free vpc on the same graph", &first_graph, "vpc-free1", 201),
        (
            "a free vpc on another graph",
            &second_graph,
            "vpc-free2",
            201,
        ),
    ];
    for (case, graph_id, vpc_id, expected) in cases {
        let (status, body) = create_endpoint(
            &router,
            graph_id,
            json!({"vpcId": vpc_id, "subnetIds": subnets(1)}),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}: {body}"
        );
        if expected == 409 {
            let code = error_code(&body);
            assert_eq!(
                code, "ConflictException",
                "case {case:?}: expected ConflictException, got {code:?}: {body}"
            );
        }
    }

    // The same vpc id on two graphs named two independent endpoints, each with its own
    // minted VPC endpoint id.
    let (_, first) = get_endpoint(&router, &first_graph, "vpc-taken1").await;
    let (_, second) = get_endpoint(&router, &second_graph, "vpc-taken1").await;
    assert_ne!(
        first["vpcEndpointId"], second["vpcEndpointId"],
        "case the same vpc on two graphs: expected two distinct endpoints, got {} on \
         both",
        first["vpcEndpointId"]
    );
}

#[tokio::test]
async fn deleting_an_endpoint_reports_it_deleting_and_then_reclaims_it() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "delete-source").await;

    // (case, vpcId, subnetIds) — the rows differ in the VPC id's form and the subnet
    // count, so the rule is asserted over the class and not over one request.
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        ("one subnet", "vpc-del1", subnets(1)),
        ("two subnets", "vpc-0123456789abcdef0", subnets(2)),
        (
            "subnets at the published bound",
            "vpc-del3",
            subnets(MAX_SUBNET_IDS),
        ),
    ];

    for (case, vpc_id, subnet_ids) in cases {
        let (status, created) = create_endpoint(
            &router,
            &graph_id,
            json!({"vpcId": vpc_id, "subnetIds": subnet_ids}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from the create, got {status}: {created}"
        );
        let stored =
            wait_until_available(&router, &format!("/graphs/{graph_id}/endpoints/{vpc_id}")).await;

        // The delete reports the STORED record, with the modelled transient status.
        let (status, deleted) = delete_endpoint(&router, &graph_id, vpc_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the delete, got {status}: {deleted}"
        );
        assert_eq!(
            deleted["status"],
            json!("DELETING"),
            "case {case:?}: expected DELETING, got {}",
            deleted["status"]
        );
        for member in ["vpcId", "subnetIds", "vpcEndpointId"] {
            assert_eq!(
                deleted[member], stored[member],
                "case {case:?}: the delete must report the stored {member} {}, got {}",
                stored[member], deleted[member]
            );
        }

        // A caller acting straight after the call still sees the transient status, and a
        // second delete of an endpoint already on its way out conflicts.
        let (read_status, read) = get_endpoint(&router, &graph_id, vpc_id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from the read while the deletion is \
             outstanding, got {read_status}: {read}"
        );
        assert_eq!(
            read["status"],
            json!("DELETING"),
            "case {case:?}: expected DELETING from the read, got {}",
            read["status"]
        );
        let (again_status, again) = delete_endpoint(&router, &graph_id, vpc_id).await;
        assert_eq!(
            again_status, 409,
            "case {case:?}: expected 409 from a second delete, got {again_status}: {again}"
        );
        let code = error_code(&again);
        assert_eq!(
            code, "ConflictException",
            "case {case:?}: expected ConflictException from a second delete, got \
             {code:?}: {again}"
        );

        // Once the clock makes the removal due the endpoint is GONE: the read answers
        // the modelled not-found, and the VPC is free to take a new endpoint — which
        // gets its own minted id rather than the reclaimed one.
        let gone = wait_until_gone(&router, &graph_id, vpc_id).await;
        let code = error_code(&gone);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException once reclaimed, got \
             {code:?}: {gone}"
        );
        let (status, recreated) = create_endpoint(
            &router,
            &graph_id,
            json!({"vpcId": vpc_id, "subnetIds": subnets(1)}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the vpc to be free again, got {status}: {recreated}"
        );
        assert_ne!(
            recreated["vpcEndpointId"], stored["vpcEndpointId"],
            "case {case:?}: a new endpoint must carry a newly minted id, got the \
             reclaimed {}",
            stored["vpcEndpointId"]
        );
        // Leave the store without an endpoint in this vpc for the next row.
        let (status, body) = delete_endpoint(&router, &graph_id, vpc_id).await;
        assert!(
            status == 200 || status == 409,
            "case {case:?}: expected the cleanup delete to be accepted or conflict, got \
             {status}: {body}"
        );
    }
}

#[tokio::test]
async fn an_endpoint_with_a_transition_outstanding_cannot_be_deleted() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "transient-source").await;

    let (status, created) = create_endpoint(
        &router,
        &graph_id,
        json!({"vpcId": "vpc-transient", "subnetIds": subnets(2)}),
    )
    .await;
    assert_eq!(status, 201, "the create: got {status}: {created}");
    assert_eq!(
        created["status"],
        json!("CREATING"),
        "case a freshly created endpoint: expected CREATING, got {}",
        created["status"]
    );

    // The refusal is taken while the provisioning transition is still outstanding.
    let (status, refused) = delete_endpoint(&router, &graph_id, "vpc-transient").await;
    assert_eq!(
        status, 409,
        "case an endpoint still provisioning: expected 409, got {status}: {refused}"
    );
    let code = error_code(&refused);
    assert_eq!(
        code, "ConflictException",
        "case an endpoint still provisioning: expected ConflictException, got {code:?}: \
         {refused}"
    );

    // The refusal persisted nothing: once the endpoint is reachable, the SAME call is
    // accepted.
    wait_until_available(&router, &format!("/graphs/{graph_id}/endpoints/vpc-transient")).await;
    let (status, accepted) = delete_endpoint(&router, &graph_id, "vpc-transient").await;
    assert_eq!(
        status, 200,
        "case the same call once the endpoint is available: expected 200, got {status}: \
         {accepted}"
    );
}

#[tokio::test]
async fn an_identifier_that_names_no_stored_endpoint_is_refused() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "identifier-source").await;
    let other_graph = graph_in_service(&router, "identifier-other").await;
    let (status, created) = create_endpoint(
        &router,
        &graph_id,
        json!({"vpcId": "vpc-stored", "subnetIds": subnets(1)}),
    )
    .await;
    assert_eq!(status, 201, "the fixture endpoint: got {status}: {created}");
    wait_until_available(&router, &format!("/graphs/{graph_id}/endpoints/vpc-stored")).await;

    // (case, graphIdentifier, vpcId, expected status, expected error code) — one row
    // per way the addressing pair can fail to name a stored endpoint, for the read and
    // for the delete alike.
    let cases: Vec<(&str, &str, &str, u16, &str)> = vec![
        ("the stored endpoint", &graph_id, "vpc-stored", 200, ""),
        (
            "a stored graph with no endpoint in that vpc",
            &graph_id,
            "vpc-never",
            404,
            "ResourceNotFoundException",
        ),
        (
            "the stored vpc id on another graph",
            &other_graph,
            "vpc-stored",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a well-formed graph id that was never created",
            "g-zzzzzzzzzz",
            "vpc-stored",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a graph identifier outside the modelled pattern",
            "not-a-graph-id",
            "vpc-stored",
            400,
            "ValidationException",
        ),
        (
            "a vpc id outside the modelled pattern",
            &graph_id,
            "VPC-Stored",
            400,
            "ValidationException",
        ),
    ];

    for (case, graph_identifier, vpc_id, expected, expected_code) in cases {
        let (status, body) = get_endpoint(&router, graph_identifier, vpc_id).await;
        assert_eq!(
            status, expected,
            "case {case:?} (read {graph_identifier}/{vpc_id}): expected {expected}, got \
             {status}: {body}"
        );
        if !expected_code.is_empty() {
            let code = error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?} (read {graph_identifier}/{vpc_id}): expected \
                 {expected_code}, got {code:?}: {body}"
            );
        }
        if expected == 200 {
            // The accepted row is exercised against the delete last, so the rows that
            // must fail are not answered out of a store this one emptied.
            continue;
        }
        let (status, body) = delete_endpoint(&router, graph_identifier, vpc_id).await;
        assert_eq!(
            status, expected,
            "case {case:?} (delete {graph_identifier}/{vpc_id}): expected {expected}, \
             got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?} (delete {graph_identifier}/{vpc_id}): expected \
             {expected_code}, got {code:?}: {body}"
        );
    }

    // The stored endpoint was never touched by any refusal above.
    let (status, body) = delete_endpoint(&router, &graph_id, "vpc-stored").await;
    assert_eq!(
        status, 200,
        "case the stored endpoint: expected 200 from the delete, got {status}: {body}"
    );
}

#[tokio::test]
async fn an_endpoint_does_not_outlive_the_graph_it_reaches() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "doomed-graph").await;
    let survivor = graph_in_service(&router, "surviving-graph").await;

    for (graph, vpc_id) in [(&graph_id, "vpc-doomed"), (&survivor, "vpc-doomed")] {
        let (status, created) = create_endpoint(
            &router,
            graph,
            json!({"vpcId": vpc_id, "subnetIds": subnets(1)}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case the endpoint on {graph}: expected 201, got {status}: {created}"
        );
        wait_until_available(&router, &format!("/graphs/{graph}/endpoints/{vpc_id}")).await;
    }

    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/graphs/{graph_id}?skipSnapshot=true"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case DeleteGraph: expected 200, got {status}: {body}"
    );

    // The graph is on its way out, so its endpoint is still reachable for as long as the
    // graph itself is...
    let (status, body) = get_endpoint(&router, &graph_id, "vpc-doomed").await;
    assert_eq!(
        status, 200,
        "case the deleting graph's endpoint: expected 200 while the graph is DELETING, \
         got {status}: {body}"
    );

    // ...and goes with it once the deletion the clock owes falls due; the other graph's
    // endpoint in the same VPC is untouched.
    tokio::time::sleep(DELETION_SPAN + Duration::from_millis(150)).await;
    let (status, body) = get_endpoint(&router, &graph_id, "vpc-doomed").await;
    assert_eq!(
        status, 404,
        "case the deleted graph's endpoint: expected 404, got {status}: {body}"
    );
    let (status, body) = get_endpoint(&router, &survivor, "vpc-doomed").await;
    assert_eq!(
        status, 200,
        "case the surviving graph's endpoint: expected 200, got {status}: {body}"
    );
    assert_eq!(
        body["status"],
        json!("AVAILABLE"),
        "case the surviving graph's endpoint: expected AVAILABLE, got {}",
        body["status"]
    );
}

#[tokio::test]
async fn a_request_outside_the_modelled_list_constraints_is_refused() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "constraint-source").await;

    // One row per clause of the modelled `SubnetIds` (`@length(1..6)`, `@uniqueItems`,
    // member `@pattern("^subnet-[a-z0-9]+$")`) and `SecurityGroupIds`
    // (`@length(1..10)`, `@uniqueItems`, member `@pattern("^sg-[a-z0-9]+$")`)
    // constraints, including each published bound and one entry past it.
    let duplicated = vec![subnets(1)[0].clone(), subnets(1)[0].clone()];
    let cases: Vec<ConstraintCase> = vec![
        (
            "subnets at the published bound",
            "vpc-ok1",
            subnets(MAX_SUBNET_IDS),
            Vec::new(),
            true,
        ),
        (
            "one subnet past the published bound",
            "vpc-bad1",
            subnets(MAX_SUBNET_IDS + 1),
            Vec::new(),
            false,
        ),
        ("an empty subnet list", "vpc-bad2", Vec::new(), Vec::new(), false),
        ("a repeated subnet id", "vpc-bad3", duplicated, Vec::new(), false),
        (
            "a subnet id outside the member pattern",
            "vpc-bad4",
            vec!["subnet_0a1b".to_string()],
            Vec::new(),
            false,
        ),
        (
            "security groups at the published bound",
            "vpc-ok2",
            subnets(1),
            security_groups(MAX_SECURITY_GROUP_IDS),
            true,
        ),
        (
            "one security group past the published bound",
            "vpc-bad5",
            subnets(1),
            security_groups(MAX_SECURITY_GROUP_IDS + 1),
            false,
        ),
        (
            "a security group id outside the member pattern",
            "vpc-bad6",
            subnets(1),
            vec!["SG-0F1E".to_string()],
            false,
        ),
        (
            "a vpc id outside the modelled pattern",
            "vpc_bad7",
            subnets(1),
            Vec::new(),
            false,
        ),
    ];

    for (case, vpc_id, subnet_ids, security_group_ids, accepted) in cases {
        let mut body = json!({"vpcId": vpc_id, "subnetIds": subnet_ids});
        if !security_group_ids.is_empty() {
            body["vpcSecurityGroupIds"] = json!(security_group_ids);
        }
        let (status, response) = create_endpoint(&router, &graph_id, body).await;
        if accepted {
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201, got {status}: {response}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        let code = error_code(&response);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected ValidationException, got {code:?}: {response}"
        );
        // A refused request persisted nothing, so the vpc it named still has no
        // endpoint.
        let (read_status, read) = get_endpoint(&router, &graph_id, vpc_id).await;
        assert_ne!(
            read_status, 200,
            "case {case:?}: a refused create must persist nothing, but the read \
             answered {read}"
        );
    }
}

#[tokio::test]
async fn a_create_that_asks_the_service_to_default_the_vpc_is_not_implemented() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "defaulting-source").await;

    // The real service defaults an absent `vpcId` to the account's default VPC and an
    // absent `subnetIds` to that VPC's subnets. This emulator holds no VPC inventory,
    // so it answers the scaffold's 501 for that facet rather than inventing ids; every
    // row that names both is served (asserted by the tables above).
    let cases: Vec<(&str, Value)> = vec![
        ("no vpcId", json!({"subnetIds": subnets(1)})),
        ("no subnetIds", json!({"vpcId": "vpc-nodefault"})),
        ("neither", json!({})),
    ];
    for (case, body) in cases {
        let (status, response) = create_endpoint(&router, &graph_id, body.clone()).await;
        assert_eq!(
            status, 501,
            "case {case:?} ({body}): expected 501, got {status}: {response}"
        );
        let code = error_code(&response);
        assert_eq!(
            code, "NotImplemented",
            "case {case:?} ({body}): expected NotImplemented, got {code:?}: {response}"
        );
    }
}
