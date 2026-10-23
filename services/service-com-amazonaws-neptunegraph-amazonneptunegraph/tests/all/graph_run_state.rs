// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for a graph's RUN STATE — `StopGraph` and `StartGraph` — driven
//! over the wire (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: an operation that changes a graph's run
//! state is accepted only on a graph in the status that transition starts from
//! (`AVAILABLE` for a stop, `STOPPED` for a start), is refused `ConflictException`
//! (409) in every other status and `ResourceNotFoundException` (404) for an identifier
//! no graph holds; when accepted it reports the transient status of the transition it
//! started (`STOPPING`, `STARTING`) and the graph settles into that transition's
//! terminal status (`STOPPED`, `AVAILABLE`) once the clock makes it due, carrying every
//! other member of the record across unchanged.
//!
//! So the tables below span what that rule generalises over: the graph configurations a
//! stop/start cycle has to preserve, every status the two operations can be called in,
//! and both operations against the same set of identifiers — rather than re-running one
//! recorded request.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::PROVISIONING_SPAN;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names, so the ARNs and endpoints the
/// service builds are predictable.
const REGION: &str = "us-east-1";

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// The members of a graph record that a stop/start cycle must not change: its
/// identity, its endpoint and every configuration member `GetGraph` reports.
/// Source: the `StopGraph`/`StartGraph` output shapes
/// (`smithy_query(query="operation",operation="StopGraph,StartGraph")`), minus the two
/// members a run-state change is about (`status`, `statusReason`).
const PRESERVED_MEMBERS: &[&str] = &[
    "id",
    "name",
    "arn",
    "endpoint",
    "createTime",
    "provisionedMemory",
    "publicConnectivity",
    "replicaCount",
    "deletionProtection",
    "kmsKeyIdentifier",
    "vectorSearchConfiguration",
    "sourceSnapshotId",
    "buildNumber",
];

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

/// `GET /graphs/{id}`.
async fn get(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/graphs/{id}"), None).await
}

/// `POST /graphs/{id}/stop`.
async fn stop(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "POST", &format!("/graphs/{id}/stop"), None).await
}

/// `POST /graphs/{id}/start`.
async fn start(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "POST", &format!("/graphs/{id}/start"), None).await
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

/// Create a graph and return its id, failing the test if the call did not answer 201.
async fn create_ok(router: &MultiServiceRouter, body: Value) -> String {
    let (status, created) = call(router, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from CreateGraph, got {status}: {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: no id in {created}"))
        .to_string()
}

/// Poll `GetGraph` until the graph reports `expected`, then return its record.
///
/// No handler ever sleeps; only this test waits, because it is asking about a status
/// the clock reaches on its own.
async fn wait_for_status(router: &MultiServiceRouter, case: &str, id: &str, expected: &str) -> Value {
    for _ in 0..40 {
        let (status, body) = get(router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetGraph, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = get(router, id).await;
    panic!(
        "case {case:?}: graph never reported {expected}, last status {}",
        body["status"]
    );
}

/// A graph created and then waited into service.
async fn create_available(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let id = create_ok(router, body).await;
    wait_for_status(router, case, &id, "AVAILABLE").await;
    id
}

/// A graph created, waited into service, stopped and waited into `STOPPED`.
async fn create_stopped(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let id = create_available(router, case, body).await;
    let (status, stopped) = stop(router, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from StopGraph, got {status}: {stopped}"
    );
    wait_for_status(router, case, &id, "STOPPED").await;
    id
}

/// Assert every member a run-state change must carry across is byte-identical.
fn assert_record_preserved(case: &str, step: &str, before: &Value, after: &Value) {
    for member in PRESERVED_MEMBERS {
        assert_eq!(
            after[member], before[member],
            "case {case:?} ({step}): member {member:?} changed, {} -> {}",
            before[member], after[member]
        );
    }
}

#[tokio::test]
async fn stop_then_start_cycles_a_graph_through_the_statuses_the_clock_owes() {
    let router = emulator();

    // (case, CreateGraph body). The rows differ along the configuration a run-state
    // change has to preserve: each published bound of `provisionedMemory`
    // (range 16..24576) and `replicaCount` (range 0..2), both booleans, the optional
    // `kmsKeyIdentifier`/`vectorSearchConfiguration`, and a tagged graph.
    let cases: Vec<(&str, Value)> = vec![
        (
            "smallest graph, every option defaulted",
            json!({"graphName": "minimal-graph", "provisionedMemory": 16}),
        ),
        (
            "largest provisionedMemory, no replicas, public endpoint",
            json!({
                "graphName": "large-public-graph",
                "provisionedMemory": 24576,
                "replicaCount": 0,
                "publicConnectivity": true,
            }),
        ),
        (
            "replicas at the published maximum, encrypted, vector search",
            json!({
                "graphName": "encrypted-graph",
                "provisionedMemory": 128,
                "replicaCount": 2,
                "kmsKeyIdentifier": KMS_KEY_ARN,
                "vectorSearchConfiguration": {"dimension": 1536},
            }),
        ),
        (
            "deletion protection and tags",
            json!({
                "graphName": "protected-tagged-graph",
                "provisionedMemory": 64,
                "deletionProtection": true,
                "tags": {"env": "test", "owner": "fixer"},
            }),
        ),
    ];

    for (case, body) in cases {
        let id = create_available(&router, case, body).await;
        let serving = get(&router, &id).await.1;

        // A stop is accepted on a serving graph, and reports the transition it started.
        let (status, stopping) = stop(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from StopGraph, got {status}: {stopping}"
        );
        assert_eq!(
            stopping["status"],
            json!("STOPPING"),
            "case {case:?}: expected STOPPING from StopGraph, got {}",
            stopping["status"]
        );
        assert_record_preserved(case, "StopGraph", &serving, &stopping);

        // The transition is not instantaneous: a caller reading straight after the
        // call still sees it in flight.
        let (get_status, in_flight) = get(&router, &id).await;
        assert_eq!(
            get_status, 200,
            "case {case:?}: expected 200 from GetGraph, got {get_status}: {in_flight}"
        );
        assert_eq!(
            in_flight["status"],
            json!("STOPPING"),
            "case {case:?}: expected STOPPING from an immediate GetGraph, got {}",
            in_flight["status"]
        );

        // Only the clock finishes it, and the record is otherwise untouched.
        let stopped = wait_for_status(&router, case, &id, "STOPPED").await;
        assert_record_preserved(case, "settled stop", &serving, &stopped);

        // A start is accepted on a stopped graph, and the same two steps hold.
        let (status, starting) = start(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from StartGraph, got {status}: {starting}"
        );
        assert_eq!(
            starting["status"],
            json!("STARTING"),
            "case {case:?}: expected STARTING from StartGraph, got {}",
            starting["status"]
        );
        assert_record_preserved(case, "StartGraph", &serving, &starting);

        let (get_status, in_flight) = get(&router, &id).await;
        assert_eq!(
            get_status, 200,
            "case {case:?}: expected 200 from GetGraph, got {get_status}: {in_flight}"
        );
        assert_eq!(
            in_flight["status"],
            json!("STARTING"),
            "case {case:?}: expected STARTING from an immediate GetGraph, got {}",
            in_flight["status"]
        );

        let serving_again = wait_for_status(&router, case, &id, "AVAILABLE").await;
        assert_record_preserved(case, "settled start", &serving, &serving_again);

        // The cycle is repeatable: a restarted graph can be stopped again.
        let (status, stopping_again) = stop(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from a second StopGraph, got {status}: {stopping_again}"
        );
        assert_eq!(
            stopping_again["status"],
            json!("STOPPING"),
            "case {case:?}: expected STOPPING from a second StopGraph, got {}",
            stopping_again["status"]
        );
    }
}

#[tokio::test]
async fn a_stopped_graph_still_appears_in_the_reporting_operations() {
    let router = emulator();
    let id = create_stopped(
        &router,
        "listed while stopped",
        json!({"graphName": "stopped-but-listed", "provisionedMemory": 16}),
    )
    .await;

    let (status, listed) = call(&router, "GET", "/graphs", None).await;
    assert_eq!(
        status, 200,
        "case listed while stopped: expected 200 from ListGraphs, got {status}: {listed}"
    );
    let summary = listed["graphs"]
        .as_array()
        .expect("graphs is a list")
        .iter()
        .find(|summary| summary["id"] == json!(id))
        .unwrap_or_else(|| panic!("case listed while stopped: {id} missing from {listed}"));
    assert_eq!(
        summary["status"],
        json!("STOPPED"),
        "case listed while stopped: expected STOPPED, got {}",
        summary["status"]
    );

    // Reading a stopped graph does not restart it: two reads report the same status.
    for read in 0..2 {
        let (get_status, body) = get(&router, &id).await;
        assert_eq!(
            get_status, 200,
            "case read {read}: expected 200 from GetGraph, got {get_status}: {body}"
        );
        assert_eq!(
            body["status"],
            json!("STOPPED"),
            "case read {read}: a read must not change the run state, got {}",
            body["status"]
        );
    }
}

#[tokio::test]
async fn stop_and_start_refuse_a_graph_whose_status_the_transition_cannot_start_from() {
    let router = emulator();

    // Every status a run-state change can be asked for in, against both operations:
    // each row names the status the graph is in, the operation, and the status the
    // graph must still report after the refusal.
    //
    // CREATING: a graph straight out of CreateGraph.
    let creating = create_ok(
        &router,
        json!({"graphName": "creating-graph", "provisionedMemory": 16}),
    )
    .await;
    for (case, operation) in [
        ("stop while CREATING", "stop"),
        ("start while CREATING", "start"),
    ] {
        let (status, body) = call(&router, "POST", &format!("/graphs/{creating}/{operation}"), None)
            .await;
        assert_eq!(status, 409, "case {case:?}: expected 409, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
    let (_, still_creating) = get(&router, &creating).await;
    assert_eq!(
        still_creating["status"],
        json!("CREATING"),
        "case refused while CREATING: the status must be unchanged, got {}",
        still_creating["status"]
    );

    // AVAILABLE: a start is a conflict, because there is nothing to start.
    let available = create_available(
        &router,
        "start while AVAILABLE",
        json!({"graphName": "available-graph", "provisionedMemory": 16}),
    )
    .await;
    let (status, body) = start(&router, &available).await;
    assert_eq!(
        status, 409,
        "case start while AVAILABLE: expected 409, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case start while AVAILABLE: unexpected error code in {body}"
    );

    // STOPPING: neither a second stop nor a start is accepted while the stop the clock
    // owes is still in flight.
    let (stop_status, _) = stop(&router, &available).await;
    assert_eq!(stop_status, 200, "the stop under test must be accepted, got {stop_status}");
    for (case, operation) in [
        ("stop while STOPPING", "stop"),
        ("start while STOPPING", "start"),
    ] {
        let (status, body) =
            call(&router, "POST", &format!("/graphs/{available}/{operation}"), None).await;
        assert_eq!(status, 409, "case {case:?}: expected 409, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // STOPPED: a stop is a conflict, because the graph is already stopped.
    wait_for_status(&router, "stop while STOPPED", &available, "STOPPED").await;
    let (status, body) = stop(&router, &available).await;
    assert_eq!(status, 409, "case stop while STOPPED: expected 409, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case stop while STOPPED: unexpected error code in {body}"
    );
    let (_, still_stopped) = get(&router, &available).await;
    assert_eq!(
        still_stopped["status"],
        json!("STOPPED"),
        "case stop while STOPPED: the status must be unchanged, got {}",
        still_stopped["status"]
    );

    // STARTING: neither a second start nor a stop is accepted while the start the clock
    // owes is still in flight.
    let (start_status, _) = start(&router, &available).await;
    assert_eq!(start_status, 200, "the start under test must be accepted, got {start_status}");
    for (case, operation) in [
        ("start while STARTING", "start"),
        ("stop while STARTING", "stop"),
    ] {
        let (status, body) =
            call(&router, "POST", &format!("/graphs/{available}/{operation}"), None).await;
        assert_eq!(status, 409, "case {case:?}: expected 409, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: unexpected error code in {body}"
        );
    }
    // The refusals changed nothing: the start still comes due on its own.
    tokio::time::sleep(PROVISIONING_SPAN + Duration::from_millis(50)).await;
    let (_, serving) = get(&router, &available).await;
    assert_eq!(
        serving["status"],
        json!("AVAILABLE"),
        "case refused while STARTING: the start must still settle, got {}",
        serving["status"]
    );

    // SNAPSHOTTING: a graph being snapshotted has a transition in flight too.
    let snapshotted = create_available(
        &router,
        "stop while SNAPSHOTTING",
        json!({"graphName": "snapshotted-graph", "provisionedMemory": 16}),
    )
    .await;
    let (snapshot_status, snapshot) = call(
        &router,
        "POST",
        "/snapshots",
        Some(json!({
            "graphIdentifier": snapshotted,
            "snapshotName": "run-state-snapshot",
        })),
    )
    .await;
    assert_eq!(
        snapshot_status, 201,
        "the snapshot under test must be accepted, got {snapshot_status}: {snapshot}"
    );
    let (status, body) = stop(&router, &snapshotted).await;
    assert_eq!(
        status, 409,
        "case stop while SNAPSHOTTING: expected 409, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case stop while SNAPSHOTTING: unexpected error code in {body}"
    );
}

#[tokio::test]
async fn stop_and_start_refuse_an_identifier_no_graph_holds() {
    let router = emulator();
    create_ok(
        &router,
        json!({"graphName": "the-only-graph", "provisionedMemory": 16}),
    )
    .await;

    for operation in ["stop", "start"] {
        // Well-formed ids (the modelled `^g-[a-z0-9]{10}$`) that name no stored graph.
        for id in ["g-0000000000", "g-zzzzzzzzzz", "g-a1b2c3d4e5"] {
            let case = format!("{operation} {id}");
            let (status, body) =
                call(&router, "POST", &format!("/graphs/{id}/{operation}"), None).await;
            assert_eq!(status, 404, "case {case:?}: expected 404, got {status}: {body}");
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: unexpected error code in {body}"
            );
        }
        // An identifier the modelled pattern refuses is a 400, not a 404.
        for id in ["not-a-graph-id", "g-TOOSHORT", "gs-0123456789"] {
            let case = format!("{operation} {id}");
            let (status, body) =
                call(&router, "POST", &format!("/graphs/{id}/{operation}"), None).await;
            assert_eq!(status, 400, "case {case:?}: expected 400, got {status}: {body}");
        }
    }
}

#[tokio::test]
async fn a_stopped_graph_is_not_reconfigurable_but_is_deletable() {
    let router = emulator();

    // A stopped graph rests: nothing is in flight, so deleting it is accepted, while
    // reconfiguring it (which needs a serving graph) is not.
    let reconfigured = create_stopped(
        &router,
        "update while STOPPED",
        json!({"graphName": "stopped-update-target", "provisionedMemory": 16}),
    )
    .await;
    let (status, body) = call(
        &router,
        "PATCH",
        &format!("/graphs/{reconfigured}"),
        Some(json!({"provisionedMemory": 32})),
    )
    .await;
    assert_eq!(
        status, 409,
        "case update while STOPPED: expected 409, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case update while STOPPED: unexpected error code in {body}"
    );
    let (_, unchanged) = get(&router, &reconfigured).await;
    assert_eq!(
        unchanged["provisionedMemory"],
        json!(16),
        "case update while STOPPED: the refused patch must change nothing, got {unchanged}"
    );

    let deleted_id = create_stopped(
        &router,
        "delete while STOPPED",
        json!({"graphName": "stopped-delete-target", "provisionedMemory": 16}),
    )
    .await;
    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/graphs/{deleted_id}?skipSnapshot=true"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete while STOPPED: expected 200, got {status}: {deleted}"
    );
    assert_eq!(
        deleted["status"],
        json!("DELETING"),
        "case delete while STOPPED: expected DELETING, got {}",
        deleted["status"]
    );
    let (get_status, body) = get(&router, &deleted_id).await;
    assert_eq!(
        get_status, 404,
        "case delete while STOPPED: the graph must be gone, got {get_status}: {body}"
    );
}
