// SPDX-License-Identifier: CC0-1.0

//! Whether the CALLER's own authority on a secret's customer managed key decides a read of
//! that secret, end to end over the wire: Secrets Manager, KMS, IAM and the token service on
//! ONE shared engine, with the key, the role and its policies written through those services'
//! own stores.
//!
//! THE RULE: an operation that cannot be served without using a customer managed KMS key
//! performs that use on the CALLER's behalf, so the authority in force on the key for THAT
//! caller — the key policy, and, where the key policy delegates to the account, the
//! principal's own identity-based policies — decides it, and a caller the key does not permit
//! is refused `AccessDeniedException` before the value is served.
//!
//! The rows differ along what the rule generalises over: WHERE the authority comes from (the
//! role's own identity policy under a key policy that delegates to the account, a key policy
//! naming the role itself, neither), how widely the identity grant is scoped (this key,
//! another key), and whether the secret names a customer managed key at all.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_based_authorization::{
    error_code, error_message, given_role_policy, given_session, role_arn, session_arn,
    ACCESS_DENIED, ACCESS_DENIED_STATUS, ACCOUNT_KEY_ID, SESSION_KEY_ID,
};
use crate::secret_client_request_token::fresh_token;

/// The Region every request below signs for: a KMS key is regional, and a secret's key is
/// used in the Region the secret lives in.
const REGION: &str = "us-west-2";

/// The whole `Message` of the refusal: Secrets Manager's own sentence for a KMS call on the
/// caller's behalf that the authority in force did not reach.
const KMS_ACCESS_DENIED_MESSAGE: &str = "Access to KMS is not allowed";

/// The value the secret under the key holds, so a served read is asserted on the material.
const SECRET_VALUE: &str = "the-value-behind-a-customer-managed-key";

/// Secrets Manager, KMS, IAM and the token service over ONE shared engine, the shape
/// `core/router/src/lib.rs` mounts them in.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
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
        given_session(&engine);
        Self { router, engine }
    }

    /// POST one awsJson1_1 operation of `service` signed with `access_key_id`.
    async fn call(
        &self,
        service: &str,
        target_prefix: &str,
        access_key_id: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{service}.{REGION}.amazonaws.com/"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={access_key_id}/20260101/{REGION}/{service}/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=deadbeef"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body is serializable"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One Secrets Manager operation signed with `access_key_id`.
    async fn sm(&self, access_key_id: &str, operation: &str, body: &Value) -> (u16, Value) {
        self.call(
            "secretsmanager",
            "secretsmanager",
            access_key_id,
            operation,
            body,
        )
        .await
    }

    /// One Secrets Manager operation, as the account's own credential, that must succeed.
    async fn sm_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.sm(ACCOUNT_KEY_ID, operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {operation}, got {status} with {parsed}"
        );
        parsed
    }

    /// One KMS operation, as the account's own credential, that must succeed.
    async fn kms_ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .call("kms", "TrentService", ACCOUNT_KEY_ID, operation, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from KMS {operation}, got {status} with {parsed}"
        );
        parsed
    }

    /// Mint a customer managed key, carrying `policy` when the row names one and AWS's
    /// default key policy otherwise, and answer its ARN.
    async fn create_key(&self, case: &str, policy: Option<Value>) -> String {
        let mut body = json!({});
        if let Some(policy) = policy {
            body["Policy"] = json!(policy.to_string());
            body["BypassPolicyLockoutSafetyCheck"] = json!(true);
        }
        let created = self.kms_ok(case, "CreateKey", &body).await;
        created["KeyMetadata"]["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: CreateKey reported no Arn in {created}"))
            .to_string()
    }
}

/// An identity-based document allowing `actions` on `resources`.
fn allow(actions: Value, resources: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": resources}],
    })
}

/// A key policy whose one statement allows `principal` and the account's own root every
/// cryptographic action — the shape that names a caller ITSELF rather than only delegating
/// to the account. The account is named beside it because the secret under the key is
/// created with the account's own credential.
fn key_policy_for(principal: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "named-principal",
            "Effect": "Allow",
            "Principal": {"AWS": [
                principal,
                format!("arn:aws:iam::{}:root", ::engine::emulator_account_id()),
            ]},
            "Action": "kms:*",
            "Resource": "*",
        }],
    })
}

/// The ARN of one AWS-MANAGED policy this emulator's IAM carries the published document of.
///
/// Source: `service_com_amazonaws_iam_awsidentitymanagementv20100508::state::
/// AWS_MANAGED_POLICIES`, the catalog of AWS-published policy documents — read from there
/// rather than spelled out, so a row attaches a policy IAM really publishes.
fn aws_managed_policy_arn(policy_name: &str) -> String {
    iam_catalog(policy_name).arn()
}

/// The catalog entry for `policy_name`, which a row attaches and whose document decides it.
fn iam_catalog(
    policy_name: &str,
) -> &'static service_com_amazonaws_iam_awsidentitymanagementv20100508::state::AwsManagedPolicy {
    service_com_amazonaws_iam_awsidentitymanagementv20100508::state::aws_managed_policy_named(
        policy_name,
    )
    .unwrap_or_else(|| panic!("IAM publishes the managed policy {policy_name:?}"))
}

/// Attach the AWS-managed policy `policy_arn` to the role the seeded session speaks for —
/// what `iam:AttachRolePolicy` does.
fn given_attached_policy(engine: &Arc<engine::InMemoryEngine>, policy_arn: &str) {
    use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

    // The role the session is a session of, named by the ARN this suite reports for it, so
    // the name is stated in exactly one place.
    let arn = role_arn();
    let role_name = arn
        .rsplit('/')
        .next()
        .expect("the role ARN names the role");
    let store = iam::service_state(&**engine).expect("the IAM store");
    let mut store = store.lock();
    store
        .role_attached_policies
        .entry(iam::name_key(role_name))
        .or_default()
        .insert(policy_arn.to_string());
}

/// One row: what the key policy says, where the role's own authority comes from and what it
/// says about the key, and whether the read is served.
struct Row {
    case: &'static str,
    /// Whether the key carries a policy naming the role itself (else AWS's default policy,
    /// which delegates to the account).
    key_policy_names_the_role: bool,
    /// Whether the role's identity policy reaches `kms:Decrypt`, and on which key.
    identity_decrypt: Decrypt,
    /// The friendly name of an AWS-managed policy the role carries ATTACHED, if any: an
    /// attached policy's document is in force on the role exactly as an inline one is.
    attached: Option<&'static str>,
    served: bool,
}

/// What the role's own identity policy says about `kms:Decrypt`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Decrypt {
    /// Nothing at all: the policy grants this service's action alone.
    NotGranted,
    /// Granted on the key the secret names.
    OnThisKey,
    /// Granted, but on another key than the one the secret names.
    OnAnotherKey,
}

const ROWS: &[Row] = &[
    Row {
        case: "the key policy delegates to the account and the role is granted no kms:Decrypt",
        key_policy_names_the_role: false,
        identity_decrypt: Decrypt::NotGranted,
        attached: None,
        served: false,
    },
    Row {
        case: "the key policy delegates to the account and the role is granted kms:Decrypt",
        key_policy_names_the_role: false,
        identity_decrypt: Decrypt::OnThisKey,
        attached: None,
        served: true,
    },
    Row {
        case: "the role's kms:Decrypt names another key than the secret's",
        key_policy_names_the_role: false,
        identity_decrypt: Decrypt::OnAnotherKey,
        attached: None,
        served: false,
    },
    Row {
        case: "the key policy names the role itself, which grants outright",
        key_policy_names_the_role: true,
        identity_decrypt: Decrypt::NotGranted,
        attached: None,
        served: true,
    },
    Row {
        // An ATTACHED AWS-managed policy is a document like any other: this one states
        // nothing about KMS, so it leaves the read refused.
        case: "the role carries an attached AWS-managed policy that states no kms:Decrypt",
        key_policy_names_the_role: false,
        identity_decrypt: Decrypt::NotGranted,
        attached: Some("AWSLambdaBasicExecutionRole"),
        served: false,
    },
    Row {
        // ...and one that DOES reach the action is a grant, which is what shows the
        // attached document is read rather than ignored.
        case: "the role carries an attached AWS-managed policy that allows every action",
        key_policy_names_the_role: false,
        identity_decrypt: Decrypt::NotGranted,
        attached: Some("AdministratorAccess"),
        served: true,
    },
];

/// RULE: the authority in force on the secret's customer managed key decides the read FOR
/// THIS CALLER, over the documents in force on the caller's identity — its inline policies
/// AND the default version of each policy attached to it — so a role none of them lets
/// reach `kms:Decrypt` on that key is refused `AccessDeniedException` under a key policy
/// that delegates to the account, and one whose documents or whose key policy reach the key
/// is served.
#[tokio::test]
async fn a_callers_own_authority_on_the_secrets_key_decides_the_read() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let emulator = Emulator::new();
        let policy = row
            .key_policy_names_the_role
            .then(|| key_policy_for(&role_arn()));
        let key_arn = emulator.create_key(case, policy).await;
        let other_key_arn = emulator.create_key(case, None).await;
        let name = format!("customer-key-authority-{index}");
        emulator
            .sm_ok(
                case,
                "CreateSecret",
                &json!({
                    "Name": &name,
                    "KmsKeyId": &key_arn,
                    "SecretString": SECRET_VALUE,
                    "ClientRequestToken": fresh_token(),
                }),
            )
            .await;

        let decrypt_resource = match row.identity_decrypt {
            Decrypt::NotGranted => None,
            Decrypt::OnThisKey => Some(key_arn.clone()),
            Decrypt::OnAnotherKey => Some(other_key_arn.clone()),
        };
        let document = match &decrypt_resource {
            None => allow(json!("secretsmanager:GetSecretValue"), json!("*")),
            Some(resource) => json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Action": "secretsmanager:GetSecretValue",
                        "Resource": "*",
                    },
                    {
                        "Effect": "Allow",
                        "Action": "kms:Decrypt",
                        "Resource": resource,
                    },
                ],
            }),
        };
        given_role_policy(&emulator.engine, Some(&document));
        if let Some(policy_name) = row.attached {
            given_attached_policy(&emulator.engine, &aws_managed_policy_arn(policy_name));
        }

        let (status, parsed) = emulator
            .sm(SESSION_KEY_ID, "GetSecretValue", &json!({"SecretId": &name}))
            .await;
        if row.served {
            assert_eq!(
                (status, parsed["SecretString"].as_str()),
                (200, Some(SECRET_VALUE)),
                "case {case:?}: expected the value to be served to {}, got {status} with \
                 {parsed}",
                session_arn()
            );
            continue;
        }
        assert_eq!(
            (
                status,
                error_code(&parsed).as_str(),
                error_message(&parsed).as_str()
            ),
            (
                ACCESS_DENIED_STATUS,
                ACCESS_DENIED,
                KMS_ACCESS_DENIED_MESSAGE
            ),
            "case {case:?}: expected the read to be refused, got {status} with {parsed}"
        );
    }
}
