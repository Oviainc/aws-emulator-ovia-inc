// SPDX-License-Identifier: CC0-1.0

//! A Cognito-vended session reading an SSE-KMS object: the whole path a client takes to
//! verify an object's encryption on read, across three services on one engine.
//!
//! RULE: a read of a stored object reports the server-side-encryption configuration the
//! object was STORED under — `GetObject`'s `ServerSideEncryption`
//! (`x-amz-server-side-encryption`), `SSEKMSKeyId`
//! (`x-amz-server-side-encryption-aws-kms-key-id`) and `BucketKeyEnabled`
//! (`x-amz-server-side-encryption-bucket-key-enabled`) are the same members `PutObject`
//! echoes on write (`smithy_query(query="operation",operation="GetObject,HeadObject")`),
//! so a reader can tell an SSE-KMS object from a plain one. And the identity-pool calls a
//! client reaches that read with — `GetId` and `GetCredentialsForIdentity`, whose model
//! lists NO auth scheme (`auth: []`, `@optionalAuth`) — are served without a SigV4
//! credential scope, because a client exchanging a login for credentials has nothing to
//! sign with.
//!
//! Only a cross-service suite can pin the two halves together: each crate's own tests can
//! assert its own half (`service-…-s3`'s `object_encryption`, `service-…-cognitoidentity`'s
//! `anonymous_credential_exchange`), but not that the session a vend answers reaches the
//! read and sees the object's encryption.
//!
//! What each leg pins. The VEND legs carry no credential scope at all, so they are served
//! only because this router claims a request that names Cognito Identity in its
//! `X-Amz-Target` alone. The S3 legs carry the vended access key id in their credential
//! scope on a router with request authentication OFF, as every suite in this test crate
//! runs it (a signed row would otherwise need a real signature), so what they pin is the
//! READ's answer for the session's key id — the encryption each object was stored under —
//! and not signature verification, which is the emulator's own and is covered where it
//! lives.
//!
//! The rows vary what the rule generalises over: the key prefix the object is written
//! under, the algorithm (`aws:kms`, `aws:kms:dsse`, `AES256`, and a write naming no
//! encryption member at all, which inherits the bucket's default), the form of the KMS key
//! id the write names (the ARN KMS minted, its bare key id), and whether a Bucket Key is
//! enabled. The key id a READ reports is held to what the WRITE reported rather than to a
//! literal, because the rule here is that the two agree; whether S3 normalises a bare key
//! id to its full ARN is a question of the WRITE path, which this rule does not decide.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "us-east-1";

/// The emulator's own address, which is what a client is pointed at: an address, so the
/// `Host` header names no service and the unsigned calls carry their service name in
/// `X-Amz-Target` alone.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// The `SSEAlgorithm` a bucket no `PutBucketEncryption` has configured encrypts an object
/// with ("all buckets have a default encryption configuration that uses server-side
/// encryption with Amazon S3 managed keys", `GetBucketEncryption`'s documentation).
const DEFAULT_SSE_ALGORITHM: &str = "AES256";

/// What one call answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// A bound response header's value.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The body as JSON (`Null` when it is empty or not JSON), for the awsJson services.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&self.body).unwrap_or(Value::Null)
        }
    }

    /// The body as text, for S3's object bytes and its restXml error documents.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// How a request is signed: with a SigV4 credential scope naming a service and the access
/// key id it is signed with, or with no credential scope at all — which is how an SDK
/// sends an operation whose model lists no auth scheme.
#[derive(Clone, Copy, Debug)]
enum Signing<'a> {
    As {
        signing_name: &'a str,
        access_key_id: &'a str,
    },
    Anonymous,
}

/// Dispatch one request, addressed and signed as `signing` says.
fn dispatch(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing: Signing<'_>,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: bytes::Bytes,
) -> Answer {
    let host = match signing {
        Signing::As { signing_name, .. } => format!("{signing_name}.{REGION}.amazonaws.com"),
        Signing::Anonymous => EMULATOR_HOST.to_string(),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host.clone());
    if let Signing::As {
        signing_name,
        access_key_id,
    } = signing
    {
        // The credential scope is how a signed request names the access key id it is
        // signed with, the service it is for, and the region a handler builds its ARNs
        // in. The signature is never checked: this router runs with authentication off.
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential={access_key_id}/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified"
            ),
        );
    }
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(body).expect("a well-formed request");
    let response = runtime.block_on(router.dispatch(request));
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The access key id every setup call is signed with: the emulator's own fixture key,
/// which is NOT the vended session.
const SETUP_KEY_ID: &str = "AKIDTEST";

/// A setup call's signing, for the service whose signing name is `signing_name`.
fn as_setup(signing_name: &str) -> Signing<'_> {
    Signing::As {
        signing_name,
        access_key_id: SETUP_KEY_ID,
    }
}

/// One awsJson1_1 call, signed as `signing` says.
fn rpc(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing: Signing<'_>,
    target: &str,
    body: &Value,
) -> Answer {
    dispatch(
        router,
        runtime,
        signing,
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target),
        ],
        bytes::Bytes::from(body.to_string()),
    )
}

/// `kms:CreateKey`, answering the new key's `(key id, key ARN)`.
fn create_key(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime) -> (String, String) {
    let created = rpc(
        router,
        runtime,
        as_setup("kms"),
        "TrentService.CreateKey",
        &json!({ "Description": "the CMK the objects below are written under" }),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateKey must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
    let metadata = created.json();
    let key_id = metadata["KeyMetadata"]["KeyId"]
        .as_str()
        .expect("CreateKey reports a KeyId")
        .to_string();
    let key_arn = metadata["KeyMetadata"]["Arn"]
        .as_str()
        .expect("CreateKey reports the key ARN")
        .to_string();
    (key_id, key_arn)
}

/// `s3:CreateBucket`.
fn create_bucket(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime, bucket: &str) {
    let created = dispatch(
        router,
        runtime,
        as_setup("s3"),
        "PUT",
        &format!("/{bucket}"),
        &[("content-type", "application/xml")],
        bytes::Bytes::new(),
    );
    assert_eq!(
        created.status,
        200,
        "setup: CreateBucket({bucket:?}) must succeed, got {} with {:?}",
        created.status,
        created.text()
    );
}

/// The access key id of a session vended to a fresh identity of a fresh identity pool,
/// reached the way a client reaches it: `GetId` and `GetCredentialsForIdentity` sent with
/// NO credential scope.
fn vended_access_key_id(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    pool_name: &str,
) -> String {
    let pool = rpc(
        router,
        runtime,
        as_setup("cognito-identity"),
        "AWSCognitoIdentityService.CreateIdentityPool",
        &json!({ "IdentityPoolName": pool_name, "AllowUnauthenticatedIdentities": true }),
    );
    assert_eq!(
        pool.status,
        200,
        "setup: CreateIdentityPool must succeed, got {} with {:?}",
        pool.status,
        pool.text()
    );
    let pool_id = pool.json()["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool reports an IdentityPoolId")
        .to_string();

    let identity = rpc(
        router,
        runtime,
        Signing::Anonymous,
        "AWSCognitoIdentityService.GetId",
        &json!({ "IdentityPoolId": pool_id }),
    );
    assert_eq!(
        identity.status,
        200,
        "an unsigned GetId must be served, got {} with {:?}",
        identity.status,
        identity.text()
    );
    let identity_id = identity.json()["IdentityId"]
        .as_str()
        .expect("GetId reports an IdentityId")
        .to_string();

    let vended = rpc(
        router,
        runtime,
        Signing::Anonymous,
        "AWSCognitoIdentityService.GetCredentialsForIdentity",
        &json!({ "IdentityId": identity_id }),
    );
    assert_eq!(
        vended.status,
        200,
        "an unsigned GetCredentialsForIdentity must be served, got {}",
        vended.status
    );
    let credentials = vended.json();
    let access_key_id = credentials["Credentials"]["AccessKeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !access_key_id.is_empty(),
        "the vend must carry an AccessKeyId, but the member was absent or empty"
    );
    // A client cannot sign a request without the session's secret and token either, so
    // each must be there — asserted by presence alone, so no secret reaches the
    // assertion text.
    for member in ["SecretKey", "SessionToken"] {
        assert!(
            !credentials["Credentials"][member]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {member:?}: the vended session must carry it, but the member was absent \
             or empty"
        );
    }
    access_key_id
}

/// `s3:GetObject` signed with the session `access_key_id` names — the read the vended
/// session performs.
fn vended_get_object(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    access_key_id: &str,
    bucket: &str,
    key: &str,
) -> Answer {
    dispatch(
        router,
        runtime,
        Signing::As {
            signing_name: "s3",
            access_key_id,
        },
        "GET",
        &format!("/{bucket}/{key}"),
        &[],
        bytes::Bytes::new(),
    )
}

/// One row: the key the object is written under, the encryption headers the write carries,
/// and the encryption the read must then report. The KMS key id is not written out as an
/// expectation of its own: the rule is that a READ agrees with the WRITE about it, so each
/// row's reference is what the write itself reported — which leaves open whether S3
/// normalises a bare key id to its ARN, a question of the WRITE path and not of this rule.
struct Row {
    case: &'static str,
    key: &'static str,
    write_headers: Vec<(String, String)>,
    expected_algorithm: Option<String>,
    /// Whether the object is stored under SSE-KMS, which is the only encryption a key id
    /// and a Bucket Key setting belong to.
    kms: bool,
    expected_bucket_key: Option<&'static str>,
}

/// The table, built around the key KMS minted for this run.
fn rows(key_id: &str, key_arn: &str) -> Vec<Row> {
    vec![
        Row {
            case: "SSE-KMS under one prefix, the key named by ARN, no Bucket Key",
            key: "open/report.csv",
            write_headers: vec![
                ("x-amz-server-side-encryption".to_string(), "aws:kms".to_string()),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id".to_string(),
                    key_arn.to_string(),
                ),
                (
                    "x-amz-server-side-encryption-bucket-key-enabled".to_string(),
                    "false".to_string(),
                ),
            ],
            expected_algorithm: Some("aws:kms".to_string()),
            kms: true,
            expected_bucket_key: Some("false"),
        },
        Row {
            case: "SSE-KMS under another prefix, the key named by its bare id",
            key: "closed/report.csv",
            write_headers: vec![
                ("x-amz-server-side-encryption".to_string(), "aws:kms".to_string()),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id".to_string(),
                    key_id.to_string(),
                ),
            ],
            expected_algorithm: Some("aws:kms".to_string()),
            kms: true,
            expected_bucket_key: None,
        },
        Row {
            case: "dual-layer SSE-KMS with a Bucket Key, deep under a prefix",
            key: "open/nested/deeper/report.csv",
            write_headers: vec![
                (
                    "x-amz-server-side-encryption".to_string(),
                    "aws:kms:dsse".to_string(),
                ),
                (
                    "x-amz-server-side-encryption-aws-kms-key-id".to_string(),
                    key_arn.to_string(),
                ),
                (
                    "x-amz-server-side-encryption-bucket-key-enabled".to_string(),
                    "true".to_string(),
                ),
            ],
            expected_algorithm: Some("aws:kms:dsse".to_string()),
            kms: true,
            expected_bucket_key: Some("true"),
        },
        Row {
            // Not SSE-KMS: the read reports the algorithm this object was stored under
            // and no KMS member, so what a reader sees follows the write rather than a
            // constant.
            case: "SSE-S3, named explicitly",
            key: "open/plain.csv",
            write_headers: vec![(
                "x-amz-server-side-encryption".to_string(),
                "AES256".to_string(),
            )],
            expected_algorithm: Some("AES256".to_string()),
            kms: false,
            expected_bucket_key: None,
        },
        Row {
            // A write naming no encryption member at all is stored under the bucket's
            // default encryption configuration, which on a bucket nothing configured is
            // SSE-S3.
            case: "no encryption member at all: the bucket's default",
            key: "open/inherited.csv",
            write_headers: Vec::new(),
            expected_algorithm: Some(DEFAULT_SSE_ALGORITHM.to_string()),
            kms: false,
            expected_bucket_key: None,
        },
    ]
}

#[test]
fn a_vended_session_reads_the_encryption_each_object_was_stored_under() {
    std::thread::Builder::new()
        .name("cognito-vended-sse-kms-read".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            let bucket = "vended-session-encryption";
            let (key_id, key_arn) = create_key(&router, &runtime);
            create_bucket(&router, &runtime, bucket);
            let access_key_id = vended_access_key_id(&router, &runtime, "vended-session-pool");

            for row in rows(&key_id, &key_arn) {
                let payload = format!("bytes of {}", row.key).into_bytes();
                let write_headers: Vec<(&str, &str)> = row
                    .write_headers
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_str()))
                    .collect();
                let written = dispatch(
                    &router,
                    &runtime,
                    as_setup("s3"),
                    "PUT",
                    &format!("/{bucket}/{}", row.key),
                    &write_headers,
                    bytes::Bytes::from(payload.clone()),
                );
                assert_eq!(
                    written.status,
                    200,
                    "case {:?}: the write must succeed, got {} with {:?}",
                    row.case,
                    written.status,
                    written.text()
                );
                assert_eq!(
                    written.header("x-amz-server-side-encryption"),
                    row.expected_algorithm,
                    "case {:?}: the write must report the encryption it stored the object \
                     under, got {:?}",
                    row.case,
                    written.header("x-amz-server-side-encryption")
                );
                // The KMS key id the WRITE reported is the reference the read is held to:
                // a key id belongs to SSE-KMS alone, so a row stored under any other
                // algorithm must report none at all.
                let written_key_id = written.header("x-amz-server-side-encryption-aws-kms-key-id");
                assert_eq!(
                    written_key_id.is_some(),
                    row.kms,
                    "case {:?}: a KMS key id belongs to an SSE-KMS write and to no other, \
                     got {written_key_id:?}",
                    row.case
                );

                let read = vended_get_object(&router, &runtime, &access_key_id, bucket, row.key);
                assert_eq!(
                    read.status,
                    200,
                    "case {:?}: the vended session's read must succeed, got {} with {:?}",
                    row.case,
                    read.status,
                    read.text()
                );
                assert_eq!(
                    read.body.as_ref(),
                    payload.as_slice(),
                    "case {:?}: the read must answer the stored bytes, got {:?}",
                    row.case,
                    read.text()
                );
                assert_eq!(
                    read.header("x-amz-server-side-encryption"),
                    row.expected_algorithm,
                    "case {:?}: the read must report the encryption the object was stored \
                     under, got {:?}",
                    row.case,
                    read.header("x-amz-server-side-encryption")
                );
                assert_eq!(
                    read.header("x-amz-server-side-encryption-aws-kms-key-id"),
                    written_key_id,
                    "case {:?}: the read must report the same KMS key the write did, got \
                     {:?}",
                    row.case,
                    read.header("x-amz-server-side-encryption-aws-kms-key-id")
                );
                assert_eq!(
                    read.header("x-amz-server-side-encryption-bucket-key-enabled")
                        .as_deref(),
                    row.expected_bucket_key,
                    "case {:?}: the read must report the Bucket Key setting the object was \
                     stored under, got {:?}",
                    row.case,
                    read.header("x-amz-server-side-encryption-bucket-key-enabled")
                );

                // The same statement from the other reader: `HeadObject` binds the same
                // three members, so a client that branches on either sees one answer.
                let headed = dispatch(
                    &router,
                    &runtime,
                    as_setup("s3"),
                    "HEAD",
                    &format!("/{bucket}/{}", row.key),
                    &[],
                    bytes::Bytes::new(),
                );
                assert_eq!(
                    headed.status,
                    200,
                    "case {:?}: HeadObject must succeed, got {} with {:?}",
                    row.case,
                    headed.status,
                    headed.text()
                );
                assert_eq!(
                    headed.header("x-amz-server-side-encryption"),
                    row.expected_algorithm,
                    "case {:?}: HeadObject must report the same encryption GetObject does, \
                     got {:?}",
                    row.case,
                    headed.header("x-amz-server-side-encryption")
                );
            }
        })
        .expect("spawn the vended-session read thread")
        .join()
        .expect("a vended session must read every object's stored encryption");
}
