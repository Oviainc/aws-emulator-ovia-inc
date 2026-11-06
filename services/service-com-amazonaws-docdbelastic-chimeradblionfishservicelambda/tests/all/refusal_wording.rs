// SPDX-License-Identifier: CC0-1.0

//! How this service words a request value it refuses, over the wire.
//!
//! The rule these suites assert: a value outside the domain DocDB Elastic documents
//! for it is refused with HTTP 400 `ValidationException` whose body carries ONE
//! member — `message` — holding the service's own sentence, `Invalid Input -
//! '<subject>'. <the rule the value broke>.`; the `reason` and `fieldList` members of
//! the modelled shape, which a client reads to tell a per-field boundary failure from
//! a service rule, are absent.
//!
//! The rule generalises over the member, over the operation that validates it and
//! over the kind of value (an out-of-domain number, a malformed name, a list, a map,
//! an opaque token), so the tables below span all of those rather than the one
//! recorded case. For `shardCapacity` the sentence itself is pinned, on every value
//! across and past the published bounds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The region the suites sign with.
const REGION: &str = "eu-west-2";

/// The values `shardCapacity` accepts: the powers of two within the published bounds.
///
/// Source: `CreateClusterInput$shardCapacity` doc — "Maximum is 64. Allowed values
/// are 2, 4, 8, 16, 32, 64".
const SHARD_CAPACITIES: &[i64] = &[2, 4, 8, 16, 32, 64];

/// The sentence this service states for a `shardCapacity` outside that domain, with
/// the rejected value as the request spelled it.
fn shard_capacity_sentence(rejected: i64) -> String {
    format!("Invalid Input - '{rejected}'. ShardCapacity must be a power of 2 within [2, 64].")
}

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

/// Send one request and return its status and parsed JSON body (`Value::Null` when
/// the body is empty).
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

/// Create a cluster that must succeed and return its ARN.
async fn create_arn(router: &MultiServiceRouter, name: &str) -> String {
    let (status, response) = call(router, "POST", "/cluster", Some(minimal_create(name))).await;
    assert_eq!(
        status, 200,
        "case {name:?}: the fixture cluster must be created, got {status} with {response}"
    );
    response["cluster"]["clusterArn"]
        .as_str()
        .expect("the created cluster carries its ARN")
        .to_string()
}

/// Assert a refusal body is a `ValidationException` carrying `message` alone, and
/// return that message.
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
    let members = body
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the body must be a JSON object, got {body}"));
    // `reason` and `fieldList` are modelled, and this service sends neither: a client
    // branching on the presence of `fieldList` must classify this as a service rule,
    // not as a per-field boundary failure.
    for absent in ["reason", "fieldList"] {
        assert!(
            !members.contains_key(absent),
            "case {case:?}: the refusal must not carry `{absent}`, got {body}"
        );
    }
    let unexpected: Vec<&String> = members
        .keys()
        .filter(|key| key.as_str() != "message" && key.as_str() != "__type")
        .collect();
    assert!(
        unexpected.is_empty(),
        "case {case:?}: `message` must be the only member of the refusal, got extra \
         {unexpected:?} in {body}"
    );
    members
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry `message`, got {body}"))
        .to_string()
}

#[tokio::test]
async fn an_out_of_domain_shard_capacity_is_refused_with_the_services_own_sentence() {
    // Every published bound, and values past and between them: the rejected value is
    // the only variable part of the sentence.
    let rejected: &[i64] = &[-2, 0, 1, 3, 5, 6, 7, 12, 31, 63, 65, 100, 128];

    let router = emulator();
    for (index, capacity) in rejected.iter().enumerate() {
        let mut body = minimal_create(&format!("capacity{index}"));
        body["shardCapacity"] = json!(capacity);
        let (status, response) = call(&router, "POST", "/cluster", Some(body)).await;
        let message = refusal_message(&format!("CreateCluster shardCapacity {capacity}"), status, &response);
        assert_eq!(
            message,
            shard_capacity_sentence(*capacity),
            "case shardCapacity {capacity}: expected the service's own sentence, got \
             {message:?}"
        );
    }

    // The same rule, validated by the other operation that takes the member.
    let cluster_arn = create_arn(&router, "capacityupdate").await;
    let path = format!("/cluster/{}", label(&cluster_arn));
    for capacity in rejected {
        let (status, response) =
            call(&router, "PUT", &path, Some(json!({"shardCapacity": capacity}))).await;
        let message =
            refusal_message(&format!("UpdateCluster shardCapacity {capacity}"), status, &response);
        assert_eq!(
            message,
            shard_capacity_sentence(*capacity),
            "case UpdateCluster shardCapacity {capacity}: the sentence must not depend on \
             which operation validated the member, got {message:?}"
        );
    }

    // Each allowed value is accepted, so the refusal is about the domain and not about
    // the member being present at all.
    for (index, capacity) in SHARD_CAPACITIES.iter().enumerate() {
        let mut body = minimal_create(&format!("allowed{index}"));
        body["shardCapacity"] = json!(capacity);
        let (status, response) = call(&router, "POST", "/cluster", Some(body)).await;
        assert_eq!(
            status, 200,
            "case shardCapacity {capacity}: an allowed value must be accepted, got {status} \
             with {response}"
        );
    }
}

#[tokio::test]
async fn every_refused_value_is_stated_in_the_services_own_wording() {
    // One row per member and kind of value this service checks itself: an
    // out-of-domain number, a malformed name, a sensitive value, a map, a list, a
    // time window and an opaque pagination token.
    let router = emulator();
    let cluster_arn = create_arn(&router, "wording").await;
    let cluster_path = format!("/cluster/{}", label(&cluster_arn));
    let oversized_tag_key = "k".repeat(129);

    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "CreateCluster shardCount above the maximum",
            "POST",
            "/cluster".to_string(),
            Some(json!({"shardCount": 33})),
        ),
        (
            "CreateCluster shardCount below the minimum",
            "POST",
            "/cluster".to_string(),
            Some(json!({"shardCount": 0})),
        ),
        (
            "CreateCluster shardInstanceCount below the minimum",
            "POST",
            "/cluster".to_string(),
            Some(json!({"shardInstanceCount": 0})),
        ),
        // `clusterName` is NOT a row here: the service words a rejected cluster name
        // in its own sentence rather than in the `Invalid Input - …` form this table
        // asserts, and that sentence is pinned in `create_cluster_service_rules`.
        (
            "CreateCluster adminUserName starting with a digit",
            "POST",
            "/cluster".to_string(),
            Some(json!({"adminUserName": "1admin"})),
        ),
        (
            "CreateCluster adminUserPassword too short",
            "POST",
            "/cluster".to_string(),
            Some(json!({"adminUserPassword": "Short12"})),
        ),
        (
            "CreateCluster adminUserPassword with a forbidden character",
            "POST",
            "/cluster".to_string(),
            Some(json!({"adminUserPassword": "has/slash1"})),
        ),
        (
            "CreateCluster tags with an oversized key",
            "POST",
            "/cluster".to_string(),
            Some(json!({"tags": {oversized_tag_key.clone(): "v"}})),
        ),
        (
            "CreateCluster subnetIds with an empty entry",
            "POST",
            "/cluster".to_string(),
            Some(json!({"subnetIds": [""]})),
        ),
        (
            "CreateCluster preferredBackupWindow in the wrong form",
            "POST",
            "/cluster".to_string(),
            Some(json!({"preferredBackupWindow": "2am-3am"})),
        ),
        (
            "CreateCluster preferredMaintenanceWindow naming no day",
            "POST",
            "/cluster".to_string(),
            Some(json!({"preferredMaintenanceWindow": "05:00-05:30"})),
        ),
        (
            "CreateCluster kmsKeyId empty",
            "POST",
            "/cluster".to_string(),
            Some(json!({"kmsKeyId": ""})),
        ),
        (
            "UpdateCluster shardCount above the maximum",
            "PUT",
            cluster_path.clone(),
            Some(json!({"shardCount": 33})),
        ),
        (
            "UpdateCluster vpcSecurityGroupIds with an empty entry",
            "PUT",
            cluster_path.clone(),
            Some(json!({"vpcSecurityGroupIds": [""]})),
        ),
        (
            // The snapshot route validates its tag map in the handler: the model
            // bounds a tag key and value but not how many entries the map holds.
            "CreateClusterSnapshot tags with more entries than the limit",
            "POST",
            "/cluster-snapshot".to_string(),
            Some(json!({
                "clusterArn": cluster_arn.clone(),
                "snapshotName": "wording-snapshot",
                "tags": (0..51)
                    .map(|n| (format!("key{n}"), json!("v")))
                    .collect::<serde_json::Map<String, Value>>(),
            })),
        ),
        (
            "ListClusters nextToken this service never issued",
            "GET",
            "/clusters?nextToken=not-a-token".to_string(),
            None,
        ),
    ];

    for (index, (case, method, path, patch)) in cases.iter().enumerate() {
        let body = match (*method, patch) {
            // A create carries every required member; the row overrides what it tests.
            ("POST", Some(patch)) if path == "/cluster" => {
                let mut body = minimal_create(&format!("wording{index}"));
                for (member, value) in patch.as_object().expect("an object of overrides") {
                    body[member] = value.clone();
                }
                Some(body)
            }
            (_, other) => other.clone(),
        };
        let (status, response) = call(&router, method, path, body).await;
        let message = refusal_message(case, status, &response);

        assert!(
            message.starts_with("Invalid Input - '"),
            "case {case:?}: the refusal must open with this service's own framing, got \
             {message:?}"
        );
        assert!(
            message.contains("'. ") && message.ends_with('.'),
            "case {case:?}: the refusal must state the subject and then the rule as one \
             sentence, got {message:?}"
        );
        assert!(
            !message.contains("validation error detected"),
            "case {case:?}: the refusal must not use the generic constraint wording, got \
             {message:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_password_is_never_stated_back() {
    // The member is `@sensitive`, so the subject of its refusal is the member name:
    // the rejected password must appear nowhere in the response.
    let router = emulator();
    let passwords = ["Short12", "has/slash1", "has\"quote1", "has@symbol1"];

    for (index, password) in passwords.iter().enumerate() {
        let mut body = minimal_create(&format!("secret{index}"));
        body["adminUserPassword"] = json!(password);
        let (status, response) = call(&router, "POST", "/cluster", Some(body)).await;
        let message = refusal_message(&format!("adminUserPassword {password:?}"), status, &response);
        assert!(
            !message.contains(password),
            "case {password:?}: a sensitive value must not be stated back, got {message:?}"
        );
        assert!(
            message.starts_with("Invalid Input - 'adminUserPassword'. AdminUserPassword must "),
            "case {password:?}: the refusal must name the member and state its rule, got \
             {message:?}"
        );
    }
}
