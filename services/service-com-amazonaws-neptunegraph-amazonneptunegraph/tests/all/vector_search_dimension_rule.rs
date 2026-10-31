// SPDX-License-Identifier: CC0-1.0

//! The bound on a graph's vector-search dimension, driven over the wire (`restJson1`)
//! through the same router the emulator serves from.
//!
//! The rule every assertion here is about: where a member's own documentation publishes a
//! bound NARROWER than the constraint trait the model carries for it, the published bound
//! is what the service accepts — a value past it is refused with the 400
//! `ValidationException` whose clause states that bound, in the form the model-driven walk
//! states the same constraint class — and a value within it is stored and reads back.
//!
//! The table below spans what that rule generalises over: where in the documented range
//! the value sits (its published minimum, values inside it, the value just below the
//! published maximum, the published maximum itself, one past it, and far past it, plus
//! values below the modelled minimum), which side of the boundary measures it (the
//! handler for the gap between the published and the modelled maximum, the model-driven
//! walk outside the modelled range), and which operation carried it — both operations
//! whose input carries a `vectorSearchConfiguration`
//! (`smithy_query(query="operation",operation="CreateGraph,CreateGraphUsingImportTask")`;
//! no other input in this service carries one).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    MAX_VECTOR_SEARCH_DIMENSION, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`ProvisionedMemory` is
/// `@range(16..24576)`), which is all a graph these tests create needs.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`, in the account
/// the emulator answers for.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneVectorDimensionRole";

/// An S3 location for the import the sibling minting operation requires.
const IMPORT_SOURCE: &str = "s3://example-bucket/vectors/";

/// The largest dimension the member's documentation publishes: "Specifies the number of
/// dimensions for vector embeddings that will be loaded into the graph. The value is
/// specified as `dimension=`value. Max = 65,535"
/// (`smithy_query(query="doc",shape="CreateGraphInput",member="vectorSearchConfiguration")`;
/// the structure's own documentation publishes the same number).
const PUBLISHED_MAX_DIMENSION: i64 = 65_535;

/// The largest dimension the MODEL admits: `VectorSearchDimension` is
/// `@range(1..65536)` (`smithy_query(query="shape",shape="VectorSearchDimension")`) — one
/// above the published maximum, which is exactly the gap no modelled trait refuses.
const MODELLED_MAX_DIMENSION: i64 = 65_536;

/// The smallest dimension the model admits, from the same `@range` trait.
const MODELLED_MIN_DIMENSION: i64 = 1;

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

/// The `message` member of a failure body.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The whole refusal message a dimension outside the accepted range must carry, or `None`
/// for a dimension the service accepts — restated here independently of the service's own
/// builders.
///
/// The bound a clause states is the bound the value actually broke: a dimension past the
/// modelled maximum breaks the model's own `@range`, and one in the gap between the
/// published maximum and the modelled one breaks the published bound alone. Either way the
/// clause reads the same, which is the point: a caller cannot tell which side of the
/// boundary measured the value.
fn expected_refusal(dimension: i64) -> Option<String> {
    let requirement = if dimension < MODELLED_MIN_DIMENSION {
        format!("Member must have value greater than or equal to {MODELLED_MIN_DIMENSION}")
    } else if dimension > MODELLED_MAX_DIMENSION {
        format!("Member must have value less than or equal to {MODELLED_MAX_DIMENSION}")
    } else if dimension > PUBLISHED_MAX_DIMENSION {
        format!("Member must have value less than or equal to {PUBLISHED_MAX_DIMENSION}")
    } else {
        return None;
    };
    Some(format!(
        "1 validation error detected: Value '{dimension}' at \
         'vectorSearchConfiguration.dimension' failed to satisfy constraint: {requirement}"
    ))
}

/// `CreateGraph` and `CreateGraphUsingImportTask` bodies carrying `dimension`: the two
/// operations whose input carries a `vectorSearchConfiguration`, each keyed by its name
/// and the path it is served at.
fn minting_requests(name: &str, dimension: i64) -> [(&'static str, &'static str, Value); 2] {
    [
        (
            "CreateGraph",
            "/graphs",
            json!({
                "graphName": name,
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
                "vectorSearchConfiguration": {"dimension": dimension},
            }),
        ),
        (
            "CreateGraphUsingImportTask",
            "/importtasks",
            json!({
                "graphName": format!("{name}-imported"),
                "source": IMPORT_SOURCE,
                "roleArn": ROLE_ARN,
                "vectorSearchConfiguration": {"dimension": dimension},
            }),
        ),
    ]
}

/// The graph a minting response names, read back through `GetGraph`: the record the store
/// holds, so acceptance was not an empty success.
async fn stored_graph(router: &MultiServiceRouter, case: &str, minted: &Value) -> Value {
    let id = minted["id"]
        .as_str()
        .or_else(|| minted["graphId"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: no graph id in {minted}"));
    let (status, graph) = call(router, "GET", &format!("/graphs/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetGraph, got {status}: {graph}"
    );
    graph
}

/// Every graph name the store holds, from `ListGraphs`.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let (status, listed) = call(router, "GET", "/graphs", None).await;
    assert_eq!(
        status, 200,
        "expected 200 from ListGraphs, got {status}: {listed}"
    );
    listed["graphs"]
        .as_array()
        .map(|graphs| {
            graphs
                .iter()
                .map(|graph| graph["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_vector_dimension_is_accepted_exactly_as_far_as_the_documented_maximum() {
    let router = emulator();

    // (case, the dimension the request asks for) — the published minimum, values inside
    // the range, the value just below the published maximum, the published maximum
    // itself, one past it (the value the model's own range admits), far past it, and two
    // below the modelled minimum.
    let rows: [(&str, i64); 10] = [
        ("the published minimum", MODELLED_MIN_DIMENSION),
        ("a small index", 2),
        ("a common embedding width", 128),
        ("a wider embedding width", 1536),
        ("one below the published maximum", PUBLISHED_MAX_DIMENSION - 1),
        ("the published maximum", PUBLISHED_MAX_DIMENSION),
        ("one past the published maximum", PUBLISHED_MAX_DIMENSION + 1),
        ("one past the modelled maximum", MODELLED_MAX_DIMENSION + 1),
        ("far past the modelled maximum", MODELLED_MAX_DIMENSION * 4),
        ("below the modelled minimum", MODELLED_MIN_DIMENSION - 1),
    ];

    // The bound this crate publishes is the one the documentation does, so the rows above
    // measure the service's own constant rather than a number only this test knows.
    assert_eq!(
        i64::from(MAX_VECTOR_SEARCH_DIMENSION),
        PUBLISHED_MAX_DIMENSION,
        "the enforced bound must be the documented maximum, got {MAX_VECTOR_SEARCH_DIMENSION}"
    );

    for (index, (case, dimension)) in rows.iter().enumerate() {
        for (operation, path, body) in minting_requests(&format!("vector-graph-{index}"), *dimension)
        {
            let label = format!("{operation}: {case} ({dimension})");
            let name = body["graphName"].as_str().unwrap_or_default().to_string();
            let (status, answer) = call(&router, "POST", path, Some(body)).await;

            let Some(expected) = expected_refusal(*dimension) else {
                assert_eq!(
                    status, 201,
                    "case {label:?}: expected 201, got {status}: {answer}"
                );
                // Reported by the minting call where the operation's output carries the
                // configuration, and read back off the stored record either way.
                if let Some(reported) = answer.get("vectorSearchConfiguration") {
                    let actual = reported["dimension"].as_i64();
                    assert_eq!(
                        actual,
                        Some(*dimension),
                        "case {label:?}: expected the response to report dimension \
                         {dimension}, got {answer}"
                    );
                }
                let graph = stored_graph(&router, &label, &answer).await;
                let stored = graph["vectorSearchConfiguration"]["dimension"].as_i64();
                assert_eq!(
                    stored,
                    Some(*dimension),
                    "case {label:?}: expected the stored graph to hold dimension \
                     {dimension}, got {graph}"
                );
                continue;
            };

            assert_eq!(
                status, 400,
                "case {label:?}: expected 400, got {status}: {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {label:?}: expected ValidationException, got {answer}"
            );
            let actual = message(&answer);
            assert_eq!(
                actual, expected,
                "case {label:?}: expected {expected:?}, got {actual:?}"
            );

            // A refused request mints nothing: the name it asked for is still free.
            let names = listed_names(&router).await;
            assert!(
                !names.contains(&name),
                "case {label:?}: expected no graph named {name:?} to be stored, got {names:?}"
            );
        }
    }
}

/// A dimension the service accepts survives every record derived from the graph it was
/// stored on — the snapshot that captures its configuration and the graph a restore of
/// that snapshot brings up — so the documented maximum is usable end to end and nothing
/// downstream re-reports a value the bound would have refused.
#[tokio::test]
async fn the_documented_maximum_survives_a_snapshot_and_a_restore() {
    let router = emulator();

    let (status, created) = call(
        &router,
        "POST",
        "/graphs",
        Some(json!({
            "graphName": "vector-max-source",
            "provisionedMemory": MIN_PROVISIONED_MEMORY,
            "vectorSearchConfiguration": {"dimension": PUBLISHED_MAX_DIMENSION},
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case the documented maximum: expected 201 from CreateGraph, got {status}: {created}"
    );
    let graph_id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case the documented maximum: no id in {created}"))
        .to_string();

    // The snapshot can only be taken once the graph is in service, and the restore once
    // the snapshot is: both are states the clock reaches on its own, so only this test
    // waits for them.
    wait_until(&router, &format!("/graphs/{graph_id}"), "AVAILABLE").await;
    let (snapshot_status, snapshot) = call(
        &router,
        "POST",
        "/snapshots",
        Some(json!({"graphIdentifier": graph_id, "snapshotName": "vector-max-snapshot"})),
    )
    .await;
    assert_eq!(
        snapshot_status, 201,
        "case the documented maximum: expected 201 from CreateGraphSnapshot, got \
         {snapshot_status}: {snapshot}"
    );
    let snapshot_id = snapshot["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case the documented maximum: no id in {snapshot}"))
        .to_string();
    wait_until(&router, &format!("/snapshots/{snapshot_id}"), "AVAILABLE").await;

    let (restored_status, restored) = call(
        &router,
        "POST",
        &format!("/snapshots/{snapshot_id}/restore"),
        Some(json!({
            "graphName": "vector-max-restored",
            "provisionedMemory": MIN_PROVISIONED_MEMORY,
        })),
    )
    .await;
    assert_eq!(
        restored_status, 201,
        "case the documented maximum: expected 201 from RestoreGraphFromSnapshot, got \
         {restored_status}: {restored}"
    );
    let actual = restored["vectorSearchConfiguration"]["dimension"].as_i64();
    assert_eq!(
        actual,
        Some(PUBLISHED_MAX_DIMENSION),
        "case the documented maximum: expected the restore to report dimension \
         {PUBLISHED_MAX_DIMENSION}, got {restored}"
    );
}

/// Poll one resource until it reports `status`. No handler ever sleeps; only this test
/// does, because it is asking about a state the clock reaches on its own.
async fn wait_until(router: &MultiServiceRouter, path: &str, status: &str) {
    for _ in 0..40 {
        let (code, body) = call(router, "GET", path, None).await;
        assert_eq!(
            code, 200,
            "case {path}: expected 200 from the read, got {code}: {body}"
        );
        if body["status"] == json!(status) {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {path}: never reported {status}, last status {}",
        body["status"]
    );
}
