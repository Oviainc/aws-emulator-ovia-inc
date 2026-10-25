// SPDX-License-Identifier: CC0-1.0

//! AgentCore Control resolves a caller-supplied execution role against IAM.
//!
//! RULE: an agent runtime RUNS AS the `roleArn` the caller names, so every operation that
//! stores one resolves that role against this account's IAM when the runtime is WRITTEN
//! and refuses the request with the operation's modelled `ValidationException` (HTTP 400)
//! — persisting nothing — unless IAM holds the role and its trust policy lets this
//! service's principal (`bedrock-agentcore.amazonaws.com`, the model's own
//! `awsService.cloudTrailEventSource`) assume it. A refusal has to be grounded in what IAM
//! ANSWERED, so a role ARN of another account — whose IAM this emulator is not the one to
//! ask — is written rather than refused on an assumption.
//!
//! The gate is the RUNTIME's: the recorded service answers 202 for a `CreateGateway`
//! naming a role that does not exist, so a gateway's role is stored as given. Both halves
//! are pinned below.
//!
//! The role is resolved through the in-process callback, so a single-service router has no
//! IAM to ask: that is why this suite lives here rather than in the service's own crate.
//! The rows span what the rule generalises over — the trust policy's shape (`Action` and
//! `Principal.Service` as one string or as a list, the wildcards IAM admits in the
//! action's place, an explicit `Deny` beside an `Allow`, a policy naming only another
//! service), where the role lives (the default path and a role under a path), which
//! account the ARN names, whether the role was ever created, and each of the operations
//! that store a runtime's role (`CreateAgentRuntime`, `UpdateAgentRuntime`).
//!
//! It also pins the projection that follows: `ListAgentRuntimes` reports exactly the
//! runtimes the creates WROTE, so a runtime whose create was refused appears nowhere.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder this suite needs is the one
// `all/cloudfront_key_value_store_handshake.rs` already owns: an
// `application/x-www-form-urlencoded` component is escaped exactly as a URI path segment
// is, so the codec is shared rather than written a third time.
use crate::cloudfront_key_value_store_handshake::segment as form_encode;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The service principal an AgentCore execution role's trust policy has to admit: the
/// model's `awsService.cloudTrailEventSource` (`smithy_query query=service_info`).
const SERVICE_PRINCIPAL: &str = "\"bedrock-agentcore.amazonaws.com\"";

/// AgentCore Control and IAM over ONE shared engine, as `router/src/lib.rs` mounts them,
/// so the control plane's in-process callback reaches this very IAM.
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
        service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// One awsQuery call to IAM: `POST /` with the parameters in the form body.
async fn iam(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One restJson1 call to AgentCore Control, answered with the status and the JSON body.
async fn control(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("bedrock-agentcore-control.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The restJson1 error code (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A trust policy with one statement, built from the pieces that vary between rows: the
/// effect, the action(s) and the service principal(s).
fn trust_policy(effect: &str, action: &str, service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":{action}}}]}}"
    )
}

/// A trust policy with TWO statements about the same principal and action: one `Allow` and
/// one explicit `Deny`. IAM's explicit deny wins, so the role cannot be assumed.
fn allow_then_deny_policy() -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[\
         {{\"Effect\":\"Allow\",\"Principal\":{{\"Service\":{SERVICE_PRINCIPAL}}},\
         \"Action\":\"sts:AssumeRole\"}},\
         {{\"Effect\":\"Deny\",\"Principal\":{{\"Service\":{SERVICE_PRINCIPAL}}},\
         \"Action\":\"sts:AssumeRole\"}}]}}"
    )
}

/// One IAM role the rows need, as `CreateRole` parameters.
struct RoleFixture {
    role_name: &'static str,
    path: Option<&'static str>,
    policy: String,
}

fn role_fixtures() -> Vec<RoleFixture> {
    vec![
        RoleFixture {
            role_name: "agentcore-role",
            path: None,
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", SERVICE_PRINCIPAL),
        },
        RoleFixture {
            role_name: "agentcore-role-lists",
            path: None,
            policy: trust_policy(
                "Allow",
                "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                "[\"lambda.amazonaws.com\",\"bedrock-agentcore.amazonaws.com\"]",
            ),
        },
        RoleFixture {
            role_name: "agentcore-role-wildcard",
            path: None,
            policy: trust_policy("Allow", "\"sts:*\"", SERVICE_PRINCIPAL),
        },
        RoleFixture {
            role_name: "path-scoped-role",
            path: Some("/service-role/"),
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", SERVICE_PRINCIPAL),
        },
        RoleFixture {
            role_name: "lambda-only-role",
            path: None,
            policy: trust_policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
        },
        RoleFixture {
            role_name: "other-action-role",
            path: None,
            policy: trust_policy("Allow", "\"s3:GetObject\"", SERVICE_PRINCIPAL),
        },
        RoleFixture {
            role_name: "denied-role",
            path: None,
            policy: trust_policy("Deny", "\"sts:AssumeRole\"", SERVICE_PRINCIPAL),
        },
        RoleFixture {
            role_name: "allow-then-deny-role",
            path: None,
            policy: allow_then_deny_policy(),
        },
    ]
}

/// One row: the execution role a create names, and whether it must be written.
struct RoleCase {
    case: &'static str,
    /// The `agentRuntimeName` / gateway name this row writes under, so each row owns its
    /// own resource. `AgentRuntimeName` is `^[a-zA-Z][a-zA-Z0-9_]{0,47}$` and
    /// `GatewayName` is `^([0-9a-zA-Z][-]?){1,48}$`, so the two are derived separately.
    stem: &'static str,
    role_arn: String,
    accepted: bool,
}

/// The ARN of a role of this account, under the default path.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

fn role_cases() -> Vec<RoleCase> {
    vec![
        RoleCase {
            case: "a role that exists and trusts this service",
            stem: "trusted",
            role_arn: role_arn("agentcore-role"),
            accepted: true,
        },
        RoleCase {
            case: "a trust policy whose Action and Service are lists",
            stem: "trusted_lists",
            role_arn: role_arn("agentcore-role-lists"),
            accepted: true,
        },
        RoleCase {
            case: "a trust policy allowing the whole sts namespace",
            stem: "trusted_wildcard",
            role_arn: role_arn("agentcore-role-wildcard"),
            accepted: true,
        },
        RoleCase {
            case: "a role under a path",
            stem: "path_scoped",
            role_arn: format!("arn:aws:iam::{ACCOUNT}:role/service-role/path-scoped-role"),
            accepted: true,
        },
        RoleCase {
            case: "a role that was never created",
            stem: "absent_role",
            role_arn: role_arn("never-created-role"),
            accepted: false,
        },
        RoleCase {
            case: "a role that trusts another service only",
            stem: "lambda_role",
            role_arn: role_arn("lambda-only-role"),
            accepted: false,
        },
        RoleCase {
            case: "a role whose statement names another action",
            stem: "other_action",
            role_arn: role_arn("other-action-role"),
            accepted: false,
        },
        RoleCase {
            case: "a role whose statement denies the assume",
            stem: "denied_role",
            role_arn: role_arn("denied-role"),
            accepted: false,
        },
        RoleCase {
            // IAM's explicit deny wins over an Allow for the same principal.
            case: "a role that both allows and explicitly denies the assume",
            stem: "allow_then_deny",
            role_arn: role_arn("allow-then-deny-role"),
            accepted: false,
        },
        RoleCase {
            // Another account's IAM is not this emulator's to ask, so nothing is known
            // about the role and the create is not refused on an assumption.
            case: "a role of another account",
            stem: "foreign_role",
            role_arn: "arn:aws:iam::123456789012:role/agentcore-role".to_string(),
            accepted: true,
        },
    ]
}

/// A `CreateAgentRuntime` body naming `role_arn`.
fn runtime_body(name: &str, role_arn: &str) -> Value {
    json!({
        "agentRuntimeName": name,
        "agentRuntimeArtifact": {"containerConfiguration": {
            "containerUri": "public.ecr.aws/agentcore/demo:v1"
        }},
        "networkConfiguration": {"networkMode": "PUBLIC"},
        "roleArn": role_arn,
    })
}

/// Create every role the rows name.
async fn create_roles(router: &MultiServiceRouter) {
    for fixture in role_fixtures() {
        let mut parameters = vec![
            ("RoleName", fixture.role_name),
            ("AssumeRolePolicyDocument", fixture.policy.as_str()),
        ];
        if let Some(path) = fixture.path {
            parameters.push(("Path", path));
        }
        let (status, body) = iam(router, "CreateRole", &parameters).await;
        assert_eq!(
            status, 200,
            "setup: CreateRole {} must succeed, got {status} {body}",
            fixture.role_name
        );
    }
}

#[tokio::test]
async fn an_agent_runtime_is_written_only_for_a_role_this_service_may_assume() {
    let router = emulator();
    create_roles(&router).await;

    let mut written: BTreeSet<String> = BTreeSet::new();
    for case in role_cases() {
        let (status, body) = control(
            &router,
            "PUT",
            "/runtimes/",
            Some(runtime_body(case.stem, &case.role_arn)),
        )
        .await;
        if case.accepted {
            assert_eq!(
                status, 202,
                "case {:?} ({}): CreateAgentRuntime must succeed, got {status} ({body})",
                case.case, case.role_arn
            );
            written.insert(case.stem.to_string());
            // The accepted runtime reads back carrying the role it was given.
            let runtime_id = text(&body, "agentRuntimeId");
            let (status, read) =
                control(&router, "GET", &format!("/runtimes/{runtime_id}/"), None).await;
            assert_eq!(
                (status, text(&read, "roleArn").as_str()),
                (200, case.role_arn.as_str()),
                "case {:?}: the stored runtime must carry the role it was given, got {status} {} \
                 ({read})",
                case.case,
                text(&read, "roleArn")
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {:?} ({}): a role this service cannot assume must be refused 400 \
                 ValidationException, got {status} {} ({body})",
                case.case,
                case.role_arn,
                error_code(&body)
            );
            assert!(
                text(&body, "message").contains(&case.role_arn),
                "case {:?}: the refusal names the role that failed, got {}",
                case.case,
                text(&body, "message")
            );
            assert!(
                body.get("agentRuntimeId").is_none(),
                "case {:?}: a refused create mints nothing, got {body}",
                case.case
            );
        }
    }

    // `ListAgentRuntimes` reports exactly what the creates WROTE: a runtime whose create
    // was refused is listed nowhere.
    let (status, listed) = control(&router, "POST", "/runtimes/", None).await;
    assert_eq!(
        status, 200,
        "ListAgentRuntimes must answer 200, got {status} ({listed})"
    );
    let names: BTreeSet<String> = listed["agentRuntimes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| text(entry, "agentRuntimeName"))
        .collect();
    assert_eq!(
        names, written,
        "ListAgentRuntimes must report exactly the runtimes that were written, got {names:?} for \
         {written:?} ({listed})"
    );
}

#[tokio::test]
async fn a_gateway_is_written_whatever_iam_says_about_its_role() {
    let router = emulator();
    create_roles(&router).await;

    // Every row is accepted, including the ones `CreateAgentRuntime` refuses: the
    // recorded service answers 202 for a `CreateGateway` naming a role that does not
    // exist, so the gateway's role is stored as given and resolved by nothing here.
    for case in role_cases() {
        // `GatewayName` admits no underscore, so the row's stem travels with dashes.
        let name = case.stem.replace('_', "-");
        let (status, body) = control(
            &router,
            "POST",
            "/gateways/",
            Some(json!({
                "name": name,
                "roleArn": case.role_arn,
                "authorizerType": "AWS_IAM",
            })),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {:?} ({}): CreateGateway must store the role as given, got {status} ({body})",
            case.case, case.role_arn
        );
        let gateway_id = text(&body, "gatewayId");
        let (status, read) =
            control(&router, "GET", &format!("/gateways/{gateway_id}/"), None).await;
        assert_eq!(
            (status, text(&read, "roleArn").as_str()),
            (200, case.role_arn.as_str()),
            "case {:?}: the stored gateway must carry the role it was given, got {status} {} \
             ({read})",
            case.case,
            text(&read, "roleArn")
        );
    }
}

/// `UpdateAgentRuntime` applies the same gate, and a refused update leaves the stored
/// runtime exactly as it was.
#[tokio::test]
async fn a_refused_update_publishes_no_version() {
    let router = emulator();
    create_roles(&router).await;

    let good = role_arn("agentcore-role");
    let (status, created) = control(
        &router,
        "PUT",
        "/runtimes/",
        Some(runtime_body("update_gate", &good)),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup: CreateAgentRuntime must succeed, got {status} ({created})"
    );
    let runtime_id = text(&created, "agentRuntimeId");

    for (case, role, expected_status) in [
        (
            "an update onto a role that was never created",
            role_arn("never-created-role"),
            400,
        ),
        (
            "an update onto a role that trusts another service only",
            role_arn("lambda-only-role"),
            400,
        ),
        (
            "an update onto a role this service may assume",
            role_arn("agentcore-role-lists"),
            202,
        ),
    ] {
        let (status, body) = control(
            &router,
            "PUT",
            &format!("/runtimes/{runtime_id}/"),
            Some(json!({
                "agentRuntimeArtifact": {"containerConfiguration": {
                    "containerUri": "public.ecr.aws/agentcore/demo:v2"
                }},
                "roleArn": role,
            })),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({role}): expected HTTP {expected_status}, got {status} ({body})"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {} ({body})",
                error_code(&body)
            );
        }
    }

    // Exactly one update went through, so the runtime is at version 2 and carries that
    // update's role: the two refused ones published nothing.
    let (status, read) = control(&router, "GET", &format!("/runtimes/{runtime_id}/"), None).await;
    assert_eq!(
        (
            status,
            text(&read, "agentRuntimeVersion").as_str(),
            text(&read, "roleArn").as_str()
        ),
        (200, "2", role_arn("agentcore-role-lists").as_str()),
        "a refused update must publish no version: expected version 2 carrying the accepted \
         role, got {status} ({read})"
    );
}
