// SPDX-License-Identifier: CC0-1.0

//! `SetDesiredCapacity` on a group whose placement is given ONLY through
//! `VPCZoneIdentifier`.
//!
//! `SetDesiredCapacity` "sets the size of the specified Auto Scaling group"
//! (`smithy_query(query="doc",shape="SetDesiredCapacity")`) and the group then launches or
//! terminates instances to reach it; its output shape is `Unit` and the only errors it
//! models are `ScalingActivityInProgressFault` and `ResourceContentionFault`
//! (`smithy_query(query="operation",operation="SetDesiredCapacity")`). Where those
//! instances go is the group's placement, and in `CreateAutoScalingGroupType` both
//! `AvailabilityZones` and `VPCZoneIdentifier` are OPTIONAL: a group that names only
//! subnets is placed in the zones of those subnets, which is a fact of the account's own
//! Amazon EC2 subnets rather than a value the Auto Scaling caller has to supply. So a
//! capacity change on such a group is served exactly like one on a group that named its
//! zones — it is never answered with an error outside the model.
//!
//! The table below spans what that generalises over: how many subnets the group names, how
//! many distinct zones they are in, the order and the spacing they are named in, and the
//! capacity trajectory the group is driven along (up from nothing, part way back in, all
//! the way in, and to the capacity it already holds). The capacity each row asks for is
//! read back through `DescribeAutoScalingGroups`, so what is asserted is the state the
//! change left behind, not the status of one call.
//!
//! Each row is driven over the awsQuery wire through one router over one shared engine, the
//! way the emulator serves it.

use std::time::Duration;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;

use service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::support::{
    new_engine, query, router_with, seed_subnets, tag_value, tag_values, REGION,
};

/// The launch source every group here is created from: a launch TEMPLATE, because no
/// account can create a launch configuration (`CreateLaunchConfiguration` is refused).
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// The `MaxSize` every group here is created with: above every capacity the table asks
/// for, so no row is refused for a reason other than the one it is testing.
const MAX_SIZE: &str = "6";

/// Wait past the lifecycle transition the service owes, so what the clock made due is the
/// state the next read reports.
fn wait_for_the_lifecycle_transition() {
    std::thread::sleep(autoscaling::state::LIFECYCLE_TRANSITION + Duration::from_millis(150));
}

/// One row of the capacity table: the subnets the group names (by index into the seeded
/// subnets, with the spacing the caller wrote), the zones those subnets resolve to, and
/// the two capacities the group is driven to.
struct CapacityCase {
    case: &'static str,
    /// Indexes into the seeded subnets, in the order `VPCZoneIdentifier` names them.
    subnets: Vec<usize>,
    /// Rendered between the ids, so a row can carry the spacing a caller wrote.
    separator: &'static str,
    /// The indexes whose zones the group places instances in, deduplicated, in order.
    zones_of: Vec<usize>,
    /// The capacity the group is scaled out to from nothing.
    up: usize,
    /// The capacity it is then scaled back to (equal to `up` for a row that asks for the
    /// capacity the group already holds).
    down: usize,
}

/// The instance states a body reports, in document order.
fn lifecycle_states(body: &str) -> Vec<String> {
    tag_values(body, "LifecycleState")
}

/// A `SetDesiredCapacity` answer carries the operation's own success envelope and nothing
/// about the instances it moved: the modelled output is `Unit`.
fn assert_served(case: &str, status: u16, body: &str) {
    assert_eq!(status, 200, "case {case:?}: expected 200, got {status}: {body}");
    assert!(
        body.contains("<SetDesiredCapacityResponse"),
        "case {case:?}: expected the operation's own response envelope, got {body}"
    );
    assert!(
        !body.contains("<ErrorResponse"),
        "case {case:?}: a served capacity change carries no error envelope, got {body}"
    );
    assert_eq!(
        tag_values(body, "RequestId").len(),
        1,
        "case {case:?}: expected exactly one RequestId, got {body}"
    );
    assert!(
        tag_values(body, "InstanceId").is_empty(),
        "case {case:?}: the modelled output is Unit, so it reports no instance, got {body}"
    );
}

/// A capacity change on a group placed only by subnet launches the instances it asks for,
/// in the zones those subnets are in, and scales them back in again.
#[tokio::test]
async fn set_desired_capacity_scales_a_group_placed_only_by_subnet() {
    let engine = new_engine();
    let router = router_with(engine.clone());
    // One more subnet than the Region has zones, so the last seeded subnet shares a zone
    // with the first: that is what the two-subnets-one-zone row names.
    let zone_count = ec2::availability_zones_in_region(REGION).len();
    let subnets = seed_subnets(&engine, zone_count + 1);

    let cases = [
        CapacityCase {
            case: "one subnet: out to one instance and all the way back in",
            subnets: vec![0],
            separator: ",",
            zones_of: vec![0],
            up: 1,
            down: 0,
        },
        CapacityCase {
            case: "two subnets in two zones: out to two, part way back in",
            subnets: vec![0, 1],
            separator: ",",
            zones_of: vec![0, 1],
            up: 2,
            down: 1,
        },
        CapacityCase {
            case: "two subnets in ONE zone: every instance in that one zone",
            subnets: vec![0, zone_count],
            separator: ",",
            zones_of: vec![0],
            up: 3,
            down: 0,
        },
        CapacityCase {
            case: "every zone of the Region, one more instance than zones",
            subnets: (0..zone_count).collect(),
            separator: ",",
            zones_of: (0..zone_count).collect(),
            up: zone_count + 1,
            down: 2,
        },
        CapacityCase {
            case: "spaces around the commas: the ids are still the subnets named",
            subnets: vec![0, 1],
            separator: " , ",
            zones_of: vec![0, 1],
            up: 2,
            down: 0,
        },
        CapacityCase {
            case: "the subnets named in the other order, then the capacity it already holds",
            subnets: vec![1, 0],
            separator: ",",
            zones_of: vec![1, 0],
            up: 2,
            down: 2,
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let name = format!("subnet-placed-{index}");
        let vpc_zone_identifier = case
            .subnets
            .iter()
            .map(|subnet| subnets[*subnet].id.as_str())
            .collect::<Vec<_>>()
            .join(case.separator);
        let expected_zones: Vec<String> = case
            .zones_of
            .iter()
            .map(|subnet| subnets[*subnet].zone.clone())
            .collect();

        // The group of the reported scenario: no AvailabilityZones at all, and no
        // capacity yet, so every instance below is one this operation launched.
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", MAX_SIZE),
                ("DesiredCapacity", "0"),
                ("VPCZoneIdentifier", vpc_zone_identifier.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateAutoScalingGroup expected 200, got {status}: {body}",
            case.case
        );
        let (_, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert!(
            tag_values(&body, "InstanceId").is_empty(),
            "case {:?}: the group starts with no instance, got {body}",
            case.case
        );

        // Scale out.
        let up = case.up.to_string();
        let (status, body) = query(
            &router,
            "SetDesiredCapacity",
            &[
                ("AutoScalingGroupName", name.as_str()),
                ("DesiredCapacity", up.as_str()),
            ],
        )
        .await;
        assert_served(case.case, status, &body);

        let (_, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            tag_value(&body, "DesiredCapacity"),
            up,
            "case {:?}: the group holds the capacity the request set, got {body}",
            case.case
        );
        let launched = tag_values(&body, "InstanceId");
        assert_eq!(
            launched.len(),
            case.up,
            "case {:?}: expected {} launched instances, got {launched:?} from {body}",
            case.case,
            case.up
        );
        // The zones the group reports are still the zones of the subnets it named, and
        // every instance is in one of them.
        assert_eq!(
            tag_value(&body, "AvailabilityZones"),
            expected_zones
                .iter()
                .map(|zone| format!("<member>{zone}</member>"))
                .collect::<String>(),
            "case {:?}: expected the zones of the named subnets {expected_zones:?}, got {body}",
            case.case
        );
        let instance_zones = tag_values(&body, "AvailabilityZone");
        for zone in &instance_zones {
            assert!(
                expected_zones.contains(zone),
                "case {:?}: an instance in {zone}, which is not one of {expected_zones:?}: {body}",
                case.case
            );
        }
        if case.up >= expected_zones.len() {
            // Enough instances to reach every zone: the launch spreads over them rather
            // than piling into the first.
            for zone in &expected_zones {
                assert!(
                    instance_zones.contains(zone),
                    "case {:?}: no instance in {zone}, though {} instances were launched \
                     over {expected_zones:?}: {body}",
                    case.case,
                    case.up
                );
            }
        }
        assert_eq!(
            lifecycle_states(&body),
            vec!["Pending".to_string(); case.up],
            "case {:?}: a freshly launched instance is reported Pending, got {body}",
            case.case
        );

        wait_for_the_lifecycle_transition();
        let (_, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            lifecycle_states(&body),
            vec!["InService".to_string(); case.up],
            "case {:?}: once the transition is due every instance is InService, got {body}",
            case.case
        );
        assert_eq!(
            tag_values(&body, "InstanceId"),
            launched,
            "case {:?}: the transition keeps the same instances, got {body}",
            case.case
        );

        // Scale (back) in. The surplus is reported Terminating while it leaves, and is
        // gone once that transition is due.
        let down = case.down.to_string();
        let (status, body) = query(
            &router,
            "SetDesiredCapacity",
            &[
                ("AutoScalingGroupName", name.as_str()),
                ("DesiredCapacity", down.as_str()),
            ],
        )
        .await;
        assert_served(case.case, status, &body);

        let (_, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            tag_value(&body, "DesiredCapacity"),
            down,
            "case {:?}: the group holds the capacity the second request set, got {body}",
            case.case
        );
        let states = lifecycle_states(&body);
        assert_eq!(
            states.iter().filter(|state| *state == "Terminating").count(),
            case.up - case.down,
            "case {:?}: expected the {} surplus instances Terminating, got {body}",
            case.case,
            case.up - case.down
        );
        assert_eq!(
            states
                .iter()
                .filter(|state| *state != "Terminating")
                .count(),
            case.down,
            "case {:?}: expected {} instances left in the group, got {body}",
            case.case,
            case.down
        );

        wait_for_the_lifecycle_transition();
        let (_, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            tag_values(&body, "InstanceId").len(),
            case.down,
            "case {:?}: once the termination is due only the {} remaining instances are \
             reported, got {body}",
            case.case,
            case.down
        );
        assert_eq!(
            lifecycle_states(&body),
            vec!["InService".to_string(); case.down],
            "case {:?}: what stays is in service, got {body}",
            case.case
        );
        // Each launch and each termination recorded its own scaling activity against the
        // group, so the change is visible as work the group did and not only as a number.
        let activities = {
            let state = autoscaling::state::service_state(&*engine).expect("the service store");
            let state = state.lock();
            state
                .activities
                .iter()
                .filter(|activity| activity.auto_scaling_group_name == name)
                .count()
        };
        assert_eq!(
            activities,
            case.up + (case.up - case.down),
            "case {:?}: expected one activity per launch and per termination, got {activities}",
            case.case
        );
    }
}

/// One row of the refusal table: the capacity asked for, what makes it unacceptable, and
/// the message the service answers — its fixed text, with the values taken from the
/// request and from the group the account holds.
struct RefusalCase {
    case: &'static str,
    group: &'static str,
    desired_capacity: &'static str,
    message: String,
}

/// A capacity change a subnet-placed group cannot serve is refused the way the service
/// refuses it for any group — a 400 `ValidationError` naming the bound — never with a
/// status or code outside what this operation models.
#[tokio::test]
async fn an_unacceptable_capacity_is_refused_within_the_model() {
    let engine = new_engine();
    let router = router_with(engine.clone());
    let subnets = seed_subnets(&engine, 2);
    let identifier = format!("{},{}", subnets[0].id, subnets[1].id);

    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "bounded"),
            LAUNCH_SOURCE,
            ("MinSize", "1"),
            ("MaxSize", "2"),
            ("DesiredCapacity", "1"),
            ("VPCZoneIdentifier", identifier.as_str()),
        ],
    )
    .await;
    assert_eq!(status, 200, "the bounded group: got {status}: {body}");

    let cases = [
        RefusalCase {
            case: "above the group's maximum",
            group: "bounded",
            desired_capacity: "3",
            message: "New SetDesiredCapacity value 3 is above max value 2 for the \
                      AutoScalingGroup: bounded"
                .to_string(),
        },
        RefusalCase {
            case: "below the group's minimum",
            group: "bounded",
            desired_capacity: "0",
            message: "New SetDesiredCapacity value 0 is below min value 1 for the \
                      AutoScalingGroup: bounded"
                .to_string(),
        },
        RefusalCase {
            case: "a group the account does not hold",
            group: "no-such-group",
            desired_capacity: "1",
            message: "AutoScalingGroup name not found - null".to_string(),
        },
    ];

    for case in &cases {
        let (status, body) = query(
            &router,
            "SetDesiredCapacity",
            &[
                ("AutoScalingGroupName", case.group),
                ("DesiredCapacity", case.desired_capacity),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status}: {body}",
            case.case
        );
        assert!(
            body.contains("<Code>ValidationError</Code>"),
            "case {:?}: expected the ValidationError code, got {body}",
            case.case
        );
        assert!(
            body.contains(&case.message),
            "case {:?}: expected the message {:?}, got {body}",
            case.case,
            case.message
        );

        // The refused change left the group's capacity alone.
        let (_, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", "bounded")],
        )
        .await;
        assert_eq!(
            tag_value(&body, "DesiredCapacity"),
            "1",
            "case {:?}: a refused capacity change leaves the group as it was, got {body}",
            case.case
        );
    }
}
