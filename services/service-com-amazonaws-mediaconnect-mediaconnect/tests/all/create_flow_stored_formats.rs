// SPDX-License-Identifier: CC0-1.0

//! What a CREATE may store: the members whose documented format the wire shape leaves
//! free-form are checked before the record is written, so no read can report a value the
//! service would never have stored.
//!
//! Two rules are under test, each over the whole class of values rather than one request:
//!
//! 1. A maintenance window's start hour is "UTC time when the maintenance will happen.
//!    Use 24-hour HH:MM format. Minutes must be 00. Example: 13:00."
//!    (`smithy_query(query="shape",shape="AddMaintenance")`, the same sentence
//!    `UpdateMaintenance` carries) — so the create path and the update path answer the
//!    SAME verdict for the same value, and a refused create stores nothing.
//! 2. An Availability Zone a request names is "limited to the Availability Zones within
//!    the current Region" (`smithy_query(query="operation",operation="CreateFlow")`, input
//!    member `AvailabilityZone`) / "must be a valid Availability Zone for the region
//!    specified by regionName, or the current region if no regionName is provided"
//!    (`CreateRouterInputRequest$AvailabilityZone`) — so every create that stores a zone
//!    refuses one that is not of the Region the request names, and a request that names
//!    none is given a zone of that Region.
//!
//! Requests go through the same `MultiServiceRouter` the emulator's front door uses, so
//! what is asserted here is what a MediaConnect client sees on the wire.

use serde_json::{json, Value};

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};
use crate::router_input_restart::union;
use crate::router_outputs::minted_network_interface;

/// A minimal valid CreateFlow body: "the request must include one source."
fn flow_body(name: &str, _interface_arn: &str) -> Value {
    json!({
        "name": name,
        "source": { "name": "src", "protocol": "rtp", "whitelistCidr": "10.0.0.0/16" }
    })
}

/// A minimal valid CreateRouterInput body, on the router network interface `interface_arn` —
/// a configuration must name an interface this account holds.
fn router_input_body(name: &str, interface_arn: &str) -> Value {
    json!({
        "name": name,
        "configuration": union(
            "Standard",
            "standard",
            json!({
                "networkInterfaceArn": interface_arn,
                "protocol": "RTP",
                "protocolConfiguration": union("Rtp", "rtp", json!({ "port": 5000 })),
            }),
        ),
        "maximumBitrate": 20_000_000_i64,
        "routingScope": "REGIONAL",
        "tier": "INPUT_20",
    })
}

/// A minimal valid CreateRouterOutput body, on the router network interface
/// `interface_arn` — a configuration must name an interface this account holds.
fn router_output_body(name: &str, interface_arn: &str) -> Value {
    json!({
        "name": name,
        "configuration": union(
            "Standard",
            "standard",
            json!({
                "networkInterfaceArn": interface_arn,
                "protocolConfiguration": union(
                    "Rtp",
                    "rtp",
                    json!({ "destinationAddress": "198.51.100.10", "destinationPort": 5000 }),
                ),
            }),
        ),
        "maximumBitrate": 20_000_000_i64,
        "routingScope": "REGIONAL",
        "tier": "OUTPUT_20",
    })
}

/// One create path that stores an Availability Zone: how to build a minimal valid body
/// (from a name and the minted router network interface a router input's or output's
/// configuration names), the response member the created resource arrives in, the member its
/// identity is reported in, and the read-back URI prefix that identity goes on.
struct CreatePath {
    operation: &'static str,
    uri: &'static str,
    body: fn(&str, &str) -> Value,
    member: &'static str,
    identity: &'static str,
    read_prefix: &'static str,
}

/// Every operation in this service that STORES an `AvailabilityZone` — the rule holds for
/// each of them, so each is driven over the same table.
fn create_paths() -> Vec<CreatePath> {
    vec![
        CreatePath {
            operation: "CreateFlow",
            uri: "/v1/flows",
            body: flow_body,
            member: "flow",
            identity: "flowArn",
            read_prefix: "/v1/flows/",
        },
        CreatePath {
            operation: "CreateRouterInput",
            uri: "/v1/routerInput",
            body: router_input_body,
            member: "routerInput",
            identity: "arn",
            read_prefix: "/v1/routerInput/",
        },
        CreatePath {
            operation: "CreateRouterOutput",
            uri: "/v1/routerOutput",
            body: router_output_body,
            member: "routerOutput",
            identity: "arn",
            read_prefix: "/v1/routerOutput/",
        },
    ]
}

/// The `MaintenanceStartHour` class, as `(case, hour, accepted)`.
///
/// The documented format is "24-hour HH:MM format. Minutes must be 00. Example: 13:00."
/// (`smithy_query(query="shape",shape="AddMaintenance")`), so the rows sit on each
/// published bound of that format, one step past it, and on forms that are not a time at
/// all.
fn hour_rows() -> Vec<(&'static str, &'static str, bool)> {
    vec![
        ("the bottom of the hour range", "00:00", true),
        ("the top of the hour range", "23:00", true),
        ("the documented example hour", "13:00", true),
        ("a single-digit hour, zero padded", "09:00", true),
        ("one hour past the top of the range", "24:00", false),
        ("an hour far past the top, minutes not 00", "25:77", false),
        ("minutes that are not 00", "13:30", false),
        ("an hour that is not zero padded", "7:00", false),
        ("seconds appended to the documented form", "13:00:00", false),
        ("a time with no separator", "1300", false),
        ("an hour that is not a time at all", "noon", false),
        ("no hour at all", "", false),
    ]
}

/// The Availability Zone class, as `(case, zone, accepted)`. `None` is a request that
/// names no zone, which is given a zone of its own Region.
///
/// AWS names an Availability Zone by its Region followed by one lowercase letter, so the
/// rows differ in the Region the zone belongs to, in the zone letter (including letters
/// the suites elsewhere never use) and in the FORM of the suffix.
fn zone_rows() -> Vec<(&'static str, Option<&'static str>, bool)> {
    vec![
        ("no zone named", None, true),
        ("the first zone of the Region", Some("us-east-1a"), true),
        ("another zone of the Region", Some("us-east-1c"), true),
        ("a zone letter this suite alone uses", Some("us-east-1f"), true),
        ("a zone of another Region", Some("eu-west-1b"), false),
        ("a zone of a third Region", Some("ap-south-1a"), false),
        ("the Region itself, with no zone letter", Some("us-east-1"), false),
        ("two zone letters", Some("us-east-1ab"), false),
        ("an uppercase zone letter", Some("us-east-1A"), false),
        ("a zone of a Region this Region is a prefix of", Some("us-east-10a"), false),
        ("not a zone at all", Some("not-a-zone"), false),
        ("an empty zone", Some(""), false),
    ]
}

/// A name no other case uses, derived from the row's case so a failure names the row.
fn case_name(prefix: &str, case: &str) -> String {
    let slug: String = case
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("{prefix}-{slug}")
}

/// Rule 1: CreateFlow refuses a maintenance window whose start hour is not in the
/// documented format, stores nothing for it, and accepts exactly the hours the UPDATE path
/// accepts — the same value cannot be valid on one path and invalid on the other.
#[tokio::test]
async fn create_flow_stores_only_a_documented_maintenance_start_hour() {
    let emulator = Emulator::new();

    // The flow the parity half of each row updates.
    let (status, _, created) = emulator
        .create_flow(flow_body("flow-maintenance-parity", ""))
        .await;
    assert_eq!(status, 201, "the parity flow is created, got {status}: {created}");
    let parity_arn = created["flow"]["flowArn"]
        .as_str()
        .expect("the created flow carries its ARN")
        .to_string();

    for (case, hour, accepted) in hour_rows() {
        let name = case_name("f-maint", case);
        let mut body = flow_body(&name, "");
        body["maintenance"] = json!({ "maintenanceDay": "Monday", "maintenanceStartHour": hour });

        let (status, code, answered) = emulator.create_flow(body.clone()).await;
        let (expected_status, expected_code) = if accepted {
            (201, None)
        } else {
            (400, Some("BadRequestException"))
        };
        assert_eq!(
            (status, code.as_deref()),
            (expected_status, expected_code),
            "case {case:?} ({hour:?}): expected {expected_status} {expected_code:?}, got \
             {status} {code:?}: {answered}"
        );

        if accepted {
            // What was stored is what a later read reports.
            let flow_arn = answered["flow"]["flowArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the create answers an ARN: {answered}"))
                .to_string();
            let (status, _, read) = emulator.describe_flow(&flow_arn).await;
            assert_eq!(
                (status, read["flow"]["maintenance"]["maintenanceStartHour"].as_str()),
                (200, Some(hour)),
                "case {case:?}: DescribeFlow reports the stored start hour, got {read}"
            );
            assert_eq!(
                read["flow"]["maintenance"]["maintenanceDay"].as_str(),
                Some("Monday"),
                "case {case:?}: the stored window keeps its day, got {read}"
            );
        } else {
            // A refused create stores NOTHING: no flow of that name exists.
            assert!(
                answered["message"].as_str().is_some_and(|message| message
                    .starts_with(mc::VALIDATION_FAILED_PREFIX)
                    && message.contains("MaintenanceStartHour")),
                "case {case:?}: the refusal names the member it refused, got {answered}"
            );
            let (status, _, listed) = emulator.list_flows("").await;
            let stored: Vec<&str> = listed["flows"]
                .as_array()
                .map(|flows| {
                    flows
                        .iter()
                        .filter_map(|flow| flow["name"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                (status, stored.contains(&name.as_str())),
                (200, false),
                "case {case:?}: a refused create stores no flow, got {listed}"
            );
        }

        // The same value on the UPDATE path answers the same verdict: one rule, both
        // paths.
        let (update_status, update_code, updated) = emulator
            .send(
                "PUT",
                &format!("/v1/flows/{parity_arn}"),
                Some(json!({ "maintenance": { "maintenanceStartHour": hour } })),
            )
            .await;
        let (expected_status, expected_code) = if accepted {
            (202, None)
        } else {
            (400, Some("BadRequestException"))
        };
        assert_eq!(
            (update_status, update_code.as_deref()),
            (expected_status, expected_code),
            "case {case:?} ({hour:?}): UpdateFlow answers the same verdict as the create, \
             expected {expected_status} {expected_code:?}, got {update_status} \
             {update_code:?}: {updated}"
        );
    }
}

/// Rule 2: every create that stores an Availability Zone stores only a zone of the Region
/// the request names, reports it back, and refuses anything else with nothing stored.
#[tokio::test]
async fn a_create_stores_only_an_availability_zone_of_its_region() {
    let emulator = Emulator::new();
    // The router network interface a router input's or output's configuration names: a
    // create resolves it against the store, so it is minted over the wire first.
    let interface_arn = minted_network_interface(&emulator, "rni-zone").await;
    // The zone a request that names none is given: a zone of its own Region.
    let default_zone = format!("{REGION}a");

    for path in create_paths() {
        for (case, zone, accepted) in zone_rows() {
            let case = format!("{} / {case}", path.operation);
            let name = case_name("z", &case);
            let mut body = (path.body)(&name, &interface_arn);
            if let Some(zone) = zone {
                body["availabilityZone"] = json!(zone);
            }

            let (status, code, answered) =
                emulator.send("POST", path.uri, Some(body.clone())).await;
            let (expected_status, expected_code) = if accepted {
                (201, None)
            } else {
                (400, Some("BadRequestException"))
            };
            assert_eq!(
                (status, code.as_deref()),
                (expected_status, expected_code),
                "case {case:?} ({zone:?}): expected {expected_status} {expected_code:?}, got \
                 {status} {code:?}: {answered}"
            );

            if !accepted {
                assert!(
                    answered["message"].as_str().is_some_and(|message| message
                        .starts_with(mc::VALIDATION_FAILED_PREFIX)
                        && message.contains("AvailabilityZone")),
                    "case {case:?}: the refusal names the member it refused, got {answered}"
                );
                continue;
            }

            let expected_zone = zone.unwrap_or(default_zone.as_str());
            let resource = &answered[path.member];
            assert_eq!(
                resource["availabilityZone"].as_str(),
                Some(expected_zone),
                "case {case:?}: the create reports the zone it stored, got {answered}"
            );

            // The stored zone is what a later read reports.
            let identity = resource[path.identity]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: the create answers an identity: {answered}"))
                .to_string();
            let (status, _, read) = emulator
                .send("GET", &format!("{}{identity}", path.read_prefix), None)
                .await;
            assert_eq!(
                (status, read[path.member]["availabilityZone"].as_str()),
                (200, Some(expected_zone)),
                "case {case:?}: the read reports the stored zone, got {read}"
            );
        }
    }
}
