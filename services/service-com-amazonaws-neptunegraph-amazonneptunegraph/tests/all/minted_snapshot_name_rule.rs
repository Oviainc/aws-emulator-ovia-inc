// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the rule that a name this service MINTS for a resource is held
//! to the same modelled constraints it enforces on a name a caller sends for that same
//! resource, driven over the wire (`restJson1`) through the same router the emulator
//! serves from.
//!
//! The one name this service mints is the `snapshotName` of a FINAL snapshot: the
//! request that asks for one (`ResetGraph` or `DeleteGraph` with `skipSnapshot=false`)
//! carries no name, so the service names it after the graph. `SnapshotName` is modelled
//! `@length(1..63)` with pattern `^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`
//! (`smithy_query(query="shape",shape="SnapshotName")`) and `GraphName` reserves a
//! DIFFERENT prefix (`^(?!g-)…`), so there are graph names a snapshot may not be named
//! after — and the minted name has to clear the pattern, not only the length.
//!
//! The rows below differ along what the rule generalises over: whether the graph's own
//! name opens with the prefix snapshot names reserve, whether it merely opens with the
//! same letters, whether it opens with the prefix GRAPH ids reserve, and whether it
//! leaves room for the suffix at all. Both operations that mint such a name are driven
//! over each row, because both reach the one naming rule.
//!
//! Every minted name is then read back over the wire three ways: through
//! `GetGraphSnapshot`, through `ListGraphSnapshots`, and through the sibling path that
//! ACCEPTS a name — `CreateGraphSnapshot`, which must refuse the minted name only as a
//! name already taken (409) and never as a name its pattern forbids (400).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    FINAL_SNAPSHOT_STEM, FINAL_SNAPSHOT_SUFFIX, MAX_RESOURCE_NAME_CHARS, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// One row of the naming table: the graph a final snapshot is taken of, and whether the
/// snapshot may be named after that graph.
struct NameCase<'a> {
    /// What this row is about, named in every assertion of it.
    case: &'a str,
    /// The graph `ResetGraph` is driven over.
    reset_name: String,
    /// The graph `DeleteGraph` is driven over, of the same shape as `reset_name`.
    delete_name: String,
    /// Whether the minted name may be built from the graph's own name — `false` when the
    /// graph's name yields no value the modelled `SnapshotName` accepts.
    after_the_graph: bool,
}

/// An emulator holding only this service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
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

/// The id a record reported.
fn id_of(record: &Value) -> String {
    record["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no id in {record}"))
        .to_string()
}

/// Whether `name` matches the modelled `SnapshotName` pattern
/// `^(?!gs-)[a-z][a-z0-9]*(-[a-z0-9]+)*$`
/// (`smithy_query(query="shape",shape="SnapshotName")`), read literally and spelled out
/// here rather than borrowed from the crate, so this test judges the emulator's answers
/// against the model and not against the emulator's own reading of it.
fn matches_snapshot_name_pattern(name: &str) -> bool {
    if name.starts_with("gs-") {
        return false;
    }
    let mut chars = name.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_lowercase()) {
        return false;
    }
    let mut after_hyphen = false;
    for c in chars {
        if c == '-' {
            if after_hyphen {
                return false;
            }
            after_hyphen = true;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            after_hyphen = false;
        } else {
            return false;
        }
    }
    !after_hyphen
}

/// Create a graph named `name`, asserting `CreateGraph` accepts it, and return its id.
async fn create_graph(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let body = json!({
        "graphName": name,
        "provisionedMemory": 16,
        "replicaCount": 0,
        "publicConnectivity": false,
        "deletionProtection": false,
    });
    let (status, created) = call(router, "POST", "/graphs", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateGraph of {name:?}, got {status}: {created}"
    );
    id_of(&created)
}

/// Poll one resource until it reports `expected`. No handler ever sleeps; only this test
/// waits, because it is asking about a status the clock reaches on its own.
async fn wait_for_status(router: &MultiServiceRouter, case: &str, path: &str, expected: &str) {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the read of {path}, got {status}: {body}"
        );
        if body["status"] == json!(expected) {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {case:?}: {path} never reported {expected}, last status {}",
        body["status"]
    );
}

/// The snapshot ids `ListGraphSnapshots` reports, paired with the name each carries.
async fn listed_snapshots(router: &MultiServiceRouter, case: &str) -> Vec<(String, String)> {
    let (status, body) = call(router, "GET", "/snapshots", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListGraphSnapshots, got {status}: {body}"
    );
    body["graphSnapshots"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no graphSnapshots in {body}"))
        .iter()
        .map(|summary| {
            (
                id_of(summary),
                summary["name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no name in {summary}"))
                    .to_string(),
            )
        })
        .collect()
}

/// The snapshot ids `ListGraphSnapshots` reports, in the order it reports them.
async fn listed_snapshot_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    listed_snapshots(router, case)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// The one snapshot id that appeared in the listing between `before` and `after`.
fn one_minted(case: &str, operation: &str, before: &[String], after: &[String]) -> String {
    let minted: Vec<&String> = after.iter().filter(|id| !before.contains(id)).collect();
    assert_eq!(
        minted.len(),
        1,
        "case {case:?}: expected {operation} to mint exactly one final snapshot, got {minted:?}"
    );
    minted[0].clone()
}

#[tokio::test]
async fn a_minted_final_snapshot_name_satisfies_the_name_constraints_the_service_enforces() {
    let router = emulator();

    // `GraphName` forbids only the `g-` prefix, so each name below is one `CreateGraph`
    // accepts; the rows differ in what the snapshot-naming rule has to make of them.
    let rows: Vec<NameCase<'_>> = vec![
        NameCase {
            case: "a plain graph name",
            reset_name: "plain-graph-one".to_string(),
            delete_name: "plain-graph-two".to_string(),
            after_the_graph: true,
        },
        NameCase {
            case: "a graph name opening with the prefix snapshot names reserve",
            reset_name: "gs-data".to_string(),
            delete_name: "gs-more-data".to_string(),
            after_the_graph: false,
        },
        NameCase {
            case: "a graph name that is the reserved prefix without its hyphen",
            reset_name: "gs".to_string(),
            delete_name: "gs-s".to_string(),
            after_the_graph: false,
        },
        NameCase {
            case: "a graph name opening with the reserved letters but not the prefix",
            reset_name: "gsx-data".to_string(),
            delete_name: "gsy-data".to_string(),
            after_the_graph: true,
        },
        NameCase {
            case: "a graph name opening with the prefix GRAPH ids reserve, hyphenless",
            reset_name: "g0-x".to_string(),
            delete_name: "g1-y".to_string(),
            after_the_graph: true,
        },
        NameCase {
            case: "a graph name that leaves no room for the suffix",
            reset_name: "a".repeat(MAX_RESOURCE_NAME_CHARS),
            delete_name: "b".repeat(MAX_RESOURCE_NAME_CHARS),
            after_the_graph: false,
        },
    ];

    // Every graph of every row is created first, so the one provisioning span the clock
    // owes them runs for all of them at once rather than once per row.
    let mut graphs: Vec<(usize, String, String)> = Vec::new();
    for (row_index, row) in rows.iter().enumerate() {
        for name in [&row.reset_name, &row.delete_name] {
            let id = create_graph(&router, row.case, name).await;
            graphs.push((row_index, name.clone(), id));
        }
    }
    // One extra graph, never reset or deleted, to offer `CreateGraphSnapshot` a source.
    let probe_graph = create_graph(&router, "the probe graph", "probe-graph").await;
    for (row_index, _, id) in &graphs {
        wait_for_status(
            &router,
            rows[*row_index].case,
            &format!("/graphs/{id}"),
            "AVAILABLE",
        )
        .await;
    }
    wait_for_status(
        &router,
        "the probe graph",
        &format!("/graphs/{probe_graph}"),
        "AVAILABLE",
    )
    .await;

    for (row_index, row) in rows.iter().enumerate() {
        let case = row.case;
        let reset_graph = graphs
            .iter()
            .find(|(index, name, _)| *index == row_index && name == &row.reset_name)
            .map(|(_, _, id)| id.clone())
            .unwrap_or_else(|| panic!("case {case:?}: the reset graph was created"));
        let delete_graph = graphs
            .iter()
            .find(|(index, name, _)| *index == row_index && name == &row.delete_name)
            .map(|(_, _, id)| id.clone())
            .unwrap_or_else(|| panic!("case {case:?}: the delete graph was created"));

        // Both operations that mint a final-snapshot name are driven over this row.
        let before: Vec<String> = listed_snapshot_ids(&router, case).await;
        let (reset_status, reset) = call(
            &router,
            "PUT",
            &format!("/graphs/{reset_graph}"),
            Some(json!({"skipSnapshot": false})),
        )
        .await;
        assert_eq!(
            reset_status, 200,
            "case {case:?}: expected 200 from ResetGraph, got {reset_status}: {reset}"
        );
        let after_reset: Vec<String> = listed_snapshot_ids(&router, case).await;
        let from_reset = one_minted(case, "ResetGraph", &before, &after_reset);

        let (delete_status, deleted) = call(
            &router,
            "DELETE",
            &format!("/graphs/{delete_graph}?skipSnapshot=false"),
            None,
        )
        .await;
        assert_eq!(
            delete_status, 200,
            "case {case:?}: expected 200 from DeleteGraph, got {delete_status}: {deleted}"
        );
        let after_delete: Vec<String> = listed_snapshot_ids(&router, case).await;
        let from_delete = one_minted(case, "DeleteGraph", &after_reset, &after_delete);

        for (operation, graph_name, snapshot_id) in [
            ("ResetGraph", &row.reset_name, &from_reset),
            ("DeleteGraph", &row.delete_name, &from_delete),
        ] {
            let (status, snapshot) =
                call(&router, "GET", &format!("/snapshots/{snapshot_id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from GetGraphSnapshot of {operation}'s \
                 snapshot, got {status}: {snapshot}"
            );
            let minted = snapshot["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no name in {snapshot}"))
                .to_string();

            // The constraint the model puts on the member this value is reported in.
            assert!(
                matches_snapshot_name_pattern(&minted),
                "case {case:?}: expected {operation}'s minted name to match the modelled \
                 SnapshotName pattern, got {minted:?}"
            );
            let minted_chars = minted.chars().count();
            assert!(
                (1..=MAX_RESOURCE_NAME_CHARS).contains(&minted_chars),
                "case {case:?}: expected {operation}'s minted name within \
                 1..{MAX_RESOURCE_NAME_CHARS} characters, got {minted_chars} in {minted:?}"
            );

            // Named after the graph exactly where that yields such a value, and after the
            // snapshot's own id where it does not.
            let after_the_graph = format!("{graph_name}{FINAL_SNAPSHOT_SUFFIX}");
            let expected = if row.after_the_graph {
                after_the_graph.clone()
            } else {
                format!("{FINAL_SNAPSHOT_STEM}-{snapshot_id}")
            };
            assert_eq!(
                minted, expected,
                "case {case:?}: expected {operation}'s minted name {expected:?}, got {minted:?}"
            );

            // Every reader of that stored name reports the same value.
            let listed = listed_snapshots(&router, case)
                .await
                .into_iter()
                .find(|(id, _)| id == snapshot_id)
                .map(|(_, name)| name);
            assert_eq!(
                listed.as_deref(),
                Some(minted.as_str()),
                "case {case:?}: expected ListGraphSnapshots to report {operation}'s \
                 minted name {minted:?}, got {listed:?}"
            );

            // The sibling path that ACCEPTS a `snapshotName` must agree that the minted
            // name belongs to this namespace: the only thing wrong with it is that the
            // minted snapshot already holds it.
            let (accepted_status, refusal) = call(
                &router,
                "POST",
                "/snapshots",
                Some(json!({"graphIdentifier": probe_graph, "snapshotName": minted})),
            )
            .await;
            assert_eq!(
                accepted_status, 409,
                "case {case:?}: expected CreateGraphSnapshot to refuse {operation}'s \
                 minted name {minted:?} only as a name already taken, got \
                 {accepted_status}: {refusal}"
            );
            let code = error_code(&refusal);
            assert_eq!(
                code, "ConflictException",
                "case {case:?}: expected ConflictException for the taken name {minted:?}, \
                 got {code}: {refusal}"
            );
        }
    }

    // The control the probe above rests on: a name that DOES open with the reserved
    // prefix is refused by `CreateGraphSnapshot` as a pattern violation, which is exactly
    // the answer no minted name may draw.
    for forbidden in [
        format!("gs-data{FINAL_SNAPSHOT_SUFFIX}"),
        "gs-x".to_string(),
    ] {
        let (status, refusal) = call(
            &router,
            "POST",
            "/snapshots",
            Some(json!({"graphIdentifier": probe_graph, "snapshotName": forbidden})),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {forbidden:?}: expected 400 from CreateGraphSnapshot, got {status}: {refusal}"
        );
        let code = error_code(&refusal);
        assert_eq!(
            code, "ValidationException",
            "case {forbidden:?}: expected ValidationException, got {code}: {refusal}"
        );
    }
}
