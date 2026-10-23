// SPDX-License-Identifier: CC0-1.0

//! S3 Files resolves a file system's `bucket` ARN against S3 before it creates anything.
//!
//! The rule under test: a create whose input names a resource of ANOTHER service by ARN
//! resolves that resource against the owning service before it persists anything, and
//! answers its own modelled not-found error — here HTTP 404 `ResourceNotFoundException`,
//! `CreateFileSystem`'s own (smithy_query: operation CreateFileSystem, errors include
//! ResourceNotFoundException httpError 404) — when the owner reports it absent, leaving
//! no resource behind. The requirement is the model's own:
//! `CreateFileSystemRequest$bucket` documents "The bucket must exist and be in the same
//! Amazon Web Services Region as the file system". Only EXISTENCE is resolved here,
//! because that is the half the modelled error channel can express.
//!
//! This suite needs TWO services on one engine — s3files reaches S3 through the
//! in-process callback, so a single-service router has no S3 to ask and the bucket a row
//! names has to be creatable — which is why it lives here rather than in either
//! service's own crate, exactly as `scheduler_execution_role.rs` does for Scheduler and
//! IAM. The s3files crate's own suite pins the other half: with no S3 to ask, the create
//! is admitted rather than refused on an assumption.
//!
//! The rows span what the rule generalises over: whether the bucket was ever created,
//! the bucket name's length (S3 documents 3..=63 characters) and form (hyphens, dots,
//! digits), the partition the ARN names, whether a `prefix` scopes the file system, and
//! a name that merely HAS an existing bucket's name as a prefix — so the resolution is
//! exact-name, not prefix, matching.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and
//! the requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// An IAM role ARN satisfying `RoleArn`'s pattern
/// `^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/S3FilesRole";

/// The wording the refusal carries, written out here rather than read from the service,
/// so a change to the implementation's own constant makes this assertion fail.
const BUCKET_NOT_FOUND_MESSAGE: &str = "The bucket you specified does not exist.";

/// What one call answered: the HTTP status and the parsed JSON body (`Value::Null` for a
/// body that is empty or not JSON).
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The restJson1 error code, without its namespace.
    fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .rsplit(['#', ':'])
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// The error message, whichever casing the envelope carries it under.
    fn error_message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    fn text(&self, name: &str) -> String {
        self.body[name]
            .as_str()
            .unwrap_or_else(|| panic!("expected a string `{name}` in {:?}", self.body))
            .to_string()
    }
}

/// Dispatch one request addressed to `signing_name`, the way a signed SDK call is.
fn send(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    method: &str,
    path: &str,
    content_type: &str,
    body: bytes::Bytes,
) -> Answer {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        // The credential scope is how a request names the service it is for and the
        // region a handler builds its ARNs in. The signature is never checked: this
        // router runs with authentication off.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .header("content-type", content_type)
        .body(body)
        .expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// One s3files restJson1 call.
fn s3files(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    send(
        router,
        runtime,
        "s3files",
        method,
        path,
        "application/json",
        payload,
    )
}

/// `PUT /<bucket>` — S3's `CreateBucket`, with no location constraint.
fn create_bucket(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime, bucket: &str) {
    let answer = send(
        router,
        runtime,
        "s3",
        "PUT",
        &format!("/{bucket}"),
        "application/xml",
        bytes::Bytes::new(),
    );
    assert_eq!(
        answer.status, 200,
        "setup: CreateBucket({bucket:?}) must succeed, got {} / {:?}",
        answer.status, answer.body
    );
}

/// `CreateFileSystem` on `bucket_arn`, with `prefix` when the row carries one.
fn create_file_system(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    bucket_arn: &str,
    prefix: Option<&str>,
) -> Answer {
    let mut request = json!({ "bucket": bucket_arn, "roleArn": ROLE_ARN });
    if let Some(prefix) = prefix {
        request["prefix"] = json!(prefix);
    }
    s3files(router, runtime, "PUT", "/file-systems", Some(request))
}

/// The `fileSystemId`s `ListFileSystems(bucket=…)` reports.
fn file_systems_on(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    bucket_arn: &str,
) -> Vec<String> {
    let listed = s3files(
        router,
        runtime,
        "GET",
        &format!("/file-systems?bucket={}", encode_query(bucket_arn)),
        None,
    );
    assert_eq!(
        listed.status, 200,
        "ListFileSystems({bucket_arn:?}) must succeed, got {} / {:?}",
        listed.status, listed.body
    );
    listed.body["fileSystems"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a fileSystems list in {:?}", listed.body))
        .iter()
        .map(|entry| {
            entry["fileSystemId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Percent-encode one query value, the way an SDK does: an ARN carries `:` and `/`.
/// The one percent-encoder these router suites share
/// (`transfer_user_with_iam_role::form_encode`), rather than a second copy of it.
fn encode_query(value: &str) -> String {
    crate::transfer_user_with_iam_role::form_encode(value)
}

/// One row: the bucket the row names, whether that bucket is created first, the ARN the
/// request carries for it, the prefix (when any), and whether the create must be
/// admitted.
struct Row {
    case: &'static str,
    /// The bucket to create through S3 before the call, when the row has one.
    create: Option<String>,
    bucket_arn: String,
    prefix: Option<&'static str>,
    accepted: bool,
}

/// A 63-character bucket name — the longest S3 documents ("Bucket names must be between
/// 3 (min) and 63 (max) characters long", S3 bucket naming rules).
fn longest_bucket_name() -> String {
    let name = format!("s3files-gate-{}", "a".repeat(50));
    assert_eq!(name.len(), 63, "the longest-name row must be 63 characters");
    name
}

fn rows() -> Vec<Row> {
    let existing = "s3files-gate-plain";
    vec![
        Row {
            case: "a bucket that exists",
            create: Some(existing.to_string()),
            bucket_arn: format!("arn:aws:s3:::{existing}"),
            prefix: None,
            accepted: true,
        },
        Row {
            case: "a bucket that was never created",
            create: None,
            bucket_arn: "arn:aws:s3:::s3files-gate-ghost".to_string(),
            prefix: None,
            accepted: false,
        },
        Row {
            // The shortest name S3's naming rules allow.
            case: "the shortest bucket name, created",
            create: Some("s3f".to_string()),
            bucket_arn: "arn:aws:s3:::s3f".to_string(),
            prefix: None,
            accepted: true,
        },
        Row {
            case: "the shortest bucket name, never created",
            create: None,
            bucket_arn: "arn:aws:s3:::s3g".to_string(),
            prefix: None,
            accepted: false,
        },
        Row {
            case: "the longest bucket name, created",
            create: Some(longest_bucket_name()),
            bucket_arn: format!("arn:aws:s3:::{}", longest_bucket_name()),
            prefix: None,
            accepted: true,
        },
        Row {
            case: "a dotted, digit-bearing bucket name, created",
            create: Some("s3files.gate.7.dots".to_string()),
            bucket_arn: "arn:aws:s3:::s3files.gate.7.dots".to_string(),
            prefix: None,
            accepted: true,
        },
        Row {
            case: "a dotted bucket name that was never created",
            create: None,
            bucket_arn: "arn:aws:s3:::s3files.gate.9.dots".to_string(),
            prefix: None,
            accepted: false,
        },
        Row {
            // Resolution is exact-name: a name that merely STARTS WITH an existing
            // bucket's name is a different bucket.
            case: "a name that only has an existing bucket's name as its prefix",
            create: None,
            bucket_arn: format!("arn:aws:s3:::{existing}-extra"),
            prefix: None,
            accepted: false,
        },
        Row {
            // The bucket's name is the ARN's resource, whichever partition the ARN
            // names, so a bucket that exists is reached through any partition's ARN.
            case: "another partition's ARN for a bucket that exists",
            create: None,
            bucket_arn: format!("arn:aws-cn:s3:::{existing}"),
            prefix: None,
            accepted: true,
        },
        Row {
            case: "another partition's ARN for a bucket that does not exist",
            create: None,
            bucket_arn: "arn:aws-us-gov:s3:::s3files-gate-ghost-gov".to_string(),
            prefix: None,
            accepted: false,
        },
        Row {
            // `prefix` scopes the file system inside the bucket; it does not change
            // which bucket is resolved. (`prefix` is `^(|.*/)$`, hence the trailing
            // slash.)
            case: "a prefix inside a bucket that exists",
            create: Some("s3files-gate-prefixed".to_string()),
            bucket_arn: "arn:aws:s3:::s3files-gate-prefixed".to_string(),
            prefix: Some("data/"),
            accepted: true,
        },
        Row {
            case: "a prefix inside a bucket that does not exist",
            create: None,
            bucket_arn: "arn:aws:s3:::s3files-gate-prefixed-ghost".to_string(),
            prefix: Some("data/a/b/"),
            accepted: false,
        },
    ]
}

#[test]
fn a_file_system_is_created_only_for_a_bucket_s3_holds() {
    std::thread::Builder::new()
        .name("s3files-bucket-gate".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            for row in rows() {
                if let Some(bucket) = &row.create {
                    create_bucket(&router, &runtime, bucket);
                }

                let created = create_file_system(&router, &runtime, &row.bucket_arn, row.prefix);
                let listed = file_systems_on(&router, &runtime, &row.bucket_arn);

                if row.accepted {
                    assert_eq!(
                        created.status, 201,
                        "case {:?}: a bucket S3 holds must be accepted, got {} / {:?}",
                        row.case, created.status, created.body
                    );
                    let id = created.text("fileSystemId");

                    // Round trip: the create is readable back through GetFileSystem,
                    // which answers from the same stored record.
                    let read =
                        s3files(&router, &runtime, "GET", &format!("/file-systems/{id}"), None);
                    assert_eq!(
                        read.status, 200,
                        "case {:?}: the file system must be persisted, got {} / {:?}",
                        row.case, read.status, read.body
                    );
                    assert_eq!(
                        read.text("bucket"),
                        row.bucket_arn,
                        "case {:?}: the stored record must carry the bucket ARN it was \
                         given, got {:?}",
                        row.case,
                        read.body
                    );
                    assert!(
                        listed.contains(&id),
                        "case {:?}: ListFileSystems on the bucket must report {id}, got \
                         {listed:?}",
                        row.case
                    );
                } else {
                    assert_eq!(
                        (created.status, created.error_code().as_str()),
                        (404, "ResourceNotFoundException"),
                        "case {:?}: a bucket S3 does not hold must be refused 404 \
                         ResourceNotFoundException, got {} / {:?}",
                        row.case,
                        created.status,
                        created.body
                    );
                    assert_eq!(
                        created.error_message(),
                        BUCKET_NOT_FOUND_MESSAGE,
                        "case {:?}: expected the absent-bucket refusal, got {:?}",
                        row.case,
                        created.body
                    );
                    assert!(
                        listed.is_empty(),
                        "case {:?}: a refused create must persist nothing, but \
                         ListFileSystems reported {listed:?}",
                        row.case
                    );
                }
            }

            // The identical request, sent twice around one bucket's creation: refused
            // first, accepted after — so what decides is the bucket's existence and
            // nothing about the request itself.
            let bucket = "s3files-gate-created-later";
            let bucket_arn = format!("arn:aws:s3:::{bucket}");

            let before = create_file_system(&router, &runtime, &bucket_arn, None);
            assert_eq!(
                (before.status, before.error_code().as_str()),
                (404, "ResourceNotFoundException"),
                "before the bucket exists the create must be refused, got {} / {:?}",
                before.status,
                before.body
            );
            assert!(
                file_systems_on(&router, &runtime, &bucket_arn).is_empty(),
                "a refused create must leave the bucket with no file system"
            );

            create_bucket(&router, &runtime, bucket);

            let after = create_file_system(&router, &runtime, &bucket_arn, None);
            assert_eq!(
                after.status, 201,
                "once the bucket exists the identical create must succeed, got {} / {:?}",
                after.status, after.body
            );
            assert_eq!(
                file_systems_on(&router, &runtime, &bucket_arn),
                vec![after.text("fileSystemId")],
                "the accepted create must be the one file system on that bucket"
            );
        })
        .expect("spawn the bucket-gate thread")
        .join()
        .expect("every file system's bucket must be resolved against S3");
}
