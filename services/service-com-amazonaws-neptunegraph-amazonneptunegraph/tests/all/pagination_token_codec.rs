// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE pagination-token codec every listing of this service
//! pages with, driven over the wire (`restJson1`) through the same router the emulator
//! serves from.
//!
//! The rule every assertion here is about: this service has a SINGLE pagination-token
//! format, so a client that pages one listing and then another sees one token grammar.
//! A token is the hex encoding of the listing that minted it and the key of the last
//! entry that page reported; a token is accepted only by that listing, and only when it
//! decodes to a position in the id space that listing orders (so one listing's token
//! never pages another's); and a token naming a position no stored entry occupies still
//! resumes from that position, because a page resumes after a POSITION rather than after
//! a row.
//!
//! The table spans what the rule generalises over: every listing the service
//! implements — `ListGraphs` (graph ids), `ListGraphSnapshots` (snapshot ids),
//! `ListPrivateGraphEndpoints` (VPC ids) and `ListExportTasks` (task ids) — each
//! against several page sizes, each OTHER listing's id space as a foreign token, and a
//! token that is not hex at all.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::PROVISIONING_SPAN;
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region these tests sign for.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// these tests need from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// How many entries each listing is populated with: enough that a page size below it
/// forces a token, and a page size equal to it forces none.
const POPULATION: usize = 3;

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneExportRole";

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

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this
/// test does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, path: &str) {
    for _ in 0..40 {
        let (status, body) = call(router, REGION, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {path}: expected 200 from the read, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, REGION, "GET", path, None).await;
    panic!(
        "case {path}: never became AVAILABLE, last status {}",
        body["status"]
    );
}

/// Create a graph and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
    let (status, created) = call(router, REGION, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from CreateGraph, got {status}: {created}"
    );
    let id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: no id in {created}"))
        .to_string();
    wait_until_available(router, &format!("/graphs/{id}")).await;
    id
}

/// Snapshot `graph_id` under `name`, waiting the source graph back into service so the
/// next call is about the stored row and not about a transition this one left pending.
async fn snapshot(router: &MultiServiceRouter, graph_id: &str, name: &str) {
    let (status, created) = call(
        router,
        REGION,
        "POST",
        "/snapshots",
        Some(json!({"graphIdentifier": graph_id, "snapshotName": name})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case snapshot {name:?} of {graph_id}: expected 201, got {status}: {created}"
    );
    wait_until_available(router, &format!("/graphs/{graph_id}")).await;
}

/// Create one private endpoint of `graph_id` in `vpc_id` and wait it into service.
async fn endpoint_in_service(router: &MultiServiceRouter, graph_id: &str, vpc_id: &str) {
    // `SubnetId`'s modelled pattern is `^subnet-[a-z0-9]+$` and `SubnetIds` is
    // `@length(1..6)` (`smithy_query(query="shape",shape="SubnetIds")`), so one subnet
    // is a well-formed request.
    let body = json!({"vpcId": vpc_id, "subnetIds": ["subnet-0a1b2c3d4e5f00000"]});
    let (status, created) = call(
        router,
        REGION,
        "POST",
        &format!("/graphs/{graph_id}/endpoints/"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {graph_id}/{vpc_id}: expected 201 from the create, got {status}: {created}"
    );
    wait_until_available(router, &format!("/graphs/{graph_id}/endpoints/{vpc_id}")).await;
}

/// Start one plain CSV export of `graph_id`, which this suite needs only as a row of
/// the export-task listing.
async fn export_task(router: &MultiServiceRouter, graph_id: &str, destination: &str) {
    let body = json!({
        "graphIdentifier": graph_id,
        "roleArn": ROLE_ARN,
        "format": "CSV",
        "destination": destination,
        "kmsKeyIdentifier": KMS_KEY_ARN,
    });
    let (status, started) = call(router, REGION, "POST", "/exporttasks", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from StartExportTask, got {status}: {started}"
    );
}

/// One listing of this service, in the terms the shared token codec is stated in: where
/// it is served, which member carries its entries, which member of an entry the token
/// names a position in, and which values are and are not positions in that id space.
struct Listing {
    operation: &'static str,
    path: String,
    entries_member: &'static str,
    key_member: &'static str,
    /// A position in THIS listing's id space to resume from, chosen to sort at or
    /// before every stored entry. Whether an entry occupies it is not assumed: the
    /// assertion is that the page reports exactly the entries sorting after it.
    probe_position: &'static str,
    /// Tokens that name no position in this listing's id space: a position in each
    /// other id space of the service, and one that is not even hex.
    foreign_tokens: Vec<String>,
}

/// The token this service issues for `position` in the listing `operation` pages: the
/// listing that minted it and the position it reached, hex-encoded.
fn issued_token(operation: &str, position: &str) -> String {
    hex::encode(format!("{operation}:{position}"))
}

/// `GET` the listing with the query string `query` (already encoded, without the `?`).
async fn list(router: &MultiServiceRouter, listing: &Listing, query: &str) -> (u16, Value) {
    let path = if query.is_empty() {
        listing.path.clone()
    } else {
        format!("{}?{query}", listing.path)
    };
    call(router, REGION, "GET", &path, None).await
}

/// The key of every entry of a successful listing, in the order they were reported.
fn keys(case: &str, listing: &Listing, status: u16, body: &Value) -> Vec<String> {
    assert_eq!(status, 200, "case {case}: expected 200, got {status}: {body}");
    body[listing.entries_member]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: {} is required, got {body}",
                listing.entries_member
            )
        })
        .iter()
        .map(|entry| {
            entry[listing.key_member]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: entry carries no {}, got {entry}",
                        listing.key_member
                    )
                })
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn every_listing_issues_and_accepts_the_one_pagination_token_format() {
    let router = emulator();
    let graph_id = graph_in_service(&router, "token-codec-graph").await;
    for i in 0..POPULATION {
        snapshot(&router, &graph_id, &format!("token-codec-snapshot-{i}")).await;
    }
    for i in 0..POPULATION {
        endpoint_in_service(&router, &graph_id, &format!("vpc-0a1b2c3d4e5f{i:05}")).await;
    }
    for i in 1..POPULATION {
        graph_in_service(&router, &format!("token-codec-graph-{i}")).await;
    }
    for i in 0..POPULATION {
        export_task(&router, &graph_id, &format!("s3://token-codec-bucket/{i}")).await;
    }

    let vpc_space = "vpc-0a1b2c3d4e5f00000";
    let graph_space = "g-0123456789";
    let snapshot_space = "gs-0123456789";
    let task_space = "t-0123456789";
    let listings = [
        Listing {
            operation: "ListGraphs",
            path: "/graphs".to_string(),
            entries_member: "graphs",
            key_member: "id",
            // The least value of the modelled `GraphId` pattern `^g-[a-z0-9]{10}$`.
            probe_position: "g-0000000000",
            foreign_tokens: vec![
                "not-a-token".to_string(),
                hex::encode(snapshot_space),
                hex::encode(vpc_space),
                hex::encode(task_space),
            ],
        },
        Listing {
            operation: "ListGraphSnapshots",
            path: "/snapshots".to_string(),
            entries_member: "graphSnapshots",
            key_member: "id",
            // The least value of the modelled `SnapshotId` pattern `^gs-[a-z0-9]{10}$`.
            probe_position: "gs-0000000000",
            foreign_tokens: vec![
                "not-a-token".to_string(),
                hex::encode(graph_space),
                hex::encode(vpc_space),
                hex::encode(task_space),
            ],
        },
        Listing {
            operation: "ListPrivateGraphEndpoints",
            path: format!("/graphs/{graph_id}/endpoints/"),
            entries_member: "privateGraphEndpoints",
            key_member: "vpcId",
            // A position in the modelled `VpcId` pattern `^vpc-[a-z0-9]+$` that sorts
            // before every VPC id these endpoints were created in.
            probe_position: "vpc-0",
            foreign_tokens: vec![
                "not-a-token".to_string(),
                hex::encode(graph_space),
                hex::encode(snapshot_space),
                hex::encode(task_space),
            ],
        },
        Listing {
            operation: "ListExportTasks",
            path: "/exporttasks".to_string(),
            entries_member: "tasks",
            key_member: "taskId",
            // The least value of the modelled `ExportTaskId` pattern `^t-[a-z0-9]{10}$`.
            probe_position: "t-0000000000",
            foreign_tokens: vec![
                "not-a-token".to_string(),
                hex::encode(graph_space),
                hex::encode(snapshot_space),
                hex::encode(vpc_space),
            ],
        },
    ];

    for listing in &listings {
        let operation = listing.operation;
        let (status, body) = list(&router, listing, "").await;
        let all = keys(operation, listing, status, &body);
        assert_eq!(
            all.len(),
            POPULATION,
            "case {operation}: expected {POPULATION} entries in one unpaginated page, got {all:?}"
        );
        let mut ordered = all.clone();
        ordered.sort();
        assert_eq!(
            all, ordered,
            "case {operation}: entries are not in one stable ascending order, got {all:?}"
        );

        // Every page a token is issued for carries that token as the hex encoding of
        // this listing and the key of its last entry, whatever the page size.
        for page_size in [1, 2, POPULATION] {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            for page in 0..=POPULATION {
                let mut query = format!("maxResults={page_size}");
                if let Some(value) = &token {
                    query = format!("{query}&nextToken={value}");
                }
                let (status, body) = list(&router, listing, &query).await;
                let case = format!("{operation}, page size {page_size}, page {page}");
                let page_keys = keys(&case, listing, status, &body);
                assert!(
                    page_keys.len() <= page_size,
                    "case {case}: expected at most {page_size} entries, got {page_keys:?}"
                );
                seen.extend(page_keys.iter().cloned());
                token = body["nextToken"].as_str().map(ToString::to_string);
                let Some(issued) = token.as_deref() else {
                    break;
                };
                let last = page_keys
                    .last()
                    .unwrap_or_else(|| panic!("case {case}: a token was issued for an empty page"));
                assert_eq!(
                    issued,
                    issued_token(operation, last),
                    "case {case}: expected the token to be the hex encoding of this listing and \
                     {last:?}, got {issued:?}"
                );
            }
            assert!(
                token.is_none(),
                "case {operation}, page size {page_size}: paging never finished, token {token:?}"
            );
            assert_eq!(
                seen, all,
                "case {operation}, page size {page_size}: paging did not report every entry \
                 exactly once in order, got {seen:?}"
            );
        }

        // A token naming no position in this listing's id space names no page of it.
        for foreign in &listing.foreign_tokens {
            let (status, body) = list(&router, listing, &format!("nextToken={foreign}")).await;
            assert_eq!(
                status, 400,
                "case {operation}, token {foreign:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {operation}, token {foreign:?}: unexpected error code in {body}"
            );
        }

        // A token naming a bare POSITION in this listing's id space resumes from it —
        // rather than being refused or restarting the list — whether or not a stored
        // entry occupies that position, which is what keeps an entry deleted between
        // two pages from hiding the entries that follow it.
        let position = listing.probe_position;
        let token = issued_token(operation, position);
        let (status, body) = list(&router, listing, &format!("nextToken={token}")).await;
        let case = format!("{operation}, position {position:?}");
        let resumed = keys(&case, listing, status, &body);
        let after: Vec<String> = all
            .iter()
            .filter(|key| key.as_str() > position)
            .cloned()
            .collect();
        assert_eq!(
            resumed, after,
            "case {case}: expected exactly the entries sorting after the position, got {resumed:?}"
        );
    }
}
