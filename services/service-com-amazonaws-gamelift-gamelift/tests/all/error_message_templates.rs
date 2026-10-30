// SPDX-License-Identifier: CC0-1.0

//! The TEXT of four refusals, over the wire, each asserted over a table that spans its
//! class rather than the one request a recording happened to carry.
//!
//! A refusal's `Message` is the service's own text for that error class, and it has
//! exactly the slots that text has: a value the REQUEST supplied where the sentence names
//! one, and nothing at all where it names none. So
//!
//! * a build no operation can find is reported as the identifier the caller asked for,
//!   whichever operation named it and whichever of the two forms `BuildIdOrArn` accepts it
//!   took;
//! * a routing strategy missing the member its own type requires names that type and that
//!   member, and carries nothing of the request;
//! * a game session that cannot be placed because the fleet has no server process both
//!   active and available carries no fleet, location or count — the text has no slot for
//!   one, and a generated id in it would move with every environment;
//! * a custom location whose name is taken names the resource kind and the requested name,
//!   and adds no clause of its own.
//!
//! The fifth refusal of this class — a rule set body the rule language version cannot be
//! read out of — is already asserted over a wider table by
//! `refusal_message_wording.rs::an_unreadable_rule_set_body_reports_the_version_extraction_failure`,
//! so it is not repeated here.
//!
//! Every suite keeps a control row that is ACCEPTED, so the refusals are about the request
//! and not a blanket answer, and reads the surviving resource back through its own
//! describe path.
//!
//! The harness (`GameLift`, `error_code`, `error_message`, `create_fleet`, …) is the one
//! `hosting_loop.rs` defines and drives the production dispatch path with.

use serde_json::{json, Value};

use crate::hosting_loop::{
    anywhere_fleet, create_fleet, ec2_fleet, error_code, error_message, gamelift_arn,
    wait_for_due_transitions, GameLift,
};

/// The longest `BuildId` a request may carry: `BuildIdOrArn`
/// (`smithy_query operation=DescribeBuild`) is len 1..512.
const MAX_BUILD_ID_LENGTH: usize = 512;

/// The `OperatingSystem` the control builds below declare, from the model's own enum
/// (`smithy_query shape=OperatingSystem`).
const OPERATING_SYSTEM: &str = "AMAZON_LINUX_2023";

/// The text a `CreateGameSession` that cannot reserve a server process is refused with.
const PROCESS_RESERVATION_FAILURE: &str =
    "Unable to reserve a process(No active and available server processes found on the fleet)";

/// A managed EC2 fleet request whose runtime configuration asks for `processes` concurrent
/// server processes, and which opens exactly `ports` ports for them to listen on — the
/// second bound a placement can hit. `IpPermission$FromPort`'s own doc bounds the range:
/// "only ports 22 and 1026-60000 are valid."
///
/// `server_process_capacity.rs` builds a managed fleet too, parameterised by the LOCATIONS
/// it deploys to rather than by the ports it opens; this one varies the ports, which is
/// what the rows below need.
fn port_bounded_fleet(name: &str, processes: i64, ports: i64) -> Value {
    let mut request = ec2_fleet(name);
    request["RuntimeConfiguration"] = json!({
        "ServerProcesses": [{
            "LaunchPath": "/local/game/server",
            "ConcurrentExecutions": processes,
        }],
    });
    request["EC2InboundPermissions"] = json!([{
        "FromPort": 1026,
        "ToPort": 1026 + ports - 1,
        "IpRange": "0.0.0.0/0",
        "Protocol": "UDP",
    }]);
    request
}

/// Assert `operation` refuses `request` with `code` at HTTP 400 and exactly `expected` as
/// its `Message`, and answer the `Message` the service actually sent.
async fn assert_refused(
    gamelift: &GameLift,
    operation: &str,
    request: Value,
    code: &str,
    expected: &str,
    case: &str,
) -> String {
    let (status, body) = gamelift.call(operation, &request).await;
    let message = error_message(&body);
    let got = format!("{}|{message}", error_code(&body));
    let wanted = format!("{code}|{expected}");
    assert_eq!(
        (status, got.as_str()),
        (400, wanted.as_str()),
        "case {case:?} on {operation}: expected HTTP 400 with {wanted:?}, got HTTP {status} \
         with {got:?}"
    );
    message
}

/// Every operation that names ONE build reports a build it cannot find as the identifier
/// the REQUEST asked for: the resource kind, that identifier verbatim in parentheses, and
/// the same sentence whichever operation was called.
///
/// The rows span the forms `BuildIdOrArn` accepts (`^(build-\S+|arn:.*:build\/build-\S+)$`)
/// and its modelled length bound (len 1..512), including an ARN for an id that IS stored
/// but in another account — which proves the identifier is copied from the request rather
/// than from anything the service resolved.
#[tokio::test]
async fn a_build_that_cannot_be_found_is_reported_as_the_identifier_asked_for() {
    let gamelift = GameLift::new();
    let created = gamelift
        .ok(
            "CreateBuild",
            json!({"Name": "the-stored-build", "OperatingSystem": OPERATING_SYSTEM}),
        )
        .await;
    let build = created["Build"].clone();
    let stored_id = build["BuildId"]
        .as_str()
        .expect("CreateBuild must report a BuildId")
        .to_string();

    let identifiers = [
        ("a bare id no build carries", "build-does-not-exist".to_string()),
        (
            "a UUID-shaped id no build carries",
            "build-00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "an ARN no build carries",
            gamelift_arn("build/build-absent-by-arn"),
        ),
        (
            "an ARN in another account for a stored id",
            format!("arn:aws:gamelift:us-west-2:999999999999:build/{stored_id}"),
        ),
        (
            "the longest identifier the model allows",
            format!(
                "build-{}",
                "b".repeat(MAX_BUILD_ID_LENGTH - "build-".len())
            ),
        ),
    ];

    // Every operation that acts on ONE named build, with the rest of each request held
    // valid so the refusal is about the build it could not find.
    type BuildRequest = fn(&str) -> Value;
    let operations: [(&str, BuildRequest); 3] = [
        ("DescribeBuild", |id| json!({"BuildId": id})),
        ("UpdateBuild", |id| {
            json!({"BuildId": id, "Name": "a new name", "Version": "2.0"})
        }),
        ("RequestUploadCredentials", |id| json!({"BuildId": id})),
    ];

    for (label, identifier) in &identifiers {
        for (operation, request) in &operations {
            let expected = format!("Build ({identifier}) not found");
            assert_refused(
                &gamelift,
                operation,
                request(identifier),
                "NotFoundException",
                &expected,
                label,
            )
            .await;
        }
    }

    // The control: the build that IS stored is still described and still updated, so the
    // refusals above are about the identifier each request named.
    let described = gamelift
        .ok("DescribeBuild", json!({"BuildId": &stored_id}))
        .await;
    assert_eq!(
        described["Build"], build,
        "the stored build must read back as created, got {}",
        described["Build"]
    );
    let updated = gamelift
        .ok(
            "UpdateBuild",
            json!({"BuildId": &stored_id, "Name": "renamed"}),
        )
        .await;
    assert_eq!(
        updated["Build"]["Name"],
        json!("renamed"),
        "the stored build must take the update, got {updated}"
    );
}

/// A routing strategy that does not carry the member its own type requires is refused with
/// the sentence naming that TYPE and that MEMBER — the same text for every such request,
/// with nothing of the request interpolated into it.
///
/// The rows span both `RoutingStrategyType` values (`smithy_query shape=RoutingStrategyType`
/// — `SIMPLE`, `TERMINAL`), each with and without the OTHER type's member set, on the
/// create path and on the update path alike, since both read the one strategy the same way.
#[tokio::test]
async fn a_routing_strategy_missing_its_required_member_names_the_type_and_the_member() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(&gamelift, ec2_fleet("alias-target")).await;
    wait_for_due_transitions();
    let stored = gamelift
        .ok(
            "CreateAlias",
            json!({
                "Name": "the-stored-alias",
                "RoutingStrategy": {"Type": "SIMPLE", "FleetId": &fleet_id},
            }),
        )
        .await;
    let alias_id = stored["Alias"]["AliasId"]
        .as_str()
        .expect("CreateAlias must report an AliasId")
        .to_string();

    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "SIMPLE with no FleetId",
            json!({"Type": "SIMPLE"}),
            "SIMPLE RoutingStrategy must contain a valid FleetId.",
        ),
        (
            "SIMPLE carrying only the terminal message",
            json!({"Type": "SIMPLE", "Message": "retired"}),
            "SIMPLE RoutingStrategy must contain a valid FleetId.",
        ),
        (
            "TERMINAL with no Message",
            json!({"Type": "TERMINAL"}),
            "TERMINAL RoutingStrategy must contain a valid Message.",
        ),
        (
            "TERMINAL carrying only a fleet id",
            json!({"Type": "TERMINAL", "FleetId": &fleet_id}),
            "TERMINAL RoutingStrategy must contain a valid Message.",
        ),
    ];

    for (index, (label, strategy, expected)) in rows.iter().enumerate() {
        // The create path: the alias name differs per row, and none of it reaches the text.
        let name = format!("refused-alias-{index}");
        let created_message = assert_refused(
            &gamelift,
            "CreateAlias",
            json!({"Name": &name, "RoutingStrategy": strategy}),
            "InvalidRequestException",
            expected,
            label,
        )
        .await;

        // The update path reads the same strategy, so it refuses it the same way.
        let updated_message = assert_refused(
            &gamelift,
            "UpdateAlias",
            json!({"AliasId": &alias_id, "RoutingStrategy": strategy}),
            "InvalidRequestException",
            expected,
            label,
        )
        .await;

        // Nothing of either request is in the text the service SENT: not the name the
        // create chose, not the alias the update named, not the fleet a row carried.
        for message in [&created_message, &updated_message] {
            for value in [name.as_str(), alias_id.as_str(), fleet_id.as_str()] {
                assert!(
                    !message.contains(value),
                    "case {label:?}: the refusal must carry no value from the request, found \
                     {value:?} in {message:?}"
                );
            }
        }
    }

    // The control: both complete strategies are accepted, and the stored alias still reads
    // back through its own describe path with the strategy the last update wrote.
    let accepted = gamelift
        .ok(
            "CreateAlias",
            json!({
                "Name": "terminal-alias",
                "RoutingStrategy": {"Type": "TERMINAL", "Message": "This version is retired."},
            }),
        )
        .await;
    assert_eq!(
        accepted["Alias"]["RoutingStrategy"]["Message"],
        json!("This version is retired."),
        "a complete terminal strategy must be stored, got {accepted}"
    );
    let described = gamelift
        .ok("DescribeAlias", json!({"AliasId": &alias_id}))
        .await;
    assert_eq!(
        described["Alias"]["RoutingStrategy"],
        json!({"Type": "SIMPLE", "FleetId": &fleet_id}),
        "the refused updates must leave the stored alias as created, got {described}"
    );
}

/// A game session that cannot be placed because the fleet has no server process both
/// active and available is refused with the one fixed text, carrying no fleet id, no
/// location name and no count — whichever way the fleet ran out of processes.
///
/// The rows span the two `ComputeType` values (`smithy_query shape=ComputeType` — `EC2`,
/// `ANYWHERE`), a fleet's home and remote locations, the number of processes a managed
/// fleet runs, and the two bounds a placement can hit: the processes of one location, and
/// the ports the whole fleet opens.
#[tokio::test]
async fn a_session_that_cannot_reserve_a_process_reports_the_fixed_text() {
    struct Case {
        label: &'static str,
        fleet: Value,
        /// The `Location` the creates name, or `None` for the fleet's home Region.
        location: Option<&'static str>,
        /// How many sessions the fleet admits before it runs out of processes or ports.
        admitted: usize,
    }

    let cases = vec![
        Case {
            label: "an Anywhere fleet, which runs no process of its own",
            fleet: anywhere_fleet("reserve-anywhere", &["custom-reserve-a"]),
            location: Some("custom-reserve-a"),
            admitted: 0,
        },
        Case {
            label: "an Anywhere fleet, in its home Region",
            fleet: anywhere_fleet("reserve-anywhere-home", &["custom-reserve-b"]),
            location: None,
            admitted: 0,
        },
        Case {
            label: "a managed fleet whose one process is already hosting",
            fleet: port_bounded_fleet("reserve-one", 1, 8),
            location: None,
            admitted: 1,
        },
        Case {
            label: "a managed fleet whose three processes are all hosting",
            fleet: port_bounded_fleet("reserve-three", 3, 8),
            location: None,
            admitted: 3,
        },
        Case {
            label: "a managed fleet whose single opened port is held",
            fleet: port_bounded_fleet("reserve-ports", 4, 1),
            location: None,
            admitted: 1,
        },
    ];

    for case in &cases {
        let label = case.label;
        let gamelift = GameLift::new();
        let fleet_id = create_fleet(&gamelift, case.fleet.clone()).await;
        wait_for_due_transitions();

        let mut create = json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": 2});
        if let Some(location) = case.location {
            create["Location"] = json!(location);
        }
        for session in 0..case.admitted {
            let created = gamelift.ok("CreateGameSession", create.clone()).await;
            assert!(
                created["GameSession"]["GameSessionId"].is_string(),
                "case {label:?}: session {session} must be placed, got {created}"
            );
        }

        let (status, body) = gamelift.call("CreateGameSession", &create).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "FleetCapacityExceededException"),
            "case {label:?}: the session past the fleet's processes must be refused, got \
             {status} {code} with body {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, PROCESS_RESERVATION_FAILURE,
            "case {label:?}: expected Message {PROCESS_RESERVATION_FAILURE:?}, got {message:?}"
        );
        // The text has no slot for the fleet, the location or a count, so none of them is
        // in it: a message carrying a generated id could not be matched on.
        let mut forbidden = vec![fleet_id.clone()];
        if let Some(location) = case.location {
            forbidden.push(location.to_string());
        }
        for value in &forbidden {
            assert!(
                !message.contains(value),
                "case {label:?}: the refusal must name no fleet or location, found {value:?} \
                 in {message:?}"
            );
        }
    }
}

/// A custom location whose name is already taken is refused with the sentence naming the
/// resource kind and the name the REQUEST asked for, and nothing else.
///
/// The rows span what `CustomInputLocationStringModel` allows (`^custom-[A-Za-z0-9\-]+$`,
/// len 8..64): both length bounds, digits, upper case and inner hyphens — and the repeat
/// create differs in shape from the first one, which does not change the text.
#[tokio::test]
async fn a_location_name_already_taken_names_the_kind_and_the_requested_name() {
    let gamelift = GameLift::new();

    let names = [
        ("the shortest name the model allows", "custom-a".to_string()),
        (
            "the longest name the model allows",
            format!("custom-{}", "a".repeat(57)),
        ),
        ("digits and upper case", "custom-WEST-2a".to_string()),
        ("several inner hyphens", "custom-rack-7-row-2".to_string()),
    ];

    for (label, name) in &names {
        let created = gamelift
            .ok("CreateLocation", json!({"LocationName": name}))
            .await;
        assert_eq!(
            created["Location"]["LocationName"],
            json!(name),
            "case {label:?}: the first create must store the location, got {created}"
        );

        let expected = format!("A Location with name: {name} already exists.");
        // The repeat create, bare and then carrying tags: the conflict is the name, so the
        // text is the same either way.
        for (shape, request) in [
            ("bare", json!({"LocationName": name})),
            (
                "with tags",
                json!({
                    "LocationName": name,
                    "Tags": [{"Key": "owner", "Value": "liveops"}],
                }),
            ),
        ] {
            assert_refused(
                &gamelift,
                "CreateLocation",
                request,
                "ConflictException",
                &expected,
                &format!("{label} ({shape})"),
            )
            .await;
        }
    }

    // The control: every name is stored exactly once, and the refused repeats changed
    // nothing — the list path serves the same records.
    let listed = gamelift
        .ok("ListLocations", json!({"Filters": ["CUSTOM"], "Limit": 200}))
        .await;
    let reported: Vec<&str> = listed["Locations"]
        .as_array()
        .expect("ListLocations must report a list")
        .iter()
        .map(|location| location["LocationName"].as_str().unwrap_or_default())
        .collect();
    for (label, name) in &names {
        let held = reported.iter().filter(|listed| *listed == name).count();
        assert_eq!(
            held, 1,
            "case {label:?}: the location must be stored exactly once, got {held} in {listed}"
        );
    }
}
