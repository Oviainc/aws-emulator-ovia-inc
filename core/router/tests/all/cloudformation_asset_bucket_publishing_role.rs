// SPDX-License-Identifier: CC0-1.0

//! A role a CloudFormation stack grants access to a bucket of the SAME stack, scoped by the
//! account that owns that bucket, can really use it — through an assumed-role session, as the
//! client that deployed the stack does.
//!
//! The rule under test: **the grant a stack embeds in a role is in force on a session of that
//! role exactly as written, including a `Condition` over a global condition key the serving
//! service answers for every request** — here `aws:ResourceAccount`, the account owning the
//! resource addressed, which is the key an asset-publishing role's policy scopes its bucket
//! actions by. So a session of the role may list, read and write the bucket when the clause
//! names the owning account, and may do none of those when it names another one.
//!
//! Four crates have to agree for that to hold — CloudFormation embeds the document in the
//! role, IAM holds it, STS mints the session and S3 decides the call — so this is the suite
//! that can pin it: a crate-local suite can only ever see one side.
//!
//! The rows differ along what the rule generalises over: which account the clause names (the
//! one owning the bucket, and another), and which call the session makes (a bucket listing, an
//! object read and an object write — a bucket-scoped resource and an object-scoped one).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder every awsQuery suite of this crate shares, and the reader that pulls
// one element's text out of an XML answer: written ONCE in `query_wire`, which its own header
// asks every new suite to call rather than copy.
use crate::query_wire::{element, form_encode};

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries, which owns every bucket it holds.
const ACCOUNT_ID: &str = "000000000000";

/// The long-term key every emulator publishes, which is the account's own credential: what
/// the stack is deployed with and what the session is assumed from.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// Long enough for the stack's clock-owed transitions to come due.
const SETTLE: Duration = Duration::from_millis(1_500);

/// How many times the suite re-reads a stack before giving up on it settling.
const SETTLE_READS: usize = 20;

/// The key the destination bucket holds, which the object rows read and overwrite.
const OBJECT_KEY: &str = "assets/held.txt";

/// The four services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and whose
/// credential is `access_key_id` — the key a service reads the caller's identity from. The
/// signature is never checked: request authentication is off.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Post one awsQuery form body to `signing_name`, presenting `access_key_id`.
async fn query(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    body: String,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(access_key_id, signing_name))
        .body(Bytes::from(body))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation over its awsQuery wire, as the account's own credential.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    query(router, AMBIENT_KEY, "cloudformation", body).await
}

/// One S3 request, presented as signed by `access_key_id`.
async fn s3(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("content-type", "application/octet-stream")
        .header("authorization", authorization(access_key_id, "s3"))
        .body(Bytes::copy_from_slice(body))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The template a row deploys: the asset bucket, a role the account may assume, and the
/// inline policy — an `AWS::IAM::Policy` naming the role, as an asset-publishing stack states
/// it — granting that role the bucket's actions, scoped to `scoped_to` as the account that
/// owns the resources addressed.
///
/// The bucket's own policy denies every non-TLS request, which is the shape such a stack
/// states beside the grant: it is in force on each call below too.
fn template(scoped_to: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Parameters": {
            "Qualifier": { "Type": "String", "Default": "probe" }
        },
        "Resources": {
            "StagingBucket": {
                "Type": "AWS::S3::Bucket",
                "Properties": {
                    "BucketName": {
                        "Fn::Sub": "assets-${Qualifier}-${AWS::AccountId}-${AWS::Region}"
                    }
                }
            },
            "StagingBucketPolicy": {
                "Type": "AWS::S3::BucketPolicy",
                "Properties": {
                    "Bucket": { "Ref": "StagingBucket" },
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Sid": "AllowSSLRequestsOnly",
                            "Action": "s3:*",
                            "Effect": "Deny",
                            "Principal": "*",
                            "Resource": [
                                { "Fn::Sub": "${StagingBucket.Arn}" },
                                { "Fn::Sub": "${StagingBucket.Arn}/*" }
                            ],
                            "Condition": { "Bool": { "aws:SecureTransport": "false" } }
                        }]
                    }
                }
            },
            "FilePublishingRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Action": "sts:AssumeRole",
                            "Effect": "Allow",
                            "Principal": { "AWS": { "Ref": "AWS::AccountId" } }
                        }]
                    },
                    "RoleName": {
                        "Fn::Sub": "publishing-role-${Qualifier}-${AWS::AccountId}-${AWS::Region}"
                    }
                }
            },
            "FilePublishingRoleDefaultPolicy": {
                "Type": "AWS::IAM::Policy",
                "Properties": {
                    "PolicyName": {
                        "Fn::Sub": "publishing-role-default-policy-${Qualifier}"
                    },
                    "Roles": [{ "Ref": "FilePublishingRole" }],
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Action": [
                                "s3:GetObject*",
                                "s3:GetBucket*",
                                "s3:GetEncryptionConfiguration",
                                "s3:List*",
                                "s3:DeleteObject*",
                                "s3:PutObject*",
                                "s3:Abort*"
                            ],
                            "Resource": [
                                { "Fn::Sub": "${StagingBucket.Arn}" },
                                { "Fn::Sub": "${StagingBucket.Arn}/*" }
                            ],
                            "Condition": {
                                "StringEquals": { "aws:ResourceAccount": [scoped_to] }
                            }
                        }]
                    }
                }
            }
        },
        "Outputs": {
            "BucketName": { "Value": { "Ref": "StagingBucket" } },
            "RoleName": { "Value": { "Ref": "FilePublishingRole" } }
        }
    })
    .to_string()
}

/// Deploy `template` under `stack_name` and answer the `DescribeStacks` body once the stack
/// has stopped moving.
async fn deploy(router: &MultiServiceRouter, stack_name: &str, template: &str) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack_name),
            ("TemplateBody", template),
            // The template names an IAM role, which is what this capability covers.
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateStack expected 200, got {status}: {body}");
    let mut described = String::new();
    for attempt in 0..SETTLE_READS {
        if attempt > 0 {
            tokio::time::sleep(SETTLE).await;
        }
        let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack_name)])
            .await;
        assert_eq!(
            status, 200,
            "DescribeStacks expected 200, got {status}: {body}"
        );
        described = body;
        if !described.contains("IN_PROGRESS") {
            break;
        }
    }
    described
}

/// Assume `role_name` and answer the access key id of the session STS minted.
async fn assume(router: &MultiServiceRouter, role_name: &str) -> String {
    let (status, body) = query(
        router,
        AMBIENT_KEY,
        "sts",
        format!(
            "Action=AssumeRole&Version=2011-06-15&RoleArn={}&RoleSessionName=publisher",
            form_encode(&format!("arn:aws:iam::{ACCOUNT_ID}:role/{role_name}"))
        ),
    )
    .await;
    assert_eq!(status, 200, "AssumeRole expected 200, got {status}: {body}");
    element(&body, "AccessKeyId")
}

/// WHICH call the session makes, over the bucket the stack created.
#[derive(Debug, Clone, Copy)]
enum Call {
    /// `ListObjectsV2`, authorized under `s3:ListBucket` on the bucket.
    ListBucket,
    /// `GetObject`, authorized under `s3:GetObject` on the key.
    GetObject,
    /// `PutObject`, authorized under `s3:PutObject` on the key.
    PutObject,
}

impl Call {
    /// Make the call as `access_key_id`.
    async fn drive(
        self,
        router: &MultiServiceRouter,
        access_key_id: &str,
        bucket: &str,
    ) -> (u16, String) {
        match self {
            Self::ListBucket => {
                s3(
                    router,
                    access_key_id,
                    "GET",
                    &format!("/{bucket}?list-type=2"),
                    b"",
                )
                .await
            }
            Self::GetObject => {
                s3(
                    router,
                    access_key_id,
                    "GET",
                    &format!("/{bucket}/{OBJECT_KEY}"),
                    b"",
                )
                .await
            }
            Self::PutObject => {
                s3(
                    router,
                    access_key_id,
                    "PUT",
                    &format!("/{bucket}/{OBJECT_KEY}"),
                    b"published-by-the-session",
                )
                .await
            }
        }
    }
}

/// RULE: the grant a stack embedded in a role, scoped by the account that owns the bucket, is
/// in force on a session of that role — every action it names is served when the clause names
/// the owning account, and none is when it names another one.
#[tokio::test]
async fn a_stack_granted_role_may_use_the_bucket_its_grant_is_scoped_to() {
    let rows: [(&str, &str, bool); 2] = [
        ("scoped to the account that owns the bucket", ACCOUNT_ID, true),
        ("scoped to another account", "111122223333", false),
    ];
    for (index, (case, scoped_to, allowed)) in rows.into_iter().enumerate() {
        let router = build_router();
        let stack_name = format!("PublishingStack{index}");
        let described = deploy(&router, &stack_name, &template(scoped_to)).await;
        assert!(
            described.contains("CREATE_COMPLETE"),
            "case {case:?}: the stack must reach CREATE_COMPLETE, got {described}"
        );
        let bucket = format!("assets-probe-{ACCOUNT_ID}-{REGION}");
        let role = format!("publishing-role-probe-{ACCOUNT_ID}-{REGION}");

        // The object the read rows address is written by the account's own credential, which
        // no identity policy bounds, so the rows decide only the SESSION's access.
        let (status, body) = s3(
            &router,
            AMBIENT_KEY,
            "PUT",
            &format!("/{bucket}/{OBJECT_KEY}"),
            b"seeded",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the object must be seeded, got {status}: {body}"
        );

        let session = assume(&router, &role).await;
        for call in [Call::ListBucket, Call::GetObject, Call::PutObject] {
            let (status, body) = call.drive(&router, &session, &bucket).await;
            let expected = if allowed { 200 } else { 403 };
            assert_eq!(
                status, expected,
                "case {case:?} ({call:?}): expected HTTP {expected} for the role session, got \
                 {status}: {body}"
            );
        }
    }
}
