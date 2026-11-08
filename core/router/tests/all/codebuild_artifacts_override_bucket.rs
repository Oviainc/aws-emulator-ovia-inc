// SPDX-License-Identifier: CC0-1.0

//! CodeBuild resolves the bucket a build's artifacts name against S3 before it starts the
//! build.
//!
//! The rule under test: a build UPLOADS its output artifacts to the destination it is
//! started with, so a `StartBuild` (or `StartBuildBatch`) whose artifacts override names
//! a bucket resolves that bucket against the service that OWNS buckets before any build
//! is minted, and answers its own modelled client error — HTTP 400
//! `InvalidInputException`, the only input error those operations model — when S3 reports
//! the bucket absent, leaving no build behind. `ProjectArtifacts$location` is "the name of
//! the output bucket" for the `S3` artifacts type, which is what makes the value a bucket
//! to resolve.
//!
//! This suite needs the whole router on one engine — CodeBuild reaches S3 (and IAM, for
//! the project's service role) through the in-process callback, so a single-service router
//! has no S3 to ask and the bucket a row names could not be created — which is why it
//! lives here rather than in the CodeBuild crate, exactly as
//! `s3files_create_file_system_bucket.rs` does for S3 Files. The CodeBuild crate's own
//! suite pins the other half of the rule: artifacts bound for a bucket must NAME one,
//! which needs no S3 to ask.
//!
//! The rows span what the rule generalises over: whether the bucket was ever created,
//! whether the location carries a prefix behind the bucket name, and which operation the
//! override arrived through.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and
//! the requests driven — on a sized thread.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// Dispatch one request addressed to `signing_name`, the way a signed SDK call is.
fn send(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    signing_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Bytes,
) -> (u16, Vec<u8>) {
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let mut request = http::Request::builder()
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
        );
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    let response = runtime.block_on(
        router.dispatch(request.body(body).expect("a well-formed request")),
    );
    (response.status().as_u16(), response.into_body().to_vec())
}

/// One awsJson1_1 call to CodeBuild, answered with the HTTP status and the JSON body.
fn codebuild(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let (status, bytes) = send(
        router,
        runtime,
        "codebuild",
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1".to_string()),
            (
                "x-amz-target",
                format!("CodeBuild_20161006.{operation}"),
            ),
        ],
        Bytes::from(serde_json::to_vec(&body).expect("the test body serializes")),
    );
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `PUT /<bucket>` — S3's `CreateBucket`, naming the Region this suite is signed for as
/// its location constraint (a create sent to a REGIONAL endpoint has to name one).
fn create_bucket(router: &MultiServiceRouter, runtime: &tokio::runtime::Runtime, bucket: &str) {
    let (status, body) = send(
        router,
        runtime,
        "s3",
        "PUT",
        &format!("/{bucket}"),
        &[("content-type", "application/xml".to_string())],
        Bytes::from(format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
        )),
    );
    assert_eq!(
        status,
        200,
        "setup: CreateBucket({bucket:?}) must succeed, got {status} / {}",
        String::from_utf8_lossy(&body)
    );
}

/// Create an IAM role CodeBuild can assume and hand back its ARN.
///
/// The whole router carries IAM, so a project's `serviceRole` is resolved against it:
/// the role has to exist and trust `codebuild.amazonaws.com` before a project can be
/// stored at all (the rule `codebuild_service_role_trust.rs` owns).
fn codebuild_role(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
) -> String {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "codebuild.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string();
    let form = format!(
        "Action=CreateRole&Version=2010-05-08&RoleName={}&AssumeRolePolicyDocument={}",
        form_encode(name),
        form_encode(&document)
    );
    let (status, body) = send(
        router,
        runtime,
        "iam",
        "POST",
        "/",
        &[(
            "content-type",
            "application/x-www-form-urlencoded".to_string(),
        )],
        Bytes::from(form),
    );
    let body = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 200,
        "setup: CreateRole({name:?}) must succeed, got {status} / {body}"
    );
    element(&body, "Arn")
}

/// Create a batch-enabled project named `name` running as `role_arn`.
fn create_project(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    name: &str,
    role_arn: &str,
) {
    let (status, body) = codebuild(
        router,
        runtime,
        "CreateProject",
        json!({
            "name": name,
            "source": {"type": "NO_SOURCE", "buildspec": "version: 0.2"},
            "artifacts": {"type": "NO_ARTIFACTS"},
            "environment": {
                "type": "LINUX_CONTAINER",
                "image": "aws/codebuild/standard:7.0",
                "computeType": "BUILD_GENERAL1_SMALL",
            },
            "serviceRole": role_arn,
            "buildBatchConfig": {"serviceRole": role_arn, "timeoutInMins": 60},
        }),
    );
    assert_eq!(
        status, 200,
        "setup: CreateProject({name:?}) must succeed, got {status} / {body}"
    );
}

/// How many builds — or batch builds — the project holds, as its own listing reports.
fn started_count(
    router: &MultiServiceRouter,
    runtime: &tokio::runtime::Runtime,
    operation: &str,
    project: &str,
) -> usize {
    let (status, body) = codebuild(
        router,
        runtime,
        operation,
        json!({"projectName": project}),
    );
    assert_eq!(
        status, 200,
        "{operation}({project:?}) must succeed, got {status} / {body}"
    );
    body["ids"].as_array().map_or(0, Vec::len)
}

/// One row: the bucket the row names, whether it is created first, the `location` the
/// override carries, the operation the override arrives through, and whether the start is
/// admitted.
struct Row {
    case: &'static str,
    create: Option<&'static str>,
    bucket: &'static str,
    location: String,
    operation: &'static str,
    accepted: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a bucket S3 holds",
            create: Some("codebuild-artifacts-held"),
            bucket: "codebuild-artifacts-held",
            location: "codebuild-artifacts-held".to_string(),
            operation: "StartBuild",
            accepted: true,
        },
        Row {
            case: "a bucket S3 holds, with a prefix behind its name",
            create: None,
            bucket: "codebuild-artifacts-held",
            location: "codebuild-artifacts-held/out/here".to_string(),
            operation: "StartBuild",
            accepted: true,
        },
        Row {
            case: "a bucket nothing created",
            create: None,
            bucket: "codebuild-artifacts-ghost",
            location: "codebuild-artifacts-ghost".to_string(),
            operation: "StartBuild",
            accepted: false,
        },
        Row {
            case: "a bucket nothing created, with a prefix behind its name",
            create: None,
            bucket: "codebuild-artifacts-ghost",
            location: "codebuild-artifacts-ghost/out".to_string(),
            operation: "StartBuild",
            accepted: false,
        },
        Row {
            case: "a bucket whose name merely extends a held one",
            create: None,
            bucket: "codebuild-artifacts-held-too",
            location: "codebuild-artifacts-held-too".to_string(),
            operation: "StartBuild",
            accepted: false,
        },
        Row {
            case: "a batch build against a bucket S3 holds",
            create: None,
            bucket: "codebuild-artifacts-held",
            location: "codebuild-artifacts-held".to_string(),
            operation: "StartBuildBatch",
            accepted: true,
        },
        Row {
            case: "a batch build against a bucket nothing created",
            create: None,
            bucket: "codebuild-artifacts-ghost",
            location: "codebuild-artifacts-ghost".to_string(),
            operation: "StartBuildBatch",
            accepted: false,
        },
    ]
}

/// The listing that reports what one of the two start operations started.
fn listing_for(operation: &str) -> &'static str {
    if operation == "StartBuildBatch" {
        "ListBuildBatchesForProject"
    } else {
        "ListBuildsForProject"
    }
}

#[test]
fn a_build_starts_only_for_an_artifacts_bucket_s3_holds() {
    std::thread::Builder::new()
        .name("codebuild-artifacts-bucket".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");

            let role = codebuild_role(&router, &runtime, "codebuild-artifacts-role");
            let project = "codebuild-artifacts-proj";
            create_project(&router, &runtime, project, &role);

            for row in rows() {
                if let Some(bucket) = row.create {
                    create_bucket(&router, &runtime, bucket);
                }
                let listing = listing_for(row.operation);
                let before = started_count(&router, &runtime, listing, project);
                let (status, body) = codebuild(
                    &router,
                    &runtime,
                    row.operation,
                    json!({
                        "projectName": project,
                        "artifactsOverride": {"type": "S3", "location": row.location},
                    }),
                );
                let after = started_count(&router, &runtime, listing, project);

                if row.accepted {
                    assert_eq!(
                        status, 200,
                        "case {:?}: a bucket S3 holds must be accepted, got {status} / {body}",
                        row.case
                    );
                    assert_eq!(
                        after,
                        before + 1,
                        "case {:?}: an accepted start must leave exactly one new record",
                        row.case
                    );
                } else {
                    assert_eq!(
                        (
                            status,
                            body["__type"].as_str().unwrap_or_default().to_string(),
                            body["message"].as_str().unwrap_or_default().to_string(),
                        ),
                        (
                            400,
                            "InvalidInputException".to_string(),
                            format!("Bucket {} does not exist", row.bucket),
                        ),
                        "case {:?}: a bucket S3 does not hold must be refused with \
                         CodeBuild's absent-bucket sentence, got {status} / {body}",
                        row.case
                    );
                    assert_eq!(
                        after, before,
                        "case {:?}: a refused start must leave nothing behind",
                        row.case
                    );
                }
            }

            // The identical request, sent twice around one bucket's creation: refused
            // first, accepted after — so what decides is the bucket's existence and
            // nothing about the request itself.
            let bucket = "codebuild-artifacts-created-later";
            let request = json!({
                "projectName": project,
                "artifactsOverride": {"type": "S3", "location": bucket},
            });
            let before = started_count(&router, &runtime, "ListBuildsForProject", project);
            let (status, body) = codebuild(&router, &runtime, "StartBuild", request.clone());
            assert_eq!(
                status, 400,
                "before the bucket exists the start must be refused, got {status} / {body}"
            );
            assert_eq!(
                started_count(&router, &runtime, "ListBuildsForProject", project),
                before,
                "a refused start must leave no build behind"
            );

            create_bucket(&router, &runtime, bucket);

            let (status, body) = codebuild(&router, &runtime, "StartBuild", request);
            assert_eq!(
                status, 200,
                "once the bucket exists the identical start must succeed, got {status} / {body}"
            );
            assert_eq!(
                started_count(&router, &runtime, "ListBuildsForProject", project),
                before + 1,
                "the accepted start must leave exactly one new build"
            );
        })
        .expect("the sized thread spawns")
        .join()
        .expect("the suite runs to completion");
}
