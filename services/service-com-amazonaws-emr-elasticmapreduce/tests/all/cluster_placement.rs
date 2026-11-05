// SPDX-License-Identifier: CC0-1.0

//! Where an EMR cluster's Amazon EC2 instances were placed, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so these are the
//! same bytes an SDK client sends. Nothing reaches into the service's state struct: the
//! placement is only ever read back through `DescribeCluster`.
//!
//! THE RULE: a cluster's `Ec2InstanceAttributes` reports the ONE subnet and the ONE
//! Availability Zone its instances were launched in. A `JobFlowInstancesConfig` states
//! each of those either as a single value (`Ec2SubnetId`, `Placement.AvailabilityZone`)
//! or — the instance-fleets form — as a LIST of candidates (`Ec2SubnetIds`,
//! `Placement.AvailabilityZones`) that Amazon EMR evaluates and then launches every
//! instance within ONE of, so the singular member (`Ec2SubnetId`,
//! `Ec2AvailabilityZone`) reports the entry the cluster is running in while the plural
//! member (`RequestedEc2SubnetIds`, `RequestedEc2AvailabilityZones`) echoes the
//! candidates the caller named. A cluster that named neither form reports no placement.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{at, Emr};

/// One row of the table: what `Instances` asked for, and what the cluster must report.
struct Case {
    label: &'static str,
    /// The `Instances` member of the `RunJobFlow` request.
    instances: Value,
    /// The subnet the cluster must report as the one it is running in.
    subnet: Option<&'static str>,
    /// The subnet candidates it must echo back.
    requested_subnets: &'static [&'static str],
    /// The Availability Zone the cluster must report as the one it is running in.
    zone: Option<&'static str>,
    /// The zone candidates it must echo back.
    requested_zones: &'static [&'static str],
}

/// One instance fleet, so a case can take the instance-fleets form the candidate lists
/// belong to ("Applies to clusters that use the instance fleet configuration").
fn master_fleet() -> Value {
    json!([{
        "InstanceFleetType": "MASTER",
        "TargetOnDemandCapacity": 1,
        "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
    }])
}

/// The reported list at `path`, as plain strings; `[]` when the member is absent.
fn list_at(cluster: &Value, path: &[&str]) -> Vec<String> {
    at(cluster, path)
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_cluster_reports_the_one_subnet_and_zone_its_instances_were_launched_in() {
    let cases = vec![
        Case {
            label: "uniform groups, one subnet named outright",
            instances: json!({
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "Ec2SubnetId": "subnet-0aaa1111bbbb2222c",
            }),
            subnet: Some("subnet-0aaa1111bbbb2222c"),
            requested_subnets: &[],
            zone: None,
            requested_zones: &[],
        },
        Case {
            label: "fleets, a single subnet candidate",
            instances: json!({
                "InstanceFleets": master_fleet(),
                "Ec2SubnetIds": ["subnet-0dddd3333eeee4444f"],
            }),
            subnet: Some("subnet-0dddd3333eeee4444f"),
            requested_subnets: &["subnet-0dddd3333eeee4444f"],
            zone: None,
            requested_zones: &[],
        },
        Case {
            label: "fleets, three subnet candidates",
            instances: json!({
                "InstanceFleets": master_fleet(),
                "Ec2SubnetIds": ["subnet-01", "subnet-02", "subnet-03"],
            }),
            subnet: Some("subnet-01"),
            requested_subnets: &["subnet-01", "subnet-02", "subnet-03"],
            zone: None,
            requested_zones: &[],
        },
        Case {
            label: "fleets, a subnet named outright beside candidates",
            instances: json!({
                "InstanceFleets": master_fleet(),
                "Ec2SubnetId": "subnet-chosen",
                "Ec2SubnetIds": ["subnet-other", "subnet-chosen"],
            }),
            subnet: Some("subnet-chosen"),
            requested_subnets: &["subnet-other", "subnet-chosen"],
            zone: None,
            requested_zones: &[],
        },
        Case {
            label: "fleets, an empty candidate list — the list's lower bound",
            instances: json!({
                "InstanceFleets": master_fleet(),
                "Ec2SubnetIds": [],
            }),
            subnet: None,
            requested_subnets: &[],
            zone: None,
            requested_zones: &[],
        },
        Case {
            label: "uniform groups, one zone named outright",
            instances: json!({
                "InstanceCount": 1,
                "MasterInstanceType": "m5.xlarge",
                "Placement": { "AvailabilityZone": "us-east-1c" },
            }),
            subnet: None,
            requested_subnets: &[],
            zone: Some("us-east-1c"),
            requested_zones: &[],
        },
        Case {
            label: "fleets, two zone candidates",
            instances: json!({
                "InstanceFleets": master_fleet(),
                "Placement": { "AvailabilityZones": ["us-west-2b", "us-west-2d"] },
            }),
            subnet: None,
            requested_subnets: &[],
            zone: Some("us-west-2b"),
            requested_zones: &["us-west-2b", "us-west-2d"],
        },
        Case {
            label: "fleets, a zone named outright beside candidates",
            instances: json!({
                "InstanceFleets": master_fleet(),
                "Placement": {
                    "AvailabilityZone": "eu-west-1a",
                    "AvailabilityZones": ["eu-west-1b", "eu-west-1a"],
                },
            }),
            subnet: None,
            requested_subnets: &[],
            zone: Some("eu-west-1a"),
            requested_zones: &["eu-west-1b", "eu-west-1a"],
        },
        Case {
            label: "fleets, subnet and zone candidates together",
            instances: json!({
                "InstanceFleets": master_fleet(),
                "Ec2SubnetIds": ["subnet-aa", "subnet-bb"],
                "Placement": { "AvailabilityZones": ["ap-south-1a"] },
            }),
            subnet: Some("subnet-aa"),
            requested_subnets: &["subnet-aa", "subnet-bb"],
            zone: Some("ap-south-1a"),
            requested_zones: &["ap-south-1a"],
        },
        Case {
            label: "uniform groups, no placement named at all",
            instances: json!({ "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" }),
            subnet: None,
            requested_subnets: &[],
            zone: None,
            requested_zones: &[],
        },
    ];

    let emr = Emr::new();
    for case in &cases {
        let label = case.label;
        let id = emr
            .run_job_flow(json!({
                "Name": label,
                "ReleaseLabel": "emr-6.15.0",
                "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
                "Instances": case.instances.clone(),
            }))
            .await;
        let cluster = emr.describe_cluster(&id).await;

        let subnet = at(&cluster, &["Ec2InstanceAttributes", "Ec2SubnetId"]);
        assert_eq!(
            subnet.as_str(),
            case.subnet,
            "case {label:?}: the cluster must report the one subnet it runs in, got {subnet:?}"
        );
        assert_eq!(
            list_at(&cluster, &["Ec2InstanceAttributes", "RequestedEc2SubnetIds"]),
            case.requested_subnets,
            "case {label:?}: the subnet candidates must be echoed back, got {cluster}"
        );
        let zone = at(&cluster, &["Ec2InstanceAttributes", "Ec2AvailabilityZone"]);
        assert_eq!(
            zone.as_str(),
            case.zone,
            "case {label:?}: the cluster must report the one zone it runs in, got {zone:?}"
        );
        assert_eq!(
            list_at(
                &cluster,
                &["Ec2InstanceAttributes", "RequestedEc2AvailabilityZones"],
            ),
            case.requested_zones,
            "case {label:?}: the zone candidates must be echoed back, got {cluster}"
        );

        // The placement a cluster launched in never moves: it was decided once, when the
        // cluster was created, so a second read reports the same subnet and zone.
        let again = emr.describe_cluster(&id).await;
        assert_eq!(
            at(&again, &["Ec2InstanceAttributes"]),
            at(&cluster, &["Ec2InstanceAttributes"]),
            "case {label:?}: a re-read must report the same placement, got {again}"
        );

        // Whichever form named it, the reported placement is one of the candidates the
        // request named — never a value the emulator invented.
        if !case.requested_subnets.is_empty() {
            let chosen = subnet.as_str().unwrap_or_default().to_string();
            assert!(
                case.requested_subnets.contains(&chosen.as_str()),
                "case {label:?}: {chosen:?} is not one of {:?}",
                case.requested_subnets
            );
        }
        if !case.requested_zones.is_empty() {
            let chosen = zone.as_str().unwrap_or_default().to_string();
            assert!(
                case.requested_zones.contains(&chosen.as_str()),
                "case {label:?}: {chosen:?} is not one of {:?}",
                case.requested_zones
            );
        }
    }
}
