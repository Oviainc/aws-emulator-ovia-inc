// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteGraphSnapshot`, driven over the wire (`restJson1`)
//! through the same router the emulator serves from.
//!
//! The rules every assertion here is about:
//!
//! * A delete of a snapshot at rest is ACCEPTED, not performed: the call answers 200
//!   with the stored record reporting the modelled `DELETING` status, and the snapshot
//!   stays a resource the service holds — readable, listed, and holding the name it
//!   reserves — until the removal the clock owes falls due, after which it is gone, its
//!   name is free, and every call that addresses it answers `ResourceNotFoundException`.
//! * A snapshot whose transition is still outstanding (`CREATING`, or a `DELETING` one
//!   whose removal has not fallen due) cannot be deleted or restored: the call is
//!   refused `ConflictException` (409) naming the status that held it back, and the
//!   stored record is left exactly as it was.
//! * A `snapshotIdentifier` that names no stored snapshot of the request's Region is a
//!   failed lookup (404), not an empty success.
//! * Deleting a snapshot is about the snapshot: the graph it was taken from is
//!   untouched.
//!
//! So each table below spans what its rule generalises over — how the snapshot was
//! minted (named by `CreateGraphSnapshot` or derived as `DeleteGraph`'s final snapshot),
//! the name's form and length, the tag entry count, whether the source graph is
//! encrypted, and each status a snapshot can be addressed in — rather than re-running
//! one recorded request.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    MAX_RESOURCE_NAME_CHARS, PROVISIONING_SPAN, SNAPSHOT_DELETION_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// Room for the emulator's own scheduling on top of a span the clock owes, so a wait for
/// a due transition is not a race.
const SETTLE_MARGIN: Duration = Duration::from_millis(150);

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

/// `GET /snapshots/{id}`.
async fn get_snapshot(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/snapshots/{id}"), None).await
}

/// `DELETE /snapshots/{id}`.
async fn delete_snapshot(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("/snapshots/{id}"), None).await
}

/// `POST /snapshots` with `body`.
async fn create_snapshot(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/snapshots", Some(body)).await
}

/// The snapshot ids `ListGraphSnapshots` reports, in the order it pages them.
async fn listed_snapshot_ids(router: &MultiServiceRouter) -> Vec<String> {
    let (status, listed) = call(router, "GET", "/snapshots", None).await;
    assert_eq!(
        status, 200,
        "ListGraphSnapshots: expected 200, got {status}: {listed}"
    );
    listed["graphSnapshots"]
        .as_array()
        .expect("graphSnapshots is a list")
        .iter()
        .map(|summary| summary["id"].as_str().unwrap_or_default().to_string())
        .collect()
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

/// Poll one resource until it reports `expected`, then return its record. No handler
/// ever sleeps; only this test waits, because it is asking about a status the clock
/// reaches on its own.
async fn wait_for_status(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    expected: &str,
) -> Value {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read of {path}, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return body;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {case:?}: {path} never reported {expected}, last status {}",
        body["status"]
    );
}

/// Create a graph (optionally encrypted) and wait it into service, returning its id.
async fn graph_in_service(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    kms_key: Option<&str>,
) -> String {
    let mut body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
    if let Some(key) = kms_key {
        body["kmsKeyIdentifier"] = json!(key);
    }
    let (status, created) = call(router, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph of {body}, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no id in {created}"))
        .to_string();
    wait_for_status(router, case, &format!("/graphs/{id}"), "AVAILABLE").await;
    id
}

/// One row of the delete table: the case name, the `snapshotName` the create carries
/// (`None` asks for `DeleteGraph`'s FINAL snapshot instead, which the service names
/// itself), the tags the create carries, and the KMS key the SOURCE GRAPH was created
/// with (`None` for a graph encrypted with the service-owned key).
type DeleteCase = (
    &'static str,
    Option<String>,
    BTreeMap<String, String>,
    Option<&'static str>,
);

#[tokio::test]
async fn an_accepted_delete_reports_deleting_then_reclaims_the_snapshot() {
    let router = emulator();

    // The graph the name-reservation probes below take their snapshots from, so a probe
    // is a real `CreateGraphSnapshot` of the account rather than a write into the store.
    let donor = graph_in_service(&router, "donor", "name-probe-donor", None).await;

    // The rows differ along what the rule generalises over: how the snapshot was minted
    // (named by `CreateGraphSnapshot`, or derived by `DeleteGraph` as a final snapshot),
    // the name's form and length at the published bound, the tag entry count, and
    // whether the source graph is encrypted.
    let longest_name = format!("s{}", "x".repeat(MAX_RESOURCE_NAME_CHARS - 1));
    let cases: Vec<DeleteCase> = vec![
        (
            "a named snapshot, no tags, an unencrypted graph",
            Some("my-snapshot-1".to_string()),
            BTreeMap::new(),
            None,
        ),
        (
            "a single-character name, one tag",
            Some("a".to_string()),
            [("env".to_string(), "dev".to_string())]
                .into_iter()
                .collect(),
            None,
        ),
        (
            "a name at the published length bound, several tags, an encrypted graph",
            Some(longest_name),
            [
                ("env".to_string(), "prod".to_string()),
                ("team".to_string(), "graph".to_string()),
                ("cost-centre".to_string(), String::new()),
            ]
            .into_iter()
            .collect(),
            Some(KMS_KEY_ARN),
        ),
        (
            "a final snapshot the service named itself",
            None,
            BTreeMap::new(),
            None,
        ),
    ];

    for (index, (case, snapshot_name, tags, kms_key)) in cases.into_iter().enumerate() {
        let source_graph =
            graph_in_service(&router, case, &format!("delete-source-{index}"), kms_key).await;

        // Whichever way it was minted, the snapshot is deleted once it rests AVAILABLE.
        let snapshot_id = match snapshot_name.as_deref() {
            Some(name) => {
                let (status, created) = create_snapshot(
                    &router,
                    json!({
                        "graphIdentifier": source_graph,
                        "snapshotName": name,
                        "tags": tags,
                    }),
                )
                .await;
                assert_eq!(
                    status, 201,
                    "case {case:?}: expected 201 from CreateGraphSnapshot, got {status}: {created}"
                );
                created["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no id in {created}"))
                    .to_string()
            }
            None => {
                let before = listed_snapshot_ids(&router).await;
                let (status, deleted) = call(
                    &router,
                    "DELETE",
                    &format!("/graphs/{source_graph}?skipSnapshot=false"),
                    None,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from DeleteGraph, got {status}: {deleted}"
                );
                let minted: Vec<String> = listed_snapshot_ids(&router)
                    .await
                    .into_iter()
                    .filter(|id| !before.contains(id))
                    .collect();
                assert_eq!(
                    minted.len(),
                    1,
                    "case {case:?}: expected exactly one final snapshot, got {minted:?}"
                );
                minted[0].clone()
            }
        };
        let at_rest = wait_for_status(
            &router,
            case,
            &format!("/snapshots/{snapshot_id}"),
            "AVAILABLE",
        )
        .await;
        let reserved_name = at_rest["name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no name in {at_rest}"))
            .to_string();

        // The delete is ACCEPTED: it answers the stored record on its way out, with
        // every member but the status the one a read of it answered.
        let (status, accepted) = delete_snapshot(&router, &snapshot_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {accepted}"
        );
        assert_eq!(
            accepted["status"],
            json!("DELETING"),
            "case {case:?}: a snapshot is not gone when the call returns, expected \
             DELETING, got {}",
            accepted["status"]
        );
        for member in [
            "id",
            "arn",
            "name",
            "sourceGraphId",
            "snapshotCreateTime",
            "kmsKeyIdentifier",
        ] {
            assert_eq!(
                accepted[member], at_rest[member],
                "case {case:?}: the delete must report the stored {member} {}, got {}",
                at_rest[member], accepted[member]
            );
        }

        // While the removal is in flight the snapshot is still a resource this service
        // holds: it reads back as DELETING, it is still listed, it still carries its
        // tags, its name is still reserved, and a second delete conflicts with the one
        // already running.
        let (read_status, in_flight) = get_snapshot(&router, &snapshot_id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: a deleting snapshot must still be readable, got \
             {read_status}: {in_flight}"
        );
        assert_eq!(
            in_flight["status"],
            json!("DELETING"),
            "case {case:?}: expected DELETING from GetGraphSnapshot, got {}",
            in_flight["status"]
        );
        assert!(
            listed_snapshot_ids(&router).await.contains(&snapshot_id),
            "case {case:?}: a deleting snapshot must still be listed"
        );
        let arn = at_rest["arn"].as_str().unwrap_or_default().to_string();
        let (tags_status, tags_body) =
            call(&router, "GET", &format!("/tags/{}", encode(&arn)), None).await;
        assert_eq!(
            tags_status, 200,
            "case {case:?}: a deleting snapshot must still answer its tags, got \
             {tags_status}: {tags_body}"
        );
        assert_eq!(
            tags_body["tags"],
            json!(tags),
            "case {case:?}: expected the stored tags {}, got {}",
            json!(tags),
            tags_body["tags"]
        );
        wait_for_status(&router, case, &format!("/graphs/{donor}"), "AVAILABLE").await;
        let (reserved_status, reserved) = create_snapshot(
            &router,
            json!({"graphIdentifier": donor, "snapshotName": reserved_name}),
        )
        .await;
        assert_eq!(
            reserved_status, 409,
            "case {case:?}: the name must stay reserved while the snapshot is DELETING, \
             got {reserved_status}: {reserved}"
        );
        let (again_status, again) = delete_snapshot(&router, &snapshot_id).await;
        assert_eq!(
            again_status, 409,
            "case {case:?}: a second delete of a deleting snapshot must conflict, got \
             {again_status}: {again}"
        );
        assert_eq!(
            error_code(&again),
            "ConflictException",
            "case {case:?}: unexpected error code in {again}"
        );
        assert_eq!(
            message(&again),
            "Unable to delete the graph snapshot because it is in the DELETING state.",
            "case {case:?}: unexpected refusal wording in {again}"
        );

        // Once the removal the clock owes falls due the snapshot is gone, its name is
        // free again, and every call that addresses it is a 404.
        tokio::time::sleep(SNAPSHOT_DELETION_SPAN + SETTLE_MARGIN).await;
        let (gone_status, gone) = get_snapshot(&router, &snapshot_id).await;
        assert_eq!(
            gone_status, 404,
            "case {case:?}: expected 404 once the removal is due, got {gone_status}: {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {gone}"
        );
        assert!(
            !listed_snapshot_ids(&router).await.contains(&snapshot_id),
            "case {case:?}: a reclaimed snapshot must not be listed"
        );
        let (gone_delete_status, gone_delete) = delete_snapshot(&router, &snapshot_id).await;
        assert_eq!(
            gone_delete_status, 404,
            "case {case:?}: expected 404 from a delete of a snapshot that is gone, got \
             {gone_delete_status}: {gone_delete}"
        );
        let (gone_tags_status, gone_tags) =
            call(&router, "GET", &format!("/tags/{}", encode(&arn)), None).await;
        assert_eq!(
            gone_tags_status, 404,
            "case {case:?}: a reclaimed snapshot's ARN must name nothing, got \
             {gone_tags_status}: {gone_tags}"
        );
        wait_for_status(&router, case, &format!("/graphs/{donor}"), "AVAILABLE").await;
        let (freed_status, freed) = create_snapshot(
            &router,
            json!({"graphIdentifier": donor, "snapshotName": reserved_name}),
        )
        .await;
        assert_eq!(
            freed_status, 201,
            "case {case:?}: the name must be free once the removal is done, got \
             {freed_status}: {freed}"
        );

        // Deleting a snapshot is about the snapshot: a source graph that still exists is
        // untouched by it.
        if snapshot_name.is_some() {
            let (graph_status, graph) =
                call(&router, "GET", &format!("/graphs/{source_graph}"), None).await;
            assert_eq!(
                graph_status, 200,
                "case {case:?}: the source graph must survive its snapshot's deletion, \
                 got {graph_status}: {graph}"
            );
            assert_eq!(
                graph["id"],
                json!(source_graph.clone()),
                "case {case:?}: expected the source graph's own record, got {graph}"
            );
        }
    }
}

#[tokio::test]
async fn a_snapshot_whose_transition_is_in_flight_cannot_be_deleted() {
    let router = emulator();
    let graph = graph_in_service(&router, "in flight", "in-flight-source", None).await;

    // (case, the status the snapshot is addressed in). The rows span every status a
    // snapshot can be addressed in that is not AVAILABLE, and each one names its own
    // status in the refusal.
    let cases: &[(&str, &str)] = &[
        ("the capture is still in flight", "CREATING"),
        ("a removal is already in flight", "DELETING"),
    ];

    for (index, (case, status_in)) in cases.iter().enumerate() {
        let (status, created) = create_snapshot(
            &router,
            json!({
                "graphIdentifier": graph,
                "snapshotName": format!("in-flight-{index}"),
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateGraphSnapshot, got {status}: {created}"
        );
        let id = created["id"].as_str().expect("an id").to_string();
        if *status_in == "DELETING" {
            wait_for_status(&router, case, &format!("/snapshots/{id}"), "AVAILABLE").await;
            let (accepted_status, accepted) = delete_snapshot(&router, &id).await;
            assert_eq!(
                accepted_status, 200,
                "case {case:?}: expected 200 from the first delete, got \
                 {accepted_status}: {accepted}"
            );
        }

        let (refused_status, refused) = delete_snapshot(&router, &id).await;
        assert_eq!(
            refused_status, 409,
            "case {case:?}: expected 409, got {refused_status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: unexpected error code in {refused}"
        );
        assert_eq!(
            message(&refused),
            format!("Unable to delete the graph snapshot because it is in the {status_in} state."),
            "case {case:?}: unexpected refusal wording in {refused}"
        );

        // The refusal persisted nothing: the snapshot is still the record it was, and
        // the status it reports is still the one that held the delete back.
        let (read_status, read) = get_snapshot(&router, &id).await;
        assert_eq!(
            read_status, 200,
            "case {case:?}: the refused snapshot must still be readable, got \
             {read_status}: {read}"
        );
        assert_eq!(
            read["id"],
            json!(id.clone()),
            "case {case:?}: expected the snapshot's own record, got {read}"
        );

        // A CREATING snapshot becomes deletable once the capture the clock owes falls
        // due — so the refusal was about its status and nothing else.
        if *status_in == "CREATING" {
            wait_for_status(&router, case, &format!("/snapshots/{id}"), "AVAILABLE").await;
            let (accepted_status, accepted) = delete_snapshot(&router, &id).await;
            assert_eq!(
                accepted_status, 200,
                "case {case:?}: expected 200 once the snapshot rests AVAILABLE, got \
                 {accepted_status}: {accepted}"
            );
            assert_eq!(
                accepted["status"],
                json!("DELETING"),
                "case {case:?}: expected DELETING from the accepted delete, got {}",
                accepted["status"]
            );
        }
        wait_for_status(&router, case, &format!("/graphs/{graph}"), "AVAILABLE").await;
    }
}

#[tokio::test]
async fn an_identifier_that_names_no_stored_snapshot_is_refused() {
    let router = emulator();
    let graph = graph_in_service(&router, "identifier", "identifier-source", None).await;
    let (status, created) = create_snapshot(
        &router,
        json!({"graphIdentifier": graph, "snapshotName": "identifier-snapshot"}),
    )
    .await;
    assert_eq!(status, 201, "the fixture snapshot: got {status}: {created}");
    let stored = created["id"].as_str().expect("an id").to_string();
    wait_for_status(
        &router,
        "identifier",
        &format!("/snapshots/{stored}"),
        "AVAILABLE",
    )
    .await;

    // (case, the snapshotIdentifier the delete is given, the status, the error code).
    // The rows span what the identifier can be: a stored snapshot, a well-formed id the
    // account never minted, and values outside the modelled `^gs-[a-z0-9]{10}$` pattern.
    let cases: Vec<(&str, &str, u16, &str)> = vec![
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
            "not-a-snapshot-id",
            400,
            "ValidationException",
        ),
        ("the stored snapshot", stored.as_str(), 200, ""),
    ];

    for (case, identifier, expected, expected_code) in cases {
        let (status, body) = delete_snapshot(&router, identifier).await;
        assert_eq!(
            status, expected,
            "case {case:?} ({identifier:?}): expected {expected}, got {status}: {body}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?} ({identifier:?}): unexpected error code in {body}"
            );
            // A refused delete leaves the stored snapshot alone.
            let (read_status, read) = get_snapshot(&router, &stored).await;
            assert_eq!(
                read_status, 200,
                "case {case:?}: the stored snapshot must survive the refusal, got \
                 {read_status}: {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_snapshot_on_its_way_out_cannot_be_restored() {
    let router = emulator();
    let graph = graph_in_service(&router, "restore", "restore-source", None).await;
    let (status, created) = create_snapshot(
        &router,
        json!({"graphIdentifier": graph, "snapshotName": "restore-snapshot"}),
    )
    .await;
    assert_eq!(status, 201, "the fixture snapshot: got {status}: {created}");
    let id = created["id"].as_str().expect("an id").to_string();
    wait_for_status(&router, "restore", &format!("/snapshots/{id}"), "AVAILABLE").await;
    let (accepted_status, accepted) = delete_snapshot(&router, &id).await;
    assert_eq!(
        accepted_status, 200,
        "the accepted delete: got {accepted_status}: {accepted}"
    );

    // While the removal is in flight the snapshot holds no restorable copy any more, so
    // the restore is the conflict the snapshot's status makes it.
    let (refused_status, refused) = call(
        &router,
        "POST",
        &format!("/snapshots/{id}/restore"),
        Some(json!({"graphName": "restored-from-deleting"})),
    )
    .await;
    assert_eq!(
        refused_status, 409,
        "case a restore of a deleting snapshot: expected 409, got {refused_status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ConflictException",
        "case a restore of a deleting snapshot: unexpected error code in {refused}"
    );
    assert_eq!(
        message(&refused),
        "Unable to restore the graph snapshot because it is in the DELETING state.",
        "case a restore of a deleting snapshot: unexpected refusal wording in {refused}"
    );

    // Once the removal falls due the snapshot names nothing at all, so the same restore
    // is a failed lookup rather than a conflict.
    tokio::time::sleep(SNAPSHOT_DELETION_SPAN + SETTLE_MARGIN).await;
    let (gone_status, gone) = call(
        &router,
        "POST",
        &format!("/snapshots/{id}/restore"),
        Some(json!({"graphName": "restored-from-reclaimed"})),
    )
    .await;
    assert_eq!(
        gone_status, 404,
        "case a restore of a reclaimed snapshot: expected 404, got {gone_status}: {gone}"
    );
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "case a restore of a reclaimed snapshot: unexpected error code in {gone}"
    );
}

/// Percent-encode one path segment.
///
/// `resourceArn` is bound to a NON-greedy `{resourceArn}` label (`/tags/{resourceArn}`),
/// so the `/` inside an ARN has to arrive encoded or it would split the URI into another
/// path segment. Only the RFC 3986 unreserved set is left as-is.
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
