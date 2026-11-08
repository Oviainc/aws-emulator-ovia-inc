// SPDX-License-Identifier: CC0-1.0

//! `DeleteMultiplex`: tearing a stored multiplex down, end to end over the
//! restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! RULE: "Delete a multiplex. The multiplex must be idle."
//! (`smithy_query(query="operation", operation="DeleteMultiplex")`) — a delete
//! is accepted only on a multiplex in the stopped state and then answers 202
//! with that stored multiplex now `DELETING`, which the clock settles into the
//! terminal `DELETED` the record keeps; a delete sent in any other state is
//! refused with the modelled 409 `ConflictException` and writes nothing, and a
//! `MultiplexId` the account does not hold is the modelled 404
//! `NotFoundException`. Once a delete has been accepted for an id, every
//! mutation of that id is refused on the multiplex's STATE — a 409
//! `ConflictException`, not a 404 — because the record is still the account's.
//!
//! The suites are tables: the recorded case (a delete refused on a `RUNNING`
//! multiplex holding no program) is ONE row of them. The WORDING the state
//! refusal carries is `crate::multiplex_delete_refusal_message`'s subject.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_lifecycle::{create_body, created, describe_multiplex, settings, MULTIPLEXES};
use crate::multiplex_programs::program_settings;
use crate::multiplex_start::{add_programs, start_multiplex};
use crate::multiplex_stop::stop_multiplex;
use crate::wire::{call, error_code, error_message, medialive};

/// The state a delete needs, as the model spells it
/// (`smithy_query(query="shape", shape="MultiplexState")`).
const IDLE: &str = "IDLE";

/// `DeleteMultiplex`'s binding (`smithy_query(query="operation",
/// operation="DeleteMultiplex")`: `DELETE /prod/multiplexes/{MultiplexId}`,
/// success 202).
///
/// Shared with [`crate::idempotency_token_after_terminal_delete`], whose rows
/// also have to take a multiplex to its terminal state, so the two suites send
/// the same delete.
pub(crate) async fn delete_multiplex(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "DELETE", &format!("{MULTIPLEXES}/{id}"), None).await
}

/// A multiplex created under `name`, with `multiplex_settings` and `tags`, as
/// its minted id.
async fn multiplex_with(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    multiplex_settings: Value,
    tags: Option<Value>,
) -> String {
    let mut body = create_body(name, &format!("token-{name}"), multiplex_settings);
    if let Some(tags) = tags {
        body["tags"] = tags;
    }
    let multiplex = created(router, case, body).await;
    multiplex["id"].as_str().unwrap_or_default().to_string()
}

/// The multiplex `id`, read back through `DescribeMultiplex` with the read
/// asserted to have succeeded and to report the state the row expects.
async fn in_state(router: &MultiServiceRouter, case: &str, id: &str, state: &str) -> Value {
    let (status, described) = describe_multiplex(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeMultiplex: expected 200, got {status}: {described}"
    );
    assert_eq!(
        described["state"],
        json!(state),
        "case {case:?}: the test did not reach {state}, it reached {:?}",
        described["state"]
    );
    described
}

/// One request that MUTATES the multiplex under test, built from its id: the
/// operation as the model names it, and the request a client sends.
struct Mutation {
    op: &'static str,
    request: fn(&str) -> (&'static str, String, Option<Value>),
}

/// The mutations of a multiplex whose documentation makes the multiplex's state
/// a PRECONDITION, so the state guard is asserted over the class and not over
/// one operation: the start, the delete itself, and the create of a program
/// inside the multiplex. `StopMultiplex` is deliberately not one of them — its
/// own documentation makes it a no-op outside the running state ("If the
/// multiplex isn't running, this action has no effect"), which the stop suite
/// owns.
fn mutations() -> [Mutation; 3] {
    [
        Mutation {
            op: "StartMultiplex",
            request: |id| ("POST", format!("{MULTIPLEXES}/{id}/start"), None),
        },
        Mutation {
            op: "DeleteMultiplex",
            request: |id| ("DELETE", format!("{MULTIPLEXES}/{id}"), None),
        },
        Mutation {
            op: "CreateMultiplexProgram",
            request: |id| {
                (
                    "POST",
                    format!("{MULTIPLEXES}/{id}/programs"),
                    Some(json!({
                        "programName": "after-the-delete",
                        "requestId": "token-after-the-delete",
                        "multiplexProgramSettings": program_settings(9),
                    })),
                )
            },
        },
    ]
}

/// Rule: a delete of a stopped multiplex is accepted whatever that multiplex
/// holds — it answers 202 with the stored multiplex now `DELETING`, nothing else
/// about it moved, and the clock settles it into the terminal `DELETED` the
/// record keeps, where the reader goes on reporting it.
///
/// The rows differ along what the rule generalises over: how many programs the
/// multiplex holds (a delete reads its state, not the shape of what it carries),
/// the `MultiplexSettings` values, and whether it carries tags.
#[tokio::test]
async fn a_stopped_multiplex_is_deleted_whatever_it_holds() {
    // (case, multiplex name, program count, multiplex settings, tags)
    let rows: [(&str, &str, i64, Value, Option<Value>); 5] = [
        ("no programs at all", "empty", 0, settings(10_000_000, 1), None),
        ("one program", "single", 1, settings(1_000_000, 0), None),
        ("three programs", "trio", 3, settings(100_000_000, 65_535), None),
        (
            "tagged",
            "tagged",
            1,
            settings(2_000_000, 7),
            Some(json!({ "env": "test", "team": "playout" })),
        ),
        (
            "every optional setting",
            "buffered",
            2,
            json!({
                "transportStreamBitrate": 5_000_000,
                "transportStreamId": 42,
                "maximumVideoBufferDelayMilliseconds": 1_000,
                "transportStreamReservedBitrate": 100_000,
            }),
            None,
        ),
    ];

    for (case, name, programs, multiplex_settings, tags) in rows {
        let router = medialive();
        let id = multiplex_with(&router, case, name, multiplex_settings, tags).await;
        add_programs(&router, case, &id, programs).await;
        // A multiplex is deleted from the stopped state, so the create's own
        // transition is let come due first.
        wait_for_the_clock();
        let idle = in_state(&router, case, &id, IDLE).await;
        assert_eq!(
            idle["programCount"],
            json!(programs),
            "case {case:?}: expected the programs the multiplex holds, got {:?}",
            idle["programCount"]
        );

        let (status, deleting) = delete_multiplex(&router, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: DeleteMultiplex: expected 202, got {status}: {deleting}"
        );
        // The delete moved the state and nothing else: every other member is the
        // stored multiplex's, read back from the store rather than from this
        // request.
        let mut expected = idle.clone();
        expected["state"] = json!("DELETING");
        assert_eq!(
            deleting, expected,
            "case {case:?}: expected only the state to have moved, got {deleting}"
        );

        // The reader keeps reporting the multiplex while the delete settles...
        let described = in_state(&router, case, &id, "DELETING").await;
        assert_eq!(
            described, expected,
            "case {case:?}: expected DescribeMultiplex to report the deleting multiplex, got {described}"
        );

        // ... and in the terminal state it then keeps, which is what the SDK's
        // `MultiplexDeleted` waiter polls for.
        wait_for_the_clock();
        let deleted = in_state(&router, case, &id, "DELETED").await;
        let mut expected = idle.clone();
        expected["state"] = json!("DELETED");
        assert_eq!(
            deleted, expected,
            "case {case:?}: expected only the state to have moved, got {deleted}"
        );
    }
}

/// Rule: a multiplex is deleted from the stopped state only, so a delete sent
/// while it is in any other state is refused with a 409 `ConflictException`
/// carrying the fixed sentence of that rejection class — which names the state
/// the multiplex is in and nothing the caller supplied — and moves nothing: a
/// refused delete is never a no-op.
///
/// The rows span every state this service can put a multiplex in besides the
/// stopped one, the recorded `RUNNING` case among them.
#[tokio::test]
async fn a_multiplex_is_deleted_only_from_the_stopped_state() {
    // (case, the state the multiplex is in when the delete is sent)
    let rows: [(&str, &str); 6] = [
        ("while still creating", "CREATING"),
        ("while starting", "STARTING"),
        ("while running", "RUNNING"),
        ("while stopping", "STOPPING"),
        ("while already deleting", "DELETING"),
        ("once deleted", "DELETED"),
    ];

    for (case, state) in rows {
        let router = medialive();
        let id = multiplex_with(&router, case, "stateful", settings(10_000_000, 1), None).await;
        // A program can be created in a multiplex that is still coming up, so
        // every row carries one: the state is what the row varies.
        add_programs(&router, case, &id, 1).await;
        reach(&router, case, &id, state).await;

        let observed = in_state(&router, case, &id, state).await;
        let (status, body) = delete_multiplex(&router, &id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected deleting a {state} multiplex to conflict, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            crate::multiplex_delete_refusal_message::not_deletable(state),
            "case {case:?}: expected the fixed sentence of the class naming the state that \
             refused the delete, got {body}"
        );

        // The refused delete wrote nothing: the multiplex reads back exactly as
        // it did an instant before it.
        let (status, after) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex after the refusal: expected 200, got {status}: {after}"
        );
        assert_eq!(
            after, observed,
            "case {case:?}: expected the refused delete to leave the multiplex alone, got {after}"
        );
    }
}

/// Rule: the state is the ONLY thing a delete is judged on, so the very same
/// request that a running multiplex refuses is accepted once a stop has settled
/// that multiplex back into the stopped state — nothing about the request, the
/// multiplex or what it holds changes in between.
///
/// The rows differ along the program count and the settings values, so the
/// refuse-then-accept pair is asserted over the class rather than over the one
/// recorded multiplex.
#[tokio::test]
async fn the_same_delete_a_running_multiplex_refuses_is_accepted_once_it_has_stopped() {
    // (case, multiplex name, program count, multiplex settings)
    let rows: [(&str, &str, i64, Value); 3] = [
        ("no programs at all", "empty", 0, settings(10_000_000, 1)),
        ("one program", "single", 1, settings(1_000_000, 0)),
        ("three programs", "trio", 3, settings(100_000_000, 65_535)),
    ];

    for (case, name, programs, multiplex_settings) in rows {
        let router = medialive();
        let id = multiplex_with(&router, case, name, multiplex_settings, None).await;
        add_programs(&router, case, &id, programs).await;
        wait_for_the_clock();
        let (status, started) = start_multiplex(&router, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: StartMultiplex: expected 202, got {status}: {started}"
        );
        wait_for_the_clock();
        let running = in_state(&router, case, &id, "RUNNING").await;

        let (status, refusal) = delete_multiplex(&router, &id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected deleting a RUNNING multiplex to conflict, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refusal}"
        );

        let (status, stopping) = stop_multiplex(&router, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: StopMultiplex: expected 202, got {status}: {stopping}"
        );
        wait_for_the_clock();
        let idle = in_state(&router, case, &id, IDLE).await;

        let (status, deleting) = delete_multiplex(&router, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the identical DeleteMultiplex on the stopped multiplex: expected 202, got {status}: {deleting}"
        );
        // The accepted delete answers the multiplex the refused one had read:
        // only `State` and the pipeline count the running state derived moved.
        let mut expected = running.clone();
        expected["state"] = json!("DELETING");
        expected["pipelinesRunningCount"] = idle["pipelinesRunningCount"].clone();
        assert_eq!(
            deleting, expected,
            "case {case:?}: expected the stored multiplex with only its state moved, got {deleting}"
        );
    }
}

/// Rule: once a delete has been accepted for a multiplex id, that id names no
/// multiplex a caller may still change: every mutation of it is refused on the
/// multiplex's STATE with a 409 `ConflictException` — not a 404 — both inside the
/// transient `DELETING` window and in the terminal `DELETED` state, and the
/// record itself stays readable and unchanged.
///
/// The rows are every mutation of a multiplex this service implements, each sent
/// in both states a delete leaves behind.
#[tokio::test]
async fn a_mutation_of_a_multiplex_whose_delete_was_accepted_conflicts() {
    let case = "accepted delete";

    // The control: every one of these requests is one the LIVE, stopped
    // multiplex accepts, so the refusals below are about the accepted delete and
    // not about a request the service would have refused anyway. Each control
    // runs on a store of its own, because each of them moves the multiplex.
    for mutation in mutations() {
        let router = medialive();
        let id = multiplex_with(&router, case, "live", settings(10_000_000, 3), None).await;
        add_programs(&router, case, &id, 1).await;
        wait_for_the_clock();
        let (method, uri, body) = (mutation.request)(&id);
        let (status, answer) = call(&router, method, &uri, body).await;
        assert!(
            (200..300).contains(&status),
            "case {case:?}/{}: expected the live multiplex to accept the request, got {status}: {answer}",
            mutation.op
        );
    }

    let router = medialive();
    let id = multiplex_with(&router, case, "doomed", settings(10_000_000, 3), None).await;
    add_programs(&router, case, &id, 1).await;
    wait_for_the_clock();
    let before = in_state(&router, case, &id, IDLE).await;

    let (status, deleting) = delete_multiplex(&router, &id).await;
    assert_eq!(
        status, 202,
        "case {case:?}: DeleteMultiplex: expected 202, got {status}: {deleting}"
    );

    for phase in ["DELETING", "DELETED"] {
        if phase == "DELETED" {
            wait_for_the_clock();
        }
        let observed = in_state(&router, case, &id, phase).await;
        for mutation in mutations() {
            let (method, uri, body) = (mutation.request)(&id);
            let (status, refusal) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, 409,
                "case {case:?}/{}: expected 409 while the multiplex is {phase}, got {status}: {refusal}",
                mutation.op
            );
            assert_eq!(
                error_code(status, &refusal),
                "ConflictException",
                "case {case:?}/{}: expected the modelled ConflictException while the multiplex is {phase}, got {refusal}",
                mutation.op
            );
            // The start and the program create name the multiplex they refused;
            // the DELETE answers the fixed sentence of its own rejection class,
            // which names the state that refused it and nothing the caller
            // supplied (`crate::multiplex_delete_refusal_message`).
            let message = error_message(&refusal);
            if mutation.op == "DeleteMultiplex" {
                assert_eq!(
                    message,
                    crate::multiplex_delete_refusal_message::not_deletable(phase),
                    "case {case:?}/{}: expected the class's sentence for {phase}, got {message:?}",
                    mutation.op
                );
            } else {
                assert!(
                    message.contains(&id),
                    "case {case:?}/{}: expected the refusal to name the multiplex it refused, got {message:?}",
                    mutation.op
                );
            }
        }

        // Every refusal above wrote nothing: the record reads back as it did
        // before them, and its programs are still the ones it held.
        let (status, after) = describe_multiplex(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeMultiplex while {phase}: expected 200, got {status}: {after}"
        );
        assert_eq!(
            after, observed,
            "case {case:?}: expected the refused mutations to leave the {phase} multiplex alone, got {after}"
        );
        let mut expected = before.clone();
        expected["state"] = json!(phase);
        assert_eq!(
            after, expected,
            "case {case:?}: expected only the state to have moved while {phase}, got {after}"
        );
    }
}

/// Rule: a delete deletes the multiplex its `MultiplexId` names — a
/// `MultiplexId` no multiplex of the account has is a 404 `NotFoundException`
/// and deletes nothing, including the id of another MediaLive resource, and
/// deleting one multiplex leaves every other multiplex of the account where it
/// was.
#[tokio::test]
async fn a_delete_touches_only_the_multiplex_it_names() {
    let router = medialive();
    let case = "neighbours";
    let doomed = multiplex_with(&router, case, "doomed", settings(10_000_000, 1), None).await;
    let untouched = multiplex_with(&router, case, "untouched", settings(3_000_000, 2), None).await;
    add_programs(&router, case, &untouched, 1).await;
    wait_for_the_clock();

    // A channel exists too, so a request that passes ITS id to the multiplex
    // delete is refused rather than answered out of the wrong store.
    let (status, response) = call(
        &router,
        "POST",
        "/prod/channels",
        Some(json!({
            "name": "not-a-multiplex",
            "encoderSettings": crate::support::channel_encoder_settings(),
        })),
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

    for (row, id) in [
        ("never minted", "9999999".to_string()),
        ("a channel's id", channel_id.clone()),
        ("not a number at all", "no-such-multiplex".to_string()),
    ] {
        let (status, body) = delete_multiplex(&router, &id).await;
        assert_eq!(
            status, 404,
            "case {row:?}: expected 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {row:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Multiplex {id} not found"),
            "case {row:?}: expected the refusal to name the id asked for, got {body}"
        );
    }

    // The channel the refused delete named is still the account's, untouched.
    let (status, channel) = call(&router, "GET", &format!("/prod/channels/{channel_id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeChannel: expected 200, got {status}: {channel}"
    );
    assert!(
        matches!(channel["state"].as_str(), Some("CREATING" | "IDLE")),
        "expected the channel a multiplex delete named to be left alone, got {:?}",
        channel["state"]
    );

    let neighbour = in_state(&router, case, &untouched, IDLE).await;
    let (status, body) = delete_multiplex(&router, &doomed).await;
    assert_eq!(
        status, 202,
        "DeleteMultiplex: expected 202, got {status}: {body}"
    );
    assert_eq!(
        in_state(&router, case, &untouched, IDLE).await,
        neighbour,
        "expected the multiplex the delete did not name to be left alone"
    );
}

/// Drive the multiplex `id` into `state`, through the operations a client uses,
/// asserting each step was accepted.
///
/// Shared with [`crate::multiplex_delete_refusal_message`], whose rows need a
/// multiplex in each state a delete refuses, so the two suites reach a state the
/// same way.
pub(crate) async fn reach(router: &MultiServiceRouter, case: &str, id: &str, state: &str) {
    if state == "CREATING" {
        return;
    }
    // Every other state is reached from the stopped one the create settles into.
    wait_for_the_clock();
    match state {
        "STARTING" | "RUNNING" | "STOPPING" => {
            let (status, started) = start_multiplex(router, id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: StartMultiplex: expected 202, got {status}: {started}"
            );
            if state != "STARTING" {
                wait_for_the_clock();
            }
            // A multiplex stops from the running state, so `STOPPING` is one
            // step past `RUNNING`.
            if state == "STOPPING" {
                let (status, stopping) = stop_multiplex(router, id).await;
                assert_eq!(
                    status, 202,
                    "case {case:?}: StopMultiplex: expected 202, got {status}: {stopping}"
                );
            }
        }
        "DELETING" | "DELETED" => {
            let (status, deleting) = delete_multiplex(router, id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: DeleteMultiplex: expected 202, got {status}: {deleting}"
            );
            if state == "DELETED" {
                wait_for_the_clock();
            }
        }
        other => panic!("case {case:?}: no step reaches {other}"),
    }
}
