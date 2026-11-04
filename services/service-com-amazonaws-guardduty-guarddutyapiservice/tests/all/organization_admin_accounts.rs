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
//! 7. The designation is NOT owned by a detector: neither
//!    `EnableOrganizationAdminAccount` nor `ListOrganizationAdminAccounts` names a
//!    `DetectorId` (`smithy_query(operation="EnableOrganizationAdminAccount")`), so
//!    a designation is made and reported whatever detectors the Region holds, it
//!    mints none, and creating or deleting one leaves it where it is.

use serde_json::{json, Value};

use service_com_amazonaws_guardduty_guarddutyapiservice::state;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    ADMIN_ACCOUNT_ID_MEMBER, ADMIN_DISABLE_SPAN_MILLIS,
};

use crate::client_rejections::page_token_refusal;
use crate::detector_lifecycle::{call_in, error_code, error_shape, new_router, Router};

/// The error CLASS every client rejection on this service carries in the body's
/// `__type` discriminator.
pub(crate) const INVALID_INPUT: &str = "InvalidInputException";

/// The modelled error SHAPE a 400 is enveloped in, as `x-amzn-errortype` carries
/// it — the only client error any operation of this service declares.
const BAD_REQUEST: &str = "BadRequestException";

/// `AdminStatus`'s two members (`smithy_query(shape="AdminStatus")`): the status a
/// designation reports, and the status it reports while a removal is pending.
pub(crate) const ENABLED: &str = "ENABLED";
pub(crate) const DISABLE_IN_PROGRESS: &str = "DISABLE_IN_PROGRESS";

/// Longer than the span the handler makes a removal due in, so a read after this
/// wait is past it. Read from the service state rather than restated here, so the
/// wait tracks the constant the handler uses.
pub(crate) fn past_the_removal() -> std::time::Duration {
    std::time::Duration::from_millis(ADMIN_DISABLE_SPAN_MILLIS as u64 + 400)
}

pub(crate) async fn enable_admin(
    router: &Router,
    region: &str,
    admin_account_id: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        "/admin/enable",
        Some(json!({"adminAccountId": admin_account_id})),
    )
    .await
}

pub(crate) async fn disable_admin(
    router: &Router,
    region: &str,
    admin_account_id: &str,
) -> (u16, Value) {
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
pub(crate) async fn assert_reports(
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
pub(crate) async fn assert_reports_none(router: &Router, region: &str, case: &str) {
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

/// The `DetectorId`s a Region's endpoint reports, read over the wire through
/// `ListDetectors` (`GET /detector`), so the detector store a designation must be
/// independent of is observed the way a client observes it.
async fn detector_ids(router: &Router, region: &str, case: &str) -> Vec<String> {
    let (status, body) = call_in(router, region, "GET", "/detector", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the detector read, got {status} {body:?}"
    );
    body["detectorIds"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected detectorIds to be present, got {body:?}")
        })
        .iter()
        .filter_map(|id| id.as_str().map(str::to_string))
        .collect()
}

/// An enabled detector in `region` (`POST /detector`), failing the case if the
/// create did not succeed.
async fn create_detector_in(router: &Router, region: &str, case: &str) -> String {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the detector create, got {status} {body:?}"
    );
    body["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a detectorId, got {body:?}"))
        .to_string()
}

/// Delete `detector_id` through `region`'s endpoint (`DELETE /detector/{DetectorId}`).
async fn delete_detector_in(router: &Router, region: &str, detector_id: &str, case: &str) {
    let (status, body) = call_in(
        router,
        region,
        "DELETE",
        &format!("/detector/{detector_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from the detector delete, got {status} {body:?}"
    );
}

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

/// Each row a designation made through a Region whose detector store is in a
/// different state when the designation is made — none at all, or one enabled
/// detector — and differing along the FORM of the account id as [`ROWS`] does.
const DETECTOR_ROWS: [(&str, &str, &str, bool); 4] = [
    ("no detector, an arbitrary account id", "us-west-1", "600000000001", false),
    ("a detector, an arbitrary account id", "eu-north-1", "600000000002", true),
    ("no detector, a leading-zero account id", "ap-east-1", "000000000002", false),
    ("a detector, the highest account id", "me-south-1", "999999999998", true),
];

#[tokio::test]
async fn a_designation_is_independent_of_the_regions_detectors() {
    let router = new_router();

    for (case, region, admin, detector_first) in DETECTOR_ROWS {
        // The detector store starts empty, and holds the row's detector when the
        // row asks for one.
        let before = if detector_first {
            vec![create_detector_in(&router, region, case).await]
        } else {
            Vec::new()
        };
        assert_eq!(
            detector_ids(&router, region, case).await,
            before,
            "case {case:?}: expected the detector store the row asks for, before designating"
        );

        // The designation names no detector, so it is accepted either way.
        let (status, body) = enable_admin(&router, region, admin).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the designation, got {status} {body:?}"
        );
        assert_reports(&router, region, admin, ENABLED, case).await;

        // And it minted no detector of its own: the store is exactly what it was.
        let after = detector_ids(&router, region, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: the designation must mint no detector, got {after:?}"
        );

        // A detector's whole lifecycle runs under the designation: creating one
        // where there was none, and deleting whichever one the Region holds,
        // leaves the designation reported unchanged. `DeleteDetector` cascades to
        // what the detector owns, and the designation is not one of those.
        let detector_id = match before.first() {
            Some(existing) => existing.clone(),
            None => create_detector_in(&router, region, case).await,
        };
        assert_reports(&router, region, admin, ENABLED, case).await;

        delete_detector_in(&router, region, &detector_id, case).await;
        assert!(
            detector_ids(&router, region, case).await.is_empty(),
            "case {case:?}: expected the detector store to be empty after the delete"
        );
        assert_reports(&router, region, admin, ENABLED, case).await;

        // With no detector left, the designation is still the one the Region
        // holds: re-designating the same account still succeeds and changes
        // nothing, and another account is still refused.
        let (status, body) = enable_admin(&router, region, admin).await;
        assert_eq!(
            status, 200,
            "case {case:?}: re-designating with no detector left: got {status} {body:?}"
        );
        let (status, refused) = enable_admin(&router, region, "600000000009").await;
        assert_eq!(
            status, 400,
            "case {case:?}: another account with no detector left: expected 400, got {status} {refused:?}"
        );
        assert_reports(&router, region, admin, ENABLED, case).await;
    }

    // Nothing any row did reached a Region of its own, and none of it designated
    // anything in a Region that designated nothing.
    assert_reports_none(&router, UNDESIGNATED_REGION, "a Region that designated nothing").await;
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
        assert_eq!(
            message,
            state::INVALID_INPUT_PARAMETERS_MESSAGE,
            "case {case:?}: expected GuardDuty's fixed constraint-refusal text, got {message:?}"
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
            // A page token this service did not issue is refused with the ONE
            // sentence the whole pagination surface answers, which names no member
            // at all.
            assert_eq!(
                message,
                page_token_refusal(),
                "case {case:?} in {holding}: expected the page-token refusal, got {message:?}"
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
