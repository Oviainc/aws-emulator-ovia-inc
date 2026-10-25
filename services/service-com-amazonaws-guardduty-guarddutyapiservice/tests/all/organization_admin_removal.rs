// SPDX-License-Identifier: CC0-1.0

//! `DisableOrganizationAdminAccount`, end to end over the wire, pinned against the
//! designations a Region's endpoint does NOT hold.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite) and the helpers that drive
//! the three delegated-administrator operations are shared with the
//! `organization_admin_accounts` suite, so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rule under test, asserted over a SET of rows rather than one call: a removal
//! of the GuardDuty delegated administrator reaches only the designation held by the
//! endpoint the request was signed for, so every other endpoint's designation keeps
//! its account and its status; once the removal the clock owes has fallen due, the
//! endpoint it was signed for holds none, which frees it for any well-formed account
//! id — including the one just removed — and makes a further removal there a refusal
//! rather than a second success.

use crate::detector_lifecycle::{error_code, new_router, Router};
use crate::organization_admin_accounts::{
    assert_reports, assert_reports_none, disable_admin, enable_admin, past_the_removal,
    DISABLE_IN_PROGRESS, ENABLED, INVALID_INPUT,
};

/// Each row one Region holding one designation, the rows differing along the FORM of
/// the account id (the lowest and highest 12-digit values, a leading-zero id, an
/// arbitrary one) and along the Region the designation was made through, so no two
/// rows share a partition of the store.
const ROWS: [(&str, &str, &str); 4] = [
    ("the lowest account id", "us-west-1", "000000000000"),
    ("the highest account id", "me-south-1", "999999999999"),
    ("a leading-zero account id", "af-south-1", "098765432109"),
    ("an arbitrary account id", "ap-east-1", "271828182845"),
];

/// A Region no row designates anything through.
const UNDESIGNATED_REGION: &str = "eu-north-1";

/// Designate every row's administrator through its own Region, failing the case if
/// any designation does not answer 200.
async fn designate_every_row(router: &Router) {
    for (case, region, account_id) in ROWS {
        let (status, body) = enable_admin(router, region, account_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the designation, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn a_removal_reaches_only_the_region_it_was_signed_for() {
    let router = new_router();
    designate_every_row(&router).await;

    // A removal signed for a Region that holds no designation is refused and reaches
    // nothing, even when it names an account another Region holds.
    for (case, _, account_id) in ROWS {
        let (status, refused) = disable_admin(&router, UNDESIGNATED_REGION, account_id).await;
        assert_eq!(
            status, 400,
            "case {case:?} removed through a Region holding nothing: expected 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            INVALID_INPUT,
            "case {case:?} removed through a Region holding nothing: expected the {INVALID_INPUT} class, got {refused:?}"
        );
        let message = refused["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains(account_id),
            "case {case:?}: a refusal must echo no account id, got {message:?}"
        );
    }
    for (case, region, account_id) in ROWS {
        assert_reports(&router, region, account_id, ENABLED, case).await;
    }

    // One row's administrator is removed, through that row's own Region.
    let (removed_case, removed_region, removed_account) = ROWS[0];
    let (status, body) = disable_admin(&router, removed_region, removed_account).await;
    assert_eq!(
        status, 200,
        "case {removed_case:?}: expected 200 from the removal, got {status} {body:?}"
    );

    // That Region reports the transient status — read first, so the assertion does
    // not wait on the other Regions' reads — and every other Region reports its own
    // designation, unchanged.
    assert_reports(
        &router,
        removed_region,
        removed_account,
        DISABLE_IN_PROGRESS,
        removed_case,
    )
    .await;
    for (case, region, account_id) in ROWS.iter().skip(1) {
        assert_reports(&router, region, account_id, ENABLED, case).await;
    }

    tokio::time::sleep(past_the_removal()).await;

    // Past the span only the Region the removal was signed for holds none.
    assert_reports_none(&router, removed_region, removed_case).await;
    for (case, region, account_id) in ROWS.iter().skip(1) {
        assert_reports(&router, region, account_id, ENABLED, case).await;
    }

    // The emptied Region has nothing left to remove, while every other Region can
    // still remove the administrator it holds.
    let (status, refused) = disable_admin(&router, removed_region, removed_account).await;
    assert_eq!(
        status, 400,
        "case {removed_case:?} removed twice: expected 400 once the removal has fallen due, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        INVALID_INPUT,
        "case {removed_case:?} removed twice: expected the {INVALID_INPUT} class, got {refused:?}"
    );
    let message = refused["message"].as_str().unwrap_or_default();
    assert!(
        !message.contains(removed_account),
        "case {removed_case:?}: a refusal must echo no account id, got {message:?}"
    );
    for (case, region, account_id) in ROWS.iter().skip(1) {
        let (status, body) = disable_admin(&router, region, account_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a Region still holding its administrator can remove it, got {status} {body:?}"
        );
        assert_reports(&router, region, account_id, DISABLE_IN_PROGRESS, case).await;
    }
}

#[tokio::test]
async fn a_settled_removal_frees_the_region_for_any_well_formed_account_id() {
    let router = new_router();

    // Each row the administrator a Region holds and the one designated after its
    // removal has fallen due: the same account again, and every other form of
    // account id.
    let rows: [(&str, &str, &str, &str); 4] = [
        (
            "the same account designated again",
            "us-east-1",
            "100000000001",
            "100000000001",
        ),
        (
            "the lowest account id replaced by the highest",
            "eu-west-3",
            "000000000000",
            "999999999999",
        ),
        (
            "a leading-zero account id replaced by an arbitrary one",
            "ap-southeast-2",
            "012345678901",
            "314159265358",
        ),
        (
            "a designation made through an invented Region",
            "zz-invented-7",
            "600000000001",
            "600000000002",
        ),
    ];

    // Each row's removal is asked for and its pending status read in the same pass,
    // so the read does not wait on the other rows' calls.
    for (case, region, first, _) in rows {
        let (status, body) = enable_admin(&router, region, first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the designation, got {status} {body:?}"
        );
        let (status, body) = disable_admin(&router, region, first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the removal, got {status} {body:?}"
        );
        assert_reports(&router, region, first, DISABLE_IN_PROGRESS, case).await;
    }

    tokio::time::sleep(past_the_removal()).await;

    // Past the span every Region holds none, and the next designation — the same
    // account or another — is accepted and reported.
    for (case, region, _, second) in rows {
        assert_reports_none(&router, region, case).await;
        let (status, body) = enable_admin(&router, region, second).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the designation past the removal span, got {status} {body:?}"
        );
        assert_reports(&router, region, second, ENABLED, case).await;
    }
}
