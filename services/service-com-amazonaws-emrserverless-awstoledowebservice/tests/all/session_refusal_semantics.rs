// SPDX-License-Identifier: CC0-1.0

//! What EMR Serverless answers for the two session calls whose outcome is settled before
//! anything is done: a start the application does not admit, and a terminate that has
//! nothing left to end.
//!
//! Every assertion goes through the real router (restJson1, SigV4 signing name
//! `emr-serverless`), so each case exercises routing, deserialization, the handler, the
//! persisted record and serialization — the same path an SDK client takes.
//!
//! Two rules are under test, each asserted over a SET of inputs rather than over one
//! request:
//!
//! 1. `StartSession` on an application that does not have interactive sessions enabled is
//!    refused with the modelled `ValidationException` (HTTP 400) carrying this service's
//!    FIXED sentence for that condition: the same text whatever release the application
//!    runs, whichever other interactive use cases it enabled and whatever the request
//!    asked for, naming no part of either and no remedy call.
//! 2. `TerminateSession` carries `@idempotent`, so once a session has no live work left
//!    for it to end it answers the modelled 200 with the STORED `applicationId` and
//!    `sessionId` and writes nothing, however often it is repeated. An id that names no
//!    session on the application is the one exception: there is no record to report, so
//!    it is the modelled `ResourceNotFoundException` (HTTP 404) with the fixed phrase for
//!    that resource class.

use serde_json::{json, Value};

use super::application_lifecycle::{emulator, emulator_with_engine, wait_for_transition, Answer};
use super::session_lifecycle::{
    application, get_session, minimal_session, session_application, sessions_enabled,
    start_session, EARLIEST_SESSION_RELEASE,
};
use super::session_termination::{force_session_state, terminate_session};

/// The `ValidationException` message a start refused for an application that does not
/// enable interactive sessions carries, written out here rather than read from
/// `crate::state` so wrong wording in `src/` cannot make this test pass.
///
/// Source: the refusal AWS records for a `StartSession` on an application created with no
/// `interactiveConfiguration`. It is fixed text with no variable parts: the condition it
/// states belongs to the application, not to the request.
const SESSIONS_NOT_ENABLED: &str = "Sessions are not enabled on this application";

/// The `ResourceNotFoundException` phrase for a session id that names nothing, written
/// out here for the same reason. Source: the recorded refusal for a `TerminateSession`
/// naming a session the application does not hold.
const SESSION_NOT_FOUND: &str = "Session with given ID doesn't exist";

/// A release later than the earliest one that serves sessions, so rule 1 is asserted on
/// more than the boundary release. Source: `StartSession` — "This operation is supported
/// for EMR release 7.13.0 and later".
const LATER_SESSION_RELEASE: &str = "emr-8.0.0";

/// The stored members of a session that a call acting on nothing must leave exactly as
/// they are.
fn stored_session(answer: &Answer) -> Value {
    json!({
        "state": answer.at("/session/state"),
        "stateDetails": answer.at("/session/stateDetails"),
        "updatedAt": answer.at("/session/updatedAt"),
        "startedAt": answer.at("/session/startedAt"),
        "endedAt": answer.at("/session/endedAt"),
    })
}

/// Rule 1: the refusal for an application without interactive sessions enabled is the
/// service's fixed sentence for that condition.
///
/// The rows differ along the `interactiveConfiguration` the application was created with
/// (absent, empty, sessions explicitly off, other use cases on instead) and along the
/// release it runs, with the application that DOES enable sessions as the control that
/// must be accepted — so a handler that refused every start could not pass.
#[tokio::test]
async fn start_session_states_one_fixed_sentence_for_an_application_that_enables_no_sessions() {
    let router = emulator();

    // (case, releaseLabel, interactiveConfiguration, whether the start is admitted)
    let cases: Vec<(&str, &str, Option<Value>, bool)> = vec![
        (
            "no interactive configuration at all",
            EARLIEST_SESSION_RELEASE,
            None,
            false,
        ),
        (
            "an empty interactive configuration",
            EARLIEST_SESSION_RELEASE,
            Some(json!({})),
            false,
        ),
        (
            "sessions explicitly disabled",
            EARLIEST_SESSION_RELEASE,
            Some(json!({ "sessionEnabled": false })),
            false,
        ),
        (
            "other interactive use cases enabled, but not sessions",
            EARLIEST_SESSION_RELEASE,
            Some(json!({ "livyEndpointEnabled": true, "studioEnabled": true })),
            false,
        ),
        (
            "sessions disabled beside another use case that is on",
            LATER_SESSION_RELEASE,
            Some(json!({ "sessionEnabled": false, "livyEndpointEnabled": true })),
            false,
        ),
        (
            "no interactive configuration on a later release",
            LATER_SESSION_RELEASE,
            None,
            false,
        ),
        (
            "sessions enabled",
            EARLIEST_SESSION_RELEASE,
            Some(sessions_enabled()),
            true,
        ),
        (
            "sessions enabled on a later release",
            LATER_SESSION_RELEASE,
            Some(json!({ "sessionEnabled": true, "studioEnabled": true })),
            true,
        ),
    ];

    for (index, (case, release_label, interactive, admitted)) in cases.iter().enumerate() {
        let (application_id, _) = application(
            &router,
            &format!("tok-enabled-app-{index}"),
            release_label,
            interactive.clone(),
        )
        .await;
        let answer = start_session(
            &router,
            &application_id,
            minimal_session(&format!("tok-enabled-{index}")),
        )
        .await;

        if *admitted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: an application that enables sessions admits the start, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 400,
            "case {case:?}: StartSession expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.resolved_error_shape(),
            Some("ValidationException"),
            "case {case:?}: a restJson1 client resolves the shape from x-amzn-errortype, got {:?}",
            answer.resolved_error_shape()
        );
        assert_eq!(
            answer.at("/message").as_str(),
            Some(SESSIONS_NOT_ENABLED),
            "case {case:?}: the refusal states the condition in its fixed sentence, got {:?}",
            answer.at("/message")
        );
    }
}

/// How a session came to have no live work left for a terminate to end.
#[derive(Debug, Clone, Copy)]
enum Reached {
    /// By a terminate the clock has finished: the session settled on `TERMINATED`.
    TerminatedByRequest,
    /// By being held in a `SessionState` through the store the handlers share, so the
    /// states a client cannot drive a session into are covered too, and the state under
    /// test is the one the repeated terminate sees.
    HeldIn(&'static str),
}

/// One row of rule 2's table: a session with nothing left to end, and how often the
/// terminate is repeated on it.
struct NoLiveWorkCase {
    case: &'static str,
    /// The `StartSession` body, beyond the client token the row is given.
    body: fn(&str) -> Value,
    reached: Reached,
    /// The `SessionState` the repeated terminates find it in.
    state: &'static str,
    repeats: usize,
}

/// The rows differ along the state that leaves the terminate nothing to do (the one a
/// terminate settles on, the one it is still in flight in, and the failure a session can
/// end on without any terminate), along how the session got there, along which optional
/// members the start named, and along how many times the terminate is repeated.
fn no_live_work_cases() -> Vec<NoLiveWorkCase> {
    vec![
        NoLiveWorkCase {
            case: "a session terminated by request",
            body: minimal_session,
            reached: Reached::TerminatedByRequest,
            state: "TERMINATED",
            repeats: 1,
        },
        NoLiveWorkCase {
            case: "a session terminated by request, terminated three times over",
            body: minimal_session,
            reached: Reached::TerminatedByRequest,
            state: "TERMINATED",
            repeats: 3,
        },
        NoLiveWorkCase {
            case: "a named session carrying tags, terminated by request",
            body: |token| {
                let mut body = minimal_session(token);
                body["name"] = json!("idempotent-terminate");
                body["tags"] = json!({ "team": "data", "env": "test" });
                body
            },
            reached: Reached::TerminatedByRequest,
            state: "TERMINATED",
            repeats: 2,
        },
        NoLiveWorkCase {
            case: "a session that asked for an idle timeout of its own",
            body: |token| {
                let mut body = minimal_session(token);
                body["idleTimeoutMinutes"] = json!(5);
                body
            },
            reached: Reached::TerminatedByRequest,
            state: "TERMINATED",
            repeats: 2,
        },
        NoLiveWorkCase {
            case: "a session whose termination is still in flight",
            body: minimal_session,
            reached: Reached::HeldIn("TERMINATING"),
            state: "TERMINATING",
            repeats: 2,
        },
        NoLiveWorkCase {
            case: "a session that ended in failure, with no terminate at all",
            body: minimal_session,
            reached: Reached::HeldIn("FAILED"),
            state: "FAILED",
            repeats: 2,
        },
        NoLiveWorkCase {
            case: "a session already terminated, carrying its own state details",
            body: minimal_session,
            reached: Reached::HeldIn("TERMINATED"),
            state: "TERMINATED",
            repeats: 2,
        },
    ]
}

/// Rule 2: a terminate asked for on a session with no live work left is the idempotent
/// no-op `@idempotent` promises — 200, the stored ids, and nothing written — however
/// often it is repeated.
#[tokio::test]
async fn terminate_session_is_an_idempotent_no_op_once_a_session_has_nothing_left_to_end() {
    let (router, engine) = emulator_with_engine();
    let (application_id, _) = session_application(&router, "tok-idempotent-app").await;

    for (index, case) in no_live_work_cases().iter().enumerate() {
        let label = case.case;
        let token = format!("tok-idempotent-{index}");
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

        match case.reached {
            Reached::TerminatedByRequest => {
                let first = terminate_session(&router, &application_id, &session_id).await;
                assert_eq!(
                    first.status, 200,
                    "case {label:?}: the first terminate expected 200, got {} with {:?}",
                    first.status, first.body
                );
                wait_for_transition();
            }
            Reached::HeldIn(state) => {
                let details = format!("Session held in {state} by the test.");
                force_session_state(&engine, &application_id, &session_id, state, &details);
            }
        }

        let before = get_session(&router, &application_id, &session_id).await;
        assert_eq!(
            before.at("/session/state").as_str(),
            Some(case.state),
            "case {label:?}: the row must reach {:?} before the repeats, got {:?}",
            case.state,
            before.at("/session/state")
        );
        let snapshot = stored_session(&before);

        for repeat in 1..=case.repeats {
            let answer = terminate_session(&router, &application_id, &session_id).await;
            assert_eq!(
                answer.status, 200,
                "case {label:?} repeat {repeat}: an idempotent terminate expected 200, got {} \
                 with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.at("/applicationId").as_str(),
                Some(application_id.as_str()),
                "case {label:?} repeat {repeat}: it reports the stored application id, got {:?}",
                answer.at("/applicationId")
            );
            assert_eq!(
                answer.at("/sessionId").as_str(),
                Some(session_id.as_str()),
                "case {label:?} repeat {repeat}: it reports the stored session id, got {:?}",
                answer.at("/sessionId")
            );

            let after = get_session(&router, &application_id, &session_id).await;
            assert_eq!(
                stored_session(&after),
                snapshot,
                "case {label:?} repeat {repeat}: a terminate with nothing left to end writes \
                 nothing, got {:?} against {snapshot:?}",
                stored_session(&after)
            );
        }
    }
}

/// Rule 2's exception: an id that names no session of the application has no stored
/// record to report, so it is the modelled 404 rather than the no-op above — including
/// after the session that DOES exist has been terminated, which is when a cleanup path
/// retries with a stale id.
///
/// The rows differ along the shape of the id (one character, `SessionId`'s maximum
/// length, an id shaped like a minted one) and along whether the application holds any
/// terminated session at all.
#[tokio::test]
async fn terminate_session_refuses_an_id_that_names_no_session() {
    let router = emulator();
    let (application_id, _) = session_application(&router, "tok-stale-app").await;

    let started = start_session(&router, &application_id, minimal_session("tok-stale-1")).await;
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
    let terminated = terminate_session(&router, &application_id, &session_id).await;
    assert_eq!(
        terminated.status, 200,
        "TerminateSession expected 200, got {} with {:?}",
        terminated.status, terminated.body
    );
    wait_for_transition();

    // (case, the session id that names nothing)
    let cases: Vec<(&str, String)> = vec![
        ("a session id of one character", "7".to_string()),
        (
            "a session id at SessionId's maximum length",
            "a".repeat(64),
        ),
        (
            "an id shaped like a minted one",
            format!("{session_id}0"),
        ),
    ];

    for (case, unknown) in cases {
        let answer = terminate_session(&router, &application_id, &unknown).await;
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
            Some(SESSION_NOT_FOUND),
            "case {case:?}: a not-found refusal states the resource class, got {:?}",
            answer.at("/message")
        );
    }

    // The session the application does hold is still readable in the state it ended on:
    // none of those refusals touched it.
    let read = get_session(&router, &application_id, &session_id).await;
    assert_eq!(
        read.at("/session/state").as_str(),
        Some("TERMINATED"),
        "a refused terminate leaves the session that exists alone, got {:?}",
        read.at("/session/state")
    );
}
