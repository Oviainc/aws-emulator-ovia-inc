// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT AN AUTHORIZATION REFUSAL OF THIS SERVICE NAMES AS THE RESOURCE,
//! driven over the real wire path (an awsJson1_1 request through the multi-service router
//! into the mounted service), so what is asserted is the sentence a client holding those
//! credentials reads.
//!
//! THE RULE: the resource clause of the `AccessDeniedException` states the identifier the
//! REQUEST supplied for the resource, verbatim — the `SecretId` exactly as it was spelled (a
//! bare name, a partial ARN or a full ARN) and a create's `Name` — never an identifier the
//! service resolved for it. The DECISION is a separate question and still resolves that id
//! to the ARN a statement's `Resource` element matches, which is what the positive rows
//! here hold fixed: the same grant scoped to the resolved ARN serves every form of the id.
//!
//! A refused caller is told which resource IT asked about, so the sentence is reproducible
//! from the request alone and a client — or policy-debugging tooling — matching the clause
//! against what it sent finds it there. It also keeps the refusal from publishing the
//! resolved ARN of a secret the refused caller was allowed to learn nothing about.
//!
//! The table spans what the rule generalises over: the form of the `SecretId` (a bare name,
//! a name carrying the `/` a secret name may contain, a full ARN, the partial ARN), whether
//! the account HOLDS a secret under that id, the operation (a read of a value, a read of
//! metadata, a tag write, a delete and a create, which addresses the secret it is about to
//! make), and which of IAM's two rules refused (the default deny of a request no statement
//! allows, and an explicit `Deny`).
//!
//! `tests/all/identity_based_authorization.rs` spans WHICH principal, action and resource a
//! decision is taken on, and owns the harness these cases reuse;
//! `tests/all/authorization_before_existence.rs` spans the ORDER the two refusals are
//! reached in.

use serde_json::{json, Value};

use crate::identity_based_authorization::{
    call_as, error_code, error_message, given_role_policy, given_secret, given_session,
    request_body, router_with_identity, session_arn, ACCESS_DENIED, ACCESS_DENIED_STATUS,
    SECRET_VALUE, SESSION_KEY_ID,
};

/// The operations whose request addresses one secret by `SecretId`, one per class of what
/// such an operation does: a read of a value, a read of metadata, a tag write and a delete.
const SECRET_ID_OPERATIONS: &[&str] = &[
    "GetSecretValue",
    "DescribeSecret",
    "TagResource",
    "DeleteSecret",
];

/// A policy allowing `action` on `resource` and nothing else.
fn allow_on(action: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }]
    })
}

/// A policy allowing `action` on everything beside an explicit `Deny` of it, so the refusal
/// a row meets is IAM's explicit deny rather than its default one.
fn deny_beside_allow(action: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": action, "Resource": "*" },
            { "Effect": "Deny", "Action": action, "Resource": "*" },
        ]
    })
}

/// The partial ARN of `arn`: the full ARN without the six trailing characters a secret's ARN
/// ends in, which names the secret too.
fn partial_arn(arn: &str) -> String {
    arn.rsplit_once('-')
        .map(|(prefix, _)| prefix.to_string())
        .unwrap_or_else(|| arn.to_string())
}

/// Assert `answer` is the refusal IAM words for `reason`, naming the session that signed,
/// `action` and — verbatim — the identifier `supplied`.
fn assert_names_the_supplied_id(
    case: &str,
    status: u16,
    answer: &Value,
    action: &str,
    supplied: &str,
    reason: &str,
) {
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
        "User: {} is not authorized to perform: {action} on resource: {supplied} {reason}",
        session_arn()
    );
    assert_eq!(
        error_message(answer),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        error_message(answer)
    );
}

/// THE RULE for a secret the account HOLDS: whichever form of `SecretId` addressed it, and
/// whichever of IAM's two rules refused, the clause carries that id exactly as the request
/// spelled it — not the ARN the account holds the secret under.
#[tokio::test]
async fn the_refusal_names_the_secret_id_the_request_supplied() {
    // The names a row addresses its secret by: a bare one, and one carrying the `/` a
    // secret name may contain ("The name of the new secret… /_+=.@-", `CreateSecretRequest$Name`).
    for name in ["held-secret", "prod/db/held-secret"] {
        for operation in SECRET_ID_OPERATIONS {
            let action = format!("secretsmanager:{operation}");
            let (router, engine) = router_with_identity();
            given_session(&engine);
            let arn = given_secret(&router, name).await;
            let partial = partial_arn(&arn);
            // (what the row varies, the id it supplies)
            let forms: &[(&str, &str)] = &[
                ("its name", name),
                ("its full ARN", arn.as_str()),
                ("its partial ARN", partial.as_str()),
            ];
            // (what the row varies, the document in force, the clause IAM closes with)
            let refusals: &[(&str, Value, String)] = &[
                (
                    "no statement allowing the action",
                    allow_on(&action, "arn:aws:secretsmanager:*:*:secret:elsewhere-*"),
                    format!("because no identity-based policy allows the {action} action"),
                ),
                (
                    "an explicit deny of the action",
                    deny_beside_allow(&action),
                    "with an explicit deny in an identity-based policy".to_string(),
                ),
            ];

            for (refused_by, document, reason) in refusals {
                given_role_policy(&engine, Some(document));
                for (form, supplied) in forms {
                    let case =
                        format!("{operation} on {name:?} by {form}, refused by {refused_by}");
                    let (status, answer) = call_as(
                        &router,
                        SESSION_KEY_ID,
                        operation,
                        &request_body(operation, supplied, "unused-create-name"),
                    )
                    .await;
                    assert_names_the_supplied_id(
                        &case, status, &answer, &action, supplied, reason,
                    );
                }
            }
        }
    }
}

/// The same for a `SecretId` the account holds NO secret for: the clause carries the id as
/// the request spelled it, so a refusal reads the same whether or not the secret exists and
/// says nothing about which it was.
#[tokio::test]
async fn an_unheld_secret_id_is_named_as_the_request_spelled_it() {
    let absent_arn = format!(
        "arn:aws:secretsmanager:us-west-2:{}:secret:absent-secret-AbCdEf",
        ::engine::emulator_account_id()
    );
    let absent_partial = partial_arn(&absent_arn);
    // (what the row varies, the id it supplies)
    let forms: &[(&str, &str)] = &[
        ("a bare name", "absent-secret"),
        ("a name carrying a slash", "prod/db/absent-secret"),
        ("a full ARN", absent_arn.as_str()),
        ("a partial ARN", absent_partial.as_str()),
    ];
    for operation in SECRET_ID_OPERATIONS {
        let action = format!("secretsmanager:{operation}");
        let reason = format!("because no identity-based policy allows the {action} action");
        let (router, engine) = router_with_identity();
        given_session(&engine);
        // A grant scoped to one OTHER secret, so the request is refused by its scope before
        // the store is consulted rather than answered `ResourceNotFoundException`.
        let in_scope = given_secret(&router, "in-scope-secret").await;
        given_role_policy(&engine, Some(&allow_on(&action, &in_scope)));

        for (form, supplied) in forms {
            let case = format!("{operation} on {form}");
            let (status, answer) = call_as(
                &router,
                SESSION_KEY_ID,
                operation,
                &request_body(operation, supplied, "unused-create-name"),
            )
            .await;
            assert_names_the_supplied_id(&case, status, &answer, &action, supplied, &reason);
        }
    }
}

/// A CREATE addresses the secret it is about to make, and its refusal names the `Name` the
/// request carried — the same reading, since that `Name` is the identifier the request
/// supplied.
#[tokio::test]
async fn a_refused_create_names_the_name_the_request_carried() {
    let action = "secretsmanager:CreateSecret";
    let reason = format!("because no identity-based policy allows the {action} action");
    for name in ["created-secret", "prod/db/created-secret"] {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        given_role_policy(
            &engine,
            Some(&allow_on(
                action,
                "arn:aws:secretsmanager:*:*:secret:elsewhere-*",
            )),
        );
        let case = format!("CreateSecret of {name:?}");
        let (status, answer) = call_as(
            &router,
            SESSION_KEY_ID,
            "CreateSecret",
            &request_body("CreateSecret", "unused-secret-id", name),
        )
        .await;
        assert_names_the_supplied_id(&case, status, &answer, action, name, &reason);
    }
}

/// THE INVARIANT the clause must not disturb: the DECISION still resolves the `SecretId`, so
/// a grant scoped to the ARN the account holds the secret under serves a request that
/// addresses it by any form of its id — the read comes back with the stored value.
#[tokio::test]
async fn the_decision_still_resolves_the_id_to_the_arn_a_statement_matches() {
    let action = "secretsmanager:GetSecretValue";
    let (router, engine) = router_with_identity();
    given_session(&engine);
    let arn = given_secret(&router, "granted-secret").await;
    let partial = partial_arn(&arn);
    // (what the row varies, the `Resource` the grant is scoped to)
    let scopes: &[(&str, String)] = &[
        ("the secret's full ARN", arn.clone()),
        ("a wildcard over the trailing characters", format!("{partial}-*")),
    ];
    // (what the row varies, the id the request supplies)
    let forms: &[(&str, &str)] = &[
        ("its name", "granted-secret"),
        ("its full ARN", arn.as_str()),
        ("its partial ARN", partial.as_str()),
    ];
    for (scope, resource) in scopes {
        given_role_policy(&engine, Some(&allow_on(action, resource)));
        for (form, supplied) in forms {
            let case = format!("a grant on {scope}, addressed by {form}");
            let (status, answer) = call_as(
                &router,
                SESSION_KEY_ID,
                "GetSecretValue",
                &json!({ "SecretId": supplied }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the grant covers this secret, got {status} {answer}"
            );
            assert_eq!(
                answer["SecretString"],
                json!(SECRET_VALUE),
                "case {case:?}: the allowed read must serve the stored value, got {answer}"
            );
        }
    }
}
