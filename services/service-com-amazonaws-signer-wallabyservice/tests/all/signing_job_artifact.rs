// SPDX-License-Identifier: CC0-1.0

//! What a code-signing job PRODUCES, end to end over HTTP and over Amazon S3's
//! own front door.
//!
//! Every case drives `StartSigningJob` and `DescribeSigningJob` through the
//! router, with Amazon S3 mounted beside signer on the one shared engine, and
//! then reads the destination through S3 itself — the handoff AWS documents for
//! code signing, where a client polls the job and fetches `signedObject.s3`. The
//! rule under test: a signing job of the S3 workflow reports a terminal status
//! that its own work backs.
//!
//! * A job whose source object Amazon S3 serves as a readable zip archive writes
//!   the signed artifact at the destination — the destination bucket, under the
//!   destination prefix, named for the job — and reports `Succeeded` with the
//!   `signedObject` naming it. An independent `HeadObject` of exactly that
//!   bucket and key answers 200, and the artifact opens as an archive carrying
//!   the source's own entries plus the signature envelope.
//! * A job whose source signs nothing — an object that is not a zip archive, a
//!   key the bucket holds nothing under, a bucket that does not exist — reports
//!   `Failed` with a non-empty `statusReason`, reports no `signedObject`, and
//!   leaves nothing under the destination prefix.
//! * Neither status is reported before the completion falls due: a job reads back
//!   `InProgress`, with no `signedObject` and no `statusReason`, until then.
//!
//! The rows differ along what the rule generalises over: how many entries the
//! source archive carries and how they are named, the destination prefix
//! (including none at all), the region, and — for the failing leg — how the
//! source fails to be a signable archive.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::JOB_COMPLETION_SPAN;
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::code_signing_objects::{
    create_bucket, enable_versioning, head_object, keys_under, mount_object_storage, put_object,
    zip_of,
};

/// The access key id every signer request below is signed with.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// An ACM certificate ARN for `signingMaterial`.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// A published platform of the S3 code-signing-job workflow — the workflow
/// `StartSigningJob` serves.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// The entry the signature envelope travels in, inside the signed artifact.
const SIGNATURE_ENTRY_NAME: &str = "aws-signer-signature.json";

/// One emulator: signer and Amazon S3 on one shared engine, which is what the
/// assembled router runs.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    mount_object_storage(&mut router, &engine);
    router
}

/// A SigV4 credential scope naming this service for `region`.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{region}/signer/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// What the emulator answered: the HTTP status and the body as JSON.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }
}

async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{region}.amazonaws.com"))
        .header("authorization", signed_for(region));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body().to_vec();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A profile on the code-signing platform, in `region`.
async fn put_profile(router: &MultiServiceRouter, region: &str, name: &str) {
    let answer = call(
        router,
        region,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(json!({
            "platformId": PLATFORM_ID,
            "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding {name} in {region}: expected PutSigningProfile 200, got {} {:?}",
        answer.status, answer.body
    );
}

/// `POST /signing-jobs`.
async fn start_job(router: &MultiServiceRouter, region: &str, request: Value) -> Answer {
    call(router, region, "POST", "/signing-jobs", Some(request)).await
}

/// `GET /signing-jobs/<jobId>`.
async fn describe_job(router: &MultiServiceRouter, region: &str, job_id: &str) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!("/signing-jobs/{job_id}"),
        None,
    )
    .await
}

/// Wait past the span the service publishes for a job's completion, plus a
/// margin for a slow machine.
async fn past_the_completion_span() {
    tokio::time::sleep(JOB_COMPLETION_SPAN + Duration::from_millis(250)).await;
}

/// The `StartSigningJob` body for one job.
fn job_request(
    source_bucket: &str,
    source_key: &str,
    version: &str,
    destination_bucket: &str,
    prefix: Option<&str>,
    profile: &str,
    token: &str,
) -> Value {
    let mut destination = json!({ "bucketName": destination_bucket });
    if let Some(prefix) = prefix {
        destination["prefix"] = json!(prefix);
    }
    json!({
        "source": { "s3": {
            "bucketName": source_bucket,
            "key": source_key,
            "version": version,
        } },
        "destination": { "s3": destination },
        "profileName": profile,
        "clientRequestToken": token,
    })
}

/// One job whose source signs.
struct Signable {
    case: &'static str,
    region: &'static str,
    /// The entries the source archive carries.
    entries: Vec<(&'static str, &'static [u8])>,
    /// The destination prefix the request carries, or `None` for a request that
    /// carries none.
    prefix: Option<&'static str>,
}

fn signable_jobs() -> Vec<Signable> {
    vec![
        Signable {
            case: "one entry, under a prefix",
            region: "us-west-2",
            entries: vec![("index.js", b"exports.handler = async () => 1;".as_ref())],
            prefix: Some("signed/"),
        },
        Signable {
            case: "three entries, one of them nested, no prefix at all",
            region: "eu-west-1",
            entries: vec![
                ("index.js", b"exports.handler = 1;".as_ref()),
                ("lib/util.js", b"module.exports = {};".as_ref()),
                ("README", b"".as_ref()),
            ],
            prefix: None,
        },
        Signable {
            case: "an archive with no entries, under a nested prefix",
            region: "us-east-1",
            entries: vec![],
            prefix: Some("a/b/"),
        },
        Signable {
            case: "an entry holding bytes that are not text",
            region: "ap-southeast-1",
            entries: vec![("bootstrap", b"\x7fELF\x02\x01\x01\x00".as_ref())],
            prefix: Some("artifacts/signed-"),
        },
    ]
}

/// A job whose source Amazon S3 serves as a readable archive writes the signed
/// artifact at the destination and reports it — and the object it reports is
/// there, with the source's entries and the signature envelope in it.
#[tokio::test]
async fn a_signed_job_reports_an_object_that_is_really_at_the_destination() {
    for job in signable_jobs() {
        let router = emulator();
        let region = job.region;
        let profile = "Artifact_Profile";
        put_profile(&router, region, profile).await;

        let source_bucket = "signer-source";
        let destination_bucket = "signer-destination";
        let source_key = "package.zip";
        let archive = zip_of(&job.entries);
        create_bucket(&router, region, source_bucket).await;
        enable_versioning(&router, region, source_bucket).await;
        create_bucket(&router, region, destination_bucket).await;
        let version = put_object(&router, region, source_bucket, source_key, &archive).await;
        assert!(
            !version.is_empty(),
            "case {:?}: the version-enabled source bucket must report a version id",
            job.case
        );

        let started = start_job(
            &router,
            region,
            job_request(
                source_bucket,
                source_key,
                &version,
                destination_bucket,
                job.prefix,
                profile,
                "tok-signable",
            ),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected StartSigningJob 200, got {} {:?}",
            job.case, started.status, started.body
        );
        let job_id = started.str_member("jobId");

        // Nothing terminal is reported before the completion is due.
        let running = describe_job(&router, region, &job_id).await;
        assert_eq!(
            running.str_member("status"),
            "InProgress",
            "case {:?}: expected a just-started job to be InProgress, got {:?}",
            job.case,
            running.body
        );
        assert!(
            running.body["signedObject"].is_null() && running.body["statusReason"].is_null(),
            "case {:?}: expected no signedObject and no statusReason while InProgress, got {:?}",
            job.case,
            running.body
        );

        past_the_completion_span().await;

        let done = describe_job(&router, region, &job_id).await;
        assert_eq!(
            done.str_member("status"),
            "Succeeded",
            "case {:?}: expected the job to have Succeeded, got {:?}",
            job.case,
            done.body
        );
        let expected_key = format!("{}{job_id}", job.prefix.unwrap_or_default());
        assert_eq!(
            done.body["signedObject"],
            json!({ "s3": { "bucketName": destination_bucket, "key": expected_key } }),
            "case {:?}: expected the signed object at {destination_bucket}/{expected_key}, got \
             {:?}",
            job.case,
            done.body["signedObject"]
        );

        // The independent read of exactly the reported key: the object is there.
        let reported_bucket = done.body["signedObject"]["s3"]["bucketName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let reported_key = done.body["signedObject"]["s3"]["key"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let head = head_object(&router, region, &reported_bucket, &reported_key).await;
        assert_eq!(
            head, 200,
            "case {:?}: expected HeadObject 200 for the reported object \
             {reported_bucket}/{reported_key}, got {head}",
            job.case
        );

        // And it is the signed artifact: the source's own entries, plus the
        // signature envelope the job added.
        let artifact = object_bytes(&router, region, &reported_bucket, &reported_key).await;
        let mut built = ::zip::ZipArchive::new(std::io::Cursor::new(artifact))
            .unwrap_or_else(|err| panic!("case {:?}: the artifact must be a zip: {err}", job.case));
        let mut names: Vec<String> = Vec::new();
        for index in 0..built.len() {
            let entry = built.by_index(index).expect("the artifact entry reads");
            names.push(entry.name().expect("the entry name reads").to_string());
        }
        for (name, _) in &job.entries {
            assert!(
                names.contains(&(*name).to_string()),
                "case {:?}: expected the source entry {name:?} in the artifact, got {names:?}",
                job.case
            );
        }
        assert!(
            names.contains(&SIGNATURE_ENTRY_NAME.to_string()),
            "case {:?}: expected the signature envelope in the artifact, got {names:?}",
            job.case
        );
        let envelope = {
            let mut bytes = Vec::new();
            ::std::io::Read::read_to_end(
                &mut built
                    .by_name(SIGNATURE_ENTRY_NAME)
                    .expect("the envelope entry is held"),
                &mut bytes,
            )
            .expect("the envelope reads");
            serde_json::from_slice::<Value>(&bytes).expect("the envelope is JSON")
        };
        assert_eq!(
            envelope["jobId"].as_str().unwrap_or_default(),
            job_id,
            "case {:?}: expected the envelope to name the job that signed it, got {:?}",
            job.case,
            envelope["jobId"]
        );
        assert!(
            !envelope["signature"].as_str().unwrap_or_default().is_empty(),
            "case {:?}: expected a signature in the envelope, got {:?}",
            job.case,
            envelope["signature"]
        );
    }
}

/// How a source fails to be something this job can sign.
struct Unsignable {
    case: &'static str,
    /// The object written at the source key, or `None` for a key the bucket holds
    /// nothing under.
    body: Option<&'static [u8]>,
    /// Whether the source bucket exists at all.
    bucket_exists: bool,
}

fn unsignable_sources() -> Vec<Unsignable> {
    vec![
        Unsignable {
            case: "an object that is not a zip archive",
            body: Some(b"this is not a valid zip archive"),
            bucket_exists: true,
        },
        Unsignable {
            case: "an empty object",
            body: Some(b""),
            bucket_exists: true,
        },
        Unsignable {
            case: "an object carrying only a truncated local header",
            body: Some(b"PK\x03\x04"),
            bucket_exists: true,
        },
        Unsignable {
            case: "a key the source bucket holds nothing under",
            body: None,
            bucket_exists: true,
        },
        Unsignable {
            case: "a source bucket that does not exist",
            body: None,
            bucket_exists: false,
        },
    ]
}

/// A job whose source signs nothing reports `Failed` with a reason, reports no
/// signed object, and leaves nothing under the destination prefix.
#[tokio::test]
async fn a_job_that_signed_nothing_fails_and_writes_nothing() {
    const REGION: &str = "us-west-2";
    const PREFIX: &str = "signed/";
    for source in unsignable_sources() {
        let router = emulator();
        let profile = "Unsignable_Profile";
        put_profile(&router, REGION, profile).await;

        let source_bucket = "signer-source";
        let destination_bucket = "signer-destination";
        let source_key = "package.zip";
        create_bucket(&router, REGION, destination_bucket).await;
        let mut version = String::new();
        if source.bucket_exists {
            create_bucket(&router, REGION, source_bucket).await;
            enable_versioning(&router, REGION, source_bucket).await;
            if let Some(body) = source.body {
                version = put_object(&router, REGION, source_bucket, source_key, body).await;
            }
        }

        let started = start_job(
            &router,
            REGION,
            job_request(
                source_bucket,
                source_key,
                &version,
                destination_bucket,
                Some(PREFIX),
                profile,
                "tok-unsignable",
            ),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected StartSigningJob 200, got {} {:?}",
            source.case, started.status, started.body
        );
        let job_id = started.str_member("jobId");

        past_the_completion_span().await;

        let done = describe_job(&router, REGION, &job_id).await;
        assert_eq!(
            done.str_member("status"),
            "Failed",
            "case {:?}: expected the job to have Failed, got {:?}",
            source.case,
            done.body
        );
        assert!(
            !done.str_member("statusReason").is_empty(),
            "case {:?}: expected a statusReason on a Failed job, got {:?}",
            source.case,
            done.body["statusReason"]
        );
        assert!(
            done.body["signedObject"].is_null(),
            "case {:?}: expected no signedObject on a Failed job, got {:?}",
            source.case,
            done.body["signedObject"]
        );
        assert!(
            done.body["signatureExpiresAt"].is_null(),
            "case {:?}: expected no signature expiry on a job that signed nothing, got {:?}",
            source.case,
            done.body["signatureExpiresAt"]
        );

        // And the destination is untouched: nothing was written under the prefix.
        let left = keys_under(&router, REGION, destination_bucket, PREFIX).await;
        assert!(
            left.is_empty(),
            "case {:?}: expected nothing under {PREFIX}, got {left:?}",
            source.case
        );
    }
}

/// The bytes of one object, read through Amazon S3's own front door.
async fn object_bytes(
    router: &MultiServiceRouter,
    region: &str,
    bucket: &str,
    key: &str,
) -> Vec<u8> {
    let host = format!("s3.{region}.amazonaws.com");
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!("https://{host}/{bucket}/{key}"))
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{region}/s3/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(Bytes::new())
        .expect("a well-formed S3 request");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "expected GetObject 200 for s3://{bucket}/{key}"
    );
    response.into_body().to_vec()
}
