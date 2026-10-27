// SPDX-License-Identifier: CC0-1.0

//! A key-policy statement's `Condition` decides WHETHER THAT STATEMENT APPLIES to the
//! request being authorized, over the awsJson1_1 wire.
//!
//! The rule under test: a statement of a key policy contributes its effect to a request
//! only when every clause of its `Condition` block holds for that request, whichever
//! effect it states — so a statement scoped by one of the condition keys that name the
//! signing principal (`aws:PrincipalArn`, `aws:PrincipalAccount`) governs exactly the
//! principals it names, and every other principal is decided by the document's remaining
//! statements. A `Deny` of a cryptographic action conditioned on one principal therefore
//! refuses that principal alone, with `AccessDeniedException` (HTTP 400) naming the
//! signing principal and the key; a principal the condition does not name is served, and
//! an `Allow` conditioned on one principal grants to that one only.
//!
//! The decision is the real one: the principals below are made with IAM's and STS's OWN
//! operations over the same engine, so the documents this service reads are the ones those
//! services wrote.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the condition operator — `StringEquals`, `StringNotEquals`, `ArnEquals`, `ArnLike`;
//! * which principal-naming key is compared — the principal's own ARN and its account;
//! * the form of the clause's value — one string and a list of them;
//! * the effect the conditioned statement states — `Deny` and `Allow`;
//! * the kind of caller — the account's own published credential, an IAM user carrying an
//!   identity policy, an IAM user carrying none;
//! * the operation the statement names — `Decrypt`, `Encrypt`, `GenerateDataKey`, each
//!   named in the refusal as its own `kms:` action;
//! * and, beside them, a clause over a condition key this service does not model, which
//!   the shared evaluator leaves undecided and therefore reads fail-closed.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{b64, error_code, error_message, text, unb64, REGION};

/// The access-key id standing for the account's own published credential: neither STS nor
/// IAM holds a record of it, so it resolves to the account's ambient identity, which no
/// identity policy governs — the position AWS account root credentials are in.
const ACCOUNT_KEY: &str = "AKIDEMULATOR";

/// The plaintext the cases below seal and open again.
const PAYLOAD: &[u8] = b"condition-scoped key policy payload";

/// An account that is not the emulator's, for a clause that compares the caller's account
/// against another one.
const OTHER_ACCOUNT: &str = "111122223333";

/// What the gate must answer one caller for one operation on one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// HTTP 200: a statement that applies allows the request.
    Served,
    /// A statement that applies states `Deny`.
    ExplicitDeny,
    /// No statement that applies allows it — IAM's implicit deny.
    NoGrant,
}

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
            .body(bytes::Bytes::from(
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

    /// One awsQuery call to IAM or STS, signed with `access_key_id`.
    async fn query(
        &self,
        access_key_id: &str,
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
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/\
                     {signing_name}/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed awsQuery request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    /// The ARN `sts:GetCallerIdentity` reports for `access_key_id`, which is the principal
    /// a key-policy condition is compared against and the one a refusal names.
    async fn caller_arn(&self, access_key_id: &str) -> String {
        let (status, body) = self
            .query(access_key_id, "sts", "2011-06-15", "GetCallerIdentity", &[])
            .await;
        assert_eq!(
            status, 200,
            "GetCallerIdentity for {access_key_id} must succeed, got {status} {body}"
        );
        xml_element(&body, "Arn")
            .unwrap_or_else(|| panic!("GetCallerIdentity must answer an Arn, got {body}"))
    }

    /// Create an IAM user with `policy` as its inline policy (when one is given) and
    /// answer the access key id of a long-term key minted for it, with the user's ARN.
    async fn iam_user(&self, user_name: &str, policy: Option<&str>) -> (String, String) {
        let (status, body) = self
            .query(
                ACCOUNT_KEY,
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
                    ACCOUNT_KEY,
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
                ACCOUNT_KEY,
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
    async fn key(&self, policy: &str) -> (String, String) {
        let metadata = self
            .ok(
                ACCOUNT_KEY,
                "CreateKey",
                &json!({"Description": "condition scope", "Policy": policy}),
            )
            .await["KeyMetadata"]
            .clone();
        (
            text("setup", &metadata, "KeyId"),
            text("setup", &metadata, "Arn"),
        )
    }

    /// Seal [`PAYLOAD`] under `key_id` with the account's own credential, which every key
    /// policy below delegates `kms:*` to, and answer the ciphertext blob.
    async fn sealed(&self, key_id: &str) -> String {
        let answer = self
            .ok(
                ACCOUNT_KEY,
                "Encrypt",
                &json!({"KeyId": key_id, "Plaintext": b64(PAYLOAD)}),
            )
            .await;
        text("setup", &answer, "CiphertextBlob")
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

/// The key policy statement every key below carries beside the conditioned one: "the
/// default key policy … gives the Amazon Web Services account that owns the KMS key
/// permission to use it in all cryptographic operations"
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="Policy")`), which names the
/// account root and so delegates the decision to the account's identity policies.
fn account_root_statement() -> Value {
    json!({
        "Sid": "DelegateToAccount",
        "Effect": "Allow",
        "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id())},
        "Action": "kms:*",
        "Resource": "*",
    })
}

/// A key policy statement carrying `effect` on `actions` for every principal, gated by
/// `condition` — the shape a client scopes a statement to one identity with.
fn conditioned(effect: &str, actions: Value, condition: Value) -> Value {
    json!({
        "Sid": "ScopedByCondition",
        "Effect": effect,
        "Principal": {"AWS": "*"},
        "Action": actions,
        "Resource": "*",
        "Condition": condition,
    })
}

/// An identity policy document allowing `actions` on every resource.
fn identity_allow(actions: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": "*"}],
    })
    .to_string()
}

/// The request body of one operation of the table, on `key_id` or over `blob`.
fn request_for(case: &str, operation: &str, key_id: &str, blob: &str) -> Value {
    match operation {
        "Decrypt" => json!({"CiphertextBlob": blob}),
        "Encrypt" => json!({"KeyId": key_id, "Plaintext": b64(PAYLOAD)}),
        "GenerateDataKey" => json!({"KeyId": key_id, "KeySpec": "AES_256"}),
        other => panic!("case {case:?}: {other} is not an operation this table drives"),
    }
}

/// The sentence AWS words a refusal a document explicitly denied with.
fn explicit_deny(principal: &str, operation: &str, key_arn: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: kms:{operation} on resource: \
         {key_arn} with an explicit deny in a resource-based policy"
    )
}

/// The sentence AWS words a refusal that no document allowed with.
fn no_grant(principal: &str, operation: &str, key_arn: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: kms:{operation} on resource: \
         {key_arn} because no identity-based policy allows the kms:{operation} action"
    )
}

/// Drive one call and assert the gate answered `expected` for it: a served request
/// carries the operation's own answer, and a refused one carries HTTP 400
/// `AccessDeniedException` with AWS's sentence for that refusal class.
async fn assert_decision(
    emulator: &Emulator,
    case: &str,
    caller: (&str, &str),
    key: (&str, &str),
    operation: &str,
    blob: &str,
    expected: Expected,
) -> Value {
    let (access_key_id, caller_arn) = caller;
    let (key_id, key_arn) = key;
    let body = request_for(case, operation, key_id, blob);
    let (status, payload) = emulator.call(access_key_id, operation, &body).await;
    // The sentence AWS words this refusal class with, and `None` for a served request.
    let sentence = match expected {
        Expected::Served => None,
        Expected::ExplicitDeny => Some(explicit_deny(caller_arn, operation, key_arn)),
        Expected::NoGrant => Some(no_grant(caller_arn, operation, key_arn)),
    };
    match sentence {
        None => assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} by {caller_arn} to be served, got {status} \
             with {payload}"
        ),
        Some(sentence) => {
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (400, "AccessDeniedException"),
                "case {case:?}: expected {operation} by {caller_arn} to be refused HTTP 400 \
                 AccessDeniedException, got {status} with {payload}"
            );
            assert_eq!(
                error_message(&payload),
                sentence,
                "case {case:?}: expected the refusal {sentence:?} for {caller_arn}, got {payload}"
            );
        }
    }
    payload
}

/// Assert `Decrypt` answered the key that sealed the ciphertext and the exact plaintext.
fn assert_opened(case: &str, answer: &Value, key_arn: &str) {
    assert_eq!(
        text(case, answer, "KeyId"),
        key_arn,
        "case {case:?}: Decrypt must report the ARN of the key that opened the ciphertext, \
         got {answer}"
    );
    let plaintext = unb64(case, answer, "Plaintext");
    assert_eq!(
        plaintext,
        PAYLOAD.to_vec(),
        "case {case:?}: Decrypt must answer the exact bytes encrypted, got {plaintext:?}"
    );
}

// ---------------------------------------------------------------------------
// Which principals a conditioned statement governs
// ---------------------------------------------------------------------------

/// A key-policy statement gated by a condition over the keys that name the signing
/// principal applies to exactly the principals the condition names — so each of the three
/// callers below is served or refused according to whether the condition holds for IT, and
/// not according to whether the statement names `Principal: "*"`.
#[tokio::test]
async fn a_conditioned_key_policy_statement_governs_only_the_principals_it_names() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();

    // The three kinds of caller: the account's own published credential (which the
    // account-root statement delegates to), an IAM user whose identity policy allows the
    // data plane, and an IAM user carrying no identity policy at all.
    let base_arn = emulator.caller_arn(ACCOUNT_KEY).await;
    let (named_key, named_arn) = emulator
        .iam_user(
            "ConditionNamed",
            Some(&identity_allow(json!(["kms:Encrypt", "kms:Decrypt"]))),
        )
        .await;
    let (bare_key, bare_arn) = emulator.iam_user("ConditionBare", None).await;

    // Rows: the case, the conditioned statement's effect and `Condition`, and what the
    // gate must answer each of the three callers' `Decrypt` of a ciphertext sealed under
    // that row's key.
    let rows: Vec<(&str, &str, Value, [Expected; 3])> = vec![
        (
            "Deny gated on the named user's ARN",
            "Deny",
            json!({"StringEquals": {"aws:PrincipalArn": &named_arn}}),
            [Expected::Served, Expected::ExplicitDeny, Expected::NoGrant],
        ),
        (
            "Deny gated on every ARN but the named user's",
            "Deny",
            json!({"StringNotEquals": {"aws:PrincipalArn": &named_arn}}),
            [
                Expected::ExplicitDeny,
                Expected::Served,
                Expected::ExplicitDeny,
            ],
        ),
        (
            "Deny gated on the account credential's own ARN",
            "Deny",
            json!({"ArnEquals": {"aws:PrincipalArn": &base_arn}}),
            [Expected::ExplicitDeny, Expected::Served, Expected::NoGrant],
        ),
        (
            "Deny gated on an ARN pattern only the named user matches",
            "Deny",
            json!({"ArnLike": {"aws:PrincipalArn": format!("arn:aws:iam::{account}:user/ConditionN*")}}),
            [Expected::Served, Expected::ExplicitDeny, Expected::NoGrant],
        ),
        (
            "Deny gated on a list naming both IAM users",
            "Deny",
            json!({"StringEquals": {"aws:PrincipalArn": [&named_arn, &bare_arn]}}),
            [
                Expected::Served,
                Expected::ExplicitDeny,
                Expected::ExplicitDeny,
            ],
        ),
        (
            "Deny gated on the account every caller belongs to",
            "Deny",
            json!({"StringEquals": {"aws:PrincipalAccount": &account}}),
            [
                Expected::ExplicitDeny,
                Expected::ExplicitDeny,
                Expected::ExplicitDeny,
            ],
        ),
        (
            "Deny gated on an account no caller belongs to",
            "Deny",
            json!({"StringEquals": {"aws:PrincipalAccount": OTHER_ACCOUNT}}),
            [Expected::Served, Expected::Served, Expected::NoGrant],
        ),
        (
            "Deny gated on belonging to another account than one caller does",
            "Deny",
            json!({"StringNotEquals": {"aws:PrincipalAccount": OTHER_ACCOUNT}}),
            [
                Expected::ExplicitDeny,
                Expected::ExplicitDeny,
                Expected::ExplicitDeny,
            ],
        ),
        (
            "Allow gated on the ARN of the user carrying no identity policy",
            "Allow",
            json!({"StringEquals": {"aws:PrincipalArn": &bare_arn}}),
            [Expected::Served, Expected::Served, Expected::Served],
        ),
        (
            "Allow gated on an ARN no caller has",
            "Allow",
            json!({"StringEquals": {"aws:PrincipalArn": format!("arn:aws:iam::{account}:user/absent")}}),
            [Expected::Served, Expected::Served, Expected::NoGrant],
        ),
        (
            // The shared evaluator decides a clause only over the condition keys the
            // request models; one it cannot decide leaves the statement undecided, which
            // withholds a grant and keeps a refusal.
            "Deny gated on a condition key this service does not model",
            "Deny",
            json!({"StringEquals": {"kms:ViaService": format!("s3.{REGION}.amazonaws.com")}}),
            [
                Expected::ExplicitDeny,
                Expected::ExplicitDeny,
                Expected::ExplicitDeny,
            ],
        ),
    ];

    for (case, effect, condition, expected) in rows {
        let document = policy(json!([
            account_root_statement(),
            conditioned(effect, json!("kms:Decrypt"), condition),
        ]));
        let (key_id, key_arn) = emulator.key(&document).await;
        // The ciphertext every caller of this row opens: produced by the account's own
        // credential, which the row's statement never governs (it names `kms:Decrypt`).
        let blob = emulator.sealed(&key_id).await;

        for (caller, expected) in [
            (ACCOUNT_KEY, base_arn.as_str()),
            (named_key.as_str(), named_arn.as_str()),
            (bare_key.as_str(), bare_arn.as_str()),
        ]
        .into_iter()
        .zip(expected)
        {
            let case = format!("{case} — {}", caller.1);
            let answer = assert_decision(
                &emulator,
                &case,
                caller,
                (&key_id, &key_arn),
                "Decrypt",
                &blob,
                expected,
            )
            .await;
            if expected == Expected::Served {
                assert_opened(&case, &answer, &key_arn);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The actions a conditioned statement covers, and withdrawing it
// ---------------------------------------------------------------------------

/// A conditioned `Deny` refuses the principal it names on exactly the actions it names,
/// leaving that principal's other operations on the same key served — and removing the
/// statement with `PutKeyPolicy` serves the named principal from the very next call,
/// including on the ciphertext produced while the `Deny` was in force.
///
/// The rows are the three cryptographic operations the statement may name, each driven
/// beside the other two on its own key.
#[tokio::test]
async fn the_actions_a_conditioned_deny_names_are_the_ones_it_refuses() {
    let emulator = Emulator::new();
    let base_arn = emulator.caller_arn(ACCOUNT_KEY).await;
    let (named_key, named_arn) = emulator
        .iam_user("ActionNamed", Some(&identity_allow(json!("kms:*"))))
        .await;

    let operations = ["Decrypt", "Encrypt", "GenerateDataKey"];
    for denied_operation in operations {
        let document = policy(json!([
            account_root_statement(),
            conditioned(
                "Deny",
                json!(format!("kms:{denied_operation}")),
                json!({"StringEquals": {"aws:PrincipalArn": &named_arn}}),
            ),
        ]));
        let (key_id, key_arn) = emulator.key(&document).await;
        let blob = emulator.sealed(&key_id).await;

        for operation in operations {
            let case = format!("Deny of kms:{denied_operation} gated on one ARN, calling {operation}");
            // The named principal is refused exactly the denied action; every other
            // operation of the same key is still served by its identity policy under the
            // account-root delegation.
            let expected = if operation == denied_operation {
                Expected::ExplicitDeny
            } else {
                Expected::Served
            };
            let answer = assert_decision(
                &emulator,
                &case,
                (&named_key, &named_arn),
                (&key_id, &key_arn),
                operation,
                &blob,
                expected,
            )
            .await;
            if expected == Expected::Served && operation == "Decrypt" {
                assert_opened(&case, &answer, &key_arn);
            }
            // The account's own credential, which the condition never names, is served
            // every operation on the same key.
            let case = format!("{case}, as the account credential");
            let answer = assert_decision(
                &emulator,
                &case,
                (ACCOUNT_KEY, &base_arn),
                (&key_id, &key_arn),
                operation,
                &blob,
                Expected::Served,
            )
            .await;
            if operation == "Decrypt" {
                assert_opened(&case, &answer, &key_arn);
            }
        }

        // Dropping the conditioned statement serves the principal it named from the next
        // call on, over the ciphertext made while it was in force.
        emulator
            .ok(
                ACCOUNT_KEY,
                "PutKeyPolicy",
                &json!({
                    "KeyId": &key_id,
                    "PolicyName": "default",
                    "Policy": policy(json!([account_root_statement()])),
                }),
            )
            .await;
        let case = format!("after the Deny of kms:{denied_operation} is removed");
        let answer = assert_decision(
            &emulator,
            &case,
            (&named_key, &named_arn),
            (&key_id, &key_arn),
            denied_operation,
            &blob,
            Expected::Served,
        )
        .await;
        if denied_operation == "Decrypt" {
            assert_opened(&case, &answer, &key_arn);
        }
    }
}
