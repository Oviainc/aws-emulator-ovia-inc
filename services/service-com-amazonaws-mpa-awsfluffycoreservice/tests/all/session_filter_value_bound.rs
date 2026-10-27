// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the one thing a `ListSessions` filter refusal may never do: carry
//! a `Message` longer than that member's own modelled bound because of how long the value
//! the request carried was.
//!
//! The rule: a refusal is a MODELLED response shape, so a request whose members all stay
//! inside their own modelled constraints can never make this operation answer outside
//! them — every refusal this operation builds by interpolating a value the request carried
//! is therefore held to the bound its own `Message` member declares, and cut on a
//! character boundary when the sentence would pass it.
//!
//! `Filter$Value` is `@length(max: 1000)`
//! (`smithy_query(query="shape",shape="Filter")`) and every error this model declares
//! gives its `Message` member `@length(max: 1000)`
//! (`smithy_query(query="shape",shape="ValidationException")`), so the longest value a
//! conforming request may carry fills the WHOLE message it is quoted into: the prose
//! around it has to come off the end rather than off the bound.
//!
//! The table spans the class the rule generalises over, not one shape of request:
//!
//! * which refusal answers — the `SessionStatus` catalog refusal (the one that quotes the
//!   value back), the `InitiationTime` unreadable-instant refusal and the `BETWEEN`
//!   form refusal (which name the member at fault and quote nothing), so a row where the
//!   bound bites sits beside rows where it must not;
//! * how long the value is — one character, an ordinary token, the length that lands the
//!   sentence exactly ON the bound, one character past it, and `Filter$Value`'s published
//!   maximum;
//! * what the value is made of — one-, two- and four-byte characters, so the byte the cut
//!   falls on lands inside a character;
//! * which operator the field was filtered with (`EQ`, `NE`);
//! * where the filter sits in `Filters` (`@length(0..10)`), since the refusal names the
//!   index: the first position and the last one a conforming request may use;
//! * whether the store holds the team at all, since the filters are compiled before the
//!   team is resolved.
//!
//! The second test is the other side of the same rule: a long value the operation CAN
//! apply is applied, not refused — the bound is on the refusal's message, not on what a
//! client may filter for.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the error envelope
//! and the HTTP status are all exercised.

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::ops::types::shared_types as model;
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_sessions::{active_team, message, start_baseline};
use crate::approval_team_lifecycle::{encode_label, error_code, expected_arn, text, Harness};

/// `Filter$Value` is `@length(max: 1000)` — the longest value a conforming request may
/// carry in a filter.
const MAX_FILTER_VALUE: usize = 1000;

/// Every error shape this model declares gives its `Message` member
/// `@length(max: 1000)`.
const MAX_MESSAGE: usize = 1000;

/// `Filters` is `@length(min: 0, max: 10)`, so the last index a conforming request may
/// name a filter at is 9.
const LAST_FILTER_INDEX: usize = 9;

/// `POST /approval-teams/{ApprovalTeamArn}/sessions/?List` — `ListSessions`.
async fn list_sessions(harness: &Harness, team_arn: &str, filters: Value) -> (u16, Value) {
    harness
        .call(
            "POST",
            &format!("/approval-teams/{}/sessions/?List", encode_label(team_arn)),
            Some(json!({ "Filters": filters })),
        )
        .await
}

/// A `SessionStatus` filter.
fn status_filter(operator: &str, value: &str) -> Value {
    json!({ "FieldName": "SessionStatus", "Operator": operator, "Value": value })
}

/// An `InitiationTime` filter.
fn time_filter(operator: &str, value: &str) -> Value {
    json!({ "FieldName": "InitiationTime", "Operator": operator, "Value": value })
}

/// A filter this operation can apply, used to push the filter at fault to a later index.
fn applicable_filter() -> Value {
    json!({ "FieldName": "ActionName", "Operator": "CONTAINS", "Value": "x" })
}

/// The sentence the catalog refusal words before the bound is applied. The catalog is the
/// model's own enum, as the generated `SESSION_STATUS_VALUES` renders it.
fn unknown_status_sentence(index: usize, value: &str) -> String {
    format!(
        "Filters[{index}].Value '{value}' is not a status a session can be in; the \
         SessionStatus filter accepts {}.",
        model::SESSION_STATUS_VALUES.join(" | ")
    )
}

/// The sentence the unreadable-instant refusal words. It names the member at fault and
/// quotes no value, so its length does not depend on the request at all.
fn unreadable_instant_sentence(index: usize) -> String {
    format!(
        "Filters[{index}].Value must be an ISO 8601 date-time, like '2026-01-01T00:00:00Z', \
         for the InitiationTime filter."
    )
}

/// The sentence the `BETWEEN` form refusal words.
fn malformed_between_sentence(index: usize) -> String {
    format!(
        "Filters[{index}].Value must name two values in the form 'a AND b' for the BETWEEN \
         operator."
    )
}

/// Which team the request addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Team {
    /// A team the store does not hold: the filters are compiled before the team is
    /// resolved, so the refusal is reachable with nothing stored at all.
    Absent,
    /// A stored team with a session, so no refusal below is an accident of an empty store.
    Stored,
}

/// One row: the filters a request carries, and the sentence the rule words for it before
/// the bound is applied.
struct Case {
    case: String,
    team: Team,
    /// The index of the filter at fault, which the refusal names.
    index: usize,
    filters: Vec<Value>,
    full: String,
}

/// A row whose single filter is the one at fault.
fn single(case: &str, team: Team, filter: Value, full: String) -> Case {
    Case {
        case: case.to_string(),
        team,
        index: 0,
        filters: vec![filter],
        full,
    }
}

fn cases() -> Vec<Case> {
    // The value length that lands the catalog refusal exactly ON the bound, derived from
    // the sentence the rule words rather than written out, so the rows either side of the
    // bound cannot drift from the wording.
    let on_the_bound = MAX_MESSAGE - unknown_status_sentence(0, "").len();
    assert!(
        on_the_bound < MAX_FILTER_VALUE,
        "a value landing the refusal on the bound ({on_the_bound} characters) must itself fit \
         Filter$Value's maximum of {MAX_FILTER_VALUE}"
    );

    // Values of equal BYTE length made of characters of different widths: 1000 bytes is
    // Filter$Value's maximum however the bound is counted, and the byte the cut falls on
    // lands inside a character for the wider two.
    let two_byte = "é".repeat(MAX_FILTER_VALUE / "é".len());
    let four_byte = "𝄞".repeat(MAX_FILTER_VALUE / "𝄞".len());

    let mut cases = vec![
        single(
            "the shortest value outside the status catalog",
            Team::Absent,
            status_filter("EQ", "x"),
            unknown_status_sentence(0, "x"),
        ),
        single(
            "an ordinary token outside the status catalog",
            Team::Absent,
            status_filter("EQ", "NOT_A_STATUS"),
            unknown_status_sentence(0, "NOT_A_STATUS"),
        ),
        single(
            // The catalog is case-sensitive, so a declared token in another case is
            // outside it: an ordinary-length refusal that is named in full.
            "a declared status token in the wrong case",
            Team::Absent,
            status_filter("NE", "pending"),
            unknown_status_sentence(0, "pending"),
        ),
        single(
            "a value landing the refusal exactly on the message bound",
            Team::Absent,
            status_filter("EQ", &"x".repeat(on_the_bound)),
            unknown_status_sentence(0, &"x".repeat(on_the_bound)),
        ),
        single(
            "a value landing the refusal one character past the message bound",
            Team::Absent,
            status_filter("EQ", &"x".repeat(on_the_bound + 1)),
            unknown_status_sentence(0, &"x".repeat(on_the_bound + 1)),
        ),
        single(
            "a value at Filter$Value's maximum",
            Team::Absent,
            status_filter("EQ", &"x".repeat(MAX_FILTER_VALUE)),
            unknown_status_sentence(0, &"x".repeat(MAX_FILTER_VALUE)),
        ),
        single(
            "a value at Filter$Value's maximum, filtered with the other operator",
            Team::Absent,
            status_filter("NE", &"x".repeat(MAX_FILTER_VALUE)),
            unknown_status_sentence(0, &"x".repeat(MAX_FILTER_VALUE)),
        ),
        single(
            "a maximal value of two-byte characters",
            Team::Absent,
            status_filter("EQ", &two_byte),
            unknown_status_sentence(0, &two_byte),
        ),
        single(
            "a maximal value of four-byte characters",
            Team::Absent,
            status_filter("EQ", &four_byte),
            unknown_status_sentence(0, &four_byte),
        ),
        single(
            "a maximal value against a team the store holds",
            Team::Stored,
            status_filter("EQ", &"x".repeat(MAX_FILTER_VALUE)),
            unknown_status_sentence(0, &"x".repeat(MAX_FILTER_VALUE)),
        ),
        single(
            // A refusal that quotes no value: its sentence is the same length whatever the
            // request carried, so a maximal value may not shorten it.
            "a maximal value naming no instant",
            Team::Absent,
            time_filter("GT", &"x".repeat(MAX_FILTER_VALUE)),
            unreadable_instant_sentence(0),
        ),
        single(
            "a maximal value that is not two values in the BETWEEN form",
            Team::Absent,
            time_filter("BETWEEN", &"x".repeat(MAX_FILTER_VALUE)),
            malformed_between_sentence(0),
        ),
    ];

    // The same refusals at the LAST index a conforming request may carry a filter at,
    // behind nine filters this operation can apply.
    for (case, filter, full) in [
        (
            "a maximal value at the last filter index",
            status_filter("EQ", &"x".repeat(MAX_FILTER_VALUE)),
            unknown_status_sentence(LAST_FILTER_INDEX, &"x".repeat(MAX_FILTER_VALUE)),
        ),
        (
            "a maximal value naming no instant at the last filter index",
            time_filter("LTE", &"x".repeat(MAX_FILTER_VALUE)),
            unreadable_instant_sentence(LAST_FILTER_INDEX),
        ),
    ] {
        let mut filters: Vec<Value> = (0..LAST_FILTER_INDEX).map(|_| applicable_filter()).collect();
        filters.push(filter);
        cases.push(Case {
            case: case.to_string(),
            team: Team::Absent,
            index: LAST_FILTER_INDEX,
            filters,
            full,
        });
    }

    cases
}

/// The team a row addresses: a stored, active team with one session, or an ARN the store
/// holds nothing for.
async fn team_arn(harness: &Harness, team: Team, case: &str) -> String {
    match team {
        Team::Absent => expected_arn("no-such-team"),
        Team::Stored => {
            let arn = active_team(harness, "filter-value-bound", 1, 1).await;
            let (status, started) = start_baseline(harness, &arn, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the session a stored-team row needs was not started: \
                 {started:?}"
            );
            arn
        }
    }
}

/// Every filter refusal is the sentence the rule words, cut to the bound its own `Message`
/// member declares and to nothing shorter — so a request inside `Filter$Value`'s own
/// maximum is never answered outside `ValidationException$Message`'s.
#[tokio::test]
async fn a_filter_refusal_stays_inside_its_modelled_message_bound_however_long_the_value_is() {
    let mut cut = 0_usize;
    let mut uncut = 0_usize;

    for case in cases() {
        // A store of its own per row: the stored-team rows create the same team.
        let harness = Harness::new();
        let arn = team_arn(&harness, case.team, &case.case).await;
        let (status, body) = list_sessions(&harness, &arn, Value::Array(case.filters)).await;

        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {body:?}",
            case.case
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {:?}: expected a ValidationException, got {body:?}",
            case.case
        );

        let actual = message(&body);
        assert!(
            actual.starts_with(&format!("Filters[{}].Value", case.index)),
            "case {:?}: the refusal must name the member at fault, got {actual:?}",
            case.case
        );
        assert!(
            actual.len() <= MAX_MESSAGE,
            "case {:?}: the refusal is {} bytes, past the modelled bound of {MAX_MESSAGE}: \
             {actual:?}",
            case.case,
            actual.len()
        );
        assert!(
            actual.chars().count() <= MAX_MESSAGE,
            "case {:?}: the refusal is {} characters, past the modelled bound of {MAX_MESSAGE}",
            case.case,
            actual.chars().count()
        );
        assert!(
            case.full.starts_with(&actual),
            "case {:?}: the refusal is not the rule's sentence cut to the bound: {actual:?} \
             against {:?}",
            case.case,
            case.full
        );
        if case.full.len() <= MAX_MESSAGE {
            uncut += 1;
            assert_eq!(
                actual, case.full,
                "case {:?}: a refusal inside the bound was shortened to {actual:?}",
                case.case
            );
        } else {
            cut += 1;
            // Cut on a character boundary, so at most the width of one character is given
            // up: a cut giving up more would be cutting further than the bound asks.
            let widest = case
                .full
                .chars()
                .map(char::len_utf8)
                .max()
                .unwrap_or(1);
            assert!(
                actual.len() + widest > MAX_MESSAGE,
                "case {:?}: the cut gave up more than one character: {} bytes for a bound of \
                 {MAX_MESSAGE}",
                case.case,
                actual.len()
            );
        }
    }

    // The table is only a test of the bound if it has rows on both sides of it.
    assert!(
        cut > 0 && uncut > 0,
        "the table drove {cut} refusal(s) past the bound and {uncut} within it; it must span both"
    );
}

/// The other side of the rule: the bound is on the message a REFUSAL carries, not on what
/// a client may filter for — a value this operation can apply is applied at any length
/// `Filter$Value` allows, and the listing it narrows is answered.
#[tokio::test]
async fn a_long_filter_value_the_operation_can_apply_is_applied_rather_than_refused() {
    let harness = Harness::new();
    let arn = active_team(&harness, "filter-value-applied", 1, 1).await;
    let (status, started) = start_baseline(&harness, &arn, None).await;
    assert_eq!(status, 200, "the session the table filters was not started: {started:?}");
    let session_arn = text(&started, "BaselineSessionArn");

    // The action the session reports, off the one record that fixes it.
    let action = state::SessionKind::Baseline.action_name().to_string();
    let maximal = "x".repeat(MAX_FILTER_VALUE);
    let maximal_wide = "é".repeat(MAX_FILTER_VALUE / "é".len());

    // (case, operator, value, how many of the team's one session the filter admits)
    let rows = [
        ("the action name itself", "EQ", action.clone(), 1_usize),
        ("a maximal value", "EQ", maximal.clone(), 0),
        ("a maximal value, CONTAINS", "CONTAINS", maximal.clone(), 0),
        (
            "a maximal value, NOT_CONTAINS",
            "NOT_CONTAINS",
            maximal.clone(),
            1,
        ),
        ("a maximal value, NE", "NE", maximal.clone(), 1),
        (
            "a maximal value of two-byte characters",
            "EQ",
            maximal_wide.clone(),
            0,
        ),
        (
            "a maximal value of two-byte characters, NE",
            "NE",
            maximal_wide,
            1,
        ),
    ];

    for (case, operator, value, admitted) in rows {
        let (status, body) = list_sessions(
            &harness,
            &arn,
            json!([{ "FieldName": "ActionName", "Operator": operator, "Value": value }]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a filter this operation can apply must be applied, got {status} \
             with {body:?}"
        );
        let listed: Vec<String> = body
            .get("Sessions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| text(entry, "SessionArn"))
            .collect();
        assert_eq!(
            listed.len(),
            admitted,
            "case {case:?}: expected {admitted} session(s), got {listed:?}"
        );
        if admitted == 1 {
            assert_eq!(
                listed,
                vec![session_arn.clone()],
                "case {case:?}: expected the team's own session, got {listed:?}"
            );
        }
    }
}
