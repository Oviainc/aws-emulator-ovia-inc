// SPDX-License-Identifier: CC0-1.0

//! The KMS key an EBS `StartSnapshot` names, resolved through KMS.
//!
//! RULE: `StartSnapshotRequest$KmsKeyArn` is a reference to a resource ANOTHER service
//! holds — all the model constrains is the ARN's form
//! (`smithy_query(query="shape",shape="com.amazonaws.ebs#StartSnapshotRequest")` reports
//! `pattern ^arn:aws[a-z\-]*:kms:.*:[0-9]{12}:key/`) — so EBS resolves it through KMS
//! BEFORE creating a snapshot that claims to be encrypted under it. A key KMS does not
//! hold is the dependent-service refusal: HTTP 400 `ValidationException` with `Reason`
//! `INVALID_DEPENDENCY_REQUEST`, whose `Message` wraps the answer KMS gave — the exception
//! class, its sentence, the status its class is answered with and the identifier of that
//! answer. A snapshot created regardless would leave the caller believing it holds a
//! snapshot encrypted under a key that does not exist, and the failure would surface
//! later, or never.
//!
//! Two services have to agree for this to hold — EBS asks and KMS answers — so this is the
//! suite that can pin it: the EBS crate's own tests see no KMS at all, and an emulator
//! assembled without one accepts the reference as given.
//!
//! The rows span what the rule generalises over: which key the reference names (one KMS
//! holds, a second one it holds, and three it does not, differing in the key id), and the
//! encryption requests that name NO key, which have nothing to resolve.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every generated emulator presents, which is the account a key ARN this
/// endpoint could hold names.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region every request here signs for. A KMS key is a regional resource, so the ARN
/// the snapshot names and the key KMS holds must agree on it.
const REGION: &str = "us-west-2";

/// The long-term key every emulator publishes, which is the account's own ambient
/// identity.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The `X-Amz-Target` prefix of KMS's awsJson1_1 wire.
const KMS_TARGET_PREFIX: &str = "TrentService";

/// How EBS introduces a dependent service's refusal, and the package the relayed KMS
/// exception class is named under.
const DEPENDENCY_PREFIX: &str = "Invalid parameters for a dependent service:";
const KMS_NOT_FOUND_CLASS: &str = "com.amazonaws.services.kms.model.NotFoundException";

/// The `Reason` a reference another service refused is reported under, a member of
/// `ValidationExceptionReason`.
const DEPENDENCY_REASON: &str = "INVALID_DEPENDENCY_REQUEST";

/// The two services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Nothing here signs: the `Authorization` header carries only the credential scope,
    // which is where a service reads the Region and the caller's access key id.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ebs_ebs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={AMBIENT_KEY}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One KMS call on its awsJson1_1 wire, returning `(status, body)`.
async fn kms(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("kms.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{KMS_TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization("kms"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
}

/// One EBS `StartSnapshot` on its restJson1 wire, returning `(status, error code, body)`.
async fn start_snapshot(router: &MultiServiceRouter, body: &Value) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/snapshots")
        .header("host", format!("ebs.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("ebs"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .unwrap_or_default();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, error_code, parsed)
}

/// Create a symmetric encryption key through KMS's own `CreateKey` and answer its ARN.
async fn create_key(router: &MultiServiceRouter, description: &str) -> String {
    let (status, created) = kms(router, "CreateKey", &json!({ "Description": description })).await;
    assert_eq!(
        status, 200,
        "case {description:?}: CreateKey must succeed, got {status} {created}"
    );
    created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {description:?}: CreateKey reports an Arn, got {created}"))
        .to_string()
}

/// A key ARN of this endpoint's Region and account naming the key id `key_id`, which is
/// the form `KmsKeyArn`'s pattern admits.
fn key_arn(key_id: &str) -> String {
    format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{key_id}")
}

/// A member of a response body, or the empty string when it is absent.
fn member(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn start_snapshot_refuses_a_kms_key_that_kms_does_not_hold() {
    let router = build_router();

    // Rows differ in the key id the reference names; none was ever issued by this
    // endpoint, and each is a well-formed key id, so the refusal is about the key not
    // existing rather than about the identifier's form.
    let unknown_key_ids: &[&str] = &[
        "00000000-0000-0000-0000-000000000000",
        "11111111-2222-3333-4444-555555555555",
        "9f8e7d6c-5b4a-3928-1706-fedcba987654",
    ];

    for key_id in unknown_key_ids {
        let arn = key_arn(key_id);
        let (status, error_code, body) = start_snapshot(
            &router,
            &json!({ "VolumeSize": 1, "Encrypted": true, "KmsKeyArn": arn.clone() }),
        )
        .await;

        assert_eq!(
            status, 400,
            "case {key_id:?}: a key KMS does not hold must be refused 400, got {status} {body}"
        );
        assert!(
            error_code.ends_with("ValidationException"),
            "case {key_id:?}: expected the ValidationException error code, got {error_code:?}"
        );
        assert_eq!(
            member(&body, "Reason"),
            DEPENDENCY_REASON,
            "case {key_id:?}: expected Reason {DEPENDENCY_REASON}, got {body}"
        );

        // The message wraps KMS's own answer: the class it refused under, its sentence
        // naming the key ARN the request supplied, and the clause naming the service, the
        // status of that class, the error code and the identifier of the answer.
        let message = member(&body, "Message");
        let expected_opening =
            format!("{DEPENDENCY_PREFIX} {KMS_NOT_FOUND_CLASS}: Key '{arn}' does not exist (");
        assert!(
            message.starts_with(&expected_opening),
            "case {key_id:?}: expected the message to open {expected_opening:?}, got {message:?}"
        );
        assert!(
            message.contains("(Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; \
                 Request ID: ")
                && message.ends_with("; Proxy: null)"),
            "case {key_id:?}: expected the relayed clause naming the dependency, got {message:?}"
        );
        assert!(
            body.get("SnapshotId").is_none(),
            "case {key_id:?}: a refused StartSnapshot must create no snapshot, got {body}"
        );
    }
}

#[tokio::test]
async fn start_snapshot_accepts_a_kms_key_that_kms_holds() {
    let router = build_router();

    // Rows: two keys KMS really holds (so the acceptance is about the key resolving, not
    // about the first create being special), plus the two encryption requests that name
    // no key at all and so have no reference to resolve.
    let first = create_key(&router, "the first snapshot key").await;
    let second = create_key(&router, "the second snapshot key").await;
    let cases: &[(&str, Value, Option<&str>)] = &[
        (
            "a key KMS holds",
            json!({ "VolumeSize": 1, "KmsKeyArn": first.clone() }),
            Some("sse-kms"),
        ),
        (
            "a second key KMS holds",
            json!({ "VolumeSize": 2, "KmsKeyArn": second.clone() }),
            Some("sse-kms"),
        ),
        (
            "encryption with no key named",
            json!({ "VolumeSize": 1, "Encrypted": true }),
            Some("sse-ebs"),
        ),
        (
            "no encryption at all",
            json!({ "VolumeSize": 1 }),
            None,
        ),
    ];

    for (case, request, expected_sse) in cases {
        let (status, error_code, body) = start_snapshot(&router, request).await;
        assert_eq!(
            status, 201,
            "case {case:?}: StartSnapshot must answer 201, got {status} {error_code} {body}"
        );
        assert_eq!(
            body.get("SseType").and_then(Value::as_str),
            *expected_sse,
            "case {case:?}: SseType must follow the encryption inputs, got {body}"
        );
        assert_eq!(
            body.get("KmsKeyArn").and_then(Value::as_str),
            request.get("KmsKeyArn").and_then(Value::as_str),
            "case {case:?}: the stored key reference must be reported back, got {body}"
        );

        // Round trip: the snapshot was really created, which is what the refusal above
        // must prevent. The read path sees a snapshot once it is sealed, so it is sealed
        // with the count of blocks written to it — none.
        let snapshot_id = body
            .get("SnapshotId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: StartSnapshot reports a SnapshotId, got {body}"))
            .to_string();
        let sealed = http::Request::builder()
            .method("POST")
            .uri(format!("/snapshots/completion/{snapshot_id}"))
            .header("host", format!("ebs.{REGION}.amazonaws.com"))
            .header("x-amz-ChangedBlocksCount", "0")
            .header("authorization", authorization("ebs"))
            .body(Bytes::new())
            .expect("the test request builds");
        assert_eq!(
            router.dispatch(sealed).await.status().as_u16(),
            202,
            "case {case:?}: the created snapshot must be completable"
        );
        let listed = http::Request::builder()
            .method("GET")
            .uri(format!("/snapshots/{snapshot_id}/blocks"))
            .header("host", format!("ebs.{REGION}.amazonaws.com"))
            .header("authorization", authorization("ebs"))
            .body(Bytes::new())
            .expect("the test request builds");
        assert_eq!(
            router.dispatch(listed).await.status().as_u16(),
            200,
            "case {case:?}: the created snapshot must be readable back"
        );
    }
}
