// SPDX-License-Identifier: CC0-1.0

//! Authorization on the Bedrock Agent Runtime session plane: three services, one
//! decision.
//!
//! RULE: a session-scoped call is answered only when the caller's own identity-based
//! permissions cover `bedrock:<Operation>` on the session's ARN. The caller is the
//! identity the access key it signed with names — the STS session registry, the same
//! source `sts:GetCallerIdentity` answers from — and its permissions are the policies the
//! account holds for that identity: for an assumed-role session, the IAM role's inline and
//! attached policies. IAM's default is deny, so a role the account grants nothing
//! authorizes nothing and the call is refused with the modelled `AccessDeniedException`
//! (HTTP 403, `smithy_query(query="operation",operation="GetSession")`), whose message is
//! built from the request: the principal, the action and the resource. The emulator's own
//! ambient credential — the long-term key it publishes — is the account's administrative
//! identity and keeps reading the same session throughout.
//!
//! Three services have to agree for that to hold (IAM holds the role and its policies,
//! STS mints the session credential and names its principal, Bedrock Agent Runtime makes
//! the decision), so this is the suite that can pin it; a crate-local test can only ever
//! reach one of the three.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The region every request here signs for.
const REGION: &str = "us-east-1";

/// A trust policy granting the account root permission to assume a role. It carries none
/// of the characters that are significant in an `application/x-www-form-urlencoded` body
/// (`&`, `=`, `+`, `%`, space), so it needs no encoding.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#
);

/// The three services this rule spans, over ONE engine, as `core/router/src/lib.rs`
/// mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
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
    router.register_service(
        service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and whose
/// credential is `access_key_id` — the key the service reads the caller's identity from.
/// The signature is never checked: the router here runs with request authentication off.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// The long-term key every emulator publishes, which is the ambient identity's.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// Post one awsQuery form body to a service and return `(status, body)`.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    body: String,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(AMBIENT_KEY, signing_name))
        .body(Bytes::from(body))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Send one restJson1 request to the session plane as `access_key_id` and return
/// `(status, body)`.
async fn session_call(
    router: &MultiServiceRouter,
    access_key_id: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let payload = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(serde_json::to_vec(&body).expect("the test body serializes"))
    };
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header("authorization", authorization(access_key_id, "bedrock"))
        .body(payload)
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// Create an IAM role and return its ARN.
async fn create_role(router: &MultiServiceRouter, role_name: &str) -> String {
    let (status, body) = query(
        router,
        "iam",
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={role_name}\
             &AssumeRolePolicyDocument={TRUST_POLICY}"
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: CreateRole expected 200, got {status} {body}"
    );
    xml_element(&body, "Arn").unwrap_or_else(|| panic!("case {role_name:?}: no <Arn> in {body}"))
}

/// Embed an inline policy in an IAM role.
async fn put_role_policy(router: &MultiServiceRouter, role_name: &str, document: &str) {
    let (status, body) = query(
        router,
        "iam",
        format!(
            "Action=PutRolePolicy&Version=2010-05-08&RoleName={role_name}\
             &PolicyName=probe-policy&PolicyDocument={document}"
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: PutRolePolicy expected 200, got {status} {body}"
    );
}

/// Assume a role and return the access key id of the session STS minted.
async fn assume_role(router: &MultiServiceRouter, role_arn: &str) -> String {
    let (status, body) = query(
        router,
        "sts",
        format!(
            "Action=AssumeRole&Version=2011-06-15&RoleArn={role_arn}&RoleSessionName=probe"
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssumeRole({role_arn}) expected 200, got {status} {body}"
    );
    xml_element(&body, "AccessKeyId")
        .unwrap_or_else(|| panic!("AssumeRole({role_arn}): no <AccessKeyId> in {body}"))
}

/// The wire error code restJson1 puts in `__type`, or `""` on a success body.
fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// RULE over a TABLE of identity-based policies: the assumed-role caller's own policy
/// decides, and the owning (ambient) caller keeps reading the session throughout.
///
/// The rows differ along what the role's policy says: nothing at all, another service's
/// action, another action of this service, the action on another resource, the action on
/// this resource, a wildcard that covers it, an explicit deny over an allow, and an
/// `Allow` carrying a `Condition` the emulator cannot evaluate.
#[tokio::test]
async fn a_session_is_read_only_by_a_principal_whose_policy_allows_it() {
    let router = build_router();

    // The session is created by the ambient identity, which owns it.
    let (status, created) = session_call(&router, AMBIENT_KEY, "PUT", "/sessions/", json!({})).await;
    assert_eq!(status, 201, "CreateSession expected 201, got {status} {created:?}");
    let session_id = created["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("no sessionId in {created:?}"))
        .to_string();
    let session_arn = created["sessionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no sessionArn in {created:?}"))
        .to_string();
    assert_eq!(
        session_arn,
        format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:session/{session_id}"),
        "the minted ARN must name the minted session, got {session_arn}"
    );

    /// One statement, as a policy document carrying nothing a form body treats specially.
    fn policy(effect: &str, action: &str, resource: &str) -> String {
        format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"}}]}}"#
        )
    }

    // (case, role name, the role's inline policy, expected status)
    let rows: Vec<(&str, &str, Option<String>, u16)> = vec![
        ("a role the account grants nothing", "no-policy-role", None, 403),
        (
            "another service's action",
            "s3-only-role",
            Some(policy("Allow", "s3:GetObject", "*")),
            403,
        ),
        (
            "another action of this service",
            "delete-only-role",
            Some(policy("Allow", "bedrock:DeleteSession", "*")),
            403,
        ),
        (
            "the action on another session",
            "other-session-role",
            Some(policy(
                "Allow",
                "bedrock:GetSession",
                "arn:aws:bedrock:us-east-1:000000000000:session/\
                 11111111-2222-3333-4444-555555555555",
            )),
            403,
        ),
        (
            "an allow carrying a Condition this emulator cannot evaluate",
            "conditioned-role",
            Some(
                r#"{"Statement":[{"Effect":"Allow","Action":"bedrock:*","Resource":"*","Condition":{"StringEquals":{"aws:username":"someone"}}}]}"#
                    .to_string(),
            ),
            403,
        ),
        (
            "the action on this session",
            "exact-role",
            Some(policy("Allow", "bedrock:GetSession", &session_arn)),
            200,
        ),
        (
            "a wildcard covering the action and the resource",
            "wildcard-role",
            Some(policy("Allow", "bedrock:*", "*")),
            200,
        ),
        (
            "every action on every resource",
            "admin-role",
            Some(policy("Allow", "*", "*")),
            200,
        ),
    ];

    for (case, role_name, document, expected) in rows {
        let role_arn = create_role(&router, role_name).await;
        if let Some(document) = &document {
            put_role_policy(&router, role_name, document).await;
        }
        let access_key_id = assume_role(&router, &role_arn).await;

        let (status, answer) = session_call(
            &router,
            &access_key_id,
            "GET",
            &format!("/sessions/{session_id}/"),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} for the assumed-role caller, got \
             {status} {answer:?}"
        );
        if expected == 403 {
            assert_eq!(
                error_code(&answer),
                "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {answer:?}"
            );
            let message = answer["message"].as_str().unwrap_or_default();
            let principal = format!("arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/probe");
            assert!(
                message.contains(&principal)
                    && message.contains("bedrock:GetSession")
                    && message.contains(&session_arn),
                "case {case:?}: the refusal must be built from the request (principal \
                 {principal}, action bedrock:GetSession, resource {session_arn}), got \
                 {message:?}"
            );
        } else {
            assert_eq!(
                answer["sessionId"].as_str(),
                Some(session_id.as_str()),
                "case {case:?}: the authorized caller must read the session, got {answer:?}"
            );
        }

        // Whatever the row, the owning (ambient) caller keeps reading the session.
        let (status, owner_read) = session_call(
            &router,
            AMBIENT_KEY,
            "GET",
            &format!("/sessions/{session_id}/"),
            Value::Null,
        )
        .await;
        assert_eq!(
            (status, owner_read["sessionId"].as_str()),
            (200, Some(session_id.as_str())),
            "case {case:?}: the owning principal must keep reading its session, got \
             {status} {owner_read:?}"
        );
    }
}

/// Attach a managed policy to an IAM role.
async fn attach_role_policy(router: &MultiServiceRouter, role_name: &str, policy_arn: &str) {
    let (status, body) = query(
        router,
        "iam",
        format!(
            "Action=AttachRolePolicy&Version=2010-05-08&RoleName={role_name}\
             &PolicyArn={policy_arn}"
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: AttachRolePolicy expected 200, got {status} {body}"
    );
}

/// RULE at its edge: a grant this emulator CANNOT read is not a refusal. An AWS-MANAGED
/// policy is owned by AWS, so the IAM store holds no document for it — which is why
/// `AttachRolePolicy` accepts one the account does not hold — and a principal carrying such
/// an attachment is served rather than refused on evidence the emulator does not have.
/// (The same reading as `services/service-com-amazonaws-rum-rum/src/state/authorization.rs`,
/// the in-tree precedent for this decision.)
#[tokio::test]
async fn an_attachment_this_emulator_cannot_read_is_not_a_refusal() {
    let router = build_router();
    let (status, created) = session_call(&router, AMBIENT_KEY, "PUT", "/sessions/", json!({})).await;
    assert_eq!(status, 201, "CreateSession expected 201, got {status} {created:?}");
    let session_id = created["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("no sessionId in {created:?}"))
        .to_string();

    // (case, role name, the policy ARN attached, expected status)
    let rows: [(&str, &str, &str, u16); 1] = [(
        "an AWS-managed policy, whose document the account does not hold",
        "aws-managed-role",
        "arn:aws:iam::aws:policy/AmazonBedrockFullAccess",
        200,
    )];
    // A customer-managed ARN the account does not hold is not a row here: IAM refuses to
    // attach one at all ("does not exist or is not attachable"), so no principal can carry
    // such an attachment. The account IS the authority on its own policies.

    for (case, role_name, policy_arn, expected) in rows {
        let role_arn = create_role(&router, role_name).await;
        attach_role_policy(&router, role_name, policy_arn).await;
        let access_key_id = assume_role(&router, &role_arn).await;
        let (status, answer) = session_call(
            &router,
            &access_key_id,
            "GET",
            &format!("/sessions/{session_id}/"),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected} for a role carrying {policy_arn}, got \
             {status} {answer:?}"
        );
    }
}

/// RULE applied to the WRITES as well as the read: an assumed-role caller the account
/// grants nothing is refused on every session-scoped operation, and the refusal names
/// that operation's own action.
#[tokio::test]
async fn every_session_scoped_operation_authorizes_the_caller() {
    let router = build_router();
    let (status, created) = session_call(&router, AMBIENT_KEY, "PUT", "/sessions/", json!({})).await;
    assert_eq!(status, 201, "CreateSession expected 201, got {status} {created:?}");
    let session_id = created["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("no sessionId in {created:?}"))
        .to_string();
    let session_arn = created["sessionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no sessionArn in {created:?}"))
        .to_string();

    let role_arn = create_role(&router, "stranger-role").await;
    let stranger = assume_role(&router, &role_arn).await;
    let invocation = "0123abcd-4567-89ef-0123-456789abcdef";
    let tag_label = session_arn.replace(':', "%3A").replace('/', "%2F");

    // (action, method, path, body)
    let rows: Vec<(&str, &str, String, Value)> = vec![
        (
            "bedrock:GetSession",
            "GET",
            format!("/sessions/{session_id}/"),
            Value::Null,
        ),
        (
            "bedrock:UpdateSession",
            "PUT",
            format!("/sessions/{session_id}/"),
            json!({ "sessionMetadata": { "k": "v" } }),
        ),
        (
            "bedrock:EndSession",
            "PATCH",
            format!("/sessions/{session_id}"),
            Value::Null,
        ),
        (
            "bedrock:DeleteSession",
            "DELETE",
            format!("/sessions/{session_id}"),
            Value::Null,
        ),
        (
            "bedrock:CreateInvocation",
            "PUT",
            format!("/sessions/{session_id}/invocations/"),
            json!({ "invocationId": invocation }),
        ),
        (
            "bedrock:PutInvocationStep",
            "PUT",
            format!("/sessions/{session_id}/invocationSteps/"),
            json!({
                "invocationIdentifier": invocation,
                "invocationStepTime": "2026-10-23T00:33:20Z",
                "payload": { "contentBlocks": [{ "text": "x" }] },
            }),
        ),
        (
            "bedrock:ListInvocationSteps",
            "POST",
            format!("/sessions/{session_id}/invocationSteps/"),
            json!({ "invocationIdentifier": invocation }),
        ),
        (
            "bedrock:GetInvocationStep",
            "POST",
            format!("/sessions/{session_id}/invocationSteps/{invocation}"),
            json!({ "invocationIdentifier": invocation }),
        ),
        (
            "bedrock:TagResource",
            "POST",
            format!("/tags/{tag_label}"),
            json!({ "tags": { "k": "v" } }),
        ),
        (
            "bedrock:UntagResource",
            "DELETE",
            format!("/tags/{tag_label}?tagKeys=k"),
            Value::Null,
        ),
        (
            "bedrock:ListTagsForResource",
            "GET",
            format!("/tags/{tag_label}"),
            Value::Null,
        ),
    ];

    for (action, method, path, body) in rows {
        let (status, refused) = session_call(&router, &stranger, method, &path, body).await;
        assert_eq!(
            (status, error_code(&refused)),
            (403, "AccessDeniedException"),
            "case {action:?}: expected 403 AccessDeniedException, got {status} {refused:?}"
        );
        let message = refused["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(action),
            "case {action:?}: the refusal must name the action it denied, got {message:?}"
        );
    }

    // None of those refusals changed the session: the owner still reads it as ACTIVE.
    let (status, owner_read) = session_call(
        &router,
        AMBIENT_KEY,
        "GET",
        &format!("/sessions/{session_id}/"),
        Value::Null,
    )
    .await;
    assert_eq!(
        (status, owner_read["sessionStatus"].as_str()),
        (200, Some("ACTIVE")),
        "a refused write must change nothing, got {status} {owner_read:?}"
    );
}
