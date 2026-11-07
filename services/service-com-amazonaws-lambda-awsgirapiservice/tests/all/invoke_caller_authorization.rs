// SPDX-License-Identifier: CC0-1.0

//! Two further dimensions of the IAM authorization of an INVOCATION, over the restJson1 wire.
//!
//! The rule under test is the one `invoke_identity_authorization` states: an invocation is
//! served only to a caller the documents in force grant the invoking action on the function
//! the request addresses, and every other caller is refused `AccessDeniedException` (HTTP 403)
//! carrying the one sentence AWS words an authorization failure with. That suite walks the
//! kinds of principal, the qualifier addressed and the entry point called. This one walks the
//! two things it does not:
//!
//! * the PERMISSIONS BOUNDARY — "a permissions boundary sets the maximum permissions that an
//!   identity-based policy can grant to an entity" (IAM, permissions boundaries), so a grant
//!   the boundary does not also allow is not a permission the principal has, and the refusal
//!   says which of the two documents withheld it;
//! * the SPELLINGS a grant's `Action` element may take — IAM compares action names
//!   case-insensitively and admits `*` and prefix wildcards in them, so what decides an
//!   invocation is the action a statement NAMES and not how it is written.
//!
//! The decision is the real one: each row's role, its inline policy and the managed policy
//! that caps it are made with IAM's and STS's OWN operations over the same engine
//! (`CreateRole`, `PutRolePolicy`, `CreatePolicy`, `PutRolePermissionsBoundary`, `AssumeRole`),
//! so the documents the gate reads are the ones those calls wrote, and the caller signs with
//! the access key id `AssumeRole` answered. The function really runs a python deployment
//! package, so a served row is proved by the handler's own value rather than by a status alone.
//!
//! One emulator and one function per test, and one ROLE per row: a ceiling in force on an
//! entity cannot be taken back off, so the row's own role is what isolates it — which also
//! keeps the suite to a single provisioning wait.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    ACTIVATION_DELAY, INVOKE_ASYNC_ACTION, INVOKE_FUNCTION_ACTION,
};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region every request below signs for and addresses.
const REGION: &str = "us-east-1";

/// The access key id standing for the account's own published credential: neither STS nor IAM
/// holds a record for it, so it resolves to the account's own identity, which holds no IAM
/// policies — the position AWS account root credentials are in. The fixtures are built with
/// it, because this gate serves it.
const ACCOUNT_KEY_ID: &str = "AKIDTESTONLYACCOUNT1";

/// The name of the inline policy each row's role carries.
const POLICY_NAME: &str = "inline";

/// The session name each row assumes its role under.
const SESSION: &str = "probe";

/// A well-formed execution role ARN for the created function, satisfying the model's
/// `RoleArn` pattern. It is not a role any CALLER assumes: a function's execution role and an
/// invoker's identity are different principals.
const EXECUTION_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module the handler of the uploaded package lives in.
const HANDLER: &str = "lambda_function.lambda_handler";

/// A handler whose returned value proves the RUN happened rather than the request merely
/// having been accepted.
const SOURCE: &str = "def lambda_handler(event, context):\n    return {'ran': True}\n";

/// How long past a due instant this suite waits before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(100);

/// What the handler above answers with.
fn ran() -> Value {
    json!({ "ran": true })
}

/// The account id every ARN of this emulator carries.
fn account() -> &'static str {
    ::engine::emulator_account_id()
}

/// The unqualified ARN of `function`, which is the resource an unqualified invocation of it is
/// judged against.
fn function_arn(function: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{}:function:{function}", account())
}

/// The ARN the account holds the role `role` under.
fn role_arn(role: &str) -> String {
    format!("arn:aws:iam::{}:role/{role}", account())
}

/// The principal ARN a session assumed from `role` speaks for — what a refusal must name.
fn session_arn(role: &str) -> String {
    format!("arn:aws:sts::{}:assumed-role/{role}/{SESSION}", account())
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
    error_type: Option<String>,
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

    fn error_code(&self) -> String {
        self.error_type
            .clone()
            .or_else(|| {
                self.json()
                    .get("__type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| self.text())
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

/// Lambda beside the IAM and STS services over ONE shared engine, which is what `core/router`
/// assembles.
fn router_with_identity() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
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
    router
}

/// Percent-encode one component of an `application/x-www-form-urlencoded` body, which is how
/// an `awsQuery` parameter travels.
fn form_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The text of the first `<tag>` element of an XML body.
fn xml_element(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One Lambda request signed with `access_key_id`.
async fn lambda(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    uri: &str,
    body: &[u8],
) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/lambda/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        error_type: parts
            .headers
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        body,
    }
}

/// One `awsQuery` call to IAM or STS, signed with the account's own credential.
async fn query(
    router: &MultiServiceRouter,
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
        .uri(format!("https://{signing_name}.amazonaws.com/"))
        .header("host", format!("{signing_name}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY_ID}/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// A .zip deployment package holding [`SOURCE`] as `lambda_function.py`, written in memory the
/// way a caller uploads one.
fn package() -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(SOURCE.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// Create the function `function` with the account's own credential, and wait out the
/// provisioning window the create documents — once per test.
async fn given_function(router: &MultiServiceRouter, function: &str) {
    let created = lambda(
        router,
        ACCOUNT_KEY_ID,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": function,
            "Role": EXECUTION_ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 5,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package()),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {function:?}: CreateFunction must succeed, got {} {}",
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// Create the role `role` through `iam:CreateRole`, put `policy` on it as its inline policy
/// through `iam:PutRolePolicy`, assume it through `sts:AssumeRole`, and answer the access key
/// id of the session — the credential that row signs its invocation with.
async fn given_assumed_role(router: &MultiServiceRouter, case: &str, role: &str, policy: &Value) -> String {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": format!("arn:aws:iam::{}:root", account()) },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let (status, body) = query(
        router,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", role.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole must succeed, got {status} {body}"
    );
    given_role_policy(router, case, role, policy).await;
    let (status, body) = query(
        router,
        "sts",
        "2011-06-15",
        "AssumeRole",
        &[
            ("RoleArn", role_arn(role)),
            ("RoleSessionName", SESSION.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: AssumeRole must succeed, got {status} {body}"
    );
    xml_element(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("case {case:?}: AssumeRole must answer an AccessKeyId, got {body}"))
}

/// Put `document` on `role` as its inline policy through `iam:PutRolePolicy`, which REPLACES
/// the policy of that name.
async fn given_role_policy(
    router: &MultiServiceRouter,
    case: &str,
    role: &str,
    document: &Value,
) {
    let (status, body) = query(
        router,
        "iam",
        "2010-05-08",
        "PutRolePolicy",
        &[
            ("RoleName", role.to_string()),
            ("PolicyName", POLICY_NAME.to_string()),
            ("PolicyDocument", document.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRolePolicy must succeed, got {status} {body}"
    );
}

/// Create `document` as a customer-managed policy named `name` through `iam:CreatePolicy` and
/// attach it to `role` as its permissions boundary through `iam:PutRolePermissionsBoundary`,
/// which is how a ceiling is put in force on an entity.
async fn given_role_boundary(
    router: &MultiServiceRouter,
    case: &str,
    role: &str,
    name: &str,
    document: &Value,
) {
    let (status, body) = query(
        router,
        "iam",
        "2010-05-08",
        "CreatePolicy",
        &[
            ("PolicyName", name.to_string()),
            ("PolicyDocument", document.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreatePolicy must succeed, got {status} {body}"
    );
    let policy_arn = xml_element(&body, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: CreatePolicy must answer an Arn, got {body}"));
    let (status, body) = query(
        router,
        "iam",
        "2010-05-08",
        "PutRolePermissionsBoundary",
        &[
            ("RoleName", role.to_string()),
            ("PermissionsBoundary", policy_arn),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRolePermissionsBoundary must succeed, got {status} {body}"
    );
}

/// A policy with one statement of `effect` over `actions`, on every resource.
fn statement(effect: &str, actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": effect, "Action": actions, "Resource": "*" }],
    })
}

/// A policy allowing `actions` on every resource.
fn allow(actions: Value) -> Value {
    statement("Allow", actions)
}

/// `Invoke` the function `function`, as `access_key_id`.
async fn invoke_as(router: &MultiServiceRouter, access_key_id: &str, function: &str) -> Answer {
    lambda(
        router,
        access_key_id,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/invocations"),
        b"{}",
    )
    .await
}

/// Assert the invocation RAN: the handler's own value is the response payload.
fn assert_ran(case: &str, answered: &Answer) {
    assert_eq!(
        answered.status, 200,
        "case {case:?}: an allowed invocation must be served, got {} {}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.json(),
        ran(),
        "case {case:?}: expected the handler's own value, got {}",
        answered.text()
    );
}

/// Assert the invocation was REFUSED with the one sentence AWS words an authorization failure
/// with, ending in `reason`, and that it carries nothing of a run.
fn assert_refused(case: &str, answered: &Answer, role: &str, function: &str, reason: &str) {
    assert_eq!(
        answered.status, 403,
        "case {case:?}: expected HTTP 403, got {} {}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.error_code(),
        "AccessDeniedException",
        "case {case:?}: expected an AccessDeniedException, got {}",
        answered.text()
    );
    assert_eq!(
        answered.message(),
        format!(
            "User: {} is not authorized to perform: {INVOKE_FUNCTION_ACTION} on resource: {} \
             {reason}",
            session_arn(role),
            function_arn(function)
        ),
        "case {case:?}: expected IAM's wording, got {}",
        answered.text()
    );
    assert!(
        answered.json().get("ran").is_none(),
        "case {case:?}: a refused invocation must carry no run, got {}",
        answered.text()
    );
}

/// RULE: a permissions boundary CAPS what the identity-based policies grant and grants nothing
/// itself, so an invocation the role's own policy allows is served only while the boundary
/// allows it too — and the refusal names which of the two documents withheld the grant.
///
/// The rows differ along what the rule generalises over: whether a ceiling is in force at all,
/// how wide it is (exactly the action, every action of this service, every action), and whether
/// it withholds the action by stating nothing about it or by denying it outright.
#[tokio::test]
async fn a_permissions_boundary_caps_the_identity_based_grant() {
    // (case, the boundary document, the refusal it produces — `None` for a served invocation)
    let rows: &[(&str, Option<Value>, Option<String>)] = &[
        ("no boundary in force", None, None),
        (
            "a boundary allowing exactly the invoke action",
            Some(allow(json!(INVOKE_FUNCTION_ACTION))),
            None,
        ),
        (
            "a boundary allowing every action of this service",
            Some(allow(json!("lambda:*"))),
            None,
        ),
        (
            "a boundary allowing every action",
            Some(allow(json!("*"))),
            None,
        ),
        (
            "a boundary allowing another service only",
            Some(allow(json!("s3:*"))),
            Some(format!(
                "because no permissions boundary allows the {INVOKE_FUNCTION_ACTION} action"
            )),
        ),
        (
            "a boundary allowing another action of this service only",
            Some(allow(json!("lambda:GetFunction"))),
            Some(format!(
                "because no permissions boundary allows the {INVOKE_FUNCTION_ACTION} action"
            )),
        ),
        (
            "a boundary denying the invoke action outright",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": "*", "Resource": "*" },
                    { "Effect": "Deny", "Action": INVOKE_FUNCTION_ACTION, "Resource": "*" },
                ],
            })),
            Some("with an explicit deny in a permissions boundary".to_string()),
        ),
    ];

    let function = "boundary-probe";
    let router = router_with_identity();
    given_function(&router, function).await;

    for (index, (case, boundary, refused)) in rows.iter().enumerate() {
        // One ROLE per row: a ceiling put in force on an entity cannot be taken back off, so
        // the row's own role is what keeps it from deciding the next row. The role's OWN
        // policy allows the invocation in every row, so whatever happens is the ceiling's
        // doing.
        let role = format!("lambda-boundary-invoker-{index}");
        let session_key = given_assumed_role(
            &router,
            case,
            &role,
            &allow(json!(INVOKE_FUNCTION_ACTION)),
        )
        .await;
        if let Some(document) = boundary {
            given_role_boundary(&router, case, &role, &format!("boundary{index}"), document).await;
        }

        let answered = invoke_as(&router, &session_key, function).await;
        match refused {
            None => assert_ran(case, &answered),
            Some(reason) => assert_refused(case, &answered, &role, function, reason),
        }
    }
}

/// RULE: what decides an invocation is the action a statement NAMES, not how it is written —
/// IAM compares action names case-insensitively and admits `*` and prefix wildcards in the
/// `Action` element, while a statement naming a DIFFERENT action grants nothing however it is
/// spelled.
///
/// The rows differ along what the rule generalises over: the casing, whether the element is one
/// string or a list, which wildcard stands in the action's place, and which action is named —
/// including the deprecated asynchronous entry point's own action, which is not this one.
#[tokio::test]
async fn the_grant_is_read_for_the_action_it_names_whatever_spelling_it_uses() {
    // (case, the `Action` element of the role's one allow, whether the invocation is served)
    let rows: &[(&str, Value, bool)] = &[
        ("the action itself", json!(INVOKE_FUNCTION_ACTION), true),
        (
            "the action in lower case",
            json!(INVOKE_FUNCTION_ACTION.to_lowercase()),
            true,
        ),
        (
            "the action in upper case",
            json!(INVOKE_FUNCTION_ACTION.to_uppercase()),
            true,
        ),
        (
            "the action in a list beside another service's",
            json!(["s3:GetObject", INVOKE_FUNCTION_ACTION]),
            true,
        ),
        ("a prefix wildcard over it", json!("lambda:Invoke*"), true),
        ("every action of this service", json!("lambda:*"), true),
        ("every action", json!("*"), true),
        (
            // The IAM Service Authorization Reference publishes `lambda:InvokeAsync` as the
            // deprecated entry point's OWN action, so a grant of it is not a grant of this one.
            "the deprecated asynchronous action",
            json!(INVOKE_ASYNC_ACTION),
            false,
        ),
        (
            "another action of this service",
            json!("lambda:GetFunction"),
            false,
        ),
        (
            "a prefix wildcard over another action of this service",
            json!("lambda:Get*"),
            false,
        ),
    ];

    let function = "spelling-probe";
    let role = "lambda-spelling-invoker";
    let router = router_with_identity();
    given_function(&router, function).await;
    // One role and one session for every row: no ceiling is in force here, and
    // `PutRolePolicy` REPLACES the policy of that name, so each row is judged on its own
    // document alone.
    let session_key = given_assumed_role(&router, "the caller", role, &allow(json!("*"))).await;
    let withheld =
        format!("because no identity-based policy allows the {INVOKE_FUNCTION_ACTION} action");

    for (case, actions, served) in rows {
        given_role_policy(&router, case, role, &allow(actions.clone())).await;
        let answered = invoke_as(&router, &session_key, function).await;
        if *served {
            assert_ran(case, &answered);
        } else {
            assert_refused(case, &answered, role, function, &withheld);
        }
    }
}
