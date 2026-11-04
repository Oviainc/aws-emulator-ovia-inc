// SPDX-License-Identifier: CC0-1.0

//! Cancelling a game session placement, over the wire: `StopGameSessionPlacement` takes a
//! placement the queue is still working on out of that queue and moves the ONE stored
//! record to `CANCELLED` with the instant it was canceled as its end time — and refuses
//! every placement that is not the queue's to withdraw.
//!
//! The rule asserted here, over tables rather than over one request: a placement is
//! cancellable exactly while the queue is still processing it and no game session has been
//! created for it. So
//!
//! * a `PENDING` placement with no session is cancelled, whatever the request that queued
//!   it carried and whatever its queue's destinations are — nothing but its status and its
//!   end time changes, the clock stops moving it, and no session is ever placed for it;
//! * a placement a game session HAS been created for is refused, whether that session is
//!   still activating or already running, and the stored placement is left alone;
//! * a placement the queue has already finished with (canceled, timed out) is refused and
//!   keeps the status it ended in;
//! * an id no placement is stored under is refused as the reservation the service could not
//!   find.
//!
//! The harness (`GameLift`, `REGION`, `gamelift_arn`, `ec2_fleet`, `create_fleet`,
//! `wait_for_due_transitions`, `error_code`, `error_message`) is the one `hosting_loop`
//! drives the production dispatch path with, and `describe_placement` / `queue_with` are
//! the placement helpers `game_session_placements` defines.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};

use crate::game_session_placements::{describe_placement, queue_with};
use crate::hosting_loop::{
    create_fleet, ec2_fleet, error_code, error_message, gamelift_arn, wait_for_due_transitions,
    GameLift, REGION,
};

/// The documented minimum a queue's `TimeoutInSeconds` may be
/// (`smithy_query doc shape=CreateGameSessionQueueInput member=TimeoutInSeconds`: "The
/// minimum value is 10 and the maximum value is 600."), and so the shortest wait a
/// placement can time out in.
const MIN_QUEUE_TIMEOUT_SECONDS: u64 = 10;

/// Start the placement `request` describes and answer the placement the response reports.
async fn start_placement(gamelift: &GameLift, request: Value) -> Value {
    let started = gamelift.ok("StartGameSessionPlacement", request).await;
    started["GameSessionPlacement"].clone()
}

/// Stop `placement_id` and answer the placement the response reports.
async fn stop_placement(gamelift: &GameLift, placement_id: &str) -> Value {
    let stopped = gamelift
        .ok(
            "StopGameSessionPlacement",
            json!({"PlacementId": placement_id}),
        )
        .await;
    stopped["GameSessionPlacement"].clone()
}

/// The epoch seconds a timestamp member carries, for the one ordering a canceled placement
/// has to satisfy: it ended no earlier than it was queued.
fn seconds(placement: &Value, member: &str) -> f64 {
    placement[member].as_f64().unwrap_or_else(|| {
        panic!("expected {member} to be a timestamp, got {}", placement[member])
    })
}

/// A `PENDING` placement is cancelled: the response moves it to `CANCELLED` and stamps its
/// end time, changes NOTHING else about the stored record, and every later read — including
/// one past the spans the queue's attempt and timeout were due on — reports that same
/// record.
///
/// The rows differ along what the queued request carried (a bare request, game properties
/// and session data, desired player sessions, player latencies), the length of the
/// placement id at both modelled bounds of `IdStringModel` (len 1..48), and what the queue
/// could have done with the request: no destination at all, a destination naming no fleet,
/// and a fleet that COULD have hosted the session.
#[tokio::test]
async fn a_pending_placement_is_cancelled_and_the_queue_stops_working_on_it() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("cancellation-fleet")).await;
    let fleet_arn = gamelift_arn(&format!("fleet/{fleet_id}"));
    // The fleet's own life cycle has to finish before it could host anything, so the row
    // that names it is a queue that really could have placed the session.
    wait_for_due_transitions();
    let dangling = gamelift_arn("fleet/fleet-00000000-0000-0000-0000-000000000000");

    let rows: Vec<(&str, String, Vec<String>, Value)> = vec![
        (
            "a bare request on a queue with no destination",
            "c".to_string(),
            vec![],
            json!({}),
        ),
        (
            "game properties and session data",
            "cancel-with-game-data".to_string(),
            vec![],
            json!({
                "GameSessionName": "the canceled session",
                "GameProperties": [{"Key": "map", "Value": "arena"}],
                "GameSessionData": "canceled-session-data",
            }),
        ),
        (
            "desired player sessions",
            "cancel-with-players".to_string(),
            vec![],
            json!({
                "DesiredPlayerSessions": [
                    {"PlayerId": "player-1", "PlayerData": "loadout-1"},
                    {"PlayerId": "player-2"},
                ],
            }),
        ),
        (
            "player latencies, on a destination naming no fleet",
            "cancel-with-latencies".to_string(),
            vec![dangling.clone()],
            json!({
                "PlayerLatencies": [
                    {"PlayerId": "player-1", "RegionIdentifier": REGION, "LatencyInMilliseconds": 30.0},
                ],
            }),
        ),
        (
            "a queue whose fleet could have hosted the session",
            "cancel-before-the-queue-places-it".to_string(),
            vec![fleet_arn.clone()],
            json!({}),
        ),
        (
            "the longest placement id the model allows",
            "c".repeat(48),
            vec![],
            json!({}),
        ),
    ];

    for (index, (label, placement_id, destinations, extra)) in rows.iter().enumerate() {
        let queue = queue_with(&gamelift, &format!("cancel-queue-{index}"), destinations).await;
        let mut request = json!({
            "PlacementId": placement_id,
            "GameSessionQueueName": &queue,
            "MaximumPlayerSessionCount": 4,
        });
        for (member, value) in extra.as_object().expect("the extra members are an object") {
            request[member] = value.clone();
        }
        let queued = start_placement(&gamelift, request).await;
        assert_eq!(
            queued["Status"],
            json!("PENDING"),
            "case {label:?}: a queued placement must be PENDING before it is stopped, got \
             {queued}"
        );

        let stopped = stop_placement(&gamelift, placement_id).await;
        assert_eq!(
            stopped["Status"],
            json!("CANCELLED"),
            "case {label:?}: a stopped placement must be CANCELLED, got {stopped}"
        );
        assert!(
            stopped["EndTime"].is_number(),
            "case {label:?}: a canceled placement carries the time stamp it was canceled at, \
             got {stopped}"
        );
        assert!(
            seconds(&stopped, "EndTime") >= seconds(&stopped, "StartTime"),
            "case {label:?}: the placement ended no earlier than it was queued, got {stopped}"
        );
        // Nothing was placed, so there is no session and no connection information to
        // report, and no player slot was reserved.
        for member in [
            "GameSessionId",
            "GameSessionArn",
            "GameSessionRegion",
            "IpAddress",
            "Port",
            "DnsName",
        ] {
            assert!(
                stopped[member].is_null(),
                "case {label:?}: a canceled placement carries no {member}, got {stopped}"
            );
        }
        assert_eq!(
            stopped["PlacedPlayerSessions"],
            json!([]),
            "case {label:?}: a canceled placement placed no player session, got {stopped}"
        );
        // The cancel moved the status and stamped the end time, and left every other
        // member of the stored record exactly as the queued request had it.
        let mut expected = queued.clone();
        expected["Status"] = json!("CANCELLED");
        expected["EndTime"] = stopped["EndTime"].clone();
        assert_eq!(
            stopped, expected,
            "case {label:?}: the cancel must change only the status and the end time, got \
             {stopped}"
        );

        // The reader is served from the same one record...
        let described = describe_placement(&gamelift, placement_id).await;
        assert_eq!(
            described, stopped,
            "case {label:?}: the describe must report the canceled placement the stop \
             reported, got {described}"
        );
        // ...and the placement is out of the queue, so neither the queue's next attempt nor
        // its timeout moves it again.
        wait_for_due_transitions();
        wait_for_due_transitions();
        let later = describe_placement(&gamelift, placement_id).await;
        assert_eq!(
            later, stopped,
            "case {label:?}: a canceled placement is no longer in the queue, so the clock \
             must not move it, got {later}"
        );
    }

    // The fleet that COULD have hosted the canceled placement's session is still hosting
    // nothing: the request was withdrawn before the queue placed it.
    let sessions = gamelift
        .ok("DescribeGameSessions", json!({"FleetId": &fleet_id}))
        .await;
    assert_eq!(
        sessions["GameSessions"],
        json!([]),
        "a canceled placement must leave no game session on the fleet that could have hosted \
         it, got {sessions}"
    );
}

/// A placement a game session has already been created for cannot be withdrawn: the call is
/// refused with `InvalidRequestException`, and the stored placement is left exactly as it
/// was — whether that session is still activating (the placement still `PENDING`) or
/// already running (the placement `FULFILLED`).
#[tokio::test]
async fn stopping_a_placement_a_game_session_was_created_for_is_refused() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("fulfilled-stop-fleet")).await;
    let fleet_arn = gamelift_arn(&format!("fleet/{fleet_id}"));
    wait_for_due_transitions();
    let queue = queue_with(&gamelift, "fulfilled-stop-queue", &[fleet_arn]).await;

    let placement_id = "placement-with-a-session";
    start_placement(
        &gamelift,
        json!({
            "PlacementId": placement_id,
            "GameSessionQueueName": &queue,
            "MaximumPlayerSessionCount": 4,
            "DesiredPlayerSessions": [{"PlayerId": "player-1"}],
        }),
    )
    .await;

    // One span on, the queue has started a session for the placement, which is still
    // activating: the session exists, so the request can no longer be withdrawn.
    wait_for_due_transitions();
    let activating = describe_placement(&gamelift, placement_id).await;
    assert_eq!(
        activating["Status"],
        json!("PENDING"),
        "the placement whose session is activating stays PENDING, got {activating}"
    );
    let (status, body) = gamelift
        .call(
            "StopGameSessionPlacement",
            &json!({"PlacementId": placement_id}),
        )
        .await;
    let code = error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "case \"a session that is still activating\": expected 400 InvalidRequestException, got \
         {status} {code} with body {body}"
    );
    let message = error_message(&body);
    assert!(
        message.contains(placement_id) && message.contains("already been created"),
        "case \"a session that is still activating\": the refusal names the placement and the \
         session already created for it, got {message:?}"
    );
    let after = describe_placement(&gamelift, placement_id).await;
    assert_eq!(
        after, activating,
        "case \"a session that is still activating\": a refused stop must change nothing, got \
         {after}"
    );

    // A span later the session is running and the placement is FULFILLED: the same refusal,
    // now naming the session the caller can clean up.
    wait_for_due_transitions();
    let fulfilled = describe_placement(&gamelift, placement_id).await;
    assert_eq!(
        fulfilled["Status"],
        json!("FULFILLED"),
        "the placement must be fulfilled once its session is running, got {fulfilled}"
    );
    let game_session_id = fulfilled["GameSessionId"]
        .as_str()
        .expect("a fulfilled placement reports its game session")
        .to_string();
    let (status, body) = gamelift
        .call(
            "StopGameSessionPlacement",
            &json!({"PlacementId": placement_id}),
        )
        .await;
    let code = error_code(&body);
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "case \"a fulfilled placement\": expected 400 InvalidRequestException, got {status} \
         {code} with body {body}"
    );
    let message = error_message(&body);
    let expected = format!(
        "Game session {game_session_id} has already been created for game session placement \
         {placement_id}; it cannot be cancelled."
    );
    assert_eq!(
        message, expected,
        "case \"a fulfilled placement\": expected Message {expected:?}, got {message:?}"
    );
    let after = describe_placement(&gamelift, placement_id).await;
    assert_eq!(
        after, fulfilled,
        "case \"a fulfilled placement\": a refused stop must change nothing, got {after}"
    );

    // The control: a placement whose queue has placed nothing is still cancelled, so the
    // refusals above are about the session that was created and not a blanket answer.
    let idle_queue = queue_with(&gamelift, "idle-stop-queue", &[]).await;
    start_placement(
        &gamelift,
        json!({
            "PlacementId": "placement-without-a-session",
            "GameSessionQueueName": &idle_queue,
            "MaximumPlayerSessionCount": 4,
        }),
    )
    .await;
    let stopped = stop_placement(&gamelift, "placement-without-a-session").await;
    assert_eq!(
        stopped["Status"],
        json!("CANCELLED"),
        "a placement with no session must still be cancelled, got {stopped}"
    );
}

/// A placement the queue has already finished with is refused and keeps the status it
/// ended in: the rows are the two terminal statuses a placement with no game session can
/// hold — the one a cancel put it in, and the one its queue's timeout put it in.
#[tokio::test]
async fn stopping_a_placement_the_queue_has_finished_with_is_refused() {
    let gamelift = GameLift::new();

    // A placement already canceled: stopping it again is the same request a second time.
    let queue = queue_with(&gamelift, "terminal-cancel-queue", &[]).await;
    start_placement(
        &gamelift,
        json!({
            "PlacementId": "already-cancelled",
            "GameSessionQueueName": &queue,
            "MaximumPlayerSessionCount": 2,
        }),
    )
    .await;
    stop_placement(&gamelift, "already-cancelled").await;

    // A placement its queue timed out, with the shortest timeout a queue may be created
    // with so the wait is the documented minimum.
    gamelift
        .ok(
            "CreateGameSessionQueue",
            json!({
                "Name": "terminal-timeout-queue",
                "TimeoutInSeconds": MIN_QUEUE_TIMEOUT_SECONDS,
            }),
        )
        .await;
    start_placement(
        &gamelift,
        json!({
            "PlacementId": "already-timed-out",
            "GameSessionQueueName": "terminal-timeout-queue",
            "MaximumPlayerSessionCount": 2,
        }),
    )
    .await;
    sleep(Duration::from_secs(MIN_QUEUE_TIMEOUT_SECONDS) + Duration::from_millis(500));

    for (label, placement_id, expected_status) in [
        ("a placement already canceled", "already-cancelled", "CANCELLED"),
        ("a placement its queue timed out", "already-timed-out", "TIMED_OUT"),
    ] {
        let terminal = describe_placement(&gamelift, placement_id).await;
        assert_eq!(
            terminal["Status"],
            json!(expected_status),
            "case {label:?}: the placement must have reached {expected_status}, got {terminal}"
        );

        let (status, body) = gamelift
            .call(
                "StopGameSessionPlacement",
                &json!({"PlacementId": placement_id}),
            )
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: expected 400 InvalidRequestException, got {status} {code} with \
             body {body}"
        );
        let message = error_message(&body);
        let expected = format!(
            "Game session placement {placement_id} is in {expected_status} status and cannot be \
             cancelled."
        );
        assert_eq!(
            message, expected,
            "case {label:?}: expected Message {expected:?}, got {message:?}"
        );
        let after = describe_placement(&gamelift, placement_id).await;
        assert_eq!(
            after, terminal,
            "case {label:?}: a refused stop must leave the placement in the status it ended \
             in, got {after}"
        );
    }
}

/// `StopGameSessionPlacement` for an id no placement is stored under reports the
/// reservation it could not find and the account it looked in — the same text every
/// operation acting on one placement by id answers.
///
/// The rows span the identifiers `IdStringModel` (len 1..48, `^[a-zA-Z0-9-]+$`) allows,
/// including both of its length bounds, and the control is the placement that IS stored,
/// which is still cancelled afterwards.
#[tokio::test]
async fn stopping_a_placement_that_is_not_stored_is_refused() {
    let gamelift = GameLift::new();
    let queue = queue_with(&gamelift, "absent-stop-queue", &[]).await;
    let stored_id = "the-stored-placement";
    start_placement(
        &gamelift,
        json!({
            "PlacementId": stored_id,
            "GameSessionQueueName": &queue,
            "MaximumPlayerSessionCount": 2,
        }),
    )
    .await;

    let account = engine::emulator_account_id();
    let unstored: Vec<(&str, String)> = vec![
        ("one character, the shortest id the model allows", "p".to_string()),
        ("a hyphenated id", "unknown-placement".to_string()),
        ("digits only", "20261029".to_string()),
        ("mixed case", "AbSeNtPlacement".to_string()),
        (
            "the stored id with one more character",
            format!("{stored_id}-2"),
        ),
        ("48 characters, the longest id the model allows", "p".repeat(48)),
    ];

    for (label, placement_id) in &unstored {
        let (status, body) = gamelift
            .call(
                "StopGameSessionPlacement",
                &json!({"PlacementId": placement_id}),
            )
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {label:?}: expected 400 NotFoundException, got {status} {code} with body \
             {body}"
        );
        let message = error_message(&body);
        let expected = format!(
            "No GameSessionReservation found for reservation {placement_id} accountId {account}"
        );
        assert_eq!(
            message, expected,
            "case {label:?}: expected Message {expected:?}, got {message:?}"
        );
    }

    // The control: the placement that IS stored survived every refusal and is cancelled.
    let stopped = stop_placement(&gamelift, stored_id).await;
    assert_eq!(
        (stopped["PlacementId"].clone(), stopped["Status"].clone()),
        (json!(stored_id), json!("CANCELLED")),
        "the stored placement must survive the refusals and be cancelled, got {stopped}"
    );
}
