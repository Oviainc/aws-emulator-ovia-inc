// SPDX-License-Identifier: CC0-1.0

//! Single-writer semantics for an elastic-cluster modification, end to end over HTTP.
//!
//! The rule these suites assert: **an elastic cluster takes one modification at a
//! time**. A modification moves the cluster out of its settled `ACTIVE` status for the
//! duration of the change, and while a change is in flight the cluster is not in a
//! modifiable status — so a further `UpdateCluster` against it is refused with
//! `ConflictException` (HTTP 409, `smithy_query(query="operation",
//! operation="UpdateCluster")`) instead of being applied on top of the first. Of two
//! overlapping modifications exactly one is therefore accepted, and the cluster's
//! configuration is the accepted one's rather than a race.
//!
//! The tables below differ along what the rule generalises over: which member the
//! modification carries, the values it carries, whether the two requests touch the same
//! member or different ones, whether the second request carries an updatable member at
//! all, and which transient status the cluster is in (a change still being applied, or a
//! cluster still being created). Each suite drives the real wire path through
//! `MultiServiceRouter::dispatch`, as a client would.
//!
//! Each suite in this crate carries its own request harness (see
//! `all/cluster_lifecycle.rs`): the helpers are private to their module, so the few
//! lines below are the local copy rather than a second implementation of anything the
//! service itself owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::Duration;

/// A SigV4 `Authorization` header for this service in [`REGION`]: the credential scope
/// is what the router routes on and what the handlers read the region from.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/docdb-elastic/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// The region every suite here signs with.
const REGION: &str = "eu-west-2";

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
        .uri(format!(
            "https://docdb-elastic.{REGION}.amazonaws.com{path}"
        ))
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

/// `PUT /cluster/{clusterArn}` — one `UpdateCluster` call.
async fn update(
    router: &MultiServiceRouter,
    cluster_arn: &str,
    patch: Value,
) -> (u16, Value) {
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

/// Create one cluster and return its `clusterArn`, without waiting for it to settle.
async fn create_cluster(router: &MultiServiceRouter, name: &str) -> String {
    let (status, response) = call(router, "POST", "/cluster", Some(minimal_create(name))).await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected CreateCluster to answer 200, got {status} with {response}"
    );
    response["cluster"]["clusterArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no clusterArn in {response}"))
        .to_string()
}

/// Poll `GetCluster` until the cluster reports `ACTIVE` — the status the clock owes it a
/// short span after the call that started its current transition — and return it.
async fn wait_until_active(router: &MultiServiceRouter, cluster_arn: &str) -> Value {
    for _ in 0..100 {
        let cluster = get_cluster(router, cluster_arn).await;
        if cluster["status"].as_str() == Some("ACTIVE") {
            return cluster;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("case {cluster_arn}: the cluster never settled to ACTIVE");
}

/// Create one cluster and return its `clusterArn` once it is settled and modifiable.
async fn create_active_cluster(router: &MultiServiceRouter, name: &str) -> String {
    let cluster_arn = create_cluster(router, name).await;
    wait_until_active(router, &cluster_arn).await;
    cluster_arn
}

/// Assert `body` is the 409 access conflict for the cluster `cluster_arn`.
///
/// `ConflictException` declares `message`, `resourceId` and `resourceType` `@required`
/// (`smithy_query(query="shape", shape="ConflictException")`), so all three are checked:
/// the conflict a client branches on names the resource it is about.
fn assert_cluster_conflict(case: &str, status: u16, body: &Value, cluster_arn: &str) {
    assert_eq!(
        status, 409,
        "case {case:?}: a modification of a cluster that is already being modified must \
         answer HTTP 409, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ConflictException",
        "case {case:?}: expected a ConflictException, got {body}"
    );
    assert_eq!(
        body["resourceId"].as_str(),
        Some(cluster_arn),
        "case {case:?}: the conflict must name the cluster it is about, got {body}"
    );
    assert_eq!(
        body["resourceType"].as_str(),
        Some("Cluster"),
        "case {case:?}: the conflict must report the Cluster resource type, got {body}"
    );
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: the conflict must carry a message, got {body}"
    );
}

/// One overlapping-modification row: the change that wins, the change that arrives while
/// it is in flight, and the member whose stored value tells the two apart.
struct Overlap {
    case: &'static str,
    first: Value,
    second: Value,
    /// The member the SECOND request would have changed, and the value it carries, or
    /// `None` when the second request carries no updatable member at all.
    rejected: Option<(&'static str, Value)>,
}

#[tokio::test]
async fn a_modification_arriving_while_one_is_in_flight_is_refused_with_conflict() {
    // The rows differ along which member is modified, the values carried, whether both
    // requests touch the same member, and whether the refused request carries an
    // updatable member at all.
    let cases: Vec<Overlap> = vec![
        Overlap {
            case: "the same member, both allowed shard capacities",
            first: json!({"shardCapacity": 4}),
            second: json!({"shardCapacity": 8}),
            rejected: Some(("shardCapacity", json!(8))),
        },
        Overlap {
            case: "the same member, the published shard-capacity bounds",
            first: json!({"shardCapacity": 64}),
            second: json!({"shardCapacity": 2}),
            rejected: Some(("shardCapacity", json!(2))),
        },
        Overlap {
            case: "the same member, shard count",
            first: json!({"shardCount": 3}),
            second: json!({"shardCount": 7}),
            rejected: Some(("shardCount", json!(7))),
        },
        Overlap {
            case: "the same member, shard instance count",
            first: json!({"shardInstanceCount": 2}),
            second: json!({"shardInstanceCount": 5}),
            rejected: Some(("shardInstanceCount", json!(5))),
        },
        Overlap {
            case: "different members, capacity then subnets",
            first: json!({"shardCapacity": 16}),
            second: json!({"subnetIds": ["subnet-second"]}),
            rejected: Some(("subnetIds", json!(["subnet-second"]))),
        },
        Overlap {
            case: "different members, auth type then maintenance window",
            first: json!({"authType": "SECRET_ARN"}),
            second: json!({"preferredMaintenanceWindow": "Sun:10:00-Sun:10:30"}),
            rejected: Some((
                "preferredMaintenanceWindow",
                json!("Sun:10:00-Sun:10:30"),
            )),
        },
        Overlap {
            case: "different members, backup window then retention period",
            first: json!({"preferredBackupWindow": "22:00-22:30"}),
            second: json!({"backupRetentionPeriod": 14}),
            rejected: Some(("backupRetentionPeriod", json!(14))),
        },
        Overlap {
            case: "a second request carrying no updatable member",
            first: json!({"shardCapacity": 32}),
            second: json!({}),
            rejected: None,
        },
    ];

    let router = emulator();
    for (index, case) in cases.iter().enumerate() {
        let name = format!("inflight{index}");
        let cluster_arn = create_active_cluster(&router, &name).await;

        // The accepted modification takes the cluster out of its settled status.
        let (status, accepted) = update(&router, &cluster_arn, case.first.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: a modification of a settled cluster must be accepted, got {status} \
             with {accepted}",
            case.case
        );
        assert_eq!(
            accepted["cluster"]["status"].as_str(),
            Some("UPDATING"),
            "case {:?}: the accepted modification must report UPDATING, got {accepted}",
            case.case
        );

        // A second one, arriving while the first is still in flight, is refused.
        let (status, refused) = update(&router, &cluster_arn, case.second.clone()).await;
        assert_cluster_conflict(case.case, status, &refused, &cluster_arn);

        // The refused request wrote nothing: the cluster still carries the accepted
        // modification, so the outcome is decided rather than last-writer-wins.
        let stored = get_cluster(&router, &cluster_arn).await;
        for (member, value) in case.first.as_object().expect("a patch object") {
            assert_eq!(
                stored.get(member),
                Some(value),
                "case {:?}: `{member}` must still be the accepted modification's {value}, \
                 got {:?}",
                case.case,
                stored.get(member)
            );
        }
        if let Some((member, value)) = case.rejected.as_ref() {
            assert_ne!(
                stored.get(*member),
                Some(value),
                "case {:?}: the refused modification must not have applied `{member}`, got \
                 {stored}",
                case.case
            );
        }

        // Once the change has settled, the very same request is accepted: the refusal is
        // about the cluster being mid-change, not about the request.
        wait_until_active(&router, &cluster_arn).await;
        let (status, retried) = update(&router, &cluster_arn, case.second.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: the refused modification must be accepted once the cluster settles, \
             got {status} with {retried}",
            case.case
        );
        if let Some((member, value)) = case.rejected.as_ref() {
            let stored = get_cluster(&router, &cluster_arn).await;
            assert_eq!(
                stored.get(*member),
                Some(value),
                "case {:?}: the retried modification must apply `{member}`, got {stored}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn two_concurrent_modifications_leave_exactly_one_accepted() {
    // The rows differ in the pair of values the two concurrent calls race with: each is
    // an allowed shardCapacity, and the pairs span the published domain.
    let pairs: &[(i64, i64)] = &[(4, 8), (2, 64), (16, 32), (8, 2)];

    let router = emulator();
    for (index, (left, right)) in pairs.iter().enumerate() {
        let name = format!("concurrent{index}");
        let cluster_arn = create_active_cluster(&router, &name).await;

        // Both calls are in flight at once against the same cluster.
        let (first, second) = tokio::join!(
            update(&router, &cluster_arn, json!({"shardCapacity": left})),
            update(&router, &cluster_arn, json!({"shardCapacity": right})),
        );

        let results = [(*left, first), (*right, second)];
        let accepted: Vec<&(i64, (u16, Value))> = results
            .iter()
            .filter(|(_, (status, _))| *status == 200)
            .collect();
        let refused: Vec<&(i64, (u16, Value))> = results
            .iter()
            .filter(|(_, (status, _))| *status != 200)
            .collect();
        assert_eq!(
            accepted.len(),
            1,
            "case ({left}, {right}): exactly one concurrent UpdateCluster may be accepted, \
             got {results:?}"
        );
        assert_eq!(
            refused.len(),
            1,
            "case ({left}, {right}): exactly one concurrent UpdateCluster must be refused, \
             got {results:?}"
        );
        for (capacity, (status, body)) in &refused {
            assert_cluster_conflict(
                &format!("({left}, {right}) shardCapacity {capacity}"),
                *status,
                body,
                &cluster_arn,
            );
        }

        // The cluster carries the accepted capacity, so the refused call wrote nothing.
        let winner = accepted[0].0;
        let response = &accepted[0].1 .1;
        assert_eq!(
            response["cluster"]["shardCapacity"].as_i64(),
            Some(winner),
            "case ({left}, {right}): the accepted call must report the capacity it asked \
             for, got {response}"
        );
        let stored = get_cluster(&router, &cluster_arn).await;
        assert_eq!(
            stored["shardCapacity"].as_i64(),
            Some(winner),
            "case ({left}, {right}): the stored capacity must be the accepted call's \
             {winner}, got {stored}"
        );
    }
}

#[tokio::test]
async fn a_cluster_that_is_not_yet_settled_is_not_modifiable() {
    // A cluster still being created is in a transient status too, so the same rule
    // refuses a modification of it. The rows differ in the member the modification
    // carries.
    let patches: Vec<(&str, Value)> = vec![
        ("shardCapacity", json!({"shardCapacity": 8})),
        ("shardCount", json!({"shardCount": 4})),
        ("authType", json!({"authType": "SECRET_ARN"})),
        ("subnetIds", json!({"subnetIds": ["subnet-while-creating"]})),
        ("no updatable member", json!({})),
    ];

    let router = emulator();
    for (index, (case, patch)) in patches.iter().enumerate() {
        let name = format!("creating{index}");
        let cluster_arn = create_cluster(&router, &name).await;
        let created = get_cluster(&router, &cluster_arn).await;
        assert_eq!(
            created["status"].as_str(),
            Some("CREATING"),
            "case {case:?}: a cluster the service is still building reports CREATING, got \
             {created}"
        );

        let (status, refused) = update(&router, &cluster_arn, patch.clone()).await;
        assert_cluster_conflict(case, status, &refused, &cluster_arn);

        let stored = get_cluster(&router, &cluster_arn).await;
        for (member, value) in patch.as_object().expect("a patch object") {
            assert_ne!(
                stored.get(member),
                Some(value),
                "case {case:?}: a refused modification must not apply `{member}`, got {stored}"
            );
        }

        // The same request is accepted once the create has settled.
        wait_until_active(&router, &cluster_arn).await;
        let (status, retried) = update(&router, &cluster_arn, patch.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the modification must be accepted once the cluster is settled, \
             got {status} with {retried}"
        );
    }
}
