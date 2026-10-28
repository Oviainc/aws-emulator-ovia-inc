// SPDX-License-Identifier: CC0-1.0

//! IAM authorization of this service's developer-credentials operations, over the
//! awsJson1_1 wire.
//!
//! The rule under test: every operation of this service that the model requires a SigV4
//! signature for is a developer-credentials call, and such a call is served only to a
//! caller whose identity-based IAM policies allow THAT operation's own action
//! (`cognito-identity:<OperationName>`) on the resource the request names — the identity
//! pool's ARN, or the account's identity pools in the request's region for a call that
//! names no single pool. Every other caller is refused with the AWS-common
//! `AccessDeniedException` (HTTP 403) BEFORE the operation reads or writes anything, so a
//! refused caller learns nothing about what the account holds and a refused mutation
//! changes nothing. The operations the model marks `@optionalAuth` — the unauthenticated
//! identity flow — are not judged: a request that carries no credential speaks for no
//! principal.
//!
//! The decision is the real one: the principals below are made with IAM's and STS's OWN
//! operations over the same engine (`CreateUser` + `PutUserPolicy` / `AttachUserPolicy` +
//! `CreateAccessKey`, `CreateRole` + `PutRolePolicy` + `AssumeRole`), so the documents
//! this service reads are the ones those services wrote, and the evaluation is
//! `service_common::iam_authorization`, shared with every other service that authorizes.
//!
//! It is asserted over TABLES that differ along what the rule generalises over: the
//! OPERATION asked for (every developer-credentials operation of this service, each with
//! its own action and its own resource), the kind of principal (the account's own
//! published credential, an IAM user, an assumed-role session, an access key id no store
//! holds), what its policies say (nothing, an `Allow` for this action on this resource, a
//! wildcard `Allow`, an `Allow` for another action, an `Allow` on another pool, an
//! explicit `Deny` beside a wildcard `Allow`) and where those policies LIVE (inline on the
//! principal, in an attached managed policy, in a policy this account does not hold).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::state::{
    identity_pool_arn, ARN_NAMESPACE, IDENTITY_POOL_RESOURCE_TYPE,
};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;

/// The region every call below is signed for, so the ARNs the decisions are made about are
/// the ones this region's pools carry.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record for it, so it resolves to the account's ambient identity, which holds
/// no IAM policies and is allowed — the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "EMULATORTESTKEY12345";

/// The wire error code of an authorization refusal, and the status it carries.
const ACCESS_DENIED: (u16, &str) = (403, "AccessDeniedException");

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
            service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService::new(
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
            .uri(format!("https://cognito-identity.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("AWSCognitoIdentityService.{operation}"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     cognito-identity/aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
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
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0"
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

    /// Create an identity pool with the account's own credential and answer its minted
    /// `IdentityPoolId`.
    async fn pool(&self, name: &str) -> String {
        let (status, created) = self
            .call(
                ACCOUNT_KEY,
                "CreateIdentityPool",
                &json!({ "IdentityPoolName": name, "AllowUnauthenticatedIdentities": true }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the setup create must succeed, got {status} with {created}"
        );
        created["IdentityPoolId"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateIdentityPool answers an IdentityPoolId: {created}"))
            .to_string()
    }

    /// Create an IAM user carrying `policy` as its inline policy (when one is given), mint
    /// a long-term access key for it, and answer that key id with the user's own ARN.
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

    /// Create an IAM role carrying `policy` as its inline policy (when one is given),
    /// assume it through STS, and answer the session's access key id with the session ARN.
    async fn assumed_role(&self, role_name: &str, policy: Option<&str>) -> Principal {
        let trust = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"AWS\":\"arn:aws:iam::{}:root\"}},\
             \"Action\":\"sts:AssumeRole\"}}]}}",
            ::engine::emulator_account_id()
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
        if let Some(policy) = policy {
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
        let assumed = self
            .query(
                "sts",
                "2011-06-15",
                "AssumeRole",
                &[
                    (
                        "RoleArn",
                        format!(
                            "arn:aws:iam::{}:role/{role_name}",
                            ::engine::emulator_account_id()
                        ),
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

    /// Create a customer-managed policy carrying `document` and answer the ARN IAM minted
    /// for it — the handle an attachment names.
    async fn managed_policy(&self, policy_name: &str, document: &str) -> String {
        let created = self
            .query(
                "iam",
                "2010-05-08",
                "CreatePolicy",
                &[
                    ("PolicyName", policy_name.to_string()),
                    ("PolicyDocument", document.to_string()),
                ],
            )
            .await;
        xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreatePolicy answers the policy's Arn, got {created}"))
    }

    /// Attach the managed policy `policy_arn` to the user `user_name`.
    async fn attach_user_policy(&self, user_name: &str, policy_arn: &str) {
        self.query(
            "iam",
            "2010-05-08",
            "AttachUserPolicy",
            &[
                ("UserName", user_name.to_string()),
                ("PolicyArn", policy_arn.to_string()),
            ],
        )
        .await;
    }

    /// Attach the managed policy `policy_arn` to the role `role_name`.
    async fn attach_role_policy(&self, role_name: &str, policy_arn: &str) {
        self.query(
            "iam",
            "2010-05-08",
            "AttachRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyArn", policy_arn.to_string()),
            ],
        )
        .await;
    }
}

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal
/// must blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal — so a policy document's braces and quotes survive
/// the round trip. `pub(crate)` so every suite in this target that drives an awsQuery setup
/// call encodes a parameter the one way.
pub(crate) fn form_encode(value: &str) -> String {
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

/// The text of the first `<name>…</name>` element of an awsQuery response. `pub(crate)` for
/// the same reason as [`form_encode`].
pub(crate) fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// A policy document allowing `action` on `resource`.
fn allow(action: &str, resource: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":\"{action}\",\"Resource\":\"{resource}\"}}]}}"
    )
}

/// A policy document that only DENIES `action` on every resource, with no `Allow` of its
/// own — so a principal carrying it alongside a grant is still refused.
fn deny(action: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Deny\",\
         \"Action\":\"{action}\",\"Resource\":\"*\"}}]}}"
    )
}

/// A policy document allowing every action of this service on everything, and explicitly
/// denying `denied`.
fn allow_all_but(denied: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Action\":\"cognito-identity:*\",\"Resource\":\"*\"}},\
         {{\"Effect\":\"Deny\",\"Action\":\"{denied}\",\"Resource\":\"*\"}}]}}"
    )
}

/// The IAM action `operation` is performed under.
fn action_of(operation: &str) -> String {
    format!("{ARN_NAMESPACE}:{operation}")
}

/// The resource a call that names no single pool is decided against: the account's identity
/// pools in [`REGION`].
fn any_pool_arn() -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        REGION,
        &format!("{IDENTITY_POOL_RESOURCE_TYPE}/*"),
    )
}

/// The refusal AWS words an identity-based decision with when no policy allows the action:
/// the principal, the action, the resource and the reason.
fn no_policy_allows(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// The error code a response carries (`__type` on the awsJson protocols), and the empty
/// string for a response that is no error at all.
///
/// The sibling suites read the code through `identity_pool_flow::error_code`, which
/// insists on finding one; this suite also asserts that a SERVED call is NOT an
/// authorization refusal, so it needs a reader that can answer about a success body.
fn code_of(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// The message an error response carries, under either casing a protocol envelope may
/// write it with.
fn message_of(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// One row of the per-operation table: the operation, the request it is driven with (the
/// pool the row created is already filled in), and the resource the decision — and the
/// refusal — must name.
struct OperationRow {
    operation: &'static str,
    request: Value,
    resource: String,
}

/// Every developer-credentials operation of this service, each with a request that
/// satisfies its modelled input and the resource it addresses.
///
/// `pool_id` is a pool of this account the row may read, write or destroy: each row gets
/// its own, so a row that deletes it does not disturb the next one. The stub this service
/// still answers 501 for (`DeleteIdentities`) is left out: it decides nothing yet, and the
/// `@optionalAuth` identity flow is covered by its own test below. `DescribeIdentity` is
/// left out for a different reason: every row here is driven with a POOL id, and that
/// operation addresses an IDENTITY, so the resource its decision is made against is the
/// pool of an identity a row would first have to mint — which is why it has a case of its
/// own ([`describe_identity_is_authorized_on_the_pool_its_identity_belongs_to`]) rather
/// than a row here.
fn operation_rows(pool_id: &str) -> Vec<OperationRow> {
    let pool_arn = identity_pool_arn(pool_id);
    let named = |operation: &'static str, request: Value| OperationRow {
        operation,
        request,
        resource: pool_arn.clone(),
    };
    vec![
        OperationRow {
            operation: "CreateIdentityPool",
            request: json!({
                "IdentityPoolName": "authorization-probe-create",
                "AllowUnauthenticatedIdentities": true,
            }),
            resource: any_pool_arn(),
        },
        OperationRow {
            operation: "ListIdentityPools",
            request: json!({ "MaxResults": 10 }),
            resource: any_pool_arn(),
        },
        named("DescribeIdentityPool", json!({ "IdentityPoolId": pool_id })),
        named(
            "UpdateIdentityPool",
            json!({
                "IdentityPoolId": pool_id,
                "IdentityPoolName": "authorization-probe-updated",
                "AllowUnauthenticatedIdentities": false,
            }),
        ),
        named(
            "ListIdentities",
            json!({ "IdentityPoolId": pool_id, "MaxResults": 10 }),
        ),
        named("GetIdentityPoolRoles", json!({ "IdentityPoolId": pool_id })),
        named(
            "SetIdentityPoolRoles",
            json!({ "IdentityPoolId": pool_id, "Roles": {} }),
        ),
        named(
            "GetPrincipalTagAttributeMap",
            json!({ "IdentityPoolId": pool_id, "IdentityProviderName": "graph.facebook.com" }),
        ),
        named(
            "SetPrincipalTagAttributeMap",
            json!({
                "IdentityPoolId": pool_id,
                "IdentityProviderName": "graph.facebook.com",
                "UseDefaults": true,
            }),
        ),
        named(
            "LookupDeveloperIdentity",
            json!({ "IdentityPoolId": pool_id, "DeveloperUserIdentifier": "probe-user" }),
        ),
        named(
            "UnlinkDeveloperIdentity",
            json!({
                "IdentityPoolId": pool_id,
                "IdentityId": pool_id,
                "DeveloperProviderName": "login.probe",
                "DeveloperUserIdentifier": "probe-user",
            }),
        ),
        named(
            "GetOpenIdTokenForDeveloperIdentity",
            json!({
                "IdentityPoolId": pool_id,
                "Logins": { "login.probe": "probe-user" },
            }),
        ),
        named(
            "MergeDeveloperIdentities",
            json!({
                "IdentityPoolId": pool_id,
                "DeveloperProviderName": "login.probe",
                "SourceUserIdentifier": "probe-source-user",
                "DestinationUserIdentifier": "probe-destination-user",
            }),
        ),
        OperationRow {
            operation: "TagResource",
            request: json!({ "ResourceArn": pool_arn, "Tags": { "probe": "value" } }),
            resource: pool_arn.clone(),
        },
        OperationRow {
            operation: "UntagResource",
            request: json!({ "ResourceArn": pool_arn, "TagKeys": ["probe"] }),
            resource: pool_arn.clone(),
        },
        OperationRow {
            operation: "ListTagsForResource",
            request: json!({ "ResourceArn": pool_arn }),
            resource: pool_arn.clone(),
        },
        named("DeleteIdentityPool", json!({ "IdentityPoolId": pool_id })),
    ]
}

#[tokio::test]
async fn every_developer_credentials_operation_is_authorized_on_its_own_action() {
    let emulator = Emulator::new();
    // One principal holding nothing at all, which is what every row below is refused as.
    let bare = emulator.user("cognito-authz-bare", None).await;

    // The row set is built against one throwaway pool first, so the table's shape — which
    // operations it covers — is independent of the pools the rows then run against.
    let operations: Vec<&'static str> = operation_rows(&emulator.pool("authorization-shape").await)
        .into_iter()
        .map(|row| row.operation)
        .collect();

    for (index, operation) in operations.iter().enumerate() {
        // A pool of this account per row, so a row that writes or destroys it leaves the
        // next row's pool untouched.
        let pool_id = emulator.pool(&format!("authorization-probe-{index}")).await;
        let row = operation_rows(&pool_id)
            .into_iter()
            .find(|row| row.operation == *operation)
            .expect("the row set is built from the same operations");
        let action = action_of(row.operation);

        // A caller holding no policy at all is refused, with AWS's own wording naming the
        // principal, the action and the resource the decision was made about.
        let (status, body) = emulator
            .call(&bare.access_key_id, row.operation, &row.request)
            .await;
        assert_eq!(
            (status, code_of(&body).as_str()),
            ACCESS_DENIED,
            "case {operation:?}: a caller with no policy must be refused, got {status} with {body}"
        );
        assert_eq!(
            message_of(&body),
            no_policy_allows(&bare.arn, &action, &row.resource),
            "case {operation:?}: the refusal must name the principal, the action and the \
             resource, got {body}"
        );

        // Nothing the refused call asked for happened: the pool it named is still there,
        // read back by the account's own credential rather than by the refused caller.
        let (status, described) = emulator
            .call(
                ACCOUNT_KEY,
                "DescribeIdentityPool",
                &json!({ "IdentityPoolId": &pool_id }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: the pool must survive a refused call, got {status} with \
             {described}"
        );

        // The identical call from a principal allowed exactly this action on exactly this
        // resource is not refused: the authorization boundary is what answered above.
        let granted = emulator
            .user(
                &format!("cognito-authz-granted-{index}"),
                Some(&allow(&action, &row.resource)),
            )
            .await;
        let (status, body) = emulator
            .call(&granted.access_key_id, row.operation, &row.request)
            .await;
        assert_ne!(
            (status, code_of(&body).as_str()),
            ACCESS_DENIED,
            "case {operation:?}: a caller allowed {action} on {} must not be refused by \
             authorization, got {status} with {body}",
            row.resource
        );
    }
}

#[tokio::test]
async fn the_refusal_of_a_delete_is_the_authorization_boundary() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("authorization-delete-probe").await;
    let pool_arn = identity_pool_arn(&pool_id);
    let action = action_of("DeleteIdentityPool");
    let probe = emulator.user("cognito-authz-delete", None).await;
    let request = json!({ "IdentityPoolId": &pool_id });

    // A caller with no permission at all destroys nothing.
    let (status, body) = emulator
        .call(&probe.access_key_id, "DeleteIdentityPool", &request)
        .await;
    assert_eq!(
        (status, code_of(&body).as_str()),
        ACCESS_DENIED,
        "case \"the zero-permission delete\": expected the authorization refusal, got {status} \
         with {body}"
    );
    assert_eq!(
        message_of(&body),
        no_policy_allows(&probe.arn, &action, &pool_arn),
        "case \"the zero-permission delete\": AWS's wording, got {body}"
    );
    let (status, described) = emulator
        .call(ACCOUNT_KEY, "DescribeIdentityPool", &request)
        .await;
    assert_eq!(
        (status, described["IdentityPoolId"].as_str()),
        (200, Some(pool_id.as_str())),
        "case \"after the refusal\": the owner must still read the pool back, got {status} with \
         {described}"
    );

    // The SAME call, from the SAME principal, once a policy allows exactly that action on
    // exactly that pool: the refusal was the authorization boundary and not a blanket
    // rejection of the operation.
    let grant = emulator
        .managed_policy("cognito-authz-delete-grant", &allow(&action, &pool_arn))
        .await;
    emulator
        .attach_user_policy("cognito-authz-delete", &grant)
        .await;
    let (status, body) = emulator
        .call(&probe.access_key_id, "DeleteIdentityPool", &request)
        .await;
    assert_eq!(
        status, 200,
        "case \"the granted delete\": the identical call must now be served, got {status} with \
         {body}"
    );

    // After the delete the pool is gone for everyone, and both the read and the repeated
    // delete say so in the same words.
    for operation in ["DescribeIdentityPool", "DeleteIdentityPool"] {
        let (status, body) = emulator.call(ACCOUNT_KEY, operation, &request).await;
        assert_eq!(
            (status, code_of(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {operation:?}: a deleted pool is not found, got {status} with {body}"
        );
        assert_eq!(
            message_of(&body),
            format!("IdentityPool '{pool_id}' not found."),
            "case {operation:?}: the not-found wording, got {body}"
        );
    }
}

/// One row of the principal table: who calls, and whether the rule serves it.
struct PrincipalRow {
    label: &'static str,
    access_key_id: String,
    allowed: bool,
}

#[tokio::test]
async fn the_decision_is_made_from_every_identity_policy_in_force() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("authorization-principals").await;
    let pool_arn = identity_pool_arn(&pool_id);
    let action = action_of("DescribeIdentityPool");
    let other_pool_arn = identity_pool_arn("us-east-1:11111111-1111-1111-1111-111111111111");

    // The documents the rows carry in an ATTACHED managed policy rather than inline, so
    // where a grant lives is shown to be no part of the rule.
    let attached_grant = emulator
        .managed_policy("cognito-authz-attached-grant", &allow(&action, &pool_arn))
        .await;
    let attached_deny = emulator
        .managed_policy("cognito-authz-attached-deny", &deny(&action))
        .await;
    // An AWS-managed policy's account segment is IAM's own `aws`, so this account holds no
    // document for it: a principal carrying one is allowed, because refusing on evidence
    // this emulator cannot read would deny access AWS permits.
    let aws_managed = format!(
        "arn:aws:iam::{}:policy/ReadOnlyAccess",
        iam_store::AWS_MANAGED_POLICY_ACCOUNT
    );

    let attached_user = emulator
        .user("cognito-authz-user-attached", None)
        .await
        .access_key_id;
    emulator
        .attach_user_policy("cognito-authz-user-attached", &attached_grant)
        .await;
    let inline_and_attached_deny = emulator
        .user(
            "cognito-authz-user-both",
            Some(&allow("cognito-identity:*", "*")),
        )
        .await
        .access_key_id;
    emulator
        .attach_user_policy("cognito-authz-user-both", &attached_deny)
        .await;
    let aws_managed_user = emulator
        .user("cognito-authz-user-aws-managed", None)
        .await
        .access_key_id;
    emulator
        .attach_user_policy("cognito-authz-user-aws-managed", &aws_managed)
        .await;
    let session_attached = emulator
        .assumed_role("cognito-authz-role-attached", None)
        .await
        .access_key_id;
    emulator
        .attach_role_policy("cognito-authz-role-attached", &attached_grant)
        .await;

    let rows: Vec<PrincipalRow> = vec![
        PrincipalRow {
            label: "the account's own credential",
            access_key_id: ACCOUNT_KEY.to_string(),
            allowed: true,
        },
        PrincipalRow {
            label: "an access key id no store holds",
            access_key_id: "AKIDNOBODYHOLDS".to_string(),
            allowed: true,
        },
        PrincipalRow {
            label: "an IAM user with no policy",
            access_key_id: emulator
                .user("cognito-authz-user-bare", None)
                .await
                .access_key_id,
            allowed: false,
        },
        PrincipalRow {
            label: "an IAM user allowed this action on this pool",
            access_key_id: emulator
                .user("cognito-authz-user-allowed", Some(&allow(&action, &pool_arn)))
                .await
                .access_key_id,
            allowed: true,
        },
        PrincipalRow {
            label: "an IAM user with a wildcard grant",
            access_key_id: emulator
                .user(
                    "cognito-authz-user-wildcard",
                    Some(&allow("cognito-identity:*", "*")),
                )
                .await
                .access_key_id,
            allowed: true,
        },
        PrincipalRow {
            label: "an IAM user allowed only another action",
            access_key_id: emulator
                .user(
                    "cognito-authz-user-other-action",
                    Some(&allow(&action_of("DeleteIdentityPool"), &pool_arn)),
                )
                .await
                .access_key_id,
            allowed: false,
        },
        PrincipalRow {
            label: "an IAM user allowed this action on another pool",
            access_key_id: emulator
                .user(
                    "cognito-authz-user-other-pool",
                    Some(&allow(&action, &other_pool_arn)),
                )
                .await
                .access_key_id,
            allowed: false,
        },
        PrincipalRow {
            label: "an IAM user explicitly denied beside a wildcard allow",
            access_key_id: emulator
                .user("cognito-authz-user-denied", Some(&allow_all_but(&action)))
                .await
                .access_key_id,
            allowed: false,
        },
        PrincipalRow {
            label: "an IAM user whose grant is in an attached managed policy",
            access_key_id: attached_user,
            allowed: true,
        },
        PrincipalRow {
            label: "an IAM user with an inline allow beside an attached deny",
            access_key_id: inline_and_attached_deny,
            allowed: false,
        },
        PrincipalRow {
            label: "an IAM user carrying a policy this account does not hold",
            access_key_id: aws_managed_user,
            allowed: true,
        },
        PrincipalRow {
            label: "an assumed-role session with no policy",
            access_key_id: emulator
                .assumed_role("cognito-authz-role-bare", None)
                .await
                .access_key_id,
            allowed: false,
        },
        PrincipalRow {
            label: "an assumed-role session allowed this action on this pool",
            access_key_id: emulator
                .assumed_role(
                    "cognito-authz-role-allowed",
                    Some(&allow(&action, &pool_arn)),
                )
                .await
                .access_key_id,
            allowed: true,
        },
        PrincipalRow {
            label: "an assumed-role session whose grant is in an attached managed policy",
            access_key_id: session_attached,
            allowed: true,
        },
    ];

    for row in &rows {
        let label = row.label;
        let (status, body) = emulator
            .call(
                &row.access_key_id,
                "DescribeIdentityPool",
                &json!({ "IdentityPoolId": &pool_id }),
            )
            .await;
        if row.allowed {
            assert_eq!(
                (status, body["IdentityPoolId"].as_str()),
                (200, Some(pool_id.as_str())),
                "case {label:?}: a principal whose documents allow the action must be served, \
                 got {status} with {body}"
            );
        } else {
            assert_eq!(
                (status, code_of(&body).as_str()),
                ACCESS_DENIED,
                "case {label:?}: a principal whose documents do not allow the action must be \
                 refused, got {status} with {body}"
            );
            assert!(
                message_of(&body).contains(&action)
                    && message_of(&body).contains(&pool_arn)
                    && !message_of(&body).contains("authorization-principals"),
                "case {label:?}: the refusal names the action and the resource and nothing of \
                 the pool's configuration, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn the_unauthenticated_identity_flow_is_not_judged_on_iam_policies() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("authorization-identity-flow").await;
    let bare = emulator.user("cognito-authz-identity-flow", None).await;

    // A vend is a session of the pool's role, so the pool is given one whose trust policy
    // admits its own users. What that session STANDS FOR is `super::vended_role_session`'s
    // rule; what this case asserts is that the vend is not judged on the identity policies
    // of the credential that happened to sign it.
    let role_trust = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Federated\":\"cognito-identity.amazonaws.com\"}},\
         \"Action\":\"sts:AssumeRoleWithWebIdentity\",\"Condition\":{{\"StringEquals\":\
         {{\"cognito-identity.amazonaws.com:aud\":\"{pool_id}\"}}}}}}]}}"
    );
    let created_role = emulator
        .query(
            "iam",
            "2010-05-08",
            "CreateRole",
            &[
                (
                    "RoleName",
                    "cognito-authz-identity-flow-unauth".to_string(),
                ),
                ("AssumeRolePolicyDocument", role_trust),
            ],
        )
        .await;
    let role_arn = xml_element(&created_role, "Arn")
        .unwrap_or_else(|| panic!("CreateRole answers the role's Arn, got {created_role}"));
    let (status, configured) = emulator
        .call(
            ACCOUNT_KEY,
            "SetIdentityPoolRoles",
            &json!({ "IdentityPoolId": &pool_id, "Roles": { "unauthenticated": role_arn } }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the setup role assignment must succeed, got {status} with {configured}"
    );

    // `GetId` and `GetCredentialsForIdentity` are modelled `@optionalAuth`
    // (`smithy_query(query="digest")`): they are the calls a client makes BEFORE it holds
    // any credential, so they are not judged on the identity policies of whatever
    // credential happened to sign them.
    let (status, got) = emulator
        .call(
            &bare.access_key_id,
            "GetId",
            &json!({ "IdentityPoolId": &pool_id }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"GetId\": the unauthenticated flow must be served, got {status} with {got}"
    );
    let identity_id = got["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("GetId answers an IdentityId: {got}"))
        .to_string();

    let (status, vended) = emulator
        .call(
            &bare.access_key_id,
            "GetCredentialsForIdentity",
            &json!({ "IdentityId": identity_id }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"GetCredentialsForIdentity\": the unauthenticated flow must be served, got \
         {status} with {vended}"
    );
}

/// A well-formed `IdentityId` that nothing ever minted: `len 1..55`, `^[\w-]+:[0-9a-f-]+$`,
/// from this service's `IdentityId` shape
/// (`smithy_query(query="shape",shape="IdentityId")`), so the request reaches the handler
/// rather than being refused at the boundary.
const ABSENT_IDENTITY_ID: &str = "us-east-1:44444444-4444-4444-4444-444444444444";

/// `DescribeIdentity` is a developer-credentials call like every row of the table above,
/// and the resource its decision is made against is the pool of the IDENTITY the request
/// names — the account's identity pools in the request's region when this service holds no
/// such identity, so a caller no policy allows is refused BEFORE the identity is read and
/// learns nothing about whether it exists.
///
/// It is its own case rather than a row of [`operation_rows`] because the request has to
/// name an identity a pool has first minted.
#[tokio::test]
async fn describe_identity_is_authorized_on_the_pool_its_identity_belongs_to() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool("authorization-described-identity").await;
    let elsewhere_pool_id = emulator.pool("authorization-described-elsewhere").await;
    let pool_arn = identity_pool_arn(&pool_id);
    let action = action_of("DescribeIdentity");

    // The identity every row describes, minted through the `@optionalAuth` identity flow
    // with the account's own credential.
    let (status, got) = emulator
        .call(ACCOUNT_KEY, "GetId", &json!({ "IdentityPoolId": &pool_id }))
        .await;
    assert_eq!(
        status, 200,
        "the setup GetId must succeed, got {status} with {got}"
    );
    let identity_id = got["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("GetId answers an IdentityId: {got}"))
        .to_string();

    let bare = emulator.user("cognito-authz-describe-bare", None).await;
    let granted = emulator
        .user(
            "cognito-authz-describe-granted",
            Some(&allow(&action, &pool_arn)),
        )
        .await;
    let other_pool = emulator
        .user(
            "cognito-authz-describe-other-pool",
            Some(&allow(&action, &identity_pool_arn(&elsewhere_pool_id))),
        )
        .await;
    let other_action = emulator
        .user(
            "cognito-authz-describe-other-action",
            Some(&allow(&action_of("DescribeIdentityPool"), &pool_arn)),
        )
        .await;
    let account_wide = emulator
        .user(
            "cognito-authz-describe-account-wide",
            Some(&allow(&action, &any_pool_arn())),
        )
        .await;

    // One row: the case, the principal the call is signed as, the identity it names, and
    // the resource the refusal must be made about — `None` for a call that must be served.
    let rows: Vec<(&str, &Principal, &str, Option<String>)> = vec![
        (
            "a principal holding no policy at all",
            &bare,
            identity_id.as_str(),
            Some(pool_arn.clone()),
        ),
        (
            "a grant for this action on this identity's pool",
            &granted,
            identity_id.as_str(),
            None,
        ),
        (
            "a grant for this action on another pool",
            &other_pool,
            identity_id.as_str(),
            Some(pool_arn.clone()),
        ),
        (
            "a grant for another action on this identity's pool",
            &other_action,
            identity_id.as_str(),
            Some(pool_arn.clone()),
        ),
        (
            "a grant covering the account's identity pools",
            &account_wide,
            identity_id.as_str(),
            None,
        ),
        // The identity does not exist, and a caller no policy allows is told only that:
        // the decision is made against the account's pools, so the refusal reveals no pool
        // and no identity.
        (
            "a principal holding no policy at all, on an identity nothing minted",
            &bare,
            ABSENT_IDENTITY_ID,
            Some(any_pool_arn()),
        ),
    ];

    for (case, principal, named_identity, refused_about) in rows {
        let (status, body) = emulator
            .call(
                &principal.access_key_id,
                "DescribeIdentity",
                &json!({ "IdentityId": named_identity }),
            )
            .await;
        match refused_about {
            None => assert_eq!(
                (status, body["IdentityId"].as_str()),
                (200, Some(named_identity)),
                "case {case:?}: a principal whose documents allow the action must be served \
                 the identity it named, got {status} with {body}"
            ),
            Some(resource) => {
                assert_eq!(
                    (status, code_of(&body).as_str()),
                    ACCESS_DENIED,
                    "case {case:?}: a principal whose documents do not allow the action must \
                     be refused, got {status} with {body}"
                );
                assert_eq!(
                    message_of(&body),
                    no_policy_allows(&principal.arn, &action, &resource),
                    "case {case:?}: AWS's wording, naming the resource the decision was made \
                     about, got {body}"
                );
                assert!(
                    !message_of(&body).contains(named_identity)
                        || resource.contains(named_identity),
                    "case {case:?}: a refusal must not report the identity it was not \
                     authorized to read, got {body}"
                );
            }
        }
    }

    // The 404 the refusal above withheld: a caller the decision DOES allow reaches the read
    // itself, where an identity this service does not hold is the modelled not-found.
    let (status, body) = emulator
        .call(
            &account_wide.access_key_id,
            "DescribeIdentity",
            &json!({ "IdentityId": ABSENT_IDENTITY_ID }),
        )
        .await;
    assert_eq!(
        (status, code_of(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"an allowed principal, on an identity nothing minted\": expected the modelled \
         not-found, got {status} with {body}"
    );
}
