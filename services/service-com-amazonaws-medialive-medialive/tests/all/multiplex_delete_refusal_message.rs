// SPDX-License-Identifier: CC0-1.0

//! The `Message` a multiplex delete refused on the multiplex's STATE carries,
//! end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! RULE: "Delete a multiplex. The multiplex must be idle."
//! (`smithy_query(query="operation", operation="DeleteMultiplex")`). A delete
//! sent while the multiplex is in any other state is refused with the modelled
//! `ConflictException` (errorKind client, httpError 409, single optional
//! `Message` member, `smithy_query(query="shape", shape="ConflictException")`)
//! carrying the FIXED sentence of that rejection class — "Cannot delete a
//! multiplex that is in <MultiplexState> state" — whose one variable part is the
//! state the multiplex is in, read out of the record the delete was judged
//! against. Nothing the caller supplied is in it: not the `MultiplexId` the
//! request named, not the multiplex's name and not its ARN, so a client that
//! classifies the conflict by its text sees one sentence per state class rather
//! than one per request.
//!
//! The suites are tables over the CLASS: the recorded case (a delete refused on
//! a `RUNNING` multiplex) is ONE row, and the rows differ along what the rule
//! generalises over — which state refused the delete, which entry point asked,
//! and what the multiplex is called, carries and holds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_deletion::{delete_multiplex, reach};
use crate::multiplex_lifecycle::{create_body, created, describe_multiplex, settings, MULTIPLEXES};
use crate::multiplex_start::add_programs;
use crate::wire::{call, error_code, error_message, medialive};

/// The sentence MediaLive refuses a multiplex delete with while the multiplex is
/// in `state`, written out here rather than read from the crate so the test pins
/// the text instead of repeating the implementation.
///
/// Shared with [`crate::multiplex_deletion`], whose own state table asserts the
/// same wording from the same entry point.
pub(crate) fn not_deletable(state: &str) -> String {
    format!("Cannot delete a multiplex that is in {state} state")
}

/// The URI `BatchDelete` is bound to (`smithy_query(query="operation",
/// operation="BatchDelete")`: POST /prod/batch/delete).
const BATCH_DELETE: &str = "/prod/batch/delete";

/// The `MultiplexState` the delete the model documents accepts
/// (`smithy_query(query="shape", shape="MultiplexState")`).
const IDLE: &str = "IDLE";

/// The states a delete is refused in: every value `MultiplexState` models that
/// this service can put a multiplex in, besides the one state a delete accepts.
/// `CREATE_FAILED` and `RECOVERING` are the enum's other two values and no
/// operation of this emulator reaches either, so no request can be sent to a
/// multiplex in them.
const REFUSING_STATES: [&str; 6] = [
    "CREATING",
    "STARTING",
    "RUNNING",
    "STOPPING",
    "DELETING",
    "DELETED",
];

/// One multiplex shape a row builds: what it is called, the `MultiplexSettings`
/// it carries, whether it carries tags and how many programs it holds — the
/// things the refusal's wording must not depend on.
struct Shape {
    name: &'static str,
    settings: Value,
    tags: Option<Value>,
    programs: i64,
}

/// The shapes the rows cycle through, so each state's refusal is asserted on a
/// multiplex that differs in everything but its state.
fn shapes() -> [Shape; 3] {
    [
        Shape {
            name: "plain",
            settings: settings(10_000_000, 1),
            tags: None,
            programs: 0,
        },
        Shape {
            name: "多重化",
            settings: json!({
                "transportStreamBitrate": 5_000_000,
                "transportStreamId": 42,
                "maximumVideoBufferDelayMilliseconds": 1_000,
                "transportStreamReservedBitrate": 100_000,
            }),
            tags: Some(json!({ "env": "test", "team": "playout" })),
            programs: 1,
        },
        Shape {
            name: "crowded",
            settings: settings(100_000_000, 65_535),
            tags: None,
            programs: 3,
        },
    ]
}

/// A multiplex built to `shape`, as `(id, arn)`.
async fn multiplex_of(router: &MultiServiceRouter, case: &str, shape: &Shape) -> (String, String) {
    let mut body = create_body(
        shape.name,
        &format!("token-{}", shape.name),
        shape.settings.clone(),
    );
    if let Some(tags) = shape.tags.clone() {
        body["tags"] = tags;
    }
    let multiplex = created(router, case, body).await;
    let id = multiplex["id"].as_str().unwrap_or_default().to_string();
    let arn = multiplex["arn"].as_str().unwrap_or_default().to_string();
    add_programs(router, case, &id, shape.programs).await;
    (id, arn)
}

/// The multiplex `id`, read back through `DescribeMultiplex`, asserted to still
/// report `state`.
async fn still_in(router: &MultiServiceRouter, case: &str, id: &str, state: &str) -> Value {
    let (status, described) = describe_multiplex(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeMultiplex: expected 200, got {status}: {described}"
    );
    assert_eq!(
        described["state"],
        json!(state),
        "case {case:?}: expected the multiplex in {state}, got {:?}",
        described["state"]
    );
    described
}

/// Assert `message` is the class's sentence for `state` and carries nothing the
/// caller named.
fn assert_class_message(case: &str, message: &str, state: &str, named: &[&str]) {
    assert_eq!(
        message,
        not_deletable(state),
        "case {case:?}: expected {:?}, got {message:?}",
        not_deletable(state)
    );
    for leaked in named {
        assert!(
            !message.contains(leaked),
            "case {case:?}: expected the message not to carry {leaked:?}, got {message:?}"
        );
    }
}

/// Rule: `DeleteMultiplex` on a multiplex that is not idle answers 409
/// `ConflictException` with the class's fixed sentence for the state the
/// multiplex is in, carrying neither the `MultiplexId` the request named nor the
/// multiplex's name or ARN — and writes nothing, so the multiplex goes on
/// reporting that same state.
///
/// The rows are every state a delete is refused in, each on a multiplex of a
/// different shape (name, settings, tags, programs held).
#[tokio::test]
async fn a_delete_refused_on_the_state_carries_the_classs_fixed_sentence() {
    for (index, state) in REFUSING_STATES.into_iter().enumerate() {
        let shape = &shapes()[index % shapes().len()];
        let case = format!("DeleteMultiplex on a {state} {:?} multiplex", shape.name);
        let router = medialive();
        let (id, arn) = multiplex_of(&router, &case, shape).await;
        reach(&router, &case, &id, state).await;
        let before = still_in(&router, &case, &id, state).await;

        let (status, refusal) = delete_multiplex(&router, &id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the modelled 409, got {status}: {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {refusal}"
        );
        assert_class_message(
            &case,
            &error_message(&refusal),
            state,
            &[id.as_str(), shape.name, arn.as_str()],
        );

        // The refused delete had no effect: the multiplex reads back exactly as
        // it did an instant before it, in the state the refusal named.
        let after = still_in(&router, &case, &id, state).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused delete to leave the multiplex alone, got {after}"
        );
    }
}

/// Rule, from the other entry point: `BatchDelete` refuses the same multiplex
/// for the same reason through its per-resource failure channel, so its entry
/// carries the same `ConflictException` and the same sentence — a client cannot
/// tell from the wording which entry point it asked through.
///
/// The rows are the same states, each on a multiplex of a different shape.
#[tokio::test]
async fn a_batch_reports_the_same_refusal_with_the_same_sentence() {
    for (index, state) in REFUSING_STATES.into_iter().enumerate() {
        let shape = &shapes()[(index + 1) % shapes().len()];
        let case = format!("BatchDelete on a {state} {:?} multiplex", shape.name);
        let router = medialive();
        let (id, arn) = multiplex_of(&router, &case, shape).await;
        reach(&router, &case, &id, state).await;
        let before = still_in(&router, &case, &id, state).await;

        let (status, response) = call(
            &router,
            "POST",
            BATCH_DELETE,
            Some(json!({ "multiplexIds": [id.clone()] })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a batch reports each resource on its own, so the call itself \
             answers 200, got {status}: {response}"
        );
        assert_eq!(
            response["failed"],
            json!([{
                "id": id,
                "arn": arn,
                "code": "ConflictException",
                "message": not_deletable(state),
            }]),
            "case {case:?}: expected the batch's failure entry to carry the class's \
             sentence, got {:?}",
            response["failed"]
        );
        assert_class_message(
            &case,
            response["failed"][0]["message"].as_str().unwrap_or_default(),
            state,
            &[shape.name],
        );

        let after = still_in(&router, &case, &id, state).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused batch delete to leave the multiplex alone, \
             got {after}"
        );
    }
}

/// The control: the state is the only thing either entry point refuses on, so
/// the very same request is ACCEPTED once the multiplex is idle — whatever it is
/// called, carries or holds. Without this row the sentence above could be the
/// answer to any delete at all.
#[tokio::test]
async fn the_same_delete_is_accepted_once_the_multiplex_is_idle() {
    for (index, shape) in shapes().iter().enumerate() {
        let case = format!("the idle {:?} multiplex", shape.name);
        let router = medialive();
        let (id, _) = multiplex_of(&router, &case, shape).await;
        // A new multiplex is still coming up, which is itself a refusing state;
        // the clock settles it into the one state a delete accepts.
        wait_for_the_clock();
        still_in(&router, &case, &id, IDLE).await;

        // Odd rows ask through the batch, even rows through the single-resource
        // delete, so both entry points are shown accepting.
        if index % 2 == 0 {
            let (status, accepted) = delete_multiplex(&router, &id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: DeleteMultiplex: expected 202, got {status}: {accepted}"
            );
            assert_eq!(
                accepted["state"],
                json!("DELETING"),
                "case {case:?}: expected the accepted delete to answer DELETING, got {accepted}"
            );
        } else {
            let (status, response) = call(
                &router,
                "POST",
                BATCH_DELETE,
                Some(json!({ "multiplexIds": [id.clone()] })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: BatchDelete: expected 200, got {status}: {response}"
            );
            assert_eq!(
                response["failed"],
                json!([]),
                "case {case:?}: expected the idle multiplex to be accepted, got {response}"
            );
            assert_eq!(
                response["successful"][0]["state"],
                json!("DELETING"),
                "case {case:?}: expected the accepted delete to report DELETING, got {response}"
            );
        }
        still_in(&router, &case, &id, "DELETING").await;
    }
}

/// Rule boundary: the sentence belongs to the state refusal alone. A
/// `MultiplexId` the account holds no multiplex for is a different refusal —
/// the modelled 404 `NotFoundException` naming the id the request asked about —
/// so a client reading the conflict sentence knows the multiplex exists and only
/// its state is in the way.
#[tokio::test]
async fn an_unheld_multiplex_id_is_not_this_refusal() {
    let router = medialive();
    for (case, id) in [
        ("an id of the shape this service mints", "9999999"),
        ("an identifier that is not numeric", "no-such-multiplex"),
    ] {
        let (status, body) = call(&router, "DELETE", &format!("{MULTIPLEXES}/{id}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the modelled 404, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Multiplex {id} not found"),
            "case {case:?}: expected the not-found to name the id asked about, got {body}"
        );
    }
}
