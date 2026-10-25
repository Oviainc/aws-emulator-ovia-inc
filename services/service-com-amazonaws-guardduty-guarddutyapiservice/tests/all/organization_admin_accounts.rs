// SPDX-License-Identifier: CC0-1.0

//! The GuardDuty delegated administrator: `EnableOrganizationAdminAccount`,
//! `DisableOrganizationAdminAccount` and `ListOrganizationAdminAccounts`, end to
//! end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. A read of the delegated administrators reports what a designation stored and
//!    nothing else, projected with both members `AdminAccount` carries
//!    (`smithy_query(shape="AdminAccount")`), and an endpoint that holds no
//!    designation reports a present, empty list rather than a failure.
//! 2. The designation is REGIONAL: it is reported by the Region the designating
//!    request was signed for and by no other.
//! 3. One organization has at most one delegated administrator (`AdminAccounts` is
//!    `len 0..1` — `smithy_query(operation="ListOrganizationAdminAccounts")`), so
//!    re-designating the account already held changes nothing and designating
//!    another is refused until the current one is removed.
//! 4. An account id that cannot be an account id at all is refused with HTTP 400
//!    under the `InvalidInputException` class naming the member at fault and
//!    echoing no value, and the store is left alone; so is a removal naming an
//!    account that is not the delegated administrator, and one asked for where
//!    there is no delegated administrator to remove.
//! 5. A removal is owed by the CLOCK rather than applied instantly: the read
//!    reports the designation under `DISABLE_IN_PROGRESS` until the span is up and
//!    reports nothing afterwards, at which point another account can be
//!    designated.
//! 6. The read is PAGINATED: a page is never longer than one entry whatever
//!    `maxResults` asks for, each published bound of `MaxResults` is accepted and
//!    one past either end is refused, and a `nextToken` this service never issued
//!    is refused naming the member at fault.

use serde_json::{json, Value};

use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    ADMIN_ACCOUNT_ID_MEMBER, ADMIN_DISABLE_SPAN_MILLIS, NEXT_TOKEN_MEMBER,
};

use crate::detector_lifecycle::{call_in, error_code, error_shape, new_router, Router};

/// The error CLASS every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// The modelled error SHAPE a 400 is enveloped in, as `x-amzn-errortype` carries
/// it — the only client error any operation of this service declares.
const BAD_REQUEST: &str = "BadRequestException";

/// `AdminStatus`'s two members (`smithy_query(shape="AdminStatus")`): the status a
/// designation reports, and the status it reports while a removal is pending.
const ENABLED: &str = "ENABLED";
const DISABLE_IN_PROGRESS: &str = "DISABLE_IN_PROGRESS";

/// Longer than the span the handler makes a removal due in, so a read after this
/// wait is past it. Read from the service state rather than restated here, so the
/// wait tracks the constant the handler uses.
fn past_the_removal() -> std::time::Duration {
    std::time::Duration::from_millis(ADMIN_DISABLE_SPAN_MILLIS as u64 + 400)
}

async fn enable_admin(router: &Router, region: &str, admin_account_id: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        "/admin/enable",
        Some(json!({"adminAccountId": admin_account_id})),
    )
    .await
}

async fn disable_admin(router: &Router, region: &str, admin_account_id: &str) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        "/admin/disable",
        Some(json!({"adminAccountId": admin_account_id})),
    )
    .await
}

async fn list_admins(router: &Router, region: &str, query: &str) -> (u16, Value) {
    call_in(router, region, "GET", &format!("/admin{query}"), None).await
}

/// The `adminAccounts` a successful read reports, failing the case if the read did
/// not answer 200 with the member present.
async fn admin_accounts(router: &Router, region: &str, case: &str) -> Vec<Value> {
    let (status, body) = list_admins(router, region, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the read, got {status} {body:?}"
    );
    assert_eq!(
        body["nextToken"],
        Value::Null,
        "case {case:?}: a list of at most one entry is one page, so it carries no nextToken, got {body:?}"
    );
    body["adminAccounts"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected adminAccounts to be present, got {body:?}")
        })
        .clone()
}

/// Assert that `region` reports exactly `admin_account_id` under `status`.
async fn assert_reports(
    router: &Router,
    region: &str,
    admin_account_id: &str,
    status: &str,
    case: &str,
) {
    let reported = admin_accounts(router, region, case).await;
    assert_eq!(
        reported,
        vec![json!({"adminAccountId": admin_account_id, "adminStatus": status})],
        "case {case:?}: expected the one designation {admin_account_id} under {status}, got {reported:?}"
    );
}

/// Assert that `region` reports no delegated administrator at all.
async fn assert_reports_none(router: &Router, region: &str, case: &str) {
    let reported = admin_accounts(router, region, case).await;
    assert!(
        reported.is_empty(),
        "case {case:?}: expected a present, empty list, got {reported:?}"
    );
}

/// Each row a designation to make, differing along the FORM of the account id (the
/// lowest and highest 12-digit values, a leading-zero id, an arbitrary one) and
/// along the Region it is made through, so no two rows share a partition.
const ROWS: [(&str, &str, &str); 4] = [
    ("the lowest account id", "us-east-1", "000000000000"),
    ("the highest account id", "eu-west-2", "999999999999"),
    ("a leading-zero account id", "ap-south-1", "012345678901"),
    ("an arbitrary account id", "zz-invented-1", "314159265358"),
];

/// A Region no row designates anything through.
const UNDESIGNATED_REGION: &str = "sa-east-1";

#[tokio::test]
async fn a_designation_is_reported_by_the_region_that_made_it_and_by_no_other() {
    let router = new_router();

    // Before any designation every endpoint reports a present, empty list.
    for (case, region, _) in ROWS {
        assert_reports_none(&router, region, case).await;
    }

    for (case, region, account_id) in ROWS {
        let (status, body) = enable_admin(&router, region, account_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the designation, got {status} {body:?}"
        );
    }

    // Each Region reports its OWN designation, under the enabled status, and a
    // Region that designated nothing still reports an empty list.
    for (case, region, account_id) in ROWS {
        assert_reports(&router, region, account_id, ENABLED, case).await;
    }
    assert_reports_none(&router, UNDESIGNATED_REGION, "a Region that designated nothing").await;
}

#[tokio::test]
async fn the_read_changes_nothing_a_later_caller_sees() {
    let router = new_router();
    let region = "us-west-2";
    let admin = "100000000001";

    let (status, body) = enable_admin(&router, region, admin).await;
    assert_eq!(status, 200, "the designation: got {status} {body:?}");

    // Reading it three times reports the same designation every time: the read
    // neither removes it nor moves its status.
    for round in 0..3 {
        let case = format!("read number {round}");
        assert_reports(&router, region, admin, ENABLED, &case).await;
    }

    // And the designation is still the one the Region holds afterwards: another
    // account is still refused by it rather than accepted into a store the reads
    // emptied.
    let (status, refused) = enable_admin(&router, region, "100000000002").await;
    assert_eq!(
        status, 400,
        "a second account after three reads: expected 400, got {status} {refused:?}"
    );
    assert_reports(&router, region, admin, ENABLED, "after the refusal").await;
}

#[tokio::test]
async fn an_account_id_that_cannot_be_one_is_refused_and_persists_nothing() {
    let router = new_router();
    let region = "us-east-2";

    // Each row a value `AdminAccountId` targets (a plain `String`, so the boundary
    // validator passes it) that cannot be an account id: too short, too long,
    // non-digits, empty, and digits with padding.
    let rows: [(&str, &str); 6] = [
        ("one digit short", "12345678901"),
        ("one digit long", "1234567890123"),
        ("a trailing non-digit", "12345678901a"),
        ("no digits at all", "abcdefghijkl"),
        ("the empty string", ""),
        ("twelve digits with a leading space", " 12345678901"),
    ];

    for (case, value) in rows {
        for (operation, path) in [("the designation", "enable"), ("the removal", "disable")] {
            let (status, body) = call_in(
                &router,
                region,
                "POST",
                &format!("/admin/{path}"),
                Some(json!({"adminAccountId": value})),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected 400, got {status} {body:?}"
            );
            assert_eq!(
                error_shape(&body),
                BAD_REQUEST,
                "case {case:?} on {operation}: expected the modelled envelope shape, got {body:?}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_INPUT,
                "case {case:?} on {operation}: expected the {INVALID_INPUT} class, got {body:?}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(ADMIN_ACCOUNT_ID_MEMBER),
                "case {case:?} on {operation}: the refusal must name {ADMIN_ACCOUNT_ID_MEMBER}, got {message:?}"
            );
            assert!(
                value.trim().is_empty() || !message.contains(value),
                "case {case:?} on {operation}: a refusal must not echo the caller's value {value:?}, got {message:?}"
            );
        }
        assert_reports_none(&router, region, case).await;
    }
}

#[tokio::test]
async fn one_organization_holds_one_delegated_administrator() {
    let router = new_router();
    let region = "eu-central-1";
    let admin = "200000000001";

    let (status, body) = enable_admin(&router, region, admin).await;
    assert_eq!(status, 200, "the first designation: got {status} {body:?}");

    // Designating the account the Region already holds is the SAME designation: it
    // succeeds and leaves one record, never a second one.
    let (status, body) = enable_admin(&router, region, admin).await;
    assert_eq!(
        status, 200,
        "re-designating the account already held: expected 200, got {status} {body:?}"
    );
    assert_reports(&router, region, admin, ENABLED, "after re-designating").await;

    // Any OTHER account is refused while that one is held, whatever its form.
    for other in ["200000000002", "000000000000", "999999999999"] {
        let case = format!("a second account {other}");
        let (status, refused) = enable_admin(&router, region, other).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            INVALID_INPUT,
            "case {case:?}: expected the {INVALID_INPUT} class, got {refused:?}"
        );
        let message = refused["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains(other) && !message.contains(admin),
            "case {case:?}: a refusal must echo no account id, got {message:?}"
        );
        assert_reports(&router, region, admin, ENABLED, &case).await;
    }
}

#[tokio::test]
async fn a_removal_is_refused_unless_it_names_the_delegated_administrator() {
    let router = new_router();
    let region = "ca-central-1";
    let admin = "300000000001";

    // Nothing to remove yet: a well-formed account id is refused because the
    // organization has no delegated administrator at all.
    let (status, refused) = disable_admin(&router, region, admin).await;
    assert_eq!(
        status, 400,
        "a removal with nothing designated: expected 400, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        INVALID_INPUT,
        "a removal with nothing designated: expected the {INVALID_INPUT} class, got {refused:?}"
    );

    let (status, body) = enable_admin(&router, region, admin).await;
    assert_eq!(status, 200, "the designation: got {status} {body:?}");

    // A removal naming any other account leaves the designation where it is.
    for other in ["300000000002", "000000000000", "999999999999"] {
        let case = format!("a removal naming {other}, which is not the administrator");
        let (status, refused) = disable_admin(&router, region, other).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            INVALID_INPUT,
            "case {case:?}: expected the {INVALID_INPUT} class, got {refused:?}"
        );
        let message = refused["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains(other) && !message.contains(admin),
            "case {case:?}: a refusal must echo no account id, got {message:?}"
        );
        assert_reports(&router, region, admin, ENABLED, &case).await;
    }
}

#[tokio::test]
async fn a_removal_is_reported_in_progress_until_the_clock_owes_it() {
    let router = new_router();
    let region = "ap-northeast-1";
    let first = "400000000001";
    let second = "400000000002";

    let (status, body) = enable_admin(&router, region, first).await;
    assert_eq!(status, 200, "the designation: got {status} {body:?}");

    // The removal is accepted and the read reports the transient status straight
    // afterwards: the span is not zero.
    let (status, body) = disable_admin(&router, region, first).await;
    assert_eq!(status, 200, "the removal: got {status} {body:?}");
    assert_reports(
        &router,
        region,
        first,
        DISABLE_IN_PROGRESS,
        "straight after the removal",
    )
    .await;

    // Asking for the same removal again is the same removal: it succeeds and does
    // not postpone the instant the first one made due.
    let (status, body) = disable_admin(&router, region, first).await;
    assert_eq!(status, 200, "the removal, repeated: got {status} {body:?}");

    // A fresh designation is still refused while the removal is pending.
    let (status, refused) = enable_admin(&router, region, second).await;
    assert_eq!(
        status, 400,
        "another account while the removal is pending: expected 400, got {status} {refused:?}"
    );

    tokio::time::sleep(past_the_removal()).await;

    // Past the span the designation is gone, and another account can take its
    // place.
    assert_reports_none(&router, region, "past the removal span").await;
    let (status, body) = enable_admin(&router, region, second).await;
    assert_eq!(
        status, 200,
        "the next designation past the removal span: got {status} {body:?}"
    );
    assert_reports(&router, region, second, ENABLED, "the next designation").await;
}

#[tokio::test]
async fn the_read_is_paginated_over_a_list_of_at_most_one_entry() {
    let router = new_router();
    let region = "eu-west-1";
    let admin = "500000000001";

    let (status, body) = enable_admin(&router, region, admin).await;
    assert_eq!(status, 200, "the designation: got {status} {body:?}");

    // `MaxResults` is `range 1..50`, so each published bound holds — and whatever
    // it asks for, the page carries the one entry the list holds and no token.
    for size in [1, 2, 25, 50] {
        let case = format!("maxResults={size}");
        let (status, body) = list_admins(&router, region, &format!("?maxResults={size}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            body["adminAccounts"],
            json!([{"adminAccountId": admin, "adminStatus": ENABLED}]),
            "case {case:?}: expected the one designation, got {body:?}"
        );
        assert_eq!(
            body["nextToken"],
            Value::Null,
            "case {case:?}: the last page carries no nextToken, got {body:?}"
        );
    }

    // One past either published bound is refused by the modelled constraint.
    for size in [0, 51, 1_000] {
        let case = format!("maxResults={size}, outside the modelled range");
        let (status, body) = list_admins(&router, region, &format!("?maxResults={size}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_shape(&body),
            BAD_REQUEST,
            "case {case:?}: expected the modelled error shape, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("maxResults"),
            "case {case:?}: the refusal must name maxResults, got {message:?}"
        );
    }

    // A `nextToken` this service never issued is refused, naming the member at
    // fault and echoing no value — in a Region holding a designation and in one
    // holding none.
    let token_rows: [(&str, &str); 4] = [
        ("a nextToken that is not a token this service issues", "not-a-token"),
        ("a nextToken pointing far past the end of the list", "9999"),
        ("a nextToken pointing at the entry just past the only one", "1"),
        ("a negative offset, which is no token at all", "-1"),
    ];
    for (case, token) in token_rows {
        for (holding, list_region) in [
            ("a Region holding a designation", region),
            ("a Region holding none", UNDESIGNATED_REGION),
        ] {
            let (status, body) =
                list_admins(&router, list_region, &format!("?nextToken={token}")).await;
            assert_eq!(
                status, 400,
                "case {case:?} in {holding}: expected 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_INPUT,
                "case {case:?} in {holding}: expected the {INVALID_INPUT} class, got {body:?}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(NEXT_TOKEN_MEMBER),
                "case {case:?} in {holding}: the refusal must name {NEXT_TOKEN_MEMBER}, got {message:?}"
            );
            assert!(
                !message.contains(token),
                "case {case:?} in {holding}: a refusal must not echo the caller's value {token:?}, got {message:?}"
            );
        }
    }

    // The refusals left the designation alone.
    assert_reports(&router, region, admin, ENABLED, "after the refused pages").await;
}
