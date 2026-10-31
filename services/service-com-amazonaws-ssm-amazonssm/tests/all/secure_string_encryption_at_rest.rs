// SPDX-License-Identifier: CC0-1.0

//! A `SecureString` parameter at rest: what the store holds for it, and what each of the
//! two read forms reports.
//!
//! `PutParameter` encrypts a `SecureString`'s value with the KMS key `KeyId` names before
//! storing it, so the parameter has two reportable forms and `WithDecryption` chooses
//! between them: "Return decrypted values for secure string parameters. This flag is
//! ignored for `String` and `StringList` parameter types"
//! (`smithy_query(query="doc",shape="GetParameterRequest",member="WithDecryption")`).
//! The undecrypted read therefore reports the KMS ciphertext, base64-encoded, and the
//! decrypted one reports the plaintext.
//!
//! What makes the undecrypted form a REAL ciphertext rather than an opaque-looking string
//! is asked of KMS itself: the bytes the read reports are handed to `kms:Decrypt`, which
//! recovers exactly the value the put wrote and reports the key's ARN — and only under
//! the encryption context that binds the ciphertext to the one parameter it belongs to
//! (`PARAMETER_ARN` naming that parameter's ARN), so the same blob under no context, or
//! under another parameter's, is refused.
//!
//! KMS is mounted BESIDE Systems Manager on one engine, so every key and alias a case
//! uses is one KMS's own operations created. Nothing here reaches into a store.
//!
//! The tables span what the rule generalises over rather than the one recorded parameter:
//!
//! * the value sealed — its size (one byte up to the published standard-tier content
//!   limit), its bytes (ASCII, UTF-8 beyond ASCII, a comma-separated list, text that is
//!   itself base64) and whether another parameter holds the same one;
//! * the form the put names the key in (key id, key ARN, alias, and no `KeyId` at all,
//!   which falls back to the Amazon Web Services managed key of this service);
//! * the read that reports the value (`GetParameter`, `GetParameterHistory`,
//!   `GetParametersByPath`), each in both of its forms;
//! * the parameter type (the two the flag is ignored for are stored as written);
//! * the version (every version of one parameter is sealed on its own).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::parameter_store::{
    call, error_code, kms_key_with_alias, router_with_kms, ACCOUNT, REGION,
};

/// The encryption-context entry Parameter Store binds a `SecureString`'s ciphertext to.
const PARAMETER_ARN_CONTEXT_KEY: &str = "PARAMETER_ARN";

/// The alias a case naming its key by alias points at it with.
const SUITE_ALIAS: &str = "alias/encryption-at-rest-suite";

/// The published content-size limit of a standard parameter ("Standard parameters have a
/// content size limit of 4 KB", `PutParameterRequest$Tier`), which is the largest value
/// the puts below seal.
const STANDARD_LIMIT: usize = 4 * 1024;

/// One KMS call, returning `(status, body)`. KMS is an awsJson1_1 service reached by
/// `X-Amz-Target: TrentService.<Operation>`.
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
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid test request");
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
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// `PutParameter`, which must succeed.
async fn put(router: &MultiServiceRouter, case: &str, body: &Value) {
    let (status, response) = call(router, "PutParameter", body).await;
    assert_eq!(
        status, 200,
        "case {case}: PutParameter {body} must succeed, got {status} with {response}"
    );
}

/// The `Parameter` a `GetParameter` of `name` reports, with `WithDecryption` of
/// `with_decryption` when it is given.
async fn read(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    with_decryption: Option<bool>,
) -> Value {
    let mut body = json!({ "Name": name });
    if let Some(flag) = with_decryption {
        body["WithDecryption"] = json!(flag);
    }
    let (status, response) = call(router, "GetParameter", &body).await;
    assert_eq!(
        status, 200,
        "case {case}: GetParameter {body} must succeed, got {status} with {response}"
    );
    response["Parameter"].clone()
}

/// The `Value` a reported parameter carries.
fn reported_value(case: &str, parameter: &Value) -> String {
    parameter["Value"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the read reports a Value, got {parameter}"))
        .to_string()
}

/// The ARN Parameter Store mints for `name` in [`REGION`] — the value of the
/// `PARAMETER_ARN` encryption-context entry its ciphertext is bound to.
fn parameter_arn(name: &str) -> String {
    format!(
        "arn:aws:ssm:{REGION}:{ACCOUNT}:parameter/{}",
        name.strip_prefix('/').unwrap_or(name)
    )
}

/// Ask KMS to decrypt `ciphertext` (the base64 blob a read reported) under `context`,
/// returning `(status, body)`.
///
/// The `CiphertextBlob` member is a blob, so the wire carries it base64-encoded — which
/// is exactly the form the parameter read reported it in.
async fn decrypt(
    router: &MultiServiceRouter,
    ciphertext: &str,
    context: Option<Value>,
) -> (u16, Value) {
    let mut body = json!({ "CiphertextBlob": ciphertext });
    if let Some(context) = context {
        body["EncryptionContext"] = context;
    }
    kms(router, "Decrypt", &body).await
}

/// The encryption context a value of the parameter `name` must be decrypted under.
fn context_of(name: &str) -> Value {
    json!({ PARAMETER_ARN_CONTEXT_KEY: parameter_arn(name) })
}

/// One row of [`a_secure_string_is_stored_as_a_kms_ciphertext_of_its_value`]: the case,
/// the parameter name, and the value its put seals.
type ValueRow = (&'static str, &'static str, String);

/// The rows every value case below seals: the size range a parameter value spans and the
/// bytes it may carry, so the rule is asserted over a class of values rather than the one
/// the recorded unit wrote.
fn value_rows() -> Vec<ValueRow> {
    vec![
        ("one byte", "/sealed/tiny", "x".to_string()),
        (
            "an ordinary secret",
            "/sealed/password",
            "s3cr3t-value".to_string(),
        ),
        (
            "bytes beyond ASCII",
            "/sealed/unicode",
            "sécret-ключ-🔐".to_string(),
        ),
        (
            "a comma-separated value",
            "/sealed/commas",
            "alpha,beta,gamma".to_string(),
        ),
        (
            "a value that is itself base64",
            "/sealed/base64",
            "c2VjcmV0LWJ5dGVz".to_string(),
        ),
        (
            "the published standard-tier content limit",
            "/sealed/limit",
            "v".repeat(STANDARD_LIMIT),
        ),
    ]
}

/// A `SecureString`'s stored value is the KMS ciphertext of the value its put wrote: the
/// undecrypted read reports base64 that is not the value, KMS opens exactly that value
/// out of it under the parameter's own encryption context and reports the key it is
/// sealed under, and the decrypting read reports the value itself.
#[tokio::test]
async fn a_secure_string_is_stored_as_a_kms_ciphertext_of_its_value() {
    let router = router_with_kms();
    let key_id = kms_key_with_alias(&router, SUITE_ALIAS).await;
    let (status, described) = kms(&router, "DescribeKey", &json!({ "KeyId": key_id })).await;
    assert_eq!(
        status, 200,
        "DescribeKey must report the suite's key, got {status} with {described}"
    );
    let key_arn = described["KeyMetadata"]["Arn"]
        .as_str()
        .expect("DescribeKey reports the key ARN")
        .to_string();

    for (case, name, value) in value_rows() {
        put(
            &router,
            case,
            &json!({
                "Name": name,
                "Type": "SecureString",
                "Value": value,
                "KeyId": key_id,
            }),
        )
        .await;

        // The read that asks for no decryption, in both spellings of asking for none.
        for flag in [None, Some(false)] {
            let case = format!("{case} with WithDecryption {flag:?}");
            let stored = reported_value(&case, &read(&router, &case, name, flag).await);
            assert_ne!(
                stored, value,
                "case {case}: the undecrypted read must not report the value, got {stored:?}"
            );
            let blob = ::aws_smithy_types::base64::decode(&stored).unwrap_or_else(|error| {
                panic!("case {case}: the undecrypted read reports base64: {error}")
            });
            assert!(
                !blob.is_empty() && blob != value.as_bytes(),
                "case {case}: the reported bytes are a ciphertext, not the value, got \
                 {stored:?}"
            );

            // KMS opens exactly the value out of those bytes, under the encryption
            // context naming this parameter, and reports the key they are sealed under.
            let (status, opened) =
                decrypt(&router, &stored, Some(context_of(name))).await;
            assert_eq!(
                status, 200,
                "case {case}: kms:Decrypt of the reported bytes must succeed, got {status} \
                 with {opened}"
            );
            assert_eq!(
                opened["Plaintext"].as_str(),
                Some(::aws_smithy_types::base64::encode(&value).as_str()),
                "case {case}: kms:Decrypt must recover the value the put wrote, got {opened}"
            );
            assert_eq!(
                opened["KeyId"].as_str(),
                Some(key_arn.as_str()),
                "case {case}: kms:Decrypt must report the key the value is sealed under, \
                 got {opened}"
            );
        }

        // The read that asks for decryption reports the value itself.
        let case = format!("{case} with WithDecryption Some(true)");
        let decrypted = reported_value(&case, &read(&router, &case, name, Some(true)).await);
        assert_eq!(
            decrypted, value,
            "case {case}: the decrypting read must report the value the put wrote, got \
             {decrypted:?}"
        );
    }
}

/// The ciphertext is bound to the ONE parameter it belongs to: `kms:Decrypt` of it under
/// no encryption context, under another parameter's, or under a differently named entry
/// carrying the right ARN, is refused — so the bytes an undecrypted read hands out open
/// only as that parameter's value.
#[tokio::test]
async fn the_ciphertext_opens_only_under_the_parameters_own_encryption_context() {
    let router = router_with_kms();
    let key_id = kms_key_with_alias(&router, SUITE_ALIAS).await;
    let mine = "/bound/mine";
    let other = "/bound/other";
    for name in [mine, other] {
        put(
            &router,
            name,
            &json!({
                "Name": name,
                "Type": "SecureString",
                "Value": format!("value of {name}"),
                "KeyId": key_id,
            }),
        )
        .await;
    }
    let stored = reported_value("bound", &read(&router, "bound", mine, Some(false)).await);

    // Each row: the case, and the encryption context to present (`None` for none at all).
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("no encryption context at all", None),
        ("an empty encryption context", Some(json!({}))),
        ("another parameter's ARN", Some(context_of(other))),
        (
            "the right ARN under another entry name",
            Some(json!({ "PARAMETER": parameter_arn(mine) })),
        ),
        (
            "the parameter's own entry plus one more",
            Some(json!({
                PARAMETER_ARN_CONTEXT_KEY: parameter_arn(mine),
                "EXTRA": "entry",
            })),
        ),
    ];

    for (case, context) in rows {
        let (status, refused) = decrypt(&router, &stored, context.clone()).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidCiphertextException"),
            "case {case}: kms:Decrypt under {context:?} must be refused, got {status} with \
             {refused}"
        );
    }

    // The one context it does open under, so the refusals above are about the context and
    // not about the bytes.
    let (status, opened) = decrypt(&router, &stored, Some(context_of(mine))).await;
    assert_eq!(
        status, 200,
        "the parameter's own encryption context must open it, got {status} with {opened}"
    );
}

/// How a put names the key its value is sealed under does not change the rule: whichever
/// form KMS resolves, the undecrypted read reports a ciphertext and the decrypting read
/// reports the value — including the put that names no key at all, which falls back to
/// the Amazon Web Services managed key of this service.
#[tokio::test]
async fn every_form_of_naming_the_key_seals_the_value() {
    let router = router_with_kms();
    let key_id = kms_key_with_alias(&router, SUITE_ALIAS).await;
    let (status, described) = kms(&router, "DescribeKey", &json!({ "KeyId": key_id })).await;
    assert_eq!(
        status, 200,
        "DescribeKey must report the suite's key, got {status} with {described}"
    );
    let key_arn = described["KeyMetadata"]["Arn"]
        .as_str()
        .expect("DescribeKey reports the key ARN")
        .to_string();

    // Each row: the case, the parameter name, and the `KeyId` the put names (`None` for a
    // put that names none).
    let rows: Vec<(&str, &str, Option<String>)> = vec![
        ("the key id", "/forms/by-id", Some(key_id.clone())),
        ("the key ARN", "/forms/by-arn", Some(key_arn)),
        ("an alias of the key", "/forms/by-alias", Some(SUITE_ALIAS.to_string())),
        ("no KeyId at all", "/forms/default", None),
    ];

    for (case, name, named) in rows {
        let value = format!("value of {name}");
        let mut body = json!({ "Name": name, "Type": "SecureString", "Value": value });
        if let Some(named) = &named {
            body["KeyId"] = json!(named);
        }
        put(&router, case, &body).await;

        let stored = reported_value(case, &read(&router, case, name, None).await);
        assert_ne!(
            stored, value,
            "case {case}: the undecrypted read must report a ciphertext, got {stored:?}"
        );
        let (status, opened) = decrypt(&router, &stored, Some(context_of(name))).await;
        assert_eq!(
            status, 200,
            "case {case}: kms:Decrypt of the reported bytes must succeed, got {status} with \
             {opened}"
        );
        assert_eq!(
            opened["Plaintext"].as_str(),
            Some(::aws_smithy_types::base64::encode(&value).as_str()),
            "case {case}: kms:Decrypt must recover the value the put wrote, got {opened}"
        );
        assert_eq!(
            reported_value(case, &read(&router, case, name, Some(true)).await),
            value,
            "case {case}: the decrypting read must report the value the put wrote"
        );
    }
}

/// Each sealing is its own: two parameters written with the SAME value under the same key,
/// and two versions of one parameter, carry different ciphertexts, and each one opens to
/// the value its own put wrote.
#[tokio::test]
async fn every_value_is_sealed_on_its_own() {
    let router = router_with_kms();
    let key_id = kms_key_with_alias(&router, SUITE_ALIAS).await;
    let shared = "the same secret";

    let mut ciphertexts: Vec<String> = Vec::new();
    for name in ["/each/first", "/each/second"] {
        put(
            &router,
            name,
            &json!({
                "Name": name,
                "Type": "SecureString",
                "Value": shared,
                "KeyId": key_id,
            }),
        )
        .await;
        let stored = reported_value(name, &read(&router, name, name, Some(false)).await);
        let (status, opened) = decrypt(&router, &stored, Some(context_of(name))).await;
        assert_eq!(
            status, 200,
            "case {name}: kms:Decrypt must open the stored ciphertext, got {status} with \
             {opened}"
        );
        assert_eq!(
            opened["Plaintext"].as_str(),
            Some(::aws_smithy_types::base64::encode(shared).as_str()),
            "case {name}: each parameter opens to its own value, got {opened}"
        );
        ciphertexts.push(stored);
    }
    assert_ne!(
        ciphertexts[0], ciphertexts[1],
        "two parameters holding the same value must not share a ciphertext, got \
         {ciphertexts:?}"
    );

    // A second version of one parameter is sealed on its own too, and the history reports
    // each version in whichever form the read asks for.
    let name = "/each/versions";
    let values = ["first value", "second value"];
    for (index, value) in values.iter().enumerate() {
        let mut body = json!({
            "Name": name,
            "Type": "SecureString",
            "Value": value,
            "KeyId": key_id,
        });
        if index > 0 {
            body["Overwrite"] = json!(true);
        }
        put(&router, name, &body).await;
    }

    for (flag, decrypted) in [(Some(false), false), (Some(true), true)] {
        let case = format!("history with WithDecryption {flag:?}");
        let (status, history) = call(
            &router,
            "GetParameterHistory",
            &json!({ "Name": name, "WithDecryption": flag }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: GetParameterHistory must succeed, got {status} with {history}"
        );
        let entries = history["Parameters"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: the history reports a list, got {history}"))
            .clone();
        assert_eq!(
            entries.len(),
            values.len(),
            "case {case}: the history reports every version, got {history}"
        );
        let mut reported: Vec<String> = Vec::new();
        for (entry, value) in entries.iter().zip(values) {
            let held = reported_value(&case, entry);
            if decrypted {
                assert_eq!(
                    held, value,
                    "case {case}: each version opens to the value its own put wrote, got \
                     {entry}"
                );
            } else {
                assert_ne!(
                    held, value,
                    "case {case}: an undecrypted history reports each version's ciphertext, \
                     got {entry}"
                );
                let (status, opened) = decrypt(&router, &held, Some(context_of(name))).await;
                assert_eq!(
                    status, 200,
                    "case {case}: kms:Decrypt must open each version, got {status} with \
                     {opened}"
                );
                assert_eq!(
                    opened["Plaintext"].as_str(),
                    Some(::aws_smithy_types::base64::encode(value).as_str()),
                    "case {case}: each version's ciphertext opens to its own value, got \
                     {opened}"
                );
            }
            reported.push(held);
        }
        if !decrypted {
            assert_ne!(
                reported[0], reported[1],
                "case {case}: two versions must not share a ciphertext, got {reported:?}"
            );
        }
    }
}

/// Only an encrypted type has two forms. "This flag is ignored for `String` and
/// `StringList` parameter types", so their values are stored as the put wrote them and
/// every read of them reports that value — through each of the three readers, which all
/// report the same form of the same value.
#[tokio::test]
async fn the_types_the_flag_is_ignored_for_are_stored_as_written() {
    let router = router_with_kms();
    let key_id = kms_key_with_alias(&router, SUITE_ALIAS).await;

    // Each row: the `ParameterType`, the value, and whether the type is encrypted.
    let rows: Vec<(&str, &str, bool)> = vec![
        ("String", "plain-value", false),
        ("StringList", "alpha,beta,gamma", false),
        ("SecureString", "sealed-value", true),
    ];

    for (type_, value, encrypted) in rows {
        let name = format!("/forms/{type_}");
        let mut body = json!({ "Name": name, "Type": type_, "Value": value });
        if encrypted {
            body["KeyId"] = json!(key_id);
        }
        put(&router, type_, &body).await;

        for flag in [None, Some(false), Some(true)] {
            let case = format!("{type_} with WithDecryption {flag:?}");
            // The hierarchy read and the history read report the same value as the
            // single-parameter read, because all three report one stored version.
            let single = reported_value(&case, &read(&router, &case, &name, flag).await);
            let mut listing = json!({ "Path": "/forms" });
            if let Some(flag) = flag {
                listing["WithDecryption"] = json!(flag);
            }
            let (status, response) = call(&router, "GetParametersByPath", &listing).await;
            assert_eq!(
                status, 200,
                "case {case}: GetParametersByPath must succeed, got {status} with {response}"
            );
            let reported = response["Parameters"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: the read reports a list, got {response}"))
                .iter()
                .find(|parameter| parameter["Name"] == json!(name))
                .unwrap_or_else(|| panic!("case {case}: the read reports {name}, got {response}"))
                .clone();
            assert_eq!(
                reported_value(&case, &reported),
                single,
                "case {case}: the hierarchy read reports the form the single read does, got \
                 {reported}"
            );

            if encrypted && flag != Some(true) {
                assert_ne!(
                    single, value,
                    "case {case}: an encrypted value is reported as its ciphertext"
                );
            } else {
                assert_eq!(
                    single, value,
                    "case {case}: the read reports the value the put wrote"
                );
            }
        }
    }
}
