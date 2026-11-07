// SPDX-License-Identifier: CC0-1.0

//! IAM authorization of an INVOCATION, over the restJson1 wire.
//!
//! The rule under test: an invocation is a signed call, and Lambda decides it at the IAM layer
//! BEFORE the function is reached — served only to a caller the documents in force grant the
//! invoking action on the function the request addresses, by its own identity-based policies
//! (capped by its permissions boundary) or by that function's resource-based policy. Every
//! other caller is refused with `AccessDeniedException` (HTTP 403) carrying the one sentence
//! AWS words an authorization failure with, and the refused invocation runs nothing, queues
//! nothing and changes nothing.
//!
//! The decision is the real one: the principals below are made with IAM's and STS's OWN
//! operations over the same engine (`CreateUser` + `PutUserPolicy` + `CreateAccessKey`,
//! `CreateRole` + `PutRolePolicy` + `AssumeRole`), and the function's own policy with Lambda's
//! `AddPermission`, so the documents the gate reads are the ones those calls wrote.
//!
//! The rows differ along what the rule generalises over: what the caller's policies say
//! (nothing, an `Allow` for this action on this function, a wildcard `Allow`, an `Allow` for
//! another action, an `Allow` on another function, an explicit `Deny` beside a wildcard
//! `Allow`), the kind of principal (the account's own published credential, an IAM user, an
//! assumed-role session), the QUALIFIER the invocation addresses (the unpublished version, a
//! published version, an alias) and which entry point is called (`Invoke`,
//! `InvokeWithResponseStream`, `InvokeAsync`), each decided on the action the IAM Service
//! Authorization Reference publishes for it.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for, so the ARNs the decisions are made about are the
/// ones this Region's functions carry.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The access-key id standing for the account's own published credential: neither STS nor IAM
/// holds a record for it, so it resolves to the account's own identity, which holds no IAM
/// policies — the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "EMULATORTESTKEY12345";

/// The module the handler of the package below lives in.
const HANDLER: &str = "lambda_function.lambda_handler";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// A handler whose returned value proves the RUN happened rather than the request merely
/// having been accepted.
const SOURCE: &str = "def lambda_handler(event, context):\n    return {'ran': True}\n";

/// What the handler above answers with.
fn ran() -> Value {
    json!({"ran": true})
}

/// How long past a due instant this suite waits before invoking. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: std::time::Duration = std::time::Duration::from_millis(100);

/// What a dispatched request answered.
struct Answer {
    status: u16,
    error_code: Option<String>,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn message(&self) -> String {
        let document = self.json();
        document
            .get("Message")
            .or_else(|| document.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// A principal a call is signed as: the access key id it presents, and the ARN a refusal must
/// blame it by — what `sts:GetCallerIdentity` reports for that key.
struct Principal {
    access_key_id: String,
    arn: String,
}

/// The emulator these tests drive: one shared engine carrying Lambda beside the IAM and STS
/// services the principals and their policies are made with.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
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
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// One call to Lambda, with `access_key_id` in the SigV4 credential scope.
    async fn call(
        &self,
        access_key_id: &str,
        method: &str,
        uri: &str,
        body: &[u8],
        headers: &[(&str, &str)],
    ) -> Answer {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://lambda.{REGION}.amazonaws.com{uri}"))
            .header("host", format!("lambda.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/lambda/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            );
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(body.to_vec()))
            .expect("the test request must build");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Answer {
            status: parts.status.as_u16(),
            error_code: parts
                .headers
                .get("x-amzn-errortype")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
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
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=0"
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

    /// Create an IAM role carrying `policy` as its inline policy, assume it through STS, and
    /// answer the session's access key id with the session ARN.
    async fn assumed_role(&self, role_name: &str, policy: Option<&str>) -> Principal {
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

    /// Create the function `name` running [`SOURCE`], publish version 1, point the alias
    /// `live` at it, and wait out the provisioning window the create documents.
    async fn function(&self, name: &str) {
        let package = crate::synchronous_invoke_run::package(SOURCE);
        let created = self
            .call(
                ACCOUNT_KEY,
                "POST",
                FUNCTIONS_URI,
                &serde_json::to_vec(&json!({
                    "FunctionName": name,
                    "Role": ROLE,
                    "Runtime": "python3.12",
                    "Handler": HANDLER,
                    "Publish": true,
                    "Timeout": 5,
                    "Code": {
                        "ZipFile": base64::engine::general_purpose::STANDARD.encode(package),
                    },
                }))
                .expect("the create body serializes"),
                &[],
            )
            .await;
        assert_eq!(
            created.status,
            201,
            "CreateFunction {name:?} must succeed, got {} {}",
            created.status,
            created.text()
        );
        let alias = self
            .call(
                ACCOUNT_KEY,
                "POST",
                &format!("{FUNCTIONS_URI}/{name}/aliases"),
                &serde_json::to_vec(&json!({ "Name": "live", "FunctionVersion": "1" }))
                    .expect("the alias body serializes"),
                &[],
            )
            .await;
        assert_eq!(
            alias.status,
            201,
            "CreateAlias on {name:?} must succeed, got {} {}",
            alias.status,
            alias.text()
        );
        tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
    }

    /// Grant `principal` the action `action` on the function `name` through Lambda's own
    /// `AddPermission`, which writes the function's resource-based policy.
    async fn add_permission(&self, name: &str, statement_id: &str, principal: &str, action: &str) {
        let added = self
            .call(
                ACCOUNT_KEY,
                "POST",
                &format!("{FUNCTIONS_URI}/{name}/policy"),
                &serde_json::to_vec(&json!({
                    "StatementId": statement_id,
                    "Action": action,
                    "Principal": principal,
                }))
                .expect("the permission body serializes"),
                &[],
            )
            .await;
        assert_eq!(
            added.status,
            201,
            "AddPermission on {name:?} must succeed, got {} {}",
            added.status,
            added.text()
        );
    }

    /// `Invoke` the function `reference` names, as `principal`.
    async fn invoke(&self, principal: &str, reference: &str, qualifier: Option<&str>) -> Answer {
        let query = qualifier.map_or_else(String::new, |value| format!("?Qualifier={value}"));
        self.call(
            principal,
            "POST",
            &format!("{FUNCTIONS_URI}/{reference}/invocations{query}"),
            b"{}",
            &[],
        )
        .await
    }

    /// `Invoke` with `InvocationType=Event`, which is the accepted-and-queued form.
    async fn invoke_event(&self, principal: &str, reference: &str) -> Answer {
        self.call(
            principal,
            "POST",
            &format!("{FUNCTIONS_URI}/{reference}/invocations"),
            b"{}",
            &[("x-amz-invocation-type", "Event")],
        )
        .await
    }

    /// `InvokeAsync`, the deprecated entry point with its own IAM action.
    async fn invoke_async(&self, principal: &str, reference: &str) -> Answer {
        self.call(
            principal,
            "POST",
            &format!("/2014-11-13/functions/{reference}/invoke-async"),
            b"{}",
            &[],
        )
        .await
    }

    /// `InvokeWithResponseStream`, which needs the same action `Invoke` does.
    async fn invoke_with_response_stream(&self, principal: &str, reference: &str) -> Answer {
        self.call(
            principal,
            "POST",
            &format!("/2021-11-15/functions/{reference}/response-streaming-invocations"),
            b"{}",
            &[],
        )
        .await
    }

    /// How many asynchronous invocations Lambda has QUEUED for `name` — what an accepted
    /// `Event` invocation persists, and what a refused one must leave untouched.
    fn queued(&self, name: &str) -> usize {
        let state = self.engine.state::<service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState>();
        let store = state.lock();
        store
            .functions
            .get(name)
            .map_or(0, |record| record.async_invocations.len())
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

/// A policy document allowing every Lambda action on everything, and explicitly denying
/// `denied`.
fn allow_all_but(denied: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Action\":\"lambda:*\",\"Resource\":\"*\"}},\
         {{\"Effect\":\"Deny\",\"Action\":\"{denied}\",\"Resource\":\"*\"}}]}}"
    )
}

/// The ARN of the function `name` in the Region every row is decided in.
fn function_arn(name: &str) -> String {
    format!(
        "arn:aws:lambda:{REGION}:{}:function:{name}",
        ::engine::emulator_account_id()
    )
}

/// The sentence AWS refuses an unauthorized request with.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// The clause a refusal carries when nothing the principal holds allows `action`.
fn no_allow(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// The action the two synchronous entry points are decided on.
const INVOKE_FUNCTION: &str = "lambda:InvokeFunction";

/// The action the deprecated `InvokeAsync` entry point is decided on.
const INVOKE_ASYNC: &str = "lambda:InvokeAsync";

/// Assert that `answer` is the modelled authorization refusal naming `principal`, `action`,
/// `resource` and `reason`.
fn assert_refused(case: &str, answer: &Answer, expected_message: &str) {
    assert_eq!(
        answer.status, 403,
        "case {case:?}: expected HTTP 403, got {} {}",
        answer.status,
        answer.text()
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("AccessDeniedException"),
        "case {case:?}: expected the AccessDeniedException code, got {:?}",
        answer.error_code
    );
    let message = answer.message();
    assert_eq!(
        message, expected_message,
        "case {case:?}: expected {expected_message:?}, got {message:?}"
    );
}

/// RULE: an invocation is served only to a principal whose identity-based policies allow the
/// invoking action on the function the request addresses; every other one is refused 403
/// `AccessDeniedException` before the function runs.
#[tokio::test]
async fn an_invocation_is_served_only_to_a_principal_iam_allows() {
    let emulator = Emulator::new();
    emulator.function("gated").await;
    let arn = function_arn("gated");

    // (case, user name, the user's inline policy, the refusal clause when it is refused)
    let rows: Vec<(&str, &str, Option<String>, Option<String>)> = vec![
        (
            "no policy at all",
            "nopolicy",
            None,
            Some(no_allow(INVOKE_FUNCTION)),
        ),
        (
            "an Allow of this action on this function",
            "scoped",
            Some(allow(INVOKE_FUNCTION, &arn)),
            None,
        ),
        (
            "a wildcard Allow of every Lambda action",
            "wildcard",
            Some(allow("lambda:*", "*")),
            None,
        ),
        (
            "an Allow of another action on this function",
            "reader",
            Some(allow("lambda:GetFunction", &arn)),
            Some(no_allow(INVOKE_FUNCTION)),
        ),
        (
            "an Allow of this action on another function",
            "elsewhere",
            Some(allow(INVOKE_FUNCTION, &function_arn("other"))),
            Some(no_allow(INVOKE_FUNCTION)),
        ),
        (
            "an explicit Deny beside a wildcard Allow",
            "denied",
            Some(allow_all_but(INVOKE_FUNCTION)),
            Some("with an explicit deny in an identity-based policy".to_string()),
        ),
    ];

    for (case, user_name, policy, refusal) in rows {
        let principal = emulator.user(user_name, policy.as_deref()).await;
        let answer = emulator
            .invoke(&principal.access_key_id, "gated", None)
            .await;
        match refusal {
            Some(reason) => assert_refused(
                case,
                &answer,
                &not_authorized(&principal.arn, INVOKE_FUNCTION, &arn, &reason),
            ),
            None => {
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?}: expected HTTP 200, got {} {}",
                    answer.status,
                    answer.text()
                );
                assert_eq!(
                    answer.json(),
                    ran(),
                    "case {case:?}: expected the handler's own value, got {}",
                    answer.text()
                );
            }
        }
    }

    // The account's own published credential holds no IAM policies — the position AWS account
    // root credentials are in — so it is not bounded by this gate.
    let account = emulator.invoke(ACCOUNT_KEY, "gated", None).await;
    assert_eq!(
        account.status,
        200,
        "case \"the account's own published credential\": expected HTTP 200, got {} {}",
        account.status,
        account.text()
    );
}

/// One row of the qualifier table below: which principal invokes which qualifier of the
/// function, the resource the decision is therefore made on, and whether it is allowed.
struct QualifierRow<'a> {
    case: &'a str,
    principal: &'a Principal,
    /// The `FunctionName` the invocation carries, in one of the documented forms.
    reference: &'a str,
    /// The `Qualifier` query parameter, when the row names one there instead.
    qualifier: Option<&'a str>,
    /// The ARN a refusal has to name, which is the ARN of the addressed qualifier.
    resource: String,
    allowed: bool,
}

/// RULE: the resource the decision is made on is the ARN of the QUALIFIER the invocation
/// addressed — the unqualified function ARN for the unpublished version, the
/// `:<qualifier>`-suffixed one for a published version or an alias — so a grant scoped to one
/// of them allows exactly the invocations that address it.
#[tokio::test]
async fn the_grant_is_read_for_the_qualifier_the_invocation_addressed() {
    let emulator = Emulator::new();
    emulator.function("qualified").await;
    let arn = function_arn("qualified");

    let unqualified = emulator
        .user("unqualified-only", Some(&allow(INVOKE_FUNCTION, &arn)))
        .await;
    let published = emulator
        .user(
            "qualifiers-only",
            Some(&allow(INVOKE_FUNCTION, &format!("{arn}:*"))),
        )
        .await;

    // (case, the principal, the FunctionName, the Qualifier, the resource decided on, allowed)
    let rows: Vec<QualifierRow<'_>> = vec![
        QualifierRow {
            case: "the unqualified grant on the unpublished version",
            principal: &unqualified,
            reference: "qualified",
            qualifier: None,
            resource: arn.clone(),
            allowed: true,
        },
        QualifierRow {
            case: "the unqualified grant on a published version",
            principal: &unqualified,
            reference: "qualified",
            qualifier: Some("1"),
            resource: format!("{arn}:1"),
            allowed: false,
        },
        QualifierRow {
            case: "the unqualified grant on an alias",
            principal: &unqualified,
            reference: "qualified:live",
            qualifier: None,
            resource: format!("{arn}:live"),
            allowed: false,
        },
        QualifierRow {
            case: "the qualifier-wildcard grant on the unpublished version",
            principal: &published,
            reference: "qualified",
            qualifier: None,
            resource: arn.clone(),
            allowed: false,
        },
        QualifierRow {
            case: "the qualifier-wildcard grant on a published version",
            principal: &published,
            reference: "qualified",
            qualifier: Some("1"),
            resource: format!("{arn}:1"),
            allowed: true,
        },
        QualifierRow {
            case: "the qualifier-wildcard grant on an alias",
            principal: &published,
            reference: "qualified:live",
            qualifier: None,
            resource: format!("{arn}:live"),
            allowed: true,
        },
    ];

    for row in rows {
        let QualifierRow {
            case,
            principal,
            reference,
            qualifier,
            resource,
            allowed,
        } = row;
        let answer = emulator
            .invoke(&principal.access_key_id, reference, qualifier)
            .await;
        if allowed {
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected HTTP 200, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.json(),
                ran(),
                "case {case:?}: expected the handler's own value, got {}",
                answer.text()
            );
        } else {
            assert_refused(
                case,
                &answer,
                &not_authorized(
                    &principal.arn,
                    INVOKE_FUNCTION,
                    &resource,
                    &no_allow(INVOKE_FUNCTION),
                ),
            );
        }
    }
}

/// RULE: each invoking entry point is decided on the action the IAM Service Authorization
/// Reference publishes for IT — `lambda:InvokeFunction` for the two synchronous ones and the
/// deprecated `lambda:InvokeAsync` for `InvokeAsync` — so a grant of one is not a grant of the
/// other.
#[tokio::test]
async fn each_invoking_entry_point_is_decided_on_its_own_published_action() {
    let emulator = Emulator::new();
    emulator.function("entrypoints").await;
    let arn = function_arn("entrypoints");

    let synchronous = emulator
        .user("sync-only", Some(&allow(INVOKE_FUNCTION, "*")))
        .await;
    let asynchronous = emulator
        .user("async-only", Some(&allow(INVOKE_ASYNC, "*")))
        .await;

    // (case, the principal, the action its grant does NOT cover, the answer)
    let refused: Vec<(&str, &Principal, &str, Answer)> = vec![
        (
            "InvokeAsync held only to the synchronous grant",
            &synchronous,
            INVOKE_ASYNC,
            emulator
                .invoke_async(&synchronous.access_key_id, "entrypoints")
                .await,
        ),
        (
            "Invoke held only to the asynchronous grant",
            &asynchronous,
            INVOKE_FUNCTION,
            emulator
                .invoke(&asynchronous.access_key_id, "entrypoints", None)
                .await,
        ),
        (
            "InvokeWithResponseStream held only to the asynchronous grant",
            &asynchronous,
            INVOKE_FUNCTION,
            emulator
                .invoke_with_response_stream(&asynchronous.access_key_id, "entrypoints")
                .await,
        ),
    ];
    for (case, principal, action, answer) in refused {
        assert_refused(
            case,
            &answer,
            &not_authorized(&principal.arn, action, &arn, &no_allow(action)),
        );
    }

    // (case, the answer the grant DOES cover, its success status)
    let served: Vec<(&str, Answer, u16)> = vec![
        (
            "Invoke under the synchronous grant",
            emulator
                .invoke(&synchronous.access_key_id, "entrypoints", None)
                .await,
            200,
        ),
        (
            "InvokeWithResponseStream under the synchronous grant",
            emulator
                .invoke_with_response_stream(&synchronous.access_key_id, "entrypoints")
                .await,
            200,
        ),
        (
            "InvokeAsync under the asynchronous grant",
            emulator
                .invoke_async(&asynchronous.access_key_id, "entrypoints")
                .await,
            202,
        ),
    ];
    for (case, answer, expected) in served {
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} {}",
            answer.status,
            answer.text()
        );
    }
}

/// RULE: a grant in the FUNCTION's own resource-based policy stands on its own, so a principal
/// holding no identity-based policy at all may invoke the qualifier that policy names — and
/// only that one.
#[tokio::test]
async fn a_function_policy_naming_the_principal_grants_the_invocation_on_its_own() {
    let emulator = Emulator::new();
    emulator.function("shared").await;
    let arn = function_arn("shared");
    let principal = emulator.user("policy-granted", None).await;

    let before = emulator
        .invoke(&principal.access_key_id, "shared", None)
        .await;
    assert_refused(
        "before the function's policy names it",
        &before,
        &not_authorized(
            &principal.arn,
            INVOKE_FUNCTION,
            &arn,
            &no_allow(INVOKE_FUNCTION),
        ),
    );

    emulator
        .add_permission("shared", "let-the-user-in", &principal.arn, INVOKE_FUNCTION)
        .await;

    let after = emulator
        .invoke(&principal.access_key_id, "shared", None)
        .await;
    assert_eq!(
        after.status,
        200,
        "case \"the function's policy names it\": expected HTTP 200, got {} {}",
        after.status,
        after.text()
    );
    assert_eq!(
        after.json(),
        ran(),
        "case \"the function's policy names it\": expected the handler's own value, got {}",
        after.text()
    );

    // The statement was granted on the UNQUALIFIED ARN, so it says nothing about a published
    // version: that invocation is still refused.
    let qualified = emulator
        .invoke(&principal.access_key_id, "shared", Some("1"))
        .await;
    assert_refused(
        "a published version the statement does not name",
        &qualified,
        &not_authorized(
            &principal.arn,
            INVOKE_FUNCTION,
            &format!("{arn}:1"),
            &no_allow(INVOKE_FUNCTION),
        ),
    );
}

/// RULE: an assumed-role session's permissions are the ROLE's, so the policies in force on the
/// role decide the invocation and the refusal blames the session ARN STS reports.
#[tokio::test]
async fn an_assumed_role_session_is_judged_on_the_roles_policies() {
    let emulator = Emulator::new();
    emulator.function("role-gated").await;
    let arn = function_arn("role-gated");

    let granted = emulator
        .assumed_role("invoker", Some(&allow(INVOKE_FUNCTION, &arn)))
        .await;
    let ungranted = emulator.assumed_role("bystander", None).await;

    let served = emulator
        .invoke(&granted.access_key_id, "role-gated", None)
        .await;
    assert_eq!(
        served.status,
        200,
        "case \"a session of a role granted the action\": expected HTTP 200, got {} {}",
        served.status,
        served.text()
    );

    let refused = emulator
        .invoke(&ungranted.access_key_id, "role-gated", None)
        .await;
    assert_refused(
        "a session of a role granted nothing",
        &refused,
        &not_authorized(
            &ungranted.arn,
            INVOKE_FUNCTION,
            &arn,
            &no_allow(INVOKE_FUNCTION),
        ),
    );
    assert!(
        ungranted.arn.contains(":assumed-role/bystander/"),
        "case \"a session of a role granted nothing\": expected the session ARN to be blamed, \
         got {:?}",
        ungranted.arn
    );
}

/// RULE: a refused invocation performs nothing — the decision is taken before the function is
/// resolved, so an `Event` invocation IAM withholds queues no event, while the same call under
/// a grant queues one.
#[tokio::test]
async fn a_refused_invocation_queues_nothing() {
    let emulator = Emulator::new();
    emulator.function("queue-gated").await;
    let arn = function_arn("queue-gated");
    let principal = emulator.user("queue-probe", None).await;

    assert_eq!(
        emulator.queued("queue-gated"),
        0,
        "case \"before any invocation\": expected no queued event, got {}",
        emulator.queued("queue-gated")
    );

    let refused = emulator
        .invoke_event(&principal.access_key_id, "queue-gated")
        .await;
    assert_refused(
        "an Event invocation IAM withholds",
        &refused,
        &not_authorized(
            &principal.arn,
            INVOKE_FUNCTION,
            &arn,
            &no_allow(INVOKE_FUNCTION),
        ),
    );
    let after_refusal = emulator.queued("queue-gated");
    assert_eq!(
        after_refusal, 0,
        "case \"after the refusal\": expected no queued event, got {after_refusal}"
    );

    let accepted = emulator.invoke_event(ACCOUNT_KEY, "queue-gated").await;
    assert_eq!(
        accepted.status,
        202,
        "case \"an Event invocation IAM allows\": expected HTTP 202, got {} {}",
        accepted.status,
        accepted.text()
    );
    let after_acceptance = emulator.queued("queue-gated");
    assert_eq!(
        after_acceptance, 1,
        "case \"after the acceptance\": expected one queued event, got {after_acceptance}"
    );
}
