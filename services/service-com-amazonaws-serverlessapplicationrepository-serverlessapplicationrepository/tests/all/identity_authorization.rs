// SPDX-License-Identifier: CC0-1.0

//! IAM authorization of this service's operations, over the restJson1 wire.
//!
//! The rule under test: every operation of this service is a signed call (`sigv4`, with no
//! `@optionalAuth` operation in the model), and such a call is served only to a caller whose
//! identity-based IAM policies allow THAT operation's own action
//! (`serverlessrepo:<OperationName>`) on the resource the request addresses — the
//! `ApplicationId` ARN for a call that names an application, and `*` for one that addresses
//! no single application (`CreateApplication`, `ListApplications`, whose actions AWS defines
//! over no resource type). Every other caller is refused HTTP 403 `AccessDeniedException`
//! BEFORE the operation reads or writes anything, so a refused caller learns nothing about
//! what the catalogue holds, a refused mutation changes nothing here, and a refused deploy
//! cuts no change set and no stack in CloudFormation.
//!
//! The refusal's message is AWS's own sentence for the condition: "User: <caller ARN> is not
//! authorized to perform: <action> on resource: <resource> because no identity-based policy
//! allows the <action> action", whose only variable parts are the caller's ARN, the resource
//! the request addressed and the action name.
//!
//! The decision is the real one: the principals below are made with IAM's and STS's OWN
//! operations over the same engine (`CreateRole` + `PutRolePolicy` + `AssumeRole`,
//! `CreateUser` + `PutUserPolicy` + `CreateAccessKey`), so the documents this service reads
//! are the ones those services wrote, and the evaluation is
//! `service_common::iam_authorization`, shared with every other service that authorizes.
//!
//! It is asserted over TABLES that differ along what the rule generalises over: the
//! OPERATION asked for (a read, a listing, a publish, a patch, a delete, a policy call and
//! both deploy paths, each with its own action and its own resource), the kind of principal
//! (the account's own published credential, an IAM user, an assumed-role session), and what
//! its policies say (nothing, an `Allow` for these actions on ONE application's ARN, a
//! wildcard `Allow`, an explicit `Deny` beside a wildcard `Allow`).

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::EmulatedService as CloudFormation;
use service_com_amazonaws_serverlessapplicationrepository_serverlessapplicationrepository::EmulatedService as ServerlessRepo;
use smithy_server_runtime::MultiServiceRouter;

use crate::applications::{minimal, path_label};
use crate::version_templates::sam_template;

/// The region every call below is signed for, so the application ARNs the decisions are made
/// about are the ones this region's catalogue mints.
const REGION: &str = "us-west-2";

/// The access-key id standing for the account's own published credential: neither STS nor IAM
/// holds a record for it, so it resolves to the account's ambient identity, which holds no
/// IAM policies and is allowed — the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "AKIDEXAMPLE";

/// The wire error code of an authorization refusal, as the AWS "Common Errors" table names
/// it.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// The wire error code restJson1 carries as the body's `__type`.
    error_code: String,
    /// The error body's `message`, or the empty string for an answer that carries none.
    message: String,
    body: Value,
}

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal must
/// blame it by — what `sts:GetCallerIdentity` reports for that key.
#[derive(Debug, Clone)]
struct Principal {
    access_key_id: String,
    arn: String,
}

/// The emulator these tests drive: one shared engine carrying this service beside
/// CloudFormation (where a deploy's change set and stack live) and the IAM and STS services
/// the principals and their policies are made with.
///
/// Every service is constructed from that one engine, so the store the decisions are made
/// against is reached through the handlers alone: this suite writes none of it directly.
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
        router.register_service(ServerlessRepo::new(Arc::clone(&engine)));
        router.register_service(CloudFormation::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
            Arc::clone(&engine),
        ));
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
        Self { router }
    }

    /// One restJson1 call to this service, with `access_key_id` in the SigV4 credential scope.
    async fn repo(
        &self,
        access_key_id: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = match body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20240101/{REGION}/\
                     serverlessrepo/aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .header("content-type", "application/json")
            .body(payload)
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.body().to_vec();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code: body
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            message: body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            body,
        }
    }

    /// One awsQuery call to IAM, STS or CloudFormation, signed with the account's own
    /// credential. Answers the status and the raw XML envelope.
    async fn query(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> (u16, String) {
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
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20240101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        (status, text)
    }

    /// One awsQuery setup call that must succeed.
    async fn setup(
        &self,
        signing_name: &str,
        version: &str,
        action: &str,
        params: &[(&str, String)],
    ) -> String {
        let (status, text) = self.query(signing_name, version, action, params).await;
        assert_eq!(
            status, 200,
            "case {action:?}: the setup call must succeed, got {status} with {text}"
        );
        text
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
        self.setup(
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
            self.setup(
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
            .setup(
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

    /// Create an IAM user carrying `policy` as its inline policy (when one is given), mint a
    /// long-term access key for it, and answer that key id with the user's own ARN.
    async fn user(&self, user_name: &str, policy: Option<&str>) -> Principal {
        let created = self
            .setup(
                "iam",
                "2010-05-08",
                "CreateUser",
                &[("UserName", user_name.to_string())],
            )
            .await;
        let arn = xml_element(&created, "Arn")
            .unwrap_or_else(|| panic!("CreateUser answers the user's Arn, got {created}"));
        if let Some(policy) = policy {
            self.setup(
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
            .setup(
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

    /// Publish `name` with version 1.0.0 from a one-queue SAM template, as the account's own
    /// credential, and answer the minted `ApplicationId`.
    async fn published(&self, name: &str) -> String {
        let created = self
            .repo(ACCOUNT_KEY, "POST", "/applications", Some(minimal(name)))
            .await;
        assert_eq!(
            created.status, 201,
            "creating {name:?}: expected 201, got {} with {}",
            created.status, created.body
        );
        let application_id = created
            .body
            .get("applicationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let path = format!(
            "/applications/{}/versions/1.0.0",
            path_label(&application_id)
        );
        let published = self
            .repo(ACCOUNT_KEY, "PUT", &path, Some(sam_template("Queue")))
            .await;
        assert_eq!(
            published.status, 201,
            "publishing {name:?} 1.0.0: expected 201, got {} with {}",
            published.status, published.body
        );
        application_id
    }

    /// Prepare a CloudFormation template of version 1.0.0 as the account's own credential and
    /// answer its `TemplateId`. The create's own response reports it `ACTIVE`, so nothing has
    /// to be held for the template to be deployable.
    async fn prepared_template(&self, application_id: &str) -> String {
        let path = format!("/applications/{}/templates", path_label(application_id));
        let prepared = self
            .repo(
                ACCOUNT_KEY,
                "POST",
                &path,
                Some(json!({ "semanticVersion": "1.0.0" })),
            )
            .await;
        assert_eq!(
            prepared.status, 201,
            "preparing a template: expected 201, got {} with {}",
            prepared.status, prepared.body
        );
        prepared
            .body
            .get("templateId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// Whether CloudFormation holds a stack named `stack_name`: what a refused deploy must
    /// leave behind nothing of.
    async fn stack_exists(&self, stack_name: &str) -> bool {
        let (status, body) = self
            .query(
                "cloudformation",
                "2010-05-15",
                "DescribeStacks",
                &[("StackName", stack_name.to_string())],
            )
            .await;
        status == 200 && body.contains(stack_name)
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

/// The sentence AWS refuses an unauthorized call with.
fn not_authorized(principal: &str, operation: &str, resource: &str) -> String {
    let action = format!("serverlessrepo:{operation}");
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// An identity-based policy allowing `actions` on `resource` and nothing else.
fn policy_allowing(actions: &[&str], resource: &str) -> String {
    let actions: Vec<String> = actions
        .iter()
        .map(|action| format!("\"serverlessrepo:{action}\""))
        .collect();
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":[{}],"Resource":"{resource}"}}]}}"#,
        actions.join(",")
    )
}

/// One operation of this service, as a request a table row can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    CreateApplication,
    ListApplications,
    GetApplication,
    UpdateApplication,
    DeleteApplication,
    CreateApplicationVersion,
    ListApplicationVersions,
    GetApplicationPolicy,
    PutApplicationPolicy,
    CreateCloudFormationTemplate,
    GetCloudFormationTemplate,
    CreateCloudFormationChangeSet,
}

impl Op {
    /// The operation's own name, which is also the `serverlessrepo:` action it is performed
    /// under.
    fn name(self) -> &'static str {
        match self {
            Self::CreateApplication => "CreateApplication",
            Self::ListApplications => "ListApplications",
            Self::GetApplication => "GetApplication",
            Self::UpdateApplication => "UpdateApplication",
            Self::DeleteApplication => "DeleteApplication",
            Self::CreateApplicationVersion => "CreateApplicationVersion",
            Self::ListApplicationVersions => "ListApplicationVersions",
            Self::GetApplicationPolicy => "GetApplicationPolicy",
            Self::PutApplicationPolicy => "PutApplicationPolicy",
            Self::CreateCloudFormationTemplate => "CreateCloudFormationTemplate",
            Self::GetCloudFormationTemplate => "GetCloudFormationTemplate",
            Self::CreateCloudFormationChangeSet => "CreateCloudFormationChangeSet",
        }
    }

    /// The resource the decision is made about: the application ARN the request addresses, or
    /// `*` for a call that addresses no single application.
    fn resource(self, application_id: &str) -> String {
        match self {
            Self::CreateApplication | Self::ListApplications => "*".to_string(),
            _ => application_id.to_string(),
        }
    }

    /// The method, path and body one call of this operation is made with.
    fn request(self, application_id: &str, template_id: &str) -> (&'static str, String, Option<Value>) {
        let label = path_label(application_id);
        match self {
            Self::CreateApplication => (
                "POST",
                "/applications".to_string(),
                Some(minimal("authz-probe-created")),
            ),
            Self::ListApplications => ("GET", "/applications?maxItems=10".to_string(), None),
            Self::GetApplication => ("GET", format!("/applications/{label}"), None),
            Self::UpdateApplication => (
                "PATCH",
                format!("/applications/{label}"),
                Some(json!({ "description": "patched by the authorization suite" })),
            ),
            Self::DeleteApplication => ("DELETE", format!("/applications/{label}"), None),
            Self::CreateApplicationVersion => (
                "PUT",
                format!("/applications/{label}/versions/2.0.0"),
                Some(sam_template("Queue")),
            ),
            Self::ListApplicationVersions => (
                "GET",
                format!("/applications/{label}/versions?maxItems=10"),
                None,
            ),
            Self::GetApplicationPolicy => ("GET", format!("/applications/{label}/policy"), None),
            Self::PutApplicationPolicy => (
                "PUT",
                format!("/applications/{label}/policy"),
                Some(json!({
                    "statements": [{ "principals": ["111122223333"], "actions": ["Deploy"] }]
                })),
            ),
            Self::CreateCloudFormationTemplate => (
                "POST",
                format!("/applications/{label}/templates"),
                Some(json!({ "semanticVersion": "1.0.0" })),
            ),
            Self::GetCloudFormationTemplate => (
                "GET",
                format!("/applications/{label}/templates/{template_id}"),
                None,
            ),
            Self::CreateCloudFormationChangeSet => (
                "POST",
                format!("/applications/{label}/changesets"),
                Some(json!({ "stackName": "authz-probe-stack", "semanticVersion": "1.0.0" })),
            ),
        }
    }
}

/// Every operation this service implements, in the order the tables below drive them.
const EVERY_OP: &[Op] = &[
    Op::CreateApplication,
    Op::ListApplications,
    Op::GetApplication,
    Op::UpdateApplication,
    Op::CreateApplicationVersion,
    Op::ListApplicationVersions,
    Op::GetApplicationPolicy,
    Op::PutApplicationPolicy,
    Op::CreateCloudFormationTemplate,
    Op::GetCloudFormationTemplate,
    Op::CreateCloudFormationChangeSet,
    // Last: it removes the application the rows above address.
    Op::DeleteApplication,
];

/// Drive `op` against `application_id` as `principal`.
async fn call(
    emulator: &Emulator,
    principal: &Principal,
    op: Op,
    application_id: &str,
    template_id: &str,
) -> Reply {
    let (method, path, body) = op.request(application_id, template_id);
    emulator
        .repo(&principal.access_key_id, method, &path, body)
        .await
}

/// RULE, part 1: a caller holding NO identity-based grant is refused every operation of this
/// service with 403 `AccessDeniedException`, in AWS's own sentence, naming that operation's
/// own action and the resource the request addressed.
///
/// The rows span every implemented operation — a read, a listing, a publish, a patch, a
/// delete, both policy calls and both deploy paths — and both kinds of principal the IAM
/// store holds policies for (an assumed-role session and an IAM user), each carrying no
/// document at all.
#[tokio::test]
async fn a_caller_with_no_grant_is_refused_every_operation() {
    let emulator = Emulator::new();
    let application_id = emulator.published("authz-nogrant-app").await;
    let template_id = emulator.prepared_template(&application_id).await;

    let principals = vec![
        (
            "an assumed-role session with no policy",
            emulator.assumed_role("authz-nogrant-role", None).await,
        ),
        (
            "an IAM user with no policy",
            emulator.user("authz-nogrant-user", None).await,
        ),
    ];

    for (who, principal) in &principals {
        for op in EVERY_OP {
            let case = format!("{who}: {}", op.name());
            let answer = call(&emulator, principal, *op, &application_id, &template_id).await;
            assert_eq!(
                answer.status, 403,
                "case {case:?}: expected 403, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code, ACCESS_DENIED,
                "case {case:?}: expected the error code {ACCESS_DENIED}, got {}",
                answer.body
            );
            let expected =
                not_authorized(&principal.arn, op.name(), &op.resource(&application_id));
            assert_eq!(
                answer.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.message
            );
        }
    }

    // Nothing the refused calls asked for happened: the application is still there, still
    // carries only the version it was published with, and its policy is still empty.
    let read = emulator
        .repo(
            ACCOUNT_KEY,
            "GET",
            &format!("/applications/{}", path_label(&application_id)),
            None,
        )
        .await;
    assert_eq!(
        read.status, 200,
        "a refused delete removes nothing: expected 200, got {} with {}",
        read.status, read.body
    );
    assert_eq!(
        read.body.get("description").and_then(Value::as_str),
        Some("a demo app"),
        "a refused patch rewrites nothing, got {}",
        read.body
    );
    let versions = emulator
        .repo(
            ACCOUNT_KEY,
            "GET",
            &format!("/applications/{}/versions?maxItems=10", path_label(&application_id)),
            None,
        )
        .await;
    let listed = versions
        .body
        .get("versions")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        listed, 1,
        "a refused publish stores no version: expected 1, got {}",
        versions.body
    );
    let policy = emulator
        .repo(
            ACCOUNT_KEY,
            "GET",
            &format!("/applications/{}/policy", path_label(&application_id)),
            None,
        )
        .await;
    let statements = policy
        .body
        .get("statements")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        statements, 0,
        "a refused policy put stores no statement: expected 0, got {}",
        policy.body
    );
}

/// RULE, part 2: authorization is decided per APPLICATION ARN — the same identity, holding a
/// grant for two actions on ONE application's ARN, is served for that application and refused
/// for another in the same account, and the refusal names the application the request
/// addressed rather than the one the policy allows.
///
/// The rows span the two operations the grant names (a read and a deploy), the two
/// applications (the one the policy names and one it does not) and, for the application the
/// policy names, an operation it does NOT name — so a row differs from its neighbour in the
/// resource, in the action, or in both.
#[tokio::test]
async fn a_grant_scoped_to_one_application_does_not_reach_another() {
    let emulator = Emulator::new();
    let allowed = emulator.published("authz-scoped-allowed").await;
    let denied = emulator.published("authz-scoped-denied").await;

    let principal = emulator
        .assumed_role(
            "authz-scoped-role",
            Some(&policy_allowing(
                &["CreateCloudFormationChangeSet", "GetApplication"],
                &allowed,
            )),
        )
        .await;
    let template_id = emulator.prepared_template(&allowed).await;

    // (case, operation, the application the request addresses, whether it is served)
    let rows: Vec<(&str, Op, &String, bool)> = vec![
        (
            "the granted read on the granted application",
            Op::GetApplication,
            &allowed,
            true,
        ),
        (
            "the granted deploy on the granted application",
            Op::CreateCloudFormationChangeSet,
            &allowed,
            true,
        ),
        (
            "the granted read on another application",
            Op::GetApplication,
            &denied,
            false,
        ),
        (
            "the granted deploy on another application",
            Op::CreateCloudFormationChangeSet,
            &denied,
            false,
        ),
        (
            "an ungranted action on the granted application",
            Op::ListApplicationVersions,
            &allowed,
            false,
        ),
        (
            "an ungranted delete on the granted application",
            Op::DeleteApplication,
            &allowed,
            false,
        ),
        (
            "a listing the grant's resource cannot cover",
            Op::ListApplications,
            &allowed,
            false,
        ),
    ];

    for (case, op, application_id, served) in &rows {
        let answer = call(&emulator, &principal, *op, application_id, &template_id).await;
        if *served {
            assert_ne!(
                answer.status, 403,
                "case {case:?}: expected the call to be served, got 403 with {}",
                answer.body
            );
            assert!(
                answer.status < 300,
                "case {case:?}: expected a success, got {} with {}",
                answer.status,
                answer.body
            );
            continue;
        }
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected 403, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, ACCESS_DENIED,
            "case {case:?}: expected the error code {ACCESS_DENIED}, got {}",
            answer.body
        );
        let expected = not_authorized(&principal.arn, op.name(), &op.resource(application_id));
        assert_eq!(
            answer.message, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message
        );
    }
}

/// RULE, part 3: the refusal comes BEFORE the handler does any work, so a deploy the caller's
/// policies do not allow cuts no change set and leaves no stack in CloudFormation — while the
/// application the policy DOES name stays deployable with the same identity.
#[tokio::test]
async fn a_refused_deploy_leaves_no_change_set_and_no_stack() {
    let emulator = Emulator::new();
    let allowed = emulator.published("authz-deploy-allowed").await;
    let denied = emulator.published("authz-deploy-denied").await;
    let principal = emulator
        .assumed_role(
            "authz-deploy-role",
            Some(&policy_allowing(
                &["CreateCloudFormationChangeSet", "GetApplication"],
                &allowed,
            )),
        )
        .await;

    // (case, the application the deploy addresses, the stack name the request carries,
    //  whether the deploy is served)
    let rows: Vec<(&str, &String, &str, bool)> = vec![
        (
            "the application the policy names",
            &allowed,
            "authz-allowed-stack",
            true,
        ),
        (
            "an application the policy never names",
            &denied,
            "authz-denied-stack",
            false,
        ),
        (
            "that same application under another stack name",
            &denied,
            "authz-denied-stack-two",
            false,
        ),
    ];

    for (case, application_id, stack_name, served) in &rows {
        let path = format!("/applications/{}/changesets", path_label(application_id));
        let answer = emulator
            .repo(
                &principal.access_key_id,
                "POST",
                &path,
                Some(json!({ "stackName": stack_name, "semanticVersion": "1.0.0" })),
            )
            .await;
        // The stack a deployment of a published application goes into carries this service's
        // own prefix.
        let stack = format!("serverlessrepo-{stack_name}");
        let exists = emulator.stack_exists(&stack).await;
        if *served {
            assert_eq!(
                answer.status, 201,
                "case {case:?}: expected 201, got {} with {}",
                answer.status, answer.body
            );
            assert!(
                answer
                    .body
                    .get("changeSetId")
                    .and_then(Value::as_str)
                    .is_some(),
                "case {case:?}: a served deploy answers a ChangeSetId, got {}",
                answer.body
            );
            assert!(
                exists,
                "case {case:?}: a served deploy cuts the stack {stack}, which CloudFormation \
                 does not hold"
            );
            continue;
        }
        assert_eq!(
            answer.status, 403,
            "case {case:?}: expected 403, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body.get("changeSetId"),
            None,
            "case {case:?}: a refused deploy answers no ChangeSetId, got {}",
            answer.body
        );
        assert!(
            !exists,
            "case {case:?}: a refused deploy leaves no stack, yet CloudFormation holds {stack}"
        );
    }
}

/// RULE, part 4: a grant is read for the action and the resource it names, whatever shape it
/// takes — and an explicit `Deny` beside a wildcard `Allow` wins, as IAM's evaluation order
/// says.
///
/// The rows span what the policy SAYS (a wildcard allow over every action and resource, a
/// wildcard allow with one action explicitly denied, an allow for another service's action
/// entirely) against the same two operations, so each row differs from its neighbour in the
/// document rather than in the request.
#[tokio::test]
async fn what_the_policy_says_decides_the_call() {
    let emulator = Emulator::new();
    let application_id = emulator.published("authz-documents-app").await;
    let template_id = emulator.prepared_template(&application_id).await;

    let wildcard = r#"{"Version":"2012-10-17","Statement":[
        {"Effect":"Allow","Action":"serverlessrepo:*","Resource":"*"}]}"#;
    let wildcard_with_deny = r#"{"Version":"2012-10-17","Statement":[
        {"Effect":"Allow","Action":"serverlessrepo:*","Resource":"*"},
        {"Effect":"Deny","Action":"serverlessrepo:GetApplication","Resource":"*"}]}"#;
    let another_service = r#"{"Version":"2012-10-17","Statement":[
        {"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

    let wildcard_user = emulator
        .user("authz-wildcard-user", Some(wildcard))
        .await;
    let denied_user = emulator
        .user("authz-denied-user", Some(wildcard_with_deny))
        .await;
    let other_user = emulator
        .user("authz-other-user", Some(another_service))
        .await;

    // (case, the principal, operation, whether it is served)
    let rows: Vec<(&str, &Principal, Op, bool)> = vec![
        (
            "a wildcard allow serves the read",
            &wildcard_user,
            Op::GetApplication,
            true,
        ),
        (
            "a wildcard allow serves the template read",
            &wildcard_user,
            Op::GetCloudFormationTemplate,
            true,
        ),
        (
            "a wildcard allow serves the listing that names no application",
            &wildcard_user,
            Op::ListApplications,
            true,
        ),
        (
            "an explicit deny refuses the action it names",
            &denied_user,
            Op::GetApplication,
            false,
        ),
        (
            "the same document still serves an action the deny does not name",
            &denied_user,
            Op::GetCloudFormationTemplate,
            true,
        ),
        (
            "a grant for another service's action serves nothing here",
            &other_user,
            Op::GetApplication,
            false,
        ),
        (
            "nor the listing",
            &other_user,
            Op::ListApplications,
            false,
        ),
    ];

    for (case, principal, op, served) in &rows {
        let answer = call(&emulator, principal, *op, &application_id, &template_id).await;
        if *served {
            assert!(
                answer.status < 300,
                "case {case:?}: expected a success, got {} with {}",
                answer.status,
                answer.body
            );
        } else {
            assert_eq!(
                answer.status, 403,
                "case {case:?}: expected 403, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code, ACCESS_DENIED,
                "case {case:?}: expected the error code {ACCESS_DENIED}, got {}",
                answer.body
            );
            assert!(
                answer.message.starts_with(&format!("User: {} ", principal.arn)),
                "case {case:?}: the refusal blames the calling principal, got {:?}",
                answer.message
            );
        }
    }
}

/// RULE, part 5: the account's own published credential holds no IAM policies — the position
/// AWS account root credentials are in — so it is served every operation.
///
/// This is the control for the four tables above: every other suite in this target signs with
/// that credential, so a gate that refused it would refuse the whole service.
#[tokio::test]
async fn the_accounts_own_credential_is_served_every_operation() {
    let emulator = Emulator::new();
    let application_id = emulator.published("authz-account-app").await;
    let template_id = emulator.prepared_template(&application_id).await;
    let account = Principal {
        access_key_id: ACCOUNT_KEY.to_string(),
        arn: format!("arn:aws:iam::{}:root", ::engine::emulator_account_id()),
    };

    for op in EVERY_OP {
        let case = op.name();
        let answer = call(&emulator, &account, *op, &application_id, &template_id).await;
        assert_ne!(
            answer.status, 403,
            "case {case:?}: the account's own credential is served, got 403 with {}",
            answer.body
        );
        assert!(
            answer.status < 300,
            "case {case:?}: expected a success, got {} with {}",
            answer.status,
            answer.body
        );
    }
    assert!(
        !account.arn.is_empty(),
        "the account's own identity has an ARN"
    );
}
