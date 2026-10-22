// SPDX-License-Identifier: CC0-1.0

//! The elastic-cluster lifecycle, end to end over HTTP.
//!
//! These suites drive the real wire path — `MultiServiceRouter::dispatch` with a signed
//! `restJson1` request — so they exercise routing, `@httpLabel` decoding, constraint
//! validation, the handler and serialization exactly as a client would.
//!
//! The rule they assert is the one the bootstrap plan names: **a cluster's response is
//! computed from the record the create persisted**, so a create followed by a
//! get/list returns what was created, an update changes only what the request carried,
//! and a delete makes the resource unreachable. Each suite asserts that over a TABLE
//! of inputs that differ along what the rule generalises over (shard capacity, shard
//! count, auth type, list/map sizes, page size, which member is updated), never over
//! one recorded case.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A SigV4 `Authorization` header for this service in `region`: the credential scope
/// is what the router routes on and what the handlers read the region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}/docdb-elastic/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// The region the suites sign with unless they are testing the region rule itself.
const REGION: &str = "eu-west-2";

/// The canonical placeholder account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The `@required` members of the `Cluster` shape.
///
/// Source: `smithy_query(query="shape", shape="Cluster")` — every member it reports
/// with `"required": true`.
const REQUIRED_CLUSTER_MEMBERS: &[&str] = &[
    "clusterName",
    "clusterArn",
    "status",
    "clusterEndpoint",
    "createTime",
    "adminUserName",
    "authType",
    "shardCapacity",
    "shardCount",
    "vpcSecurityGroupIds",
    "subnetIds",
    "preferredMaintenanceWindow",
    "kmsKeyId",
];

/// The `@required` members of the `ClusterSnapshot` shape.
///
/// Source: `smithy_query(query="shape", shape="ClusterSnapshot")`.
const REQUIRED_SNAPSHOT_MEMBERS: &[&str] = &[
    "subnetIds",
    "snapshotName",
    "snapshotArn",
    "snapshotCreationTime",
    "clusterArn",
    "clusterCreationTime",
    "status",
    "vpcSecurityGroupIds",
    "adminUserName",
    "kmsKeyId",
];

/// The values `shardCapacity` accepts.
///
/// Source: `CreateClusterInput$shardCapacity` doc — "Allowed values are 2, 4, 8, 16,
/// 32, 64".
const SHARD_CAPACITIES: &[i64] = &[2, 4, 8, 16, 32, 64];

/// The days a `preferredMaintenanceWindow` may name.
///
/// Source: `CreateClusterInput$preferredMaintenanceWindow` doc — "Valid days: Mon,
/// Tue, Wed, Thu, Fri, Sat, Sun".
const MAINTENANCE_DAYS: &[&str] = &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// The values `SnapshotType` accepts on the wire.
///
/// Source: `smithy_query(query="shape", shape="SnapshotType")`, which agrees with the
/// scaffold's own `ops::types::SNAPSHOT_TYPE_VALUES`.
const SNAPSHOT_TYPES: &[&str] = &["MANUAL", "AUTOMATED"];

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

/// Send one request in `region` and return its status and parsed JSON body
/// (`Value::Null` when the body is empty).
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://docdb-elastic.{region}.amazonaws.com{path}"))
        .header("host", format!("docdb-elastic.{region}.amazonaws.com"))
        .header("authorization", authorization(region))
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

/// Send one request in [`REGION`].
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_in(router, REGION, method, path, body).await
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

/// The wire error code of an error response (`__type`, bare or namespaced).
fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// Is `value` an ISO 8601 / RFC 3339 UTC instant, as every `*Time` member of this
/// service is documented to be ("in Universal Coordinated Time (UTC)")?
fn is_utc_instant(value: &str) -> bool {
    let Some(rest) = value.strip_suffix('Z') else {
        return false;
    };
    let Some((date, time)) = rest.split_once('T') else {
        return false;
    };
    let date_ok = date.len() == 10
        && date.split('-').count() == 3
        && date.chars().all(|c| c.is_ascii_digit() || c == '-');
    let time_ok = time.len() >= 8
        && time.split(':').count() == 3
        && time
            .chars()
            .all(|c| c.is_ascii_digit() || c == ':' || c == '.');
    date_ok && time_ok
}

/// Is `window` the documented `ddd:hh24:mi-ddd:hh24:mi` form, with both endpoints on a
/// valid day and a valid wall-clock time?
fn is_maintenance_window(window: &str) -> bool {
    let Some((start, end)) = window.split_once('-') else {
        return false;
    };
    [start, end].iter().all(|part| {
        let fields: Vec<&str> = part.split(':').collect();
        if fields.len() != 3 {
            return false;
        }
        let day_ok = MAINTENANCE_DAYS.contains(&fields[0]);
        let hour_ok = fields[1].len() == 2 && matches!(fields[1].parse::<u32>(), Ok(h) if h < 24);
        let minute_ok = fields[2].len() == 2 && matches!(fields[2].parse::<u32>(), Ok(m) if m < 60);
        day_ok && hour_ok && minute_ok
    })
}

/// Create one cluster, asserting it succeeded, and return its `cluster` object.
async fn create_ok(router: &MultiServiceRouter, body: Value) -> Value {
    let (status, response) = call(router, "POST", "/cluster", Some(body.clone())).await;
    assert_eq!(
        status, 200,
        "case {body}: expected CreateCluster to answer 200, got {status} with {response}"
    );
    response
        .get("cluster")
        .cloned()
        .unwrap_or_else(|| panic!("case {body}: no `cluster` member in {response}"))
}

/// Read one cluster back, asserting it succeeded, and return its `cluster` object.
async fn get_ok(router: &MultiServiceRouter, cluster_arn: &str) -> Value {
    let path = format!("/cluster/{}", label(cluster_arn));
    let (status, response) = call(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case {cluster_arn}: expected GetCluster to answer 200, got {status} with {response}"
    );
    response
        .get("cluster")
        .cloned()
        .unwrap_or_else(|| panic!("case {cluster_arn}: no `cluster` member in {response}"))
}

#[tokio::test]
async fn a_created_cluster_round_trips_through_get_cluster() {
    // The rows differ along everything the create persists: the published shard
    // capacities, the shard-count bounds, both auth types, and present vs absent
    // optional lists, maps and scalars.
    let cases: Vec<Value> = vec![
        minimal_create("alpha"),
        json!({
            "clusterName": "bravo", "authType": "SECRET_ARN", "adminUserName": "dbadmin2",
            "adminUserPassword": "LongerPassword1", "shardCapacity": 4, "shardCount": 2,
            "subnetIds": ["subnet-aaa", "subnet-bbb"],
            "vpcSecurityGroupIds": ["sg-111"],
            "kmsKeyId": "arn:aws:kms:eu-west-2:000000000000:key/abc",
            "preferredMaintenanceWindow": "Tue:03:30-Tue:04:00",
            "preferredBackupWindow": "01:00-01:30",
            "backupRetentionPeriod": 7,
            "shardInstanceCount": 3,
            "tags": {"team": "data", "env": "test"},
        }),
        json!({
            "clusterName": "charlie", "authType": "PLAIN_TEXT", "adminUserName": "a",
            "adminUserPassword": "12345678", "shardCapacity": 64, "shardCount": 32,
            "subnetIds": [],
            "vpcSecurityGroupIds": [],
        }),
        json!({
            "clusterName": "delta-one", "authType": "PLAIN_TEXT", "adminUserName": "admin",
            "adminUserPassword": "Secret123!", "shardCapacity": 32, "shardCount": 8,
            "subnetIds": ["subnet-1", "subnet-2", "subnet-3", "subnet-4"],
        }),
    ];

    let router = emulator();
    for case in cases {
        let created = create_ok(&router, case.clone()).await;

        for member in REQUIRED_CLUSTER_MEMBERS {
            assert!(
                created.get(member).is_some_and(|v| !v.is_null()),
                "case {case}: required Cluster member `{member}` missing from {created}"
            );
        }

        let cluster_arn = created["clusterArn"].as_str().expect("clusterArn is a string");
        assert!(
            cluster_arn.starts_with(&format!("arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:cluster/")),
            "case {case}: clusterArn is not a docdb-elastic cluster ARN in the request's \
             region and the emulator account, got {cluster_arn:?}"
        );
        let expected_name = case["clusterName"]
            .as_str()
            .expect("clusterName is a string")
            .to_ascii_lowercase();
        assert_eq!(
            created["clusterName"].as_str(),
            Some(expected_name.as_str()),
            "case {case}: clusterName is stored as a lowercase string, got {created}"
        );
        assert_eq!(
            created["status"].as_str(),
            Some("CREATING"),
            "case {case}: a cluster AWS is still building reports CREATING, got {created}"
        );
        assert!(
            is_utc_instant(created["createTime"].as_str().unwrap_or_default()),
            "case {case}: createTime is not a UTC instant, got {:?}",
            created["createTime"]
        );
        assert!(
            is_maintenance_window(
                created["preferredMaintenanceWindow"].as_str().unwrap_or_default()
            ),
            "case {case}: preferredMaintenanceWindow is not ddd:hh24:mi-ddd:hh24:mi, got {:?}",
            created["preferredMaintenanceWindow"]
        );
        let endpoint = created["clusterEndpoint"].as_str().unwrap_or_default();
        assert!(
            endpoint.starts_with(&format!("{expected_name}."))
                && endpoint.ends_with(&format!(".{REGION}.docdb-elastic.amazonaws.com")),
            "case {case}: clusterEndpoint is not derived from the cluster's own identity, \
             got {endpoint:?}"
        );
        assert_eq!(
            created["shards"].as_array().map(Vec::len),
            Some(case["shardCount"].as_i64().unwrap_or_default() as usize),
            "case {case}: the cluster carries one shard per shardCount, got {created}"
        );

        // The password is never part of any response.
        let password = case["adminUserPassword"].as_str().expect("a password");
        assert!(
            !created.to_string().contains(password)
                && created.get("adminUserPassword").is_none(),
            "case {case}: adminUserPassword leaked into the response {created}"
        );

        // The reader returns the SAME structure the create persisted.
        let fetched = get_ok(&router, cluster_arn).await;
        for member in REQUIRED_CLUSTER_MEMBERS {
            if *member == "status" {
                // The clock may already have settled CREATING -> ACTIVE by now.
                continue;
            }
            assert_eq!(
                fetched.get(member),
                created.get(member),
                "case {case}: GetCluster disagrees with CreateCluster on `{member}`: \
                 created {:?}, fetched {:?}",
                created.get(member),
                fetched.get(member)
            );
        }
        for member in ["backupRetentionPeriod", "preferredBackupWindow", "shardInstanceCount"] {
            assert_eq!(
                fetched.get(member).cloned().unwrap_or(Value::Null),
                case.get(member).cloned().unwrap_or(Value::Null),
                "case {case}: optional member `{member}` did not round-trip, got {fetched}"
            );
        }
    }
}

#[tokio::test]
async fn a_cluster_settles_from_creating_to_active_without_another_call() {
    let router = emulator();
    let created = create_ok(&router, minimal_create("settling")).await;
    let cluster_arn = created["clusterArn"].as_str().expect("clusterArn").to_string();

    // A caller acting straight after the create still sees the transient status.
    assert_eq!(
        created["status"].as_str(),
        Some("CREATING"),
        "case settling: a freshly created cluster must report CREATING, got {created}"
    );

    // The transition is owed by the clock, so it happens with no further request.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    let settled = get_ok(&router, &cluster_arn).await;
    assert_eq!(
        settled["status"].as_str(),
        Some("ACTIVE"),
        "case settling: the cluster must settle to ACTIVE once the span has elapsed, got {settled}"
    );
    assert!(
        settled["shards"]
            .as_array()
            .is_some_and(|shards| shards.iter().all(|s| s["status"] == json!("ACTIVE"))),
        "case settling: a settled cluster's shards settle with it, got {settled}"
    );
}

#[tokio::test]
async fn a_documented_constraint_the_model_does_not_express_is_refused_with_400() {
    // Every published bound, and one step past it on each side.
    let cases: Vec<(&str, Value)> = vec![
        ("shardCapacity below the smallest allowed value", json!({"shardCapacity": 1})),
        ("shardCapacity between two allowed values", json!({"shardCapacity": 3})),
        ("shardCapacity above the largest allowed value", json!({"shardCapacity": 65})),
        ("shardCount below the minimum", json!({"shardCount": 0})),
        ("shardCount above the maximum", json!({"shardCount": 33})),
        ("shardInstanceCount below the minimum", json!({"shardInstanceCount": 0})),
        ("adminUserPassword one character short", json!({"adminUserPassword": "Short12"})),
        ("adminUserPassword with a forward slash", json!({"adminUserPassword": "has/slash1"})),
        ("adminUserPassword with a double quote", json!({"adminUserPassword": "has\"quote1"})),
        ("adminUserPassword with an at symbol", json!({"adminUserPassword": "has@symbol1"})),
        ("clusterName starting with a digit", json!({"clusterName": "9lives"})),
        ("clusterName ending with a hyphen", json!({"clusterName": "trailing-"})),
        ("clusterName with two consecutive hyphens", json!({"clusterName": "a--b"})),
        ("clusterName with an illegal character", json!({"clusterName": "under_score"})),
        ("adminUserName starting with a digit", json!({"adminUserName": "1admin"})),
        ("adminUserName with a hyphen", json!({"adminUserName": "ad-min"})),
    ];
    // The allowed values themselves must pass, so the refusal is about the constraint
    // and not about the member being present at all.
    let accepted: Vec<(&str, Value)> = SHARD_CAPACITIES
        .iter()
        .map(|capacity| ("an allowed shardCapacity", json!({"shardCapacity": capacity})))
        .chain([
            ("the smallest shardCount", json!({"shardCount": 1})),
            ("the largest shardCount", json!({"shardCount": 32})),
            ("the shortest allowed password", json!({"adminUserPassword": "12345678"})),
        ])
        .collect();

    let router = emulator();
    for (index, (case, overrides)) in cases.iter().enumerate() {
        let mut body = minimal_create(&format!("refused{index}"));
        for (member, value) in overrides.as_object().expect("an object of overrides") {
            body[member] = value.clone();
        }
        let (status, response) = call(&router, "POST", "/cluster", Some(body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response}"
        );
    }
    for (index, (case, overrides)) in accepted.iter().enumerate() {
        let mut body = minimal_create(&format!("accepted{index}"));
        for (member, value) in overrides.as_object().expect("an object of overrides") {
            body[member] = value.clone();
        }
        let (status, response) = call(&router, "POST", "/cluster", Some(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {response}"
        );
    }
}

#[tokio::test]
async fn a_duplicate_cluster_name_conflicts() {
    let router = emulator();
    let first = create_ok(&router, minimal_create("twinned")).await;

    // The name is compared in its stored (lowercase) form, so a differently-cased
    // request collides too.
    for case in ["twinned", "TWINNED"] {
        let (status, response) =
            call(&router, "POST", "/cluster", Some(minimal_create(case))).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a second cluster of the same name must conflict, got {status} \
             with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected a ConflictException, got {response}"
        );
        assert_eq!(
            response["resourceId"].as_str(),
            first["clusterArn"].as_str(),
            "case {case:?}: the conflict must name the cluster already holding the name, \
             got {response}"
        );
    }
}

#[tokio::test]
async fn list_clusters_honours_max_results_and_the_next_token() {
    let router = emulator();
    let mut all_arns = Vec::new();
    for name in ["lima", "mike", "november", "oscar", "papa"] {
        let created = create_ok(&router, minimal_create(name)).await;
        all_arns.push(created["clusterArn"].as_str().expect("clusterArn").to_string());
    }
    all_arns.sort();

    // The rows differ in page size: smaller than, a divisor of, and larger than the
    // number of clusters, plus the absent-maxResults case.
    for page_size in [None, Some(1), Some(2), Some(5), Some(100)] {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(cursor) = &token {
                query.push(format!("nextToken={cursor}"));
            }
            let path = if query.is_empty() {
                "/clusters".to_string()
            } else {
                format!("/clusters?{}", query.join("&"))
            };
            let (status, response) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case maxResults={page_size:?}: expected 200, got {status} with {response}"
            );
            let page = response["clusters"]
                .as_array()
                .unwrap_or_else(|| panic!("case maxResults={page_size:?}: no clusters in {response}"));
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size as usize,
                    "case maxResults={page_size:?}: page of {} exceeds the page size, got {response}",
                    page.len()
                );
            }
            for item in page {
                for member in ["clusterName", "clusterArn", "status"] {
                    assert!(
                        item.get(member).is_some_and(|v| !v.is_null()),
                        "case maxResults={page_size:?}: ClusterInList member `{member}` \
                         missing from {item}"
                    );
                }
                seen.push(item["clusterArn"].as_str().expect("clusterArn").to_string());
            }
            pages += 1;
            match response.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        !page.is_empty(),
                        "case maxResults={page_size:?}: a page carrying a nextToken must \
                         carry items too, got {response}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
            assert!(
                pages <= all_arns.len() + 1,
                "case maxResults={page_size:?}: pagination did not terminate"
            );
        }
        seen.sort();
        assert_eq!(
            seen, all_arns,
            "case maxResults={page_size:?}: the pages together must be exactly the stored \
             clusters, once each"
        );
        let expected_pages = page_size
            .map(|size| all_arns.len().div_ceil(size as usize))
            .unwrap_or(1);
        assert_eq!(
            pages, expected_pages,
            "case maxResults={page_size:?}: expected {expected_pages} pages, walked {pages}"
        );
    }
}

#[tokio::test]
async fn list_clusters_refuses_a_page_size_outside_the_modelled_range_and_a_foreign_token() {
    let router = emulator();
    create_ok(&router, minimal_create("quebec")).await;

    // `maxResults` is modelled range(min: 1, max: 100): both published bounds pass and
    // one step past each is refused.
    for (case, size, expected) in [
        ("the modelled minimum", 1, 200),
        ("the modelled maximum", 100, 200),
        ("one below the minimum", 0, 400),
        ("one above the maximum", 101, 400),
    ] {
        let (status, response) =
            call(&router, "GET", &format!("/clusters?maxResults={size}"), None).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} for maxResults={size}, got {status} \
             with {response}"
        );
    }

    // A token this service never issued is a bad request, not an empty page.
    for case in ["not-a-token", "zzzz", "0"] {
        let (status, response) =
            call(&router, "GET", &format!("/clusters?nextToken={case}"), None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a foreign nextToken must be refused, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response}"
        );
    }
}

#[tokio::test]
async fn update_cluster_applies_only_the_members_the_request_carries() {
    // One row per updatable member: each changes that member and must leave every
    // other one exactly as it was.
    let cases: Vec<(&str, Value)> = vec![
        ("shardCapacity", json!({"shardCapacity": 16})),
        ("shardCount", json!({"shardCount": 4})),
        ("shardInstanceCount", json!({"shardInstanceCount": 2})),
        ("authType", json!({"authType": "SECRET_ARN"})),
        ("vpcSecurityGroupIds", json!({"vpcSecurityGroupIds": ["sg-new", "sg-other"]})),
        ("subnetIds", json!({"subnetIds": ["subnet-x"]})),
        ("preferredMaintenanceWindow", json!({"preferredMaintenanceWindow": "Sun:10:00-Sun:10:30"})),
        ("preferredBackupWindow", json!({"preferredBackupWindow": "22:00-22:30"})),
        ("backupRetentionPeriod", json!({"backupRetentionPeriod": 14})),
        ("adminUserPassword", json!({"adminUserPassword": "RotatedPass9"})),
        ("nothing at all", json!({})),
    ];

    let router = emulator();
    for (index, (case, patch)) in cases.iter().enumerate() {
        let before = create_ok(
            &router,
            json!({
                "clusterName": format!("updated{index}"),
                "authType": "PLAIN_TEXT",
                "adminUserName": "admin",
                "adminUserPassword": "Secret123!",
                "shardCapacity": 2,
                "shardCount": 2,
                "subnetIds": ["subnet-keep"],
                "vpcSecurityGroupIds": ["sg-keep"],
                "preferredMaintenanceWindow": "Wed:05:00-Wed:05:30",
                "preferredBackupWindow": "02:00-02:30",
                "backupRetentionPeriod": 3,
                "shardInstanceCount": 1,
            }),
        )
        .await;
        let cluster_arn = before["clusterArn"].as_str().expect("clusterArn").to_string();

        let path = format!("/cluster/{}", label(&cluster_arn));
        let (status, response) = call(&router, "PUT", &path, Some(patch.clone())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateCluster to answer 200, got {status} with {response}"
        );
        let updated = response["cluster"].clone();
        if patch.as_object().is_some_and(|members| members.is_empty()) {
            // An update that carried no updatable member changed nothing, so it must
            // not move the status either.
            assert!(
                matches!(updated["status"].as_str(), Some("CREATING" | "ACTIVE")),
                "case {case:?}: an update that changed nothing must not report UPDATING, \
                 got {updated}"
            );
        } else {
            assert_eq!(
                updated["status"].as_str(),
                Some("UPDATING"),
                "case {case:?}: an applied update reports UPDATING, got {updated}"
            );
        }

        // The change is visible to the next read, and nothing else moved.
        let fetched = get_ok(&router, &cluster_arn).await;
        for member in REQUIRED_CLUSTER_MEMBERS.iter().chain(
            ["backupRetentionPeriod", "preferredBackupWindow", "shardInstanceCount"].iter(),
        ) {
            if *member == "status" {
                continue;
            }
            let expected = match patch.get(member) {
                Some(value) => value.clone(),
                None => before.get(member).cloned().unwrap_or(Value::Null),
            };
            assert_eq!(
                fetched.get(member).cloned().unwrap_or(Value::Null),
                expected,
                "case {case:?}: `{member}` is wrong after the update: expected {expected:?}, \
                 got {:?}",
                fetched.get(member)
            );
        }
        assert_eq!(
            fetched["shards"].as_array().map(Vec::len),
            Some(fetched["shardCount"].as_i64().unwrap_or_default() as usize),
            "case {case:?}: the shard list must track shardCount, got {fetched}"
        );
        assert!(
            !fetched.to_string().contains("RotatedPass9"),
            "case {case:?}: adminUserPassword leaked into {fetched}"
        );
    }
}

#[tokio::test]
async fn update_cluster_refuses_a_value_outside_a_documented_constraint() {
    let router = emulator();
    let created = create_ok(&router, minimal_create("romeo")).await;
    let path = format!("/cluster/{}", label(created["clusterArn"].as_str().expect("clusterArn")));

    for (case, patch) in [
        ("shardCapacity between two allowed values", json!({"shardCapacity": 5})),
        ("shardCount above the maximum", json!({"shardCount": 33})),
        ("shardInstanceCount below the minimum", json!({"shardInstanceCount": 0})),
        ("adminUserPassword with an at symbol", json!({"adminUserPassword": "bad@pass1"})),
    ] {
        let (status, response) = call(&router, "PUT", &path, Some(patch)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response}"
        );
    }

    // The refused update left the cluster alone.
    let fetched = get_ok(&router, created["clusterArn"].as_str().expect("clusterArn")).await;
    assert_eq!(
        fetched["shardCapacity"], created["shardCapacity"],
        "case refused-update: a refused update must not change the record, got {fetched}"
    );
}

#[tokio::test]
async fn delete_cluster_makes_the_cluster_unreachable() {
    let router = emulator();
    let mut arns = Vec::new();
    for name in ["sierra", "tango", "uniform"] {
        let created = create_ok(&router, minimal_create(name)).await;
        arns.push(created["clusterArn"].as_str().expect("clusterArn").to_string());
    }

    for (index, cluster_arn) in arns.iter().enumerate() {
        let path = format!("/cluster/{}", label(cluster_arn));
        let (status, response) = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 200,
            "case {cluster_arn}: expected DeleteCluster to answer 200, got {status} with {response}"
        );
        assert_eq!(
            response["cluster"]["status"].as_str(),
            Some("DELETING"),
            "case {cluster_arn}: the final cluster reports DELETING, got {response}"
        );

        let (status, response) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {cluster_arn}: GetCluster after a delete must be 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {cluster_arn}: expected a ResourceNotFoundException, got {response}"
        );

        // A second delete of the same ARN finds nothing.
        let (status, response) = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 404,
            "case {cluster_arn}: a second DeleteCluster must be 404, got {status} with {response}"
        );

        let (status, listed) = call(&router, "GET", "/clusters", None).await;
        assert_eq!(status, 200, "case {cluster_arn}: ListClusters failed: {listed}");
        let remaining: Vec<&str> = listed["clusters"]
            .as_array()
            .expect("clusters")
            .iter()
            .map(|c| c["clusterArn"].as_str().expect("clusterArn"))
            .collect();
        assert!(
            !remaining.contains(&cluster_arn.as_str()),
            "case {cluster_arn}: a deleted cluster must not be listed, got {listed}"
        );
        assert_eq!(
            remaining.len(),
            arns.len() - index - 1,
            "case {cluster_arn}: exactly the undeleted clusters remain, got {listed}"
        );
    }

    // Deleting a cluster releases its name, so the name can be created again.
    for case in ["sierra", "tango", "uniform"] {
        let (status, response) =
            call(&router, "POST", "/cluster", Some(minimal_create(case))).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a deleted cluster's name must be free again, got {status} with \
             {response}"
        );
    }
}

#[tokio::test]
async fn a_snapshot_copies_the_cluster_it_names_and_round_trips() {
    // The rows differ in the cluster being snapshotted: its subnet/security-group list
    // sizes, its KMS key (given vs defaulted) and its admin user.
    let clusters: Vec<Value> = vec![
        json!({
            "clusterName": "snapsrc1", "authType": "PLAIN_TEXT", "adminUserName": "owner1",
            "adminUserPassword": "Secret123!", "shardCapacity": 2, "shardCount": 1,
            "subnetIds": ["subnet-a"], "vpcSecurityGroupIds": ["sg-a"],
            "kmsKeyId": "arn:aws:kms:eu-west-2:000000000000:key/k1",
        }),
        json!({
            "clusterName": "snapsrc2", "authType": "SECRET_ARN", "adminUserName": "owner2",
            "adminUserPassword": "Secret123!", "shardCapacity": 8, "shardCount": 3,
            "subnetIds": ["subnet-a", "subnet-b", "subnet-c"],
            "vpcSecurityGroupIds": [],
        }),
        json!({
            "clusterName": "snapsrc3", "authType": "PLAIN_TEXT", "adminUserName": "owner3",
            "adminUserPassword": "Secret123!", "shardCapacity": 4, "shardCount": 1,
        }),
    ];

    let router = emulator();
    for (index, case) in clusters.iter().enumerate() {
        let cluster = create_ok(&router, case.clone()).await;
        let cluster_arn = cluster["clusterArn"].as_str().expect("clusterArn");

        let (status, response) = call(
            &router,
            "POST",
            "/cluster-snapshot",
            Some(json!({
                "clusterArn": cluster_arn,
                "snapshotName": format!("snap{index}"),
                "tags": {"purpose": "test"},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected CreateClusterSnapshot to answer 200, got {status} with {response}"
        );
        let snapshot = response["snapshot"].clone();
        for member in REQUIRED_SNAPSHOT_MEMBERS {
            assert!(
                snapshot.get(member).is_some_and(|v| !v.is_null()),
                "case {case}: required ClusterSnapshot member `{member}` missing from {snapshot}"
            );
        }
        let snapshot_arn = snapshot["snapshotArn"].as_str().expect("snapshotArn");
        assert!(
            snapshot_arn.starts_with(&format!(
                "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:cluster-snapshot/"
            )),
            "case {case}: snapshotArn is not a docdb-elastic cluster-snapshot ARN, got \
             {snapshot_arn:?}"
        );
        assert_eq!(
            snapshot["snapshotType"].as_str(),
            Some("MANUAL"),
            "case {case}: a caller-requested snapshot is MANUAL, got {snapshot}"
        );
        // Every member the snapshot copies comes from the CLUSTER, not from the request.
        for (snapshot_member, cluster_member) in [
            ("subnetIds", "subnetIds"),
            ("vpcSecurityGroupIds", "vpcSecurityGroupIds"),
            ("adminUserName", "adminUserName"),
            ("kmsKeyId", "kmsKeyId"),
            ("clusterCreationTime", "createTime"),
            ("clusterArn", "clusterArn"),
        ] {
            assert_eq!(
                snapshot.get(snapshot_member),
                cluster.get(cluster_member),
                "case {case}: snapshot `{snapshot_member}` must be the cluster's \
                 `{cluster_member}`: cluster {:?}, snapshot {:?}",
                cluster.get(cluster_member),
                snapshot.get(snapshot_member)
            );
        }
        assert!(
            is_utc_instant(snapshot["snapshotCreationTime"].as_str().unwrap_or_default()),
            "case {case}: snapshotCreationTime is not a UTC instant, got {:?}",
            snapshot["snapshotCreationTime"]
        );

        // The stored snapshot reads back unchanged.
        let (status, fetched) = call(
            &router,
            "GET",
            &format!("/cluster-snapshot/{}", label(snapshot_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected GetClusterSnapshot to answer 200, got {status} with {fetched}"
        );
        for member in REQUIRED_SNAPSHOT_MEMBERS {
            if *member == "status" {
                continue;
            }
            assert_eq!(
                fetched["snapshot"].get(member),
                snapshot.get(member),
                "case {case}: GetClusterSnapshot disagrees with CreateClusterSnapshot on \
                 `{member}`: created {:?}, fetched {:?}",
                snapshot.get(member),
                fetched["snapshot"].get(member)
            );
        }
    }

    // A duplicate snapshot name conflicts.
    let cluster = create_ok(&router, minimal_create("snapsrc4")).await;
    let (status, response) = call(
        &router,
        "POST",
        "/cluster-snapshot",
        Some(json!({
            "clusterArn": cluster["clusterArn"],
            "snapshotName": "snap0",
        })),
    )
    .await;
    assert_eq!(
        status, 409,
        "case duplicate-snapshot-name: expected HTTP 409, got {status} with {response}"
    );
}

#[tokio::test]
async fn an_arn_this_store_never_minted_is_404_rather_than_501() {
    let router = emulator();
    let unknown_cluster =
        format!("arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:cluster/11111111-2222-3333-4444-555555555555");
    let unknown_snapshot = format!(
        "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:cluster-snapshot/66666666-7777-8888-9999-000000000000"
    );

    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        ("GetCluster", "GET", format!("/cluster/{}", label(&unknown_cluster)), None),
        (
            "UpdateCluster",
            "PUT",
            format!("/cluster/{}", label(&unknown_cluster)),
            Some(json!({"shardCount": 2})),
        ),
        ("DeleteCluster", "DELETE", format!("/cluster/{}", label(&unknown_cluster)), None),
        (
            "GetClusterSnapshot",
            "GET",
            format!("/cluster-snapshot/{}", label(&unknown_snapshot)),
            None,
        ),
        (
            "CreateClusterSnapshot",
            "POST",
            "/cluster-snapshot".to_string(),
            Some(json!({"clusterArn": unknown_cluster, "snapshotName": "orphan"})),
        ),
    ];

    for (case, method, path, body) in cases {
        let (status, response) = call(&router, method, &path, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: an unknown ARN must be 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {response}"
        );
    }

    // And an empty store lists nothing rather than failing.
    let (status, response) = call(&router, "GET", "/clusters", None).await;
    assert_eq!(status, 200, "case empty-store: ListClusters failed: {response}");
    assert_eq!(
        response["clusters"].as_array().map(Vec::len),
        Some(0),
        "case empty-store: an empty store lists no clusters, got {response}"
    );
    assert!(
        response.get("nextToken").is_none_or(Value::is_null),
        "case empty-store: the last page carries no nextToken, got {response}"
    );
}

#[tokio::test]
async fn a_caller_sized_value_is_bounded_before_it_is_stored() {
    // Nothing a caller sends may grow one stored record without limit. The rows span
    // every caller-sized member the create persists, at the bound and one past it.
    let too_many_ids: Vec<String> = (0..65).map(|i| format!("subnet-{i}")).collect();
    let at_the_id_bound: Vec<String> = (0..64).map(|i| format!("subnet-{i}")).collect();
    let too_long_id = "s".repeat(256);
    let too_many_tags: Value = Value::Object(
        (0..51)
            .map(|i| (format!("k{i}"), json!("v")))
            .collect::<serde_json::Map<String, Value>>(),
    );
    let at_the_tag_bound: Value = Value::Object(
        (0..50)
            .map(|i| (format!("k{i}"), json!("v")))
            .collect::<serde_json::Map<String, Value>>(),
    );

    let cases: Vec<(&str, Value, u16)> = vec![
        ("subnetIds at the bound", json!({"subnetIds": at_the_id_bound}), 200),
        ("subnetIds one past the bound", json!({"subnetIds": too_many_ids}), 400),
        ("a subnet id one character too long", json!({"subnetIds": [too_long_id]}), 400),
        ("an empty subnet id", json!({"subnetIds": [""]}), 400),
        (
            "vpcSecurityGroupIds one past the bound",
            json!({"vpcSecurityGroupIds": (0..65).map(|i| format!("sg-{i}")).collect::<Vec<_>>()}),
            400,
        ),
        ("tags at the bound", json!({"tags": at_the_tag_bound}), 200),
        ("tags one past the bound", json!({"tags": too_many_tags}), 400),
        ("a tag key one character too long", json!({"tags": {"k".repeat(129): "v"}}), 400),
        ("a tag value one character too long", json!({"tags": {"k": "v".repeat(257)}}), 400),
        ("a kmsKeyId one character too long", json!({"kmsKeyId": "k".repeat(2049)}), 400),
        ("a kmsKeyId at the bound", json!({"kmsKeyId": "k".repeat(2048)}), 200),
        (
            "a well-formed maintenance window",
            json!({"preferredMaintenanceWindow": "Fri:23:30-Sat:00:00"}),
            200,
        ),
        (
            "a maintenance window naming no day",
            json!({"preferredMaintenanceWindow": "23:30-00:00"}),
            400,
        ),
        (
            "a maintenance window with an hour past 23",
            json!({"preferredMaintenanceWindow": "Fri:24:00-Fri:24:30"}),
            400,
        ),
        (
            "a maintenance window that is free text",
            json!({"preferredMaintenanceWindow": "x".repeat(4096)}),
            400,
        ),
        ("a well-formed backup window", json!({"preferredBackupWindow": "00:00-00:30"}), 200),
        (
            "a backup window with a minute past 59",
            json!({"preferredBackupWindow": "01:60-02:00"}),
            400,
        ),
        ("a backup window that is free text", json!({"preferredBackupWindow": "nightly"}), 400),
    ];

    let router = emulator();
    for (index, (case, overrides, expected)) in cases.iter().enumerate() {
        let mut body = minimal_create(&format!("bounded{index}"));
        for (member, value) in overrides.as_object().expect("an object of overrides") {
            body[member] = value.clone();
        }
        let (status, response) = call(&router, "POST", "/cluster", Some(body)).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} with {response}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn the_record_count_is_capped_and_a_delete_frees_a_slot() {
    // The store lives for the life of the process, so each growth site is capped and
    // the cap is released by an explicit delete. `MAX_CLUSTERS` is 100 in
    // `src/state.rs`; `MAX_CLUSTER_SNAPSHOTS` is 500.
    let router = emulator();
    let mut arns = Vec::new();
    for index in 0..100 {
        let created = create_ok(&router, minimal_create(&format!("capped{index}"))).await;
        arns.push(created["clusterArn"].as_str().expect("clusterArn").to_string());
    }

    let (status, response) =
        call(&router, "POST", "/cluster", Some(minimal_create("overflow"))).await;
    assert_eq!(
        status, 402,
        "case at-the-cap: a create past the cap must be refused, got {status} with {response}"
    );
    assert_eq!(
        error_code(&response),
        "ServiceQuotaExceededException",
        "case at-the-cap: expected a ServiceQuotaExceededException, got {response}"
    );

    // Deleting one frees exactly one slot.
    let (status, _) = call(
        &router,
        "DELETE",
        &format!("/cluster/{}", label(&arns[0])),
        None,
    )
    .await;
    assert_eq!(status, 200, "case freed-slot: the cleanup delete failed");
    let (status, response) =
        call(&router, "POST", "/cluster", Some(minimal_create("overflow"))).await;
    assert_eq!(
        status, 200,
        "case freed-slot: a create must succeed once a slot is free, got {status} with {response}"
    );
    let (status, response) =
        call(&router, "POST", "/cluster", Some(minimal_create("overflow2"))).await;
    assert_eq!(
        status, 402,
        "case freed-slot: only one slot was freed, got {status} with {response}"
    );
}

#[tokio::test]
async fn deleting_a_snapshot_makes_it_unreachable_and_frees_its_name() {
    let router = emulator();
    let cluster = create_ok(&router, minimal_create("snapdelsrc")).await;
    let cluster_arn = cluster["clusterArn"].as_str().expect("clusterArn").to_string();

    // The rows differ in the snapshot being deleted, so the reclamation is not a
    // property of one record.
    for name in ["delsnap-a", "delsnap-b", "delsnap-c"] {
        let (status, response) = call(
            &router,
            "POST",
            "/cluster-snapshot",
            Some(json!({"clusterArn": cluster_arn, "snapshotName": name})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected CreateClusterSnapshot to answer 200, got {status} with \
             {response}"
        );
        let snapshot_arn = response["snapshot"]["snapshotArn"]
            .as_str()
            .expect("snapshotArn")
            .to_string();
        let path = format!("/cluster-snapshot/{}", label(&snapshot_arn));

        let (status, response) = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected DeleteClusterSnapshot to answer 200, got {status} with \
             {response}"
        );
        assert_eq!(
            response["snapshot"]["status"].as_str(),
            Some("DELETING"),
            "case {name:?}: the final snapshot reports DELETING, got {response}"
        );

        let (status, response) = call(&router, "GET", &path, None).await;
        assert_eq!(
            status, 404,
            "case {name:?}: GetClusterSnapshot after a delete must be 404, got {status} with \
             {response}"
        );
        let (status, response) = call(&router, "DELETE", &path, None).await;
        assert_eq!(
            status, 404,
            "case {name:?}: a second DeleteClusterSnapshot must be 404, got {status} with \
             {response}"
        );

        // The name is free again, so the same snapshot name can be taken once more.
        let (status, response) = call(
            &router,
            "POST",
            "/cluster-snapshot",
            Some(json!({"clusterArn": cluster_arn, "snapshotName": name})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a deleted snapshot's name must be free again, got {status} with \
             {response}"
        );
    }
}

#[tokio::test]
async fn the_minted_arn_and_endpoint_follow_the_requests_region() {
    // The ARN's and the endpoint's region segment come from the CALLER's SigV4
    // credential scope, so the rows differ in the region the request is signed for —
    // the rule is "whatever region the request names", not one value.
    let router = emulator();
    for (index, region) in ["eu-west-2", "ap-south-1", "us-east-1", "sa-east-1"]
        .iter()
        .enumerate()
    {
        let name = format!("regional{index}");
        let (status, response) = call_in(
            &router,
            region,
            "POST",
            "/cluster",
            Some(minimal_create(&name)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected CreateCluster to answer 200, got {status} with {response}"
        );
        let created = response["cluster"].clone();
        let cluster_arn = created["clusterArn"].as_str().expect("clusterArn");
        let fields: Vec<&str> = cluster_arn.split(':').collect();
        assert_eq!(
            fields.len(),
            6,
            "case {region:?}: a cluster ARN has six colon-separated fields, got {cluster_arn:?}"
        );
        assert_eq!(
            (fields[0], fields[1], fields[2], fields[3], fields[4]),
            ("arn", "aws", "docdb-elastic", *region, ACCOUNT),
            "case {region:?}: unexpected ARN prefix in {cluster_arn:?}"
        );
        assert!(
            fields[5].starts_with("cluster/") && fields[5].len() > "cluster/".len(),
            "case {region:?}: the ARN resource must be cluster/<id>, got {:?}",
            fields[5]
        );
        let endpoint = created["clusterEndpoint"].as_str().unwrap_or_default();
        assert!(
            endpoint.ends_with(&format!(".{region}.docdb-elastic.amazonaws.com")),
            "case {region:?}: the endpoint must name the request's region, got {endpoint:?}"
        );

        // A snapshot of it is minted in the same region.
        let (status, response) = call_in(
            &router,
            region,
            "POST",
            "/cluster-snapshot",
            Some(json!({"clusterArn": cluster_arn, "snapshotName": name})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected CreateClusterSnapshot to answer 200, got {status} with \
             {response}"
        );
        let snapshot_arn = response["snapshot"]["snapshotArn"]
            .as_str()
            .expect("snapshotArn");
        assert!(
            snapshot_arn.starts_with(&format!(
                "arn:aws:docdb-elastic:{region}:{ACCOUNT}:cluster-snapshot/"
            )),
            "case {region:?}: the snapshot ARN must name the request's region, got \
             {snapshot_arn:?}"
        );
        assert!(
            SNAPSHOT_TYPES.contains(&response["snapshot"]["snapshotType"].as_str().unwrap_or("")),
            "case {region:?}: snapshotType must be one of the modelled wire values \
             {SNAPSHOT_TYPES:?}, got {:?}",
            response["snapshot"]["snapshotType"]
        );
    }
}

#[tokio::test]
async fn a_snapshot_name_outside_its_modelled_length_is_refused() {
    // `CreateClusterSnapshotInput$snapshotName` is modelled length(1, 63): both
    // published bounds pass and one step past each is refused, so neither the record
    // nor the name index can hold an unbounded string.
    let router = emulator();
    let cluster = create_ok(&router, minimal_create("snapnamesrc")).await;
    let cluster_arn = cluster["clusterArn"].as_str().expect("clusterArn").to_string();

    for (case, name, expected) in [
        ("the shortest allowed name", "a".to_string(), 200),
        ("a name at the modelled maximum", "b".repeat(63), 200),
        ("a name one character past the maximum", "c".repeat(64), 400),
        ("a name far past the maximum", "d".repeat(4096), 400),
        ("an empty name", String::new(), 400),
    ] {
        let (status, response) = call(
            &router,
            "POST",
            "/cluster-snapshot",
            Some(json!({"clusterArn": cluster_arn, "snapshotName": name})),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} for a {}-character name, got {status} \
             with {response}",
            name.chars().count()
        );
        if expected == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn a_snapshot_of_a_cluster_that_does_not_exist_is_404_before_any_other_refusal() {
    // The referenced cluster is resolved first, so a request naming a cluster that was
    // never created is a 404 whatever else about it is also wrong.
    let router = emulator();
    let cluster = create_ok(&router, minimal_create("precedence")).await;
    let cluster_arn = cluster["clusterArn"].as_str().expect("clusterArn").to_string();
    let (status, _) = call(
        &router,
        "POST",
        "/cluster-snapshot",
        Some(json!({"clusterArn": cluster_arn, "snapshotName": "taken"})),
    )
    .await;
    assert_eq!(status, 200, "case setup: the first snapshot must be created");

    let unknown =
        format!("arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:cluster/00000000-0000-0000-0000-000000000001");
    for (case, name) in [
        ("a name nobody has taken", "free"),
        ("a name already taken on another cluster", "taken"),
    ] {
        let (status, response) = call(
            &router,
            "POST",
            "/cluster-snapshot",
            Some(json!({"clusterArn": unknown, "snapshotName": name})),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: an unknown clusterArn must be 404, got {status} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected a ResourceNotFoundException, got {response}"
        );
    }
}
