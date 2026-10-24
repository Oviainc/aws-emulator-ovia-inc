// SPDX-License-Identifier: CC0-1.0

//! What becomes of an invited member account once Detective has verified it.
//!
//! The rule under test, from `MemberDetail$Status`: `VERIFICATION_IN_PROGRESS` means
//! "Detective is verifying that the account identifier and email address provided for
//! the member account match. If they do match, then Detective sends the invitation. If
//! the email address and account identifier don't match, then the member cannot be added
//! to the behavior graph", and `VERIFICATION_FAILED` means "the account and email address
//! provided for the member account do not match, and Detective did not send an invitation
//! to the account". So the transient status is left for a TERMINAL one at an instant the
//! clock owes, independently for each account in a batch, and the outcome is decided by
//! whether the pair matches a registered root-user email address. This emulator registers
//! none (`crate::state::registered_root_email`), so every invited account settles at
//! `VERIFICATION_FAILED` — and a client polling for it is never left waiting forever.
//!
//! Every case drives the real wire path: the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_detective_amazondetective::state::MEMBER_VERIFICATION_MILLIS;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{account_id, assert_iso8601, call, create_graph_ok, emulator, REGION};

/// One `Accounts` entry: an account id and the email address given for it.
fn entry(account_id: &str, email: &str) -> Value {
    json!({ "AccountId": account_id, "EmailAddress": email })
}

/// The `MemberDetails` of one `GetMembers` call, keyed by account id.
async fn get_members(
    router: &MultiServiceRouter,
    graph_arn: &str,
    account_ids: &[String],
) -> BTreeMap<String, Value> {
    let (status, got) = call(
        router,
        "/graph/members/get",
        &json!({ "GraphArn": graph_arn, "AccountIds": account_ids }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetMembers({account_ids:?}): expected 200, got {status} {got:?}"
    );
    assert_eq!(
        got.get("UnprocessedAccounts").and_then(Value::as_array),
        Some(&Vec::new()),
        "GetMembers({account_ids:?}): expected every account processed, got {got:?}"
    );
    got.get("MemberDetails")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("GetMembers carried no MemberDetails, got {got:?}"))
        .iter()
        .map(|detail| {
            (
                detail
                    .get("AccountId")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("a member detail carried no AccountId: {detail:?}"))
                    .to_string(),
                detail.clone(),
            )
        })
        .collect()
}

/// One row of the verification table: a batch of `Accounts` entries, and the rest of the
/// `CreateMembers` request body it is sent in.
struct BatchCase {
    name: &'static str,
    /// `(account id, email address)` pairs, in the order the request lists them.
    accounts: Vec<(String, String)>,
    /// `DisableEmailNotification`, which suppresses the invitation email only.
    disable_email_notification: bool,
    /// An optional `Message` (modelled `EmailMessage`, length 1..1000).
    message: Option<&'static str>,
}

/// The table. The rule generalises over how many accounts one call invites and over the
/// form of each account's id and email address, so the rows differ along the batch size
/// (one, the three the recorded batch used, and ten), the email local part, domain and
/// letter case, and whether the request asks for an email at all.
fn batch_cases() -> Vec<BatchCase> {
    vec![
        BatchCase {
            name: "one-account",
            accounts: vec![(account_id(0), "member@example.com".to_string())],
            disable_email_notification: true,
            message: None,
        },
        BatchCase {
            name: "three-accounts-varied-emails",
            accounts: vec![
                (account_id(1), "plain@example.com".to_string()),
                (account_id(2), "dotted.local.part@mail.example.org".to_string()),
                (account_id(3), "MiXeD.Case@Example.COM".to_string()),
            ],
            disable_email_notification: true,
            message: None,
        },
        BatchCase {
            name: "batch-of-ten-with-an-invitation-message",
            accounts: (10..20)
                .map(|n| (account_id(n), format!("member-{n}@example.net")))
                .collect(),
            disable_email_notification: false,
            message: Some("Join this behavior graph"),
        },
    ]
}

#[tokio::test]
async fn every_invited_account_leaves_verification_for_verification_failed_independently() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    let mut invited: Vec<String> = Vec::new();

    for case in batch_cases() {
        let name = case.name;
        let accounts: Vec<Value> = case
            .accounts
            .iter()
            .map(|(id, email)| entry(id, email))
            .collect();
        let mut body = json!({
            "GraphArn": graph_arn,
            "Accounts": accounts,
            "DisableEmailNotification": case.disable_email_notification,
        });
        if let Some(message) = case.message {
            body["Message"] = json!(message);
        }

        let (status, created) = call(&router, "/graph/members", &body).await;
        assert_eq!(
            status, 200,
            "case {name:?}: CreateMembers expected 200, got {status} {created:?}"
        );
        let members = created
            .get("Members")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {name:?}: no Members in {created:?}"));
        assert_eq!(
            members.len(),
            case.accounts.len(),
            "case {name:?}: expected one member per account, got {members:?}"
        );
        assert_eq!(
            created.get("UnprocessedAccounts").and_then(Value::as_array),
            Some(&Vec::new()),
            "case {name:?}: expected no unprocessed accounts, got {created:?}"
        );
        // "CreateMembers verifies the accounts and then invites the verified accounts":
        // the verification is still running when the call answers.
        for member in members {
            assert_eq!(
                member.get("Status").and_then(Value::as_str),
                Some("VERIFICATION_IN_PROGRESS"),
                "case {name:?}: expected the transient status, got {member:?}"
            );
        }

        invited.extend(case.accounts.iter().map(|(id, _)| id.clone()));
    }

    // The move out of the transient status is owed by the clock, not by any later call.
    tokio::time::sleep(std::time::Duration::from_millis(
        (MEMBER_VERIFICATION_MILLIS as u64) + 250,
    ))
    .await;

    let settled = get_members(&router, &graph_arn, &invited).await;
    assert_eq!(
        settled.len(),
        invited.len(),
        "expected every invited account reported, got {:?}",
        settled.keys().collect::<Vec<_>>()
    );
    let mut updated_times: BTreeMap<String, String> = BTreeMap::new();
    for id in &invited {
        let detail = settled
            .get(id)
            .unwrap_or_else(|| panic!("case {id:?}: GetMembers reported no member for it"));
        assert_eq!(
            detail.get("Status").and_then(Value::as_str),
            Some("VERIFICATION_FAILED"),
            "case {id:?}: expected the terminal verification status, got {detail:?}"
        );
        // `DisabledReason` is reported only "for member accounts with a status of
        // ACCEPTED_BUT_DISABLED", which this is not.
        assert!(
            detail.get("DisabledReason").is_none(),
            "case {id:?}: a failed verification carries no DisabledReason, got {detail:?}"
        );
        assert_iso8601(id, detail.get("InvitedTime"));
        updated_times.insert(
            id.clone(),
            assert_iso8601(id, detail.get("UpdatedTime")),
        );
    }

    // A read does not change what a later caller sees: the terminal status and the
    // instant it was due read identically the second time.
    let again = get_members(&router, &graph_arn, &invited).await;
    for id in &invited {
        let detail = again
            .get(id)
            .unwrap_or_else(|| panic!("case {id:?}: the second GetMembers reported no member"));
        assert_eq!(
            detail.get("Status").and_then(Value::as_str),
            Some("VERIFICATION_FAILED"),
            "case {id:?}: expected a stable terminal status, got {detail:?}"
        );
        assert_eq!(
            detail.get("UpdatedTime").and_then(Value::as_str),
            updated_times.get(id).map(String::as_str),
            "case {id:?}: expected a stable UpdatedTime, got {detail:?}"
        );
    }

    // None of the batch was dropped: `ListMembers` reports every invited account, each
    // in the same settled status.
    let (status, listed) = call(
        &router,
        "/graph/members/list",
        &json!({ "GraphArn": graph_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListMembers: expected 200, got {status} {listed:?}"
    );
    let details = listed
        .get("MemberDetails")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListMembers carried no MemberDetails, got {listed:?}"));
    let mut reported: Vec<String> = details
        .iter()
        .map(|detail| {
            assert_eq!(
                detail.get("Status").and_then(Value::as_str),
                Some("VERIFICATION_FAILED"),
                "ListMembers: expected the terminal verification status, got {detail:?}"
            );
            detail
                .get("AccountId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    reported.sort();
    let mut expected = invited.clone();
    expected.sort();
    assert_eq!(
        reported, expected,
        "ListMembers: expected every invited account once, got {reported:?}"
    );
}

#[tokio::test]
async fn a_verification_that_is_not_due_yet_is_still_in_progress() {
    let router = emulator();
    let graph_arn = create_graph_ok(&router, REGION, &json!({})).await;
    // The span is never zero, so a caller reading straight back still observes the
    // transient status and can tell "pending" from "failed verification".
    let member = account_id(42);
    let (status, created) = call(
        &router,
        "/graph/members",
        &json!({
            "GraphArn": graph_arn,
            "Accounts": [entry(&member, "member@example.com")],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateMembers: expected 200, got {status} {created:?}"
    );

    let detail = get_members(&router, &graph_arn, std::slice::from_ref(&member)).await;
    let read = detail
        .get(&member)
        .unwrap_or_else(|| panic!("GetMembers reported no member for {member:?}"));
    assert_eq!(
        read.get("Status").and_then(Value::as_str),
        Some("VERIFICATION_IN_PROGRESS"),
        "a read before the due instant: expected the transient status, got {read:?}"
    );
}
