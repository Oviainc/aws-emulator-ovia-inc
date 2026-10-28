// SPDX-License-Identifier: CC0-1.0

//! IAM authorization of this service's operations, over the awsJson1_1 wire.
//!
//! The rule under test: every operation of this service is signed, so every call is made by
//! a principal, and a principal may only perform an action some policy in force on it
//! ALLOWS — an action no identity-based policy of the calling principal allows is refused
//! with the AWS-common `AccessDeniedException` BEFORE the operation reads or writes
//! anything. So a refused mutation changes nothing, a refused read reports nothing about
//! what the registry holds, and adding the missing `Allow` makes the identical call succeed.
//! The decision is taken against the resource the action takes: the repository's ARN for an
//! operation that addresses one (each of them for the reader that may name several), and
//! every resource for a registry-wide action, which takes no resource type.
//!
//! The principals below are made with IAM's and STS's OWN operations over the same engine
//! (`CreateUser` + `PutUserPolicy` / `CreatePolicy` + `AttachUserPolicy` + `CreateAccessKey`,
//! `CreateRole` + `PutRolePolicy` + `AssumeRole`), so the documents this service reads are
//! the ones those services wrote, and the evaluation is
//! `service_common::iam_authorization`, shared with every other service that authorizes.
//!
//! It is asserted over TABLES that differ along what the rule generalises over: the
//! OPERATION asked for (every one of the 23, each with its own action and its own resource),
//! what the caller's policies say (nothing, an `Allow` of this action on this resource, a
//! wildcard `Allow`, an `Allow` of another action, an `Allow` on another repository, an
//! `Allow` whose action is a prefix pattern, an explicit `Deny` beside a wildcard `Allow`),
//! where those policies LIVE (inline on the principal, in an attached managed policy) and
//! which kind of principal signs (the account's own published credential, an IAM user, an
//! assumed-role session, an access key id no store holds).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::{
    repository_arn, API_REGION, ARN_NAMESPACE,
};

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "SpencerFrontendService";

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record for it, so it resolves to the account's ambient identity, which holds
/// no IAM policies and is allowed — the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "AKIDECRPUBLIC";

/// The status and the wire error code an authorization refusal carries: the AWS "Common
/// Errors" table's `AccessDeniedException`, which this service's model does not declare.
const ACCESS_DENIED: (u16, &str) = (400, "AccessDeniedException");

/// The resource a decision about a registry-wide action is made against, spelled with IAM's
/// own resource wildcard (`state::authorization::NO_RESOURCE`).
const NO_RESOURCE: &str = "*";

/// An `uploadId` inside the modelled
/// `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`, so the
/// boundary validator passes the request and the authorization decision is what answers it.
const WELL_FORMED_UPLOAD_ID: &str = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0";

/// A `layerDigest` inside the modelled `^[a-zA-Z0-9-_+.]+:[a-fA-F0-9]+$`.
const WELL_FORMED_LAYER_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// The emulator these tests drive: one shared engine carrying this service beside the IAM
/// and STS services the principals and their policies are made with — the same set
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
            service_com_amazonaws_ecrpublic_spencerfrontendservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One call to this service, with `access_key_id` in the SigV4 credential scope.
    async fn call(&self, access_key_id: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{API_REGION}\
                     /{ARN_NAMESPACE}/aws4_request, SignedHeaders=host;x-amz-target, \
                     Signature=00"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&payload)
                .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
        };
        (status, parsed)
    }

    /// One awsQuery call to IAM or STS, signed with the account's own credential.
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
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{API_REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(body))
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
            self.put_user_policy(user_name, policy).await;
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

    /// Write (or rewrite) the one inline policy of `user_name`, under the same policy name
    /// every time, so a put REPLACES what the principal carried.
    async fn put_user_policy(&self, user_name: &str, policy: &str) {
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

    /// Create a customer-managed policy carrying `document`, attach it to `user_name`, mint
    /// a key for that user and answer the principal — the same grant as an inline policy,
    /// held somewhere else.
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

    /// Create an IAM role carrying `policy` as its inline policy, assume it through STS, and
    /// answer the session's access key id with the session ARN.
    async fn assumed_role(&self, role_name: &str, policy: &str) -> Principal {
        let account = ::engine::emulator_account_id();
        let trust = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"AWS\":\"arn:aws:iam::{account}:root\"}},\
             \"Action\":\"sts:AssumeRole\"}}]}}"
        );
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
        let assumed = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    ("RoleArn", format!("arn:aws:iam::{account}:role/{role_name}")),
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
}

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal
/// must blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal — so a policy document's braces and quotes survive the
/// round trip.
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

/// The error code a response carries (`__type` on the awsJson protocols), and the empty
/// string for a response that is no error at all.
fn code_of(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The message an error response carries, under either casing a protocol envelope may write
/// it with.
fn message_of(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The IAM action an operation of this service is performed under.
fn action_of(operation: &str) -> String {
    format!("{ARN_NAMESPACE}:{operation}")
}

/// The placeholder a row of the grant-shape table writes where the ARN of the repository
/// that row addresses goes: the name is minted per row, so the document is filled in once
/// the row has one.
const THIS_REPOSITORY: &str = "{the repository this row addresses}";

/// A policy document allowing `action` on `resource`.
fn allow(action: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// A policy document allowing every action of this service on everything, and explicitly
/// denying `denied`.
fn allow_all_but(denied: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Action\":\"{ARN_NAMESPACE}:*\",\"Resource\":\"*\"}},\
         {{\"Effect\":\"Deny\",\"Action\":\"{denied}\",\"Resource\":\"*\"}}]}}"
    )
}

/// A policy document allowing each of `actions` on `Resource` `*` — the shape of a
/// least-privilege policy scoped to a handful of actions.
fn allow_actions(actions: &[&str]) -> String {
    let listed = actions
        .iter()
        .map(|action| format!("\"{action}\""))
        .collect::<Vec<String>>()
        .join(",");
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":[{listed}],\"Resource\":\"*\"}}]}}"
    )
}

/// The refusal AWS words an identity-based decision with when NO policy allows the action:
/// the principal, the action, the resource and the reason.
fn no_policy_allows(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// The refusal for a call a statement explicitly DENIES, which is a different fact about the
/// policies in force and so a different sentence.
fn explicit_deny(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         with an explicit deny in an identity-based policy"
    )
}

/// Assert that `reply` is the authorization refusal `expected_message`.
fn assert_refused(case: &str, reply: &(u16, Value), expected_message: &str) {
    let (status, body) = reply;
    assert_eq!(
        *status, ACCESS_DENIED.0,
        "case {case:?}: expected HTTP {} for a refused call, got {status} with {body}",
        ACCESS_DENIED.0
    );
    let code = code_of(body);
    assert!(
        code.contains(ACCESS_DENIED.1),
        "case {case:?}: expected {} , got {code:?} in {body}",
        ACCESS_DENIED.1
    );
    let message = message_of(body);
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected the refusal {expected_message:?}, got {message:?}"
    );
}

/// Assert that `reply` is NOT an authorization refusal: the grant was honoured, whatever the
/// operation then answered about the registry's own state.
fn assert_not_refused(case: &str, reply: &(u16, Value)) {
    let (status, body) = reply;
    let code = code_of(body);
    assert!(
        !code.contains(ACCESS_DENIED.1),
        "case {case:?}: expected the call to be authorized, got {status} with {body}"
    );
}

/// One row of the per-operation table: the operation, a request that satisfies its modelled
/// input, and the resource its decision — and its refusal — must name.
struct OperationRow {
    operation: &'static str,
    request: Value,
    resource: String,
}

/// Every operation of this service, each with a model-valid request and the resource it
/// addresses.
///
/// Each row names its own repository, so one row's effect cannot change what the next row is
/// answered; the registry-wide actions (the authorization token, the registry listing and
/// both legs of the registry catalog) take no resource type, so their decision is made
/// against [`NO_RESOURCE`].
fn operation_rows() -> Vec<OperationRow> {
    let on_repository = |operation: &'static str, repository: &str, request: Value| OperationRow {
        operation,
        request,
        resource: repository_arn(repository),
    };
    let registry_wide = |operation: &'static str, request: Value| OperationRow {
        operation,
        request,
        resource: NO_RESOURCE.to_string(),
    };
    vec![
        on_repository(
            "BatchCheckLayerAvailability",
            "authz-batch-check",
            json!({
                "repositoryName": "authz-batch-check",
                "layerDigests": [WELL_FORMED_LAYER_DIGEST],
            }),
        ),
        on_repository(
            "BatchDeleteImage",
            "authz-batch-delete",
            json!({
                "repositoryName": "authz-batch-delete",
                "imageIds": [{"imageTag": "latest"}],
            }),
        ),
        on_repository(
            "CompleteLayerUpload",
            "authz-complete-upload",
            json!({
                "repositoryName": "authz-complete-upload",
                "uploadId": WELL_FORMED_UPLOAD_ID,
                "layerDigests": [WELL_FORMED_LAYER_DIGEST],
            }),
        ),
        on_repository(
            "CreateRepository",
            "authz-create",
            json!({"repositoryName": "authz-create"}),
        ),
        on_repository(
            "DeleteRepository",
            "authz-delete",
            json!({"repositoryName": "authz-delete"}),
        ),
        on_repository(
            "DeleteRepositoryPolicy",
            "authz-delete-policy",
            json!({"repositoryName": "authz-delete-policy"}),
        ),
        on_repository(
            "DescribeImageTags",
            "authz-image-tags",
            json!({"repositoryName": "authz-image-tags"}),
        ),
        on_repository(
            "DescribeImages",
            "authz-images",
            json!({"repositoryName": "authz-images"}),
        ),
        registry_wide("DescribeRegistries", json!({})),
        // A listing that names no repository addresses the whole registry; the row below
        // that names one is decided against that repository.
        registry_wide("DescribeRepositories", json!({})),
        OperationRow {
            operation: "DescribeRepositories",
            request: json!({"repositoryNames": ["authz-describe-named"]}),
            resource: repository_arn("authz-describe-named"),
        },
        registry_wide("GetAuthorizationToken", json!({})),
        registry_wide("GetRegistryCatalogData", json!({})),
        on_repository(
            "GetRepositoryCatalogData",
            "authz-repo-catalog",
            json!({"repositoryName": "authz-repo-catalog"}),
        ),
        on_repository(
            "GetRepositoryPolicy",
            "authz-get-policy",
            json!({"repositoryName": "authz-get-policy"}),
        ),
        on_repository(
            "InitiateLayerUpload",
            "authz-initiate-upload",
            json!({"repositoryName": "authz-initiate-upload"}),
        ),
        OperationRow {
            operation: "ListTagsForResource",
            request: json!({"resourceArn": repository_arn("authz-list-tags")}),
            resource: repository_arn("authz-list-tags"),
        },
        on_repository(
            "PutImage",
            "authz-put-image",
            json!({
                "repositoryName": "authz-put-image",
                "imageManifest": "{\"schemaVersion\":2}",
            }),
        ),
        registry_wide(
            "PutRegistryCatalogData",
            json!({"displayName": "authz probe"}),
        ),
        on_repository(
            "PutRepositoryCatalogData",
            "authz-put-repo-catalog",
            json!({
                "repositoryName": "authz-put-repo-catalog",
                "catalogData": {"description": "authz probe"},
            }),
        ),
        on_repository(
            "SetRepositoryPolicy",
            "authz-set-policy",
            json!({
                "repositoryName": "authz-set-policy",
                "policyText": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            }),
        ),
        OperationRow {
            operation: "TagResource",
            request: json!({
                "resourceArn": repository_arn("authz-tag"),
                "tags": [{"Key": "probe", "Value": "value"}],
            }),
            resource: repository_arn("authz-tag"),
        },
        OperationRow {
            operation: "UntagResource",
            request: json!({
                "resourceArn": repository_arn("authz-untag"),
                "tagKeys": ["probe"],
            }),
            resource: repository_arn("authz-untag"),
        },
        on_repository(
            "UploadLayerPart",
            "authz-upload-part",
            json!({
                "repositoryName": "authz-upload-part",
                "uploadId": WELL_FORMED_UPLOAD_ID,
                "partFirstByte": 0,
                "partLastByte": 3,
                // awsJson1_1 carries a blob as base64 of its bytes.
                "layerPartBlob": aws_smithy_types::base64::encode(b"blob"),
            }),
        ),
    ]
}

#[tokio::test]
async fn every_operation_is_refused_unless_a_policy_allows_its_own_action() {
    let emulator = Emulator::new();
    // One principal holding no policy at all, which is what every row below is refused as.
    let bare = emulator.user("authz-bare", None).await;

    for (index, row) in operation_rows().into_iter().enumerate() {
        let action = action_of(row.operation);
        let case = format!("{} on {}", row.operation, row.resource);

        let refused = emulator
            .call(&bare.access_key_id, row.operation, &row.request)
            .await;
        assert_refused(
            &case,
            &refused,
            &no_policy_allows(&bare.arn, &action, &row.resource),
        );

        // The SAME call, by a principal whose only statement allows exactly that action on
        // exactly that resource: the grant is what makes the difference.
        let granted = emulator
            .user(
                &format!("authz-allowed-{index}"),
                Some(&allow(&action, &row.resource)),
            )
            .await;
        let served = emulator
            .call(&granted.access_key_id, row.operation, &row.request)
            .await;
        assert_not_refused(&case, &served);
    }
}

#[tokio::test]
async fn a_grant_is_read_for_the_action_and_the_resource_it_names() {
    let emulator = Emulator::new();
    let repository = "authz-grant-shapes";
    let other = repository_arn("authz-grant-elsewhere");
    let action = action_of("CreateRepository");

    // (case, the policy in force, whether the call is allowed, the reason it is refused for)
    let rows: Vec<(&str, Option<String>, bool, bool)> = vec![
        ("no policy at all", None, false, false),
        (
            "an Allow of this action on this repository",
            Some(allow(&action, THIS_REPOSITORY)),
            true,
            false,
        ),
        (
            "an Allow of this action on another repository",
            Some(allow(&action, &other)),
            false,
            false,
        ),
        (
            "an Allow of every action of this service on everything",
            Some(allow(&format!("{ARN_NAMESPACE}:*"), "*")),
            true,
            false,
        ),
        (
            "an Allow whose action is a prefix pattern",
            Some(allow(&format!("{ARN_NAMESPACE}:Create*"), "*")),
            true,
            false,
        ),
        (
            "an Allow of another action of this service",
            Some(allow(&action_of("DescribeRepositories"), "*")),
            false,
            false,
        ),
        (
            "an Allow of every action of another service",
            Some(allow("ecr:*", "*")),
            false,
            false,
        ),
        (
            "a wildcard Allow with an explicit Deny of this action",
            Some(allow_all_but(&action)),
            false,
            true,
        ),
    ];

    for (index, (case, policy, allowed, explicit)) in rows.into_iter().enumerate() {
        // A fresh repository name per row, so an allowed create cannot make the next row's
        // answer depend on what this one stored.
        let name = format!("{repository}-{index}");
        // The row that scopes its grant to the repository the call addresses names it
        // through the placeholder, since the name is minted per row here.
        let policy =
            policy.map(|document| document.replace(THIS_REPOSITORY, &repository_arn(&name)));
        let principal = emulator
            .user(&format!("authz-shape-{index}"), policy.as_deref())
            .await;
        let reply = emulator
            .call(
                &principal.access_key_id,
                "CreateRepository",
                &json!({"repositoryName": name}),
            )
            .await;
        if allowed {
            assert_eq!(
                reply.0, 200,
                "case {case:?}: expected the allowed create to be served, got {} with {}",
                reply.0, reply.1
            );
        } else {
            let expected = if explicit {
                explicit_deny(&principal.arn, &action, &repository_arn(&name))
            } else {
                no_policy_allows(&principal.arn, &action, &repository_arn(&name))
            };
            assert_refused(case, &reply, &expected);
        }
    }
}

#[tokio::test]
async fn a_grant_is_honoured_wherever_the_document_holding_it_lives() {
    let emulator = Emulator::new();
    let action = action_of("CreateRepository");

    // (case, the principal, the repository the row creates)
    let inline = emulator
        .user("authz-where-inline", Some(&allow(&action, "*")))
        .await;
    let attached = emulator
        .user_with_attached_policy("authz-where-attached", &allow(&action, "*"))
        .await;
    let session = emulator
        .assumed_role("authz-where-role", &allow(&action, "*"))
        .await;
    let rows: Vec<(&str, &Principal, &str)> = vec![
        ("an inline policy on the user", &inline, "authz-where-inline"),
        (
            "a managed policy attached to the user",
            &attached,
            "authz-where-attached",
        ),
        (
            "an inline policy on the role behind the session",
            &session,
            "authz-where-session",
        ),
    ];

    for (case, principal, repository) in rows {
        let reply = emulator
            .call(
                &principal.access_key_id,
                "CreateRepository",
                &json!({"repositoryName": repository}),
            )
            .await;
        assert_eq!(
            reply.0, 200,
            "case {case:?}: expected the granted create to be served, got {} with {}",
            reply.0, reply.1
        );
        // The same principal holds no grant for another action of this service, so the
        // document is being read for the action it names rather than taken as a blanket
        // grant.
        let elsewhere = emulator
            .call(
                &principal.access_key_id,
                "DeleteRepository",
                &json!({"repositoryName": repository}),
            )
            .await;
        assert_refused(
            case,
            &elsewhere,
            &no_policy_allows(
                &principal.arn,
                &action_of("DeleteRepository"),
                &repository_arn(repository),
            ),
        );
    }
}

#[tokio::test]
async fn the_accounts_own_credential_is_not_a_principal_the_store_holds_policies_for() {
    let emulator = Emulator::new();
    // (case, the access key id signing the call, the repository it creates)
    let rows: &[(&str, &str, &str)] = &[
        (
            "the account's own published credential",
            ACCOUNT_KEY,
            "authz-ambient",
        ),
        (
            "an access key id no store holds a record for",
            "AKIDNOTMINTEDHERE123",
            "authz-unknown-key",
        ),
    ];
    for (case, access_key_id, repository) in rows {
        let reply = emulator
            .call(
                access_key_id,
                "CreateRepository",
                &json!({"repositoryName": repository}),
            )
            .await;
        assert_eq!(
            reply.0, 200,
            "case {case:?}: expected the account's own credential to be served, got {} with {}",
            reply.0, reply.1
        );
    }
}

#[tokio::test]
async fn a_refused_create_stores_nothing_and_the_missing_grant_restores_the_identical_call() {
    let emulator = Emulator::new();
    let repository = "authz-least-privilege/app";
    let resource = repository_arn(repository);
    let create = action_of("CreateRepository");
    // The least-privilege policy of the recorded case: two read actions of this service and
    // nothing else.
    let scoped = [
        action_of("DescribeRepositories"),
        action_of("GetAuthorizationToken"),
    ];
    let scoped_actions: Vec<&str> = scoped.iter().map(String::as_str).collect();
    let principal = emulator
        .user(
            "authz-least-privilege",
            Some(&allow_actions(&scoped_actions)),
        )
        .await;

    // The positive control: the two actions the policy DOES allow are served.
    for operation in ["DescribeRepositories", "GetAuthorizationToken"] {
        let reply = emulator
            .call(&principal.access_key_id, operation, &json!({}))
            .await;
        assert_eq!(
            reply.0, 200,
            "case {operation:?}: the policy allows this action, so it must be served, \
             got {} with {}",
            reply.0, reply.1
        );
    }

    // The action the policy does not allow is refused, and the refusal names the resource
    // the request addressed.
    let request = json!({"repositoryName": repository});
    let refused = emulator
        .call(&principal.access_key_id, "CreateRepository", &request)
        .await;
    assert_refused(
        "a create the policy does not allow",
        &refused,
        &no_policy_allows(&principal.arn, &create, &resource),
    );

    // Nothing came into existence: the administrative reader does not hold the repository.
    let (status, body) = emulator
        .call(
            ACCOUNT_KEY,
            "DescribeRepositories",
            &json!({"repositoryNames": [repository]}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"the effect-absent read\": expected the refused create to have stored \
         nothing, got {status} with {body}"
    );
    assert!(
        code_of(&body).contains("RepositoryNotFoundException"),
        "case \"the effect-absent read\": expected RepositoryNotFoundException, got {body}"
    );

    // Adding the missing action to the SAME inline policy, with nothing else changed, makes
    // the identical call succeed.
    let mut widened = scoped_actions.clone();
    widened.push(create.as_str());
    emulator
        .put_user_policy("authz-least-privilege", &allow_actions(&widened))
        .await;
    let (status, created) = emulator
        .call(&principal.access_key_id, "CreateRepository", &request)
        .await;
    assert_eq!(
        status, 200,
        "case \"the identical call once the grant is added\": expected it to be served, \
         got {status} with {created}"
    );
    assert_eq!(
        created["repository"]["repositoryArn"].as_str(),
        Some(resource.as_str()),
        "case \"the identical call once the grant is added\": expected the repository's \
         ARN to be {resource:?}, got {created}"
    );

    // And the administrative reader now reports it, so the allowed call really wrote the
    // repository the refused one did not.
    let (status, listed) = emulator
        .call(
            ACCOUNT_KEY,
            "DescribeRepositories",
            &json!({"repositoryNames": [repository]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the terminal administrative read\": expected it to be served, got \
         {status} with {listed}"
    );
    assert_eq!(
        listed["repositories"][0]["repositoryName"].as_str(),
        Some(repository),
        "case \"the terminal administrative read\": expected the created repository, \
         got {listed}"
    );
}
