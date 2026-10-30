// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the page `GetResourceShareInvitations` reports.
//!
//! `tests/all/pending_invitation_resources.rs` pins WHICH invitations the read
//! selects. This suite pins what the read answers for each of them and what the read
//! leaves behind, each rule over a table of inputs rather than one case:
//!
//! * every member of a reported invitation is the one the store holds for it,
//!   including the instant it was sent, which the restJson1 `DateTime` carries as
//!   epoch seconds, and the deprecated association list, which the read never fills;
//! * the operation reports and does not mutate: reading, filtering and paging through
//!   the invitations leaves every stored record exactly as it was, so a later caller
//!   sees what the first one saw;
//! * `maxResults` caps a page at the number of invitations asked for, and only a
//!   value inside the modelled range is accepted.

use serde_json::{json, Value};
use service_com_amazonaws_ram_amazonresourcesharing::state::{
    INVITATION_STATUS_PENDING, MAX_PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

// Every suite in this crate calls the service the one way: through `tests/all/harness.rs`.
use super::harness::*;

/// Accounts outside the one that owns the shares, so a principal in one of them is
/// external and gets invited.
const FOREIGN_ACCOUNTS: [&str; 4] = [
    "111122223333",
    "444455556666",
    "777788889999",
    "222233334444",
];
/// An IAM role in the first of them: a principal that names a role rather than a
/// whole account, so the invitation to it names a receiver ARN too.
const FOREIGN_ROLE: &str = "arn:aws:iam::111122223333:role/Reader";

/// A row of the reported-members table: the principals a share is created with, and
/// how many invitations that must have sent.
struct ReportCase {
    case: &'static str,
    principals: Vec<&'static str>,
    invitations: usize,
}

#[tokio::test]
async fn every_reported_member_is_the_one_the_store_holds() {
    // The rule generalises over the invitations a read reports, so the rows differ
    // along their COUNT and along the FORM of the principal each was sent to — a bare
    // account id, and an IAM role ARN, which is the form that fills `receiverArn`.
    let cases = vec![
        ReportCase {
            case: "one account invited",
            principals: vec![FOREIGN_ACCOUNTS[0]],
            invitations: 1,
        },
        ReportCase {
            case: "one IAM role invited",
            principals: vec![FOREIGN_ROLE],
            invitations: 1,
        },
        ReportCase {
            case: "three accounts invited",
            principals: vec![FOREIGN_ACCOUNTS[0], FOREIGN_ACCOUNTS[1], FOREIGN_ACCOUNTS[2]],
            invitations: 3,
        },
    ];

    for ReportCase {
        case,
        principals,
        invitations,
    } in cases
    {
        let (router, engine) = emulator_with_engine();
        create_ok(
            &router,
            case,
            &json!({"name": "invited-share", "principals": principals}),
        )
        .await;

        let reported = invitations_ok(&router, case, &json!({})).await;
        assert_eq!(
            reported.len(),
            invitations,
            "case {case:?}: expected {invitations} invitation(s), got {reported:?}"
        );

        for invitation in &reported {
            let arn = member(invitation, "resourceShareInvitationArn").unwrap_or_else(|| {
                panic!("case {case:?}: no resourceShareInvitationArn on {invitation:?}")
            });
            let state = store(&engine);
            let locked = state.lock();
            let record = locked
                .invitations
                .get(arn)
                .unwrap_or_else(|| panic!("case {case:?}: {arn} must be stored"));

            // Every member the modelled `ResourceShareInvitation` carries, against
            // the record the store holds for it.
            for (name, expected) in [
                ("resourceShareInvitationArn", Some(record.arn.as_str())),
                ("resourceShareArn", Some(record.resource_share_arn.as_str())),
                (
                    "resourceShareName",
                    Some(record.resource_share_name.as_str()),
                ),
                ("senderAccountId", Some(record.sender_account_id.as_str())),
                (
                    "receiverAccountId",
                    Some(record.receiver_account_id.as_str()),
                ),
                ("receiverArn", record.receiver_arn.as_deref()),
                ("status", Some(record.status.as_str())),
            ] {
                assert_eq!(
                    member(invitation, name),
                    expected,
                    "case {case:?}: expected {name} {expected:?}, got {invitation:?}"
                );
            }
            assert_eq!(
                record.status, INVITATION_STATUS_PENDING,
                "case {case:?}: a fresh invitation is stored PENDING, got {record:?}"
            );

            // `invitationTimestamp` is a `DateTime`, which restJson1 renders as epoch
            // seconds — the stored instant is in milliseconds.
            let reported_seconds = timestamp(case, invitation, "invitationTimestamp");
            let expected_seconds = record.invitation_timestamp as f64 / 1000.0;
            assert!(
                (reported_seconds - expected_seconds).abs() < 0.001,
                "case {case:?}: expected invitationTimestamp {expected_seconds}, \
                 got {reported_seconds}"
            );

            // The association list is deprecated in favour of
            // `ListPendingInvitationResources`, so this read fills it with nothing.
            assert!(
                invitation.get("resourceShareAssociations").is_none(),
                "case {case:?}: expected no resourceShareAssociations, got {invitation:?}"
            );
        }
    }
}

#[tokio::test]
async fn reading_invitations_changes_nothing_a_later_caller_sees() {
    let router_case = "four invitations across two shares";
    let (router, engine) = emulator_with_engine();
    let first_share = arn_of(
        router_case,
        &create_ok(
            &router,
            router_case,
            &json!({
                "name": "first",
                "principals": [FOREIGN_ACCOUNTS[0], FOREIGN_ACCOUNTS[1]],
            }),
        )
        .await,
    );
    create_ok(
        &router,
        router_case,
        &json!({
            "name": "second",
            "principals": [FOREIGN_ACCOUNTS[2], FOREIGN_ACCOUNTS[3]],
        }),
    )
    .await;
    let before = snapshot(&engine);
    assert_eq!(
        before.len(),
        4,
        "case {router_case:?}: expected four stored invitations, got {before:?}"
    );
    let an_invitation_arn = snapshot_arns(&engine)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {router_case:?}: an invitation must be stored"));

    // The rule generalises over the SHAPE of the read, so the rows differ along it:
    // unfiltered, each filter, a page smaller than the result set, a page at the
    // published maximum, and a read the service refuses.
    let cases = vec![
        ("no filter", json!({}), 200),
        (
            "an invitation ARN filter",
            json!({"resourceShareInvitationArns": [an_invitation_arn.clone()]}),
            200,
        ),
        (
            "a resource share ARN filter",
            json!({"resourceShareArns": [first_share.clone()]}),
            200,
        ),
        ("a page of one", json!({"maxResults": 1}), 200),
        (
            "a page at the published maximum",
            json!({"maxResults": MAX_PAGE_SIZE}),
            200,
        ),
        (
            "an invitation ARN nothing has, which is refused",
            json!({"resourceShareInvitationArns": [unknown_invitation_arn()]}),
            400,
        ),
    ];

    for (case, body, expected_status) in cases {
        let (status, first) = get_invitations(&router, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {first:?}"
        );
        // Repeating the read answers the same thing: nothing the first one did can
        // have moved a status, consumed an entry or renumbered a page.
        let (again_status, again) = get_invitations(&router, &body).await;
        assert_eq!(
            (again_status, &again),
            (status, &first),
            "case {case:?}: the second read answered differently, got {again:?}"
        );
        assert_eq!(
            snapshot(&engine),
            before,
            "case {case:?}: the read changed the stored invitations"
        );
    }

    // Walking every page leaves the store alone too, and reports each invitation once.
    let mut seen: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut body = json!({"maxResults": 1});
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let (status, page) = get_invitations(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {router_case:?}: expected 200 while paging, got {status} with {page:?}"
        );
        let entries = page
            .get("resourceShareInvitations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for invitation in &entries {
            seen.push(
                member(invitation, "resourceShareInvitationArn")
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        next_token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        assert!(
            seen.len() <= before.len(),
            "case {router_case:?}: paging reported {} invitations, more than the {} stored",
            seen.len(),
            before.len()
        );
        if next_token.is_none() {
            break;
        }
    }
    seen.sort();
    assert_eq!(
        seen,
        snapshot_arns(&engine),
        "case {router_case:?}: paging must report every stored invitation exactly once, got {seen:?}"
    );
    assert_eq!(
        snapshot(&engine),
        before,
        "case {router_case:?}: paging changed the stored invitations"
    );
}

#[tokio::test]
async fn max_results_caps_the_page_within_the_modelled_range() {
    let setup = "one share inviting four accounts";
    let router = emulator();
    create_ok(
        &router,
        setup,
        &json!({"name": "paged", "principals": FOREIGN_ACCOUNTS}),
    )
    .await;
    let total = FOREIGN_ACCOUNTS.len();

    // One row per page size: below, at and above the number of invitations, plus the
    // maximum of the modelled `MaxResults` range
    // (`smithy_query(query="shape",shape="MaxResults")` reports 1..500, which
    // `MAX_PAGE_SIZE` mirrors).
    for requested in [1usize, 2, 3, total, total + 1, MAX_PAGE_SIZE] {
        let case = format!("maxResults={requested}");
        let (status, body) = get_invitations(&router, &json!({"maxResults": requested})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body:?}"
        );
        let reported = body
            .get("resourceShareInvitations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: response carried no resourceShareInvitations, got {body:?}")
            });
        assert_eq!(
            reported.len(),
            requested.min(total),
            "case {case:?}: expected a page of {}, got {reported:?}",
            requested.min(total)
        );
        assert_eq!(
            body.get("nextToken").and_then(Value::as_str).is_some(),
            requested < total,
            "case {case:?}: a nextToken is reported exactly when invitations remain, got {body:?}"
        );
    }

    // Each published bound of the modelled range, and one step past each.
    for (requested, accepted) in [(1, true), (500, true), (0, false), (501, false)] {
        let case = format!("bound maxResults={requested}");
        let (status, body) = get_invitations(&router, &json!({"maxResults": requested})).await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} with {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A string member of a reported object.
fn member<'a>(value: &'a Value, member: &str) -> Option<&'a str> {
    value.get(member).and_then(Value::as_str)
}

/// `GetResourceShareInvitations` over the wire.
async fn get_invitations(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/getresourceshareinvitations", Some(body)).await
}

/// The invitations a successful `GetResourceShareInvitations` reported.
async fn invitations_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Vec<Value> {
    let (status, reported) = get_invitations(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {reported:?}"
    );
    reported
        .get("resourceShareInvitations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: response carried no resourceShareInvitations, got {reported:?}")
        })
}

/// Every stored invitation record, rendered field by field, so two snapshots taken
/// around a read compare for equality.
fn snapshot(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = store(engine);
    let locked = state.lock();
    locked
        .invitations
        .values()
        .map(|record| format!("{record:?}"))
        .collect()
}

/// The ARNs of every stored invitation, in the store's own order.
fn snapshot_arns(engine: &engine::InMemoryEngine) -> Vec<String> {
    let state = store(engine);
    let locked = state.lock();
    locked.invitations.keys().cloned().collect()
}

/// An invitation ARN of exactly the form this service issues, which no invitation has.
fn unknown_invitation_arn() -> String {
    format!(
        "arn:aws:ram:{REGION}:{ACCOUNT}:resource-share-invitation/00000000-0000-4000-8000-000000000000"
    )
}
