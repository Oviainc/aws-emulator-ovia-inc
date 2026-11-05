// SPDX-License-Identifier: CC0-1.0

//! `ListActs`: the acts STORED under one workflow definition, selected, ordered and paged.
//!
//! The rule these cases assert is not "this one request answers these bytes": the listing
//! reports the acts held under the definition the path names, narrowed to the run and the
//! session the optional query filters name, each summary member read off the stored act and
//! its place in the resource chain, ordered by the one instant the summary carries
//! (`startedAt`) in the requested `sortOrder` with the act id as the tie-break, in pages of
//! at most `maxResults`, each page resuming strictly after the last act the previous one
//! emitted; a definition the store does not hold is refused with 404.
//!
//! So the tables below vary what that rule generalises over — which filters are present and
//! which resource they name (including one that names nothing and one that names another
//! definition's run), how many acts are stored and where in the chain they sit, page size
//! (every published bound and one past it), sort order, how a resumption token is damaged,
//! and whether an act has finished — and the recorded "list the acts of one session" case
//! is one row among them.

use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::ops::types::TraceLocation;
use service_com_amazonaws_novaact_amazonnovaagentsdataplane::state::ActStatus;

use crate::workflow_lifecycle::{text, Emulator};

/// The definition every case lists through.
const DEFINITION: &str = "act-listing";

/// A second definition, so a case can prove the listing is scoped to the definition the
/// path names rather than to the store.
const OTHER_DEFINITION: &str = "other_definition-9";

/// A well-formed `UuidString` (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`)
/// no record was ever minted for.
const ABSENT_UUID: &str = "00000000-0000-4000-8000-000000000000";

/// `MaxResults`' modelled range is 1..100
/// (`smithy_query(query="shape",shape="MaxResults")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// The byte that separates the fields inside a `nextToken` this operation issues.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// The members of one `ListActs` request beside the definition it lists.
#[derive(Debug, Default, Clone)]
struct Query {
    workflow_run_id: Option<String>,
    session_id: Option<String>,
    max_results: Option<i64>,
    next_token: Option<String>,
    sort_order: Option<String>,
}

impl Query {
    fn run(run_id: &str) -> Self {
        Self {
            workflow_run_id: Some(run_id.to_string()),
            ..Self::default()
        }
    }

    fn session(session_id: &str) -> Self {
        Self {
            session_id: Some(session_id.to_string()),
            ..Self::default()
        }
    }

    fn run_and_session(run_id: &str, session_id: &str) -> Self {
        Self {
            workflow_run_id: Some(run_id.to_string()),
            session_id: Some(session_id.to_string()),
            ..Self::default()
        }
    }
}

/// One `ListActs` call: `workflowRunId`, `sessionId`, `maxResults` and `nextToken` are
/// `@httpQuery` members and `sortOrder` travels in the body
/// (`smithy_query(query="operation",operation="ListActs")`).
async fn list(emulator: &Emulator, definition: &str, query: &Query) -> (u16, Value) {
    let mut members: Vec<String> = Vec::new();
    if let Some(run_id) = &query.workflow_run_id {
        members.push(format!("workflowRunId={run_id}"));
    }
    if let Some(session_id) = &query.session_id {
        members.push(format!("sessionId={session_id}"));
    }
    if let Some(max_results) = query.max_results {
        members.push(format!("maxResults={max_results}"));
    }
    if let Some(token) = &query.next_token {
        members.push(format!("nextToken={token}"));
    }
    let path = if members.is_empty() {
        format!("/workflow-definitions/{definition}/acts")
    } else {
        format!(
            "/workflow-definitions/{definition}/acts?{}",
            members.join("&")
        )
    };
    let body = match &query.sort_order {
        Some(order) => json!({ "sortOrder": order }),
        None => json!({}),
    };
    emulator.call("POST", &path, Some(body)).await
}

/// The summaries of one page, refused when the required member is missing or not a list.
fn summaries(body: &Value) -> Vec<Value> {
    body["actSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("actSummaries must be a list in {body}"))
        .clone()
}

/// The `actId` of every summary in one page, in the order the page carries them.
fn page_act_ids(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| text(summary, "actId"))
        .collect()
}

/// One act the fixture created, and where in the chain it sits.
#[derive(Debug, Clone)]
struct CreatedAct {
    run_id: String,
    session_id: String,
    act_id: String,
}

/// The chain every case lists over: one definition holding two runs, the first with two
/// sessions and the second with one, and seven acts spread across them — so a filter has
/// something to exclude at every level — plus a second definition holding its own act.
struct Fixture {
    emulator: Emulator,
    /// The two runs of [`DEFINITION`], in creation order.
    runs: [String; 2],
    /// The three sessions of [`DEFINITION`]: two under `runs[0]`, one under `runs[1]`.
    sessions: [String; 3],
    /// Every act of [`DEFINITION`], in creation order.
    acts: Vec<CreatedAct>,
    /// The one act of [`OTHER_DEFINITION`].
    other: CreatedAct,
}

impl Fixture {
    /// Build the chain through the real wire calls.
    ///
    /// Acts are created one millisecond apart so every act's `startedAt` is distinct: the
    /// ordering's tie-break is the act id, which is a minted uuid, so only distinct
    /// instants make the expected order the creation order.
    async fn build(region: &str) -> Self {
        let emulator = Emulator::new(region);
        let mut runs: Vec<String> = Vec::new();
        for definition in [DEFINITION, OTHER_DEFINITION] {
            let (status, body) = emulator.create_definition(definition).await;
            assert_eq!(
                status, 201,
                "case fixture: {definition} must be created, got {status} {body}"
            );
        }
        for index in 0..2 {
            let (status, body) = emulator.create_run(DEFINITION).await;
            assert_eq!(
                status, 201,
                "case fixture: run {index} must be created, got {status} {body}"
            );
            runs.push(text(&body, "workflowRunId"));
        }

        // (which run holds the session, how many acts it holds)
        let layout: [(usize, usize); 3] = [(0, 3), (0, 2), (1, 2)];
        let mut sessions: Vec<String> = Vec::new();
        let mut acts: Vec<CreatedAct> = Vec::new();
        for (slot, (run_index, _)) in layout.iter().enumerate() {
            let (status, body) = emulator.create_session(DEFINITION, &runs[*run_index]).await;
            assert_eq!(
                status, 201,
                "case fixture: session {slot} must be created, got {status} {body}"
            );
            sessions.push(text(&body, "sessionId"));
        }
        for (slot, (run_index, act_count)) in layout.iter().enumerate() {
            for act_index in 0..*act_count {
                let (status, body) = emulator
                    .create_act(
                        DEFINITION,
                        &runs[*run_index],
                        &sessions[slot],
                        &format!("task {slot}.{act_index}"),
                    )
                    .await;
                assert_eq!(
                    status, 201,
                    "case fixture: act {slot}.{act_index} must be created, got {status} {body}"
                );
                acts.push(CreatedAct {
                    run_id: runs[*run_index].clone(),
                    session_id: sessions[slot].clone(),
                    act_id: text(&body, "actId"),
                });
                sleep(Duration::from_millis(1));
            }
        }

        let (status, body) = emulator.create_run(OTHER_DEFINITION).await;
        assert_eq!(
            status, 201,
            "case fixture: the other definition's run must be created, got {status} {body}"
        );
        let other_run = text(&body, "workflowRunId");
        let (status, body) = emulator.create_session(OTHER_DEFINITION, &other_run).await;
        assert_eq!(
            status, 201,
            "case fixture: the other definition's session must be created, got {status} {body}"
        );
        let other_session = text(&body, "sessionId");
        let (status, body) = emulator
            .create_act(
                OTHER_DEFINITION,
                &other_run,
                &other_session,
                "an act of another definition",
            )
            .await;
        assert_eq!(
            status, 201,
            "case fixture: the other definition's act must be created, got {status} {body}"
        );
        let other = CreatedAct {
            run_id: other_run,
            session_id: other_session,
            act_id: text(&body, "actId"),
        };

        Self {
            emulator,
            runs: [runs[0].clone(), runs[1].clone()],
            sessions: [
                sessions[0].clone(),
                sessions[1].clone(),
                sessions[2].clone(),
            ],
            acts,
            other,
        }
    }

    /// The ids of the fixture's acts that satisfy `selects`, in creation order — the order
    /// an ascending listing reports them in.
    fn expected(&self, selects: impl Fn(&CreatedAct) -> bool) -> Vec<String> {
        self.acts
            .iter()
            .filter(|act| selects(act))
            .map(|act| act.act_id.clone())
            .collect()
    }
}

/// Every filter combination reports exactly the acts it selects, in creation order.
#[tokio::test]
async fn filters_select_the_acts_of_the_run_and_session_they_name() {
    let fixture = Fixture::build("us-east-1").await;
    let [run_a, run_b] = fixture.runs.clone();
    let [session_1, session_2, session_3] = fixture.sessions.clone();

    let cases: Vec<(String, Query, Vec<String>)> = vec![
        (
            "no filter: every act of the definition".to_string(),
            Query::default(),
            fixture.expected(|_| true),
        ),
        (
            format!("workflowRunId {run_a}: the acts of its two sessions"),
            Query::run(&run_a),
            fixture.expected(|act| act.run_id == run_a),
        ),
        (
            format!("workflowRunId {run_b}: the acts of its one session"),
            Query::run(&run_b),
            fixture.expected(|act| act.run_id == run_b),
        ),
        (
            "sessionId of the first session".to_string(),
            Query::session(&session_1),
            fixture.expected(|act| act.session_id == session_1),
        ),
        (
            "sessionId of the second session, under the same run".to_string(),
            Query::session(&session_2),
            fixture.expected(|act| act.session_id == session_2),
        ),
        (
            "sessionId of the session under the other run".to_string(),
            Query::session(&session_3),
            fixture.expected(|act| act.session_id == session_3),
        ),
        (
            "both filters, naming a session that run holds".to_string(),
            Query::run_and_session(&run_a, &session_1),
            fixture.expected(|act| act.run_id == run_a && act.session_id == session_1),
        ),
        (
            "both filters, naming a session the other run holds".to_string(),
            Query::run_and_session(&run_a, &session_3),
            Vec::new(),
        ),
        (
            "a workflowRunId no run was minted for".to_string(),
            Query::run(ABSENT_UUID),
            Vec::new(),
        ),
        (
            "a sessionId no session was minted for".to_string(),
            Query::session(ABSENT_UUID),
            Vec::new(),
        ),
        (
            "another definition's run".to_string(),
            Query::run(&fixture.other.run_id),
            Vec::new(),
        ),
        (
            "another definition's session".to_string(),
            Query::session(&fixture.other.session_id),
            Vec::new(),
        ),
    ];

    for (label, query, expected) in cases {
        let (status, body) = list(&fixture.emulator, DEFINITION, &query).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            page_act_ids(&body),
            expected,
            "case {label:?}: expected the acts {expected:?}, got {body}"
        );
        assert_eq!(
            body["nextToken"],
            Value::Null,
            "case {label:?}: the whole selection fits one default page, got {body}"
        );
    }

    // The scoping is a selection, not a hiding: the other definition answers its own act.
    let (status, body) = list(&fixture.emulator, OTHER_DEFINITION, &Query::default()).await;
    assert_eq!(
        status, 200,
        "case the other definition: expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        page_act_ids(&body),
        vec![fixture.other.act_id.clone()],
        "case the other definition: it answers its own act, got {body}"
    );
}

/// Every summary member is the stored act's own: its id, the run and session holding it,
/// the status it is in and the instant it was created.
#[tokio::test]
async fn each_summary_is_read_off_the_stored_act() {
    let fixture = Fixture::build("eu-west-1").await;

    let (status, body) = list(&fixture.emulator, DEFINITION, &Query::default()).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {body}");
    let listed = summaries(&body);
    assert_eq!(
        listed.len(),
        fixture.acts.len(),
        "every created act must be listed, got {body}"
    );

    for (summary, created) in listed.iter().zip(&fixture.acts) {
        let label = &created.act_id;
        assert_eq!(
            text(summary, "actId"),
            created.act_id,
            "case {label}: the summary reports the minted act id, got {summary}"
        );
        assert_eq!(
            text(summary, "workflowRunId"),
            created.run_id,
            "case {label}: the summary reports the run holding the act, got {summary}"
        );
        assert_eq!(
            text(summary, "sessionId"),
            created.session_id,
            "case {label}: the summary reports the session holding the act, got {summary}"
        );
        assert_eq!(
            summary["status"], "RUNNING",
            "case {label}: an act this service has just accepted is RUNNING, got {summary}"
        );
        let started_at = text(summary, "startedAt");
        assert!(
            started_at.contains('T') && started_at.ends_with('Z'),
            "case {label}: startedAt is a date-time string, got {started_at:?}"
        );
        assert!(
            summary.get("endedAt").is_none(),
            "case {label}: an act that has not finished carries no endedAt, got {summary}"
        );
        assert!(
            summary.get("traceLocation").is_none(),
            "case {label}: an act with no trace recorded carries no traceLocation, got {summary}"
        );
    }

    // The minted ids are distinct, so the listing reports one summary per stored act rather
    // than one record several times.
    let distinct: BTreeSet<String> = page_act_ids(&body).into_iter().collect();
    assert_eq!(
        distinct.len(),
        fixture.acts.len(),
        "every summary names a different act, got {body}"
    );
}

/// `status`, `endedAt` and `traceLocation` are the stored act's, whatever state it is in:
/// an act that has finished reports its end instant and the location of its trace.
///
/// `UpdateAct` is still a scaffolded 501, so the only way to observe a finished act is to
/// put the store into that state the way that operation would.
#[tokio::test]
async fn a_finished_act_reports_its_stored_end_and_trace() {
    let fixture = Fixture::build("us-west-2").await;

    // Rows: the terminal statuses `ActStatus` declares
    // (`smithy_query(query="shape",shape="ActStatus")`), how long after the start each act
    // ended, and whether a trace location was recorded for it.
    let rows: [(ActStatus, Duration, Option<&str>); 3] = [
        (
            ActStatus::Succeeded,
            Duration::from_secs(2),
            Some("s3://nova-act-traces/acts/0"),
        ),
        (ActStatus::Failed, Duration::from_millis(400), None),
        (
            ActStatus::TimedOut,
            Duration::from_secs(900),
            Some("s3://nova-act-traces/acts/2"),
        ),
    ];

    let mut expected: Vec<(String, &'static str, bool)> = Vec::new();
    {
        let store = fixture.emulator.store();
        let mut store = store.lock();
        for (slot, (status, elapsed, location)) in rows.iter().enumerate() {
            let created = &fixture.acts[slot];
            let act = store
                .workflow_definitions
                .get_mut(DEFINITION)
                .and_then(|definition| definition.runs.get_mut(&created.run_id))
                .and_then(|run| run.sessions.get_mut(&created.session_id))
                .and_then(|session| session.acts.get_mut(&created.act_id))
                .unwrap_or_else(|| panic!("case {slot}: the act must be stored"));
            act.status = *status;
            act.ended_at = Some(act.created_at + *elapsed);
            act.trace_location = location.map(|location| TraceLocation {
                location_type: "S3".to_string(),
                location: location.to_string(),
            });
            expected.push((created.act_id.clone(), status.as_wire(), location.is_some()));
        }
    }

    let (status, body) = list(&fixture.emulator, DEFINITION, &Query::default()).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {body}");
    let listed = summaries(&body);

    for (act_id, wire_status, has_trace) in expected {
        let summary = listed
            .iter()
            .find(|summary| text(summary, "actId") == act_id)
            .unwrap_or_else(|| panic!("case {act_id}: the act must be listed, got {body}"));
        assert_eq!(
            summary["status"], wire_status,
            "case {act_id}: expected the stored status {wire_status}, got {summary}"
        );
        let ended_at = text(summary, "endedAt");
        assert!(
            ended_at.contains('T') && ended_at.ends_with('Z'),
            "case {act_id}: a finished act reports its end instant, got {summary}"
        );
        assert!(
            text(summary, "startedAt") < ended_at,
            "case {act_id}: the act ended after it started, got {summary}"
        );
        if has_trace {
            assert_eq!(
                summary["traceLocation"]["locationType"], "S3",
                "case {act_id}: the summary reports the stored trace location, got {summary}"
            );
            assert!(
                text(&summary["traceLocation"], "location").starts_with("s3://"),
                "case {act_id}: the summary reports the stored trace location, got {summary}"
            );
        } else {
            assert!(
                summary.get("traceLocation").is_none(),
                "case {act_id}: no trace was recorded for this act, got {summary}"
            );
        }
    }

    // The acts nothing changed still report what they were created with, so the write above
    // did not leak across records.
    for created in &fixture.acts[rows.len()..] {
        let summary = listed
            .iter()
            .find(|summary| text(summary, "actId") == created.act_id)
            .unwrap_or_else(|| panic!("case {}: must be listed, got {body}", created.act_id));
        assert_eq!(
            summary["status"], "RUNNING",
            "case {}: an untouched act is still RUNNING, got {summary}",
            created.act_id
        );
        assert!(
            summary.get("endedAt").is_none(),
            "case {}: an untouched act has not ended, got {summary}",
            created.act_id
        );
    }
}

/// Page through the whole listing, asserting the paging invariants on the way: no page is
/// bigger than the page size, every page carrying a token is full, and the listing ends.
async fn drain(
    emulator: &Emulator,
    label: &str,
    base: &Query,
    expected_page_size: usize,
    stored: usize,
) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page_index in 0..=stored + 1 {
        let mut query = base.clone();
        query.next_token = token.clone();
        let (status, body) = list(emulator, DEFINITION, &query).await;
        assert_eq!(
            status, 200,
            "case {label}: page {page_index} expected HTTP 200, got {status} {body}"
        );
        let ids = page_act_ids(&body);
        assert!(
            ids.len() <= expected_page_size,
            "case {label}: page {page_index} must hold at most {expected_page_size} summaries, got {ids:?}"
        );
        collected.extend(ids.iter().cloned());
        match body["nextToken"].as_str() {
            Some(next) => {
                assert_eq!(
                    ids.len(),
                    expected_page_size,
                    "case {label}: page {page_index} carries a nextToken, so it must be full, got {ids:?}"
                );
                assert!(
                    !next.is_empty() && next.len() <= 2048 && !next.contains(char::is_whitespace),
                    "case {label}: page {page_index} nextToken must satisfy NextToken's length 1..2048 and ^\\S*$, got {next:?}"
                );
                token = Some(next.to_string());
            }
            None => return collected,
        }
    }
    panic!("case {label}: the listing never ended, collected {collected:?}");
}

/// One row of the paging table: the label, the acts the request must report, the filter it
/// sends, its `sortOrder` and `maxResults`, and the page size it is answered in.
type PagingCase<'a> = (
    &'a str,
    &'a [String],
    Query,
    Option<&'a str>,
    Option<i64>,
    usize,
);

/// Every page size and both orders answer the same acts, in the order the request asked
/// for, split into pages of the size it asked for — with or without a filter.
#[tokio::test]
async fn page_size_and_sort_order_table() {
    let fixture = Fixture::build("ap-southeast-2").await;
    let session_1 = fixture.sessions[0].clone();

    let all: Vec<String> = fixture.expected(|_| true);
    let filtered: Vec<String> = fixture.expected(|act| act.session_id == session_1);
    let stored = all.len();
    let (all, filtered) = (all.as_slice(), filtered.as_slice());

    // (label, the acts the row must report, the filter, sortOrder, maxResults, the page
    // size that request is answered in)
    let cases: Vec<PagingCase<'_>> = vec![
        ("no sortOrder, no maxResults", all, Query::default(), None, None, stored),
        ("no sortOrder, the smallest page", all, Query::default(), None, Some(MAX_RESULTS_MIN), 1),
        ("ascending, one page per act", all, Query::default(), Some("Ascending"), Some(1), 1),
        ("ascending, two at a time", all, Query::default(), Some("Ascending"), Some(2), 2),
        ("ascending, a page size that does not divide the store", all, Query::default(), Some("Ascending"), Some(3), 3),
        ("ascending, exactly the store", all, Query::default(), Some("Ascending"), Some(stored as i64), stored),
        ("ascending, one page larger than the store", all, Query::default(), Some("Ascending"), Some(stored as i64 + 1), stored),
        ("ascending, the largest page", all, Query::default(), Some("Ascending"), Some(MAX_RESULTS_MAX), stored),
        ("descending, one page per act", all, Query::default(), Some("Descending"), Some(1), 1),
        ("descending, two at a time", all, Query::default(), Some("Descending"), Some(2), 2),
        ("descending, the largest page", all, Query::default(), Some("Descending"), Some(MAX_RESULTS_MAX), stored),
        ("descending, no maxResults", all, Query::default(), Some("Descending"), None, stored),
        ("one session, ascending, one at a time", filtered, Query::session(&session_1), Some("Ascending"), Some(1), 1),
        ("one session, descending, two at a time", filtered, Query::session(&session_1), Some("Descending"), Some(2), 2),
    ];

    for (label, expected, filter, sort_order, max_results, page_size) in cases {
        let mut query = filter;
        query.sort_order = sort_order.map(str::to_string);
        query.max_results = max_results;
        let collected = drain(
            &fixture.emulator,
            label,
            &query,
            page_size.min(expected.len().max(1)),
            stored,
        )
        .await;
        let wanted: Vec<String> = if sort_order == Some("Descending") {
            expected.iter().rev().cloned().collect()
        } else {
            expected.to_vec()
        };
        assert_eq!(
            collected, wanted,
            "case {label}: the whole listing must be every selected act in the requested order, got {collected:?}"
        );
    }
}

/// A resumption carries a POSITION, not an offset: the page after a token holds the acts
/// that sort after it, even when an act is created meanwhile.
#[tokio::test]
async fn a_page_resumes_after_its_cursor_across_changes_to_the_store() {
    let fixture = Fixture::build("us-east-1").await;
    let base = Query {
        sort_order: Some("Ascending".to_string()),
        max_results: Some(2),
        ..Query::default()
    };

    let (status, first) = list(&fixture.emulator, DEFINITION, &base).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {first}");
    let expected: Vec<String> = fixture.expected(|_| true);
    assert_eq!(
        page_act_ids(&first),
        expected[..2].to_vec(),
        "case the first page: got {first}"
    );
    let token = text(&first, "nextToken");

    // A brand-new act is the youngest, so it belongs on the LAST page of an ascending
    // listing — after the cursor, hence in the resumption.
    sleep(Duration::from_millis(1));
    let (status, body) = fixture
        .emulator
        .create_act(
            DEFINITION,
            &fixture.runs[1],
            &fixture.sessions[2],
            "an act created mid-listing",
        )
        .await;
    assert_eq!(status, 201, "the act must be created, got {status} {body}");
    let youngest = text(&body, "actId");

    let mut drained: Vec<String> = page_act_ids(&first);
    let mut token = Some(token);
    while let Some(next) = token {
        let mut query = base.clone();
        query.next_token = Some(next);
        let (status, page) = list(&fixture.emulator, DEFINITION, &query).await;
        assert_eq!(status, 200, "expected HTTP 200, got {status} {page}");
        drained.extend(page_act_ids(&page));
        token = page["nextToken"].as_str().map(str::to_string);
    }

    let mut wanted = expected.clone();
    wanted.push(youngest);
    assert_eq!(
        drained, wanted,
        "case resumed across a create: every act exactly once, oldest first, got {drained:?}"
    );
}

/// A `nextToken` this operation did not issue for this listing is refused with the modelled
/// `ValidationException`, naming the member at fault — never answered with a page from some
/// other position.
#[tokio::test]
async fn a_token_this_listing_did_not_issue_is_refused() {
    let fixture = Fixture::build("us-east-1").await;
    let ascending = Query {
        sort_order: Some("Ascending".to_string()),
        max_results: Some(1),
        ..Query::default()
    };
    let (_, first) = list(&fixture.emulator, DEFINITION, &ascending).await;
    let ascending_token = text(&first, "nextToken");

    let cases: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-token".to_string()),
        ("hex of nothing this operation encodes", hex::encode(ABSENT_UUID)),
        (
            "a field missing",
            hex::encode(format!("Ascending{TOKEN_SEPARATOR}1.000000000")),
        ),
        (
            "a field too many",
            hex::encode(format!(
                "Ascending{TOKEN_SEPARATOR}1.000000000{TOKEN_SEPARATOR}{ABSENT_UUID}{TOKEN_SEPARATOR}x"
            )),
        ),
        ("an order it was not issued for", ascending_token.clone()),
        (
            "an unreadable instant",
            hex::encode(format!(
                "Descending{TOKEN_SEPARATOR}not-an-instant{TOKEN_SEPARATOR}{ABSENT_UUID}"
            )),
        ),
        (
            "a nanosecond field out of range",
            hex::encode(format!(
                "Descending{TOKEN_SEPARATOR}1.1000000000{TOKEN_SEPARATOR}{ABSENT_UUID}"
            )),
        ),
        (
            "an instant past what a clock can name",
            hex::encode(format!(
                "Descending{TOKEN_SEPARATOR}{}.000000000{TOKEN_SEPARATOR}{ABSENT_UUID}",
                u64::MAX
            )),
        ),
        (
            "a key outside UuidString's pattern",
            hex::encode(format!(
                "Descending{TOKEN_SEPARATOR}1.000000000{TOKEN_SEPARATOR}not-an-act-id"
            )),
        ),
        (
            "a key in UuidString's form but upper case",
            hex::encode(format!(
                "Descending{TOKEN_SEPARATOR}1.000000000{TOKEN_SEPARATOR}00000000-0000-4000-8000-00000000000A"
            )),
        ),
    ];

    for (label, token) in cases {
        // Every row is sent on a Descending listing, so the Ascending token above is a
        // token for the other order.
        let query = Query {
            sort_order: Some("Descending".to_string()),
            max_results: Some(1),
            next_token: Some(token),
            ..Query::default()
        };
        let (status, body) = list(&fixture.emulator, DEFINITION, &query).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"], "ValidationException",
            "case {label:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["reason"], "FieldValidationFailed",
            "case {label:?}: a broken field rule is reported as FieldValidationFailed, got {body}"
        );
        assert_eq!(
            body["fieldList"][0]["name"], "nextToken",
            "case {label:?}: the refusal names the member at fault, got {body}"
        );
        assert!(
            !text(&body, "message").is_empty(),
            "case {label:?}: the refusal says what was wrong, got {body}"
        );
    }

    // And the token it WAS issued for still pages that listing.
    let mut query = ascending.clone();
    query.next_token = Some(ascending_token);
    let (status, body) = list(&fixture.emulator, DEFINITION, &query).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status} {body}");
    assert_eq!(
        page_act_ids(&body),
        vec![fixture.acts[1].act_id.clone()],
        "case the token it was issued for: got {body}"
    );
}

/// A page size outside `MaxResults`' published range is refused by the modelled constraint,
/// never clamped into an answer; a size ON each bound is answered.
#[tokio::test]
async fn page_size_outside_the_modeled_range_is_refused() {
    let fixture = Fixture::build("us-east-1").await;

    for (label, max_results) in [
        ("one below the minimum", MAX_RESULTS_MIN - 1),
        ("one above the maximum", MAX_RESULTS_MAX + 1),
    ] {
        let query = Query {
            max_results: Some(max_results),
            ..Query::default()
        };
        let (status, body) = list(&fixture.emulator, DEFINITION, &query).await;
        assert_eq!(
            status, 400,
            "case {label} ({max_results}): expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"], "ValidationException",
            "case {label} ({max_results}): expected ValidationException, got {body}"
        );
    }

    let expected = fixture.expected(|_| true);
    for (label, max_results, page) in [
        ("the published minimum", MAX_RESULTS_MIN, 1usize),
        ("the published maximum", MAX_RESULTS_MAX, expected.len()),
    ] {
        let query = Query {
            max_results: Some(max_results),
            ..Query::default()
        };
        let (status, body) = list(&fixture.emulator, DEFINITION, &query).await;
        assert_eq!(
            status, 200,
            "case {label} ({max_results}): expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            page_act_ids(&body),
            expected[..page].to_vec(),
            "case {label} ({max_results}): expected the first {page} acts, got {body}"
        );
    }
}

/// A `sortOrder` outside the model's enum is refused, never answered in some other order.
#[tokio::test]
async fn a_sort_order_outside_the_enum_is_refused() {
    let fixture = Fixture::build("us-east-1").await;

    // `SortOrder`'s values are `Ascending | Descending`
    // (`smithy_query(query="shape",shape="SortOrder")`), so each of these is outside it.
    for label in ["ASCENDING", "ascending", "Newest", ""] {
        let query = Query {
            sort_order: Some(label.to_string()),
            ..Query::default()
        };
        let (status, body) = list(&fixture.emulator, DEFINITION, &query).await;
        assert_eq!(
            status, 400,
            "case sortOrder {label:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"], "ValidationException",
            "case sortOrder {label:?}: expected ValidationException, got {body}"
        );
    }
}

/// Nothing stored under the definition, nothing reported: the required list is present and
/// empty, and no token invites a page that does not exist.
#[tokio::test]
async fn a_definition_with_no_acts_answers_an_empty_page() {
    let emulator = Emulator::new("eu-central-1");
    let (status, body) = emulator.create_definition(DEFINITION).await;
    assert_eq!(status, 201, "the definition must be created, got {status} {body}");
    // A run and a session with no act in it: the listing walks the chain and still reports
    // nothing, rather than reporting the session itself.
    let (status, body) = emulator.create_run(DEFINITION).await;
    assert_eq!(status, 201, "the run must be created, got {status} {body}");
    let run_id = text(&body, "workflowRunId");
    let (status, body) = emulator.create_session(DEFINITION, &run_id).await;
    assert_eq!(status, 201, "the session must be created, got {status} {body}");
    let session_id = text(&body, "sessionId");

    for (label, query) in [
        ("no members", Query::default()),
        ("ascending, the smallest page", Query {
            sort_order: Some("Ascending".to_string()),
            max_results: Some(MAX_RESULTS_MIN),
            ..Query::default()
        }),
        ("filtered by the empty session", Query::session(&session_id)),
        ("filtered by the empty run", Query::run(&run_id)),
    ] {
        let (status, body) = list(&emulator, DEFINITION, &query).await;
        assert_eq!(
            status, 200,
            "case {label}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            page_act_ids(&body),
            Vec::<String>::new(),
            "case {label}: nothing stored reports no summaries, got {body}"
        );
        assert_eq!(
            body["nextToken"],
            Value::Null,
            "case {label}: there is no further page to invite, got {body}"
        );
    }
}

/// The definition is the resource this operation reads, so one the store does not hold is
/// refused with the modelled 404 — whatever the filters say.
#[tokio::test]
async fn a_definition_the_store_does_not_hold_is_refused() {
    let fixture = Fixture::build("us-east-1").await;

    // Rows vary the name's form (`^[a-zA-Z0-9_-]{1,40}$`) and the filters sent with it.
    let cases: Vec<(&str, &str, Query)> = vec![
        ("a plain absent name", "absent", Query::default()),
        ("one character", "A", Query::default()),
        (
            "the modelled maximum of 40 characters",
            "max-length-definition-name-0123456789_ab",
            Query::default(),
        ),
        (
            "an absent definition with both filters",
            "absent_with-filters",
            Query::run_and_session(&fixture.runs[0], &fixture.sessions[0]),
        ),
    ];

    for (label, name, query) in cases {
        let (status, body) = list(&fixture.emulator, name, &query).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            body["__type"], "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {body}"
        );
        assert_eq!(
            body["resourceType"], "workflow-definition",
            "case {label:?}: the refusal names the kind that was missing, got {body}"
        );
        assert_eq!(
            text(&body, "resourceId"),
            format!(
                "arn:aws:nova-act:us-east-1:{}:workflow-definition/{name}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {label:?}: the refusal identifies the resource by its ARN, got {body}"
        );
    }
}
