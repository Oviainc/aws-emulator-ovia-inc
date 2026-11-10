// SPDX-License-Identifier: CC0-1.0

//! The ceiling Amazon EMR holds one instance fleet's target capacities to, end to end over
//! the wire.
//!
//! THE RULE: an operation that SETS a fleet's target capacities refuses the request when the
//! capacities the fleet would hold need more Amazon EC2 instances than Amazon EMR provisions
//! for one instance fleet — with the client error the operation models and Amazon EMR's own
//! fixed sentence for the ceiling, which carries no value from the request — and the call is
//! atomic, so a refused modification leaves the fleet's capacities exactly as they were. How
//! many instances a target needs follows from the weights the fleet's instance type
//! configurations give a unit: `WeightedCapacity` is "the number of units that a provisioned
//! instance of this type provides toward fulfilling the target capacities"
//! (`InstanceTypeConfig$WeightedCapacity` doc, Smithy model), so the smallest weight a fleet
//! offers is the one that needs the most instances.
//!
//! The rows span what the rule generalises over: which market's target is over the ceiling
//! (On-Demand, Spot, the two together), where the ceiling is crossed (at it, one past it,
//! absurdly past it), the weights the fleet's configurations give a unit (none stated, one
//! unit per instance, several units per instance), and which operation sets the capacities
//! (`RunJobFlow` at launch, `AddInstanceFleet` on a running cluster, `ModifyInstanceFleet`
//! on a fleet the cluster already carries). Every outcome is read back over the wire through
//! `ListInstanceFleets`.

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::MAX_INSTANCES_PER_FLEET;

use crate::job_flow_lifecycle::{at, Emr, INSTANCE_PROFILE, SERVICE_ROLE};
use crate::refusal_messages::message;

/// The sentence Amazon EMR refuses a target capacity past the per-fleet instance ceiling
/// with, written out here rather than read off the implementation: it is recorded behavior,
/// and it is ENTIRELY fixed text — no part of the request, and no bound of this emulator, is
/// interpolated into it.
const CEILING_SENTENCE: &str = "The maximum number of possible instances to be provisioned \
                               per instance fleet has been exceeded. Revise the \
                               configuration and resubmit.";

/// One row: the capacities a fleet is asked to hold, the weight its instance type
/// configurations give a unit, and whether the ceiling refuses it.
struct Case {
    label: &'static str,
    on_demand: i64,
    spot: i64,
    /// The `WeightedCapacity` every instance type configuration of the fleet carries, or
    /// `None` for configurations that state none (one unit per instance).
    weight: Option<i64>,
    refused: bool,
}

fn cases() -> Vec<Case> {
    let ceiling = MAX_INSTANCES_PER_FLEET;
    vec![
        Case {
            label: "one On-Demand unit, the smallest fleet there is",
            on_demand: 1,
            spot: 0,
            weight: None,
            refused: false,
        },
        Case {
            label: "On-Demand units exactly at the per-fleet instance ceiling",
            on_demand: ceiling,
            spot: 0,
            weight: None,
            refused: false,
        },
        Case {
            label: "one On-Demand unit past the ceiling",
            on_demand: ceiling + 1,
            spot: 0,
            weight: None,
            refused: true,
        },
        Case {
            label: "Spot units past the ceiling",
            on_demand: 0,
            spot: ceiling + 1,
            weight: None,
            refused: true,
        },
        Case {
            label: "the two markets together past the ceiling",
            on_demand: ceiling,
            spot: 1,
            weight: Some(1),
            refused: true,
        },
        Case {
            label: "the two markets together at the ceiling",
            on_demand: ceiling - 1,
            spot: 1,
            weight: Some(1),
            refused: false,
        },
        Case {
            label: "units an instance provides four of, so four times the units fit",
            on_demand: ceiling * 4,
            spot: 0,
            weight: Some(4),
            refused: false,
        },
        Case {
            label: "units an instance provides four of, one instance past the ceiling",
            on_demand: ceiling * 4 + 1,
            spot: 0,
            weight: Some(4),
            refused: true,
        },
        Case {
            label: "an absurd target, the one a scaling calculation gets wrong",
            on_demand: 999_999_999,
            spot: 0,
            weight: None,
            refused: true,
        },
    ]
}

/// The instance type configurations one row's fleet provisions from.
fn instance_type_configs(case: &Case) -> Value {
    let mut config = json!({ "InstanceType": "m5.xlarge" });
    if let Some(weight) = case.weight {
        config["WeightedCapacity"] = json!(weight);
    }
    json!([config])
}

/// The fleet configuration one row asks for, hosting `fleet_type`.
fn fleet_config(case: &Case, fleet_type: &str, name: &str) -> Value {
    json!({
        "Name": name,
        "InstanceFleetType": fleet_type,
        "TargetOnDemandCapacity": case.on_demand,
        "TargetSpotCapacity": case.spot,
        "InstanceTypeConfigs": instance_type_configs(case),
    })
}

/// A launch with one MASTER fleet of a single On-Demand unit, which every row builds on.
fn fleet_cluster_request(name: &str, core: Value) -> Value {
    json!({
        "Name": name,
        "ReleaseLabel": "emr-6.15.0",
        "JobFlowRole": INSTANCE_PROFILE,
        "ServiceRole": SERVICE_ROLE,
        "Instances": {
            "KeepJobFlowAliveWhenNoSteps": true,
            "InstanceFleets": [
                {
                    "Name": "master",
                    "InstanceFleetType": "MASTER",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                },
                core,
            ],
        },
    })
}

/// The fleet named `name` of `cluster_id`, as `ListInstanceFleets` reports it.
async fn fleet(emr: &Emr, case: &str, cluster_id: &str, name: &str) -> Value {
    let out = emr
        .ok("ListInstanceFleets", json!({ "ClusterId": cluster_id }))
        .await;
    at(&out, &["InstanceFleets"])
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|fleet| fleet["Name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("case {case:?}: the fleet {name:?} must be listed, got {out}"))
}

/// Assert `payload` is the ceiling refusal: HTTP 400, the client error the operation models,
/// and Amazon EMR's fixed sentence with nothing of the request interpolated into it.
fn assert_ceiling_refusal(case: &str, status: u16, payload: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    let said = message(case, payload);
    assert_eq!(
        said, CEILING_SENTENCE,
        "case {case:?}: expected the ceiling sentence, got {said:?} in {payload}"
    );
}

/// THE RULE on `ModifyInstanceFleet`: a resize past the ceiling is refused with the fixed
/// sentence under `InvalidRequestException`, the fleet keeps the capacities it had, and a
/// resize inside the ceiling is applied.
#[tokio::test]
async fn modify_instance_fleet_holds_a_resize_to_the_per_fleet_instance_ceiling() {
    let emr = Emr::new();

    for case in cases() {
        let label = case.label;
        let cluster_id = emr
            .run_job_flow(fleet_cluster_request(
                label,
                json!({
                    "Name": "core",
                    "InstanceFleetType": "CORE",
                    "TargetOnDemandCapacity": 1,
                    "TargetSpotCapacity": 0,
                    "InstanceTypeConfigs": instance_type_configs(&case),
                }),
            ))
            .await;
        let core = fleet(&emr, label, &cluster_id, "core").await;
        let fleet_id = core["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the CORE fleet must carry an Id"))
            .to_string();

        let (status, payload) = emr
            .call(
                "ModifyInstanceFleet",
                json!({
                    "ClusterId": cluster_id.clone(),
                    "InstanceFleet": {
                        "InstanceFleetId": fleet_id.clone(),
                        "TargetOnDemandCapacity": case.on_demand,
                        "TargetSpotCapacity": case.spot,
                    },
                }),
            )
            .await;

        if case.refused {
            assert_ceiling_refusal(label, status, &payload);
            // Atomic: the fleet is exactly as the launch left it.
            let after = fleet(&emr, label, &cluster_id, "core").await;
            for (member, expected) in [("TargetOnDemandCapacity", 1), ("TargetSpotCapacity", 0)] {
                assert_eq!(
                    after[member].as_i64(),
                    Some(expected),
                    "case {label:?}: a refused resize leaves {member} at {expected}, got {after}"
                );
            }
            // And a resize INSIDE the ceiling still succeeds on that same fleet.
            let (status, payload) = emr
                .call(
                    "ModifyInstanceFleet",
                    json!({
                        "ClusterId": cluster_id.clone(),
                        "InstanceFleet": {
                            "InstanceFleetId": fleet_id.clone(),
                            "TargetOnDemandCapacity": 2,
                        },
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: a resize inside the ceiling must be accepted, got \
                 {status} with {payload}"
            );
            let resized = fleet(&emr, label, &cluster_id, "core").await;
            assert_eq!(
                resized["TargetOnDemandCapacity"].as_i64(),
                Some(2),
                "case {label:?}: the accepted resize must be applied, got {resized}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {label:?}: expected the resize to be accepted, got {status} with {payload}"
        );
        let after = fleet(&emr, label, &cluster_id, "core").await;
        for (member, expected) in [
            ("TargetOnDemandCapacity", case.on_demand),
            ("TargetSpotCapacity", case.spot),
        ] {
            assert_eq!(
                after[member].as_i64(),
                Some(expected),
                "case {label:?}: expected {member} {expected}, got {after}"
            );
        }
    }
}

/// THE SAME RULE where a fleet's capacities are first SET: `AddInstanceFleet` adds no fleet
/// past the ceiling, and `RunJobFlow` launches no cluster carrying one.
#[tokio::test]
async fn adding_and_launching_a_fleet_hold_it_to_the_same_ceiling() {
    let emr = Emr::new();
    let cluster_id = emr
        .run_job_flow(fleet_cluster_request(
            "ceiling-host",
            json!({
                "Name": "core",
                "InstanceFleetType": "CORE",
                "TargetOnDemandCapacity": 1,
                "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
            }),
        ))
        .await;

    for case in cases() {
        let label = case.label;
        let added = format!("added-{label}");

        let (status, payload) = emr
            .call(
                "AddInstanceFleet",
                json!({
                    "ClusterId": cluster_id.clone(),
                    "InstanceFleet": fleet_config(&case, "TASK", &added),
                }),
            )
            .await;
        if case.refused {
            assert_ceiling_refusal(label, status, &payload);
            let listed = emr
                .ok("ListInstanceFleets", json!({ "ClusterId": cluster_id.clone() }))
                .await;
            let names: Vec<&str> = at(&listed, &["InstanceFleets"])
                .as_array()
                .map(|fleets| {
                    fleets
                        .iter()
                        .filter_map(|fleet| fleet["Name"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                !names.contains(&added.as_str()),
                "case {label:?}: a refused AddInstanceFleet adds no fleet, got {names:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {label:?}: expected the fleet to be added, got {status} with {payload}"
            );
            let reported = fleet(&emr, label, &cluster_id, &added).await;
            assert_eq!(
                reported["TargetOnDemandCapacity"].as_i64(),
                Some(case.on_demand),
                "case {label:?}: the added fleet reports the capacity it was given, got \
                 {reported}"
            );
        }

        // The launch path: one cluster per row, whose CORE fleet carries the row's
        // capacities.
        let (status, payload) = emr
            .call(
                "RunJobFlow",
                fleet_cluster_request(label, fleet_config(&case, "CORE", "core")),
            )
            .await;
        if case.refused {
            assert_eq!(
                status, 400,
                "case {label:?}: expected the launch to be refused, got {status} with {payload}"
            );
            let said = message(label, &payload);
            assert_eq!(
                said, CEILING_SENTENCE,
                "case {label:?}: expected the ceiling sentence from RunJobFlow, got \
                 {said:?} in {payload}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {label:?}: expected the launch to be accepted, got {status} with {payload}"
            );
            let launched = payload["JobFlowId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: RunJobFlow answers an id"))
                .to_string();
            let reported = fleet(&emr, label, &launched, "core").await;
            assert_eq!(
                reported["TargetSpotCapacity"].as_i64(),
                Some(case.spot),
                "case {label:?}: the launched fleet reports the capacity it was given, got \
                 {reported}"
            );
        }
    }
}
