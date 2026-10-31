// SPDX-License-Identifier: CC0-1.0

//! Which LISTING a `nextToken` of this service belongs to, driven over the wire
//! (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: a pagination token names a position in the
//! order of the ONE listing that minted it, so a listing resumes from a token it issued
//! itself and refuses a token any other listing issued — with the modelled
//! `ValidationException` (400) for a token this service never issued for it — rather
//! than using it as a cut point in its own collection. A token of another listing is
//! refused even when the position it carries lies in the same modelled id space as this
//! listing's positions: the model spells one `^t-[a-z0-9]{10}$` pattern for an import
//! task and an export task, while the two task listings order two independently keyed
//! collections, so one such position names a different cut point in each of them.
//!
//! The table spans what the rule generalises over: every ORDERED PAIR of distinct
//! listings this service serves (`ListGraphs`, `ListGraphSnapshots`, `ListImportTasks`,
//! `ListExportTasks`, `ListPrivateGraphEndpoints`) — twenty pairs over four id spaces,
//! including both directions of the two listings that share one id space — each asked
//! with a token the other listing really minted. Every listing is also asked with its
//! OWN minted token, and read again with no token after the refusals, so the refusals
//! are about which listing the token came from and not about pagination being refused
//! outright or about entries having gone missing.

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

/// How many entries each listing is populated with: more than one, so a page of one
/// entry leaves an entry behind and the listing mints a token to resume from.
const POPULATION: usize = 2;

/// The modelled `reason` a token the caller sent that this service cannot resolve to a
/// position is classified by, from `ValidationExceptionReason`'s values
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`): the request named
/// an argument this operation does not accept.
const ILLEGAL_ARGUMENT: &str = "ILLEGAL_ARGUMENT";

/// A KMS key ARN matching the modelled `KmsKeyArn` pattern
/// `^arn:aws[^:]*:kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$`.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012";

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneExportRole";

/// An emulator holding only this service, on its own engine, so the test starts from an
/// empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request signed for `REGION` and return its status and parsed
/// JSON body.
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

/// Poll one resource until it reports `AVAILABLE`. No handler ever sleeps; only this
/// test does, because it is asking about a state the clock reaches on its own.
async fn wait_until_available(router: &MultiServiceRouter, path: &str) {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {path}: expected 200 from the read, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {path}: never became AVAILABLE, last status {}",
        body["status"]
    );
}

/// Create a graph and wait it into service, returning its id.
async fn graph_in_service(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
    let (status, created) = call(router, "POST", "/graphs", Some(body.clone())).await;
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

/// Start one plain CSV export of `graph_id`, which this suite needs only as a row of the
/// export-task listing.
async fn export_task(router: &MultiServiceRouter, graph_id: &str, destination: &str) {
    let body = json!({
        "graphIdentifier": graph_id,
        "roleArn": ROLE_ARN,
        "format": "CSV",
        "destination": destination,
        "kmsKeyIdentifier": KMS_KEY_ARN,
    });
    let (status, started) = call(router, "POST", "/exporttasks", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from StartExportTask, got {status}: {started}"
    );
}

/// `CreateGraphUsingImportTask`, which mints a graph AND the import task that loads it:
/// the cheapest way to put several rows in the import-task listing, since each task gets
/// its own target graph.
async fn graph_with_import_task(router: &MultiServiceRouter, graph_name: &str) {
    let body = json!({
        "graphName": graph_name,
        "source": "s3://token-scope-bucket/import/",
        "roleArn": ROLE_ARN,
    });
    let (status, created) = call(router, "POST", "/importtasks", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from CreateGraphUsingImportTask, got {status}: {created}"
    );
}

/// One paginated listing of this service: the operation, where it is served, the member
/// carrying its entries and the member of an entry its order is keyed by.
struct Listing {
    operation: &'static str,
    path: String,
    entries_member: &'static str,
    key_member: &'static str,
}

/// `GET` the listing with the query string `query` (already encoded, without the `?`).
async fn list(router: &MultiServiceRouter, listing: &Listing, query: &str) -> (u16, Value) {
    let path = if query.is_empty() {
        listing.path.clone()
    } else {
        format!("{}?{query}", listing.path)
    };
    call(router, "GET", &path, None).await
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

/// Every listing of this service, populated so each of them mints a token of its own.
async fn populated(router: &MultiServiceRouter) -> Vec<Listing> {
    let graph_id = graph_in_service(router, "token-scope-graph").await;
    for i in 0..POPULATION {
        snapshot(router, &graph_id, &format!("token-scope-snapshot-{i}")).await;
    }
    for i in 0..POPULATION {
        endpoint_in_service(router, &graph_id, &format!("vpc-0a1b2c3d4e5f{i:05}")).await;
    }
    for i in 0..POPULATION {
        export_task(router, &graph_id, &format!("s3://token-scope-bucket/{i}")).await;
    }
    // Each of these mints its own graph too, which is what puts the graph listing above
    // one entry as well.
    for i in 0..POPULATION {
        graph_with_import_task(router, &format!("token-scope-import-{i}")).await;
    }

    vec![
        Listing {
            operation: "ListGraphs",
            path: "/graphs".to_string(),
            entries_member: "graphs",
            key_member: "id",
        },
        Listing {
            operation: "ListGraphSnapshots",
            path: "/snapshots".to_string(),
            entries_member: "graphSnapshots",
            key_member: "id",
        },
        Listing {
            operation: "ListImportTasks",
            path: "/importtasks".to_string(),
            entries_member: "tasks",
            key_member: "taskId",
        },
        Listing {
            operation: "ListExportTasks",
            path: "/exporttasks".to_string(),
            entries_member: "tasks",
            key_member: "taskId",
        },
        Listing {
            operation: "ListPrivateGraphEndpoints",
            path: format!("/graphs/{graph_id}/endpoints/"),
            entries_member: "privateGraphEndpoints",
            key_member: "vpcId",
        },
    ]
}

#[tokio::test]
async fn a_token_is_taken_back_only_by_the_listing_that_minted_it() {
    let router = emulator();
    let listings = populated(&router).await;

    // Every listing, its entries in full, and the token it mints after a page of one.
    let mut minted: Vec<(&Listing, Vec<String>, String)> = Vec::new();
    for listing in &listings {
        let operation = listing.operation;
        let (status, body) = list(&router, listing, "").await;
        let all = keys(operation, listing, status, &body);
        assert!(
            all.len() >= 2,
            "case {operation}: this suite needs at least two entries to page over, got {all:?}"
        );

        let (status, page) = list(&router, listing, "maxResults=1").await;
        let first = keys(&format!("{operation} first page"), listing, status, &page);
        assert_eq!(
            first,
            all[..1].to_vec(),
            "case {operation}: expected the first entry on a page of one, got {first:?}"
        );
        let token = page["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {operation}: expected a nextToken while entries remain, got {page}")
            })
            .to_string();
        minted.push((listing, all, token));
    }

    // Each listing takes its OWN token back and serves exactly the entries that follow
    // the page it was minted after.
    for (listing, all, token) in &minted {
        let operation = listing.operation;
        let case = format!("{operation} resumed from its own token {token:?}");
        let (status, body) = list(&router, listing, &format!("nextToken={token}")).await;
        let resumed = keys(&case, listing, status, &body);
        assert_eq!(
            resumed,
            all[1..].to_vec(),
            "case {case}: expected the entries after the first, got {resumed:?}"
        );
    }

    // Every ORDERED PAIR of distinct listings: the token one listing minted names no
    // page of another, so it is refused as a token this service never issued for it
    // rather than used as a cut point in that listing's own collection.
    for (minting, _, token) in &minted {
        for (asked, all, _) in &minted {
            if minting.operation == asked.operation {
                continue;
            }
            let case = format!(
                "{} asked with the token {} minted ({token:?})",
                asked.operation, minting.operation
            );
            let (status, answer) = list(&router, asked, &format!("nextToken={token}")).await;
            assert_eq!(
                status, 400,
                "case {case}: expected 400, got {status}: {answer}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "ValidationException",
                "case {case}: expected ValidationException, got {code:?}: {answer}"
            );
            let expected =
                format!("nextToken '{token}' is not a pagination token this service issued");
            let actual = answer["message"].as_str().unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case {case}: expected {expected:?}, got {actual:?}"
            );
            let reason = answer["reason"].as_str();
            assert_eq!(
                reason,
                Some(ILLEGAL_ARGUMENT),
                "case {case}: expected reason {ILLEGAL_ARGUMENT:?}, got {reason:?} in {answer}"
            );

            // The refused request changed nothing: the listing still reports every one
            // of its entries, so a foreign token can neither truncate a page nor empty
            // one.
            let (status, body) = list(&router, asked, "").await;
            let again = keys(&case, asked, status, &body);
            assert_eq!(
                &again, all,
                "case {case}: expected the listing to still report {all:?}, got {again:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_minted_token_names_the_listing_it_resumes() {
    let router = emulator();
    let listings = populated(&router).await;

    // Whatever else a token carries, it carries WHICH listing handed it out: that is
    // what a listing checks before reading a position out of it, and the one thing the
    // id space of a position cannot say when two listings share one id space.
    for listing in &listings {
        let operation = listing.operation;
        let (status, page) = list(&router, listing, "maxResults=1").await;
        let keys = keys(&format!("{operation} first page"), listing, status, &page);
        let token = page["nextToken"].as_str().unwrap_or_else(|| {
            panic!("case {operation}: expected a nextToken while entries remain, got {page}")
        });
        let decoded = hex::decode(token)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_else(|| panic!("case {operation}: token {token:?} is not hex-encoded text"));
        assert!(
            decoded.starts_with(operation),
            "case {operation}: expected the token to name its own listing, got {decoded:?}"
        );
        let last = keys
            .last()
            .unwrap_or_else(|| panic!("case {operation}: a token was issued for an empty page"));
        assert!(
            decoded.ends_with(last.as_str()),
            "case {operation}: expected the token to name the position {last:?}, got {decoded:?}"
        );
    }
}
