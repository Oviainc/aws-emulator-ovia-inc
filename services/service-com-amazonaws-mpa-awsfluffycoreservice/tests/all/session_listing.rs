// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListSessions`: the paged, filtered listing of ONE approval
//! team's sessions.
//!
//! The rules under test:
//!
//! * a listed session is a STORED session. Every entry is read off the
//!   [`state::Session`] record `StartApprovalTeamBaseline` wrote, so each member a
//!   listed entry reports is the one `GetSession` reports for the same ARN, and a team's
//!   listing names every session started for THAT team and no other team's.
//! * the listing is paged in INITIATION order: `NextToken` is the position the next page
//!   starts after, so a client paging with any `MaxResults` sees each session exactly
//!   once, in the order the sessions were started, and `NextToken` is absent on the last
//!   page. A token this service did not issue earns 400.
//! * `Filters` narrows the listing to the sessions satisfying EVERY filter, over the
//!   three fields `Filter$FieldName` documents for this operation (`ActionName`,
//!   `SessionStatus`, `InitiationTime`) and the operators `Filter$Operator` documents for
//!   each of them. A filter this operation cannot apply is refused 400 rather than
//!   ignored.
//! * the ARN-addressed boundary every read of this service shares: an ARN outside the
//!   caller's account is refused 403 BEFORE the lookup, a team this store does not hold
//!   is 404 with the one not-found sentence, and a value that is not a well-formed MPA
//!   ARN is refused 400 with `state::INVALID_ARN_MESSAGE`.
//! * a read changes nothing: two identical listings agree.
//!
//! The tables span the class each rule generalises over (session count, page size, which
//! field is filtered, which operator, the form the instant is written in, which member is
//! left out), not one recorded shape. Where a table names a published bound (`MaxResults`
//! 1..20) or a catalog (`FilterField`'s tokens) a comment names the modelled member it
//! comes from.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON envelope
//! and the HTTP status are all exercised.

use std::collections::BTreeSet;
use std::time::SystemTime;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{
    active_team, expire_session, get_session, message, start_baseline, wait_for_the_clock,
};
use crate::approval_team_lifecycle::{encode_label, error_code, instant, text, Harness, REGION};

/// `MaxResults` is `range(min: 1, max: 20)` with `default: 20` on `ListSessionsRequest`.
const MAX_PAGE_SIZE: i64 = 20;

/// The `MaxResults` a request that names none is answered with — the member's modelled
/// `default`.
const DEFAULT_PAGE_SIZE: i64 = 20;

/// `POST /approval-teams/{ApprovalTeamArn}/sessions/?List` — `ListSessions`. Every other
/// input member is bound to the body.
async fn list_sessions(harness: &Harness, team_arn: &str, body: Value) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!(
                "/approval-teams/{}/sessions/?List",
                encode_label(team_arn)
            ),
            Some(body),
        )
        .await
}

/// The `SessionArn`s one page reports, in the order the page lists them.
fn listed_arns(page: &Value) -> Vec<String> {
    entries(page)
        .iter()
        .map(|entry| text(entry, "SessionArn"))
        .collect()
}

/// The `Sessions` a page reports.
fn entries(page: &Value) -> Vec<Value> {
    page.get("Sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The instant an ISO 8601 date-time names, for a fixture that pins a session's
/// `InitiationTime` to the same instant a filter value below names.
fn at(iso: &str) -> SystemTime {
    let parsed = DateTime::from_str(iso, Format::DateTime)
        .unwrap_or_else(|error| panic!("{iso:?} is not a readable instant: {error}"));
    SystemTime::try_from(parsed).unwrap_or_else(|_| panic!("{iso:?} is not a holdable instant"))
}

/// Pin a stored session's `InitiationTime`, so the time filters below are asked about
/// instants the table names rather than about whatever the clock read during the test.
/// Only the recorded instant is moved; everything the listing reports is still the
/// service's own.
fn set_initiation_time(harness: &Harness, session_arn: &str, initiated: SystemTime) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let mut guard = store.lock();
    guard
        .session_mut(session_arn)
        .expect("a stored session to re-time")
        .initiation_time = initiated;
}

/// Start `count` baseline sessions on `team_arn` and return their ARNs in start order.
async fn start_sessions(harness: &Harness, team_arn: &str, count: usize, case: &str) -> Vec<String> {
    let mut arns = Vec::with_capacity(count);
    for n in 0..count {
        let (status, started) = start_baseline(harness, team_arn, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: starting session {n} returned {status} with {started:?}"
        );
        arns.push(text(&started, "BaselineSessionArn"));
    }
    arns
}

/// One row of the paging table.
struct PageCase {
    case: &'static str,
    team_name: &'static str,
    sessions: usize,
    /// The `MaxResults` the request names, or `None` to leave the member absent.
    page_size: Option<i64>,
}

/// The paging table: rows differ along how many sessions the team has and how wide a page
/// the client asks for — including the member left absent (so the modelled default
/// decides) and both published bounds of `MaxResults`.
fn page_cases() -> Vec<PageCase> {
    vec![
        PageCase {
            case: "one session, the narrowest page",
            team_name: "listing-solo",
            sessions: 1,
            // `MaxResults` is `range(min: 1, …)` — the minimum.
            page_size: Some(1),
        },
        PageCase {
            case: "five sessions, two at a time",
            team_name: "Listing.Team_2",
            sessions: 5,
            page_size: Some(2),
        },
        PageCase {
            case: "three sessions, MaxResults absent",
            team_name: "l",
            sessions: 3,
            page_size: None,
        },
        PageCase {
            case: "more sessions than the widest page holds",
            team_name: "listing-two-pages",
            // One past `MaxResults`' maximum, so the answer cannot be one page.
            sessions: usize::try_from(MAX_PAGE_SIZE).expect("a page size") + 1,
            page_size: Some(MAX_PAGE_SIZE),
        },
    ]
}

/// Page through a team's sessions with `page_size`, returning every entry in the order
/// the pages reported them.
async fn page_through(
    harness: &Harness,
    team_arn: &str,
    page_size: Option<i64>,
    case: &str,
) -> Vec<Value> {
    let width = usize::try_from(page_size.unwrap_or(DEFAULT_PAGE_SIZE)).expect("a page size");
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({});
        if let Some(size) = page_size {
            body["MaxResults"] = json!(size);
        }
        if let Some(next) = &token {
            body["NextToken"] = json!(next);
        }
        let (status, page) = list_sessions(harness, team_arn, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a page returned {status} with {page:?}"
        );
        let page_entries = entries(&page);
        assert!(
            page_entries.len() <= width,
            "case {case:?}: a page of at most {width} entries was asked for, got {} in {page:?}",
            page_entries.len()
        );
        collected.extend(page_entries);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            // The LAST page carries no `NextToken`: that is how a client knows the
            // listing is over, so it is asserted rather than left implied by the loop
            // ending.
            assert!(
                page.get("NextToken").is_none_or(Value::is_null),
                "case {case:?}: the last page must carry no NextToken: {page:?}"
            );
            break;
        }
        // A page that is followed by another is FULL: a short page with a token would
        // make a client's page count depend on something other than `MaxResults`.
        assert_eq!(
            entries(&page).len(),
            width,
            "case {case:?}: a page followed by a NextToken must be {width} wide: {page:?}"
        );
        assert!(
            collected.len() <= 1_000,
            "case {case:?}: the pages never ran out, which is a paging loop"
        );
    }
    collected
}

/// Every listed session is a stored session, reported in initiation order, paged so that
/// a client sees each one exactly once whatever `MaxResults` it asks for — and each entry
/// agrees member for member with what `GetSession` reports for the same ARN.
#[tokio::test]
async fn a_team_s_sessions_are_listed_in_initiation_order_and_read_off_the_stored_record() {
    let harness = Harness::new();

    for case in page_cases() {
        let label = case.case;
        let team_arn = active_team(&harness, case.team_name, 2, 1).await;
        let started = start_sessions(&harness, &team_arn, case.sessions, label).await;
        // Past the activation delay every session is `PENDING`, which it stays for its
        // whole 24-hour expiration — so the status two reads below see cannot move
        // between them.
        wait_for_the_clock(state::SESSION_ACTIVATION_DELAY);

        let collected = page_through(&harness, &team_arn, case.page_size, label).await;
        let listed: Vec<String> = collected
            .iter()
            .map(|entry| text(entry, "SessionArn"))
            .collect();
        assert_eq!(
            listed, started,
            "case {label:?}: expected every started session once, in start order, got {listed:?}"
        );
        assert_eq!(
            listed.iter().collect::<BTreeSet<_>>().len(),
            started.len(),
            "case {label:?}: a session was reported twice in {listed:?}"
        );

        for entry in &collected {
            let arn = text(entry, "SessionArn");
            let (status, session) = get_session(&harness, &arn).await;
            assert_eq!(
                status, 200,
                "case {label:?}: GetSession({arn}) returned {status} with {session:?}"
            );
            for member in [
                "SessionArn",
                "ApprovalTeamArn",
                "ApprovalTeamName",
                "Status",
                "ActionName",
                "ProtectedResourceArn",
                "RequesterServicePrincipal",
                "RequesterPrincipalArn",
                "RequesterAccountId",
                "RequesterRegion",
                "ActionCompletionStrategy",
            ] {
                let listed = text(entry, member);
                let read = text(&session, member);
                assert!(
                    !listed.is_empty(),
                    "case {label:?}: the listed entry reports no {member}: {entry:?}"
                );
                assert_eq!(
                    listed, read,
                    "case {label:?}: {member} must be the stored record's; the listing says \
                     {listed:?} and GetSession says {read:?}"
                );
            }
            for member in ["InitiationTime", "ExpirationTime"] {
                let listed = instant(entry, member);
                let read = instant(&session, member);
                assert!(
                    (listed - read).abs() < f64::EPSILON,
                    "case {label:?}: {member} must be the stored record's; the listing says \
                     {listed} and GetSession says {read}"
                );
            }
            assert_eq!(
                text(entry, "ApprovalTeamArn"),
                team_arn,
                "case {label:?}: a listed session must belong to the team asked for: {entry:?}"
            );
            assert_eq!(
                text(entry, "ActionName"),
                state::SessionKind::Baseline.action_name(),
                "case {label:?}: the action is the operation that started the session: {entry:?}"
            );
            assert!(
                model::SESSION_STATUS_VALUES.contains(&text(entry, "Status").as_str()),
                "case {label:?}: expected a token SessionStatus declares, got {entry:?}"
            );
            // An outstanding session has not completed, so it reports no
            // `CompletionTime`; `StatusCode` and `StatusMessage` belong to one that
            // reached a status needing an explanation.
            for member in ["CompletionTime", "StatusCode", "StatusMessage"] {
                assert!(
                    entry.get(member).is_none_or(Value::is_null),
                    "case {label:?}: an outstanding session reports no {member}: {entry:?}"
                );
            }
            assert_eq!(
                entry.get("AdditionalSecurityRequirements"),
                Some(&json!([])),
                "case {label:?}: the recorded (empty) requirement list is what is reported: \
                 {entry:?}"
            );
        }
    }
}

/// One team's listing names that team's sessions only.
#[tokio::test]
async fn a_listing_names_only_the_sessions_of_the_team_asked_for() {
    let harness = Harness::new();
    let first = active_team(&harness, "isolation-first", 1, 1).await;
    let second = active_team(&harness, "isolation-second", 1, 1).await;
    let third = active_team(&harness, "isolation-empty", 1, 1).await;

    let first_sessions = start_sessions(&harness, &first, 2, "first").await;
    let second_sessions = start_sessions(&harness, &second, 3, "second").await;

    for (case, team, expected) in [
        ("the first team", &first, first_sessions.clone()),
        ("the second team", &second, second_sessions.clone()),
        ("a team with no session", &third, Vec::new()),
    ] {
        let (status, page) = list_sessions(&harness, team, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {page:?}"
        );
        assert_eq!(
            listed_arns(&page),
            expected,
            "case {case:?}: expected exactly that team's sessions, got {page:?}"
        );
    }
}

/// One row of the filter table.
struct FilterCase {
    case: &'static str,
    filters: Value,
    /// Which of the fixture's sessions the filters admit, by start position.
    expected: Vec<usize>,
}

/// The instants the filter fixture pins its four sessions to, in start order. Written as
/// text so the filter values below and the stored records name the same instants.
const INITIATED: [&str; 4] = [
    "2026-01-01T00:00:00Z",
    "2026-01-01T01:00:00Z",
    "2026-01-01T02:00:00Z",
    "2026-01-02T00:00:00Z",
];

/// The action every session of this model reports, since `StartApprovalTeamBaseline` is
/// the one operation that starts one (`state::SessionKind::action_name`).
const ACTION: &str = "StartApprovalTeamBaseline";

/// The filter table: rows differ along which documented field is filtered, which of that
/// field's documented operators is used, how the instant is written (`Z`, a numeric UTC
/// offset), the casing of `BETWEEN`'s separator, how many filters are combined, and
/// whether the answer is all, some or none of the sessions.
fn filter_cases() -> Vec<FilterCase> {
    vec![
        FilterCase {
            case: "no filter at all",
            filters: json!([]),
            expected: vec![0, 1, 2, 3],
        },
        FilterCase {
            case: "ActionName EQ the action every session carries",
            filters: json!([{ "FieldName": "ActionName", "Operator": "EQ", "Value": ACTION }]),
            expected: vec![0, 1, 2, 3],
        },
        FilterCase {
            case: "ActionName NE the action every session carries",
            filters: json!([{ "FieldName": "ActionName", "Operator": "NE", "Value": ACTION }]),
            expected: Vec::new(),
        },
        FilterCase {
            case: "ActionName EQ another operation's name",
            filters: json!([{ "FieldName": "ActionName", "Operator": "EQ", "Value": "CancelSession" }]),
            expected: Vec::new(),
        },
        FilterCase {
            case: "ActionName CONTAINS a substring of the action",
            filters: json!([{ "FieldName": "ActionName", "Operator": "CONTAINS", "Value": "Baseline" }]),
            expected: vec![0, 1, 2, 3],
        },
        FilterCase {
            case: "ActionName NOT_CONTAINS a substring of the action",
            filters: json!([{ "FieldName": "ActionName", "Operator": "NOT_CONTAINS", "Value": "Baseline" }]),
            expected: Vec::new(),
        },
        FilterCase {
            case: "ActionName NOT_CONTAINS text the action does not carry",
            filters: json!([{ "FieldName": "ActionName", "Operator": "NOT_CONTAINS", "Value": "Cancel" }]),
            expected: vec![0, 1, 2, 3],
        },
        FilterCase {
            case: "SessionStatus EQ PENDING",
            filters: json!([{ "FieldName": "SessionStatus", "Operator": "EQ", "Value": "PENDING" }]),
            expected: vec![0, 1, 2],
        },
        FilterCase {
            case: "SessionStatus EQ FAILED",
            filters: json!([{ "FieldName": "SessionStatus", "Operator": "EQ", "Value": "FAILED" }]),
            expected: vec![3],
        },
        FilterCase {
            case: "SessionStatus NE PENDING",
            filters: json!([{ "FieldName": "SessionStatus", "Operator": "NE", "Value": "PENDING" }]),
            expected: vec![3],
        },
        FilterCase {
            case: "SessionStatus EQ a status no session of the team is in",
            filters: json!([{ "FieldName": "SessionStatus", "Operator": "EQ", "Value": "APPROVED" }]),
            expected: Vec::new(),
        },
        FilterCase {
            case: "InitiationTime GT the second session's instant",
            filters: json!([{ "FieldName": "InitiationTime", "Operator": "GT", "Value": INITIATED[1] }]),
            expected: vec![2, 3],
        },
        FilterCase {
            case: "InitiationTime GTE the second session's instant",
            filters: json!([{ "FieldName": "InitiationTime", "Operator": "GTE", "Value": INITIATED[1] }]),
            expected: vec![1, 2, 3],
        },
        FilterCase {
            case: "InitiationTime LT the second session's instant",
            filters: json!([{ "FieldName": "InitiationTime", "Operator": "LT", "Value": INITIATED[1] }]),
            expected: vec![0],
        },
        FilterCase {
            case: "InitiationTime LTE the second session's instant",
            filters: json!([{ "FieldName": "InitiationTime", "Operator": "LTE", "Value": INITIATED[1] }]),
            expected: vec![0, 1],
        },
        FilterCase {
            case: "InitiationTime GT the last session's instant",
            filters: json!([{ "FieldName": "InitiationTime", "Operator": "GT", "Value": INITIATED[3] }]),
            expected: Vec::new(),
        },
        FilterCase {
            case: "InitiationTime BETWEEN two instants, inclusive of both",
            filters: json!([{
                "FieldName": "InitiationTime",
                "Operator": "BETWEEN",
                "Value": format!("{} AND {}", INITIATED[1], INITIATED[2]),
            }]),
            expected: vec![1, 2],
        },
        FilterCase {
            case: "BETWEEN with the separator in lower case",
            filters: json!([{
                "FieldName": "InitiationTime",
                "Operator": "BETWEEN",
                "Value": format!("{} and {}", INITIATED[1], INITIATED[2]),
            }]),
            expected: vec![1, 2],
        },
        FilterCase {
            case: "BETWEEN over instants written with a numeric UTC offset",
            // The same two instants as the row above, written +01:00 rather than Z.
            filters: json!([{
                "FieldName": "InitiationTime",
                "Operator": "BETWEEN",
                "Value": "2026-01-01T02:00:00+01:00 AND 2026-01-01T03:00:00+01:00",
            }]),
            expected: vec![1, 2],
        },
        FilterCase {
            case: "BETWEEN whose lower value is above its upper one",
            filters: json!([{
                "FieldName": "InitiationTime",
                "Operator": "BETWEEN",
                "Value": format!("{} AND {}", INITIATED[2], INITIATED[1]),
            }]),
            expected: Vec::new(),
        },
        FilterCase {
            case: "a status filter and a time filter together",
            filters: json!([
                { "FieldName": "SessionStatus", "Operator": "EQ", "Value": "PENDING" },
                { "FieldName": "InitiationTime", "Operator": "GTE", "Value": INITIATED[1] },
            ]),
            expected: vec![1, 2],
        },
        FilterCase {
            case: "two time filters bounding the listing from both sides",
            filters: json!([
                { "FieldName": "InitiationTime", "Operator": "GT", "Value": INITIATED[0] },
                { "FieldName": "InitiationTime", "Operator": "LT", "Value": INITIATED[3] },
            ]),
            expected: vec![1, 2],
        },
        FilterCase {
            case: "filters that cannot both hold",
            filters: json!([
                { "FieldName": "SessionStatus", "Operator": "EQ", "Value": "PENDING" },
                { "FieldName": "SessionStatus", "Operator": "EQ", "Value": "FAILED" },
            ]),
            expected: Vec::new(),
        },
    ]
}

/// The fixture the filter table is asked about: one team with four sessions, three
/// outstanding and one expired, each pinned to one of [`INITIATED`]'s instants.
async fn filter_fixture(harness: &Harness) -> (String, Vec<String>) {
    let team_arn = active_team(harness, "filtered-team", 2, 1).await;
    let sessions = start_sessions(harness, &team_arn, INITIATED.len(), "filters").await;
    wait_for_the_clock(state::SESSION_ACTIVATION_DELAY);
    for (session, initiated) in sessions.iter().zip(INITIATED) {
        set_initiation_time(harness, session, at(initiated));
    }
    // The last session's expiration has passed, so the clock owes it `FAILED` — the one
    // status a session of this model reaches without an approver answering.
    expire_session(
        harness,
        &sessions[INITIATED.len() - 1],
        SystemTime::now() - std::time::Duration::from_secs(1),
    );
    (team_arn, sessions)
}

/// `Filters` narrows the listing to the sessions satisfying every filter, over each field
/// and operator the model documents for this operation.
#[tokio::test]
async fn the_documented_filters_narrow_the_listing() {
    let harness = Harness::new();
    let (team_arn, sessions) = filter_fixture(&harness).await;

    for case in filter_cases() {
        let label = case.case;
        let (status, page) = list_sessions(
            &harness,
            &team_arn,
            json!({ "MaxResults": MAX_PAGE_SIZE, "Filters": case.filters }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200, got {status} with {page:?}"
        );
        let expected: Vec<String> = case
            .expected
            .iter()
            .map(|index| sessions[*index].clone())
            .collect();
        assert_eq!(
            listed_arns(&page),
            expected,
            "case {label:?}: expected {expected:?}, got {page:?}"
        );
    }
}

/// Every `FilterField` token the shared enum declares, with whether `ListSessions`
/// supports it: the catalog comes from the generated `FILTER_FIELD_VALUES` list, and
/// `Filter$FieldName`'s documentation names the three this operation applies ("The
/// supported filters for ListSessions are: ActionName, SessionStatus, and
/// InitiationTime").
fn field_support() -> Vec<(&'static str, bool)> {
    vec![
        ("ActionName", true),
        ("SessionStatus", true),
        ("InitiationTime", true),
        ("ApprovalTeamName", false),
        ("Vote", false),
        ("VotingTime", false),
    ]
}

/// A filter naming a `FilterField` this operation does not support is refused, and the
/// catalog the refusal is decided against is the model's own enum.
#[tokio::test]
async fn a_filter_field_this_operation_does_not_support_is_refused() {
    let harness = Harness::new();
    let (team_arn, _) = filter_fixture(&harness).await;

    let declared: BTreeSet<&str> = field_support().iter().map(|(field, _)| *field).collect();
    let catalog: BTreeSet<&str> = model::FILTER_FIELD_VALUES.iter().copied().collect();
    assert_eq!(
        declared, catalog,
        "the table must span FilterField's own tokens; the model declares {catalog:?}"
    );

    for (field, supported) in field_support() {
        // A value and an operator each field takes, so the only thing under test is the
        // FIELD: the time field's own operator and instant, a declared status token for
        // the status field, and free text for the rest.
        let (operator, value) = match field {
            "InitiationTime" => ("GTE", INITIATED[0].to_string()),
            "SessionStatus" => ("EQ", "PENDING".to_string()),
            _ => ("EQ", "x".to_string()),
        };
        let (status, body) = list_sessions(
            &harness,
            &team_arn,
            json!({ "Filters": [{ "FieldName": field, "Operator": operator, "Value": value }] }),
        )
        .await;
        if supported {
            assert_eq!(
                status, 200,
                "case {field:?}: a supported filter must be applied, got {status} with {body:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {field:?}: an unsupported filter must be refused, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {field:?}: wrong error code in {body:?}"
        );
        assert!(
            message(&body).contains("Filters[0].FieldName"),
            "case {field:?}: the refusal must name the member at fault, got {body:?}"
        );
    }
}

/// One row of the refused-filter table: the filter sent, and the member the refusal must
/// name.
fn refused_filters() -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "CONTAINS on SessionStatus, which takes EQ | NE only",
            json!({ "FieldName": "SessionStatus", "Operator": "CONTAINS", "Value": "PENDING" }),
            "Filters[0].Operator",
        ),
        (
            "GT on ActionName, which takes no ordering operator",
            json!({ "FieldName": "ActionName", "Operator": "GT", "Value": ACTION }),
            "Filters[0].Operator",
        ),
        (
            "BETWEEN on ActionName",
            json!({ "FieldName": "ActionName", "Operator": "BETWEEN", "Value": "a AND b" }),
            "Filters[0].Operator",
        ),
        (
            "EQ on InitiationTime, which takes the ordering operators only",
            json!({ "FieldName": "InitiationTime", "Operator": "EQ", "Value": INITIATED[0] }),
            "Filters[0].Operator",
        ),
        (
            "NOT_CONTAINS on InitiationTime",
            json!({ "FieldName": "InitiationTime", "Operator": "NOT_CONTAINS", "Value": INITIATED[0] }),
            "Filters[0].Operator",
        ),
        (
            "a filter naming no field",
            json!({ "Operator": "EQ", "Value": ACTION }),
            "Filters[0].FieldName",
        ),
        (
            "a filter naming no operator",
            json!({ "FieldName": "ActionName", "Value": ACTION }),
            "Filters[0].Operator",
        ),
        (
            "a filter naming no value",
            json!({ "FieldName": "ActionName", "Operator": "EQ" }),
            "Filters[0].Value",
        ),
        (
            "a status value outside SessionStatus' token catalog",
            json!({ "FieldName": "SessionStatus", "Operator": "EQ", "Value": "NOT_A_STATUS" }),
            "Filters[0].Value",
        ),
        (
            "a status value in the wrong case",
            json!({ "FieldName": "SessionStatus", "Operator": "NE", "Value": "pending" }),
            "Filters[0].Value",
        ),
        (
            "a time filter whose value names no instant",
            json!({ "FieldName": "InitiationTime", "Operator": "GT", "Value": "yesterday" }),
            "Filters[0].Value",
        ),
        (
            "a time filter whose value is a bare date",
            json!({ "FieldName": "InitiationTime", "Operator": "LT", "Value": "2026-01-01" }),
            "Filters[0].Value",
        ),
        (
            "a BETWEEN value naming one instant only",
            json!({ "FieldName": "InitiationTime", "Operator": "BETWEEN", "Value": INITIATED[0] }),
            "Filters[0].Value",
        ),
        (
            "a BETWEEN value whose second half names no instant",
            json!({
                "FieldName": "InitiationTime",
                "Operator": "BETWEEN",
                "Value": format!("{} AND soon", INITIATED[0]),
            }),
            "Filters[0].Value",
        ),
    ]
}

/// A filter this operation cannot apply is REFUSED, so a client never reads a page as
/// narrowed when the narrowing was not applied.
#[tokio::test]
async fn a_filter_this_operation_cannot_apply_is_refused_rather_than_ignored() {
    let harness = Harness::new();
    let (team_arn, sessions) = filter_fixture(&harness).await;

    for (case, filter, member) in refused_filters() {
        let (status, body) =
            list_sessions(&harness, &team_arn, json!({ "Filters": [filter.clone()] })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: wrong error code in {body:?}"
        );
        assert!(
            message(&body).contains(member),
            "case {case:?}: the refusal must name {member}, got {body:?}"
        );

        // The refusal is about the FILTER, not about the request: the same listing with a
        // filter this operation applies is still answered.
        let (status, page) = list_sessions(
            &harness,
            &team_arn,
            json!({ "Filters": [{ "FieldName": "ActionName", "Operator": "EQ", "Value": ACTION }] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an applicable filter must still answer, got {status} with {page:?}"
        );
        assert_eq!(
            listed_arns(&page),
            sessions,
            "case {case:?}: expected every session, got {page:?}"
        );
    }
}

/// Every token `SessionStatus` declares is a value the status filter applies, and the
/// catalog it is decided against is the model's own enum: a listing filtered on each
/// token answers, and reports only sessions in that status.
#[tokio::test]
async fn the_status_filter_applies_every_token_the_model_declares() {
    let harness = Harness::new();
    let (team_arn, _) = filter_fixture(&harness).await;

    // The catalog comes from the generated `SESSION_STATUS_VALUES` list, rendered from
    // the model's own `SessionStatus` enum.
    for status in model::SESSION_STATUS_VALUES {
        let (code, page) = list_sessions(
            &harness,
            &team_arn,
            json!({ "Filters": [{ "FieldName": "SessionStatus", "Operator": "EQ", "Value": status }] }),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {status:?}: a declared status must be applied, got {code} with {page:?}"
        );
        for entry in entries(&page) {
            assert_eq!(
                text(&entry, "Status"),
                *status,
                "case {status:?}: only sessions in that status may be reported: {entry:?}"
            );
        }
    }
}

/// A `NextToken` this service did not issue is refused, and a token it did issue is
/// answered.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let harness = Harness::new();
    let team_arn = active_team(&harness, "token-team", 1, 1).await;
    let sessions = start_sessions(&harness, &team_arn, 3, "tokens").await;

    for (case, token) in [
        ("text that is not a position", "not-a-token"),
        ("a session ARN", sessions[0].as_str()),
        ("a negative position", "-1"),
        ("a position with trailing text", "1x"),
    ] {
        let (status, body) = list_sessions(
            &harness,
            &team_arn,
            json!({ "MaxResults": 1, "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: wrong error code in {body:?}"
        );
    }

    // The token the service itself issued is accepted, and resumes after the first page.
    let (status, first) = list_sessions(&harness, &team_arn, json!({ "MaxResults": 1 })).await;
    assert_eq!(status, 200, "the first page returned {first:?}");
    let token = text(&first, "NextToken");
    assert!(!token.is_empty(), "the first of three pages must carry a NextToken: {first:?}");
    let (status, second) = list_sessions(
        &harness,
        &team_arn,
        json!({ "MaxResults": 1, "NextToken": token }),
    )
    .await;
    assert_eq!(status, 200, "the second page returned {second:?}");
    assert_eq!(
        listed_arns(&second),
        vec![sessions[1].clone()],
        "the token must resume after the first page: {second:?}"
    );
}

/// The ARN-addressed boundary: 403 outside the caller's account (decided before the
/// lookup), 404 for a team this store does not hold, and 400 for a value that is not a
/// well-formed MPA ARN.
#[tokio::test]
async fn the_team_arn_earns_the_account_existence_or_form_answer() {
    let harness = Harness::new();
    // A stored team with a session, so no refusal below can be an accident of an empty
    // store.
    let stored = active_team(&harness, "boundary-team", 1, 1).await;
    let sessions = start_sessions(&harness, &stored, 1, "boundary").await;

    let own = engine::emulator_account_id();
    for (case, arn) in [
        (
            "a team in another account",
            format!("arn:aws:mpa:{REGION}:999999999999:approval-team/elsewhere"),
        ),
        (
            "a team in another account and another region",
            "arn:aws:mpa:eu-central-1:999999999999:approval-team/elsewhere".to_string(),
        ),
    ] {
        let (status, body) = list_sessions(&harness, &arn, json!({})).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected 403, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: wrong error code in {body:?}"
        );
    }

    for (case, arn) in [
        (
            "a team that was never created",
            format!("arn:aws:mpa:{REGION}:{own}:approval-team/never-created"),
        ),
        (
            "a team in another region of the caller's account",
            format!("arn:aws:mpa:eu-central-1:{own}:approval-team/elsewhere"),
        ),
        (
            "a team in another partition of the caller's account",
            format!("arn:aws-us-gov:mpa:us-gov-west-1:{own}:approval-team/elsewhere"),
        ),
    ] {
        let (status, body) = list_sessions(&harness, &arn, json!({})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: wrong error code in {body:?}"
        );
        assert_eq!(
            message(&body),
            state::RESOURCE_NOT_FOUND_MESSAGE,
            "case {case:?}: wrong not-found wording in {body:?}"
        );
    }

    // `ApprovalTeamArn` carries the modelled ARN pattern, so a value of another form is
    // refused as malformed — with the one sentence this service states for the class.
    for (case, arn) in [
        ("not an ARN at all", "not-an-arn-at-all".to_string()),
        (
            "a session ARN rather than a team ARN",
            sessions[0].clone(),
        ),
        (
            "an identity-source ARN",
            format!("arn:aws:mpa:{REGION}:{own}:identity-source/ssoins-0001"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:iam:{REGION}:{own}:role/not-an-mpa-resource"),
        ),
    ] {
        let (status, body) = list_sessions(&harness, &arn, json!({})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: wrong error code in {body:?}"
        );
        assert_eq!(
            message(&body),
            state::INVALID_ARN_MESSAGE,
            "case {case:?}: wrong refusal wording in {body:?}"
        );
    }
}

/// Listing is a READ: two identical listings agree, and the sessions a later caller sees
/// are the ones that were there before.
#[tokio::test]
async fn listing_sessions_changes_nothing_a_later_caller_sees() {
    let harness = Harness::new();
    let (team_arn, sessions) = filter_fixture(&harness).await;

    let request = json!({
        "MaxResults": 2,
        "Filters": [{ "FieldName": "SessionStatus", "Operator": "EQ", "Value": "PENDING" }],
    });
    let (first_status, first) = list_sessions(&harness, &team_arn, request.clone()).await;
    let (second_status, second) = list_sessions(&harness, &team_arn, request).await;
    assert_eq!(
        (first_status, &first),
        (second_status, &second),
        "two identical listings must agree"
    );

    let (status, page) = list_sessions(
        &harness,
        &team_arn,
        json!({ "MaxResults": MAX_PAGE_SIZE }),
    )
    .await;
    assert_eq!(status, 200, "the full listing returned {page:?}");
    assert_eq!(
        listed_arns(&page),
        sessions,
        "every session must still be listed after the reads: {page:?}"
    );
    for session in &sessions {
        let (status, read) = get_session(&harness, session).await;
        assert_eq!(
            status, 200,
            "GetSession({session}) returned {status} with {read:?} after the listings"
        );
    }
}
