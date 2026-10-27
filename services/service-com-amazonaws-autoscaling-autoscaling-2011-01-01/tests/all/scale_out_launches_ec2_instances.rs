// SPDX-License-Identifier: CC0-1.0

//! The instances an Auto Scaling group reports are the account's own Amazon EC2
//! instances.
//!
//! A capacity change on a group STARTS A SCALING ACTIVITY THAT LAUNCHES INSTANCES: the
//! ids the group then reports under `AutoScalingGroups[].Instances[].InstanceId` name
//! instances of the account, so Amazon EC2's own `DescribeInstances` resolves every one
//! of them — running, from the launch source's image and instance type, in a subnet of
//! the group's `VPCZoneIdentifier` and carrying the group's propagate-at-launch tags —
//! and a scale-in moves those same ids to `terminated`. An id that resolved in no EC2
//! state would be an id no client could inspect, tag, connect to or terminate.
//!
//! The table below spans what that rule generalises over: the launch source (a launch
//! template named by name, by id, or through a mixed instances policy whose override
//! names the instance type), how many placements the group has (one, two or three
//! subnets, or zones with no subnet at all), how much capacity is asked for, whether it
//! is asked for by the create or by a later `SetDesiredCapacity`, and how many of the
//! group's tags propagate at launch.
//!
//! Both services are mounted on ONE engine and driven over their own wire protocols —
//! awsQuery for Auto Scaling, ec2Query for Amazon EC2 — the way the emulator serves
//! them, so the ids these assertions follow are the ids a client is given.

use std::sync::Arc;
use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;
use service_com_amazonaws_ec2_amazonec2::state as ec2;

use crate::support::{form_encode, query, seed_subnets, tag_value, tag_values, REGION};

/// The ec2Query API version a client sends in an Amazon EC2 request body.
const EC2_API_VERSION: &str = "2016-11-15";

/// Wait until every clock-owed transition started before now has certainly fallen due —
/// this service's own lifecycle span and Amazon EC2's, whichever is longer, plus a
/// margin. Derived from the services' own spans, not written out, so the three cannot
/// drift apart.
async fn past_the_transitions() {
    let span = autoscaling::state::LIFECYCLE_TRANSITION.max(ec2::TRANSITION_DELAY);
    tokio::time::sleep(span + Duration::from_millis(250)).await;
}

/// One emulator with BOTH services on one engine: an Auto Scaling group's instances
/// live in the same account's Amazon EC2 store, so one engine is what makes the two
/// views one account.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// POST one ec2Query action, signed for Amazon EC2 in [`REGION`], and answer its status
/// and body.
async fn ec2_query(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={EC2_API_VERSION}");
    for (key, value) in params {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/ec2/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// One `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// What Amazon EC2 reports for one instance, read out of a `DescribeInstances` answer
/// that named exactly that instance.
#[derive(Debug, Clone, Default)]
struct DescribedInstance {
    instance_id: String,
    state: String,
    image_id: String,
    instance_type: String,
    subnet_id: String,
    availability_zone: String,
    /// The instance's tags, as `key=value` pairs in the order reported.
    tags: Vec<String>,
}

/// Ask Amazon EC2 about ONE instance id, so every member of the answer belongs to that
/// instance. Answers `None` when EC2 refuses the id — which is what an id that resolves
/// in no EC2 state does.
async fn described_instance(
    router: &MultiServiceRouter,
    instance_id: &str,
) -> Option<DescribedInstance> {
    let (status, body) = ec2_query(router, "DescribeInstances", &[p("InstanceId.1", instance_id)]).await;
    if status != 200 {
        return None;
    }
    let keys = tag_values(&body, "key");
    let values = tag_values(&body, "value");
    Some(DescribedInstance {
        instance_id: tag_value(&body, "instanceId"),
        state: tag_value(&tag_value(&body, "instanceState"), "name"),
        image_id: tag_value(&body, "imageId"),
        instance_type: tag_value(&body, "instanceType"),
        subnet_id: tag_value(&body, "subnetId"),
        availability_zone: tag_value(&tag_value(&body, "placement"), "availabilityZone"),
        tags: keys
            .iter()
            .zip(values.iter())
            .map(|(key, value)| format!("{key}={value}"))
            .collect(),
    })
}

/// Create one Amazon EC2 launch template over the wire and answer its id.
async fn create_launch_template(
    router: &MultiServiceRouter,
    name: &str,
    image_id: &str,
    instance_type: &str,
) -> String {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", name),
            p("LaunchTemplateData.ImageId", image_id),
            p("LaunchTemplateData.InstanceType", instance_type),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate for {name} expected 200, got {status}: {body}"
    );
    let id = tag_value(&body, "launchTemplateId");
    assert!(
        !id.is_empty(),
        "CreateLaunchTemplate must report the template's id, got {body}"
    );
    id
}

/// How the group names its launch source.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// `LaunchTemplate` naming the template by name.
    TemplateByName,
    /// `LaunchTemplate` naming the template by id.
    TemplateById,
    /// `MixedInstancesPolicy` whose launch template is the one created, with one
    /// override naming the instance type to launch.
    MixedPolicyOverriding(&'static str),
}

/// How the capacity is asked for.
#[derive(Debug, Clone, Copy)]
enum AskedBy {
    /// `CreateAutoScalingGroup`'s own `DesiredCapacity`.
    Create,
    /// A later `SetDesiredCapacity`.
    SetDesiredCapacity,
}

/// One row of the launch table.
struct Row {
    case: &'static str,
    /// How many subnets the group's `VPCZoneIdentifier` names; 0 names zones instead.
    subnets: usize,
    source: Source,
    image_id: &'static str,
    /// The instance type the launch template carries.
    instance_type: &'static str,
    /// The instance type the group's instances must run, which an override replaces.
    expected_instance_type: &'static str,
    capacity: i32,
    asked_by: AskedBy,
    /// The group's tags, as `(key, value, propagate at launch)`.
    tags: &'static [(&'static str, &'static str, bool)],
}

/// Every instance id an Auto Scaling group reports resolves in the account's Amazon EC2
/// state, with the launch source's parameters and a placement of the group — and a
/// scale-in terminates those same instances.
#[tokio::test]
async fn reported_instances_are_the_accounts_own_ec2_instances() {
    let rows = [
        Row {
            case: "one subnet, template by name, capacity asked for by the create",
            subnets: 1,
            source: Source::TemplateByName,
            image_id: "ami-00000000000000011",
            instance_type: "t3.micro",
            expected_instance_type: "t3.micro",
            capacity: 1,
            asked_by: AskedBy::Create,
            tags: &[],
        },
        Row {
            case: "two subnets, template by id, capacity set after the create, two tags",
            subnets: 2,
            source: Source::TemplateById,
            image_id: "ami-00000000000000012",
            instance_type: "t3.small",
            expected_instance_type: "t3.small",
            capacity: 2,
            asked_by: AskedBy::SetDesiredCapacity,
            tags: &[("env", "prod", true), ("team", "core", false)],
        },
        Row {
            case: "three subnets, a mixed instances policy overriding the instance type",
            subnets: 3,
            source: Source::MixedPolicyOverriding("m5.large"),
            image_id: "ami-00000000000000013",
            instance_type: "t3.micro",
            expected_instance_type: "m5.large",
            capacity: 3,
            asked_by: AskedBy::SetDesiredCapacity,
            tags: &[("Name", "web", true)],
        },
        Row {
            case: "no subnets: a group placed by Availability Zone alone",
            subnets: 0,
            source: Source::TemplateByName,
            image_id: "ami-00000000000000014",
            instance_type: "c5.xlarge",
            expected_instance_type: "c5.xlarge",
            capacity: 2,
            asked_by: AskedBy::Create,
            tags: &[("stage", "", true)],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One emulator per row: each row is its own account, so the ids and addresses
        // of one row never explain another's.
        let (router, engine) = emulator();
        let subnets = seed_subnets(&engine, row.subnets);
        let template_name = format!("lt-{index}");
        let template_id =
            create_launch_template(&router, &template_name, row.image_id, row.instance_type).await;

        let name = format!("asg-{index}");
        let capacity = row.capacity.to_string();
        let identifier = subnets
            .iter()
            .map(|subnet| subnet.id.as_str())
            .collect::<Vec<_>>()
            .join(",");
        // The zone a subnetless group is placed by: the first of the Region's own
        // catalog, never written out here.
        let fallback_zone = ec2::availability_zones_in_region(REGION)
            .first()
            .map(|zone| zone.zone_name.clone())
            .expect("the Region has Availability Zones");

        let mut params: Vec<(String, String)> = vec![
            p("AutoScalingGroupName", name.clone()),
            p("MinSize", "0"),
            p("MaxSize", row.capacity.to_string()),
        ];
        match row.source {
            Source::TemplateByName => {
                params.push(p("LaunchTemplate.LaunchTemplateName", template_name.clone()));
            }
            Source::TemplateById => {
                params.push(p("LaunchTemplate.LaunchTemplateId", template_id.clone()));
            }
            Source::MixedPolicyOverriding(instance_type) => {
                params.push(p(
                    "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification.LaunchTemplateName",
                    template_name.clone(),
                ));
                params.push(p(
                    "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.InstanceType",
                    instance_type,
                ));
            }
        }
        if row.subnets > 0 {
            params.push(p("VPCZoneIdentifier", identifier.clone()));
        } else {
            params.push(p("AvailabilityZones.member.1", fallback_zone.clone()));
        }
        for (position, (key, value, propagate)) in row.tags.iter().enumerate() {
            let member = position + 1;
            params.push(p(&format!("Tags.member.{member}.Key"), *key));
            params.push(p(&format!("Tags.member.{member}.Value"), *value));
            params.push(p(
                &format!("Tags.member.{member}.PropagateAtLaunch"),
                if *propagate { "true" } else { "false" },
            ));
        }
        if matches!(row.asked_by, AskedBy::Create) {
            params.push(p("DesiredCapacity", capacity.clone()));
        }

        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let (status, body) = query(&router, "CreateAutoScalingGroup", &borrowed).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateAutoScalingGroup expected 200, got {status}: {body}",
            row.case
        );

        if matches!(row.asked_by, AskedBy::SetDesiredCapacity) {
            let (status, body) = query(
                &router,
                "SetDesiredCapacity",
                &[
                    ("AutoScalingGroupName", name.as_str()),
                    ("DesiredCapacity", capacity.as_str()),
                ],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: SetDesiredCapacity expected 200, got {status}: {body}",
                row.case
            );
        }

        past_the_transitions().await;
        let (status, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeAutoScalingGroups expected 200, got {status}: {body}",
            row.case
        );
        let reported = tag_values(&body, "InstanceId");
        assert_eq!(
            reported.len(),
            usize::try_from(row.capacity).expect("a capacity is a count"),
            "case {:?}: expected {} instances, got {reported:?} in {body}",
            row.case,
            row.capacity
        );

        // Every id the group reported is an instance of the account, running what the
        // group's launch source says and placed where the group places.
        let expected_zones: Vec<String> = if row.subnets > 0 {
            subnets.iter().map(|subnet| subnet.zone.clone()).collect()
        } else {
            vec![fallback_zone.clone()]
        };
        let expected_subnets: Vec<String> = subnets.iter().map(|subnet| subnet.id.clone()).collect();
        for instance_id in &reported {
            let described = described_instance(&router, instance_id)
                .await
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the group reports {instance_id}, which Amazon EC2 \
                         resolves to no instance",
                        row.case
                    )
                });
            assert_eq!(
                described.instance_id, *instance_id,
                "case {:?}: expected Amazon EC2 to report {instance_id}, got {described:?}",
                row.case
            );
            assert_eq!(
                described.state, "running",
                "case {:?}: expected {instance_id} running once the transition is due, \
                 got {described:?}",
                row.case
            );
            assert_eq!(
                described.image_id, row.image_id,
                "case {:?}: expected the launch source's image {}, got {described:?}",
                row.case, row.image_id
            );
            assert_eq!(
                described.instance_type, row.expected_instance_type,
                "case {:?}: expected the launch source's instance type {}, got {described:?}",
                row.case, row.expected_instance_type
            );
            assert!(
                expected_zones.contains(&described.availability_zone),
                "case {:?}: expected a zone of {expected_zones:?}, got {described:?}",
                row.case
            );
            if row.subnets > 0 {
                assert!(
                    expected_subnets.contains(&described.subnet_id),
                    "case {:?}: expected a subnet of the group's {expected_subnets:?}, \
                     got {described:?}",
                    row.case
                );
            } else {
                assert!(
                    described.subnet_id.is_empty(),
                    "case {:?}: a group that named no subnet places no instance in one, \
                     got {described:?}",
                    row.case
                );
            }
            // The tags marked PropagateAtLaunch are on the instance; the others are not.
            // Beside them, the group's own name, which Amazon EC2 Auto Scaling tags
            // every instance it launches with.
            for (key, value, propagate) in row.tags {
                let pair = format!("{key}={value}");
                assert_eq!(
                    described.tags.contains(&pair),
                    *propagate,
                    "case {:?}: the tag {pair:?} propagates at launch: {propagate}, \
                     got {described:?}",
                    row.case
                );
            }
            assert!(
                described.tags.contains(&format!(
                    "{}={name}",
                    autoscaling::state::ec2_instances::GROUP_NAME_TAG
                )),
                "case {:?}: an instance of the group carries the group's name, got {described:?}",
                row.case
            );
        }

        // Scaling back in terminates THOSE instances: the same ids, in Amazon EC2.
        let (status, body) = query(
            &router,
            "SetDesiredCapacity",
            &[
                ("AutoScalingGroupName", name.as_str()),
                ("DesiredCapacity", "0"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: scaling in expected 200, got {status}: {body}",
            row.case
        );
        past_the_transitions().await;
        for instance_id in &reported {
            let described = described_instance(&router, instance_id)
                .await
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: a terminated instance is still reported by Amazon EC2, \
                         but {instance_id} resolves to none",
                        row.case
                    )
                });
            assert_eq!(
                described.state, "terminated",
                "case {:?}: expected {instance_id} terminated after the scale-in, \
                 got {described:?}",
                row.case
            );
        }
    }
}

/// A forced delete takes the group's instances with it: "the group is to be deleted
/// along with all instances associated with the group"
/// (`smithy_query(query="doc",shape="DeleteAutoScalingGroupType",member="ForceDelete")`),
/// so the account's own Amazon EC2 instances are terminated.
#[tokio::test]
async fn a_forced_delete_terminates_the_groups_ec2_instances() {
    let (router, engine) = emulator();
    let subnets = seed_subnets(&engine, 2);
    create_launch_template(&router, "lt-force", "ami-00000000000000021", "t3.micro").await;
    let identifier = format!("{},{}", subnets[0].id, subnets[1].id);

    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "forced"),
            ("LaunchTemplate.LaunchTemplateName", "lt-force"),
            ("MinSize", "2"),
            ("MaxSize", "2"),
            ("VPCZoneIdentifier", identifier.as_str()),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateAutoScalingGroup: got {status}: {body}");

    past_the_transitions().await;
    let (_, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "forced")],
    )
    .await;
    let reported = tag_values(&body, "InstanceId");
    assert_eq!(
        reported.len(),
        2,
        "the group must hold the capacity it was created with, got {body}"
    );

    let (status, body) = query(
        &router,
        "DeleteAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "forced"),
            ("ForceDelete", "true"),
        ],
    )
    .await;
    assert_eq!(status, 200, "a forced delete: got {status}: {body}");

    past_the_transitions().await;
    for instance_id in &reported {
        let described = described_instance(&router, instance_id)
            .await
            .unwrap_or_else(|| panic!("Amazon EC2 resolves {instance_id} to no instance"));
        assert_eq!(
            described.state, "terminated",
            "a forced delete terminates {instance_id}, got {described:?}"
        );
    }
}

/// One row of the refusal table: the group's name and how much capacity it holds —
/// neither of which may reach the refusal's message.
struct RefusalCase {
    case: &'static str,
    name: &'static str,
    capacity: &'static str,
}

/// `DeleteAutoScalingGroup` without `ForceDelete` on a group that still holds instances
/// is refused with the sentence Amazon EC2 Auto Scaling states for that error class,
/// which has NO variable part: it names neither the group nor any instance of it.
#[tokio::test]
async fn the_resource_in_use_refusal_is_the_fixed_sentence() {
    const EXPECTED: &str = "You cannot delete an AutoScalingGroup while there are instances or \
                            pending Spot instance request(s) still in the group.";

    let cases = [
        RefusalCase {
            case: "one instance in the group",
            name: "in-use-one",
            capacity: "1",
        },
        RefusalCase {
            case: "three instances in the group",
            name: "in-use-three",
            capacity: "3",
        },
        RefusalCase {
            case: "a group whose name would be a tempting suffix",
            name: "still.in-the-group",
            capacity: "2",
        },
    ];

    for case in &cases {
        let (router, engine) = emulator();
        let subnets = seed_subnets(&engine, 1);
        create_launch_template(&router, "lt-in-use", "ami-00000000000000031", "t3.micro").await;

        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", case.name),
                ("LaunchTemplate.LaunchTemplateName", "lt-in-use"),
                ("MinSize", "0"),
                ("MaxSize", case.capacity),
                ("DesiredCapacity", case.capacity),
                ("VPCZoneIdentifier", subnets[0].id.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateAutoScalingGroup expected 200, got {status}: {body}",
            case.case
        );
        past_the_transitions().await;

        let (status, body) = query(
            &router,
            "DeleteAutoScalingGroup",
            &[("AutoScalingGroupName", case.name)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status}: {body}",
            case.case
        );
        assert!(
            body.contains("<Code>ResourceInUse</Code>"),
            "case {:?}: expected the ResourceInUse query error code, got {body}",
            case.case
        );
        assert_eq!(
            tag_value(&body, "Message"),
            EXPECTED,
            "case {:?}: expected the fixed ResourceInUse sentence, got {body}",
            case.case
        );
        assert!(
            !tag_value(&body, "Message").contains(case.name),
            "case {:?}: the message must not name the group, got {body}",
            case.case
        );

        // The refusal left the group and its instances alone.
        let (_, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", case.name)],
        )
        .await;
        assert_eq!(
            tag_values(&body, "InstanceId").len(),
            case.capacity
                .parse::<usize>()
                .expect("the capacity is a count"),
            "case {:?}: a refused delete leaves the group's instances alone, got {body}",
            case.case
        );
    }
}
