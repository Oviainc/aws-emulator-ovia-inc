// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for paging a NARROWED listing: `ListSessions` with `Filters` and a
//! `NextToken`, across the writes that move a session out of what those filters admit.
//!
//! The rule under test: whether a `NextToken` was handed out is decided against every
//! record the listing pages over — one team's sessions — and never against the narrower
//! set the request's own `Filters` admit, so a token stays resumable while the session it
//! names is the team's, whatever the filters now admit and however that session's status
//! has moved since the page that issued the token. The page it resumes is the sessions
//! ordered after it that the filters admit NOW, so a client paging a narrowed listing
//! sees each session at most once and reaches every page, down to an empty final one.
//!
//! The rows span the class the rule generalises over: which documented filter field
//! narrows the listing (`SessionStatus`, `ActionName`, `InitiationTime`, and two fields
//! combined), which operator it uses, how wide a page is asked for, and what moves between
//! two pages — a session the first page REPORTED leaving the filter, one it had not
//! reported yet leaving it, the whole remainder leaving it, and nothing moving at all. The
//! move itself is made two ways: through `CancelSession`, and by the clock alone (a
//! session whose expiration has passed, which the store owes `FAILED`).
//!
//! The provenance refusal is pinned in the same shape: a narrowed listing still refuses
//! every token it did not hand out with the one sentence this service states for the class.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON envelope
//! and the HTTP status are all exercised.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{active_team, expire_session, message, wait_for_the_clock};
use crate::approval_team_lifecycle::{error_code, text, Harness};
use crate::session_cancellation::cancel_session;
use crate::session_listing::{entries, list_sessions, listed_arns, start_sessions};

/// `MaxResults`' modelled maximum (`ListSessionsRequest$MaxResults` is
/// `range(min: 1, max: 20)`): wide enough that one page reports every session each
/// fixture below holds.
const WIDEST_PAGE: i64 = 20;

/// The action every session of this model carries, since `StartApprovalTeamBaseline` is
/// the one operation that starts one (`state::SessionKind::action_name`).
const ACTION: &str = "StartApprovalTeamBaseline";

/// An instant every session this fixture starts is initiated before, for the time filter
/// rows. It is only an upper bound: nothing in a response is compared against it.
const AFTER_EVERY_SESSION: &str = "2099-01-01T00:00:00Z";

/// What moves between the first page and the rest of the walk.
#[derive(Clone, Copy, Debug)]
enum Change {
    /// Nothing moves: the control, where paging a narrowed listing is just paging.
    Nothing,
    /// `CancelSession` on the team's session at this start position, which leaves it
    /// `CANCELLED`.
    Cancel(usize),
    /// The clock's own move: the session at this start position has passed its
    /// expiration, so the store owes it `FAILED` on the next call.
    Expire(usize),
}

/// One row of the narrowed-paging table.
struct Case {
    case: &'static str,
    team_name: &'static str,
    /// How many sessions the team holds, in start order.
    sessions: usize,
    /// The `MaxResults` every page of the walk asks for.
    page_size: i64,
    /// The `Filters` every page of the walk carries.
    filters: Value,
    change: Change,
}

/// The table: rows differ along the filter field and operator, the page width, how many
/// sessions the team holds, and which session moves out of the filter between pages.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a status filter, the session the first page reported cancelled",
            team_name: "narrowed-cancel-reported",
            sessions: 3,
            // `MaxResults` is `range(min: 1, …)` — the narrowest page a client may ask for.
            page_size: 1,
            filters: json!([{ "FieldName": "SessionStatus", "Operator": "EQ", "Value": "PENDING" }]),
            change: Change::Cancel(0),
        },
        Case {
            case: "a status filter, a session the first page had not reported cancelled",
            team_name: "narrowed-cancel-unreported",
            sessions: 4,
            page_size: 1,
            filters: json!([{ "FieldName": "SessionStatus", "Operator": "EQ", "Value": "PENDING" }]),
            change: Change::Cancel(2),
        },
        Case {
            case: "a status filter, the reported session failed by the clock",
            team_name: "narrowed-expired-reported",
            sessions: 3,
            page_size: 1,
            filters: json!([{ "FieldName": "SessionStatus", "Operator": "EQ", "Value": "PENDING" }]),
            change: Change::Expire(0),
        },
        Case {
            case: "a status filter, every session left to report cancelled",
            team_name: "narrowed-empty-tail",
            sessions: 2,
            page_size: 1,
            filters: json!([{ "FieldName": "SessionStatus", "Operator": "EQ", "Value": "PENDING" }]),
            change: Change::Cancel(1),
        },
        Case {
            case: "a negated status filter, the last session the page reported cancelled",
            team_name: "narrowed-negated-status",
            sessions: 5,
            page_size: 2,
            filters: json!([{ "FieldName": "SessionStatus", "Operator": "NE", "Value": "CANCELLED" }]),
            change: Change::Cancel(1),
        },
        Case {
            case: "a status and a time filter together, the reported session cancelled",
            team_name: "narrowed-two-filters",
            sessions: 4,
            page_size: 1,
            filters: json!([
                { "FieldName": "SessionStatus", "Operator": "NE", "Value": "CANCELLED" },
                { "FieldName": "InitiationTime", "Operator": "LTE", "Value": AFTER_EVERY_SESSION },
            ]),
            change: Change::Cancel(0),
        },
        Case {
            case: "an action filter, with nothing moving between the pages",
            team_name: "narrowed-action-control",
            sessions: 3,
            page_size: 1,
            filters: json!([{ "FieldName": "ActionName", "Operator": "EQ", "Value": ACTION }]),
            change: Change::Nothing,
        },
        Case {
            case: "a time filter, the reported session cancelled but still admitted by it",
            team_name: "narrowed-time-filter",
            sessions: 3,
            page_size: 1,
            filters: json!([{ "FieldName": "InitiationTime", "Operator": "LT", "Value": AFTER_EVERY_SESSION }]),
            change: Change::Cancel(0),
        },
    ]
}

/// One narrowed page.
async fn narrowed_page(
    harness: &Harness,
    team_arn: &str,
    filters: &Value,
    page_size: i64,
    token: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({ "MaxResults": page_size, "Filters": filters.clone() });
    if let Some(next) = token {
        body["NextToken"] = json!(next);
    }
    list_sessions(harness, team_arn, body).await
}

/// Apply a row's change to the team's sessions.
async fn apply(harness: &Harness, sessions: &[String], change: Change, case: &str) {
    match change {
        Change::Nothing => {}
        Change::Cancel(index) => {
            let (status, body) = cancel_session(harness, &sessions[index]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: cancelling session {index} returned {status} with {body:?}"
            );
        }
        Change::Expire(index) => {
            // Only the DUE INSTANT is moved; the status the store then owes the session is
            // the service's own.
            expire_session(
                harness,
                &sessions[index],
                SystemTime::now() - Duration::from_secs(1),
            );
        }
    }
}

/// The rest of the walk, resumed from `token`: every session the following pages report,
/// in the order they reported them.
async fn walk_from(
    harness: &Harness,
    team_arn: &str,
    filters: &Value,
    page_size: i64,
    token: String,
    case: &str,
) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut token = Some(token);
    let mut page_number = 2;
    while let Some(next) = token {
        let (status, page) = narrowed_page(harness, team_arn, filters, page_size, Some(&next)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: page {page_number} resumed from a token this listing handed out \
             returned {status} with {page:?}"
        );
        let reported = listed_arns(&page);
        let width = usize::try_from(page_size).expect("a page size");
        assert!(
            reported.len() <= width,
            "case {case:?}: page {page_number} asked for at most {width} entries, got {} in \
             {page:?}",
            reported.len()
        );
        collected.extend(reported);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        page_number += 1;
        assert!(
            page_number <= 100,
            "case {case:?}: the pages never ran out, which is a paging loop"
        );
    }
    collected
}

/// A token this narrowed listing handed out is answered, and the walk it resumes reports
/// every session the filters still admit, each exactly once — whatever moved out of those
/// filters between the two pages.
#[tokio::test]
async fn a_token_a_narrowed_listing_issued_resumes_after_a_session_leaves_the_filter() {
    for case in cases() {
        let label = case.case;
        let harness = Harness::new();
        let team_arn = active_team(&harness, case.team_name, 2, 1).await;
        let sessions = start_sessions(&harness, &team_arn, case.sessions, label).await;
        // Past the activation delay every session is `PENDING`, so a status filter
        // narrows the fixture by the writes below rather than by the clock.
        wait_for_the_clock(state::SESSION_ACTIVATION_DELAY);

        let (status, first) =
            narrowed_page(&harness, &team_arn, &case.filters, case.page_size, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first narrowed page returned {status} with {first:?}"
        );
        let reported_first = listed_arns(&first);
        let token = text(&first, "NextToken");
        assert!(
            !token.is_empty(),
            "case {label:?}: a fixture with sessions left over must hand out a token on a page \
             of {}, got {first:?}",
            case.page_size
        );
        let last_reported = reported_first
            .last()
            .unwrap_or_else(|| panic!("case {label:?}: a page that hands out a token reported a session"))
            .clone();
        let stopped_at = sessions
            .iter()
            .position(|arn| *arn == last_reported)
            .unwrap_or_else(|| panic!("case {label:?}: {last_reported} is not a session of this team"));

        apply(&harness, &sessions, case.change, label).await;

        // What the filters admit NOW, read through the same operation unpaged: the walk
        // below must report exactly those of them ordered after where the first page
        // stopped.
        let (status, whole) =
            narrowed_page(&harness, &team_arn, &case.filters, WIDEST_PAGE, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the unpaged narrowed listing returned {status} with {whole:?}"
        );
        let admitted: BTreeSet<String> = listed_arns(&whole).into_iter().collect();
        let expected: Vec<String> = sessions
            .iter()
            .enumerate()
            .filter(|(index, arn)| *index > stopped_at && admitted.contains(*arn))
            .map(|(_, arn)| arn.clone())
            .collect();

        let walked = walk_from(
            &harness,
            &team_arn,
            &case.filters,
            case.page_size,
            token,
            label,
        )
        .await;
        assert_eq!(
            walked, expected,
            "case {label:?}: the pages after the token must report the sessions the filters \
             still admit, ordered after {last_reported}: expected {expected:?}, got {walked:?}"
        );

        let mut whole_walk = reported_first.clone();
        whole_walk.extend(walked.iter().cloned());
        let distinct: BTreeSet<&String> = whole_walk.iter().collect();
        assert_eq!(
            distinct.len(),
            whole_walk.len(),
            "case {label:?}: a session was reported twice in {whole_walk:?}"
        );
    }
}

/// The token a narrowed listing hands out for its LAST remaining session is answered with
/// an empty final page once that session leaves the filter — the end of the listing, not a
/// refusal.
#[tokio::test]
async fn a_token_whose_remaining_sessions_all_left_the_filter_answers_an_empty_final_page() {
    let harness = Harness::new();
    let team_arn = active_team(&harness, "narrowed-final-page", 2, 1).await;
    let sessions = start_sessions(&harness, &team_arn, 2, "the final page").await;
    wait_for_the_clock(state::SESSION_ACTIVATION_DELAY);
    let filters = json!([{ "FieldName": "SessionStatus", "Operator": "EQ", "Value": "PENDING" }]);

    let (status, first) = narrowed_page(&harness, &team_arn, &filters, 1, None).await;
    assert_eq!(
        status, 200,
        "the first narrowed page returned {status} with {first:?}"
    );
    let token = text(&first, "NextToken");
    assert!(
        !token.is_empty(),
        "two PENDING sessions must hand out a token on a page of one, got {first:?}"
    );

    let (status, body) = cancel_session(&harness, &sessions[1]).await;
    assert_eq!(
        status, 200,
        "cancelling the second session returned {status} with {body:?}"
    );

    let (status, second) = narrowed_page(&harness, &team_arn, &filters, 1, Some(&token)).await;
    assert_eq!(
        status, 200,
        "case \"the final page\": the token must still be answered, got {status} with {second:?}"
    );
    assert!(
        entries(&second).is_empty(),
        "case \"the final page\": no session is left for the filter, got {second:?}"
    );
    assert!(
        second.get("NextToken").is_none_or(Value::is_null),
        "case \"the final page\": the last page carries no NextToken: {second:?}"
    );
}

/// Narrowing changes nothing about PROVENANCE: a narrowed listing still refuses every
/// `NextToken` it did not hand out, with the one sentence this service states for the
/// class.
#[tokio::test]
async fn a_narrowed_listing_still_refuses_a_next_token_it_did_not_hand_out() {
    let harness = Harness::new();
    let team_arn = active_team(&harness, "narrowed-provenance", 2, 1).await;
    let other_team = active_team(&harness, "narrowed-provenance-other", 2, 1).await;
    start_sessions(&harness, &team_arn, 3, "provenance").await;
    let other_sessions = start_sessions(&harness, &other_team, 3, "provenance other").await;
    wait_for_the_clock(state::SESSION_ACTIVATION_DELAY);
    let filters = json!([{ "FieldName": "SessionStatus", "Operator": "EQ", "Value": "PENDING" }]);

    let this_listing = state::Listing::Sessions {
        approval_team_arn: &team_arn,
    };
    let other_listing = state::Listing::Sessions {
        approval_team_arn: &other_team,
    };
    // The token the OTHER team's narrowed listing handed out: a token of this operation's,
    // issued to someone paging another team's sessions.
    let (status, other_first) = narrowed_page(&harness, &other_team, &filters, 1, None).await;
    assert_eq!(
        status, 200,
        "the other team's first page returned {status} with {other_first:?}"
    );
    let other_token = text(&other_first, "NextToken");
    assert!(
        !other_token.is_empty(),
        "the other team's fixture must hand out a token: {other_first:?}"
    );

    let rows: Vec<(String, String)> = vec![
        ("text that is no token at all".to_string(), "not-a-token".to_string()),
        ("a bare position".to_string(), "1".to_string()),
        (
            "this listing's own form for the position before the first session".to_string(),
            state::position_token(this_listing, 0),
        ),
        (
            "this listing's own form for a position past every session".to_string(),
            state::position_token(this_listing, 999_999),
        ),
        (
            "the token the other team's narrowed listing handed out".to_string(),
            other_token,
        ),
        (
            "the other team's listing form for a session it holds".to_string(),
            state::position_token(other_listing, 1),
        ),
    ];

    for (row, token) in rows {
        let (status, body) = narrowed_page(&harness, &team_arn, &filters, 1, Some(&token)).await;
        assert_eq!(
            status, 400,
            "case {row:?}: a token this listing did not hand out must be refused, got {status} \
             with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {row:?}: wrong error code in {body:?}"
        );
        assert_eq!(
            message(&body),
            state::FOREIGN_NEXT_TOKEN_MESSAGE,
            "case {row:?}: expected the one pagination-token sentence, got {body:?}"
        );
        assert!(
            body.get("Sessions").is_none(),
            "case {row:?}: a refused token must not be answered with a page, got {body:?}"
        );
    }

    // The control: the other team's own token still pages the other team's sessions, so
    // the refusals above are about provenance and not about the filter.
    let (status, page) = narrowed_page(&harness, &other_team, &filters, 1, None).await;
    assert_eq!(
        status, 200,
        "the other team's listing must still page: {status} with {page:?}"
    );
    assert_eq!(
        listed_arns(&page),
        vec![other_sessions[0].clone()],
        "the other team's first page must report its first session, got {page:?}"
    );
}
