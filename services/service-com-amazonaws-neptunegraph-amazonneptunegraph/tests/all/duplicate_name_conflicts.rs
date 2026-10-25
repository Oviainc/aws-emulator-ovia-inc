// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a create whose requested NAME is already held in the
//! account's Region, driven over the wire (`restJson1`) through the same router the
//! emulator serves from.
//!
//! One rule, asserted over a set of inputs rather than one recorded request: a create
//! refused because the requested name is already taken reports the conflict from THAT
//! name alone — `<Kind> with name <name> already exists!` — and discloses nothing about
//! the record that holds the name, which is a resource the request never addressed.
//!
//! The rows span what the rule generalises over:
//!
//! * the namespace the name belongs to (graphs and snapshots, the two named resource
//!   kinds this service's creates key uniqueness on);
//! * the operation that mints into that namespace (`CreateGraph`,
//!   `CreateGraphUsingImportTask` and `RestoreGraphFromSnapshot` for a graph name,
//!   `CreateGraphSnapshot` for a snapshot name), both as the call that TOOK the name and
//!   as the call that collides with it;
//! * the name itself, at the published ends of the modelled `@length(1..63)` and across
//!   the forms its pattern admits (one letter, digits, several hyphen groups).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::PROVISIONING_SPAN;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`, for the import
/// form of a graph create.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneImportRole";

/// The largest `GraphName` / `SnapshotName` the model allows: both are
/// `@length(1..63)` (`smithy_query(query="operation",operation="CreateGraph")`,
/// `smithy_query(query="operation",operation="CreateGraphSnapshot")`).
const MAX_NAME_CHARS: usize = 63;

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

/// The `message` member of a failure body.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The names the rows below collide on: the published ends of the modelled
/// `@length(1..63)` and the forms the shared pattern
/// `^(?!g-)[a-z][a-z0-9]*(-[a-z0-9]+)*$` admits between them.
fn colliding_names() -> Vec<String> {
    vec![
        // The length minimum: a single letter.
        "a".to_string(),
        // Digits after the leading letter.
        "taken7".to_string(),
        // Several hyphen groups, one of them all digits.
        "second-copy-of-23".to_string(),
        // The length maximum, exactly 63 characters.
        format!("n{}", "x".repeat(MAX_NAME_CHARS - 1)),
    ]
}

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this
/// test does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, case: &str, path: &str) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from {path}, got {status}: {body}"
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

/// `POST /graphs` — take a graph name through `CreateGraph`, returning the minted id.
async fn create_graph(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/graphs",
        Some(json!({"graphName": name, "provisionedMemory": 16})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph, got {status}: {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no graph id in {created}"))
        .to_string()
}

/// `POST /importtasks` — take a graph name through `CreateGraphUsingImportTask`,
/// returning the minted graph id.
async fn create_graph_using_import(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/importtasks",
        Some(json!({
            "graphName": name,
            "roleArn": ROLE_ARN,
            "source": "s3://import-bucket/seed",
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraphUsingImportTask, got {status}: {created}"
    );
    created["graphId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no graph id in {created}"))
        .to_string()
}

/// `POST /snapshots` — take a snapshot name off `graph_id`, returning the minted
/// snapshot id. The source graph goes through `SNAPSHOTTING`, so it is waited back into
/// service before the caller's next call.
async fn create_snapshot(
    router: &MultiServiceRouter,
    case: &str,
    graph_id: &str,
    name: &str,
) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/snapshots",
        Some(json!({"graphIdentifier": graph_id, "snapshotName": name})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraphSnapshot, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no snapshot id in {created}"))
        .to_string();
    wait_until_available(router, case, &format!("/graphs/{graph_id}")).await;
    wait_until_available(router, case, &format!("/snapshots/{id}")).await;
    id
}

/// Assert one refusal: the status, the modelled error code, the whole message the rule
/// calls for, and that nothing of the colliding record leaked into it.
fn assert_name_conflict(
    case: &str,
    status: u16,
    body: &Value,
    kind: &str,
    name: &str,
    holder_id: &str,
) {
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
    let expected = format!("{kind} with name {name} already exists!");
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
    assert!(
        !actual.contains(holder_id),
        "case {case:?}: the refusal must not name the record holding the name \
         ({holder_id}), got {actual:?}"
    );
}

/// Every operation that mints a GRAPH refuses a `graphName` the account already holds
/// in the Region with the same request-derived wording, whichever operation took the
/// name first.
#[tokio::test]
async fn a_taken_graph_name_is_refused_from_the_requested_name_alone() {
    let router = emulator();

    // A snapshot the restore rows restore from, taken off a graph whose own name is not
    // one of the colliding names.
    let source_graph = create_graph(&router, "restore source", "conflict-source").await;
    wait_until_available(&router, "restore source", &format!("/graphs/{source_graph}")).await;
    let source_snapshot =
        create_snapshot(&router, "restore source", &source_graph, "conflict-capture").await;

    for (row, name) in colliding_names().into_iter().enumerate() {
        // The name is taken through a different minting operation on alternating rows,
        // so the index the collision is read from is not one operation's own.
        let seeded_by_import = row % 2 == 1;
        let seeding = format!("seeding {name:?}");
        let holder = if seeded_by_import {
            create_graph_using_import(&router, &seeding, &name).await
        } else {
            create_graph(&router, &seeding, &name).await
        };

        // (the operation colliding with the name, its method, its path, its body)
        let collisions: Vec<(&str, &str, String, Value)> = vec![
            (
                "CreateGraph",
                "POST",
                "/graphs".to_string(),
                json!({"graphName": name, "provisionedMemory": 32}),
            ),
            (
                "CreateGraphUsingImportTask",
                "POST",
                "/importtasks".to_string(),
                json!({
                    "graphName": name,
                    "roleArn": ROLE_ARN,
                    "source": "s3://import-bucket/again",
                }),
            ),
            (
                "RestoreGraphFromSnapshot",
                "POST",
                format!("/snapshots/{source_snapshot}/restore"),
                json!({"graphName": name}),
            ),
        ];

        for (operation, method, path, body) in collisions {
            let case = format!("{operation} on the taken name {name:?}");
            let (status, answer) = call(&router, method, &path, Some(body)).await;
            assert_name_conflict(&case, status, &answer, "Graph", &name, &holder);
        }
    }

    // Every graph that took a name is still there: the refusals above reported a
    // collision, they did not disturb what the store holds.
    let (listed_status, listed) = call(&router, "GET", "/graphs", None).await;
    let held = listed["graphs"].as_array().map(Vec::len);
    assert_eq!(
        (listed_status, held),
        (200, Some(colliding_names().len() + 1)),
        "the seeded graphs must all still be listed, got {listed_status}: {listed}"
    );
}

/// `CreateGraphSnapshot` refuses a `snapshotName` the account already holds in the
/// Region by the same rule, with the snapshot namespace's own resource kind.
#[tokio::test]
async fn a_taken_snapshot_name_is_refused_from_the_requested_name_alone() {
    let router = emulator();

    let graph_id = create_graph(&router, "snapshot source", "snapshot-source").await;
    wait_until_available(&router, "snapshot source", &format!("/graphs/{graph_id}")).await;

    for name in colliding_names() {
        let seeding = format!("seeding {name:?}");
        let holder = create_snapshot(&router, &seeding, &graph_id, &name).await;

        let case = format!("CreateGraphSnapshot on the taken name {name:?}");
        let (status, answer) = call(
            &router,
            "POST",
            "/snapshots",
            Some(json!({"graphIdentifier": graph_id, "snapshotName": name})),
        )
        .await;
        assert_name_conflict(&case, status, &answer, "Snapshot", &name, &holder);
    }

    // Every snapshot that took a name is still there.
    let (listed_status, listed) = call(&router, "GET", "/snapshots", None).await;
    let held = listed["graphSnapshots"].as_array().map(Vec::len);
    assert_eq!(
        (listed_status, held),
        (200, Some(colliding_names().len())),
        "the seeded snapshots must all still be listed, got {listed_status}: {listed}"
    );
}

/// A snapshot name that is taken is reported as the name conflict it is WHATEVER the
/// source graph is doing: the name-uniqueness check does not depend on the graph being
/// in service, so a caller gets the same refusal while the graph is still snapshotting.
#[tokio::test]
async fn a_taken_snapshot_name_is_refused_while_the_graph_is_still_snapshotting() {
    let router = emulator();

    let case = "a second snapshot of the same name, back to back";
    let graph_id = create_graph(&router, case, "back-to-back").await;
    wait_until_available(&router, case, &format!("/graphs/{graph_id}")).await;

    let holder = {
        let (status, created) = call(
            &router,
            "POST",
            "/snapshots",
            Some(json!({"graphIdentifier": graph_id, "snapshotName": "back-to-back-copy"})),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateGraphSnapshot, got {status}: {created}"
        );
        created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no snapshot id in {created}"))
            .to_string()
    };

    // No wait: the source graph is still SNAPSHOTTING here, which is exactly the state
    // this row is about.
    let (status, answer) = call(
        &router,
        "POST",
        "/snapshots",
        Some(json!({"graphIdentifier": graph_id, "snapshotName": "back-to-back-copy"})),
    )
    .await;
    assert_name_conflict(
        case,
        status,
        &answer,
        "Snapshot",
        "back-to-back-copy",
        &holder,
    );
}
