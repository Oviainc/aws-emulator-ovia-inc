// SPDX-License-Identifier: CC0-1.0

//! The Auto Scaling group lifecycle, end to end over the awsQuery wire.
//!
//! Every assertion here drives the service the way the router does — one
//! `MultiServiceRouter` holding this service over one shared engine — so what is
//! under test is the HTTP behaviour (status, error code, XML members), not a
//! handler called directly.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;

use crate::support::{new_engine, query, router_with, tag_value, tag_values, REGION};

/// Wait past the lifecycle transition the service owes, so what the clock made
/// due is the state the next read reports.
fn wait_for_the_lifecycle_transition() {
    std::thread::sleep(autoscaling::state::LIFECYCLE_TRANSITION + Duration::from_millis(150));
}

/// The launch source every group in this suite is created from.
///
/// A launch TEMPLATE, not a launch configuration: no account can create a launch
/// configuration (`CreateLaunchConfiguration` is refused — see
/// `launch_configurations_are_unavailable.rs`), so a group that named one could
/// never be created either.
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// The happy path the bootstrap plan lays out: create, read back, scale, update,
/// refuse a duplicate, refuse a delete that would drop live instances, force the
/// delete, and read back that the group is gone.
#[tokio::test]
async fn the_group_lifecycle_round_trips_over_the_wire() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-1"),
            LAUNCH_SOURCE,
            ("MinSize", "1"),
            ("MaxSize", "4"),
            ("DesiredCapacity", "2"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateAutoScalingGroup: got {status}: {body}");

    // An instance Amazon EC2 Auto Scaling has just launched is reported `Pending`
    // and reaches `InService` only once the transition comes due.
    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-1")],
    )
    .await;
    assert_eq!(status, 200, "DescribeAutoScalingGroups: got {status}: {body}");
    assert_eq!(
        tag_values(&body, "LifecycleState"),
        vec!["Pending".to_string(), "Pending".to_string()],
        "a freshly launched instance must be reported Pending, got {body}"
    );

    wait_for_the_lifecycle_transition();
    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-1")],
    )
    .await;
    assert_eq!(status, 200, "DescribeAutoScalingGroups: got {status}: {body}");
    for (tag, expected) in [
        ("AutoScalingGroupName", "asg-1"),
        ("MinSize", "1"),
        ("MaxSize", "4"),
        ("DesiredCapacity", "2"),
        ("HealthCheckType", "EC2"),
        ("LaunchTemplateName", "lt-1"),
    ] {
        assert_eq!(
            tag_value(&body, tag),
            expected,
            "DescribeAutoScalingGroups {tag}: expected {expected}, got {body}"
        );
    }
    assert!(
        !tag_value(&body, "CreatedTime").is_empty(),
        "the group must report a CreatedTime, got {body}"
    );
    assert!(
        tag_value(&body, "AutoScalingGroupARN").contains(&format!(
            "arn:aws:autoscaling:{REGION}:{}:autoScalingGroup:",
            engine::EMULATOR_ACCOUNT_ID
        )),
        "group ARN: got {body}"
    );
    let instances = tag_values(&body, "InstanceId");
    assert_eq!(
        instances.len(),
        2,
        "the group must hold DesiredCapacity instances, got {instances:?}"
    );
    assert_eq!(
        tag_values(&body, "LifecycleState"),
        vec!["InService".to_string(), "InService".to_string()],
        "once the transition is due every instance is InService, got {body}"
    );
    assert_eq!(
        tag_values(&body, "HealthStatus"),
        vec!["Healthy".to_string(), "Healthy".to_string()],
        "a launched instance is Healthy, got {body}"
    );
    for instance_id in &instances {
        assert!(
            instance_id.starts_with("i-") && instance_id.len() == 19,
            "instance id {instance_id:?} must have the form AWS reports (i- plus 17 hex digits)"
        );
    }

    // Each launch recorded its own scaling activity against the group.
    {
        let state = autoscaling::state::service_state(&*engine).expect("the service store");
        let state = state.lock();
        let launches: Vec<&String> = state
            .activities
            .iter()
            .filter(|activity| activity.auto_scaling_group_name == "asg-1")
            .filter_map(|activity| activity.description.as_ref())
            .collect();
        assert_eq!(
            launches.len(),
            2,
            "two launches must record two activities, got {launches:?}"
        );
        for instance_id in &instances {
            assert!(
                launches.iter().any(|d| d.contains(instance_id.as_str())),
                "an activity must name the instance it launched ({instance_id}), got {launches:?}"
            );
        }
    }

    let (status, body) = query(
        &router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", "asg-1"),
            ("DesiredCapacity", "3"),
        ],
    )
    .await;
    assert_eq!(status, 200, "SetDesiredCapacity: got {status}: {body}");
    wait_for_the_lifecycle_transition();
    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-1")],
    )
    .await;
    assert_eq!(
        tag_value(&body, "DesiredCapacity"),
        "3",
        "SetDesiredCapacity must change the stored capacity, got {body}"
    );
    assert_eq!(
        tag_values(&body, "InstanceId").len(),
        3,
        "the group's instances must match its desired capacity, got {body}"
    );

    // An update applies only the members it carries.
    let (status, body) = query(
        &router,
        "UpdateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-1"),
            ("MaxSize", "6"),
            ("HealthCheckType", "ELB"),
            ("HealthCheckGracePeriod", "300"),
        ],
    )
    .await;
    assert_eq!(status, 200, "UpdateAutoScalingGroup: got {status}: {body}");
    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-1")],
    )
    .await;
    for (tag, expected) in [
        ("MaxSize", "6"),
        ("HealthCheckType", "ELB"),
        ("HealthCheckGracePeriod", "300"),
        ("MinSize", "1"),
        ("DesiredCapacity", "3"),
        ("LaunchTemplateName", "lt-1"),
    ] {
        assert_eq!(
            tag_value(&body, tag),
            expected,
            "after the update {tag}: expected {expected}, got {body}"
        );
    }

    // A second group under a name already taken is refused.
    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-1"),
            LAUNCH_SOURCE,
            ("MinSize", "1"),
            ("MaxSize", "2"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(status, 400, "a duplicate group name: got {status}: {body}");
    assert!(
        body.contains("<Code>AlreadyExists</Code>"),
        "a duplicate group name must answer the AlreadyExists query error code, got {body}"
    );

    // A group that still holds instances cannot be deleted without ForceDelete.
    let (status, body) = query(
        &router,
        "DeleteAutoScalingGroup",
        &[("AutoScalingGroupName", "asg-1")],
    )
    .await;
    assert_eq!(status, 400, "deleting a group that holds instances: got {status}: {body}");
    assert!(
        body.contains("<Code>ResourceInUse</Code>"),
        "a group that still holds instances must answer ResourceInUse, got {body}"
    );

    let (status, body) = query(
        &router,
        "DeleteAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-1"),
            ("ForceDelete", "true"),
        ],
    )
    .await;
    assert_eq!(status, 200, "DeleteAutoScalingGroup --force-delete: got {status}: {body}");

    let (status, body) = query(&router, "DescribeAutoScalingGroups", &[]).await;
    assert_eq!(status, 200, "DescribeAutoScalingGroups: got {status}: {body}");
    assert!(
        tag_values(&body, "AutoScalingGroupName").is_empty(),
        "a deleted group must no longer be listed, got {body}"
    );

    // And a name that names nothing is refused, not silently accepted.
    for action in ["DeleteAutoScalingGroup", "SetDesiredCapacity", "UpdateAutoScalingGroup"] {
        let params: Vec<(&str, &str)> = if action == "SetDesiredCapacity" {
            vec![("AutoScalingGroupName", "nope"), ("DesiredCapacity", "1")]
        } else {
            vec![("AutoScalingGroupName", "nope")]
        };
        let (status, body) = query(&router, action, &params).await;
        assert_eq!(status, 400, "{action} on an unknown group: got {status}: {body}");
        assert!(
            body.contains("<Code>ValidationError</Code>"),
            "{action} on an unknown group must answer ValidationError, got {body}"
        );
    }
    let (status, body) = query(
        &router,
        "DeleteLaunchConfiguration",
        &[("LaunchConfigurationName", "nope")],
    )
    .await;
    assert_eq!(status, 400, "DeleteLaunchConfiguration on an unknown name: got {status}: {body}");
    assert!(
        body.contains("<Code>ValidationError</Code>"),
        "an unknown launch configuration must answer ValidationError, got {body}"
    );
}

/// One row of the create/describe table: a group created with `params` must read
/// back with `expected` for each member named, whether the request set it or the
/// service defaulted it.
struct GroupCase {
    case: &'static str,
    params: Vec<(&'static str, &'static str)>,
    expected: Vec<(&'static str, &'static str)>,
    instances: usize,
}

/// Every member a create carries is persisted and reported back, and every member
/// it omits is reported at the default the model documents.
#[tokio::test]
async fn created_group_members_read_back_as_created() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    let cases = [
        GroupCase {
            case: "defaults: only the required members plus a launch source and a zone",
            params: vec![
                LAUNCH_SOURCE,
                ("MinSize", "1"),
                ("MaxSize", "4"),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
            expected: vec![
                // "If you do not specify a desired capacity, the default is the
                // minimum size of the group."
                ("DesiredCapacity", "1"),
                ("DefaultCooldown", "300"),
                ("HealthCheckType", "EC2"),
                ("HealthCheckGracePeriod", "0"),
                ("TerminationPolicies", "<member>Default</member>"),
                ("NewInstancesProtectedFromScaleIn", "false"),
            ],
            instances: 1,
        },
        GroupCase {
            case: "every size and schedule member set explicitly, two zones",
            params: vec![
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "10"),
                ("DesiredCapacity", "3"),
                ("DefaultCooldown", "60"),
                ("HealthCheckType", "ELB"),
                ("HealthCheckGracePeriod", "120"),
                ("AvailabilityZones.member.1", "us-west-2a"),
                ("AvailabilityZones.member.2", "us-west-2b"),
                ("TerminationPolicies.member.1", "OldestInstance"),
                ("NewInstancesProtectedFromScaleIn", "true"),
                ("VPCZoneIdentifier", "subnet-1,subnet-2"),
                ("MaxInstanceLifetime", "86400"),
                ("CapacityRebalance", "true"),
                ("Context", "a context"),
            ],
            expected: vec![
                ("MinSize", "0"),
                ("MaxSize", "10"),
                ("DesiredCapacity", "3"),
                ("DefaultCooldown", "60"),
                ("HealthCheckType", "ELB"),
                ("HealthCheckGracePeriod", "120"),
                ("TerminationPolicies", "<member>OldestInstance</member>"),
                ("NewInstancesProtectedFromScaleIn", "true"),
                ("VPCZoneIdentifier", "subnet-1,subnet-2"),
                ("MaxInstanceLifetime", "86400"),
                ("CapacityRebalance", "true"),
                ("Context", "a context"),
                (
                    "AvailabilityZones",
                    "<member>us-west-2a</member><member>us-west-2b</member>",
                ),
            ],
            instances: 3,
        },
        GroupCase {
            case: "a group that asks for no capacity holds no instances",
            params: vec![
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "0"),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
            expected: vec![("DesiredCapacity", "0"), ("MaxSize", "0")],
            instances: 0,
        },
        GroupCase {
            case: "a launch template instead of a launch configuration",
            params: vec![
                ("LaunchTemplate.LaunchTemplateName", "lt-1"),
                ("LaunchTemplate.Version", "2"),
                ("MinSize", "2"),
                ("MaxSize", "2"),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
            expected: vec![
                ("LaunchTemplateName", "lt-1"),
                ("Version", "2"),
                ("DesiredCapacity", "2"),
            ],
            instances: 2,
        },
        GroupCase {
            case: "tags are reported against the group that carries them",
            params: vec![
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("AvailabilityZones.member.1", "us-west-2a"),
                ("Tags.member.1.Key", "env"),
                ("Tags.member.1.Value", "prod"),
                ("Tags.member.1.PropagateAtLaunch", "true"),
            ],
            expected: vec![
                ("Key", "env"),
                ("Value", "prod"),
                ("PropagateAtLaunch", "true"),
                ("ResourceType", "auto-scaling-group"),
            ],
            instances: 0,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let name = format!("asg-{index}");
        let mut params = vec![("AutoScalingGroupName", name.as_str())];
        params.extend(case.params.iter().copied());
        let (status, body) = query(&router, "CreateAutoScalingGroup", &params).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateAutoScalingGroup expected 200, got {status}: {body}",
            case.case
        );

        let (status, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeAutoScalingGroups expected 200, got {status}: {body}",
            case.case
        );
        for (tag, expected) in &case.expected {
            assert_eq!(
                &tag_value(&body, tag),
                expected,
                "case {:?}: {tag} expected {expected:?}, got {body}",
                case.case
            );
        }
        assert_eq!(
            tag_values(&body, "InstanceId").len(),
            case.instances,
            "case {:?}: expected {} instances, got {body}",
            case.case,
            case.instances
        );
        assert_eq!(
            tag_value(&body, "ResourceId"),
            if case.expected.iter().any(|(tag, _)| *tag == "Key") {
                name.clone()
            } else {
                String::new()
            },
            "case {:?}: a reported tag names its own group, got {body}",
            case.case
        );

        // `IncludeInstances=false` leaves the instances out, and nothing else.
        let (_, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[
                ("AutoScalingGroupNames.member.1", name.as_str()),
                ("IncludeInstances", "false"),
            ],
        )
        .await;
        assert!(
            tag_values(&body, "InstanceId").is_empty(),
            "case {:?}: IncludeInstances=false must omit the instances, got {body}",
            case.case
        );
        assert_eq!(
            tag_value(&body, "AutoScalingGroupName"),
            name,
            "case {:?}: IncludeInstances=false still reports the group, got {body}",
            case.case
        );
    }
}

/// A size or capacity outside the group's own bounds is refused with
/// `ValidationError`, and a value ON either bound is accepted — on create and on
/// every later capacity change alike.
#[tokio::test]
async fn capacity_outside_the_group_bounds_is_refused() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    // (case, MinSize, MaxSize, DesiredCapacity, accepted)
    let creates = [
        ("the desired capacity on the lower bound", "1", "4", "1", true),
        ("the desired capacity on the upper bound", "1", "4", "4", true),
        ("one below the lower bound", "1", "4", "0", false),
        ("one above the upper bound", "1", "4", "5", false),
        ("a maximum below the minimum", "2", "1", "1", false),
        ("a negative minimum", "-1", "4", "1", false),
    ];
    for (index, (case, min, max, desired, accepted)) in creates.iter().enumerate() {
        let name = format!("bounds-{index}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                LAUNCH_SOURCE,
                ("MinSize", min),
                ("MaxSize", max),
                ("DesiredCapacity", desired),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to be accepted, got {status}: {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the create to be refused, got {status}: {body}"
            );
            assert!(
                body.contains("<Code>ValidationError</Code>"),
                "case {case:?}: expected the ValidationError code, got {body}"
            );
        }
    }

    // A group with no launch source at all, and one with no placement at all, are
    // refused the same way.
    for (case, params) in [
        (
            "neither a launch template nor a launch configuration",
            vec![
                ("AutoScalingGroupName", "no-source"),
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
        ),
        (
            "neither Availability Zones nor subnets",
            vec![
                ("AutoScalingGroupName", "no-placement"),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "1"),
            ],
        ),
        (
            // No launch configuration can exist, so a group naming one names a
            // configuration that is not there.
            "a launch configuration, which this account cannot have",
            vec![
                ("AutoScalingGroupName", "no-config"),
                ("LaunchConfigurationName", "lc-missing"),
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
        ),
    ] {
        let (status, body) = query(&router, "CreateAutoScalingGroup", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create to be refused, got {status}: {body}"
        );
        assert!(
            body.contains("<Code>ValidationError</Code>"),
            "case {case:?}: expected the ValidationError code, got {body}"
        );
    }

    // The same bounds hold for SetDesiredCapacity and for UpdateAutoScalingGroup
    // against a group created with [1, 4].
    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-bounds"),
            LAUNCH_SOURCE,
            ("MinSize", "1"),
            ("MaxSize", "4"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateAutoScalingGroup: got {status}: {body}");

    for (case, desired, accepted) in [
        ("on the lower bound", "1", true),
        ("on the upper bound", "4", true),
        ("one below the lower bound", "0", false),
        ("one above the upper bound", "5", false),
    ] {
        let (status, body) = query(
            &router,
            "SetDesiredCapacity",
            &[
                ("AutoScalingGroupName", "asg-bounds"),
                ("DesiredCapacity", desired),
            ],
        )
        .await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "SetDesiredCapacity {case:?}: expected {expected}, got {status}: {body}"
        );
        if !accepted {
            assert!(
                body.contains("<Code>ValidationError</Code>"),
                "SetDesiredCapacity {case:?}: expected the ValidationError code, got {body}"
            );
        }

        let (status, body) = query(
            &router,
            "UpdateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", "asg-bounds"),
                ("DesiredCapacity", desired),
            ],
        )
        .await;
        assert_eq!(
            status, expected,
            "UpdateAutoScalingGroup {case:?}: expected {expected}, got {status}: {body}"
        );
        if !accepted {
            assert!(
                body.contains("<Code>ValidationError</Code>"),
                "UpdateAutoScalingGroup {case:?}: expected the ValidationError code, got {body}"
            );
        }
    }
}

/// An update that narrows the group's bounds brings the capacity inside them,
/// which the model documents: a new `MinSize` above the current size raises the
/// desired capacity, and a new `MaxSize` below it lowers it.
#[tokio::test]
async fn an_update_clamps_the_capacity_into_the_new_bounds() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    // (case, the update's members, the capacity and bounds afterwards)
    let cases = [
        ("a new minimum above the current capacity", vec![("MinSize", "3")], ("3", "3", "6")),
        ("a new maximum below the current capacity", vec![("MaxSize", "1")], ("1", "0", "1")),
        ("bounds that leave the capacity alone", vec![("MaxSize", "5")], ("2", "0", "5")),
    ];

    for (index, (case, update, (desired, min, max))) in cases.iter().enumerate() {
        let name = format!("clamp-{index}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "6"),
                ("DesiredCapacity", "2"),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: create got {status}: {body}");

        let mut params = vec![("AutoScalingGroupName", name.as_str())];
        params.extend(update.iter().copied());
        let (status, body) = query(&router, "UpdateAutoScalingGroup", &params).await;
        assert_eq!(status, 200, "case {case:?}: update got {status}: {body}");

        wait_for_the_lifecycle_transition();
        let (_, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        for (tag, expected) in [
            ("DesiredCapacity", desired),
            ("MinSize", min),
            ("MaxSize", max),
        ] {
            assert_eq!(
                &tag_value(&body, tag),
                expected,
                "case {case:?}: {tag} expected {expected}, got {body}"
            );
        }
        assert_eq!(
            tag_values(&body, "InstanceId").len(),
            desired.parse::<usize>().expect("a number"),
            "case {case:?}: the instances must match the clamped capacity, got {body}"
        );
    }
}

/// Scaling in reports the surplus instances `Terminating` first, and they leave
/// the group only once that transition comes due.
#[tokio::test]
async fn scaling_in_terminates_the_surplus_instances() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-in"),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", "4"),
            ("DesiredCapacity", "3"),
            ("AvailabilityZones.member.1", "us-west-2a"),
            ("AvailabilityZones.member.2", "us-west-2b"),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateAutoScalingGroup: got {status}: {body}");
    wait_for_the_lifecycle_transition();

    // Instances are spread over the group's zones in launch order.
    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-in")],
    )
    .await;
    assert_eq!(
        tag_values(&body, "AvailabilityZone"),
        vec![
            "us-west-2a".to_string(),
            "us-west-2b".to_string(),
            "us-west-2a".to_string()
        ],
        "instances must be placed across the group's zones, got {body}"
    );

    let (status, body) = query(
        &router,
        "SetDesiredCapacity",
        &[("AutoScalingGroupName", "asg-in"), ("DesiredCapacity", "1")],
    )
    .await;
    assert_eq!(status, 200, "SetDesiredCapacity: got {status}: {body}");

    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-in")],
    )
    .await;
    assert_eq!(
        tag_value(&body, "DesiredCapacity"),
        "1",
        "the desired capacity changes at once, got {body}"
    );
    let states = tag_values(&body, "LifecycleState");
    assert_eq!(
        states.iter().filter(|state| *state == "Terminating").count(),
        2,
        "the surplus instances must be reported Terminating, got {body}"
    );

    wait_for_the_lifecycle_transition();
    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-in")],
    )
    .await;
    assert_eq!(
        tag_values(&body, "InstanceId").len(),
        1,
        "a terminated instance leaves the group once the transition is due, got {body}"
    );
    assert_eq!(
        tag_values(&body, "LifecycleState"),
        vec!["InService".to_string()],
        "the instance that stayed is in service, got {body}"
    );
}

/// `HonorCooldown` makes the service wait for the cooldown period to complete
/// before initiating the scaling activity, as the model documents; without it a
/// manual capacity change is applied at once.
#[tokio::test]
async fn honoring_the_cooldown_defers_the_capacity_change() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    // A one-second cooldown, so the deferral is observable either side of it.
    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-cooldown"),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", "4"),
            ("DesiredCapacity", "1"),
            ("DefaultCooldown", "1"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateAutoScalingGroup: got {status}: {body}");

    let (status, body) = query(
        &router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", "asg-cooldown"),
            ("DesiredCapacity", "3"),
            ("HonorCooldown", "true"),
        ],
    )
    .await;
    assert_eq!(status, 200, "SetDesiredCapacity: got {status}: {body}");

    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-cooldown")],
    )
    .await;
    assert_eq!(
        tag_value(&body, "DesiredCapacity"),
        "1",
        "a change that honours the cooldown waits for it, got {body}"
    );

    std::thread::sleep(Duration::from_millis(1100));
    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-cooldown")],
    )
    .await;
    assert_eq!(
        tag_value(&body, "DesiredCapacity"),
        "3",
        "once the cooldown has elapsed the deferred change is applied, got {body}"
    );

    // A deferred change is brought inside the bounds in force when it is applied:
    // an update that narrows them while the change waits is not overridden by it.
    let (status, body) = query(
        &router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", "asg-cooldown"),
            ("DesiredCapacity", "4"),
            ("HonorCooldown", "true"),
        ],
    )
    .await;
    assert_eq!(status, 200, "a second deferred change: got {status}: {body}");
    let (status, body) = query(
        &router,
        "UpdateAutoScalingGroup",
        &[("AutoScalingGroupName", "asg-cooldown"), ("MaxSize", "2")],
    )
    .await;
    assert_eq!(status, 200, "narrowing the bounds: got {status}: {body}");
    std::thread::sleep(Duration::from_millis(1100));
    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-cooldown")],
    )
    .await;
    assert_eq!(
        tag_value(&body, "DesiredCapacity"),
        "2",
        "a deferred change must not take the group outside its own bounds, got {body}"
    );

    // The same call without HonorCooldown is applied immediately, cooldown or not.
    let (status, body) = query(
        &router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", "asg-cooldown"),
            ("DesiredCapacity", "1"),
        ],
    )
    .await;
    assert_eq!(status, 200, "SetDesiredCapacity: got {status}: {body}");
    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-cooldown")],
    )
    .await;
    assert_eq!(
        tag_value(&body, "DesiredCapacity"),
        "1",
        "a manual change does not honour the cooldown by default, got {body}"
    );
}

/// `MaxRecords` bounds a page and `NextToken` walks the rest, over the documented
/// default (50) and maximum (100); a token the service did not issue is
/// `InvalidNextToken`.
#[tokio::test]
async fn describes_page_by_max_records_and_next_token() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    // 51 groups: one more than the documented default page size. They ask for no
    // capacity, so the page itself is what is under test.
    let total = 51;
    for index in 0..total {
        let name = format!("page-{index:03}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "0"),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
        )
        .await;
        assert_eq!(status, 200, "creating {name}: got {status}: {body}");
    }

    // (case, MaxRecords, the page size to expect)
    // (case, MaxRecords, the page size to expect)
    for (case, max_records, expected) in [
        ("the documented default page size", None, 50),
        ("one record per page", Some("1"), 1),
        ("a page smaller than the list", Some("20"), 20),
        ("the documented maximum", Some("100"), total),
    ] {
        let mut params = vec![];
        if let Some(max_records) = max_records {
            params.push(("MaxRecords", max_records));
        }
        let (status, body) = query(&router, "DescribeAutoScalingGroups", &params).await;
        assert_eq!(status, 200, "case {case:?}: got {status}: {body}");
        assert_eq!(
            tag_values(&body, "AutoScalingGroupName").len(),
            expected,
            "case {case:?}: expected {expected} groups on the page, got {body}"
        );
    }

    // A page size outside the documented range is refused, on both describes.
    for action in ["DescribeAutoScalingGroups", "DescribeLaunchConfigurations"] {
        for (case, max_records) in [
            ("one past the documented maximum", "101"),
            ("far past the documented maximum", "500"),
            ("below the smallest page", "0"),
            ("a negative page size", "-1"),
        ] {
            let (status, body) = query(&router, action, &[("MaxRecords", max_records)]).await;
            assert_eq!(
                status, 400,
                "{action} with {case:?}: got {status}: {body}"
            );
            assert!(
                body.contains("<Code>ValidationError</Code>"),
                "{action} with {case:?}: expected ValidationError, got {body}"
            );
        }
    }

    // Walking the tokens visits every group exactly once, in name order.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..total + 1 {
        let mut params = vec![("MaxRecords", "20")];
        if let Some(token) = token.as_deref() {
            params.push(("NextToken", token));
        }
        let (status, body) = query(&router, "DescribeAutoScalingGroups", &params).await;
        assert_eq!(status, 200, "paging: got {status}: {body}");
        seen.extend(tag_values(&body, "AutoScalingGroupName"));
        let next = tag_values(&body, "NextToken").first().cloned();
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    let expected: Vec<String> = (0..total).map(|index| format!("page-{index:03}")).collect();
    assert_eq!(
        seen, expected,
        "walking the pages must visit every group once, in name order"
    );

    for (case, token) in [
        ("a token that is not a number", "not-a-token"),
        ("a token that is not the shape the service issues", "page-003"),
        ("a bare offset, without the marker an issued token carries", "9999"),
        ("a token that points past the end of the list", "next-9999"),
        ("the first page, for which no token is ever issued", "next-0"),
        ("a bare zero", "0"),
    ] {
        let (status, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("NextToken", token)],
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: got {status}: {body}");
        assert!(
            body.contains("<Code>InvalidNextToken</Code>"),
            "case {case:?}: expected the InvalidNextToken code, got {body}"
        );
        // The refusal names the token the caller sent: that is what it has to
        // stop using.
        assert!(
            body.contains(token),
            "case {case:?}: the refusal must quote the token {token:?}, got {body}"
        );
    }
}

/// One row of the tag-filter table: the filter parameters a describe carries, and
/// the groups they must select.
type FilterCase = (&'static str, Vec<(&'static str, &'static str)>, Vec<&'static str>);

/// The tag filters the model documents for this operation select by tag key, by
/// tag value, and by the key/value pair; filters are joined with AND. A filter
/// name the operation does not define is refused.
#[tokio::test]
async fn tag_filters_select_the_groups_that_carry_the_tags() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    // (group, tag key, tag value)
    for (name, key, value) in [
        ("tagged-prod", "env", "prod"),
        ("tagged-test", "env", "test"),
        ("tagged-team", "team", "platform"),
    ] {
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "0"),
                ("AvailabilityZones.member.1", "us-west-2a"),
                ("Tags.member.1.Key", key),
                ("Tags.member.1.Value", value),
            ],
        )
        .await;
        assert_eq!(status, 200, "creating {name}: got {status}: {body}");
    }

    // (case, filter params, the groups to expect)
    let cases: Vec<FilterCase> = vec![
        (
            "by tag key",
            vec![
                ("Filters.member.1.Name", "tag-key"),
                ("Filters.member.1.Values.member.1", "env"),
            ],
            vec!["tagged-prod", "tagged-test"],
        ),
        (
            "by tag value",
            vec![
                ("Filters.member.1.Name", "tag-value"),
                ("Filters.member.1.Values.member.1", "platform"),
            ],
            vec!["tagged-team"],
        ),
        (
            "by the key/value pair",
            vec![
                ("Filters.member.1.Name", "tag:env"),
                ("Filters.member.1.Values.member.1", "prod"),
            ],
            vec!["tagged-prod"],
        ),
        (
            "two filters are joined with AND",
            vec![
                ("Filters.member.1.Name", "tag-key"),
                ("Filters.member.1.Values.member.1", "env"),
                ("Filters.member.2.Name", "tag-value"),
                ("Filters.member.2.Values.member.1", "test"),
            ],
            vec!["tagged-test"],
        ),
        (
            "several values in one filter are joined with OR",
            vec![
                ("Filters.member.1.Name", "tag-value"),
                ("Filters.member.1.Values.member.1", "prod"),
                ("Filters.member.1.Values.member.2", "platform"),
            ],
            vec!["tagged-prod", "tagged-team"],
        ),
        (
            "a tag nothing carries selects nothing",
            vec![
                ("Filters.member.1.Name", "tag-key"),
                ("Filters.member.1.Values.member.1", "absent"),
            ],
            vec![],
        ),
    ];

    for (case, params, expected) in cases {
        let (status, body) = query(&router, "DescribeAutoScalingGroups", &params).await;
        assert_eq!(status, 200, "case {case:?}: got {status}: {body}");
        let names = tag_values(&body, "AutoScalingGroupName");
        let expected: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
        assert_eq!(
            names, expected,
            "case {case:?}: expected {expected:?}, got {names:?} in {body}"
        );
    }

    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[
            ("Filters.member.1.Name", "instance-id"),
            ("Filters.member.1.Values.member.1", "i-123"),
        ],
    )
    .await;
    assert_eq!(status, 400, "an undefined filter name: got {status}: {body}");
    assert!(
        body.contains("<Code>ValidationError</Code>"),
        "an undefined filter name must answer ValidationError, got {body}"
    );
}

/// Deletion protection controls whether `DeleteAutoScalingGroup` is allowed at
/// all, per the level the group carries.
#[tokio::test]
async fn deletion_protection_refuses_the_deletes_it_names() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    // (case, DeletionProtection, ForceDelete, the status to expect)
    let cases = [
        ("no protection", "none", "false", 200),
        ("force deletion prevented, plain delete", "prevent-force-deletion", "false", 200),
        ("force deletion prevented, forced delete", "prevent-force-deletion", "true", 400),
        ("all deletion prevented, plain delete", "prevent-all-deletion", "false", 400),
        ("all deletion prevented, forced delete", "prevent-all-deletion", "true", 400),
    ];

    for (index, (case, protection, force, expected)) in cases.iter().enumerate() {
        let name = format!("protected-{index}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "0"),
                ("AvailabilityZones.member.1", "us-west-2a"),
                ("DeletionProtection", protection),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: create got {status}: {body}");

        let (status, body) = query(
            &router,
            "DeleteAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                ("ForceDelete", force),
            ],
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected {expected}, got {status}: {body}"
        );
        if *expected == 400 {
            assert!(
                body.contains("<Code>ValidationError</Code>"),
                "case {case:?}: expected the ValidationError code, got {body}"
            );
            let (_, body) = query(
                &router,
                "DescribeAutoScalingGroups",
                &[("AutoScalingGroupNames.member.1", name.as_str())],
            )
            .await;
            assert_eq!(
                tag_value(&body, "AutoScalingGroupName"),
                name,
                "case {case:?}: a refused delete leaves the group in place, got {body}"
            );
        }
    }
}

/// One row of the refusal table: the case, the action to call, and its
/// parameters.
type RefusalCase = (&'static str, &'static str, Vec<(&'static str, &'static str)>);

/// Configuration this emulator cannot read is refused outright — never reported as
/// a resource with invented attributes.
///
/// A group that names only subnets is NOT such a case: its zones are the zones of
/// those subnets, which the account's own Amazon EC2 store holds, so it is created
/// and its capacity placed (`vpc_zone_identifier_zones.rs`).
#[tokio::test]
async fn what_the_emulator_cannot_answer_faithfully_is_refused() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    // (case, action, params) — the configuration asked for lives on a resource of
    // another service that this request does not name, so it answers 501 rather
    // than inventing it, and creates nothing.
    let refused: Vec<RefusalCase> = vec![(
        "a group built from a running EC2 instance",
        "CreateAutoScalingGroup",
        vec![
            ("AutoScalingGroupName", "from-instance"),
            ("InstanceId", "i-0123456789abcdef0"),
            ("MinSize", "1"),
            ("MaxSize", "1"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )];

    for (case, action, params) in refused {
        let (status, body) = query(&router, action, &params).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected 501, got {status}: {body}"
        );
        assert!(
            body.contains("<Code>NotImplemented</Code>"),
            "case {case:?}: expected the NotImplemented code, got {body}"
        );
    }

    let (_, body) = query(&router, "DescribeAutoScalingGroups", &[]).await;
    assert!(
        tag_values(&body, "AutoScalingGroupName").is_empty(),
        "a refused create must leave no group behind, got {body}"
    );
}

/// Sizes this emulator cannot hold are refused before anything is allocated: the
/// bounds an operation checks against each other cannot bound the ABSOLUTE size,
/// and every unit of capacity is a stored instance.
#[tokio::test]
async fn sizes_past_the_capacity_ceiling_are_refused() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    let ceiling = autoscaling::state::MAX_GROUP_CAPACITY;
    let on_the_ceiling = ceiling.to_string();
    let past_the_ceiling = (ceiling + 1).to_string();
    let the_top_of_the_range = i32::MAX.to_string();

    // (case, MinSize, MaxSize, DesiredCapacity, the status and code to expect)
    let cases: Vec<(&str, &str, &str, &str, u16, &str)> = vec![
        (
            "a maximum on the ceiling",
            "0",
            on_the_ceiling.as_str(),
            "0",
            200,
            "",
        ),
        (
            "a maximum one past the ceiling",
            "0",
            past_the_ceiling.as_str(),
            "0",
            400,
            "LimitExceeded",
        ),
        (
            "a desired capacity one past the ceiling",
            "0",
            the_top_of_the_range.as_str(),
            past_the_ceiling.as_str(),
            400,
            "LimitExceeded",
        ),
        (
            "every size at the top of the modelled integer range",
            the_top_of_the_range.as_str(),
            the_top_of_the_range.as_str(),
            the_top_of_the_range.as_str(),
            400,
            "LimitExceeded",
        ),
    ];

    for (index, (case, min, max, desired, status_expected, code)) in cases.iter().enumerate() {
        let name = format!("ceiling-{index}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                LAUNCH_SOURCE,
                ("MinSize", min),
                ("MaxSize", max),
                ("DesiredCapacity", desired),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
        )
        .await;
        assert_eq!(
            status, *status_expected,
            "case {case:?}: expected {status_expected}, got {status}: {body}"
        );
        if !code.is_empty() {
            assert!(
                body.contains(&format!("<Code>{code}</Code>")),
                "case {case:?}: expected the {code} code, got {body}"
            );
            let (_, body) = query(
                &router,
                "DescribeAutoScalingGroups",
                &[("AutoScalingGroupNames.member.1", name.as_str())],
            )
            .await;
            assert!(
                tag_values(&body, "AutoScalingGroupName").is_empty(),
                "case {case:?}: a refused create must store nothing, got {body}"
            );
        }
    }

    // An update cannot raise a stored group past the ceiling either.
    let (status, body) = query(
        &router,
        "UpdateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "ceiling-0"),
            ("MaxSize", past_the_ceiling.as_str()),
        ],
    )
    .await;
    assert_eq!(status, 400, "an update past the ceiling: got {status}: {body}");
    assert!(
        body.contains("<Code>ValidationError</Code>"),
        "an update past the ceiling must answer ValidationError, got {body}"
    );
    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "ceiling-0")],
    )
    .await;
    assert_eq!(
        tag_value(&body, "MaxSize"),
        on_the_ceiling,
        "a refused update must leave the group's bounds alone, got {body}"
    );

    // Nor can a capacity change past it.
    let (status, body) = query(
        &router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", "ceiling-0"),
            ("DesiredCapacity", past_the_ceiling.as_str()),
        ],
    )
    .await;
    assert_eq!(status, 400, "a capacity change past the ceiling: got {status}: {body}");
    assert!(
        body.contains("<Code>ValidationError</Code>"),
        "a capacity change past the ceiling must answer ValidationError, got {body}"
    );
}

/// The member constraints the model documents in prose — which the boundary
/// validator cannot enforce — are enforced here: the group-name character rule,
/// the mutual exclusion of `AvailabilityZones` and `AvailabilityZoneIds`, and the
/// `MaxInstanceLifetime` range.
#[tokio::test]
async fn documented_member_rules_are_enforced() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    // "The name can contain any ASCII character 33 to 126 ... You cannot use a
    // colon (:) in the name."
    for (case, name, accepted) in [
        ("punctuation inside the documented range", "asg-1.name_ok+", true),
        ("the smallest and largest documented characters", "!~", true),
        ("a colon, which the model forbids", "asg:1", false),
        ("a space, which is ASCII 32", "asg 1", false),
        ("a character outside ASCII", "asg-\u{e9}", false),
    ] {
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "0"),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
        )
        .await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?} ({name:?}): expected {expected}, got {status}: {body}"
        );
        if !accepted {
            assert!(
                body.contains("<Code>ValidationError</Code>"),
                "case {case:?}: expected ValidationError, got {body}"
            );
        }
    }

    // "If specified, the value must be either 0 or a number equal to or greater
    // than 86,400 seconds (1 day)."
    for (index, (case, lifetime, accepted)) in [
        ("zero, which the model allows", "0", true),
        ("exactly one day, the documented bound", "86400", true),
        ("one second past the bound", "86401", true),
        ("one second below the bound", "86399", false),
        ("a small positive value", "1", false),
    ]
    .iter()
    .enumerate()
    {
        let name = format!("lifetime-{index}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "0"),
                ("AvailabilityZones.member.1", "us-west-2a"),
                ("MaxInstanceLifetime", lifetime),
            ],
        )
        .await;
        let expected = if *accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}: {body}"
        );
        if !*accepted {
            assert!(
                body.contains("<Code>ValidationError</Code>"),
                "case {case:?}: expected ValidationError, got {body}"
            );
        }

        // The same rule holds on the update path.
        let (status, body) = query(
            &router,
            "UpdateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", "lifetime-0"),
                ("MaxInstanceLifetime", lifetime),
            ],
        )
        .await;
        assert_eq!(
            status, expected,
            "UpdateAutoScalingGroup {case:?}: expected {expected}, got {status}: {body}"
        );
    }

    // "You cannot specify both AvailabilityZones and AvailabilityZoneIds in the
    // same request."
    for (case, action, params) in [
        (
            "a create naming both",
            "CreateAutoScalingGroup",
            vec![
                ("AutoScalingGroupName", "both-zones"),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "0"),
                ("AvailabilityZones.member.1", "us-west-2a"),
                ("AvailabilityZoneIds.member.1", "usw2-az1"),
            ],
        ),
        (
            "an update naming both",
            "UpdateAutoScalingGroup",
            vec![
                ("AutoScalingGroupName", "lifetime-0"),
                ("AvailabilityZones.member.1", "us-west-2a"),
                ("AvailabilityZoneIds.member.1", "usw2-az1"),
            ],
        ),
    ] {
        let (status, body) = query(&router, action, &params).await;
        assert_eq!(status, 400, "case {case:?}: got {status}: {body}");
        assert!(
            body.contains("<Code>ValidationError</Code>"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }
}

/// An instance protected from scale-in is not terminated when the group scales
/// in — that is what the protection means.
#[tokio::test]
async fn scale_in_leaves_protected_instances_running() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    // (case, NewInstancesProtectedFromScaleIn, the instances left after scaling in)
    for (index, (case, protected, left)) in [
        ("unprotected instances are terminated", "false", 0),
        ("protected instances stay running", "true", 2),
    ]
    .iter()
    .enumerate()
    {
        let name = format!("protect-{index}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "4"),
                ("DesiredCapacity", "2"),
                ("AvailabilityZones.member.1", "us-west-2a"),
                ("NewInstancesProtectedFromScaleIn", protected),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: create got {status}: {body}");
        wait_for_the_lifecycle_transition();

        let (status, body) = query(
            &router,
            "SetDesiredCapacity",
            &[
                ("AutoScalingGroupName", name.as_str()),
                ("DesiredCapacity", "0"),
            ],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: scale in got {status}: {body}");
        wait_for_the_lifecycle_transition();

        let (_, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            tag_value(&body, "DesiredCapacity"),
            "0",
            "case {case:?}: the desired capacity is what was asked for, got {body}"
        );
        assert_eq!(
            tag_values(&body, "InstanceId").len(),
            *left,
            "case {case:?}: expected {left} instances left, got {body}"
        );
    }
}

/// One captured `tracing` event: its target and its fields as text.
#[derive(Clone, Debug)]
struct CapturedEvent {
    target: String,
    fields: std::collections::BTreeMap<String, String>,
}

/// Collects an event's fields. A `&str` field arrives through `record_str`; a
/// `%`-formatted one (`region`, `access_key_id`) through `record_debug` over
/// `format_args!`, which renders as the plain value.
#[derive(Default)]
struct FieldCollector(std::collections::BTreeMap<String, String>);

impl tracing::field::Visit for FieldCollector {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_string(), format!("{value:?}"));
    }
}

/// A subscriber that keeps every event it is handed, so this suite can assert on
/// the audit records the service emits. Hand-written over `tracing` itself, which
/// this crate already depends on, so the test adds no dependency (and so landing
/// it does not have to touch `Cargo.lock`).
struct CapturingSubscriber {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl tracing::Subscriber for CapturingSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = FieldCollector::default();
        event.record(&mut fields);
        self.events
            .lock()
            .expect("the captured events")
            .push(CapturedEvent {
                target: event.metadata().target().to_string(),
                fields: fields.0,
            });
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The sink the capturing subscriber installed by [`install_the_audit_capture`]
/// writes into: every audit record this test binary emits, from any suite.
static CAPTURED_EVENTS: std::sync::OnceLock<Arc<Mutex<Vec<CapturedEvent>>>> =
    std::sync::OnceLock::new();

/// Install the capturing subscriber as this process's default, once. Suites
/// running in parallel emit into the same sink, which is why the assertions below
/// match each record by the resource it names.
fn install_the_audit_capture() -> Arc<Mutex<Vec<CapturedEvent>>> {
    let events = Arc::clone(CAPTURED_EVENTS.get_or_init(|| Arc::new(Mutex::new(Vec::new()))));
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let subscriber = CapturingSubscriber {
            events: Arc::clone(&events),
        };
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
    events
}

/// The audit records captured so far.
fn captured_audit_records() -> Vec<CapturedEvent> {
    let events = install_the_audit_capture();
    let events = events.lock().expect("the captured events");
    events
        .iter()
        .filter(|event| event.target == autoscaling::state::AUDIT_TARGET)
        .cloned()
        .collect()
}

/// Every state-changing operation emits the structured audit record its outcome
/// owes — the operation, the resource, the outcome, the region and the access key
/// id the caller presented — and no secret alongside it.
#[tokio::test]
async fn state_changing_operations_emit_an_audit_record() {
    install_the_audit_capture();
    let engine = new_engine();
    let router = router_with(engine.clone());

    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-audit"),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", "2"),
            ("DesiredCapacity", "1"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateAutoScalingGroup: got {status}: {body}");
    let (status, body) = query(
        &router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", "asg-audit"),
            ("DesiredCapacity", "2"),
        ],
    )
    .await;
    assert_eq!(status, 200, "SetDesiredCapacity: got {status}: {body}");
    // A refused destructive call is recorded too, with its outcome.
    let (status, body) = query(
        &router,
        "DeleteAutoScalingGroup",
        &[("AutoScalingGroupName", "asg-audit")],
    )
    .await;
    assert_eq!(status, 400, "DeleteAutoScalingGroup: got {status}: {body}");
    let (status, body) = query(
        &router,
        "DeleteAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-audit"),
            ("ForceDelete", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteAutoScalingGroup --force-delete: got {status}: {body}"
    );

    let records = captured_audit_records();

    // (case, the operation, the resource, the outcome)
    for (case, operation, resource, outcome) in [
        ("the group create", "CreateAutoScalingGroup", "asg-audit", "created"),
        ("the capacity change", "SetDesiredCapacity", "asg-audit", "capacity set"),
        (
            "the refused delete",
            "DeleteAutoScalingGroup",
            "asg-audit",
            "refused: the group is still in use",
        ),
        ("the forced delete", "DeleteAutoScalingGroup", "asg-audit", "deleted"),
    ] {
        let matching: Vec<&CapturedEvent> = records
            .iter()
            .filter(|record| {
                record.fields.get("operation").map(String::as_str) == Some(operation)
                    && record.fields.get("resource").map(String::as_str) == Some(resource)
                    && record.fields.get("outcome").map(String::as_str) == Some(outcome)
            })
            .collect();
        assert!(
            !matching.is_empty(),
            "case {case:?}: no audit record for {operation} on {resource} with outcome \
             {outcome:?}, got {records:?}"
        );
        for record in matching {
            assert_eq!(
                record.fields.get("access_key_id").map(String::as_str),
                Some("AKIDEXAMPLE"),
                "case {case:?}: the audit record must name the presented credential, got {record:?}"
            );
            assert_eq!(
                record.fields.get("region").map(String::as_str),
                Some(REGION),
                "case {case:?}: the audit record must name the region, got {record:?}"
            );
        }
    }
    assert!(
        !records.iter().any(|record| {
            record
                .fields
                .values()
                .any(|value| value.contains("Signature") || value.contains("AWS4-HMAC"))
        }),
        "an audit record must carry no signature, got {records:?}"
    );
}

/// The instances a group holds — including the ones still terminating, which stay
/// stored until the clock retires them — never exceed the per-group ceiling, so a
/// caller cycling the capacity faster than the transition comes due cannot stack
/// one ceiling's worth of instances on top of another. A change that does not fit
/// beside them is the modelled `ScalingActivityInProgressFault`.
#[tokio::test]
async fn instances_still_terminating_occupy_the_group_ceiling() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    let ceiling = autoscaling::state::MAX_GROUP_CAPACITY;
    let full = ceiling.to_string();
    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-full"),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", full.as_str()),
            ("DesiredCapacity", full.as_str()),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(status, 200, "a group filled to the ceiling: got {status}: {body}");
    wait_for_the_lifecycle_transition();

    // Scale all the way in: every instance is now Terminating, and still stored.
    let (status, body) = query(
        &router,
        "SetDesiredCapacity",
        &[("AutoScalingGroupName", "asg-full"), ("DesiredCapacity", "0")],
    )
    .await;
    assert_eq!(status, 200, "scaling in: got {status}: {body}");
    {
        let state = autoscaling::state::service_state(&*engine).expect("the service store");
        let state = state.lock();
        let group = state.groups.get("asg-full").expect("the group");
        assert_eq!(
            group.instances.len(),
            usize::try_from(ceiling).expect("the ceiling fits"),
            "the terminating instances are still stored"
        );
        assert_eq!(
            group.launch_headroom(),
            0,
            "a group holding a ceiling's worth of instances has no launch headroom"
        );
    }

    // Asking for them all back while they are still terminating would double the
    // stored instances, so it is refused rather than allocated.
    for (case, action, params) in [
        (
            "a capacity change",
            "SetDesiredCapacity",
            vec![
                ("AutoScalingGroupName", "asg-full"),
                ("DesiredCapacity", full.as_str()),
            ],
        ),
        (
            "an update",
            "UpdateAutoScalingGroup",
            vec![
                ("AutoScalingGroupName", "asg-full"),
                ("DesiredCapacity", full.as_str()),
            ],
        ),
    ] {
        let (status, body) = query(&router, action, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the change to be refused, got {status}: {body}"
        );
        assert!(
            body.contains("<Code>ScalingActivityInProgress</Code>"),
            "case {case:?}: expected the ScalingActivityInProgress code, got {body}"
        );
    }
    {
        let state = autoscaling::state::service_state(&*engine).expect("the service store");
        let state = state.lock();
        let group = state.groups.get("asg-full").expect("the group");
        assert!(
            group.instances.len() <= usize::try_from(ceiling).expect("the ceiling fits"),
            "the stored instances must never exceed the ceiling, got {}",
            group.instances.len()
        );
    }

    // Once the terminations are due the group is empty and has its headroom back.
    wait_for_the_lifecycle_transition();
    let (status, body) = query(
        &router,
        "SetDesiredCapacity",
        &[
            ("AutoScalingGroupName", "asg-full"),
            ("DesiredCapacity", full.as_str()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "once the terminations are due the capacity can be raised again: got {status}: {body}"
    );
    {
        let state = autoscaling::state::service_state(&*engine).expect("the service store");
        let state = state.lock();
        let group = state.groups.get("asg-full").expect("the group");
        assert_eq!(
            group.instances.len(),
            usize::try_from(ceiling).expect("the ceiling fits"),
            "the group holds exactly its capacity again"
        );
    }
}

/// A member whose value set the model documents is refused outside that set,
/// rather than stored and echoed back.
#[tokio::test]
async fn members_outside_their_documented_value_sets_are_refused() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    // (case, the member, the value, accepted) — the accepted rows are drawn from
    // the model's own value lists, including entries the issue never cited.
    let cases = [
        ("a health check type the model lists", "HealthCheckType", "EBS", true),
        ("another one the model lists", "HealthCheckType", "VPC_LATTICE", true),
        (
            "the comma-separated form the model documents",
            "HealthCheckType",
            "EC2,ELB",
            true,
        ),
        ("a health check type nothing lists", "HealthCheckType", "NOPE", false),
        (
            "a termination policy the model lists",
            "TerminationPolicies.member.1",
            "ClosestToNextInstanceHour",
            true,
        ),
        (
            "the Lambda form the model documents",
            "TerminationPolicies.member.1",
            "arn:aws:lambda:us-west-2:000000000000:function:my-function:my-alias",
            true,
        ),
        (
            "a termination policy nothing lists",
            "TerminationPolicies.member.1",
            "Oldest",
            false,
        ),
        ("a capacity unit the model lists", "DesiredCapacityType", "vcpu", true),
        ("another unit the model lists", "DesiredCapacityType", "memory-mib", true),
        ("a capacity unit nothing lists", "DesiredCapacityType", "gigabytes", false),
    ];

    for (index, (case, member, value, accepted)) in cases.iter().enumerate() {
        let name = format!("values-{index}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "0"),
                ("AvailabilityZones.member.1", "us-west-2a"),
                (member, value),
            ],
        )
        .await;
        let expected = if *accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {case:?} ({member}={value:?}): expected {expected}, got {status}: {body}"
        );
        if !*accepted {
            assert!(
                body.contains("<Code>ValidationError</Code>"),
                "case {case:?}: expected ValidationError, got {body}"
            );
        }

        // The same value set holds on the update path.
        let (status, body) = query(
            &router,
            "UpdateAutoScalingGroup",
            &[("AutoScalingGroupName", "values-0"), (member, value)],
        )
        .await;
        assert_eq!(
            status, expected,
            "UpdateAutoScalingGroup {case:?}: expected {expected}, got {status}: {body}"
        );
    }
}
