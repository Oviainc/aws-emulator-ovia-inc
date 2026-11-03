// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue statement slice: the `RunStatement` that records a
//! statement on an interactive session, and the `GetStatement` that reads it back.
//!
//! RULE: a statement is a record of the session it was run on, addressed by the session's
//! `Id` and an `Id` that session mints for it in submission order, and a session takes one
//! only while its runtime is up and free — so a run is refused when the session is in any
//! status but `READY` and when a statement of that session has not finished, and the
//! statement a run records carries the code it submitted until the clock moves it to its
//! terminal state.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The round-trip rows differ along what the rule generalises over: the size and the form
//! of the code submitted, each published bound of `Code` included, and the number of
//! statements one session holds.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    messages, ComAmazonawsGlueAwsglueState, FIRST_STATEMENT_ID,
    STATEMENT_NOT_EXECUTED_ERROR_NAME, STATEMENT_NOT_EXECUTED_MESSAGE, STATEMENT_SETTLES_AFTER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{already_past, far_future};
use crate::wire::{call, call_in_region, emulator_on, error_code, error_message, ok, REGION};

/// The longest `Code` the model accepts: `OrchestrationStatementCodeString`, "len 0..68000"
/// (`smithy_query operation=RunStatement`).
const MAX_CODE_LENGTH: usize = 68_000;

/// The `StatementState` values this suite observes, of the modelled enum (`WAITING`,
/// `RUNNING`, `AVAILABLE`, `CANCELLING`, `CANCELLED`, `ERROR`, from
/// `smithy_query shape=StatementState`).
const STATE_WAITING: &str = "WAITING";
const STATE_ERROR: &str = "ERROR";

/// The `SessionStatus` values a run is refused in here, of the modelled enum
/// (`PROVISIONING`, `READY`, `FAILED`, `TIMEOUT`, `STOPPING`, `STOPPED`, from
/// `smithy_query shape=SessionStatus`).
const STATUS_PROVISIONING: &str = "PROVISIONING";
const STATUS_TIMEOUT: &str = "TIMEOUT";

/// An IAM role ARN of the form the modelled `OrchestrationRoleArn` pattern requires
/// (`^arn:aws[^:]*:iam::[0-9]*:role/.+$`).
const ROLE: &str = "arn:aws:iam::000000000000:role/GlueSessionRole";

/// Seconds in one minute: both documented session timeouts are stated in minutes.
const SECONDS_PER_MINUTE: u64 = 60;

/// Create the session `id`, with the create body `extra` merged over the required members.
pub(crate) async fn create_session(router: &MultiServiceRouter, id: &str, extra: Value) {
    let mut body = json!({
        "Id": id,
        "Role": ROLE,
        "Command": { "Name": "glueetl" },
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    ok(router, "CreateSession", &body).await;
}

/// Put the instant the clock owes the session `id` its next status at `at`.
pub(crate) fn set_session_due(engine: &Arc<engine::InMemoryEngine>, id: &str, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .session_mut(REGION, id)
        .unwrap_or_else(|| panic!("{id} was created"));
    entry.settles_at = Some(at);
}

/// Put the instant the clock owes every unsettled statement of the session `id` its
/// terminal state at `at`.
pub(crate) fn set_statement_due(engine: &Arc<engine::InMemoryEngine>, id: &str, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .session_mut(REGION, id)
        .unwrap_or_else(|| panic!("{id} was created"));
    for statement in &mut entry.statements {
        if statement.settles_at.is_some() {
            statement.settles_at = Some(at);
        }
    }
}

/// Bring every unsettled statement of the session `id` to the instant the clock owed it
/// its terminal state, so a suite can observe that state without sleeping for the span:
/// the record is pulled back by the span its run recorded, which leaves the span between
/// a statement starting and finishing the one the run recorded.
pub(crate) fn reach_due_instant(engine: &Arc<engine::InMemoryEngine>, id: &str) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .session_mut(REGION, id)
        .unwrap_or_else(|| panic!("{id} was created"));
    for statement in &mut entry.statements {
        if statement.settles_at.is_none() {
            continue;
        }
        let started_on = statement
            .started_on
            .checked_sub(STATEMENT_SETTLES_AFTER)
            .expect("an instant in range");
        statement.started_on = started_on;
        statement.settles_at = started_on.checked_add(STATEMENT_SETTLES_AFTER);
    }
}

/// Pull the session `id`'s own instants back by `elapsed`, so a suite can observe the
/// status the clock owes it without sleeping for the span.
pub(crate) fn rewind_session(engine: &Arc<engine::InMemoryEngine>, id: &str, elapsed: Duration) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .session_mut(REGION, id)
        .unwrap_or_else(|| panic!("{id} was created"));
    let back = |at: SystemTime| at.checked_sub(elapsed).expect("an instant in range");
    entry.created_on = back(entry.created_on);
    entry.last_active_on = back(entry.last_active_on);
    entry.settles_at = entry.settles_at.map(back);
}

/// A session in `READY`: created, with the status its provisioning owed already due.
pub(crate) async fn ready_session(router: &MultiServiceRouter, engine: &Arc<engine::InMemoryEngine>, id: &str) {
    create_session(router, id, json!({})).await;
    set_session_due(engine, id, already_past());
}

/// Run `code` on the session `id` and return the `Id` the answer reports.
pub(crate) async fn run(router: &MultiServiceRouter, id: &str, code: &str) -> i64 {
    let answer = ok(
        router,
        "RunStatement",
        &json!({ "SessionId": id, "Code": code }),
    )
    .await;
    answer["Id"]
        .as_i64()
        .unwrap_or_else(|| panic!("RunStatement must report the statement Id, got {answer}"))
}

/// The `Statement` the `GetStatement` answer for `(id, statement)` reports.
pub(crate) async fn get(router: &MultiServiceRouter, id: &str, statement: i64) -> Value {
    let read = ok(
        router,
        "GetStatement",
        &json!({ "SessionId": id, "Id": statement }),
    )
    .await;
    read.get("Statement")
        .cloned()
        .unwrap_or_else(|| panic!("GetStatement must report a Statement, got {read}"))
}

// ---------------------------------------------------------------------------
// Run, then read back
// ---------------------------------------------------------------------------

/// What `RunStatement` records is what `GetStatement` reports: the code the run submitted,
/// the state the session took it in, and the id that session minted for it.
///
/// The rows differ along the size and the form of the code, both published bounds of
/// `Code` included.
#[tokio::test]
async fn run_statements_are_read_back_from_the_store() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Exactly the longest code the model accepts, as a comment so it is still a statement.
    let longest_code = format!("# {}", "c".repeat(MAX_CODE_LENGTH - 2));

    // (case, the session to run it on, the code submitted)
    let cases: Vec<(&str, &str, String)> = vec![
        (
            // "len 0..68000": the empty string is inside the modelled length.
            "code at the modelled length floor",
            "empty-code",
            String::new(),
        ),
        (
            "a one-line expression",
            "one-liner",
            "1 + 1".to_string(),
        ),
        (
            "a multi-line cell",
            "multi-line",
            "import sys\nprint(sys.version)\nspark.sql('SELECT 1').show()\n".to_string(),
        ),
        (
            // A notebook magic, which is code to the session like any other.
            "a cell magic",
            "magic",
            "%%sql\nSELECT count(*) FROM sales".to_string(),
        ),
        (
            "code outside ASCII",
            "unicode",
            "print('h\u{e9}llo w\u{f6}rld \u{2014} \u{1f389}')".to_string(),
        ),
        (
            "code carrying quotes and backslashes",
            "quoting",
            "path = \"s3://bucket/key\\\\part=1\"\nprint(path)".to_string(),
        ),
        (
            "code at the modelled length bound",
            "longest-code",
            longest_code,
        ),
    ];

    for (case, session, code) in &cases {
        ready_session(&router, &engine, session).await;

        let id = run(&router, session, code).await;
        // The first statement of a session carries the id its sequence counts from.
        assert_eq!(
            id,
            i64::from(FIRST_STATEMENT_ID),
            "case {case}: expected the first statement of a session to be \
             {FIRST_STATEMENT_ID}, got {id}"
        );

        let read = get(&router, session, id).await;
        assert_eq!(
            read["Id"].as_i64(),
            Some(id),
            "case {case}: the read must report the id the run answered, got {read}"
        );
        // "The execution code of the statement", exactly as the run submitted it.
        assert_eq!(
            read["Code"].as_str().unwrap_or_default(),
            code.as_str(),
            "case {case}: the code must round-trip, got {} bytes",
            read["Code"].as_str().unwrap_or_default().len()
        );
        // "The state while request is actioned": the session has taken it and nothing has
        // actioned it yet.
        assert_eq!(
            read["State"].as_str(),
            Some(STATE_WAITING),
            "case {case}: expected {STATE_WAITING} straight after the run, got {read}"
        );
        // "The unix time and date that the job definition was started."
        assert!(
            read["StartedOn"].as_i64().is_some_and(|at| at > 0),
            "case {case}: expected a StartedOn instant, got {read}"
        );
        // A statement that has not finished has neither an end nor output, and its code
        // execution has made no progress.
        assert_eq!(
            read["CompletedOn"].as_i64().unwrap_or_default(),
            0,
            "case {case}: an unfinished statement reports no CompletedOn, got {read}"
        );
        assert_eq!(
            read["Progress"].as_f64().unwrap_or_default(),
            0.0,
            "case {case}: an unfinished statement has made no progress, got {read}"
        );
        assert!(
            read.get("Output").is_none_or(Value::is_null),
            "case {case}: an unfinished statement reports no Output, got {read}"
        );
    }

    // Each run is its own record on its own session: after every one has landed, each
    // session still reports ITS code under the same id, so no run overwrote another.
    for (case, session, code) in &cases {
        let read = get(&router, session, i64::from(FIRST_STATEMENT_ID)).await;
        assert_eq!(
            read["Code"].as_str().unwrap_or_default(),
            code.as_str(),
            "case {case}: every session keeps its own statement, got {} bytes",
            read["Code"].as_str().unwrap_or_default().len()
        );
    }
}

/// A session mints its statement ids in SUBMISSION ORDER, counting up from the first, and
/// every id keeps addressing the code it was minted for.
#[tokio::test]
async fn one_session_mints_statement_ids_in_submission_order() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let session = "ordered";
    ready_session(&router, &engine, session).await;

    // Rows: how many statements one session holds, from one up.
    let submitted: Vec<String> = (0..4).map(|index| format!("print({index})")).collect();
    let mut minted = Vec::new();
    for (index, code) in submitted.iter().enumerate() {
        let id = run(&router, session, code).await;
        minted.push(id);
        let expected = i64::from(FIRST_STATEMENT_ID) + index as i64;
        assert_eq!(
            id, expected,
            "case statement {index}: expected the ids to count up to {expected}, got {minted:?}"
        );
        // The session takes the next statement once the clock has finished this one.
        set_statement_due(&engine, session, already_past());
    }

    for (index, (id, code)) in minted.iter().zip(submitted.iter()).enumerate() {
        let read = get(&router, session, *id).await;
        assert_eq!(
            read["Code"].as_str(),
            Some(code.as_str()),
            "case statement {index}: id {id} must still address its own code, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A session still actioning a statement cannot take another: the run is refused, nothing
/// is recorded for it, and the session takes a statement again once the clock has finished
/// the one in flight.
#[tokio::test]
async fn a_run_on_a_session_still_actioning_a_statement_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let session = "busy";
    ready_session(&router, &engine, session).await;

    let in_flight = run(&router, session, "import time; time.sleep(60)").await;
    // The statement stays in flight for the whole span, whatever a caller does next.
    set_statement_due(&engine, session, far_future());

    for (case, code) in [
        ("a second run", "print('second')"),
        ("a third run", "print('third')"),
    ] {
        let (status, body) = call(
            &router,
            "RunStatement",
            &json!({ "SessionId": session, "Code": code }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a busy session must be refused with 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            messages::session_busy_with_statement(session, in_flight as i32),
            "case {case}: the refusal must name the statement the session is still \
             running, got {body}"
        );
    }

    // A refused run records nothing: the next id the session mints is the one after the
    // statement in flight, not after the refusals.
    let (status, body) = call(
        &router,
        "GetStatement",
        &json!({ "SessionId": session, "Id": in_flight + 1 }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a refused run must record no statement, got {status} with {body}"
    );

    set_statement_due(&engine, session, already_past());
    let next = run(&router, session, "print('taken')").await;
    assert_eq!(
        next,
        in_flight + 1,
        "a session takes a statement again once the clock has finished the one in flight, \
         and mints the next id for it, got {next}"
    );
}

/// A session whose runtime is not up takes no statement: a run is refused in every status
/// but `READY`, and nothing is recorded for it.
///
/// The rows are the statuses a session reaches by the clock alone: the one its create put
/// it in, and the one its own documented timeout brings it to.
#[tokio::test]
async fn a_run_on_a_session_that_is_not_ready_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // (case, the session, the create body, how far its clock is pulled back, the status)
    let cases: Vec<(&str, &str, Value, Duration, &str)> = vec![
        (
            "a session whose provisioning has not finished",
            "provisioning",
            json!({}),
            Duration::ZERO,
            STATUS_PROVISIONING,
        ),
        (
            // "The number of minutes before session times out", at the modelled floor of
            // `Timeout` ("range 1.."), so one minute later the session has ended.
            "a session its own timeout has ended",
            "timed-out",
            json!({ "Timeout": 1, "IdleTimeout": 1 }),
            Duration::from_secs(2 * SECONDS_PER_MINUTE),
            STATUS_TIMEOUT,
        ),
    ];

    for (case, session, extra, elapsed, status_expected) in &cases {
        create_session(&router, session, extra.clone()).await;
        if elapsed.is_zero() {
            // The provisioning this row is about is held open for the whole row.
            set_session_due(&engine, session, far_future());
        } else {
            rewind_session(&engine, session, *elapsed);
            set_session_due(&engine, session, already_past());
        }

        let (status, body) = call(
            &router,
            "RunStatement",
            &json!({ "SessionId": session, "Code": "print('too early')" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a session that is not ready must be refused with 400, got \
             {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("IllegalSessionStateException"),
            "case {case}: expected IllegalSessionStateException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            messages::session_not_ready(session, status_expected),
            "case {case}: the refusal must name the session and the status it is in, got \
             {body}"
        );

        // The refused run recorded nothing.
        let (status, body) = call(
            &router,
            "GetStatement",
            &json!({ "SessionId": session, "Id": FIRST_STATEMENT_ID }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a refused run records no statement, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::statement_not_found(session, FIRST_STATEMENT_ID),
            "case {case}: expected the miss to name the statement asked for, got {body}"
        );
    }
}

/// A run or a read for a session this endpoint never created is a miss, and so is one for a
/// statement id its session never minted.
#[tokio::test]
async fn an_address_nothing_holds_is_a_miss() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let session = "held";
    ready_session(&router, &engine, session).await;
    let held = run(&router, session, "print('held')").await;

    for (case, id) in [
        ("an id nothing created", "absent"),
        ("a prefix of a stored id", "hel"),
        ("a stored id in another case", "HELD"),
    ] {
        for operation in ["RunStatement", "GetStatement"] {
            let (status, body) = call(
                &router,
                operation,
                &json!({ "SessionId": id, "Code": "print(1)", "Id": held }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case}: {operation} on an unknown session must be refused, got \
                 {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with("EntityNotFoundException"),
                "case {case}: expected EntityNotFoundException, got {} in {body}",
                error_code(&body)
            );
            assert_eq!(
                error_message(&body),
                messages::session_not_found(id),
                "case {case}: the refusal must name the session the request addressed, got \
                 {body}"
            );
        }
    }

    // The same session through another Region's endpoint: a session is served only by the
    // endpoint that created it.
    for region in ["eu-west-1", "us-west-2"] {
        let (status, body) = call_in_region(
            &router,
            region,
            "RunStatement",
            &json!({ "SessionId": session, "Code": "print(1)" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {region}: a session is reachable only through its own endpoint, got \
             {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::session_not_found(session),
            "case {region}: expected the miss to name the session, got {body}"
        );
    }

    // An id the session never minted, on either side of the one it did.
    for id in [-1, 0, held + 1, held + 99] {
        let (status, body) = call(
            &router,
            "GetStatement",
            &json!({ "SessionId": session, "Id": id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {id}: a statement nothing recorded must be refused, got {status} with \
             {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {id}: expected EntityNotFoundException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            messages::statement_not_found(session, id as i32),
            "case {id}: the refusal must name the statement asked for, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The state the clock owes
// ---------------------------------------------------------------------------

/// A statement leaves `WAITING` when the clock owes it its terminal state, and not before:
/// while the due instant is ahead every read reports it still waiting, and once it is past
/// every read reports the same terminal record, carrying the output of a statement nothing
/// ran.
#[tokio::test]
async fn the_clock_owes_every_statement_its_terminal_state() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let session = "owed";
    ready_session(&router, &engine, session).await;
    let id = run(&router, session, "spark.sql('SELECT 1').show()").await;

    // Before the due instant: reading does not move the statement, however often.
    set_statement_due(&engine, session, far_future());
    for read_number in 1..=3 {
        let read = get(&router, session, id).await;
        assert_eq!(
            read["State"].as_str(),
            Some(STATE_WAITING),
            "read {read_number}: a statement whose span has not elapsed is still \
             {STATE_WAITING}, got {read}"
        );
    }

    // At the due instant: the state the clock owed it, which for a statement no runtime
    // ran is the modelled error state carrying the reason.
    reach_due_instant(&engine, session);
    let settled = get(&router, session, id).await;
    assert_eq!(
        settled["State"].as_str(),
        Some(STATE_ERROR),
        "expected {STATE_ERROR} once the span has elapsed, got {settled}"
    );
    let output = settled
        .get("Output")
        .cloned()
        .unwrap_or_else(|| panic!("a finished statement reports an Output, got {settled}"));
    assert_eq!(
        output["Status"].as_str(),
        Some(STATE_ERROR),
        "the output status must be the state the statement came to rest in, got {output}"
    );
    assert_eq!(
        output["ErrorName"].as_str(),
        Some(STATEMENT_NOT_EXECUTED_ERROR_NAME),
        "the output must name the error, got {output}"
    );
    assert_eq!(
        output["ErrorValue"].as_str(),
        Some(STATEMENT_NOT_EXECUTED_MESSAGE),
        "the output must state why the statement produced nothing, got {output}"
    );
    assert!(
        output.get("Data").is_none_or(Value::is_null),
        "a statement that ran no code reports no output data, got {output}"
    );
    let started_on = settled["StartedOn"].as_i64().unwrap_or_default();
    let completed_on = settled["CompletedOn"].as_i64().unwrap_or_default();
    let span = i64::try_from(STATEMENT_SETTLES_AFTER.as_millis()).expect("a span in range");
    assert!(
        started_on > 0,
        "expected a StartedOn instant, got {settled}"
    );
    assert_eq!(
        completed_on - started_on,
        span,
        "expected the statement to complete the span after it started, got {started_on} \
         then {completed_on}"
    );

    // The terminal state was owed by the CLOCK: reading again reports the same record.
    for read_number in 1..=3 {
        let again = get(&router, session, id).await;
        assert_eq!(
            again, settled,
            "read {read_number}: a settled statement reports what the first read did, got \
             {again}"
        );
    }
}
