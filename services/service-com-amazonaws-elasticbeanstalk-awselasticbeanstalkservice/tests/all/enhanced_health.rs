// SPDX-License-Identifier: CC0-1.0

//! Elastic Beanstalk's enhanced health reports — `DescribeInstancesHealth` and
//! `DescribeEnvironmentHealth`'s `InstancesHealth` summary — driven end to end
//! over the service's awsQuery HTTP surface.
//!
//! RULE: both reports are two views of ONE set, the Amazon EC2 instances the
//! environment's own inventory holds (what `DescribeEnvironmentResources`
//! reports): the per-instance list carries one entry per instance, each in the
//! health status that belongs to the colour the environment reports, and the
//! summary counts that same list into its buckets. So an environment that is
//! `Ready`/`Green` presents every instance it is made of as `Ok` and a non-zero
//! `Ok` count, one with an operation in progress presents them as `Pending`, and
//! an environment the clock has terminated — which runs nothing — presents none.
//! Each instance's own members are derived from the same stored environment: its
//! id and launch instant from the inventory, its type from the configuration
//! options, its zone from its place in the group, and its deployment from the
//! version the environment runs.
//!
//! Each test is a TABLE over the class the rule generalises over — the
//! environment type, the Auto Scaling minimum, the tier, the environment's
//! lifecycle state, which option names the instance type — and every assertion
//! names its case and prints what the emulator actually answered.
//!
//! The harness is the one `lifecycle.rs` owns: the same `MultiServiceRouter` the
//! production binary mounts, spoken to with a form-urlencoded `Action=…` body.
//! Nothing here reaches into the store, so a report that is not derived from the
//! persisted environment fails these tests.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::lifecycle::{Answer, Emulator, PAST_DUE, SOLUTION_STACK, TEST_REGION};

/// The application every environment below is launched under.
const APPLICATION: &str = "health-app";

/// The version label the application's one version carries, which the
/// environments then deploy — what each instance's `Deployment` reports.
const VERSION: &str = "v1";

/// Create [`APPLICATION`] with one version.
async fn application(emulator: &Emulator) {
    let created = emulator
        .call("CreateApplication", &[("ApplicationName", APPLICATION)])
        .await;
    assert_eq!(
        created.status, 200,
        "fixture: CreateApplication answered {} with body {}",
        created.status, created.body
    );
    let version = emulator
        .call(
            "CreateApplicationVersion",
            &[
                ("ApplicationName", APPLICATION),
                ("VersionLabel", VERSION),
                ("SourceBundle.S3Bucket", "bundles"),
                ("SourceBundle.S3Key", "app.zip"),
            ],
        )
        .await;
    assert_eq!(
        version.status, 200,
        "fixture: CreateApplicationVersion answered {} with body {}",
        version.status, version.body
    );
}

/// Launch `environment` with `options` on top of the fixture application,
/// WITHOUT waiting for the launch the clock owes.
async fn launch(emulator: &Emulator, environment: &str, options: &[(&str, &str)]) {
    let mut params: Vec<(&str, &str)> = vec![
        ("ApplicationName", APPLICATION),
        ("EnvironmentName", environment),
        ("VersionLabel", VERSION),
        ("SolutionStackName", SOLUTION_STACK.as_str()),
    ];
    params.extend_from_slice(options);
    let launched = emulator.call("CreateEnvironment", &params).await;
    assert_eq!(
        launched.status, 200,
        "fixture {environment:?}: CreateEnvironment answered {} with body {}",
        launched.status, launched.body
    );
}

/// The instance ids the environment's own inventory holds.
async fn inventory(emulator: &Emulator, environment: &str) -> Vec<String> {
    let resources = emulator
        .call(
            "DescribeEnvironmentResources",
            &[("EnvironmentName", environment)],
        )
        .await;
    resources
        .values_at("EnvironmentResources/Instances/member/Id")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// `DescribeInstancesHealth` over `environment`, asking for every attribute.
async fn instances_health(emulator: &Emulator, environment: &str) -> Answer {
    emulator
        .call(
            "DescribeInstancesHealth",
            &[
                ("EnvironmentName", environment),
                ("AttributeNames.member.1", "All"),
            ],
        )
        .await
}

/// `DescribeEnvironmentHealth` over `environment`, asking for every attribute.
async fn environment_health(emulator: &Emulator, environment: &str) -> Answer {
    emulator
        .call(
            "DescribeEnvironmentHealth",
            &[
                ("EnvironmentName", environment),
                ("AttributeNames.member.1", "All"),
            ],
        )
        .await
}

/// The eight `InstanceHealthSummary` buckets, as the elements they are reported
/// in. From the modelled shape's members, which the service's own
/// `InstanceHealthSummary` mirrors.
const BUCKETS: &[&str] = &[
    "NoData", "Unknown", "Pending", "Ok", "Info", "Warning", "Degraded", "Severe",
];

/// What `DescribeEnvironmentHealth` counted in `bucket`, failing the named case
/// when the bucket is not reported at all.
fn bucket(health: &Answer, bucket: &str, case: &str) -> usize {
    let reported = health.expect(&format!("InstancesHealth/{bucket}"), case);
    reported.parse::<usize>().unwrap_or_else(|_| {
        panic!("case {case:?}: {bucket} is a count, got {reported:?} in {}", health.body)
    })
}

/// RULE (the set): the per-instance list and the summary buckets agree with the
/// instances the SAME environment reports being made of, for every shape of
/// environment — and a `Ready`/`Green` one presents each of them in the green
/// `Ok` bucket.
#[tokio::test]
async fn enhanced_health_covers_every_instance_the_environment_is_made_of() {
    let emulator = Emulator::new();
    application(&emulator).await;

    struct Case {
        environment: &'static str,
        options: Vec<(&'static str, &'static str)>,
        /// How many instances this environment is made of, from the options:
        /// `SingleInstance` is "one EC2 instance", and a load-balanced group
        /// holds its Auto Scaling minimum.
        instances: usize,
    }
    // Rows differ along the environment type, the requested group size and the
    // tier — everything the instance set follows.
    let cases = [
        Case {
            environment: "health-default",
            options: vec![],
            instances: eb::state::DEFAULT_MIN_SIZE,
        },
        Case {
            environment: "health-single",
            options: vec![
                (
                    "OptionSettings.member.1.Namespace",
                    "aws:elasticbeanstalk:environment",
                ),
                ("OptionSettings.member.1.OptionName", "EnvironmentType"),
                ("OptionSettings.member.1.Value", "SingleInstance"),
            ],
            instances: 1,
        },
        Case {
            environment: "health-group-of-three",
            options: vec![
                ("OptionSettings.member.1.Namespace", "aws:autoscaling:asg"),
                ("OptionSettings.member.1.OptionName", "MinSize"),
                ("OptionSettings.member.1.Value", "3"),
            ],
            instances: 3,
        },
        Case {
            environment: "health-worker-of-two",
            options: vec![
                ("Tier.Name", "Worker"),
                ("Tier.Type", "SQS/HTTP"),
                ("OptionSettings.member.1.Namespace", "aws:autoscaling:asg"),
                ("OptionSettings.member.1.OptionName", "MinSize"),
                ("OptionSettings.member.1.Value", "2"),
            ],
            instances: 2,
        },
    ];

    for case in &cases {
        launch(&emulator, case.environment, &case.options).await;
    }
    // The launch the clock owes every one of them: wait it out once, so each
    // environment is `Ready`/`Green`.
    std::thread::sleep(PAST_DUE);

    for case in &cases {
        let name = case.environment;
        let instances = inventory(&emulator, name).await;
        assert_eq!(
            instances.len(),
            case.instances,
            "case {name:?}: expected {} instances in the inventory, got {instances:?}",
            case.instances
        );

        let per_instance = instances_health(&emulator, name).await;
        assert_eq!(
            per_instance.status, 200,
            "case {name:?}: expected 200, got {} and body {}",
            per_instance.status, per_instance.body
        );
        assert_eq!(
            per_instance.values_at("InstanceHealthList/member/InstanceId"),
            instances,
            "case {name:?}: the list covers the environment's own instances, got {}",
            per_instance.body
        );
        for entry in per_instance.elements_at("InstanceHealthList/member") {
            let instance = entry.expect("InstanceId", name);
            assert_eq!(
                entry.expect("HealthStatus", name),
                "Ok",
                "case {name:?}: instance {instance} of a green Ready environment is passing health checks, got {}",
                entry.body
            );
            assert_eq!(
                entry.expect("Color", name),
                "Green",
                "case {name:?}: instance {instance} reports the colour of its health status, got {}",
                entry.body
            );
            assert!(
                entry.values_at("Causes/member").is_empty(),
                "case {name:?}: a healthy instance reports no cause, got {}",
                entry.body
            );
        }

        let summary = environment_health(&emulator, name).await;
        assert_eq!(
            bucket(&summary, "Ok", name),
            case.instances,
            "case {name:?}: the green bucket counts every instance, got {}",
            summary.body
        );
        let counted: usize = BUCKETS
            .iter()
            .map(|name_of_bucket| bucket(&summary, name_of_bucket, name))
            .sum();
        assert_eq!(
            counted, case.instances,
            "case {name:?}: the buckets count the instance set and nothing else, got {}",
            summary.body
        );
    }
}

/// RULE (the status): each instance is in the health status that belongs to the
/// colour its environment reports — green `Ok` for a `Ready` environment, grey
/// `Pending` while an operation is in progress on it — and an environment the
/// clock has terminated runs no instance at all, so both reports are empty.
#[tokio::test]
async fn instance_health_follows_the_environments_own_health() {
    let emulator = Emulator::new();
    application(&emulator).await;
    launch(&emulator, "health-lifecycle", &[]).await;

    // Launching: the environment is grey, so its instances have an operation in
    // progress on them.
    let launching = instances_health(&emulator, "health-lifecycle").await;
    let case = "Launching";
    assert_eq!(
        launching.value("InstanceHealthList/member/HealthStatus"),
        Some("Pending"),
        "case {case:?}: an instance of a grey environment has an operation in progress, got {}",
        launching.body
    );
    assert_eq!(
        launching.value("InstanceHealthList/member/Color"),
        Some("Grey"),
        "case {case:?}: the instance reports the colour of its status, got {}",
        launching.body
    );
    assert_eq!(
        launching.values_at("InstanceHealthList/member/Causes/member").len(),
        1,
        "case {case:?}: an instance that is not green reports its cause, got {}",
        launching.body
    );
    assert_eq!(
        launching.value("InstanceHealthList/member/Deployment/Status"),
        Some("In Progress"),
        "case {case:?}: the deployment that is still owed is in progress, got {}",
        launching.body
    );
    let launching_summary = environment_health(&emulator, "health-lifecycle").await;
    assert_eq!(
        bucket(&launching_summary, "Pending", case),
        1,
        "case {case:?}: the grey bucket counts the instance, got {}",
        launching_summary.body
    );
    assert_eq!(
        bucket(&launching_summary, "Ok", case),
        0,
        "case {case:?}: nothing is green yet, got {}",
        launching_summary.body
    );

    // Ready: the launch the clock owed has happened.
    std::thread::sleep(PAST_DUE);
    let ready = instances_health(&emulator, "health-lifecycle").await;
    let case = "Ready";
    assert_eq!(
        ready.value("InstanceHealthList/member/HealthStatus"),
        Some("Ok"),
        "case {case:?}: the instance of a green Ready environment is passing health checks, got {}",
        ready.body
    );
    assert_eq!(
        ready.value("InstanceHealthList/member/Deployment/Status"),
        Some("Deployed"),
        "case {case:?}: the deployment the clock completed succeeded, got {}",
        ready.body
    );

    // A read changes nothing a later caller sees: the same instances, in the
    // same status.
    let again = instances_health(&emulator, "health-lifecycle").await;
    assert_eq!(
        again.values_at("InstanceHealthList/member/InstanceId"),
        ready.values_at("InstanceHealthList/member/InstanceId"),
        "case {case:?}: two reads report the same instances, got {} then {}",
        ready.body,
        again.body
    );

    // A rebuild "deletes and recreates all of the Amazon Web Services
    // resources" — so the report follows the instances the environment is made
    // of NOW rather than the ones it was launched with.
    let rebuilt = emulator
        .call(
            "RebuildEnvironment",
            &[("EnvironmentName", "health-lifecycle")],
        )
        .await;
    assert_eq!(
        rebuilt.status, 200,
        "case \"rebuilt\": RebuildEnvironment answered {} with body {}",
        rebuilt.status, rebuilt.body
    );
    std::thread::sleep(PAST_DUE);
    let case = "rebuilt";
    let after_rebuild = instances_health(&emulator, "health-lifecycle").await;
    assert_eq!(
        after_rebuild.values_at("InstanceHealthList/member/InstanceId"),
        inventory(&emulator, "health-lifecycle").await,
        "case {case:?}: the report covers the instances the rebuild created, got {}",
        after_rebuild.body
    );
    assert_ne!(
        after_rebuild.value("InstanceHealthList/member/InstanceId"),
        ready.value("InstanceHealthList/member/InstanceId"),
        "case {case:?}: the rebuilt environment runs a new instance, got {}",
        after_rebuild.body
    );
    assert_ne!(
        after_rebuild.value("InstanceHealthList/member/LaunchedAt"),
        ready.value("InstanceHealthList/member/LaunchedAt"),
        "case {case:?}: the instance the rebuild launched was launched later, got {}",
        after_rebuild.body
    );

    // Updating: an operation is in progress again.
    let updated = emulator
        .call(
            "UpdateEnvironment",
            &[
                ("EnvironmentName", "health-lifecycle"),
                ("Description", "a configuration change"),
            ],
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "case \"Updating\": UpdateEnvironment answered {} with body {}",
        updated.status, updated.body
    );
    let updating = instances_health(&emulator, "health-lifecycle").await;
    let case = "Updating";
    assert_eq!(
        updating.value("InstanceHealthList/member/HealthStatus"),
        Some("Pending"),
        "case {case:?}: an instance of an updating environment has an operation in progress, got {}",
        updating.body
    );

    // Terminated: "Environment is not running", so it is made of no instance
    // and enhanced health knows of none.
    std::thread::sleep(PAST_DUE);
    let terminated = emulator
        .call(
            "TerminateEnvironment",
            &[("EnvironmentName", "health-lifecycle")],
        )
        .await;
    assert_eq!(
        terminated.status, 200,
        "case \"Terminated\": TerminateEnvironment answered {} with body {}",
        terminated.status, terminated.body
    );
    std::thread::sleep(PAST_DUE);
    let case = "Terminated";
    assert!(
        inventory(&emulator, "health-lifecycle").await.is_empty(),
        "case {case:?}: a terminated environment holds no inventory"
    );
    let gone = instances_health(&emulator, "health-lifecycle").await;
    assert!(
        gone.values_at("InstanceHealthList/member/InstanceId").is_empty(),
        "case {case:?}: an environment that runs nothing reports no instance health, got {}",
        gone.body
    );
    let gone_summary = environment_health(&emulator, "health-lifecycle").await;
    for name_of_bucket in BUCKETS {
        assert_eq!(
            bucket(&gone_summary, name_of_bucket, case),
            0,
            "case {case:?}: {name_of_bucket} counts no instance, got {}",
            gone_summary.body
        );
    }
}

/// RULE (the members): each instance's reported members are derived from the
/// same stored environment — its type from the configuration options the
/// environment carries, its zone from its place in the group (a zone of the
/// environment's own Region), its launch instant and its deployment from the
/// environment's record — and two reads of one environment report the same
/// values.
#[tokio::test]
async fn each_instances_members_are_derived_from_the_stored_environment() {
    let emulator = Emulator::new();
    application(&emulator).await;

    struct Case {
        environment: &'static str,
        options: Vec<(&'static str, &'static str)>,
        /// The instance type every instance of this environment reports.
        instance_type: String,
    }
    // Rows differ along WHICH option names the instance type: the Auto Scaling
    // group's list of types (`aws:ec2:instances` / `InstanceTypes`, "a
    // comma-separated list"), the launch configuration's single type
    // (`aws:autoscaling:launchconfiguration` / `InstanceType`) — a pair the
    // issue did not cite — and neither.
    let cases = [
        Case {
            environment: "health-type-default",
            options: vec![],
            instance_type: eb::state::DEFAULT_INSTANCE_TYPE.to_string(),
        },
        Case {
            environment: "health-type-list",
            options: vec![
                ("OptionSettings.member.1.Namespace", "aws:ec2:instances"),
                ("OptionSettings.member.1.OptionName", "InstanceTypes"),
                ("OptionSettings.member.1.Value", "m5.large,t3.small"),
            ],
            instance_type: "m5.large".to_string(),
        },
        Case {
            environment: "health-type-launch-configuration",
            options: vec![
                (
                    "OptionSettings.member.1.Namespace",
                    "aws:autoscaling:launchconfiguration",
                ),
                ("OptionSettings.member.1.OptionName", "InstanceType"),
                ("OptionSettings.member.1.Value", "c5.xlarge"),
            ],
            instance_type: "c5.xlarge".to_string(),
        },
        Case {
            environment: "health-type-group",
            options: vec![
                ("OptionSettings.member.1.Namespace", "aws:autoscaling:asg"),
                ("OptionSettings.member.1.OptionName", "MinSize"),
                ("OptionSettings.member.1.Value", "3"),
                ("OptionSettings.member.2.Namespace", "aws:ec2:instances"),
                ("OptionSettings.member.2.OptionName", "InstanceTypes"),
                ("OptionSettings.member.2.Value", "t3.medium"),
            ],
            instance_type: "t3.medium".to_string(),
        },
    ];

    for case in &cases {
        launch(&emulator, case.environment, &case.options).await;
    }
    std::thread::sleep(PAST_DUE);

    for case in &cases {
        let name = case.environment;
        let health = instances_health(&emulator, name).await;
        let entries = health.elements_at("InstanceHealthList/member");
        assert!(
            !entries.is_empty(),
            "case {name:?}: expected the environment's instances, got {}",
            health.body
        );
        let mut zones = Vec::new();
        for entry in &entries {
            let instance = entry.expect("InstanceId", name);
            assert_eq!(
                entry.expect("InstanceType", name),
                case.instance_type,
                "case {name:?}: instance {instance} runs the type the options name, got {}",
                entry.body
            );
            let zone = entry.expect("AvailabilityZone", name);
            assert!(
                zone.starts_with(TEST_REGION) && zone.len() > TEST_REGION.len(),
                "case {name:?}: instance {instance} runs in a zone of its own Region, got {zone:?}"
            );
            zones.push(zone.to_string());
            // "The time at which the EC2 instance was launched": the launch
            // that provisioned this instance, dated on the environment's own
            // record rather than at the moment of the read.
            let launched_at = entry.expect("LaunchedAt", name);
            assert!(
                launched_at.ends_with('Z'),
                "case {name:?}: instance {instance} reports when it was launched, got {launched_at:?}"
            );
            assert_eq!(
                entry.expect("Deployment/VersionLabel", name),
                VERSION,
                "case {name:?}: instance {instance} runs the version the environment deploys, got {}",
                entry.body
            );
            assert_eq!(
                entry.expect("Deployment/DeploymentId", name),
                "1",
                "case {name:?}: the launch is the environment's first deployment, got {}",
                entry.body
            );
            // No application traffic is served here, so the metric window holds
            // no request.
            assert_eq!(
                entry.expect("ApplicationMetrics/RequestCount", name),
                "0",
                "case {name:?}: instance {instance} served no request, got {}",
                entry.body
            );
        }
        // An Auto Scaling group of several instances is spread over the Region's
        // zones rather than piled into one.
        let distinct: std::collections::BTreeSet<&String> = zones.iter().collect();
        assert_eq!(
            distinct.len(),
            zones.len().min(3),
            "case {name:?}: the group is spread over the Region's zones, got {zones:?}"
        );

        // The values are PERSISTED, not minted per call.
        let again = instances_health(&emulator, name).await;
        for member in ["AvailabilityZone", "LaunchedAt", "InstanceType"] {
            let path = format!("InstanceHealthList/member/{member}");
            assert_eq!(
                again.values_at(&path),
                health.values_at(&path),
                "case {name:?}: two reads report the same {member}, got {} then {}",
                health.body,
                again.body
            );
        }
    }

    // "This number increases by one each time that you deploy source code or
    // change instance configuration settings": the update is the next
    // deployment of the environment, which its instances then report.
    let environment = cases[0].environment;
    let updated = emulator
        .call(
            "UpdateEnvironment",
            &[
                ("EnvironmentName", environment),
                ("VersionLabel", VERSION),
            ],
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "case \"redeployed\": UpdateEnvironment answered {} with body {}",
        updated.status, updated.body
    );
    std::thread::sleep(PAST_DUE);
    let redeployed = instances_health(&emulator, environment).await;
    assert_eq!(
        redeployed.value("InstanceHealthList/member/Deployment/DeploymentId"),
        Some("2"),
        "case \"redeployed\": the update is the next deployment, got {}",
        redeployed.body
    );
}
