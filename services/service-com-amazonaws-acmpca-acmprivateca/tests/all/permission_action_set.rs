// SPDX-License-Identifier: CC0-1.0

//! The action set a grant on a private CA has to name, and what a repeat of a grant does.
//!
//! The rules under test:
//!
//! 1. `CreatePermission` records a grant only when it names EVERY `ActionType` member; a
//!    request naming only part of that set is refused with HTTP 400 `ValidationException`
//!    and the service's fixed sentence, and stores nothing.
//! 2. `CreatePermission` is idempotent over the (CA, principal, source account) a grant is
//!    keyed by: re-asserting the grant already in effect answers 200 with an empty body and
//!    leaves the grant exactly as it was, whatever order (and with whatever repeats) the
//!    request lists the actions in.
//! 3. `ListPermissions` then reports that grant with every granted action, and derives the
//!    policy document's `Action` array from the same set.
//!
//! The harness — the router, the signed request and the success/refusal helpers — is the
//! one `private_ca_lifecycle` drives the rest of this service with; only the cases here
//! are new.

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::ops::types::ACTION_TYPE_VALUES;
use service_com_amazonaws_acmpca_acmprivateca::state::{
    iam_action, ACTIONS_IN_MODEL_ORDER, INCOMPLETE_PERMISSION_ACTIONS_MESSAGE,
};

use crate::policy_documents::in_reported_order;
use crate::private_ca_lifecycle::{root_ca_request, string, Emulator, ACCOUNT};
use crate::resource_policy::error_message;

/// The error code a refusal of an incomplete grant carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// One idempotence row: the case, the principal that holds the grant, the account that
/// assigned it (when the request names one), the actions the grant named and the actions
/// the repeat of it names.
type RepeatRow<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    &'a [&'a str],
    &'a [&'a str],
);

/// Create a private CA and return its ARN.
async fn certificate_authority(emulator: &Emulator, case: &str) -> String {
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &root_ca_request(case))
        .await;
    string(&created, "CertificateAuthorityArn", case)
}

/// A `CreatePermission` body naming `actions`, with a source account when the case has one.
fn grant(arn: &str, principal: &str, source_account: Option<&str>, actions: &[&str]) -> Value {
    let mut body = json!({
        "CertificateAuthorityArn": arn,
        "Principal": principal,
        "Actions": actions,
    });
    if let Some(account) = source_account {
        body["SourceAccount"] = json!(account);
    }
    body
}

/// Every permission one CA holds, in a single page.
async fn all_permissions(emulator: &Emulator, case: &str, arn: &str) -> Vec<Value> {
    let page = emulator
        .ok(
            case,
            "ListPermissions",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    page.get("Permissions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no Permissions list in {page:?}"))
        .clone()
}

/// The permission a list reports for `principal` and `source_account`, if any.
fn reported<'a>(
    permissions: &'a [Value],
    principal: &str,
    source_account: &str,
) -> Option<&'a Value> {
    permissions.iter().find(|permission| {
        permission["Principal"] == json!(principal)
            && permission["SourceAccount"] == json!(source_account)
    })
}

/// Every action a grant has to name, in the order a `Permission` reports them
/// (`policy_documents::in_reported_order`, the one place that rule lives).
fn every_action_in_reported_order() -> Vec<String> {
    in_reported_order(ACTIONS_IN_MODEL_ORDER)
}

/// The action lists that are NOT the full set: every proper subset of the actions a grant
/// can name, including the empty-but-padded ones a repeat of one member makes.
fn incomplete_action_lists() -> Vec<(String, Vec<&'static str>)> {
    let mut rows: Vec<(String, Vec<&'static str>)> = Vec::new();
    // Every proper subset of the three members, by the bitmask of which ones it names.
    // `ActionList` is len 1..3, so the empty subset never reaches the handler.
    for mask in 1u8..((1 << ACTIONS_IN_MODEL_ORDER.len()) - 1) {
        let named: Vec<&'static str> = ACTIONS_IN_MODEL_ORDER
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, action)| *action)
            .collect();
        rows.push((format!("the subset {named:?}"), named));
    }
    // Three elements, so the list is as LONG as a complete grant, and still incomplete:
    // a member repeated is one action, not two.
    rows.push((
        "a member repeated to the list's maximum length".to_string(),
        vec![
            ACTIONS_IN_MODEL_ORDER[0],
            ACTIONS_IN_MODEL_ORDER[0],
            ACTIONS_IN_MODEL_ORDER[0],
        ],
    ));
    rows.push((
        "two members, one of them repeated".to_string(),
        vec![
            ACTIONS_IN_MODEL_ORDER[2],
            ACTIONS_IN_MODEL_ORDER[1],
            ACTIONS_IN_MODEL_ORDER[2],
        ],
    ));
    rows
}

/// A grant that does not name every action is refused with the service's fixed sentence,
/// and nothing is stored — whichever part of the set it leaves out, and for whichever
/// principal and source account it is made.
#[tokio::test]
async fn a_grant_that_names_less_than_every_action_is_refused_and_stores_nothing() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "incomplete").await;

    // The rows differ along which actions the grant names, how many it names, whether one
    // is repeated, which principal holds it and whether the request names a source
    // account: the rule is about the SET of actions and nothing else.
    let holders: [(&str, Option<&str>); 3] = [
        ("acm.amazonaws.com", None),
        ("acm.amazonaws.com", Some("123456789012")),
        ("ram.amazonaws.com", Some("210987654321")),
    ];

    for (principal, source_account) in holders {
        for (actions_case, actions) in incomplete_action_lists() {
            let case = format!("{principal} / {source_account:?}: {actions_case}");
            let refusal = emulator
                .refused(
                    &case,
                    "CreatePermission",
                    &grant(&arn, principal, source_account, &actions),
                    VALIDATION_EXCEPTION,
                )
                .await;
            let message = error_message(&refusal, &case);
            assert_eq!(
                message, INCOMPLETE_PERMISSION_ACTIONS_MESSAGE,
                "case {case:?}: expected {INCOMPLETE_PERMISSION_ACTIONS_MESSAGE:?}, \
                 got {message:?}"
            );

            // The refusal is before any write: the CA holds no grant at all.
            let permissions = all_permissions(&emulator, &case, &arn).await;
            assert!(
                permissions.is_empty(),
                "case {case:?}: a refused grant must store nothing, got {permissions:?}"
            );
        }
    }
}

/// The sentence a refused grant carries names every action the model declares, in the
/// order the model declares them, and nothing of the request.
#[tokio::test]
async fn the_refusal_sentence_names_every_action_the_model_declares() {
    // `ACTION_TYPE_VALUES` is generated from `ActionType` (and sorted); the sentence lists
    // the same members in the model's declaration order, so neither list can drift from
    // the model without this failing.
    let mut declared: Vec<&str> = ACTION_TYPE_VALUES.to_vec();
    declared.sort();
    let mut stated: Vec<&str> = ACTIONS_IN_MODEL_ORDER.to_vec();
    stated.sort();
    assert_eq!(
        stated, declared,
        "case \"the stated set\": expected the model's own ActionType members, got {stated:?}"
    );

    // The sentence is FIXED text, so it is pinned as recorded rather than rebuilt from the
    // constants it is written from: a drift in any word of it fails here.
    assert_eq!(
        INCOMPLETE_PERMISSION_ACTIONS_MESSAGE,
        "Permissions must contain all three actions [IssueCertificate, GetCertificate, \
         ListPermissions] for ACM to perform renewals.",
        "case \"the sentence\": expected the recorded sentence, got \
         {INCOMPLETE_PERMISSION_ACTIONS_MESSAGE:?}"
    );
    // And it lists the actions in the order the model declares them.
    assert!(
        INCOMPLETE_PERMISSION_ACTIONS_MESSAGE
            .contains(&format!("[{}]", ACTIONS_IN_MODEL_ORDER.join(", "))),
        "case \"the stated order\": expected the actions in the model's order, got \
         {INCOMPLETE_PERMISSION_ACTIONS_MESSAGE:?}"
    );
}

/// A grant naming every action is recorded, and asking again for the grant already in
/// effect answers success and changes nothing it reports.
#[tokio::test]
async fn re_asserting_the_grant_already_in_effect_is_idempotent() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "idempotent").await;

    // The rows differ along the order the request lists the complete set in, whether a
    // member is repeated inside it, which principal holds the grant and whether the
    // request names a source account.
    let rows: [RepeatRow; 4] = [
        (
            "the actions in the model's order, no source account",
            "acm.amazonaws.com",
            None,
            &[
                ACTIONS_IN_MODEL_ORDER[0],
                ACTIONS_IN_MODEL_ORDER[1],
                ACTIONS_IN_MODEL_ORDER[2],
            ],
            &[
                ACTIONS_IN_MODEL_ORDER[0],
                ACTIONS_IN_MODEL_ORDER[1],
                ACTIONS_IN_MODEL_ORDER[2],
            ],
        ),
        (
            "the repeat lists the same actions in another order",
            "acm.amazonaws.com",
            Some("123456789012"),
            &[
                ACTIONS_IN_MODEL_ORDER[0],
                ACTIONS_IN_MODEL_ORDER[1],
                ACTIONS_IN_MODEL_ORDER[2],
            ],
            &[
                ACTIONS_IN_MODEL_ORDER[2],
                ACTIONS_IN_MODEL_ORDER[0],
                ACTIONS_IN_MODEL_ORDER[1],
            ],
        ),
        (
            "the grant was made in reverse order",
            "ram.amazonaws.com",
            Some("210987654321"),
            &[
                ACTIONS_IN_MODEL_ORDER[2],
                ACTIONS_IN_MODEL_ORDER[1],
                ACTIONS_IN_MODEL_ORDER[0],
            ],
            &[
                ACTIONS_IN_MODEL_ORDER[1],
                ACTIONS_IN_MODEL_ORDER[2],
                ACTIONS_IN_MODEL_ORDER[0],
            ],
        ),
        (
            "the same principal under another source account",
            "ram.amazonaws.com",
            Some("123456789012"),
            &[
                ACTIONS_IN_MODEL_ORDER[1],
                ACTIONS_IN_MODEL_ORDER[0],
                ACTIONS_IN_MODEL_ORDER[2],
            ],
            &[
                ACTIONS_IN_MODEL_ORDER[1],
                ACTIONS_IN_MODEL_ORDER[0],
                ACTIONS_IN_MODEL_ORDER[2],
            ],
        ),
    ];

    for (case, principal, source_account, granted, repeated) in rows {
        let account = source_account.unwrap_or(ACCOUNT);
        let answer = emulator
            .ok(
                case,
                "CreatePermission",
                &grant(&arn, principal, source_account, granted),
            )
            .await;
        assert!(
            answer.as_object().is_none_or(|members| members.is_empty()),
            "case {case:?}: CreatePermission models no output members, got {answer:?}"
        );
        let first = reported(&all_permissions(&emulator, case, &arn).await, principal, account)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the grant must be listed"));

        // The repeat asks for exactly the access that already holds, twice over.
        for attempt in 1..=2 {
            let case = format!("{case}: repeat {attempt}");
            let answer = emulator
                .ok(
                    &case,
                    "CreatePermission",
                    &grant(&arn, principal, source_account, repeated),
                )
                .await;
            assert!(
                answer.as_object().is_none_or(|members| members.is_empty()),
                "case {case:?}: an idempotent repeat answers an empty body, got {answer:?}"
            );
            let permissions = all_permissions(&emulator, &case, &arn).await;
            let again = reported(&permissions, principal, account)
                .unwrap_or_else(|| panic!("case {case:?}: the grant must still be listed"));
            assert_eq!(
                again, &first,
                "case {case:?}: a repeat must leave the grant as it was, got {again:?}"
            );
            // One grant per (CA, principal, source account): a repeat records no second one.
            assert_eq!(
                permissions
                    .iter()
                    .filter(|permission| {
                        permission["Principal"] == json!(principal)
                            && permission["SourceAccount"] == json!(account)
                    })
                    .count(),
                1,
                "case {case:?}: a repeat must not record a second grant, got {permissions:?}"
            );
        }
    }

    // Every grant the rows made is still exactly one grant, each reporting every action and
    // a policy whose `Action` array is derived from that same set.
    let permissions = all_permissions(&emulator, "idempotent", &arn).await;
    assert_eq!(
        permissions.len(),
        rows.len(),
        "case \"idempotent\": expected one permission per grant, got {permissions:?}"
    );
    let expected_actions = every_action_in_reported_order();
    // The IAM action an `ActionType` member denotes is the service's own derivation
    // (`state::iam_action`), so the expectation names it rather than re-spelling the prefix.
    let expected_namespaced: Vec<String> = expected_actions
        .iter()
        .map(|action| iam_action(action))
        .collect();
    for (case, principal, source_account, _, _) in rows {
        let account = source_account.unwrap_or(ACCOUNT);
        let permission = reported(&permissions, principal, account)
            .unwrap_or_else(|| panic!("case {case:?}: no grant for {principal} / {account}"));
        assert_eq!(
            permission["Actions"],
            json!(expected_actions),
            "case {case:?}: expected every granted action, got {permission:?}"
        );
        let policy = permission
            .get("Policy")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no Policy member in {permission:?}"));
        let document: Value = serde_json::from_str(policy)
            .unwrap_or_else(|_| panic!("case {case:?}: Policy must be a JSON document: {policy:?}"));
        assert_eq!(
            document["Statement"][0]["Action"],
            json!(expected_namespaced),
            "case {case:?}: expected the granted actions as {expected_namespaced:?}, \
             got {document:?}"
        );
        assert_eq!(
            document["Statement"][0]["Principal"],
            json!({"Service": principal}),
            "case {case:?}: expected the holder as the statement's principal, got {document:?}"
        );
        assert_eq!(
            document["Statement"][0]["Condition"],
            json!({"StringEquals": {"aws:SourceAccount": account}}),
            "case {case:?}: expected the assigning account as the condition, got {document:?}"
        );
    }
}

/// A grant refused for its action set leaves the grant that triple already holds alone, and
/// the complete grant it was supposed to be still succeeds afterwards.
#[tokio::test]
async fn a_refused_grant_neither_replaces_nor_blocks_the_complete_one() {
    let emulator = Emulator::new();
    let arn = certificate_authority(&emulator, "refused-then-granted").await;
    let principal = "acm.amazonaws.com";
    let complete: Vec<&str> = ACTIONS_IN_MODEL_ORDER.to_vec();

    // Incomplete first: nothing is stored, so the complete grant that follows is accepted
    // rather than refused as an existing one.
    emulator
        .refused(
            "incomplete first",
            "CreatePermission",
            &grant(&arn, principal, None, &complete[..2]),
            VALIDATION_EXCEPTION,
        )
        .await;
    emulator
        .ok(
            "complete after the refusal",
            "CreatePermission",
            &grant(&arn, principal, None, &complete),
        )
        .await;

    // Incomplete again, with the complete grant now in effect: the refusal is the same one
    // and the grant in effect is untouched.
    let before = all_permissions(&emulator, "before", &arn).await;
    emulator
        .refused(
            "incomplete after the grant",
            "CreatePermission",
            &grant(&arn, principal, None, &complete[1..]),
            VALIDATION_EXCEPTION,
        )
        .await;
    let after = all_permissions(&emulator, "after", &arn).await;
    assert_eq!(
        after, before,
        "case \"incomplete after the grant\": the grant in effect must stand, got {after:?}"
    );
    let permission = reported(&after, principal, ACCOUNT)
        .unwrap_or_else(|| panic!("case \"after\": the grant must be listed, got {after:?}"));
    assert_eq!(
        permission["Actions"],
        json!(every_action_in_reported_order()),
        "case \"after\": expected every granted action, got {permission:?}"
    );
}
