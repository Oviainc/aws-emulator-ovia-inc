// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListPrivateGraphEndpoints`, driven over the wire (`restJson1`)
//! through the same router the emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * The listing REPORTS the stored endpoints of the ONE graph it names: every entry
//!   carries the same members an independent `GetPrivateGraphEndpoint` read of that
//!   endpoint answers, and nothing a request fabricated. An endpoint of another graph —
//!   even in the same VPC — is not reported, and a graph holding none lists an empty
//!   list rather than failing.
//! * `graphIdentifier` has to name a graph of the Region the request was signed for: an
//!   identifier naming no such graph is a failed lookup (404) rather than an empty page.
//! * The `@paginated` contract: a page carries at most `maxResults` entries, a
//!   `nextToken` is issued only while entries remain, and resuming from it returns the
//!   remainder without repeats or omissions, in the service's one stable order.
//! * A `nextToken` this service did not issue names no page and is refused (400).
//! * Listing changes nothing a later caller sees: the only move it may materialise is
//!   the one the clock already owed, so repeated listings report the same records and
//!   every endpoint stays readable afterwards.
//!
//! Each table below spans what its rule generalises over — how many endpoints a graph
//! holds, the page size against that population, the form and length of the VPC id, the
//! subnet count up to the published bound, which Region signs the call, and the form of
//! a rejected identifier or token — rather than re-running one recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    ENDPOINT_DELETION_SPAN, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region most of these tests sign for.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// The most subnets one endpoint may name: `SubnetIds` is `@length(1..6)`
/// (`smithy_query(query="shape",shape="SubnetIds")`).
const MAX_SUBNET_IDS: usize = 6;

/// Every member the modelled `PrivateGraphEndpointSummary` carries
/// (`smithy_query(query="shape",shape="PrivateGraphEndpointSummary")`), which is also
/// every member `GetPrivateGraphEndpointOutput` carries: a listed entry must equal the
/// stored record member for member.
const SUMMARY_MEMBERS: &[&str] = &["vpcId", "subnetIds", "status", "vpcEndpointId"];

/// The members the modelled summary marks **required**, so an entry missing one is not
/// the modelled summary at all.
const REQUIRED_SUMMARY_MEMBERS: &[&str] = &["vpcId", "subnetIds", "status"];

/// An emulator holding only this service, on its own engine, so each test starts from
/// an empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request signed for `region` and return its status and
/// parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
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
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/neptune-graph/\
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

/// Poll one resource until it reports `AVAILABLE`, and report the body that did. No
/// handler ever sleeps; only this test does, because it is asking about a state the
/// clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, region: &str, path: &str) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, region, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {path}: expected 200 from the read, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, region, "GET", path, None).await;
    panic!(
        "case {path}: never became AVAILABLE, last status {}",
        body["status"]
    );
}

/// Create a graph in `region` and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
    let (status, created) = call(router, region, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: no id in {created}"))
        .to_string();
    wait_until_available(router, region, &format!("/graphs/{id}")).await;
    id
}

/// `n` distinct subnet ids matching the modelled `SubnetId` pattern
/// `^subnet-[a-z0-9]+$`.
fn subnets(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("subnet-0a1b2c3d4e5f{i:05}")).collect()
}

/// `POST /graphs/{graphIdentifier}/endpoints/` — accept one endpoint, without waiting
/// for the provisioning the clock owes it.
async fn create_endpoint(
    router: &MultiServiceRouter,
    region: &str,
    graph_id: &str,
    vpc_id: &str,
    subnet_ids: &[String],
) -> Value {
    let body = json!({"vpcId": vpc_id, "subnetIds": subnet_ids});
    let (status, created) = call(
        router,
        region,
        "POST",
        &format!("/graphs/{graph_id}/endpoints/"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {graph_id}/{vpc_id}: expected 201 from the create, got {status}: {created}"
    );
    created
}

/// Create one endpoint and wait it into service, returning the body the read settled
/// on.
async fn endpoint_in_service(
    router: &MultiServiceRouter,
    region: &str,
    graph_id: &str,
    vpc_id: &str,
    subnet_ids: &[String],
) -> Value {
    create_endpoint(router, region, graph_id, vpc_id, subnet_ids).await;
    wait_until_available(
        router,
        region,
        &format!("/graphs/{graph_id}/endpoints/{vpc_id}"),
    )
    .await
}

/// `GET /graphs/{graphIdentifier}/endpoints/` with the query string `query` (already
/// encoded, without the `?`).
async fn list(
    router: &MultiServiceRouter,
    region: &str,
    graph_id: &str,
    query: &str,
) -> (u16, Value) {
    let path = if query.is_empty() {
        format!("/graphs/{graph_id}/endpoints/")
    } else {
        format!("/graphs/{graph_id}/endpoints/?{query}")
    };
    call(router, region, "GET", &path, None).await
}

/// The `privateGraphEndpoints` of a successful listing.
fn entries(case: &str, status: u16, body: &Value) -> Vec<Value> {
    assert_eq!(status, 200, "case {case}: expected 200, got {status}: {body}");
    body["privateGraphEndpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: privateGraphEndpoints is required, got {body}"))
        .clone()
}

/// Walk every page of one graph's listing, returning the VPC ids in the order they
/// were reported. Asserts the `@paginated` contract on each page: at most `page_size`
/// entries, a token only after a FULL page, and no VPC id twice.
async fn page_through(
    router: &MultiServiceRouter,
    region: &str,
    graph_id: &str,
    page_size: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=64 {
        let mut query = format!("maxResults={page_size}");
        if let Some(value) = &token {
            query = format!("{query}&nextToken={value}");
        }
        let (status, body) = list(router, region, graph_id, &query).await;
        let page_case = format!("{graph_id}, page size {page_size}, page {page}");
        let listed = entries(&page_case, status, &body);
        assert!(
            listed.len() <= page_size,
            "case {page_case}: expected at most {page_size} entries, got {}",
            listed.len()
        );
        for entry in &listed {
            let vpc_id = entry["vpcId"].as_str().unwrap_or_default().to_string();
            assert!(
                !seen.contains(&vpc_id),
                "case {page_case}: vpcId {vpc_id:?} was reported twice"
            );
            seen.push(vpc_id);
        }
        token = body["nextToken"].as_str().map(ToString::to_string);
        match &token {
            Some(_) => assert_eq!(
                listed.len(),
                page_size,
                "case {page_case}: a token was issued after a short page"
            ),
            None => break,
        }
    }
    assert!(
        token.is_none(),
        "case {graph_id}, page size {page_size}: paging never finished, token {token:?}"
    );
    seen
}

#[tokio::test]
async fn the_listing_reports_the_stored_endpoints_of_the_named_graph() {
    let router = emulator();
    let graph = graph_in_service(&router, REGION, "listing-source").await;

    // A graph holding no endpoint lists an empty list and issues no token: the required
    // member is still present.
    let (status, body) = list(&router, REGION, &graph, "").await;
    let listed = entries("a graph with no endpoint", status, &body);
    assert!(
        listed.is_empty(),
        "case a graph with no endpoint: expected no entries, got {listed:?}"
    );
    assert_eq!(
        body["nextToken"],
        Value::Null,
        "case a graph with no endpoint: expected no token, got {}",
        body["nextToken"]
    );

    // (case, vpcId, subnetIds) — the rows differ in the form and length of the VPC id
    // (the modelled `^vpc-[a-z0-9]+$` bounds neither) and in the subnet count, from one
    // up to the published bound, so the rule is asserted over the class.
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        ("one subnet", "vpc-listed01", subnets(1)),
        (
            "the longest form of the vpc id",
            "vpc-0123456789abcdef0",
            subnets(3),
        ),
        ("the shortest form of the vpc id", "vpc-7", subnets(2)),
        (
            "subnets at the published bound",
            "vpc-zz99",
            subnets(MAX_SUBNET_IDS),
        ),
    ];

    let mut created: BTreeSet<String> = BTreeSet::new();
    for (case, vpc_id, subnet_ids) in cases {
        endpoint_in_service(&router, REGION, &graph, vpc_id, &subnet_ids).await;
        created.insert(vpc_id.to_string());

        let (status, body) = list(&router, REGION, &graph, "").await;
        let listed = entries(case, status, &body);
        assert_eq!(
            listed.len(),
            created.len(),
            "case {case:?}: expected {} entries, got {}",
            created.len(),
            listed.len()
        );

        // Every entry is the STORED record: an independent read of the same endpoint
        // answers the same members, so the listing cannot drift from the store.
        for entry in &listed {
            let keys: Vec<&String> = entry
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: an entry is not an object: {entry}"))
                .keys()
                .collect();
            assert!(
                keys.iter()
                    .all(|key| SUMMARY_MEMBERS.contains(&key.as_str())),
                "case {case:?}: expected only the modelled members {SUMMARY_MEMBERS:?}, got \
                 {keys:?}"
            );
            for member in REQUIRED_SUMMARY_MEMBERS {
                assert!(
                    keys.iter().any(|key| key.as_str() == *member),
                    "case {case:?}: expected the required member {member:?}, got {keys:?}"
                );
            }
            let entry_vpc = entry["vpcId"].as_str().unwrap_or_default();
            let (read_status, read) = call(
                &router,
                REGION,
                "GET",
                &format!("/graphs/{graph}/endpoints/{entry_vpc}"),
                None,
            )
            .await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: expected 200 from GetPrivateGraphEndpoint for {entry_vpc:?}, \
                 got {read_status}: {read}"
            );
            for member in SUMMARY_MEMBERS {
                assert_eq!(
                    entry[member], read[member],
                    "case {case:?}: the listed {member} of {entry_vpc:?} must be the stored \
                     value {}, got {}",
                    read[member], entry[member]
                );
            }
        }

        // One stable total order underlies every page, which is what lets a token name
        // a position in it.
        let reported: Vec<String> = listed
            .iter()
            .map(|entry| entry["vpcId"].as_str().unwrap_or_default().to_string())
            .collect();
        let mut sorted = reported.clone();
        sorted.sort();
        assert_eq!(
            reported, sorted,
            "case {case:?}: expected the listing in vpcId order, got {reported:?}"
        );
        assert_eq!(
            reported.iter().cloned().collect::<BTreeSet<String>>(),
            created,
            "case {case:?}: the listing did not report every stored endpoint"
        );
    }

    // An endpoint is reported while it is still CREATING too: the listing reports the
    // store, not only its terminal rows.
    create_endpoint(&router, REGION, &graph, "vpc-pending01", &subnets(1)).await;
    let (status, body) = list(&router, REGION, &graph, "").await;
    let listed = entries("an endpoint still CREATING", status, &body);
    let pending = listed
        .iter()
        .find(|entry| entry["vpcId"] == json!("vpc-pending01"))
        .unwrap_or_else(|| panic!("case an endpoint still CREATING: not listed in {body}"));
    assert_eq!(
        pending["status"],
        json!("CREATING"),
        "case an endpoint still CREATING: expected CREATING, got {}",
        pending["status"]
    );
}

#[tokio::test]
async fn the_listing_spans_one_graph_only() {
    let router = emulator();

    // (the graph's name, the VPC ids its endpoints live in) — a graph with several, a
    // graph with one, and a graph with none, so the scoping is tested against each
    // count. `vpc-shared1` appears on two graphs, because the same VPC id against
    // another graph names another endpoint.
    let population: Vec<(&str, Vec<&str>)> = vec![
        ("busy-graph", vec!["vpc-shared1", "vpc-busy02", "vpc-busy03"]),
        ("quiet-graph", vec!["vpc-shared1"]),
        ("bare-graph", Vec::new()),
    ];

    let mut graphs: Vec<(&str, String, BTreeSet<String>)> = Vec::new();
    for (name, vpc_ids) in &population {
        let graph = graph_in_service(&router, REGION, name).await;
        let mut owned = BTreeSet::new();
        for vpc_id in vpc_ids {
            endpoint_in_service(&router, REGION, &graph, vpc_id, &subnets(2)).await;
            owned.insert((*vpc_id).to_string());
        }
        graphs.push((name, graph, owned));
    }

    // Each graph reports exactly its OWN endpoints, at a page size below its population
    // as well as above it, so the scoping survives a resumed page.
    for (name, graph, owned) in &graphs {
        for page_size in [1_usize, 100] {
            let seen = page_through(&router, REGION, graph, page_size).await;
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<String>>(),
                *owned,
                "case {name} ({graph}), page size {page_size}: expected {owned:?}, got {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_listing_pages_over_every_endpoint_without_repeats() {
    let router = emulator();
    let graph = graph_in_service(&router, REGION, "paging-source").await;

    const POPULATION: usize = 7;
    let mut all_vpcs: BTreeSet<String> = BTreeSet::new();
    for index in 0..POPULATION {
        let vpc_id = format!("vpc-page{index}");
        endpoint_in_service(&router, REGION, &graph, &vpc_id, &subnets(1 + index % MAX_SUBNET_IDS))
            .await;
        all_vpcs.insert(vpc_id);
    }

    // The `@paginated` contract over every interesting page size: 1, sizes that do not
    // divide the population, exactly the population, and the largest `maxResults`
    // allows (`range 1..100`).
    for page_size in [1_usize, 2, 3, POPULATION, 100] {
        let seen = page_through(&router, REGION, &graph, page_size).await;
        assert_eq!(
            seen.len(),
            POPULATION,
            "case page size {page_size}: expected {POPULATION} endpoints in total, got {}",
            seen.len()
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            all_vpcs,
            "case page size {page_size}: the pages did not cover every stored endpoint"
        );
    }

    // A token that names a position in the VPC-id space resumes from that position,
    // even though no endpoint was ever created in that VPC: a page resumes after a
    // POSITION, so an endpoint deleted between two pages cannot hide the endpoints that
    // follow it.
    let position = "vpc-page3";
    let unissued = hex::encode(position);
    let (status, body) = list(&router, REGION, &graph, &format!("nextToken={unissued}")).await;
    let listed = entries("an unissued position", status, &body);
    assert!(
        !listed.is_empty(),
        "case an unissued position: expected the endpoints after {position:?}, got none"
    );
    for entry in &listed {
        let vpc_id = entry["vpcId"].as_str().unwrap_or_default();
        assert!(
            vpc_id > position,
            "case an unissued position: {vpc_id:?} does not sort after {position:?}"
        );
    }
}

#[tokio::test]
async fn the_listing_refuses_an_identifier_or_token_it_cannot_resolve() {
    let router = emulator();
    let graph = graph_in_service(&router, REGION, "refusal-source").await;
    endpoint_in_service(&router, REGION, &graph, "vpc-refusal1", &subnets(1)).await;

    // (case, the graphIdentifier the request carries, status, error code)
    let identifier_cases: Vec<(&str, &str, u16, &str)> = vec![
        ("a stored graph", &graph, 200, ""),
        (
            "a well-formed graph id that was never created",
            "g-zzzzzzzzzz",
            404,
            "ResourceNotFoundException",
        ),
        (
            "another well-formed absent graph id",
            "g-0000000000",
            404,
            "ResourceNotFoundException",
        ),
        (
            "an identifier outside the modelled pattern",
            "not-a-graph-id",
            400,
            "ValidationException",
        ),
        (
            "a snapshot id where a graph id belongs",
            "gs-abcdefghij",
            400,
            "ValidationException",
        ),
    ];
    for (case, identifier, expected, expected_code) in identifier_cases {
        let (status, body) = list(&router, REGION, identifier, "").await;
        assert_eq!(
            status, expected,
            "case {case:?} ({identifier:?}): expected {expected}, got {status}: {body}"
        );
        if !expected_code.is_empty() {
            let code = error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?} ({identifier:?}): expected {expected_code}, got {code:?}: {body}"
            );
        }
    }

    // A token this service never issued names no page: it is refused rather than
    // treated as the first page. The rows differ in form — not hex at all, hex of a
    // value that is no VPC id, and hex of a position in the GRAPH id space.
    for token in [
        "zzz",
        "not-a-token",
        "deadbeefdeadbeef",
        &hex::encode("not-a-vpc-id"),
        &hex::encode("vpc-"),
        &hex::encode("g-0123456789"),
    ] {
        let (status, body) = list(&router, REGION, &graph, &format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case token {token:?}: expected 400, got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "ValidationException",
            "case token {token:?}: expected ValidationException, got {code:?}: {body}"
        );
    }

    // `maxResults` is modelled `range 1..100`: each published bound is served and one
    // step past it is refused at the boundary.
    for (max_results, expected) in [("1", 200), ("100", 200), ("0", 400), ("101", 400)] {
        let (status, body) =
            list(&router, REGION, &graph, &format!("maxResults={max_results}")).await;
        assert_eq!(
            status, expected,
            "case maxResults {max_results}: expected {expected}, got {status}: {body}"
        );
    }
}

#[tokio::test]
async fn an_endpoint_is_listed_only_from_its_own_region() {
    let router = emulator();

    // Two Regions holding a graph with an endpoint each, and one holding a graph with
    // none: the same VPC id is used in each, so only the Region scoping can tell the
    // listings apart.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];
    let mut owned: Vec<(&str, String, Option<&str>)> = Vec::new();
    for region in regions {
        let graph = graph_in_service(&router, region, "regional-source").await;
        let vpc_id = if region == "ap-southeast-2" {
            None
        } else {
            endpoint_in_service(&router, region, &graph, "vpc-regional1", &subnets(1)).await;
            Some("vpc-regional1")
        };
        owned.push((region, graph, vpc_id));
    }

    for (region, graph, mine) in &owned {
        let (status, body) = list(&router, region, graph, "").await;
        let listed = entries(region, status, &body);
        let expected: Vec<String> = mine.iter().map(|vpc| (*vpc).to_string()).collect();
        let actual: Vec<String> = listed
            .iter()
            .map(|entry| entry["vpcId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            actual, expected,
            "case the listing signed for {region}: expected {expected:?}, got {actual:?}"
        );

        // A graph of another Region does not exist here, so a listing of its endpoints
        // is a failed lookup rather than an empty page.
        for (other, other_graph, _) in &owned {
            if other == region {
                continue;
            }
            let (status, body) = list(&router, region, other_graph, "").await;
            assert_eq!(
                status, 404,
                "case {other_graph} of {other} listed from {region}: expected 404, got \
                 {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {other_graph} of {other} listed from {region}: expected \
                 ResourceNotFoundException, got {code:?}: {body}"
            );
        }
    }
}

#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let graph = graph_in_service(&router, REGION, "stability-source").await;

    // (case, vpcId, subnetIds) — the rows differ in the VPC id's form and the subnet
    // count, so the rule is asserted over the class and not over one request.
    let cases: Vec<(&str, &str, Vec<String>)> = vec![
        ("one subnet", "vpc-stable01", subnets(1)),
        ("three subnets", "vpc-9", subnets(3)),
        (
            "subnets at the published bound",
            "vpc-0a1b2c3d4e5f67891",
            subnets(MAX_SUBNET_IDS),
        ),
    ];
    for (_, vpc_id, subnet_ids) in &cases {
        endpoint_in_service(&router, REGION, &graph, vpc_id, subnet_ids).await;
    }

    // However many times the listing is called, it reports the same records...
    let (status, first) = list(&router, REGION, &graph, "").await;
    let baseline = entries("the first listing", status, &first);
    for round in 1..=5 {
        let (status, body) = list(&router, REGION, &graph, "").await;
        let listed = entries(&format!("listing {round}"), status, &body);
        assert_eq!(
            listed, baseline,
            "case listing {round}: expected the stored records {baseline:?}, got {listed:?}"
        );
        assert_eq!(
            body["nextToken"],
            Value::Null,
            "case listing {round}: expected no token for a single page, got {}",
            body["nextToken"]
        );
    }

    // ...and every endpoint is still readable afterwards, with the record the listing
    // reported: a listing mints nothing, consumes nothing and deletes nothing.
    for (case, vpc_id, _) in &cases {
        let (status, read) = call(
            &router,
            REGION,
            "GET",
            &format!("/graphs/{graph}/endpoints/{vpc_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read after the listings, got {status}: {read}"
        );
        let listed = baseline
            .iter()
            .find(|entry| entry["vpcId"] == json!(vpc_id))
            .unwrap_or_else(|| panic!("case {case:?}: {vpc_id} was not listed in {baseline:?}"));
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                read[member], listed[member],
                "case {case:?}: expected the listed {member} {}, got {}",
                listed[member], read[member]
            );
        }
    }

    // The one move a listing may materialise is the one the clock already owed: an
    // accepted deletion is reported as DELETING until its span has passed, and is gone
    // from the listing afterwards — never earlier because something listed it.
    let accepted_at = Instant::now();
    let (status, deleted) = call(
        &router,
        REGION,
        "DELETE",
        &format!("/graphs/{graph}/endpoints/vpc-stable01"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case the accepted deletion: expected 200, got {status}: {deleted}"
    );
    assert_eq!(
        deleted["status"],
        json!("DELETING"),
        "case the accepted deletion: expected DELETING from the delete, got {}",
        deleted["status"]
    );

    // However many times the listing is called, the endpoint leaves it no earlier than
    // the span the clock owes the deletion, and reports DELETING for as long as it is
    // still listed: listing neither hastens the reclaim nor postpones it.
    let mut listings_while_deleting = 0usize;
    loop {
        let (status, body) = list(&router, REGION, &graph, "").await;
        let listed = entries("while the deletion is outstanding", status, &body);
        let Some(entry) = listed
            .iter()
            .find(|entry| entry["vpcId"] == json!("vpc-stable01"))
        else {
            break;
        };
        assert_eq!(
            entry["status"],
            json!("DELETING"),
            "case listing {listings_while_deleting} while the deletion is outstanding: \
             expected DELETING, got {}",
            entry["status"]
        );
        listings_while_deleting += 1;
        assert!(
            listings_while_deleting < 1_000,
            "case while the deletion is outstanding: the endpoint was still listed after \
             {listings_while_deleting} listings"
        );
        tokio::time::sleep(ENDPOINT_DELETION_SPAN / 20).await;
    }
    let elapsed = accepted_at.elapsed();
    assert!(
        elapsed >= ENDPOINT_DELETION_SPAN,
        "case once the deletion is due: expected the endpoint to leave the listing no \
         earlier than {ENDPOINT_DELETION_SPAN:?} after the delete, got {elapsed:?} over \
         {listings_while_deleting} listings"
    );
    assert!(
        listings_while_deleting > 0,
        "case while the deletion is outstanding: the endpoint was never reported DELETING \
         by the listing"
    );

    // And every other endpoint is still listed: the reclaim took exactly the one record
    // the delete named.
    let (status, body) = list(&router, REGION, &graph, "").await;
    let listed = entries("once the deletion is due", status, &body);
    let still_there: Vec<String> = listed
        .iter()
        .map(|entry| entry["vpcId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        !still_there.contains(&"vpc-stable01".to_string()),
        "case once the deletion is due: expected the endpoint gone, got {still_there:?}"
    );
    assert_eq!(
        still_there.len(),
        cases.len() - 1,
        "case once the deletion is due: expected the other {} endpoints, got {still_there:?}",
        cases.len() - 1
    );
}
