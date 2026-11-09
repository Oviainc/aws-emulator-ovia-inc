// SPDX-License-Identifier: CC0-1.0

//! Who may reach the images and layers a private repository holds, over the awsJson1_1
//! wire.
//!
//! RULE under test: a private registry's contents are private. Every operation of this
//! service is signed, so every call is made by a principal, and a call that reads or writes
//! the images or layers of a repository is served only when a policy in force ALLOWS
//! `ecr:<OperationName>` on that repository — the calling principal's identity-based
//! policies, or the repository's own resource-based policy, either of which grants within one
//! account while an explicit `Deny` in either refuses. With neither, the call is refused with
//! the AWS-common `AccessDeniedException` (HTTP 400) before the operation reads or writes
//! anything: a refused pull reports no manifest, a refused push stores nothing, and adding
//! the missing grant — in either document — makes the identical call succeed.
//!
//! The principals below are made with IAM's and STS's OWN operations over the same engine
//! (`CreateUser` + `PutUserPolicy` / `CreatePolicy` + `AttachUserPolicy` + `CreateAccessKey`,
//! `CreateRole` + `PutRolePolicy` + `AssumeRole`), so the documents this service reads are
//! the ones those services wrote, and the decision is the shared
//! `service_common::iam_authorization` every other authorizing service answers from.
//!
//! It is asserted over TABLES that differ along what the rule generalises over: the
//! OPERATION asked for (every one that reaches a repository's images or layers, each with
//! its own action), what the caller's policies say (nothing about this repository, an
//! `Allow` of this action on it, a wildcard `Allow`, an action-prefix pattern, an `Allow` on
//! another repository, an explicit `Deny` beside a wildcard `Allow`), what the REPOSITORY's
//! policy says (nothing, an `Allow` naming the caller's role, an `Allow` naming another
//! principal, an explicit `Deny`), where a grant LIVES (inline on the principal, in an
//! attached managed policy) and which kind of principal signs (the account's own published
//! credential, an access key id no store holds, an IAM user, an assumed-role session).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, call_as, ok, ACCOUNT_ACCESS_KEY_ID, REGION, REGISTRY_ID};

/// The status and the wire error code an authorization refusal carries: the AWS "Common
/// Errors" table's `AccessDeniedException`, which this service's model does not declare.
const ACCESS_DENIED: (u16, &str) = (400, "AccessDeniedException");

/// The repository every row of the tables addresses.
const REPOSITORY: &str = "probe-repo";

/// A second repository of the same registry, which a grant scoped to it must not cover.
const OTHER_REPOSITORY: &str = "other-repo";

/// The role whose policies the operation table rewrites between legs.
const ROLE: &str = "puller";

/// Another role of the same account, which a repository policy naming IT must not share the
/// repository with the caller signed as [`ROLE`]'s session. It exists in IAM because a
/// repository policy's principals are resolved against this account before the document is
/// stored.
const BYSTANDER_ROLE: &str = "bystander";

/// A `layerDigest` / `imageDigest` inside the modelled `^[a-zA-Z0-9-_+.]+:[a-fA-F0-9]+$`, so
/// the boundary validator passes the request and the authorization decision is what answers
/// it.
const WELL_FORMED_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// An `uploadId` inside the modelled
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`.
const WELL_FORMED_UPLOAD_ID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";

/// A Docker image manifest V2 schema 2, which references no blob: what is under test is who
/// may reach a repository's images, not which blobs one needs.
const MANIFEST: &str = concat!(
    r#"{"schemaVersion":2,"#,
    r#""mediaType":"application/vnd.docker.distribution.manifest.v2+json"}"#
);

/// The tag the seeded image carries.
const TAG: &str = "probe";

/// The emulator these tests drive: one shared engine carrying this service beside the IAM and
/// STS services the principals and their policies are made with — the same set
/// `core/router`'s `build_router` assembles, narrowed to what this rule touches.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing: the `Authorization` header carries only the credential
        // scope, which is where a service reads the caller's access key id from.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One awsQuery call to IAM or STS, made as the account's own credential.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> String {
        let mut body = format!("Action={action}&Version={version}");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={ACCOUNT_ACCESS_KEY_ID}/20260101/{REGION}/{signing_name}\
                     /aws4_request, SignedHeaders=host, Signature=abc"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "case {action:?}: the setup call must succeed, got {status} with {text}"
        );
        text
    }

    /// Create an IAM user carrying `policy` as its inline policy (when one is given), mint a
    /// long-term access key for it, and answer that key id with the user's own ARN.
    async fn user(&self, user_name: &str, policy: Option<&str>) -> Principal {
        let created = self
            .query(
                "iam",
                "2010-05-08",
                "CreateUser",
                &[("UserName", user_name.to_string())],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateUser answers the user's Arn, got {created}"));
        if let Some(policy) = policy {
            self.query(
                "iam",
                "2010-05-08",
                "PutUserPolicy",
                &[
                    ("UserName", user_name.to_string()),
                    ("PolicyName", "inline".to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        let minted = self
            .query(
                "iam",
                "2010-05-08",
                "CreateAccessKey",
                &[("UserName", user_name.to_string())],
            )
            .await;
        Principal {
            access_key_id: xml_element(&minted, "AccessKeyId").unwrap_or_else(|| {
                panic!("CreateAccessKey answers an AccessKeyId, got {minted}")
            }),
            arn,
        }
    }

    /// Create a customer-managed policy carrying `document`, attach it to `user_name`, and
    /// answer the principal: the same grant as an inline policy, held somewhere else.
    async fn user_with_attached_policy(&self, user_name: &str, document: &str) -> Principal {
        let principal = self.user(user_name, None).await;
        let created = self
            .query(
                "iam",
                "2010-05-08",
                "CreatePolicy",
                &[
                    ("PolicyName", format!("{user_name}-managed")),
                    ("PolicyDocument", document.to_string()),
                ],
            )
            .await;
        let policy_arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreatePolicy answers the policy's Arn, got {created}"));
        self.query(
            "iam",
            "2010-05-08",
            "AttachUserPolicy",
            &[
                ("UserName", user_name.to_string()),
                ("PolicyArn", policy_arn),
            ],
        )
        .await;
        principal
    }

    /// Create the IAM role `role_name`, trusted by this account, and answer nothing: the
    /// session is minted by [`Self::assume`] and its policies rewritten by
    /// [`Self::put_role_policy`].
    async fn create_role(&self, role_name: &str) {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "AWS": format!("arn:aws:iam::{REGISTRY_ID}:root") },
                "Action": "sts:AssumeRole",
            }]
        })
        .to_string();
        self.query(
            "iam",
            "2010-05-08",
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
    }

    /// Write (or rewrite) the one inline policy of `role_name`, under the same policy name
    /// every time, so a put REPLACES what the role carried.
    async fn put_role_policy(&self, role_name: &str, policy: &str) {
        self.query(
            "iam",
            "2010-05-08",
            "PutRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    }

    /// Assume `role_name` through STS and answer the session's access key id with the session
    /// ARN a refusal must blame it by.
    async fn assume(&self, role_name: &str) -> Principal {
        let assumed = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    (
                        "RoleArn",
                        format!("arn:aws:iam::{REGISTRY_ID}:role/{role_name}"),
                    ),
                    ("RoleSessionName", "probe".to_string()),
                ],
            )
            .await;
        Principal {
            access_key_id: xml_element(&assumed, "AccessKeyId")
                .unwrap_or_else(|| panic!("AssumeRole answers an AccessKeyId, got {assumed}")),
            arn: xml_element(&assumed, "Arn")
                .unwrap_or_else(|| panic!("AssumeRole answers the session's Arn, got {assumed}")),
        }
    }

    /// Store `document` as the repository policy of `REPOSITORY`, as the account's own
    /// credential — the control-plane call that shares a repository.
    async fn share_repository(&self, case: &str, document: &str) {
        ok(
            &self.router,
            case,
            "SetRepositoryPolicy",
            &json!({ "repositoryName": REPOSITORY, "policyText": document }),
        )
        .await;
    }

    /// Remove the repository policy of `REPOSITORY` again, so the next leg starts from a
    /// repository that shares nothing.
    async fn unshare_repository(&self, case: &str) {
        ok(
            &self.router,
            case,
            "DeleteRepositoryPolicy",
            &json!({ "repositoryName": REPOSITORY }),
        )
        .await;
    }
}

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal must
/// blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the round
/// trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The ARN of a repository of this registry, spelled out here rather than imported so a wrong
/// resource in the decision fails this suite.
fn repository_arn(name: &str) -> String {
    format!("arn:aws:ecr:{REGION}:{REGISTRY_ID}:repository/{name}")
}

/// The IAM action an operation of this service is performed under.
fn action_of(operation: &str) -> String {
    format!("ecr:{operation}")
}

/// An identity-based policy allowing `action` on `resource`.
fn allow(action: &str, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }]
    })
    .to_string()
}

/// An identity-based policy allowing every action of this service on everything and
/// explicitly denying `denied`.
fn allow_all_but(denied: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": "ecr:*", "Resource": "*" },
            { "Effect": "Deny", "Action": denied, "Resource": "*" },
        ]
    })
    .to_string()
}

/// A repository policy with `effect` on `action` for `principal`: a resource-based statement
/// names WHO it is about, and leaving `Resource` out covers the repository it is attached to.
fn repository_policy(effect: &str, principal: &str, action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "ShareWithPrincipal",
            "Effect": effect,
            "Principal": { "AWS": principal },
            "Action": action,
        }]
    })
    .to_string()
}

/// The refusal AWS words a withheld grant with: the principal, the action, the resource and
/// why no grant was found.
fn no_policy_allows(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// The refusal for a call a document in force explicitly DENIES, which is a different fact
/// about the policies and so a different sentence. `document` is the kind of policy the deny
/// is in, as AWS names it.
fn explicit_deny(principal: &str, action: &str, resource: &str, document: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         with an explicit deny in {document}"
    )
}

/// Every operation that reaches the images or layers a repository holds, with a request body
/// the boundary validator passes — so what answers the call is the authorization decision and
/// not a malformed request. The actions are the ones an Amazon ECR repository policy governs.
fn image_operations() -> Vec<(&'static str, Value)> {
    vec![
        (
            "BatchCheckLayerAvailability",
            json!({ "repositoryName": REPOSITORY, "layerDigests": [WELL_FORMED_DIGEST] }),
        ),
        (
            "BatchDeleteImage",
            json!({ "repositoryName": REPOSITORY, "imageIds": [{ "imageTag": TAG }] }),
        ),
        (
            "BatchGetImage",
            json!({ "repositoryName": REPOSITORY, "imageIds": [{ "imageTag": TAG }] }),
        ),
        (
            "CompleteLayerUpload",
            json!({
                "repositoryName": REPOSITORY,
                "uploadId": WELL_FORMED_UPLOAD_ID,
                "layerDigests": [WELL_FORMED_DIGEST],
            }),
        ),
        (
            "DescribeImageReplicationStatus",
            json!({ "repositoryName": REPOSITORY, "imageId": { "imageTag": TAG } }),
        ),
        (
            "DescribeImageScanFindings",
            json!({ "repositoryName": REPOSITORY, "imageId": { "imageTag": TAG } }),
        ),
        (
            "DescribeImageSigningStatus",
            json!({ "repositoryName": REPOSITORY, "imageId": { "imageTag": TAG } }),
        ),
        ("DescribeImages", json!({ "repositoryName": REPOSITORY })),
        (
            "GetDownloadUrlForLayer",
            json!({ "repositoryName": REPOSITORY, "layerDigest": WELL_FORMED_DIGEST }),
        ),
        ("InitiateLayerUpload", json!({ "repositoryName": REPOSITORY })),
        (
            "ListImageReferrers",
            json!({
                "repositoryName": REPOSITORY,
                "subjectId": { "imageDigest": WELL_FORMED_DIGEST },
            }),
        ),
        ("ListImages", json!({ "repositoryName": REPOSITORY })),
        (
            "PutImage",
            json!({ "repositoryName": REPOSITORY, "imageManifest": MANIFEST }),
        ),
        (
            "StartImageScan",
            json!({ "repositoryName": REPOSITORY, "imageId": { "imageTag": TAG } }),
        ),
        (
            "UpdateImageStorageClass",
            json!({
                "repositoryName": REPOSITORY,
                "imageId": { "imageTag": TAG },
                "targetStorageClass": "ARCHIVE",
            }),
        ),
        (
            "UploadLayerPart",
            json!({
                "repositoryName": REPOSITORY,
                "uploadId": WELL_FORMED_UPLOAD_ID,
                "partFirstByte": 0,
                "partLastByte": 3,
                "layerPartBlob": "dGVzdA==",
            }),
        ),
    ]
}

/// Seed the two repositories and one tagged image in the addressed one, as the account's own
/// credential — the state every row then asks about.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    for name in [REPOSITORY, OTHER_REPOSITORY] {
        ok(
            &emulator.router,
            "the seeded registry",
            "CreateRepository",
            &json!({ "repositoryName": name }),
        )
        .await;
    }
    ok(
        &emulator.router,
        "the seeded image",
        "PutImage",
        &json!({ "repositoryName": REPOSITORY, "imageManifest": MANIFEST, "imageTag": TAG }),
    )
    .await;
    emulator
}

/// Assert `operation` was refused as unauthorized, carrying `message`.
async fn assert_denied(
    emulator: &Emulator,
    case: &str,
    access_key_id: &str,
    operation: &str,
    body: &Value,
    message: &str,
) {
    let answer = call_as(&emulator.router, access_key_id, operation, body).await;
    let (status, code) = ACCESS_DENIED;
    assert_eq!(
        (answer.status, answer.error_code()),
        (status, code),
        "case {case:?}: expected {status} {code} from {operation}, got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.message(),
        message,
        "case {case:?}: expected the refusal {message:?} from {operation}, got {:?}",
        answer.body
    );
}

/// Assert `operation` was NOT refused as unauthorized: the gate let it through, whatever the
/// operation itself then made of the request.
async fn assert_not_denied(
    emulator: &Emulator,
    case: &str,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let answer = call_as(&emulator.router, access_key_id, operation, body).await;
    assert_ne!(
        answer.error_code(),
        ACCESS_DENIED.1,
        "case {case:?}: expected {operation} to be authorized, got {:?}",
        answer.body
    );
    answer.body
}

/// RULE: for every operation that reaches a repository's images or layers, the call is
/// refused unless a policy in force allows that operation's own action on that repository —
/// and each of the two documents that can allow it does, while an explicit deny in either
/// refuses whatever else allows.
#[tokio::test]
async fn every_image_operation_is_decided_by_the_policies_in_force() {
    let emulator = seeded().await;
    emulator.create_role(ROLE).await;
    emulator.create_role(BYSTANDER_ROLE).await;
    let session = emulator.assume(ROLE).await;
    let role_arn = format!("arn:aws:iam::{REGISTRY_ID}:role/{ROLE}");

    for (operation, body) in image_operations() {
        let action = action_of(operation);
        let resource = repository_arn(REPOSITORY);

        // A grant scoped to ANOTHER repository of the same registry says nothing about this
        // one, so the call falls to IAM's default deny.
        let case = format!("{operation}: granted on another repository");
        emulator
            .put_role_policy(ROLE, &allow(&action, &repository_arn(OTHER_REPOSITORY)))
            .await;
        assert_denied(
            &emulator,
            &case,
            &session.access_key_id,
            operation,
            &body,
            &no_policy_allows(&session.arn, &action, &resource),
        )
        .await;

        // The repository's own policy, naming the role the session belongs to, is the other
        // document that can allow the call — with the identity-based policies unchanged.
        let case = format!("{operation}: shared with the caller's role by the repository");
        emulator
            .share_repository(&case, &repository_policy("Allow", &role_arn, &action))
            .await;
        assert_not_denied(&emulator, &case, &session.access_key_id, operation, &body).await;

        // A repository policy naming SOME OTHER principal of this account shares nothing
        // with this caller.
        let case = format!("{operation}: shared with another principal by the repository");
        emulator
            .share_repository(
                &case,
                &repository_policy(
                    "Allow",
                    &format!("arn:aws:iam::{REGISTRY_ID}:role/{BYSTANDER_ROLE}"),
                    &action,
                ),
            )
            .await;
        assert_denied(
            &emulator,
            &case,
            &session.access_key_id,
            operation,
            &body,
            &no_policy_allows(&session.arn, &action, &resource),
        )
        .await;
        emulator.unshare_repository(&case).await;

        // The caller's own policies allowing this action on this repository.
        let case = format!("{operation}: granted by the caller's own policy");
        emulator.put_role_policy(ROLE, &allow(&action, &resource)).await;
        assert_not_denied(&emulator, &case, &session.access_key_id, operation, &body).await;

        // An explicit deny in the identity-based policies wins over its own wildcard allow.
        let case = format!("{operation}: denied by the caller's own policy");
        emulator.put_role_policy(ROLE, &allow_all_but(&action)).await;
        assert_denied(
            &emulator,
            &case,
            &session.access_key_id,
            operation,
            &body,
            &explicit_deny(
                &session.arn,
                &action,
                &resource,
                "an identity-based policy",
            ),
        )
        .await;

        // An explicit deny in the REPOSITORY's policy refuses a caller its own policies
        // allow.
        let case = format!("{operation}: denied by the repository's policy");
        emulator.put_role_policy(ROLE, &allow(&action, &resource)).await;
        emulator
            .share_repository(&case, &repository_policy("Deny", &role_arn, &action))
            .await;
        assert_denied(
            &emulator,
            &case,
            &session.access_key_id,
            operation,
            &body,
            &explicit_deny(&session.arn, &action, &resource, "a resource-based policy"),
        )
        .await;
        emulator.unshare_repository(&case).await;
    }
}

/// RULE: the pull of a private repository is served only once something grants it — and the
/// grant may be the repository's own policy, which is what shares a repository with a
/// principal that has no identity-based grant. Taking the policy away refuses the identical
/// call again, and the refused pull reports nothing about what the repository holds.
#[tokio::test]
async fn a_private_repository_is_pulled_only_by_a_principal_something_grants() {
    let emulator = seeded().await;
    emulator.create_role(ROLE).await;
    // A role with permissions, none of them on this registry: a least-privilege principal
    // that has simply not been granted the pull.
    emulator
        .put_role_policy(
            ROLE,
            &allow("s3:GetObject", "arn:aws:s3:::some-bucket/*"),
        )
        .await;
    let session = emulator.assume(ROLE).await;
    let role_arn = format!("arn:aws:iam::{REGISTRY_ID}:role/{ROLE}");
    let action = action_of("BatchGetImage");
    let resource = repository_arn(REPOSITORY);
    let pull = json!({ "repositoryName": REPOSITORY, "imageIds": [{ "imageTag": TAG }] });

    let case = "no repository policy at all";
    let answer = call_as(
        &emulator.router,
        &session.access_key_id,
        "BatchGetImage",
        &pull,
    )
    .await;
    let (status, code) = ACCESS_DENIED;
    assert_eq!(
        (answer.status, answer.error_code()),
        (status, code),
        "case {case:?}: expected {status} {code}, got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.message(),
        no_policy_allows(&session.arn, &action, &resource),
        "case {case:?}: expected the withheld-grant refusal, got {:?}",
        answer.body
    );
    assert!(
        answer.body.get("images").is_none() && answer.body.get("failures").is_none(),
        "case {case:?}: a refused pull must report nothing about the repository, got {:?}",
        answer.body
    );

    let case = "a repository policy allowing the puller's role";
    emulator
        .share_repository(case, &repository_policy("Allow", &role_arn, &action))
        .await;
    let served = assert_not_denied(
        &emulator,
        case,
        &session.access_key_id,
        "BatchGetImage",
        &pull,
    )
    .await;
    assert_eq!(
        served["images"][0]["imageManifest"].as_str(),
        Some(MANIFEST),
        "case {case:?}: expected the stored manifest once the policy grants the pull, got {served:?}"
    );
    assert_eq!(
        served["images"][0]["imageId"]["imageTag"].as_str(),
        Some(TAG),
        "case {case:?}: expected the tag the request addressed, got {served:?}"
    );

    let case = "the repository policy taken away again";
    emulator.unshare_repository(case).await;
    assert_denied(
        &emulator,
        case,
        &session.access_key_id,
        "BatchGetImage",
        &pull,
        &no_policy_allows(&session.arn, &action, &resource),
    )
    .await;
}

/// RULE: the decision is taken BEFORE the operation writes anything, so a refused push leaves
/// the repository exactly as it was.
#[tokio::test]
async fn a_refused_push_stores_nothing() {
    let emulator = seeded().await;
    emulator.create_role(ROLE).await;
    let session = emulator.assume(ROLE).await;
    let before = ok(
        &emulator.router,
        "before the refused push",
        "ListImages",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;

    assert_denied(
        &emulator,
        "a push by a principal nothing grants",
        &session.access_key_id,
        "PutImage",
        &json!({
            "repositoryName": REPOSITORY,
            "imageManifest": r#"{"schemaVersion":2,"refused":"push"}"#,
            "imageTag": "refused",
        }),
        &no_policy_allows(
            &session.arn,
            &action_of("PutImage"),
            &repository_arn(REPOSITORY),
        ),
    )
    .await;

    let after = ok(
        &emulator.router,
        "after the refused push",
        "ListImages",
        &json!({ "repositoryName": REPOSITORY }),
    )
    .await;
    assert_eq!(
        after["imageIds"], before["imageIds"],
        "case \"a refused push\": expected the repository's images unchanged, got {:?}",
        after["imageIds"]
    );
}

/// RULE: the grant is read for the action and the resource its statement names, whatever
/// shape the statement takes.
#[tokio::test]
async fn a_grant_is_read_for_the_action_and_the_resource_it_names() {
    let emulator = seeded().await;
    emulator.create_role(ROLE).await;
    let session = emulator.assume(ROLE).await;
    let action = action_of("BatchGetImage");
    let resource = repository_arn(REPOSITORY);
    let pull = json!({ "repositoryName": REPOSITORY, "imageIds": [{ "imageTag": TAG }] });

    // (case, the inline policy in force on the role, whether the pull is served)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "this action on this repository",
            allow(&action, &resource),
            true,
        ),
        (
            "every action of this service on every resource",
            allow("ecr:*", "*"),
            true,
        ),
        (
            "an action-prefix pattern covering this action",
            allow("ecr:BatchGet*", "*"),
            true,
        ),
        (
            "a resource pattern covering this repository",
            allow(&action, &repository_arn("probe-*")),
            true,
        ),
        (
            "another action of this service",
            allow(&action_of("DescribeImages"), "*"),
            false,
        ),
        (
            "this action on another repository",
            allow(&action, &repository_arn(OTHER_REPOSITORY)),
            false,
        ),
        (
            "this action in another service's namespace",
            allow("ecr-public:BatchGetImage", "*"),
            false,
        ),
        (
            "an explicit deny beside a wildcard allow",
            allow_all_but(&action),
            false,
        ),
    ];

    for (case, document, served) in rows {
        emulator.put_role_policy(ROLE, &document).await;
        if served {
            assert_not_denied(&emulator, case, &session.access_key_id, "BatchGetImage", &pull)
                .await;
        } else {
            let expected = if case == "an explicit deny beside a wildcard allow" {
                explicit_deny(&session.arn, &action, &resource, "an identity-based policy")
            } else {
                no_policy_allows(&session.arn, &action, &resource)
            };
            assert_denied(
                &emulator,
                case,
                &session.access_key_id,
                "BatchGetImage",
                &pull,
                &expected,
            )
            .await;
        }
    }
}

/// RULE: which principal a request speaks for is read from the access key id it presents, and
/// the decision is made on the policies in force on THAT principal — wherever they live.
#[tokio::test]
async fn the_decision_is_made_on_the_principal_the_access_key_names() {
    let emulator = seeded().await;
    let action = action_of("BatchGetImage");
    let resource = repository_arn(REPOSITORY);
    let pull = json!({ "repositoryName": REPOSITORY, "imageIds": [{ "imageTag": TAG }] });

    // The account's own published credential holds no IAM policies — the position AWS
    // account root credentials are in — so it is served.
    assert_not_denied(
        &emulator,
        "the account's own published credential",
        ACCOUNT_ACCESS_KEY_ID,
        "BatchGetImage",
        &pull,
    )
    .await;
    // A key no store holds resolves to that same identity.
    assert_not_denied(
        &emulator,
        "an access key id no store holds",
        "AKIDNOSUCHKEYHERE",
        "BatchGetImage",
        &pull,
    )
    .await;

    let no_grant = emulator.user("reader-no-grant", None).await;
    assert_denied(
        &emulator,
        "an IAM user carrying no policy",
        &no_grant.access_key_id,
        "BatchGetImage",
        &pull,
        &no_policy_allows(&no_grant.arn, &action, &resource),
    )
    .await;

    let inline = emulator
        .user("reader-inline", Some(&allow(&action, &resource)))
        .await;
    assert_not_denied(
        &emulator,
        "an IAM user whose inline policy allows it",
        &inline.access_key_id,
        "BatchGetImage",
        &pull,
    )
    .await;

    let attached = emulator
        .user_with_attached_policy("reader-attached", &allow(&action, &resource))
        .await;
    assert_not_denied(
        &emulator,
        "an IAM user whose attached managed policy allows it",
        &attached.access_key_id,
        "BatchGetImage",
        &pull,
    )
    .await;

    emulator.create_role("session-no-grant").await;
    let session = emulator.assume("session-no-grant").await;
    assert_denied(
        &emulator,
        "an assumed-role session whose role carries no policy",
        &session.access_key_id,
        "BatchGetImage",
        &pull,
        &no_policy_allows(&session.arn, &action, &resource),
    )
    .await;
}

/// RULE: a repository the registry does not hold is still a repository nothing grants the
/// caller, so the decision — which is taken from the request — answers before the lookup
/// that would report whether it exists.
#[tokio::test]
async fn the_decision_does_not_depend_on_the_repository_existing() {
    let emulator = seeded().await;
    emulator.create_role(ROLE).await;
    let session = emulator.assume(ROLE).await;
    let absent = "never-created";
    assert_denied(
        &emulator,
        "a repository this registry does not hold",
        &session.access_key_id,
        "BatchGetImage",
        &json!({ "repositoryName": absent, "imageIds": [{ "imageTag": TAG }] }),
        &no_policy_allows(
            &session.arn,
            &action_of("BatchGetImage"),
            &repository_arn(absent),
        ),
    )
    .await;
    // The account's own credential, which nothing refuses, reads the absence instead.
    let answer = call(
        &emulator.router,
        "BatchGetImage",
        &json!({ "repositoryName": absent, "imageIds": [{ "imageTag": TAG }] }),
    )
    .await;
    assert_eq!(
        answer.error_code(),
        "RepositoryNotFoundException",
        "case \"a repository this registry does not hold\": expected the absence to be \
         reported to a caller nothing refuses, got {:?}",
        answer.body
    );
}
