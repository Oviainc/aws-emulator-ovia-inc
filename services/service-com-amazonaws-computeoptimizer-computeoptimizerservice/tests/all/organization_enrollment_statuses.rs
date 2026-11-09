// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetEnrollmentStatusesForOrganization`.
//!
//! Every assertion drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same `MultiServiceRouter` the emulator binary mounts — so a response that is
//! not computed from persisted state fails here.
//!
//! WHICH accounts an organization holds is recorded by AWS Organizations, and no operation
//! of THIS API creates one, so the fixtures below record member accounts into the store the
//! way that service would and then read them back only over the wire. What is asserted is
//! the RULE the read follows — it is served to an organization's MANAGEMENT account only, and
//! reports one row per member account the store holds, each carrying the `Status` its
//! enrollment has reached, selected by the `Status` filter and paged at `maxResults` — over a
//! table of inputs, never one recorded call. The one thing this API does write for a member
//! account is its ENROLLMENT, through an `UpdateEnrollmentStatus` call that includes member
//! accounts; that write is asserted here too, from both sides.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::get_enrollment_statuses_for_organization::NOT_A_MANAGEMENT_ACCOUNT_MESSAGE;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::ops::types::STATUS_VALUES;
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    enrollment_status_reason, service_state, MemberAccountEnrollment,
    ENROLLMENT_STATUSES_MAX_PAGE_SIZE, MAX_PAGE_SIZE, MEMBER_ENROLLMENT_DELAY,
    MEMBER_ENROLLMENT_PENDING_REASON,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, emulator_with_engine, error_code};

/// The `lastUpdatedTimestamp` the recorded member accounts carry, in epoch seconds — the
/// form awsJson1_0 renders a timestamp in.
const RECORDED_AT_SECONDS: i64 = 1_761_000_000;

/// One member account of the organization, with the enrollment already settled at `status`:
/// what AWS Organizations plus an earlier enrollment leaves in the store.
fn recorded(account_id: &str, status: &str) -> MemberAccountEnrollment {
    let mut member = MemberAccountEnrollment::new(account_id);
    member.status = status.to_string();
    member.status_reason = Some(format!("The member account enrollment is {status}."));
    member.last_updated_timestamp = Some(::aws_smithy_types::DateTime::from_secs(
        RECORDED_AT_SECONDS,
    ));
    member
}

/// The organization every fixture below reads: five member accounts spanning all four
/// `Status` values (`crate::ops::types::STATUS_VALUES`), with two accounts sharing one
/// status so a filter selects a SET rather than a single row.
fn corpus() -> Vec<MemberAccountEnrollment> {
    vec![
        recorded("200000000001", "Active"),
        recorded("200000000002", "Inactive"),
        recorded("200000000003", "Active"),
        recorded("200000000004", "Failed"),
        recorded("200000000005", "Pending"),
    ]
}

/// Record `members` as accounts of the organization, the way AWS Organizations would.
fn record(engine: &Arc<engine::InMemoryEngine>, members: &[MemberAccountEnrollment]) {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let mut guard = state.lock();
    for member in members {
        guard.organization_members.insert(member.clone());
    }
}

/// Everything the store holds about the enrollment of the account and of its organization,
/// so a test can prove a read moved none of it.
fn snapshot(engine: &Arc<engine::InMemoryEngine>) -> String {
    let state = service_state(&**engine).expect("the engine bears this service's state");
    let guard = state.lock();
    format!("{:?} {:?}", guard.enrollment, guard.organization_members)
}

/// An emulator whose store holds [`corpus`], and the engine it was built on.
fn emulator_with_corpus() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = emulator_with_engine();
    record(&engine, &corpus());
    (router, engine)
}

/// Read the organization's enrollment statuses, asserting the call was accepted.
async fn get_org(router: &MultiServiceRouter, case: &str, request: Value) -> Value {
    let (status, body) = call(
        router,
        "GetEnrollmentStatusesForOrganization",
        request.clone(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {request}, got {status}: {body}"
    );
    body
}

/// The `accountId` of every row in a response, in the order reported.
fn reported_ids(body: &Value) -> Vec<String> {
    body["accountEnrollmentStatuses"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list of accountEnrollmentStatuses, got {body}"))
        .iter()
        .map(|row| row["accountId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `status` of every row in a response, in the order reported.
fn reported_statuses(body: &Value) -> Vec<String> {
    body["accountEnrollmentStatuses"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list of accountEnrollmentStatuses, got {body}"))
        .iter()
        .map(|row| row["status"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Walk the whole sequence `request` selects, one page at a time, returning the account ids
/// of each page in order. `max_pages` bounds the walk so a token that never terminates fails
/// the test instead of hanging it.
async fn walk_pages(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
    max_pages: usize,
) -> Vec<Vec<String>> {
    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..max_pages {
        let mut page_request = request.clone();
        if let Some(token) = token.as_deref() {
            page_request["nextToken"] = json!(token);
        }
        let body = get_org(router, case, page_request).await;
        pages.push(reported_ids(&body));
        match body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return pages,
        }
    }
    panic!("case {case:?}: the walk did not finish within {max_pages} pages")
}

/// Enroll the account, optionally including the organization's member accounts.
async fn update_enrollment(router: &MultiServiceRouter, status: &str, include_members: bool) {
    let (http_status, body) = call(
        router,
        "UpdateEnrollmentStatus",
        json!({"status": status, "includeMemberAccounts": include_members}),
    )
    .await;
    assert_eq!(
        http_status, 200,
        "enrolling at {status} (includeMemberAccounts={include_members}): got {http_status}: \
         {body}"
    );
}

/// Wait out the span a member account's enrollment takes to reach it
/// ([`MEMBER_ENROLLMENT_DELAY`]); the extra margin covers scheduler jitter.
async fn settle() {
    tokio::time::sleep(MEMBER_ENROLLMENT_DELAY + Duration::from_millis(100)).await;
}

/// The account's own enrollment, as `GetEnrollmentStatus` reports it.
async fn get_enrollment(router: &MultiServiceRouter) -> Value {
    let (status, body) = call(router, "GetEnrollmentStatus", json!({})).await;
    assert_eq!(status, 200, "GetEnrollmentStatus: got {status}: {body}");
    body
}

/// An emulator that records no organization presents a STANDALONE account, and this operation
/// "returns the Compute Optimizer enrollment (opt-in) status of organization member accounts,
/// if your account is an organization management account": every read of it — filtered, paged
/// or bare — is refused with the operation's modelled 400 rather than answered with an empty
/// sequence, while `GetEnrollmentStatus`, the action that reports a standalone account's own
/// enrollment, still answers. Enrolling the account itself still creates no member account —
/// and asking that enrollment to include member accounts is refused for the same reason this
/// read is (`include_member_accounts.rs` pins that refusal across every operation taking it).
#[tokio::test]
async fn a_standalone_account_is_refused_the_organization_s_statuses() {
    let router = emulator();

    for (case, request) in [
        ("no selection at all", json!({})),
        (
            "a Status filter",
            json!({"filters": [{"name": "Status", "values": ["Active"]}]}),
        ),
        ("a page size", json!({"maxResults": 1})),
    ] {
        let (status, body) = call(
            &router,
            "GetEnrollmentStatusesForOrganization",
            request.clone(),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {request} to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected InvalidParameterValueException, got {body}"
        );
        assert_eq!(
            body["message"].as_str().unwrap_or_default(),
            NOT_A_MANAGEMENT_ACCOUNT_MESSAGE,
            "case {case:?}: expected the not-a-management-account refusal, got {body}"
        );
        assert!(
            body["accountEnrollmentStatuses"].is_null(),
            "case {case:?}: a refused read reports no statuses, got {body}"
        );
    }

    // The account's OWN enrollment is readable by the same client throughout: "to get the
    // enrollment status of standalone accounts, use the GetEnrollmentStatus action."
    let enrollment = get_enrollment(&router).await;
    assert_eq!(
        enrollment["numberOfMemberAccountsOptedIn"],
        json!(0),
        "a standalone account has no member account opted in, got {enrollment}"
    );

    update_enrollment(&router, "Active", false).await;
    settle().await;
    let enrollment = get_enrollment(&router).await;
    assert_eq!(
        enrollment["status"],
        json!("Active"),
        "the account's own enrollment is readable after enrolling, got {enrollment}"
    );
    assert_eq!(
        enrollment["numberOfMemberAccountsOptedIn"],
        json!(0),
        "enrolling a standalone account creates no member account, got {enrollment}"
    );
}

/// Every member of a recorded member account is reported back unchanged, in account-id
/// order, and reporting it twice changes nothing a later caller sees.
#[tokio::test]
async fn a_recorded_member_account_is_reported_member_for_member() {
    let (router, engine) = emulator_with_corpus();
    let before = snapshot(&engine);

    let body = get_org(&router, "the recorded organization", json!({})).await;
    let rows = body["accountEnrollmentStatuses"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a list of accountEnrollmentStatuses, got {body}"));
    assert_eq!(
        rows.len(),
        corpus().len(),
        "one row per recorded member account: expected {}, got {body}",
        corpus().len()
    );

    for member in corpus() {
        let case = member.account_id.clone();
        let reported = rows
            .iter()
            .find(|row| row["accountId"].as_str() == Some(case.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: no row for the recorded account, got {body}"));
        let expected = json!({
            "accountId": member.account_id,
            "status": member.status,
            "statusReason": member.status_reason,
            "lastUpdatedTimestamp": RECORDED_AT_SECONDS,
        });
        for (name, wanted) in expected.as_object().expect("an object of members") {
            assert_eq!(
                &reported[name], wanted,
                "case {case:?}: expected {name} {wanted}, got {reported}"
            );
        }
    }

    // In account-id order, which is the order a page boundary is taken in.
    let mut sorted = reported_ids(&body);
    sorted.sort();
    assert_eq!(
        reported_ids(&body),
        sorted,
        "the rows are reported in account-id order, got {body}"
    );

    // A read is a READ: the same call answers the same thing, and the store did not move.
    let again = get_org(&router, "the same read again", json!({})).await;
    assert_eq!(
        again, body,
        "two identical reads must answer identically, got {again}"
    );
    assert_eq!(
        snapshot(&engine),
        before,
        "reading the organization must not write to the store"
    );
}

/// The `Status` filter selects the member accounts whose status is one of its values: a
/// filter matches on that member, several values are ORed, several filters are ANDed, and a
/// filter carrying no values constrains nothing.
#[tokio::test]
async fn the_status_filter_selects_by_the_status_a_member_account_reports() {
    let (router, _engine) = emulator_with_corpus();
    let all: Vec<String> = corpus()
        .iter()
        .map(|member| member.account_id.clone())
        .collect();

    // One row per value of the model's own `Status` vocabulary
    // (`crate::ops::types::STATUS_VALUES`), plus the combinations the rule generalises
    // over: several values, several filters, and no values at all.
    let mut cases: Vec<(Value, Vec<String>)> = Vec::new();
    for status in STATUS_VALUES {
        let expected: Vec<String> = corpus()
            .iter()
            .filter(|member| member.status == *status)
            .map(|member| member.account_id.clone())
            .collect();
        cases.push((
            json!({"filters": [{"name": "Status", "values": [status]}]}),
            expected,
        ));
    }
    cases.push((
        json!({"filters": [{"name": "Status", "values": ["Active", "Failed"]}]}),
        vec![
            "200000000001".to_string(),
            "200000000003".to_string(),
            "200000000004".to_string(),
        ],
    ));
    cases.push((
        json!({"filters": [{"name": "Status", "values": []}]}),
        all.clone(),
    ));
    cases.push((json!({"filters": []}), all.clone()));
    cases.push((
        json!({"filters": [
            {"name": "Status", "values": ["Active"]},
            {"name": "Status", "values": ["Failed"]},
        ]}),
        Vec::new(),
    ));

    for (request, expected) in cases {
        let case = request.to_string();
        let body = get_org(&router, &case, request).await;
        assert_eq!(
            reported_ids(&body),
            expected,
            "case {case:?}: expected the accounts {expected:?}, got {body}"
        );
        for status in reported_statuses(&body) {
            assert!(
                STATUS_VALUES.contains(&status.as_str()),
                "case {case:?}: every reported status is a modelled Status, got {status}"
            );
        }
    }
}

/// A filter or a page token the service cannot honour is REFUSED, never answered with a page
/// of something else — the handler's own 400 for what it checks, the model's for what the
/// request boundary checks.
#[tokio::test]
async fn an_unhonourable_filter_or_token_is_refused() {
    let (router, engine) = emulator_with_corpus();
    let before = snapshot(&engine);

    // Rows the HANDLER refuses, with the error the operation models for a bad parameter.
    let token = get_org(&router, "a bounded first page", json!({"maxResults": 1}))
        .await
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a first page of five leaves a next page")
        .to_string();
    for (case, request) in [
        (
            "a filter with no name",
            json!({"filters": [{"values": ["Active"]}]}),
        ),
        (
            "a Status value outside the enum",
            json!({"filters": [{"name": "Status", "values": ["Enabled"]}]}),
        ),
        (
            "a Status value in the wrong case",
            json!({"filters": [{"name": "Status", "values": ["active"]}]}),
        ),
        (
            "a token this service never issued",
            json!({"nextToken": "not-a-token"}),
        ),
        (
            "a token that decodes to nothing this service issued",
            json!({"nextToken": "6162636465"}),
        ),
        (
            "a token resumed with a different selection",
            json!({
                "maxResults": 1,
                "nextToken": token,
                "filters": [{"name": "Status", "values": ["Active"]}],
            }),
        ),
    ] {
        let (status, body) = call(&router, "GetEnrollmentStatusesForOrganization", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected the operation's own refusal, got {body}"
        );
    }

    // Rows the MODEL refuses before the handler runs: a filter name outside
    // `EnrollmentFilterName`, and a `maxResults` one past each published bound of 0..1000.
    for (case, request) in [
        (
            "a filter name the model does not declare",
            json!({"filters": [{"name": "Finding", "values": ["Active"]}]}),
        ),
        (
            "a maxResults above the modelled range",
            json!({"maxResults": MAX_PAGE_SIZE + 1}),
        ),
        (
            "a maxResults below the modelled range",
            json!({"maxResults": -1}),
        ),
    ] {
        let (status, body) = call(&router, "GetEnrollmentStatusesForOrganization", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValueException",
            "case {case:?}: expected the model's own refusal, got {body}"
        );
    }

    // Both published bounds of `maxResults` are inside the range and answered.
    for size in [0, MAX_PAGE_SIZE] {
        let body = get_org(&router, &format!("maxResults={size}"), json!({"maxResults": size})).await;
        assert_eq!(
            reported_ids(&body).len(),
            corpus().len(),
            "maxResults={size}: every member account fits one page, got {body}"
        );
    }

    assert_eq!(
        snapshot(&engine),
        before,
        "a refused or accepted read must not write to the store"
    );
}

/// An `UpdateEnrollmentStatus` call that includes member accounts writes its status to every
/// member account the organization holds; one that does not leaves them exactly as they
/// were. A member account reports `Pending` until the enrollment reaches it, and the
/// account's own `numberOfMemberAccountsOptedIn` counts the same records.
#[tokio::test]
async fn enrolling_the_organization_moves_every_member_account_it_includes() {
    // The rows cross the two statuses a client may set with `includeMemberAccounts`.
    let cases = [
        ("Active", true, "Active"),
        ("Inactive", true, "Inactive"),
        ("Active", false, ""),
        ("Inactive", false, ""),
    ];
    for (status, include_members, settled_status) in cases {
        let case = format!("status={status} includeMemberAccounts={include_members}");
        let (router, engine) = emulator_with_corpus();
        let before = get_org(&router, &case, json!({})).await;
        let stored_before = snapshot(&engine);

        update_enrollment(&router, status, include_members).await;
        let during = get_org(&router, &case, json!({})).await;

        if !include_members {
            assert_eq!(
                during, before,
                "case {case:?}: a call that includes no member account moves none, got {during}"
            );
            assert!(
                snapshot(&engine).contains("200000000001"),
                "case {case:?}: the member accounts are still stored"
            );
            settle().await;
            let after = get_org(&router, &case, json!({})).await;
            assert_eq!(
                after, before,
                "case {case:?}: and none of them moves later either, got {after}"
            );
            // The count still agrees with the rows: one recorded member account is Active.
            let enrollment = get_enrollment(&router).await;
            assert_eq!(
                enrollment["numberOfMemberAccountsOptedIn"],
                json!(2),
                "case {case:?}: the two recorded Active member accounts are counted, got \
                 {enrollment}"
            );
            continue;
        }

        // The enrollment is written for every member account and takes a moment to reach
        // it, so straight after the call every row is the transient `Pending`.
        for row in during["accountEnrollmentStatuses"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected rows, got {during}"))
        {
            assert_eq!(
                row["status"], "Pending",
                "case {case:?}: a just-enrolled member account is Pending, got {row}"
            );
            assert_eq!(
                row["statusReason"], MEMBER_ENROLLMENT_PENDING_REASON,
                "case {case:?}: expected the pending reason, got {row}"
            );
            assert!(
                row["lastUpdatedTimestamp"].as_f64().unwrap_or_default() > 0.0,
                "case {case:?}: the enrollment was just written, got {row}"
            );
        }
        let enrollment = get_enrollment(&router).await;
        assert_eq!(
            enrollment["numberOfMemberAccountsOptedIn"],
            json!(0),
            "case {case:?}: a member account still Pending is not opted in, got {enrollment}"
        );

        // Once it has reached them, every member account reports the status the call asked
        // for, with the reason that status carries.
        settle().await;
        let after = get_org(&router, &case, json!({})).await;
        assert_eq!(
            reported_ids(&after),
            reported_ids(&before),
            "case {case:?}: enrolling changes no member account's identity, got {after}"
        );
        assert_eq!(
            reported_statuses(&after),
            vec![settled_status.to_string(); corpus().len()],
            "case {case:?}: expected every member account at {settled_status}, got {after}"
        );
        for row in after["accountEnrollmentStatuses"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected rows, got {after}"))
        {
            assert_eq!(
                row["statusReason"],
                json!(enrollment_status_reason(settled_status)),
                "case {case:?}: expected the reason that status carries, got {row}"
            );
        }

        // The account's own count is counted over those same records.
        let expected_opted_in = i64::from(settled_status == "Active") * corpus().len() as i64;
        let enrollment = get_enrollment(&router).await;
        assert_eq!(
            enrollment["numberOfMemberAccountsOptedIn"],
            json!(expected_opted_in),
            "case {case:?}: expected {expected_opted_in} opted-in member accounts, got \
             {enrollment}"
        );
        assert_eq!(
            enrollment["memberAccountsEnrolled"],
            json!(true),
            "case {case:?}: the account's record confirms member accounts were included, got \
             {enrollment}"
        );
        assert_ne!(
            snapshot(&engine),
            stored_before,
            "case {case:?}: enrolling member accounts writes their enrollment"
        );
    }
}

/// `maxResults` caps a page and the `nextToken` resumes exactly where the previous page
/// stopped: whatever page size walks the organization, the sequence it reports is the same
/// one an unpaged read reports, each account exactly once.
#[tokio::test]
async fn pages_walk_every_member_account_exactly_once() {
    let (router, _engine) = emulator_with_corpus();
    let all = reported_ids(&get_org(&router, "the unpaged read", json!({})).await);
    assert_eq!(all.len(), corpus().len(), "the whole organization: {all:?}");

    // Every page size that splits the organization, no size at all, the lower bound `0`
    // (which carries no documented meaning and so reads as "no size asked for") and the
    // ceiling the model puts on `maxResults`.
    for page_size in [None, Some(0), Some(1), Some(2), Some(3), Some(5), Some(MAX_PAGE_SIZE)] {
        let case = format!("maxResults={page_size:?}");
        let mut request = json!({});
        if let Some(size) = page_size {
            request["maxResults"] = json!(size);
        }
        let pages = walk_pages(&router, &case, &request, all.len() + 2).await;
        let cap = page_size
            .filter(|size| *size > 0)
            .map_or(ENROLLMENT_STATUSES_MAX_PAGE_SIZE, |size| {
                size.min(ENROLLMENT_STATUSES_MAX_PAGE_SIZE)
            });
        for page in &pages {
            assert!(
                page.len() <= cap,
                "case {case:?}: a page must not exceed {cap}, got {} rows",
                page.len()
            );
        }
        let walked: Vec<String> = pages.concat();
        assert_eq!(
            walked, all,
            "case {case:?}: the walk must report the same sequence once, got {walked:?}"
        );
    }

    // A filtered walk pages the SELECTED sequence, not the whole organization.
    let request = json!({"maxResults": 1, "filters": [{"name": "Status", "values": ["Active"]}]});
    let pages = walk_pages(&router, "a filtered walk", &request, all.len() + 2).await;
    assert_eq!(
        pages.concat(),
        vec!["200000000001".to_string(), "200000000003".to_string()],
        "a filtered walk reports only the selected accounts, got {pages:?}"
    );
}

/// No `maxResults` a request may carry can make a page larger than the documented maximum of
/// 100 statuses, and the rest is still reachable through the `nextToken`.
#[tokio::test]
async fn a_page_never_exceeds_the_documented_maximum() {
    let (router, engine) = emulator_with_engine();
    let members: Vec<MemberAccountEnrollment> = (0..ENROLLMENT_STATUSES_MAX_PAGE_SIZE + 5)
        .map(|index| recorded(&format!("3000{index:08}"), "Active"))
        .collect();
    record(&engine, &members);

    let body = get_org(
        &router,
        "a maxResults above the documented page maximum",
        json!({"maxResults": MAX_PAGE_SIZE}),
    )
    .await;
    assert_eq!(
        reported_ids(&body).len(),
        ENROLLMENT_STATUSES_MAX_PAGE_SIZE,
        "a page is capped at {ENROLLMENT_STATUSES_MAX_PAGE_SIZE} statuses, got {} rows",
        reported_ids(&body).len()
    );
    assert!(
        body["nextToken"].is_string(),
        "the rest of the organization is still reachable, got {body}"
    );

    let pages = walk_pages(
        &router,
        "the whole walk",
        &json!({"maxResults": MAX_PAGE_SIZE}),
        members.len() + 2,
    )
    .await;
    let walked: Vec<String> = pages.concat();
    let mut expected: Vec<String> = members
        .iter()
        .map(|member| member.account_id.clone())
        .collect();
    expected.sort();
    assert_eq!(
        walked, expected,
        "the walk must report every member account exactly once, got {} rows",
        walked.len()
    );
}

/// `GetEnrollmentStatus$numberOfMemberAccountsOptedIn` counts the very member accounts
/// `GetEnrollmentStatusesForOrganization` reports as `Active`, so the two readers of one
/// organization cannot disagree.
#[tokio::test]
async fn the_opted_in_count_agrees_with_the_reported_rows() {
    let (router, engine) = emulator_with_corpus();

    for (case, extra) in [
        ("the recorded organization", Vec::new()),
        (
            "one more Active member account",
            vec![recorded("200000000006", "Active")],
        ),
        (
            "one more Failed member account",
            vec![recorded("200000000007", "Failed")],
        ),
        (
            "an Inactive account turned Active",
            vec![recorded("200000000002", "Active")],
        ),
    ] {
        record(&engine, &extra);
        let active = get_org(
            &router,
            case,
            json!({"filters": [{"name": "Status", "values": ["Active"]}]}),
        )
        .await;
        let reported = reported_ids(&active).len();
        let enrollment = get_enrollment(&router).await;
        assert_eq!(
            enrollment["numberOfMemberAccountsOptedIn"],
            json!(reported),
            "case {case:?}: expected the {reported} Active rows to be the counted ones, got \
             {enrollment}"
        );
    }
}
