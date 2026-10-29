// SPDX-License-Identifier: CC0-1.0

//! `EnableOrganizationAdminAccount` issued WHILE the removal of the account it
//! names is still in flight, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite) and the helpers that drive
//! the three delegated-administrator operations are shared with the
//! `organization_admin_accounts` suite, so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rule under test, asserted over a SET of rows rather than one call: a request
//! that asks a transitional record for the status it is transitioning AWAY from
//! turns that transition around instead of being refused or queueing behind it, so
//! designating the delegated administrator a Region already holds while its removal
//! is still pending answers success, reports the account enabled again and leaves
//! the clock owing nothing — while a designation naming ANOTHER account is still
//! refused, because that one would be a second administrator for one organization.

use crate::detector_lifecycle::{error_code, new_router, Router};
use crate::organization_admin_accounts::{
    assert_reports, assert_reports_none, disable_admin, enable_admin, past_the_removal,
    DISABLE_IN_PROGRESS, ENABLED, INVALID_INPUT,
};

/// Each row one Region holding one designation whose removal is then reversed. The
/// rows differ along the FORM of the account id (the lowest and highest 12-digit
/// values, a leading-zero id, an arbitrary one), along the Region the designation
/// was made through — so no two rows share a partition of the store — along how
/// many times the removal is asked for before the reversal, and along how many
/// times the reversal itself is repeated.
const ROWS: [(&str, &str, &str, usize, usize); 4] = [
    ("the lowest account id", "us-east-1", "000000000000", 1, 1),
    ("the highest account id", "eu-west-1", "999999999999", 2, 1),
    (
        "a leading-zero account id",
        "ap-southeast-1",
        "012345678901",
        1,
        3,
    ),
    (
        "an arbitrary account id, through an invented Region",
        "zz-invented-3",
        "271828182845",
        3,
        2,
    ),
];

/// A Region no row designates anything through.
const UNDESIGNATED_REGION: &str = "sa-east-1";

/// Designate `account_id` through `region` and ask for its removal `removals`
/// times, failing the case unless every call answers 200 and the read then reports
/// the transient status.
async fn designate_and_begin_removal(
    router: &Router,
    region: &str,
    account_id: &str,
    removals: usize,
    case: &str,
) {
    let (status, body) = enable_admin(router, region, account_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the designation, got {status} {body:?}"
    );
    for round in 0..removals {
        let (status, body) = disable_admin(router, region, account_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from removal number {round}, got {status} {body:?}"
        );
    }
    assert_reports(router, region, account_id, DISABLE_IN_PROGRESS, case).await;
}

#[tokio::test]
async fn designating_the_same_account_turns_a_pending_removal_around() {
    let router = new_router();

    // Each row's reversal is driven in the same pass as its removal, so the
    // reversal happens while the removal is genuinely still in flight.
    for (case, region, account_id, removals, reversals) in ROWS {
        designate_and_begin_removal(&router, region, account_id, removals, case).await;

        for round in 0..reversals {
            let (status, body) = enable_admin(&router, region, account_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from reversal number {round}, got {status} {body:?}"
            );
            assert_reports(&router, region, account_id, ENABLED, case).await;
        }
    }

    // The reversal left the clock owing nothing: past the span the removal would
    // have fallen due in, every row's administrator is still the one it holds,
    // reported enabled.
    tokio::time::sleep(past_the_removal()).await;
    for (case, region, account_id, _, _) in ROWS {
        assert_reports(&router, region, account_id, ENABLED, case).await;
    }
    assert_reports_none(&router, UNDESIGNATED_REGION, "a Region that designated nothing").await;
}

#[tokio::test]
async fn another_account_is_still_refused_while_the_removal_is_pending() {
    let router = new_router();

    // The reversal is the only direction the pending removal opens: a designation
    // naming an account the Region does NOT hold is still refused, and leaves the
    // pending removal exactly where it was.
    for (case, region, account_id, removals, _) in ROWS {
        designate_and_begin_removal(&router, region, account_id, removals, case).await;

        for other in ["100000000001", "000000000000", "999999999999"] {
            // The highest and lowest ids are themselves rows, so a row naming one
            // of them would be naming the account it holds.
            if other == account_id {
                continue;
            }
            let other_case = format!("{case}, with {other} designated instead");
            let (status, refused) = enable_admin(&router, region, other).await;
            assert_eq!(
                status, 400,
                "case {other_case:?}: expected 400, got {status} {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                INVALID_INPUT,
                "case {other_case:?}: expected the {INVALID_INPUT} class, got {refused:?}"
            );
            let message = refused["message"].as_str().unwrap_or_default();
            assert!(
                !message.contains(other) && !message.contains(account_id),
                "case {other_case:?}: a refusal must echo no account id, got {message:?}"
            );
            assert_reports(&router, region, account_id, DISABLE_IN_PROGRESS, &other_case).await;
        }
    }

    // None of the refusals cancelled what was owed: past the span every row's
    // removal has fallen due and its Region holds no administrator.
    tokio::time::sleep(past_the_removal()).await;
    for (case, region, _, _, _) in ROWS {
        assert_reports_none(&router, region, case).await;
    }
}

#[tokio::test]
async fn a_reversed_removal_can_be_asked_for_again_and_is_owed_from_then() {
    let router = new_router();

    // A turned-around removal is not a removal the record remembers: asking for it
    // again is accepted, reported in progress, and falls due from the LATER
    // request rather than from the one that was reversed.
    for (case, region, account_id, removals, reversals) in ROWS {
        designate_and_begin_removal(&router, region, account_id, removals, case).await;
        for round in 0..reversals {
            let (status, body) = enable_admin(&router, region, account_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from reversal number {round}, got {status} {body:?}"
            );
        }
    }

    // Past the span the reversed removals would have fallen due in, every row is
    // still held — and the removal asked for now is a fresh one.
    tokio::time::sleep(past_the_removal()).await;
    for (case, region, account_id, _, _) in ROWS {
        assert_reports(&router, region, account_id, ENABLED, case).await;
        let (status, body) = disable_admin(&router, region, account_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the removal asked for again, got {status} {body:?}"
        );
        assert_reports(&router, region, account_id, DISABLE_IN_PROGRESS, case).await;
    }

    tokio::time::sleep(past_the_removal()).await;
    for (case, region, _, _, _) in ROWS {
        assert_reports_none(&router, region, case).await;
    }
}
