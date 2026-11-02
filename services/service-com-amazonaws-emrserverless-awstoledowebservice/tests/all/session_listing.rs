// SPDX-License-Identifier: CC0-1.0

//! `ListSessions`, end to end over HTTP.
//!
//! Every assertion goes through the real router (restJson1, SigV4 signing name
//! `emr-serverless`), so each case exercises routing, deserialization, the handler, the
//! persisted record and serialization — the same path an SDK client takes.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one request:
//!
//! 1. The listing reports the sessions the STORE holds for the application the route
//!    names — each one as the summary projection of that stored record, so every member
//!    it carries is the one a read of the session reports — and no session of another
//!    application.
//! 2. `states` keeps the sessions whose stored `SessionState` is one of the values it
//!    names, over the whole enum; with no filter every stored session is reported, one
//!    that has ended included.
//! 3. `createdAtAfter` / `createdAtBefore` bound the stored creation instant inclusively
//!    on each side.
//! 4. A page carries at most `maxResults` sessions in one stable order, and `nextToken`
//!    is present exactly when a session remains after the page — so the pages of a
//!    listing concatenate to its sessions, each exactly once.
//! 5. An application id that names nothing is the modelled `ResourceNotFoundException`
//!    (HTTP 404), and a `nextToken` this service did not mint is the modelled
//!    `ValidationException` (HTTP 400).

use std::collections::BTreeSet;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_emrserverless_awstoledowebservice::state::ComAmazonawsEmrserverlessAwstoledowebserviceState;

use super::application_lifecycle::{
    call, emulator, emulator_with_engine, rfc3339, urlencode, wait_for_transition, Answer,
};
use super::session_lifecycle::{
    get_session, minimal_session, session_application, sessions_enabled, start_session,
    EARLIEST_SESSION_RELEASE,
};
use super::session_termination::{force_session_state, terminate_session};

/// Every value of the `SessionState` enum.
///
/// Written out here rather than read from `crate::state`, so a wrong catalog in `src/`
/// cannot make this test pass. Source: `smithy_query(query="shape",shape="SessionState")`.
const SESSION_STATES: &[&str] = &[
    "SUBMITTED",
    "STARTING",
    "STARTED",
    "IDLE",
    "BUSY",
    "FAILED",
    "TERMINATING",
    "TERMINATED",
];

/// The members `SessionSummary` declares, and which of them every summary carries.
///
/// Written out here rather than read from `src/`. Source:
/// `smithy_query(query="shape",shape="SessionSummary")` — `name` is the one optional
/// member, every other is `@required`.
const REQUIRED_SUMMARY_MEMBERS: &[&str] = &[
    "applicationId",
    "sessionId",
    "arn",
    "state",
    "stateDetails",
    "releaseLabel",
    "executionRoleArn",
    "createdBy",
    "createdAt",
    "updatedAt",
];
const OPTIONAL_SUMMARY_MEMBERS: &[&str] = &["name"];

/// The published bounds of `ListSessionsRequest$maxResults`, and the values just past
/// them.
///
/// Written out here rather than read from `crate::state`. Source:
/// `smithy_query(query="operation",operation="ListSessions")` — `maxResults` is
/// `@range(min: 1, max: 50)`.
const SMALLEST_PAGE: usize = 1;
const LARGEST_PAGE: usize = 50;
const BELOW_SMALLEST_PAGE: i64 = 0;
const ABOVE_LARGEST_PAGE: i64 = 51;

/// What a not-found refusal says when the application a request names does not exist.
/// Written out here rather than read from `crate::state`. Source: the recorded refusals
/// this service's other handlers answer with
/// (`session_termination::a_terminate_naming_an_id_that_names_nothing_is_refused`).
const APPLICATION_NOT_FOUND: &str = "Application does not exist";

pub(crate) async fn list_sessions(
    router: &MultiServiceRouter,
    application_id: &str,
    query: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/applications/{application_id}/sessions{query}"),
        None,
    )
    .await
}

/// The session ids one page reports, in the order the page carries them.
fn listed_ids(page: &Answer) -> Vec<String> {
    page.at("/sessions")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| entry["sessionId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Move one stored session's creation instant back to the whole second it falls in,
/// through the store the handlers share, and answer that instant. Only the creation
/// instant is touched; everything else about the session is left as the start stored it.
fn whole_second_creation_instant(
    engine: &engine::InMemoryEngine,
    application_id: &str,
    session_id: &str,
) -> f64 {
    let store = engine.state::<ComAmazonawsEmrserverlessAwstoledowebserviceState>();
    let mut store = store.lock();
    let record = store
        .applications
        .get_mut(application_id)
        .expect("the application the test created is in the store");
    let session = record
        .sessions
        .get_mut(session_id)
        .expect("the session the test started is in the store");
    let whole_second = session.session.created_at.as_secs_f64().floor();
    session.session.created_at = DateTime::from_secs_f64(whole_second);
    whole_second
}

/// Start `count` sessions on `application_id`, answering their ids in the order they were
/// started. The rows differ along which optional members the start names, so the
/// summaries under test are not all the same shape.
async fn start_sessions(
    router: &MultiServiceRouter,
    application_id: &str,
    prefix: &str,
    count: usize,
) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..count {
        let token = format!("{prefix}-{index}");
        let mut body = minimal_session(&token);
        if index % 2 == 0 {
            body["name"] = json!(format!("listing-{index}"));
        }
        if index % 3 == 0 {
            body["tags"] = json!({ "team": "data" });
        }
        let started = start_session(router, application_id, body).await;
        assert_eq!(
            started.status, 200,
            "case {token:?}: StartSession expected 200, got {} with {:?}",
            started.status, started.body
        );
        ids.push(
            started
                .at("/sessionId")
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }
    ids
}

/// Rule 1: the listing reports the stored sessions of the application the route names,
/// each as the summary of its record, and nobody else's.
#[tokio::test]
async fn list_sessions_reports_each_stored_session_as_the_summary_of_its_record() {
    let router = emulator();
    let (application_id, _) = session_application(&router, "tok-listing-app").await;
    let (other_application_id, _) = session_application(&router, "tok-listing-other").await;

    let started = start_sessions(&router, &application_id, "tok-listed", 4).await;
    let elsewhere = start_sessions(&router, &other_application_id, "tok-elsewhere", 2).await;
    // Let the clock settle every session it owes a state, so what the listing reports and
    // what a read of the same session reports cannot be separated by a transition falling
    // due between the two calls. A session that named no idle timeout is owed nothing
    // further once it has come up.
    wait_for_transition();

    let page = list_sessions(&router, &application_id, "").await;
    assert_eq!(
        page.status, 200,
        "ListSessions expected 200, got {} with {:?}",
        page.status, page.body
    );
    let listed: BTreeSet<String> = listed_ids(&page).into_iter().collect();
    let expected: BTreeSet<String> = started.iter().cloned().collect();
    assert_eq!(
        listed, expected,
        "the listing reports every session of the application it names, got {listed:?}"
    );
    for id in &elsewhere {
        assert!(
            !listed.contains(id),
            "case {id:?}: a session of another application is not listed here, got {listed:?}"
        );
    }

    // Each summary is a projection of the stored record: every member it carries is the
    // one a read of that session reports.
    for entry in page.at("/sessions").as_array().cloned().unwrap_or_default() {
        let session_id = entry["sessionId"].as_str().unwrap_or_default().to_string();
        let read = get_session(&router, &application_id, &session_id).await;
        assert_eq!(
            read.status, 200,
            "case {session_id:?}: GetSession expected 200, got {} with {:?}",
            read.status, read.body
        );
        for member in REQUIRED_SUMMARY_MEMBERS {
            let summarised = entry.get(member).cloned().unwrap_or(Value::Null);
            let stored = read.at(&format!("/session/{member}"));
            assert_eq!(
                summarised, stored,
                "case {session_id:?}: the summary's {member} is the stored session's, got \
                 {summarised:?} against {stored:?}"
            );
            assert!(
                summarised != Value::Null,
                "case {session_id:?}: {member} is required, got {summarised:?}"
            );
        }
        for member in OPTIONAL_SUMMARY_MEMBERS {
            assert_eq!(
                entry.get(member).cloned().unwrap_or(Value::Null),
                read.at(&format!("/session/{member}")),
                "case {session_id:?}: the summary's {member} is the stored session's, got {:?}",
                entry.get(member)
            );
        }
        let unexpected: Vec<&String> = entry
            .as_object()
            .map(|members| {
                members
                    .keys()
                    .filter(|key| {
                        !REQUIRED_SUMMARY_MEMBERS.contains(&key.as_str())
                            && !OPTIONAL_SUMMARY_MEMBERS.contains(&key.as_str())
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            unexpected.is_empty(),
            "case {session_id:?}: a summary carries only SessionSummary's members, got \
             {unexpected:?}"
        );
    }

    // An application that holds no session reports none, rather than refusing: the
    // listing answers from the store.
    let (empty_application_id, _) = super::session_lifecycle::application(
        &router,
        "tok-listing-empty",
        EARLIEST_SESSION_RELEASE,
        Some(sessions_enabled()),
    )
    .await;
    let empty = list_sessions(&router, &empty_application_id, "").await;
    assert_eq!(
        empty.status, 200,
        "an application holding no session expected 200, got {} with {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.at("/sessions"),
        json!([]),
        "an application holding no session reports an empty list, got {:?}",
        empty.at("/sessions")
    );
    assert_eq!(
        empty.at("/nextToken"),
        Value::Null,
        "a listing with nothing after it carries no cursor, got {:?}",
        empty.at("/nextToken")
    );
}

/// Rule 2 over the WHOLE `SessionState` catalog: the filter keeps the sessions whose
/// stored state it names, and the unfiltered listing reports all of them.
#[tokio::test]
async fn list_sessions_filters_on_the_stored_session_state() {
    let (router, engine) = emulator_with_engine();
    let (application_id, _) = session_application(&router, "tok-state-filter-app").await;

    // One session held in each value of the enum, so every row of the catalog has a
    // session to find.
    let mut by_state: Vec<(&str, String)> = Vec::new();
    for (index, state) in SESSION_STATES.iter().enumerate() {
        let started = start_session(
            &router,
            &application_id,
            minimal_session(&format!("tok-state-filter-{index}")),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {state:?}: StartSession expected 200, got {} with {:?}",
            started.status, started.body
        );
        let session_id = started
            .at("/sessionId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        force_session_state(
            &engine,
            &application_id,
            &session_id,
            state,
            &format!("Session held in {state} by the test."),
        );
        by_state.push((state, session_id));
    }
    let every_session: BTreeSet<String> =
        by_state.iter().map(|(_, id)| id.clone()).collect();

    // One row per state in the catalog: the filter admits exactly the session in it.
    for (state, session_id) in &by_state {
        let page = list_sessions(&router, &application_id, &format!("?states={state}")).await;
        assert_eq!(
            page.status, 200,
            "case {state:?}: ListSessions expected 200, got {} with {:?}",
            page.status, page.body
        );
        let listed: BTreeSet<String> = listed_ids(&page).into_iter().collect();
        assert_eq!(
            listed,
            BTreeSet::from([session_id.clone()]),
            "case {state:?}: the filter admits exactly the session in that state, got {listed:?}"
        );
    }

    // A filter naming SEVERAL states admits their union; one naming every state in the
    // catalog (the modelled maximum of eight) admits the lot; and the absent filter
    // reports every stored session, the ones in a terminal state included.
    let three_states = &SESSION_STATES[..3];
    let union_query = three_states
        .iter()
        .map(|state| format!("states={state}"))
        .collect::<Vec<_>>()
        .join("&");
    let union_expected: BTreeSet<String> = by_state
        .iter()
        .filter(|(state, _)| three_states.contains(state))
        .map(|(_, id)| id.clone())
        .collect();
    let all_states_query = SESSION_STATES
        .iter()
        .map(|state| format!("states={state}"))
        .collect::<Vec<_>>()
        .join("&");
    for (case, query, expected) in [
        ("three states", format!("?{union_query}"), union_expected),
        (
            "every state the enum declares",
            format!("?{all_states_query}"),
            every_session.clone(),
        ),
        ("no filter at all", String::new(), every_session.clone()),
    ] {
        let page = list_sessions(&router, &application_id, &query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: ListSessions expected 200, got {} with {:?}",
            page.status, page.body
        );
        let listed: BTreeSet<String> = listed_ids(&page).into_iter().collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // A state no session of the application is in admits nothing — on a second
    // application, so a state that IS occupied on the first one cannot mask it.
    let (fresh_application_id, _) = session_application(&router, "tok-state-filter-fresh").await;
    let fresh = start_sessions(&router, &fresh_application_id, "tok-state-fresh", 1).await;
    for (case, query, expected) in [
        ("the state the start left it in", "?states=SUBMITTED", true),
        ("a state it is not in", "?states=BUSY", false),
        (
            "a terminal state it is not in",
            "?states=TERMINATED&states=FAILED",
            false,
        ),
    ] {
        let page = list_sessions(&router, &fresh_application_id, query).await;
        let listed = listed_ids(&page);
        assert_eq!(
            listed.contains(&fresh[0]),
            expected,
            "case {case:?}: expected the session to be {}present, got {listed:?}",
            if expected { "" } else { "ab" }
        );
    }
}

/// Rule 2's other half: a session that has ENDED stays in the listing, and is found by
/// the terminal state it settled on.
#[tokio::test]
async fn a_terminated_session_is_still_listed_in_its_terminal_state() {
    let router = emulator();
    let (application_id, _) = session_application(&router, "tok-listing-terminate").await;
    let started = start_sessions(&router, &application_id, "tok-listing-end", 2).await;

    let ended = started[0].clone();
    let live = started[1].clone();
    let terminated = terminate_session(&router, &application_id, &ended).await;
    assert_eq!(
        terminated.status, 200,
        "TerminateSession expected 200, got {} with {:?}",
        terminated.status, terminated.body
    );
    // The clock owes the terminating session `TERMINATED`, and the live one `STARTED`.
    wait_for_transition();

    for (case, query, expected) in [
        (
            "no filter",
            String::new(),
            BTreeSet::from([ended.clone(), live.clone()]),
        ),
        (
            "the terminal state",
            "?states=TERMINATED".to_string(),
            BTreeSet::from([ended.clone()]),
        ),
        (
            "the live state",
            "?states=STARTED".to_string(),
            BTreeSet::from([live.clone()]),
        ),
    ] {
        let page = list_sessions(&router, &application_id, &query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: ListSessions expected 200, got {} with {:?}",
            page.status, page.body
        );
        let listed: BTreeSet<String> = listed_ids(&page).into_iter().collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?}"
        );
    }

    // The state the listing reports for the ended session is the state a read of it
    // reports: the summary never re-derives it.
    let read = get_session(&router, &application_id, &ended).await;
    let page = list_sessions(&router, &application_id, "?states=TERMINATED").await;
    let summary = page.at("/sessions/0");
    assert_eq!(
        summary["state"],
        read.at("/session/state"),
        "case {ended:?}: the summary's state is the stored one, got {:?}",
        summary["state"]
    );
    assert_eq!(
        summary["updatedAt"],
        read.at("/session/updatedAt"),
        "case {ended:?}: the summary's updatedAt is the stored one, got {:?}",
        summary["updatedAt"]
    );
}

/// Rule 3: the creation-time bounds are inclusive on each side, and judged against the
/// stored creation instant.
#[tokio::test]
async fn list_sessions_bounds_the_listing_by_the_stored_creation_instant() {
    let (router, engine) = emulator_with_engine();
    let (application_id, _) = session_application(&router, "tok-listing-window").await;
    let started = start_sessions(&router, &application_id, "tok-window", 1).await;
    let session_id = started[0].clone();

    // A bound travels as an RFC-3339 date-time, whose sub-second part the wire form need
    // not carry, so the stored creation instant is moved to a whole second: a bound
    // naming THAT instant is then exactly the instant under test, which is what makes the
    // inclusive rows decidable rather than dependent on when the test ran.
    let created_at = whole_second_creation_instant(&engine, &application_id, &session_id);
    let read = get_session(&router, &application_id, &session_id).await;
    assert_eq!(
        read.at("/session/createdAt").as_f64(),
        Some(created_at),
        "the stored creation instant is the one the rows are built from, got {:?}",
        read.at("/session/createdAt")
    );

    for (case, query, expect_present) in [
        (
            "a lower bound before it was created",
            format!("?createdAtAfter={}", urlencode(&rfc3339(created_at - 60.0))),
            true,
        ),
        (
            "a lower bound at the instant it was created",
            format!("?createdAtAfter={}", urlencode(&rfc3339(created_at))),
            true,
        ),
        (
            "a lower bound after it was created",
            format!("?createdAtAfter={}", urlencode(&rfc3339(created_at + 60.0))),
            false,
        ),
        (
            "an upper bound after it was created",
            format!(
                "?createdAtBefore={}",
                urlencode(&rfc3339(created_at + 60.0))
            ),
            true,
        ),
        (
            "an upper bound at the instant it was created",
            format!("?createdAtBefore={}", urlencode(&rfc3339(created_at))),
            true,
        ),
        (
            "an upper bound before it was created",
            format!(
                "?createdAtBefore={}",
                urlencode(&rfc3339(created_at - 60.0))
            ),
            false,
        ),
        (
            "a window around it",
            format!(
                "?createdAtAfter={}&createdAtBefore={}",
                urlencode(&rfc3339(created_at - 60.0)),
                urlencode(&rfc3339(created_at + 60.0))
            ),
            true,
        ),
        (
            "a window entirely before it",
            format!(
                "?createdAtAfter={}&createdAtBefore={}",
                urlencode(&rfc3339(created_at - 120.0)),
                urlencode(&rfc3339(created_at - 60.0))
            ),
            false,
        ),
    ] {
        let page = list_sessions(&router, &application_id, &query).await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: ListSessions expected 200, got {} with {:?}",
            page.status, page.body
        );
        let listed = listed_ids(&page);
        assert_eq!(
            listed.contains(&session_id),
            expect_present,
            "case {case:?}: expected the session to be {}present, got {listed:?}",
            if expect_present { "" } else { "ab" }
        );
    }
}

/// Rule 4: the pages of a listing concatenate to its sessions, each exactly once, in one
/// stable order, with the cursor present exactly when a session remains.
#[tokio::test]
async fn the_pages_of_a_session_listing_concatenate_to_every_session_once() {
    let router = emulator();
    let (application_id, _) = session_application(&router, "tok-listing-pages").await;
    let started = start_sessions(&router, &application_id, "tok-page", 5).await;
    let mut expected = started.clone();
    expected.sort();

    for (case, page_size) in [
        ("the smallest page the model admits", SMALLEST_PAGE),
        ("two at a time", 2),
        ("one short of the whole listing", expected.len() - 1),
        ("the largest page the model admits", LARGEST_PAGE),
    ] {
        let mut seen: Vec<String> = Vec::new();
        let mut next: Option<String> = None;
        for _ in 0..32 {
            let query = match next.as_deref() {
                None => format!("?maxResults={page_size}"),
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
            };
            let page = list_sessions(&router, &application_id, &query).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: ListSessions expected 200, got {} with {:?}",
                page.status, page.body
            );
            let ids = listed_ids(&page);
            assert!(
                ids.len() <= page_size,
                "case {case:?}: a page carries at most maxResults sessions, got {ids:?}"
            );
            seen.extend(ids);
            match page.at("/nextToken").as_str() {
                Some(token) => {
                    assert!(
                        seen.len() < expected.len(),
                        "case {case:?}: a cursor is served only while a session remains, got \
                         {seen:?}"
                    );
                    next = Some(token.to_string());
                }
                None => {
                    next = None;
                    break;
                }
            }
        }
        assert!(
            next.is_none(),
            "case {case:?}: paging must terminate, got a cursor after 32 pages"
        );
        assert_eq!(
            seen, expected,
            "case {case:?}: the pages concatenate to every session exactly once, got {seen:?}"
        );
    }

    // A page size outside the modelled range is refused at the boundary rather than
    // clamped.
    for (case, page_size) in [
        ("one below the smallest page", BELOW_SMALLEST_PAGE),
        ("one above the largest page", ABOVE_LARGEST_PAGE),
    ] {
        let page = list_sessions(&router, &application_id, &format!("?maxResults={page_size}")).await;
        assert_eq!(
            page.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            page.status, page.body
        );
        assert_eq!(
            page.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            page.body
        );
    }
}

/// Rule 5: an id that names nothing, and a cursor this service did not mint, are the
/// modelled refusals.
#[tokio::test]
async fn list_sessions_refuses_an_unknown_application_and_a_foreign_cursor() {
    let router = emulator();
    let (application_id, _) = session_application(&router, "tok-listing-refusals").await;
    let started = start_sessions(&router, &application_id, "tok-refusal", 1).await;

    // The rows differ along the shape of the id that names nothing: its length, its
    // characters, and whether it looks like an id this emulator mints.
    for (case, unknown) in [
        ("an application id of one character", "0".to_string()),
        (
            "an application id at ApplicationId's maximum length",
            "z".repeat(64),
        ),
        (
            "an application id shaped like a minted one",
            "00000000deadbeef".to_string(),
        ),
    ] {
        let page = list_sessions(&router, &unknown, "").await;
        assert_eq!(
            page.status, 404,
            "case {case:?}: expected 404, got {} with {:?}",
            page.status, page.body
        );
        assert_eq!(
            page.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            page.body
        );
        assert_eq!(
            page.at("/message").as_str(),
            Some(APPLICATION_NOT_FOUND),
            "case {case:?}: a not-found refusal states the resource class, got {:?}",
            page.at("/message")
        );
    }

    for (case, token) in [
        ("not a cursor at all", "nonsense"),
        ("the right prefix with a bad body", "v1_zz"),
        ("the right prefix with an empty body", "v1_"),
    ] {
        let page = list_sessions(&router, &application_id, &format!("?nextToken={token}")).await;
        assert_eq!(
            page.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            page.status, page.body
        );
        assert_eq!(
            page.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            page.body
        );
    }

    // None of those refusals disturbed the listing that works.
    let page = list_sessions(&router, &application_id, "").await;
    assert_eq!(
        listed_ids(&page),
        started,
        "a refused listing leaves the stored sessions alone, got {:?}",
        page.body
    );
}
