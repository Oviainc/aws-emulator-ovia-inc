// SPDX-License-Identifier: CC0-1.0

//! What this service REPORTS when there is nothing to report, and how it words the two
//! refusals a client branches on.
//!
//! Three rules, each asserted over a table rather than on one recorded request:
//!
//! * `GetHomeRegionResult$HomeRegion` is an OPTIONAL member and the operation returns the
//!   home region "if configured", so an account with no home region control is answered by
//!   a success carrying no region — never by an error, which is what a caller discovering
//!   the home region cannot act on.
//! * a value outside a member's modelled `@length`/`@pattern`/`@range` is this API's
//!   `InvalidInputException` (HTTP 400), stated as the validation envelope naming the
//!   member and the constraint it broke.
//! * a well-formed `ControlId` that names no control is the same error, naming the id the
//!   request asked for.
//!
//! Every case drives the service the way a client does, through the shared
//! [`crate::harness`]: a `POST /` with
//! `X-Amz-Target: AWSMigrationHubMultiAccountService.<Operation>` through the
//! `MultiServiceRouter`, so routing, deserialization, constraint validation, the handler
//! and serialization all run.

use serde_json::{json, Value};

use crate::harness::{
    create_body, error_code, message, Emulator, HOME_REGIONS, HOME_REGION_PATTERN,
};

#[tokio::test]
async fn get_home_region_reports_an_unset_region_as_a_success_carrying_no_region() {
    for region in HOME_REGIONS {
        let emulator = Emulator::new();

        // Unset: a 200 whose body carries no member at all.
        let (status, body) = emulator.call("GetHomeRegion", &json!({})).await;
        assert_eq!(
            (status, body.clone()),
            (200, json!({})),
            "case {region:?} before create: expected 200 with no HomeRegion, got {status} {body}"
        );

        let (status, created) = emulator
            .call("CreateHomeRegionControl", &create_body(region, false))
            .await;
        assert_eq!(status, 200, "case {region:?}: create got {status} {created}");
        let control_id = created["HomeRegionControl"]["ControlId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let (status, body) = emulator.call("GetHomeRegion", &json!({})).await;
        assert_eq!(
            (status, body["HomeRegion"].as_str()),
            (200, Some(*region)),
            "case {region:?} after create: got {status} {body}"
        );

        // Reading it twice does not change it: the read reports, it never writes.
        let (status, again) = emulator.call("GetHomeRegion", &json!({})).await;
        assert_eq!(
            (status, again["HomeRegion"].as_str()),
            (200, Some(*region)),
            "case {region:?} read twice: got {status} {again}"
        );

        let (status, body) = emulator
            .call("DeleteHomeRegionControl", &json!({ "ControlId": control_id }))
            .await;
        assert_eq!(status, 200, "case {region:?}: delete got {status} {body}");

        // Unset again, and again a success with no region.
        let (status, body) = emulator.call("GetHomeRegion", &json!({})).await;
        assert_eq!(
            (status, body.clone()),
            (200, json!({})),
            "case {region:?} after delete: expected 200 with no HomeRegion, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_value_outside_a_modeled_constraint_names_the_member_and_the_constraint() {
    let emulator = Emulator::new();

    // Rows span the constraint classes the model declares on this service's input
    // members, and the forms a bad region takes: wrong case, underscores, punctuation,
    // too few segments, a numeric segment that is not numeric, and the `@length` bound
    // (where a value breaks TWO constraints at once, so the envelope counts both).
    // (case, operation, the whole message, the request)
    let pattern_clause =
        format!("failed to satisfy constraint: Member must satisfy regular expression pattern: {HOME_REGION_PATTERN}");
    let rows: Vec<(&str, &str, String, Value)> = vec![
        (
            "uppercase region",
            "CreateHomeRegionControl",
            format!("1 validation error detected: Value 'US-WEST-2' at 'homeRegion' {pattern_clause}"),
            create_body("US-WEST-2", false),
        ),
        (
            "region with punctuation",
            "CreateHomeRegionControl",
            format!("1 validation error detected: Value 'Not_A_Region!' at 'homeRegion' {pattern_clause}"),
            create_body("Not_A_Region!", false),
        ),
        (
            "region missing its ordinal",
            "CreateHomeRegionControl",
            format!("1 validation error detected: Value 'eu-west' at 'homeRegion' {pattern_clause}"),
            create_body("eu-west", false),
        ),
        (
            "region whose ordinal is a word",
            "CreateHomeRegionControl",
            format!("1 validation error detected: Value 'eu-west-one' at 'homeRegion' {pattern_clause}"),
            create_body("eu-west-one", false),
        ),
        (
            "region below the minimum length",
            "CreateHomeRegionControl",
            format!(
                "2 validation errors detected: Value '' at 'homeRegion' failed to satisfy \
                 constraint: Member must have length greater than or equal to 1; Value '' at \
                 'homeRegion' {pattern_clause}"
            ),
            create_body("", false),
        ),
        (
            "control id outside its pattern",
            "DeleteHomeRegionControl",
            "1 validation error detected: Value 'hrc-NOT-AN-ID' at 'controlId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^hrc-[a-z0-9]{12}$".to_string(),
            json!({ "ControlId": "hrc-NOT-AN-ID" }),
        ),
        (
            "page size above the range",
            "DescribeHomeRegionControls",
            "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".to_string(),
            json!({ "MaxResults": 101 }),
        ),
        (
            "page size below the range",
            "DescribeHomeRegionControls",
            "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1".to_string(),
            json!({ "MaxResults": 0 }),
        ),
    ];

    for (case, operation, expected, request) in rows {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInputException"),
            "case {case:?}: expected 400 InvalidInputException, got {status} {body}"
        );
        let stated = message(&body);
        assert_eq!(
            stated, expected,
            "case {case:?}: the refusal must name the member and the constraint, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_control_that_does_not_exist_names_the_requested_id() {
    let emulator = Emulator::new();

    // A control that DOES exist, so the store is not merely empty: every row below still
    // names an id that is not this one.
    let (status, created) = emulator
        .call("CreateHomeRegionControl", &create_body(HOME_REGIONS[0], false))
        .await;
    assert_eq!(status, 200, "setup create got {status} {created}");
    let existing = created["HomeRegionControl"]["ControlId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Ids spanning the model's `ControlId` alphabet (`^hrc-[a-z0-9]{12}$`): all digits,
    // all letters, and a mix.
    let unknown = [
        "hrc-000000000000",
        "hrc-abcdefghijkl",
        "hrc-0a1b2c3d4e5f",
        "hrc-zzzzzzzzzzzz",
    ];
    for id in unknown {
        let (status, body) = emulator
            .call("DeleteHomeRegionControl", &json!({ "ControlId": id }))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "InvalidInputException"),
            "case {id:?}: expected 400 InvalidInputException, got {status} {body}"
        );
        let stated = message(&body);
        assert_eq!(
            stated,
            format!("Home Region Control ID \"{id}\" does not exist"),
            "case {id:?}: the refusal must name the requested id, got {stated:?}"
        );
    }

    // None of those refusals touched the control that does exist.
    let (status, body) = emulator.call("DescribeHomeRegionControls", &json!({})).await;
    assert_eq!(
        (
            status,
            body["HomeRegionControls"][0]["ControlId"].as_str()
        ),
        (200, Some(existing.as_str())),
        "a refused delete must leave the stored control alone, got {status} {body}"
    );

    // And deleting the one that exists twice: the second call names it the same way.
    let (status, body) = emulator
        .call(
            "DeleteHomeRegionControl",
            &json!({ "ControlId": existing.clone() }),
        )
        .await;
    assert_eq!(status, 200, "deleting the stored control got {status} {body}");
    let (status, body) = emulator
        .call(
            "DeleteHomeRegionControl",
            &json!({ "ControlId": existing.clone() }),
        )
        .await;
    assert_eq!(
        (status, message(&body).to_string()),
        (
            400,
            format!("Home Region Control ID \"{existing}\" does not exist")
        ),
        "deleting {existing} twice must name it as nonexistent, got {status} {body}"
    );
}
