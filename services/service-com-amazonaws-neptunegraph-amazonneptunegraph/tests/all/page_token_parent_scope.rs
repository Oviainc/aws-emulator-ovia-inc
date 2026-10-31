// SPDX-License-Identifier: CC0-1.0

//! Which PARENT a `nextToken` of a per-parent listing belongs to, driven over the wire
//! (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: a pagination token names a position in the
//! order of the ONE listing scope it was minted for, and `ListPrivateGraphEndpoints`
//! pages the endpoints held under one graph — a separate order per graph, keyed by a VPC
//! id that is a legal key under every graph — so the graph is part of that scope. A token
//! minted while paging one graph's endpoints therefore names no position in another
//! graph's order: it is a value this service never issued for the page it was sent to, so
//! it is refused with the modelled `ValidationException` (400, `reason`
//! `ILLEGAL_ARGUMENT`) rather than used as a cut point in a collection it was never taken
//! from, which would report a short page — or an empty one with no `nextToken`, reading
//! as "this graph has no endpoints" — for a graph whose endpoints sort at or before it.
//!
//! The table spans what the rule generalises over: three graphs holding different
//! populations whose VPC ids deliberately OVERLAP and span the forms of the modelled
//! `VpcId` pattern `^vpc-[a-z0-9]+$` (one character after the prefix, letters, and the
//! long digit form), every ORDERED PAIR of distinct graphs for the minting and the asked
//! graph, and every page size from one to past the population. Each graph is also asked
//! with the token IT minted, so the refusals are about the scope the token belongs to and
//! not about pagination being refused outright, and after every refusal the asked graph's
//! own listing is walked again to prove no entry was dropped.
//!
//! Which LISTING a token belongs to is `tests/all/page_token_listing_scope.rs`'s rule and
//! which SPELLING of it is accepted is `tests/all/page_token_spelling.rs`'s; here it is
//! only which PARENT within one listing.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::PROVISIONING_SPAN;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region these tests sign for.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// The modelled `reason` a token the caller sent that this service cannot resolve to a
/// position is classified by, from `ValidationExceptionReason`'s values
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`): the request named
/// an argument this operation does not accept.
const ILLEGAL_ARGUMENT: &str = "ILLEGAL_ARGUMENT";

/// One graph of the table: its name, and the VPC ids its endpoints are addressed by.
///
/// The ids overlap across graphs on purpose — the same VPC id is legal under every graph
/// — and span the forms the modelled `VpcId` pattern `^vpc-[a-z0-9]+$` admits
/// (`smithy_query(query="shape",shape="VpcId")`): one character after the prefix,
/// letters, and the long digit form an account's VPC ids actually take. Every graph holds
/// at least two, so a page of one leaves an entry behind and a token is minted.
const POPULATIONS: [(&str, &[&str]); 3] = [
    (
        "scope-host-a",
        &[
            "vpc-0a1b2c3d4e5f00001",
            "vpc-0a1b2c3d4e5f00002",
            "vpc-0a1b2c3d4e5f00003",
        ],
    ),
    // Its whole order sits at or before the cut points the other graphs mint, which is
    // what a cross-graph cursor would silently drop.
    ("scope-host-b", &["vpc-0a1b2c3d4e5f00001", "vpc-zz9"]),
    (
        "scope-host-c",
        &["vpc-0", "vpc-0a1b2c3d4e5f00001", "vpc-abcdef"],
    ),
];

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

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this test
/// does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, path: &str) {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {path}: expected 200 from the read, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return;
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
    let body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
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

/// Create one endpoint of `graph_id` in `vpc_id` and wait it into service.
async fn endpoint_in_service(router: &MultiServiceRouter, graph_id: &str, vpc_id: &str) {
    let body = json!({"vpcId": vpc_id, "subnetIds": ["subnet-0a1b2c3d4e5f00000"]});
    let (status, created) = call(
        router,
        "POST",
        &format!("/graphs/{graph_id}/endpoints/"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {graph_id}/{vpc_id}: expected 201 from the create, got {status}: {created}"
    );
    wait_until_available(router, &format!("/graphs/{graph_id}/endpoints/{vpc_id}")).await;
}

/// `GET /graphs/{graphIdentifier}/endpoints/` with the query string `query` (already
/// encoded, without the `?`).
async fn list(router: &MultiServiceRouter, graph_id: &str, query: &str) -> (u16, Value) {
    let path = if query.is_empty() {
        format!("/graphs/{graph_id}/endpoints/")
    } else {
        format!("/graphs/{graph_id}/endpoints/?{query}")
    };
    call(router, "GET", &path, None).await
}

/// The VPC ids of a successful listing, in the order they were reported.
fn listed_vpc_ids(case: &str, status: u16, body: &Value) -> Vec<String> {
    assert_eq!(status, 200, "case {case}: expected 200, got {status}: {body}");
    body["privateGraphEndpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: privateGraphEndpoints is required, got {body}"))
        .iter()
        .map(|entry| {
            entry["vpcId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: an entry carries no vpcId, got {entry}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of one graph's listing at `page_size`, returning the VPC ids in the
/// order they were reported.
async fn page_through(
    router: &MultiServiceRouter,
    graph_id: &str,
    page_size: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=16 {
        let mut query = format!("maxResults={page_size}");
        if let Some(value) = &token {
            query = format!("{query}&nextToken={value}");
        }
        let (status, body) = list(router, graph_id, &query).await;
        let case = format!("{graph_id}, page size {page_size}, page {page}");
        let listed = listed_vpc_ids(&case, status, &body);
        assert!(
            listed.len() <= page_size,
            "case {case}: expected at most {page_size} entries, got {listed:?}"
        );
        seen.extend(listed);
        token = body["nextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            return seen;
        }
    }
    panic!("case {graph_id}, page size {page_size}: paging never finished, got {seen:?}");
}

/// The `nextToken` the first page of `graph_id`'s listing at `page_size` carries.
async fn minted_token(router: &MultiServiceRouter, graph_id: &str, page_size: usize) -> String {
    let (status, body) = list(router, graph_id, &format!("maxResults={page_size}")).await;
    let case = format!("{graph_id} first page of {page_size}");
    listed_vpc_ids(&case, status, &body);
    body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a nextToken while entries remain, got {body}"))
        .to_string()
}

/// Ask `graph_id`'s listing with `token` and assert it is refused as a token this service
/// never issued for that page: the modelled 400, naming the value the caller sent.
async fn assert_refused(router: &MultiServiceRouter, graph_id: &str, case: &str, token: &str) {
    let (status, answer) = list(router, graph_id, &format!("nextToken={token}")).await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400, got {status}: {answer}"
    );
    let code = error_code(&answer);
    assert_eq!(
        code, "ValidationException",
        "case {case}: expected ValidationException, got {code:?}: {answer}"
    );
    let expected = format!("nextToken '{token}' is not a pagination token this service issued");
    let actual = answer["message"].as_str().unwrap_or_default();
    assert_eq!(
        actual, expected,
        "case {case}: expected {expected:?}, got {actual:?}"
    );
    let reason = answer["reason"].as_str();
    assert_eq!(
        reason,
        Some(ILLEGAL_ARGUMENT),
        "case {case}: expected reason {ILLEGAL_ARGUMENT:?}, got {reason:?} in {answer}"
    );
}

/// Create every graph of the table with its endpoints, returning `(name, id, vpc ids)`
/// per graph.
async fn populated(router: &MultiServiceRouter) -> Vec<(&'static str, String, Vec<String>)> {
    let mut graphs = Vec::new();
    for (name, vpc_ids) in POPULATIONS {
        let id = graph_in_service(router, name).await;
        for vpc_id in vpc_ids {
            endpoint_in_service(router, &id, vpc_id).await;
        }
        graphs.push((
            name,
            id,
            vpc_ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ));
    }
    graphs
}

#[tokio::test]
async fn one_graphs_endpoint_token_names_no_page_of_another_graphs_endpoints() {
    let router = emulator();
    let graphs = populated(&router).await;

    for (minting_name, minting_graph, minting_vpc_ids) in &graphs {
        // Every page size the minting graph can issue a token at: 1 up to one less than
        // its population, so an entry always remains after the page and a token is minted.
        for page_size in 1..minting_vpc_ids.len() {
            let token = minted_token(&router, minting_graph, page_size).await;
            for (asked_name, asked_graph, asked_vpc_ids) in &graphs {
                let case = format!(
                    "a token minted on {minting_name} at page size {page_size} sent to \
                     {asked_name}"
                );
                if asked_graph == minting_graph {
                    // The graph that minted it takes it back: the token names a position
                    // in ITS order, so the page resumes after that position.
                    let (status, body) = list(
                        &router,
                        asked_graph,
                        &format!("maxResults={page_size}&nextToken={token}"),
                    )
                    .await;
                    let resumed = listed_vpc_ids(&case, status, &body);
                    let expected: Vec<String> = asked_vpc_ids
                        .iter()
                        .skip(page_size)
                        .take(page_size)
                        .cloned()
                        .collect();
                    assert_eq!(
                        resumed, expected,
                        "case {case}: expected the entries after the page, got {resumed:?}"
                    );
                    continue;
                }

                assert_refused(&router, asked_graph, &case, &token).await;

                // Nothing of the asked graph was dropped by the refusal: its own listing
                // still reports every endpoint it holds, at every page size from one to
                // past its population.
                for asked_page_size in 1..=asked_vpc_ids.len() + 1 {
                    let seen = page_through(&router, asked_graph, asked_page_size).await;
                    assert_eq!(
                        seen.iter().cloned().collect::<BTreeSet<String>>(),
                        asked_vpc_ids.iter().cloned().collect::<BTreeSet<String>>(),
                        "case {case}, then paging {asked_name} at {asked_page_size}: expected \
                         {asked_vpc_ids:?}, got {seen:?}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn an_endpoint_token_carries_the_graph_whose_order_it_names_a_position_in() {
    let router = emulator();
    let graphs = populated(&router).await;

    for (name, graph_id, vpc_ids) in &graphs {
        // The token a page issues is the hex encoding of this listing, the graph it pages
        // and the last entry the page reported — the position within THAT graph's order.
        let token = minted_token(&router, graph_id, 1).await;
        let first = &vpc_ids[0];
        let expected = hex::encode(format!("ListPrivateGraphEndpoints:{graph_id}:{first}"));
        assert_eq!(
            token, expected,
            "case {name}: expected the token to carry the listing, {graph_id} and {first:?}, \
             got {token:?}"
        );

        // Forms of the same position that name no page of this graph's listing: the
        // position alone, the listing and the position without the graph whose order it
        // lies in, the listing under a graph that was never created, and the listing
        // under another graph of the store.
        let other_graph = graphs
            .iter()
            .find(|(_, candidate, _)| candidate != graph_id)
            .map(|(_, candidate, _)| candidate.clone())
            .unwrap_or_else(|| panic!("case {name}: the table holds only one graph"));
        for (form, foreign) in [
            ("the position alone", hex::encode(first)),
            (
                "the listing and the position, with no graph",
                hex::encode(format!("ListPrivateGraphEndpoints:{first}")),
            ),
            (
                "the listing under a graph that was never created",
                // A well-formed `GraphIdentifier` (`^g-[a-z0-9]{10}$`) this store holds
                // nothing under.
                hex::encode(format!("ListPrivateGraphEndpoints:g-zzzzzzzzzz:{first}")),
            ),
            (
                "the listing under another graph of the store",
                hex::encode(format!("ListPrivateGraphEndpoints:{other_graph}:{first}")),
            ),
        ] {
            let case = format!("{name} asked with {form}: {foreign:?}");
            assert_refused(&router, graph_id, &case, &foreign).await;
        }
    }
}
