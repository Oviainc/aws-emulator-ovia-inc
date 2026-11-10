// SPDX-License-Identifier: CC0-1.0

//! Deleting an elastic cluster is gated on the cluster's stored status.
//!
//! The rule these suites assert: **`DeleteCluster` reads the stored cluster's status and
//! refuses the call, before any write, for a status that does not admit a delete — a
//! cluster that has been stopped, or one on its way to stopped, has to be started again
//! first — while a cluster in any other status is deleted.** The refusal is
//! `ValidationException` (HTTP 400), the client refusal `DeleteCluster` models
//! (`smithy_query(query="operation", operation="DeleteCluster")`), stated as one sentence
//! whose only variable part is the status the cluster is stored in; a refused delete
//! removes nothing, so the cluster is still readable through `GetCluster` and still
//! listed.
//!
//! `StartCluster` is the way out of that gate, so it is asserted here too: it restarts a
//! stopped cluster (`STARTING`, then the available status the clock owes it), from which
//! the delete the stopped cluster refused is accepted, and it refuses a cluster that is
//! not stopped with the same `ValidationException` its model declares.
//!
//! The tables differ along what the rule generalises over: the status the cluster is in
//! when the call arrives (both of the refused ones and two accepted ones), the shape of
//! the cluster (shard count, shard capacity, shard instance count, auth type, tags), and
//! how many times the refused delete is retried.
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
const REGION: &str = "us-east-2";

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

/// `DELETE /cluster/{clusterArn}` — one `DeleteCluster` call.
async fn delete(router: &MultiServiceRouter, cluster_arn: &str) -> (u16, Value) {
    let path = format!("/cluster/{}", label(cluster_arn));
    call(router, "DELETE", &path, None).await
}

/// `POST /cluster/{clusterArn}/stop` — one `StopCluster` call, asserted accepted.
async fn stop(router: &MultiServiceRouter, case: &str, cluster_arn: &str) {
    let path = format!("/cluster/{}/stop", label(cluster_arn));
    let (status, response) = call(router, "POST", &path, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the setup stop must be accepted, got {status} with {response}"
    );
}

/// `POST /cluster/{clusterArn}/start` — one `StartCluster` call.
async fn start(router: &MultiServiceRouter, cluster_arn: &str) -> (u16, Value) {
    let path = format!("/cluster/{}/start", label(cluster_arn));
    call(router, "POST", &path, None).await
}

/// `GET /cluster/{clusterArn}` — the stored cluster, asserted readable.
async fn get_cluster(router: &MultiServiceRouter, case: &str, cluster_arn: &str) -> Value {
    let path = format!("/cluster/{}", label(cluster_arn));
    let (status, response) = call(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetCluster to answer 200, got {status} with {response}"
    );
    response["cluster"].clone()
}

/// Whether `ListClusters` still reports a summary for `cluster_arn`.
async fn is_listed(router: &MultiServiceRouter, case: &str, cluster_arn: &str) -> bool {
    let (status, response) = call(router, "GET", "/clusters", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListClusters to answer 200, got {status} with {response}"
    );
    response["clusters"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no clusters array in {response}"))
        .iter()
        .any(|summary| summary["clusterArn"].as_str() == Some(cluster_arn))
}

/// A `CreateCluster` body with every required member, plus whatever `extra` carries.
fn create_body(name: &str, extra: &Value) -> Value {
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

/// Create one cluster from `body` and return its `clusterArn`, without waiting for it to
/// settle.
async fn create_cluster(router: &MultiServiceRouter, case: &str, name: &str, extra: &Value) -> String {
    let (status, response) = call(router, "POST", "/cluster", Some(create_body(name, extra))).await;
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
async fn wait_for_status(
    router: &MultiServiceRouter,
    case: &str,
    cluster_arn: &str,
    status: &str,
) -> Value {
    for _ in 0..100 {
        let cluster = get_cluster(router, case, cluster_arn).await;
        if cluster["status"].as_str() == Some(status) {
            return cluster;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("case {case:?}: the cluster never settled to {status}");
}

/// Create one cluster and return its `clusterArn` once it has settled to `ACTIVE`.
async fn create_active_cluster(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    extra: &Value,
) -> String {
    let cluster_arn = create_cluster(router, case, name, extra).await;
    wait_for_status(router, case, &cluster_arn, "ACTIVE").await;
    cluster_arn
}

/// One cluster shape to delete: the members `CreateCluster` carries beyond the required
/// minimum, and the number of shards the cluster then holds.
struct Shape {
    case: &'static str,
    name: &'static str,
    extra: Value,
    shard_count: usize,
}

/// The shapes every table here spans: they differ in shard count, shard capacity across
/// the published domain, instances per shard, auth type and tags, so no row's outcome can
/// be a property of one record.
fn shapes() -> Vec<Shape> {
    vec![
        Shape {
            case: "the minimum shape: one shard, smallest capacity",
            name: "gateminimal",
            extra: json!({}),
            shard_count: 1,
        },
        Shape {
            case: "several shards",
            name: "gateshards",
            extra: json!({"shardCount": 4, "shardCapacity": 8}),
            shard_count: 4,
        },
        Shape {
            case: "the largest published shard capacity",
            name: "gatelargest",
            extra: json!({"shardCapacity": 64, "shardCount": 2}),
            shard_count: 2,
        },
        Shape {
            case: "multiple instances per shard",
            name: "gateinstances",
            extra: json!({"shardInstanceCount": 3, "shardCount": 3}),
            shard_count: 3,
        },
        Shape {
            case: "the other auth type, and tags",
            name: "gateauthtags",
            extra: json!({"authType": "SECRET_ARN", "tags": {"env": "test"}}),
            shard_count: 1,
        },
    ]
}

#[tokio::test]
async fn deleting_a_stopped_cluster_is_refused_and_leaves_it_stopped() {
    let router = emulator();
    for shape in &shapes() {
        let case = shape.case;
        let cluster_arn = create_active_cluster(&router, case, shape.name, &shape.extra).await;
        stop(&router, case, &cluster_arn).await;
        let stopped = wait_for_status(&router, case, &cluster_arn, "STOPPED").await;

        // The refusal does not wear out: the gate is a function of the stored status, so
        // every attempt is refused the same way and none of them writes.
        for attempt in 1..=3 {
            let (status, refused) = delete(&router, &cluster_arn).await;
            assert_eq!(
                status, 400,
                "case {case:?} attempt {attempt}: deleting a stopped cluster must answer HTTP \
                 400, got {status} with {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case:?} attempt {attempt}: expected a ValidationException, got {refused}"
            );
            assert_eq!(
                refused["message"].as_str(),
                Some("Can't delete the cluster because it is in STOPPED state."),
                "case {case:?} attempt {attempt}: the refusal must name the status the cluster \
                 is stored in, got {refused}"
            );
        }

        // Nothing was removed: the cluster reads back exactly as the stop left it, shards
        // included, and is still listed.
        let stored = get_cluster(&router, case, &cluster_arn).await;
        assert_eq!(
            stored, stopped,
            "case {case:?}: a refused delete must leave the record untouched, got {stored} \
             where the stopped cluster was {stopped}"
        );
        let shards = stored["shards"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no shards in {stored}"));
        assert_eq!(
            shards.len(),
            shape.shard_count,
            "case {case:?}: expected {} shards to survive the refused delete, got {stored}",
            shape.shard_count
        );
        for shard in shards {
            assert_eq!(
                shard["status"].as_str(),
                Some("STOPPED"),
                "case {case:?}: a refused delete must not move a shard's status, got {shard}"
            );
        }
        assert!(
            is_listed(&router, case, &cluster_arn).await,
            "case {case:?}: a cluster whose delete was refused must still be listed"
        );
    }
}

#[tokio::test]
async fn deleting_a_cluster_that_is_stopping_is_refused_and_it_settles_stopped() {
    let router = emulator();
    for shape in &shapes() {
        let case = shape.case;
        let name = format!("stopping{}", shape.name);
        let cluster_arn = create_active_cluster(&router, case, &name, &shape.extra).await;
        // The delete arrives while the stop is still in flight, so the status the gate
        // reads is the transient one on the way to stopped.
        stop(&router, case, &cluster_arn).await;

        let (status, refused) = delete(&router, &cluster_arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: deleting a cluster that is stopping must answer HTTP 400, got \
             {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {refused}"
        );
        assert_eq!(
            refused["message"].as_str(),
            Some("Can't delete the cluster because it is in STOPPING state."),
            "case {case:?}: the refusal must name the status the cluster is stored in, got \
             {refused}"
        );

        // The refused delete wrote nothing, so the stop the clock owed still completes and
        // the cluster is still there to report it.
        let settled = wait_for_status(&router, case, &cluster_arn, "STOPPED").await;
        assert_eq!(
            settled["clusterArn"].as_str(),
            Some(cluster_arn.as_str()),
            "case {case:?}: the surviving cluster is the one the delete was refused for, got \
             {settled}"
        );
        assert!(
            is_listed(&router, case, &cluster_arn).await,
            "case {case:?}: a cluster whose delete was refused must still be listed"
        );
    }
}

#[tokio::test]
async fn deleting_a_cluster_in_a_status_that_admits_it_still_succeeds() {
    // The rows differ in the status the cluster is in when the delete arrives: its settled
    // available status, and the transient status a create leaves it in — a delete is how a
    // caller gets rid of a cluster the service has not finished bringing up, so neither
    // status blocks one.
    enum When {
        /// Delete the cluster once the create has settled.
        WhenActive,
        /// Delete the freshly created cluster, before the create settles.
        WhileCreating,
    }

    let router = emulator();
    for (when, suffix, expected) in [
        (When::WhenActive, "active", "ACTIVE"),
        (When::WhileCreating, "creating", "CREATING"),
    ] {
        for shape in &shapes() {
            let case = format!("{} while {suffix}", shape.case);
            let name = format!("{}{suffix}", shape.name);
            let cluster_arn = match when {
                When::WhenActive => create_active_cluster(&router, &case, &name, &shape.extra).await,
                When::WhileCreating => create_cluster(&router, &case, &name, &shape.extra).await,
            };
            let before = get_cluster(&router, &case, &cluster_arn).await;
            assert_eq!(
                before["status"].as_str(),
                Some(expected),
                "case {case:?}: the setup must leave the cluster in {expected}, got {before}"
            );

            let (status, response) = delete(&router, &cluster_arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a delete from {expected} must answer 200, got {status} with \
                 {response}"
            );
            let deleted = response["cluster"].clone();
            assert_eq!(
                deleted["status"].as_str(),
                Some("DELETING"),
                "case {case:?}: the final cluster reports DELETING, got {deleted}"
            );
            let shards = deleted["shards"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no shards in {deleted}"));
            assert_eq!(
                shards.len(),
                shape.shard_count,
                "case {case:?}: expected {} shards, got {deleted}",
                shape.shard_count
            );
            for shard in shards {
                assert_eq!(
                    shard["status"].as_str(),
                    Some("DELETING"),
                    "case {case:?}: every shard goes with the cluster, got {shard}"
                );
            }

            // The accepted delete really removed the record.
            let (status, gone) = call(
                &router,
                "GET",
                &format!("/cluster/{}", label(&cluster_arn)),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: GetCluster after an accepted delete must be 404, got {status} \
                 with {gone}"
            );
            assert_eq!(
                error_code(&gone),
                "ResourceNotFoundException",
                "case {case:?}: expected a ResourceNotFoundException, got {gone}"
            );
            assert!(
                !is_listed(&router, &case, &cluster_arn).await,
                "case {case:?}: a deleted cluster must not be listed"
            );
        }
    }
}

#[tokio::test]
async fn starting_a_stopped_cluster_lets_it_be_deleted_again() {
    // The gate is escapable exactly the way the service documents: `StartCluster`
    // restarts the stopped cluster, the clock owes it its available status, and from
    // there the delete the stopped cluster refused is accepted. The rows span the same
    // cluster shapes, so the release is not a property of one record.
    let router = emulator();
    for shape in &shapes() {
        let case = shape.case;
        let name = format!("restart{}", shape.name);
        let cluster_arn = create_active_cluster(&router, case, &name, &shape.extra).await;
        stop(&router, case, &cluster_arn).await;
        let stopped = wait_for_status(&router, case, &cluster_arn, "STOPPED").await;

        let (status, response) = start(&router, &cluster_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: starting a stopped cluster must answer 200, got {status} with \
             {response}"
        );
        let starting = response["cluster"].clone();
        assert_eq!(
            starting["status"].as_str(),
            Some("STARTING"),
            "case {case:?}: the start is in flight when the call answers, so the cluster must \
             report STARTING, got {starting}"
        );
        for shard in starting["shards"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no shards in {starting}"))
        {
            assert_eq!(
                shard["status"].as_str(),
                Some("STARTING"),
                "case {case:?}: every shard moves with the cluster, got {shard}"
            );
        }
        // The start moves the status and nothing else: the configuration the create
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
                starting.get(member),
                stopped.get(member),
                "case {case:?}: `{member}` must be unchanged by the start, got {:?} where the \
                 stopped cluster has {:?}",
                starting.get(member),
                stopped.get(member)
            );
        }

        let restarted = wait_for_status(&router, case, &cluster_arn, "ACTIVE").await;
        assert_eq!(
            restarted["shardCount"],
            stopped["shardCount"],
            "case {case:?}: the restarted cluster is the same one, got {restarted}"
        );

        let (status, response) = delete(&router, &cluster_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a restarted cluster deletes, got {status} with {response}"
        );
        assert_eq!(
            response["cluster"]["status"].as_str(),
            Some("DELETING"),
            "case {case:?}: the final cluster reports DELETING, got {response}"
        );
        assert!(
            !is_listed(&router, case, &cluster_arn).await,
            "case {case:?}: the deleted cluster must not be listed"
        );
    }
}

#[tokio::test]
async fn starting_a_cluster_that_is_not_stopped_is_refused() {
    // A start acts only on a stopped cluster, so the rows span the statuses it may not
    // act from: still being created, settled available, and still stopping.
    enum Setup {
        WhileCreating,
        WhenActive,
        WhileStopping,
    }

    let router = emulator();
    let cases: &[(&str, Setup, &[&str])] = &[
        ("a cluster still being created", Setup::WhileCreating, &["CREATING", "ACTIVE"]),
        ("an available cluster", Setup::WhenActive, &["ACTIVE"]),
        ("a cluster still stopping", Setup::WhileStopping, &["STOPPING", "STOPPED"]),
    ];

    for (index, (case, setup, allowed)) in cases.iter().enumerate() {
        let name = format!("notstopped{index}");
        let cluster_arn = match setup {
            Setup::WhileCreating => create_cluster(&router, case, &name, &json!({})).await,
            Setup::WhenActive => create_active_cluster(&router, case, &name, &json!({})).await,
            Setup::WhileStopping => {
                let arn = create_active_cluster(&router, case, &name, &json!({})).await;
                stop(&router, case, &arn).await;
                arn
            }
        };

        let (status, refused) = start(&router, &cluster_arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: starting a cluster that is not stopped must answer HTTP 400, got \
             {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {refused}"
        );
        // The service states this refusal as fixed text; it interpolates nothing, so the
        // whole sentence is asserted.
        assert_eq!(
            refused["message"].as_str(),
            Some("Can't start the cluster as it's currently in state unavailable for starting."),
            "case {case:?}: expected the service's refusal sentence, got {refused}"
        );

        // The refused start wrote nothing: the cluster reports the status it was already
        // in, or the one the clock owed it whether or not anybody called.
        let stored = get_cluster(&router, case, &cluster_arn).await;
        let reported = stored["status"].as_str().unwrap_or("<no status>");
        assert!(
            allowed.contains(&reported),
            "case {case:?}: a refused start must not move the status; expected one of \
             {allowed:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn starting_a_cluster_this_service_does_not_hold_is_not_found() {
    let router = emulator();
    let unknown = format!(
        "arn:aws:docdb-elastic:{REGION}:000000000000:cluster/00000000-0000-0000-0000-000000000000"
    );
    let (status, refused) = start(&router, &unknown).await;
    assert_eq!(
        status, 404,
        "case \"an ARN no cluster carries\": expected HTTP 404, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "case \"an ARN no cluster carries\": expected a ResourceNotFoundException, got {refused}"
    );
    assert_eq!(
        refused["resourceId"].as_str(),
        Some(unknown.as_str()),
        "case \"an ARN no cluster carries\": the refusal must name the identifier the request \
         carried, got {refused}"
    );
}

#[tokio::test]
async fn the_status_gate_is_asked_of_the_stored_cluster_not_of_the_arn() {
    // Two clusters of the same shape: one stopped, one left available. The same delete
    // request form against each gets the opposite answer, so the gate is a function of the
    // stored status alone.
    let router = emulator();
    let extra = json!({"shardCount": 2, "shardCapacity": 4});
    let stopped_arn = create_active_cluster(&router, "stopped twin", "twinstopped", &extra).await;
    let active_arn = create_active_cluster(&router, "available twin", "twinactive", &extra).await;
    stop(&router, "stopped twin", &stopped_arn).await;
    wait_for_status(&router, "stopped twin", &stopped_arn, "STOPPED").await;

    let (status, refused) = delete(&router, &stopped_arn).await;
    assert_eq!(
        status, 400,
        "case \"stopped twin\": expected HTTP 400, got {status} with {refused}"
    );
    assert_eq!(
        refused["message"].as_str(),
        Some("Can't delete the cluster because it is in STOPPED state."),
        "case \"stopped twin\": expected the service's refusal sentence, got {refused}"
    );

    let (status, accepted) = delete(&router, &active_arn).await;
    assert_eq!(
        status, 200,
        "case \"available twin\": an available cluster deletes, got {status} with {accepted}"
    );

    // The refused one is still there; the accepted one is gone.
    let stored = get_cluster(&router, "stopped twin", &stopped_arn).await;
    assert_eq!(
        stored["status"].as_str(),
        Some("STOPPED"),
        "case \"stopped twin\": the preserved cluster still reports STOPPED, got {stored}"
    );
    assert!(
        !is_listed(&router, "available twin", &active_arn).await,
        "case \"available twin\": the deleted cluster must not be listed"
    );
}
