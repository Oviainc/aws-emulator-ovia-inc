// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the key a vector store is encrypted under being CONSULTED, driven
//! over the real `restJson1` wire through the router production uses, beside the KMS
//! service on one shared engine — so the key every case acts on is the record KMS's own
//! `CreateKey`, `DisableKey`, `EnableKey` and `ScheduleKeyDeletion` handlers wrote.
//!
//! THE RULE: a vector bucket or vector index whose effective encryption configuration names
//! a customer managed KMS key is created THROUGH that key, so the creating call uses the
//! key before anything exists: a key KMS will not use (one in a key state that refuses
//! cryptographic operations) refuses the create with `AccessDeniedException` (HTTP 403) and
//! creates nothing, while a key KMS will use leaves the create to be served and the
//! resource reports the key it was created under. The refusal carries KMS's own key-state
//! sentence when the REQUEST named the key, and the asynchronous-indexing sentence when the
//! index inherits its bucket's key, each closing with the id of the KMS call this service
//! made on the caller's behalf.
//!
//! The tables span what the rule generalises over: the operation (CreateVectorBucket and
//! CreateIndex), where the key came from (named by the request, inherited from the parent
//! bucket), the key state that refuses (a disabled key, a key awaiting deletion), the key
//! state that serves (a key never moved, and one disabled and then enabled again), and the
//! encryption configurations that reach no customer key at all.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_s3vectors_s3vectors::state::{BUCKET_NOT_FOUND, INDEX_NOT_FOUND};
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_lifecycle::{call, error_code, REGION};

/// The access-key id every call below is signed with: one neither STS nor IAM holds a
/// record of, which is the position an AWS account root credential is in.
const ACCOUNT_KEY: &str = "AKIDTESTS3VECTORS";

/// The sentence this service words a refusal with when the key is one the vector index
/// INHERITS from its bucket: the asynchronous indexing it performs for the index cannot
/// reach the key.
const INSUFFICIENT_INDEXING_ACCESS: &str = "Insufficient access to perform asynchronous \
indexing. Ensure that the key used to encrypt the index still exists, is active, and has a \
resource policy granting access to the S3 service principal \
'indexing.s3vectors.amazonaws.com'.";

/// The dimensions and metric every index a case creates carries, which this rule is
/// indifferent to.
fn index_body(bucket: &str, index: &str) -> Value {
    json!({
        "vectorBucketName": bucket,
        "indexName": index,
        "dataType": "float32",
        "dimension": 4,
        "distanceMetric": "cosine",
    })
}

/// A router serving this service beside the KMS service over ONE shared engine — the same
/// assembly `core/router` performs, minus the other services.
fn router_with_kms() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3vectors_s3vectors::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
        engine,
    ));
    router
}

/// Call one KMS operation: `awsJson1_1`, `POST /` with `X-Amz-Target: TrentService.<op>`,
/// signed for the same Region this service's calls are signed for.
async fn kms(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://kms.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/kms/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Call one KMS operation, requiring success.
async fn kms_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, payload) = kms(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: KMS {operation} must succeed, got {status} {payload}"
    );
    payload
}

/// A fresh customer managed key, by the ARN KMS minted for it.
async fn create_key(router: &MultiServiceRouter, case: &str) -> String {
    let metadata = kms_ok(router, case, "CreateKey", &json!({})).await["KeyMetadata"].clone();
    metadata["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateKey must report an ARN, got {metadata}"))
        .to_string()
}

/// The encryption configuration naming `key_arn`, in the one form the model allows a key to
/// be named in (`kmsKeyArn` "is allowed if and only if `sseType` is set to `aws:kms`").
fn kms_encryption(key_arn: &str) -> Value {
    json!({ "sseType": "aws:kms", "kmsKeyArn": key_arn })
}

/// The sentence a refusal carries, and the id of the KMS call it closes with, split apart.
fn refusal_parts(case: &str, message: &str) -> (String, String) {
    const MARKER: &str = " (kms_request_id=";
    let at = message
        .rfind(MARKER)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {MARKER:?} suffix, got {message:?}"));
    let id = message[at + MARKER.len()..]
        .strip_suffix(')')
        .unwrap_or_else(|| panic!("case {case:?}: the suffix must close, got {message:?}"));
    (message[..at].to_string(), id.to_string())
}

/// Whether `id` has the canonical 8-4-4-4-12 lowercase-hex form every AWS request id
/// carries.
fn is_canonical_uuid(id: &str) -> bool {
    let widths: Vec<usize> = id.split('-').map(str::len).collect();
    widths == [8, 4, 4, 4, 12]
        && id
            .bytes()
            .all(|byte| byte == b'-' || (byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
}

/// Assert that `body` is the refusal `sentence` worded for `case`, answered with HTTP 403
/// and `AccessDeniedException`, closing with the id of a KMS call.
fn assert_refused(case: &str, status: u16, body: &Value, sentence: &str) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (403, "AccessDeniedException"),
        "case {case:?}: expected a 403 AccessDeniedException, got {status} {body}"
    );
    let message = body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the refusal must carry a message, got {body}"));
    let (worded, request_id) = refusal_parts(case, message);
    assert_eq!(
        worded, sentence,
        "case {case:?}: expected {sentence:?}, got {worded:?}"
    );
    assert!(
        is_canonical_uuid(&request_id),
        "case {case:?}: expected a canonical UUID kms_request_id, got {request_id:?}"
    );
}

/// A create under a customer managed key KMS will not use is refused and creates nothing.
///
/// The key-state rows are the two states a customer can move a usable key into
/// (`smithy_query(query="shape",shape="KeyState")` on the KMS model lists `Enabled`,
/// `Disabled` and `PendingDeletion` among them), each with the prose KMS words that state
/// with; the legs are the three ways a create reaches a key — an index inheriting its
/// bucket's, an index naming one, and a bucket naming one.
#[tokio::test]
async fn creating_under_a_key_kms_will_not_use_is_refused_and_creates_nothing() {
    let unusable: [(&str, &str, Value, &str); 2] = [
        (
            "a disabled key",
            "DisableKey",
            json!({}),
            "disabled",
        ),
        (
            "a key awaiting deletion",
            "ScheduleKeyDeletion",
            json!({ "PendingWindowInDays": 7 }),
            "pending deletion",
        ),
    ];

    for (case, operation, extra, prose) in unusable {
        let router = router_with_kms();
        let key_arn = create_key(&router, case).await;
        let bucket = format!("gated-{}", case.replace(' ', "-").to_lowercase());

        // The positive control: while the key is usable the bucket is created under it, and
        // an index inheriting that encryption is served too.
        let (status, created) = call(
            &router,
            "CreateVectorBucket",
            &json!({
                "vectorBucketName": bucket,
                "encryptionConfiguration": kms_encryption(&key_arn),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a bucket under a usable key must be created, got {status} {created}"
        );
        let (status, created) = call(&router, "CreateIndex", &index_body(&bucket, "before")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an index under a usable key must be created, got {status} {created}"
        );

        // Now the key stops being one KMS will use.
        let mut request = json!({ "KeyId": key_arn });
        for (member, value) in extra.as_object().expect("the row carries an object") {
            request[member] = value.clone();
        }
        kms_ok(&router, case, operation, &request).await;

        let refused_bucket = format!("{bucket}-two");
        let legs: [(&str, &str, Value, String); 3] = [
            (
                "an index inheriting the bucket's key",
                "CreateIndex",
                index_body(&bucket, "inheriting"),
                INSUFFICIENT_INDEXING_ACCESS.to_string(),
            ),
            (
                "an index naming the key",
                "CreateIndex",
                {
                    let mut body = index_body(&bucket, "naming");
                    body["encryptionConfiguration"] = kms_encryption(&key_arn);
                    body
                },
                format!("{key_arn} is {prose}."),
            ),
            (
                "a bucket naming the key",
                "CreateVectorBucket",
                json!({
                    "vectorBucketName": refused_bucket,
                    "encryptionConfiguration": kms_encryption(&key_arn),
                }),
                format!("{key_arn} is {prose}."),
            ),
        ];

        for (leg, operation, body, sentence) in legs {
            let leg = format!("{case}, {leg}");
            let (status, answer) = call(&router, operation, &body).await;
            assert_refused(&leg, status, &answer, &sentence);
        }

        // Nothing a refused call named exists afterwards.
        let probes: [(&str, &str, Value, &str); 3] = [
            (
                "the inheriting index",
                "GetIndex",
                json!({ "vectorBucketName": bucket, "indexName": "inheriting" }),
                INDEX_NOT_FOUND,
            ),
            (
                "the index that named the key",
                "GetIndex",
                json!({ "vectorBucketName": bucket, "indexName": "naming" }),
                INDEX_NOT_FOUND,
            ),
            (
                "the bucket that named the key",
                "GetVectorBucket",
                json!({ "vectorBucketName": refused_bucket }),
                BUCKET_NOT_FOUND,
            ),
        ];
        for (probe, operation, body, sentence) in probes {
            let probe = format!("{case}, {probe}");
            let (status, answer) = call(&router, operation, &body).await;
            assert_eq!(
                (status, error_code(&answer).as_str(), answer["message"].as_str()),
                (404, "NotFoundException", Some(sentence)),
                "case {probe:?}: a refused create must leave nothing behind, got \
                 {status} {answer}"
            );
        }
    }
}

/// A create whose effective key KMS WILL use is served, and the resource reports the
/// encryption it was created under.
///
/// The rows span the configurations that reach a usable key or no customer key at all: a
/// key never moved, a key disabled and then enabled again, the S3 managed default, an
/// explicit `AES256` configuration, and a well-formed key ARN this endpoint's KMS holds no
/// key for — there is no key state to consult for the last three, so nothing is withheld.
#[tokio::test]
async fn creating_under_a_usable_key_is_served_and_the_resource_reports_it() {
    /// How a row's key is prepared, and what the index it creates must report.
    enum Row {
        /// A fresh key, left exactly as `CreateKey` made it.
        Usable,
        /// A fresh key disabled and then enabled again.
        Reenabled,
        /// No customer key: the configuration the row names, verbatim.
        NoKey(Option<Value>),
        /// A well-formed key ARN naming no key of this endpoint.
        Unknown(&'static str),
    }

    let rows: [(&str, Row); 5] = [
        ("a key never moved", Row::Usable),
        ("a key enabled again", Row::Reenabled),
        ("no configuration at all", Row::NoKey(None)),
        (
            "the S3 managed type",
            Row::NoKey(Some(json!({ "sseType": "AES256" }))),
        ),
        (
            "a key this endpoint holds none of",
            Row::Unknown("arn:aws:kms:us-east-1:000000000000:key/00000000-0000-4000-8000-000000000000"),
        ),
    ];

    for (case, row) in rows {
        let router = router_with_kms();
        let bucket = format!("served-{}", case.replace(' ', "-").to_lowercase());
        let configuration = match row {
            Row::Usable => Some(kms_encryption(&create_key(&router, case).await)),
            Row::Reenabled => {
                let key_arn = create_key(&router, case).await;
                kms_ok(&router, case, "DisableKey", &json!({ "KeyId": key_arn })).await;
                kms_ok(&router, case, "EnableKey", &json!({ "KeyId": key_arn })).await;
                Some(kms_encryption(&key_arn))
            }
            Row::NoKey(configuration) => configuration,
            Row::Unknown(key_arn) => Some(kms_encryption(key_arn)),
        };

        let mut request = json!({ "vectorBucketName": bucket });
        if let Some(configuration) = configuration.as_ref() {
            request["encryptionConfiguration"] = configuration.clone();
        }
        let (status, created) = call(&router, "CreateVectorBucket", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the bucket must be created, got {status} {created}"
        );

        // The index INHERITS whatever the bucket carries, which is the leg the recorded
        // divergence was about, and a read reports it.
        let (status, created) = call(&router, "CreateIndex", &index_body(&bucket, "served")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the index must be created, got {status} {created}"
        );
        let (status, got) = call(
            &router,
            "GetIndex",
            &json!({ "vectorBucketName": bucket, "indexName": "served" }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetIndex must report the index, got {status} {got}"
        );
        let expected = configuration
            .clone()
            .unwrap_or_else(|| json!({ "sseType": "AES256" }));
        assert_eq!(
            got["index"]["encryptionConfiguration"], expected,
            "case {case:?}: expected {expected}, got {got}"
        );
    }
}
