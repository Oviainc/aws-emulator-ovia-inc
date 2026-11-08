// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ORDER the two refusals of a secret-addressing operation are
//! reached in, driven over the real wire path (an awsJson1_1 request through the
//! multi-service router into the mounted service), so what is asserted is what a client
//! holding those credentials sees.
//!
//! THE RULE: a request that addresses a secret is authorized BEFORE the store is consulted,
//! so which refusal a caller meets is decided by its own permissions first: a caller no
//! statement allows for the resource it addressed is answered the AWS common
//! `AccessDeniedException` whether or not that secret exists, and only a caller a statement
//! DOES allow can be answered `ResourceNotFoundException`. The refusal's resource clause
//! names the resource the request addressed as the service resolved it — the ARN the account
//! holds the secret under, whichever form of `SecretId` the request carried, and otherwise
//! the `SecretId` as the request spelled it, because an id this account holds no secret for
//! addresses no resource of this account and so has no ARN of its own to name.
//!
//! Without that order a narrowly scoped caller learns from `ResourceNotFoundException` that
//! a name it was never allowed to ask about is free, which turns any secret-addressing
//! operation into an existence oracle over the account's secret names.
//!
//! The tables span what the rule generalises over: the operation (a read of a value, a read
//! of metadata, a per-secret version list, a read of an attached policy, a tag write, a
//! value write and a delete), the form of the `SecretId` the request addresses the secret by
//! (a bare name, a full ARN, a partial ARN), whether the account HOLDS a secret under that
//! id, and the scope of the grant in force (this action on everything, and this action on
//! one other secret's ARN).
//!
//! `tests/all/identity_based_authorization.rs` spans WHICH principal, action and resource a
//! decision is taken on, and owns the harness these cases reuse.

use serde_json::{json, Value};

use crate::identity_based_authorization::{
    call_as, error_code, error_message, given_role_policy, given_secret, given_session,
    request_body, router_with_identity, session_arn, ACCESS_DENIED, ACCESS_DENIED_STATUS,
    SESSION_KEY_ID,
};

/// The Region every case signs for, the one the shared harness builds its requests in.
const REGION: &str = "us-west-2";

/// The error code an operation answers a `SecretId` it finds no secret for with, and the
/// sentence that carries — Secrets Manager's own, which states no part of the request.
const NOT_FOUND: &str = "ResourceNotFoundException";
const NOT_FOUND_MESSAGE: &str =
    service_com_amazonaws_secretsmanager_secretsmanager::state::SECRET_NOT_FOUND_MESSAGE;

/// The operations whose request addresses one secret, one per class of what such an
/// operation does: a read of a value, a read of metadata, a per-secret version list, a read
/// of an attached policy, a tag write, a value write and a delete.
const SECRET_OPERATIONS: &[&str] = &[
    "GetSecretValue",
    "DescribeSecret",
    "ListSecretVersionIds",
    "GetResourcePolicy",
    "TagResource",
    "PutSecretValue",
    "DeleteSecret",
];

/// A policy allowing `action` on `resource` and nothing else.
fn allow_on(action: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }]
    })
}

/// The forms a `SecretId` takes, as the model states it ("The ARN or name of the secret",
/// `GetSecretValueRequest$SecretId`) plus the partial ARN that names a secret too (the full
/// ARN without its six trailing characters): `(case, the id, what a refusal names for it)`.
///
/// `arn` is the full ARN the account would hold a secret of that `name` under.
fn secret_id_forms(name: &str, arn: &str, reported_arn: Option<&str>) -> Vec<(String, String)> {
    let partial = arn
        .rsplit_once('-')
        .map(|(prefix, _)| prefix.to_string())
        .unwrap_or_else(|| arn.to_string());
    // Where the account HOLDS the secret, every form resolves to the ARN it holds it under;
    // where it does not, the refusal names the id as the request spelled it.
    vec![
        (
            name.to_string(),
            reported_arn.unwrap_or(name).to_string(),
        ),
        (
            arn.to_string(),
            reported_arn.unwrap_or(arn).to_string(),
        ),
        (
            partial.clone(),
            reported_arn.unwrap_or(&partial).to_string(),
        ),
    ]
}

/// The ARN the account would hold a secret of `name` under, with the six trailing characters
/// a secret's ARN ends in.
fn arn_of(name: &str) -> String {
    format!(
        "arn:aws:secretsmanager:{REGION}:{}:secret:{name}-AbCdEf",
        ::engine::emulator_account_id()
    )
}

/// Assert `answer` is the `AccessDeniedException` IAM words for the default deny of
/// `action` on `resource`, blaming the session that signed.
fn assert_denied(case: &str, status: u16, answer: &Value, action: &str, resource: &str) {
    assert_eq!(
        status, ACCESS_DENIED_STATUS,
        "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {status} {answer}"
    );
    assert_eq!(
        error_code(answer),
        ACCESS_DENIED,
        "case {case:?}: expected an {ACCESS_DENIED}, got {:?}",
        error_code(answer)
    );
    let expected = format!(
        "User: {} is not authorized to perform: {action} on resource: {resource} because no \
         identity-based policy allows the {action} action",
        session_arn()
    );
    assert_eq!(
        error_message(answer),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        error_message(answer)
    );
}

/// Assert `answer` is the `ResourceNotFoundException` an AUTHORIZED caller meets for a
/// `SecretId` the account holds no secret for.
fn assert_not_found(case: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, ACCESS_DENIED_STATUS,
        "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {status} {answer}"
    );
    assert_eq!(
        error_code(answer),
        NOT_FOUND,
        "case {case:?}: expected a {NOT_FOUND}, got {:?} ({answer})",
        error_code(answer)
    );
    assert_eq!(
        error_message(answer),
        NOT_FOUND_MESSAGE,
        "case {case:?}: expected {NOT_FOUND_MESSAGE:?}, got {:?}",
        error_message(answer)
    );
}

/// THE RULE for a `SecretId` the account holds no secret for: the caller's scope decides
/// first. A grant covering every resource reaches the operation's own
/// `ResourceNotFoundException`; a grant scoped to one other secret's ARN is refused
/// `AccessDeniedException` naming the id the request spelled — and that same scoped grant
/// still serves the secret it DOES cover, so the refusal is the scope and not a blanket
/// deny.
#[tokio::test]
async fn a_missing_secret_is_refused_by_scope_before_it_is_reported_missing() {
    for operation in SECRET_OPERATIONS {
        let action = format!("secretsmanager:{operation}");
        for (missing_id, reported) in
            secret_id_forms("absent-secret", &arn_of("absent-secret"), None)
        {
            let (router, engine) = router_with_identity();
            given_session(&engine);
            let in_scope = given_secret(&router, "in-scope-secret").await;
            let scoped = allow_on(&action, &in_scope);

            // A grant scoped to ANOTHER secret's ARN: the request addresses a resource no
            // statement allows, so it is refused before the store is consulted.
            given_role_policy(&engine, Some(&scoped));
            let case = format!("{operation} on {missing_id:?} with a grant scoped elsewhere");
            let (status, answer) = call_as(
                &router,
                SESSION_KEY_ID,
                operation,
                &request_body(operation, &missing_id, "unused-create-name"),
            )
            .await;
            assert_denied(&case, status, &answer, &action, &reported);

            // The same request under a grant covering every resource: authorized, so it
            // reaches the operation's own answer for a secret that is not there.
            given_role_policy(&engine, Some(&allow_on(&action, "*")));
            let case = format!("{operation} on {missing_id:?} with a grant on every resource");
            let (status, answer) = call_as(
                &router,
                SESSION_KEY_ID,
                operation,
                &request_body(operation, &missing_id, "unused-create-name"),
            )
            .await;
            assert_not_found(&case, status, &answer);

            // The positive control, last because a write among these operations changes the
            // secret it is served on: the scoped grant still serves its OWN secret.
            given_role_policy(&engine, Some(&scoped));
            let case = format!("{operation} on the secret the scoped grant covers");
            let (status, answer) = call_as(
                &router,
                SESSION_KEY_ID,
                operation,
                &request_body(operation, &in_scope, "unused-create-name"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the secret the grant covers must be served, got {status} {answer}"
            );
        }
    }
}

/// THE RULE's other half: a secret the account DOES hold is named by the ARN it holds it
/// under, whichever form of `SecretId` addressed it — so a scoped caller's refusal about an
/// existing secret outside its scope carries that ARN, not the id it typed.
#[tokio::test]
async fn an_existing_secret_outside_the_scope_is_named_by_the_arn_the_account_holds_it_under() {
    for operation in SECRET_OPERATIONS {
        let action = format!("secretsmanager:{operation}");
        let (router, engine) = router_with_identity();
        given_session(&engine);
        let in_scope = given_secret(&router, "covered-secret").await;
        let elsewhere = given_secret(&router, "uncovered-secret").await;
        given_role_policy(&engine, Some(&allow_on(&action, &in_scope)));

        for (addressed, reported) in
            secret_id_forms("uncovered-secret", &elsewhere, Some(&elsewhere))
        {
            let case = format!("{operation} on {addressed:?}, a held secret outside the scope");
            let (status, answer) = call_as(
                &router,
                SESSION_KEY_ID,
                operation,
                &request_body(operation, &addressed, "unused-create-name"),
            )
            .await;
            assert_denied(&case, status, &answer, &action, &reported);
        }
    }
}
