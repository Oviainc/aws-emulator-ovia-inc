// SPDX-License-Identifier: CC0-1.0

//! The instance fleets of an EMR cluster, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so these are the
//! same bytes an SDK client sends. Nothing reaches into the service's state struct: a
//! fleet is only ever read back through `ListInstanceFleets`, which is what makes these
//! tests evidence that the listing is computed from persisted state.
//!
//! THE RULE: `ListInstanceFleets` reports the instance fleets the named cluster
//! persisted — the ones its `RunJobFlow` request configured, then every
//! `AddInstanceFleet` — in creation order, each as a projection of the stored
//! configuration plus the id EMR minted and the lifecycle the clock moves, in pages a
//! `Marker` resumes; a cluster id the service holds no cluster for, and a marker no fleet
//! of that cluster carries, are refused with the modelled 400 `InvalidRequestException`.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{assert_minted_id, assert_refused, at, Emr};

/// Longer than the service's clock-owed transition span (500 ms), so a fleet that was
/// `PROVISIONING`, and a cluster that was `TERMINATING`, has reached its next state by
/// the time we look.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// The chosen bounds this suite takes against the fleet store (`crate::state`'s
/// `LIST_INSTANCE_FLEETS_PAGE_SIZE` and `MAX_INSTANCE_FLEETS_PER_CLUSTER`).
const PAGE_SIZE: usize = 20;
const MAX_INSTANCE_FLEETS_PER_CLUSTER: usize = 50;

/// Cluster ids no cluster carries: the shape a real id has, a prefix with nothing after
/// it, the empty string (`ClusterId` is `len 0..256`, so the boundary validator passes it
/// on to the handler) and a value that is no cluster id at all.
const UNKNOWN_CLUSTER_IDS: &[&str] = &["j-ZZZZZZZZZZZZZ", "j-", "", "not-a-cluster-id"];

/// What one reported `InstanceFleet` must carry. Every field is what the request asked
/// for, so a row states the request's own values rather than anything the emulator
/// invents.
struct Expected {
    fleet_type: &'static str,
    name: Option<&'static str>,
    target_on_demand: Option<i64>,
    target_spot: Option<i64>,
    /// `(instance type, weighted capacity, bid price as percentage)` per instance type
    /// the fleet configured, in configuration order.
    instance_types: &'static [(&'static str, Option<i64>, Option<f64>)],
    /// `(device name, volume type)` per EBS volume the fleet's first instance type
    /// requested, in order.
    ebs_devices: &'static [(&'static str, &'static str)],
}

/// The `Instances` member of a `RunJobFlow` request launching `fleets`.
fn launch(name: &str, fleets: Value) -> Value {
    json!({
        "Name": name,
        "ReleaseLabel": "emr-6.15.0",
        "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
        "Instances": { "KeepJobFlowAliveWhenNoSteps": true, "InstanceFleets": fleets },
    })
}

/// A minimal fleet configuration of `fleet_type`, enough for a cluster to launch with.
fn fleet(fleet_type: &str, instance_type: &str) -> Value {
    json!({
        "InstanceFleetType": fleet_type,
        "TargetOnDemandCapacity": 1,
        "InstanceTypeConfigs": [{ "InstanceType": instance_type }],
    })
}

/// The fleets `ListInstanceFleets` reports for `cluster_id`, every page of them:
/// following the `Marker` to its end is what collects the whole listing.
async fn list_all_fleets(emr: &Emr, case: &str, cluster_id: &str) -> Vec<Value> {
    let mut all: Vec<Value> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let mut request = json!({ "ClusterId": cluster_id });
        if let Some(token) = marker.as_ref() {
            request["Marker"] = json!(token);
        }
        let out = emr.ok("ListInstanceFleets", request).await;
        let page = out["InstanceFleets"].as_array().cloned().unwrap_or_else(|| {
            panic!("case {case:?}: ListInstanceFleets must answer InstanceFleets, got {out}")
        });
        assert!(
            page.len() <= PAGE_SIZE,
            "case {case:?}: a page holds at most {PAGE_SIZE} fleets, got {}",
            page.len()
        );
        all.extend(page);
        match out["Marker"].as_str() {
            Some(token) => marker = Some(token.to_string()),
            None => break,
        }
    }
    all
}

/// Assert one reported fleet is the projection of what the request configured.
fn assert_fleet(case: &str, expected: &Expected, got: &Value) {
    assert_minted_id(case, "if-", got["Id"].as_str().unwrap_or_default());
    assert_eq!(
        got["InstanceFleetType"].as_str(),
        Some(expected.fleet_type),
        "case {case:?}: wrong InstanceFleetType, got {got}"
    );
    assert_eq!(
        got["Name"].as_str(),
        expected.name,
        "case {case:?}: wrong Name, got {got}"
    );
    assert_eq!(
        got["TargetOnDemandCapacity"].as_i64(),
        expected.target_on_demand,
        "case {case:?}: wrong TargetOnDemandCapacity, got {got}"
    );
    assert_eq!(
        got["TargetSpotCapacity"].as_i64(),
        expected.target_spot,
        "case {case:?}: wrong TargetSpotCapacity, got {got}"
    );

    // One InstanceTypeSpecification per configured instance type, in configuration
    // order, carrying the members the configuration named.
    let reported: Vec<(&str, Option<i64>, Option<f64>)> = got["InstanceTypeSpecifications"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["InstanceType"].as_str().unwrap_or_default(),
                        entry["WeightedCapacity"].as_i64(),
                        entry["BidPriceAsPercentageOfOnDemandPrice"].as_f64(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported, expected.instance_types,
        "case {case:?}: wrong InstanceTypeSpecifications, got {got}"
    );

    let devices: Vec<(&str, &str)> = got["InstanceTypeSpecifications"]
        .get(0)
        .and_then(|first| first.get("EbsBlockDevices"))
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|device| {
                    (
                        device["Device"].as_str().unwrap_or_default(),
                        at(device, &["VolumeSpecification", "VolumeType"])
                            .as_str()
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        devices, expected.ebs_devices,
        "case {case:?}: wrong EbsBlockDevices, got {got}"
    );
}

/// THE RULE above, over the fleet shapes `RunJobFlow` accepts: one fleet and three, a
/// named fleet and an unnamed one, On-Demand targets and Spot ones, one instance type and
/// several, with and without EBS volumes, launch and resize specifications.
#[tokio::test]
async fn list_instance_fleets_reports_every_fleet_a_cluster_was_launched_with() {
    struct Case {
        label: &'static str,
        fleets: Value,
        expected: &'static [Expected],
    }

    let cases = vec![
        Case {
            label: "one master fleet, nothing optional configured",
            fleets: json!([fleet("MASTER", "m5.xlarge")]),
            expected: &[Expected {
                fleet_type: "MASTER",
                name: None,
                target_on_demand: Some(1),
                target_spot: None,
                instance_types: &[("m5.xlarge", None, None)],
                ebs_devices: &[],
            }],
        },
        Case {
            label: "master, core and task fleets, named, Spot and weighted",
            fleets: json!([
                {
                    "Name": "primary",
                    "InstanceFleetType": "MASTER",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.2xlarge" }],
                },
                {
                    "Name": "core",
                    "InstanceFleetType": "CORE",
                    "TargetOnDemandCapacity": 2,
                    "TargetSpotCapacity": 3,
                    "InstanceTypeConfigs": [
                        { "InstanceType": "r5.xlarge", "WeightedCapacity": 2 },
                        {
                            "InstanceType": "r5.2xlarge",
                            "WeightedCapacity": 4,
                            "BidPriceAsPercentageOfOnDemandPrice": 60.0,
                        },
                    ],
                },
                {
                    "InstanceFleetType": "TASK",
                    "TargetSpotCapacity": 5,
                    "InstanceTypeConfigs": [{ "InstanceType": "c5.xlarge" }],
                    "LaunchSpecifications": {
                        "SpotSpecification": {
                            "TimeoutDurationMinutes": 30,
                            "TimeoutAction": "SWITCH_TO_ON_DEMAND",
                        },
                    },
                },
            ]),
            expected: &[
                Expected {
                    fleet_type: "MASTER",
                    name: Some("primary"),
                    target_on_demand: Some(1),
                    target_spot: None,
                    instance_types: &[("m5.2xlarge", None, None)],
                    ebs_devices: &[],
                },
                Expected {
                    fleet_type: "CORE",
                    name: Some("core"),
                    target_on_demand: Some(2),
                    target_spot: Some(3),
                    instance_types: &[
                        ("r5.xlarge", Some(2), None),
                        ("r5.2xlarge", Some(4), Some(60.0)),
                    ],
                    ebs_devices: &[],
                },
                Expected {
                    fleet_type: "TASK",
                    name: None,
                    target_on_demand: None,
                    target_spot: Some(5),
                    instance_types: &[("c5.xlarge", None, None)],
                    ebs_devices: &[],
                },
            ],
        },
        Case {
            label: "one fleet whose instance type asks for three EBS volumes",
            fleets: json!([{
                "InstanceFleetType": "MASTER",
                "TargetOnDemandCapacity": 1,
                "InstanceTypeConfigs": [{
                    "InstanceType": "m5.xlarge",
                    "EbsConfiguration": {
                        "EbsOptimized": true,
                        "EbsBlockDeviceConfigs": [
                            {
                                "VolumeSpecification": { "VolumeType": "gp3", "SizeInGB": 64 },
                                "VolumesPerInstance": 2,
                            },
                            {
                                "VolumeSpecification": { "VolumeType": "st1", "SizeInGB": 500 },
                            },
                        ],
                    },
                }],
            }]),
            expected: &[Expected {
                fleet_type: "MASTER",
                name: None,
                target_on_demand: Some(1),
                target_spot: None,
                instance_types: &[("m5.xlarge", None, None)],
                // `/dev/sda` is the root volume, so the requested volumes start at
                // `/dev/sdb`, in configuration order.
                ebs_devices: &[("/dev/sdb", "gp3"), ("/dev/sdc", "gp3"), ("/dev/sdd", "st1")],
            }],
        },
    ];

    let emr = Emr::new();
    for case in &cases {
        let label = case.label;
        let id = emr.run_job_flow(launch(label, case.fleets.clone())).await;
        let fleets = list_all_fleets(&emr, label, &id).await;
        assert_eq!(
            fleets.len(),
            case.expected.len(),
            "case {label:?}: wrong number of reported fleets, got {fleets:?}"
        );
        for (expected, got) in case.expected.iter().zip(&fleets) {
            assert_fleet(label, expected, got);
        }

        // Every fleet of one cluster carries its own minted id.
        let ids: BTreeSet<&str> = fleets
            .iter()
            .map(|fleet| fleet["Id"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            ids.len(),
            fleets.len(),
            "case {label:?}: fleet ids must be distinct, got {ids:?}"
        );

        // A listing is a read: it reports the same fleets, in the same order, however
        // many times it is called.
        let again = list_all_fleets(&emr, label, &id).await;
        assert_eq!(
            again, fleets,
            "case {label:?}: a second listing must report the same fleets"
        );
    }
}

/// THE RULE: a fleet AWS answers in a non-terminal state reaches its next state once the
/// clock makes the transition due — `PROVISIONING` becomes `RUNNING`, and that is
/// recorded as `Timeline.ReadyDateTime` — and a fleet has provisioned capacity toward its
/// targets only once it is up. A fleet of a cluster that has terminated is `TERMINATED`,
/// for the modelled reason `CLUSTER_TERMINATED`.
#[tokio::test]
async fn a_provisioning_fleet_reaches_running_when_the_clock_makes_it_due() {
    let emr = Emr::new();
    let id = emr
        .run_job_flow(launch(
            "clock",
            json!([
                fleet("MASTER", "m5.xlarge"),
                {
                    "InstanceFleetType": "CORE",
                    "TargetOnDemandCapacity": 2,
                    "TargetSpotCapacity": 4,
                    "InstanceTypeConfigs": [{ "InstanceType": "r5.xlarge" }],
                },
            ]),
        ))
        .await;

    // A brand-new fleet is acquiring its instances: it is not up, so none of its target
    // capacity is provisioned yet, and it has no ready instant.
    let provisioning = list_all_fleets(&emr, "provisioning", &id).await;
    for got in &provisioning {
        assert_eq!(
            at(got, &["Status", "State"]).as_str(),
            Some("PROVISIONING"),
            "case \"provisioning\": a new fleet is PROVISIONING, got {got}"
        );
        for (target, provisioned) in [
            ("TargetOnDemandCapacity", "ProvisionedOnDemandCapacity"),
            ("TargetSpotCapacity", "ProvisionedSpotCapacity"),
        ] {
            // A fleet that asked for no units of a market reports none for it; one that
            // did has provisioned none of them while it is still acquiring instances.
            assert_eq!(
                got[provisioned].as_i64(),
                got[target].as_i64().map(|_| 0),
                "case \"provisioning\": a fleet that is not up has provisioned no \
                 {target}, got {got}"
            );
        }
        assert!(
            at(got, &["Status", "Timeline", "ReadyDateTime"]).is_null()
                && at(got, &["Status", "Timeline", "EndDateTime"]).is_null()
                && !at(got, &["Status", "Timeline", "CreationDateTime"]).is_null(),
            "case \"provisioning\": only the creation instant is recorded yet, got {got}"
        );
        assert!(
            at(got, &["Status", "StateChangeReason"]).is_null(),
            "case \"provisioning\": nothing moved this fleet's state, got {got}"
        );
    }

    tokio::time::sleep(PAST_TRANSITION).await;

    let running = list_all_fleets(&emr, "running", &id).await;
    for got in &running {
        assert_eq!(
            at(got, &["Status", "State"]).as_str(),
            Some("RUNNING"),
            "case \"running\": a fleet past its span is RUNNING, got {got}"
        );
        assert!(
            !at(got, &["Status", "Timeline", "ReadyDateTime"]).is_null(),
            "case \"running\": a fleet that is up records when it became ready, got {got}"
        );
        for (target, provisioned) in [
            ("TargetOnDemandCapacity", "ProvisionedOnDemandCapacity"),
            ("TargetSpotCapacity", "ProvisionedSpotCapacity"),
        ] {
            assert_eq!(
                got[provisioned].as_i64(),
                got[target].as_i64(),
                "case \"running\": {provisioned} must fulfill {target}, got {got}"
            );
        }
    }

    // Terminating the cluster ends its fleets, at the instant the termination came due.
    emr.ok("TerminateJobFlows", json!({ "JobFlowIds": [id.clone()] }))
        .await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let ended = list_all_fleets(&emr, "terminated", &id).await;
    assert_eq!(
        ended.len(),
        running.len(),
        "case \"terminated\": a terminated cluster still reports its fleets, got {ended:?}"
    );
    for got in &ended {
        assert_eq!(
            at(got, &["Status", "State"]).as_str(),
            Some("TERMINATED"),
            "case \"terminated\": a fleet of a terminated cluster is TERMINATED, got {got}"
        );
        assert_eq!(
            at(got, &["Status", "StateChangeReason", "Code"]).as_str(),
            Some("CLUSTER_TERMINATED"),
            "case \"terminated\": wrong StateChangeReason, got {got}"
        );
        assert!(
            !at(got, &["Status", "Timeline", "EndDateTime"]).is_null(),
            "case \"terminated\": a fleet that has ended records when, got {got}"
        );
        assert_eq!(
            got["ProvisionedOnDemandCapacity"].as_i64(),
            Some(0),
            "case \"terminated\": a fleet that has ended holds no instances, got {got}"
        );
    }
}

/// THE RULE: the listing is paginated on `Marker` — a page reports at most the page size,
/// and the marker resumes the listing at the first fleet the page did not report — and a
/// fleet `AddInstanceFleet` appends is reported after the ones the cluster launched with.
#[tokio::test]
async fn the_listing_pages_on_marker_and_follows_every_added_fleet() {
    let emr = Emr::new();
    let id = emr
        .run_job_flow(launch("paged", json!([fleet("MASTER", "m5.xlarge")])))
        .await;
    tokio::time::sleep(PAST_TRANSITION).await;

    // Fill the cluster to its fleet ceiling, so more fleets exist than one page reports.
    let mut added: Vec<String> = Vec::new();
    for index in 1..MAX_INSTANCE_FLEETS_PER_CLUSTER {
        let out = emr
            .ok(
                "AddInstanceFleet",
                json!({
                    "ClusterId": id.clone(),
                    "InstanceFleet": {
                        "Name": format!("task-{index}"),
                        "InstanceFleetType": "TASK",
                        "TargetSpotCapacity": 1,
                        "InstanceTypeConfigs": [{ "InstanceType": "c5.xlarge" }],
                    },
                }),
            )
            .await;
        added.push(
            out["InstanceFleetId"]
                .as_str()
                .unwrap_or_else(|| panic!("AddInstanceFleet must answer an id, got {out}"))
                .to_string(),
        );
    }

    // The first page stops at the page size and names where to resume.
    let first = emr
        .ok("ListInstanceFleets", json!({ "ClusterId": id.clone() }))
        .await;
    assert_eq!(
        first["InstanceFleets"].as_array().map(Vec::len),
        Some(PAGE_SIZE),
        "case \"paged\": the first page holds exactly {PAGE_SIZE} fleets, got {first}"
    );
    let marker = first["Marker"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"paged\": the first page must carry a Marker, got {first}"));
    assert_eq!(
        Some(marker),
        added.get(PAGE_SIZE - 1).map(String::as_str),
        "case \"paged\": the Marker names the first fleet the page did not report"
    );

    // Following it to the end collects every fleet, launched and added, in order.
    let all = list_all_fleets(&emr, "paged", &id).await;
    let reported: Vec<&str> = all
        .iter()
        .map(|fleet| fleet["Id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        reported.len(),
        MAX_INSTANCE_FLEETS_PER_CLUSTER,
        "case \"paged\": every fleet must be reported once, got {reported:?}"
    );
    assert_eq!(
        reported[1..],
        added.iter().map(String::as_str).collect::<Vec<_>>()[..],
        "case \"paged\": added fleets follow the launched one, in the order they were added"
    );
    assert_eq!(
        all.last().and_then(|fleet| fleet["Name"].as_str()),
        Some(format!("task-{}", MAX_INSTANCE_FLEETS_PER_CLUSTER - 1)).as_deref(),
        "case \"paged\": the last page reports the last added fleet"
    );
}

/// THE RULE: a cluster id this service holds no cluster for, and a marker no fleet of the
/// named cluster carries, are refused with the modelled 400 `InvalidRequestException` —
/// never an empty listing, which would say the cluster exists and has no fleets.
#[tokio::test]
async fn an_unknown_cluster_or_marker_is_refused() {
    let emr = Emr::new();
    for cluster_id in UNKNOWN_CLUSTER_IDS {
        let (status, payload) = emr
            .call("ListInstanceFleets", json!({ "ClusterId": cluster_id }))
            .await;
        assert_refused(cluster_id, status, &payload, "InvalidRequestException");
    }

    let id = emr
        .run_job_flow(launch("markers", json!([fleet("MASTER", "m5.xlarge")])))
        .await;
    // A marker that is no fleet id, one belonging to no fleet, and the empty string.
    for marker in ["if-ZZZZZZZZZZZZZ", "not-a-marker", ""] {
        let (status, payload) = emr
            .call(
                "ListInstanceFleets",
                json!({ "ClusterId": id.clone(), "Marker": marker }),
            )
            .await;
        assert_refused(marker, status, &payload, "InvalidRequestException");
    }
}
