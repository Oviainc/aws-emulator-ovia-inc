// SPDX-License-Identifier: CC0-1.0

//! IAM authorization of this service's operations, over the restJson1 wire.
//!
//! The rule under test: every operation of this service is a signed call, and such a call is
//! served only to a caller whose identity-based IAM policies allow THAT operation's own
//! action (`entityresolution:<OperationName>`) on the resource the request addresses — the
//! schema mapping's, matching workflow's, ID mapping workflow's or ID namespace's ARN, the
//! ARN a policy call names, or the account's resources of that type in the request's region
//! for a call that names no single resource. Every other caller is refused with the modelled
//! `AccessDeniedException` (HTTP 403) BEFORE the operation reads or writes anything, so a
//! refused caller learns nothing about what the account holds and a refused mutation changes
//! nothing.
//!
//! The decision is the real one: the principals below are made with IAM's and STS's OWN
//! operations over the same engine (`CreateUser` + `PutUserPolicy` / `AttachUserPolicy` +
//! `CreateAccessKey`, `CreateRole` + `PutRolePolicy` + `AssumeRole`), so the documents this
//! service reads are the ones those services wrote, and the evaluation is
//! `service_common::iam_authorization`, shared with every other service that authorizes.
//!
//! It is asserted over TABLES that differ along what the rule generalises over: the
//! OPERATION asked for (a read, a write, a delete, a listing and a policy call, each with
//! its own action and its own resource), the kind of principal (the account's own published
//! credential, an IAM user, an assumed-role session), what its policies say (nothing, an
//! `Allow` for this action on this resource, a wildcard `Allow`, an `Allow` for another
//! action, an `Allow` on another resource, an explicit `Deny` beside a wildcard `Allow`) and
//! where those policies LIVE (inline on the principal, or in an attached managed policy).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_entityresolution_awsveniceservice::EmulatedService;

/// The region every call below is signed for, so the ARNs the decisions are made about are
/// the ones this region's resources carry.
const REGION: &str = "us-east-1";

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record for it, so it resolves to the account's ambient identity, which holds
/// no IAM policies and is allowed — the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "EMULATORTESTKEY12345";

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal
/// must blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// The emulator these tests drive: one shared engine carrying this service beside the IAM
/// and STS services the principals and their policies are made with.
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
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
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
    async fn call(
        &self,
        access_key_id: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://entityresolution.{REGION}.amazonaws.com{path}"
            ))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     entityresolution/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
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
            access_key_id: xml_element(&minted, "AccessKeyId")
                .unwrap_or_else(|| panic!("CreateAccessKey answers an AccessKeyId, got {minted}")),
            arn,
        }
    }

    /// Create an IAM role carrying `policy` as its inline policy (when one is given), assume
    /// it through STS, and answer the session's access key id with the session ARN.
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

    /// Attach a customer-managed policy carrying `document` to the role `role_name`.
    async fn attach_role_policy(&self, role_name: &str, policy_name: &str, document: &str) {
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
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreatePolicy answers the policy's Arn, got {created}"));
        self.query(
            "iam",
            "2010-05-08",
            "AttachRolePolicy",
            &[
                ("RoleName", role_name.to_string()),
                ("PolicyArn", arn),
            ],
        )
        .await;
    }

    /// Create an IAM group carrying `document` as its inline policy and put `user_name` in
    /// it: the group's policies are in force on its members.
    async fn group_with_policy(&self, group_name: &str, user_name: &str, document: &str) {
        self.query(
            "iam",
            "2010-05-08",
            "CreateGroup",
            &[("GroupName", group_name.to_string())],
        )
        .await;
        self.query(
            "iam",
            "2010-05-08",
            "PutGroupPolicy",
            &[
                ("GroupName", group_name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", document.to_string()),
            ],
        )
        .await;
        self.query(
            "iam",
            "2010-05-08",
            "AddUserToGroup",
            &[
                ("GroupName", group_name.to_string()),
                ("UserName", user_name.to_string()),
            ],
        )
        .await;
    }

    /// Set the permissions boundary of `user_name` to a managed policy carrying `document`.
    async fn user_boundary(&self, user_name: &str, policy_name: &str, document: &str) {
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
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreatePolicy answers the policy's Arn, got {created}"));
        self.query(
            "iam",
            "2010-05-08",
            "PutUserPermissionsBoundary",
            &[
                ("UserName", user_name.to_string()),
                ("PermissionsBoundary", arn),
            ],
        )
        .await;
    }
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
         {{\"Effect\":\"Allow\",\"Action\":\"entityresolution:*\",\"Resource\":\"*\"}},\
         {{\"Effect\":\"Deny\",\"Action\":\"{denied}\",\"Resource\":\"*\"}}]}}"
    )
}

/// The IAM action an operation of this service is performed under.
fn action_of(operation: &str) -> String {
    format!("entityresolution:{operation}")
}

/// The ARN of one resource of this service, as its create mints it.
fn resource_arn(resource_type: &str, name: &str) -> String {
    format!(
        "arn:aws:entityresolution:{REGION}:{}:{resource_type}/{name}",
        ::engine::emulator_account_id()
    )
}

/// The refusal AWS words an identity-based decision with when no policy allows the action.
fn no_policy_allows(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// The refusal AWS words an identity-based decision with when a policy explicitly denies it.
fn explicit_deny(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         with an explicit deny in an identity-based policy"
    )
}

/// The schema mapping every row below reads, and its name.
const SCHEMA: &str = "authz-probe";

/// A `CreateSchemaMapping` body for `name`.
fn schema_request(name: &str) -> Value {
    json!({
        "schemaName": name,
        "mappedInputFields": [
            { "fieldName": "id", "type": "UNIQUE_ID" },
            { "fieldName": "email", "type": "EMAIL_ADDRESS" },
        ],
    })
}

/// One operation of this service, as the request that drives it and the resource the
/// decision about it is made on.
struct OperationCase {
    case: &'static str,
    operation: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    resource: String,
    /// The status the SAME call answers once the caller is allowed — never a 403, and for a
    /// row addressing a resource that does not exist, the modelled miss.
    allowed_status: u16,
}

/// The table: a read, a listing, a write, a delete and a policy read, each with its own
/// action and its own resource.
fn operation_cases() -> Vec<OperationCase> {
    vec![
        OperationCase {
            case: "a read of one schema mapping",
            operation: "GetSchemaMapping",
            method: "GET",
            path: format!("/schemas/{SCHEMA}"),
            body: None,
            resource: resource_arn("schemamapping", SCHEMA),
            allowed_status: 200,
        },
        OperationCase {
            case: "a listing, which names no single schema mapping",
            operation: "ListSchemaMappings",
            method: "GET",
            path: "/schemas".to_string(),
            body: None,
            resource: resource_arn("schemamapping", "*"),
            allowed_status: 200,
        },
        OperationCase {
            case: "a create, decided on the resource it would create",
            operation: "CreateSchemaMapping",
            method: "POST",
            path: "/schemas".to_string(),
            body: Some(schema_request("authz-created")),
            resource: resource_arn("schemamapping", "authz-created"),
            allowed_status: 200,
        },
        OperationCase {
            case: "a delete of a schema mapping that does not exist",
            operation: "DeleteSchemaMapping",
            method: "DELETE",
            path: "/schemas/authz-absent".to_string(),
            body: None,
            resource: resource_arn("schemamapping", "authz-absent"),
            // Allowed, and then the modelled resource miss: the authorization decision is
            // taken before the resource is resolved.
            allowed_status: 404,
        },
        OperationCase {
            case: "a read of a matching workflow that does not exist",
            operation: "GetMatchingWorkflow",
            method: "GET",
            path: "/matchingworkflows/authz-absent".to_string(),
            body: None,
            resource: resource_arn("matchingworkflow", "authz-absent"),
            allowed_status: 404,
        },
        OperationCase {
            case: "a read of an ID namespace that does not exist",
            operation: "GetIdNamespace",
            method: "GET",
            path: "/idnamespaces/authz-absent".to_string(),
            body: None,
            resource: resource_arn("idnamespace", "authz-absent"),
            allowed_status: 404,
        },
        OperationCase {
            case: "a policy read, decided on the resource the policy is attached to",
            operation: "GetPolicy",
            method: "GET",
            // `/policies/{arn}` binds the ARN to a non-greedy `@httpLabel`, so its slashes
            // travel percent-encoded — the same spelling the sibling `resource_policy`
            // suite sends.
            path: format!(
                "/policies/{}",
                resource_arn("schemamapping", SCHEMA).replace('/', "%2F")
            ),
            body: None,
            resource: resource_arn("schemamapping", SCHEMA),
            allowed_status: 404,
        },
    ]
}

/// Assert `reply` is the modelled authorization refusal carrying `message`.
fn assert_denied(case: &str, reply: &Reply, message: &str) {
    assert_eq!(
        reply.status, 403,
        "case {case:?}: expected HTTP 403, got {reply:?}"
    );
    assert_eq!(
        reply.error_code.as_deref(),
        Some("AccessDeniedException"),
        "case {case:?}: expected AccessDeniedException, got {:?}",
        reply.error_code
    );
    assert_eq!(
        reply.body.get("message").and_then(Value::as_str),
        Some(message),
        "case {case:?}: expected the message {message:?}, got {:?}",
        reply.body
    );
}

/// RULE: a principal whose identity-based policies say nothing about this service is refused
/// every operation with `AccessDeniedException` (HTTP 403) naming it, the action and the
/// resource — while the account's own published credential, which holds no IAM policies, is
/// served. A refused call changes nothing: the resource a refused create would have made is
/// still absent afterwards.
#[tokio::test]
async fn a_principal_with_no_entity_resolution_grant_is_refused_every_operation() {
    let emulator = Emulator::new();
    let seeded = emulator
        .call(ACCOUNT_KEY, "POST", "/schemas", Some(&schema_request(SCHEMA)))
        .await;
    assert_eq!(
        seeded.status, 200,
        "the account's own credential must be served: {seeded:?}"
    );

    // (case, the principal, how it presents itself)
    let bare_user = emulator.user("authz-noperm-user", None).await;
    let bare_session = emulator.assumed_role("authz-noperm-role", None).await;
    let principals = [
        ("an IAM user with no policy at all", &bare_user),
        ("an assumed-role session with no policy at all", &bare_session),
    ];

    for (principal_case, principal) in principals {
        for row in operation_cases() {
            let case = format!("{principal_case}, {}", row.case);
            let reply = emulator
                .call(
                    &principal.access_key_id,
                    row.method,
                    &row.path,
                    row.body.as_ref(),
                )
                .await;
            assert_denied(
                &case,
                &reply,
                &no_policy_allows(
                    &principal.arn,
                    &action_of(row.operation),
                    &row.resource,
                ),
            );
        }
    }

    // Nothing the refused calls asked for happened: the create's resource is still absent,
    // and the schema mapping the reads named is still there.
    let created = emulator
        .call(ACCOUNT_KEY, "GET", "/schemas/authz-created", None)
        .await;
    assert_eq!(
        created.status, 404,
        "a refused create must store nothing, got {created:?}"
    );
    let present = emulator
        .call(ACCOUNT_KEY, "GET", &format!("/schemas/{SCHEMA}"), None)
        .await;
    assert_eq!(
        present.status, 200,
        "a refused delete must remove nothing, got {present:?}"
    );
}

/// RULE: the decision is per ACTION and per RESOURCE. A grant of exactly the action on
/// exactly the resource serves the call; a grant of another action, or of the same action on
/// another resource, does not; a wildcard grant serves it; and an explicit `Deny` beside a
/// wildcard `Allow` refuses it with the deny's own wording. The grant is read whether it sits
/// inline on the principal or in a managed policy attached to it.
#[tokio::test]
async fn a_grant_serves_exactly_the_action_and_resource_it_names() {
    let emulator = Emulator::new();
    let seeded = emulator
        .call(ACCOUNT_KEY, "POST", "/schemas", Some(&schema_request(SCHEMA)))
        .await;
    assert_eq!(
        seeded.status, 200,
        "the account's own credential must be served: {seeded:?}"
    );

    let read = action_of("GetSchemaMapping");
    let resource = resource_arn("schemamapping", SCHEMA);
    let other_resource = resource_arn("schemamapping", "authz-other");

    // (case, the document the principal carries, whether the read is served, the message a
    // refusal must carry when it is not)
    enum Expected {
        Served,
        NoAllow,
        ExplicitDeny,
    }
    let rows: Vec<(&str, String, Expected)> = vec![
        (
            "an Allow for this action on this resource",
            allow(&read, &resource),
            Expected::Served,
        ),
        (
            "an Allow for every action of this service on everything",
            allow("entityresolution:*", "*"),
            Expected::Served,
        ),
        (
            "an Allow for this action on every resource of this account",
            allow(&read, &resource_arn("schemamapping", "*")),
            Expected::Served,
        ),
        (
            "an Allow for ANOTHER action on this resource",
            allow(&action_of("DeleteSchemaMapping"), &resource),
            Expected::NoAllow,
        ),
        (
            "an Allow for this action on ANOTHER resource",
            allow(&read, &other_resource),
            Expected::NoAllow,
        ),
        (
            "an Allow for another service's action",
            allow("iam:GetUser", "*"),
            Expected::NoAllow,
        ),
        (
            "a wildcard Allow with an explicit Deny of this action",
            allow_all_but(&read),
            Expected::ExplicitDeny,
        ),
    ];

    for (index, (case, document, expected)) in rows.iter().enumerate() {
        // The same document is asserted twice: inline on a user, and in a managed policy
        // attached to a role the caller assumed — the two places a grant can live.
        let user = emulator
            .user(&format!("authz-user-{index}"), Some(document))
            .await;
        let role_name = format!("authz-role-{index}");
        let session = emulator.assumed_role(&role_name, None).await;
        emulator
            .attach_role_policy(&role_name, &format!("authz-managed-{index}"), document)
            .await;

        for (where_case, principal) in [
            ("inline on an IAM user", &user),
            ("in a managed policy attached to the assumed role", &session),
        ] {
            let case = format!("{case}, {where_case}");
            let reply = emulator
                .call(
                    &principal.access_key_id,
                    "GET",
                    &format!("/schemas/{SCHEMA}"),
                    None,
                )
                .await;
            match expected {
                Expected::Served => {
                    assert_eq!(
                        reply.status, 200,
                        "case {case:?}: expected the read to be served, got {reply:?}"
                    );
                    assert_eq!(
                        reply.body.get("schemaName").and_then(Value::as_str),
                        Some(SCHEMA),
                        "case {case:?}: expected the stored schema mapping, got {:?}",
                        reply.body
                    );
                }
                Expected::NoAllow => assert_denied(
                    &case,
                    &reply,
                    &no_policy_allows(&principal.arn, &read, &resource),
                ),
                Expected::ExplicitDeny => assert_denied(
                    &case,
                    &reply,
                    &explicit_deny(&principal.arn, &read, &resource),
                ),
            }
        }
    }
}

/// RULE: granting the ONE missing action makes the identical call succeed, and that grant
/// reaches only that action — which is what shows the gate reads the policy rather than the
/// credential's shape. Every row drives the same assumed-role session before and after the
/// grant.
#[tokio::test]
async fn granting_the_missing_action_serves_the_identical_call() {
    let emulator = Emulator::new();
    let seeded = emulator
        .call(ACCOUNT_KEY, "POST", "/schemas", Some(&schema_request(SCHEMA)))
        .await;
    assert_eq!(
        seeded.status, 200,
        "the account's own credential must be served: {seeded:?}"
    );

    for (index, row) in operation_cases().into_iter().enumerate() {
        let case = row.case;
        let role_name = format!("authz-grant-role-{index}");
        let session = emulator.assumed_role(&role_name, None).await;

        let refused = emulator
            .call(
                &session.access_key_id,
                row.method,
                &row.path,
                row.body.as_ref(),
            )
            .await;
        assert_denied(
            case,
            &refused,
            &no_policy_allows(
                &session.arn,
                &action_of(row.operation),
                &row.resource,
            ),
        );

        // The single missing action, granted on the resource the request addresses.
        emulator
            .attach_role_policy(
                &role_name,
                &format!("authz-grant-{index}"),
                &allow(&action_of(row.operation), &row.resource),
            )
            .await;
        let served = emulator
            .call(
                &session.access_key_id,
                row.method,
                &row.path,
                row.body.as_ref(),
            )
            .await;
        assert_eq!(
            served.status, row.allowed_status,
            "case {case:?}: with {} granted the identical call must answer {}, got {served:?}",
            action_of(row.operation),
            row.allowed_status
        );

        // And the grant reaches that action ALONE: another operation on the same resource is
        // still refused.
        let neighbour = emulator
            .call(
                &session.access_key_id,
                "GET",
                &format!("/schemas/{SCHEMA}"),
                None,
            )
            .await;
        if row.operation != "GetSchemaMapping" {
            assert_denied(
                case,
                &neighbour,
                &no_policy_allows(
                    &session.arn,
                    &action_of("GetSchemaMapping"),
                    &resource_arn("schemamapping", SCHEMA),
                ),
            );
        }
    }
}

/// RULE: "the policies in force on the principal" is every document IAM holds in force on
/// it, which for a user includes the policies of the GROUPS it belongs to; and a grant is
/// capped by the principal's permissions boundary, since "a permissions boundary sets the
/// maximum permissions that an identity-based policy can grant". The rows differ along where
/// the grant lives (a group's inline policy, the user's own) and what the boundary says
/// (none, an Allow covering the action, nothing about it, an explicit Deny).
#[tokio::test]
async fn the_documents_in_force_include_a_users_groups_and_are_capped_by_its_boundary() {
    let emulator = Emulator::new();
    let seeded = emulator
        .call(ACCOUNT_KEY, "POST", "/schemas", Some(&schema_request(SCHEMA)))
        .await;
    assert_eq!(
        seeded.status, 200,
        "the account's own credential must be served: {seeded:?}"
    );

    let read = action_of("GetSchemaMapping");
    let resource = resource_arn("schemamapping", SCHEMA);

    /// What the row expects of the read, and which clause a refusal must carry.
    enum Expected {
        Served,
        BoundaryStatesNothing,
        BoundaryDenies,
    }

    // (case, the grant's own policy — `None` puts it on the group only, the boundary)
    let rows: Vec<(&str, bool, Option<String>, Expected)> = vec![
        (
            "the only grant is a group's inline policy, with no boundary",
            true,
            None,
            Expected::Served,
        ),
        (
            "a group grant with a boundary that allows the action",
            true,
            Some(allow(&read, &resource)),
            Expected::Served,
        ),
        (
            "a group grant with a boundary that says nothing about this service",
            true,
            Some(allow("iam:GetUser", "*")),
            Expected::BoundaryStatesNothing,
        ),
        (
            "a group grant with a boundary that denies the action",
            true,
            Some(allow_all_but(&read)),
            Expected::BoundaryDenies,
        ),
        (
            "the user's own grant with a boundary that says nothing about this service",
            false,
            Some(allow("iam:GetUser", "*")),
            Expected::BoundaryStatesNothing,
        ),
    ];

    for (index, (case, grant_on_group, boundary, expected)) in rows.iter().enumerate() {
        let user_name = format!("authz-group-user-{index}");
        let grant = allow(&read, &resource);
        let user = if *grant_on_group {
            // The user itself carries NO policy: every grant it has comes from the group.
            let created = emulator.user(&user_name, None).await;
            emulator
                .group_with_policy(&format!("authz-group-{index}"), &user_name, &grant)
                .await;
            created
        } else {
            emulator.user(&user_name, Some(&grant)).await
        };
        if let Some(document) = boundary {
            emulator
                .user_boundary(&user_name, &format!("authz-boundary-{index}"), document)
                .await;
        }

        let reply = emulator
            .call(
                &user.access_key_id,
                "GET",
                &format!("/schemas/{SCHEMA}"),
                None,
            )
            .await;
        match expected {
            Expected::Served => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: expected the read to be served, got {reply:?}"
                );
                assert_eq!(
                    reply.body.get("schemaName").and_then(Value::as_str),
                    Some(SCHEMA),
                    "case {case:?}: expected the stored schema mapping, got {:?}",
                    reply.body
                );
            }
            Expected::BoundaryStatesNothing => assert_denied(
                case,
                &reply,
                &format!(
                    "User: {} is not authorized to perform: {read} on resource: {resource} \
                     because no permissions boundary allows the {read} action",
                    user.arn
                ),
            ),
            Expected::BoundaryDenies => assert_denied(
                case,
                &reply,
                &format!(
                    "User: {} is not authorized to perform: {read} on resource: {resource} \
                     with an explicit deny in a permissions boundary",
                    user.arn
                ),
            ),
        }
    }
}
