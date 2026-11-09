// SPDX-License-Identifier: CC0-1.0

//! The Anywhere-fleet compute contract, over the wire: `RegisterCompute` persists the
//! compute resource the request names under the fleet and the name it gave, reports the
//! record it stored, and `DescribeCompute` serves that same record back by either of the
//! forms `ComputeNameOrArn` accepts.
//!
//! The rule the rows below span: a compute is identified by its fleet and its own name, so
//! a register stores exactly one compute under that pair whatever the request's name form,
//! address form or location is; a name already registered in that fleet is refused with
//! `ConflictException` and leaves the stored compute untouched, while the same name in
//! another fleet is a compute of its own; and a register the service refuses stores
//! nothing.
//!
//! Beside those, the requirements a register is subject to — the fleet it names, that
//! fleet's compute type, the location the compute resides in, and the address the compute
//! is reached at — each asserted to store nothing when it refuses.
//!
//! The harness (`GameLift`, `REGION`, `gamelift_arn`, `anywhere_fleet`, `ec2_fleet`,
//! `create_fleet`, `error_code`, `error_message`) is the one `hosting_loop.rs` defines and
//! drives the production dispatch path with.

use serde_json::{json, Value};
use service_com_amazonaws_gamelift_gamelift::state::MAX_COMPUTES_PER_FLEET;

use crate::hosting_loop::{
    anywhere_fleet, create_fleet, ec2_fleet, error_code, error_message, gamelift_arn,
    wait_for_due_transitions, GameLift, REGION,
};

/// The longest `ComputeName` a register may carry: `ComputeName` (`smithy_query
/// operation=RegisterCompute`) is len 1..128.
const MAX_COMPUTE_NAME_LENGTH: usize = 128;

/// The two custom locations the fleets below deploy to.
const RACK_ONE: &str = "custom-rack-1";
/// See [`RACK_ONE`].
const RACK_TWO: &str = "custom-rack-2";

/// The Amazon GameLift Servers SDK endpoint a compute registered in [`REGION`] reports:
/// the service's own WebSocket endpoint in the Region the compute was registered in, which
/// a game server process passes to the server SDK's `InitSDK()`.
fn sdk_endpoint() -> String {
    format!("wss://{REGION}.api.amazongamelift.com")
}

/// The ARN assigned to a compute named `name`. `Compute$ComputeArn`'s modelled pattern is
/// `^arn:.*:compute\/[a-zA-Z0-9\-]+(\/[a-zA-Z0-9\-]+)?$`, so the resource is the compute's
/// own name.
fn compute_arn(name: &str) -> String {
    gamelift_arn(&format!("compute/{name}"))
}

/// A `RegisterCompute` request for `name` in `location` on `fleet_id`, carrying the
/// address members (and anything else) `extra` holds.
fn register_request(fleet_id: &str, name: &str, location: &str, extra: &Value) -> Value {
    let mut request = json!({
        "FleetId": fleet_id,
        "ComputeName": name,
        "Location": location,
    });
    for (member, value) in extra.as_object().expect("the extra members are an object") {
        request[member] = value.clone();
    }
    request
}

/// The `Compute` `DescribeCompute` reports for `name_or_arn` on `fleet_id`.
async fn describe_compute(gamelift: &GameLift, fleet_id: &str, name_or_arn: &str) -> Value {
    let described = gamelift
        .ok(
            "DescribeCompute",
            json!({"FleetId": fleet_id, "ComputeName": name_or_arn}),
        )
        .await;
    described["Compute"].clone()
}

/// Assert `operation` refuses `request` at HTTP 400 with `code` and exactly `expected` as
/// its `Message`.
async fn assert_refused(
    gamelift: &GameLift,
    operation: &str,
    request: Value,
    code: &str,
    expected: &str,
    case: &str,
) {
    let (status, body) = gamelift.call(operation, &request).await;
    let got = format!("{}|{}", error_code(&body), error_message(&body));
    let wanted = format!("{code}|{expected}");
    assert_eq!(
        (status, got.as_str()),
        (400, wanted.as_str()),
        "case {case:?} on {operation}: expected HTTP 400 with {wanted:?}, got HTTP {status} \
         with {got:?}"
    );
}

/// A registered compute is reported as stored by the register, reads back identically
/// through `DescribeCompute` by name and by ARN, and the name it holds cannot be
/// registered a second time in the same fleet.
///
/// The rows differ along what the rule generalises over: the LENGTH of the compute name at
/// both modelled bounds (`ComputeName`, len 1..128) and the forms its pattern allows
/// (`^[a-zA-Z0-9\-]+(\/[a-zA-Z0-9\-]+)?$`: digits, upper case, hyphens, the optional second
/// segment), the address the compute is reached at (`IpAddress` — whose pattern
/// `^[0-9A-Fa-f\:\.]+$` covers IPv4 and IPv6 — a `DnsName`, or both, with and without a
/// `CertificatePath`), and which of the fleet's locations it resides in.
#[tokio::test]
async fn a_registered_compute_is_reported_as_stored_and_reads_back_by_name_and_by_arn() {
    struct Case {
        label: &'static str,
        name: String,
        location: &'static str,
        extra: Value,
    }

    let cases = vec![
        Case {
            label: "an IPv4 address in a custom location",
            name: "rack-1-host-a".to_string(),
            location: RACK_ONE,
            extra: json!({"IpAddress": "198.51.100.4"}),
        },
        Case {
            label: "the shortest name the model allows (1 character)",
            name: "a".to_string(),
            location: RACK_ONE,
            extra: json!({"IpAddress": "198.51.100.5"}),
        },
        Case {
            label: "the longest name the model allows (128 characters)",
            name: format!("h{}", "x".repeat(MAX_COMPUTE_NAME_LENGTH - 1)),
            location: RACK_ONE,
            extra: json!({"IpAddress": "198.51.100.6"}),
        },
        Case {
            label: "digits, upper case and inner hyphens, which the pattern allows",
            name: "Rack-1-HOST-07".to_string(),
            location: RACK_ONE,
            extra: json!({"IpAddress": "198.51.100.7"}),
        },
        Case {
            label: "a name carrying the optional second segment",
            name: "rack-2/host-b".to_string(),
            location: RACK_TWO,
            extra: json!({"IpAddress": "198.51.100.8"}),
        },
        Case {
            label: "an IPv6 address, which the IpAddress pattern allows",
            name: "host-v6".to_string(),
            location: RACK_TWO,
            extra: json!({"IpAddress": "2001:db8::1"}),
        },
        Case {
            label: "a DNS name and no IP address",
            name: "host-by-dns".to_string(),
            location: RACK_TWO,
            extra: json!({"DnsName": "host-b.hosting.example.internal"}),
        },
        Case {
            label: "both an address and a DNS name, with a certificate path",
            name: "host-with-certificate".to_string(),
            location: RACK_ONE,
            extra: json!({
                "IpAddress": "198.51.100.9",
                "DnsName": "host-c.hosting.example.internal",
                "CertificatePath": "/local/certificates/server.pem",
            }),
        },
        Case {
            label: "the fleet's home location",
            name: "home-host".to_string(),
            location: REGION,
            extra: json!({"IpAddress": "198.51.100.10"}),
        },
    ];

    let gamelift = GameLift::new();
    let fleet_id = create_fleet(
        &gamelift,
        anywhere_fleet("compute-host-fleet", &[RACK_ONE, RACK_TWO]),
    )
    .await;
    let fleet_arn = gamelift_arn(&format!("fleet/{fleet_id}"));

    for case in &cases {
        let label = case.label;
        let request = register_request(&fleet_id, &case.name, case.location, &case.extra);
        let registered = gamelift.ok("RegisterCompute", request.clone()).await;
        let compute = registered["Compute"].clone();

        // The identity the service assigned the compute, and the fleet it belongs to.
        let reported = json!({
            "FleetId": compute["FleetId"],
            "FleetArn": compute["FleetArn"],
            "ComputeName": compute["ComputeName"],
            "ComputeArn": compute["ComputeArn"],
            "Location": compute["Location"],
            "ComputeStatus": compute["ComputeStatus"],
            "GameLiftServiceSdkEndpoint": compute["GameLiftServiceSdkEndpoint"],
        });
        let expected = json!({
            "FleetId": &fleet_id,
            "FleetArn": &fleet_arn,
            "ComputeName": &case.name,
            "ComputeArn": compute_arn(&case.name),
            "Location": case.location,
            // `Compute$ComputeStatus`: "A compute must have an ACTIVE status to host game
            // sessions", and a registered compute is hardware already running at the
            // address the request gave.
            "ComputeStatus": "ACTIVE",
            "GameLiftServiceSdkEndpoint": sdk_endpoint(),
        });
        assert_eq!(
            reported, expected,
            "case {label:?}: expected {expected}, got {reported} in {registered}"
        );
        assert!(
            compute["CreationTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {label:?}: the compute must report when it was created, got {compute}"
        );
        // The address members carry what the REQUEST gave, and nothing where it gave none.
        for member in ["IpAddress", "DnsName"] {
            let wanted = case.extra.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                compute[member], wanted,
                "case {label:?}: expected {member} {wanted}, got {} in {compute}",
                compute[member]
            );
        }
        // A registered compute in an Anywhere fleet is not a managed instance, so it
        // carries none of the members that describe one.
        for member in ["Type", "InstanceId", "GameLiftAgentEndpoint", "ContainerAttributes"] {
            assert_eq!(
                compute[member],
                Value::Null,
                "case {label:?}: an Anywhere compute carries no {member}, got {compute}"
            );
        }

        // The stored record reads back whole through the describe path, by the compute's
        // name and by its ARN alike.
        for (form, identifier) in [
            ("its name", case.name.clone()),
            ("its ARN", compute_arn(&case.name)),
        ] {
            let described = describe_compute(&gamelift, &fleet_id, &identifier).await;
            assert_eq!(
                described, compute,
                "case {label:?}: the compute must read back by {form} as registered, got \
                 {described}"
            );
        }

        // The name is taken in this fleet now: a second register of it is refused —
        // whatever address or location that attempt carries — naming the location the
        // compute IS registered in, and the stored compute is untouched.
        let taken = format!(
            "Compute name {} is already registered in location {}.",
            case.name, case.location
        );
        let elsewhere = if case.location == RACK_ONE {
            RACK_TWO
        } else {
            RACK_ONE
        };
        for (shape, location, extra) in [
            ("the same request again", case.location, case.extra.clone()),
            (
                "another address under the same name",
                case.location,
                json!({"IpAddress": "198.51.100.200"}),
            ),
            (
                "another location of the fleet under the same name",
                elsewhere,
                json!({"IpAddress": "198.51.100.201"}),
            ),
        ] {
            assert_refused(
                &gamelift,
                "RegisterCompute",
                register_request(&fleet_id, &case.name, location, &extra),
                "ConflictException",
                &taken,
                &format!("{label} ({shape})"),
            )
            .await;
        }
        let described = describe_compute(&gamelift, &fleet_id, &case.name).await;
        assert_eq!(
            described, compute,
            "case {label:?}: the refused re-registers must leave the compute as stored, got \
             {described}"
        );
    }

    // A compute name is unique within ONE fleet: the same names register again on a second
    // fleet, each as a compute of its own.
    let other_fleet = create_fleet(
        &gamelift,
        anywhere_fleet("second-compute-host-fleet", &[RACK_ONE]),
    )
    .await;
    for case in &cases {
        let registered = gamelift
            .ok(
                "RegisterCompute",
                register_request(&other_fleet, &case.name, RACK_ONE, &case.extra),
            )
            .await;
        assert_eq!(
            registered["Compute"]["FleetId"],
            json!(&other_fleet),
            "case {:?}: the same name on another fleet must register there, got {registered}",
            case.label
        );
        // And each fleet still serves its OWN compute under that name.
        let described = describe_compute(&gamelift, &fleet_id, &case.name).await;
        assert_eq!(
            described["FleetId"],
            json!(&fleet_id),
            "case {:?}: the first fleet's compute must be unchanged, got {described}",
            case.label
        );
    }
}

/// A register the service refuses stores no compute: the fleet must be one it holds, that
/// fleet's compute type must be the one whose computes a caller registers, the location
/// must be one the fleet deploys to, and the compute must carry an address.
#[tokio::test]
async fn a_refused_register_names_the_requirement_and_stores_nothing() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(
        &gamelift,
        anywhere_fleet("refusal-host-fleet", &[RACK_ONE]),
    )
    .await;
    let managed_fleet = create_fleet(&gamelift, ec2_fleet("managed-host-fleet")).await;
    wait_for_due_transitions();

    struct Case {
        label: &'static str,
        request: Value,
        expected: String,
    }

    let mut cases = vec![
        Case {
            label: "a location the fleet does not deploy to",
            request: json!({
                "FleetId": &fleet_id,
                "ComputeName": "refused-location",
                "Location": RACK_TWO,
                "IpAddress": "198.51.100.11",
            }),
            expected: format!("Location {RACK_TWO} is not a location of fleet {fleet_id}."),
        },
        Case {
            label: "no location at all",
            request: json!({
                "FleetId": &fleet_id,
                "ComputeName": "refused-no-location",
                "IpAddress": "198.51.100.12",
            }),
            expected: "Location is required to register a compute to a fleet with compute type \
                       ANYWHERE."
                .to_string(),
        },
        Case {
            label: "neither a DNS name nor an IP address",
            request: json!({
                "FleetId": &fleet_id,
                "ComputeName": "refused-no-address",
                "Location": RACK_ONE,
            }),
            expected: "Either DnsName or IpAddress is required to register a compute.".to_string(),
        },
        Case {
            label: "a managed EC2 fleet, whose computes the service provisions itself",
            request: json!({
                "FleetId": &managed_fleet,
                "ComputeName": "refused-managed",
                "Location": REGION,
                "IpAddress": "198.51.100.13",
            }),
            expected: format!(
                "Fleet {managed_fleet} has compute type EC2; a compute can only be registered \
                 to a fleet with compute type ANYWHERE."
            ),
        },
    ];
    // Fleet identifiers that are well formed and name no stored fleet, differing in form
    // (bare id, UUID-shaped id, ARN).
    for fleet in [
        "fleet-absent".to_string(),
        "fleet-00000000-0000-0000-0000-000000000000".to_string(),
        gamelift_arn("fleet/fleet-absent-by-arn"),
    ] {
        cases.push(Case {
            label: "a fleet the store does not hold",
            request: json!({
                "FleetId": &fleet,
                "ComputeName": "refused-absent-fleet",
                "Location": RACK_ONE,
                "IpAddress": "198.51.100.14",
            }),
            expected: format!("Cannot find fleet with ID {fleet}"),
        });
    }

    for case in &cases {
        assert_refused(
            &gamelift,
            "RegisterCompute",
            case.request.clone(),
            // The model declares no `NotFoundException` for this operation, so every one
            // of these is the invalid parameter value it is.
            "InvalidRequestException",
            &case.expected,
            case.label,
        )
        .await;

        // Nothing was stored: the name the refused register asked for is still free on the
        // Anywhere fleet.
        let name = case.request["ComputeName"]
            .as_str()
            .expect("every row names a compute");
        let (status, body) = gamelift
            .call(
                "DescribeCompute",
                &json!({"FleetId": &fleet_id, "ComputeName": name}),
            )
            .await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "NotFoundException"),
            "case {:?}: the refused register must have stored nothing, got HTTP {status} \
             {code} with body {body}",
            case.label
        );
    }

    // The control: a complete register on the same fleet is accepted, so the refusals
    // above are about the requests and not a blanket answer.
    let accepted = gamelift
        .ok(
            "RegisterCompute",
            json!({
                "FleetId": &fleet_id,
                "ComputeName": "accepted-host",
                "Location": RACK_ONE,
                "IpAddress": "198.51.100.15",
            }),
        )
        .await;
    assert_eq!(
        accepted["Compute"]["ComputeName"],
        json!("accepted-host"),
        "the complete register must store the compute, got {accepted}"
    );
}

/// A fleet holds at most [`MAX_COMPUTES_PER_FLEET`] computes: the register past that count
/// is refused with the modelled `LimitExceededException`, stores nothing, and the quota is
/// the FLEET's own, so another fleet still registers computes of its own.
#[tokio::test]
async fn the_register_past_the_fleets_compute_quota_is_refused() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(
        &gamelift,
        anywhere_fleet("quota-host-fleet", &[RACK_ONE]),
    )
    .await;
    let other_fleet = create_fleet(
        &gamelift,
        anywhere_fleet("quota-other-fleet", &[RACK_ONE]),
    )
    .await;

    for index in 0..MAX_COMPUTES_PER_FLEET {
        let registered = gamelift
            .ok(
                "RegisterCompute",
                json!({
                    "FleetId": &fleet_id,
                    "ComputeName": format!("quota-host-{index}"),
                    "Location": RACK_ONE,
                    "IpAddress": "198.51.100.80",
                }),
            )
            .await;
        assert_eq!(
            registered["Compute"]["ComputeName"],
            json!(format!("quota-host-{index}")),
            "compute {index} must register within the quota, got {registered}"
        );
    }

    assert_refused(
        &gamelift,
        "RegisterCompute",
        json!({
            "FleetId": &fleet_id,
            "ComputeName": "one-too-many",
            "Location": RACK_ONE,
            "IpAddress": "198.51.100.81",
        }),
        "LimitExceededException",
        &format!(
            "Fleet {fleet_id} already has the maximum of {MAX_COMPUTES_PER_FLEET} computes."
        ),
        "a register past the fleet's compute quota",
    )
    .await;

    // Nothing was stored: the fleet still holds exactly the computes it admitted.
    let listed = gamelift
        .ok(
            "ListCompute",
            json!({"FleetId": &fleet_id, "Limit": MAX_COMPUTES_PER_FLEET}),
        )
        .await;
    let held = listed["ComputeList"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_else(|| panic!("ListCompute must report a list, got {listed}"));
    assert_eq!(
        held, MAX_COMPUTES_PER_FLEET,
        "the fleet must hold exactly {MAX_COMPUTES_PER_FLEET} computes, got {held}"
    );

    // The quota is the fleet's own, not the store's.
    let registered = gamelift
        .ok(
            "RegisterCompute",
            json!({
                "FleetId": &other_fleet,
                "ComputeName": "quota-host-0",
                "Location": RACK_ONE,
                "IpAddress": "198.51.100.82",
            }),
        )
        .await;
    assert_eq!(
        registered["Compute"]["FleetId"],
        json!(&other_fleet),
        "another fleet must still register a compute of its own, got {registered}"
    );
}

/// A compute the named fleet does not hold is reported as the identifier the request asked
/// for, and a fleet's computes leave state with the fleet.
#[tokio::test]
async fn a_compute_that_cannot_be_found_is_reported_as_the_identifier_asked_for() {
    let gamelift = GameLift::new();
    let fleet_id = create_fleet(
        &gamelift,
        anywhere_fleet("lookup-host-fleet", &[RACK_ONE]),
    )
    .await;
    let other_fleet = create_fleet(
        &gamelift,
        anywhere_fleet("lookup-other-fleet", &[RACK_ONE]),
    )
    .await;
    gamelift
        .ok(
            "RegisterCompute",
            json!({
                "FleetId": &fleet_id,
                "ComputeName": "stored-host",
                "Location": RACK_ONE,
                "IpAddress": "198.51.100.16",
            }),
        )
        .await;

    // Compute identifiers, in both forms `ComputeNameOrArn` accepts, that the named fleet
    // holds nothing under — including the name of a compute stored on ANOTHER fleet,
    // which proves the lookup is scoped to the fleet the request named.
    let mut cases = vec![
        (
            "a name no compute carries",
            fleet_id.clone(),
            "absent-host".to_string(),
        ),
        (
            "an ARN no compute carries",
            fleet_id.clone(),
            compute_arn("absent-host-by-arn"),
        ),
        (
            "a stored name on another fleet",
            other_fleet.clone(),
            "stored-host".to_string(),
        ),
        (
            "a stored compute's ARN on another fleet",
            other_fleet.clone(),
            compute_arn("stored-host"),
        ),
    ];
    // A name that differs from a stored one only in case: a compute name selects one
    // record exactly.
    cases.push((
        "a stored name in another case",
        fleet_id.clone(),
        "STORED-HOST".to_string(),
    ));

    for (label, fleet, identifier) in &cases {
        let expected = format!("Host ({identifier}) not found.");
        assert_refused(
            &gamelift,
            "DescribeCompute",
            json!({"FleetId": fleet, "ComputeName": identifier}),
            "NotFoundException",
            &expected,
            label,
        )
        .await;
    }

    // A fleet the store does not hold is reported as the fleet, not as the compute.
    assert_refused(
        &gamelift,
        "DescribeCompute",
        json!({"FleetId": "fleet-absent", "ComputeName": "stored-host"}),
        "NotFoundException",
        "Cannot find fleet with ID fleet-absent",
        "a fleet the store does not hold",
    )
    .await;

    // The control: the stored compute is still served on its own fleet.
    let described = describe_compute(&gamelift, &fleet_id, "stored-host").await;
    assert_eq!(
        described["ComputeName"],
        json!("stored-host"),
        "the stored compute must still be served, got {described}"
    );

    // "Deletes all resources and information related to a fleet": the computes registered
    // with it go with it, so the describe that follows no longer finds the fleet.
    gamelift
        .ok("DeleteFleet", json!({"FleetId": &fleet_id}))
        .await;
    assert_refused(
        &gamelift,
        "DescribeCompute",
        json!({"FleetId": &fleet_id, "ComputeName": "stored-host"}),
        "NotFoundException",
        &format!("Cannot find fleet with ID {fleet_id}"),
        "a deleted fleet",
    )
    .await;
}

/// `ListCompute` reports every compute registered with the fleet exactly once, oldest
/// first, narrowed to whichever of the selectors the request carries, and served in pages
/// of the size it asks for.
///
/// The rows differ along what the rule generalises over: which selector the request
/// carries (none, each of the fleet's locations, each `ListComputeInputStatus` the model
/// declares, a container group definition name), how many computes that selects, and the
/// page size the walk takes — and every listed entry is compared against the record the
/// describe path serves for it, so both reads can only agree.
#[tokio::test]
async fn the_fleets_computes_are_listed_once_each_narrowed_and_paged() {
    /// Every `ListComputeInputStatus` the model declares (`smithy_query
    /// shape=ListComputeInputStatus`).
    const COMPUTE_STATUS_FILTERS: [&str; 2] = ["ACTIVE", "IMPAIRED"];

    let gamelift = GameLift::new();
    let fleet_id = create_fleet(
        &gamelift,
        anywhere_fleet("list-host-fleet", &[RACK_ONE, RACK_TWO]),
    )
    .await;
    // A second fleet with a compute of its own: a list is scoped to the fleet it names.
    let other_fleet = create_fleet(
        &gamelift,
        anywhere_fleet("list-other-fleet", &[RACK_ONE]),
    )
    .await;
    gamelift
        .ok(
            "RegisterCompute",
            json!({
                "FleetId": &other_fleet,
                "ComputeName": "other-fleet-host",
                "Location": RACK_ONE,
                "IpAddress": "198.51.100.30",
            }),
        )
        .await;

    // The computes of the fleet under test, in the order they were registered.
    let registered = [
        ("rack-1-host-a", RACK_ONE),
        ("rack-1-host-b", RACK_ONE),
        ("rack-2-host-a", RACK_TWO),
        ("home-host", REGION),
    ];
    for (index, (name, location)) in registered.iter().enumerate() {
        gamelift
            .ok(
                "RegisterCompute",
                json!({
                    "FleetId": &fleet_id,
                    "ComputeName": name,
                    "Location": location,
                    "IpAddress": format!("198.51.100.{}", 40 + index),
                }),
            )
            .await;
    }
    let every_name: Vec<&str> = registered.iter().map(|(name, _)| *name).collect();

    struct Case {
        label: &'static str,
        request: Value,
        expected: Vec<&'static str>,
    }

    let mut cases = vec![
        Case {
            label: "no selector but the fleet",
            request: json!({"FleetId": &fleet_id}),
            expected: every_name.clone(),
        },
        Case {
            label: "one of the fleet's custom locations",
            request: json!({"FleetId": &fleet_id, "Location": RACK_ONE}),
            expected: vec!["rack-1-host-a", "rack-1-host-b"],
        },
        Case {
            label: "the fleet's other custom location",
            request: json!({"FleetId": &fleet_id, "Location": RACK_TWO}),
            expected: vec!["rack-2-host-a"],
        },
        Case {
            label: "the fleet's home location",
            request: json!({"FleetId": &fleet_id, "Location": REGION}),
            expected: vec!["home-host"],
        },
        Case {
            label: "a location the fleet does not deploy to",
            request: json!({"FleetId": &fleet_id, "Location": "custom-rack-9"}),
            expected: Vec::new(),
        },
        Case {
            label: "the fleet by ARN rather than by id",
            request: json!({"FleetId": gamelift_arn(&format!("fleet/{fleet_id}"))}),
            expected: every_name.clone(),
        },
        Case {
            label: "a fleet the store does not hold",
            request: json!({"FleetId": "fleet-absent"}),
            expected: Vec::new(),
        },
        Case {
            label: "a container group definition name, which no registered compute runs",
            request: json!({"FleetId": &fleet_id, "ContainerGroupDefinitionName": "game-servers"}),
            expected: Vec::new(),
        },
    ];
    for status in COMPUTE_STATUS_FILTERS {
        cases.push(Case {
            label: "a compute status the request selects",
            request: json!({"FleetId": &fleet_id, "ComputeStatus": status}),
            // A registered compute is ACTIVE, so the other status selects none of them.
            expected: if status == "ACTIVE" {
                every_name.clone()
            } else {
                Vec::new()
            },
        });
    }

    for case in &cases {
        let label = case.label;
        let listed = gamelift.ok("ListCompute", case.request.clone()).await;
        let computes = listed["ComputeList"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: ComputeList must be a list, got {listed}"));
        let names: Vec<&str> = computes
            .iter()
            .map(|compute| compute["ComputeName"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names, case.expected,
            "case {label:?} ({}): expected {:?}, got {names:?} in {listed}",
            case.request, case.expected
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {label:?}: a whole result set carries no NextToken, got {listed}"
        );
        // Each listed entry is the record the describe path serves for that compute.
        for compute in &computes {
            let name = compute["ComputeName"].as_str().unwrap_or_default();
            let described = describe_compute(&gamelift, &fleet_id, name).await;
            assert_eq!(
                compute, &described,
                "case {label:?}: the listed compute {name:?} must match the described one, got \
                 {described}"
            );
        }
    }

    // Paged: a walk in pages of `limit` reports the same computes, in the same order, each
    // exactly once, and hands out no token on its last page.
    for limit in 1..=every_name.len() + 1 {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..every_name.len() + 2 {
            let mut request = json!({"FleetId": &fleet_id, "Limit": limit});
            if let Some(next) = &token {
                request["NextToken"] = json!(next);
            }
            let listed = gamelift.ok("ListCompute", request).await;
            let computes = listed["ComputeList"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                computes.len() <= limit,
                "limit {limit}, page {page}: a page must hold at most {limit} computes, got \
                 {listed}"
            );
            walked.extend(
                computes
                    .iter()
                    .map(|compute| compute["ComputeName"].as_str().unwrap_or_default().to_string()),
            );
            token = listed
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            token, None,
            "limit {limit}: the walk must end without a token, got {token:?}"
        );
        assert_eq!(
            walked, every_name,
            "limit {limit}: the paged walk must report every compute once, in order, got \
             {walked:?}"
        );
    }

    // A token this collection did not issue is not an offset into it: the
    // `DescribeFleetAttributes` token below belongs to another collection (this store holds
    // two fleets, so a page of one hands out a token), and the rows after it are no token
    // at all.
    let fleets = gamelift
        .ok("DescribeFleetAttributes", json!({"Limit": 1}))
        .await;
    let foreign = fleets["NextToken"]
        .as_str()
        .expect("DescribeFleetAttributes must page the fleets it serves")
        .to_string();
    for (label, token) in [
        ("a token of another collection", foreign),
        ("a token no collection issued", "not-a-token".to_string()),
        ("a token of the right shape but the wrong tag", "z000000000000000".to_string()),
    ] {
        assert_refused(
            &gamelift,
            "ListCompute",
            json!({"FleetId": &fleet_id, "NextToken": token}),
            "InvalidRequestException",
            "NextToken is not a pagination token this service issued.",
            label,
        )
        .await;
    }
}

/// A deregistered compute leaves the fleet: the describe and the list stop reporting it,
/// the fleet's other computes are untouched, the name it held is free to register again,
/// and a second deregister of it has nothing to remove and succeeds.
///
/// The rows differ along which of the forms `ComputeNameOrArn` accepts the deregister
/// names the compute by, the name's own form, and the fleet location it resides in.
#[tokio::test]
async fn a_deregistered_compute_leaves_the_fleet_and_frees_its_name() {
    struct Case {
        label: &'static str,
        name: &'static str,
        location: &'static str,
        /// Whether the deregister names the compute by its ARN rather than by its name.
        by_arn: bool,
    }

    let cases = [
        Case {
            label: "by its name",
            name: "host-a",
            location: RACK_ONE,
            by_arn: false,
        },
        Case {
            label: "by its ARN",
            name: "host-b",
            location: RACK_TWO,
            by_arn: true,
        },
        Case {
            label: "a two-segment name, by its ARN",
            name: "rack-2/host-c",
            location: RACK_TWO,
            by_arn: true,
        },
        Case {
            label: "a compute in the fleet's home location",
            name: "home-host",
            location: REGION,
            by_arn: false,
        },
    ];

    let gamelift = GameLift::new();
    let fleet_id = create_fleet(
        &gamelift,
        anywhere_fleet("deregister-host-fleet", &[RACK_ONE, RACK_TWO]),
    )
    .await;
    // A compute nothing below deregisters: the fleet's other computes must survive each
    // deregister.
    gamelift
        .ok(
            "RegisterCompute",
            json!({
                "FleetId": &fleet_id,
                "ComputeName": "kept-host",
                "Location": RACK_ONE,
                "IpAddress": "198.51.100.60",
            }),
        )
        .await;

    for (index, case) in cases.iter().enumerate() {
        let label = case.label;
        gamelift
            .ok(
                "RegisterCompute",
                json!({
                    "FleetId": &fleet_id,
                    "ComputeName": case.name,
                    "Location": case.location,
                    "IpAddress": format!("198.51.100.{}", 61 + index),
                }),
            )
            .await;
        let identifier = if case.by_arn {
            compute_arn(case.name)
        } else {
            case.name.to_string()
        };

        gamelift
            .ok(
                "DeregisterCompute",
                json!({"FleetId": &fleet_id, "ComputeName": &identifier}),
            )
            .await;

        // Gone from both read paths.
        let expected = format!("Host ({}) not found.", case.name);
        assert_refused(
            &gamelift,
            "DescribeCompute",
            json!({"FleetId": &fleet_id, "ComputeName": case.name}),
            "NotFoundException",
            &expected,
            label,
        )
        .await;
        let listed = gamelift
            .ok("ListCompute", json!({"FleetId": &fleet_id}))
            .await;
        let names: Vec<&str> = listed["ComputeList"]
            .as_array()
            .expect("ListCompute must report a list")
            .iter()
            .map(|compute| compute["ComputeName"].as_str().unwrap_or_default())
            .collect();
        assert!(
            !names.contains(&case.name),
            "case {label:?}: the deregistered compute must be gone from the list, got {names:?}"
        );
        // The fleet's other compute is untouched.
        assert!(
            names.contains(&"kept-host"),
            "case {label:?}: the fleet's other compute must survive, got {names:?}"
        );

        // A second deregister of it has nothing to remove, and says so by succeeding: the
        // removal is idempotent, so the compute is still absent and the fleet's other
        // compute is untouched.
        gamelift
            .ok(
                "DeregisterCompute",
                json!({"FleetId": &fleet_id, "ComputeName": &identifier}),
            )
            .await;
        assert_refused(
            &gamelift,
            "DescribeCompute",
            json!({"FleetId": &fleet_id, "ComputeName": case.name}),
            "NotFoundException",
            &expected,
            label,
        )
        .await;
        let kept = describe_compute(&gamelift, &fleet_id, "kept-host").await;
        assert_eq!(
            kept["ComputeName"],
            json!("kept-host"),
            "case {label:?}: a repeated deregister must leave the fleet's other compute, got \
             {kept}"
        );

        // And the name is free: registering it again stores a NEW compute at the address
        // that register gives it.
        let address = format!("198.51.100.{}", 71 + index);
        let registered = gamelift
            .ok(
                "RegisterCompute",
                json!({
                    "FleetId": &fleet_id,
                    "ComputeName": case.name,
                    "Location": case.location,
                    "IpAddress": &address,
                }),
            )
            .await;
        assert_eq!(
            registered["Compute"]["IpAddress"],
            json!(&address),
            "case {label:?}: the freed name must register again, got {registered}"
        );
        let described = describe_compute(&gamelift, &fleet_id, case.name).await;
        assert_eq!(
            described, registered["Compute"],
            "case {label:?}: the re-registered compute must read back as registered, got \
             {described}"
        );
    }

    // A fleet the store does not hold is reported as the fleet, not as the compute.
    assert_refused(
        &gamelift,
        "DeregisterCompute",
        json!({"FleetId": "fleet-absent", "ComputeName": "kept-host"}),
        "NotFoundException",
        "Cannot find fleet with ID fleet-absent",
        "a fleet the store does not hold",
    )
    .await;
    // The control: the compute nothing deregistered is still served.
    let described = describe_compute(&gamelift, &fleet_id, "kept-host").await;
    assert_eq!(
        described["ComputeName"],
        json!("kept-host"),
        "the compute nothing deregistered must still be served, got {described}"
    );
}
