// SPDX-License-Identifier: CC0-1.0

//! `GetPolicy` reports the resource-based policy this service persists for a private CA.
//!
//! The rule under test: `GetPolicy` answers, byte for byte, the policy document
//! persisted on the private CA its `ResourceArn` names; it refuses with
//! `ResourceNotFoundException` whenever that CA or its policy is absent, with
//! `InvalidArnException` when the ARN names no private CA of this service, and it leaves
//! the stored policy exactly as it found it.
//!
//! The policy is ATTACHED here by writing the CA record in the shared engine rather than
//! through `PutPolicy`: what is under test is `GetPolicy`'s REPORTING, over documents
//! including ones no attach would accept, so the rows are not limited to what `PutPolicy`
//! takes. The attach-then-display path over the wire is driven by `policy_documents`.
//! `DeletePolicy` is still a scaffold stub
//! (`issues/things-that-do-not-work/20261022T143159Z-acm-pca-deletepolicy-stub.md`).
//! Every read goes over the wire
//! through the same router the emulator boots, so what is asserted is what a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::state::{
    service_state, INVALID_ARN_MESSAGE, POLICY_NOT_FOUND_MESSAGE,
};
use service_com_amazonaws_acmpca_acmprivateca::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service and the
/// region every ARN the request mints must carry. The router verifies no signature here
/// (`set_request_authentication(false)`); the scope is read for routing and the region.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
                             Credential=AKIDTESTTESTTEST/20260101/eu-west-1/acm-pca/aws4_request, \
                             SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The region of [`AUTHORIZATION`]'s credential scope.
const REGION: &str = "eu-west-1";

/// The account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// `AWSPolicy`'s modelled length bounds, from
/// `smithy_query(query="operation",operation="GetPolicy")`: `GetPolicyResponse$Policy`
/// is `AWSPolicy`, len 1..81920.
const POLICY_MIN_LENGTH: usize = 1;
/// Upper bound of that same `AWSPolicy` length constraint.
const POLICY_MAX_LENGTH: usize = 81_920;

/// One emulator with this service mounted, keeping the engine so a test can persist a
/// policy beside the wire and read it back through `GetPolicy`.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call `operation` and return the HTTP status with the parsed JSON body.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("https://acm-pca.eu-west-1.amazonaws.com/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("host", "acm-pca.eu-west-1.amazonaws.com")
            .header("x-amz-target", format!("ACMPrivateCA.{operation}"))
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a private CA with `common_name` as its subject and answer its ARN.
    async fn create_ca(&self, common_name: &str) -> String {
        let body = json!({
            "CertificateAuthorityConfiguration": {
                "KeyAlgorithm": "RSA_2048",
                "SigningAlgorithm": "SHA256WITHRSA",
                "Subject": {"CommonName": common_name}
            },
            "CertificateAuthorityType": "ROOT"
        });
        let (status, parsed) = self.call("CreateCertificateAuthority", &body).await;
        assert_eq!(
            status, 200,
            "case {common_name:?}: expected the CA to be created, got {status} {parsed:?}"
        );
        parsed
            .get("CertificateAuthorityArn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {common_name:?}: a created CA carries an ARN"))
            .to_string()
    }

    /// `GetPolicy` for `resource_arn`, requiring 200, and answer the `Policy` member.
    async fn policy(&self, case: &str, resource_arn: &str) -> Option<String> {
        let (status, parsed) = self
            .call("GetPolicy", &json!({"ResourceArn": resource_arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetPolicy to answer 200, got {status} {parsed:?}"
        );
        parsed
            .get("Policy")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// `GetPolicy` for `resource_arn`, requiring HTTP 400 with `code` and `message`.
    async fn refused(&self, case: &str, resource_arn: &str, code: &str, message: &str) {
        let (status, parsed) = self
            .call("GetPolicy", &json!({"ResourceArn": resource_arn}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected GetPolicy to refuse with 400 {code}, got {status} {parsed:?}"
        );
        let wire_code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            wire_code.contains(code),
            "case {case:?}: expected GetPolicy to answer {code}, got {wire_code:?} ({parsed:?})"
        );
        let wire_message = parsed
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            wire_message, message,
            "case {case:?}: expected the message {message:?}, got {wire_message:?}"
        );
    }

    /// Persist `policy` on the CA `ca_arn` names, beside the wire: this suite pins what
    /// `GetPolicy` reports for whatever is stored, including documents `PutPolicy` would
    /// refuse to attach.
    fn attach_policy(&self, case: &str, ca_arn: &str, policy: &str) {
        let state = service_state(&*self.engine).expect("the private CA store is reachable");
        let mut state = state.lock();
        let record = state
            .certificate_authorities
            .get_mut(ca_arn)
            .unwrap_or_else(|| panic!("case {case:?}: the CA {ca_arn} was created"));
        record.resource_policy = Some(policy.to_string());
    }

    /// The policy the store holds for `ca_arn`, read without going through the wire.
    fn stored_policy(&self, ca_arn: &str) -> Option<String> {
        let state = service_state(&*self.engine).expect("the private CA store is reachable");
        let state = state.lock();
        state
            .certificate_authorities
            .get(ca_arn)
            .and_then(|record| record.resource_policy.clone())
    }
}

/// A policy document of exactly `length` characters.
///
/// Up to the modelled maximum it is a real IAM policy whose `Sid` is padded out, so a row
/// at a length bound is still the kind of document a CA carries; a length too short to
/// frame one is filled with a single repeated character, which `AWSPolicy`'s pattern
/// (`^[\u0009\u000A\u000D\u0020-\u00FF]+$`) admits.
fn policy_of_length(length: usize) -> String {
    let head = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Sid\":\"";
    let tail = "\",\"Effect\":\"Allow\",\"Principal\":{\"AWS\":\"111122223333\"},\
                \"Action\":[\"acm-pca:IssueCertificate\"],\"Resource\":\"*\"}]}";
    match length.checked_sub(head.len() + tail.len()) {
        Some(padding) => format!("{head}{}{tail}", "A".repeat(padding)),
        None => "x".repeat(length),
    }
}

/// The policy documents this suite reads back: the rows differ in length (both modelled
/// bounds included), in statement count, in whitespace form and in character class.
fn policy_rows() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = vec![
        (
            "a single-statement policy".to_string(),
            policy_of_length(200),
        ),
        (
            "two statements and two principals".to_string(),
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Sid": "share",
                        "Effect": "Allow",
                        "Principal": {"AWS": ["111122223333", "444455556666"]},
                        "Action": ["acm-pca:DescribeCertificateAuthority",
                                   "acm-pca:GetCertificate"],
                        "Resource": "*"
                    },
                    {
                        "Sid": "issue",
                        "Effect": "Allow",
                        "Principal": {"Service": "acm.amazonaws.com"},
                        "Action": "acm-pca:IssueCertificate",
                        "Resource": "*"
                    }
                ]
            })
            .to_string(),
        ),
        (
            // `AWSPolicy`'s pattern admits tab (U+0009), LF (U+000A) and CR (U+000D), so
            // a pretty-printed document with CRLF endings is a policy like any other.
            "tabs, newlines and carriage returns".to_string(),
            "{\r\n\t\"Version\": \"2012-10-17\",\r\n\t\"Statement\": []\r\n}".to_string(),
        ),
        (
            // The same pattern admits every Latin-1 character up to U+00FF.
            "Latin-1 characters above ASCII".to_string(),
            json!({
                "Version": "2012-10-17",
                "Statement": [{"Sid": "café-naïve-£-ÿ", "Effect": "Deny",
                               "Principal": "*", "Action": "acm-pca:GetPolicy",
                               "Resource": "*"}]
            })
            .to_string(),
        ),
    ];
    // Both ends of the modelled length constraint, and a length either side of the
    // framing a real document needs.
    let lengths = [
        POLICY_MIN_LENGTH,
        2,
        64,
        1_024,
        POLICY_MAX_LENGTH - 1,
        POLICY_MAX_LENGTH,
    ];
    for length in lengths {
        rows.push((
            format!("a policy of {length} characters"),
            policy_of_length(length),
        ));
    }
    rows
}

/// Whatever is persisted for a CA is what `GetPolicy` answers, unchanged and repeatable.
#[tokio::test]
async fn get_policy_answers_the_policy_persisted_for_the_named_ca() {
    let emulator = Emulator::new();

    for (index, (case, policy)) in policy_rows().into_iter().enumerate() {
        let arn = emulator.create_ca(&format!("policy-row-{index}")).await;
        emulator.attach_policy(&case, &arn, &policy);

        let answered = emulator.policy(&case, &arn).await;
        assert_eq!(
            answered.as_deref(),
            Some(policy.as_str()),
            "case {case:?}: expected the persisted policy of {} characters, got {:?}",
            policy.len(),
            answered.as_ref().map(String::len)
        );

        // A read is a read: the second call answers the same document and the store
        // still holds it.
        let again = emulator.policy(&case, &arn).await;
        assert_eq!(
            again, answered,
            "case {case:?}: expected a repeated read to answer the same policy, got {again:?}"
        );
        assert_eq!(
            emulator.stored_policy(&arn).as_deref(),
            Some(policy.as_str()),
            "case {case:?}: expected the stored policy to survive the read, got {:?}",
            emulator.stored_policy(&arn).as_ref().map(String::len)
        );
    }
}

/// A policy belongs to the one CA it was attached to.
#[tokio::test]
async fn each_private_ca_answers_only_its_own_policy() {
    let emulator = Emulator::new();

    let mut attached: Vec<(String, String)> = Vec::new();
    for (index, (case, policy)) in policy_rows().into_iter().enumerate().take(4) {
        let arn = emulator.create_ca(&format!("own-policy-{index}")).await;
        emulator.attach_policy(&case, &arn, &policy);
        attached.push((arn, policy));
    }
    // One more CA, left without a policy, so the suite also pins that a sibling's policy
    // is not visible under its ARN.
    let bare = emulator.create_ca("own-policy-none").await;

    for (arn, policy) in &attached {
        let case = format!("the policy of {arn}");
        let answered = emulator.policy(&case, arn).await;
        assert_eq!(
            answered.as_deref(),
            Some(policy.as_str()),
            "case {case:?}: expected each CA to answer its own policy, got {:?}",
            answered.as_ref().map(String::len)
        );
    }
    emulator
        .refused(
            "a CA beside CAs that do carry a policy",
            &bare,
            "ResourceNotFoundException",
            POLICY_NOT_FOUND_MESSAGE,
        )
        .await;
}

/// "If either the private CA resource or the policy cannot be found, this action returns
/// a ResourceNotFoundException" — one refusal, about the policy, for every absence.
#[tokio::test]
async fn get_policy_refuses_a_ca_or_a_policy_it_cannot_find() {
    let emulator = Emulator::new();

    // A CA that exists and carries no policy — read immediately after the create, so the
    // CA is still in the transient CREATING status, and again after it has settled.
    let fresh = emulator.create_ca("not-found-fresh").await;
    emulator
        .refused(
            "a CA still being created",
            &fresh,
            "ResourceNotFoundException",
            POLICY_NOT_FOUND_MESSAGE,
        )
        .await;
    // A test may wait for a due instant; a handler may not.
    std::thread::sleep(
        service_com_amazonaws_acmpca_acmprivateca::state::SETTLE_AFTER
            + std::time::Duration::from_millis(250),
    );

    let rows: Vec<(String, String)> = vec![
        (
            "a settled CA with no policy attached".to_string(),
            fresh.clone(),
        ),
        (
            "a CA that was never created".to_string(),
            format!(
                "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
                 00000000-0000-4000-8000-000000000000"
            ),
        ),
        (
            "a CA id of another shape entirely".to_string(),
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/not-a-uuid"),
        ),
        (
            "a well-formed CA ARN of another account".to_string(),
            format!("arn:aws:acm-pca:{REGION}:111122223333:certificate-authority/abcdef"),
        ),
        (
            "a well-formed CA ARN of another region".to_string(),
            format!("arn:aws:acm-pca:us-gov-west-1:{ACCOUNT}:certificate-authority/abcdef"),
        ),
        (
            "a well-formed CA ARN of another partition".to_string(),
            format!("arn:aws-cn:acm-pca:cn-north-1:{ACCOUNT}:certificate-authority/abcdef"),
        ),
    ];

    for (case, resource_arn) in rows {
        emulator
            .refused(
                &case,
                &resource_arn,
                "ResourceNotFoundException",
                POLICY_NOT_FOUND_MESSAGE,
            )
            .await;
        assert_eq!(
            emulator.stored_policy(&resource_arn),
            None,
            "case {case:?}: a refused read must attach no policy"
        );
    }
}

/// An ARN of this service that names no private CA refers to no resource whose policy can
/// be read, whatever the policy store holds.
#[tokio::test]
async fn get_policy_refuses_an_arn_that_names_no_private_ca() {
    let emulator = Emulator::new();
    let arn = emulator.create_ca("not-a-ca-arn").await;
    emulator.attach_policy("a CA with a policy", &arn, &policy_of_length(120));

    // Each row satisfies `Arn`'s modelled pattern and length (so it reaches the handler)
    // and names something other than `certificate-authority/<id>` in this service.
    let rows: Vec<(&str, String)> = vec![
        (
            "no CA id",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority"),
        ),
        (
            "an extra resource segment",
            format!("{arn}/certificate/0123456789abcdef"),
        ),
        (
            "another resource type",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:private-ca/abcdef"),
        ),
        (
            "a permission of a CA",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:permission/abcdef"),
        ),
        (
            "an empty account",
            format!("arn:aws:acm-pca:{REGION}::certificate-authority/abcdef"),
        ),
        (
            "an eleven-digit account",
            format!("arn:aws:acm-pca:{REGION}:11112222333:certificate-authority/abcdef"),
        ),
        (
            "a thirteen-digit account",
            format!("arn:aws:acm-pca:{REGION}:1111222233334:certificate-authority/abcdef"),
        ),
    ];

    for (case, resource_arn) in rows {
        emulator
            .refused(
                case,
                &resource_arn,
                "InvalidArnException",
                INVALID_ARN_MESSAGE,
            )
            .await;
    }

    // And the CA whose ARN those rows were built around still answers its policy.
    let case = "the CA itself";
    let answered = emulator.policy(case, &arn).await;
    assert_eq!(
        answered,
        Some(policy_of_length(120)),
        "case {case:?}: expected the CA's own policy to be unaffected, got {answered:?}"
    );
}
