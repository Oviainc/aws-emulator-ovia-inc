// SPDX-License-Identifier: CC0-1.0

//! Game session placement, over the wire: `StartGameSessionPlacement` adds the request
//! to the queue that processes it, the queue starts a game session on the first
//! destination that can host one, and `DescribeGameSessionPlacement` reports the
//! placement — PENDING while the queue is working on it, FULFILLED with the connection
//! information of the session it started, and refused outright for a placement id that
//! names nothing.
//!
//! The harness (`GameLift`, `REGION`, `gamelift_arn`, `ec2_fleet`, `create_fleet`,
//! `wait_for_due_transitions`) is the one `hosting_loop` already drives the production
//! dispatch path with.

use serde_json::{json, Value};

use crate::hosting_loop::{
    create_fleet, ec2_fleet, gamelift_arn, wait_for_due_transitions, GameLift, REGION,
};

/// The placement `DescribeGameSessionPlacement` reports for `placement_id`.
pub(crate) async fn describe_placement(gamelift: &GameLift, placement_id: &str) -> Value {
    let described = gamelift
        .ok(
            "DescribeGameSessionPlacement",
            json!({"PlacementId": placement_id}),
        )
        .await;
    described["GameSessionPlacement"].clone()
}

/// Create a queue named `name` whose destinations are `destinations`, in placement
/// preference order.
pub(crate) async fn queue_with(gamelift: &GameLift, name: &str, destinations: &[String]) -> String {
    gamelift
        .ok(
            "CreateGameSessionQueue",
            json!({
                "Name": name,
                "TimeoutInSeconds": 60,
                "Destinations": destinations
                    .iter()
                    .map(|arn| json!({"DestinationArn": arn}))
                    .collect::<Vec<_>>(),
            }),
        )
        .await;
    name.to_string()
}

/// A queue that does not exist cannot take a placement request: the request is refused
/// with the modelled `NotFoundException`, nothing is stored for the placement id it
/// carried, and a describe of that id is refused too.
///
/// The rows differ along the form of the identifier (name or ARN) and the length of the
/// placement id, at both modelled bounds of `IdStringModel` (len 1..48).
#[tokio::test]
async fn a_placement_on_a_queue_that_does_not_exist_is_refused() {
    let gamelift = GameLift::new();
    // A stored queue, so the refusals below are about the queue the request names and
    // not about an empty store.
    queue_with(&gamelift, "the-only-queue", &[]).await;

    let rows: Vec<(&str, String, String)> = vec![
        (
            "a queue name that names nothing",
            "absent-queue".to_string(),
            "p".to_string(),
        ),
        (
            "a queue ARN that names nothing",
            gamelift_arn("gamesessionqueue/absent-queue"),
            "placement-by-arn".to_string(),
        ),
        (
            "a queue name that names nothing, with the longest placement id",
            "another-absent-queue".to_string(),
            "p".repeat(48),
        ),
    ];

    for (label, queue, placement_id) in &rows {
        let (status, code) = gamelift
            .err(
                "StartGameSessionPlacement",
                json!({
                    "PlacementId": placement_id,
                    "GameSessionQueueName": queue,
                    "MaximumPlayerSessionCount": 4,
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {label:?}: an unknown queue must be refused with NotFoundException, got \
             {status} {code}"
        );

        let (status, code) = gamelift
            .err(
                "DescribeGameSessionPlacement",
                json!({"PlacementId": placement_id}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {label:?}: a placement the refused start did not create must be refused with \
             NotFoundException, got {status} {code}"
        );
    }

    // The queue that IS there still takes a placement, so the refusals are about the
    // named queue and nothing else.
    let started = gamelift
        .ok(
            "StartGameSessionPlacement",
            json!({
                "PlacementId": "placement-on-stored-queue",
                "GameSessionQueueName": "the-only-queue",
                "MaximumPlayerSessionCount": 4,
            }),
        )
        .await;
    assert_eq!(
        started["GameSessionPlacement"]["Status"],
        json!("PENDING"),
        "a placement on a stored queue must be accepted and PENDING, got {started}"
    );
}

/// A request that breaks a rule this operation documents is refused, and stores nothing.
#[tokio::test]
async fn a_placement_that_breaks_a_documented_rule_is_refused() {
    let gamelift = GameLift::new();
    queue_with(&gamelift, "plain-queue", &[]).await;
    // A queue that prioritizes locations, which is the only kind a
    // `PriorityConfigurationOverride` may be used with.
    gamelift
        .ok(
            "CreateGameSessionQueue",
            json!({
                "Name": "location-queue",
                "PriorityConfiguration": {
                    "PriorityOrder": ["LOCATION", "LATENCY"],
                    "LocationOrder": [REGION],
                },
            }),
        )
        .await;

    let rows: Vec<(&str, Value)> = vec![
        (
            "player latencies together with a priority override",
            json!({
                "PlacementId": "latencies-and-override",
                "GameSessionQueueName": "location-queue",
                "MaximumPlayerSessionCount": 4,
                "PlayerLatencies": [
                    {"PlayerId": "p1", "RegionIdentifier": REGION, "LatencyInMilliseconds": 30.0},
                ],
                "PriorityConfigurationOverride": {"LocationOrder": [REGION]},
            }),
        ),
        (
            "a priority override on a queue that does not prioritize locations",
            json!({
                "PlacementId": "override-on-plain-queue",
                "GameSessionQueueName": "plain-queue",
                "MaximumPlayerSessionCount": 4,
                "PriorityConfigurationOverride": {"LocationOrder": [REGION]},
            }),
        ),
        (
            "more players than the game session holds",
            json!({
                "PlacementId": "too-many-players",
                "GameSessionQueueName": "plain-queue",
                "MaximumPlayerSessionCount": 1,
                "DesiredPlayerSessions": [{"PlayerId": "p1"}, {"PlayerId": "p2"}],
            }),
        ),
        (
            "one player asking for two slots",
            json!({
                "PlacementId": "repeated-player",
                "GameSessionQueueName": "plain-queue",
                "MaximumPlayerSessionCount": 4,
                "DesiredPlayerSessions": [{"PlayerId": "p1"}, {"PlayerId": "p1"}],
            }),
        ),
        (
            "a desired player session without a player id",
            json!({
                "PlacementId": "player-without-id",
                "GameSessionQueueName": "plain-queue",
                "MaximumPlayerSessionCount": 4,
                "DesiredPlayerSessions": [{"PlayerData": "loadout"}],
            }),
        ),
        (
            "more player slots than a game session can hold",
            json!({
                "PlacementId": "above-the-player-ceiling",
                "GameSessionQueueName": "plain-queue",
                "MaximumPlayerSessionCount": 201,
            }),
        ),
    ];

    for (label, request) in &rows {
        let (status, code) = gamelift
            .err("StartGameSessionPlacement", request.clone())
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: the request must be refused, got {status} {code}"
        );
        let placement_id = request["PlacementId"].as_str().unwrap_or_default();
        let (status, code) = gamelift
            .err(
                "DescribeGameSessionPlacement",
                json!({"PlacementId": placement_id}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {label:?}: a refused start must store nothing, got {status} {code}"
        );
    }

    // A placement id a placement already holds cannot be taken a second time.
    gamelift
        .ok(
            "StartGameSessionPlacement",
            json!({
                "PlacementId": "unique-placement",
                "GameSessionQueueName": "plain-queue",
                "MaximumPlayerSessionCount": 4,
            }),
        )
        .await;
    let (status, code) = gamelift
        .err(
            "StartGameSessionPlacement",
            json!({
                "PlacementId": "unique-placement",
                "GameSessionQueueName": "plain-queue",
                "MaximumPlayerSessionCount": 8,
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidRequestException"),
        "a repeated placement id must be refused, got {status} {code}"
    );
    let placement = describe_placement(&gamelift, "unique-placement").await;
    assert_eq!(
        placement["MaximumPlayerSessionCount"],
        json!(4),
        "the refused start must leave the stored placement untouched, got {placement}"
    );
}

/// The queue walks its destinations in order and starts a game session on the first one
/// that can host it; the placement is then FULFILLED and carries that session's
/// connection information and a player session for each player the request named.
///
/// The rows differ along what the destination list holds: a fleet ARN, an alias that
/// routes at the fleet, a dangling destination BEFORE the one that can host (so the order
/// of the walk is what decides), and a terminal alias that resolves to no fleet at all.
#[tokio::test]
async fn a_placement_is_fulfilled_on_the_first_destination_that_can_host_it() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("placement-fleet")).await;
    let fleet_arn = gamelift_arn(&format!("fleet/{fleet_id}"));
    let simple_alias = gamelift
        .ok(
            "CreateAlias",
            json!({
                "Name": "placement-alias",
                "RoutingStrategy": {"Type": "SIMPLE", "FleetId": &fleet_id},
            }),
        )
        .await["Alias"]["AliasId"]
        .as_str()
        .expect("CreateAlias must report an AliasId")
        .to_string();
    let terminal_alias = gamelift
        .ok(
            "CreateAlias",
            json!({
                "Name": "terminal-alias",
                "RoutingStrategy": {"Type": "TERMINAL", "Message": "upgrade your client"},
            }),
        )
        .await["Alias"]["AliasId"]
        .as_str()
        .expect("CreateAlias must report an AliasId")
        .to_string();
    // The fleet's own life cycle has to finish before any session can be placed on it.
    wait_for_due_transitions();

    let dangling = gamelift_arn("fleet/fleet-00000000-0000-0000-0000-000000000000");
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("the fleet by ARN", vec![fleet_arn.clone()]),
        (
            "an alias that routes at the fleet",
            vec![simple_alias.clone()],
        ),
        (
            "a dangling destination before the fleet",
            vec![dangling.clone(), fleet_arn.clone()],
        ),
        (
            "a terminal alias before the fleet",
            vec![terminal_alias.clone(), fleet_arn.clone()],
        ),
    ];

    for (index, (label, destinations)) in rows.iter().enumerate() {
        let queue = queue_with(&gamelift, &format!("fulfil-queue-{index}"), destinations).await;
        let placement_id = format!("fulfilled-{index}");
        let started = gamelift
            .ok(
                "StartGameSessionPlacement",
                json!({
                    "PlacementId": &placement_id,
                    "GameSessionQueueName": &queue,
                    "MaximumPlayerSessionCount": 4,
                    "GameSessionName": "the placed session",
                    "GameProperties": [{"Key": "map", "Value": "arena"}],
                    "GameSessionData": "placed-session-data",
                    "DesiredPlayerSessions": [
                        {"PlayerId": "player-1", "PlayerData": "loadout-1"},
                        {"PlayerId": "player-2"},
                    ],
                }),
            )
            .await;
        let placement = started["GameSessionPlacement"].clone();
        // "If the game session placement status is PENDING, the properties for game
        // session ID/ARN, region, IP address/DNS, and port aren't final": a caller acting
        // straight after the call sees the transient status and no connection
        // information.
        assert_eq!(
            placement["Status"],
            json!("PENDING"),
            "case {label:?}: a new placement must be PENDING, got {placement}"
        );
        for member in [
            "GameSessionId",
            "GameSessionArn",
            "GameSessionRegion",
            "IpAddress",
            "Port",
            "EndTime",
        ] {
            assert!(
                placement[member].is_null(),
                "case {label:?}: a PENDING placement carries no {member}, got {placement}"
            );
        }
        assert_eq!(
            placement["PlacedPlayerSessions"],
            json!([]),
            "case {label:?}: a PENDING placement has placed no player session, got {placement}"
        );

        // The queue's attempt and the session's own activation are each one span away,
        // and each is materialised by the next call: the first read past the attempt span
        // finds the session started but not yet running, so the placement is still
        // PENDING; the read past its activation finds it fulfilled.
        wait_for_due_transitions();
        let attempted = describe_placement(&gamelift, &placement_id).await;
        assert_eq!(
            attempted["Status"],
            json!("PENDING"),
            "case {label:?}: a placement whose session is still activating stays PENDING, got \
             {attempted}"
        );
        wait_for_due_transitions();

        let placement = describe_placement(&gamelift, &placement_id).await;
        assert_eq!(
            placement["Status"],
            json!("FULFILLED"),
            "case {label:?}: the placement must be fulfilled on the fleet that can host it, got \
             {placement}"
        );
        let game_session_id = placement["GameSessionId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {label:?}: a fulfilled placement reports its game session, got {placement}")
            })
            .to_string();
        assert_eq!(
            placement["GameSessionArn"],
            json!(&game_session_id),
            "case {label:?}: GameSessionArn is the same value as GameSessionId, got {placement}"
        );
        assert_eq!(
            placement["GameSessionRegion"],
            json!(REGION),
            "case {label:?}: the placement must report the Region the session runs in, got \
             {placement}"
        );
        assert!(
            placement["Port"].is_number() && placement["IpAddress"].is_string(),
            "case {label:?}: a fulfilled placement carries connection information, got {placement}"
        );
        assert!(
            placement["EndTime"].is_number(),
            "case {label:?}: a completed placement carries an EndTime, got {placement}"
        );
        let placed: Vec<String> = placement["PlacedPlayerSessions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|placed| placed["PlayerId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            placed,
            vec!["player-1".to_string(), "player-2".to_string()],
            "case {label:?}: a player session must be created for each player named, got \
             {placement}"
        );

        // The game session the queue started is a real stored session on the fleet, with
        // the properties the placement asked for and the slots its players took.
        let described = gamelift
            .ok(
                "DescribeGameSessions",
                json!({"GameSessionId": &game_session_id}),
            )
            .await;
        let session = described["GameSessions"][0].clone();
        assert_eq!(
            (
                session["Status"].clone(),
                session["FleetId"].clone(),
                session["Name"].clone(),
                session["MaximumPlayerSessionCount"].clone(),
                session["CurrentPlayerSessionCount"].clone(),
                session["GameSessionData"].clone(),
                session["GameProperties"].clone(),
            ),
            (
                json!("ACTIVE"),
                json!(&fleet_id),
                json!("the placed session"),
                json!(4),
                json!(2),
                json!("placed-session-data"),
                json!([{"Key": "map", "Value": "arena"}]),
            ),
            "case {label:?}: the placed session must be stored as the placement asked, got \
             {described}"
        );
        assert_eq!(
            (
                session["IpAddress"].clone(),
                session["Port"].clone(),
                session["DnsName"].clone()
            ),
            (
                placement["IpAddress"].clone(),
                placement["Port"].clone(),
                placement["DnsName"].clone()
            ),
            "case {label:?}: the placement and the session it placed must report the same \
             connection information, got {described}"
        );

        // Reading the placement changed nothing: a second read reports the same record.
        let again = describe_placement(&gamelift, &placement_id).await;
        assert_eq!(
            again, placement,
            "case {label:?}: describing a placement must not change what a later describe \
             reports, got {again}"
        );
    }
}

/// A placement whose queue has no destination that can host a session stays PENDING: the
/// queue keeps retrying until its timeout, and nothing is reported as placed.
///
/// The rows differ along why no destination can host: the queue lists none, the one it
/// lists names no stored fleet, and the one it lists is a terminal alias that resolves to
/// no fleet.
#[tokio::test]
async fn a_placement_no_destination_can_host_stays_pending() {
    let gamelift = GameLift::new();
    let terminal_alias = gamelift
        .ok(
            "CreateAlias",
            json!({
                "Name": "pending-terminal-alias",
                "RoutingStrategy": {"Type": "TERMINAL", "Message": "no fleet here"},
            }),
        )
        .await["Alias"]["AliasId"]
        .as_str()
        .expect("CreateAlias must report an AliasId")
        .to_string();
    // An Anywhere fleet runs no server process of its own, so it has no capacity to host
    // a placed session even though it is a stored, ACTIVE fleet.
    let anywhere = create_fleet(
        &gamelift,
        json!({
            "Name": "pending-anywhere-fleet",
            "ComputeType": "ANYWHERE",
            "Locations": [{"Location": REGION}],
        }),
    )
    .await;
    wait_for_due_transitions();

    let rows: Vec<(&str, Vec<String>)> = vec![
        ("a queue with no destination at all", vec![]),
        (
            "a destination naming no stored fleet",
            vec![gamelift_arn("fleet/fleet-absent")],
        ),
        (
            "a terminal alias, which resolves to no fleet",
            vec![terminal_alias.clone()],
        ),
        (
            "a fleet with no server process that could host a session",
            vec![gamelift_arn(&format!("fleet/{anywhere}"))],
        ),
    ];

    for (index, (label, destinations)) in rows.iter().enumerate() {
        let queue = queue_with(&gamelift, &format!("pending-queue-{index}"), destinations).await;
        let placement_id = format!("still-pending-{index}");
        gamelift
            .ok(
                "StartGameSessionPlacement",
                json!({
                    "PlacementId": &placement_id,
                    "GameSessionQueueName": &queue,
                    "MaximumPlayerSessionCount": 2,
                }),
            )
            .await;

        // Past two attempt spans the placement is still waiting: the queue's timeout is
        // what ends it, not a failed attempt.
        wait_for_due_transitions();
        wait_for_due_transitions();
        let placement = describe_placement(&gamelift, &placement_id).await;
        assert_eq!(
            placement["Status"],
            json!("PENDING"),
            "case {label:?}: a placement no destination can host must stay PENDING, got \
             {placement}"
        );
        assert!(
            placement["GameSessionId"].is_null() && placement["Port"].is_null(),
            "case {label:?}: nothing was placed, so there is no session to report, got \
             {placement}"
        );
    }
}
