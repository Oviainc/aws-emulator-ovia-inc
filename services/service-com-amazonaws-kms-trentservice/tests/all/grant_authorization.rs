// SPDX-License-Identifier: CC0-1.0

//! This service's AUTHORIZATION, over the awsJson1_1 wire.
//!
//! The rule under test: a KMS request whose permission must come from somewhere is
//! authorized against the three authorities in force at the instant of the call — the
//! key's own key policy, the calling principal's identity-based policies, and the grants
//! on that key the caller holds — and a request none of them allows is refused
//! `AccessDeniedException` (HTTP 400) with the IAM denial sentence built from the caller,
//! the action and the key ARN, having read or written nothing.
//!
//! The decision is the real one: the principals below are made with IAM's and STS's OWN
//! operations over the same engine — `CreateRole` + `PutRolePolicy` + `AssumeRole`,
//! `CreateUser` + `PutUserPolicy` + `CreateAccessKey` — so the documents this service
//! reads are the ones those services wrote, and the evaluation is
//! `service_common::iam_authorization`, shared with every other service that authorizes.
//!
//! The tables differ along what the rule generalises over:
//!
//! * which authority is asked — a key policy naming the principal, a key policy naming
//!   the account (which delegates to IAM), an identity policy, a grant, and an explicit
//!   deny beside a grant;
//! * the kind of caller — the account's own published credential, an assumed-role
//!   session, an IAM user signing with a long-term key;
//! * the operation — `CreateGrant`, `Encrypt`, `Decrypt`, `GenerateDataKey`, each named in
//!   the refusal as its own `kms:` action;
//! * how the authority is WITHDRAWN — `RetireGrant` and `RevokeGrant`, after which the
//!   same tokenless request is refused.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{b64, error_code, error_message, text, REGION};

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record of it, so it resolves to the account's ambient identity, which no
/// identity policy governs — the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

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
            service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
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

    /// One call to this service, signed with `access_key_id` in the SigV4 credential
    /// scope, with its HTTP status and JSON body.
    async fn call(&self, access_key_id: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://kms.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("TrentService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/kms/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, payload)
    }

    /// One call that must succeed.
    async fn ok(&self, access_key_id: &str, operation: &str, body: &Value) -> Value {
        let (status, payload) = self.call(access_key_id, operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation} {body}: expected HTTP 200, got {status} with {payload}"
        );
        payload
    }

    /// One awsQuery call to IAM or STS.
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
            .uri(format!("https://{signing_name}.amazonaws.com/"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={ACCOUNT_KEY}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// Create an IAM role with `policy` as its inline policy (when one is given), assume
    /// it through STS, and answer the session's access key id and the session ARN STS
    /// reports for it.
    async fn assumed_role(
        &self,
        role_name: &str,
        policy: Option<&str>,
    ) -> (String, String) {
        let account = engine::emulator_account_id();
        let trust = format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
             \"Principal\":{{\"AWS\":\"arn:aws:iam::{account}:root\"}},\
             \"Action\":\"sts:AssumeRole\"}}]}}"
        );
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateRole",
                &[
                    ("RoleName", role_name.to_string()),
                    ("AssumeRolePolicyDocument", trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "creating the role {role_name:?} must succeed, got {status} {body}"
        );
        if let Some(policy) = policy {
            let (status, body) = self
                .query(
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
            assert_eq!(
                status, 200,
                "putting the inline policy of {role_name:?} must succeed, got {status} {body}"
            );
        }
        let (status, body) = self
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
        assert_eq!(
            status, 200,
            "assuming {role_name:?} must succeed, got {status} {body}"
        );
        let key = xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("AssumeRole must answer an AccessKeyId, got {body}"));
        let arn = xml_element(&body, "Arn")
            .unwrap_or_else(|| panic!("AssumeRole must answer the session Arn, got {body}"));
        (key, arn)
    }

    /// Create an IAM user with `policy` as its inline policy (when one is given) and
    /// answer the access key id of a long-term key minted for it, with the user's ARN.
    async fn iam_user(&self, user_name: &str, policy: Option<&str>) -> (String, String) {
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateUser",
                &[("UserName", user_name.to_string())],
            )
            .await;
        assert_eq!(
            status, 200,
            "creating the user {user_name:?} must succeed, got {status} {body}"
        );
        let arn = xml_element(&body, "Arn")
            .unwrap_or_else(|| panic!("CreateUser must answer the user Arn, got {body}"));
        if let Some(policy) = policy {
            let (status, body) = self
                .query(
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
            assert_eq!(
                status, 200,
                "putting the inline policy of {user_name:?} must succeed, got {status} {body}"
            );
        }
        let (status, body) = self
            .query(
                "iam",
                "2010-05-08",
                "CreateAccessKey",
                &[("UserName", user_name.to_string())],
            )
            .await;
        assert_eq!(
            status, 200,
            "minting a key for {user_name:?} must succeed, got {status} {body}"
        );
        let key = xml_element(&body, "AccessKeyId")
            .unwrap_or_else(|| panic!("CreateAccessKey must answer an AccessKeyId, got {body}"));
        (key, arn)
    }

    /// Create a symmetric encryption key carrying `policy`, and answer its key id and ARN.
    async fn key(&self, policy: Option<&str>) -> (String, String) {
        let mut request = json!({"Description": "authorization"});
        if let Some(policy) = policy {
            request["Policy"] = json!(policy);
        }
        let metadata = self.ok(ACCOUNT_KEY, "CreateKey", &request).await["KeyMetadata"].clone();
        (
            text("setup", &metadata, "KeyId"),
            text("setup", &metadata, "Arn"),
        )
    }
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal — so a policy document's braces and quotes survive
/// the round trip.
///
/// A copy: an integration-test module cannot be imported across crates, so every service
/// suite that drives an awsQuery service holds its own
/// (`services/service-com-amazonaws-sqs-amazonsqs/tests/all/support.rs` is the one to
/// migrate onto if a shared test-support crate ever lands).
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

/// The text of the first `<name>…</name>` element of an awsQuery response. A copy, for the
/// same reason as [`form_encode`].
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// A key policy document carrying `statements`.
fn policy(statements: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// A key policy statement allowing `actions` to `principal` on every resource the policy
/// is attached to.
fn allow_to(principal: &str, actions: Value) -> Value {
    json!({
        "Effect": "Allow",
        "Principal": {"AWS": principal},
        "Action": actions,
        "Resource": "*",
    })
}

/// The key policy statement every key carries by default: "the default key policy …
/// gives the Amazon Web Services account that owns the KMS key permission to use it"
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="Policy")`), which names the
/// account root and so delegates the decision to the account's identity policies.
fn account_root_statement() -> Value {
    allow_to(
        &format!("arn:aws:iam::{}:root", engine::emulator_account_id()),
        json!("kms:*"),
    )
}

/// An identity policy document allowing `actions` on `resource`.
fn identity_allow(actions: Value, resource: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": resource}],
    })
    .to_string()
}

/// The sentence AWS words a refusal that no document allowed with, which is what the
/// service answers for every operation below.
fn denial(principal: &str, operation: &str, key_arn: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: kms:{operation} on resource: \
         {key_arn} because no identity-based policy allows the kms:{operation} action"
    )
}

/// Assert `payload` is the authorization refusal, with AWS's own sentence.
fn assert_denied(case: &str, status: u16, payload: &Value, expected_message: &str) {
    assert_eq!(
        (status, error_code(payload).as_str()),
        (400, "AccessDeniedException"),
        "case {case:?}: expected HTTP 400 AccessDeniedException, got {status} with {payload}"
    );
    assert_eq!(
        error_message(payload),
        expected_message,
        "case {case:?}: expected the IAM denial sentence {expected_message:?}, got {payload}"
    );
}

// ---------------------------------------------------------------------------
// Which authority serves a request
// ---------------------------------------------------------------------------

/// A request is served exactly when one of the three authorities in force allows it, and
/// refused with AWS's denial sentence when none does — for every authority, every kind of
/// caller and every operation in the table.
#[tokio::test]
async fn only_a_caller_an_authority_in_force_reaches_is_served() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let role_arn = |name: &str| format!("arn:aws:iam::{account}:role/{name}");

    // A key whose policy carries only the account-root statement: the default one, which
    // delegates to the account's own identity policies.
    let (delegating_key, delegating_arn) = emulator.key(None).await;
    // A key whose policy names ONE role directly, for `kms:DescribeKey` alone — the
    // least-privilege policy a client writes to withhold grant issuance.
    let (describe_key_id, describe_arn) = emulator
        .key(Some(&policy(json!([
            account_root_statement(),
            allow_to(&role_arn("DescribeOnly"), json!("kms:DescribeKey")),
        ]))))
        .await;
    // A key whose policy names one role for the whole data plane and grant issuance.
    let (named_key_id, named_arn) = emulator
        .key(Some(&policy(json!([
            account_root_statement(),
            allow_to(
                &role_arn("Named"),
                json!(["kms:Encrypt", "kms:Decrypt", "kms:CreateGrant"]),
            ),
        ]))))
        .await;
    // A key whose policy DENIES one role everything, beside the account statement: an
    // explicit deny wins over every other authority, including a grant.
    let (denied_key_id, denied_arn) = emulator
        .key(Some(&policy(json!([
            account_root_statement(),
            {
                "Effect": "Deny",
                "Principal": {"AWS": role_arn("Denied")},
                "Action": "kms:*",
                "Resource": "*",
            },
        ]))))
        .await;

    let (describe_only, describe_only_arn) = emulator.assumed_role("DescribeOnly", None).await;
    let (named, named_session_arn) = emulator.assumed_role("Named", None).await;
    let (denied, denied_session_arn) = emulator.assumed_role("Denied", None).await;
    let (policied, policied_arn) = emulator
        .assumed_role(
            "Policied",
            Some(&identity_allow(
                json!(["kms:Encrypt", "kms:CreateGrant"]),
                &delegating_arn,
            )),
        )
        .await;
    let (bare, bare_arn) = emulator.assumed_role("Bare", None).await;
    let (user, user_arn) = emulator
        .iam_user(
            "Operator",
            Some(&identity_allow(json!("kms:*"), &delegating_arn)),
        )
        .await;
    let (bare_user, bare_user_arn) = emulator.iam_user("Nobody", None).await;
    let ambient_arn = format!("arn:aws:iam::{account}:user/emulator");

    // The grant the "Denied" role holds, which the key policy's explicit deny overrides.
    emulator
        .ok(
            ACCOUNT_KEY,
            "CreateGrant",
            &json!({
                "KeyId": denied_key_id,
                "GranteePrincipal": role_arn("Denied"),
                "Operations": ["Encrypt", "Decrypt"],
            }),
        )
        .await;
    // The grant the "Bare" role holds on the delegating key: its only authority.
    emulator
        .ok(
            ACCOUNT_KEY,
            "CreateGrant",
            &json!({
                "KeyId": delegating_key,
                "GranteePrincipal": role_arn("Bare"),
                "Operations": ["Encrypt"],
            }),
        )
        .await;

    // (case, caller's access key, caller ARN, key id, key ARN, operation, allowed)
    let rows: Vec<(&str, &str, &str, &str, &str, &str, bool)> = vec![
        (
            "the account's own credential, on a key policy that delegates to it",
            ACCOUNT_KEY,
            &ambient_arn,
            &delegating_key,
            &delegating_arn,
            "Encrypt",
            true,
        ),
        (
            "the account's own credential issuing a grant",
            ACCOUNT_KEY,
            &ambient_arn,
            &delegating_key,
            &delegating_arn,
            "CreateGrant",
            true,
        ),
        (
            "a role the key policy allows only DescribeKey, issuing a grant",
            &describe_only,
            &describe_only_arn,
            &describe_key_id,
            &describe_arn,
            "CreateGrant",
            false,
        ),
        (
            "a role the key policy allows only DescribeKey, encrypting",
            &describe_only,
            &describe_only_arn,
            &describe_key_id,
            &describe_arn,
            "Encrypt",
            false,
        ),
        (
            "a role the key policy names for this action",
            &named,
            &named_session_arn,
            &named_key_id,
            &named_arn,
            "Encrypt",
            true,
        ),
        (
            "a role the key policy names, issuing a grant",
            &named,
            &named_session_arn,
            &named_key_id,
            &named_arn,
            "CreateGrant",
            true,
        ),
        (
            "a role the key policy names, on an action it does not name",
            &named,
            &named_session_arn,
            &named_key_id,
            &named_arn,
            "GenerateDataKey",
            false,
        ),
        (
            "a role whose identity policy allows the action under a delegating key policy",
            &policied,
            &policied_arn,
            &delegating_key,
            &delegating_arn,
            "Encrypt",
            true,
        ),
        (
            "a role whose identity policy allows another action",
            &policied,
            &policied_arn,
            &delegating_key,
            &delegating_arn,
            "GenerateDataKey",
            false,
        ),
        (
            "a role holding a grant for the action, with no policy at all",
            &bare,
            &bare_arn,
            &delegating_key,
            &delegating_arn,
            "Encrypt",
            true,
        ),
        (
            "a role holding a grant for another action",
            &bare,
            &bare_arn,
            &delegating_key,
            &delegating_arn,
            "GenerateDataKey",
            false,
        ),
        (
            "a role holding a grant the key policy explicitly denies",
            &denied,
            &denied_session_arn,
            &denied_key_id,
            &denied_arn,
            "Encrypt",
            false,
        ),
        (
            "an IAM user whose inline policy allows every KMS action",
            &user,
            &user_arn,
            &delegating_key,
            &delegating_arn,
            "Encrypt",
            true,
        ),
        (
            "an IAM user with no policy at all",
            &bare_user,
            &bare_user_arn,
            &delegating_key,
            &delegating_arn,
            "Encrypt",
            false,
        ),
    ];

    for (case, key, caller_arn, key_id, key_arn, operation, allowed) in rows {
        let body = match operation {
            "Encrypt" => json!({"KeyId": key_id, "Plaintext": b64(b"authorize me")}),
            "GenerateDataKey" => json!({"KeyId": key_id, "KeySpec": "AES_256"}),
            "CreateGrant" => json!({
                "KeyId": key_id,
                "GranteePrincipal": format!("arn:aws:iam::{account}:role/Grantee"),
                "Operations": ["Decrypt"],
            }),
            other => panic!("case {case:?}: {other} is not an operation this table drives"),
        };
        let (status, payload) = emulator.call(key, operation, &body).await;
        if allowed {
            assert_eq!(
                status, 200,
                "case {case:?}: expected {operation} to be served, got {status} with {payload}"
            );
        } else {
            // The refusal names the caller, the action and the key: the only three things
            // the decision was made about.
            let expected = if case.contains("explicitly denies") {
                format!(
                    "User: {caller_arn} is not authorized to perform: kms:{operation} on \
                     resource: {key_arn} with an explicit deny in a resource-based policy"
                )
            } else {
                denial(caller_arn, operation, key_arn)
            };
            assert_denied(case, status, &payload, &expected);
        }
    }

    // Nothing was persisted for a refused grant request: the key the least-privilege
    // policy governs holds no grant at all.
    let listed = emulator
        .ok(ACCOUNT_KEY, "ListGrants", &json!({"KeyId": describe_key_id}))
        .await;
    assert_eq!(
        listed["Grants"],
        json!([]),
        "a refused CreateGrant must mint no grant, got {listed}"
    );
}

// ---------------------------------------------------------------------------
// Withdrawing a grant
// ---------------------------------------------------------------------------

/// A grant is the only authority a bare role has on a key, and withdrawing it withdraws
/// the access: the tokenless request the grantee could make is refused from the next call
/// on, whichever operation withdrew it.
///
/// The rows are the two operations that withdraw a grant (`RetireGrant` by grant id, and
/// `RevokeGrant`) and the two cryptographic operations the grant permitted.
#[tokio::test]
async fn withdrawing_a_grant_withdraws_the_access_it_carried() {
    let account = engine::emulator_account_id();

    for withdraw in ["RetireGrant", "RevokeGrant"] {
        let emulator = Emulator::new();
        let (key_id, key_arn) = emulator.key(None).await;
        let role = format!("Granted{withdraw}");
        let (session, session_arn) = emulator.assumed_role(&role, None).await;
        let role_arn = format!("arn:aws:iam::{account}:role/{role}");

        let created = emulator
            .ok(
                ACCOUNT_KEY,
                "CreateGrant",
                &json!({
                    "KeyId": key_id,
                    "GranteePrincipal": role_arn,
                    "RetiringPrincipal": role_arn,
                    "Operations": ["Encrypt", "Decrypt"],
                }),
            )
            .await;
        let grant_id = text(withdraw, &created, "GrantId");

        // The grantee holds the grant's permissions without presenting anything: the
        // propagated-grant lookup, not the token path.
        let sealed = emulator
            .ok(
                &session,
                "Encrypt",
                &json!({"KeyId": key_id, "Plaintext": b64(b"granted")}),
            )
            .await;
        let blob = text(withdraw, &sealed, "CiphertextBlob");
        emulator
            .ok(&session, "Decrypt", &json!({"CiphertextBlob": blob}))
            .await;

        // Withdrawing it is the account's own call, and answers the empty 200.
        emulator
            .ok(
                ACCOUNT_KEY,
                withdraw,
                &json!({"KeyId": key_arn, "GrantId": grant_id}),
            )
            .await;

        for operation in ["Encrypt", "Decrypt"] {
            let body = match operation {
                "Encrypt" => json!({"KeyId": key_id, "Plaintext": b64(b"after withdrawal")}),
                _ => json!({"CiphertextBlob": blob}),
            };
            let (status, payload) = emulator.call(&session, operation, &body).await;
            let case = format!("{operation} after {withdraw}");
            assert_denied(
                &case,
                status,
                &payload,
                &denial(&session_arn, operation, &key_arn),
            );
        }

        // The account's own credential still reaches the key: withdrawing a grant
        // withdrew the GRANT's authority and nothing else.
        emulator
            .ok(
                ACCOUNT_KEY,
                "Encrypt",
                &json!({"KeyId": key_id, "Plaintext": b64(b"still the owner")}),
            )
            .await;
    }
}

// ---------------------------------------------------------------------------
// What a refusal says
// ---------------------------------------------------------------------------

/// `RetireGrant` refuses a grant it cannot attribute to a held grant with the
/// authorization failure that discloses nothing about the grant — not even whether it
/// exists — while `RevokeGrant`, which is the key owner's call, reports the absent grant.
///
/// The rows are the ways a `RetireGrant` can identify no held grant: a well-formed grant
/// id nothing holds, a grant id of ANOTHER key, a grant already retired, and a grant token
/// naming a grant the store no longer holds.
#[tokio::test]
async fn retiring_a_grant_nothing_holds_discloses_nothing_about_it() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let caller = format!("arn:aws:iam::{account}:user/emulator");
    let expected = format!(
        "User: {caller} is not authorized to perform: kms:RetireGrant on this resource \
         because the resource does not exist in this Region, no resource-based policies \
         allow access, or a resource-based policy explicitly denies access"
    );

    let (key_id, key_arn) = emulator.key(None).await;
    let (_, other_arn) = emulator.key(None).await;
    let created = emulator
        .ok(
            ACCOUNT_KEY,
            "CreateGrant",
            &json!({
                "KeyId": key_id,
                "GranteePrincipal": format!("arn:aws:iam::{account}:role/Grantee"),
                "Operations": ["Decrypt"],
            }),
        )
        .await;
    let grant_id = text("setup", &created, "GrantId");
    let grant_token = text("setup", &created, "GrantToken");
    emulator
        .ok(
            ACCOUNT_KEY,
            "RetireGrant",
            &json!({"KeyId": key_arn, "GrantId": grant_id}),
        )
        .await;

    // (case, the request)
    let rows: Vec<(&str, Value)> = vec![
        (
            "a well-formed grant id nothing holds",
            json!({"KeyId": key_arn, "GrantId": "0".repeat(64)}),
            ),
        (
            "a grant id of another key",
            json!({"KeyId": other_arn, "GrantId": grant_id}),
        ),
        (
            "a grant already retired",
            json!({"KeyId": key_arn, "GrantId": grant_id}),
        ),
        (
            "a grant token whose grant is gone",
            json!({"GrantToken": grant_token}),
        ),
    ];

    for (case, request) in rows {
        let (status, payload) = emulator.call(ACCOUNT_KEY, "RetireGrant", &request).await;
        assert_denied(case, status, &payload, &expected);
        assert!(
            !error_message(&payload).contains(grant_id.as_str()),
            "case {case:?}: the refusal must not disclose the grant it was asked about, \
             got {payload}"
        );
    }

    // The sibling refusal is unchanged: `RevokeGrant` is the key owner's own call, so it
    // reports the grant that is not there.
    let (status, payload) = emulator
        .call(
            ACCOUNT_KEY,
            "RevokeGrant",
            &json!({"KeyId": key_arn, "GrantId": grant_id}),
        )
        .await;
    assert_eq!(
        (status, error_code(&payload).as_str()),
        (400, "NotFoundException"),
        "case RevokeGrant on a grant nothing holds: expected HTTP 400 NotFoundException, \
         got {status} with {payload}"
    );
}
