// SPDX-License-Identifier: CC0-1.0

//! `TerminateSession`, end to end over HTTP.
//!
//! Every assertion goes through the real router (restJson1, SigV4 signing name
//! `emr-serverless`), so each case exercises routing, deserialization, the handler, the
//! persisted record and serialization — the same path an SDK client takes.
//!
//! The rules under test, each asserted over a SET of inputs rather than over one
//! request:
//!
//! 1. A terminate asked for on a LIVE session moves that session to `TERMINATING`
//!    straight away and the clock owes it `TERMINATED` after the transition span, with
//!    the end instant that state implies; the termination REPLACES whatever change the
//!    clock still owed the session, and the session stays readable afterwards.
//! 2. Whether a terminate is accepted is decided by the session's `SessionState` alone:
//!    it moves every state that names a live session, it is the idempotent no-op that
//!    writes nothing for a session already terminating, and it is refused with the
//!    modelled `ValidationException` (HTTP 400) for a session that has reached a state it
//!    never leaves — leaving the state and its details exactly as they were.
//! 3. Both ids a terminate names have to name something, in the scope the route gives
//!    them: an application the account does not hold, or a session that application does
//!    not hold, is the modelled `ResourceNotFoundException` (HTTP 404), and the session
//!    of ANOTHER application is not that application's session.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_emrserverless_awstoledowebservice::state::ComAmazonawsEmrserverlessAwstoledowebserviceState;

use super::application_lifecycle::{
    call, emulator, emulator_with_engine, matches_resource_id, wait_for_transition, Answer,
};
use super::session_lifecycle::{get_session, minimal_session, session_application, start_session};

/// Every value of the `SessionState` enum, with what a terminate asked for on a session
/// carrying it does.
///
/// Written out here rather than read from `crate::state`, so a wrong catalog in `src/`
/// cannot make this test pass. Source: the `SessionState` enum —
/// `smithy_query(query="shape",shape="SessionState")` lists exactly `SUBMITTED`,
/// `STARTING`, `STARTED`, `IDLE`, `BUSY`, `FAILED`, `TERMINATING`, `TERMINATED`. A
/// terminate ends the states that name a live session; `TERMINATING` already has the
/// termination in flight, so an `@idempotent` retry reports it and writes nothing; and
/// `FAILED` and `TERMINATED` name a session that is over, so there is nothing left to
/// end.
const SESSION_STATES: &[(&str, Outcome)] = &[
    ("SUBMITTED", Outcome::Terminates),
    ("STARTING", Outcome::Terminates),
    ("STARTED", Outcome::Terminates),
    ("IDLE", Outcome::Terminates),
    ("BUSY", Outcome::Terminates),
    ("FAILED", Outcome::Refused),
    ("TERMINATING", Outcome::Unchanged),
    ("TERMINATED", Outcome::Refused),
];

/// What a terminate does to a session in a given `SessionState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// It is accepted and moves the session to `TERMINATING`.
    Terminates,
    /// It answers the modelled success and leaves the record exactly as it is.
    Unchanged,
    /// It is refused with the modelled `ValidationException` and writes nothing.
    Refused,
}

/// What a not-found refusal says, by resource class: EMR Serverless states the class that
/// was not found and names no identifier. Written out here rather than read from
/// `crate::state`. Source: the recorded refusals this service's other handlers already
/// answer with (`application_lifecycle::an_id_that_names_nothing_is_refused`).
const APPLICATION_NOT_FOUND: &str = "Application does not exist";
const SESSION_NOT_FOUND: &str = "Session does not exist";

async fn terminate_session(
    router: &MultiServiceRouter,
    application_id: &str,
    session_id: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/applications/{application_id}/sessions/{session_id}"),
        None,
    )
    .await
}

/// `String1024`'s constraints on `Session$stateDetails`: length 1..1024 and the pattern
/// `.*\S.*` (at least one non-whitespace character).
fn is_state_details(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| (1..=1024).contains(&text.chars().count()) && text.trim() != "")
}

/// Put a session into `state` through the store the handlers share, so the whole
/// `SessionState` catalog can be asserted — the states a client cannot drive a session
/// into over HTTP included — and clear whatever the clock owed it, so the state under
/// test is the one the terminate sees.
fn force_session_state(
    engine: &engine::InMemoryEngine,
    application_id: &str,
    session_id: &str,
    state: &str,
    state_details: &str,
) {
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
    session.session.state = state.to_string();
    session.session.state_details = state_details.to_string();
    session.pending = None;
}

/// One row of the lifecycle table: a live session, and how it got to be live.
struct TerminateCase {
    case: &'static str,
    /// The `StartSession` body, beyond the client token the row is given.
    body: fn(&str) -> Value,
    /// Whether the row waits out the start transition before terminating, so the
    /// terminate acts on a session that has come up rather than on one still arriving.
    settle_first: bool,
}

/// The rows differ along the state the terminate finds the session in (the transient one
/// the start left, or the one the clock settled it on), along which optional members the
/// start named, and along whether the session's own `idleTimeoutMinutes` already owed it
/// a termination.
fn terminate_cases() -> Vec<TerminateCase> {
    vec![
        TerminateCase {
            case: "a session the start has not brought up yet",
            body: minimal_session,
            settle_first: false,
        },
        TerminateCase {
            case: "a session that has come up",
            body: minimal_session,
            settle_first: true,
        },
        TerminateCase {
            case: "a named session carrying tags, terminated straight away",
            body: |token| {
                let mut body = minimal_session(token);
                body["name"] = json!("termination-case");
                body["tags"] = json!({ "team": "data", "env": "test" });
                body
            },
            settle_first: false,
        },
        TerminateCase {
            case: "a session whose own idle timeout already owed it a termination",
            body: |token| {
                let mut body = minimal_session(token);
                body["idleTimeoutMinutes"] = json!(5);
                body
            },
            settle_first: true,
        },
        TerminateCase {
            case: "a session that asked for no idle timeout",
            body: |token| {
                let mut body = minimal_session(token);
                body["idleTimeoutMinutes"] = json!(0);
                body
            },
            settle_first: true,
        },
    ]
}

/// Rule 1: a terminate moves a live session to `TERMINATING`, the clock owes it
/// `TERMINATED`, and the session it ended is still readable in that state — and rule 2's
/// refusal, asserted here on the retry that follows a terminate.
#[tokio::test]
async fn terminate_session_moves_a_live_session_through_terminating_to_terminated() {
    let router = emulator();
    let (application_id, _) = session_application(&router, "tok-terminate-app").await;

    for (index, case) in terminate_cases().iter().enumerate() {
        let label = case.case;
        let token = format!("tok-terminate-{index}");
        let started = start_session(&router, &application_id, (case.body)(&token)).await;
        assert_eq!(
            started.status, 200,
            "case {label:?}: StartSession expected 200, got {} with {:?}",
            started.status, started.body
        );
        let session_id = started
            .at("/sessionId")
            .as_str()
            .unwrap_or_default()
            .to_string();
        if case.settle_first {
            wait_for_transition();
        }

        let terminated = terminate_session(&router, &application_id, &session_id).await;
        assert_eq!(
            terminated.status, 200,
            "case {label:?}: TerminateSession expected 200, got {} with {:?}",
            terminated.status, terminated.body
        );
        assert_eq!(
            terminated.at("/applicationId").as_str(),
            Some(application_id.as_str()),
            "case {label:?}: the terminate reports the application the session belongs to, \
             got {:?}",
            terminated.at("/applicationId")
        );
        assert_eq!(
            terminated.at("/sessionId").as_str(),
            Some(session_id.as_str()),
            "case {label:?}: the terminate reports the session it ended, got {:?}",
            terminated.at("/sessionId")
        );
        assert!(
            matches_resource_id(terminated.at("/sessionId").as_str().unwrap_or_default()),
            "case {label:?}: sessionId must match SessionId's ^[0-9a-z]+$, got {:?}",
            terminated.at("/sessionId")
        );

        // The terminate has returned; the session is reported in the transient state it
        // documents ("it enters the TERMINATING state and then the TERMINATED state"),
        // and has not ended yet.
        let terminating = get_session(&router, &application_id, &session_id).await;
        assert_eq!(
            terminating.at("/session/state").as_str(),
            Some("TERMINATING"),
            "case {label:?}: a session the clock has not finished terminating is TERMINATING, \
             got {:?}",
            terminating.at("/session/state")
        );
        assert!(
            is_state_details(&terminating.at("/session/stateDetails")),
            "case {label:?}: stateDetails must satisfy String1024, got {:?}",
            terminating.at("/session/stateDetails")
        );
        assert_eq!(
            terminating.at("/session/endedAt"),
            Value::Null,
            "case {label:?}: a session that has not finished terminating has no end instant, \
             got {:?}",
            terminating.at("/session/endedAt")
        );

        // Rule 2's in-flight row, where the termination this call asks for is already
        // under way: `@idempotent`, so the retry reports the same ids and writes nothing.
        let in_flight = terminate_session(&router, &application_id, &session_id).await;
        assert_eq!(
            in_flight.status, 200,
            "case {label:?}: a retry while the termination is in flight expected 200, got {} \
             with {:?}",
            in_flight.status, in_flight.body
        );
        assert_eq!(
            in_flight.at("/sessionId").as_str(),
            Some(session_id.as_str()),
            "case {label:?}: the retry reports the session it was asked for, got {:?}",
            in_flight.at("/sessionId")
        );
        let after_in_flight = get_session(&router, &application_id, &session_id).await;
        assert_eq!(
            after_in_flight.at("/session/state").as_str(),
            Some("TERMINATING"),
            "case {label:?}: a retry does not move the state it found, got {:?}",
            after_in_flight.at("/session/state")
        );
        assert_eq!(
            after_in_flight.at("/session/updatedAt").as_f64(),
            terminating.at("/session/updatedAt").as_f64(),
            "case {label:?}: a retry writes nothing, got {:?}",
            after_in_flight.at("/session/updatedAt")
        );

        // The clock owes it `TERMINATED` once the transition span has passed, at the
        // instant that change fell due.
        wait_for_transition();
        let ended = get_session(&router, &application_id, &session_id).await;
        assert_eq!(
            ended.at("/session/state").as_str(),
            Some("TERMINATED"),
            "case {label:?}: the clock owes a terminating session TERMINATED, got {:?}",
            ended.at("/session/state")
        );
        let ended_at = ended.at("/session/endedAt").as_f64();
        assert!(
            ended_at.is_some(),
            "case {label:?}: a terminated session reports the instant it ended, got {:?}",
            ended.at("/session/endedAt")
        );
        assert_eq!(
            ended.at("/session/updatedAt").as_f64(),
            ended_at,
            "case {label:?}: updatedAt moved to the instant the termination fell due, got {:?}",
            ended.at("/session/updatedAt")
        );
        assert!(
            ended_at >= ended.at("/session/createdAt").as_f64(),
            "case {label:?}: a session cannot end before it was created, got {ended_at:?} \
             against {:?}",
            ended.at("/session/createdAt")
        );

        // The terminate REPLACED whatever the clock still owed the session: one asked to
        // end before it came up never reports a start instant, and one that had come up
        // keeps the one it had.
        if case.settle_first {
            assert!(
                ended.at("/session/startedAt").as_f64().is_some(),
                "case {label:?}: a session that came up keeps its start instant, got {:?}",
                ended.at("/session/startedAt")
            );
        } else {
            assert_eq!(
                ended.at("/session/startedAt"),
                Value::Null,
                "case {label:?}: a session asked to end before it came up never starts, got {:?}",
                ended.at("/session/startedAt")
            );
        }

        // The session is still READABLE after it ended ("You can still access the Spark
        // History Server for a terminated session"), reporting the record the start
        // stored rather than being dropped.
        assert_eq!(
            ended.at("/session/arn").as_str(),
            started.at("/arn").as_str(),
            "case {label:?}: a terminated session still reports its ARN, got {:?}",
            ended.at("/session/arn")
        );

        // Rule 2 on the retry: the session is over, so there is nothing left to
        // terminate and the refusal writes nothing.
        let retried = terminate_session(&router, &application_id, &session_id).await;
        assert_eq!(
            retried.status, 400,
            "case {label:?}: terminating a terminated session expected 400, got {} with {:?}",
            retried.status, retried.body
        );
        assert_eq!(
            retried.error_code(),
            "ValidationException",
            "case {label:?}: expected ValidationException, got {:?}",
            retried.body
        );
        assert_eq!(
            retried.resolved_error_shape(),
            Some("ValidationException"),
            "case {label:?}: a restJson1 client resolves the shape from x-amzn-errortype, \
             got {:?}",
            retried.resolved_error_shape()
        );
        assert!(
            retried
                .at("/message")
                .as_str()
                .is_some_and(|message| message.trim() != ""),
            "case {label:?}: a refusal states its condition, got {:?}",
            retried.at("/message")
        );

        let after_refusal = get_session(&router, &application_id, &session_id).await;
        assert_eq!(
            after_refusal.at("/session/state").as_str(),
            Some("TERMINATED"),
            "case {label:?}: a refused terminate leaves the state alone, got {:?}",
            after_refusal.at("/session/state")
        );
        assert_eq!(
            after_refusal.at("/session/updatedAt").as_f64(),
            ended_at,
            "case {label:?}: a refused terminate writes nothing, got {:?}",
            after_refusal.at("/session/updatedAt")
        );
    }
}

/// Rule 2 over the WHOLE `SessionState` catalog: a terminate ends exactly the states that
/// name a live session, reports the in-flight one without writing, and is refused on the
/// ones that are over — with the record unchanged either way it does not act.
#[tokio::test]
async fn a_terminate_is_accepted_exactly_for_the_states_that_name_a_live_session() {
    let (router, engine) = emulator_with_engine();
    let (application_id, _) = session_application(&router, "tok-states-app").await;

    for (index, (state, outcome)) in SESSION_STATES.iter().enumerate() {
        let token = format!("tok-state-{index}");
        let started = start_session(&router, &application_id, minimal_session(&token)).await;
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

        let details = format!("Session held in {state} by the test.");
        force_session_state(&engine, &application_id, &session_id, state, &details);
        let before = get_session(&router, &application_id, &session_id).await;

        let answer = terminate_session(&router, &application_id, &session_id).await;
        match outcome {
            Outcome::Terminates => {
                assert_eq!(
                    answer.status, 200,
                    "case {state:?}: a live session can be terminated, got {} with {:?}",
                    answer.status, answer.body
                );
                let read = get_session(&router, &application_id, &session_id).await;
                assert_eq!(
                    read.at("/session/state").as_str(),
                    Some("TERMINATING"),
                    "case {state:?}: an accepted terminate moves the session to TERMINATING, \
                     got {:?}",
                    read.at("/session/state")
                );
                wait_for_transition();
                let ended = get_session(&router, &application_id, &session_id).await;
                assert_eq!(
                    ended.at("/session/state").as_str(),
                    Some("TERMINATED"),
                    "case {state:?}: the clock owes a terminating session TERMINATED, got {:?}",
                    ended.at("/session/state")
                );
                assert!(
                    ended.at("/session/endedAt").as_f64().is_some(),
                    "case {state:?}: a terminated session reports the instant it ended, got {:?}",
                    ended.at("/session/endedAt")
                );
            }
            Outcome::Unchanged => {
                assert_eq!(
                    answer.status, 200,
                    "case {state:?}: a termination already in flight is reported, got {} \
                     with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.at("/sessionId").as_str(),
                    Some(session_id.as_str()),
                    "case {state:?}: it reports the session asked for, got {:?}",
                    answer.at("/sessionId")
                );
            }
            Outcome::Refused => {
                assert_eq!(
                    answer.status, 400,
                    "case {state:?}: a session that is over has nothing left to terminate, \
                     got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ValidationException",
                    "case {state:?}: expected ValidationException, got {:?}",
                    answer.body
                );
            }
        }

        // Neither the reported in-flight termination nor the refusal writes anything: the
        // record is exactly what it was before the call.
        if *outcome != Outcome::Terminates {
            let read = get_session(&router, &application_id, &session_id).await;
            assert_eq!(
                read.at("/session/state").as_str(),
                Some(*state),
                "case {state:?}: a call that acts on nothing leaves the state alone, got {:?}",
                read.at("/session/state")
            );
            assert_eq!(
                read.at("/session/stateDetails").as_str(),
                Some(details.as_str()),
                "case {state:?}: a call that acts on nothing leaves the details alone, got {:?}",
                read.at("/session/stateDetails")
            );
            assert_eq!(
                read.at("/session/updatedAt").as_f64(),
                before.at("/session/updatedAt").as_f64(),
                "case {state:?}: a call that acts on nothing writes nothing, got {:?}",
                read.at("/session/updatedAt")
            );
        }
    }
}

/// Rule 3: each id a terminate names has to name something, in the scope the route gives
/// it. The rows differ along which id names nothing and along the shape of that id,
/// including an id that names a session of ANOTHER application.
#[tokio::test]
async fn a_terminate_naming_an_id_that_names_nothing_is_refused() {
    let router = emulator();
    let (application_id, _) = session_application(&router, "tok-notfound-app").await;
    let (other_application_id, _) = session_application(&router, "tok-notfound-other").await;

    let started = start_session(&router, &application_id, minimal_session("tok-notfound-1")).await;
    assert_eq!(
        started.status, 200,
        "StartSession expected 200, got {} with {:?}",
        started.status, started.body
    );
    let session_id = started
        .at("/sessionId")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // (case, applicationId, sessionId, the phrase the refusal states)
    let cases: Vec<(&str, String, String, &str)> = vec![
        (
            "an application id of one character",
            "0".to_string(),
            session_id.clone(),
            APPLICATION_NOT_FOUND,
        ),
        (
            "an application id at ApplicationId's maximum length",
            "z".repeat(64),
            session_id.clone(),
            APPLICATION_NOT_FOUND,
        ),
        (
            "an application id shaped like a minted one",
            "00000000deadbeef".to_string(),
            session_id.clone(),
            APPLICATION_NOT_FOUND,
        ),
        (
            "a session id of one character",
            application_id.clone(),
            "7".to_string(),
            SESSION_NOT_FOUND,
        ),
        (
            "a session id at SessionId's maximum length",
            application_id.clone(),
            "a".repeat(64),
            SESSION_NOT_FOUND,
        ),
        (
            "a session of another application",
            other_application_id.clone(),
            session_id.clone(),
            SESSION_NOT_FOUND,
        ),
    ];

    for (case, application, session, phrase) in cases {
        let answer = terminate_session(&router, &application, &session).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.at("/message").as_str(),
            Some(phrase),
            "case {case:?}: a not-found refusal states the resource class, got {:?}",
            answer.at("/message")
        );
    }

    // None of those refusals touched the session that does exist: it is still live, on
    // the application that holds it.
    let read = get_session(&router, &application_id, &session_id).await;
    assert_eq!(
        read.status, 200,
        "GetSession expected 200, got {} with {:?}",
        read.status, read.body
    );
    assert!(
        read.at("/session/state")
            .as_str()
            .is_some_and(|state| state != "TERMINATING" && state != "TERMINATED"),
        "a refused terminate must not end the session it failed to name, got {:?}",
        read.at("/session/state")
    );
}
