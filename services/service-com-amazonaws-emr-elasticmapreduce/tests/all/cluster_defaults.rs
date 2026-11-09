// SPDX-License-Identifier: CC0-1.0

//! What Amazon EMR fills in for a launch that left it out, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the router's
//! `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so these are the
//! same bytes an SDK client sends. Nothing reaches into the service's state struct: a
//! cluster is read back only through `DescribeCluster`, `ListInstanceFleets` and
//! `ListInstanceGroups`, and the application versions are compared against what
//! `DescribeReleaseLabel` publishes, so nothing below restates a catalog the service
//! already answers from.
//!
//! THE RULE: a read reports the EFFECTIVE configuration of the resource, not the request's
//! own words — a member the request omitted is reported with the value Amazon EMR supplies
//! for it (the cluster's scale-down behaviour, node replacement and step concurrency, a
//! configuration's property map, an application's installed version, a fleet instance
//! type's weighted capacity, Spot bid percentage and default Amazon EBS layout, a fleet's
//! Spot resize timeout), a collection the request supplied nothing for is reported as an
//! empty list, and a request for something the service does not create (placement strategy
//! `NONE`) is reported as what exists rather than echoed.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{at, Emr, INSTANCE_PROFILE, SERVICE_ROLE};

/// A `RunJobFlow` body carrying `members` on top of the required ones, launching the
/// uniform one-node instance-group cluster.
fn launch(name: &str, members: Value) -> Value {
    let mut request = json!({
        "Name": name,
        "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
        "Instances": {
            "InstanceCount": 1,
            "MasterInstanceType": "m5.xlarge",
            "KeepJobFlowAliveWhenNoSteps": true,
        },
    });
    if let Some(members) = members.as_object() {
        for (member, value) in members {
            request[member] = value.clone();
        }
    }
    request
}

/// A `RunJobFlow` body launching `fleets` as the cluster's instance fleets.
fn fleet_launch(name: &str, fleets: Value) -> Value {
    json!({
        "Name": name,
        "ReleaseLabel": "emr-6.15.0",
        "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
        "Instances": { "KeepJobFlowAliveWhenNoSteps": true, "InstanceFleets": fleets },
    })
}

/// The fleets of `cluster_id`, in creation order.
async fn fleets_of(emr: &Emr, case: &str, cluster_id: &str) -> Vec<Value> {
    let out = emr
        .ok("ListInstanceFleets", json!({ "ClusterId": cluster_id }))
        .await;
    out["InstanceFleets"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: ListInstanceFleets must answer fleets, got {out}"))
}

/// The one `InstanceTypeSpecification` of the fleet `role` carries.
fn specification_of<'a>(case: &str, fleets: &'a [Value], role: &str) -> &'a Value {
    let fleet = fleets
        .iter()
        .find(|fleet| fleet["InstanceFleetType"].as_str() == Some(role))
        .unwrap_or_else(|| panic!("case {case:?}: no {role} fleet among {fleets:?}"));
    fleet["InstanceTypeSpecifications"]
        .get(0)
        .unwrap_or_else(|| panic!("case {case:?}: the {role} fleet reports no instance type, got {fleet}"))
}

/// `(Device, VolumeType, SizeInGB)` per reported `EbsBlockDevice`, in order.
fn volumes(specification: &Value) -> Vec<(&str, &str, i64)> {
    specification["EbsBlockDevices"]
        .as_array()
        .map(|devices| {
            devices
                .iter()
                .map(|device| {
                    (
                        device["Device"].as_str().unwrap_or_default(),
                        at(device, &["VolumeSpecification", "VolumeType"])
                            .as_str()
                            .unwrap_or_default(),
                        at(device, &["VolumeSpecification", "SizeInGB"])
                            .as_i64()
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// THE RULE for the cluster's own defaulted members: `DescribeCluster` reports the
/// scale-down behaviour, node replacement and step concurrency the cluster RUNS under —
/// the request's value where it named one, the service's where it did not — and reports
/// every collection it carries, empty where the launch supplied none.
#[tokio::test]
async fn an_omitted_cluster_member_is_reported_with_the_value_emr_supplies() {
    struct Case {
        label: &'static str,
        members: Value,
        scale_down_behavior: &'static str,
        unhealthy_node_replacement: bool,
        step_concurrency_level: i64,
    }

    // The rows differ along what the rule generalises over: whether the member was
    // supplied at all, which of the two scale-down behaviours was asked for, both
    // booleans, a concurrency level away from the default, and three release series, so
    // the defaults are not read off one release.
    let cases = vec![
        Case {
            label: "nothing optional supplied",
            members: json!({ "ReleaseLabel": "emr-6.15.0" }),
            scale_down_behavior: "TERMINATE_AT_TASK_COMPLETION",
            unhealthy_node_replacement: true,
            step_concurrency_level: 1,
        },
        Case {
            label: "every member supplied, each away from the default",
            members: json!({
                "ReleaseLabel": "emr-5.36.1",
                "ScaleDownBehavior": "TERMINATE_AT_INSTANCE_HOUR",
                "StepConcurrencyLevel": 11,
                "Instances": {
                    "InstanceCount": 1,
                    "MasterInstanceType": "m5.xlarge",
                    "KeepJobFlowAliveWhenNoSteps": true,
                    "UnhealthyNodeReplacement": false,
                },
            }),
            scale_down_behavior: "TERMINATE_AT_INSTANCE_HOUR",
            unhealthy_node_replacement: false,
            step_concurrency_level: 11,
        },
        Case {
            label: "the other scale-down behaviour, node replacement asked for",
            members: json!({
                "ReleaseLabel": "emr-7.5.0",
                "ScaleDownBehavior": "TERMINATE_AT_TASK_COMPLETION",
                "Instances": {
                    "InstanceCount": 1,
                    "MasterInstanceType": "m5.xlarge",
                    "KeepJobFlowAliveWhenNoSteps": true,
                    "UnhealthyNodeReplacement": true,
                },
            }),
            scale_down_behavior: "TERMINATE_AT_TASK_COMPLETION",
            unhealthy_node_replacement: true,
            step_concurrency_level: 1,
        },
        Case {
            label: "another release series, nothing optional supplied",
            members: json!({ "ReleaseLabel": "emr-5.30.0" }),
            scale_down_behavior: "TERMINATE_AT_TASK_COMPLETION",
            unhealthy_node_replacement: true,
            step_concurrency_level: 1,
        },
    ];

    let emr = Emr::new();
    for case in &cases {
        let label = case.label;
        let id = emr.run_job_flow(launch(label, case.members.clone())).await;
        let cluster = emr.describe_cluster(&id).await;
        assert_eq!(
            cluster["ScaleDownBehavior"].as_str(),
            Some(case.scale_down_behavior),
            "case {label:?}: wrong ScaleDownBehavior, got {cluster}"
        );
        assert_eq!(
            cluster["UnhealthyNodeReplacement"].as_bool(),
            Some(case.unhealthy_node_replacement),
            "case {label:?}: wrong UnhealthyNodeReplacement, got {cluster}"
        );
        assert_eq!(
            cluster["StepConcurrencyLevel"].as_i64(),
            Some(case.step_concurrency_level),
            "case {label:?}: wrong StepConcurrencyLevel, got {cluster}"
        );
        // Every collection the cluster carries is reported, empty rather than absent:
        // none of these launches supplied any of them.
        for member in ["Configurations", "Applications", "Tags", "PlacementGroups"] {
            assert_eq!(
                cluster[member].as_array(),
                Some(&Vec::new()),
                "case {label:?}: {member} must be reported as an empty list, got {cluster}"
            );
        }

        // A read changes nothing: the same cluster reports the same values again.
        let again = emr.describe_cluster(&id).await;
        assert_eq!(
            again, cluster,
            "case {label:?}: a second DescribeCluster must report the same cluster"
        );
    }
}

/// THE RULE for a configuration tree: every served `Configuration`, at any depth and
/// wherever it is reported, carries its `Properties` map — empty for a classification that
/// set no property — so a client walking the tree has no missing map to special-case.
#[tokio::test]
async fn a_served_configuration_always_reports_its_property_map() {
    /// Walk a served configuration tree, collecting
    /// `(Classification, property count)` depth-first, and failing the case when a
    /// configuration reports no `Properties` member at all.
    fn walk(case: &str, configurations: &Value, into: &mut Vec<(String, usize)>) {
        for configuration in configurations.as_array().into_iter().flatten() {
            let classification = configuration["Classification"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let properties = configuration["Properties"].as_object().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: configuration {classification:?} must report a \
                     Properties map, got {configuration}"
                )
            });
            into.push((classification, properties.len()));
            walk(case, &configuration["Configurations"], into);
        }
    }

    // One tree spanning the forms the rule generalises over: a parent that sets
    // properties and nests children, a child that sets none and nests one of its own, a
    // grandchild that sets one, and a sibling leaf that sets none.
    let configurations = json!([
        {
            "Classification": "spark-defaults",
            "Properties": { "spark.executor.memory": "2g" },
            "Configurations": [
                {
                    "Classification": "spark-env",
                    "Configurations": [
                        { "Classification": "export", "Properties": { "PYSPARK_PYTHON": "python3" } },
                    ],
                },
                { "Classification": "spark-log4j" },
            ],
        },
        { "Classification": "core-site", "Properties": {} },
    ]);
    let expected = vec![
        ("spark-defaults".to_string(), 1),
        ("spark-env".to_string(), 0),
        ("export".to_string(), 1),
        ("spark-log4j".to_string(), 0),
        ("core-site".to_string(), 0),
    ];

    let emr = Emr::new();

    // On the cluster itself, and on the instance group that carries the same tree.
    let cluster_id = emr
        .run_job_flow(json!({
            "Name": "configurations",
            "ReleaseLabel": "emr-5.36.1",
            "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
            "Configurations": configurations.clone(),
            "Instances": {
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceGroups": [{
                    "InstanceRole": "MASTER",
                    "InstanceType": "m5.xlarge",
                    "InstanceCount": 1,
                    "Configurations": configurations.clone(),
                }],
            },
        }))
        .await;
    let cluster = emr.describe_cluster(&cluster_id).await;
    let mut reported = Vec::new();
    walk("cluster", &cluster["Configurations"], &mut reported);
    assert_eq!(
        reported, expected,
        "case \"cluster\": wrong served configuration tree, got {cluster}"
    );

    let groups = emr
        .ok("ListInstanceGroups", json!({ "ClusterId": cluster_id }))
        .await;
    let mut reported = Vec::new();
    walk("instance group", &at(&groups, &["InstanceGroups"])[0]["Configurations"], &mut reported);
    assert_eq!(
        reported, expected,
        "case \"instance group\": wrong served configuration tree, got {groups}"
    );

    // And on a fleet's instance type, which carries a configuration tree of its own.
    let fleet_cluster = emr
        .run_job_flow(fleet_launch(
            "fleet configurations",
            json!([{
                "InstanceFleetType": "MASTER",
                "TargetOnDemandCapacity": 1,
                "InstanceTypeConfigs": [{
                    "InstanceType": "m5.xlarge",
                    "Configurations": configurations.clone(),
                }],
            }]),
        ))
        .await;
    let fleets = fleets_of(&emr, "fleet instance type", &fleet_cluster).await;
    let specification = specification_of("fleet instance type", &fleets, "MASTER");
    let mut reported = Vec::new();
    walk(
        "fleet instance type",
        &specification["Configurations"],
        &mut reported,
    );
    assert_eq!(
        reported, expected,
        "case \"fleet instance type\": wrong served configuration tree, got {specification}"
    );
}

/// THE RULE for applications: a cluster's `Applications` report the version the RELEASE it
/// launched on installs — the same version `DescribeReleaseLabel` publishes for that
/// release, whatever case the launch spelled the name in — and an application that release
/// does not bundle is reported with no version to read.
#[tokio::test]
async fn an_installed_application_reports_the_version_its_release_ships() {
    struct Case {
        label: &'static str,
        release_label: &'static str,
        /// `(name as the launch spells it, name as the cluster reports it)`.
        requested: &'static [(&'static str, &'static str)],
    }

    // The rows differ in the release (three series, so three different version sets), in
    // how many applications are asked for, and in the case the name is spelled in — the
    // model documents the list as case-insensitive.
    let cases = vec![
        Case {
            label: "four applications of a 5.x release",
            release_label: "emr-5.36.1",
            requested: &[
                ("Spark", "Spark"),
                ("Zeppelin", "Zeppelin"),
                ("Hadoop", "Hadoop"),
                ("Pig", "Pig"),
            ],
        },
        Case {
            label: "a 6.x release, names spelled in other cases",
            release_label: "emr-6.15.0",
            requested: &[("spark", "Spark"), ("HIVE", "Hive"), ("TrInO", "Trino")],
        },
        Case {
            label: "one application of a 7.x release",
            release_label: "emr-7.5.0",
            requested: &[("Hadoop", "Hadoop")],
        },
    ];

    let emr = Emr::new();
    for case in &cases {
        let label = case.label;
        // What the release itself publishes, read back through the operation that reports
        // one release's applications: the expectation is not restated here.
        let published = emr
            .ok(
                "DescribeReleaseLabel",
                json!({ "ReleaseLabel": case.release_label }),
            )
            .await;
        let versions: Vec<(String, String)> = at(&published, &["Applications"])
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|application| {
                (
                    application["Name"].as_str().unwrap_or_default().to_string(),
                    application["Version"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = case
            .requested
            .iter()
            .map(|(_, reported)| {
                let version = versions
                    .iter()
                    .find(|(name, _)| name == reported)
                    .map(|(_, version)| version.clone())
                    .unwrap_or_else(|| {
                        panic!(
                            "case {label:?}: {} must publish a version for {reported:?}, got \
                             {published}",
                            case.release_label
                        )
                    });
                ((*reported).to_string(), version)
            })
            .collect();

        let id = emr
            .run_job_flow(launch(
                label,
                json!({
                    "ReleaseLabel": case.release_label,
                    "Applications": case
                        .requested
                        .iter()
                        .map(|(requested, _)| json!({ "Name": requested }))
                        .collect::<Vec<Value>>(),
                }),
            ))
            .await;
        let cluster = emr.describe_cluster(&id).await;
        let reported: Vec<(String, String)> = cluster["Applications"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|application| {
                (
                    application["Name"].as_str().unwrap_or_default().to_string(),
                    application["Version"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {label:?}: wrong installed applications, got {cluster}"
        );
    }

    // An application the named release does not bundle has no installed version to
    // report: Amazon EMR dropped Ganglia from the 7.x series, which `DescribeReleaseLabel`
    // publishes no version for either.
    let id = emr
        .run_job_flow(launch(
            "unbundled application",
            json!({
                "ReleaseLabel": "emr-7.5.0",
                "Applications": [{ "Name": "Ganglia" }],
            }),
        ))
        .await;
    let cluster = emr.describe_cluster(&id).await;
    assert_eq!(
        cluster["Applications"][0]["Name"].as_str(),
        Some("Ganglia"),
        "case \"unbundled application\": the requested application must be reported, got {cluster}"
    );
    assert!(
        cluster["Applications"][0]["Version"].is_null(),
        "case \"unbundled application\": a release that bundles it reports no version, got \
         {cluster}"
    );
}

/// THE RULE for a fleet's instance types: every `InstanceTypeSpecification` reports the
/// units one instance provides (1 where the configuration named none), the Spot bid as a
/// percentage of the On-Demand price (100 where it named neither bid), and the Amazon EBS
/// volumes one instance is attached — the ones the configuration requested, or the default
/// layout of the instance type where it requested none.
#[tokio::test]
async fn a_fleet_instance_type_reports_the_capacity_bid_and_ebs_layout_emr_supplies() {
    struct Case {
        label: &'static str,
        instance_type: &'static str,
        /// `(Device, VolumeType, SizeInGB)` per default volume, in device order.
        volumes: &'static [(&'static str, &'static str, i64)],
    }

    // SOURCE for the layouts: the Amazon EMR Management Guide's default Amazon EBS
    // storage table (the page the service's instance-type catalog is refreshed from) —
    // one `gp2` volume of 32 GiB for a `large`, two for an `xlarge`, four from `2xlarge`
    // up, each sized so the instance is attached 16 GiB per vCPU. The rows span the size
    // ladder (`large` through `24xlarge`, including a `9xlarge` step), two families, a
    // storage-optimised type that carries instance-store disks instead, and a type
    // outside the catalog, which has no published layout to state.
    let cases = vec![
        Case {
            label: "the smallest EBS-only size, one volume",
            instance_type: "m5.large",
            volumes: &[("/dev/sdb", "gp2", 32)],
        },
        Case {
            label: "an xlarge, two volumes",
            instance_type: "m5.xlarge",
            volumes: &[("/dev/sdb", "gp2", 32), ("/dev/sdc", "gp2", 32)],
        },
        Case {
            label: "a 2xlarge, four volumes",
            instance_type: "m5.2xlarge",
            volumes: &[
                ("/dev/sdb", "gp2", 32),
                ("/dev/sdc", "gp2", 32),
                ("/dev/sdd", "gp2", 32),
                ("/dev/sde", "gp2", 32),
            ],
        },
        Case {
            label: "a 4xlarge, four larger volumes",
            instance_type: "m5.4xlarge",
            volumes: &[
                ("/dev/sdb", "gp2", 64),
                ("/dev/sdc", "gp2", 64),
                ("/dev/sdd", "gp2", 64),
                ("/dev/sde", "gp2", 64),
            ],
        },
        Case {
            label: "a compute-optimised 9xlarge step",
            instance_type: "c5.9xlarge",
            volumes: &[
                ("/dev/sdb", "gp2", 144),
                ("/dev/sdc", "gp2", 144),
                ("/dev/sdd", "gp2", 144),
                ("/dev/sde", "gp2", 144),
            ],
        },
        Case {
            label: "the top of a memory-optimised ladder",
            instance_type: "r5.24xlarge",
            volumes: &[
                ("/dev/sdb", "gp2", 384),
                ("/dev/sdc", "gp2", 384),
                ("/dev/sdd", "gp2", 384),
                ("/dev/sde", "gp2", 384),
            ],
        },
        Case {
            label: "a type carrying instance-store disks of its own",
            instance_type: "i3.2xlarge",
            volumes: &[],
        },
        Case {
            label: "a type outside this service's catalog",
            instance_type: "m1.small",
            volumes: &[],
        },
    ];

    let emr = Emr::new();
    for case in &cases {
        let label = case.label;
        let id = emr
            .run_job_flow(fleet_launch(
                label,
                json!([{
                    "InstanceFleetType": "MASTER",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": case.instance_type }],
                }]),
            ))
            .await;
        let fleets = fleets_of(&emr, label, &id).await;
        let specification = specification_of(label, &fleets, "MASTER");
        assert_eq!(
            specification["InstanceType"].as_str(),
            Some(case.instance_type),
            "case {label:?}: wrong InstanceType, got {specification}"
        );
        assert_eq!(
            specification["WeightedCapacity"].as_i64(),
            Some(1),
            "case {label:?}: an instance type that named no weight provides one unit, got \
             {specification}"
        );
        assert_eq!(
            specification["BidPriceAsPercentageOfOnDemandPrice"].as_f64(),
            Some(100.0),
            "case {label:?}: an instance type that named neither bid bids the whole \
             On-Demand price, got {specification}"
        );
        assert_eq!(
            volumes(specification),
            case.volumes.to_vec(),
            "case {label:?}: wrong EbsBlockDevices, got {specification}"
        );
    }

    // A configuration that states the values itself keeps them: the weight and bid
    // percentage it named, and one volume per requested volume of its own layout — while
    // the role beside it, which stated nothing, keeps the default layout of its type.
    let id = emr
        .run_job_flow(fleet_launch(
            "declared beside undeclared",
            json!([
                {
                    "InstanceFleetType": "MASTER",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.2xlarge" }],
                },
                {
                    "InstanceFleetType": "CORE",
                    "TargetSpotCapacity": 6,
                    "InstanceTypeConfigs": [{
                        "InstanceType": "m5.2xlarge",
                        "WeightedCapacity": 3,
                        "BidPriceAsPercentageOfOnDemandPrice": 45.0,
                        "EbsConfiguration": {
                            "EbsOptimized": true,
                            "EbsBlockDeviceConfigs": [
                                {
                                    "VolumeSpecification": { "VolumeType": "gp3", "SizeInGB": 100 },
                                    "VolumesPerInstance": 2,
                                },
                                { "VolumeSpecification": { "VolumeType": "st1", "SizeInGB": 500 } },
                            ],
                        },
                    }],
                },
                {
                    "InstanceFleetType": "TASK",
                    "TargetSpotCapacity": 1,
                    "InstanceTypeConfigs": [{
                        "InstanceType": "m5.xlarge",
                        "BidPrice": "0.10",
                    }],
                },
            ]),
        ))
        .await;
    let fleets = fleets_of(&emr, "declared beside undeclared", &id).await;

    let declared = specification_of("declared", &fleets, "CORE");
    assert_eq!(
        (
            declared["WeightedCapacity"].as_i64(),
            declared["BidPriceAsPercentageOfOnDemandPrice"].as_f64(),
            volumes(declared),
        ),
        (
            Some(3),
            Some(45.0),
            vec![
                ("/dev/sdb", "gp3", 100),
                ("/dev/sdc", "gp3", 100),
                ("/dev/sdd", "st1", 500),
            ],
        ),
        "case \"declared\": a configuration's own values must stand, got {declared}"
    );

    let undeclared = specification_of("undeclared", &fleets, "MASTER");
    assert_eq!(
        volumes(undeclared),
        vec![
            ("/dev/sdb", "gp2", 32),
            ("/dev/sdc", "gp2", 32),
            ("/dev/sdd", "gp2", 32),
            ("/dev/sde", "gp2", 32),
        ],
        "case \"undeclared\": the role that declared nothing keeps the default layout, got \
         {undeclared}"
    );

    // A bid price in USD IS the bid this instance type made, so there is no percentage of
    // the On-Demand price to report beside it.
    let priced = specification_of("bid price in USD", &fleets, "TASK");
    assert_eq!(
        (
            priced["BidPrice"].as_str(),
            priced["BidPriceAsPercentageOfOnDemandPrice"].as_f64(),
        ),
        (Some("0.10"), None),
        "case \"bid price in USD\": the named bid price stands alone, got {priced}"
    );

    // The two degenerate forms of an `EbsConfiguration`: one that declares no block
    // device at all requested no volumes, so the instance type keeps the default layout
    // (its `EbsOptimized` flag is still reported); one that declares a device but asks
    // for none of it keeps the empty layout it declared.
    let id = emr
        .run_job_flow(fleet_launch(
            "degenerate ebs configurations",
            json!([
                {
                    "InstanceFleetType": "MASTER",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{
                        "InstanceType": "m5.xlarge",
                        "EbsConfiguration": { "EbsOptimized": true },
                    }],
                },
                {
                    "InstanceFleetType": "CORE",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{
                        "InstanceType": "m5.xlarge",
                        "EbsConfiguration": {
                            "EbsBlockDeviceConfigs": [{
                                "VolumeSpecification": { "VolumeType": "gp3", "SizeInGB": 100 },
                                "VolumesPerInstance": 0,
                            }],
                        },
                    }],
                },
            ]),
        ))
        .await;
    let fleets = fleets_of(&emr, "degenerate ebs configurations", &id).await;

    let optimized_only = specification_of("no block device declared", &fleets, "MASTER");
    assert_eq!(
        (
            volumes(optimized_only),
            optimized_only["EbsOptimized"].as_bool(),
        ),
        (
            vec![("/dev/sdb", "gp2", 32), ("/dev/sdc", "gp2", 32)],
            Some(true),
        ),
        "case \"no block device declared\": the default layout stands beside the declared \
         flag, got {optimized_only}"
    );

    let no_volumes = specification_of("zero volumes declared", &fleets, "CORE");
    assert_eq!(
        volumes(no_volumes),
        Vec::new(),
        "case \"zero volumes declared\": a configuration that asked for no volume of its \
         declared device keeps that answer, got {no_volumes}"
    );
}

/// THE RULE for a fleet's resize policy: a fleet Amazon EMR can resize reports the resize
/// specifications it runs under — the Spot resize timeout the service supplies where the
/// configuration named none, and the configuration's own value where it did — while the
/// fleet running the cluster's primary nodes, which is never resized, reports only what
/// its configuration named.
#[tokio::test]
async fn a_fleet_reports_the_spot_resize_timeout_it_runs_under() {
    /// Amazon EMR's default Spot resize timeout, in minutes.
    const DEFAULT_TIMEOUT: i64 = 60;
    /// The documented minimum of `SpotResizingSpecification$TimeoutDurationMinutes`
    /// ("minimum value is 5 minutes and maximum value is 10,080 minutes (7 days)").
    const MINIMUM_TIMEOUT: i64 = 5;
    /// That member's documented maximum.
    const MAXIMUM_TIMEOUT: i64 = 10_080;

    /// One fleet of `role`, naming `timeout` minutes as its Spot resize timeout when
    /// there is one.
    fn fleet(role: &str, timeout: Option<i64>) -> Value {
        let mut fleet = json!({
            "InstanceFleetType": role,
            "TargetSpotCapacity": 1,
            "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
        });
        if let Some(timeout) = timeout {
            fleet["ResizeSpecifications"] =
                json!({ "SpotResizeSpecification": { "TimeoutDurationMinutes": timeout } });
        }
        fleet
    }

    // The rows differ in the role, in whether the configuration named a timeout, and —
    // where it did — in which end of the documented band it named.
    let cases: Vec<(&str, Value, Option<i64>)> = vec![
        ("CORE, nothing named", fleet("CORE", None), Some(DEFAULT_TIMEOUT)),
        ("TASK, nothing named", fleet("TASK", None), Some(DEFAULT_TIMEOUT)),
        (
            "CORE, the documented minimum named",
            fleet("CORE", Some(MINIMUM_TIMEOUT)),
            Some(MINIMUM_TIMEOUT),
        ),
        (
            "TASK, the documented maximum named",
            fleet("TASK", Some(MAXIMUM_TIMEOUT)),
            Some(MAXIMUM_TIMEOUT),
        ),
        // The primary fleet is never resized, so there is no policy to supply for it —
        // and the one a configuration names for it is still reported.
        ("MASTER, nothing named", fleet("MASTER", None), None),
        (
            "MASTER, a timeout named",
            fleet("MASTER", Some(MINIMUM_TIMEOUT)),
            Some(MINIMUM_TIMEOUT),
        ),
    ];

    let emr = Emr::new();
    for (label, fleet, expected) in &cases {
        // Every launch needs a primary fleet; a row about another role gets one beside it.
        let fleets = if fleet["InstanceFleetType"] == json!("MASTER") {
            json!([fleet.clone()])
        } else {
            json!([
                {
                    "InstanceFleetType": "MASTER",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                },
                fleet.clone(),
            ])
        };
        let id = emr.run_job_flow(fleet_launch(label, fleets)).await;
        let reported = fleets_of(&emr, label, &id).await;
        let role = fleet["InstanceFleetType"].as_str().unwrap_or_default();
        let got = reported
            .iter()
            .find(|reported| reported["InstanceFleetType"].as_str() == Some(role))
            .unwrap_or_else(|| panic!("case {label:?}: no {role} fleet among {reported:?}"));
        assert_eq!(
            at(
                got,
                &["ResizeSpecifications", "SpotResizeSpecification", "TimeoutDurationMinutes"],
            )
            .as_i64(),
            *expected,
            "case {label:?}: wrong Spot resize timeout, got {got}"
        );
        if expected.is_none() {
            assert!(
                got["ResizeSpecifications"].is_null(),
                "case {label:?}: a fleet with no resize policy reports none, got {got}"
            );
        }
    }
}

/// THE RULE for placement groups: a described cluster reports the placement groups it
/// HAS — an entry asking for the `NONE` strategy creates none, so it is not reported,
/// while every entry that does ask for a group is.
#[tokio::test]
async fn a_placement_group_the_launch_does_not_create_is_not_reported() {
    struct Case {
        label: &'static str,
        requested: Value,
        /// `(InstanceRole, PlacementStrategy)` per placement group the cluster has.
        expected: Vec<(&'static str, Option<&'static str>)>,
    }

    // `com.amazonaws.emr#PlacementGroupStrategy` values, every one the model declares
    // (`smithy_query(query="shape", shape="PlacementGroupStrategy")`): `SPREAD`,
    // `PARTITION`, `CLUSTER` each ask for a placement group of that strategy, and `NONE`
    // asks for none. A configuration that names no strategy at all is still a request for
    // a group.
    let cases = vec![
        Case {
            label: "strategy NONE",
            requested: json!([{ "InstanceRole": "MASTER", "PlacementStrategy": "NONE" }]),
            expected: vec![],
        },
        Case {
            label: "strategy SPREAD",
            requested: json!([{ "InstanceRole": "MASTER", "PlacementStrategy": "SPREAD" }]),
            expected: vec![("MASTER", Some("SPREAD"))],
        },
        Case {
            label: "strategy PARTITION",
            requested: json!([{ "InstanceRole": "MASTER", "PlacementStrategy": "PARTITION" }]),
            expected: vec![("MASTER", Some("PARTITION"))],
        },
        Case {
            label: "strategy CLUSTER",
            requested: json!([{ "InstanceRole": "MASTER", "PlacementStrategy": "CLUSTER" }]),
            expected: vec![("MASTER", Some("CLUSTER"))],
        },
        Case {
            label: "no strategy named",
            requested: json!([{ "InstanceRole": "MASTER" }]),
            expected: vec![("MASTER", None)],
        },
        Case {
            label: "no placement group configs at all",
            requested: json!([]),
            expected: vec![],
        },
    ];

    let emr = Emr::new();
    for case in &cases {
        let label = case.label;
        let id = emr
            .run_job_flow(launch(
                label,
                json!({
                    "ReleaseLabel": "emr-6.15.0",
                    "PlacementGroupConfigs": case.requested.clone(),
                }),
            ))
            .await;
        let cluster = emr.describe_cluster(&id).await;
        let reported: Vec<(&str, Option<&str>)> = cluster["PlacementGroups"]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .map(|group| {
                        (
                            group["InstanceRole"].as_str().unwrap_or_default(),
                            group["PlacementStrategy"].as_str(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_else(|| {
                panic!("case {label:?}: PlacementGroups must be reported, got {cluster}")
            });
        assert_eq!(
            reported, case.expected,
            "case {label:?}: wrong PlacementGroups, got {cluster}"
        );
    }
}
