// SPDX-License-Identifier: CC0-1.0

//! A game session is placed only where a server process is running and free to host it.
//!
//! `CreateGameSession` "prompts an available server process to start a game session", so a
//! fleet LOCATION with no active, available server process left answers the modelled
//! `FleetCapacityExceededException` (HTTP 400) and persists nothing — the session it
//! refused is absent from `DescribeGameSessions`.
//!
//! The rows span the two `ComputeType` values (`smithy_query shape=ComputeType` —
//! `EC2`, `ANYWHERE`), the sizes a managed fleet's `RuntimeConfiguration` asks for, and a
//! fleet's home and remote locations. Every call goes through the same
//! `MultiServiceRouter::dispatch` the emulator serves real requests with.

use serde_json::{json, Value};

use crate::hosting_loop::{
    anywhere_fleet, create_fleet, ec2_fleet, wait_for_due_transitions, GameLift,
};

/// A managed fleet request whose runtime configuration asks for `processes` concurrent
/// server processes, deployed to `locations` beyond the home Region.
fn managed_fleet(name: &str, processes: i64, locations: &[&str]) -> Value {
    let mut request = ec2_fleet(name);
    request["RuntimeConfiguration"] = json!({
        "ServerProcesses": [{
            "LaunchPath": "/local/game/server",
            "ConcurrentExecutions": processes,
        }],
    });
    if !locations.is_empty() {
        request["Locations"] = json!(locations
            .iter()
            .map(|location| json!({"Location": location}))
            .collect::<Vec<_>>());
    }
    request
}

/// How many game sessions `DescribeGameSessions` reports for a fleet, optionally narrowed
/// to one of its locations.
async fn session_count(gamelift: &GameLift, fleet_id: &str, location: Option<&str>) -> usize {
    let mut request = json!({"FleetId": fleet_id});
    if let Some(location) = location {
        request["Location"] = json!(location);
    }
    let listed = gamelift.ok("DescribeGameSessions", request).await;
    listed["GameSessions"]
        .as_array()
        .map_or(0, |sessions| sessions.len())
}

/// A fleet location hosts exactly as many concurrent game sessions as it has server
/// processes running — which, for an Anywhere fleet, is none, because its game servers run
/// on compute resources the customer registers and each process announces itself through
/// the server SDK rather than through this API. The request past that count is refused
/// with `FleetCapacityExceededException`, and the refused session is never stored.
#[tokio::test]
async fn a_fleet_location_hosts_one_game_session_per_available_server_process() {
    struct Case {
        label: &'static str,
        request: Value,
        /// The `Location` the create names, or `None` to target the fleet's home Region.
        location: Option<&'static str>,
        /// How many sessions that location can host at once.
        capacity: usize,
    }

    let cases = vec![
        Case {
            label: "Anywhere fleet, its custom location",
            request: anywhere_fleet("anywhere-custom", &["custom-anywhere-a"]),
            location: Some("custom-anywhere-a"),
            capacity: 0,
        },
        Case {
            label: "Anywhere fleet, home Region",
            request: anywhere_fleet("anywhere-home", &["custom-anywhere-b"]),
            location: None,
            capacity: 0,
        },
        Case {
            label: "Anywhere fleet, second of two custom locations",
            request: anywhere_fleet(
                "anywhere-second",
                &["custom-anywhere-c", "custom-anywhere-d"],
            ),
            location: Some("custom-anywhere-d"),
            capacity: 0,
        },
        Case {
            label: "Anywhere fleet carrying a runtime configuration",
            request: {
                let mut request = anywhere_fleet("anywhere-runtime", &["custom-anywhere-e"]);
                request["RuntimeConfiguration"] = json!({
                    "ServerProcesses": [{
                        "LaunchPath": "/local/game/server",
                        "ConcurrentExecutions": 4,
                    }],
                });
                request
            },
            location: Some("custom-anywhere-e"),
            capacity: 0,
        },
        Case {
            label: "managed fleet, one process, home Region",
            request: managed_fleet("managed-one", 1, &[]),
            location: None,
            capacity: 1,
        },
        Case {
            label: "managed fleet, three processes, home Region",
            request: managed_fleet("managed-three", 3, &[]),
            location: None,
            capacity: 3,
        },
        Case {
            label: "managed fleet, two processes, remote location",
            request: managed_fleet("managed-remote", 2, &["custom-managed-a"]),
            location: Some("custom-managed-a"),
            capacity: 2,
        },
    ];

    for case in &cases {
        let gamelift = GameLift::new();
        let fleet_id = create_fleet(&gamelift, case.request.clone()).await;
        // A managed fleet is created NEW and is ACTIVE once its creation workflow comes
        // due; an Anywhere fleet is ACTIVE already.
        wait_for_due_transitions();

        let mut create = json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": 2});
        if let Some(location) = case.location {
            create["Location"] = json!(location);
        }

        for session in 0..case.capacity {
            let created = gamelift.ok("CreateGameSession", create.clone()).await;
            assert!(
                created["GameSession"]["GameSessionId"].is_string(),
                "case {}: session {session} of {} must be placed, got {created}",
                case.label,
                case.capacity
            );
        }

        let (status, code) = gamelift.err("CreateGameSession", create.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "FleetCapacityExceededException"),
            "case {}: the session past the location's {} server process(es) must be refused, \
             got {status} {code}",
            case.label,
            case.capacity
        );

        // The refused create wrote nothing: the fleet reports only the sessions its
        // processes actually host.
        let listed = session_count(&gamelift, &fleet_id, case.location).await;
        assert_eq!(
            listed,
            case.capacity,
            "case {}: DescribeGameSessions must report the {} hosted session(s), got {listed}",
            case.label,
            case.capacity
        );
    }
}

/// Capacity belongs to a fleet LOCATION: the processes running in one location are not
/// consumed by sessions placed in another, and each location refuses the session past its
/// own count.
#[tokio::test]
async fn server_process_capacity_is_counted_per_fleet_location() {
    let remote = ["custom-location-a", "custom-location-b"];
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, managed_fleet("per-location", 1, &remote)).await;
    wait_for_due_transitions();

    let created = gamelift
        .ok(
            "DescribeFleetAttributes",
            json!({"FleetIds": [&fleet_id]}),
        )
        .await;
    assert!(
        created["FleetAttributes"][0]["FleetId"].is_string(),
        "the fleet must be stored before its locations are placed on: got {created}"
    );

    // Every location of the fleet: the home Region the request was signed for, plus the
    // two remote ones. Each runs the one process the runtime configuration asks for.
    let home = "us-west-2";
    let locations = [home, remote[0], remote[1]];

    for location in locations {
        let create = json!({
            "FleetId": &fleet_id,
            "MaximumPlayerSessionCount": 2,
            "Location": location,
        });
        let placed = gamelift.ok("CreateGameSession", create.clone()).await;
        assert_eq!(
            placed["GameSession"]["Location"],
            json!(location),
            "case {location}: the session must be placed in the location asked for, got {placed}"
        );

        let (status, code) = gamelift.err("CreateGameSession", create).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "FleetCapacityExceededException"),
            "case {location}: the second session in a location running one process must be \
             refused, got {status} {code}"
        );
    }

    for location in locations {
        let listed = session_count(&gamelift, &fleet_id, Some(location)).await;
        assert_eq!(
            listed, 1,
            "case {location}: exactly the one session that location hosts must be reported, \
             got {listed}"
        );
    }
    let total = session_count(&gamelift, &fleet_id, None).await;
    assert_eq!(
        total,
        locations.len(),
        "the fleet must report one session per location, got {total}"
    );
}

/// The ports a fleet opens are opened for the whole fleet, so they bound its live
/// sessions across ALL of its locations: a fleet whose processes outnumber its ports hosts
/// only as many sessions as it has ports, wherever those sessions are placed, and every
/// session it does admit gets connection information.
///
/// The opened ranges come from `IpPermission$FromPort`'s doc: "only ports 22 and
/// 1026-60000 are valid."
#[tokio::test]
async fn the_ports_a_fleet_opens_bound_its_sessions_across_its_locations() {
    // (label, opened range, how many ports that range covers).
    let cases: [(&str, (i64, i64), usize); 3] = [
        ("the remote-access port alone", (22, 22), 1),
        ("two ports", (1026, 1027), 2),
        ("three ports", (7777, 7779), 3),
    ];
    let remote = ["custom-ports-a", "custom-ports-b"];

    for (label, (from, to), ports) in cases {
        let gamelift = GameLift::new();
        // Four processes per location over three locations: twelve processes, but only
        // `ports` ports to listen on.
        let mut request = managed_fleet(label, 4, &remote);
        request["EC2InboundPermissions"] = json!([{
            "FromPort": from,
            "ToPort": to,
            "IpRange": "0.0.0.0/0",
            "Protocol": "UDP",
        }]);
        let fleet_id = create_fleet(&gamelift, request).await;
        wait_for_due_transitions();

        // Place one session in a different location each time, so the ports are taken
        // across locations rather than within one.
        let locations = ["us-west-2", remote[0], remote[1]];
        for index in 0..ports {
            let created = gamelift
                .ok(
                    "CreateGameSession",
                    json!({
                        "FleetId": &fleet_id,
                        "MaximumPlayerSessionCount": 2,
                        "Location": locations[index % locations.len()],
                    }),
                )
                .await;
            assert!(
                created["GameSession"]["GameSessionId"].is_string(),
                "case {label}: session {index} of {ports} must be placed, got {created}"
            );
        }

        let (status, code) = gamelift
            .err(
                "CreateGameSession",
                json!({
                    "FleetId": &fleet_id,
                    "MaximumPlayerSessionCount": 2,
                    "Location": locations[ports % locations.len()],
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "FleetCapacityExceededException"),
            "case {label}: a fleet opening {ports} port(s) must refuse the session past them, \
             in any location, got {status} {code}"
        );

        // Every admitted session reaches ACTIVE with a port of its own: none is left
        // ACTIVATING for want of one.
        wait_for_due_transitions();
        let listed = gamelift
            .ok("DescribeGameSessions", json!({"FleetId": &fleet_id}))
            .await;
        let sessions = listed["GameSessions"]
            .as_array()
            .expect("DescribeGameSessions reports a list")
            .clone();
        assert_eq!(
            sessions.len(),
            ports,
            "case {label}: the fleet must host one session per opened port, got {sessions:?}"
        );
        let mut held: Vec<i64> = Vec::with_capacity(ports);
        for session in &sessions {
            assert_eq!(
                session["Status"],
                json!("ACTIVE"),
                "case {label}: an admitted session must reach ACTIVE, got {session}"
            );
            let port = session["Port"]
                .as_i64()
                .unwrap_or_else(|| panic!("case {label}: an ACTIVE session reports a port: {session}"));
            assert!(
                (from..=to).contains(&port),
                "case {label}: the port must come from the opened range {from}-{to}, got {port}"
            );
            held.push(port);
        }
        held.sort_unstable();
        let distinct = held.len();
        held.dedup();
        assert_eq!(
            held.len(),
            distinct,
            "case {label}: no two live sessions may hold the same port, got {held:?}"
        );
    }
}

/// The session a `FleetCapacityExceededException` refused never reaches the describe
/// path, however the describe asks for it: not by fleet, not by location, and not by its
/// own id.
#[tokio::test]
async fn a_refused_game_session_is_never_reported() {
    let location = "custom-refused-a";
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, anywhere_fleet("refused", &[location])).await;
    wait_for_due_transitions();

    let (status, body) = gamelift
        .call(
            "CreateGameSession",
            &json!({
                "FleetId": &fleet_id,
                "MaximumPlayerSessionCount": 10,
                "Location": location,
                "Name": "phantom",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a fleet with no available server process must refuse the session: got {status} {body}"
    );
    assert!(
        body.get("GameSession").is_none(),
        "the refused response must carry no game session, got {body}"
    );

    for (label, request) in [
        ("by fleet", json!({"FleetId": &fleet_id})),
        (
            "by location",
            json!({"FleetId": &fleet_id, "Location": location}),
        ),
        (
            "by status filter",
            json!({"FleetId": &fleet_id, "StatusFilter": "ACTIVATING"}),
        ),
    ] {
        let listed = gamelift.ok("DescribeGameSessions", request).await;
        let sessions = listed["GameSessions"]
            .as_array()
            .expect("DescribeGameSessions reports a list")
            .clone();
        assert!(
            sessions.is_empty(),
            "case {label}: no session may be reported for a fleet whose create was refused, \
             got {sessions:?}"
        );
        assert!(
            listed["NextToken"].is_null(),
            "case {label}: an empty page has no next token, got {listed}"
        );
    }

    // Nor does waiting for the clock-owed transitions conjure one up.
    wait_for_due_transitions();
    let listed = session_count(&gamelift, &fleet_id, Some(location)).await;
    assert_eq!(
        listed, 0,
        "no session may appear after the status changes the clock owes, got {listed}"
    );
}
