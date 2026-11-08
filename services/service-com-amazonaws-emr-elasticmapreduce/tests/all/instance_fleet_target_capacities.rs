// SPDX-License-Identifier: CC0-1.0

//! The target and provisioned capacities every instance fleet reports.
//!
//! THE RULE: `InstanceFleet`'s `TargetOnDemandCapacity` and `TargetSpotCapacity` are
//! `WholeNumber` (`@range` min 0) and a fleet AWS describes carries BOTH, with 0 standing for
//! a market the fleet asked for no units of — "if not specified or set to 0, only Spot
//! Instances are provisioned for the instance fleet using `TargetSpotCapacity`" reads the
//! absent value and the explicit zero as one and the same request. The provisioned
//! counterparts are reported the same way: both members, always, zero while the fleet is not
//! up. That is what lets a client read the capacity it is NOT changing and pass it back to
//! `ModifyInstanceFleet`, whose two capacities travel in one call.
//!
//! The rows span what the rule generalises over: which markets a fleet asked units of (only
//! On-Demand, only Spot, both, an explicit zero of one, an explicit zero of both), the node
//! type the fleet hosts, and whether the fleet arrived with the cluster or through
//! `AddInstanceFleet`. Every value is read back over the wire through `ListInstanceFleets`.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{at, Emr, INSTANCE_PROFILE, SERVICE_ROLE};

/// Longer than the service's clock-owed transition span (500 ms), so a fleet that was
/// `PROVISIONING` is up by the time we look.
const PAST_TRANSITION: std::time::Duration = std::time::Duration::from_millis(900);

/// One row: the capacities a fleet configuration asks for, and the ones it must report.
struct Case {
    label: &'static str,
    /// The `TargetOnDemandCapacity` the configuration carries, if any.
    requested_on_demand: Option<i64>,
    /// The `TargetSpotCapacity` the configuration carries, if any.
    requested_spot: Option<i64>,
    /// What the fleet must report for each target: the request's value, or 0.
    reported_on_demand: i64,
    reported_spot: i64,
}

const CASES: &[Case] = &[
    Case {
        label: "only On-Demand units requested",
        requested_on_demand: Some(2),
        requested_spot: None,
        reported_on_demand: 2,
        reported_spot: 0,
    },
    Case {
        label: "only Spot units requested",
        requested_on_demand: None,
        requested_spot: Some(3),
        reported_on_demand: 0,
        reported_spot: 3,
    },
    Case {
        label: "units of both markets requested",
        requested_on_demand: Some(1),
        requested_spot: Some(4),
        reported_on_demand: 1,
        reported_spot: 4,
    },
    Case {
        label: "an explicit zero of one market",
        requested_on_demand: Some(5),
        requested_spot: Some(0),
        reported_on_demand: 5,
        reported_spot: 0,
    },
    Case {
        label: "an explicit zero of both markets",
        requested_on_demand: Some(0),
        requested_spot: Some(0),
        reported_on_demand: 0,
        reported_spot: 0,
    },
];

/// The fleet configuration one row asks for, hosting `fleet_type`.
fn fleet_config(case: &Case, fleet_type: &str, name: &str) -> Value {
    let mut config = json!({
        "Name": name,
        "InstanceFleetType": fleet_type,
        "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
    });
    if let Some(units) = case.requested_on_demand {
        config["TargetOnDemandCapacity"] = json!(units);
    }
    if let Some(units) = case.requested_spot {
        config["TargetSpotCapacity"] = json!(units);
    }
    config
}

/// The fleets `ListInstanceFleets` reports for `cluster_id`, by name.
async fn fleets_by_name(emr: &Emr, case: &str, cluster_id: &str) -> Vec<(String, Value)> {
    let out = emr
        .ok("ListInstanceFleets", json!({ "ClusterId": cluster_id }))
        .await;
    at(&out, &["InstanceFleets"])
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ListInstanceFleets must answer InstanceFleets, got {out}")
        })
        .into_iter()
        .map(|fleet| {
            (
                fleet["Name"].as_str().unwrap_or_default().to_string(),
                fleet,
            )
        })
        .collect()
}

/// Assert one reported fleet carries both targets, and the provisioned units `up` implies.
fn assert_capacities(case: &str, row: &Case, got: &Value, up: bool) {
    for (member, expected) in [
        ("TargetOnDemandCapacity", row.reported_on_demand),
        ("TargetSpotCapacity", row.reported_spot),
    ] {
        assert_eq!(
            got[member].as_i64(),
            Some(expected),
            "case {case:?}: expected {member} {expected}, got {got}"
        );
    }
    for (target, provisioned) in [
        ("TargetOnDemandCapacity", "ProvisionedOnDemandCapacity"),
        ("TargetSpotCapacity", "ProvisionedSpotCapacity"),
    ] {
        let expected = if up { got[target].as_i64() } else { Some(0) };
        assert_eq!(
            got[provisioned].as_i64(),
            expected,
            "case {case:?}: expected {provisioned} {expected:?}, got {got}"
        );
    }
}

#[tokio::test]
async fn every_fleet_reports_both_target_capacities() {
    let emr = Emr::new();

    for case in CASES {
        let label = case.label;

        // One cluster per row, launched with the row's capacities on its CORE fleet and a
        // master fleet beside it, so the row is read off a fleet of a running cluster.
        let launched = emr
            .ok(
                "RunJobFlow",
                json!({
                    "Name": label,
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
                            fleet_config(case, "CORE", "core"),
                        ],
                    },
                }),
            )
            .await;
        let cluster_id = launched["JobFlowId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: RunJobFlow answers an id, got {launched}"))
            .to_string();

        // A fleet that is still acquiring its instances reports both targets and has
        // provisioned none of either.
        let provisioning = fleets_by_name(&emr, label, &cluster_id).await;
        let core = provisioning
            .iter()
            .find(|(name, _)| name == "core")
            .map(|(_, fleet)| fleet.clone())
            .unwrap_or_else(|| panic!("case {label:?}: the CORE fleet must be listed"));
        assert_capacities(label, case, &core, false);

        tokio::time::sleep(PAST_TRANSITION).await;

        // A fleet that is up has provisioned the units it asked for, and still reports
        // both targets.
        let running = fleets_by_name(&emr, label, &cluster_id).await;
        let core = running
            .iter()
            .find(|(name, _)| name == "core")
            .map(|(_, fleet)| fleet.clone())
            .unwrap_or_else(|| panic!("case {label:?}: the CORE fleet must be listed"));
        assert_capacities(label, case, &core, true);

        // The same rule holds for a TASK fleet appended after the launch, so it is the
        // fleet's own configuration that decides the values and not the launch path.
        let added = format!("added-{label}");
        emr.ok(
            "AddInstanceFleet",
            json!({
                "ClusterId": cluster_id.clone(),
                "InstanceFleet": fleet_config(case, "TASK", &added),
            }),
        )
        .await;
        let listed = fleets_by_name(&emr, label, &cluster_id).await;
        let task = listed
            .iter()
            .find(|(name, _)| *name == added)
            .map(|(_, fleet)| fleet.clone())
            .unwrap_or_else(|| panic!("case {label:?}: the added TASK fleet must be listed"));
        assert_capacities(label, case, &task, false);
    }
}
