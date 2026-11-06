// SPDX-License-Identifier: CC0-1.0

//! The IAM role an API Gateway ACCOUNT writes its logs under: what `UpdateAccount` accepts
//! for `/cloudwatchRoleArn`, across the two services the answer spans.
//!
//! RULE: `UpdateAccount` stores a `cloudwatchRoleArn` only where the role it names is one
//! API Gateway can ASSUME — a role of this account whose trust policy grants
//! `sts:AssumeRole` to `apigateway.amazonaws.com`. A value naming a role this account's IAM
//! holds nothing under, or one whose trust policy withholds that assume, is refused with
//! the modelled `BadRequestException` (HTTP 400) stating the one sentence for the condition,
//! and the account keeps the role it had; a value that IS assumable is stored and the
//! operation answers the whole `Account` representation, which `GetAccount` then reads back.
//!
//! Only a cross-service suite can pin this: the role, its trust policy and whether this
//! account holds it at all are IAM's, while the account settings are API Gateway's, so the
//! rule is observable only where both are mounted on one engine. Each crate's own tests
//! reach exactly one of them.
//!
//! The rows vary what the rule generalises over: WHAT the trust policy grants (this
//! service's principal, another service's, an account principal and no statement at all),
//! the FORM of the role ARN (a role at the account root and one under a path), and whether
//! IAM holds a role under the name at all.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The long-term key every emulator publishes, which is what these calls are signed with.
/// The signature itself is never checked: this router runs with request authentication off,
/// as every suite in this crate runs it.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The sentence API Gateway refuses a role it cannot assume with. Entirely fixed text: it
/// names what is missing and quotes back neither the ARN nor the role.
const ROLE_WITHOUT_PERMISSIONS: &str = "The role ARN does not have required permissions \
     configured. Please grant trust permission for API Gateway and add the required role \
     policy.";

/// A trust policy that lets API Gateway assume the role it is attached to. It carries none
/// of the characters that are significant in an `application/x-www-form-urlencoded` body.
const APIGATEWAY_TRUST: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"apigateway.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The same policy for ANOTHER service's principal: a role this service cannot assume,
/// which is the only thing that differs between it and the policy above.
const LAMBDA_TRUST: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// A trust policy granting the assume to an ACCOUNT principal rather than to a service: it
/// names no service at all, so it grants this service nothing.
const ACCOUNT_TRUST: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"sts:AssumeRole"}]}"#
);

/// A trust policy with no statement at all: it grants nothing to anyone.
const EMPTY_TRUST: &str = r#"{"Version":"2012-10-17","Statement":[]}"#;

/// What one call answered.
struct Answer {
    status: u16,
    body: Bytes,
}

impl Answer {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The two services this rule spans, over ONE engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={AMBIENT_KEY}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body,
    }
}

/// One API Gateway control-plane call.
async fn apigw(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::from(payload),
    )
    .await
}

/// `UpdateAccount` replacing `/cloudwatchRoleArn` with `value`.
async fn set_role(router: &MultiServiceRouter, value: &str) -> Answer {
    apigw(
        router,
        "PATCH",
        "/account",
        Some(json!({
            "patchOperations": [
                { "op": "replace", "path": "/cloudwatchRoleArn", "value": value }
            ]
        })),
    )
    .await
}

/// `GetAccount`, asserting the 200 it always answers.
async fn account(router: &MultiServiceRouter, case: &str) -> Value {
    let answer = apigw(router, "GET", "/account", None).await;
    assert_eq!(
        answer.status,
        200,
        "case {case}: expected 200 from GetAccount, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// One IAM awsQuery call that has to succeed, answering its body.
async fn iam_ok(router: &MultiServiceRouter, case: &str, body: String) -> String {
    let answer = dispatch(
        router,
        "POST",
        "iam.amazonaws.com",
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("iam")),
        ],
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "setup {case:?}: expected HTTP 200 from IAM, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.text()
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One IAM role under `path`, created with `trust` as its trust policy; answers its ARN.
async fn role(router: &MultiServiceRouter, name: &str, path: &str, trust: &str) -> String {
    let created = iam_ok(
        router,
        &format!("CreateRole({name})"),
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={name}&Path={path}\
             &AssumeRolePolicyDocument={trust}"
        ),
    )
    .await;
    xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("setup CreateRole({name}): no <Arn> in {created}"))
}

/// One row: what the account is asked to log under, and whether API Gateway takes it.
struct Row {
    case: &'static str,
    /// The role to create, as `(name, path, trust policy)`, or `None` for a row whose ARN
    /// names no role IAM holds.
    role: Option<(&'static str, &'static str, &'static str)>,
    /// Whether the patch is served.
    accepted: bool,
}

/// The table the rule is asserted over.
const ROWS: &[Row] = &[
    Row {
        case: "a role whose trust policy admits this service",
        role: Some(("account-logs-trusted", "/", APIGATEWAY_TRUST)),
        accepted: true,
    },
    Row {
        case: "a role under a path whose trust policy admits this service",
        role: Some((
            "account-logs-service-role",
            "/service-role/",
            APIGATEWAY_TRUST,
        )),
        accepted: true,
    },
    Row {
        case: "a role whose trust policy admits another service",
        role: Some(("account-logs-lambda", "/", LAMBDA_TRUST)),
        accepted: false,
    },
    Row {
        case: "a role whose trust policy admits an account principal only",
        role: Some(("account-logs-account", "/", ACCOUNT_TRUST)),
        accepted: false,
    },
    Row {
        case: "a role whose trust policy grants nothing",
        role: Some(("account-logs-nobody", "/", EMPTY_TRUST)),
        accepted: false,
    },
    Row {
        case: "an ARN naming a role this account does not hold",
        role: None,
        accepted: false,
    },
];

/// The ARN a row addresses: the role it created, or one of the same form naming no role.
async fn target(router: &MultiServiceRouter, row: &Row) -> String {
    match row.role {
        Some((name, path, trust)) => role(router, name, path, trust).await,
        // Built from an ARN IAM itself minted, so the partition and the account are this
        // emulator's own and only the role name names nothing.
        None => {
            let held = role(router, "account-logs-held", "/", APIGATEWAY_TRUST).await;
            held.rsplit_once('/')
                .map(|(prefix, _)| format!("{prefix}/account-logs-absent"))
                .unwrap_or_else(|| panic!("setup: no role segment in {held}"))
        }
    }
}

#[tokio::test]
async fn an_account_logs_only_under_a_role_this_service_can_assume() {
    for row in ROWS {
        let router = emulator();
        let role_arn = target(&router, row).await;

        let answer = set_role(&router, &role_arn).await;
        if row.accepted {
            assert_eq!(
                answer.status,
                200,
                "case {}: expected 200 from UpdateAccount, got {} with {:?}",
                row.case,
                answer.status,
                answer.text()
            );
            let reported = answer.json();
            assert_eq!(
                reported["cloudwatchRoleArn"].as_str(),
                Some(role_arn.as_str()),
                "case {}: expected the stored role {role_arn:?}, got {reported}",
                row.case
            );
            // The answer is the whole `Account` representation, not the one member the
            // patch named.
            assert!(
                reported["apiKeyVersion"].is_string(),
                "case {}: expected an apiKeyVersion, got {reported}",
                row.case
            );
            assert!(
                reported["features"].is_array(),
                "case {}: expected the features list, got {reported}",
                row.case
            );
            assert!(
                reported["throttleSettings"]["burstLimit"].is_number()
                    && reported["throttleSettings"]["rateLimit"].is_number(),
                "case {}: expected the account's throttle settings, got {reported}",
                row.case
            );
            // And what was stored is what a later read reports.
            assert_eq!(
                account(&router, row.case).await,
                reported,
                "case {}: expected GetAccount to read back what UpdateAccount reported",
                row.case
            );
        } else {
            assert_eq!(
                answer.status,
                400,
                "case {}: expected 400 from UpdateAccount, got {} with {:?}",
                row.case,
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.json()["message"].as_str(),
                Some(ROLE_WITHOUT_PERMISSIONS),
                "case {}: expected the role-permission sentence, got {:?}",
                row.case,
                answer.text()
            );
            // A refused patch stored nothing: the account still logs under no role.
            let read = account(&router, row.case).await;
            assert!(
                read["cloudwatchRoleArn"].is_null(),
                "case {}: expected no stored role after the refusal, got {read}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_refused_role_leaves_the_configured_one_standing() {
    let router = emulator();
    let configured = role(&router, "account-logs-configured", "/", APIGATEWAY_TRUST).await;
    let untrusted = role(&router, "account-logs-untrusted", "/", LAMBDA_TRUST).await;

    let accepted = set_role(&router, &configured).await;
    assert_eq!(
        accepted.status,
        200,
        "setup: expected 200 for the assumable role, got {} with {:?}",
        accepted.status,
        accepted.text()
    );

    // Each refusal leaves the configured role exactly as it was: the role is replaced only
    // by a patch that is served.
    for (case, value) in [
        ("a role this service cannot assume", untrusted.as_str()),
        ("a value that is no ARN at all", "not-an-arn"),
    ] {
        let refused = set_role(&router, value).await;
        assert_eq!(
            refused.status,
            400,
            "case {case}: expected 400 from UpdateAccount, got {} with {:?}",
            refused.status,
            refused.text()
        );
        let read = account(&router, case).await;
        assert_eq!(
            read["cloudwatchRoleArn"].as_str(),
            Some(configured.as_str()),
            "case {case}: expected the configured role {configured:?} to stand, got {read}"
        );
    }

    // An EMPTY value names no role, so it CLEARS the member rather than naming one to
    // resolve: the account then logs under none.
    let cleared = set_role(&router, "").await;
    assert_eq!(
        cleared.status,
        200,
        "case clearing the role: expected 200, got {} with {:?}",
        cleared.status,
        cleared.text()
    );
    let read = account(&router, "clearing the role").await;
    assert!(
        read["cloudwatchRoleArn"].is_null(),
        "case clearing the role: expected no stored role, got {read}"
    );
}
