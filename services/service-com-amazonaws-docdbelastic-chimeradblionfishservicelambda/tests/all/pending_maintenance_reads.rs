// SPDX-License-Identifier: CC0-1.0

//! Reading the pending-maintenance view of a resource, end to end over HTTP.
//!
//! The rule these suites assert: `GetPendingMaintenanceAction` answers from the
//! PERSISTED record the ARN it is given names — reporting that ARN back together with
//! the maintenance actions the service has scheduled for it, which is an EMPTY list for
//! a resource with nothing scheduled — it is a reporting call, so it leaves every later
//! caller's view of that resource untouched, and an ARN no live cluster carries is
//! refused 404 instead of answered.
//!
//! The rule generalises over how the cluster came to exist, over its configuration and
//! over the lifecycle point it is read at, so the table below spans clusters minted by
//! `CreateCluster` and by `RestoreClusterFromSnapshot`, read while the clock still owes
//! them their move to `ACTIVE`, after it has settled them, after an update has moved
//! them out of `ACTIVE` again and after tags were written — never over one recorded
//! case. The read is then repeated, and the resource's own state re-read through
//! `GetCluster`, `ListClusters` and `ListPendingMaintenanceActions`, which is exactly
//! what a handler that mutated (or that answered from something other than the record)
//! would fail.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

/// The region the suites sign with.
const REGION: &str = "us-east-2";

/// The canonical placeholder account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The sentence this service answers when a cluster cannot be found. The whole message
/// is fixed text; the identifier the request named travels in `resourceId`.
const CLUSTER_NOT_FOUND: &str = "Unable to find the cluster";

/// The AWS resource type name a cluster refusal reports in `resourceType`.
///
/// Source: the modelled resource shape's own name (`smithy_query(query="shape",
/// shape="Cluster")`), which is what `ResourceNotFoundException$resourceType` carries.
const CLUSTER_TYPE_NAME: &str = "Cluster";

/// How many times a row re-reads the view to show the answer does not drift.
const REPEATED_READS: usize = 3;

/// A SigV4 `Authorization` header for this service in [`REGION`].
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/docdb-elastic/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One emulator: the shared engine plus a router carrying only this service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_docdbelastic_chimeradblionfishservicelambda::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Percent-encode one `@httpLabel` value, the way an AWS SDK does: a label is a single
/// path segment and an ARN carries both `:` and `/`, so neither may survive raw.
fn label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when the
/// body is empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://docdb-elastic.{REGION}.amazonaws.com{path}"))
        .header("host", format!("docdb-elastic.{REGION}.amazonaws.com"))
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{method} {path}: body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code of an error response (`__type`, bare or namespaced).
fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// A `CreateCluster` body with every required member and nothing else.
fn minimal_create(name: &str) -> Value {
    json!({
        "clusterName": name,
        "authType": "PLAIN_TEXT",
        "adminUserName": "admin",
        "adminUserPassword": "Secret123!",
        "shardCapacity": 2,
        "shardCount": 1,
    })
}

/// Create one cluster, asserting it succeeded, and return its `clusterArn`.
async fn create_cluster(router: &MultiServiceRouter, body: Value) -> String {
    let (status, response) = call(router, "POST", "/cluster", Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "case {body}: CreateCluster must answer 200, got {status} with {response}"
    );
    response["cluster"]["clusterArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: the created cluster must carry its ARN: {response}"))
        .to_string()
}

/// Read one cluster back, asserting it succeeded, and return its `cluster` object.
async fn get_cluster(router: &MultiServiceRouter, cluster_arn: &str) -> Value {
    let path = format!("/cluster/{}", label(cluster_arn));
    let (status, response) = call(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case {cluster_arn}: GetCluster must answer 200, got {status} with {response}"
    );
    response["cluster"].clone()
}

/// Poll `GetCluster` until the cluster reports `ACTIVE` — the status the clock owes it
/// after a create, a restore or an update — and return it.
async fn settled_cluster(router: &MultiServiceRouter, cluster_arn: &str) -> Value {
    for _ in 0..80 {
        let cluster = get_cluster(router, cluster_arn).await;
        if cluster["status"].as_str() == Some("ACTIVE") {
            return cluster;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("case {cluster_arn}: the cluster never settled to ACTIVE");
}

/// Read the pending-maintenance view of one resource, asserting it succeeded, and
/// return its `resourcePendingMaintenanceAction` object.
async fn pending_view(router: &MultiServiceRouter, resource_arn: &str) -> Value {
    let path = format!("/pending-action/{}", label(resource_arn));
    let (status, response) = call(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case {resource_arn}: GetPendingMaintenanceAction must answer 200, got {status} \
         with {response}"
    );
    response["resourcePendingMaintenanceAction"].clone()
}

/// Every `resourcePendingMaintenanceAction` `ListPendingMaintenanceActions` reports.
async fn listed_pending_views(router: &MultiServiceRouter) -> Vec<Value> {
    let (status, response) = call(router, "GET", "/pending-actions", None).await;
    assert_eq!(
        status, 200,
        "ListPendingMaintenanceActions must answer 200, got {status} with {response}"
    );
    response["resourcePendingMaintenanceActions"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("the required resourcePendingMaintenanceActions member is missing: {response}")
        })
}

/// Every `clusterArn` `ListClusters` currently reports, in the order it reports them.
async fn listed_cluster_arns(router: &MultiServiceRouter) -> Vec<String> {
    let (status, response) = call(router, "GET", "/clusters", None).await;
    assert_eq!(
        status, 200,
        "ListClusters must answer 200, got {status} with {response}"
    );
    response["clusters"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|cluster| {
            cluster["clusterArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Assert one view reports `resource_arn` and an EMPTY action list, which is how the
/// service says "this resource has nothing scheduled".
fn assert_reports_nothing_pending(case: &str, resource_arn: &str, view: &Value) {
    assert_eq!(
        view["resourceArn"].as_str(),
        Some(resource_arn),
        "case {case:?}: the view must name the resource it was asked about, got {view}"
    );
    let details = view["pendingMaintenanceActionDetails"].as_array();
    assert!(
        details.is_some(),
        "case {case:?}: the action list member must be present, got {view}"
    );
    assert_eq!(
        details.map(Vec::len),
        Some(0),
        "case {case:?}: a resource with nothing scheduled reports an EMPTY action list, \
         got {view}"
    );
}

/// What a row does to its cluster between creating it and reading its view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prepare {
    /// Read it straight away, while the clock still owes it its move to `ACTIVE`.
    Immediately,
    /// Read it once the clock has settled it.
    AfterSettling,
    /// Settle it, then update it, so it is read while a change is in flight.
    AfterAnUpdate,
    /// Settle it, then write tags on it, so it is read after a write to its record.
    AfterTagging,
}

/// How a row's cluster came to exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provenance {
    /// Minted by `CreateCluster`.
    Created,
    /// Minted by `RestoreClusterFromSnapshot` from a snapshot of another cluster.
    RestoredFromSnapshot,
}

/// One row of the table: a cluster of some provenance and configuration, read at some
/// lifecycle point.
struct Row {
    case: &'static str,
    provenance: Provenance,
    /// The `CreateCluster` body of this row's cluster (the source cluster, for a
    /// restored row).
    create: Value,
    prepare: Prepare,
}

/// The table: provenance x configuration x lifecycle point.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a just-created cluster with the minimum configuration",
            provenance: Provenance::Created,
            create: minimal_create("pmaalpha"),
            prepare: Prepare::Immediately,
        },
        Row {
            case: "a settled cluster with many shards and a maintenance window",
            provenance: Provenance::Created,
            create: json!({
                "clusterName": "pmabravo", "authType": "SECRET_ARN", "adminUserName": "dbadmin",
                "adminUserPassword": "LongerPassword1", "shardCapacity": 16, "shardCount": 4,
                "shardInstanceCount": 2, "subnetIds": ["subnet-aaa", "subnet-bbb"],
                "vpcSecurityGroupIds": ["sg-111"], "backupRetentionPeriod": 7,
                "preferredMaintenanceWindow": "Tue:04:00-Tue:04:30",
                "preferredBackupWindow": "02:00-02:30",
            }),
            prepare: Prepare::AfterSettling,
        },
        Row {
            case: "a cluster with a change still in flight",
            provenance: Provenance::Created,
            create: json!({
                "clusterName": "pmacharlie", "authType": "PLAIN_TEXT", "adminUserName": "a",
                "adminUserPassword": "12345678", "shardCapacity": 64, "shardCount": 2,
            }),
            prepare: Prepare::AfterAnUpdate,
        },
        Row {
            case: "a cluster whose record was written by a tagging call",
            provenance: Provenance::Created,
            create: json!({
                "clusterName": "pmadelta", "authType": "PLAIN_TEXT", "adminUserName": "admin",
                "adminUserPassword": "Secret123!", "shardCapacity": 2, "shardCount": 1,
                "tags": {"team": "db"},
            }),
            prepare: Prepare::AfterTagging,
        },
        Row {
            case: "a cluster restored from a snapshot, read straight away",
            provenance: Provenance::RestoredFromSnapshot,
            create: minimal_create("pmaecho"),
            prepare: Prepare::Immediately,
        },
        Row {
            case: "a cluster restored from a snapshot, read once settled",
            provenance: Provenance::RestoredFromSnapshot,
            create: minimal_create("pmafoxtrot"),
            prepare: Prepare::AfterSettling,
        },
    ]
}

/// Bring one row's cluster into being and to its lifecycle point, and return the ARN of
/// the cluster whose view the row reads.
async fn stage(router: &MultiServiceRouter, row: &Row) -> String {
    let source_arn = create_cluster(router, row.create.clone()).await;
    let source_name = row.create["clusterName"].as_str().expect("a cluster name");

    let cluster_arn = match row.provenance {
        Provenance::Created => source_arn,
        Provenance::RestoredFromSnapshot => {
            // A snapshot can only be taken of a settled cluster, and a restore reads
            // the snapshot's record of it.
            settled_cluster(router, &source_arn).await;
            let snapshot_name = format!("{source_name}-snap");
            let (status, snapshot) = call(
                router,
                "POST",
                "/cluster-snapshot",
                Some(json!({"clusterArn": source_arn, "snapshotName": snapshot_name})),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: CreateClusterSnapshot must answer 200, got {status} with {snapshot}",
                row.case
            );
            let snapshot_arn = snapshot["snapshot"]["snapshotArn"]
                .as_str()
                .expect("the created snapshot carries its ARN")
                .to_string();
            let path = format!("/cluster-snapshot/{}/restore", label(&snapshot_arn));
            let body = json!({"clusterName": format!("{source_name}r")});
            let (status, restored) = call(router, "POST", &path, Some(body)).await;
            assert_eq!(
                status, 200,
                "case {:?}: RestoreClusterFromSnapshot must answer 200, got {status} with \
                 {restored}",
                row.case
            );
            restored["cluster"]["clusterArn"]
                .as_str()
                .expect("the restored cluster carries its ARN")
                .to_string()
        }
    };

    match row.prepare {
        Prepare::Immediately => {}
        Prepare::AfterSettling => {
            settled_cluster(router, &cluster_arn).await;
        }
        Prepare::AfterAnUpdate => {
            settled_cluster(router, &cluster_arn).await;
            let path = format!("/cluster/{}", label(&cluster_arn));
            let (status, updated) =
                call(router, "PUT", &path, Some(json!({"shardCount": 3}))).await;
            assert_eq!(
                status, 200,
                "case {:?}: UpdateCluster must answer 200, got {status} with {updated}",
                row.case
            );
        }
        Prepare::AfterTagging => {
            settled_cluster(router, &cluster_arn).await;
            let path = format!("/tags/{}", label(&cluster_arn));
            let (status, tagged) =
                call(router, "POST", &path, Some(json!({"tags": {"owner": "qa"}}))).await;
            assert_eq!(
                status, 200,
                "case {:?}: TagResource must answer 200, got {status} with {tagged}",
                row.case
            );
        }
    }

    cluster_arn
}

#[tokio::test]
async fn the_pending_maintenance_view_is_computed_from_the_named_resources_own_record() {
    let router = emulator();

    let mut staged: Vec<(&str, String)> = Vec::new();
    for row in rows() {
        let cluster_arn = stage(&router, &row).await;
        let view = pending_view(&router, &cluster_arn).await;
        assert_reports_nothing_pending(row.case, &cluster_arn, &view);

        // Repeating the read answers the same thing: the view is a report of the
        // record, not something the call itself produced or consumed.
        for attempt in 1..=REPEATED_READS {
            let again = pending_view(&router, &cluster_arn).await;
            assert_eq!(
                again, view,
                "case {:?}: read {attempt} must report what the first read did ({view}), \
                 got {again}",
                row.case
            );
        }
        staged.push((row.case, cluster_arn));
    }

    // Every staged cluster still reports its OWN ARN once the store holds all of them:
    // the view follows the ARN the request named and not the store's contents.
    for (case, cluster_arn) in &staged {
        let view = pending_view(&router, cluster_arn).await;
        assert_reports_nothing_pending(
            &format!("{case} / with every cluster present"),
            cluster_arn,
            &view,
        );
    }

    // The per-resource view and the listing agree: nothing is scheduled for any of
    // them, so the listing of resources WITH something pending is empty.
    let listed = listed_pending_views(&router).await;
    assert!(
        listed.is_empty(),
        "case every staged cluster: a resource with nothing scheduled must not be \
         listed, got {listed:?}"
    );
}

#[tokio::test]
async fn reading_the_pending_maintenance_view_changes_nothing_a_later_caller_sees() {
    let router = emulator();

    // Settled clusters, so the only thing that could move their state is the read under
    // test rather than a transition the clock still owes them.
    let cases: Vec<(&str, Value)> = vec![
        ("the minimum configuration", minimal_create("stablealpha")),
        (
            "a richer configuration",
            json!({
                "clusterName": "stablebravo", "authType": "SECRET_ARN",
                "adminUserName": "dbadmin", "adminUserPassword": "LongerPassword1",
                "shardCapacity": 8, "shardCount": 3, "tags": {"team": "db"},
            }),
        ),
    ];

    let mut arns = Vec::new();
    for (case, body) in &cases {
        let cluster_arn = create_cluster(&router, body.clone()).await;
        settled_cluster(&router, &cluster_arn).await;
        arns.push((*case, cluster_arn));
    }

    let clusters_before = listed_cluster_arns(&router).await;
    let listing_before = listed_pending_views(&router).await;
    let mut records_before = Vec::new();
    for (case, cluster_arn) in &arns {
        records_before.push((case, get_cluster(&router, cluster_arn).await));
    }

    // Read every view several times — the whole effect of which must be nothing.
    for _ in 0..REPEATED_READS {
        for (case, cluster_arn) in &arns {
            let view = pending_view(&router, cluster_arn).await;
            assert_reports_nothing_pending(case, cluster_arn, &view);
        }
    }

    for ((case, before), (_, cluster_arn)) in records_before.iter().zip(arns.iter()) {
        let after = get_cluster(&router, cluster_arn).await;
        assert_eq!(
            &after, before,
            "case {case:?}: reading the pending-maintenance view must leave the cluster \
             exactly as it was ({before}), got {after}"
        );
    }
    let clusters_after = listed_cluster_arns(&router).await;
    assert_eq!(
        clusters_after, clusters_before,
        "case every cluster: the reads must not add, drop or reorder a cluster \
         ({clusters_before:?}), got {clusters_after:?}"
    );
    let listing_after = listed_pending_views(&router).await;
    assert_eq!(
        listing_after, listing_before,
        "case every cluster: the reads must not schedule a maintenance action \
         ({listing_before:?}), got {listing_after:?}"
    );
}

#[tokio::test]
async fn the_view_is_answered_for_a_live_cluster_and_refused_for_anything_else() {
    let router = emulator();

    // Two clusters and a snapshot of one of them, so each refusal below is about the
    // ARN the request named and not about an empty store.
    let kept = create_cluster(&router, minimal_create("keptcluster")).await;
    let doomed = create_cluster(&router, minimal_create("doomedcluster")).await;
    settled_cluster(&router, &kept).await;
    settled_cluster(&router, &doomed).await;
    let (status, snapshot) = call(
        &router,
        "POST",
        "/cluster-snapshot",
        Some(json!({"clusterArn": kept, "snapshotName": "keptcluster-snap"})),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture snapshot must be created, got {status} with {snapshot}"
    );
    let snapshot_arn = snapshot["snapshot"]["snapshotArn"]
        .as_str()
        .expect("the created snapshot carries its ARN")
        .to_string();

    // While the cluster is live, its view is answered.
    let view = pending_view(&router, &doomed).await;
    assert_reports_nothing_pending("before the delete", &doomed, &view);

    let path = format!("/cluster/{}", label(&doomed));
    let (status, deleted) = call(&router, "DELETE", &path, None).await;
    assert_eq!(
        status, 200,
        "DeleteCluster must answer 200, got {status} with {deleted}"
    );

    // The rows differ along WHY the ARN names no live cluster: a record that is gone, a
    // resource of the other class that does exist, and an identifier that was never a
    // cluster ARN at all.
    let refused: Vec<(String, String)> = vec![
        ("a cluster that has been deleted".to_string(), doomed.clone()),
        (
            "the ARN of a snapshot that does exist".to_string(),
            snapshot_arn,
        ),
        (
            "an ARN no resource carries".to_string(),
            format!(
                "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:cluster/00000000-0000-0000-0000-000000000000"
            ),
        ),
        (
            "an identifier that is not an ARN at all".to_string(),
            "not-an-arn-at-all".to_string(),
        ),
    ];

    for (case, identifier) in refused {
        let path = format!("/pending-action/{}", label(&identifier));
        let (status, response) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a resource that is not a live cluster is HTTP 404, got \
             {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            response["message"].as_str(),
            Some(CLUSTER_NOT_FOUND),
            "case {case:?}: expected the fixed cluster not-found sentence, got {response}"
        );
        assert_eq!(
            response["resourceId"].as_str(),
            Some(identifier.as_str()),
            "case {case:?}: resourceId must be the identifier the request named, got \
             {response}"
        );
        assert_eq!(
            response["resourceType"].as_str(),
            Some(CLUSTER_TYPE_NAME),
            "case {case:?}: resourceType must be the AWS resource type name of the \
             resource this operation addresses, got {response}"
        );
    }

    // The cluster that was NOT deleted still answers, so the refusals above are about
    // the resource each request named.
    let still_there = pending_view(&router, &kept).await;
    assert_reports_nothing_pending("the cluster that was kept", &kept, &still_there);
}
