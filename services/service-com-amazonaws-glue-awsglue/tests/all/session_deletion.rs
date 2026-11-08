// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue interactive session DELETE slice: `DeleteSession` and the
//! `CreateSession` / `GetSession` / `RunStatement` calls that observe what it removed.
//!
//! RULE: a delete addressed by a session id removes the WHOLE record the Region it is served
//! in holds under that id — the session and the statements run on it — and answers with that
//! id; afterwards the id reads as one nothing holds a session under and is free to create
//! again, while every other session is left as it was. A session the service is itself still
//! moving between statuses (its environment is being brought up, or taken down) has nothing
//! settled for the delete to act on, so it is refused with the modelled
//! `IllegalSessionStateException` naming the status the session is in and the record survives.
//! Because the operation models no entity-miss failure, an id the Region holds no session
//! under is answered the same way as one it does instead of being refused, which makes a
//! repeated delete a no-op.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: which of the modelled
//! `SessionStatus` values the session rests in when the delete lands, how that status was
//! reached (the clock, or a status nothing but a caller reaches), how many statements the
//! session holds, the form and the length of the id, the Region the call is served in, and
//! how a requested id misses.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    messages, ComAmazonawsGlueAwsglueState, SESSION_STATUS_PROVISIONING, SESSION_STATUS_READY,
    SESSION_STATUS_STOPPING, SESSION_STATUS_TIMEOUT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{already_past, far_future};
use crate::statements::{
    create_session, ready_session, reach_due_instant, rewind_session, run, set_session_due,
};
use crate::wire::{call, call_in_region, emulator_on, error_code, error_message, ok, REGION};

/// The longest `Id` the model accepts: `NameString`, "len 1..255"
/// (`smithy_query operation=DeleteSession`).
const MAX_ID_LENGTH: usize = 255;

/// The two modelled `SessionStatus` values a session rests in that nothing in the emulator
/// reaches on its own — a session that failed to come up, and one a stop has taken all the
/// way down (`smithy_query shape=SessionStatus`).
const SESSION_STATUS_FAILED: &str = "FAILED";
const SESSION_STATUS_STOPPED: &str = "STOPPED";

/// Minutes past which a session with a one-minute idle timeout has timed out, so the clock
/// alone carries it to [`SESSION_STATUS_TIMEOUT`].
const IDLE_MINUTES: u64 = 5;

/// Seconds in one minute: both documented session timeouts are stated in minutes.
const SECONDS_PER_MINUTE: u64 = 60;

/// An emulator and the engine behind it, so a suite can reach a stored session's record.
fn harness() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    (router, engine)
}

/// Put the stored session `id` into `status`.
///
/// The way a suite reaches a status no call of this emulator produces yet, so the rule can be
/// asserted over the whole modelled `SessionStatus` enum and not only over the statuses the
/// clock reaches.
fn force_status(engine: &Arc<engine::InMemoryEngine>, id: &str, status: &str) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .session_mut(REGION, id)
        .unwrap_or_else(|| panic!("{id} was created"));
    entry.settles_at = None;
    entry.status = status.to_string();
}

/// Whether the store itself holds a session under `id` in `region`.
fn stored(engine: &Arc<engine::InMemoryEngine>, region: &str, id: &str) -> bool {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    store.session(region, id).is_some()
}

/// Delete the session `id` and return the `Id` the answer reports, if any.
async fn delete(router: &MultiServiceRouter, id: &str) -> Option<String> {
    let deleted = ok(router, "DeleteSession", &json!({ "Id": id })).await;
    deleted
        .get("Id")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Assert that `GetSession` reports `id` as a session this Region does not hold.
async fn assert_missing(router: &MultiServiceRouter, case: &str, id: &str) {
    let (status, body) = call(router, "GetSession", &json!({ "Id": id })).await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400 reading the deleted {id:?}, got {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("EntityNotFoundException"),
        "case {case}: expected EntityNotFoundException for {id:?}, got {} in {body}",
        error_code(&body)
    );
    assert_eq!(
        error_message(&body),
        messages::session_not_found(id),
        "case {case}: the refusal must name the session asked for, got {body}"
    );
}

/// How a row's session is brought to the status the delete lands on.
enum Reach {
    /// The status the clock owed it once its provisioning was due.
    Provisioned,
    /// The status the clock owed it once it had been idle `minutes` past its idle timeout.
    IdleFor { minutes: u64 },
    /// A status only a caller reaches, written straight onto the stored record.
    Forced(&'static str),
}

/// One row of the delete table: the id to create a session under, how it reaches the status
/// the delete lands on, and how many statements were run on it first.
struct Row {
    case: &'static str,
    id: String,
    reach: Reach,
    status: &'static str,
    statements: usize,
}

// ---------------------------------------------------------------------------
// Delete a settled session, then observe what went
// ---------------------------------------------------------------------------

/// A delete removes the whole record the Region holds under the id it addresses and answers
/// with that id: afterwards the session reads as missing, its statements are unreachable, the
/// id is free to create again and holds a FRESH session, and every other session is untouched.
#[tokio::test]
async fn deleting_a_settled_session_removes_the_record_stored_under_its_id() {
    let long_id = "i".repeat(MAX_ID_LENGTH);

    let rows = vec![
        Row {
            // The shape a notebook client sends: create, work, delete.
            case: "a ready session with no statements",
            id: "ready-idle".to_string(),
            reach: Reach::Provisioned,
            status: SESSION_STATUS_READY,
            statements: 0,
        },
        Row {
            case: "a ready session with one statement",
            id: "ready-one-statement".to_string(),
            reach: Reach::Provisioned,
            status: SESSION_STATUS_READY,
            statements: 1,
        },
        Row {
            // More than one statement, so what goes with the record is the whole history.
            case: "a ready session with several statements",
            id: "Mixed.Case-session_42".to_string(),
            reach: Reach::Provisioned,
            status: SESSION_STATUS_READY,
            statements: 3,
        },
        Row {
            // A status the CLOCK reached, not a caller: the session's own timeout fell due.
            case: "a session its own timeout ended",
            id: "timed-out".to_string(),
            reach: Reach::IdleFor {
                minutes: IDLE_MINUTES,
            },
            status: SESSION_STATUS_TIMEOUT,
            statements: 0,
        },
        Row {
            case: "a session a stop took all the way down",
            id: "stopped".to_string(),
            reach: Reach::Forced(SESSION_STATUS_STOPPED),
            status: SESSION_STATUS_STOPPED,
            statements: 0,
        },
        Row {
            case: "a session that failed to come up",
            id: "failed".to_string(),
            reach: Reach::Forced(SESSION_STATUS_FAILED),
            status: SESSION_STATUS_FAILED,
            statements: 0,
        },
        Row {
            case: "the shortest id the model accepts",
            id: "x".to_string(),
            reach: Reach::Provisioned,
            status: SESSION_STATUS_READY,
            statements: 0,
        },
        Row {
            case: "an id at the modelled length bound",
            id: long_id,
            reach: Reach::Provisioned,
            status: SESSION_STATUS_READY,
            statements: 1,
        },
    ];

    for row in &rows {
        let (router, engine) = harness();
        let case = row.case;
        let id = row.id.as_str();

        // A second session nothing in this row addresses: the delete must leave it alone.
        ready_session(&router, &engine, "bystander").await;

        match &row.reach {
            Reach::Provisioned => ready_session(&router, &engine, id).await,
            Reach::IdleFor { minutes } => {
                create_session(&router, id, json!({ "IdleTimeout": 1 })).await;
                rewind_session(&engine, id, Duration::from_secs(minutes * SECONDS_PER_MINUTE));
            }
            Reach::Forced(status) => {
                create_session(&router, id, json!({})).await;
                force_status(&engine, id, status);
            }
        }

        // The statements the session holds when the delete lands, each settled so the next
        // run of the session is taken.
        let mut statement_ids = Vec::new();
        for index in 0..row.statements {
            statement_ids.push(run(&router, id, &format!("{index} + 1")).await);
            reach_due_instant(&engine, id);
        }

        // The status the delete lands on is the one the row means it to.
        let read = ok(&router, "GetSession", &json!({ "Id": id })).await;
        assert_eq!(
            read["Session"]["Status"].as_str(),
            Some(row.status),
            "case {case}: the row's session must be in {}, got {read}",
            row.status
        );

        // "Returns the ID of the deleted session."
        assert_eq!(
            delete(&router, id).await.as_deref(),
            Some(id),
            "case {case}: DeleteSession must report the id it deleted"
        );

        // The record is gone from the store, so every read of it is a miss.
        assert!(
            !stored(&engine, REGION, id),
            "case {case}: the store must hold no session under {id:?} after the delete"
        );
        assert_missing(&router, case, id).await;
        for statement in &statement_ids {
            let (status, body) = call(
                &router,
                "GetStatement",
                &json!({ "SessionId": id, "Id": statement }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case}: a statement of a deleted session must not be readable, got \
                 {status} with {body}"
            );
            assert_eq!(
                error_message(&body),
                messages::session_id_not_found(id),
                "case {case}: the statement read misses on the session itself, got {body}"
            );
        }

        // The id the delete freed can be created again, and what it then holds is a FRESH
        // session: being provisioned once more, with none of the deleted record's statements.
        // The due instant is held far enough ahead that the wall clock cannot cross it while
        // the read is made, since the status turns on that stored instant.
        create_session(&router, id, json!({})).await;
        set_session_due(&engine, id, far_future());
        let reread = ok(&router, "GetSession", &json!({ "Id": id })).await;
        assert_eq!(
            reread["Session"]["Status"].as_str(),
            Some(SESSION_STATUS_PROVISIONING),
            "case {case}: a re-created id holds a fresh session, got {reread}"
        );
        for statement in &statement_ids {
            let (status, body) = call(
                &router,
                "GetStatement",
                &json!({ "SessionId": id, "Id": statement }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case}: the statements of the deleted session must not survive it, got \
                 {status} with {body}"
            );
        }

        // The session the row never addressed still reads back as its own create wrote it.
        let bystander = ok(&router, "GetSession", &json!({ "Id": "bystander" })).await;
        assert_eq!(
            bystander["Session"]["Status"].as_str(),
            Some(SESSION_STATUS_READY),
            "case {case}: the delete must leave every other session alone, got {bystander}"
        );
    }
}

// ---------------------------------------------------------------------------
// A session the service is still moving between statuses
// ---------------------------------------------------------------------------

/// A delete of a session the service is itself still moving between statuses is refused with
/// the modelled `IllegalSessionStateException` naming the status it is in, and the record
/// survives the refusal unchanged — while the same id deletes once it has settled.
#[tokio::test]
async fn deleting_a_session_the_service_is_still_moving_is_refused() {
    // (case, the status the delete lands on, whether the clock settles it on its own)
    let rows: &[(&str, &str, bool)] = &[
        (
            // Where every session starts: "the remote Spark runtime environment" is still
            // being provisioned.
            "a session still being provisioned",
            SESSION_STATUS_PROVISIONING,
            true,
        ),
        (
            "a session still being taken down",
            SESSION_STATUS_STOPPING,
            false,
        ),
    ];

    for (case, status, settles_on_its_own) in rows {
        let (router, engine) = harness();
        let id = "in-flight";
        create_session(&router, id, json!({ "Description": "In flight." })).await;
        if *settles_on_its_own {
            // The status turns on the stored due instant, so a row that means "the clock has
            // not reached it yet" holds that instant far enough ahead that the wall clock
            // cannot cross it while the row's calls are made.
            set_session_due(&engine, id, far_future());
        } else {
            force_status(&engine, id, status);
        }

        let (code, body) = call(&router, "DeleteSession", &json!({ "Id": id })).await;
        assert_eq!(
            code, 400,
            "case {case}: a session in {status} must not be deleted, got {code} with {body}"
        );
        assert!(
            error_code(&body).ends_with("IllegalSessionStateException"),
            "case {case}: expected IllegalSessionStateException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            messages::session_in_illegal_state(status),
            "case {case}: the refusal must name the status the session is in, got {body}"
        );
        assert!(
            body.get("Id").is_none_or(Value::is_null),
            "case {case}: a refused delete reports no deleted id, got {body}"
        );

        // The record survives the refusal exactly as it was.
        assert!(
            stored(&engine, REGION, id),
            "case {case}: a refused delete must leave the session stored"
        );
        let read = ok(&router, "GetSession", &json!({ "Id": id })).await;
        assert_eq!(
            read["Session"]["Status"].as_str(),
            Some(*status),
            "case {case}: the refused session must still be in {status}, got {read}"
        );
        assert_eq!(
            read["Session"]["Description"].as_str(),
            Some("In flight."),
            "case {case}: the stored session must survive the refusal, got {read}"
        );

        // Once the session has settled — the clock finishing its provisioning, or a status
        // nothing but a caller reaches — the very same delete is served.
        if *settles_on_its_own {
            set_session_due(&engine, id, already_past());
        } else {
            force_status(&engine, id, SESSION_STATUS_STOPPED);
        }
        assert_eq!(
            delete(&router, id).await.as_deref(),
            Some(id),
            "case {case}: a settled session must delete"
        );
        assert_missing(&router, case, id).await;
    }
}

// ---------------------------------------------------------------------------
// An id the Region holds no session under
// ---------------------------------------------------------------------------

/// An id this Region holds no session under is not refused — the operation models no
/// entity-miss failure — and changes nothing: the call is served and reports the id it was
/// addressed by. A session is addressed by its id and the Region it was created in together,
/// so a differently-cased or partial id, an id already deleted, and a stored id asked for
/// through another Region's endpoint all hold no session.
#[tokio::test]
async fn deleting_an_id_no_session_is_held_under_is_served_and_changes_nothing() {
    let (router, engine) = harness();
    ready_session(&router, &engine, "StoredSession").await;
    // A session that IS deleted, so the second delete of it holds no session either.
    ready_session(&router, &engine, "already-deleted").await;
    assert_eq!(
        delete(&router, "already-deleted").await.as_deref(),
        Some("already-deleted"),
        "the first delete of a stored session reports its id"
    );

    let longest = "i".repeat(MAX_ID_LENGTH);
    let misses: Vec<(&str, &str)> = vec![
        ("never created", "no-such-session"),
        ("already deleted, so deleted twice", "already-deleted"),
        ("a prefix of a stored id", "Stored"),
        ("a suffix of a stored id", "Session"),
        ("a stored id lowercased", "storedsession"),
        ("a stored id uppercased", "STOREDSESSION"),
        ("an id carrying dots, dashes and underscores", "a.b-c_d"),
        ("the shortest id the model accepts", "x"),
        ("an id at the longest the model accepts", &longest),
    ];

    for (case, id) in &misses {
        // Served, not refused — and it reports the id it was addressed by, since the answer's
        // only member names the session the delete was for.
        assert_eq!(
            delete(&router, id).await.as_deref(),
            Some(*id),
            "case {case}: a delete of {id:?} must be served and report that id"
        );
        assert!(
            !stored(&engine, REGION, id),
            "case {case}: {id:?} must still hold no session"
        );
        // Nothing else went with it: the one stored session is still readable.
        let read = ok(&router, "GetSession", &json!({ "Id": "StoredSession" })).await;
        assert_eq!(
            read["Session"]["Status"].as_str(),
            Some(SESSION_STATUS_READY),
            "case {case}: a delete that removed nothing must leave the stored session \
             untouched, got {read}"
        );
    }

    // A session is served only through the endpoint that created it, so another Region's
    // endpoint holds no session under a stored id — and the record it names stays where it is.
    let (status, body) = call_in_region(
        &router,
        "eu-west-1",
        "DeleteSession",
        &json!({ "Id": "StoredSession" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case a stored id in another Region: expected the call to be served, got {status} \
         with {body}"
    );
    assert!(
        stored(&engine, REGION, "StoredSession"),
        "case a stored id in another Region: the session must stay where it was created"
    );

    // The stored id itself deletes, so the ids above held no session and the lookup is not
    // simply broken.
    assert_eq!(
        delete(&router, "StoredSession").await.as_deref(),
        Some("StoredSession"),
        "case the stored id: the delete reports the id it removed"
    );
    assert_missing(&router, "the stored id", "StoredSession").await;
}
