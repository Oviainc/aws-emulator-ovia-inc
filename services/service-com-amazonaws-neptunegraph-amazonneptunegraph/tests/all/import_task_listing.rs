// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListImportTasks`, driven over the wire (`restJson1`) through
//! the same router the emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * The listing REPORTS the stored import tasks of the Region the request was signed
//!   for: every entry carries the same members an independent `GetImportTask` read of
//!   that task answers, and nothing a request fabricated.
//! * `graphIdentifier` narrows the listing to the tasks that load into that one graph;
//!   absent, the listing spans every import task of the Region. An identifier naming no
//!   graph of this Region is a failed lookup (404) rather than an empty page.
//! * The `@paginated` contract: a page carries at most `maxResults` entries, a
//!   `nextToken` is issued only while entries remain, and resuming from it returns the
//!   remainder without repeats or omissions, in the service's one stable order.
//! * A `nextToken` this service did not issue names no page and is refused (400).
//!
//! Each table below spans what its rule generalises over — the import format and its
//! parquet type, the source's form, the IAM role's path form, which operation accepted
//! the task, the page size against the population, how many tasks a graph holds, whether
//! the request names a graph, which Region signs the call, and the form of a rejected
//! identifier or token — rather than re-running one recorded request.

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

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneLoadFromS3";

/// The same pattern's `role/service-role` form, so the table spans both role paths.
const SERVICE_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/NeptuneLoad";

/// Every member `ImportTaskSummary` carries
/// (`smithy_query(query="shape",shape="ImportTaskSummary")`), so a listed entry must
/// equal the stored record member for member.
const SUMMARY_MEMBERS: &[&str] = &[
    "graphId",
    "taskId",
    "source",
    "format",
    "parquetType",
    "roleArn",
    "status",
];

/// The members of a listed entry that no clock ever moves, so they can be compared to an
/// independent read while the task is still in flight.
const STABLE_MEMBERS: &[&str] = &[
    "graphId",
    "taskId",
    "source",
    "format",
    "parquetType",
    "roleArn",
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

/// Whether `value` matches the modelled `TaskId` pattern `^t-[a-z0-9]{10}$`.
fn is_task_id(value: &str) -> bool {
    value.strip_prefix("t-").is_some_and(|suffix| {
        suffix.len() == 10
            && suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// Poll one resource until it reports `expected`, then return that read. No handler ever
/// sleeps; only this test does, because it is asking about a status the clock reaches on
/// its own.
async fn wait_for_status(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    path: &str,
    expected: &str,
) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, region, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read of {path}, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, region, "GET", path, None).await;
    panic!(
        "case {case:?}: {path} never reported {expected}, last status {}",
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
    wait_for_status(router, region, name, &format!("/graphs/{id}"), "AVAILABLE").await;
    id
}

/// One row of the start-and-list table: the import parameters a request carries.
struct StartCase {
    case: &'static str,
    role_arn: &'static str,
    source: &'static str,
    format: Option<&'static str>,
    parquet_type: Option<&'static str>,
    blank_node_handling: Option<&'static str>,
}

/// `POST /graphs/{graphIdentifier}/importtasks` for the graph `graph_id` of `region`,
/// returning the minted task id and the response that reported it.
///
/// An import takes its target graph through `IMPORTING`, and a graph accepts an import
/// only while it is `AVAILABLE`, so the graph is waited back to rest first: that is the
/// documented precondition, not a property of the listing.
async fn start_import_task(
    router: &MultiServiceRouter,
    region: &str,
    graph_id: &str,
    row: &StartCase,
) -> (String, Value) {
    wait_for_status(
        router,
        region,
        row.case,
        &format!("/graphs/{graph_id}"),
        "AVAILABLE",
    )
    .await;
    let mut request = json!({"source": row.source, "roleArn": row.role_arn});
    if let Some(format) = row.format {
        request["format"] = json!(format);
    }
    if let Some(parquet) = row.parquet_type {
        request["parquetType"] = json!(parquet);
    }
    if let Some(blank_nodes) = row.blank_node_handling {
        request["blankNodeHandling"] = json!(blank_nodes);
    }
    let (status, started) = call(
        router,
        region,
        "POST",
        &format!("/graphs/{graph_id}/importtasks"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: expected 201 from StartImportTask of {request}, got {status}: {started}",
        row.case
    );
    let task_id = started["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: no taskId in {started}", row.case))
        .to_string();
    assert!(
        is_task_id(&task_id),
        "case {:?}: expected a taskId matching ^t-[a-z0-9]{{10}}$, got {task_id:?}",
        row.case
    );
    (task_id, started)
}

/// Start one plain import of `graph_id`, for a table that is about the listing rather
/// than about the import parameters.
async fn plain_import_task(
    router: &MultiServiceRouter,
    region: &str,
    graph_id: &str,
    case: &'static str,
) -> String {
    let row = StartCase {
        case,
        role_arn: ROLE_ARN,
        source: "s3://example-bucket/plain/",
        format: None,
        parquet_type: None,
        blank_node_handling: None,
    };
    start_import_task(router, region, graph_id, &row).await.0
}

/// `POST /importtasks` — `CreateGraphUsingImportTask`, which mints a graph AND the
/// import task that loads it, returning both ids. A task this call accepted is a stored
/// import task like any other, so the listing has to report it too.
async fn create_graph_using_import_task(
    router: &MultiServiceRouter,
    region: &str,
    graph_name: &str,
) -> (String, String) {
    let request = json!({
        "graphName": graph_name,
        "source": "s3://example-bucket/created-with-the-graph/",
        "roleArn": ROLE_ARN,
    });
    let (status, created) = call(router, region, "POST", "/importtasks", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {graph_name}: expected 201 from CreateGraphUsingImportTask, got {status}: {created}"
    );
    let task_id = created["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {graph_name}: no taskId in {created}"))
        .to_string();
    let graph_id = created["graphId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {graph_name}: no graphId in {created}"))
        .to_string();
    (graph_id, task_id)
}

/// `GET /importtasks/{taskIdentifier}`.
async fn get_import(router: &MultiServiceRouter, region: &str, task_id: &str) -> (u16, Value) {
    call(
        router,
        region,
        "GET",
        &format!("/importtasks/{task_id}"),
        None,
    )
    .await
}

/// `GET /importtasks` with the query string `query` (already encoded, without the `?`).
async fn list(router: &MultiServiceRouter, region: &str, query: &str) -> (u16, Value) {
    let path = if query.is_empty() {
        "/importtasks".to_string()
    } else {
        format!("/importtasks?{query}")
    };
    call(router, region, "GET", &path, None).await
}

/// The `tasks` of a successful listing.
fn entries(case: &str, status: u16, body: &Value) -> Vec<Value> {
    assert_eq!(status, 200, "case {case}: expected 200, got {status}: {body}");
    body["tasks"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: tasks is required, got {body}"))
        .clone()
}

/// Walk every page of a listing, returning the task ids in the order they were reported.
/// Asserts the `@paginated` contract on each page: at most `page_size` entries, a token
/// only after a FULL page, and no id twice.
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
            let id = entry["taskId"].as_str().unwrap_or_default().to_string();
            assert!(
                is_task_id(&id),
                "case {page_case}: expected a taskId matching ^t-[a-z0-9]{{10}}$, got {id:?}"
            );
            assert!(
                !seen.contains(&id),
                "case {page_case}: taskId {id:?} was reported twice"
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
async fn list_import_tasks_reports_the_stored_tasks_of_the_region() {
    let router = emulator();
    let graph = graph_in_service(&router, REGION, "listing-target").await;

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

    // The rows differ along what the import parameters generalise over: the modelled
    // `Format` values (`smithy_query(query="shape",shape="Format")` -> CSV, NTRIPLES,
    // OPEN_CYPHER, PARQUET) including the absent one, the parquet type that goes with
    // PARQUET, the source's shape, and the role's path form.
    let cases = vec![
        StartCase {
            case: "no format, a plain role, a bucket prefix",
            role_arn: ROLE_ARN,
            source: "s3://example-bucket/no-format/",
            format: None,
            parquet_type: None,
            blank_node_handling: None,
        },
        StartCase {
            case: "CSV from a dotted bucket, a service role",
            role_arn: SERVICE_ROLE_ARN,
            source: "s3://example.bucket.with.dots/gremlin/",
            format: Some("CSV"),
            parquet_type: None,
            blank_node_handling: None,
        },
        StartCase {
            case: "OPEN_CYPHER from a bucket root",
            role_arn: ROLE_ARN,
            source: "s3://example-bucket",
            format: Some("OPEN_CYPHER"),
            parquet_type: None,
            blank_node_handling: None,
        },
        StartCase {
            case: "PARQUET, columnar",
            role_arn: ROLE_ARN,
            source: "s3://example-bucket/parquet/",
            format: Some("PARQUET"),
            parquet_type: Some("COLUMNAR"),
            blank_node_handling: None,
        },
        StartCase {
            case: "NTRIPLES, which the model pairs with blankNodeHandling",
            role_arn: SERVICE_ROLE_ARN,
            source: "s3://example-bucket/rdf/",
            format: Some("NTRIPLES"),
            parquet_type: None,
            blank_node_handling: Some("convertToIri"),
        },
    ];

    let mut created: BTreeSet<String> = BTreeSet::new();
    for row in &cases {
        let (task_id, started) = start_import_task(&router, REGION, &graph, row).await;
        created.insert(task_id.clone());

        // A task is listed from the instant it is accepted, in the status it is in: the
        // listing reports the store, not only its terminal rows.
        let (status, body) = list(&router, REGION, "").await;
        let listed = entries(row.case, status, &body);
        assert_eq!(
            listed.len(),
            created.len(),
            "case {:?}: expected {} entries, got {}",
            row.case,
            created.len(),
            listed.len()
        );
        let entry = listed
            .iter()
            .find(|entry| entry["taskId"] == json!(task_id))
            .unwrap_or_else(|| panic!("case {:?}: {task_id} is not listed", row.case));

        // Every member the summary carries is the value the accepting call reported, so
        // the listing cannot drift from the record that was stored.
        for member in STABLE_MEMBERS {
            assert_eq!(
                entry[member], started[member],
                "case {:?}: expected the listed {member} to be the stored value {}, got {}",
                row.case, started[member], entry[member]
            );
        }
        assert_eq!(
            entry["graphId"],
            json!(graph.clone()),
            "case {:?}: expected graphId {graph:?}, got {}",
            row.case,
            entry["graphId"]
        );

        // One stable total order underlies every page, which is what lets a token name
        // a position in it.
        let ids: Vec<String> = listed
            .iter()
            .map(|entry| entry["taskId"].as_str().unwrap_or_default().to_string())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(
            ids, sorted,
            "case {:?}: expected the listing in task-id order, got {ids:?}",
            row.case
        );
        assert_eq!(
            ids.iter().cloned().collect::<BTreeSet<String>>(),
            created,
            "case {:?}: the listing did not report every stored import task",
            row.case
        );
    }

    // A task accepted by `CreateGraphUsingImportTask` is a stored import task too, so it
    // joins the same listing, under the graph that call minted.
    let (own_graph, own_task) =
        create_graph_using_import_task(&router, REGION, "listing-created-with-its-task").await;
    created.insert(own_task.clone());
    let (status, body) = list(&router, REGION, "").await;
    let listed = entries("a task that minted its own graph", status, &body);
    let entry = listed
        .iter()
        .find(|entry| entry["taskId"] == json!(own_task))
        .unwrap_or_else(|| panic!("case a task that minted its own graph: {own_task} is not listed"));
    assert_eq!(
        entry["graphId"],
        json!(own_graph.clone()),
        "case a task that minted its own graph: expected graphId {own_graph:?}, got {}",
        entry["graphId"]
    );
    assert_eq!(
        listed
            .iter()
            .map(|entry| entry["taskId"].as_str().unwrap_or_default().to_string())
            .collect::<BTreeSet<String>>(),
        created,
        "case a task that minted its own graph: the listing did not report every stored task"
    );

    // Once the clock has settled every task, the whole summary — the status included —
    // is the stored record an independent read answers.
    for task_id in &created {
        wait_for_status(
            &router,
            REGION,
            task_id,
            &format!("/importtasks/{task_id}"),
            "FAILED",
        )
        .await;
    }
    let (status, body) = list(&router, REGION, "").await;
    let listed = entries("settled", status, &body);
    for entry in &listed {
        let entry_id = entry["taskId"].as_str().unwrap_or_default();
        let (read_status, read) = get_import(&router, REGION, entry_id).await;
        assert_eq!(
            read_status, 200,
            "case settled: expected 200 from GetImportTask for {entry_id:?}, got {read_status}: \
             {read}"
        );
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                entry[member], read[member],
                "case settled: the listed {member} of {entry_id:?} must be the stored value {}, \
                 got {}",
                read[member], entry[member]
            );
        }
    }

    // A listing changes nothing a later caller sees: repeating it answers the same page.
    let (again_status, again) = list(&router, REGION, "").await;
    let listed_again = entries("repeated", again_status, &again);
    assert_eq!(
        listed_again, listed,
        "case repeated: expected the same page, got {listed_again:?}"
    );
}

#[tokio::test]
async fn list_import_tasks_pages_over_every_task_without_repeats() {
    let router = emulator();

    // Each task minted with its own graph, so the population is built without waiting a
    // graph back out of IMPORTING between rows.
    const POPULATION: usize = 7;
    let mut all_ids: BTreeSet<String> = BTreeSet::new();
    for index in 0..POPULATION {
        let (_, task_id) =
            create_graph_using_import_task(&router, REGION, &format!("paging-target-{index}")).await;
        all_ids.insert(task_id);
    }

    // The `@paginated` contract over every interesting page size: 1, sizes that do not
    // divide the population, exactly the population, and the largest `maxResults`
    // allows (`range 1..100`).
    for page_size in [1_usize, 2, 3, POPULATION, 100] {
        let seen = page_through(&router, REGION, "every import task", "", page_size).await;
        assert_eq!(
            seen.len(),
            POPULATION,
            "case page size {page_size}: expected {POPULATION} tasks in total, got {}",
            seen.len()
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<String>>(),
            all_ids,
            "case page size {page_size}: the pages did not cover every stored import task"
        );
    }

    // A token that names a position in the task-id space resumes from that position,
    // even though no task was ever minted with that id: a page resumes after a
    // POSITION, so a row that leaves the store between two pages cannot hide the rows
    // that follow it.
    let position = "t-aaaaaaaaaa";
    let unissued = hex::encode(position);
    let (status, body) = list(&router, REGION, &format!("nextToken={unissued}")).await;
    let listed = entries("an unissued position", status, &body);
    for entry in &listed {
        let id = entry["taskId"].as_str().unwrap_or_default();
        assert!(
            id > position,
            "case an unissued position: {id:?} does not sort after {position:?}"
        );
    }
}

#[tokio::test]
async fn graph_identifier_narrows_the_listing_to_one_graphs_import_tasks() {
    let router = emulator();

    // (the graph's name, how many import tasks it holds) — a graph with several, a graph
    // with one, and a graph with none, so the filter is tested against each count.
    let population: Vec<(&str, usize)> =
        vec![("busy-target", 3), ("quiet-target", 1), ("bare-target", 0)];
    let mut graphs: Vec<(String, BTreeSet<String>)> = Vec::new();
    for (name, count) in &population {
        let graph = graph_in_service(&router, REGION, name).await;
        let mut ids = BTreeSet::new();
        for _ in 0..*count {
            ids.insert(plain_import_task(&router, REGION, &graph, "filtering").await);
        }
        graphs.push((graph, ids));
    }
    let every_id: BTreeSet<String> = graphs
        .iter()
        .flat_map(|(_, ids)| ids.iter().cloned())
        .collect();

    // Filtered to each graph, the listing reports exactly that graph's tasks; with no
    // filter it reports every task of the Region. Each row is paged at a size below its
    // own population too, so the filter survives a resumed page.
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
            "case no graphIdentifier, page size {page_size}: expected every import task of the \
             Region {every_id:?}, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn list_import_tasks_refuses_an_identifier_or_token_it_cannot_resolve() {
    let router = emulator();
    let graph = graph_in_service(&router, REGION, "refusal-target").await;
    plain_import_task(&router, REGION, &graph, "refusal").await;

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
        (
            "a task id where a graph id belongs",
            "t-abcdefghij",
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
    // value that is no task id, and hex of a position in the GRAPH and SNAPSHOT id
    // spaces.
    for token in [
        "zzz",
        "not-a-token",
        "deadbeefdeadbeef",
        &hex::encode("not-a-task-id"),
        &hex::encode("t-short"),
        &hex::encode("g-0123456789"),
        &hex::encode("gs-0123456789"),
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
async fn an_import_task_is_listed_only_in_its_own_region() {
    let router = emulator();

    // Two Regions holding an import task each, and one holding none.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];
    let mut owned: Vec<(&str, Option<(String, String)>)> = Vec::new();
    for region in regions {
        if region == "ap-southeast-2" {
            owned.push((region, None));
            continue;
        }
        let (graph, task_id) =
            create_graph_using_import_task(&router, region, "regional-target").await;
        owned.push((region, Some((graph, task_id))));
    }

    for (region, mine) in &owned {
        let (status, body) = list(&router, region, "").await;
        let listed = entries(region, status, &body);
        let expected: Vec<String> = mine.iter().map(|(_, id)| id.clone()).collect();
        let actual: Vec<String> = listed
            .iter()
            .map(|entry| entry["taskId"].as_str().unwrap_or_default().to_string())
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
