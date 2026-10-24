// SPDX-License-Identifier: CC0-1.0

//! `GetPrivateGraphEndpoint` as the REPORTING operation the model documents
//! ("Retrieves information about a specified private endpoint",
//! `smithy_query(query="operation",operation="GetPrivateGraphEndpoint")`), driven over
//! the wire (`restJson1`) through the same router the emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * The read answers from PERSISTED state: every member it reports is the stored one,
//!   so reading the same endpoint again reports the same record.
//! * A read changes nothing a later caller sees. The only move it may materialise is
//!   the one the clock already owed — an endpoint's provisioning coming due, and an
//!   accepted deletion being reclaimed — so repeated reads can neither hasten nor
//!   postpone either, a read that finds nothing mints nothing, and reading one endpoint
//!   leaves every other endpoint exactly as it was.
//!
//! The lifecycle suite beside this one covers what each call ACCEPTS; these tables span
//! what the read generalises over — the subnet count from one up to the published
//! bound, the presence and count of security groups, the form of the VPC id, how many
//! times in a row the same endpoint is read, and which identifier of the addressing
//! pair names nothing.

use std::sync::Arc;
use std::time::{Duration, Instant};

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

/// The most security groups one endpoint may name: `SecurityGroupIds` is
/// `@length(1..10)` (`smithy_query(query="shape",shape="SecurityGroupIds")`).
const MAX_SECURITY_GROUP_IDS: usize = 10;

/// How many times in a row each table row reads the same endpoint: more than one, so
/// "reading again reports the same record" is asserted over a run of reads rather than
/// over a single repeat.
const READS_PER_CASE: usize = 5;

/// Every member the modelled `GetPrivateGraphEndpointOutput` carries
/// (`smithy_query(query="operation",operation="GetPrivateGraphEndpoint")`): `status`,
/// `subnetIds` and `vpcId` are required, `vpcEndpointId` is optional.
const OUTPUT_MEMBERS: [&str; 4] = ["status", "subnetIds", "vpcId", "vpcEndpointId"];

/// The members the modelled output marks **required**, so a body missing one is not
/// the modelled output at all.
const REQUIRED_OUTPUT_MEMBERS: [&str; 3] = ["status", "subnetIds", "vpcId"];

/// Whether `value` matches the modelled `VpcEndpointId` pattern `^vpce-[0-9a-f]{17}$`.
fn is_vpc_endpoint_id(value: &str) -> bool {
    value.strip_prefix("vpce-").is_some_and(|suffix| {
        suffix.len() == 17 && suffix.chars().all(|c| c.is_ascii_digit() || matches!(c, 'a'..='f'))
    })
}

/// The members of a stored endpoint that NOTHING about reading it may change: the
/// status is left out, because the clock alone moves that one.
const STABLE_MEMBERS: [&str; 3] = ["vpcId", "subnetIds", "vpcEndpointId"];

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

/// Poll one resource until it reports `AVAILABLE`, and report the body that did. No
/// handler ever sleeps; only this test does, because it is asking about a state the
/// clock reaches on its own.
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

/// Create one endpoint and wait it into service, returning the body the read settled
/// on.
async fn endpoint_in_service(
    router: &MultiServiceRouter,
    graph_id: &str,
    vpc_id: &str,
    subnet_ids: &[String],
) -> Value {
    let (status, created) = create_endpoint(
        router,
        graph_id,
        json!({"vpcId": vpc_id, "subnetIds": subnet_ids}),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {graph_id}/{vpc_id}: expected 201 from the create, got {status}: {created}"
    );
    wait_until_available(router, &format!("/graphs/{graph_id}/endpoints/{vpc_id}")).await
}

/// One row of the read-back table: the case name, the `vpcId` the endpoint lives in,
/// the subnets it names, and the security groups attached to it.
type EndpointCase = (&'static str, &'static str, Vec<String>, Vec<String>);

#[tokio::test]
async fn reading_an_endpoint_again_reports_the_same_stored_record() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "read-source").await;

    // (case, vpcId, subnetIds, vpcSecurityGroupIds) — the rows differ in the form of the
    // VPC id, the subnet count from one up to the published bound, and whether (and how
    // many) security groups are attached, so the rule is asserted over the class rather
    // than over one request.
    let cases: Vec<EndpointCase> = vec![
        (
            "one subnet, no security group",
            "vpc-read01",
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
            "vpc-3",
            subnets(MAX_SUBNET_IDS),
            security_groups(MAX_SECURITY_GROUP_IDS),
        ),
    ];

    for (case, vpc_id, subnet_ids, security_group_ids) in cases {
        let mut body = json!({"vpcId": vpc_id, "subnetIds": subnet_ids});
        if !security_group_ids.is_empty() {
            body["vpcSecurityGroupIds"] = json!(security_group_ids);
        }
        let (status, created) = create_endpoint(&router, &graph_id, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from the create, got {status}: {created}"
        );

        // A run of reads taken while the provisioning is still outstanding: each one
        // reports the stored record, and the only member that may move is the status the
        // clock owes — which moves one way only.
        let mut settled_seen = false;
        for read_number in 1..=READS_PER_CASE {
            let (status, read) = get_endpoint(&router, &graph_id, vpc_id).await;
            assert_eq!(
                status, 200,
                "case {case:?} read {read_number}: expected 200, got {status}: {read}"
            );
            let keys: Vec<&String> = read
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?} read {read_number}: not an object: {read}"))
                .keys()
                .collect();
            assert!(
                keys.iter().all(|key| OUTPUT_MEMBERS.contains(&key.as_str())),
                "case {case:?} read {read_number}: expected only the modelled members \
                 {OUTPUT_MEMBERS:?}, got {keys:?}"
            );
            // And every REQUIRED member is there: a body missing one is not the
            // modelled output, however few extras it carries.
            for member in REQUIRED_OUTPUT_MEMBERS {
                assert!(
                    keys.iter().any(|key| key.as_str() == member),
                    "case {case:?} read {read_number}: expected the required member \
                     {member:?}, got {keys:?}"
                );
            }
            let vpc_endpoint_id = read["vpcEndpointId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?} read {read_number}: no vpcEndpointId in {read}")
                })
                .to_string();
            assert!(
                is_vpc_endpoint_id(&vpc_endpoint_id),
                "case {case:?} read {read_number}: expected an id matching \
                 ^vpce-[0-9a-f]{{17}}$, got {vpc_endpoint_id:?}"
            );
            for member in STABLE_MEMBERS {
                assert_eq!(
                    read[member], created[member],
                    "case {case:?} read {read_number}: expected the stored {member} {}, got \
                     {}",
                    created[member], read[member]
                );
            }
            let reported = read["status"].as_str().unwrap_or_default().to_string();
            assert!(
                reported == "CREATING" || reported == "AVAILABLE",
                "case {case:?} read {read_number}: expected CREATING or AVAILABLE, got \
                 {reported:?}"
            );
            if reported == "AVAILABLE" {
                settled_seen = true;
            } else {
                assert!(
                    !settled_seen,
                    "case {case:?} read {read_number}: an endpoint that already reported \
                     AVAILABLE must not report CREATING again, got {reported:?}"
                );
            }
        }

        // Once the endpoint rests AVAILABLE nothing is owed it, so every later read is
        // the WHOLE body again, status included.
        let settled =
            wait_until_available(&router, &format!("/graphs/{graph_id}/endpoints/{vpc_id}")).await;
        for read_number in 1..=READS_PER_CASE {
            let (status, read) = get_endpoint(&router, &graph_id, vpc_id).await;
            assert_eq!(
                status, 200,
                "case {case:?} settled read {read_number}: expected 200, got {status}: {read}"
            );
            assert_eq!(
                read, settled,
                "case {case:?} settled read {read_number}: expected the stored record \
                 {settled}, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_read_that_finds_no_endpoint_mints_nothing() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "minting-source").await;
    let other_graph = graph_in_service(&router, "minting-other").await;
    endpoint_in_service(&router, &other_graph, "vpc-elsewhere", &subnets(1)).await;

    // (case, vpcId) — one row per way a VPC can hold no endpoint on THIS graph: a vpc id
    // nothing ever used, one whose endpoint lives on another graph, and ids of different
    // lengths and shapes within the modelled `^vpc-[a-z0-9]+$` pattern.
    let cases: Vec<(&str, &str)> = vec![
        ("a vpc nothing ever used", "vpc-never01"),
        ("the longest form of the vpc id", "vpc-0123456789abcdef0"),
        ("the shortest form of the vpc id", "vpc-7"),
        (
            "a vpc whose endpoint lives on another graph",
            "vpc-elsewhere",
        ),
    ];

    for (case, vpc_id) in cases {
        // Reading repeatedly must neither create the endpoint nor reserve the vpc.
        for read_number in 1..=READS_PER_CASE {
            let (status, body) = get_endpoint(&router, &graph_id, vpc_id).await;
            assert_eq!(
                status, 404,
                "case {case:?} read {read_number}: expected 404, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?} read {read_number}: expected ResourceNotFoundException, got \
                 {code:?}: {body}"
            );
        }

        // So the create that follows them is the FIRST endpoint in that vpc: it is
        // accepted, and it starts from the transient status rather than from a record a
        // read left behind.
        let (status, created) = create_endpoint(
            &router,
            &graph_id,
            json!({"vpcId": vpc_id, "subnetIds": subnets(2)}),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: the reads above must leave the vpc free, got {status}: {created}"
        );
        assert_eq!(
            created["status"],
            json!("CREATING"),
            "case {case:?}: expected CREATING from the create, got {}",
            created["status"]
        );
    }

    // The endpoint on the other graph was never disturbed by any of those reads.
    let (status, elsewhere) = get_endpoint(&router, &other_graph, "vpc-elsewhere").await;
    assert_eq!(
        status, 200,
        "case the other graph's endpoint: expected 200, got {status}: {elsewhere}"
    );
    assert_eq!(
        elsewhere["status"],
        json!("AVAILABLE"),
        "case the other graph's endpoint: expected AVAILABLE, got {}",
        elsewhere["status"]
    );
}

#[tokio::test]
async fn reads_neither_hasten_nor_postpone_what_the_clock_owes() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "clock-source").await;

    // (case, vpcId, subnetIds) — the rows differ in the subnet count, so the timing rule
    // is asserted over the class and not over one request.
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        ("one subnet", "vpc-clock01", subnets(1)),
        (
            "subnets at the published bound",
            "vpc-clock02",
            subnets(MAX_SUBNET_IDS),
        ),
    ];

    for (case, vpc_id, subnet_ids) in cases {
        let started = Instant::now();
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
        // The transient status is observable to a caller acting on the call's own answer,
        // which is where this rule is asserted without racing the clock.
        assert_eq!(
            created["status"],
            json!("CREATING"),
            "case {case:?}: expected CREATING from the create, got {}",
            created["status"]
        );

        // However many times the endpoint is read, it becomes reachable no earlier than
        // the span the clock owes it: the provisioning is not hastened by polling.
        let mut reads_while_creating = 0usize;
        let settled = loop {
            let (status, read) = get_endpoint(&router, &graph_id, vpc_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 while provisioning, got {status}: {read}"
            );
            if read["status"] == json!("AVAILABLE") {
                break read;
            }
            reads_while_creating += 1;
            assert!(
                reads_while_creating < 1_000,
                "case {case:?}: the endpoint never became AVAILABLE after \
                 {reads_while_creating} reads"
            );
            tokio::time::sleep(PROVISIONING_SPAN / 20).await;
        };
        let elapsed = started.elapsed();
        assert!(
            elapsed >= PROVISIONING_SPAN,
            "case {case:?}: expected the endpoint to settle no earlier than \
             {PROVISIONING_SPAN:?} after the create, got {elapsed:?} over \
             {reads_while_creating} reads"
        );

        // The same on the way out: an accepted deletion is reclaimed when the clock makes
        // it due, and a run of reads in the meantime keeps reporting the stored record
        // without either reclaiming it early or keeping it alive past its span.
        let (status, deleted) = delete_endpoint(&router, &graph_id, vpc_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the delete, got {status}: {deleted}"
        );
        for read_number in 1..=READS_PER_CASE {
            let (status, read) = get_endpoint(&router, &graph_id, vpc_id).await;
            if status == 404 {
                // The clock made the removal due between reads, which is the one move a
                // read may materialise; the rows below assert it is then GONE.
                break;
            }
            assert_eq!(
                status, 200,
                "case {case:?} deleting read {read_number}: expected 200 or 404 while the \
                 deletion is outstanding, got {status}: {read}"
            );
            assert_eq!(
                read["status"],
                json!("DELETING"),
                "case {case:?} deleting read {read_number}: expected DELETING, got {}",
                read["status"]
            );
            for member in STABLE_MEMBERS {
                assert_eq!(
                    read[member], settled[member],
                    "case {case:?} deleting read {read_number}: expected the stored {member} \
                     {}, got {}",
                    settled[member], read[member]
                );
            }
        }
        tokio::time::sleep(ENDPOINT_DELETION_SPAN + Duration::from_millis(150)).await;
        let (status, gone) = get_endpoint(&router, &graph_id, vpc_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 once the deletion is due, got {status}: {gone}"
        );
    }
}

#[tokio::test]
async fn reading_one_endpoint_leaves_every_other_endpoint_as_it_was() {
    let router = emulator();
    let first_graph = graph_in_service(&router, "sibling-graph-a").await;
    let second_graph = graph_in_service(&router, "sibling-graph-b").await;

    // (case, the graph the endpoint reaches, its vpcId, its subnets) — two endpoints on
    // one graph and a third that shares a VPC id with one of them, because the same VPC
    // id against another graph names another endpoint.
    let fixtures: Vec<(&str, String, &str, Vec<String>)> = vec![
        (
            "the read endpoint",
            first_graph.clone(),
            "vpc-shared1",
            subnets(1),
        ),
        (
            "a sibling on the same graph",
            first_graph.clone(),
            "vpc-sibling1",
            subnets(3),
        ),
        (
            "the same vpc id on another graph",
            second_graph.clone(),
            "vpc-shared1",
            subnets(MAX_SUBNET_IDS),
        ),
    ];

    let mut stored: Vec<(&str, String, &str, Value)> = Vec::new();
    let mut minted: Vec<String> = Vec::new();
    for (case, graph_id, vpc_id, subnet_ids) in fixtures {
        let body = endpoint_in_service(&router, &graph_id, vpc_id, &subnet_ids).await;
        let vpc_endpoint_id = body["vpcEndpointId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no vpcEndpointId in {body}"))
            .to_string();
        assert!(
            !minted.contains(&vpc_endpoint_id),
            "case {case:?}: expected a distinct endpoint, got the already-stored \
             {vpc_endpoint_id:?} among {minted:?}"
        );
        minted.push(vpc_endpoint_id);
        stored.push((case, graph_id, vpc_id, body));
    }

    // Hammer the first endpoint's read...
    for read_number in 1..=(READS_PER_CASE * 4) {
        let (status, read) = get_endpoint(&router, &first_graph, "vpc-shared1").await;
        assert_eq!(
            status, 200,
            "case the read endpoint, read {read_number}: expected 200, got {status}: {read}"
        );
    }

    // ...and every endpoint, the one that was read and the two that were not, still
    // reports exactly the record that was stored for it.
    for (case, graph_id, vpc_id, expected) in &stored {
        let (status, read) = get_endpoint(&router, graph_id.as_str(), vpc_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {read}"
        );
        assert_eq!(
            &read, expected,
            "case {case:?}: expected the stored record {expected}, got {read}"
        );
    }
}
