// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the default a minted graph takes for a configuration member
//! the request omits, driven over the wire (`restJson1`) through the same router the
//! emulator serves from.
//!
//! The rule every assertion here is about: on every operation that MINTS a graph, a
//! configuration member the request omits takes the default the member's own model
//! documentation states, whatever value that call could otherwise have inherited — and
//! a member the request carries is reported exactly as supplied.
//!
//! `replicaCount` is the member of these inputs whose documentation states a default:
//! "The number of replicas in other AZs. Min =0, Max = 2, Default =1"
//! (`smithy_query(query="doc",shape="RestoreGraphFromSnapshotInput",
//! member="replicaCount")`, and the same sentence on `CreateGraphInput` and
//! `CreateGraphUsingImportTaskInput`). So an omitted `replicaCount` reports 1 on
//! `CreateGraph`, on `CreateGraphUsingImportTask` and on `RestoreGraphFromSnapshot` —
//! the last of which could otherwise have reported the count the snapshot captured
//! from the graph it was taken from.
//!
//! The members of a restore input whose documentation states NO default
//! (`provisionedMemory` — "Min = 16" only — `publicConnectivity`,
//! `deletionProtection`) are unaffected by this rule and keep being restored as the
//! snapshot captured them; `graph_restore_from_snapshot.rs` covers them.
//!
//! So the tables below span what the rule generalises over: which minting operation
//! runs, every published `ReplicaCount` the request may carry (`@range(0..2)`), and —
//! for a restore — every count the snapshot could have captured, including the two
//! that differ from the documented default.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::PROVISIONING_SPAN;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows: `ProvisionedMemory` is
/// `@range(16..24576)` (`smithy_query(query="operation",operation="CreateGraph")`).
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// The replica count a graph takes when the request that mints it carries none, from
/// the member's own documentation: "The number of replicas in other AZs. Min =0, Max =
/// 2, Default =1".
const DOCUMENTED_DEFAULT_REPLICA_COUNT: i64 = 1;

/// Every value a request may carry for `replicaCount`, from the modelled
/// `ReplicaCount` range `@range(0..2)` — the whole published interval, so the table
/// covers the two values that differ from the documented default as well as the one
/// that matches it.
const PUBLISHED_REPLICA_COUNTS: &[i64] = &[0, 1, 2];

/// A role ARN for `CreateGraphUsingImportTask`, which requires one.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneImportRole";

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

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this
/// test does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, case: &str, path: &str) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read of {path}, got {status}: {body}"
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

/// `POST /graphs` with `body`, waiting the graph it mints into service and returning
/// the record `CreateGraph` reported.
async fn create_graph(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, created) = call(router, "POST", "/graphs", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no id in {created}"))
        .to_string();
    wait_until_available(router, case, &format!("/graphs/{id}")).await;
    created
}

/// Take a snapshot of `graph_id`, wait it into `AVAILABLE`, and return its id. The
/// source graph goes through `SNAPSHOTTING`, so it is waited back into service too.
async fn snapshot_in_service(
    router: &MultiServiceRouter,
    case: &str,
    graph_id: &str,
    snapshot_name: &str,
) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/snapshots",
        Some(json!({"graphIdentifier": graph_id, "snapshotName": snapshot_name})),
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
    wait_until_available(router, case, &format!("/snapshots/{id}")).await;
    wait_until_available(router, case, &format!("/graphs/{graph_id}")).await;
    id
}

/// Assert that a minted graph, and an independent read of it, report
/// `expected_replica_count`.
async fn assert_replica_count(
    router: &MultiServiceRouter,
    case: &str,
    minted: &Value,
    expected_replica_count: i64,
) {
    assert_eq!(
        minted["replicaCount"],
        json!(expected_replica_count),
        "case {case:?}: expected replicaCount {expected_replica_count}, got {}",
        minted["replicaCount"]
    );
    // The round trip: an independent read reports the record that was stored, not the
    // response that was rendered.
    let id = minted["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no graph id in {minted}"));
    let (status, read) = call(router, "GET", &format!("/graphs/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetGraph, got {status}: {read}"
    );
    assert_eq!(
        read["replicaCount"],
        json!(expected_replica_count),
        "case {case:?}: GetGraph must report the stored replicaCount \
         {expected_replica_count}, got {}",
        read["replicaCount"]
    );
}

#[tokio::test]
async fn a_restore_takes_the_documented_default_for_an_omitted_replica_count() {
    let router = emulator();

    // (case, the replicaCount the SOURCE graph was created with, the replicaCount the
    // restore request carries, the count the restored graph must report)
    let mut cases: Vec<(String, Option<i64>, Option<i64>, i64)> = Vec::new();
    for captured in PUBLISHED_REPLICA_COUNTS {
        // The member is omitted by the restore, so the documented default answers for
        // it however many replicas the snapshot captured.
        cases.push((
            format!("a capture of {captured} replicas, the restore omitting the member"),
            Some(*captured),
            None,
            DOCUMENTED_DEFAULT_REPLICA_COUNT,
        ));
        // The member is carried, so the request's value answers for it — including the
        // value equal to what the snapshot captured, and the one equal to the default.
        for requested in PUBLISHED_REPLICA_COUNTS {
            cases.push((
                format!("a capture of {captured} replicas, the restore asking for {requested}"),
                Some(*captured),
                Some(*requested),
                *requested,
            ));
        }
    }
    // A source graph that took the default itself, snapshotted and restored with the
    // member omitted on both calls.
    cases.push((
        "a capture taken from a graph that was itself created without the member".to_string(),
        None,
        None,
        DOCUMENTED_DEFAULT_REPLICA_COUNT,
    ));

    for (index, (case, captured, requested, expected)) in cases.into_iter().enumerate() {
        let mut source_body = json!({
            "graphName": format!("default-source-{index}"),
            "provisionedMemory": MIN_PROVISIONED_MEMORY,
        });
        if let Some(captured) = captured {
            source_body["replicaCount"] = json!(captured);
        }
        let source = create_graph(&router, &case, source_body).await;
        let source_id = source["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no source graph id in {source}"))
            .to_string();
        // What the snapshot captures is the source graph's own stored count, which is
        // the documented default when the create omitted the member too.
        let expected_capture = captured.unwrap_or(DOCUMENTED_DEFAULT_REPLICA_COUNT);
        assert_eq!(
            source["replicaCount"],
            json!(expected_capture),
            "case {case:?}: expected the source graph to hold {expected_capture} replicas, got {}",
            source["replicaCount"]
        );
        let snapshot_id = snapshot_in_service(
            &router,
            &case,
            &source_id,
            &format!("default-snapshot-{index}"),
        )
        .await;

        let mut body = json!({"graphName": format!("default-restored-{index}")});
        if let Some(requested) = requested {
            body["replicaCount"] = json!(requested);
        }
        let (status, restored) = call(
            &router,
            "POST",
            &format!("/snapshots/{snapshot_id}/restore"),
            Some(body),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from RestoreGraphFromSnapshot, got {status}: {restored}"
        );
        assert_replica_count(&router, &case, &restored, expected).await;
        // The restore left the snapshotted graph alone, so the capture is still the
        // source graph's own configuration.
        let (source_status, source_read) =
            call(&router, "GET", &format!("/graphs/{source_id}"), None).await;
        assert_eq!(
            source_status, 200,
            "case {case:?}: expected 200 from GetGraph on the source, got {source_status}: \
             {source_read}"
        );
        assert_eq!(
            source_read["replicaCount"],
            json!(expected_capture),
            "case {case:?}: the restore must not change the source graph's \
             {expected_capture} replicas, got {}",
            source_read["replicaCount"]
        );
    }
}

#[tokio::test]
async fn every_graph_minting_operation_takes_the_same_documented_default() {
    let router = emulator();

    // (case, the replicaCount the request carries, the count the minted graph must
    // report) — run once per operation that mints a graph.
    let mut cases: Vec<(String, Option<i64>, i64)> = vec![(
        "the member omitted".to_string(),
        None,
        DOCUMENTED_DEFAULT_REPLICA_COUNT,
    )];
    for requested in PUBLISHED_REPLICA_COUNTS {
        cases.push((
            format!("the member carried as {requested}"),
            Some(*requested),
            *requested,
        ));
    }

    for (index, (case, requested, expected)) in cases.into_iter().enumerate() {
        // `CreateGraph`.
        let mut create_body = json!({
            "graphName": format!("minted-create-{index}"),
            "provisionedMemory": MIN_PROVISIONED_MEMORY,
        });
        if let Some(requested) = requested {
            create_body["replicaCount"] = json!(requested);
        }
        let case_create = format!("CreateGraph with {case}");
        let created = create_graph(&router, &case_create, create_body).await;
        assert_replica_count(&router, &case_create, &created, expected).await;

        // `CreateGraphUsingImportTask`, which mints a graph as well as a task, so the
        // count is read off the graph the task reports.
        let mut import_body = json!({
            "graphName": format!("minted-import-{index}"),
            "roleArn": ROLE_ARN,
            "source": "s3://import-bucket/seed",
        });
        if let Some(requested) = requested {
            import_body["replicaCount"] = json!(requested);
        }
        let case_import = format!("CreateGraphUsingImportTask with {case}");
        let (status, task) = call(&router, "POST", "/importtasks", Some(import_body)).await;
        assert_eq!(
            status, 201,
            "case {case_import:?}: expected 201 from CreateGraphUsingImportTask, got \
             {status}: {task}"
        );
        let graph_id = task["graphId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case_import:?}: no graphId in {task}"))
            .to_string();
        let (graph_status, graph) = call(&router, "GET", &format!("/graphs/{graph_id}"), None).await;
        assert_eq!(
            graph_status, 200,
            "case {case_import:?}: expected 200 from GetGraph, got {graph_status}: {graph}"
        );
        assert_replica_count(&router, &case_import, &graph, expected).await;
    }
}
