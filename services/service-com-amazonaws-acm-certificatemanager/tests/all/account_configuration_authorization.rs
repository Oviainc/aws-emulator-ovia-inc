// SPDX-License-Identifier: CC0-1.0

//! WHO may read and write this account's expiry-events configuration, driven end to end
//! over the wire (an awsJson1_1 request through the multi-service router into the mounted
//! service), so what is asserted is what a client holding those credentials sees.
//!
//! THE RULE: an operation whose model declares `AccessDeniedException` evaluates the
//! identity-based IAM policies in force on the principal its credentials name against
//! `acm:<OperationName>` BEFORE it reads or writes anything; an account-level action names
//! no resource, so it is judged against the resource IAM writes for such a request (`*`),
//! and a request no statement allows — or one an explicit `Deny` covers — is answered HTTP
//! 403 `AccessDeniedException` and performs nothing, while a request a matching `Allow`
//! covers reaches the operation's own answer.
//!
//! The account-configuration pair is what this suite holds to that rule: the read
//! (`GetAccountConfiguration`) and the write (`PutAccountConfiguration`), each judged
//! against its OWN action. The acme suite pins the same rule across this service's
//! resource-scoped operations; here the interest is the account-level pair, where there is
//! no resource to name, and what a REFUSED call must leave behind — an unchanged
//! configuration and an unspent idempotency token.
//!
//! The role, its policies and the session live in the IAM and STS stores of the SAME
//! shared engine, so the documents under test are the ones `iam:PutRolePolicy` stores and
//! the principal a refusal blames is the one `sts:GetCallerIdentity` answers for the key
//! the request signed with.
//!
//! The tables span what the rule generalises over: the operation (the account-level read
//! and the account-level write), the document in force (none at all, another action of the
//! same pair, another service's actions, this action scoped to a resource collection
//! instead of `*`, an explicit deny beside an allow, an allow of exactly the action, of the
//! action in a list, of the action spelled in another case, of the whole service and of
//! everything) and the principal (an assumed-role session and the account's own published
//! credential).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_authorization::{
    allow, call_as, given_role_policy, given_session, identity_documents, router_with_identity,
    session_arn, Refusal, ACCOUNT_KEY_ID, SESSION_KEY_ID,
};
use crate::certificate_lifecycle::{error_code, error_message, ACCOUNT, REGION};

/// The account-level operations of this service: both model `AccessDeniedException`
/// (HTTP 403) and neither carries a resource member
/// (`smithy_query(query="operation", operation="GetAccountConfiguration,PutAccountConfiguration")`).
const ACCOUNT_CONFIGURATION_OPERATIONS: &[&str] =
    &["GetAccountConfiguration", "PutAccountConfiguration"];

/// The `DaysBeforeExpiry` the baseline write puts in place before each case, so what a
/// refused call must leave behind is a value no case asks for.
const BASELINE_DAYS: i64 = 30;

/// `DaysBeforeExpiry` values an accepted write may carry: the target `PositiveInteger`
/// declares `@range(min: 1)`
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#ExpiryEventsConfiguration")`) and
/// ACM refuses anything over its default 45-day notice window, so these walk that span
/// including both published bounds. None of them is [`BASELINE_DAYS`], so a write that was
/// applied cannot be mistaken for one that was not.
const REQUESTED_DAYS: &[i64] = &[1, 7, 15, 22, 44, 45];

/// The collection ARN one row scopes its allow to: this account's certificates in the
/// Region the caller signs for. An account-level action addresses no resource, so a
/// statement naming this collection is not about it.
fn certificate_collection() -> String {
    format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/*")
}

/// The request body a case sends `operation`.
fn request_body(operation: &str, token: &str, days: i64) -> Value {
    match operation {
        // `smithy.api#Unit` input: there is nothing to send.
        "GetAccountConfiguration" => json!({}),
        "PutAccountConfiguration" => json!({
            "ExpiryEvents": { "DaysBeforeExpiry": days },
            "IdempotencyToken": token,
        }),
        other => panic!("case {other:?}: this suite states no request for that operation"),
    }
}

/// The documents a case puts in force on the role, and which of IAM's refusals the request
/// then meets — `None` for a request a matching `Allow` permits.
///
/// The rows every suite of this crate shares come from
/// [`crate::acme_authorization::identity_documents`], so the two tables cannot drift; the
/// rest is what this account-level pair adds to the class: an action of another service, an
/// allow scoped to a resource collection an account-level action is not covered by, a deny
/// of the whole service, and the spellings a grant may take.
fn documents_in_force(
    action: &str,
    other_action: &str,
) -> Vec<(&'static str, Option<Value>, Option<Refusal>)> {
    let mut rows = identity_documents(action, &[other_action.to_string()]);
    rows.extend([
        (
            "an allow of every action of another service",
            Some(allow(json!("s3:*"))),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an allow of this action scoped to this account's certificates",
            Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": certificate_collection() },
                ],
            })),
            Some(Refusal::DefaultDeny),
        ),
        (
            "an explicit deny of this whole service beside an allow of exactly this action",
            Some(json!({
                "Statement": [
                    { "Effect": "Allow", "Action": action, "Resource": "*" },
                    { "Effect": "Deny", "Action": "acm:*", "Resource": "*" },
                ],
            })),
            Some(Refusal::ExplicitDeny),
        ),
        (
            // IAM compares action names case-insensitively, so the spelling of the allow is
            // not what decides the request.
            "an allow of this action spelled in lower case",
            Some(allow(json!(action.to_lowercase()))),
            None,
        ),
        (
            "an allow listing this action beside another service's",
            Some(allow(json!(["s3:GetObject", action]))),
            None,
        ),
        ("an allow of every action", Some(allow(json!("*"))), None),
    ]);
    rows
}

/// Put `days` in place with the account's own credential, which this gate serves: the state
/// every case starts from.
async fn given_configured_days(router: &MultiServiceRouter, token: &str, days: i64) {
    let (status, answer) = call_as(
        router,
        ACCOUNT_KEY_ID,
        "PutAccountConfiguration",
        &json!({
            "ExpiryEvents": { "DaysBeforeExpiry": days },
            "IdempotencyToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {token:?}: the baseline write must succeed, got {status} {answer}"
    );
}

/// What the store holds now, read with the account's own credential.
async fn configured_days(router: &MultiServiceRouter, case: &str) -> Value {
    let (status, answer) =
        call_as(router, ACCOUNT_KEY_ID, "GetAccountConfiguration", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the account's own credential must be served the configuration, got \
         {status} {answer}"
    );
    answer["ExpiryEvents"]["DaysBeforeExpiry"].clone()
}

/// THE RULE, for both account-level operations: each is judged against its own action
/// before it reads or writes anything, a refusal is HTTP 403 `AccessDeniedException`
/// naming the principal and the action with no resource clause, a refused call leaves the
/// stored configuration exactly as it was, and an allowed call reaches the operation's own
/// answer.
#[tokio::test]
async fn both_account_level_operations_are_authorized_against_their_own_action() {
    for operation in ACCOUNT_CONFIGURATION_OPERATIONS {
        let action = format!("acm:{operation}");
        let other_action = ACCOUNT_CONFIGURATION_OPERATIONS
            .iter()
            .find(|other| *other != operation)
            .map(|other| format!("acm:{other}"))
            .expect("the pair holds another operation");

        for (index, (row, document, refused)) in
            documents_in_force(&action, &other_action).iter().enumerate()
        {
            let case = format!("{operation} with {row}");
            let days = REQUESTED_DAYS[index % REQUESTED_DAYS.len()];

            // A fresh store, session and baseline per row: no row can pass on another's
            // write, and what a refusal must leave behind is a value this row did not ask
            // for.
            let (router, engine) = router_with_identity();
            given_session(&engine);
            given_configured_days(&router, "baseline", BASELINE_DAYS).await;
            given_role_policy(&engine, document.as_ref());

            let token = format!("case{index}");
            let body = request_body(operation, &token, days);
            let (status, answer) = call_as(&router, SESSION_KEY_ID, operation, &body).await;

            match refused {
                Some(refusal) => {
                    assert_eq!(
                        status, 403,
                        "case {case:?}: expected HTTP 403, got {status} {answer}"
                    );
                    assert_eq!(
                        error_code(&answer),
                        "AccessDeniedException",
                        "case {case:?}: expected an AccessDeniedException, got {answer}"
                    );
                    assert_eq!(
                        error_message(&answer),
                        format!(
                            "User: {} is not authorized to perform: {action} {}",
                            session_arn(),
                            refusal.reason(&action)
                        ),
                        "case {case:?}: expected IAM's wording, got {answer}"
                    );
                    if *operation == "GetAccountConfiguration" {
                        // The refusal carries no part of the read's own answer: an
                        // unauthorized caller is told nothing about this account.
                        assert!(
                            answer["ExpiryEvents"].is_null(),
                            "case {case:?}: a refused read must report no configuration, got \
                             {answer}"
                        );
                    }
                }
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: an allowed request must be served, got {status} {answer}"
                    );
                    if *operation == "GetAccountConfiguration" {
                        assert_eq!(
                            answer["ExpiryEvents"]["DaysBeforeExpiry"],
                            json!(BASELINE_DAYS),
                            "case {case:?}: the read must report the stored configuration, got \
                             {answer}"
                        );
                    }
                }
            }

            // What the store holds after the call: only an ALLOWED write changed it.
            let expected = if *operation == "PutAccountConfiguration" && refused.is_none() {
                days
            } else {
                BASELINE_DAYS
            };
            let seen = configured_days(&router, &case).await;
            assert_eq!(
                seen,
                json!(expected),
                "case {case:?}: expected the store to hold {expected} days, got {seen}"
            );
        }
    }
}

/// The account's own published credential holds no IAM policies, so it is served both
/// operations while the session carrying no policy is refused either — the decision is
/// about the principal, not about the operation.
#[tokio::test]
async fn the_accounts_own_credential_is_served_what_a_permissionless_session_is_refused() {
    for (index, operation) in ACCOUNT_CONFIGURATION_OPERATIONS.iter().enumerate() {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        given_role_policy(&engine, None);

        let days = REQUESTED_DAYS[index % REQUESTED_DAYS.len()];
        let (status, answer) = call_as(
            &router,
            SESSION_KEY_ID,
            operation,
            &request_body(operation, "session", days),
        )
        .await;
        assert_eq!(
            status, 403,
            "case {operation:?} as a permissionless session: expected HTTP 403, got {status} \
             {answer}"
        );

        let (status, answer) = call_as(
            &router,
            ACCOUNT_KEY_ID,
            operation,
            &request_body(operation, "account", days),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {operation:?} as the account's own credential: expected HTTP 200, got {status} \
             {answer}"
        );
    }
}

/// A refused write performs NOTHING: it leaves the configuration the last allowed write
/// put in place, and it spends no idempotency token — so the very same request, once the
/// principal is allowed it, is a NEW request and is applied.
#[tokio::test]
async fn a_refused_write_changes_nothing_and_spends_no_idempotency_token() {
    for days in REQUESTED_DAYS {
        let (router, engine) = router_with_identity();
        given_session(&engine);
        given_configured_days(&router, "baseline", BASELINE_DAYS).await;
        given_role_policy(&engine, None);

        let token = "retried";
        let body = request_body("PutAccountConfiguration", token, *days);
        let (status, answer) =
            call_as(&router, SESSION_KEY_ID, "PutAccountConfiguration", &body).await;
        assert_eq!(
            status, 403,
            "case {days}: a write no policy allows must be refused, got {status} {answer}"
        );
        let seen = configured_days(&router, &format!("{days} refused")).await;
        assert_eq!(
            seen,
            json!(BASELINE_DAYS),
            "case {days}: a refused write must leave the stored configuration, got {seen}"
        );

        // The same token and the same values, now allowed: a token the refused call never
        // recorded makes this a new request, so it is APPLIED rather than answered as a
        // repeat of one already processed.
        given_role_policy(&engine, Some(&allow(json!("acm:PutAccountConfiguration"))));
        let (status, answer) =
            call_as(&router, SESSION_KEY_ID, "PutAccountConfiguration", &body).await;
        assert_eq!(
            status, 200,
            "case {days}: the allowed retry must succeed, got {status} {answer}"
        );
        let seen = configured_days(&router, &format!("{days} allowed")).await;
        assert_eq!(
            seen,
            json!(days),
            "case {days}: the allowed retry must be applied, got {seen}"
        );
    }
}

/// A refused read reports nothing about the account and changes nothing: every later
/// caller sees exactly the configuration the last allowed write left, however many
/// refusals came in between.
#[tokio::test]
async fn a_refused_read_discloses_nothing_and_leaves_the_store_unchanged() {
    let (router, engine) = router_with_identity();
    given_session(&engine);
    given_configured_days(&router, "baseline", BASELINE_DAYS).await;
    given_role_policy(&engine, None);

    for attempt in 1..=3 {
        let (status, answer) = call_as(
            &router,
            SESSION_KEY_ID,
            "GetAccountConfiguration",
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 403,
            "case attempt {attempt}: a read no policy allows must be refused, got {status} \
             {answer}"
        );
        assert!(
            answer["ExpiryEvents"].is_null(),
            "case attempt {attempt}: a refused read must report no configuration, got {answer}"
        );
        let seen = configured_days(&router, &format!("attempt {attempt}")).await;
        assert_eq!(
            seen,
            json!(BASELINE_DAYS),
            "case attempt {attempt}: a refused read must change nothing, got {seen}"
        );
    }
}
