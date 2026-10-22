// SPDX-License-Identifier: CC0-1.0

//! The `FleetAttributes` contract, over the wire: what the service decides for a fleet
//! the request left unset, that create and describe report the SAME record, that every
//! ARN it mints names the owning account, that `UpdateFleetAttributes` is persisted, and
//! that a deleted fleet stops being reported.
//!
//! The harness (`GameLift`, `REGION`, `gamelift_arn`, `wait_for_due_transitions`) is the
//! one `hosting_loop` already drives the production dispatch path with.

use serde_json::{json, Value};

use super::hosting_loop::{
    anywhere_fleet, create_fleet, ec2_fleet, gamelift_arn, wait_for_due_transitions, GameLift,
    REGION,
};

/// The custom location every Anywhere fleet in this suite is deployed to, which
/// `CreateFleet` requires for that compute type.
const CUSTOM_LOCATION: &str = "custom-attributes";

/// The `FleetAttributes` `DescribeFleetAttributes` reports for one fleet.
async fn describe_fleet(gamelift: &GameLift, fleet_id: &str) -> Value {
    let described = gamelift
        .ok("DescribeFleetAttributes", json!({"FleetIds": [fleet_id]}))
        .await;
    described["FleetAttributes"][0].clone()
}

/// A member the request leaves unset is decided by the service, stored on the fleet, and
/// reported by every projection of it: `CreateFleet`'s response and `DescribeFleetAttributes`
/// carry the same record.
///
/// The values a row sets are drawn from the modelled enums — `CertificateType`
/// (DISABLED | GENERATED) and `PlayerGatewayMode` (DISABLED | ENABLED | REQUIRED), from
/// `smithy_query shape=CertificateType,PlayerGatewayMode` — so each enum is exercised on
/// values beyond the defaults, and the rows differ in compute type, in which members the
/// request sets and in the size of `MetricGroups`.
#[tokio::test]
async fn fleet_attributes_defaults_are_decided_once_and_reported_by_every_read() {
    struct Case {
        label: &'static str,
        request: Value,
        certificate_type: &'static str,
        metric_groups: Value,
        player_gateway_mode: &'static str,
    }

    let cases = vec![
        Case {
            label: "Anywhere, nothing set",
            request: anywhere_fleet("defaults-anywhere", &[CUSTOM_LOCATION]),
            certificate_type: "DISABLED",
            metric_groups: json!(["default"]),
            player_gateway_mode: "DISABLED",
        },
        Case {
            label: "EC2, nothing set",
            request: ec2_fleet("defaults-ec2"),
            certificate_type: "DISABLED",
            metric_groups: json!(["default"]),
            player_gateway_mode: "DISABLED",
        },
        Case {
            label: "EC2 asking for a generated certificate",
            request: json!({
                "Name": "generated-certificate",
                "ComputeType": "EC2",
                "ScriptId": "script-attributes",
                "EC2InstanceType": "r6i.large",
                "CertificateConfiguration": {"CertificateType": "GENERATED"},
            }),
            certificate_type: "GENERATED",
            metric_groups: json!(["default"]),
            player_gateway_mode: "DISABLED",
        },
        Case {
            label: "Anywhere naming its own metric group",
            request: json!({
                "Name": "own-metric-group",
                "ComputeType": "ANYWHERE",
                "Locations": [{"Location": CUSTOM_LOCATION}],
                "MetricGroups": ["team-metrics"],
            }),
            certificate_type: "DISABLED",
            metric_groups: json!(["team-metrics"]),
            player_gateway_mode: "DISABLED",
        },
        Case {
            label: "Anywhere naming no metric group at all",
            request: json!({
                "Name": "no-metric-group",
                "ComputeType": "ANYWHERE",
                "Locations": [{"Location": CUSTOM_LOCATION}],
                "MetricGroups": [],
            }),
            certificate_type: "DISABLED",
            metric_groups: json!([]),
            player_gateway_mode: "DISABLED",
        },
        Case {
            label: "EC2 with player gateway enabled",
            request: json!({
                "Name": "player-gateway-enabled",
                "ComputeType": "EC2",
                "BuildId": "build-attributes",
                "EC2InstanceType": "c5.large",
                "PlayerGatewayMode": "ENABLED",
            }),
            certificate_type: "DISABLED",
            metric_groups: json!(["default"]),
            player_gateway_mode: "ENABLED",
        },
        Case {
            label: "Anywhere with player gateway required and a certificate",
            request: json!({
                "Name": "player-gateway-required",
                "ComputeType": "ANYWHERE",
                "Locations": [{"Location": CUSTOM_LOCATION}],
                "PlayerGatewayMode": "REQUIRED",
                "CertificateConfiguration": {"CertificateType": "GENERATED"},
                "MetricGroups": ["required-metrics"],
            }),
            certificate_type: "GENERATED",
            metric_groups: json!(["required-metrics"]),
            player_gateway_mode: "REQUIRED",
        },
    ];

    let gamelift = GameLift::new();
    for case in cases {
        let label = case.label;
        let created = gamelift.ok("CreateFleet", case.request.clone()).await;
        let attributes = created["FleetAttributes"].clone();
        let fleet_id = attributes["FleetId"]
            .as_str()
            .expect("CreateFleet must report a FleetId")
            .to_string();

        assert_eq!(
            attributes["CertificateConfiguration"],
            json!({"CertificateType": case.certificate_type}),
            "case {label:?}: CreateFleet must report the fleet's certificate configuration, got {}",
            attributes["CertificateConfiguration"]
        );
        assert_eq!(
            attributes["MetricGroups"], case.metric_groups,
            "case {label:?}: CreateFleet must report the fleet's metric group, got {}",
            attributes["MetricGroups"]
        );
        assert_eq!(
            attributes["PlayerGatewayMode"],
            json!(case.player_gateway_mode),
            "case {label:?}: CreateFleet must report the fleet's player gateway mode, got {}",
            attributes["PlayerGatewayMode"]
        );
        // `FleetAttributes$TerminationTime` is "a time stamp indicating when this data
        // object was terminated", so a fleet that has not been terminated reports none:
        // the member is served from the record, never invented.
        assert!(
            attributes["TerminationTime"].is_null(),
            "case {label:?}: a fleet that has not been terminated must report no TerminationTime, got {}",
            attributes["TerminationTime"]
        );

        // The describe path serves the SAME stored record, member for member. Read it
        // past the span the fleet-creation workflow is due on, so the only member the
        // clock may have moved — `Status`, which a managed fleet starts `NEW` in and the
        // workflow leaves `ACTIVE` — has settled, and compare against the created record
        // with that settled status.
        wait_for_due_transitions();
        let mut expected = attributes.clone();
        expected["Status"] = json!("ACTIVE");
        let described = describe_fleet(&gamelift, &fleet_id).await;
        assert_eq!(
            described, expected,
            "case {label:?}: DescribeFleetAttributes must report the record CreateFleet reported, got {described}"
        );

        // And so does the unfiltered list, which reports every fleet of the account.
        let listed = gamelift.ok("DescribeFleetAttributes", json!({})).await;
        let from_list = listed["FleetAttributes"]
            .as_array()
            .expect("DescribeFleetAttributes must report a list")
            .iter()
            .find(|fleet| fleet["FleetId"] == json!(&fleet_id))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            from_list, expected,
            "case {label:?}: the unfiltered describe must report the same record, got {from_list}"
        );
    }
}

/// Every ARN this service mints carries the account that owns the resource in the
/// account segment, so a client can recover the owning account from the ARN and match it
/// against an IAM resource pattern.
///
/// The rows are the resource kinds the fleet path mints an ARN for; the account is
/// `engine::emulator_account_id()`, the one account every emulated response presents.
#[tokio::test]
async fn every_minted_arn_names_the_owning_account() {
    let gamelift = GameLift::new();
    let created = gamelift
        .ok(
            "CreateFleet",
            json!({
                "Name": "arn-fleet",
                "ComputeType": "EC2",
                "BuildId": "build-arn",
                "EC2InstanceType": "c5.large",
                "EC2InboundPermissions": [
                    {"FromPort": 7777, "ToPort": 7780, "IpRange": "0.0.0.0/0", "Protocol": "UDP"}
                ],
            }),
        )
        .await;
    let fleet = created["FleetAttributes"].clone();
    let fleet_id = fleet["FleetId"]
        .as_str()
        .expect("CreateFleet must report a FleetId")
        .to_string();
    let script_fleet = gamelift
        .ok(
            "CreateFleet",
            json!({
                "Name": "arn-script-fleet",
                "ComputeType": "EC2",
                "ScriptId": "script-arn",
                "EC2InstanceType": "c5.large",
            }),
        )
        .await;

    wait_for_due_transitions();
    let described = describe_fleet(&gamelift, &fleet_id).await;
    let session = gamelift
        .ok(
            "CreateGameSession",
            json!({"FleetId": &fleet_id, "MaximumPlayerSessionCount": 2}),
        )
        .await;
    let game_session_id = session["GameSession"]["GameSessionId"]
        .as_str()
        .expect("CreateGameSession must report a GameSessionId")
        .to_string();
    wait_for_due_transitions();
    let player = gamelift
        .ok(
            "CreatePlayerSession",
            json!({"GameSessionId": &game_session_id, "PlayerId": "player-arn"}),
        )
        .await;

    let cases: Vec<(&str, Value)> = vec![
        ("CreateFleet FleetArn", fleet["FleetArn"].clone()),
        ("CreateFleet BuildArn", fleet["BuildArn"].clone()),
        (
            "CreateFleet ScriptArn",
            script_fleet["FleetAttributes"]["ScriptArn"].clone(),
        ),
        (
            "DescribeFleetAttributes FleetArn",
            described["FleetArn"].clone(),
        ),
        (
            "CreateGameSession GameSessionId",
            json!(&game_session_id),
        ),
        (
            "CreateGameSession FleetArn",
            session["GameSession"]["FleetArn"].clone(),
        ),
        (
            "CreatePlayerSession FleetArn",
            player["PlayerSession"]["FleetArn"].clone(),
        ),
    ];

    let account = engine::emulator_account_id();
    for (label, arn) in cases {
        let arn = arn.as_str().unwrap_or("").to_string();
        let segments: Vec<&str> = arn.split(':').collect();
        assert!(
            segments.len() >= 6,
            "case {label:?}: an ARN must have the six ARN segments, got {arn:?}"
        );
        assert_eq!(
            segments.get(4).copied().unwrap_or_default(),
            account,
            "case {label:?}: the account segment must name the owning account, got {arn:?}"
        );
        assert_eq!(
            segments.get(2).copied().unwrap_or_default(),
            "gamelift",
            "case {label:?}: the service segment must be gamelift, got {arn:?}"
        );
        assert_eq!(
            segments.get(3).copied().unwrap_or_default(),
            REGION,
            "case {label:?}: the region segment must be the signed-for region, got {arn:?}"
        );
    }

    // The documented format, rebuilt from the id, is exactly what the fleet reports.
    assert_eq!(
        fleet["FleetArn"],
        json!(gamelift_arn(&format!("fleet/{fleet_id}"))),
        "the fleet ARN must be the documented format with the account populated, got {}",
        fleet["FleetArn"]
    );
}

/// `UpdateFleetAttributes` writes the members the request names into the fleet record and
/// leaves every other member as it was, so the next describe serves the updated fleet.
#[tokio::test]
async fn update_fleet_attributes_is_persisted_on_the_fleet_record() {
    struct Case {
        label: &'static str,
        update: Value,
        /// The members the update must change, as they must read back.
        expected: Vec<(&'static str, Value)>,
    }

    let cases = vec![
        Case {
            label: "description only",
            update: json!({"Description": "updated by the test"}),
            expected: vec![("Description", json!("updated by the test"))],
        },
        Case {
            label: "protection policy FullProtection",
            update: json!({"NewGameSessionProtectionPolicy": "FullProtection"}),
            expected: vec![("NewGameSessionProtectionPolicy", json!("FullProtection"))],
        },
        Case {
            label: "protection policy NoProtection",
            update: json!({"NewGameSessionProtectionPolicy": "NoProtection"}),
            expected: vec![("NewGameSessionProtectionPolicy", json!("NoProtection"))],
        },
        Case {
            label: "name only",
            update: json!({"Name": "renamed-by-the-test"}),
            expected: vec![("Name", json!("renamed-by-the-test"))],
        },
        Case {
            label: "a new metric group",
            update: json!({"MetricGroups": ["moved-metrics"]}),
            expected: vec![("MetricGroups", json!(["moved-metrics"]))],
        },
        Case {
            label: "out of every metric group",
            update: json!({"MetricGroups": []}),
            expected: vec![("MetricGroups", json!([]))],
        },
        Case {
            label: "a resource creation limit policy",
            update: json!({
                "ResourceCreationLimitPolicy": {
                    "NewGameSessionsPerCreator": 4,
                    "PolicyPeriodInMinutes": 10,
                }
            }),
            expected: vec![(
                "ResourceCreationLimitPolicy",
                json!({"NewGameSessionsPerCreator": 4, "PolicyPeriodInMinutes": 10}),
            )],
        },
        Case {
            label: "an Anywhere configuration",
            update: json!({"AnywhereConfiguration": {"Cost": "0.5"}}),
            expected: vec![("AnywhereConfiguration", json!({"Cost": "0.5"}))],
        },
        Case {
            label: "several members at once",
            update: json!({
                "Description": "several at once",
                "NewGameSessionProtectionPolicy": "FullProtection",
                "MetricGroups": ["several-metrics"],
            }),
            expected: vec![
                ("Description", json!("several at once")),
                ("NewGameSessionProtectionPolicy", json!("FullProtection")),
                ("MetricGroups", json!(["several-metrics"])),
            ],
        },
        Case {
            label: "no mutable member at all",
            update: json!({}),
            expected: vec![],
        },
    ];

    let gamelift = GameLift::new();
    for case in cases {
        let label = case.label;
        let fleet_id =
            create_fleet(&gamelift, anywhere_fleet("update-target", &[CUSTOM_LOCATION])).await;
        let before = describe_fleet(&gamelift, &fleet_id).await;

        let mut request = case.update.clone();
        request["FleetId"] = json!(&fleet_id);
        let updated = gamelift.ok("UpdateFleetAttributes", request).await;
        assert_eq!(
            updated,
            json!({
                "FleetId": &fleet_id,
                "FleetArn": gamelift_arn(&format!("fleet/{fleet_id}")),
            }),
            "case {label:?}: UpdateFleetAttributes must report the updated fleet's identifiers, got {updated}"
        );

        let after = describe_fleet(&gamelift, &fleet_id).await;
        for (member, value) in &case.expected {
            assert_eq!(
                &after[member], value,
                "case {label:?}: {member} must read back as the update set it, got {}",
                after[member]
            );
        }
        // Every member the update did not name is untouched.
        let changed: Vec<&str> = case.expected.iter().map(|(member, _)| *member).collect();
        let mut expected_record = before.clone();
        for (member, value) in &case.expected {
            expected_record[member] = value.clone();
        }
        assert_eq!(
            after, expected_record,
            "case {label:?}: the update must change only {changed:?}, got {after}"
        );
    }
}

/// An update is refused when there is no fleet to update, or when the fleet it names is
/// being torn down.
#[tokio::test]
async fn update_fleet_attributes_refuses_an_absent_or_deleting_fleet() {
    let gamelift = GameLift::new();

    let (status, code) = gamelift
        .err(
            "UpdateFleetAttributes",
            json!({"FleetId": "fleet-does-not-exist", "Description": "nope"}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "NotFoundException"),
        "an update of an unknown fleet must be refused: got {status} {code}"
    );

    // A managed EC2 fleet passes through DELETING while its instances are shut down.
    let fleet_id = create_fleet(&gamelift, ec2_fleet("deleting-update-target")).await;
    gamelift
        .ok("DeleteFleet", json!({"FleetId": &fleet_id}))
        .await;
    let (status, code) = gamelift
        .err(
            "UpdateFleetAttributes",
            json!({"FleetId": &fleet_id, "Description": "too late"}),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "InvalidFleetStatusException"),
        "an update of a fleet being deleted must be refused: got {status} {code}"
    );
}

/// `DeleteFleet` "deletes all resources and information related to a fleet and shuts down
/// any currently running fleet instances": a fleet with no instances for the service to
/// shut down is gone as soon as the call returns, while one whose instances it manages
/// passes through `DELETING` — carrying its termination time — and leaves state when the
/// deletion completes. Either way, a fleet that has left state is reported by no describe.
#[tokio::test]
async fn a_deleted_fleet_stops_being_reported_once_its_deletion_completes() {
    struct Case {
        label: &'static str,
        request: Value,
        /// Whether the deletion still has instances to shut down, so the fleet is
        /// reported as `DELETING` before it leaves state.
        passes_through_deleting: bool,
    }

    let cases = vec![
        Case {
            label: "Anywhere fleet: no managed instances",
            request: anywhere_fleet("delete-anywhere", &[CUSTOM_LOCATION]),
            passes_through_deleting: false,
        },
        Case {
            label: "managed EC2 fleet",
            request: ec2_fleet("delete-ec2"),
            passes_through_deleting: true,
        },
    ];

    for case in cases {
        let label = case.label;
        let gamelift = GameLift::new();
        let fleet_id = create_fleet(&gamelift, case.request.clone()).await;
        gamelift
            .ok("DeleteFleet", json!({"FleetId": &fleet_id}))
            .await;

        if case.passes_through_deleting {
            let described = describe_fleet(&gamelift, &fleet_id).await;
            assert_eq!(
                described["Status"],
                json!("DELETING"),
                "case {label:?}: the fleet must be reported as DELETING while it is torn down, got {described}"
            );
            assert!(
                described["TerminationTime"].is_number(),
                "case {label:?}: a fleet being deleted must carry its termination time, got {described}"
            );
            wait_for_due_transitions();
        }

        let listed = gamelift.ok("DescribeFleetAttributes", json!({})).await;
        assert_eq!(
            listed["FleetAttributes"],
            json!([]),
            "case {label:?}: a deleted fleet must be reported by no describe, got {}",
            listed["FleetAttributes"]
        );
        // Naming a fleet that has left state selects nothing: "Attribute objects are
        // returned only for fleets that currently exist."
        let named = gamelift
            .ok("DescribeFleetAttributes", json!({"FleetIds": [&fleet_id]}))
            .await;
        assert_eq!(
            named["FleetAttributes"],
            json!([]),
            "case {label:?}: a describe that names a fleet that has left state must report an empty page, got {named}"
        );
    }
}
