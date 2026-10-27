// SPDX-License-Identifier: CC0-1.0

//! Confining a KMS key to one service's path to it with a `kms:ViaService` key-policy
//! condition, driven over the wire through the shared [`super::harness`].
//!
//! The rule under test: a key policy statement grants an action only when every clause of
//! its `Condition` holds for the request being authorized. A clause the service cannot
//! decide — one over a condition key the request does not model, which `kms:ViaService` is
//! here, since it has a value only for a request another AWS service made on the caller's
//! behalf — leaves the statement undecided, and an undecided statement is no grant. So a
//! key whose only grant of an action is gated on that condition — the canonical
//! confinement of a key to one service — cannot be used for that action by a caller
//! calling KMS directly: the request is refused with HTTP 400 `AccessDeniedException`,
//! whose message is AWS's denial sentence naming the caller, the IAM action and the key,
//! and saying that no resource-based policy allows that action. A statement that grants
//! the same action without that gate still grants it, so the refusal is caused by the
//! condition rather than by the policy having been replaced.
//!
//! Every assertion is made over a TABLE. The rows differ in the operation that is judged
//! (each implemented cryptographic operation, under its own IAM action — `ReEncrypt` under
//! one action per half), in the condition operator (equality, several values, a wildcard
//! comparison, a set qualifier), in the service the policy confines the key to, in the
//! action patterns the statement names, and in whether the policy also carries an
//! unconditioned grant of the action.
//!
//! The sibling suite `key_policy_conditions.rs` drives the same condition machinery over
//! the keys that name the signing PRINCIPAL; this one is about the key that names the
//! service a request was made through.

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{assert_empty, b64, create_key, ok, refused, router, text, REGION};

/// The regional endpoint name of an AWS service, which is the value `kms:ViaService`
/// carries: the service's regional endpoint, `<service>.<region>.amazonaws.com`.
fn endpoint(service: &str) -> String {
    format!("{service}.{REGION}.amazonaws.com")
}

/// The principal every policy below grants to: the identity this emulator authenticates
/// every request as, which is also the identity a refusal names.
fn caller() -> String {
    state::caller_identity_arn()
}

/// A key policy carrying exactly the statements given.
fn policy(statements: Vec<Value>) -> String {
    json!({"Version": "2012-10-17", "Id": "key-under-test", "Statement": statements}).to_string()
}

/// One `Allow` statement for [`caller`] over every resource, carrying `actions` and the
/// `Condition` given (none when it is [`Value::Null`]).
fn allow(actions: Value, condition: Value) -> Value {
    let mut statement = json!({
        "Sid": "StatementUnderTest",
        "Effect": "Allow",
        "Principal": {"AWS": caller()},
        "Action": actions,
        "Resource": "*",
    });
    if !condition.is_null() {
        statement["Condition"] = condition;
    }
    statement
}

/// Attach `document` to `key_id`, requiring the empty 200 `PutKeyPolicy` answers.
async fn put_policy(router: &MultiServiceRouter, case: &str, key_id: &str, document: &str) {
    let answer = ok(
        router,
        "PutKeyPolicy",
        &json!({"KeyId": key_id, "Policy": document}),
    )
    .await;
    assert_empty(case, "PutKeyPolicy", &answer);
}

/// Assert `operation` on the key whose ARN is `key_arn` is refused the way an
/// authorization failure a key policy withheld is refused: HTTP 400,
/// `AccessDeniedException`, and AWS's denial sentence — fixed text whose variable parts
/// are the caller it refused, the IAM action it asked for and the key it asked for it on.
async fn assert_denied(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
    action: &str,
    key_arn: &str,
) {
    let (status, code, message) = refused(router, REGION, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} with {code}"
    );
    assert_eq!(
        code, "AccessDeniedException",
        "case {case}: expected AccessDeniedException, got {code}"
    );
    let expected = format!(
        "User: {} is not authorized to perform: {action} on resource: {key_arn} because no \
         resource-based policy allows the {action} action",
        caller()
    );
    assert_eq!(
        message, expected,
        "case {case}: expected AWS's denial sentence, got {message:?}"
    );
}

/// A symmetric encryption key, as its id and its ARN.
async fn symmetric_key(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let metadata = create_key(router, &json!({"Description": case})).await;
    (
        text(case, &metadata, "KeyId"),
        text(case, &metadata, "Arn"),
    )
}

/// A ciphertext sealed under `key_id` while its policy still permits encryption, which is
/// what a row that judges `Decrypt` or `ReEncrypt` presents afterwards.
async fn sealed(router: &MultiServiceRouter, case: &str, key_id: &str) -> String {
    let answer = ok(
        router,
        "Encrypt",
        &json!({"KeyId": key_id, "Plaintext": b64(b"a payload to unwrap")}),
    )
    .await;
    text(case, &answer, "CiphertextBlob")
}

/// One operation judged by the key policy: how a request for it is built, and the IAM
/// action the refusal must name.
struct OperationCase {
    case: &'static str,
    operation: &'static str,
    /// The IAM action the operation is authorized under, which for `ReEncrypt` is the
    /// action of its SOURCE half rather than the operation's own name.
    action: &'static str,
    /// Whether the request carries a ciphertext this key already sealed.
    needs_ciphertext: bool,
}

/// Every implemented cryptographic operation that uses a symmetric encryption key's
/// material, each under its own IAM action.
const OPERATION_CASES: &[OperationCase] = &[
    OperationCase {
        case: "GenerateDataKey",
        operation: "GenerateDataKey",
        action: "kms:GenerateDataKey",
        needs_ciphertext: false,
    },
    OperationCase {
        case: "GenerateDataKeyWithoutPlaintext",
        operation: "GenerateDataKeyWithoutPlaintext",
        action: "kms:GenerateDataKeyWithoutPlaintext",
        needs_ciphertext: false,
    },
    OperationCase {
        case: "GenerateDataKeyPairWithoutPlaintext",
        operation: "GenerateDataKeyPairWithoutPlaintext",
        action: "kms:GenerateDataKeyPairWithoutPlaintext",
        needs_ciphertext: false,
    },
    OperationCase {
        case: "Encrypt",
        operation: "Encrypt",
        action: "kms:Encrypt",
        needs_ciphertext: false,
    },
    OperationCase {
        case: "Decrypt",
        operation: "Decrypt",
        action: "kms:Decrypt",
        needs_ciphertext: true,
    },
    OperationCase {
        case: "ReEncrypt, judged on its source half",
        operation: "ReEncrypt",
        action: "kms:ReEncryptFrom",
        needs_ciphertext: true,
    },
];

/// The request body `row` sends against `key_id`, with `ciphertext` for the rows that
/// present one.
fn request_for(row: &OperationCase, key_id: &str, ciphertext: &str) -> Value {
    match row.operation {
        "GenerateDataKey" | "GenerateDataKeyWithoutPlaintext" => {
            json!({"KeyId": key_id, "KeySpec": "AES_256"})
        }
        "GenerateDataKeyPairWithoutPlaintext" => {
            json!({"KeyId": key_id, "KeyPairSpec": "ECC_NIST_P256"})
        }
        "Encrypt" => json!({"KeyId": key_id, "Plaintext": b64(b"a payload to seal")}),
        "Decrypt" => json!({"CiphertextBlob": ciphertext}),
        "ReEncrypt" => json!({"CiphertextBlob": ciphertext, "DestinationKeyId": key_id}),
        other => panic!(
            "case {}: {other} is not an operation this suite drives",
            row.case
        ),
    }
}

#[tokio::test]
async fn a_via_service_condition_refuses_every_cryptographic_operation_called_directly() {
    let router = router();
    // One confined policy for every row: the key may be used in cryptographic operations
    // only through one service's path to it.
    let confined = policy(vec![
        // The admin statement the canonical pattern carries, so the key stays manageable:
        // it covers no cryptographic action, so it grants none either.
        allow(
            json!(["kms:DescribeKey", "kms:GetKeyPolicy", "kms:PutKeyPolicy"]),
            Value::Null,
        ),
        allow(
            json!([
                "kms:Encrypt",
                "kms:Decrypt",
                "kms:ReEncrypt*",
                "kms:GenerateDataKey*",
                "kms:GetPublicKey"
            ]),
            json!({"StringEquals": {"kms:ViaService": endpoint("s3")}}),
        ),
    ]);

    for row in OPERATION_CASES {
        let case = row.case;
        let (key_id, key_arn) = symmetric_key(&router, case).await;
        let ciphertext = if row.needs_ciphertext {
            sealed(&router, case, &key_id).await
        } else {
            String::new()
        };
        let request = request_for(row, &key_id, &ciphertext);

        // Under the key's default policy — which grants the account every action
        // unconditionally — the same request succeeds, so the row measures the condition
        // and nothing else.
        ok(&router, row.operation, &request).await;

        put_policy(&router, case, &key_id, &confined).await;
        assert_denied(&router, case, row.operation, &request, row.action, &key_arn).await;
    }
}

/// One policy a row judges `GenerateDataKey` against, and whether the key policy lets the
/// call through.
struct PolicyCase {
    case: &'static str,
    statements: fn() -> Vec<Value>,
    permitted: bool,
}

/// The policy shapes the rule is asserted over: the ways a confinement condition is
/// written and the service it names, beside the unconditioned grants that are not gated at
/// all.
const POLICY_CASES: &[PolicyCase] = &[
    PolicyCase {
        case: "an unconditioned grant of every action",
        statements: || vec![allow(json!("kms:*"), Value::Null)],
        permitted: true,
    },
    PolicyCase {
        case: "an unconditioned grant naming the action exactly",
        statements: || vec![allow(json!("kms:GenerateDataKey"), Value::Null)],
        permitted: true,
    },
    PolicyCase {
        case: "StringEquals confining the key to S3",
        statements: || {
            vec![allow(
                json!("kms:*"),
                json!({"StringEquals": {"kms:ViaService": endpoint("s3")}}),
            )]
        },
        permitted: false,
    },
    PolicyCase {
        case: "StringEquals confining the key to another service",
        statements: || {
            vec![allow(
                json!("kms:GenerateDataKey*"),
                json!({"StringEquals": {"kms:ViaService": endpoint("dynamodb")}}),
            )]
        },
        permitted: false,
    },
    PolicyCase {
        case: "StringEquals naming several services",
        statements: || {
            vec![allow(
                json!("kms:*"),
                json!({"StringEquals": {"kms:ViaService": [endpoint("s3"), endpoint("lambda")]}}),
            )]
        },
        permitted: false,
    },
    PolicyCase {
        case: "StringLike with a wildcard over the Region",
        statements: || {
            vec![allow(
                json!("kms:*"),
                json!({"StringLike": {"kms:ViaService": "s3.*.amazonaws.com"}}),
            )]
        },
        permitted: false,
    },
    PolicyCase {
        case: "a set qualifier over the same comparison",
        statements: || {
            vec![allow(
                json!("kms:*"),
                json!({"ForAnyValue:StringEquals": {"kms:ViaService": endpoint("s3")}}),
            )]
        },
        permitted: false,
    },
    PolicyCase {
        case: "a confined statement beside an unconditioned grant of the same action",
        statements: || {
            vec![
                allow(
                    json!("kms:*"),
                    json!({"StringEquals": {"kms:ViaService": endpoint("s3")}}),
                ),
                allow(json!("kms:GenerateDataKey"), Value::Null),
            ]
        },
        permitted: true,
    },
    PolicyCase {
        case: "a confined statement beside an unconditioned grant of another action",
        statements: || {
            vec![
                allow(
                    json!("kms:*"),
                    json!({"StringEquals": {"kms:ViaService": endpoint("s3")}}),
                ),
                allow(json!("kms:Decrypt"), Value::Null),
            ]
        },
        permitted: false,
    },
];

#[tokio::test]
async fn the_confinement_holds_however_the_condition_is_written() {
    let router = router();
    for row in POLICY_CASES {
        let case = row.case;
        let (key_id, key_arn) = symmetric_key(&router, case).await;
        let document = policy((row.statements)());
        put_policy(&router, case, &key_id, &document).await;

        let request = json!({"KeyId": key_id, "KeySpec": "AES_256"});
        if row.permitted {
            let answer = ok(&router, "GenerateDataKey", &request).await;
            assert!(
                answer["CiphertextBlob"].is_string(),
                "case {case}: expected a data key for {document}, got {answer}"
            );
        } else {
            assert_denied(
                &router,
                case,
                "GenerateDataKey",
                &request,
                "kms:GenerateDataKey",
                &key_arn,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn re_encrypt_is_judged_on_each_half_under_that_halfs_own_action() {
    let router = router();
    // Each half of a `ReEncrypt` is authorized under its own action, so a policy confining
    // ONE of the two keys refuses the call naming that half's action and that key.
    let rows: [(&str, bool, &str); 2] = [
        ("the source key is confined", true, "kms:ReEncryptFrom"),
        ("the destination key is confined", false, "kms:ReEncryptTo"),
    ];
    for (case, confine_source, action) in rows {
        let (source_id, source_arn) = symmetric_key(&router, case).await;
        let (destination_id, destination_arn) = symmetric_key(&router, case).await;
        let ciphertext = sealed(&router, case, &source_id).await;
        let request = json!({
            "CiphertextBlob": ciphertext,
            "DestinationKeyId": destination_id.clone(),
        });

        // Both keys still carry their unconditioned default policy, so the call goes
        // through and the row measures the condition and nothing else.
        ok(&router, "ReEncrypt", &request).await;

        let (confined_id, confined_arn) = if confine_source {
            (source_id, source_arn)
        } else {
            (destination_id, destination_arn)
        };
        put_policy(
            &router,
            case,
            &confined_id,
            &policy(vec![allow(
                json!("kms:ReEncrypt*"),
                json!({"StringEquals": {"kms:ViaService": endpoint("s3")}}),
            )]),
        )
        .await;

        assert_denied(&router, case, "ReEncrypt", &request, action, &confined_arn).await;
    }
}

#[tokio::test]
async fn a_via_service_condition_refuses_get_public_key_on_an_asymmetric_key() {
    let case = "GetPublicKey on a key confined to one service";
    let router = router();
    let metadata = create_key(
        &router,
        &json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY", "Description": case}),
    )
    .await;
    let key_id = text(case, &metadata, "KeyId");
    let key_arn = text(case, &metadata, "Arn");
    let request = json!({"KeyId": key_id});

    ok(&router, "GetPublicKey", &request).await;
    put_policy(
        &router,
        case,
        &key_id,
        &policy(vec![allow(
            json!(["kms:GetPublicKey", "kms:Sign", "kms:Verify"]),
            json!({"StringEquals": {"kms:ViaService": endpoint("acm")}}),
        )]),
    )
    .await;

    assert_denied(
        &router,
        case,
        "GetPublicKey",
        &request,
        "kms:GetPublicKey",
        &key_arn,
    )
    .await;
}
