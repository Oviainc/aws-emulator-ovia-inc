// SPDX-License-Identifier: CC0-1.0

//! The ciphertext an undecrypted `SecureString` read hands out, taken straight to KMS.
//!
//! RULE: `PutParameter(Type=SecureString)` stores the value as the KMS ciphertext its key
//! sealed, bound to the parameter by the encryption context whose `PARAMETER_ARN` entry is
//! that parameter's own ARN; a read that does not ask for decryption therefore reports that
//! ciphertext base64-encoded — never the plaintext — and the bytes it reports are ones
//! `kms:Decrypt` at the same endpoint opens under that very encryption context. Because the
//! encryption context travels with the request, a `kms:Decrypt` whose permission is
//! conditioned on `kms:EncryptionContext:PARAMETER_ARN` is decided per parameter: a
//! principal the condition covers opens that parameter's value, and one it does not is
//! refused `AccessDeniedException` (HTTP 400) naming the principal, `kms:Decrypt` and the
//! key's ARN.
//!
//! That is the workflow a client runs when it takes a secret out of Parameter Store and
//! calls KMS itself — an envelope re-wrap, a cross-account key check, or verifying that a
//! key policy scoped to one parameter really does scope it. It only works if the two
//! services agree about three things at once: the bytes Parameter Store hands out are a
//! ciphertext of its own key, the encryption context it bound them to is the ARN it reports
//! for the parameter, and the decision KMS takes on them is the one the policies in force
//! state.
//!
//! Four services have to agree, so this is the suite that can pin it: Systems Manager seals
//! and reports the value, KMS holds the key and takes the decision, IAM holds the role and
//! its policy document, and STS mints the session whose principal the refusal names. Each
//! crate's own tests can assert only its own half.
//!
//! The rows span what the rule generalises over:
//!
//! * the parameter the value belongs to — a hierarchical name, a sibling under the same
//!   path, and a flat one, with values of different bytes;
//! * what the role's one policy document says: nothing at all, the action unconditioned,
//!   the action conditioned on one parameter ARN, on a list of them, on a prefix pattern,
//!   on the ENTRY NAME rather than its value, negated, the action on another key, and
//!   another action of KMS on this key;
//! * the two spellings of asking for no decryption (`WithDecryption` false and absent),
//!   which must report the same ciphertext;
//! * which half a mismatch falls on — the authorization decision (the context the request
//!   carries) or the ciphertext's own binding (the bytes presented).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, form_encode};

/// The account every generated emulator presents.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region every request here signs for. Deliberately not the emulator's default, so an
/// ARN carrying it proves it came from the request's credential scope.
const REGION: &str = "us-west-2";

/// The long-term key every emulator publishes, which is the account's own ambient
/// identity: IAM holds no policy for it, so it is the administrative caller the fixtures
/// are written as.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The `X-Amz-Target` prefix of Systems Manager's awsJson1_1 wire.
const SSM_TARGET_PREFIX: &str = "AmazonSSM";

/// The `X-Amz-Target` prefix of KMS's awsJson1_1 wire.
const KMS_TARGET_PREFIX: &str = "TrentService";

/// The session name every `AssumeRole` below asks for, which is the last segment of the
/// assumed-role ARN a refusal names.
const SESSION: &str = "probe";

/// The encryption-context entry Parameter Store binds a `SecureString`'s ciphertext to.
const PARAMETER_ARN_CONTEXT_KEY: &str = "PARAMETER_ARN";

/// That entry named as a policy condition key: `kms:EncryptionContext:<entry>` compares the
/// entry of that name in the request's encryption context with the value the policy states.
const PARAMETER_ARN_CONDITION: &str = "kms:EncryptionContext:PARAMETER_ARN";

/// The condition key naming the ENTRY NAMES of the request's encryption context, one value
/// per entry.
const CONTEXT_KEYS_CONDITION: &str = "kms:EncryptionContextKeys";

/// The KMS action a decrypt of a stored value performs on the key.
const DECRYPT_ACTION: &str = "kms:Decrypt";

/// A trust policy letting the account root assume a role.
fn trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT}:root")},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// The four services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Nothing here signs: the `Authorization` header carries only the credential scope,
    // which is where a service reads the Region and the caller's access key id.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` and whose
/// credential is `access_key_id` — the key a service reads the caller's identity from.
fn authorization(access_key_id: &str, signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
    )
}

/// One awsJson1_1 call as `access_key_id`, returning `(status, body)`. The error shape
/// travels in `__type`; the header the framework also sets is folded in so one accessor
/// reads either.
async fn json_call(
    router: &MultiServiceRouter,
    access_key_id: &str,
    signing_name: &str,
    target_prefix: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(access_key_id, signing_name))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the test request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// One Systems Manager call as `access_key_id`.
async fn ssm(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    json_call(
        router,
        access_key_id,
        "ssm",
        SSM_TARGET_PREFIX,
        operation,
        body,
    )
    .await
}

/// One KMS call as `access_key_id`.
async fn kms(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    json_call(
        router,
        access_key_id,
        "kms",
        KMS_TARGET_PREFIX,
        operation,
        body,
    )
    .await
}

/// One awsQuery form call as the ambient identity, returning `(status, body)`.
async fn query(router: &MultiServiceRouter, signing_name: &str, body: String) -> (u16, String) {
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

/// The wire error code of a failure body (`__type`, without its namespace).
fn error_code(body: &Value) -> String {
    let rendered = body["__type"].as_str().unwrap_or("<no __type>");
    rendered
        .rsplit(['#', '/'])
        .next()
        .unwrap_or(rendered)
        .to_string()
}

/// The `message`/`Message` member of a refusal, whichever casing it carries.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// The encryption context a value of the parameter `parameter_arn` is bound to.
fn context_of(parameter_arn: &str) -> Value {
    json!({ PARAMETER_ARN_CONTEXT_KEY: parameter_arn })
}

/// Create an IAM role and answer its ARN.
async fn create_role(router: &MultiServiceRouter, role_name: &str) -> String {
    let (status, body) = query(
        router,
        "iam",
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={role_name}\
             &AssumeRolePolicyDocument={}",
            form_encode(&trust_policy())
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: CreateRole must succeed, got {status} {body}"
    );
    element(&body, "Arn")
}

/// Embed `document` in `role_name` as its one inline policy.
async fn put_role_policy(router: &MultiServiceRouter, role_name: &str, document: &str) {
    let (status, body) = query(
        router,
        "iam",
        format!(
            "Action=PutRolePolicy&Version=2010-05-08&RoleName={role_name}\
             &PolicyName=scoped&PolicyDocument={}",
            form_encode(document)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: PutRolePolicy must succeed, got {status} {body}"
    );
}

/// Assume `role_arn` and answer the access key id of the session STS minted.
async fn assume_role(router: &MultiServiceRouter, role_arn: &str) -> String {
    let (status, body) = query(
        router,
        "sts",
        format!(
            "Action=AssumeRole&Version=2011-06-15&RoleArn={}&RoleSessionName={SESSION}",
            form_encode(role_arn)
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "AssumeRole({role_arn}) must succeed, got {status} {body}"
    );
    element(&body, "AccessKeyId")
}

/// The assumed-role ARN a session of `role_name` speaks as, which is the principal a
/// refusal of that session must blame.
fn session_principal(role_name: &str) -> String {
    format!("arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/{SESSION}")
}

/// One identity-based document whose single `Allow` names `action` on `resource`, scoped by
/// `condition` when there is one.
fn allow(action: &str, resource: &str, condition: Option<Value>) -> String {
    let mut statement = json!({
        "Effect": "Allow",
        "Action": action,
        "Resource": resource,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    json!({ "Version": "2012-10-17", "Statement": [statement] }).to_string()
}

/// The refusal AWS words for an action no identity-based policy in force allows.
fn no_identity_policy_refusal(principal: &str, action: &str, resource: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} \
         because no identity-based policy allows the {action} action"
    )
}

/// One `SecureString` parameter of the fixture: what was written, what the undecrypted read
/// reported, and the ARN the read reported for it.
struct Sealed {
    /// The case label this parameter stands for.
    case: &'static str,
    /// The parameter's name, as the put wrote it.
    name: &'static str,
    /// The plaintext the put wrote.
    value: &'static str,
    /// The ARN `GetParameter` reports, which is the `PARAMETER_ARN` entry of the
    /// encryption context its ciphertext is bound to.
    arn: String,
    /// The base64 ciphertext the undecrypted read reported.
    ciphertext: String,
}

/// The emulator the suites drive, with the parameters already sealed under one key.
struct Fixture {
    router: MultiServiceRouter,
    /// The key every parameter below is sealed under, by ARN.
    key_arn: String,
    /// A second key of the same account, named by the row whose policy allows the action
    /// somewhere else.
    other_key_arn: String,
    sealed: Vec<Sealed>,
}

/// The parameters every table below reads: a hierarchical name, a sibling under the same
/// path, and a flat one, with values of different bytes.
const PARAMETERS: &[(&str, &str, &str)] = &[
    ("a hierarchical name", "/sim-scope/alpha", "alpha-plaintext"),
    (
        "a sibling under the same path",
        "/sim-scope/beta",
        "bêta-plaintext-ключ",
    ),
    ("a flat name", "gamma-flat", "gamma,plaintext,list"),
];

/// Create a symmetric encryption key through KMS's own `CreateKey`, with the account-root
/// key policy KMS writes for a key whose create names none — the policy that delegates the
/// decision to the calling principal's own identity-based policies.
async fn create_key(router: &MultiServiceRouter, description: &str) -> String {
    let (status, created) = kms(
        router,
        AMBIENT_KEY,
        "CreateKey",
        &json!({ "Description": description }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {description:?}: CreateKey must succeed, got {status} {created}"
    );
    created["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {description:?}: CreateKey reports an Arn, got {created}"))
        .to_string()
}

impl Fixture {
    /// Stand the emulator up, seal every parameter of [`PARAMETERS`] under one key, and
    /// assert the undecrypted read of each one reports a base64 KMS ciphertext rather than
    /// the value — the half of the rule Parameter Store owns.
    async fn new() -> Self {
        let router = build_router();
        let key_arn = create_key(&router, "the suite's parameter key").await;
        let other_key_arn = create_key(&router, "a key no parameter is sealed under").await;

        let mut sealed = Vec::new();
        for (case, name, value) in PARAMETERS {
            let (status, put) = ssm(
                &router,
                AMBIENT_KEY,
                "PutParameter",
                &json!({
                    "Name": name,
                    "Type": "SecureString",
                    "Value": value,
                    "KeyId": key_arn,
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: PutParameter must succeed, got {status} {put}"
            );

            // Both spellings of asking for no decryption report the SAME ciphertext, and
            // neither reports the value.
            let mut reported: Vec<(String, String)> = Vec::new();
            for flag in [None, Some(false)] {
                let mut request = json!({ "Name": name });
                if let Some(flag) = flag {
                    request["WithDecryption"] = json!(flag);
                }
                let (status, read) = ssm(&router, AMBIENT_KEY, "GetParameter", &request).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} with WithDecryption {flag:?}: GetParameter must succeed, got \
                     {status} {read}"
                );
                let held = read["Parameter"]["Value"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the read reports a Value, got {read}")
                    })
                    .to_string();
                let arn = read["Parameter"]["ARN"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: the read reports an ARN, got {read}"))
                    .to_string();
                assert_ne!(
                    held.as_str(), *value,
                    "case {case:?} with WithDecryption {flag:?}: the undecrypted read must not \
                     report the plaintext, got {held:?}"
                );
                let blob = ::aws_smithy_types::base64::decode(&held).unwrap_or_else(|error| {
                    panic!(
                        "case {case:?} with WithDecryption {flag:?}: the undecrypted read must \
                         report base64, got {held:?} ({error})"
                    )
                });
                assert!(
                    !blob.is_empty() && blob != value.as_bytes(),
                    "case {case:?} with WithDecryption {flag:?}: the reported bytes must be a \
                     ciphertext rather than the value, got {held:?}"
                );
                reported.push((held, arn));
            }
            let (ciphertext, arn) = reported
                .first()
                .cloned()
                .expect("both read forms were asked");
            assert_eq!(
                reported[1].0, ciphertext,
                "case {case:?}: the two spellings of asking for no decryption must report the \
                 same ciphertext, got {:?}",
                reported[1].0
            );
            assert_eq!(
                reported[1].1, arn,
                "case {case:?}: both reads must report the same ARN, got {:?}",
                reported[1].1
            );

            // The decrypting read of the same parameter reports the value, so the ciphertext
            // above is the OTHER form of a value that round-trips rather than a lost one.
            let (status, opened) = ssm(
                &router,
                AMBIENT_KEY,
                "GetParameter",
                &json!({ "Name": name, "WithDecryption": true }),
            )
            .await;
            assert_eq!(
                (status, opened["Parameter"]["Value"].as_str()),
                (200, Some(*value)),
                "case {case:?}: the decrypting read must report the value the put wrote, got \
                 {status} {opened}"
            );

            sealed.push(Sealed {
                case,
                name,
                value,
                arn,
                ciphertext,
            });
        }

        Self {
            router,
            key_arn,
            other_key_arn,
            sealed,
        }
    }

    /// `kms:Decrypt` of `ciphertext` under `context`, as `access_key_id`.
    ///
    /// `CiphertextBlob` is a blob, so the wire carries it base64-encoded — which is exactly
    /// the form the parameter read reported it in, with no re-encoding in between.
    async fn decrypt(
        &self,
        access_key_id: &str,
        ciphertext: &str,
        context: Option<Value>,
    ) -> (u16, Value) {
        let mut body = json!({ "CiphertextBlob": ciphertext });
        if let Some(context) = context {
            body["EncryptionContext"] = context;
        }
        kms(&self.router, access_key_id, "Decrypt", &body).await
    }

    /// Create `role_name` with `document` as its one inline policy (or with none at all),
    /// and answer the access key id of a session of it.
    async fn session(&self, role_name: &str, document: Option<String>) -> String {
        let role_arn = create_role(&self.router, role_name).await;
        if let Some(document) = &document {
            put_role_policy(&self.router, role_name, document).await;
        }
        assume_role(&self.router, &role_arn).await
    }
}

/// One row of [`an_encryption_context_condition_scopes_a_direct_decrypt`]: what the role's
/// one policy document says, and which parameters' values it therefore opens.
struct Row {
    case: &'static str,
    role_name: &'static str,
    /// The role's one inline policy, or `None` for a role the account grants nothing.
    document: Option<String>,
    /// The names of the parameters whose ciphertext this role may open.
    opens: Vec<&'static str>,
}

/// The table, built once the fixture's ARNs are known: a policy document names the key by
/// ARN and conditions the action on the ARN of the parameter the value belongs to.
fn rows(fixture: &Fixture) -> Vec<Row> {
    let key = fixture.key_arn.as_str();
    let arn_of = |name: &str| {
        fixture
            .sealed
            .iter()
            .find(|sealed| sealed.name == name)
            .map(|sealed| sealed.arn.clone())
            .expect("the fixture sealed every parameter the table names")
    };
    let alpha = arn_of("/sim-scope/alpha");
    let beta = arn_of("/sim-scope/beta");
    let gamma = arn_of("gamma-flat");
    // The prefix pattern covers the two parameters of one hierarchy and not the flat one.
    let hierarchy = format!("arn:aws:ssm:{REGION}:{ACCOUNT}:parameter/sim-scope/*");

    vec![
        Row {
            case: "a role the account grants nothing",
            role_name: "ctx-nothing",
            document: None,
            opens: vec![],
        },
        Row {
            case: "the action on the key, unconditioned",
            role_name: "ctx-unconditioned",
            document: Some(allow(DECRYPT_ACTION, key, None)),
            opens: vec!["/sim-scope/alpha", "/sim-scope/beta", "gamma-flat"],
        },
        Row {
            case: "conditioned on one parameter's ARN",
            role_name: "ctx-one-arn",
            document: Some(allow(
                DECRYPT_ACTION,
                key,
                Some(json!({ "StringEquals": { PARAMETER_ARN_CONDITION: alpha } })),
            )),
            opens: vec!["/sim-scope/alpha"],
        },
        Row {
            case: "conditioned on a list of two parameter ARNs",
            role_name: "ctx-two-arns",
            document: Some(allow(
                DECRYPT_ACTION,
                key,
                Some(json!({ "StringEquals": { PARAMETER_ARN_CONDITION: [alpha, gamma] } })),
            )),
            opens: vec!["/sim-scope/alpha", "gamma-flat"],
        },
        Row {
            case: "conditioned by a pattern on the hierarchy the parameters sit in",
            role_name: "ctx-prefix",
            document: Some(allow(
                DECRYPT_ACTION,
                key,
                Some(json!({ "StringLike": { PARAMETER_ARN_CONDITION: hierarchy } })),
            )),
            opens: vec!["/sim-scope/alpha", "/sim-scope/beta"],
        },
        Row {
            case: "conditioned on every parameter's ARN but one",
            role_name: "ctx-negated",
            document: Some(allow(
                DECRYPT_ACTION,
                key,
                Some(json!({ "StringNotEquals": { PARAMETER_ARN_CONDITION: beta } })),
            )),
            opens: vec!["/sim-scope/alpha", "gamma-flat"],
        },
        Row {
            case: "conditioned on the encryption-context entry name rather than its value",
            role_name: "ctx-entry-name",
            document: Some(allow(
                DECRYPT_ACTION,
                key,
                Some(json!({
                    "ForAnyValue:StringEquals": {
                        CONTEXT_KEYS_CONDITION: PARAMETER_ARN_CONTEXT_KEY,
                    },
                })),
            )),
            opens: vec!["/sim-scope/alpha", "/sim-scope/beta", "gamma-flat"],
        },
        Row {
            case: "the action on another key of the same account",
            role_name: "ctx-other-key",
            document: Some(allow(DECRYPT_ACTION, &fixture.other_key_arn, None)),
            opens: vec![],
        },
        Row {
            case: "another action of this service on the key",
            role_name: "ctx-other-action",
            document: Some(allow("kms:Encrypt", key, None)),
            opens: vec![],
        },
    ]
}

/// The base64 ciphertext an undecrypted read hands out is one the emulator's own
/// `kms:Decrypt` opens under the parameter's encryption context — and whether it opens for
/// a given principal is the decision that principal's policy states about
/// `kms:EncryptionContext:PARAMETER_ARN`, per parameter.
#[tokio::test]
async fn an_encryption_context_condition_scopes_a_direct_decrypt() {
    let fixture = Fixture::new().await;

    for row in rows(&fixture) {
        let session = fixture.session(row.role_name, row.document.clone()).await;
        let principal = session_principal(row.role_name);

        for sealed in &fixture.sealed {
            let case = format!("{} opening {}", row.case, sealed.case);
            let (status, answer) = fixture
                .decrypt(
                    &session,
                    &sealed.ciphertext,
                    Some(context_of(&sealed.arn)),
                )
                .await;

            if row.opens.contains(&sealed.name) {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the policy covers this parameter, so kms:Decrypt must \
                     succeed, got {status} {answer}"
                );
                assert_eq!(
                    answer["Plaintext"].as_str(),
                    Some(::aws_smithy_types::base64::encode(sealed.value).as_str()),
                    "case {case:?}: kms:Decrypt must recover the value the put wrote, got \
                     {answer}"
                );
                assert_eq!(
                    answer["KeyId"].as_str(),
                    Some(fixture.key_arn.as_str()),
                    "case {case:?}: kms:Decrypt must report the key the value is sealed \
                     under, got {answer}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&answer).as_str()),
                    (400, "AccessDeniedException"),
                    "case {case:?}: the policy does not cover this parameter, so kms:Decrypt \
                     must be refused, got {status} {answer}"
                );
                let stated = message_of(&answer);
                let expected =
                    no_identity_policy_refusal(&principal, DECRYPT_ACTION, &fixture.key_arn);
                assert_eq!(
                    stated, expected,
                    "case {case:?}: expected the refusal {expected:?}, got {stated:?}"
                );
            }
        }

        // Whatever the row withheld from its session, the account's own credential keeps
        // opening every value — so each refusal above is about the row's policy and not
        // about the ciphertext.
        for sealed in &fixture.sealed {
            let (status, owner) = fixture
                .decrypt(AMBIENT_KEY, &sealed.ciphertext, Some(context_of(&sealed.arn)))
                .await;
            assert_eq!(
                (status, owner["Plaintext"].as_str()),
                (
                    200,
                    Some(::aws_smithy_types::base64::encode(sealed.value).as_str())
                ),
                "case {:?} with {}: the account's own credential keeps opening the value, got \
                 {status} {owner}",
                row.case,
                sealed.case
            );
        }
    }
}

/// One row of [`the_condition_judges_the_context_and_the_ciphertext_judges_the_bytes`]: the
/// ciphertext presented, the encryption context presented with it, and the answer expected —
/// a refusal of the DECISION, a refusal of the BYTES, or (an empty `expected_code`) the
/// plaintext.
struct SplitRow<'a> {
    case: &'a str,
    ciphertext: &'a str,
    context: Option<Value>,
    expected_status: u16,
    expected_code: &'a str,
}

/// The two halves of the decision are separate and both hold: the CONDITION is judged on
/// the encryption context the request carries, while the CIPHERTEXT still opens only as the
/// parameter it was sealed for.
#[tokio::test]
async fn the_condition_judges_the_context_and_the_ciphertext_judges_the_bytes() {
    let fixture = Fixture::new().await;
    let alpha = &fixture.sealed[0];
    let beta = &fixture.sealed[1];

    // A session allowed `kms:Decrypt` on the key only under alpha's own encryption context.
    let session = fixture
        .session(
            "ctx-split",
            Some(allow(
                DECRYPT_ACTION,
                &fixture.key_arn,
                Some(json!({ "StringEquals": { PARAMETER_ARN_CONDITION: alpha.arn } })),
            )),
        )
        .await;
    let principal = session_principal("ctx-split");
    let refusal = no_identity_policy_refusal(&principal, DECRYPT_ACTION, &fixture.key_arn);

    let rows: Vec<SplitRow<'_>> = vec![
        SplitRow {
            case: "alpha's bytes under alpha's context",
            ciphertext: &alpha.ciphertext,
            context: Some(context_of(&alpha.arn)),
            expected_status: 200,
            expected_code: "",
        },
        SplitRow {
            case: "alpha's bytes under beta's context",
            ciphertext: &alpha.ciphertext,
            context: Some(context_of(&beta.arn)),
            expected_status: 400,
            expected_code: "AccessDeniedException",
        },
        SplitRow {
            case: "alpha's bytes under no context at all",
            ciphertext: &alpha.ciphertext,
            context: None,
            expected_status: 400,
            expected_code: "AccessDeniedException",
        },
        SplitRow {
            case: "beta's bytes under alpha's context",
            ciphertext: &beta.ciphertext,
            context: Some(context_of(&alpha.arn)),
            expected_status: 400,
            expected_code: "InvalidCiphertextException",
        },
    ];

    for row in rows {
        let SplitRow {
            case,
            ciphertext,
            context,
            expected_status,
            expected_code,
        } = row;
        let (status, answer) = fixture.decrypt(&session, ciphertext, context).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {answer}"
        );
        if expected_code.is_empty() {
            assert_eq!(
                answer["Plaintext"].as_str(),
                Some(::aws_smithy_types::base64::encode(alpha.value).as_str()),
                "case {case:?}: the admitted decrypt must recover alpha's value, got {answer}"
            );
            continue;
        }
        assert_eq!(
            error_code(&answer).as_str(),
            expected_code,
            "case {case:?}: expected {expected_code}, got {status} {answer}"
        );
        if expected_code == "AccessDeniedException" {
            let stated = message_of(&answer);
            assert_eq!(
                stated, refusal,
                "case {case:?}: expected the refusal {refusal:?}, got {stated:?}"
            );
        }
    }
}
