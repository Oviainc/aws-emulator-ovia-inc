// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE paging rule every listing of this service serves pages
//! by, driven over the wire (`restJson1`) through the same router the emulator serves
//! from.
//!
//! The rule every assertion here is about, in two parts:
//!
//! * A page carries at most the `maxResults` the request asked for, and — when the
//!   request asks for none — at most the largest `maxResults` the member allows, so an
//!   unpaginated caller sees as much as one page can carry.
//! * A `nextToken` is issued ONLY while an entry remains after the page, so paging a
//!   listing of N entries at a page size of K takes exactly ceil(N / K) pages: the last
//!   page carries no token and no empty page is ever served while entries remain.
//!
//! The table spans what the rule generalises over: every paginated listing the service
//! implements — `ListGraphs`, `ListGraphSnapshots`, `ListImportTasks`, `ListExportTasks`
//! and `ListPrivateGraphEndpoints`, whose entries are keyed by four different id spaces
//! — each against page sizes of one, sizes that do not divide its population, exactly
//! its population and the largest the member allows, plus a listing with no entries at
//! all and the page sizes the modelled range refuses.

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

/// How many entries each listing is populated with beyond what its own fixtures already
/// put there: enough that a page size below it forces several tokens, and a page size
/// equal to it forces none.
const POPULATION: usize = 3;

/// The largest `maxResults` every one of these listings allows: `MaxResults` is
/// `range 1..100` (`smithy_query(query="operation",operation="ListGraphs,ListGraphSnapshots,ListImportTasks,ListExportTasks,ListPrivateGraphEndpoints")`),
/// which is also the page size a request carrying no `maxResults` is served.
const MAX_PAGE_SIZE: usize = 100;

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneExportRole";

/// An emulator holding only this service, on its own engine, so each test starts from
/// an empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request signed for `REGION` and return its status and
/// parsed JSON body.
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

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this
/// test does, because it is asking about a state the clock reaches on its own.
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

/// Snapshot `graph_id` under `name`, waiting the source graph back into service so the
/// next call is about the stored row and not about a transition this one left pending.
async fn snapshot(router: &MultiServiceRouter, graph_id: &str, name: &str) {
    let (status, created) = call(
        router,
        "POST",
        "/snapshots",
        Some(json!({"graphIdentifier": graph_id, "snapshotName": name})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case snapshot {name:?} of {graph_id}: expected 201, got {status}: {created}"
    );
    wait_until_available(router, &format!("/graphs/{graph_id}")).await;
}

/// Create one private endpoint of `graph_id` in `vpc_id` and wait it into service.
async fn endpoint_in_service(router: &MultiServiceRouter, graph_id: &str, vpc_id: &str) {
    // `SubnetId`'s modelled pattern is `^subnet-[a-z0-9]+$` and `SubnetIds` is
    // `@length(1..6)` (`smithy_query(query="shape",shape="SubnetIds")`), so one subnet
    // is a well-formed request.
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

/// Start one plain CSV export of `graph_id`, which this suite needs only as a row of
/// the export-task listing.
async fn export_task(router: &MultiServiceRouter, graph_id: &str, destination: &str) {
    let body = json!({
        "graphIdentifier": graph_id,
        "roleArn": ROLE_ARN,
        "format": "CSV",
        "destination": destination,
        "kmsKeyIdentifier": KMS_KEY_ARN,
    });
    let (status, started) = call(router, "POST", "/exporttasks", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from StartExportTask, got {status}: {started}"
    );
}

/// `CreateGraphUsingImportTask`, which mints a graph AND the import task that loads it:
/// the cheapest way to put several rows in the import-task listing, since each task gets
/// its own target graph.
async fn graph_with_import_task(router: &MultiServiceRouter, graph_name: &str) {
    let body = json!({
        "graphName": graph_name,
        "source": "s3://paging-bucket/import/",
        "roleArn": ROLE_ARN,
    });
    let (status, created) = call(router, "POST", "/importtasks", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from CreateGraphUsingImportTask, got {status}: {created}"
    );
}

/// One listing of this service, in the terms the shared paging rule is stated in: where
/// it is served, which member carries its entries, which member of an entry orders it,
/// and how many entries the store holds for it.
struct Listing {
    operation: &'static str,
    path: String,
    entries_member: &'static str,
    key_member: &'static str,
    population: usize,
}

/// `GET` the listing with the query string `query` (already encoded, without the `?`).
async fn list(router: &MultiServiceRouter, listing: &Listing, query: &str) -> (u16, Value) {
    let path = if query.is_empty() {
        listing.path.clone()
    } else {
        format!("{}?{query}", listing.path)
    };
    call(router, "GET", &path, None).await
}

/// The key of every entry of a successful listing, in the order they were reported.
fn keys(case: &str, listing: &Listing, status: u16, body: &Value) -> Vec<String> {
    assert_eq!(status, 200, "case {case}: expected 200, got {status}: {body}");
    body[listing.entries_member]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: {} is required, got {body}",
                listing.entries_member
            )
        })
        .iter()
        .map(|entry| {
            entry[listing.key_member]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: entry carries no {}, got {entry}",
                        listing.key_member
                    )
                })
                .to_string()
        })
        .collect()
}

/// Page `listing` from its start at `page_size`, asserting the whole rule on the way:
/// every page carries at most `page_size` entries, a token is issued only while entries
/// remain (so a page a token was issued for is FULL), and the walk takes exactly
/// `ceil(population / page_size)` pages. Returns the keys in the order they were
/// reported.
async fn page_through(
    router: &MultiServiceRouter,
    listing: &Listing,
    page_size: usize,
) -> Vec<String> {
    let operation = listing.operation;
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0_usize;
    // One more turn than the rule allows, so a listing that keeps issuing tokens is
    // caught by the assertion below rather than by looping forever.
    let turns = listing.population / page_size + 2;
    for page in 0..turns {
        let mut query = format!("maxResults={page_size}");
        if let Some(value) = &token {
            query = format!("{query}&nextToken={value}");
        }
        let (status, body) = list(router, listing, &query).await;
        let case = format!("{operation}, page size {page_size}, page {page}");
        let page_keys = keys(&case, listing, status, &body);
        pages += 1;
        assert!(
            page_keys.len() <= page_size,
            "case {case}: expected at most {page_size} entries, got {} ({page_keys:?})",
            page_keys.len()
        );
        seen.extend(page_keys.iter().cloned());
        token = body["nextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            break;
        }
        // A token says entries remain, which can only be true of a page that was filled
        // to the page size: a short page exhausted the listing.
        assert_eq!(
            page_keys.len(),
            page_size,
            "case {case}: a token was issued after a page of {} entries ({page_keys:?})",
            page_keys.len()
        );
    }
    assert!(
        token.is_none(),
        "case {operation}, page size {page_size}: paging never finished, token {token:?}"
    );
    let expected_pages = listing.population.div_ceil(page_size).max(1);
    assert_eq!(
        pages, expected_pages,
        "case {operation}, page size {page_size}: expected {expected_pages} pages for \
         {} entries, got {pages}",
        listing.population
    );
    seen
}

#[tokio::test]
async fn every_listing_serves_the_one_page_size_and_next_token_rule() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "paging-rule-graph").await;
    for i in 0..POPULATION {
        snapshot(&router, &graph_id, &format!("paging-rule-snapshot-{i}")).await;
    }
    for i in 0..POPULATION {
        endpoint_in_service(&router, &graph_id, &format!("vpc-0a1b2c3d4e5f{i:05}")).await;
    }
    for i in 0..POPULATION {
        export_task(&router, &graph_id, &format!("s3://paging-rule-bucket/{i}")).await;
    }
    for i in 0..POPULATION {
        graph_with_import_task(&router, &format!("paging-rule-import-{i}")).await;
    }
    // A graph of its own, holding no endpoint: the empty listing below is about it.
    let bare_graph = graph_in_service(&router, "paging-rule-bare-graph").await;

    let listings = [
        Listing {
            operation: "ListGraphs",
            path: "/graphs".to_string(),
            entries_member: "graphs",
            key_member: "id",
            // The graph above, the one graph per import task, and the bare graph.
            population: POPULATION + 2,
        },
        Listing {
            operation: "ListGraphSnapshots",
            path: "/snapshots".to_string(),
            entries_member: "graphSnapshots",
            key_member: "id",
            population: POPULATION,
        },
        Listing {
            operation: "ListImportTasks",
            path: "/importtasks".to_string(),
            entries_member: "tasks",
            key_member: "taskId",
            population: POPULATION,
        },
        Listing {
            operation: "ListExportTasks",
            path: "/exporttasks".to_string(),
            entries_member: "tasks",
            key_member: "taskId",
            population: POPULATION,
        },
        Listing {
            operation: "ListPrivateGraphEndpoints",
            path: format!("/graphs/{graph_id}/endpoints/"),
            entries_member: "privateGraphEndpoints",
            key_member: "vpcId",
            population: POPULATION,
        },
        Listing {
            operation: "ListPrivateGraphEndpoints of a graph holding none",
            path: format!("/graphs/{bare_graph}/endpoints/"),
            entries_member: "privateGraphEndpoints",
            key_member: "vpcId",
            population: 0,
        },
    ];

    for listing in &listings {
        let operation = listing.operation;

        // A request carrying no `maxResults` is served the largest page the member
        // allows, so every entry of a population below that bound comes back in ONE
        // page, with no token to resume from.
        let (status, body) = list(&router, listing, "").await;
        let all = keys(operation, listing, status, &body);
        assert_eq!(
            all.len(),
            listing.population,
            "case {operation}: expected {} entries in one unpaginated page, got {} ({all:?})",
            listing.population,
            all.len()
        );
        assert!(
            listing.population <= MAX_PAGE_SIZE,
            "case {operation}: this suite's population {} is above the default page size \
             {MAX_PAGE_SIZE}, so the assertion above is no longer about the default",
            listing.population
        );
        assert_eq!(
            body["nextToken"],
            Value::Null,
            "case {operation}: expected no token when one page holds every entry, got {body}"
        );
        let mut ordered = all.clone();
        ordered.sort();
        assert_eq!(
            all, ordered,
            "case {operation}: entries are not in one stable ascending order, got {all:?}"
        );

        // The same entries, in the same order, at every interesting page size: one, a
        // size that does not divide the population, exactly the population, and the
        // largest `maxResults` allows.
        for page_size in [
            1,
            2,
            POPULATION,
            listing.population.max(1),
            MAX_PAGE_SIZE - 1,
            MAX_PAGE_SIZE,
        ] {
            let seen = page_through(&router, listing, page_size).await;
            assert_eq!(
                seen, all,
                "case {operation}, page size {page_size}: paging did not report every entry \
                 exactly once in order, got {seen:?}"
            );
        }

        // A `maxResults` outside the modelled range names no page size: it is refused at
        // the boundary, so no page a client can ask for is served a size below one or
        // above the bound.
        for refused in [0, MAX_PAGE_SIZE + 1] {
            let (status, body) = list(&router, listing, &format!("maxResults={refused}")).await;
            assert_eq!(
                status, 400,
                "case {operation}, maxResults={refused}: expected 400, got {status}: {body}"
            );
        }
    }
}
