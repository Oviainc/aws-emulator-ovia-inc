// SPDX-License-Identifier: CC0-1.0

//! The Amazon S3 bucket a storage configuration records to: resolved, and granted to IVS,
//! where Amazon S3 owns it.
//!
//! `CreateStorageConfiguration` is documented as doing both — "used to enable recording to
//! Amazon S3. When a StorageConfiguration is created, IVS will modify the S3 bucketPolicy
//! of the provided bucket. This will ensure that IVS has sufficient permissions to write
//! content to the provided bucket" — and neither is a fact of this service's own store.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. A CREATE OVER A BUCKET AMAZON S3 DOES NOT HOLD IS REFUSED, with the modelled
//!    `ValidationException` (HTTP 400) and the one sentence this service states it with, and
//!    nothing is persisted: no configuration is minted and the account holds no more of them
//!    than before.
//! 2. A CREATE OVER A BUCKET IT DOES HOLD SUCCEEDS and round-trips: the configuration reads
//!    back through `GetStorageConfiguration` naming the same bucket.
//! 3. THE BUCKET CARRIES THE RECORDING GRANT afterwards: its own policy holds a statement
//!    allowing this Region's composite recorder to write every object of the bucket over a
//!    secure transport — merged into whatever policy the bucket already carried, once however
//!    many configurations record to it.
//!
//! Both services are mounted on ONE shared engine, exactly as the assembled emulator mounts
//! them, so the bucket is created through Amazon S3's own wire and resolved through the same
//! state Amazon S3 serves its own reads from.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The Region most of this suite signs for.
const REGION: &str = "us-east-1";

/// A second Region, so the grant's principal is pinned as a function of the Region the
/// configuration is created in rather than as one fixed string.
const OTHER_REGION: &str = "eu-west-1";

/// The Region an Amazon S3 `CreateBucket` places a bucket in when the request carries no
/// `CreateBucketConfiguration` (`BucketLocationConstraint`'s own documentation: the default
/// is `us-east-1`), which is the one Region this suite creates a bucket in without one.
const DEFAULT_BUCKET_REGION: &str = "us-east-1";

/// The sentence a create over a bucket Amazon S3 holds none of is refused with, read from
/// the crate so this suite cannot drift from the wording it answers.
const BUCKET_NOT_FOUND_MESSAGE: &str = state::BUCKET_NOT_FOUND_MESSAGE;

/// One emulator holding this service and the service that owns a recording bucket, both on
/// one shared engine — what the assembled router hands every handler.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pin authentication off so the suite does not depend on the process setting.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// The SigV4 `Authorization` header of a request signed for `service` in `region`, which
    /// is how the router picks the service a request belongs to.
    fn authorization(service: &str, region: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVSRT/20260101/{region}/{service}/\
             aws4_request, SignedHeaders=host;x-amz-date, Signature=00"
        )
    }

    async fn dispatch(&self, request: http::Request<Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call one of this service's restJson1 operations in `region`.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://ivsrealtime.{region}.amazonaws.com/{operation}"))
            .header("host", format!("ivsrealtime.{region}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", Self::authorization("ivs", region))
            .body(Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        self.dispatch(request).await
    }

    /// Call one of this service's restJson1 operations in [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// Create an Amazon S3 bucket through Amazon S3's own (REST) wire, in `region`. Path
    /// style, and with the `CreateBucketConfiguration` Amazon S3 requires outside its
    /// default Region.
    async fn given_bucket_in(&self, region: &str, name: &str) {
        let body = if region == DEFAULT_BUCKET_REGION {
            Bytes::new()
        } else {
            Bytes::from(format!(
                "<CreateBucketConfiguration \
                 xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
                 <LocationConstraint>{region}</LocationConstraint>\
                 </CreateBucketConfiguration>"
            ))
        };
        let request = http::Request::builder()
            .method("PUT")
            .uri(format!("/{name}"))
            .header("host", format!("s3.{region}.amazonaws.com"))
            .header("authorization", Self::authorization("s3", region))
            .body(body)
            .expect("the test request is well formed");
        let (status, payload) = self.dispatch(request).await;
        assert_eq!(
            status, 200,
            "CreateBucket({name}) must succeed, got {status} with {payload}"
        );
    }

    /// Create a bucket in [`REGION`].
    async fn given_bucket(&self, name: &str) {
        self.given_bucket_in(REGION, name).await;
    }

    /// Remove an Amazon S3 bucket, so nothing holds it any more.
    async fn delete_bucket(&self, name: &str) {
        let request = http::Request::builder()
            .method("DELETE")
            .uri(format!("/{name}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", Self::authorization("s3", REGION))
            .body(Bytes::new())
            .expect("the test request is well formed");
        let (status, payload) = self.dispatch(request).await;
        assert_eq!(
            status, 204,
            "DeleteBucket({name}) must succeed, got {status} with {payload}"
        );
    }

    /// Attach `policy` to `name` through Amazon S3's own wire, so the merge below has a
    /// policy of the bucket owner's own to merge into.
    async fn given_bucket_policy(&self, name: &str, policy: &Value) {
        let request = http::Request::builder()
            .method("PUT")
            .uri(format!("/{name}?policy"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", Self::authorization("s3", REGION))
            .body(Bytes::from(policy.to_string()))
            .expect("the test request is well formed");
        let (status, payload) = self.dispatch(request).await;
        assert_eq!(
            status, 204,
            "PutBucketPolicy({name}) must succeed, got {status} with {payload}"
        );
    }

    /// The policy `name` carries right now, read through Amazon S3's own wire: the status and
    /// the document.
    async fn bucket_policy(&self, name: &str) -> (u16, Value) {
        let request = http::Request::builder()
            .method("GET")
            .uri(format!("/{name}?policy"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", Self::authorization("s3", REGION))
            .body(Bytes::new())
            .expect("the test request is well formed");
        self.dispatch(request).await
    }

    /// How many storage configurations this Region's view holds, read through
    /// `ListStorageConfigurations`.
    async fn held(&self, case: &str) -> usize {
        let (status, response) = self.call("ListStorageConfigurations", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListStorageConfigurations must answer HTTP 200, \
             got {status} {response}"
        );
        response["storageConfigurations"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default()
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// A string member, read as the empty string when absent.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The statements a bucket policy document holds.
fn statements(document: &Value) -> Vec<Value> {
    document["Statement"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The statements of `document` that grant the service principal `principal`.
fn granting(document: &Value, principal: &str) -> Vec<Value> {
    statements(document)
        .into_iter()
        .filter(|statement| statement["Principal"]["Service"] == json!(principal))
        .collect()
}

/// The service principal IVS records a composition as, in `region` — the identity the
/// bucket's policy has to grant.
fn recorder_principal(region: &str) -> String {
    format!("ivs-composite.{region}.amazonaws.com")
}

/// Assert that `document` grants `region`'s recorder exactly once, with the permission,
/// the resource and the transport condition the grant is made of.
fn assert_grants_recorder(case: &str, document: &Value, region: &str, bucket: &str) {
    let principal = recorder_principal(region);
    let grants = granting(document, &principal);
    assert_eq!(
        grants.len(),
        1,
        "case {case:?}: the bucket must grant {principal} exactly once, got {document}"
    );
    let grant = &grants[0];
    assert_eq!(
        grant["Effect"],
        json!("Allow"),
        "case {case:?}: the grant must allow, got {grant}"
    );
    assert_eq!(
        grant["Resource"],
        json!(format!("arn:aws:s3:::{bucket}/*")),
        "case {case:?}: the grant must cover every object of the bucket, got {grant}"
    );
    assert_eq!(
        grant["Condition"]["Bool"]["aws:SecureTransport"],
        json!("true"),
        "case {case:?}: the grant must require a secure transport, got {grant}"
    );
    let actions = grant["Action"].as_array().cloned().unwrap_or_default();
    assert!(
        actions.contains(&json!("s3:PutObject")),
        "case {case:?}: the grant must allow the recording write, got {grant}"
    );
}

/// RULE 1: a create over a bucket Amazon S3 does not hold is the modelled
/// `ValidationException`, and persists nothing.
///
/// One row per WAY a name can name no bucket — never created, created and then deleted, a
/// name one character off a bucket that IS held, and names at both published length bounds
/// of `S3BucketName` (`@length(3..63)`, `@pattern ^[a-z0-9-.]+$`, from
/// `smithy_query(query="shape",shape="S3StorageConfiguration")`) — since the rule is about
/// what Amazon S3 holds rather than about the shape of the name.
#[tokio::test]
async fn a_create_over_a_bucket_amazon_s3_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let held = "ivs-recordings-held";
    let removed = "ivs-recordings-removed";
    emulator.given_bucket(held).await;
    emulator.given_bucket(removed).await;
    emulator.delete_bucket(removed).await;

    // (case, the request)
    let cases: Vec<(&str, Value)> = vec![
        (
            "a bucket nothing ever created",
            json!({ "s3": { "bucketName": "ivs-recordings-absent" } }),
        ),
        (
            "a bucket that was created and then deleted",
            json!({ "s3": { "bucketName": removed } }),
        ),
        (
            "a name one character off a bucket that is held",
            json!({ "s3": { "bucketName": format!("{held}x") } }),
        ),
        (
            "a name at the modelled 3-character minimum",
            json!({ "s3": { "bucketName": "abc" }, "name": "shortest" }),
        ),
        (
            "a name at the modelled 63-character maximum, with dots and dashes",
            json!({
                "s3": { "bucketName": format!("a.b-{}", "c".repeat(59)) },
                "tags": { "env": "test" },
            }),
        ),
    ];

    let before = emulator.held("before the refusals").await;
    for (case, request) in cases {
        let (status, response) = emulator.call("CreateStorageConfiguration", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: CreateStorageConfiguration {request} must answer HTTP 400, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: the refusal must be the modelled ValidationException, \
             got {response}"
        );
        assert_eq!(
            member(&response, "message"),
            BUCKET_NOT_FOUND_MESSAGE,
            "case {case:?}: the refusal must state {BUCKET_NOT_FOUND_MESSAGE:?}, \
             got {response}"
        );
        assert!(
            response.get("storageConfiguration").is_none(),
            "case {case:?}: a refused create must mint nothing, got {response}"
        );
    }
    let after = emulator.held("after the refusals").await;
    assert_eq!(
        after, before,
        "a refused create must persist nothing, got {after} against {before}"
    );
    // The bucket that IS held carries no policy, because no create over it succeeded.
    let (status, policy) = emulator.bucket_policy(held).await;
    assert_eq!(
        status, 404,
        "a refused create must amend no bucket policy, got {status} {policy}"
    );
}

/// RULE 2 and RULE 3: a create over a bucket Amazon S3 holds succeeds, reads back naming
/// that bucket, and leaves the bucket granting this Region's recorder.
///
/// One row per FORM of the request and of the bucket name — both published length bounds of
/// `S3BucketName`, both punctuation characters its pattern admits, with and without the
/// optional `name`, and in each of two Regions, since the grant is a function of the Region
/// the configuration is created in.
#[tokio::test]
async fn a_create_over_a_held_bucket_grants_the_recorder() {
    // (case, the Region the configuration is created in, the bucket, the request)
    let cases: Vec<(&str, &str, String, Value)> = vec![
        (
            "a bucket name at the modelled 3-character minimum",
            REGION,
            "abc".to_string(),
            json!({}),
        ),
        (
            "a bucket name with dots and dashes, and a configuration name",
            REGION,
            "ivs.recordings-2026".to_string(),
            json!({ "name": "studio-recordings" }),
        ),
        (
            "a bucket name at the modelled 63-character maximum",
            REGION,
            format!("a.b-{}", "c".repeat(59)),
            json!({ "tags": { "env": "test" } }),
        ),
        (
            "a configuration created in another Region",
            OTHER_REGION,
            "ivs-recordings-elsewhere".to_string(),
            json!({ "name": "elsewhere" }),
        ),
    ];

    for (case, region, bucket, extra) in cases {
        // One emulator per row, so each row decides against a store holding only its own
        // bucket and configuration.
        let emulator = Emulator::new();
        emulator.given_bucket_in(region, &bucket).await;
        let mut request = json!({ "s3": { "bucketName": bucket } });
        if let (Some(request), Some(extra)) = (request.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                request.insert(key.clone(), value.clone());
            }
        }

        let (status, response) = emulator
            .call_in(region, "CreateStorageConfiguration", &request)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStorageConfiguration {request} must answer HTTP 200, \
             got {status} {response}"
        );
        let created = response["storageConfiguration"].clone();
        assert_eq!(
            created["s3"]["bucketName"],
            json!(bucket),
            "case {case:?}: the create must report the bucket it was given, got {created}"
        );

        // RULE 2: the configuration round-trips through its own read.
        let arn = member(&created, "arn");
        let (status, read) = emulator
            .call_in(region, "GetStorageConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStorageConfiguration on {arn} must answer HTTP 200, \
             got {status} {read}"
        );
        assert_eq!(
            read["storageConfiguration"], created,
            "case {case:?}: the read must report the configuration the create did, \
             got {read} against {created}"
        );

        // RULE 3: the bucket carries the grant.
        let (status, document) = emulator.bucket_policy(&bucket).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the bucket {bucket} must carry a policy, got {status} {document}"
        );
        assert_grants_recorder(case, &document, region, &bucket);
        assert!(
            granting(&document, &recorder_principal("ap-south-1")).is_empty(),
            "case {case:?}: only this Region's recorder may be granted, got {document}"
        );
    }
}

/// RULE 3, over what the bucket already carries: the grant is MERGED, so a policy of the
/// bucket owner's own keeps its statements and a second configuration recording to one
/// bucket leaves it granting exactly once.
#[tokio::test]
async fn the_grant_is_merged_into_the_policy_the_bucket_already_carries() {
    let emulator = Emulator::new();
    let bucket = "ivs-recordings-shared";
    emulator.given_bucket(bucket).await;
    let owners = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "OwnerRead",
            "Effect": "Allow",
            "Principal": { "AWS": "arn:aws:iam::000000000000:root" },
            "Action": ["s3:GetObject"],
            "Resource": format!("arn:aws:s3:::{bucket}/*"),
        }],
    });
    emulator.given_bucket_policy(bucket, &owners).await;

    // Two configurations record to the one bucket: the first merges the grant into the
    // owner's policy, the second finds the grant already there.
    for (case, request) in [
        ("the first configuration over the bucket", json!({ "name": "first" })),
        ("a second configuration over the same bucket", json!({ "name": "second" })),
    ] {
        let mut body = request;
        body["s3"] = json!({ "bucketName": bucket });
        let (status, response) = emulator.call("CreateStorageConfiguration", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStorageConfiguration {body} must answer HTTP 200, \
             got {status} {response}"
        );

        let (status, document) = emulator.bucket_policy(bucket).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the bucket must carry a policy, got {status} {document}"
        );
        assert_grants_recorder(case, &document, REGION, bucket);
        let kept: Vec<Value> = statements(&document)
            .into_iter()
            .filter(|statement| statement["Sid"] == json!("OwnerRead"))
            .collect();
        assert_eq!(
            kept.len(),
            1,
            "case {case:?}: the owner's own statement must be left as it was, got {document}"
        );
        assert_eq!(
            kept[0], owners["Statement"][0],
            "case {case:?}: the owner's statement must be unchanged, got {}",
            kept[0]
        );
        assert_eq!(
            statements(&document).len(),
            2,
            "case {case:?}: the policy must hold the owner's statement and one grant, \
             got {document}"
        );
    }
}
