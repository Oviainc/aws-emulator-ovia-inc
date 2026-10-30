// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteFleet`.
//!
//! The rules under test:
//!
//! * `DeleteFleet` removes the fleet its path names from the farm its path names, so a
//!   later `GetFleet` of it is a 404 and `ListFleets` no longer reports it, while every
//!   other fleet of that farm is left exactly as it was;
//! * a fleet is reached only through its own farm, so a delete naming an unheld farm, an
//!   unheld fleet or a fleet of another farm removes nothing and answers a 404 naming the
//!   level that is missing;
//! * `clientToken` is `@idempotencyToken`, so a delete retried with the same token on the
//!   same fleet answers the first call's success instead of a 404.
//!
//! The harness (`Emulator`) and the fleet helpers are the ones the `fleet` suite drives
//! the service with, so these tests go through routing, deserialization, constraint
//! validation, the handler and serialization, exactly as the router does in production.

use std::time::Duration;

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, string_at, Emulator};
use crate::fleet::{
    create_fleet, create_fleet_path, customer_managed, get_fleet_path, page_of, service_managed,
    ROLE_ARN,
};

/// Create a fleet under `farm_id` with `configuration` and return its id.
async fn create_configured_fleet(
    emulator: &Emulator,
    farm_id: &str,
    display_name: &str,
    configuration: Value,
) -> String {
    let (status, body) = emulator
        .post(
            &create_fleet_path(farm_id),
            json!({
                "displayName": display_name,
                "roleArn": ROLE_ARN,
                "minWorkerCount": 0,
                "maxWorkerCount": 3,
                "configuration": configuration,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateFleet {display_name}: expected 200, got {status} with {body}"
    );
    string_at(&body, "fleetId")
}

/// `DELETE /2023-10-12/farms/{farmId}/fleets/{fleetId}`, with an optional client token.
async fn delete_fleet(
    emulator: &Emulator,
    farm_id: &str,
    fleet_id: &str,
    client_token: Option<&str>,
) -> (u16, Value) {
    emulator
        .call(
            "DELETE",
            &get_fleet_path(farm_id, fleet_id),
            None,
            client_token,
        )
        .await
}

#[tokio::test]
async fn delete_fleet_removes_the_named_fleet_and_leaves_the_rest() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("deleting").await;

    // The rule: the delete removes the one fleet its path names. The rows differ along
    // what the fleet is — the union member its configuration uses, whether the clock has
    // already moved it out of CREATE_IN_PROGRESS, and whether it is the first, a middle or
    // the last fleet its farm holds.
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "a customer managed fleet still CREATE_IN_PROGRESS",
            customer_managed("LINUX", "x86_64", json!({})),
            false,
        ),
        (
            "a service managed EC2 fleet",
            service_managed("WINDOWS", "x86_64", "on-demand", json!({})),
            false,
        ),
        (
            "a fleet with custom capabilities",
            customer_managed(
                "LINUX",
                "arm64",
                json!({
                    "customAmounts": [{"name": "amount.renderlicense", "min": 1.0, "max": 4.0}],
                    "customAttributes": [{"name": "attr.gpu", "values": ["a10g"]}],
                }),
            ),
            false,
        ),
        (
            "a fleet the clock has already moved to ACTIVE",
            customer_managed("MACOS", "arm64", json!({})),
            true,
        ),
    ];

    let mut fleet_ids = Vec::new();
    for (case, configuration, _) in &cases {
        fleet_ids.push(create_configured_fleet(&emulator, &farm_id, case, configuration.clone()).await);
    }
    // One fleet of another farm, so the delete is shown to be scoped to its own farm.
    let other_farm_id = emulator.farm("untouched").await;
    let stranger = create_fleet(&emulator, &other_farm_id, "stranger").await;

    // The ACTIVE row needs the create span to have elapsed before it is deleted; the
    // transition is owed by the clock, so waiting once covers it.
    if cases.iter().any(|(_, _, settled)| *settled) {
        tokio::time::sleep(Duration::from_millis(3_400)).await;
    }

    let mut remaining: Vec<String> = fleet_ids.clone();
    for ((case, _, settled), fleet_id) in cases.iter().zip(fleet_ids.iter()) {
        if *settled {
            let (status, fleet) = emulator.get(&get_fleet_path(&farm_id, fleet_id)).await;
            assert_eq!(
                (status, string_at(&fleet, "status")),
                (200, "ACTIVE".to_string()),
                "case {case:?}: expected the fleet to be ACTIVE before the delete, got {status} with {fleet}"
            );
        }

        let (status, body) = delete_fleet(&emulator, &farm_id, fleet_id, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteFleet to answer 200, got {status} with {body}"
        );
        assert!(
            body.is_null() || body.as_object().is_some_and(|members| members.is_empty()),
            "case {case:?}: expected an empty DeleteFleet response, got {body}"
        );

        // The fleet is gone: reading it back is the 404 naming it.
        let (status, body) = emulator.get(&get_fleet_path(&farm_id, fleet_id)).await;
        assert_eq!(
            (status, error_code(&body), string_at(&body, "resourceId")),
            (
                404,
                "ResourceNotFoundException".to_string(),
                fleet_id.clone()
            ),
            "case {case:?}: expected GetFleet to answer 404 for the deleted fleet, got {status} with {body}"
        );

        // Every other fleet of this farm is still there, in the order the list reports.
        remaining.retain(|id| id != fleet_id);
        let (status, body) = emulator.get(&create_fleet_path(&farm_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListFleets to answer 200, got {status} with {body}"
        );
        let (listed, _) = page_of(&body);
        assert_eq!(
            listed, remaining,
            "case {case:?}: expected ListFleets to report exactly the fleets not deleted, got {body}"
        );
        for still_held in &remaining {
            let (status, fleet) = emulator.get(&get_fleet_path(&farm_id, still_held)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the undeleted fleet {still_held} to still be readable, got {status} with {fleet}"
            );
        }
    }

    // The other farm's fleet was never in reach of these deletes.
    let (status, fleet) = emulator
        .get(&get_fleet_path(&other_farm_id, &stranger))
        .await;
    assert_eq!(
        status, 200,
        "after the deletes: expected another farm's fleet {stranger} to be untouched, got {status} with {fleet}"
    );
}

#[tokio::test]
async fn delete_fleet_refuses_a_fleet_it_does_not_hold() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("owner").await;
    let other_farm_id = emulator.farm("stranger").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "owned").await;
    let deleted_id = create_fleet(&emulator, &farm_id, "already deleted").await;
    let (status, body) = delete_fleet(&emulator, &farm_id, &deleted_id, None).await;
    assert_eq!(
        status, 200,
        "the first delete: expected 200, got {status} with {body}"
    );
    let absent_farm = absent_id("farm", "00000000000000000000000000000011");
    let absent_fleet = absent_id("fleet", "00000000000000000000000000000012");

    // The rule: a fleet hangs off its farm, so a delete naming another farm, an unheld
    // farm or an unheld fleet removes nothing and answers a 404 naming the level that is
    // missing.
    let cases: Vec<(&str, String, String, &str, String)> = vec![
        (
            "a fleet of another farm",
            other_farm_id.clone(),
            fleet_id.clone(),
            "fleet",
            fleet_id.clone(),
        ),
        (
            "an unheld farm",
            absent_farm.clone(),
            fleet_id.clone(),
            "farm",
            absent_farm.clone(),
        ),
        (
            "an unheld fleet",
            farm_id.clone(),
            absent_fleet.clone(),
            "fleet",
            absent_fleet.clone(),
        ),
        (
            "a fleet this farm already deleted",
            farm_id.clone(),
            deleted_id.clone(),
            "fleet",
            deleted_id.clone(),
        ),
    ];

    for (case, farm, fleet, resource_type, resource_id) in cases {
        let (status, body) = delete_fleet(&emulator, &farm, &fleet, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected DeleteFleet to answer 404, got {status} with {body}"
        );
        assert_eq!(
            (
                error_code(&body),
                string_at(&body, "resourceType"),
                string_at(&body, "resourceId"),
            ),
            (
                "ResourceNotFoundException".to_string(),
                resource_type.to_string(),
                resource_id.clone(),
            ),
            "case {case:?}: expected the 404 to name what is missing, got {body}"
        );
    }

    // None of those refusals removed anything: the fleet is still readable through its
    // own farm.
    let (status, fleet) = emulator.get(&get_fleet_path(&farm_id, &fleet_id)).await;
    assert_eq!(
        status, 200,
        "after the refusals: expected the fleet to still be held, got {status} with {fleet}"
    );
}

#[tokio::test]
async fn one_client_token_deletes_one_fleet() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("idempotent deletes").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "deleted once").await;
    let absent_fleet = absent_id("fleet", "00000000000000000000000000000021");

    let (status, body) = delete_fleet(&emulator, &farm_id, &fleet_id, Some("delete-one")).await;
    assert_eq!(
        status, 200,
        "the first delete: expected 200, got {status} with {body}"
    );

    // The rule: `clientToken` lets the server recognize a retry of the SAME request, so
    // only a retry naming that token AND that fleet answers the first call's success. The
    // rows differ along what changes between the two calls: nothing, the token, or the
    // fleet the delete names.
    let cases: Vec<(&str, String, Option<&str>, u16)> = vec![
        ("the same token on the same fleet", fleet_id.clone(), Some("delete-one"), 200),
        ("the same token again", fleet_id.clone(), Some("delete-one"), 200),
        ("another token on the same fleet", fleet_id.clone(), Some("delete-two"), 404),
        ("no token on the same fleet", fleet_id.clone(), None, 404),
        (
            "the same token on another fleet",
            absent_fleet.clone(),
            Some("delete-one"),
            404,
        ),
    ];

    for (case, fleet, token, expected) in cases {
        let (status, body) = delete_fleet(&emulator, &farm_id, &fleet, token).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected DeleteFleet to answer {expected}, got {status} with {body}"
        );
    }

    // A recognized retry resurrects nothing: the fleet is still gone.
    let (status, body) = emulator.get(&get_fleet_path(&farm_id, &fleet_id)).await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException".to_string()),
        "after the retries: expected the fleet to still be deleted, got {status} with {body}"
    );

    // The create token the deleted fleet spent went with it, so the same create request
    // makes a NEW fleet rather than answering with the id of one nothing holds.
    let request = json!({
        "displayName": "recreated",
        "roleArn": ROLE_ARN,
        "maxWorkerCount": 2,
        "configuration": customer_managed("LINUX", "x86_64", json!({})),
    });
    let (status, first) = emulator
        .call(
            "POST",
            &create_fleet_path(&farm_id),
            Some(request.clone()),
            Some("create-one"),
        )
        .await;
    assert_eq!(
        status, 200,
        "the create before the delete: expected 200, got {status} with {first}"
    );
    let recreated = string_at(&first, "fleetId");
    let (status, body) = delete_fleet(&emulator, &farm_id, &recreated, None).await;
    assert_eq!(
        status, 200,
        "deleting the recreated fleet: expected 200, got {status} with {body}"
    );
    let (status, second) = emulator
        .call(
            "POST",
            &create_fleet_path(&farm_id),
            Some(request),
            Some("create-one"),
        )
        .await;
    assert_eq!(
        status, 200,
        "the create after the delete: expected 200, got {status} with {second}"
    );
    let again = string_at(&second, "fleetId");
    assert_ne!(
        again, recreated,
        "after the delete: expected the create to make a new fleet, got the deleted {recreated} again"
    );
    let (status, fleet) = emulator.get(&get_fleet_path(&farm_id, &again)).await;
    assert_eq!(
        status, 200,
        "after the delete: expected the new fleet {again} to be readable, got {status} with {fleet}"
    );
}
