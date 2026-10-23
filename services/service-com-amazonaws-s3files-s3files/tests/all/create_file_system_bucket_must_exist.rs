// SPDX-License-Identifier: CC0-1.0

//! `CreateFileSystem`'s bucket gate refuses only what S3 answered, never what it could
//! not ask.
//!
//! The rule this crate's own suite can exercise: `CreateFileSystem` resolves its
//! `bucket` ARN against S3 — the model documents "The bucket must exist and be in the
//! same Amazon Web Services Region as the file system" — and refuses a bucket S3 reports
//! absent with the modelled 404 `ResourceNotFoundException`; but a refusal has to be
//! GROUNDED in S3's answer, so on a router with no S3 mounted, where nothing is known
//! about the bucket, the create is admitted rather than refused on an assumption. That
//! is the same reasoning the scheduler's execution-role gate applies to IAM.
//!
//! The other half of the rule — the refusal itself, over a table of bucket names, ARN
//! partitions and prefixes, with the bucket really created through S3 — needs TWO
//! services on one engine, which a single-service router cannot give: it lives in
//! `core/router/tests/all/s3files_create_file_system_bucket.rs`.
//!
//! The rows below span what "no S3 to ask" generalises over: the bucket name's length
//! and form, the ARN's partition, and whether a `prefix` scopes the file system. None of
//! those buckets exists anywhere, and every one of them is still created, because this
//! router has nothing to resolve them against.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// An IAM role ARN satisfying `RoleArn`'s pattern
/// `^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/S3FilesRole";

/// What one call answered: the HTTP status and the parsed body (`Value::Null` for a body
/// that is empty or not JSON).
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn text(&self, name: &str) -> String {
        self.body[name]
            .as_str()
            .unwrap_or_else(|| panic!("expected a string `{name}` in {:?}", self.body))
            .to_string()
    }
}

/// A router with s3files alone — no S3 to ask about any bucket.
fn router_without_s3() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3files_s3files::EmulatedService::new(
        engine,
    ));
    router
}

/// One s3files restJson1 call.
async fn s3files(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let host = format!("s3files.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        // The credential scope is how a request names the service it is for and the
        // region a handler builds its ARNs in.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/s3files/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// One row: a `bucket` ARN for a bucket nothing ever created, and the prefix (when any)
/// the request carries with it.
const ROWS: &[(&str, &str, Option<&str>)] = &[
    ("a plain name", "arn:aws:s3:::s3files-unasked-plain", None),
    // The shortest and longest names S3's naming rules allow ("Bucket names must be
    // between 3 (min) and 63 (max) characters long", S3 bucket naming rules).
    ("the shortest name", "arn:aws:s3:::s3f", None),
    (
        "the longest name",
        "arn:aws:s3:::s3files-unasked-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        None,
    ),
    (
        "a dotted, digit-bearing name",
        "arn:aws:s3:::s3files.unasked.7.dots",
        None,
    ),
    (
        "another partition's ARN",
        "arn:aws-cn:s3:::s3files-unasked-cn",
        None,
    ),
    (
        "a prefix inside the bucket",
        "arn:aws:s3:::s3files-unasked-prefixed",
        Some("data/a/b/"),
    ),
];

#[tokio::test]
async fn with_no_s3_to_ask_the_create_is_admitted_and_reads_back() {
    let router = router_without_s3();

    for (case, bucket_arn, prefix) in ROWS {
        let mut request = json!({ "bucket": bucket_arn, "roleArn": ROLE_ARN });
        if let Some(prefix) = prefix {
            request["prefix"] = json!(prefix);
        }

        let created = s3files(&router, "PUT", "/file-systems", Some(request)).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: with no S3 to ask, the create must be admitted, got {} / {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.text("bucket"),
            *bucket_arn,
            "case {case:?}: the created file system must carry the bucket ARN it was given, \
             got {:?}",
            created.body
        );

        // Round trip: what the admitted create answered is what GetFileSystem reads back.
        let id = created.text("fileSystemId");
        let read = s3files(&router, "GET", &format!("/file-systems/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the admitted file system must be persisted, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("bucket"),
            *bucket_arn,
            "case {case:?}: the stored record must carry the bucket ARN it was given, got {:?}",
            read.body
        );
    }
}
