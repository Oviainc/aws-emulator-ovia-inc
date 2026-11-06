// SPDX-License-Identifier: CC0-1.0

//! The rules DocDB Elastic applies to a cluster's creation-time configuration, end to
//! end over HTTP.
//!
//! Three rules are asserted here, each over a TABLE of inputs rather than one case:
//!
//! * a `clusterName` outside the one form the service documents for it — its length
//!   bound, its character set, its first character, a trailing or a doubled hyphen — is
//!   refused with the service's own sentence, which names the rejected name: `Invalid
//!   cluster name - <clusterName>.`;
//! * a `shardCount` outside its documented range is refused with the service's rule
//!   sentence for that member, naming the rejected value;
//! * a `preferredMaintenanceWindow` names its day of the week case-insensitively, is
//!   accepted in any case and is echoed with the day in the one spelling the service
//!   reports, alongside every other optional member of the request, which is echoed as
//!   requested and not as a service default.
//!
//! The rows differ along what each rule generalises over (which way the name is
//! malformed, the value and its distance from each published bound, the day and the
//! case it is written in, which operation validates the member), and every rule is
//! asserted on an input the issue that prompted this suite did not cite.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region the suites sign with.
const REGION: &str = "eu-west-2";

/// The days a `preferredMaintenanceWindow` may name, in the spelling the service
/// reports them in.
///
/// Source: `CreateClusterInput$preferredMaintenanceWindow` doc — "Valid days: Mon,
/// Tue, Wed, Thu, Fri, Sat, Sun".
const MAINTENANCE_DAYS: &[&str] = &["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// The largest `shardCount` a cluster may have, and the smallest.
///
/// Source: `CreateClusterInput$shardCount` doc — "The number of shards assigned to the
/// elastic cluster. Maximum is 32"; a cluster has at least one shard.
const SHARD_COUNT_BOUNDS: (i64, i64) = (1, 32);

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

/// Percent-encode one `@httpLabel` value, the way an AWS SDK does.
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

/// Assert a refusal is HTTP 400 `ValidationException` and return its `message`.
fn refusal_message(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        status, 400,
        "case {case:?}: a refused value is HTTP 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {body}"
    );
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry `message`, got {body}"))
        .to_string()
}

/// Create one cluster, asserting it succeeded, and return its `cluster` object.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, response) = call(router, "POST", "/cluster", Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateCluster must answer 200, got {status} with {response}"
    );
    response["cluster"].clone()
}

/// Read one cluster back, asserting it succeeded, and return its `cluster` object.
async fn get_cluster(router: &MultiServiceRouter, case: &str, cluster_arn: &str) -> Value {
    let path = format!("/cluster/{}", label(cluster_arn));
    let (status, response) = call(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCluster must answer 200, got {status} with {response}"
    );
    response["cluster"].clone()
}

/// Wait out the clock-owed `CREATING` -> `ACTIVE` transition of `cluster_arn`, and
/// return the settled cluster.
async fn wait_until_active(router: &MultiServiceRouter, case: &str, cluster_arn: &str) -> Value {
    for _ in 0..40 {
        let cluster = get_cluster(router, case, cluster_arn).await;
        if cluster["status"].as_str() == Some("ACTIVE") {
            return cluster;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("case {case:?}: the cluster never reached ACTIVE");
}

/// The sentence this service states for a rejected cluster name: fixed text, the name
/// exactly as the request spelled it, and a period.
fn invalid_cluster_name(rejected: &str) -> String {
    format!("Invalid cluster name - {rejected}.")
}

/// The sentence this service states for a `shardCount` outside its range.
fn shard_count_sentence(rejected: i64) -> String {
    let (min, max) = SHARD_COUNT_BOUNDS;
    format!("Invalid Input - '{rejected}'. ShardCount should be between {min} and {max}.")
}

#[tokio::test]
async fn a_malformed_cluster_name_is_refused_with_the_services_own_sentence() {
    // One row per way of breaking the single documented name form — "Must contain from
    // 1 to 63 letters, numbers, or hyphens. The first character must be a letter.
    // Cannot end with a hyphen or contain two consecutive hyphens"
    // (`CreateClusterInput$clusterName`) — plus names at and past the length bound.
    let too_long = "c".repeat(64);
    let at_the_bound = "c".repeat(63);
    let rejected: Vec<String> = vec![
        "1leading-digit".to_string(),
        "-leading-hyphen".to_string(),
        "trailing-hyphen-".to_string(),
        "double--hyphen".to_string(),
        "under_score".to_string(),
        "has space".to_string(),
        "dotted.name".to_string(),
        "Mixed_Case_Underscore".to_string(),
        String::new(),
        too_long.clone(),
    ];

    let router = emulator();
    for name in &rejected {
        let mut body = minimal_create("placeholder");
        body["clusterName"] = json!(name);
        let (status, response) = call(&router, "POST", "/cluster", Some(body)).await;
        let case = format!("CreateCluster clusterName {name:?}");
        let message = refusal_message(&case, status, &response);
        assert_eq!(
            message,
            invalid_cluster_name(name),
            "case {case:?}: expected the service's own cluster-name sentence, got {message:?}"
        );
    }

    // The same rule, validated by the other operation that names a NEW cluster: the
    // sentence does not depend on which operation checked the name.
    let source = create_ok(&router, "restore source", minimal_create("restoresource")).await;
    let source_arn = source["clusterArn"].as_str().expect("clusterArn");
    let (status, snapshot) = call(
        &router,
        "POST",
        "/cluster-snapshot",
        Some(json!({"clusterArn": source_arn, "snapshotName": "rule-snapshot"})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case restore source: the snapshot must be created, got {status} with {snapshot}"
    );
    let snapshot_arn = snapshot["snapshot"]["snapshotArn"]
        .as_str()
        .expect("snapshotArn")
        .to_string();
    let restore_path = format!("/cluster-snapshot/{}/restore", label(&snapshot_arn));
    for name in &rejected {
        let (status, response) =
            call(&router, "POST", &restore_path, Some(json!({"clusterName": name}))).await;
        let case = format!("RestoreClusterFromSnapshot clusterName {name:?}");
        let message = refusal_message(&case, status, &response);
        assert_eq!(
            message,
            invalid_cluster_name(name),
            "case {case:?}: the sentence must not depend on which operation validated the \
             name, got {message:?}"
        );
    }

    // A compliant name is still accepted, and stored as a lowercase string
    // (`CreateClusterInput$clusterName`), so the refusal is about the form and not
    // about the member at all.
    let accepted: Vec<(&str, String)> = vec![
        ("a single letter", "c".to_string()),
        ("letters and digits", "cluster42".to_string()),
        ("hyphen separated", "my-elastic-cluster".to_string()),
        ("mixed case", "MyCluster".to_string()),
        ("at the length bound", at_the_bound.clone()),
    ];
    for (case, name) in &accepted {
        let cluster = create_ok(&router, case, minimal_create(name)).await;
        assert_eq!(
            cluster["clusterName"].as_str(),
            Some(name.to_ascii_lowercase().as_str()),
            "case {case:?}: a compliant name must be accepted and echoed lower-cased, got \
             {cluster}"
        );
    }
}

#[tokio::test]
async fn an_out_of_range_shard_count_is_refused_with_the_services_own_sentence() {
    // Every published bound, one past each of them, and values well outside.
    let rejected: &[i64] = &[-5, 0, 33, 34, 64, 100];
    let accepted: &[i64] = &[1, 2, 7, 16, 32];

    let router = emulator();
    for count in rejected {
        let mut body = minimal_create(&format!("shardcount{}", count.unsigned_abs()));
        body["shardCount"] = json!(count);
        let (status, response) = call(&router, "POST", "/cluster", Some(body)).await;
        let case = format!("CreateCluster shardCount {count}");
        let message = refusal_message(&case, status, &response);
        assert_eq!(
            message,
            shard_count_sentence(*count),
            "case {case:?}: expected the service's own sentence, got {message:?}"
        );
    }

    // An in-range count is accepted, and the cluster carries one `shards` entry per
    // SHARD, not one per replica instance.
    for count in accepted {
        let case = format!("CreateCluster shardCount {count}");
        let mut body = minimal_create(&format!("ok{count}shards"));
        body["shardCount"] = json!(count);
        body["shardInstanceCount"] = json!(2);
        let cluster = create_ok(&router, &case, body).await;
        assert_eq!(
            cluster["shardCount"].as_i64(),
            Some(*count),
            "case {case:?}: the accepted count must be echoed, got {cluster}"
        );
        assert_eq!(
            cluster["shards"].as_array().map(Vec::len),
            Some(*count as usize),
            "case {case:?}: expected one shards entry per shard, got {cluster}"
        );
    }

    // The same rule, validated by the other operation that takes the member.
    let cluster = create_ok(&router, "shard count update", minimal_create("countupdate")).await;
    let cluster_arn = cluster["clusterArn"].as_str().expect("clusterArn").to_string();
    let path = format!("/cluster/{}", label(&cluster_arn));
    for count in rejected {
        let (status, response) =
            call(&router, "PUT", &path, Some(json!({"shardCount": count}))).await;
        let case = format!("UpdateCluster shardCount {count}");
        let message = refusal_message(&case, status, &response);
        assert_eq!(
            message,
            shard_count_sentence(*count),
            "case {case:?}: the sentence must not depend on which operation validated the \
             member, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_maintenance_window_names_its_day_in_any_case_and_is_echoed_in_one() {
    // The day abbreviation is case-insensitive on the way in: one row per day, each
    // written in a different case, with the window's times varying too. The expected
    // echo is the same window with each day in its documented spelling.
    let cases: Vec<(&str, String, String)> = MAINTENANCE_DAYS
        .iter()
        .enumerate()
        .map(|(index, day)| {
            let written = match index % 3 {
                0 => day.to_ascii_lowercase(),
                1 => day.to_ascii_uppercase(),
                // `mOn` — neither the documented spelling nor a uniform case.
                _ => {
                    let mut chars = day.chars();
                    let first = chars.next().expect("a three-letter day");
                    first.to_ascii_lowercase().to_string() + &chars.as_str().to_ascii_uppercase()
                }
            };
            // A different 30-minute window per row, so the times vary with the day.
            let hour = index * 3;
            let times = format!("{hour:02}:00-");
            (
                *day,
                format!("{written}:{times}{written}:{hour:02}:30"),
                format!("{day}:{times}{day}:{hour:02}:30"),
            )
        })
        .collect();

    let router = emulator();
    for (index, (day, requested, expected)) in cases.iter().enumerate() {
        let case = format!("preferredMaintenanceWindow {requested:?} ({day})");
        let mut body = minimal_create(&format!("window{index}"));
        body["preferredMaintenanceWindow"] = json!(requested);
        body["shardInstanceCount"] = json!(2);
        body["backupRetentionPeriod"] = json!(3);
        body["preferredBackupWindow"] = json!("04:00-04:30");
        let created = create_ok(&router, &case, body).await;
        assert_eq!(
            created["preferredMaintenanceWindow"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the window echoed with the day in its documented \
             spelling, got {created}"
        );

        // And the whole optional configuration round-trips through the reader, as
        // requested rather than as a service default.
        let cluster_arn = created["clusterArn"].as_str().expect("clusterArn").to_string();
        let read = get_cluster(&router, &case, &cluster_arn).await;
        assert_eq!(
            read["preferredMaintenanceWindow"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: the stored window must be the normalized one, got {read}"
        );
        assert_eq!(
            read["shardInstanceCount"].as_i64(),
            Some(2),
            "case {case:?}: shardInstanceCount must persist as requested, got {read}"
        );
        assert_eq!(
            read["backupRetentionPeriod"].as_i64(),
            Some(3),
            "case {case:?}: backupRetentionPeriod must persist as requested, got {read}"
        );
        assert_eq!(
            read["preferredBackupWindow"].as_str(),
            Some("04:00-04:30"),
            "case {case:?}: preferredBackupWindow must persist as requested, got {read}"
        );
        assert_eq!(
            read["shards"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: one shards entry per shard, whatever the replica count, got \
             {read}"
        );
    }

    // The same rule on the other operation that takes the member: the day is matched
    // case-insensitively there too and the stored value is the normalized one.
    let cluster = create_ok(&router, "window update", minimal_create("windowupdate")).await;
    let cluster_arn = cluster["clusterArn"].as_str().expect("clusterArn").to_string();
    wait_until_active(&router, "window update", &cluster_arn).await;
    let path = format!("/cluster/{}", label(&cluster_arn));
    let (status, response) = call(
        &router,
        "PUT",
        &path,
        Some(json!({"preferredMaintenanceWindow": "sUn:21:00-sUn:21:30"})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case window update: a lower-case day must be accepted, got {status} with {response}"
    );
    assert_eq!(
        response["cluster"]["preferredMaintenanceWindow"].as_str(),
        Some("Sun:21:00-Sun:21:30"),
        "case window update: expected the normalized window, got {response}"
    );

    // A day no abbreviation names is still refused, in whatever case it is written, so
    // the accept is about the case of a KNOWN day and not about the day at all.
    let refused = [
        "Fun:05:00-Fun:05:30",
        "fun:05:00-fun:05:30",
        "mo:05:00-mo:05:30",
        "monday:05:00-monday:05:30",
        "05:00-05:30",
        "mon:25:00-mon:25:30",
    ];
    for (index, window) in refused.iter().enumerate() {
        let mut body = minimal_create(&format!("badwindow{index}"));
        body["preferredMaintenanceWindow"] = json!(window);
        let (status, response) = call(&router, "POST", "/cluster", Some(body)).await;
        let case = format!("preferredMaintenanceWindow {window:?}");
        let message = refusal_message(&case, status, &response);
        assert!(
            message.starts_with(&format!("Invalid Input - '{window}'. PreferredMaintenanceWindow")),
            "case {case:?}: the refusal must state the rejected window and the member's \
             rule, got {message:?}"
        );
    }
}
