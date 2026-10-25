// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `RestoreGraphFromSnapshot`, driven over the wire (`restJson1`)
//! through the same router the emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * A restore mints a NEW graph out of what the snapshot captured: it reports a
//!   minted `^g-[a-z0-9]{10}$` id, the ARN and endpoint of a `graph` resource in the
//!   request's Region, the name the request chose, the snapshot's id as its
//!   `sourceSnapshotId`, the snapshot's encryption key, and a `CREATING` status the
//!   clock settles to `AVAILABLE`.
//! * Every configuration member the request carries overrides the captured value for
//!   it, and every member it omits is restored as the snapshot captured it — including
//!   `vectorSearchConfiguration`, which the request cannot carry at all
//!   (`smithy_query(query="operation",operation="RestoreGraphFromSnapshot")`).
//! * The capture is of the graph AS IT WAS when the snapshot was taken, so a restore
//!   reproduces that configuration even after the source graph is reconfigured or
//!   deleted.
//! * A snapshot whose creation is still in flight cannot be restored (409), a name
//!   another graph of the Region holds conflicts (409), an identifier naming no stored
//!   snapshot is refused (404), and an input outside the modelled constraints is
//!   refused (400) — none of which persists anything.
//!
//! So each table below spans what its rule generalises over — which members the
//! request overrides, whether the source graph is encrypted or vector-indexed, the
//! tag entry count, and each published range bound and one value past it — rather
//! than re-running one recorded request.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    ARN_NAMESPACE, PROVISIONING_SPAN, SERVICE_OWNED_KMS_KEY,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names, so the ARNs the service mints
/// are predictable.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows: `ProvisionedMemory` is
/// `@range(16..24576)`
/// (`smithy_query(query="operation",operation="RestoreGraphFromSnapshot")`).
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// The largest `provisionedMemory` the model allows, from the same `@range` trait.
const MAX_PROVISIONED_MEMORY: i64 = 24576;

/// The largest `replicaCount` the model allows: `ReplicaCount` is `@range(0..2)`.
const MAX_REPLICA_COUNT: i64 = 2;

/// The longest `graphName` the model allows: `GraphName` is `@length(1..63)`.
const MAX_GRAPH_NAME_CHARS: usize = 63;

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// The configuration members a restore request may override, which are exactly the
/// members `RestoreGraphFromSnapshotInput` carries beside the snapshot and the name.
const OVERRIDABLE_MEMBERS: &[&str] = &[
    "provisionedMemory",
    "publicConnectivity",
    "deletionProtection",
    "replicaCount",
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

/// Percent-encode one path segment.
///
/// `resourceArn` is bound to a NON-greedy `{resourceArn}` label (`/tags/{resourceArn}`),
/// so the `/` inside an ARN has to arrive encoded or it would split the URI into
/// another path segment. Only the RFC 3986 unreserved set is left as-is.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
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

/// Whether `value` matches the modelled `GraphId` pattern `^g-[a-z0-9]{10}$`.
fn is_graph_id(value: &str) -> bool {
    value.strip_prefix("g-").is_some_and(|suffix| {
        suffix.len() == 10
            && suffix
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// `POST /snapshots/{snapshotIdentifier}/restore` with `body`.
async fn restore(router: &MultiServiceRouter, snapshot_identifier: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/snapshots/{snapshot_identifier}/restore"),
        Some(body),
    )
    .await
}

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this
/// test does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, path: &str) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {path}: expected 200 from the read, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {path}: never became AVAILABLE, last status {}",
        body["status"]
    );
}

/// Create a graph from `body` (which carries every member but the name) and wait it
/// into service, returning the record `CreateGraph` reported.
async fn graph_in_service(router: &MultiServiceRouter, name: &str, body: &Value) -> Value {
    let mut request = body.clone();
    request["graphName"] = json!(name);
    let (status, created) = call(router, "POST", "/graphs", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {request}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {request}: no id in {created}"))
        .to_string();
    wait_until_available(router, &format!("/graphs/{id}")).await;
    created
}

/// Take a snapshot of `graph_id`, wait it into `AVAILABLE`, and return its id. The
/// source graph goes through `SNAPSHOTTING`, so it is waited back into service too.
async fn snapshot_in_service(
    router: &MultiServiceRouter,
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
        "case {snapshot_name:?}: expected 201 from CreateGraphSnapshot, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {snapshot_name:?}: no id in {created}"))
        .to_string();
    wait_until_available(router, &format!("/snapshots/{id}")).await;
    wait_until_available(router, &format!("/graphs/{graph_id}")).await;
    id
}

/// One row of the restore table: the case name, the `CreateGraph` body the SOURCE
/// graph is created with, the members the restore request overrides, and the tags it
/// carries.
type RestoreCase = (&'static str, Value, Value, BTreeMap<String, String>);

#[tokio::test]
async fn a_restore_reports_the_capture_with_the_requests_overrides() {
    let router = emulator();

    let cases: Vec<RestoreCase> = vec![
        (
            "no overrides, an unencrypted graph on the service's defaults",
            json!({"provisionedMemory": MIN_PROVISIONED_MEMORY}),
            json!({}),
            BTreeMap::new(),
        ),
        (
            "every overridable member carried, a vector-indexed source",
            json!({
                "provisionedMemory": 32,
                "replicaCount": 0,
                "publicConnectivity": true,
                "deletionProtection": false,
                "vectorSearchConfiguration": {"dimension": 128},
            }),
            json!({
                "provisionedMemory": 64,
                "replicaCount": MAX_REPLICA_COUNT,
                "publicConnectivity": false,
                "deletionProtection": true,
            }),
            [("env".to_string(), "prod".to_string())]
                .into_iter()
                .collect(),
        ),
        (
            "only the memory overridden, to the published upper bound, an encrypted source",
            json!({
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
                "replicaCount": MAX_REPLICA_COUNT,
                "publicConnectivity": true,
                "kmsKeyIdentifier": KMS_KEY_ARN,
            }),
            json!({"provisionedMemory": MAX_PROVISIONED_MEMORY}),
            [
                ("env".to_string(), "dev".to_string()),
                ("team".to_string(), "graph".to_string()),
                ("cost-centre".to_string(), String::new()),
            ]
            .into_iter()
            .collect(),
        ),
        (
            "the memory overridden down to the published lower bound, a one-dimension index",
            json!({
                "provisionedMemory": MAX_PROVISIONED_MEMORY,
                "vectorSearchConfiguration": {"dimension": 1},
                "deletionProtection": false,
            }),
            json!({"provisionedMemory": MIN_PROVISIONED_MEMORY}),
            BTreeMap::new(),
        ),
    ];

    for (index, (case, source_body, overrides, tags)) in cases.into_iter().enumerate() {
        let source = graph_in_service(&router, &format!("restore-source-{index}"), &source_body).await;
        let source_id = source["id"].as_str().expect("a source graph id").to_string();
        let snapshot_id =
            snapshot_in_service(&router, &source_id, &format!("restore-snapshot-{index}")).await;

        let restored_name = format!("restored-graph-{index}");
        let mut body = overrides.clone();
        body["graphName"] = json!(restored_name);
        body["tags"] = json!(tags);
        let (status, restored) = restore(&router, &snapshot_id, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status}: {restored}"
        );

        let id = restored["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no id in {restored}"))
            .to_string();
        assert!(
            is_graph_id(&id),
            "case {case:?}: expected an id matching ^g-[a-z0-9]{{10}}$, got {id:?}"
        );
        assert_ne!(
            id, source_id,
            "case {case:?}: a restore mints a new graph, got the source graph's id {id:?}"
        );
        let expected_arn = engine::emulator_arn(ARN_NAMESPACE, REGION, &format!("graph/{id}"));
        assert_eq!(
            restored["arn"],
            json!(expected_arn),
            "case {case:?}: expected arn {expected_arn:?}, got {}",
            restored["arn"]
        );
        assert_eq!(
            restored["name"],
            json!(restored_name),
            "case {case:?}: expected the requested name {restored_name:?}, got {}",
            restored["name"]
        );
        assert_eq!(
            restored["status"],
            json!("CREATING"),
            "case {case:?}: a restored graph is not usable when the call returns, \
             expected CREATING, got {}",
            restored["status"]
        );
        assert_eq!(
            restored["sourceSnapshotId"],
            json!(snapshot_id),
            "case {case:?}: expected sourceSnapshotId {snapshot_id:?}, got {}",
            restored["sourceSnapshotId"]
        );
        // The restored data is encrypted with the key the snapshotted data is
        // encrypted with; a graph created without a customer managed key is encrypted
        // with the service-owned key its sentinel names.
        let expected_key = source_body
            .get("kmsKeyIdentifier")
            .cloned()
            .unwrap_or_else(|| json!(SERVICE_OWNED_KMS_KEY));
        assert_eq!(
            restored["kmsKeyIdentifier"], expected_key,
            "case {case:?}: a restore carries the snapshot's key, expected \
             {expected_key}, got {}",
            restored["kmsKeyIdentifier"]
        );
        // The request cannot carry a vector-search configuration at all, so the
        // restored graph indexes its vectors exactly as the snapshotted one did.
        assert_eq!(
            restored["vectorSearchConfiguration"], source["vectorSearchConfiguration"],
            "case {case:?}: expected the captured vectorSearchConfiguration {}, got {}",
            source["vectorSearchConfiguration"], restored["vectorSearchConfiguration"]
        );
        // The rule itself: an overridden member reports the request's value, an
        // omitted one reports what the snapshot captured.
        for member in OVERRIDABLE_MEMBERS {
            let expected = overrides
                .get(*member)
                .cloned()
                .unwrap_or_else(|| source[*member].clone());
            assert_eq!(
                restored[*member], expected,
                "case {case:?}: expected {member} {expected}, got {}",
                restored[*member]
            );
        }
        assert!(
            restored["buildNumber"].as_str().is_some_and(|b| !b.is_empty()),
            "case {case:?}: expected a buildNumber, got {}",
            restored["buildNumber"]
        );
        assert!(
            restored["createTime"].as_f64().is_some_and(|t| t > 0.0),
            "case {case:?}: expected a createTime, got {}",
            restored["createTime"]
        );

        // The round trip: an independent read reports the record that was stored, not
        // the response that was rendered.
        let (read_status, read) = call(&router, "GET", &format!("/graphs/{id}"), None).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: expected 200 from GetGraph, got {read_status}: {read}"
        );
        for member in [
            "id",
            "arn",
            "name",
            "endpoint",
            "createTime",
            "buildNumber",
            "kmsKeyIdentifier",
            "sourceSnapshotId",
            "vectorSearchConfiguration",
            "provisionedMemory",
            "publicConnectivity",
            "deletionProtection",
            "replicaCount",
        ] {
            assert_eq!(
                read[member], restored[member],
                "case {case:?}: GetGraph's {member} must be the stored value {}, got {}",
                restored[member], read[member]
            );
        }

        // The tags supplied by the restore live ON the graph it minted, so the tagging
        // surface reads them back through that graph's own ARN.
        let (tags_status, tags_body) =
            call(&router, "GET", &format!("/tags/{}", encode(&expected_arn)), None).await;
        assert_eq!(
            tags_status, 200,
            "case {case:?}: expected 200 from ListTagsForResource, got {tags_status}: {tags_body}"
        );
        assert_eq!(
            tags_body["tags"],
            json!(tags),
            "case {case:?}: expected the supplied tags {}, got {}",
            json!(tags),
            tags_body["tags"]
        );

        // The clock owes the move to AVAILABLE, and settling changes nothing else.
        let settled = wait_until_available(&router, &format!("/graphs/{id}")).await;
        for member in ["id", "arn", "name", "sourceSnapshotId", "provisionedMemory"] {
            assert_eq!(
                settled[member], restored[member],
                "case {case:?}: settling must not change {member}, expected {}, got {}",
                restored[member], settled[member]
            );
        }
    }
}

#[tokio::test]
async fn a_restore_reproduces_the_snapshotted_graph_not_the_current_one() {
    let router = emulator();
    let source = graph_in_service(
        &router,
        "capture-source",
        &json!({
            "provisionedMemory": 32,
            "publicConnectivity": false,
            "replicaCount": 1,
        }),
    )
    .await;
    let source_id = source["id"].as_str().expect("a source graph id").to_string();
    let snapshot_id = snapshot_in_service(&router, &source_id, "capture-snapshot").await;

    // The source graph is reconfigured AFTER the snapshot was taken, so the two
    // records now disagree about every member this request changes.
    let (update_status, updated) = call(
        &router,
        "PATCH",
        &format!("/graphs/{source_id}"),
        Some(json!({"provisionedMemory": 128, "publicConnectivity": true})),
    )
    .await;
    assert_eq!(
        update_status, 200,
        "the reconfiguration: got {update_status}: {updated}"
    );
    wait_until_available(&router, &format!("/graphs/{source_id}")).await;

    // (case, the name the restore asks for, whether the source graph is deleted first)
    let cases: &[(&str, &str, bool)] = &[
        ("the source graph still exists", "restored-before-delete", false),
        ("the source graph has been deleted", "restored-after-delete", true),
    ];
    for (case, name, delete_source) in cases {
        if *delete_source {
            let (status, deleted) = call(
                &router,
                "DELETE",
                &format!("/graphs/{source_id}?skipSnapshot=true"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from DeleteGraph, got {status}: {deleted}"
            );
        }
        let (status, restored) = restore(&router, &snapshot_id, json!({"graphName": name})).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status}: {restored}"
        );
        // (member, the value the snapshot captured)
        let expected: &[(&str, Value)] = &[
            ("provisionedMemory", json!(32)),
            ("publicConnectivity", json!(false)),
            ("replicaCount", json!(1)),
        ];
        for (member, value) in expected {
            assert_eq!(
                restored[*member], *value,
                "case {case:?}: expected the captured {member} {value}, got {}",
                restored[*member]
            );
        }
    }
}

#[tokio::test]
async fn a_snapshot_whose_creation_is_still_in_flight_cannot_be_restored() {
    let router = emulator();
    let source = graph_in_service(
        &router,
        "in-flight-source",
        &json!({"provisionedMemory": MIN_PROVISIONED_MEMORY}),
    )
    .await;
    let source_id = source["id"].as_str().expect("a source graph id").to_string();

    let (status, created) = call(
        &router,
        "POST",
        "/snapshots",
        Some(json!({"graphIdentifier": source_id, "snapshotName": "in-flight-snapshot"})),
    )
    .await;
    assert_eq!(status, 201, "the snapshot call: got {status}: {created}");
    let snapshot_id = created["id"].as_str().expect("a snapshot id").to_string();
    assert_eq!(
        created["status"],
        json!("CREATING"),
        "case a freshly taken snapshot: expected CREATING, got {}",
        created["status"]
    );

    // The refusal is taken while the snapshot's creation is still outstanding.
    let (refused_status, refused) =
        restore(&router, &snapshot_id, json!({"graphName": "too-early-restore"})).await;
    assert_eq!(
        refused_status, 409,
        "case a snapshot still being created: expected 409, got {refused_status}: {refused}"
    );
    let code = error_code(&refused);
    assert_eq!(
        code, "ConflictException",
        "case a snapshot still being created: expected ConflictException, got {code:?}: {refused}"
    );

    // Once the snapshot is complete the SAME call succeeds, and the name it asked for
    // is still free — so the refusal persisted nothing, not even the name index entry.
    wait_until_available(&router, &format!("/snapshots/{snapshot_id}")).await;
    let (status, accepted) =
        restore(&router, &snapshot_id, json!({"graphName": "too-early-restore"})).await;
    assert_eq!(
        status, 201,
        "case the same call once the snapshot is complete: expected 201, got {status}: {accepted}"
    );
    let id = accepted["id"].as_str().expect("an id").to_string();
    let (read_status, read) = call(&router, "GET", &format!("/graphs/{id}"), None).await;
    assert_eq!(
        read_status, 200,
        "case the restored graph: expected 200 from GetGraph, got {read_status}: {read}"
    );
    assert_eq!(
        read["name"],
        json!("too-early-restore"),
        "case the restored graph: expected the requested name, got {}",
        read["name"]
    );
}

#[tokio::test]
async fn an_identifier_or_name_outside_what_the_store_holds_is_refused() {
    let router = emulator();
    let source = graph_in_service(
        &router,
        "refusal-source",
        &json!({"provisionedMemory": MIN_PROVISIONED_MEMORY}),
    )
    .await;
    let source_id = source["id"].as_str().expect("a source graph id").to_string();
    let snapshot_id = snapshot_in_service(&router, &source_id, "refusal-snapshot").await;

    // (case, the snapshotIdentifier the restore names, status, error code)
    let identifier_cases: Vec<(&str, &str, u16, &str)> = vec![
        ("the stored snapshot", &snapshot_id, 201, ""),
        (
            "a well-formed snapshot id that was never created",
            "gs-zzzzzzzzzz",
            404,
            "ResourceNotFoundException",
        ),
        (
            "another well-formed absent snapshot id",
            "gs-0000000000",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a graph id where a snapshot id belongs",
            "g-abcdefghij",
            400,
            "ValidationException",
        ),
        (
            "an identifier outside the modelled pattern",
            "not-a-snapshot",
            400,
            "ValidationException",
        ),
    ];
    for (index, (case, snapshot_identifier, expected, expected_code)) in
        identifier_cases.into_iter().enumerate()
    {
        let (status, body) = restore(
            &router,
            snapshot_identifier,
            json!({"graphName": format!("identifier-case-{index}")}),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?} ({snapshot_identifier:?}): expected {expected}, got {status}: {body}"
        );
        if !expected_code.is_empty() {
            let code = error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?} ({snapshot_identifier:?}): expected {expected_code}, got \
                 {code:?}: {body}"
            );
        }
    }

    // One row per clause of the modelled `GraphName` constraints (`@length(1..63)`,
    // `@pattern("^(?!g-)[a-z][a-z0-9]*(-[a-z0-9]+)*$")`), plus the length bound itself
    // and one character past it, plus the names already taken in this Region.
    let at_bound = format!("g{}", "x".repeat(MAX_GRAPH_NAME_CHARS - 1));
    let past_bound = format!("g{}", "x".repeat(MAX_GRAPH_NAME_CHARS));
    let name_cases: Vec<(&str, String, u16, &str)> = vec![
        ("a free name", "a-free-name".to_string(), 201, ""),
        (
            "the name of the source graph",
            "refusal-source".to_string(),
            409,
            "ConflictException",
        ),
        (
            "the name of a graph an earlier row restored",
            "a-free-name".to_string(),
            409,
            "ConflictException",
        ),
        ("at the length bound", at_bound, 201, ""),
        (
            "one character past the length bound",
            past_bound,
            400,
            "ValidationException",
        ),
        ("empty", String::new(), 400, "ValidationException"),
        (
            "the reserved graph-id prefix",
            "g-abcdefghij".to_string(),
            400,
            "ValidationException",
        ),
        (
            "the snapshot-id prefix is not reserved here",
            "gs-abcdefghij".to_string(),
            201,
            "",
        ),
        ("a leading digit", "1graph".to_string(), 400, "ValidationException"),
        ("a leading hyphen", "-graph".to_string(), 400, "ValidationException"),
        ("an uppercase letter", "My-Graph".to_string(), 400, "ValidationException"),
        ("an underscore", "my_graph".to_string(), 400, "ValidationException"),
        (
            "two consecutive hyphens",
            "my--graph".to_string(),
            400,
            "ValidationException",
        ),
        ("a trailing hyphen", "my-graph-".to_string(), 400, "ValidationException"),
    ];
    for (case, graph_name, expected, expected_code) in name_cases {
        let (status, body) = restore(&router, &snapshot_id, json!({"graphName": graph_name})).await;
        assert_eq!(
            status, expected,
            "case {case:?} ({graph_name:?}): expected {expected}, got {status}: {body}"
        );
        if !expected_code.is_empty() {
            let code = error_code(&body);
            assert_eq!(
                code, expected_code,
                "case {case:?} ({graph_name:?}): expected {expected_code}, got {code:?}: {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_value_outside_a_modelled_range_is_refused_at_its_bounds() {
    let router = emulator();
    let source = graph_in_service(
        &router,
        "range-source",
        &json!({"provisionedMemory": MIN_PROVISIONED_MEMORY}),
    )
    .await;
    let source_id = source["id"].as_str().expect("a source graph id").to_string();
    let snapshot_id = snapshot_in_service(&router, &source_id, "range-snapshot").await;

    // (case, the member the row varies, its value, the expected status) — each
    // published bound of `ProvisionedMemory` (16..24576) and `ReplicaCount` (0..2),
    // and one value past it.
    let cases: &[(&str, &str, i64, u16)] = &[
        ("memory at the lower bound", "provisionedMemory", MIN_PROVISIONED_MEMORY, 201),
        (
            "memory one below the lower bound",
            "provisionedMemory",
            MIN_PROVISIONED_MEMORY - 1,
            400,
        ),
        ("memory at the upper bound", "provisionedMemory", MAX_PROVISIONED_MEMORY, 201),
        (
            "memory one above the upper bound",
            "provisionedMemory",
            MAX_PROVISIONED_MEMORY + 1,
            400,
        ),
        ("replicas at the lower bound", "replicaCount", 0, 201),
        ("replicas one below the lower bound", "replicaCount", -1, 400),
        ("replicas at the upper bound", "replicaCount", MAX_REPLICA_COUNT, 201),
        (
            "replicas one above the upper bound",
            "replicaCount",
            MAX_REPLICA_COUNT + 1,
            400,
        ),
    ];
    for (index, (case, member, value, expected)) in cases.iter().enumerate() {
        let mut body = json!({"graphName": format!("range-case-{index}")});
        body[*member] = json!(value);
        let (status, response) = restore(&router, &snapshot_id, body).await;
        assert_eq!(
            status, *expected,
            "case {case:?} ({member}={value}): expected {expected}, got {status}: {response}"
        );
        if *expected == 400 {
            let code = error_code(&response);
            assert_eq!(
                code, "ValidationException",
                "case {case:?} ({member}={value}): expected ValidationException, got \
                 {code:?}: {response}"
            );
        } else {
            assert_eq!(
                response[*member],
                json!(value),
                "case {case:?}: expected the overridden {member} {value}, got {}",
                response[*member]
            );
        }
    }
}
