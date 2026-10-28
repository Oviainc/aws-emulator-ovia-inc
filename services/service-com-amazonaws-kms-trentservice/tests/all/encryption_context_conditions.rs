// SPDX-License-Identifier: CC0-1.0

//! A key-policy statement conditioned on the request's ENCRYPTION CONTEXT, and which
//! document a refusal that no authority allowed names — both driven over the awsJson1_1
//! wire through the shared [`super::harness`].
//!
//! The rules under test:
//!
//! 1. A policy statement contributes its effect to a request only when every clause of its
//!    `Condition` block holds for THAT request, whichever effect it states — so a statement
//!    conditioned on an entry of the request's encryption context governs exactly the
//!    requests whose context satisfies it, and every other request is decided by the
//!    document's remaining statements. `kms:EncryptionContext:<entry>` is compared against
//!    the entry of that name in the request's `EncryptionContext`, and
//!    `kms:EncryptionContextKeys` against the set of entry names it carries; a request that
//!    carries no value for the entry a clause names is one the entry is ABSENT from, which
//!    is what IAM's negated and existence operators ask about.
//! 2. When no authority allows a key-scoped action, the refusal (HTTP 400
//!    `AccessDeniedException`) names the document that has to change: the key's own
//!    resource-based policy, unless that policy DELEGATES the action to the account's
//!    identity-based policies, which are then the permission the caller is missing. One
//!    sentence per class, whichever path reached it — including the path a request that
//!    presents a grant token takes.
//!
//! The rows differ along what the rules generalise over: the condition operator
//! (`StringEquals`, `StringNotEquals`, `StringLike`, `Null`, and the two set qualifiers),
//! the form of a clause's value (one string and a list of them), how many entries the
//! clause names, the effect the conditioned statement states (`Deny` and `Allow`), the
//! entries the request carries (the named one, another value for it, a second entry beside
//! it, and none at all), the operation the statement names (`Decrypt`, beside the
//! `Encrypt`/`GenerateDataKey` the statement leaves alone), whether the key policy
//! delegates to the account, and whether the request presents a grant token.
//!
//! The sibling suites drive the same condition machinery over the keys that name the
//! signing principal (`key_policy_conditions.rs`) and over a key the request models no
//! value for at all (`via_service_confinement.rs`).

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    assert_empty, b64, create_key, ok, principal, refused, router, text, unb64, REGION,
};

/// The plaintext every case below seals and opens again.
const PAYLOAD: &[u8] = b"tenant-scoped row";

/// What the gate must answer one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// HTTP 200: a statement that applies allows the request.
    Served,
    /// A statement that applies states `Deny`.
    ExplicitDeny,
    /// No statement that applies allows it, on a key whose policy does not delegate the
    /// action to the account's identity policies.
    NoResourcePolicy,
    /// No statement that applies allows it, on a key whose policy DOES delegate it.
    NoIdentityPolicy,
}

/// One row of the tables below: the case, the `Condition` block the conditioned statement
/// carries, and what the gate must answer a `Decrypt` carrying each encryption context —
/// [`Value::Null`] standing for a request that carries no `EncryptionContext` member.
type ConditionRow = (&'static str, Value, Vec<(Value, Expected)>);

/// A key policy carrying exactly the statements given.
fn policy(statements: Vec<Value>) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// The statement a key carries by default: "the default key policy … gives the Amazon Web
/// Services account that owns the KMS key permission to use it in all cryptographic
/// operations" (`smithy_query(query="doc",shape="CreateKeyRequest",member="Policy")`), which
/// names the account root and so DELEGATES the decision to the account's identity policies.
fn account_root_statement() -> Value {
    json!({
        "Sid": "DelegateToAccount",
        "Effect": "Allow",
        "Principal": {"AWS": format!("arn:aws:iam::{}:root", engine::emulator_account_id())},
        "Action": "kms:*",
        "Resource": "*",
    })
}

/// A statement stating `effect` on `actions` for every principal, gated by `condition` —
/// the shape a client scopes a key to one encryption context with.
fn conditioned(effect: &str, actions: Value, condition: Value) -> Value {
    json!({
        "Sid": "ScopedByEncryptionContext",
        "Effect": effect,
        "Principal": {"AWS": "*"},
        "Action": actions,
        "Resource": "*",
        "Condition": condition,
    })
}

/// A statement allowing `principal` every KMS action — a policy whose only grant names
/// SOMEONE ELSE, so it delegates nothing to the calling account.
fn allow_to(principal: &str) -> Value {
    json!({
        "Sid": "AnotherPrincipal",
        "Effect": "Allow",
        "Principal": {"AWS": principal},
        "Action": "kms:*",
        "Resource": "*",
    })
}

/// The principal every refusal below names: the identity this emulator authenticates every
/// request as.
fn caller() -> String {
    state::caller_identity_arn()
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

/// A symmetric encryption key carrying the default key policy, as its id and its ARN.
async fn symmetric_key(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let metadata = create_key(router, &json!({"Description": case})).await;
    (
        text(case, &metadata, "KeyId"),
        text(case, &metadata, "Arn"),
    )
}

/// `request` carrying `context` as its `EncryptionContext`, and carrying no such member at
/// all when `context` is [`Value::Null`] — the two forms a request takes.
fn with_context(mut request: Value, context: &Value) -> Value {
    if !context.is_null() {
        request["EncryptionContext"] = context.clone();
    }
    request
}

/// How a case names the encryption context it drives.
fn label(context: &Value) -> String {
    if context.is_null() {
        "no encryption context".to_string()
    } else {
        context.to_string()
    }
}

/// Seal [`PAYLOAD`] under `key_id` with `context` as the encryption context, and answer the
/// ciphertext blob — which only a `Decrypt` carrying the SAME context can open, since the
/// context is authenticated.
async fn seal(router: &MultiServiceRouter, case: &str, key_id: &str, context: &Value) -> String {
    let answer = ok(
        router,
        "Encrypt",
        &with_context(json!({"KeyId": key_id, "Plaintext": b64(PAYLOAD)}), context),
    )
    .await;
    text(case, &answer, "CiphertextBlob")
}

/// Drive one call and assert the gate answered `expected` for it: a served request carries
/// HTTP 200, and a refused one carries HTTP 400 `AccessDeniedException` with AWS's sentence
/// for that refusal class — fixed text whose variable parts are the caller it refused, the
/// IAM action it asked for and the key it asked for it on.
async fn assert_decision(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
    key_arn: &str,
    expected: Expected,
) -> Value {
    let action = format!("kms:{operation}");
    let sentence = match expected {
        Expected::Served => None,
        Expected::ExplicitDeny => Some(format!(
            "User: {} is not authorized to perform: {action} on resource: {key_arn} with an \
             explicit deny in a resource-based policy",
            caller()
        )),
        Expected::NoResourcePolicy => Some(format!(
            "User: {} is not authorized to perform: {action} on resource: {key_arn} because no \
             resource-based policy allows the {action} action",
            caller()
        )),
        Expected::NoIdentityPolicy => Some(format!(
            "User: {} is not authorized to perform: {action} on resource: {key_arn} because no \
             identity-based policy allows the {action} action",
            caller()
        )),
    };
    match sentence {
        None => ok(router, operation, request).await,
        Some(sentence) => {
            let (status, code, message) = refused(router, REGION, operation, request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "AccessDeniedException"),
                "case {case}: expected {operation} to be refused HTTP 400 \
                 AccessDeniedException, got {status} {code}"
            );
            assert_eq!(
                message, sentence,
                "case {case}: expected the refusal {sentence:?}, got {message:?}"
            );
            Value::Null
        }
    }
}

/// Assert `Decrypt` answered the key that sealed the ciphertext and the exact plaintext.
fn assert_opened(case: &str, answer: &Value, key_arn: &str) {
    assert_eq!(
        text(case, answer, "KeyId"),
        key_arn,
        "case {case}: Decrypt must report the ARN of the key that opened the ciphertext, got \
         {answer}"
    );
    let plaintext = unb64(case, answer, "Plaintext");
    assert_eq!(
        plaintext,
        PAYLOAD.to_vec(),
        "case {case}: Decrypt must answer the exact bytes encrypted, got {plaintext:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 1: which requests a conditioned statement governs
// ---------------------------------------------------------------------------

/// A key-policy `Deny` gated on the request's encryption context refuses exactly the
/// requests whose context satisfies the condition: every other request is served by the
/// document's remaining statements, and the operations the statement does not name are
/// untouched whatever the context is.
#[tokio::test]
async fn a_conditioned_deny_refuses_only_the_encryption_contexts_its_condition_covers() {
    let router = router();

    // (case, the conditioned statement's `Condition`, and what the gate must answer a
    // `Decrypt` carrying each encryption context).
    let rows: Vec<ConditionRow> = vec![
        (
            "StringNotEquals on one entry — the per-tenant scoping",
            json!({"StringNotEquals": {"kms:EncryptionContext:tenant": "allowed"}}),
            vec![
                (json!({"tenant": "allowed"}), Expected::Served),
                (json!({"tenant": "other"}), Expected::ExplicitDeny),
                (
                    json!({"tenant": "allowed", "dept": "eng"}),
                    Expected::Served,
                ),
                // A negated operator holds where the request carries no value for the key,
                // so a request carrying no context at all is outside the permitted scope.
                (Value::Null, Expected::ExplicitDeny),
            ],
        ),
        (
            "StringEquals on one entry",
            json!({"StringEquals": {"kms:EncryptionContext:tenant": "other"}}),
            vec![
                (json!({"tenant": "other"}), Expected::ExplicitDeny),
                (json!({"tenant": "allowed"}), Expected::Served),
                (json!({"tenant": "OTHER"}), Expected::Served),
                // A positive operator does not hold on a key the request carries no value
                // for, so the statement does not apply.
                (Value::Null, Expected::Served),
            ],
        ),
        (
            "StringEquals naming a list of values",
            json!({"StringEquals": {"kms:EncryptionContext:tenant": ["other", "third"]}}),
            vec![
                (json!({"tenant": "other"}), Expected::ExplicitDeny),
                (json!({"tenant": "third"}), Expected::ExplicitDeny),
                (json!({"tenant": "allowed"}), Expected::Served),
            ],
        ),
        (
            "StringLike with a wildcard",
            json!({"StringLike": {"kms:EncryptionContext:tenant": "oth*"}}),
            vec![
                (json!({"tenant": "other"}), Expected::ExplicitDeny),
                (json!({"tenant": "allowed"}), Expected::Served),
            ],
        ),
        (
            "two clauses over two entries, both of which must hold",
            json!({"StringEquals": {
                "kms:EncryptionContext:tenant": "other",
                "kms:EncryptionContext:dept": "eng",
            }}),
            vec![
                (
                    json!({"tenant": "other", "dept": "eng"}),
                    Expected::ExplicitDeny,
                ),
                (
                    json!({"tenant": "other", "dept": "ops"}),
                    Expected::Served,
                ),
                (json!({"tenant": "other"}), Expected::Served),
            ],
        ),
        (
            "Null asserting the entry is absent",
            json!({"Null": {"kms:EncryptionContext:tenant": "true"}}),
            vec![
                (Value::Null, Expected::ExplicitDeny),
                (json!({"dept": "eng"}), Expected::ExplicitDeny),
                (json!({"tenant": "allowed"}), Expected::Served),
            ],
        ),
        (
            "ForAnyValue over the entry names",
            json!({"ForAnyValue:StringEquals": {"kms:EncryptionContextKeys": "tenant"}}),
            vec![
                (json!({"tenant": "allowed"}), Expected::ExplicitDeny),
                (json!({"dept": "eng"}), Expected::Served),
                // IAM's empty-set rule: `ForAnyValue:` does not hold where the request
                // carries no value for the key.
                (Value::Null, Expected::Served),
            ],
        ),
        (
            "ForAllValues over the entry names",
            json!({"ForAllValues:StringEquals": {
                "kms:EncryptionContextKeys": ["tenant", "dept"],
            }}),
            vec![
                (
                    json!({"tenant": "allowed", "dept": "eng"}),
                    Expected::ExplicitDeny,
                ),
                (json!({"tenant": "allowed", "zone": "a"}), Expected::Served),
                // IAM's empty-set rule: `ForAllValues:` holds where the request carries no
                // value for the key.
                (Value::Null, Expected::ExplicitDeny),
            ],
        ),
        (
            // The boundary of what the request models: a clause over a key this service
            // answers no value for leaves the statement undecided, which the shared
            // evaluator reads fail-closed — so the `Deny` is kept for every request.
            "a clause over a condition key this service does not model",
            json!({"StringEquals": {"kms:ViaService": format!("s3.{REGION}.amazonaws.com")}}),
            vec![
                (json!({"tenant": "allowed"}), Expected::ExplicitDeny),
                (Value::Null, Expected::ExplicitDeny),
            ],
        ),
    ];

    for (case, condition, requests) in rows {
        let (key_id, key_arn) = symmetric_key(&router, case).await;
        // Every ciphertext this row opens, sealed under its own context before the
        // conditioned statement is in force — the context is authenticated, so a `Decrypt`
        // must carry the one its ciphertext was sealed with.
        let mut sealed = Vec::new();
        for (context, _) in &requests {
            sealed.push(seal(&router, case, &key_id, context).await);
        }
        put_policy(
            &router,
            case,
            &key_id,
            &policy(vec![
                account_root_statement(),
                conditioned("Deny", json!("kms:Decrypt"), condition.clone()),
            ]),
        )
        .await;

        for ((context, expected), blob) in requests.iter().zip(&sealed) {
            let case = format!("{case} — Decrypt with {}", label(context));
            let answer = assert_decision(
                &router,
                &case,
                "Decrypt",
                &with_context(json!({"CiphertextBlob": blob}), context),
                &key_arn,
                *expected,
            )
            .await;
            if *expected == Expected::Served {
                assert_opened(&case, &answer, &key_arn);
            }

            // The statement names `kms:Decrypt` alone, so the operations it does not name
            // are served under every one of these contexts — including the ones it refuses
            // `Decrypt` for.
            let case = format!("{case}, with Encrypt and GenerateDataKey under it");
            assert_decision(
                &router,
                &case,
                "Encrypt",
                &with_context(
                    json!({"KeyId": &key_id, "Plaintext": b64(PAYLOAD)}),
                    context,
                ),
                &key_arn,
                Expected::Served,
            )
            .await;
            let generated = assert_decision(
                &router,
                &case,
                "GenerateDataKey",
                &with_context(
                    json!({"KeyId": &key_id, "KeySpec": "AES_256"}),
                    context,
                ),
                &key_arn,
                Expected::Served,
            )
            .await;
            assert_eq!(
                unb64(&case, &generated, "Plaintext").len(),
                32,
                "case {case}: GenerateDataKey must answer a 256-bit key, got {generated}"
            );
        }
    }
}

/// A key-policy `Allow` gated on the request's encryption context grants exactly the
/// requests whose context satisfies the condition; every other request is refused with the
/// sentence naming the resource-based policy, because that policy delegates nothing to the
/// account's identity policies.
#[tokio::test]
async fn a_conditioned_allow_grants_only_the_encryption_contexts_its_condition_covers() {
    let router = router();

    let rows: Vec<ConditionRow> = vec![
        (
            "StringEquals on one entry",
            json!({"StringEquals": {"kms:EncryptionContext:tenant": "allowed"}}),
            vec![
                (json!({"tenant": "allowed"}), Expected::Served),
                (
                    json!({"tenant": "allowed", "dept": "eng"}),
                    Expected::Served,
                ),
                (json!({"tenant": "other"}), Expected::NoResourcePolicy),
                (Value::Null, Expected::NoResourcePolicy),
            ],
        ),
        (
            "StringLike with a wildcard",
            json!({"StringLike": {"kms:EncryptionContext:tenant": "prod-*"}}),
            vec![
                (json!({"tenant": "prod-7"}), Expected::Served),
                (json!({"tenant": "dev-7"}), Expected::NoResourcePolicy),
            ],
        ),
        (
            "ForAllValues over the entry names — the context may carry no other entry",
            json!({"ForAllValues:StringEquals": {"kms:EncryptionContextKeys": "tenant"}}),
            vec![
                (json!({"tenant": "allowed"}), Expected::Served),
                (
                    json!({"tenant": "allowed", "dept": "eng"}),
                    Expected::NoResourcePolicy,
                ),
                (Value::Null, Expected::Served),
            ],
        ),
        (
            "StringNotEquals on one entry",
            json!({"StringNotEquals": {"kms:EncryptionContext:tenant": "blocked"}}),
            vec![
                (json!({"tenant": "allowed"}), Expected::Served),
                (json!({"tenant": "blocked"}), Expected::NoResourcePolicy),
                (Value::Null, Expected::Served),
            ],
        ),
    ];

    for (case, condition, requests) in rows {
        let (key_id, key_arn) = symmetric_key(&router, case).await;
        let mut sealed = Vec::new();
        for (context, _) in &requests {
            sealed.push(seal(&router, case, &key_id, context).await);
        }
        // The key's ONLY grant of `kms:Decrypt` is the conditioned one, and it names no
        // account root — so nothing is delegated to the account's identity policies.
        put_policy(
            &router,
            case,
            &key_id,
            &policy(vec![conditioned(
                "Allow",
                json!("kms:Decrypt"),
                condition.clone(),
            )]),
        )
        .await;

        for ((context, expected), blob) in requests.iter().zip(&sealed) {
            let case = format!("{case} — Decrypt with {}", label(context));
            let answer = assert_decision(
                &router,
                &case,
                "Decrypt",
                &with_context(json!({"CiphertextBlob": blob}), context),
                &key_arn,
                *expected,
            )
            .await;
            if *expected == Expected::Served {
                assert_opened(&case, &answer, &key_arn);
            }
        }
    }
}

/// What this gate answers an encryption-context condition for is BOUNDED: a request whose
/// context carries more entries than
/// [`state::authorization::MAX_PUBLISHED_ENCRYPTION_CONTEXT_ENTRIES`] is judged with none of
/// those keys answered for, so a statement conditioned on one stays undecided and the shared
/// evaluator reads it fail-closed — a conditioned `Allow` grants nothing. At the bound and
/// below, the very same condition decides as usual.
#[tokio::test]
async fn an_encryption_context_past_the_answerable_bound_is_read_fail_closed() {
    let router = router();
    let bound = state::authorization::MAX_PUBLISHED_ENCRYPTION_CONTEXT_ENTRIES;

    // (case, how many entries the context carries BESIDE the one the condition names, what
    // the conditioned `Allow` must then answer).
    let rows = [
        ("the named entry alone", 0, Expected::Served),
        ("entries filling the bound", bound - 1, Expected::Served),
        ("one entry past the bound", bound, Expected::NoResourcePolicy),
    ];

    for (case, padding, expected) in rows {
        let mut context = json!({"tenant": "allowed"});
        for index in 0..padding {
            context[format!("pad{index}")] = json!("beside the named entry");
        }
        let (key_id, key_arn) = symmetric_key(&router, case).await;
        let blob = seal(&router, case, &key_id, &context).await;
        put_policy(
            &router,
            case,
            &key_id,
            &policy(vec![conditioned(
                "Allow",
                json!("kms:Decrypt"),
                json!({"StringEquals": {"kms:EncryptionContext:tenant": "allowed"}}),
            )]),
        )
        .await;

        let case = format!("{case} ({} entries)", padding + 1);
        let answer = assert_decision(
            &router,
            &case,
            "Decrypt",
            &with_context(json!({"CiphertextBlob": blob}), &context),
            &key_arn,
            expected,
        )
        .await;
        if expected == Expected::Served {
            assert_opened(&case, &answer, &key_arn);
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: which document a no-allow refusal names
// ---------------------------------------------------------------------------

/// The refusal of a key-scoped action no authority allowed names the document that has to
/// change — the key's own policy, unless that policy delegates the action to the account's
/// identity policies — and it names the same document whether or not the request presented
/// a grant token.
#[tokio::test]
async fn the_refusal_of_a_key_scoped_action_names_the_document_that_withholds_it() {
    let router = router();
    let grantee = principal("role/EncryptionContextGrantee");

    // Both keys start with the default policy, which is what lets this caller seal a
    // ciphertext and issue the two grants before the policy under test is attached.
    let (delegating_id, delegating_arn) = symmetric_key(&router, "delegating key").await;
    let (withholding_id, withholding_arn) = symmetric_key(&router, "withholding key").await;
    let mut tokens = Vec::new();
    let mut blobs = Vec::new();
    for key_id in [&delegating_id, &withholding_id] {
        blobs.push(seal(&router, "grant tokens", key_id, &Value::Null).await);
        let mut minted = Vec::new();
        for operations in [json!(["Encrypt"]), json!(["Decrypt"])] {
            let answer = ok(
                &router,
                "CreateGrant",
                &json!({
                    "KeyId": key_id,
                    "GranteePrincipal": &grantee,
                    "Operations": operations,
                }),
            )
            .await;
            minted.push(text("grant tokens", &answer, "GrantToken"));
        }
        tokens.push(minted);
    }
    // The withholding key's only grant names another principal, so this caller's
    // `kms:Decrypt` is withheld by the key policy itself; the delegating key keeps the
    // default policy, which leaves the decision to the account's identity policies.
    put_policy(
        &router,
        "withholding key",
        &withholding_id,
        &policy(vec![allow_to(&principal("role/SomeoneElse"))]),
    )
    .await;

    // (case, which key, which of that key's grant tokens the request presents, expected).
    // `None` presents no token at all; `Some(0)` presents the token of the grant that
    // covers `Encrypt` only, and `Some(1)` the one that covers `Decrypt`.
    let rows: Vec<(&str, usize, Option<usize>, Expected)> = vec![
        ("the key policy delegates, no token", 0, None, Expected::Served),
        (
            "the key policy delegates, a token whose grant does not cover Decrypt",
            0,
            Some(0),
            Expected::NoIdentityPolicy,
        ),
        (
            "the key policy delegates, a token whose grant covers Decrypt",
            0,
            Some(1),
            Expected::Served,
        ),
        (
            "the key policy withholds the action, no token",
            1,
            None,
            Expected::NoResourcePolicy,
        ),
        (
            "the key policy withholds the action, a token whose grant does not cover Decrypt",
            1,
            Some(0),
            Expected::NoResourcePolicy,
        ),
        (
            "the key policy withholds the action, a token whose grant covers Decrypt",
            1,
            Some(1),
            Expected::Served,
        ),
    ];

    for (case, key, token, expected) in rows {
        let key_arn = [&delegating_arn, &withholding_arn][key];
        let mut request = json!({"CiphertextBlob": &blobs[key]});
        if let Some(token) = token {
            request["GrantTokens"] = json!([&tokens[key][token]]);
        }
        let answer =
            assert_decision(&router, case, "Decrypt", &request, key_arn, expected).await;
        if expected == Expected::Served {
            assert_opened(case, &answer, key_arn);
        }
    }
}
