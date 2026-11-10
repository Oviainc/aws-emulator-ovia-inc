// SPDX-License-Identifier: CC0-1.0

//! Pending maintenance actions and snapshot restore, end to end over HTTP.
//!
//! These suites drive the real wire path — `MultiServiceRouter::dispatch` with a signed
//! `restJson1` request — so they exercise routing, `@httpLabel` decoding, constraint
//! validation, the handler and serialization exactly as a client would.
//!
//! The two rules they assert:
//!
//! * a maintenance-action call is answered from the state of the cluster it names. An
//!   ARN no cluster carries is the fixed cluster not-found refusal; an opt-in that needs
//!   a date and carries none is refused on the request alone; an action the cluster does
//!   not have pending is refused naming the resource the request asked about. A refused
//!   call records nothing, so the cluster's pending set is unchanged afterwards — and a
//!   cluster with nothing pending reports an EMPTY action list, not a refusal.
//! * a restore builds a NEW cluster from the snapshot's record of the cluster it was
//!   taken from. An unknown snapshot is the fixed snapshot not-found refusal and creates
//!   nothing; a cluster name already in use is refused as request validation and leaves
//!   the cluster holding that name untouched; a restore under a free name yields a
//!   distinct cluster carrying the source's configuration, with the members the request
//!   overrides taking the request's values.
//!
//! Each suite asserts its rule over a TABLE whose rows differ along what the rule
//! generalises over — which maintenance action, which opt-in type, the form of the
//! identifier, the source cluster's own configuration, which members the restore
//! overrides — never over one recorded case.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region the suites sign with.
const REGION: &str = "eu-west-2";

/// The canonical placeholder account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The maintenance actions a caller may ask to have applied.
///
/// Source: `ApplyPendingMaintenanceActionInput$applyAction` doc — "Valid actions are:
/// ENGINE_UPDATE ENGINE_UPGRADE SECURITY_UPDATE OS_UPDATE MASTER_USER_PASSWORD_UPDATE".
const MAINTENANCE_ACTIONS: &[&str] = &[
    "ENGINE_UPDATE",
    "ENGINE_UPGRADE",
    "SECURITY_UPDATE",
    "OS_UPDATE",
    "MASTER_USER_PASSWORD_UPDATE",
];

/// The opt-in types `optInType` accepts, and whether each one needs an `applyOn` date.
///
/// Source: `smithy_query(query="shape", shape="OptInType")` reports the wire values as
/// IMMEDIATE / NEXT_MAINTENANCE / APPLY_ON / UNDO_OPT_IN, and
/// `ApplyPendingMaintenanceActionInput$applyOn` is documented "Required if opt-in-type
/// is APPLY_ON".
const OPT_IN_TYPES: &[(&str, bool)] = &[
    ("IMMEDIATE", false),
    ("NEXT_MAINTENANCE", false),
    ("APPLY_ON", true),
    ("UNDO_OPT_IN", false),
];

/// A date in the documented `applyOn` form.
///
/// Source: `ApplyPendingMaintenanceActionInput$applyOn` doc — "Format: yyyy/MM/dd
/// HH:mm-yyyy/MM/dd HH:mm".
const APPLY_ON_WINDOW: &str = "2027/03/14 02:00-2027/03/14 03:00";

/// The sentence the service answers when a cluster cannot be found. The whole message
/// is fixed text: the ARN the request named is reported in `resourceId`.
const CLUSTER_NOT_FOUND: &str = "Unable to find the cluster";

/// The sentence the service answers when a snapshot cannot be found.
const SNAPSHOT_NOT_FOUND: &str = "Unable to find the snapshot";

/// The refusal of an `APPLY_ON` opt-in that carries no `applyOn`. Fixed text naming the
/// two members.
const APPLY_ON_REQUIRED: &str = "ApplyOn is required when OptInType is APPLY_ON";

/// A SigV4 `Authorization` header for this service in `region`.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}/docdb-elastic/aws4_request, \
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
        .header("authorization", authorization(REGION))
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

/// The `message` member of an error response.
fn message_of(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
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

/// Create one cluster, asserting it succeeded, and return its `cluster` object.
async fn create_cluster(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, response) = call(router, "POST", "/cluster", Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "case {body}: CreateCluster must answer 200, got {status} with {response}"
    );
    response["cluster"].clone()
}

/// Snapshot one cluster, asserting it succeeded, and return its `snapshot` object.
async fn create_snapshot(
    router: &MultiServiceRouter,
    cluster_arn: &str,
    snapshot_name: &str,
) -> Value {
    let body = json!({"clusterArn": cluster_arn, "snapshotName": snapshot_name});
    let (status, response) = call(router, "POST", "/cluster-snapshot", Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "case {body}: CreateClusterSnapshot must answer 200, got {status} with {response}"
    );
    response["snapshot"].clone()
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

/// Read the pending-maintenance view of one cluster, asserting it succeeded, and return
/// its `resourcePendingMaintenanceAction` object.
async fn get_pending_actions(router: &MultiServiceRouter, resource_arn: &str) -> Value {
    let path = format!("/pending-action/{}", label(resource_arn));
    let (status, response) = call(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case {resource_arn}: GetPendingMaintenanceAction must answer 200, got {status} \
         with {response}"
    );
    response["resourcePendingMaintenanceAction"].clone()
}

/// Assert `resource_arn` has nothing pending: the view names that ARN and carries an
/// empty action list.
async fn assert_nothing_pending(router: &MultiServiceRouter, case: &str, resource_arn: &str) {
    let view = get_pending_actions(router, resource_arn).await;
    assert_eq!(
        view["resourceArn"].as_str(),
        Some(resource_arn),
        "case {case:?}: the pending-maintenance view must name the resource it was asked \
         about, got {view}"
    );
    let details = view["pendingMaintenanceActionDetails"].as_array();
    assert!(
        details.is_some_and(|actions| actions.is_empty()),
        "case {case:?}: a cluster with nothing pending reports an EMPTY action list, got \
         {view}"
    );
}

/// Every `clusterName` `ListClusters` currently reports.
async fn listed_cluster_names(router: &MultiServiceRouter) -> Vec<String> {
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
        .map(|cluster| cluster["clusterName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Identifiers an unknown-resource lookup can name, differing along form, region,
/// account, which ARN resource segment they spell, and whether they are an ARN at all.
fn unknown_identifiers(segment: &str, other_segment: &str) -> Vec<(String, String)> {
    vec![
        (
            "an ARN of this kind in the signing region".to_string(),
            format!(
                "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:{segment}/11111111-2222-3333-4444-555555555555"
            ),
        ),
        (
            "an ARN of this kind in another region".to_string(),
            format!(
                "arn:aws:docdb-elastic:us-west-2:{ACCOUNT}:{segment}/00000000-0000-0000-0000-000000000000"
            ),
        ),
        (
            "an ARN of this kind in another account".to_string(),
            format!(
                "arn:aws:docdb-elastic:{REGION}:210987654321:{segment}/ABCDEF01-2345-6789-abcd-ef0123456789"
            ),
        ),
        (
            "an ARN spelling the other kind's resource segment".to_string(),
            format!(
                "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:{other_segment}/99999999-8888-7777-6666-555555555555"
            ),
        ),
        (
            "an identifier that is not an ARN at all".to_string(),
            "not-an-arn-at-all".to_string(),
        ),
    ]
}

/// An `ApplyPendingMaintenanceAction` body.
fn apply_body(resource_arn: &str, action: &str, opt_in_type: &str, apply_on: Option<&str>) -> Value {
    let mut body = json!({
        "resourceArn": resource_arn,
        "applyAction": action,
        "optInType": opt_in_type,
    });
    if let Some(date) = apply_on {
        body["applyOn"] = json!(date);
    }
    body
}

#[tokio::test]
async fn applying_a_maintenance_action_to_an_unknown_resource_is_the_cluster_not_found_refusal() {
    let router = emulator();
    // A cluster DOES exist, so the suite proves the refusal is about the ARN the request
    // named and not about an empty store.
    let present = create_cluster(&router, minimal_create("present")).await;
    let present_arn = present["clusterArn"].as_str().expect("an ARN").to_string();

    // The rows differ along the identifier's form AND along the opt-in type, so the
    // not-found refusal is shown to be independent of both.
    for (form, identifier) in unknown_identifiers("cluster", "cluster-snapshot") {
        for (opt_in_type, needs_date) in OPT_IN_TYPES {
            let date = needs_date.then_some(APPLY_ON_WINDOW);
            let case = format!("{form} / optInType {opt_in_type}");
            let body = apply_body(&identifier, MAINTENANCE_ACTIONS[0], opt_in_type, date);
            let (status, response) = call(&router, "POST", "/pending-action", Some(body)).await;

            assert_eq!(
                status, 404,
                "case {case:?}: an absent resource is HTTP 404, got {status} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {case:?}: expected a ResourceNotFoundException, got {response}"
            );
            assert_eq!(
                message_of(&response),
                CLUSTER_NOT_FOUND,
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
                Some("Cluster"),
                "case {case:?}: resourceType must be the AWS resource type name of the \
                 resource this operation addresses, got {response}"
            );
        }
    }

    // The cluster that does exist was untouched by any of those calls.
    assert_nothing_pending(&router, "the cluster that exists", &present_arn).await;
}

#[tokio::test]
async fn an_apply_on_opt_in_without_a_date_is_refused_on_the_request_alone() {
    let router = emulator();
    let cluster = create_cluster(&router, minimal_create("datebound")).await;
    let cluster_arn = cluster["clusterArn"].as_str().expect("an ARN").to_string();

    // The rows differ along which action is asked for: the rule is about `optInType`
    // and `applyOn`, so every action is refused the same way.
    for action in MAINTENANCE_ACTIONS {
        let case = format!("APPLY_ON with no applyOn / applyAction {action}");
        let body = apply_body(&cluster_arn, action, "APPLY_ON", None);
        let (status, response) = call(&router, "POST", "/pending-action", Some(body)).await;

        assert_eq!(
            status, 400,
            "case {case:?}: a request that cannot hold together is HTTP 400, got {status} \
             with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response}"
        );
        assert_eq!(
            message_of(&response),
            APPLY_ON_REQUIRED,
            "case {case:?}: expected the fixed applyOn-required sentence, got {response}"
        );

        // The SAME request carrying a date gets past this rule and is refused by the
        // next one instead, which is what shows the missing date was the reason.
        let with_date = apply_body(&cluster_arn, action, "APPLY_ON", Some(APPLY_ON_WINDOW));
        let (status, dated) = call(&router, "POST", "/pending-action", Some(with_date)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: with a date the call is refused for the action instead, got \
             {status} with {dated}"
        );
        assert_ne!(
            message_of(&dated),
            APPLY_ON_REQUIRED,
            "case {case:?}: a request carrying applyOn must not be refused for a missing \
             applyOn, got {dated}"
        );
    }

    assert_nothing_pending(&router, "after every refused opt-in", &cluster_arn).await;
}

#[tokio::test]
async fn applying_an_action_that_is_not_pending_is_refused_and_records_nothing() {
    let router = emulator();
    // Two clusters, so the ARN inside the refusal is shown to follow the request.
    let first = create_cluster(&router, minimal_create("first")).await;
    let second = create_cluster(
        &router,
        json!({
            "clusterName": "second", "authType": "SECRET_ARN", "adminUserName": "dbadmin",
            "adminUserPassword": "LongerPassword1", "shardCapacity": 8, "shardCount": 2,
        }),
    )
    .await;
    let arns = [
        first["clusterArn"].as_str().expect("an ARN").to_string(),
        second["clusterArn"].as_str().expect("an ARN").to_string(),
    ];

    // The rows span every documented action and every opt-in type: nothing is pending
    // for a cluster the service has just created, so each combination is refused.
    for cluster_arn in &arns {
        for action in MAINTENANCE_ACTIONS {
            for (opt_in_type, needs_date) in OPT_IN_TYPES {
                let date = needs_date.then_some(APPLY_ON_WINDOW);
                let case = format!("{cluster_arn} / {action} / {opt_in_type}");
                let body = apply_body(cluster_arn, action, opt_in_type, date);
                let (status, response) =
                    call(&router, "POST", "/pending-action", Some(body)).await;

                assert_eq!(
                    status, 400,
                    "case {case:?}: an action that is not pending is HTTP 400, got {status} \
                     with {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "ValidationException",
                    "case {case:?}: expected a ValidationException, got {response}"
                );
                assert_eq!(
                    message_of(&response),
                    format!("Pending maintenance action not found for ResourceArn {cluster_arn}"),
                    "case {case:?}: the refusal must name the resourceArn the request \
                     carried, got {response}"
                );
            }
        }
        // Nothing was recorded by any of those calls.
        assert_nothing_pending(&router, "after every refused action", cluster_arn).await;
    }
}

#[tokio::test]
async fn the_pending_maintenance_view_is_read_per_cluster() {
    let router = emulator();
    // The rows differ along the cluster's own configuration and along how many clusters
    // the store holds when the view is read.
    let cases: Vec<Value> = vec![
        minimal_create("viewalpha"),
        json!({
            "clusterName": "viewbravo", "authType": "SECRET_ARN", "adminUserName": "dbadmin2",
            "adminUserPassword": "LongerPassword1", "shardCapacity": 16, "shardCount": 3,
            "subnetIds": ["subnet-aaa"], "vpcSecurityGroupIds": ["sg-111"],
        }),
        json!({
            "clusterName": "viewcharlie", "authType": "PLAIN_TEXT", "adminUserName": "a",
            "adminUserPassword": "12345678", "shardCapacity": 64, "shardCount": 32,
        }),
    ];

    let mut arns = Vec::new();
    for case in cases {
        let cluster = create_cluster(&router, case.clone()).await;
        let arn = cluster["clusterArn"].as_str().expect("an ARN").to_string();
        assert_nothing_pending(&router, &case.to_string(), &arn).await;
        arns.push(arn);
    }
    // Every cluster still reports its OWN ARN once the store holds all of them.
    for arn in &arns {
        assert_nothing_pending(&router, "with every cluster present", arn).await;
    }

    // An ARN no cluster carries is the same fixed refusal the apply path answers.
    for (form, identifier) in unknown_identifiers("cluster", "cluster-snapshot") {
        let path = format!("/pending-action/{}", label(&identifier));
        let (status, response) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {form:?}: an absent resource is HTTP 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {form:?}: expected a ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            message_of(&response),
            CLUSTER_NOT_FOUND,
            "case {form:?}: expected the fixed cluster not-found sentence, got {response}"
        );
        assert_eq!(
            response["resourceId"].as_str(),
            Some(identifier.as_str()),
            "case {form:?}: resourceId must be the identifier the request named, got \
             {response}"
        );
    }
}

#[tokio::test]
async fn listing_pending_maintenance_actions_reports_only_the_resources_with_something_pending() {
    let router = emulator();

    // The rows differ along how many clusters the store holds and along the page size
    // asked for, including both published bounds of `maxResults` (range 1..100).
    let page_sizes: &[Option<i64>] = &[None, Some(1), Some(100)];
    for (count, name) in ["listalpha", "listbravo", "listcharlie"].iter().enumerate() {
        for page_size in page_sizes {
            let case = format!("{} cluster(s) / maxResults {page_size:?}", count + 1);
            let path = match page_size {
                Some(size) => format!("/pending-actions?maxResults={size}"),
                None => "/pending-actions".to_string(),
            };
            let (status, response) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: ListPendingMaintenanceActions must answer 200, got \
                 {status} with {response}"
            );
            let listed = response["resourcePendingMaintenanceActions"].as_array();
            assert!(
                listed.is_some(),
                "case {case:?}: the required resourcePendingMaintenanceActions member is \
                 missing from {response}"
            );
            // Every cluster in the store reports nothing pending through
            // `GetPendingMaintenanceAction`, so none of them belongs in this list.
            assert_eq!(
                listed.map(Vec::len),
                Some(0),
                "case {case:?}: a resource with nothing pending must not be listed, got \
                 {response}"
            );
            assert!(
                response.get("nextToken").is_none_or(Value::is_null),
                "case {case:?}: a page that listed everything carries no nextToken, got \
                 {response}"
            );
        }
        let cluster = create_cluster(&router, minimal_create(name)).await;
        let arn = cluster["clusterArn"].as_str().expect("an ARN").to_string();
        assert_nothing_pending(&router, name, &arn).await;
    }

    // A cursor this service did not issue is a refused request, not an empty page.
    for token in ["not-a-token", "6e6f742d612d746f6b656e"] {
        let path = format!("/pending-actions?nextToken={token}");
        let (status, response) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 400,
            "case {token:?}: a forged pagination token is HTTP 400, got {status} with \
             {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {token:?}: expected a ValidationException, got {response}"
        );
    }
}

#[tokio::test]
async fn restoring_an_unknown_snapshot_is_refused_and_creates_no_cluster() {
    let router = emulator();
    let source = create_cluster(&router, minimal_create("restoresource")).await;
    let source_arn = source["clusterArn"].as_str().expect("an ARN").to_string();
    create_snapshot(&router, &source_arn, "restoresource-snap").await;

    // The rows differ along the identifier's form; each asks for a DIFFERENT, unused
    // cluster name, so the suite can prove none of them was created.
    for (index, (form, identifier)) in unknown_identifiers("cluster-snapshot", "cluster")
        .into_iter()
        .enumerate()
    {
        let wanted_name = format!("ghost{index}");
        let path = format!("/cluster-snapshot/{}/restore", label(&identifier));
        let body = json!({"clusterName": wanted_name});
        let (status, response) = call(&router, "POST", &path, Some(body)).await;

        assert_eq!(
            status, 404,
            "case {form:?}: an absent snapshot is HTTP 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {form:?}: expected a ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            message_of(&response),
            SNAPSHOT_NOT_FOUND,
            "case {form:?}: expected the fixed snapshot not-found sentence, got {response}"
        );
        assert_eq!(
            response["resourceId"].as_str(),
            Some(identifier.as_str()),
            "case {form:?}: resourceId must be the identifier the request named, got \
             {response}"
        );
        assert_eq!(
            response["resourceType"].as_str(),
            Some("Snapshot"),
            "case {form:?}: resourceType must be the AWS resource type name of the \
             resource this operation addresses, got {response}"
        );

        let listed = listed_cluster_names(&router).await;
        assert!(
            !listed.contains(&wanted_name),
            "case {form:?}: a refused restore must create no cluster, but {wanted_name:?} \
             is listed in {listed:?}"
        );
    }

    let listed = listed_cluster_names(&router).await;
    assert_eq!(
        listed,
        vec!["restoresource".to_string()],
        "only the source cluster must exist after every refused restore, got {listed:?}"
    );
}

#[tokio::test]
async fn restoring_under_a_name_already_in_use_is_refused_and_leaves_that_cluster_untouched() {
    let router = emulator();
    let source = create_cluster(&router, minimal_create("takenone")).await;
    let source_arn = source["clusterArn"].as_str().expect("an ARN").to_string();
    let other = create_cluster(
        &router,
        json!({
            "clusterName": "takentwo", "authType": "SECRET_ARN", "adminUserName": "dbadmin",
            "adminUserPassword": "LongerPassword1", "shardCapacity": 4, "shardCount": 2,
        }),
    )
    .await;
    let other_arn = other["clusterArn"].as_str().expect("an ARN").to_string();
    let snapshot = create_snapshot(&router, &source_arn, "takenone-snap").await;
    let snapshot_arn = snapshot["snapshotArn"].as_str().expect("an ARN").to_string();

    // The rows differ along WHICH existing name is asked for and along its letter case:
    // a cluster name is stored lowercase, so a differently cased spelling names the
    // same cluster.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("the source cluster's own name", "takenone", &source_arn),
        ("the source cluster's name in upper case", "TAKENONE", &source_arn),
        ("another cluster's name", "takentwo", &other_arn),
        ("another cluster's name in mixed case", "TakenTwo", &other_arn),
    ];

    for (case, wanted_name, holder_arn) in cases {
        let holder_before = get_cluster(&router, holder_arn).await;
        let path = format!("/cluster-snapshot/{}/restore", label(&snapshot_arn));
        let body = json!({"clusterName": wanted_name});
        let (status, response) = call(&router, "POST", &path, Some(body)).await;

        assert_eq!(
            status, 400,
            "case {case:?}: a name already in use is refused as request validation \
             (HTTP 400), got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response}"
        );
        assert!(
            message_of(&response).contains(&wanted_name.to_ascii_lowercase()),
            "case {case:?}: the refusal must state the name it refused, got {response}"
        );

        // The cluster holding the name is untouched: same ARN, same configuration.
        let holder_after = get_cluster(&router, holder_arn).await;
        for member in ["clusterArn", "clusterName", "shardCapacity", "shardCount", "authType"] {
            assert_eq!(
                holder_after[member], holder_before[member],
                "case {case:?}: the cluster holding the name must be untouched, but \
                 {member} changed from {:?} to {:?}",
                holder_before[member], holder_after[member]
            );
        }
        let listed = listed_cluster_names(&router).await;
        assert_eq!(
            listed.len(),
            2,
            "case {case:?}: a refused restore must create no cluster, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_restored_cluster_is_a_new_cluster_built_from_the_snapshot() {
    // The rows differ along the SOURCE cluster's configuration and along which members
    // the restore request overrides, which is what the rule generalises over.
    struct Case {
        name: &'static str,
        source: Value,
        /// The members the restore request carries beyond `clusterName`.
        overrides: Value,
    }

    let cases = vec![
        Case {
            name: "a minimal source, nothing overridden",
            source: minimal_create("sourcealpha"),
            overrides: json!({}),
        },
        Case {
            name: "a fully configured source, nothing overridden",
            source: json!({
                "clusterName": "sourcebravo", "authType": "SECRET_ARN",
                "adminUserName": "dbadmin2", "adminUserPassword": "LongerPassword1",
                "shardCapacity": 8, "shardCount": 3, "shardInstanceCount": 2,
                "subnetIds": ["subnet-aaa", "subnet-bbb"],
                "vpcSecurityGroupIds": ["sg-111"],
                "kmsKeyId": "arn:aws:kms:eu-west-2:000000000000:key/abc",
                "preferredMaintenanceWindow": "Tue:03:30-Tue:04:00",
                "preferredBackupWindow": "01:00-01:30",
                "backupRetentionPeriod": 7,
                "tags": {"team": "data"},
            }),
            overrides: json!({}),
        },
        Case {
            name: "shard capacity and instance count overridden",
            source: json!({
                "clusterName": "sourcecharlie", "authType": "PLAIN_TEXT",
                "adminUserName": "admin", "adminUserPassword": "Secret123!",
                "shardCapacity": 2, "shardCount": 2,
            }),
            overrides: json!({"shardCapacity": 32, "shardInstanceCount": 4}),
        },
        Case {
            name: "network, key and tags overridden",
            source: json!({
                "clusterName": "sourcedelta", "authType": "PLAIN_TEXT",
                "adminUserName": "admin", "adminUserPassword": "Secret123!",
                "shardCapacity": 64, "shardCount": 1,
                "subnetIds": ["subnet-old"], "vpcSecurityGroupIds": ["sg-old"],
            }),
            overrides: json!({
                "subnetIds": ["subnet-new-1", "subnet-new-2"],
                "vpcSecurityGroupIds": ["sg-new"],
                "kmsKeyId": "arn:aws:kms:eu-west-2:000000000000:key/restored",
                "tags": {"restored": "yes"},
            }),
        },
    ];

    let router = emulator();
    for (index, case) in cases.into_iter().enumerate() {
        let label_of_case = case.name;
        let source = create_cluster(&router, case.source.clone()).await;
        let source_arn = source["clusterArn"].as_str().expect("an ARN").to_string();
        let snapshot = create_snapshot(
            &router,
            &source_arn,
            &format!("restorable-snapshot-{index}"),
        )
        .await;
        let snapshot_arn = snapshot["snapshotArn"].as_str().expect("an ARN").to_string();

        let restored_name = format!("restored{index}");
        let mut body = json!({"clusterName": restored_name});
        for (member, value) in case.overrides.as_object().expect("an object") {
            body[member] = value.clone();
        }
        let path = format!("/cluster-snapshot/{}/restore", label(&snapshot_arn));
        let (status, response) = call(&router, "POST", &path, Some(body.clone())).await;
        assert_eq!(
            status, 200,
            "case {label_of_case:?}: a restore under a free name must answer 200, got \
             {status} with {response}"
        );
        let restored = response["cluster"].clone();

        // A NEW cluster: its own name and its own ARN.
        let restored_arn = restored["clusterArn"].as_str().unwrap_or_default().to_string();
        assert!(
            restored_arn
                .starts_with(&format!("arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:cluster/")),
            "case {label_of_case:?}: the restored clusterArn is not a cluster ARN in the \
             request's region and the emulator account, got {restored_arn:?}"
        );
        assert_ne!(
            restored_arn, source_arn,
            "case {label_of_case:?}: a restore must mint a cluster distinct from its \
             source, got {restored_arn:?}"
        );
        assert_eq!(
            restored["clusterName"].as_str(),
            Some(restored_name.as_str()),
            "case {label_of_case:?}: the restored cluster takes the requested name, got \
             {restored}"
        );
        assert_eq!(
            restored["status"].as_str(),
            Some("CREATING"),
            "case {label_of_case:?}: a cluster the service is still building reports \
             CREATING, got {restored}"
        );

        // What the request did NOT carry comes from the snapshot's record of the source
        // cluster; what it DID carry wins.
        let inherited = [
            ("adminUserName", source["adminUserName"].clone()),
            ("authType", source["authType"].clone()),
            ("shardCount", source["shardCount"].clone()),
            (
                "preferredMaintenanceWindow",
                source["preferredMaintenanceWindow"].clone(),
            ),
        ];
        for (member, expected) in inherited {
            assert_eq!(
                restored[member], expected,
                "case {label_of_case:?}: {member} must be inherited from the snapshot, \
                 expected {expected:?}, got {:?}",
                restored[member]
            );
        }
        for member in [
            "shardCapacity",
            "shardInstanceCount",
            "kmsKeyId",
            "subnetIds",
            "vpcSecurityGroupIds",
        ] {
            let expected = case
                .overrides
                .get(member)
                .cloned()
                .unwrap_or_else(|| source[member].clone());
            if expected.is_null() {
                continue;
            }
            assert_eq!(
                restored[member], expected,
                "case {label_of_case:?}: {member} must be the request's value when it \
                 carries one and the snapshot's otherwise, expected {expected:?}, got {:?}",
                restored[member]
            );
        }
        assert_eq!(
            restored["shards"].as_array().map(Vec::len),
            source["shardCount"].as_u64().map(|count| count as usize),
            "case {label_of_case:?}: the restored cluster carries one shard per \
             shardCount, got {restored}"
        );

        // The restore persisted: the new cluster reads back the same way, and both
        // clusters are listed.
        let read_back = get_cluster(&router, &restored_arn).await;
        for member in [
            "clusterName",
            "clusterArn",
            "adminUserName",
            "authType",
            "shardCapacity",
            "shardCount",
            "subnetIds",
            "vpcSecurityGroupIds",
            "kmsKeyId",
            "preferredMaintenanceWindow",
            "createTime",
            "clusterEndpoint",
        ] {
            assert_eq!(
                read_back[member], restored[member],
                "case {label_of_case:?}: GetCluster must report the restored {member}, \
                 expected {:?}, got {:?}",
                restored[member], read_back[member]
            );
        }
        let listed = listed_cluster_names(&router).await;
        for expected_name in [
            case.source["clusterName"].as_str().unwrap_or_default().to_ascii_lowercase(),
            restored_name.clone(),
        ] {
            assert!(
                listed.contains(&expected_name),
                "case {label_of_case:?}: {expected_name:?} must be listed after the \
                 restore, got {listed:?}"
            );
        }
        // Nothing is pending for a cluster that has only just been restored.
        assert_nothing_pending(&router, label_of_case, &restored_arn).await;
    }
}

#[tokio::test]
async fn a_snapshot_still_restores_after_its_source_cluster_is_deleted() {
    // The configuration a restore rebuilds from travels with the SNAPSHOT, so deleting
    // the source cluster — which also frees its name — changes neither what the restore
    // produces nor that it succeeds.
    let router = emulator();
    let source = create_cluster(
        &router,
        json!({
            "clusterName": "gonesoon", "authType": "SECRET_ARN", "adminUserName": "dbadmin3",
            "adminUserPassword": "LongerPassword1", "shardCapacity": 16, "shardCount": 2,
            "shardInstanceCount": 3,
            "subnetIds": ["subnet-x", "subnet-y"], "vpcSecurityGroupIds": ["sg-x"],
            "preferredMaintenanceWindow": "Sat:22:00-Sat:22:30",
        }),
    )
    .await;
    let source_arn = source["clusterArn"].as_str().expect("an ARN").to_string();
    let snapshot = create_snapshot(&router, &source_arn, "gonesoon-snap").await;
    let snapshot_arn = snapshot["snapshotArn"].as_str().expect("an ARN").to_string();

    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/cluster/{}", label(&source_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the source cluster must delete, got {status} with {deleted}"
    );

    // The name the deleted cluster held is free again, so the restore may reuse it.
    let path = format!("/cluster-snapshot/{}/restore", label(&snapshot_arn));
    let (status, response) = call(&router, "POST", &path, Some(json!({"clusterName": "gonesoon"}))).await;
    assert_eq!(
        status, 200,
        "a restore after the source was deleted must answer 200, got {status} with \
         {response}"
    );
    let restored = response["cluster"].clone();
    assert_ne!(
        restored["clusterArn"], source["clusterArn"],
        "the restored cluster must be a new resource, got {restored}"
    );
    for member in [
        "adminUserName",
        "authType",
        "shardCapacity",
        "shardCount",
        "shardInstanceCount",
        "subnetIds",
        "vpcSecurityGroupIds",
        "preferredMaintenanceWindow",
    ] {
        assert_eq!(
            restored[member], source[member],
            "{member} must come from the snapshot's record of the deleted source, \
             expected {:?}, got {:?}",
            source[member], restored[member]
        );
    }
}
