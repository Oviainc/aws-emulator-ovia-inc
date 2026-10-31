// SPDX-License-Identifier: CC0-1.0

//! Which `SecureString` reads the parameter's own KMS key lets through.
//!
//! A `SecureString`'s value is sealed under the key its put named, and Parameter Store
//! asks KMS to open it as the caller and through its own service path, under the
//! encryption context that binds the ciphertext to the ONE parameter it belongs to
//! (`PARAMETER_ARN` naming that parameter's ARN). So a key policy conditioned on that
//! entry admits the reads of the parameters it names and refuses every other parameter
//! sealed under the same key — the way one key serves many parameters while each
//! parameter's value stays readable only by the principals its own statement names.
//!
//! KMS is mounted BESIDE Systems Manager on one engine, so every key, alias and key
//! policy a case uses is one KMS's own operations created. Nothing here reaches into a
//! store to plant a key, a policy or a decision, and nothing re-words a refusal: the
//! sentence asserted is asked of the KMS crate.
//!
//! The tables span what the rule generalises over rather than one recorded read:
//!
//! * the condition a statement is scoped by — the operator (`StringEquals`,
//!   `StringNotEquals`, `StringLike`, and the two set qualifiers), the form of its value
//!   (one ARN and a list of them), the entry it names (`kms:EncryptionContext:PARAMETER_ARN`
//!   and `kms:EncryptionContextKeys`), and the effect the conditioned statement states
//!   (`Allow` and `Deny`);
//! * the read that reports the decrypted value (`GetParameter`, `GetParameterHistory`,
//!   `GetParametersByPath`), each of which reports a value under the same key;
//! * the reads that decrypt NOTHING and are therefore never refused (`WithDecryption`
//!   absent and false, and the two parameter types the flag is ignored for);
//! * the key a parameter names (a key id, a key ARN, an alias, the Amazon Web Services
//!   managed key a put naming none falls back to, and a key nobody created);
//! * whether the key policy confines the key to this service's path to it
//!   (`kms:ViaService`).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use ::service_com_amazonaws_kms_trentservice::state as kms_state;

use super::parameter_store::{call, error_code, ACCOUNT, REGION};

/// The encryption-context entry Parameter Store binds a `SecureString`'s ciphertext to,
/// named as a key-policy condition key: `kms:EncryptionContext:<entry>` "compares the
/// encryption context pair in the request with the pair specified in the policy" (KMS's
/// own condition keys).
const PARAMETER_ARN_CONDITION: &str = "kms:EncryptionContext:PARAMETER_ARN";

/// The condition key naming the KEYS of the request's encryption context, one value per
/// entry (KMS's own condition keys).
const CONTEXT_KEYS_CONDITION: &str = "kms:EncryptionContextKeys";

/// The condition key naming the service a KMS request was made THROUGH (KMS's own
/// condition keys).
const VIA_SERVICE_CONDITION: &str = "kms:ViaService";

/// The KMS action a read that reports a decrypted `SecureString` performs on the key.
const DECRYPT_ACTION: &str = "kms:Decrypt";

/// The parameter each table scopes the key to.
const IN_SCOPE: &str = "/scoped/alpha";

/// A second parameter sealed under the SAME key, which the scoped statements do not name.
const OUT_OF_SCOPE: &str = "/other/beta";

/// An emulator holding Systems Manager and KMS on one shared engine.
///
/// The crate's shared harness mounts Systems Manager alone, which is right for every
/// suite that asks nothing of another service; a `SecureString`'s key lives in KMS.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite signs nothing: the `Authorization` header carries only the credential
    // scope, which is where a service reads the Region and the caller's access key id.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_kms_trentservice::EmulatedService::new(engine),
    );
    router
}

/// One KMS call. KMS is an awsJson1_1 service reached by `X-Amz-Target:
/// TrentService.<Operation>`.
async fn kms(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("kms.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20240101/{REGION}/kms/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A KMS call that must succeed.
async fn kms_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, response) = kms(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case}: KMS {operation} must succeed, got {status} with {response}"
    );
    response
}

/// A symmetric encryption key carrying `document` as its key policy, as `(key id, ARN)`.
///
/// `document` is the policy UNDER TEST — it says what the PARAMETER's key permits this
/// caller and need not leave it a path to a later `kms:PutKeyPolicy`, which KMS's
/// policy-lockout safety check otherwise requires. That check has its own tests in the
/// KMS crate, so the key is created the way the KMS model provides for.
async fn key_with_policy(
    router: &MultiServiceRouter,
    case: &str,
    document: &str,
) -> (String, String) {
    let created = kms_ok(
        router,
        case,
        "CreateKey",
        &json!({
            "Description": case,
            "Policy": document,
            "BypassPolicyLockoutSafetyCheck": true,
        }),
    )
    .await;
    let metadata = &created["KeyMetadata"];
    let key_id = metadata["KeyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: CreateKey reports a KeyId, got {created}"))
        .to_string();
    let arn = metadata["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: CreateKey reports an Arn, got {created}"))
        .to_string();
    (key_id, arn)
}

/// A key policy document carrying exactly `statements`.
fn policy(statements: Vec<Value>) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// A statement allowing the calling principal `actions` on the key, gated by `condition`
/// when one is given.
fn allow_caller(actions: Value, condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Sid": "ScopedDecrypt",
        "Effect": "Allow",
        "Principal": {"AWS": caller()},
        "Action": actions,
        "Resource": "*",
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// A statement denying every principal `kms:Decrypt`, gated by `condition`.
fn deny_everyone(condition: Value) -> Value {
    json!({
        "Sid": "ScopedDeny",
        "Effect": "Deny",
        "Principal": {"AWS": "*"},
        "Action": DECRYPT_ACTION,
        "Resource": "*",
        "Condition": condition,
    })
}

/// The principal every request in this suite authenticates as, asked of the KMS crate so
/// a test cannot invent an identity the service does not publish.
fn caller() -> String {
    kms_state::caller_identity_arn()
}

/// The ARN Parameter Store mints for `name` in [`REGION`] — the value of the
/// `PARAMETER_ARN` encryption-context entry a read of it is decided under.
fn parameter_arn(name: &str) -> String {
    format!(
        "arn:aws:ssm:{REGION}:{ACCOUNT}:parameter/{}",
        name.strip_prefix('/').unwrap_or(name)
    )
}

/// Store `name` as a `SecureString` sealed under `key_id`.
async fn put_secure(router: &MultiServiceRouter, case: &str, name: &str, key_id: &str) {
    put(
        router,
        case,
        &json!({
            "Name": name,
            "Type": "SecureString",
            "Value": format!("value of {name}"),
            "KeyId": key_id,
        }),
    )
    .await;
}

/// `PutParameter`, which must succeed: the put seals the value under the key it names,
/// and what the key's policy decides is the READ that opens it again.
async fn put(router: &MultiServiceRouter, case: &str, body: &Value) {
    let (status, response) = call(router, "PutParameter", body).await;
    assert_eq!(
        status, 200,
        "case {case}: PutParameter {body} must succeed, got {status} with {response}"
    );
}

/// What a read must answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// HTTP 200, carrying the stored value.
    Served,
    /// HTTP 400 `AccessDeniedException`: the key does not permit this caller the decrypt.
    Refused,
}

/// Read `name` with `WithDecryption` of `with_decryption` (absent when `None`) and assert
/// the answer is `expected`, returning the parsed body.
async fn read_parameter(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    with_decryption: Option<bool>,
    expected: Expected,
) -> Value {
    let mut body = json!({ "Name": name });
    if let Some(flag) = with_decryption {
        body["WithDecryption"] = json!(flag);
    }
    assert_answer(router, case, "GetParameter", &body, expected).await
}

/// Call `operation` and assert it answered `expected`, returning the parsed body.
async fn assert_answer(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    expected: Expected,
) -> Value {
    let (status, response) = call(router, operation, body).await;
    match expected {
        Expected::Served => assert_eq!(
            status, 200,
            "case {case}: {operation} {body} must be served, got {status} with {response}"
        ),
        Expected::Refused => {
            assert_eq!(
                status, 400,
                "case {case}: {operation} {body} must be refused with HTTP 400, got {status} \
                 with {response}"
            );
            assert_eq!(
                error_code(&response),
                "AccessDeniedException",
                "case {case}: {operation} {body} must be refused with AccessDeniedException, \
                 got {response}"
            );
        }
    }
    response
}

/// The value a served `GetParameter` body reports.
fn reported_value(case: &str, response: &Value) -> String {
    response["Parameter"]["Value"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the read reports a Value, got {response}"))
        .to_string()
}

/// One row of [`an_encryption_context_condition_scopes_which_parameters_decrypt`]: the
/// case, the statements the key policy carries, and what a decrypting read of the
/// in-scope and of the out-of-scope parameter must answer.
type ConditionRow = (&'static str, Vec<Value>, Expected, Expected);

/// A key policy statement's `Condition` governs which requests it applies to, so a
/// statement conditioned on the `PARAMETER_ARN` entry of the request's encryption context
/// decides exactly the reads of the parameters it names — whichever operator, value form
/// and effect it states — and a read of a parameter no statement allows is refused.
#[tokio::test]
async fn an_encryption_context_condition_scopes_which_parameters_decrypt() {
    let in_scope = parameter_arn(IN_SCOPE);
    let out_of_scope = parameter_arn(OUT_OF_SCOPE);
    // `arn:aws:ssm:<region>:<account>:parameter/scoped/*`: the hierarchy the in-scope
    // parameter lives in and the out-of-scope one does not.
    let in_scope_prefix = format!("{}*", parameter_arn("/scoped/"));
    let third = parameter_arn("/elsewhere/gamma");

    let rows: Vec<ConditionRow> = vec![
        (
            "StringEquals naming the in-scope parameter",
            vec![allow_caller(
                json!(DECRYPT_ACTION),
                Some(json!({"StringEquals": {PARAMETER_ARN_CONDITION: in_scope}})),
            )],
            Expected::Served,
            Expected::Refused,
        ),
        (
            "StringEquals naming a list the in-scope parameter is in",
            vec![allow_caller(
                json!(DECRYPT_ACTION),
                Some(json!({"StringEquals": {PARAMETER_ARN_CONDITION: [third, in_scope]}})),
            )],
            Expected::Served,
            Expected::Refused,
        ),
        (
            "StringLike on the in-scope hierarchy",
            vec![allow_caller(
                json!(DECRYPT_ACTION),
                Some(json!({"StringLike": {PARAMETER_ARN_CONDITION: in_scope_prefix}})),
            )],
            Expected::Served,
            Expected::Refused,
        ),
        (
            "ForAnyValue:StringEquals naming the in-scope parameter",
            vec![allow_caller(
                json!(DECRYPT_ACTION),
                Some(json!({"ForAnyValue:StringEquals": {PARAMETER_ARN_CONDITION: in_scope}})),
            )],
            Expected::Served,
            Expected::Refused,
        ),
        (
            "StringNotEquals naming the out-of-scope parameter",
            vec![allow_caller(
                json!(DECRYPT_ACTION),
                Some(json!({"StringNotEquals": {PARAMETER_ARN_CONDITION: out_of_scope}})),
            )],
            Expected::Served,
            Expected::Refused,
        ),
        (
            "the condition names a third parameter, so neither read is allowed",
            vec![allow_caller(
                json!(DECRYPT_ACTION),
                Some(json!({"StringEquals": {PARAMETER_ARN_CONDITION: third}})),
            )],
            Expected::Refused,
            Expected::Refused,
        ),
        (
            "an unconditional allow of the decrypt",
            vec![allow_caller(json!(DECRYPT_ACTION), None)],
            Expected::Served,
            Expected::Served,
        ),
        (
            "an unconditional allow with a Deny scoped to the out-of-scope parameter",
            vec![
                allow_caller(json!(DECRYPT_ACTION), None),
                deny_everyone(json!({"StringEquals": {PARAMETER_ARN_CONDITION: out_of_scope}})),
            ],
            Expected::Served,
            Expected::Refused,
        ),
        (
            "ForAllValues:StringEquals on the entry names the read carries",
            vec![allow_caller(
                json!(DECRYPT_ACTION),
                Some(json!({"ForAllValues:StringEquals": {CONTEXT_KEYS_CONDITION: "PARAMETER_ARN"}})),
            )],
            Expected::Served,
            Expected::Served,
        ),
        (
            "the statement allows another action, not the decrypt",
            vec![allow_caller(json!("kms:Encrypt"), None)],
            Expected::Refused,
            Expected::Refused,
        ),
    ];

    for (case, statements, scoped, unscoped) in rows {
        let router = router();
        let (key_id, _arn) = key_with_policy(&router, case, &policy(statements)).await;
        put_secure(&router, case, IN_SCOPE, &key_id).await;
        put_secure(&router, case, OUT_OF_SCOPE, &key_id).await;

        for (name, expected) in [(IN_SCOPE, scoped), (OUT_OF_SCOPE, unscoped)] {
            let case = format!("{case}: {name}");
            let response =
                read_parameter(&router, &case, name, Some(true), expected).await;
            if expected == Expected::Served {
                assert_eq!(
                    reported_value(&case, &response),
                    format!("value of {name}"),
                    "case {case}: a served read reports the stored value, got {response}"
                );
            }
            // The flag selects the decrypt, so the read that asks for none is served
            // whatever the key says — and reports the stored ciphertext rather than the
            // value, which is what makes the undecrypted read no disclosure of it.
            for flag in [None, Some(false)] {
                let case = format!("{case} with WithDecryption {flag:?}");
                let response =
                    read_parameter(&router, &case, name, flag, Expected::Served).await;
                let reported = reported_value(&case, &response);
                assert_ne!(
                    reported,
                    format!("value of {name}"),
                    "case {case}: the read must report the ciphertext, not the value, got \
                     {response}"
                );
                assert!(
                    ::aws_smithy_types::base64::decode(&reported).is_ok(),
                    "case {case}: the read reports base64 ciphertext, got {reported:?}"
                );
            }
        }
    }
}

/// Every read that reports a decrypted `SecureString` asks the parameter's key, so one
/// scoped statement governs `GetParameter`, `GetParameterHistory` and
/// `GetParametersByPath` alike — and a hierarchy read that would report a parameter the
/// key refuses is refused as a whole.
#[tokio::test]
async fn every_read_that_reports_a_decrypted_value_asks_the_key() {
    let case = "one statement scoped to the in-scope parameter";
    let router = router();
    let document = policy(vec![allow_caller(
        json!(DECRYPT_ACTION),
        Some(json!({"StringEquals": {PARAMETER_ARN_CONDITION: parameter_arn(IN_SCOPE)}})),
    )]);
    let (key_id, _arn) = key_with_policy(&router, case, &document).await;
    put_secure(&router, case, IN_SCOPE, &key_id).await;
    put_secure(&router, case, OUT_OF_SCOPE, &key_id).await;

    // Each row: the operation, the request, and what it must answer.
    let rows: Vec<(&str, Value, Expected)> = vec![
        (
            "GetParameter",
            json!({ "Name": IN_SCOPE, "WithDecryption": true }),
            Expected::Served,
        ),
        (
            "GetParameter",
            json!({ "Name": OUT_OF_SCOPE, "WithDecryption": true }),
            Expected::Refused,
        ),
        (
            "GetParameterHistory",
            json!({ "Name": IN_SCOPE, "WithDecryption": true }),
            Expected::Served,
        ),
        (
            "GetParameterHistory",
            json!({ "Name": OUT_OF_SCOPE, "WithDecryption": true }),
            Expected::Refused,
        ),
        (
            "GetParametersByPath",
            json!({ "Path": "/scoped", "WithDecryption": true }),
            Expected::Served,
        ),
        (
            "GetParametersByPath",
            json!({ "Path": "/other", "WithDecryption": true }),
            Expected::Refused,
        ),
        (
            "GetParametersByPath",
            json!({ "Path": "/", "Recursive": true, "WithDecryption": true }),
            Expected::Refused,
        ),
        (
            "GetParametersByPath",
            json!({ "Path": "/", "Recursive": true }),
            Expected::Served,
        ),
    ];

    for (operation, body, expected) in rows {
        let case = format!("{case}: {operation} {body}");
        assert_answer(&router, &case, operation, &body, expected).await;
    }
}

/// A read that decrypts nothing is never refused: the flag it did not ask for, and the two
/// parameter types it is ignored for ("This flag is ignored for String and StringList
/// parameter types", `GetParameterRequest$WithDecryption`), reach no key at all.
#[tokio::test]
async fn a_read_that_decrypts_nothing_is_never_refused() {
    let case = "a key whose policy allows this caller nothing";
    let router = router();
    // The key's only grant names somebody else, so nothing it says can let this caller
    // decrypt: every read below is served because it opens nothing.
    let document = policy(vec![json!({
        "Sid": "AnotherPrincipal",
        "Effect": "Allow",
        "Principal": {"AWS": format!("arn:aws:iam::{ACCOUNT}:user/somebody-else")},
        "Action": "kms:*",
        "Resource": "*",
    })]);
    let (key_id, _arn) = key_with_policy(&router, case, &document).await;

    // Each row: the `ParameterType`, the value, and the `KeyId` to name (`None` for a
    // type that takes none).
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        ("String", "plain", None),
        ("StringList", "alpha,beta", None),
        ("SecureString", "sealed", Some(key_id.as_str())),
    ];

    for (type_, value, named_key) in rows {
        let name = format!("/nothing/{type_}");
        let mut body = json!({ "Name": name, "Type": type_, "Value": value });
        if let Some(key_id) = named_key {
            body["KeyId"] = json!(key_id);
        }
        put(&router, case, &body).await;

        // A `SecureString` asked for decrypted IS opened under the key, so only the two
        // types the flag is ignored for are served with it set.
        let decrypting = if type_ == "SecureString" {
            Expected::Refused
        } else {
            Expected::Served
        };
        for (flag, expected) in [
            (None, Expected::Served),
            (Some(false), Expected::Served),
            (Some(true), decrypting),
        ] {
            let case = format!("{case}: {type_} with WithDecryption {flag:?}");
            let response = read_parameter(&router, &case, &name, flag, expected).await;
            if expected == Expected::Served {
                let reported = reported_value(&case, &response);
                // Only an encrypted type has two forms: the undecrypted read of one
                // reports its ciphertext, while a type the flag is ignored for reports
                // the value it was written with however the read asks.
                if type_ == "SecureString" {
                    assert_ne!(
                        reported, value,
                        "case {case}: an undecrypted read of an encrypted value reports \
                         the ciphertext, got {response}"
                    );
                } else {
                    assert_eq!(
                        reported, value,
                        "case {case}: the read reports the stored value, got {response}"
                    );
                }
            }
        }
    }
}

/// How a put names the key its `SecureString` is sealed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyForm {
    /// The key id KMS minted.
    KeyId,
    /// The key's ARN.
    KeyArn,
    /// An alias of the key, which KMS resolves to it.
    Alias,
    /// No `KeyId` at all: "If you don't specify a key ID, the system uses the default key
    /// associated with your Amazon Web Services account" (`PutParameter`'s `KeyId` doc),
    /// an Amazon Web Services managed key whose policy AWS manages for the service.
    Default,
    /// A well-formed key id nobody created, which names no key at all — so a put naming
    /// it has nothing to encrypt the value under.
    Unknown,
}

/// The alias every [`KeyForm::Alias`] row points at its key with.
const SUITE_ALIAS: &str = "alias/parameter-store-suite";

/// The key a parameter is sealed under is resolved in every form a put may name it, and
/// the scoped statement decides the read the same way through each — while the Amazon Web
/// Services managed key a put naming none falls back to leaves every read served, and an
/// identifier naming no key at all leaves the put with nothing to encrypt the value
/// under, so it is refused rather than stored in the clear.
#[tokio::test]
async fn the_key_is_resolved_in_every_form_a_put_names_it() {
    let scoped_to_in_scope = policy(vec![allow_caller(
        json!(DECRYPT_ACTION),
        Some(json!({"StringEquals": {PARAMETER_ARN_CONDITION: parameter_arn(IN_SCOPE)}})),
    )]);

    // Each row: the form, and what a decrypting read of the in-scope and of the
    // out-of-scope parameter must answer when the put named the key that way — `None`
    // for a form whose put is refused, so there is nothing to read.
    let rows: [(KeyForm, Option<(Expected, Expected)>); 5] = [
        (
            KeyForm::KeyId,
            Some((Expected::Served, Expected::Refused)),
        ),
        (
            KeyForm::KeyArn,
            Some((Expected::Served, Expected::Refused)),
        ),
        (
            KeyForm::Alias,
            Some((Expected::Served, Expected::Refused)),
        ),
        (KeyForm::Default, Some((Expected::Served, Expected::Served))),
        (KeyForm::Unknown, None),
    ];

    for (form, reads) in rows {
        let case = format!("a put naming the key as {form:?}");
        let router = router();
        let (key_id, key_arn) = key_with_policy(&router, &case, &scoped_to_in_scope).await;
        if form == KeyForm::Alias {
            kms_ok(
                &router,
                &case,
                "CreateAlias",
                &json!({ "AliasName": SUITE_ALIAS, "TargetKeyId": key_id }),
            )
            .await;
        }
        let named = match form {
            KeyForm::KeyId => Some(key_id),
            KeyForm::KeyArn => Some(key_arn),
            KeyForm::Alias => Some(SUITE_ALIAS.to_string()),
            KeyForm::Default => None,
            KeyForm::Unknown => Some("11111111-2222-3333-4444-555555555555".to_string()),
        };

        for (index, parameter) in [IN_SCOPE, OUT_OF_SCOPE].into_iter().enumerate() {
            let case = format!("{case}: {parameter}");
            let mut body = json!({
                "Name": parameter,
                "Type": "SecureString",
                "Value": format!("value of {parameter}"),
            });
            if let Some(named) = &named {
                body["KeyId"] = json!(named);
            }
            let Some((scoped, unscoped)) = reads else {
                // "The query key ID isn't valid" (`InvalidKeyId`): nothing holds the key
                // the put named, so the value cannot be encrypted and nothing is stored.
                let (status, response) = call(&router, "PutParameter", &body).await;
                assert_eq!(
                    (status, error_code(&response).as_str()),
                    (400, "InvalidKeyId"),
                    "case {case}: a put naming no key must be refused, got {status} with \
                     {response}"
                );
                let (status, response) =
                    call(&router, "GetParameter", &json!({ "Name": parameter })).await;
                assert_eq!(
                    (status, error_code(&response).as_str()),
                    (400, "ParameterNotFound"),
                    "case {case}: the refused put stored nothing, got {status} with {response}"
                );
                continue;
            };
            let expected = if index == 0 { scoped } else { unscoped };
            put(&router, &case, &body).await;
            let response = read_parameter(&router, &case, parameter, Some(true), expected).await;
            if expected == Expected::Served {
                assert_eq!(
                    reported_value(&case, &response),
                    format!("value of {parameter}"),
                    "case {case}: a served read reports the value the put wrote, got {response}"
                );
            }
        }
    }
}

/// A key policy may confine a key to Parameter Store's path to it: the decrypt this
/// service performs on the caller's behalf carries this service's regional endpoint name
/// as `kms:ViaService`, so a statement naming it applies and one naming another service
/// does not.
#[tokio::test]
async fn a_via_service_condition_confines_a_key_to_this_service() {
    // Each row: the case, the `kms:ViaService` clause, and what a decrypting read must
    // answer under it.
    let rows: Vec<(&str, Value, Expected)> = vec![
        (
            "StringEquals naming this service's endpoint",
            json!({"StringEquals": {VIA_SERVICE_CONDITION: format!("ssm.{REGION}.amazonaws.com")}}),
            Expected::Served,
        ),
        (
            "StringLike naming this service in any Region",
            json!({"StringLike": {VIA_SERVICE_CONDITION: "ssm.*.amazonaws.com"}}),
            Expected::Served,
        ),
        (
            "StringEquals naming another service's endpoint",
            json!({"StringEquals": {VIA_SERVICE_CONDITION: format!("s3.{REGION}.amazonaws.com")}}),
            Expected::Refused,
        ),
    ];

    for (case, clause, expected) in rows {
        let router = router();
        let document = policy(vec![allow_caller(json!(DECRYPT_ACTION), Some(clause))]);
        let (key_id, _arn) = key_with_policy(&router, case, &document).await;
        put_secure(&router, case, IN_SCOPE, &key_id).await;
        read_parameter(&router, case, IN_SCOPE, Some(true), expected).await;
    }
}

/// The refusal is the sentence KMS words for an authorization failure its own key policy
/// withheld — Parameter Store relays it rather than wording a refusal of its own — and it
/// names the action, the key and the principal the read was refused for.
#[tokio::test]
async fn the_refusal_carries_the_sentence_kms_words() {
    let case = "a statement scoped to the in-scope parameter";
    let router = router();
    let document = policy(vec![allow_caller(
        json!(DECRYPT_ACTION),
        Some(json!({"StringEquals": {PARAMETER_ARN_CONDITION: parameter_arn(IN_SCOPE)}})),
    )]);
    let (key_id, key_arn) = key_with_policy(&router, case, &document).await;
    put_secure(&router, case, OUT_OF_SCOPE, &key_id).await;

    let response = read_parameter(
        &router,
        case,
        OUT_OF_SCOPE,
        Some(true),
        Expected::Refused,
    )
    .await;
    let message = response["Message"]
        .as_str()
        .or_else(|| response["message"].as_str())
        .unwrap_or_default();
    assert_eq!(
        message,
        kms_state::authorization::no_resource_policy_message(&caller(), DECRYPT_ACTION, &key_arn),
        "case {case}: the refusal carries KMS's own sentence, got {response}"
    );
}
