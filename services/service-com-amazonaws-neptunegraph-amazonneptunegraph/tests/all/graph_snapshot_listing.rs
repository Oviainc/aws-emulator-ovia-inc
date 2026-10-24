// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListGraphSnapshots`, driven over the wire (`restJson1`)
//! through the same router the emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * The listing REPORTS the stored snapshots of the Region the request was signed
//!   for: every entry carries the same members an independent `GetGraphSnapshot` read
//!   of that snapshot answers, and nothing a request fabricated.
//! * `graphIdentifier` narrows the listing to the snapshots taken from that one graph;
//!   absent, the listing spans every snapshot of the Region. An identifier naming no
//!   graph of this Region is a failed lookup (404) rather than an empty page.
//! * The `@paginated` contract: a page carries at most `maxResults` entries, a
//!   `nextToken` is issued only while entries remain, and resuming from it returns the
//!   remainder without repeats or omissions, in the service's one stable order.
//! * A `nextToken` this service did not issue names no page and is refused (400).
//!
//! Each table below spans what its rule generalises over — the page size against the
//! population, how many snapshots a graph holds, whether the request names a graph,
//! which Region signs the call, and the form of a rejected identifier or token —
//! rather than re-running one recorded request.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::PROVISIONING_SPAN;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region most of these tests sign for.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// The members `GraphSnapshotSummary` shares with the snapshot record a
/// `GetGraphSnapshot` read answers
/// (`smithy_query(query="shape",shape="GraphSnapshotSummary")`): every member of the
/// summary, so a listed entry must equal the stored record member for member.
const SUMMARY_MEMBERS: &[&str] = &[
    "id",
    "name",
    "arn",
    "sourceGraphId",
    "snapshotCreateTime",
    "status",
    "kmsKeyIdentifier",
];

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

/// Whether `value` matches the modelled `SnapshotId` pattern `^gs-[a-z0-9]{10}$`.
fn is_snapshot_id(value: &str) -> bool {
    value.strip_prefix("gs-").is_some_and(|suffix| {
        suffix.len() == 10
            && suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this
/// test does, because it is asking about a state the clock reaches on its own.
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

/// Snapshot the graph `graph_id` of `region` under `name`, returning the snapshot's id.
/// The source graph is waited back into service, so the next snapshot of it is about
/// the row and not about a transition this one left outstanding.
async fn snapshot(
    router: &MultiServiceRouter,
    region: &str,
    graph_id: &str,
    name: &str,
) -> String {
    let (status, created) = call(
        router,
        region,
        "POST",
        "/snapshots",
        Some(json!({"graphIdentifier": graph_id, "snapshotName": name})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case snapshot {name:?} of {graph_id}: expected 201, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case snapshot {name:?}: no id in {created}"))
        .to_string();
    wait_until_available(router, region, &format!("/graphs/{graph_id}")).await;
    id
}

/// `GET /snapshots` with the query string `query` (already encoded, without the `?`).
async fn list(router: &MultiServiceRouter, region: &str, query: &str) -> (u16, Value) {
    let path = if query.is_empty() {
        "/snapshots".to_string()
    } else {
        format!("/snapshots?{query}")
    };
    call(router, region, "GET", &path, None).await
}

/// The `graphSnapshots` of a successful listing.
fn entries(case: &str, status: u16, body: &Value) -> Vec<Value> {
    assert_eq!(status, 200, "case {case}: expected 200, got {status}: {body}");
    body["graphSnapshots"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: graphSnapshots is required, got {body}"))
        .clone()
}

/// Walk every page of a listing, returning the ids in the order they were reported.
/// Asserts the `@paginated` contract on each page: at most `page_size` entries, a
/// token only after a FULL page, and no id twice.
async fn page_through(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    filter: &str,
    page_size: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=64 {
        let mut query = format!("maxResults={page_size}");
        if !filter.is_empty() {
            query = format!("{filter}&{query}");
        }
        if let Some(value) = &token {
            query = format!("{query}&nextToken={value}");
        }
        let (status, body) = list(router, region, &query).await;
        let page_case = format!("{case}, page size {page_size}, page {page}");
        let listed = entries(&page_case, status, &body);
        assert!(
            listed.len() <= page_size,
            "case {page_case}: expected at most {page_size} entries, got {}",
            listed.len()
        );
        for entry in &listed {
            let id = entry["id"].as_str().unwrap_or_default().to_string();
            assert!(
                is_snapshot_id(&id),
                "case {page_case}: expected an id matching ^gs-[a-z0-9]{{10}}$, got {id:?}"
            );
            assert!(
                !seen.contains(&id),
                "case {page_case}: id {id:?} was reported twice"
            );
            seen.push(id);
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
        "case {case}, page size {page_size}: paging never finished, token {token:?}"
    );
    seen
}

#[tokio::test]
async fn list_graph_snapshots_reports_the_stored_snapshots_of_the_region() {
    let router = emulator();
    let graph = graph_in_service(&router, REGION, "listing-source").await;

    // An empty store lists nothing and issues no token: the required member is still
    // present, as an empty list.
    let (status, body) = list(&router, REGION, "").await;
    let listed = entries("an empty store", status, &body);
    assert!(
        listed.is_empty(),
        "case an empty store: expected no entries, got {listed:?}"
    );
    assert_eq!(
        body["nextToken"],
        Value::Null,
        "case an empty store: expected no token, got {}",
        body["nextToken"]
    );

    // (case, the snapshot's name)
    let cases: Vec<(&str, &str)> = vec![
        ("a plain name", "listed-snapshot-one"),
        ("a single-character name", "a"),
        ("digits in the name", "snapshot9"),
    ];
    let mut created: BTreeSet<String> = BTreeSet::new();
    for (case, name) in cases {
        let id = snapshot(&router, REGION, &graph, name).await;
        created.insert(id.clone());

        let (status, body) = list(&router, REGION, "").await;
        let listed = entries(case, status, &body);
        assert_eq!(
            listed.len(),
            created.len(),
            "case {case:?}: expected {} entries, got {}",
            created.len(),
            listed.len()
        );

        // Every entry is the STORED record: an independent read of the same snapshot
        // answers the same members, so the listing cannot drift from the store.
        for entry in &listed {
            let entry_id = entry["id"].as_str().unwrap_or_default();
            let (read_status, read) =
                call(&router, REGION, "GET", &format!("/snapshots/{entry_id}"), None).await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: expected 200 from GetGraphSnapshot for {entry_id:?}, got \
                 {read_status}: {read}"
            );
            for member in SUMMARY_MEMBERS {
                assert_eq!(
                    entry[member], read[member],
                    "case {case:?}: the listed {member} of {entry_id:?} must be the stored \
                     value {}, got {}",
                    read[member], entry[member]
                );
            }
            assert_eq!(
                entry["sourceGraphId"],
                json!(graph),
                "case {case:?}: expected sourceGraphId {graph:?}, got {}",
                entry["sourceGraphId"]
            );
        }

        // One stable total order underlies every page, which is what lets a token name
        // a position in it.
        let ids: Vec<String> = listed
            .iter()
            .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(
            ids, sorted,
            "case {case:?}: expected the listing in id order, got {ids:?}"
        );
        assert_eq!(
            ids.iter().cloned().collect::<BTreeSet<String>>(),
            created,
            "case {case:?}: the listing did not report every stored snapshot"
        );
    }

    // A snapshot is reported while it is still CREATING too: the listing reports the
    // store, not only its terminal rows.
    let (status, pending) = call(
        &router,
        REGION,
        "POST",
        "/snapshots",
        Some(json!({"graphIdentifier": graph, "snapshotName": "still-creating"})),
    )
    .await;
    assert_eq!(status, 201, "the pending snapshot: got {status}: {pending}");
    let pending_id = pending["id"].as_str().expect("an id").to_string();
    let (status, body) = list(&router, REGION, "").await;
    let listed = entries("a snapshot still CREATING", status, &body);
    let pending_entry = listed
        .iter()
        .find(|entry| entry["id"] == json!(pending_id))
        .unwrap_or_else(|| panic!("case a snapshot still CREATING: {pending_id} is not listed"));
    assert_eq!(
        pending_entry["status"],
        json!("CREATING"),
        "case a snapshot still CREATING: expected CREATING, got {}",
        pending_entry["status"]
    );
}

#[tokio::test]
async fn list_graph_snapshots_pages_over_every_snapshot_without_repeats() {
    let router = emulator();
    let graph = graph_in_service(&router, REGION, "paging-source").await;

    const POPULATION: usize = 7;
    let mut all_ids: BTreeSet<String> = BTreeSet::new();
    for index in 0..POPULATION {
        all_ids.insert(snapshot(&router, REGION, &graph, &format!("page-snapshot-{index}")).await);
    }

    // The `@paginated` contract over every interesting page size: 1, sizes that do not
    // divide the population, exactly the population, and the largest `maxResults`
    // allows (`range 1..100`).
    for page_size in [1_usize, 2, 3, POPULATION, 100] {
        let seen = page_through(&router, REGION, "every snapshot", "", page_size).await;
        assert_eq!(
            seen.len(),
            POPULATION,
            "case page size {page_size}: expected {POPULATION} snapshots in total, got {}",
            seen.len()
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            all_ids,
            "case page size {page_size}: the pages did not cover every stored snapshot"
        );
    }

    // A token that names a position in the snapshot-id space resumes from that
    // position, even though no snapshot was ever minted with that id: a page resumes
    // after a POSITION, so a snapshot deleted between two pages cannot hide the
    // snapshots that follow it.
    let position = "gs-aaaaaaaaaa";
    let unissued = hex::encode(position);
    let (status, body) = list(&router, REGION, &format!("nextToken={unissued}")).await;
    let listed = entries("an unissued position", status, &body);
    for entry in &listed {
        let id = entry["id"].as_str().unwrap_or_default();
        assert!(
            id > position,
            "case an unissued position: {id:?} does not sort after {position:?}"
        );
    }
}

#[tokio::test]
async fn graph_identifier_narrows_the_listing_to_one_graphs_snapshots() {
    let router = emulator();

    // (the graph's name, how many snapshots it holds) — a graph with several, a graph
    // with one, and a graph with none, so the filter is tested against each count.
    let population: Vec<(&str, usize)> = vec![("busy-graph", 3), ("quiet-graph", 1), ("bare-graph", 0)];
    let mut graphs: Vec<(String, BTreeSet<String>)> = Vec::new();
    for (name, count) in &population {
        let graph = graph_in_service(&router, REGION, name).await;
        let mut ids = BTreeSet::new();
        for index in 0..*count {
            ids.insert(snapshot(&router, REGION, &graph, &format!("{name}-snap-{index}")).await);
        }
        graphs.push((graph, ids));
    }
    let every_id: BTreeSet<String> = graphs
        .iter()
        .flat_map(|(_, ids)| ids.iter().cloned())
        .collect();

    // Filtered to each graph, the listing reports exactly that graph's snapshots; with
    // no filter it reports every snapshot of the Region. Each row is paged at a size
    // below its own population too, so the filter survives a resumed page.
    for (graph, ids) in &graphs {
        for page_size in [1_usize, 100] {
            let seen = page_through(
                &router,
                REGION,
                &format!("graphIdentifier={graph}"),
                &format!("graphIdentifier={graph}"),
                page_size,
            )
            .await;
            assert_eq!(
                seen.iter().cloned().collect::<BTreeSet<String>>(),
                *ids,
                "case graphIdentifier={graph}, page size {page_size}: expected {ids:?}, got \
                 {seen:?}"
            );
        }
    }
    for page_size in [1_usize, 2, 100] {
        let seen = page_through(&router, REGION, "no graphIdentifier", "", page_size).await;
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            every_id,
            "case no graphIdentifier, page size {page_size}: expected every snapshot of the \
             Region {every_id:?}, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn list_graph_snapshots_refuses_an_identifier_or_token_it_cannot_resolve() {
    let router = emulator();
    let graph = graph_in_service(&router, REGION, "refusal-source").await;
    snapshot(&router, REGION, &graph, "refusal-snapshot").await;

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
        let (status, body) = list(&router, REGION, &format!("graphIdentifier={identifier}")).await;
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
    // value that is no snapshot id, and hex of a position in the GRAPH id space.
    for token in [
        "zzz",
        "not-a-token",
        "deadbeefdeadbeef",
        &hex::encode("not-a-snapshot-id"),
        &hex::encode("gs-short"),
        &hex::encode("g-0123456789"),
    ] {
        let (status, body) = list(&router, REGION, &format!("nextToken={token}")).await;
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
        let (status, body) = list(&router, REGION, &format!("maxResults={max_results}")).await;
        assert_eq!(
            status, expected,
            "case maxResults {max_results}: expected {expected}, got {status}: {body}"
        );
    }
}

#[tokio::test]
async fn a_snapshot_is_listed_only_in_its_own_region() {
    let router = emulator();

    // Two Regions holding a snapshot each, and one holding none.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];
    let mut owned: Vec<(&str, Option<(String, String)>)> = Vec::new();
    for region in regions {
        if region == "ap-southeast-2" {
            owned.push((region, None));
            continue;
        }
        let graph = graph_in_service(&router, region, "regional-source").await;
        let id = snapshot(&router, region, &graph, "regional-snapshot").await;
        owned.push((region, Some((graph, id))));
    }

    for (region, mine) in &owned {
        let (status, body) = list(&router, region, "").await;
        let listed = entries(region, status, &body);
        let expected: Vec<String> = mine.iter().map(|(_, id)| id.clone()).collect();
        let actual: Vec<String> = listed
            .iter()
            .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            actual, expected,
            "case the listing signed for {region}: expected {expected:?}, got {actual:?}"
        );

        // A graph of another Region does not exist here, so a listing narrowed to it is
        // a failed lookup rather than an empty page.
        for (other, theirs) in &owned {
            let Some((other_graph, _)) = theirs else {
                continue;
            };
            if other == region {
                continue;
            }
            let (status, body) =
                list(&router, region, &format!("graphIdentifier={other_graph}")).await;
            assert_eq!(
                status, 404,
                "case {other_graph} of {other} read from {region}: expected 404, got {status}: \
                 {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {other_graph} of {other} read from {region}: expected \
                 ResourceNotFoundException, got {code:?}: {body}"
            );
        }
    }
}
