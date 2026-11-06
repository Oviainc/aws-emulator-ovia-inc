// SPDX-License-Identifier: CC0-1.0

//! Stopping an elastic cluster, end to end over HTTP.
//!
//! The rule these suites assert: **a lifecycle call acts only from the cluster's settled
//! available status, and the status it moves the cluster to is persisted on the cluster's
//! own record**. So `StopCluster` against an `ACTIVE` cluster reports `STOPPING`, the
//! clock owes that cluster `STOPPED` and every later reader sees it; a cluster in any
//! other status is refused with `ValidationException` (HTTP 400 — the client refusal
//! `StopCluster` models, `smithy_query(query="operation", operation="StopCluster")`)
//! and nothing is written; an ARN no cluster carries is `ResourceNotFoundException`
//! (HTTP 404); and a stopped cluster is not in a modifiable status, so a modification of
//! it is refused with `ConflictException` (HTTP 409, the refusal `UpdateCluster` models).
//!
//! The tables differ along what the rule generalises over: the shape of the cluster being
//! stopped (shard count, shard capacity, shard instance count, auth type), the status the
//! cluster is in when the stop arrives, which member a modification of a stopped cluster
//! carries, and which ARN the refusal is addressed to.
//!
//! Each suite in this crate carries its own request harness (see
//! `all/cluster_lifecycle.rs`): the helpers are private to their module, so the few lines
//! below are the local copy rather than a second implementation of anything the service
//! itself owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

/// The region every suite here signs with.
const REGION: &str = "eu-west-2";

/// A SigV4 `Authorization` header for this service in [`REGION`]: the credential scope is
/// what the router routes on and what the handlers read the region from.
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

/// `POST /cluster/{clusterArn}/stop` — one `StopCluster` call.
async fn stop(router: &MultiServiceRouter, cluster_arn: &str) -> (u16, Value) {
    let path = format!("/cluster/{}/stop", label(cluster_arn));
    call(router, "POST", &path, None).await
}

/// `PUT /cluster/{clusterArn}` — one `UpdateCluster` call.
async fn update(router: &MultiServiceRouter, cluster_arn: &str, patch: Value) -> (u16, Value) {
    let path = format!("/cluster/{}", label(cluster_arn));
    call(router, "PUT", &path, Some(patch)).await
}

/// `GET /cluster/{clusterArn}` — the stored cluster, asserted readable.
async fn get_cluster(router: &MultiServiceRouter, cluster_arn: &str) -> Value {
    let path = format!("/cluster/{}", label(cluster_arn));
    let (status, response) = call(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case {cluster_arn}: expected GetCluster to answer 200, got {status} with {response}"
    );
    response["cluster"].clone()
}

/// The summary `ListClusters` reports for `cluster_arn`.
async fn cluster_in_list(router: &MultiServiceRouter, cluster_arn: &str) -> Value {
    let (status, response) = call(router, "GET", "/clusters", None).await;
    assert_eq!(
        status, 200,
        "case {cluster_arn}: expected ListClusters to answer 200, got {status} with {response}"
    );
    response["clusters"]
        .as_array()
        .unwrap_or_else(|| panic!("case {cluster_arn}: no clusters array in {response}"))
        .iter()
        .find(|summary| summary["clusterArn"].as_str() == Some(cluster_arn))
        .cloned()
        .unwrap_or_else(|| panic!("case {cluster_arn}: the cluster is missing from {response}"))
}

/// Create one cluster from `body` and return its `clusterArn`, without waiting for it to
/// settle.
async fn create_cluster(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, response) = call(router, "POST", "/cluster", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateCluster to answer 200, got {status} with {response}"
    );
    response["cluster"]["clusterArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no clusterArn in {response}"))
        .to_string()
}

/// Poll `GetCluster` until the cluster reports `status` — the status the clock owes it a
/// short span after the call that started its current transition — and return it.
async fn wait_for_status(router: &MultiServiceRouter, cluster_arn: &str, status: &str) -> Value {
    for _ in 0..100 {
        let cluster = get_cluster(router, cluster_arn).await;
        if cluster["status"].as_str() == Some(status) {
            return cluster;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("case {cluster_arn}: the cluster never settled to {status}");
}

/// A `CreateCluster` body with every required member, plus whatever `extra` carries.
fn create_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "clusterName": name,
        "authType": "PLAIN_TEXT",
        "adminUserName": "admin",
        "adminUserPassword": "Secret123!",
        "shardCapacity": 2,
        "shardCount": 1,
    });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    body
}

/// Create one cluster and return its `clusterArn` once it has settled to `ACTIVE`, the
/// status a stop acts from.
async fn create_active_cluster(router: &MultiServiceRouter, case: &str, extra: Value) -> String {
    let name = format!(
        "stop{}",
        case.chars().filter(char::is_ascii_alphanumeric).collect::<String>()
    );
    let cluster_arn = create_cluster(router, case, create_body(&name, extra)).await;
    wait_for_status(router, &cluster_arn, "ACTIVE").await;
    cluster_arn
}

/// One cluster shape to stop: the members `CreateCluster` carries beyond the required
/// minimum, and the shard count the stopped cluster must report shards for.
struct Shape {
    case: &'static str,
    extra: Value,
    shard_count: usize,
}

#[tokio::test]
async fn stopping_an_available_cluster_reports_stopping_and_settles_stopped() {
    // The rows differ in the shape of the cluster being stopped: shard count (so the
    // number of shards whose status moves with the cluster varies), shard capacity across
    // the published domain, shard instance count, and auth type.
    let shapes: Vec<Shape> = vec![
        Shape {
            case: "the minimum shape: one shard, smallest capacity",
            extra: json!({}),
            shard_count: 1,
        },
        Shape {
            case: "several shards",
            extra: json!({"shardCount": 4, "shardCapacity": 8}),
            shard_count: 4,
        },
        Shape {
            case: "the largest published shard capacity",
            extra: json!({"shardCapacity": 64, "shardCount": 2}),
            shard_count: 2,
        },
        Shape {
            case: "multiple instances per shard",
            extra: json!({"shardInstanceCount": 3, "shardCount": 3}),
            shard_count: 3,
        },
        Shape {
            case: "the other auth type, and tags",
            extra: json!({"authType": "SECRET_ARN", "tags": {"env": "test"}}),
            shard_count: 1,
        },
    ];

    let router = emulator();
    for shape in &shapes {
        let case = shape.case;
        let cluster_arn = create_active_cluster(&router, case, shape.extra.clone()).await;
        let before = get_cluster(&router, &cluster_arn).await;

        let (status, response) = stop(&router, &cluster_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: stopping an available cluster must answer 200, got {status} with \
             {response}"
        );
        let stopping = response["cluster"].clone();
        assert_eq!(
            stopping["status"].as_str(),
            Some("STOPPING"),
            "case {case:?}: the stop is in flight when the call answers, so the cluster must \
             report STOPPING, got {stopping}"
        );
        let shards = stopping["shards"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no shards in {stopping}"));
        assert_eq!(
            shards.len(),
            shape.shard_count,
            "case {case:?}: expected {} shards, got {stopping}",
            shape.shard_count
        );
        for shard in shards {
            assert_eq!(
                shard["status"].as_str(),
                Some("STOPPING"),
                "case {case:?}: every shard moves with the cluster, got {shard}"
            );
        }
        // The stop moves the status and nothing else: the configuration the create
        // persisted is still what the response is computed from.
        for member in [
            "clusterName",
            "clusterArn",
            "clusterEndpoint",
            "createTime",
            "adminUserName",
            "authType",
            "shardCapacity",
            "shardCount",
            "subnetIds",
            "vpcSecurityGroupIds",
            "preferredMaintenanceWindow",
            "kmsKeyId",
        ] {
            assert_eq!(
                stopping.get(member),
                before.get(member),
                "case {case:?}: `{member}` must be unchanged by the stop, got {:?} where the \
                 stored cluster has {:?}",
                stopping.get(member),
                before.get(member)
            );
        }

        // The stop is persisted, not just reported: the next reader sees it, the clock
        // settles the cluster to STOPPED, and STOPPED is where it stays.
        let settled = wait_for_status(&router, &cluster_arn, "STOPPED").await;
        for shard in settled["shards"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no shards in {settled}"))
        {
            assert_eq!(
                shard["status"].as_str(),
                Some("STOPPED"),
                "case {case:?}: a stopped cluster's shards are stopped too, got {shard}"
            );
        }
        let reread = get_cluster(&router, &cluster_arn).await;
        assert_eq!(
            reread["status"].as_str(),
            Some("STOPPED"),
            "case {case:?}: STOPPED is a settled status, so reading the cluster again must not \
             move it, got {reread}"
        );
        let summary = cluster_in_list(&router, &cluster_arn).await;
        assert_eq!(
            summary["status"].as_str(),
            Some("STOPPED"),
            "case {case:?}: the listing reports the same stored status, got {summary}"
        );
    }
}

#[tokio::test]
async fn stopping_a_cluster_that_is_not_available_is_refused() {
    let router = emulator();

    // Each row leaves the cluster in one status a stop may not act from, and names the
    // statuses the refused cluster may report afterwards (its own, or the one the clock
    // owed it anyway). The rows span: a cluster still being built, one being changed, one
    // already stopping, and one already stopped.
    enum Setup {
        /// Stop the freshly created cluster, before the create settles.
        WhileCreating,
        /// Modify the settled cluster, then stop it while the change is in flight.
        WhileUpdating,
        /// Stop the settled cluster twice, the second time while the first is in flight.
        WhileStopping,
        /// Stop the settled cluster and let the stop settle, then stop it again.
        WhenStopped,
    }

    let cases: &[(&str, Setup, &[&str])] = &[
        ("a cluster still being created", Setup::WhileCreating, &["CREATING", "ACTIVE"]),
        ("a cluster being modified", Setup::WhileUpdating, &["UPDATING", "ACTIVE"]),
        ("a cluster already stopping", Setup::WhileStopping, &["STOPPING", "STOPPED"]),
        ("a cluster already stopped", Setup::WhenStopped, &["STOPPED"]),
    ];

    for (index, (case, setup, allowed)) in cases.iter().enumerate() {
        let name = format!("notavailable{index}");
        let cluster_arn = match setup {
            Setup::WhileCreating => create_cluster(&router, case, create_body(&name, json!({}))).await,
            Setup::WhileUpdating => {
                let arn = create_active_cluster(&router, case, json!({})).await;
                let (status, response) = update(&router, &arn, json!({"shardCapacity": 8})).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the setup modification must be accepted, got {status} with \
                     {response}"
                );
                arn
            }
            Setup::WhileStopping => {
                let arn = create_active_cluster(&router, case, json!({})).await;
                let (status, response) = stop(&router, &arn).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the setup stop must be accepted, got {status} with {response}"
                );
                arn
            }
            Setup::WhenStopped => {
                let arn = create_active_cluster(&router, case, json!({})).await;
                let (status, response) = stop(&router, &arn).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the setup stop must be accepted, got {status} with {response}"
                );
                wait_for_status(&router, &arn, "STOPPED").await;
                arn
            }
        };

        let (status, refused) = stop(&router, &cluster_arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a stop of a cluster that is not available must answer HTTP 400, got \
             {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {refused}"
        );
        assert!(
            refused["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: the refusal must carry a message, got {refused}"
        );

        // The refused stop wrote nothing: the cluster reports the status it was already
        // in, or the one the clock owed it whether or not anybody called.
        let stored = get_cluster(&router, &cluster_arn).await;
        let reported = stored["status"].as_str().unwrap_or("<no status>");
        assert!(
            allowed.contains(&reported),
            "case {case:?}: a refused stop must not move the status; expected one of \
             {allowed:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn stopping_a_cluster_this_service_does_not_hold_is_not_found() {
    let router = emulator();

    // The rows differ in which ARN the stop addresses: one no cluster ever carried, one
    // whose cluster has been deleted, and one naming another of this service's resource
    // kinds.
    let created = create_active_cluster(&router, "deleted", json!({})).await;
    let (status, response) = call(
        &router,
        "DELETE",
        &format!("/cluster/{}", label(&created)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the setup delete must be accepted, got {status} with {response}"
    );

    let snapshot_arn = created.replace("cluster/", "cluster-snapshot/");
    let cases: Vec<(&str, String)> = vec![
        (
            "an ARN no cluster ever carried",
            format!("arn:aws:docdb-elastic:{REGION}:000000000000:cluster/00000000-0000-0000-0000-000000000000"),
        ),
        ("the ARN of a deleted cluster", created.clone()),
        ("a cluster-snapshot ARN", snapshot_arn),
    ];

    for (case, arn) in &cases {
        let (status, refused) = stop(&router, arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a stop of an ARN this service holds no cluster under must answer \
             HTTP 404, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {refused}"
        );
        assert_eq!(
            refused["resourceId"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the refusal must name the identifier the request carried, got \
             {refused}"
        );
        assert_eq!(
            refused["resourceType"].as_str(),
            Some("Cluster"),
            "case {case:?}: the refusal must report the Cluster resource type, got {refused}"
        );
    }
}

#[tokio::test]
async fn a_stopped_cluster_is_not_modifiable() {
    // A stopped cluster is not in a modifiable status, so a modification of it is refused
    // rather than applied. The rows differ in which member the modification carries.
    let patches: Vec<(&str, Value)> = vec![
        ("shardCapacity", json!({"shardCapacity": 16})),
        ("shardCount", json!({"shardCount": 5})),
        ("authType", json!({"authType": "SECRET_ARN"})),
        ("subnetIds", json!({"subnetIds": ["subnet-while-stopped"]})),
        ("backupRetentionPeriod", json!({"backupRetentionPeriod": 21})),
        ("no updatable member", json!({})),
    ];

    let router = emulator();
    for (index, (case, patch)) in patches.iter().enumerate() {
        let cluster_arn = create_active_cluster(&router, &format!("stopped{index}"), json!({})).await;
        let (status, response) = stop(&router, &cluster_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the setup stop must be accepted, got {status} with {response}"
        );
        let stopped = wait_for_status(&router, &cluster_arn, "STOPPED").await;

        let (status, refused) = update(&router, &cluster_arn, patch.clone()).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a modification of a stopped cluster must answer HTTP 409, got \
             {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: expected a ConflictException, got {refused}"
        );
        assert_eq!(
            refused["resourceId"].as_str(),
            Some(cluster_arn.as_str()),
            "case {case:?}: the conflict must name the cluster it is about, got {refused}"
        );
        assert_eq!(
            refused["resourceType"].as_str(),
            Some("Cluster"),
            "case {case:?}: the conflict must report the Cluster resource type, got {refused}"
        );

        // The refused modification wrote nothing: the cluster is still the stopped one
        // the stop left behind.
        let stored = get_cluster(&router, &cluster_arn).await;
        assert_eq!(
            stored["status"].as_str(),
            Some("STOPPED"),
            "case {case:?}: a refused modification must leave the cluster stopped, got {stored}"
        );
        for (member, value) in patch.as_object().expect("a patch object") {
            assert_ne!(
                stored.get(member),
                Some(value),
                "case {case:?}: a refused modification must not apply `{member}`, got {stored}"
            );
            assert_eq!(
                stored.get(member),
                stopped.get(member),
                "case {case:?}: `{member}` must still be what the stop left, got {:?} where the \
                 stopped cluster had {:?}",
                stored.get(member),
                stopped.get(member)
            );
        }
    }
}
