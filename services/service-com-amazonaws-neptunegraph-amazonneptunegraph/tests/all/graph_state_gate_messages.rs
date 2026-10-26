// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the refusal of an operation that would transition THE GRAPH
//! ITSELF while the graph's status does not permit that transition, driven over the wire
//! (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: `StartGraph`, `StopGraph`, `UpdateGraph`,
//! `DeleteGraph` and `ResetGraph` each answer `ConflictException` (409) whose message
//! states the action the service could not take and the state that held it back —
//! `Unable to <verb> the graph because it is in the <STATUS> state.` — naming the
//! graph's CURRENT status and no resource identifier.
//!
//! So the table below spans what that rule generalises over: every status this service
//! puts a graph into (all ten), each crossed with every one of the five operations that
//! status refuses, rather than re-running one recorded request. The second test drives
//! ONE request against one graph in three successive statuses, so the status in the
//! message can only come from the stored record.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::PROVISIONING_SPAN;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// (`smithy_query(query="operation",operation="StartImportTask")`), used only to drive a
/// graph into `IMPORTING`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneLoadFromS3";

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

/// The whole message a graph state gate must carry, written out here rather than reached
/// from the service's own helper so the test states the expectation independently of the
/// code under test.
///
/// Source: the recorded AWS response for `StartGraph` on an `AVAILABLE` graph, "Unable to
/// start the graph because it is in the AVAILABLE state."; the sibling gates word the
/// same refusal with their own verb.
fn expected_message(verb: &str, graph_status: &str) -> String {
    format!("Unable to {verb} the graph because it is in the {graph_status} state.")
}

/// The wire request for one of the five graph-transitioning operations, as
/// (method, path, body).
///
/// Source: the operations' HTTP bindings
/// (`smithy_query(query="operation",operation="StartGraph,StopGraph,UpdateGraph,DeleteGraph,ResetGraph")`).
fn transition_request(verb: &str, id: &str) -> (&'static str, String, Option<Value>) {
    match verb {
        "start" => ("POST", format!("/graphs/{id}/start"), None),
        "stop" => ("POST", format!("/graphs/{id}/stop"), None),
        "update" => (
            "PATCH",
            format!("/graphs/{id}"),
            Some(json!({"provisionedMemory": 32})),
        ),
        "reset" => (
            "PUT",
            format!("/graphs/{id}"),
            Some(json!({"skipSnapshot": true})),
        ),
        "delete" => (
            "DELETE",
            format!("/graphs/{id}?skipSnapshot=true"),
            None,
        ),
        other => panic!("no graph-transitioning operation is spelled {other:?}"),
    }
}

/// `GET /graphs/{id}`.
async fn get(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/graphs/{id}"), None).await
}

/// Create a graph and return its id and the record `CreateGraph` answered with.
async fn create_ok(router: &MultiServiceRouter, case: &str, name: &str) -> (String, Value) {
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
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no id in {created}"))
        .to_string();
    (id, created)
}

/// Poll `GetGraph` until the graph reports `expected`.
///
/// No handler ever sleeps; only this test waits, because it is asking about a status the
/// clock reaches on its own.
async fn wait_for_status(router: &MultiServiceRouter, case: &str, id: &str, expected: &str) {
    for _ in 0..40 {
        let (status, body) = get(router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetGraph, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = get(router, id).await;
    panic!(
        "case {case:?}: graph never reported {expected}, last status {}",
        body["status"]
    );
}

/// Drive one accepted call and assert the status it reports the graph moved into, so the
/// setup of a transient status is self-checking.
async fn accept(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected_status: &str,
    success_code: u16,
) {
    let (code, answered) = call(router, method, path, body).await;
    assert_eq!(
        code, success_code,
        "case {case:?}: expected {success_code} from {method} {path}, got {code}: {answered}"
    );
    if answered["status"].is_string() {
        assert_eq!(
            answered["status"],
            json!(expected_status),
            "case {case:?}: expected the graph to report {expected_status} after \
             {method} {path}, got {}",
            answered["status"]
        );
    }
}

/// A graph in `graph_status`, left there with the transition that put it there still in
/// flight for the transient statuses — so the caller must fire its refusals immediately.
async fn graph_in_status(
    router: &MultiServiceRouter,
    case: &str,
    graph_status: &str,
    name: &str,
) -> String {
    // A graph is created CREATING and settles into AVAILABLE on its own; every other
    // status is entered by the operation that owns that transition.
    let (id, created) = create_ok(router, case, name).await;
    if graph_status == "CREATING" {
        assert_eq!(
            created["status"],
            json!("CREATING"),
            "case {case:?}: expected CREATING from CreateGraph, got {}",
            created["status"]
        );
        return id;
    }
    wait_for_status(router, case, &id, "AVAILABLE").await;
    match graph_status {
        "AVAILABLE" => {}
        "UPDATING" => {
            accept(
                router,
                case,
                "PATCH",
                &format!("/graphs/{id}"),
                Some(json!({"publicConnectivity": true})),
                "UPDATING",
                200,
            )
            .await;
        }
        "SNAPSHOTTING" => {
            accept(
                router,
                case,
                "POST",
                "/snapshots",
                Some(json!({"graphIdentifier": id, "snapshotName": "gate-snapshot"})),
                "CREATING",
                201,
            )
            .await;
        }
        "IMPORTING" => {
            accept(
                router,
                case,
                "POST",
                &format!("/graphs/{id}/importtasks"),
                Some(json!({"source": "s3://example-bucket/data/", "roleArn": ROLE_ARN})),
                "INITIALIZING",
                201,
            )
            .await;
        }
        "STOPPING" => {
            accept(
                router,
                case,
                "POST",
                &format!("/graphs/{id}/stop"),
                None,
                "STOPPING",
                200,
            )
            .await;
        }
        "STOPPED" => {
            accept(
                router,
                case,
                "POST",
                &format!("/graphs/{id}/stop"),
                None,
                "STOPPING",
                200,
            )
            .await;
            wait_for_status(router, case, &id, "STOPPED").await;
        }
        "STARTING" => {
            accept(
                router,
                case,
                "POST",
                &format!("/graphs/{id}/stop"),
                None,
                "STOPPING",
                200,
            )
            .await;
            wait_for_status(router, case, &id, "STOPPED").await;
            accept(
                router,
                case,
                "POST",
                &format!("/graphs/{id}/start"),
                None,
                "STARTING",
                200,
            )
            .await;
        }
        "RESETTING" => {
            accept(
                router,
                case,
                "PUT",
                &format!("/graphs/{id}"),
                Some(json!({"skipSnapshot": true})),
                "RESETTING",
                200,
            )
            .await;
        }
        "DELETING" => {
            accept(
                router,
                case,
                "DELETE",
                &format!("/graphs/{id}?skipSnapshot=true"),
                None,
                "DELETING",
                200,
            )
            .await;
        }
        other => panic!("case {case:?}: this service puts no graph into {other:?}"),
    }
    id
}

/// Assert one refused transition: the modelled status and code, the whole message the
/// rule words, and that the refusal fronts no identifier.
fn assert_refusal(case: &str, verb: &str, graph_status: &str, id: &str, code: u16, body: &Value) {
    assert_eq!(
        code, 409,
        "case {case:?}: expected 409, got {code}: {body}"
    );
    assert_eq!(
        error_code(body),
        "ConflictException",
        "case {case:?}: unexpected error code in {body}"
    );
    let actual = message(body);
    assert_eq!(
        actual,
        expected_message(verb, graph_status),
        "case {case:?}: expected message {:?}, got {actual:?}",
        expected_message(verb, graph_status)
    );
    assert!(
        !actual.contains(id),
        "case {case:?}: the refusal must not front the graph identifier, got {actual:?}"
    );
}

#[tokio::test]
async fn a_graph_transition_refused_by_the_graphs_status_words_the_action_and_that_status() {
    let router = emulator();

    // (the status the graph is in, the operations that status refuses). Every status
    // this service puts a graph into appears: the two at-rest ones, which refuse only
    // the transitions they cannot start, and the eight in-flight ones, which refuse all
    // five. Source for the status set: the modelled `GraphStatus` enum
    // (`smithy_query(query="shape",shape="GraphStatus")`) minus `FAILED`, which no path
    // in this service produces.
    let rows: &[(&str, &[&str])] = &[
        ("CREATING", &["start", "stop", "update", "reset", "delete"]),
        // A serving graph accepts a stop, an update, a reset and a delete; there is
        // nothing to start.
        ("AVAILABLE", &["start"]),
        ("UPDATING", &["start", "stop", "update", "reset", "delete"]),
        (
            "SNAPSHOTTING",
            &["start", "stop", "update", "reset", "delete"],
        ),
        ("IMPORTING", &["start", "stop", "update", "reset", "delete"]),
        ("STOPPING", &["start", "stop", "update", "reset", "delete"]),
        // A stopped graph rests, so it is deletable and startable; it is not serving,
        // so the three transitions that need a serving graph are refused.
        ("STOPPED", &["stop", "update", "reset"]),
        ("STARTING", &["start", "stop", "update", "reset", "delete"]),
        ("RESETTING", &["start", "stop", "update", "reset", "delete"]),
        ("DELETING", &["start", "stop", "update", "reset", "delete"]),
    ];

    for (graph_status, refused) in rows {
        let name = format!("gate-{}", graph_status.to_lowercase());
        let case_setup = format!("a graph in {graph_status}");
        let id = graph_in_status(&router, &case_setup, graph_status, &name).await;

        // Fired back to back with no wait: an in-flight transition is in flight only
        // until the clock makes it due, and a refusal changes nothing, so one graph
        // serves every operation this status refuses.
        for verb in *refused {
            let case = format!("{verb} a graph in {graph_status}");
            let (method, path, body) = transition_request(verb, &id);
            let (code, answered) = call(&router, method, &path, body).await;
            assert_refusal(&case, verb, graph_status, &id, code, &answered);
        }
    }
}

#[tokio::test]
async fn the_status_a_refusal_names_is_the_one_the_graph_currently_reports() {
    let router = emulator();

    // ONE request — StartGraph — against ONE graph, as the stored record moves: the
    // message can only differ by the status the store holds.
    let (id, _) = create_ok(&router, "a graph read through its own statuses", "gate-tracking").await;

    let mut seen: Vec<String> = Vec::new();
    for expected_status in ["CREATING", "AVAILABLE"] {
        if expected_status == "AVAILABLE" {
            wait_for_status(
                &router,
                "a graph read through its own statuses",
                &id,
                "AVAILABLE",
            )
            .await;
        }
        let (read_code, record) = get(&router, &id).await;
        assert_eq!(
            read_code, 200,
            "case {expected_status}: expected 200 from GetGraph, got {read_code}: {record}"
        );
        assert_eq!(
            record["status"],
            json!(expected_status),
            "case {expected_status}: the graph must report that status, got {}",
            record["status"]
        );
        let (code, answered) = call(&router, "POST", &format!("/graphs/{id}/start"), None).await;
        assert_refusal(
            &format!("start a graph reporting {expected_status}"),
            "start",
            expected_status,
            &id,
            code,
            &answered,
        );
        seen.push(message(&answered));
    }

    // STOPPING: the same request again, while the stop the clock owes is in flight.
    accept(
        &router,
        "a graph read through its own statuses",
        "POST",
        &format!("/graphs/{id}/stop"),
        None,
        "STOPPING",
        200,
    )
    .await;
    let (code, answered) = call(&router, "POST", &format!("/graphs/{id}/start"), None).await;
    assert_refusal(
        "start a graph reporting STOPPING",
        "start",
        "STOPPING",
        &id,
        code,
        &answered,
    );
    seen.push(message(&answered));

    assert_eq!(
        seen.len(),
        3,
        "case one request across three statuses: expected three refusals, got {seen:?}"
    );
    let distinct: std::collections::BTreeSet<&String> = seen.iter().collect();
    assert_eq!(
        distinct.len(),
        seen.len(),
        "case one request across three statuses: the same request in three statuses must \
         word three different refusals, got {seen:?}"
    );

    // Once the stop comes due the graph is startable, which is what the refusals were
    // about: none of them changed the run state.
    wait_for_status(
        &router,
        "a graph read through its own statuses",
        &id,
        "STOPPED",
    )
    .await;
    let (code, started) = call(&router, "POST", &format!("/graphs/{id}/start"), None).await;
    assert_eq!(
        code, 200,
        "case a stopped graph: expected 200 from StartGraph, got {code}: {started}"
    );
    assert_eq!(
        started["status"],
        json!("STARTING"),
        "case a stopped graph: expected STARTING, got {}",
        started["status"]
    );
}
