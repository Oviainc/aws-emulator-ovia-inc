// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the records that OUTLIVE the graph they came from, driven over
//! the wire (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: a snapshot, an import task and an export task
//! are resources in their own right that keep the id of the graph they came from, so a
//! per-graph listing resolves its `graphIdentifier` against everything the Region still
//! holds under that id — the graph while it exists, and afterwards the records whose
//! provenance names it — and reports those records; only an identifier the Region holds
//! NOTHING under is the failed lookup (404) the listing answers instead of a page.
//!
//! The table spans what that rule generalises over: which kind of record is retained
//! (a named snapshot, the final snapshot a delete mints, an import task, an export
//! task, several of each at once), how many of them there are, and whether the delete
//! was asked to skip the final snapshot — so a graph that leaves nothing behind is a row
//! of the same table rather than a separate rule. Each row asserts the per-graph listing
//! BEFORE the graph is deleted and again once the clock has reclaimed it, against the
//! same records read back by their own ids.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::PROVISIONING_SPAN;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region these tests sign for.
const REGION: &str = "us-east-1";

/// A second Region, so a row can ask what the Region that never held the graph (nor any
/// record of it) answers for the same identifier.
const OTHER_REGION: &str = "eu-west-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneLoadFromS3";

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`, which
/// `StartExportTask` requires.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

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

/// Poll one resource until it reports `expected`. No handler ever sleeps; only this test
/// does, because it is asking about a status the clock reaches on its own.
async fn wait_for_status(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    path: &str,
    expected: &str,
) {
    for _ in 0..40 {
        let (status, body) = call(router, region, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read of {path}, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, region, "GET", path, None).await;
    panic!(
        "case {case:?}: {path} never reported {expected}, last status {}",
        body["status"]
    );
}

/// Poll a graph until the deletion the clock owes it has fallen due and the record is
/// gone: the instant from which the records it left behind are all that is left of it.
async fn wait_until_reclaimed(router: &MultiServiceRouter, region: &str, case: &str, id: &str) {
    let path = format!("/graphs/{id}");
    for _ in 0..40 {
        let (status, body) = call(router, region, "GET", &path, None).await;
        if status == 404 {
            let code = error_code(&body);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException once {id} is gone, \
                 got {code:?}: {body}"
            );
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (status, body) = call(router, region, "GET", &path, None).await;
    panic!("case {case:?}: {id} was never reclaimed, last read {status}: {body}");
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
    wait_for_status(router, region, name, &format!("/graphs/{id}"), "AVAILABLE").await;
    id
}

/// Snapshot `graph_id` under `name`, returning the snapshot's id. The source graph is
/// waited back to rest, because a snapshot takes it through a transient status and the
/// next call on it is about this table's row, not about that transition.
async fn snapshot(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
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
        "case {case:?}: expected 201 from CreateGraphSnapshot {name:?}, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no id in {created}"))
        .to_string();
    wait_for_status(
        router,
        region,
        case,
        &format!("/graphs/{graph_id}"),
        "AVAILABLE",
    )
    .await;
    id
}

/// Start an import into `graph_id`, returning the task id. An import takes the graph
/// through `IMPORTING`, so the graph is waited back to rest afterwards: the row's next
/// call needs a graph at rest.
async fn import_task(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    graph_id: &str,
    source: &str,
) -> String {
    let request = json!({"source": source, "roleArn": ROLE_ARN});
    let (status, started) = call(
        router,
        region,
        "POST",
        &format!("/graphs/{graph_id}/importtasks"),
        Some(request),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from StartImportTask of {source}, got {status}: {started}"
    );
    let task_id = started["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no taskId in {started}"))
        .to_string();
    wait_for_status(
        router,
        region,
        case,
        &format!("/graphs/{graph_id}"),
        "AVAILABLE",
    )
    .await;
    task_id
}

/// Start an export of `graph_id`, returning the task id.
async fn export_task(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    graph_id: &str,
    destination: &str,
) -> String {
    let request = json!({
        "graphIdentifier": graph_id,
        "roleArn": ROLE_ARN,
        "format": "CSV",
        "destination": destination,
        "kmsKeyIdentifier": KMS_KEY_ARN,
    });
    let (status, started) = call(router, region, "POST", "/exporttasks", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from StartExportTask to {destination}, got \
         {status}: {started}"
    );
    started["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no taskId in {started}"))
        .to_string()
}

/// The three listings this rule covers: the collection each pages over, the member an
/// entry is identified by, and the member that carries the graph the record came from.
#[derive(Clone, Copy)]
struct Listing {
    /// The listing's path, which is also what names it in an assertion.
    path: &'static str,
    /// The required output member carrying the page.
    page_member: &'static str,
    /// The member of an entry that carries its own id.
    id_member: &'static str,
    /// The member of an entry that carries the id of the graph it came from.
    provenance_member: &'static str,
}

/// `ListGraphSnapshots`, `ListImportTasks` and `ListExportTasks`, whose output members
/// come from
/// `smithy_query(query="operation",operation="ListGraphSnapshots,ListImportTasks,ListExportTasks")`.
const LISTINGS: [Listing; 3] = [
    Listing {
        path: "/snapshots",
        page_member: "graphSnapshots",
        id_member: "id",
        provenance_member: "sourceGraphId",
    },
    Listing {
        path: "/importtasks",
        page_member: "tasks",
        id_member: "taskId",
        provenance_member: "graphId",
    },
    Listing {
        path: "/exporttasks",
        page_member: "tasks",
        id_member: "taskId",
        provenance_member: "graphId",
    },
];

/// `GET <listing>` with the query string `query` (already encoded, without the `?`).
async fn list(
    router: &MultiServiceRouter,
    region: &str,
    listing: Listing,
    query: &str,
) -> (u16, Value) {
    let path = if query.is_empty() {
        listing.path.to_string()
    } else {
        format!("{}?{query}", listing.path)
    };
    call(router, region, "GET", &path, None).await
}

/// The ids a successful page of `listing` reported, asserting every entry's provenance
/// names `of_graph` when the request narrowed the listing to one graph.
fn reported_ids(case: &str, listing: Listing, status: u16, body: &Value) -> BTreeSet<String> {
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {}, got {status}: {body}",
        listing.path
    );
    let entries = body[listing.page_member].as_array().unwrap_or_else(|| {
        panic!(
            "case {case:?}: {} is required in {body}",
            listing.page_member
        )
    });
    entries
        .iter()
        .map(|entry| {
            entry[listing.id_member]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: no {} in entry {entry}",
                        listing.id_member
                    )
                })
                .to_string()
        })
        .collect()
}

/// Every id `listing` reports for the graph `graph_id`, paging with `page_size` so the
/// `@paginated` contract is exercised against a graph whose record is gone as well as one
/// that is still there.
async fn ids_of_graph(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    listing: Listing,
    graph_id: &str,
    page_size: usize,
) -> Result<BTreeSet<String>, (u16, Value)> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut token: Option<String> = None;
    for _ in 0..64 {
        let mut query = format!("graphIdentifier={graph_id}&maxResults={page_size}");
        if let Some(value) = &token {
            query = format!("{query}&nextToken={value}");
        }
        let (status, body) = list(router, region, listing, &query).await;
        if status != 200 {
            return Err((status, body));
        }
        for id in reported_ids(case, listing, status, &body) {
            assert!(
                seen.insert(id.clone()),
                "case {case:?}: {} reported {id:?} twice",
                listing.path
            );
        }
        for entry in body[listing.page_member].as_array().unwrap_or(&Vec::new()) {
            let provenance = entry[listing.provenance_member].as_str().unwrap_or_default();
            assert_eq!(
                provenance, graph_id,
                "case {case:?}: {} reported an entry of another graph, expected \
                 {} {graph_id:?}, got {provenance:?}: {entry}",
                listing.path, listing.provenance_member
            );
        }
        token = body["nextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            return Ok(seen);
        }
    }
    panic!("case {case:?}: paging {} never finished", listing.path);
}

/// One row of the table: what the graph is stocked with before it is deleted, and how it
/// is deleted.
struct Row {
    /// What this row is about, named in every assertion of it.
    case: &'static str,
    /// The graph's name, unique per Region.
    graph_name: &'static str,
    /// The `snapshotName` of each snapshot taken of the graph before the delete.
    named_snapshots: &'static [&'static str],
    /// The `source` of each import started into the graph.
    import_sources: &'static [&'static str],
    /// The `destination` of each export started off the graph.
    export_destinations: &'static [&'static str],
    /// `DeleteGraph`'s required `skipSnapshot`: `false` mints one more snapshot of the
    /// graph, which is a record of it like any other.
    skip_snapshot: bool,
}

/// Every retained record of one row, by listing: the ids each per-graph listing has to
/// keep reporting under the graph once that graph is gone.
struct Retained {
    snapshots: BTreeSet<String>,
    imports: BTreeSet<String>,
    exports: BTreeSet<String>,
}

impl Retained {
    /// The ids `listing` is expected to report.
    fn of(&self, listing: Listing) -> &BTreeSet<String> {
        match listing.path {
            "/snapshots" => &self.snapshots,
            "/importtasks" => &self.imports,
            _ => &self.exports,
        }
    }

    /// Whether the Region holds any record of the graph at all: with none, the graph's
    /// id names nothing once the graph itself is reclaimed.
    fn is_empty(&self) -> bool {
        self.snapshots.is_empty() && self.imports.is_empty() && self.exports.is_empty()
    }
}

/// The ids an unfiltered listing reports whose provenance names `graph_id`: how the test
/// finds the final snapshot a delete minted without going through the listing under test.
async fn ids_from_the_unfiltered_listing(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    listing: Listing,
    graph_id: &str,
) -> BTreeSet<String> {
    let (status, body) = list(router, region, listing, "maxResults=100").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the unfiltered {}, got {status}: {body}",
        listing.path
    );
    body[listing.page_member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {} is required in {body}", listing.page_member))
        .iter()
        .filter(|entry| entry[listing.provenance_member].as_str() == Some(graph_id))
        .map(|entry| {
            entry[listing.id_member]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_graphs_records_stay_listed_under_it_once_the_graph_is_gone() {
    let rows: &[Row] = &[
        Row {
            case: "one named snapshot, the final one skipped",
            graph_name: "snapshot-source",
            named_snapshots: &["kept-snapshot"],
            import_sources: &[],
            export_destinations: &[],
            skip_snapshot: true,
        },
        Row {
            case: "only the final snapshot the delete mints",
            graph_name: "final-snapshot-source",
            named_snapshots: &[],
            import_sources: &[],
            export_destinations: &[],
            skip_snapshot: false,
        },
        Row {
            case: "one import task",
            graph_name: "import-target",
            named_snapshots: &[],
            import_sources: &["s3://example-bucket/one/"],
            export_destinations: &[],
            skip_snapshot: true,
        },
        Row {
            case: "one export task",
            graph_name: "export-source",
            named_snapshots: &[],
            import_sources: &[],
            export_destinations: &["s3://export-bucket/one"],
            skip_snapshot: true,
        },
        Row {
            case: "snapshots, imports and exports at once, plus the final snapshot",
            graph_name: "busy-graph",
            named_snapshots: &["first-of-many", "second-of-many"],
            import_sources: &["s3://example-bucket/first/", "s3://example-bucket/second/"],
            export_destinations: &["s3://export-bucket/first", "s3://export-bucket/second"],
            skip_snapshot: false,
        },
        Row {
            case: "nothing retained: no record of the graph is left",
            graph_name: "leaves-nothing",
            named_snapshots: &[],
            import_sources: &[],
            export_destinations: &[],
            skip_snapshot: true,
        },
    ];

    for row in rows {
        // Each row gets its own emulator, so the unfiltered listings hold this row's
        // records and nothing another row left behind.
        let router = emulator();
        let graph_id = graph_in_service(&router, REGION, row.graph_name).await;

        let mut retained = Retained {
            snapshots: BTreeSet::new(),
            imports: BTreeSet::new(),
            exports: BTreeSet::new(),
        };
        for name in row.named_snapshots {
            retained
                .snapshots
                .insert(snapshot(&router, REGION, row.case, &graph_id, name).await);
        }
        for source in row.import_sources {
            retained
                .imports
                .insert(import_task(&router, REGION, row.case, &graph_id, source).await);
        }
        for destination in row.export_destinations {
            retained
                .exports
                .insert(export_task(&router, REGION, row.case, &graph_id, destination).await);
        }

        // While the graph exists, each listing reports exactly the records taken from it
        // — including none, which is an empty page and not a failure.
        for listing in LISTINGS {
            let case = format!("{} ({}, before the delete)", row.case, listing.path);
            let listed = ids_of_graph(&router, REGION, &case, listing, &graph_id, 100)
                .await
                .unwrap_or_else(|(status, body)| {
                    panic!("case {case:?}: expected 200 while the graph exists, got {status}: {body}")
                });
            assert_eq!(
                &listed,
                retained.of(listing),
                "case {case:?}: expected {:?}, got {listed:?}",
                retained.of(listing)
            );
        }

        let (status, deleted) = call(
            &router,
            REGION,
            "DELETE",
            &format!("/graphs/{graph_id}?skipSnapshot={}", row.skip_snapshot),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 from DeleteGraph, got {status}: {deleted}",
            row.case
        );
        assert_eq!(
            deleted["status"],
            json!("DELETING"),
            "case {:?}: expected DELETING from DeleteGraph, got {deleted}",
            row.case
        );
        if !row.skip_snapshot {
            // The final snapshot is found through the UNFILTERED listing, so the test
            // learns its id from somewhere other than the listing under test.
            let minted = ids_from_the_unfiltered_listing(
                &router,
                REGION,
                row.case,
                LISTINGS[0],
                &graph_id,
            )
            .await;
            assert_eq!(
                minted.len(),
                retained.snapshots.len() + 1,
                "case {:?}: expected one more snapshot of {graph_id} than the {} the row \
                 took, got {minted:?}",
                row.case,
                retained.snapshots.len()
            );
            retained.snapshots = minted;
        }
        wait_until_reclaimed(&router, REGION, row.case, &graph_id).await;

        for listing in LISTINGS {
            let case = format!("{} ({}, once the graph is gone)", row.case, listing.path);
            let listed = ids_of_graph(&router, REGION, &case, listing, &graph_id, 1).await;
            if retained.is_empty() {
                let (status, body) = listed.expect_err(&format!(
                    "case {case:?}: expected a refusal for an identifier the Region holds \
                     nothing under"
                ));
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected 404, got {status}: {body}"
                );
                let code = error_code(&body);
                assert_eq!(
                    code, "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {code:?}: {body}"
                );
                continue;
            }
            let listed = listed.unwrap_or_else(|(status, body)| {
                panic!(
                    "case {case:?}: expected 200 for a graph whose records the Region still \
                     holds, got {status}: {body}"
                )
            });
            assert_eq!(
                &listed,
                retained.of(listing),
                "case {case:?}: expected {:?}, got {listed:?}",
                retained.of(listing)
            );
        }

        // Every retained record is still readable by its own id, and still names the
        // graph it came from: the listing reports records, it does not invent them.
        for (id, path, provenance_member) in retained
            .snapshots
            .iter()
            .map(|id| (id, "snapshots", "sourceGraphId"))
            .chain(
                retained
                    .imports
                    .iter()
                    .map(|id| (id, "importtasks", "graphId")),
            )
            .chain(
                retained
                    .exports
                    .iter()
                    .map(|id| (id, "exporttasks", "graphId")),
            )
        {
            let (status, body) =
                call(&router, REGION, "GET", &format!("/{path}/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {:?}: expected 200 reading /{path}/{id} after the graph is gone, \
                 got {status}: {body}",
                row.case
            );
            assert_eq!(
                body[provenance_member],
                json!(graph_id),
                "case {:?}: expected /{path}/{id} to still name {graph_id}, got {body}",
                row.case
            );
        }

        // The records live in ONE Region: the Region that never held the graph holds no
        // record of it either, so the same identifier names nothing there.
        for listing in LISTINGS {
            let case = format!("{} ({}, from another Region)", row.case, listing.path);
            let (status, body) = list(
                &router,
                OTHER_REGION,
                listing,
                &format!("graphIdentifier={graph_id}"),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 in a Region holding nothing under \
                 {graph_id}, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {code:?}: {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_graph_id_the_region_never_held_is_refused_by_every_per_graph_listing() {
    let router = emulator();
    // A graph with a record of its own, so the refusals below are about the identifier
    // the request carries rather than about an empty store.
    let graph_id = graph_in_service(&router, REGION, "unrelated-graph").await;
    snapshot(
        &router,
        REGION,
        "unrelated-graph",
        &graph_id,
        "unrelated-snapshot",
    )
    .await;

    // Well-formed `GraphIdentifier`s (pattern `^g-[a-z0-9]{10}$`) that name no graph and
    // no record of one: the ends of the id space and a form in between.
    for identifier in ["g-0000000000", "g-zzzzzzzzzz", "g-abc123def4"] {
        for listing in LISTINGS {
            let case = format!("{identifier} on {}", listing.path);
            let (status, body) = list(
                &router,
                REGION,
                listing,
                &format!("graphIdentifier={identifier}"),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 for an identifier the Region holds nothing \
                 under, got {status}: {body}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {code:?}: {body}"
            );
        }
    }
}
