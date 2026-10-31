// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT A CLIENT TOKEN TIES: the request as the handler
//! reads it.
//!
//! The rule under test: a `clientToken` exists so that the retry of ONE request
//! is replayed instead of acted on twice, so the fingerprint the token ledger
//! compares is taken over the request as the handler READS it — every member the
//! handler normalises before acting carries the normalised value. Two spellings
//! of one request are therefore replayed under one token, while a request the
//! service acts on differently stays a different request and is refused
//! `IdempotentParameterMismatchException` (409) with the one fixed message that
//! refusal carries, having changed nothing.
//!
//! The table spans the class rather than one request: the two members this
//! service normalises — `accountIds`, read as the accounts it names each once
//! and in the order it first names them, driven through BOTH association
//! operations, and the `tagKeys` of `UntagResource`, read by membership — the
//! kind of re-spelling (an id or key repeated, repeats interleaved, a list of
//! the documented maximum size naming one account, a permuted key list) and both
//! directions, since either spelling may be the one a token is first used with.
//! The counterpart rows are requests that really do differ, including the
//! REORDERED account list, whose order an association answer reports back.
//!
//! Every case drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same router the emulator binary mounts, using the harness
//! [`crate::automation_rule_lifecycle`] owns.

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;

use crate::account_association::stored_enrollment;
use crate::account_id_list_rules::{
    first_named_once, reported_account_ids, ASSOCIATION_OPERATIONS,
};
use crate::automation_rule_lifecycle::{
    call, call_ok, emulator, emulator_with_engine, error_code, member, minimal_rule, string_member,
};
use crate::error_payloads::error_message;

/// Member account ids, each matching `AccountId`'s modelled `^[0-9]{12}$`, that
/// are not the calling account.
const MEMBER_A: &str = "111111111111";
const MEMBER_B: &str = "222222222222";
const MEMBER_C: &str = "333333333333";

/// A client token in the modelled `ClientToken` form
/// (`^[a-zA-Z0-9_-]{1,64}$`). Every row below runs on its own fresh store, so
/// one spelling of a token serves them all.
const TOKEN: &str = "request-spelling-token";

/// `n` distinct member account ids in the modelled `^[0-9]{12}$` form, for the
/// rows that re-spell a list of the documented maximum size.
fn member_account_ids(n: usize) -> Vec<String> {
    (0..n)
        .map(|index| format!("{:012}", 500_000_000_000u64 + index as u64))
        .collect()
}

fn owned(account_ids: &[&str]) -> Vec<String> {
    account_ids.iter().map(|id| (*id).to_string()).collect()
}

/// Every entry of `account_ids` spelled twice, one straight after the other —
/// the same accounts, in the same order of first naming.
fn each_twice(account_ids: &[String]) -> Vec<String> {
    account_ids
        .iter()
        .flat_map(|account_id| [account_id.clone(), account_id.clone()])
        .collect()
}

/// Pairs of `accountIds` spellings that name ONE request, as `(case, one
/// spelling, the other)`: the accounts each spelling names, and the order it
/// first names them in, are the same, so the two differ only in repeats.
///
/// The rows differ along what the rule generalises over: how many accounts are
/// named, where the repeats fall, how many times an id is repeated and how long
/// the list is — up to the documented maximum of `accountIds` ("You can specify
/// up to 50 account IDs", `state::MAX_ACCOUNT_IDS_PER_REQUEST`). The last row is
/// the plain retry, which must keep working.
fn one_request_two_spellings() -> Vec<(&'static str, Vec<String>, Vec<String>)> {
    let half_the_maximum = member_account_ids(state::MAX_ACCOUNT_IDS_PER_REQUEST / 2);
    vec![
        (
            "the-same-id-twice",
            owned(&[MEMBER_A, MEMBER_A]),
            owned(&[MEMBER_A]),
        ),
        (
            "repeats-interleaved",
            owned(&[MEMBER_A, MEMBER_B, MEMBER_A, MEMBER_B]),
            owned(&[MEMBER_A, MEMBER_B]),
        ),
        (
            "a-repeat-in-the-middle",
            owned(&[MEMBER_A, MEMBER_B, MEMBER_B, MEMBER_C]),
            owned(&[MEMBER_A, MEMBER_B, MEMBER_C]),
        ),
        (
            "the-documented-maximum-of-one-repeated-id",
            vec![MEMBER_A.to_string(); state::MAX_ACCOUNT_IDS_PER_REQUEST],
            owned(&[MEMBER_A]),
        ),
        (
            "the-documented-maximum-of-each-id-twice",
            each_twice(&half_the_maximum),
            half_the_maximum,
        ),
        (
            "the-identical-list",
            owned(&[MEMBER_A, MEMBER_B]),
            owned(&[MEMBER_A, MEMBER_B]),
        ),
    ]
}

/// Pairs of `accountIds` the service acts on DIFFERENTLY, as `(case, one
/// request, the other)`: a different set of accounts, or the same accounts in
/// another order of first naming — which an association answers in, so those
/// two really are two requests.
fn requests_that_really_differ() -> Vec<(&'static str, Vec<String>, Vec<String>)> {
    vec![
        (
            "the-accounts-reordered",
            owned(&[MEMBER_A, MEMBER_B]),
            owned(&[MEMBER_B, MEMBER_A]),
        ),
        (
            "the-accounts-reordered-with-repeats",
            owned(&[MEMBER_A, MEMBER_A, MEMBER_B]),
            owned(&[MEMBER_B, MEMBER_B, MEMBER_A]),
        ),
        (
            "an-account-added",
            owned(&[MEMBER_A]),
            owned(&[MEMBER_A, MEMBER_C]),
        ),
        (
            "another-account",
            owned(&[MEMBER_A, MEMBER_A]),
            owned(&[MEMBER_C]),
        ),
    ]
}

/// The accounts `account_ids` names, associated over the wire with no client
/// token, so a disassociation row has an association to remove and stamp.
///
/// A row's PRECONDITION: `AssociateAccounts`'s own behaviour is asserted in
/// [`crate::account_association`].
async fn associate_first(
    router: &smithy_server_runtime::MultiServiceRouter,
    account_ids: &[String],
    case: &str,
) {
    call_ok(
        router,
        "AssociateAccounts",
        &json!({"accountIds": account_ids}),
        case,
    )
    .await;
}

/// The stamp the store holds for `account_id`, as the rows compare it.
fn stamp_of(engine: &std::sync::Arc<engine::InMemoryEngine>, account_id: &str) -> Option<f64> {
    stored_enrollment(engine, account_id)
        .last_updated_timestamp
        .map(|at| at.as_secs_f64())
}

/// Two spellings of one association request are ONE request under one client
/// token: the second is replayed — the same accounts reported, in the same
/// order, and nothing acted on or stamped a second time — whichever of the two
/// spellings the token is first used with, and for BOTH operations that read an
/// `accountIds` list.
#[tokio::test]
async fn one_association_request_spelled_two_ways_replays_under_one_client_token() {
    for (row, one, other) in one_request_two_spellings() {
        for (first, retry) in [(&one, &other), (&other, &one)] {
            for operation in ASSOCIATION_OPERATIONS {
                let case = format!(
                    "{operation}/{row}: {} ids then {} ids",
                    first.len(),
                    retry.len()
                );
                let case = case.as_str();
                // One fresh store per row, so no row is judged against what
                // another left behind.
                let (router, engine) = emulator_with_engine();
                let expected = first_named_once(first);
                if operation == "DisassociateAccounts" {
                    associate_first(&router, first, case).await;
                }

                let answered = call_ok(
                    &router,
                    operation,
                    &json!({"accountIds": first, "clientToken": TOKEN}),
                    case,
                )
                .await;
                assert_eq!(
                    reported_account_ids(&answered),
                    expected,
                    "case {case:?}: expected each named account once in the order the request \
                     first names it, got {answered}"
                );
                let stamped = stamp_of(&engine, &expected[0]);

                // The retry: the same token, the other spelling of the same
                // request.
                let replay = call_ok(
                    &router,
                    operation,
                    &json!({"accountIds": retry, "clientToken": TOKEN}),
                    case,
                )
                .await;
                assert_eq!(
                    reported_account_ids(&replay),
                    expected,
                    "case {case:?}: the retry must answer the first call's result, expected \
                     {expected:?}, got {replay}"
                );
                assert_eq!(
                    stamp_of(&engine, &expected[0]),
                    stamped,
                    "case {case:?}: a replayed token must not act on the account a second time, \
                     expected the stamp {stamped:?}, got {:?}",
                    stamp_of(&engine, &expected[0])
                );
            }
        }
    }
}

/// A token first used with a request the service acts on differently — other
/// accounts, or the same accounts in another order — is still refused 409
/// `IdempotentParameterMismatchException` with the fixed message that refusal
/// carries, and the refused call stores nothing.
#[tokio::test]
async fn a_client_token_still_refuses_an_association_request_that_really_differs() {
    for (row, one, other) in requests_that_really_differ() {
        for (first, retry) in [(&one, &other), (&other, &one)] {
            for operation in ASSOCIATION_OPERATIONS {
                let case = format!("{operation}/{row}");
                let case = case.as_str();
                let (router, engine) = emulator_with_engine();
                let mut union: Vec<String> = first_named_once(first);
                for account_id in first_named_once(retry) {
                    if !union.contains(&account_id) {
                        union.push(account_id);
                    }
                }
                if operation == "DisassociateAccounts" {
                    associate_first(&router, &union, case).await;
                }

                call_ok(
                    &router,
                    operation,
                    &json!({"accountIds": first, "clientToken": TOKEN}),
                    case,
                )
                .await;
                let before: Vec<Option<f64>> = union
                    .iter()
                    .map(|account_id| stamp_of(&engine, account_id))
                    .collect();

                let (status, body) = call(
                    &router,
                    operation,
                    &json!({"accountIds": retry, "clientToken": TOKEN}),
                )
                .await;
                assert_eq!(
                    status, 409,
                    "case {case:?}: a token first used with another request is a conflict, got \
                     {status} and {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "IdempotentParameterMismatchException",
                    "case {case:?}: expected the parameter-mismatch refusal, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    state::IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
                    "case {case:?}: expected the fixed message \
                     {:?}, got {body}",
                    state::IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE
                );
                let after: Vec<Option<f64>> = union
                    .iter()
                    .map(|account_id| stamp_of(&engine, account_id))
                    .collect();
                assert_eq!(
                    after, before,
                    "case {case:?}: the refused call must have acted on nothing, expected the \
                     stamps {before:?} of {union:?}, got {after:?}"
                );
            }
        }
    }
}

/// The tags every untagging row starts from: three distinct keys inside the
/// modelled `Tag` constraints, so a row can remove one, two or another two.
fn three_tags() -> Value {
    json!([
        {"key": "team", "value": "platform"},
        {"key": "env", "value": "prod"},
        {"key": "tier", "value": "gold"},
    ])
}

/// A rule carrying [`three_tags`], as `(ruleArn, ruleRevision)`.
async fn tagged_rule(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    case: &str,
) -> (String, i64) {
    let mut request = minimal_rule(name);
    request["tags"] = three_tags();
    let created = call_ok(router, "CreateAutomationRule", &request, case).await;
    (
        string_member(&created, "ruleArn"),
        member(&created, "ruleRevision")
            .as_i64()
            .expect("a created rule carries a revision"),
    )
}

/// The tag keys `ListTagsForResource` still reports for `rule_arn`, in the order
/// it reports them.
async fn remaining_tag_keys(
    router: &smithy_server_runtime::MultiServiceRouter,
    rule_arn: &str,
    case: &str,
) -> Vec<String> {
    let body = call_ok(
        router,
        "ListTagsForResource",
        &json!({"resourceArn": rule_arn}),
        case,
    )
    .await;
    member(&body, "tags")
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    tag.get("key")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Pairs of `tagKeys` spellings, as `(case, one spelling, the other, whether the
/// two are one request)`.
///
/// `UntagResource` keeps every tag whose key the list does not CONTAIN, so the
/// list names a set: neither its order nor a key repeated in it changes what the
/// call removes or answers. A list naming other keys removes other tags and is
/// another request.
fn tag_key_spellings() -> Vec<(&'static str, Vec<String>, Vec<String>, bool)> {
    vec![
        (
            "the-keys-permuted",
            owned(&["team", "env"]),
            owned(&["env", "team"]),
            true,
        ),
        (
            "a-key-repeated",
            owned(&["team", "env"]),
            owned(&["env", "team", "team", "env"]),
            true,
        ),
        (
            "the-same-key-twice",
            owned(&["team", "team"]),
            owned(&["team"]),
            true,
        ),
        (
            "a-key-added",
            owned(&["team"]),
            owned(&["team", "env"]),
            false,
        ),
        ("other-keys", owned(&["team"]), owned(&["env"]), false),
    ]
}

/// The same rule over `tagKeys`: two spellings of one key SET are one request
/// and the second is replayed, while a list naming other keys is another request
/// and is refused — in both directions, and with the stored tag list unchanged
/// by the second call either way.
#[tokio::test]
async fn one_untagging_spelled_two_ways_replays_under_one_client_token() {
    for (row, one, other, same_request) in tag_key_spellings() {
        for (first, retry) in [(&one, &other), (&other, &one)] {
            let case = format!("UntagResource/{row}: {first:?} then {retry:?}");
            let case = case.as_str();
            let router = emulator();
            let (rule_arn, revision) = tagged_rule(&router, "token-spellings", case).await;

            call_ok(
                &router,
                "UntagResource",
                &json!({
                    "resourceArn": &rule_arn,
                    "ruleRevision": revision,
                    "tagKeys": first,
                    "clientToken": TOKEN,
                }),
                case,
            )
            .await;
            let left = remaining_tag_keys(&router, &rule_arn, case).await;

            let (status, body) = call(
                &router,
                "UntagResource",
                &json!({
                    "resourceArn": &rule_arn,
                    "ruleRevision": revision,
                    "tagKeys": retry,
                    "clientToken": TOKEN,
                }),
            )
            .await;
            if same_request {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the two spellings name one key set, so the second call is a \
                     replay, got {status} and {body}"
                );
                // `UntagResourceOutput` models no members, so a replay answers
                // the empty document this operation always answers with.
                assert_eq!(
                    body,
                    json!({}),
                    "case {case:?}: the replay must answer the operation's modelled empty output, \
                     got {body}"
                );
            } else {
                assert_eq!(
                    status, 409,
                    "case {case:?}: a token first used to remove other keys is a conflict, got \
                     {status} and {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "IdempotentParameterMismatchException",
                    "case {case:?}: expected the parameter-mismatch refusal, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    state::IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE,
                    "case {case:?}: expected the fixed message {:?}, got {body}",
                    state::IDEMPOTENT_PARAMETER_MISMATCH_MESSAGE
                );
            }
            let now_left = remaining_tag_keys(&router, &rule_arn, case).await;
            assert_eq!(
                now_left, left,
                "case {case:?}: the second call must leave the first call's tag list, expected \
                 {left:?}, got {now_left:?}"
            );
        }
    }
}
