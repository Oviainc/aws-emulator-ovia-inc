// SPDX-License-Identifier: CC0-1.0

//! How long a multiplex `RequestId` stays a replay: `CreateMultiplex` answers a
//! retry with the multiplex its `@idempotencyToken` minted only while that
//! multiplex is still the account's.
//!
//! `DeleteMultiplex` is a tombstone delete — the record stays readable, in the
//! transient `DELETING` state and then in the terminal `DELETED` one
//! (`smithy_query(query="shape", shape="MultiplexState")`) — so once the
//! deletion has come due the token that minted it must resolve to nothing: the
//! retried create mints a multiplex of its own rather than answering 201 with a
//! record that can neither be started nor updated, having stored nothing at all.
//! That is the rule the channel, input, network, cluster and channel placement
//! group tokens of this crate already follow.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are TABLES: the reported create, retried once its multiplex had
//! settled into `DELETED`, is ONE row of them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_lifecycle::{
    create_body, created, describe_multiplex, settings, MULTIPLEXES,
};
use crate::multiplex_start::{add_programs, start_multiplex};
use crate::wire::{call, error_code, medialive};

/// The states a multiplex a delete has been accepted for reports — the transient
/// one the acceptance writes and the terminal one the clock owes it.
const DEAD_STATES: [&str; 2] = ["DELETING", "DELETED"];

/// `DeleteMultiplex`'s binding (`smithy_query(query="operation",
/// operation="DeleteMultiplex")`: `DELETE /prod/multiplexes/{MultiplexId}`,
/// success 202).
async fn delete_multiplex(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = call(router, "DELETE", &format!("{MULTIPLEXES}/{id}"), None).await;
    assert_eq!(
        status, 202,
        "case {case:?}: DeleteMultiplex: expected 202, got {status}: {body}"
    );
    body
}

/// `UpdateMultiplex`'s binding (`smithy_query(query="operation",
/// operation="UpdateMultiplex")`: `PUT /prod/multiplexes/{MultiplexId}`, success
/// 200): the proof that the multiplex a retry answered with is one a caller may
/// still CHANGE, which the tombstone is not.
async fn update_multiplex(
    router: &MultiServiceRouter,
    id: &str,
    body: Value,
) -> (u16, Value) {
    let (status, response) = call(router, "PUT", &format!("{MULTIPLEXES}/{id}"), Some(body)).await;
    if status == 200 {
        (status, response["multiplex"].clone())
    } else {
        (status, response)
    }
}

/// Every multiplex id the account holds, sorted, as `ListMultiplexes` reports
/// them — a multiplex whose delete has COMPLETED is no longer one the account
/// holds, so the listing leaves it out (`crate::multiplex_listing`) while its
/// record stays readable through `DescribeMultiplex`, which is what this suite
/// asserts separately.
async fn held_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "GET", MULTIPLEXES, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListMultiplexes: expected 200, got {status}: {body}"
    );
    let mut ids: Vec<String> = body["multiplexes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(id_of)
        .collect();
    ids.sort();
    ids
}

/// The `id` a reported multiplex carries.
fn id_of(multiplex: &Value) -> String {
    multiplex["id"].as_str().unwrap_or_default().to_string()
}

/// The `state` a reported multiplex carries.
fn state_of(multiplex: &Value) -> String {
    multiplex["state"].as_str().unwrap_or_default().to_string()
}

/// The same multiplex with its `state` dropped, so two readings taken either
/// side of a transient transition are compared on everything the clock does not
/// move.
fn without_state(multiplex: &Value) -> Value {
    let mut stripped = multiplex.clone();
    if let Some(object) = stripped.as_object_mut() {
        object.remove("state");
    }
    stripped
}

/// `CreateMultiplex` carrying `token`, as a row's retry sends it: its own name
/// and its own settings, so an answer that replays the first multiplex is told
/// apart from one the retry minted.
async fn create_with_token(
    router: &MultiServiceRouter,
    token: &str,
    name: &str,
    multiplex_settings: Value,
) -> (u16, Value) {
    let (status, response) = call(
        router,
        "POST",
        MULTIPLEXES,
        Some(create_body(name, token, multiplex_settings)),
    )
    .await;
    if status == 201 {
        (status, response["multiplex"].clone())
    } else {
        (status, response)
    }
}

/// The multiplex `id`, read back through `DescribeMultiplex` with the read
/// asserted to have succeeded.
async fn described(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = describe_multiplex(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeMultiplex: expected 200, got {status}: {body}"
    );
    body
}

/// Rule: a create's `@idempotencyToken` never outlives the resource it minted —
/// once the deletion of that resource has come due the token's idempotency
/// window is over, so the retried create mints a NEW resource with the values
/// its own request carries instead of answering with the tombstone, and the
/// token then names that new resource.
///
/// The rows differ along what the rule generalises over: how many programs the
/// deleted multiplex held, the `MultiplexSettings` the retry asks for, whether
/// the retry asks for the deleted multiplex's own name or another, whether the
/// multiplex was run and stopped before the delete, and the spelling of the
/// token.
#[tokio::test]
async fn a_deleted_multiplex_is_not_resurrected_by_its_idempotency_token() {
    // (case, token, programs the first multiplex held, whether it ran before the
    //  delete, the name the retry asks for, the settings the retry asks for)
    let rows: [(&str, &str, i64, bool, &str, Value); 5] = [
        (
            "an empty multiplex",
            "token-a",
            0,
            false,
            "second",
            settings(4_000_000, 2),
        ),
        (
            "one program",
            "t",
            1,
            false,
            "second",
            settings(1_000_000, 0),
        ),
        (
            "three programs",
            "a-much-longer-idempotency-token",
            3,
            false,
            "second",
            settings(100_000_000, 65_535),
        ),
        (
            "started and stopped before the delete",
            "token-a",
            1,
            true,
            "second",
            settings(10_000_000, 7),
        ),
        (
            "the retry asks for the deleted multiplex's own name",
            "token-a",
            0,
            false,
            "first",
            json!({
                "transportStreamBitrate": 5_000_000,
                "transportStreamId": 42,
                "maximumVideoBufferDelayMilliseconds": 1_000,
                "transportStreamReservedBitrate": 100_000,
            }),
        ),
    ];

    for (case, token, programs, ran, retried_name, retried_settings) in rows {
        let router = medialive();
        let first = created(
            &router,
            case,
            create_body("first", token, settings(2_000_000, 1)),
        )
        .await;
        let first_id = id_of(&first);
        add_programs(&router, case, &first_id, programs).await;

        // While the multiplex is the account's the token IS a replay: the same
        // create carrying it answers with that multiplex and stores nothing new.
        let (status, replayed) =
            create_with_token(&router, token, retried_name, retried_settings.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the live replay to answer 201, got {status}: {replayed}"
        );
        assert_eq!(
            id_of(&replayed),
            first_id,
            "case {case:?}: expected a live multiplex's token to replay {first_id:?}, got {replayed}"
        );
        assert_eq!(
            held_ids(&router, case).await,
            vec![first_id.clone()],
            "case {case:?}: expected the replay to have stored nothing, got {:?}",
            held_ids(&router, case).await
        );

        // A multiplex is deleted from the stopped state, so the create's own
        // transition is let come due first; a row that runs it puts it back there
        // through the start and the stop.
        wait_for_the_clock();
        if ran {
            let (status, started) = start_multiplex(&router, &first_id).await;
            assert_eq!(
                status, 202,
                "case {case:?}: StartMultiplex: expected 202, got {status}: {started}"
            );
            wait_for_the_clock();
            let (status, stopping) = call(
                &router,
                "POST",
                &format!("{MULTIPLEXES}/{first_id}/stop"),
                None,
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?}: StopMultiplex: expected 202, got {status}: {stopping}"
            );
            wait_for_the_clock();
        }

        delete_multiplex(&router, case, &first_id).await;
        wait_for_the_clock();
        let dead = described(&router, case, &first_id).await;
        assert_eq!(
            state_of(&dead),
            "DELETED",
            "case {case:?}: expected the deleted multiplex to report DELETED, got {dead}"
        );

        // The window is over: the same create carrying the same token mints a
        // multiplex of its own, carrying the values ITS request asked for.
        let (status, retried) =
            create_with_token(&router, token, retried_name, retried_settings.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the retry to answer 201, got {status}: {retried}"
        );
        let retried_id = id_of(&retried);
        assert_ne!(
            retried_id, first_id,
            "case {case:?}: expected a freshly minted multiplex, got the deleted {first_id:?}"
        );
        assert_eq!(
            retried["name"],
            json!(retried_name),
            "case {case:?}: expected the name the retry asked for, got {retried}"
        );
        assert_eq!(
            retried["multiplexSettings"],
            retried_settings,
            "case {case:?}: expected the settings the retry asked for, got {retried}"
        );
        assert_eq!(
            retried["programCount"],
            json!(0),
            "case {case:?}: expected a multiplex of its own, holding no program, got {retried}"
        );
        assert!(
            !DEAD_STATES.contains(&state_of(&retried).as_str()),
            "case {case:?}: expected the retry to answer with a live multiplex, got {retried}"
        );

        // The retry really STORED that multiplex: the reader reports it, and the
        // deleted record stayed exactly as the clock had left it.
        let read = described(&router, case, &retried_id).await;
        assert_eq!(
            without_state(&read),
            without_state(&retried),
            "case {case:?}: expected the reader to report the multiplex the retry stored, got {read}"
        );
        assert!(
            !DEAD_STATES.contains(&state_of(&read).as_str()),
            "case {case:?}: expected the stored retry to be live, got {read}"
        );
        assert_eq!(
            described(&router, case, &first_id).await,
            dead,
            "case {case:?}: expected the deleted multiplex to stay as it was, got {:?}",
            described(&router, case, &first_id).await
        );

        // The deleted multiplex is out of the account, so the listing reports
        // the retry's own multiplex alone — while the tombstone stays readable
        // through its describe, which the assertion above just made.
        let expected_held = vec![retried_id.clone()];
        assert_eq!(
            held_ids(&router, case).await,
            expected_held,
            "case {case:?}: expected the account to hold the retry's own multiplex and not the \
             deleted one, got {:?}",
            held_ids(&router, case).await
        );

        // The multiplex the retry answered with is one the caller may CHANGE and
        // START, which is what the tombstone was not, and the token now names
        // that live multiplex: one more retry replays it rather than the deleted
        // record.
        let (status, renamed) =
            update_multiplex(&router, &retried_id, json!({ "name": "renamed" })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the retry's multiplex to be updatable, got {status}: {renamed}"
        );
        wait_for_the_clock();
        let (status, started) = start_multiplex(&router, &retried_id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected the retry's multiplex to be startable, got {status}: {started}"
        );
        let (status, again) =
            create_with_token(&router, token, "renamed", retried_settings.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the third create to answer 201, got {status}: {again}"
        );
        assert_eq!(
            id_of(&again),
            retried_id,
            "case {case:?}: expected the token to replay the live multiplex {retried_id:?}, got \
             {again}"
        );
        assert_eq!(
            held_ids(&router, case).await,
            expected_held,
            "case {case:?}: expected the second replay to have stored nothing, got {:?}",
            held_ids(&router, case).await
        );
    }
}

/// Rule: the replay a tombstone cannot serve is the one thing the delete takes
/// away — the token window a delete ends belongs to the deleted multiplex alone,
/// so every other token of the account goes on replaying the multiplex it
/// minted, and the deleted multiplex's own id goes on being refused as the
/// unusable record it is.
///
/// The rows differ along what the rule generalises over: how many programs the
/// surviving multiplex holds, and whether it was created before or after the
/// doomed one.
#[tokio::test]
async fn deleting_one_multiplex_leaves_every_other_token_replaying() {
    // (case, programs the survivor holds, whether the survivor was created first)
    let rows: [(&str, i64, bool); 3] = [
        ("the survivor created second", 0, false),
        ("the survivor created first", 0, true),
        ("the survivor holding programs", 2, false),
    ];

    for (case, programs, survivor_first) in rows {
        let router = medialive();
        let (doomed, survivor) = if survivor_first {
            let survivor = created(
                &router,
                case,
                create_body("survivor", "token-b", settings(3_000_000, 2)),
            )
            .await;
            let doomed = created(
                &router,
                case,
                create_body("doomed", "token-a", settings(2_000_000, 1)),
            )
            .await;
            (doomed, survivor)
        } else {
            let doomed = created(
                &router,
                case,
                create_body("doomed", "token-a", settings(2_000_000, 1)),
            )
            .await;
            let survivor = created(
                &router,
                case,
                create_body("survivor", "token-b", settings(3_000_000, 2)),
            )
            .await;
            (doomed, survivor)
        };
        let doomed_id = id_of(&doomed);
        let survivor_id = id_of(&survivor);
        add_programs(&router, case, &survivor_id, programs).await;

        wait_for_the_clock();
        delete_multiplex(&router, case, &doomed_id).await;
        wait_for_the_clock();

        // The survivor's own token still replays its multiplex, member for
        // member.
        let stored = described(&router, case, &survivor_id).await;
        let (status, replayed) =
            create_with_token(&router, "token-b", "another-name", settings(9_000_000, 9)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the survivor's replay to answer 201, got {status}: {replayed}"
        );
        assert_eq!(
            id_of(&replayed),
            survivor_id,
            "case {case:?}: expected the surviving token to replay {survivor_id:?}, got {replayed}"
        );
        assert_eq!(
            replayed, stored,
            "case {case:?}: expected the replay to report the stored multiplex, got {replayed}"
        );

        // And the deleted multiplex's id is still the unusable record it was: the
        // retry that minted a new multiplex changed nothing about it.
        let (status, refused) = start_multiplex(&router, &doomed_id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected starting the deleted multiplex to conflict, got {status}: \
             {refused}"
        );
        assert_eq!(
            error_code(status, &refused),
            "ConflictException",
            "case {case:?}: expected the modelled ConflictException, got {refused}"
        );
        assert_eq!(
            state_of(&described(&router, case, &doomed_id).await),
            "DELETED",
            "case {case:?}: expected the deleted multiplex to stay DELETED"
        );
    }
}
