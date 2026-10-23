// SPDX-License-Identifier: CC0-1.0

//! `StopMultiplex`: stopping a running multiplex, end to end over the restJson1
//! wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables: the recorded case (a stop of the multiplex a recorded
//! start-stop lifecycle had just started) is ONE row of them. The documented
//! rule they assert is `StopMultiplex`'s own — "Stops a running multiplex. If
//! the multiplex isn't running, this action has no effect."

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_lifecycle::{describe_multiplex, settings, MULTIPLEXES};
use crate::multiplex_start::{add_programs, multiplex_with_settings, start_multiplex, ZONES};
use crate::wire::{call, error_code, error_message, medialive};

/// `StopMultiplex`'s binding (`smithy_query(query="operation",
/// operation="StopMultiplex")`: `POST /prod/multiplexes/{MultiplexId}/stop`,
/// success 202).
async fn stop_multiplex(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "POST", &format!("{MULTIPLEXES}/{id}/stop"), None).await
}

/// The members of a multiplex description the clock may move between two reads —
/// its lifecycle `State` and the pipeline count derived from it. A row that
/// cannot pin the state deterministically compares everything BUT these.
const CLOCK_MOVED: [&str; 2] = ["state", "pipelinesRunningCount"];

/// `body` without the members the clock may have moved, so two reads taken at
/// different instants can be compared on everything the stop must not touch.
fn without_clock_moved(body: &Value) -> Value {
    let mut stripped = body.clone();
    if let Some(object) = stripped.as_object_mut() {
        for member in CLOCK_MOVED {
            object.remove(member);
        }
    }
    stripped
}

/// A multiplex of `multiplex_settings` holding `programs` programs, started and
/// settled into `RUNNING`, as its minted id and the description it reports
/// there.
async fn running_multiplex(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    programs: i64,
    multiplex_settings: Value,
) -> (String, Value) {
    let id = multiplex_with_settings(router, case, name, multiplex_settings).await;
    add_programs(router, case, &id, programs).await;
    // A multiplex is started from the stopped state, so the create's own
    // transition is let come due first.
    wait_for_the_clock();
    let (status, started) = start_multiplex(router, &id).await;
    assert_eq!(
        status, 202,
        "case {case:?}: StartMultiplex: expected 202, got {status}: {started}"
    );
    wait_for_the_clock();
    let (status, running) = describe_multiplex(router, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeMultiplex: expected 200, got {status}: {running}"
    );
    assert_eq!(
        running["state"],
        json!("RUNNING"),
        "case {case:?}: the test did not reach RUNNING, it reached {:?}",
        running["state"]
    );
    (id, running)
}

/// Rule: `StopMultiplex` stops a running multiplex — it answers 202 with that
/// multiplex now `STOPPING` and no pipeline healthy, and the clock settles it
/// back into the stopped `IDLE`; nothing else about the stored multiplex moves,
/// and the reader reports the same transition.
///
/// What the multiplex HOLDS is not a precondition, so the rows differ along the
/// program count, from none upwards (the recorded stop is of a multiplex holding
/// none), and along the `MultiplexSettings` values: a stop reads the multiplex's
/// state, not the shape of what it carries.
#[tokio::test]
async fn a_running_multiplex_stops_whatever_it_holds() {
    // (case, multiplex name, program count, multiplex settings)
    let rows: [(&str, &str, i64, Value); 5] = [
        ("no programs at all", "empty", 0, settings(2_000_000, 1)),
        ("one program", "single", 1, settings(1_000_000, 0)),
        ("two programs", "pair", 2, settings(20_000_000, 42)),
        ("three programs", "trio", 3, settings(100_000_000, 65_535)),
        (
            "every optional setting",
            "buffered",
            1,
            json!({
                "transportStreamBitrate": 5_000_000,
                "transportStreamId": 7,
                "maximumVideoBufferDelayMilliseconds": 1_000,
                "transportStreamReservedBitrate": 100_000,
            }),
        ),
    ];

    for (case, name, programs, multiplex_settings) in rows {
        let router = medialive();
        let (id, running) = running_multiplex(&router, case, name, programs, multiplex_settings).await;
        assert_eq!(
            running["pipelinesRunningCount"],
            json!(ZONES),
            "case {case:?}: expected one healthy pipeline per availability zone while RUNNING, got {:?}",
            running["pipelinesRunningCount"]
        );
        assert_eq!(
            running["programCount"],
            json!(programs),
            "case {case:?}: expected the programs the multiplex holds, got {:?}",
            running["programCount"]
        );

        let (status, stopping) = stop_multiplex(&router, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: StopMultiplex: expected 202, got {status}: {stopping}"
        );
        assert_eq!(
            stopping["state"],
            json!("STOPPING"),
            "case {case:?}: expected StopMultiplex to answer STOPPING, got {:?}",
            stopping["state"]
        );
        assert_eq!(
            stopping["pipelinesRunningCount"],
            json!(0),
            "case {case:?}: expected no pipeline healthy once the multiplex is no longer running, got {:?}",
            stopping["pipelinesRunningCount"]
        );
        // The stop moved the state (and the pipeline count derived from it) and
        // nothing else: every other member is the stored multiplex's, read back
        // from the store rather than from this request.
        let mut expected = running.clone();
        expected["state"] = json!("STOPPING");
        expected["pipelinesRunningCount"] = json!(0);
        assert_eq!(
            stopping, expected,
            "case {case:?}: expected only the state and the pipeline count to have moved, got {stopping}"
        );

        // The reader reports the same transition, so the SDK's waiter sees it.
        let (status, described) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex after the stop: expected 200, got {status}: {described}"
        );
        assert!(
            matches!(described["state"].as_str(), Some("STOPPING" | "IDLE")),
            "case {case:?}: expected a stopping multiplex to be reported stopping or stopped, got {:?}",
            described["state"]
        );

        // The transition the clock owes: `STOPPING` settles back to `IDLE`,
        // where nothing runs.
        wait_for_the_clock();
        let (status, stopped) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex after the clock: expected 200, got {status}: {stopped}"
        );
        assert_eq!(
            stopped, expected_stopped(&running),
            "case {case:?}: expected a stopped multiplex IDLE with no pipeline healthy and nothing else moved, got {stopped}"
        );
    }
}

/// The description a multiplex that has finished stopping must report: the one
/// it reported while running, with the lifecycle state back at `IDLE` and no
/// pipeline healthy.
fn expected_stopped(running: &Value) -> Value {
    let mut expected = running.clone();
    expected["state"] = json!("IDLE");
    expected["pipelinesRunningCount"] = json!(0);
    expected
}

/// Rule: a stop of a multiplex that is NOT running has no effect — it answers
/// 202 with the multiplex exactly as it stands, starts no `STOPPING` transition
/// and does not disturb the transition the clock already owed it, so the
/// multiplex goes on to the state it was already heading for.
///
/// The rows span every non-running state this service can put a multiplex in:
/// one still coming up, one still starting, one already stopped and one already
/// stopping (the repeat stop, which is what makes a stop idempotent).
#[tokio::test]
async fn stopping_a_multiplex_that_is_not_running_has_no_effect() {
    // (case, the state the multiplex is in when the stop is sent, the states
    // that stop may report — the state itself, plus the one the clock may
    // already have settled it into — and the state the clock settles it into)
    let rows: [(&str, &str, &[&str], &str); 4] = [
        ("while still creating", "CREATING", &["CREATING", "IDLE"], "IDLE"),
        ("while still starting", "STARTING", &["STARTING", "RUNNING"], "RUNNING"),
        ("while already stopped", "IDLE", &["IDLE"], "IDLE"),
        ("while already stopping", "STOPPING", &["STOPPING", "IDLE"], "IDLE"),
    ];

    for (case, state, reported, settled) in rows {
        let router = medialive();
        let id = multiplex_with_settings(&router, case, "stateful", settings(2_000_000, 1)).await;
        // A program can be created in a multiplex that is still coming up, so
        // every row carries one: the state is what the row varies.
        add_programs(&router, case, &id, 1).await;
        if state != "CREATING" {
            wait_for_the_clock();
        }
        if state == "STARTING" || state == "STOPPING" {
            let (status, started) = start_multiplex(&router, &id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: StartMultiplex: expected 202, got {status}: {started}"
            );
        }
        if state == "STOPPING" {
            // The effective stop: the one under test is the SECOND one, sent
            // while this transition is still owed.
            wait_for_the_clock();
            let (status, first) = stop_multiplex(&router, &id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: the first StopMultiplex: expected 202, got {status}: {first}"
            );
            assert_eq!(
                first["state"],
                json!("STOPPING"),
                "case {case:?}: the test did not reach STOPPING, it reached {:?}",
                first["state"]
            );
        }

        let (status, before) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex: expected 200, got {status}: {before}"
        );

        let (status, body) = stop_multiplex(&router, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected stopping a {state} multiplex to be accepted as a no-op, got {status}: {body}"
        );
        let answered = body["state"].as_str().unwrap_or_default();
        assert!(
            reported.contains(&answered),
            "case {case:?}: expected the stop of a {state} multiplex to report one of {reported:?}, got {answered:?}"
        );
        // It wrote nothing: every member but the ones the clock moves is the
        // stored multiplex's, as the read taken just before the stop saw it.
        assert_eq!(
            without_clock_moved(&body),
            without_clock_moved(&before),
            "case {case:?}: expected the no-op stop to leave the stored multiplex alone, got {body}"
        );

        // And it did not disturb what the clock already owed: the multiplex
        // reaches the state it was heading for, not one a stop would have
        // forced on it.
        wait_for_the_clock();
        let (status, after) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex after the clock: expected 200, got {status}: {after}"
        );
        assert_eq!(
            after["state"],
            json!(settled),
            "case {case:?}: expected the multiplex to settle into {settled}, got {:?}",
            after["state"]
        );
    }
}

/// Rule: a stop acts on the multiplex its `MultiplexId` names — a `MultiplexId`
/// no multiplex of the account has is a 404 `NotFoundException` and stops
/// nothing, including the id of another MediaLive resource, and stopping one
/// multiplex leaves every other multiplex of the account where it was.
#[tokio::test]
async fn a_stop_touches_only_the_multiplex_it_names() {
    let router = medialive();
    let (stopped, _) = running_multiplex(&router, "neighbours", "stopped", 0, settings(2_000_000, 1)).await;
    let (untouched, running) =
        running_multiplex(&router, "neighbours", "untouched", 1, settings(3_000_000, 2)).await;

    // A channel exists too, so a stop that passes ITS id is refused rather than
    // served out of the wrong store.
    let (status, response) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({ "name": "not-a-multiplex" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateChannel: expected 201, got {status}: {response}"
    );
    let channel_id = response["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    for (case, id) in [
        ("never minted", "9999999".to_string()),
        ("a channel's id", channel_id),
        ("not a number at all", "no-such-multiplex".to_string()),
    ] {
        let (status, body) = stop_multiplex(&router, &id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Multiplex {id} not found"),
            "case {case:?}: expected the refusal to name the id asked for, got {body}"
        );
    }

    let (status, body) = stop_multiplex(&router, &stopped).await;
    assert_eq!(
        status, 202,
        "StopMultiplex: expected 202, got {status}: {body}"
    );
    let (status, neighbour) = describe_multiplex(&router, &untouched).await;
    assert_eq!(
        status, 200,
        "DescribeMultiplex: expected 200, got {status}: {neighbour}"
    );
    assert_eq!(
        neighbour, running,
        "expected the multiplex the stop did not name to be left running, got {neighbour}"
    );
}
