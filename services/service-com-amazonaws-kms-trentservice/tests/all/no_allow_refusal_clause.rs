// SPDX-License-Identifier: CC0-1.0

//! WHICH document an authorization failure that no authority allowed tells the caller to
//! repair, over the awsJson1_1 wire.
//!
//! The rule under test: a KMS request that no authority in force allows is refused
//! `AccessDeniedException` (HTTP 400) with AWS's implicit-deny sentence, and the clause
//! that sentence ends on names the document class that failed to allow the action — the
//! RESOURCE-based policy when the key's own policy allows the action to neither the
//! calling principal nor its account (so no identity policy could widen it), and the
//! IDENTITY-based policy when the key policy does allow the action to the caller's account
//! and therefore delegates the decision to the principal's own documents. A key-policy
//! statement counts as allowing only when it APPLIES to the request, so a statement gated
//! on a condition the request does not satisfy — `kms:ViaService` on a direct call, a
//! principal clause naming somebody else — allows nothing and leaves the key policy as the
//! document that withheld the action.
//!
//! Every assertion is made over a TABLE whose rows differ along what the rule generalises
//! over: which principal spelling the statement grants to (the calling principal itself,
//! the account root, the bare account id), whether the grant carries a `Condition` and
//! whether that condition HOLDS for the request, the condition operator and the condition
//! key it compares (`kms:ViaService`, which a direct call does not model, and
//! `aws:PrincipalArn` / `aws:PrincipalAccount`, which it does), the kind of caller (the
//! account's own published credential, an IAM user whose identity policy allows the action,
//! an IAM user carrying none) and the operation that is judged, each named in the refusal
//! under its own `kms:` action.
//!
//! The principals are made with IAM's own operations over the same engine, so the
//! identity-based documents this service reads are the ones IAM wrote.

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    iam_user, ok, ok_as, refused_as, router, router_with_identity_services, text, unb64,
    ACCOUNT_KEY, REGION,
};

/// What the gate must answer one caller for one operation on one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// HTTP 200: an authority in force allows the request.
    Served,
    /// Refused, naming the key's own policy as the document that withheld the action.
    ResourceBased,
    /// Refused, naming the principal's identity-based policies as the missing permission.
    IdentityBased,
}

/// The operations the table judges, each with the IAM action its refusal must name.
const OPERATIONS: &[(&str, &str)] = &[
    ("GenerateDataKey", "kms:GenerateDataKey"),
    ("Encrypt", "kms:Encrypt"),
];

/// A key policy document carrying `statements`.
fn policy(statements: Value) -> String {
    json!({"Version": "2012-10-17", "Id": "key-under-test", "Statement": statements}).to_string()
}

/// One `Allow` statement for `principal` over every resource, carrying `actions` and the
/// `Condition` given (none when it is [`Value::Null`]).
fn allow(sid: &str, principal: Value, actions: Value, condition: Value) -> Value {
    let mut statement = json!({
        "Sid": sid,
        "Effect": "Allow",
        "Principal": {"AWS": principal},
        "Action": actions,
        "Resource": "*",
    });
    if !condition.is_null() {
        statement["Condition"] = condition;
    }
    statement
}

/// The statement the canonical confinement pattern carries beside the cryptographic one,
/// so the key stays manageable: it covers no cryptographic action, so it grants none.
fn admin_statement(root: &str) -> Value {
    allow(
        "Admin",
        json!(root),
        json!([
            "kms:DescribeKey",
            "kms:GetKeyPolicy",
            "kms:PutKeyPolicy",
            "kms:ScheduleKeyDeletion",
            "kms:CancelKeyDeletion"
        ]),
        Value::Null,
    )
}

/// The cryptographic actions a key policy confines, as the canonical pattern names them.
fn crypto_actions() -> Value {
    json!([
        "kms:Encrypt",
        "kms:Decrypt",
        "kms:ReEncrypt*",
        "kms:GenerateDataKey*"
    ])
}

/// An identity policy document allowing `actions` on every resource.
fn identity_allow(actions: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": "*"}],
    })
    .to_string()
}

/// The regional endpoint name of an AWS service, which is the value `kms:ViaService`
/// carries for a request another service made on the caller's behalf.
fn endpoint(service: &str) -> String {
    format!("{service}.{REGION}.amazonaws.com")
}

/// The request body of one operation of the table, on `key_id`.
fn request_for(case: &str, operation: &str, key_id: &str) -> Value {
    match operation {
        "GenerateDataKey" => json!({"KeyId": key_id, "KeySpec": "AES_256"}),
        "Encrypt" => json!({"KeyId": key_id, "Plaintext": "cGF5bG9hZA=="}),
        other => panic!("case {case}: {other} is not an operation this table drives"),
    }
}

/// The member a served call of `operation` must carry.
fn served_member(operation: &str) -> &'static str {
    match operation {
        "GenerateDataKey" => "Plaintext",
        _ => "CiphertextBlob",
    }
}

/// The sentence AWS refuses a request with when the key's own policy withheld the action.
fn resource_based(principal: &str, action: &str, key_arn: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {key_arn} \
         because no resource-based policy allows the {action} action"
    )
}

/// The sentence AWS refuses a request with when the key policy delegates to the caller's
/// account and the principal's own documents do not allow the action.
fn identity_based(principal: &str, action: &str, key_arn: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {key_arn} \
         because no identity-based policy allows the {action} action"
    )
}

/// Drive one call and assert the gate answered `expected`: a served request carries the
/// operation's own answer, and a refused one carries HTTP 400 `AccessDeniedException` with
/// the sentence of its refusal class, whose variable parts are the caller it refused, the
/// IAM action it asked for and the key it asked for it on.
async fn assert_decision(
    router: &MultiServiceRouter,
    case: &str,
    caller: (&str, &str),
    key: (&str, &str),
    operation: (&str, &str),
    expected: Expected,
) {
    let (access_key_id, caller_arn) = caller;
    let (key_id, key_arn) = key;
    let (operation, action) = operation;
    let body = request_for(case, operation, key_id);
    let sentence = match expected {
        Expected::Served => None,
        Expected::ResourceBased => Some(resource_based(caller_arn, action, key_arn)),
        Expected::IdentityBased => Some(identity_based(caller_arn, action, key_arn)),
    };
    match sentence {
        None => {
            let answer = ok_as(router, access_key_id, operation, &body).await;
            let member = served_member(operation);
            assert!(
                answer[member].is_string(),
                "case {case}: expected {operation} by {caller_arn} to answer {member}, \
                 got {answer}"
            );
        }
        Some(sentence) => {
            let (status, code, message) = refused_as(router, access_key_id, operation, &body).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "AccessDeniedException"),
                "case {case}: expected {operation} by {caller_arn} to be refused HTTP 400 \
                 AccessDeniedException, got {status} with {code}"
            );
            assert_eq!(
                message, sentence,
                "case {case}: expected the refusal {sentence:?} for {caller_arn}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_no_allow_refusal_names_the_document_that_withheld_the_action() {
    let router = router_with_identity_services();
    let account = engine::emulator_account_id();
    let root = format!("arn:aws:iam::{account}:root");
    let ambient = state::caller_identity_arn();
    let (allowed_key, allowed_arn) = iam_user(
        &router,
        "ClauseAllowed",
        Some(&identity_allow(json!("kms:*"))),
    )
    .await;
    let (bare_key, bare_arn) = iam_user(&router, "ClauseBare", None).await;

    // The three kinds of caller, in the order every row's expectations are given in.
    let callers: [(&str, &str); 3] = [
        (ACCOUNT_KEY, ambient.as_str()),
        (allowed_key.as_str(), allowed_arn.as_str()),
        (bare_key.as_str(), bare_arn.as_str()),
    ];
    // The spelling a statement names each caller ITSELF by, which grants to it outright.
    let every_principal = json!([&ambient, &allowed_arn, &bare_arn]);

    let rows: Vec<(&str, String, [Expected; 3])> = vec![
        (
            "the cryptographic allow delegates to the account root, gated on kms:ViaService",
            policy(json!([
                admin_statement(&root),
                allow(
                    "CryptoUse",
                    json!(&root),
                    crypto_actions(),
                    json!({"StringEquals": {"kms:ViaService": endpoint("s3")}}),
                ),
            ])),
            [
                Expected::ResourceBased,
                Expected::ResourceBased,
                Expected::ResourceBased,
            ],
        ),
        (
            "the cryptographic allow names each caller itself, gated on kms:ViaService",
            policy(json!([
                admin_statement(&root),
                allow(
                    "CryptoUse",
                    every_principal.clone(),
                    crypto_actions(),
                    json!({"StringEquals": {"kms:ViaService": endpoint("dynamodb")}}),
                ),
            ])),
            [
                Expected::ResourceBased,
                Expected::ResourceBased,
                Expected::ResourceBased,
            ],
        ),
        (
            "the gating condition compares the Region with a wildcard",
            policy(json!([
                admin_statement(&root),
                allow(
                    "CryptoUse",
                    json!(&root),
                    json!("kms:*"),
                    json!({"StringLike": {"kms:ViaService": "s3.*.amazonaws.com"}}),
                ),
            ])),
            [
                Expected::ResourceBased,
                Expected::ResourceBased,
                Expected::ResourceBased,
            ],
        ),
        (
            "the gating condition qualifies the same comparison over a set",
            policy(json!([
                admin_statement(&root),
                allow(
                    "CryptoUse",
                    json!(&root),
                    json!("kms:*"),
                    json!({"ForAnyValue:StringEquals": {
                        "kms:ViaService": [endpoint("s3"), endpoint("lambda")]
                    }}),
                ),
            ])),
            [
                Expected::ResourceBased,
                Expected::ResourceBased,
                Expected::ResourceBased,
            ],
        ),
        (
            // A condition key the request DOES model, whose comparison is false for every
            // caller: the clause follows whether the statement applies, not whether the
            // key it compares is one this service can decide.
            "the cryptographic allow is gated on the ARN of a principal that is not calling",
            policy(json!([
                admin_statement(&root),
                allow(
                    "CryptoUse",
                    json!(&root),
                    json!("kms:*"),
                    json!({"StringEquals": {
                        "aws:PrincipalArn": format!("arn:aws:iam::{account}:user/absent")
                    }}),
                ),
            ])),
            [
                Expected::ResourceBased,
                Expected::ResourceBased,
                Expected::ResourceBased,
            ],
        ),
        (
            // No statement names the cryptographic actions at all: the key policy
            // withholds them outright, with no condition involved.
            "the policy carries only the administrative allow",
            policy(json!([admin_statement(&root)])),
            [
                Expected::ResourceBased,
                Expected::ResourceBased,
                Expected::ResourceBased,
            ],
        ),
        (
            // The delegation applies, so the missing permission is the principal's own.
            "the cryptographic allow delegates to the account root, ungated",
            policy(json!([
                admin_statement(&root),
                allow("CryptoUse", json!(&root), crypto_actions(), Value::Null),
            ])),
            [
                Expected::Served,
                Expected::Served,
                Expected::IdentityBased,
            ],
        ),
        (
            // The other spelling of the same account, which delegates just as the root ARN
            // does.
            "the cryptographic allow delegates to the bare account id, ungated",
            policy(json!([
                admin_statement(&root),
                allow("CryptoUse", json!(&account), json!("kms:*"), Value::Null),
            ])),
            [
                Expected::Served,
                Expected::Served,
                Expected::IdentityBased,
            ],
        ),
        (
            // A delegation whose condition HOLDS for the request is a delegation, so the
            // presence of a `Condition` block is not what selects the clause.
            "the delegation is gated on the account every caller belongs to",
            policy(json!([
                admin_statement(&root),
                allow(
                    "CryptoUse",
                    json!(&root),
                    json!("kms:*"),
                    json!({"StringEquals": {"aws:PrincipalAccount": &account}}),
                ),
            ])),
            [
                Expected::Served,
                Expected::Served,
                Expected::IdentityBased,
            ],
        ),
        (
            // A statement naming the principal itself grants outright, so no identity
            // policy is needed by any of the three callers.
            "the cryptographic allow names each caller itself, ungated",
            policy(json!([
                admin_statement(&root),
                allow("CryptoUse", every_principal, crypto_actions(), Value::Null),
            ])),
            [Expected::Served, Expected::Served, Expected::Served],
        ),
    ];

    for (case, document, expected) in rows {
        let metadata = ok_as(
            &router,
            ACCOUNT_KEY,
            "CreateKey",
            &json!({"Description": case, "Policy": document}),
        )
        .await["KeyMetadata"]
            .clone();
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");

        for (caller, expected) in callers.into_iter().zip(expected) {
            for operation in OPERATIONS {
                let case = format!("{case} — {} calling {}", caller.1, operation.0);
                assert_decision(
                    &router,
                    &case,
                    caller,
                    (&key_id, &key_arn),
                    *operation,
                    expected,
                )
                .await;
            }
        }
    }
}

/// The data key sizes a request asks for, in the two forms it may ask in, with the length
/// of the plaintext key each must answer: `KeySpec` names the length AWS publishes for that
/// specification (`smithy_query(query="shape",shape="DataKeySpec")`: `AES_256`, `AES_128`),
/// and `NumberOfBytes` states it directly.
const SIZES: &[(&str, usize)] = &[("AES_256", 32), ("AES_128", 16), ("64", 64)];

/// The request `GenerateDataKey` is asked for one row of [`SIZES`] with.
fn data_key_request(key_id: &str, size: &str) -> Value {
    match size.parse::<i64>() {
        Ok(bytes) => json!({"KeyId": key_id, "NumberOfBytes": bytes}),
        Err(_) => json!({"KeyId": key_id, "KeySpec": size}),
    }
}

/// Dropping the gating condition from the cryptographic allow serves the very next direct
/// call, which pins the refusal to the condition having been evaluated and not satisfied —
/// rather than to the statement having been lost, or to the key.
#[tokio::test]
async fn removing_the_gating_condition_re_allows_the_direct_call() {
    let router = router();
    let account = engine::emulator_account_id();
    let root = format!("arn:aws:iam::{account}:root");
    let caller_arn = state::caller_identity_arn();
    let condition = json!({"StringEquals": {"kms:ViaService": endpoint("s3")}});

    for (size, length) in SIZES {
        let case = format!("a data key of {size}");
        let case = case.as_str();
        let confined = policy(json!([
            admin_statement(&root),
            allow(
                "CryptoUse",
                json!(&root),
                crypto_actions(),
                condition.clone(),
            ),
        ]));
        let metadata = ok(
            &router,
            "CreateKey",
            &json!({"Description": case, "Policy": confined}),
        )
        .await["KeyMetadata"]
            .clone();
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");

        // The condition is in the document the key carries, read back through the service's
        // own reader.
        let stored = ok(&router, "GetKeyPolicy", &json!({"KeyId": &key_id})).await;
        let stored: Value = serde_json::from_str(&text(case, &stored, "Policy"))
            .unwrap_or_else(|error| panic!("case {case}: the stored policy is JSON: {error}"));
        assert_eq!(
            stored["Statement"][1]["Condition"], condition,
            "case {case}: the key must carry the gating condition, got {stored}"
        );

        // While it is in force, the direct call is refused and the key policy is named as
        // the document that withheld the action.
        let request = data_key_request(&key_id, size);
        let (status, code, message) =
            refused_as(&router, ACCOUNT_KEY, "GenerateDataKey", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "AccessDeniedException"),
            "case {case}: expected HTTP 400 AccessDeniedException, got {status} with {code}"
        );
        assert_eq!(
            message,
            resource_based(&caller_arn, "kms:GenerateDataKey", &key_arn),
            "case {case}: expected AWS's denial sentence, got {message:?}"
        );

        // The same statement without its condition allows the same call.
        let opened = policy(json!([
            admin_statement(&root),
            allow("CryptoUse", json!(&root), crypto_actions(), Value::Null),
        ]));
        ok(
            &router,
            "PutKeyPolicy",
            &json!({"KeyId": &key_id, "PolicyName": "default", "Policy": opened}),
        )
        .await;

        let answer = ok(&router, "GenerateDataKey", &request).await;
        let plaintext = unb64(case, &answer, "Plaintext");
        assert_eq!(
            plaintext.len(),
            *length,
            "case {case}: expected a {length}-byte data key, got {} bytes",
            plaintext.len()
        );
        let ciphertext = text(case, &answer, "CiphertextBlob");

        // The data key the refusal had withheld round-trips through the key that sealed it.
        let opened = ok(&router, "Decrypt", &json!({"CiphertextBlob": ciphertext})).await;
        assert_eq!(
            text(case, &opened, "KeyId"),
            key_arn,
            "case {case}: Decrypt must report the key that opened the ciphertext, got {opened}"
        );
        let decrypted = unb64(case, &opened, "Plaintext");
        assert_eq!(
            decrypted, plaintext,
            "case {case}: Decrypt must answer the data key's own bytes, got {decrypted:?}"
        );
    }
}

/// A request whose grant token names no grant that permits the operation is refused the
/// same way, naming the document class the key policy left missing: the key's own policy
/// when it allows the operation to neither the caller nor its account, and the identity
/// policies when it delegates to the account.
#[tokio::test]
async fn a_grant_token_that_permits_nothing_is_refused_under_the_same_rule() {
    let router = router();
    let account = engine::emulator_account_id();
    let root = format!("arn:aws:iam::{account}:root");
    let caller_arn = state::caller_identity_arn();

    // A grant token this service really minted, for an operation the request below does not
    // ask for, so the grants it names permit nothing here.
    let delegating = policy(json!([
        admin_statement(&root),
        allow("CryptoUse", json!(&root), json!("kms:*"), Value::Null),
    ]));
    let metadata = ok(
        &router,
        "CreateKey",
        &json!({"Description": "a grant token that permits nothing", "Policy": delegating}),
    )
    .await["KeyMetadata"]
        .clone();
    let key_id = text("grant token", &metadata, "KeyId");
    let key_arn = text("grant token", &metadata, "Arn");
    let grant = ok(
        &router,
        "CreateGrant",
        &json!({
            "KeyId": &key_id,
            "GranteePrincipal": format!("arn:aws:iam::{account}:role/Grantee"),
            "Operations": ["DescribeKey"],
        }),
    )
    .await;
    let token = text("grant token", &grant, "GrantToken");

    // Rows: the key policy in force, and the document class the refusal must name.
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "the key policy delegates the operation to the account",
            policy(json!([
                admin_statement(&root),
                allow("CryptoUse", json!(&root), json!("kms:*"), Value::Null),
            ])),
            true,
        ),
        (
            "the key policy's only cryptographic allow is gated on kms:ViaService",
            policy(json!([
                admin_statement(&root),
                allow(
                    "CryptoUse",
                    json!(&root),
                    crypto_actions(),
                    json!({"StringEquals": {"kms:ViaService": endpoint("s3")}}),
                ),
            ])),
            false,
        ),
    ];

    for (case, document, delegates) in rows {
        ok(
            &router,
            "PutKeyPolicy",
            &json!({"KeyId": &key_id, "PolicyName": "default", "Policy": document}),
        )
        .await;
        let (status, code, message) = refused_as(
            &router,
            ACCOUNT_KEY,
            "GenerateDataKey",
            &json!({"KeyId": &key_id, "KeySpec": "AES_256", "GrantTokens": [&token]}),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "AccessDeniedException"),
            "case {case}: expected HTTP 400 AccessDeniedException, got {status} with {code}"
        );
        let expected = if delegates {
            identity_based(&caller_arn, "kms:GenerateDataKey", &key_arn)
        } else {
            resource_based(&caller_arn, "kms:GenerateDataKey", &key_arn)
        };
        assert_eq!(
            message, expected,
            "case {case}: expected the refusal {expected:?}, got {message:?}"
        );
    }
}
