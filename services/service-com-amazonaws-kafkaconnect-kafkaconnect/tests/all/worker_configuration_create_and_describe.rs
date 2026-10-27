// SPDX-License-Identifier: CC0-1.0

//! `CreateWorkerConfiguration` and `DescribeWorkerConfiguration`, end to end over
//! HTTP.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles, and the
//! response body read back as JSON. Nothing reaches into the state struct, so a
//! handler that answered from the request instead of from state would fail here.
//!
//! The table below spans the class the rule generalises over — a description and
//! tags present or absent, and a properties file that is empty, one line, several
//! lines or kilobytes long — rather than the single configuration the issue
//! cited. The modelled bounds on `name` and `description` are exercised at each
//! published bound and one past it.
//!
//! The sibling suite's request helpers are private to its own module, so this one
//! carries its own copies of the four it needs.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::TRANSITION_SPAN;
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// A SigV4 credential scope naming this service and the region every minted ARN
/// must carry. No authenticator is registered, so the signature is never
/// checked; the scope is what routes the request and names the region.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/kafkaconnect/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`], which every ARN the service mints carries.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// One emulator: a single shared engine behind the production router.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and read back `(status, body)`.
async fn call(emulator: &MultiServiceRouter, method: &str, path: &str, body: Value) -> (u16, Value) {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://kafkaconnect.{REGION}.amazonaws.com{path}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(bytes))
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|e| panic!("a JSON response body: {e}"))
    };
    (status, value)
}

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way
/// an AWS SDK encodes a label: every character a path segment reserves
/// (including the ARN's own `:` and `/`) is escaped.
fn label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The error code a failure response carries (`__type`), or `""`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A member of a JSON object as a string, for an assertion message.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether `suffix` is the UUID AWS gives the random part of a kafkaconnect
/// resource ARN: the 8-4-4-4-12 lowercase-hex form, with RFC 4122's version
/// (4) and variant (8..b) nibbles.
fn is_uuid(suffix: &str) -> bool {
    let groups: Vec<&str> = suffix.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(want, group)| group.len() == *want)
        && groups.iter().all(|group| {
            group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
        && groups[2].starts_with('4')
        && matches!(groups[3].as_bytes().first(), Some(b'8' | b'9' | b'a' | b'b'))
}

/// One row of the worker-configuration table.
struct WorkerCase {
    name: String,
    description: Option<String>,
    /// `propertiesFileContent`: the base64 `connect-distributed.properties`
    /// body. `__sensitiveString` carries no length constraint, so every size
    /// from empty upwards is a legal request.
    properties: String,
    tags: Vec<(String, String)>,
}

/// The `CreateWorkerConfiguration` body for one row.
fn worker_request(case: &WorkerCase) -> Value {
    let mut body = json!({
        "name": case.name,
        "propertiesFileContent": case.properties,
    });
    if let Some(description) = &case.description {
        body["description"] = json!(description);
    }
    if !case.tags.is_empty() {
        body["tags"] = Value::Object(
            case.tags
                .iter()
                .map(|(key, value)| (key.clone(), json!(value)))
                .collect(),
        );
    }
    body
}

/// The table: the properties file spans empty, one line, several lines and
/// kilobytes, with and without a description and tags, and a name at the
/// modelled minimum length (`__stringMin1Max128`, len 1..128).
fn worker_cases() -> Vec<WorkerCase> {
    vec![
        WorkerCase {
            // Base64 of `key.converter=org.apache.kafka.connect.storage.StringConverter`.
            name: "wc-one-line".to_string(),
            description: None,
            properties: "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=".to_string(),
            tags: Vec::new(),
        },
        WorkerCase {
            // Base64 of a two-line properties file (`key.converter` and
            // `value.converter`), so the stored body carries a newline.
            name: "wc-many-lines".to_string(),
            description: Some("the described configuration".to_string()),
            properties: "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXIKdmFsdWUuY29udmVydGVyPW9yZy5hcGFjaGUua2Fma2EuY29ubmVjdC5zdG9yYWdlLlN0cmluZ0NvbnZlcnRlcgo=".to_string(),
            tags: vec![("owner".to_string(), "platform".to_string())],
        },
        WorkerCase {
            name: "wc-empty-properties".to_string(),
            description: Some(String::new()),
            properties: String::new(),
            tags: vec![
                ("env".to_string(), "test".to_string()),
                ("team".to_string(), "data".to_string()),
            ],
        },
        WorkerCase {
            // Kilobytes of properties: `QUJD` is base64 of `ABC`, so repeating
            // it stays a legal base64 body.
            name: "w".to_string(),
            description: Some("a name at the modelled minimum length".to_string()),
            properties: "QUJD".repeat(1024),
            tags: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn a_created_worker_configuration_is_active_at_once_and_reads_back_from_state() {
    let emulator = emulator();
    let mut arns = BTreeSet::new();

    for case in worker_cases() {
        let (status, created) = call(
            &emulator,
            "POST",
            "/v1/worker-configurations",
            worker_request(&case),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateWorkerConfiguration must succeed, got {status} {created}",
            case.name
        );

        let arn = text(&created, "workerConfigurationArn");
        let expected_prefix = format!(
            "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:worker-configuration/{}/",
            case.name
        );
        assert!(
            arn.starts_with(&expected_prefix),
            "case {:?}: expected an ARN starting {expected_prefix}, got {arn:?}",
            case.name
        );
        assert!(
            is_uuid(&arn[expected_prefix.len()..]),
            "case {:?}: expected a uuid ARN suffix, got {:?}",
            case.name,
            &arn[expected_prefix.len()..]
        );
        assert!(
            arns.insert(arn.clone()),
            "case {:?}: every worker configuration gets its own ARN, got a repeat of {arn:?}",
            case.name
        );
        assert_eq!(
            text(&created, "name"),
            case.name,
            "case {:?}: the created name must come back, got {created}",
            case.name
        );
        // `WorkerConfigurationState` has no creating value, so the create is
        // answered ACTIVE rather than with a transient state.
        assert_eq!(
            text(&created, "workerConfigurationState"),
            "ACTIVE",
            "case {:?}: a created worker configuration is ACTIVE at once, got {created}",
            case.name
        );
        assert!(
            created.get("creationTime").is_some(),
            "case {:?}: a created worker configuration carries its creationTime, got {created}",
            case.name
        );
        let summary = &created["latestRevision"];
        assert_eq!(
            summary["revision"].as_i64(),
            Some(1),
            "case {:?}: a new worker configuration is at revision 1, got {summary}",
            case.name
        );
        assert_eq!(
            summary.get("description").and_then(Value::as_str),
            case.description.as_deref(),
            "case {:?}: the revision summary carries the created description, got {summary}",
            case.name
        );
        assert!(
            summary.get("creationTime").is_some(),
            "case {:?}: the revision summary carries its creationTime, got {summary}",
            case.name
        );
        // `propertiesFileContent` is not a member of the create response, so
        // the sensitive body must not come back on the create path.
        assert!(
            summary.get("propertiesFileContent").is_none()
                && created.get("propertiesFileContent").is_none(),
            "case {:?}: a create response carries no propertiesFileContent, got {created}",
            case.name
        );

        let (status, described) = call(
            &emulator,
            "GET",
            &format!("/v1/worker-configurations/{}", label(&arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the created configuration must read back, got {status} {described}",
            case.name
        );
        assert_eq!(
            text(&described, "workerConfigurationArn"),
            arn,
            "case {:?}: the described ARN must be the created one, got {described}",
            case.name
        );
        assert_eq!(
            text(&described, "name"),
            case.name,
            "case {:?}: the described name must be the created one, got {described}",
            case.name
        );
        assert_eq!(
            text(&described, "workerConfigurationState"),
            "ACTIVE",
            "case {:?}: a described configuration is ACTIVE, got {described}",
            case.name
        );
        assert_eq!(
            described.get("description").and_then(Value::as_str),
            case.description.as_deref(),
            "case {:?}: the described description must be the created one, got {described}",
            case.name
        );
        assert!(
            described.get("creationTime").is_some(),
            "case {:?}: a described configuration carries its creationTime, got {described}",
            case.name
        );
        let revision = &described["latestRevision"];
        assert_eq!(
            revision["revision"].as_i64(),
            Some(1),
            "case {:?}: the described revision is 1, got {revision}",
            case.name
        );
        assert_eq!(
            text(revision, "propertiesFileContent"),
            case.properties,
            "case {:?}: the described revision must carry the stored properties file, got a {}-byte body",
            case.name,
            text(revision, "propertiesFileContent").len()
        );
        assert_eq!(
            revision.get("description").and_then(Value::as_str),
            case.description.as_deref(),
            "case {:?}: the described revision carries the created description, got {revision}",
            case.name
        );
    }

    // A configuration AWS answers ready stays ready: nothing the clock owes
    // moves it, and a second read answers exactly what the first did.
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
    for arn in &arns {
        let (status, first) = call(
            &emulator,
            "GET",
            &format!("/v1/worker-configurations/{}", label(arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {arn:?}: an ACTIVE configuration stays readable, got {status} {first}"
        );
        assert_eq!(
            text(&first, "workerConfigurationState"),
            "ACTIVE",
            "case {arn:?}: an ACTIVE configuration stays ACTIVE, got {first}"
        );
        let (_, second) = call(
            &emulator,
            "GET",
            &format!("/v1/worker-configurations/{}", label(arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            first, second,
            "case {arn:?}: a read must not change what the next caller sees, got {second}"
        );
    }
}

#[tokio::test]
async fn a_repeated_worker_configuration_name_is_a_conflict_whatever_the_rest_of_the_request_says() {
    let emulator = emulator();
    for case in worker_cases() {
        let (status, body) = call(
            &emulator,
            "POST",
            "/v1/worker-configurations",
            worker_request(&case),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the first create must succeed, got {status} {body}",
            case.name
        );

        // The same name with a different description, properties file and tags
        // is still the same resource, so it is a 409.
        let repeat = WorkerCase {
            name: case.name.clone(),
            description: Some("a different description".to_string()),
            properties: "ZGlmZmVyZW50".to_string(),
            tags: vec![("round".to_string(), "two".to_string())],
        };
        let (status, body) = call(
            &emulator,
            "POST",
            "/v1/worker-configurations",
            worker_request(&repeat),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {:?}: a repeated name is a 409, got {status} {body}",
            case.name
        );
        assert!(
            error_code(&body).contains("ConflictException"),
            "case {:?}: expected ConflictException, got {body}",
            case.name
        );
    }
}

#[tokio::test]
async fn the_modelled_constraints_on_a_create_worker_configuration_request_are_refused() {
    // The bounds come from the model, not from this test: `name` is
    // `__stringMin1Max128` (len 1..128) and `description` is `__stringMax1024`
    // (len 0..1024). `propertiesFileContent` is `__sensitiveString`, which
    // constrains nothing, so no length of it is refused.
    let emulator = emulator();
    let name_at_min = "n".to_string();
    let name_at_max = "n".repeat(128);
    let name_past_max = "n".repeat(129);
    let description_at_max = "d".repeat(1024);
    let description_past_max = "d".repeat(1025);

    let cases: Vec<(&str, Value, u16)> = vec![
        (
            "name at the minimum length",
            json!({"name": name_at_min, "propertiesFileContent": "QUJD"}),
            200,
        ),
        (
            "name at the maximum length",
            json!({"name": name_at_max, "propertiesFileContent": "QUJD"}),
            200,
        ),
        (
            "name one past the maximum length",
            json!({"name": name_past_max, "propertiesFileContent": "QUJD"}),
            400,
        ),
        (
            "empty name, one below the minimum length",
            json!({"name": "", "propertiesFileContent": "QUJD"}),
            400,
        ),
        (
            "description at the maximum length",
            json!({"name": "description-at-max", "description": description_at_max, "propertiesFileContent": "QUJD"}),
            200,
        ),
        (
            "description one past the maximum length",
            json!({"name": "description-past-max", "description": description_past_max, "propertiesFileContent": "QUJD"}),
            400,
        ),
        (
            "the required propertiesFileContent is absent",
            json!({"name": "no-properties"}),
            400,
        ),
        (
            "the required name is absent",
            json!({"propertiesFileContent": "QUJD"}),
            400,
        ),
    ];

    for (case, body, want) in cases {
        let (status, answered) = call(&emulator, "POST", "/v1/worker-configurations", body).await;
        assert_eq!(
            status, want,
            "case {case:?}: expected HTTP {want}, got {status} {answered}"
        );
        if want == 400 {
            assert!(
                error_code(&answered).contains("BadRequestException"),
                "case {case:?}: expected BadRequestException, got {answered}"
            );
        }
    }
}

#[tokio::test]
async fn describing_a_worker_configuration_arn_the_service_never_minted_is_a_not_found() {
    let emulator = emulator();
    let created = worker_cases().remove(0);
    let (status, body) = call(
        &emulator,
        "POST",
        "/v1/worker-configurations",
        worker_request(&created),
    )
    .await;
    assert_eq!(status, 200, "the create must succeed, got {status} {body}");
    let stored = text(&body, "workerConfigurationArn");

    // Every row is a WELL-FORMED kafkaconnect ARN this store does not hold: one
    // for a name that was never created, one of the right shape in another
    // region, the stored ARN with its uuid altered, and one naming another of
    // this service's resource kinds. A value that is not an ARN at all is
    // incorrect input instead, which `arn_label_refusals` pins.
    let unknown: Vec<(&str, String)> = vec![
        (
            "a name that was never created",
            format!(
                "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:worker-configuration/ghost/00000000-0000-4000-8000-000000000000"
            ),
        ),
        (
            "the same name in another region",
            stored.replace(REGION, "us-east-2"),
        ),
        (
            "the stored ARN with a different uuid",
            {
                let (head, last) = stored.split_at(stored.len() - 1);
                // A different final hex digit, whatever the stored one is.
                let other = if last == "0" { "1" } else { "0" };
                format!("{head}{other}")
            },
        ),
        (
            "a well-formed ARN naming another of this service's resource kinds",
            format!(
                "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:connector/ghost/00000000-0000-4000-8000-000000000000"
            ),
        ),
    ];

    for (case, arn) in unknown {
        let (status, answered) = call(
            &emulator,
            "GET",
            &format!("/v1/worker-configurations/{}", label(&arn)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: an unknown ARN is a 404, got {status} {answered}"
        );
        assert!(
            error_code(&answered).contains("NotFoundException"),
            "case {case:?}: expected NotFoundException, got {answered}"
        );
    }
}
