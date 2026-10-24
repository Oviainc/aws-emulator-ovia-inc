// SPDX-License-Identifier: CC0-1.0

//! The capacity configuration a rescore execution plan carries, end to end over the
//! wire.
//!
//! The rule under test: a rescore execution plan ALWAYS has a capacity configuration,
//! so `DescribeRescoreExecutionPlan` always reports `CapacityUnits`, and a plan given
//! no additional capacity units carries `RescoreCapacityUnits: 0` — "A capacity of zero
//! indicates that the rescore execution plan is using the default capacity."
//! (`DescribeRescoreExecutionPlanResponse$CapacityUnits`), because not setting the
//! member asks for zero EXTRA units ("You are given a single capacity unit by default.
//! If you want to use the default capacity, you don't set additional capacity units." —
//! `CreateRescoreExecutionPlanRequest$CapacityUnits`).
//!
//! Each rule is asserted over a TABLE whose rows differ along the capacity the plan was
//! configured with — the member absent, the bottom of the published range, and values
//! above it — and over the call that configured it (create, then update).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state;

use crate::harness::{call, emulator, sleep_past_the_transition};

/// Describe `id`, asserting the call succeeded, and return the body.
async fn describe(router: &MultiServiceRouter, id: &str) -> Value {
    let (status, described) =
        call(router, "DescribeRescoreExecutionPlan", json!({ "Id": id })).await;
    assert_eq!(status, 200, "describe {id}: got {described}");
    described
}

/// The `CapacityUnits.RescoreCapacityUnits` a describe reported, or `None` when the
/// answer omitted either level.
fn reported_capacity(described: &Value) -> Option<i64> {
    described["CapacityUnits"]["RescoreCapacityUnits"].as_i64()
}

/// One row of the create table: the `CapacityUnits` member the create carried
/// (`Value::Null` for a create that carries no such member at all), and the capacity the
/// plan must report afterwards.
struct CreateCase {
    case: &'static str,
    capacity_units: Value,
    expected: i64,
}

/// Create a plan named `name` carrying `capacity_units` (`Value::Null` to omit the
/// member) and return its id.
async fn create_with_capacity(
    router: &MultiServiceRouter,
    name: &str,
    capacity_units: &Value,
) -> String {
    let mut request = json!({ "Name": name });
    if !capacity_units.is_null() {
        request["CapacityUnits"] = capacity_units.clone();
    }
    let (status, created) = call(router, "CreateRescoreExecutionPlan", request).await;
    assert_eq!(status, 200, "create {name}: got {created}");
    created["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("create {name}: no Id in {created}"))
        .to_string()
}

#[tokio::test]
async fn describe_always_reports_the_capacity_a_create_configured() {
    let router = emulator();

    // The rows differ along the capacity the create configured. The absent-member row is
    // the one the default rule is about; the `0` row is the bottom of
    // `CapacityUnitsConfiguration$RescoreCapacityUnits`'s published `@range` (min 0), and
    // the others are values above it, including one no other suite in this crate cites.
    let cases = [
        CreateCase {
            case: "no CapacityUnits member at all",
            capacity_units: Value::Null,
            expected: state::DEFAULT_RESCORE_CAPACITY_UNITS.into(),
        },
        CreateCase {
            case: "the bottom of the published range",
            capacity_units: json!({ "RescoreCapacityUnits": 0 }),
            expected: 0,
        },
        CreateCase {
            case: "one additional capacity unit",
            capacity_units: json!({ "RescoreCapacityUnits": 1 }),
            expected: 1,
        },
        CreateCase {
            case: "many additional capacity units",
            capacity_units: json!({ "RescoreCapacityUnits": 42 }),
            expected: 42,
        },
        CreateCase {
            case: "the top of the integer the member is modelled as",
            capacity_units: json!({ "RescoreCapacityUnits": i32::MAX }),
            expected: i64::from(i32::MAX),
        },
    ];

    for (index, CreateCase {
        case,
        capacity_units,
        expected,
    }) in cases.iter().enumerate()
    {
        let id = create_with_capacity(&router, &format!("capacity-plan-{index}"), capacity_units)
            .await;

        // Reported the same way while the plan is still CREATING and once the clock owes
        // it ACTIVE: the capacity is a configuration, not a function of the status.
        let described = describe(&router, &id).await;
        assert_eq!(
            reported_capacity(&described),
            Some(*expected),
            "case {case:?}: expected CapacityUnits.RescoreCapacityUnits {expected}, got {described}"
        );

        sleep_past_the_transition();
        let described = describe(&router, &id).await;
        assert_eq!(
            reported_capacity(&described),
            Some(*expected),
            "case {case:?} (settled): expected CapacityUnits.RescoreCapacityUnits \
             {expected}, got {described}"
        );
    }
}

/// One row of the update table: the capacity the plan was created with, the
/// `CapacityUnits` member the update carried, and the capacity the plan must report
/// afterwards.
struct UpdateCase {
    case: &'static str,
    created_with: Value,
    update_with: Value,
    expected: i64,
}

#[tokio::test]
async fn describe_always_reports_the_capacity_after_an_update() {
    let router = emulator();

    let cases = [
        UpdateCase {
            case: "a default-capacity plan updated without CapacityUnits",
            created_with: Value::Null,
            update_with: Value::Null,
            expected: state::DEFAULT_RESCORE_CAPACITY_UNITS.into(),
        },
        UpdateCase {
            case: "a default-capacity plan given additional units",
            created_with: Value::Null,
            update_with: json!({ "RescoreCapacityUnits": 5 }),
            expected: 5,
        },
        UpdateCase {
            case: "a provisioned plan taken back to the default capacity",
            created_with: json!({ "RescoreCapacityUnits": 3 }),
            update_with: json!({ "RescoreCapacityUnits": 0 }),
            expected: 0,
        },
        UpdateCase {
            case: "a provisioned plan updated without CapacityUnits keeps its capacity",
            created_with: json!({ "RescoreCapacityUnits": 7 }),
            update_with: Value::Null,
            expected: 7,
        },
    ];

    for (index, UpdateCase {
        case,
        created_with,
        update_with,
        expected,
    }) in cases.iter().enumerate()
    {
        let id =
            create_with_capacity(&router, &format!("updated-plan-{index}"), created_with).await;
        // An update is only accepted while the plan is ACTIVE.
        sleep_past_the_transition();

        let mut request = json!({ "Id": id, "Name": format!("updated-plan-{index}-renamed") });
        if !update_with.is_null() {
            request["CapacityUnits"] = update_with.clone();
        }
        let (status, updated) = call(&router, "UpdateRescoreExecutionPlan", request).await;
        assert_eq!(status, 200, "case {case:?}: update got {updated}");

        let described = describe(&router, &id).await;
        assert_eq!(
            reported_capacity(&described),
            Some(*expected),
            "case {case:?}: expected CapacityUnits.RescoreCapacityUnits {expected}, got {described}"
        );
    }
}
