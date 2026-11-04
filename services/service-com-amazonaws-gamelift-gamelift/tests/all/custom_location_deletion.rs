// SPDX-License-Identifier: CC0-1.0

//! `DeleteLocation`, over the wire: which identifiers select the one custom location the
//! call removes, and what the call leaves alone.
//!
//! The rule both suites below assert: a delete selects AT MOST ONE stored resource through
//! the identifier forms its modelled identifier member accepts — an identifier outside that
//! member's published constraints is refused before any state changes, an in-constraint
//! identifier that resolves to no stored resource answers the modelled not-found refusal,
//! and a served delete removes exactly the resolved record and nothing else.
//!
//! `resource_deletion.rs` asserts the removal itself across the delete family; this suite
//! pins the IDENTIFIER walk of `DeleteLocation`'s own member (`CustomLocationNameOrArnModel`:
//! `smithy_query shape=CustomLocationNameOrArnModel`, len 1..128, pattern
//! `^(custom-[A-Za-z0-9\-]+|arn:.*:location\/custom-\S+)$`) and the resources the delete
//! does not touch.
//!
//! The harness (`GameLift`, `REGION`, `gamelift_arn`, `error_code`, `error_message`,
//! `anywhere_fleet`, `ec2_fleet`, `create_fleet`, `wait_for_due_transitions`) is the one
//! `hosting_loop.rs` already drives the production dispatch path with.

use serde_json::{json, Value};

use crate::hosting_loop::{
    anywhere_fleet, create_fleet, ec2_fleet, error_code, error_message, gamelift_arn,
    wait_for_due_transitions, GameLift, REGION,
};

/// The published length bounds of the member a delete names its location with
/// (`smithy_query shape=CustomLocationNameOrArnModel`: len 1..128).
const IDENTIFIER_LENGTH: (usize, usize) = (1, 128);

/// The custom locations every row of the identifier walk starts from. Each name satisfies
/// the create's own member (`smithy_query shape=CustomInputLocationStringModel`: len 8..64,
/// pattern `^custom-[A-Za-z0-9\-]+$`).
const STORED: [&str; 3] = ["custom-stored-a", "custom-stored-b", "custom-stored-c"];

/// The ARN a custom location named `name` is assigned. `LocationModel$LocationArn`'s
/// modelled pattern is `^arn:.*:location\/custom-\S+$`, whose resource is the location's
/// own name.
fn location_arn(name: &str) -> String {
    gamelift_arn(&format!("location/{name}"))
}

/// An ARN-form identifier of exactly `length` characters: the modelled pattern's ARN
/// alternative, padded in its location name, so a row differs from its neighbour in LENGTH
/// alone.
fn arn_of_length(length: usize) -> String {
    let prefix = location_arn("custom-");
    let padding = length
        .checked_sub(prefix.len())
        .expect("the ARN prefix must be shorter than the requested length");
    format!("{prefix}{}", "a".repeat(padding))
}

/// The `LocationName`s `ListLocations` reports for `filter`, in order.
async fn listed_names(gamelift: &GameLift, filter: &str) -> Vec<String> {
    let listed = gamelift
        .ok("ListLocations", json!({"Filters": [filter], "Limit": 200}))
        .await;
    listed["Locations"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("ListLocations must report a list, got {listed}"))
        .iter()
        .map(|location| {
            location["LocationName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// What a delete carrying one identifier must answer.
enum Expected {
    /// The delete is served and takes the named location out of the store.
    Removes(&'static str),
    /// The identifier satisfies the member's constraints but names no stored location, so
    /// the modelled `NotFoundException` is the answer and nothing is removed.
    NotFound,
    /// The identifier is outside the member's published constraints, so the request walk
    /// refuses it with this wire error code and nothing is removed.
    OutsideTheConstraints(&'static str),
}

/// The wire error code the request walk refuses a value its modelled constraints reject
/// with, as `locations.rs` already pins it for `CreateLocation$LocationName`.
const CONSTRAINT_REFUSAL: &str = "ValidationException";

/// A delete names its custom location by either form the modelled member accepts, and only
/// a resolved identifier removes anything: an identifier outside the member's published
/// constraints, and one inside them that resolves to no stored location, both leave the
/// whole collection as it was.
///
/// The rows differ along what the rule generalises over: the FORM of the identifier (the
/// location's own name or its ARN), its LENGTH at the published bound and one past it, the
/// character classes the pattern allows and excludes, which stored location it resolves to
/// (the first, a middle one, the last), whether it resolves at all, and whether the member
/// is supplied.
#[tokio::test]
async fn a_delete_removes_only_the_location_its_identifier_resolves_to() {
    let (shortest, longest) = IDENTIFIER_LENGTH;
    struct Case {
        label: &'static str,
        request: Value,
        expected: Expected,
    }

    let cases = vec![
        Case {
            label: "the first stored location, by its own name",
            request: json!({"LocationName": STORED[0]}),
            expected: Expected::Removes(STORED[0]),
        },
        Case {
            label: "a middle stored location, by its ARN",
            request: json!({"LocationName": location_arn(STORED[1])}),
            expected: Expected::Removes(STORED[1]),
        },
        Case {
            label: "the last stored location, by its own name",
            request: json!({"LocationName": STORED[2]}),
            expected: Expected::Removes(STORED[2]),
        },
        Case {
            label: "an in-constraint name no location carries",
            request: json!({"LocationName": "custom-absent"}),
            expected: Expected::NotFound,
        },
        Case {
            label: "an in-constraint ARN no location carries",
            request: json!({"LocationName": location_arn("custom-absent")}),
            expected: Expected::NotFound,
        },
        Case {
            label: "a stored name under another account's ARN",
            request: json!({
                "LocationName": format!(
                    "arn:aws:gamelift:{REGION}:999999999999:location/{}",
                    STORED[0]
                ),
            }),
            expected: Expected::NotFound,
        },
        Case {
            label: "a stored name under another Region's ARN",
            request: json!({
                "LocationName": format!(
                    "arn:aws:gamelift:eu-central-1:{}:location/{}",
                    engine::emulator_account_id(),
                    STORED[0]
                ),
            }),
            expected: Expected::NotFound,
        },
        Case {
            label: "an identifier on the highest published length bound",
            request: json!({"LocationName": arn_of_length(longest)}),
            expected: Expected::NotFound,
        },
        Case {
            label: "one character past the highest published length bound",
            request: json!({"LocationName": arn_of_length(longest + 1)}),
            expected: Expected::OutsideTheConstraints(CONSTRAINT_REFUSAL),
        },
        Case {
            label: "an identifier of the shortest published length, which neither \
                    alternative of the pattern can match",
            request: json!({"LocationName": "a".repeat(shortest)}),
            expected: Expected::OutsideTheConstraints(CONSTRAINT_REFUSAL),
        },
        Case {
            label: "an underscore, which the pattern's character class excludes",
            request: json!({"LocationName": "custom-stored_a"}),
            expected: Expected::OutsideTheConstraints(CONSTRAINT_REFUSAL),
        },
        Case {
            label: "a space, which the pattern's character class excludes",
            request: json!({"LocationName": "custom-stored a"}),
            expected: Expected::OutsideTheConstraints(CONSTRAINT_REFUSAL),
        },
        Case {
            label: "a stored location named without the custom- prefix the pattern requires",
            request: json!({"LocationName": "stored-a"}),
            expected: Expected::OutsideTheConstraints(CONSTRAINT_REFUSAL),
        },
        Case {
            label: "the prefix in the wrong case",
            request: json!({"LocationName": "Custom-stored-a"}),
            expected: Expected::OutsideTheConstraints(CONSTRAINT_REFUSAL),
        },
        Case {
            label: "an ARN whose resource is not a location",
            request: json!({"LocationName": gamelift_arn("fleet/fleet-custom-stored-a")}),
            expected: Expected::OutsideTheConstraints(CONSTRAINT_REFUSAL),
        },
        Case {
            label: "no LocationName at all, which the model makes required",
            request: json!({}),
            expected: Expected::OutsideTheConstraints(CONSTRAINT_REFUSAL),
        },
    ];

    for case in &cases {
        let label = case.label;
        let gamelift = GameLift::new();
        for name in STORED {
            gamelift
                .ok("CreateLocation", json!({"LocationName": name}))
                .await;
        }
        let aws_before = listed_names(&gamelift, "AWS").await;

        let (status, body) = gamelift.call("DeleteLocation", &case.request).await;
        match &case.expected {
            Expected::Removes(_) => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: a resolved identifier must be served, got HTTP {status} \
                     with body {body}"
                );
                assert!(
                    body.is_null() || body == json!({}),
                    "case {label:?}: DeleteLocation reports no members, got {body}"
                );
            }
            Expected::NotFound => {
                let selector = case.request["LocationName"].as_str().unwrap_or_default();
                let got = format!("{}|{}", error_code(&body), error_message(&body));
                let wanted = format!("NotFoundException|Location ({selector}) not found");
                assert_eq!(
                    (status, got.as_str()),
                    (400, wanted.as_str()),
                    "case {label:?}: expected HTTP 400 with {wanted:?}, got HTTP {status} with \
                     {got:?}"
                );
            }
            Expected::OutsideTheConstraints(code) => {
                let got = error_code(&body);
                assert_eq!(
                    (status, got.as_str()),
                    (400, *code),
                    "case {label:?}: an identifier outside the member's constraints must be \
                     refused with HTTP 400 {code}, got HTTP {status} with body {body}"
                );
            }
        }

        // The collection afterwards: short by the resolved location, and by nothing else.
        let expected_custom: Vec<String> = STORED
            .iter()
            .filter(|name| match &case.expected {
                Expected::Removes(removed) => name != &removed,
                _ => true,
            })
            .map(|name| (*name).to_string())
            .collect();
        let custom_after = listed_names(&gamelift, "CUSTOM").await;
        assert_eq!(
            custom_after, expected_custom,
            "case {label:?}: expected the custom locations {expected_custom:?}, got \
             {custom_after:?}"
        );
        let aws_after = listed_names(&gamelift, "AWS").await;
        assert_eq!(
            aws_after, aws_before,
            "case {label:?}: deleting a custom location must leave the Amazon Web Services \
             locations alone, got {aws_after:?}"
        );

        // The same call again: a served delete took the record out, so the identifier now
        // resolves to nothing; a refused one changed nothing, so it is refused the same way.
        let (repeat_status, repeat_body) = gamelift.call("DeleteLocation", &case.request).await;
        match &case.expected {
            Expected::Removes(removed) => {
                let selector = case.request["LocationName"].as_str().unwrap_or_default();
                let got = format!("{}|{}", error_code(&repeat_body), error_message(&repeat_body));
                let wanted = format!("NotFoundException|Location ({selector}) not found");
                assert_eq!(
                    (repeat_status, got.as_str()),
                    (400, wanted.as_str()),
                    "case {label:?}: the removed {removed:?} must no longer resolve, got HTTP \
                     {repeat_status} with {got:?}"
                );
                // And the name is free again, which a create of it proves.
                let recreated = gamelift
                    .ok("CreateLocation", json!({"LocationName": removed}))
                    .await;
                assert_eq!(
                    recreated["Location"],
                    json!({"LocationName": removed, "LocationArn": location_arn(removed)}),
                    "case {label:?}: the removed name must be free for another create, got \
                     {recreated}"
                );
            }
            _ => {
                assert_eq!(
                    (repeat_status, error_code(&repeat_body).as_str()),
                    (status, error_code(&body).as_str()),
                    "case {label:?}: a refused delete changes nothing, so the repeat must be \
                     refused the same way, got HTTP {repeat_status} with {repeat_body}"
                );
                let still = listed_names(&gamelift, "CUSTOM").await;
                assert_eq!(
                    still, expected_custom,
                    "case {label:?}: expected the custom locations {expected_custom:?} after the \
                     repeat, got {still:?}"
                );
            }
        }
    }
}

/// The delete removes the location RESOURCE and nothing else: the fleets deployed to that
/// location read back exactly as they did, since reviewing and deregistering them is
/// documented as the caller's own step before the delete ("before deleting a custom
/// location, review any fleets currently using the custom location and deregister the
/// location if it is in use") rather than something this call performs.
///
/// The fleets span what the rule generalises over: how many locations a fleet deploys to,
/// whether those are custom or Amazon Web Services locations, how many fleets deploy to the
/// deleted location, and which compute type the fleet uses.
#[tokio::test]
async fn deleting_a_custom_location_leaves_the_fleets_deployed_to_it_untouched() {
    let gamelift = GameLift::new();
    let deleted = "custom-rack-01";
    let surviving = "custom-rack-02";
    for name in [deleted, surviving] {
        gamelift
            .ok("CreateLocation", json!({"LocationName": name}))
            .await;
    }

    let fleets = [
        ("one custom location", anywhere_fleet("rack-only", &[deleted])),
        (
            "a custom location beside an Amazon Web Services one",
            anywhere_fleet("rack-and-region", &[deleted, "eu-central-1"]),
        ),
        (
            "a second fleet on the same custom location",
            anywhere_fleet("rack-shared", &[deleted]),
        ),
        (
            "another custom location entirely",
            anywhere_fleet("other-rack", &[surviving]),
        ),
        ("a managed EC2 fleet at home", ec2_fleet("home-ec2")),
    ];
    let mut fleet_ids = Vec::new();
    for (label, request) in &fleets {
        fleet_ids.push((*label, create_fleet(&gamelift, request.clone()).await));
    }

    // Past the span the fleet status changes are due on, so the attributes below are the
    // settled ones and the delete is the only thing that could move them.
    wait_for_due_transitions();
    let mut before = Vec::new();
    for (label, fleet_id) in &fleet_ids {
        let described = gamelift
            .ok("DescribeFleetAttributes", json!({"FleetIds": [fleet_id]}))
            .await;
        before.push((*label, described));
    }
    // The Amazon Web Services locations BEFORE the delete. They are derived from stored
    // state — the Region of the request plus every non-custom location a stored fleet
    // deploys to — so this is the set the delete must leave exactly as it is.
    let aws_before = listed_names(&gamelift, "AWS").await;
    assert!(
        aws_before.iter().any(|location| location == "eu-central-1"),
        "a Region only a stored fleet deploys to must be reported, got {aws_before:?}"
    );

    gamelift
        .ok("DeleteLocation", json!({"LocationName": deleted}))
        .await;

    for ((label, fleet_id), (_, described_before)) in fleet_ids.iter().zip(before.iter()) {
        let described_after = gamelift
            .ok("DescribeFleetAttributes", json!({"FleetIds": [fleet_id]}))
            .await;
        assert_eq!(
            &described_after, described_before,
            "case {label:?}: fleet {fleet_id} must read back unchanged across the location \
             delete, got {described_after}"
        );
    }

    // The deleted location is reported under NEITHER filter, even though fleets still name
    // it: a custom location is only ever the record a create stored.
    let custom = listed_names(&gamelift, "CUSTOM").await;
    assert_eq!(
        custom,
        vec![surviving.to_string()],
        "the deleted location must be gone from the custom listing, got {custom:?}"
    );
    let aws = listed_names(&gamelift, "AWS").await;
    assert_eq!(
        aws, aws_before,
        "the Amazon Web Services locations must be exactly those of before the delete, got \
         {aws:?}"
    );
    assert!(
        !aws.iter().any(|location| location == deleted),
        "a custom location is never an Amazon Web Services one, got {aws:?}"
    );
    assert!(
        aws.iter().any(|location| location == REGION),
        "the Region the requests are served in must still be reported, got {aws:?}"
    );

    // And a location a fleet still deploys to is deletable too: the delete is never blocked
    // by a fleet using it.
    gamelift
        .ok("DeleteLocation", json!({"LocationName": surviving}))
        .await;
    let custom = listed_names(&gamelift, "CUSTOM").await;
    assert!(
        custom.is_empty(),
        "every created location has been deleted, got {custom:?}"
    );
}
