// SPDX-License-Identifier: CC0-1.0

//! The LAUNCH TEMPLATE SPECIFICATION a group reports, once Amazon EC2 Auto Scaling has
//! resolved the template the create named.
//!
//! RULE: "Conditional: You must specify either a `LaunchTemplateId` or a
//! `LaunchTemplateName`" — a caller states ONE of them, and the group this service stores
//! carries BOTH, because it resolves the template against the account's own Amazon EC2
//! launch templates when the group is created. So a client that created a group by id reads
//! the template's name off the group, and one that created it by name reads the id EC2
//! assigned: either value names the same template, and both are values EC2 answers a read
//! for.
//!
//! The rows span what the rule generalises over: which half the request states, where the
//! group names the template (its own `LaunchTemplate`, the launch template of a
//! `MixedInstancesPolicy`, or the one an override of that policy names), whether the id or
//! name the request states resolves at all, and whether the group was created with it or
//! updated onto it.
//!
//! Both services are mounted on ONE engine and driven over their own wire protocols —
//! awsQuery for Auto Scaling, ec2Query for Amazon EC2 — the way the emulator serves them, so
//! the templates these assertions resolve against are the ones a client created.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;

use crate::support::{borrowed, ec2_query, p, query, tag_value, tag_values};

/// The image and instance type every launch template in this suite carries.
const IMAGE_ID: &str = "ami-00000000000000031";
const INSTANCE_TYPE: &str = "t3.micro";

/// The name the account's launch template is created under.
const TEMPLATE_NAME: &str = "asg-fleet-template";

/// A launch template name no row creates, and an id of the published form no template of the
/// account carries: a specification naming either resolves to nothing.
const ABSENT_TEMPLATE_NAME: &str = "asg-fleet-template-never-created";
const ABSENT_TEMPLATE_ID: &str = "lt-00000000000000999";

/// Where the group names the launch template under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The group's own `LaunchTemplate`.
    Own,
    /// The launch template of its `MixedInstancesPolicy`.
    MixedPolicy,
    /// The template an OVERRIDE of that policy names.
    MixedPolicyOverride,
}

impl Source {
    /// The parameter prefix the specification's members are carried under.
    fn prefix(self) -> &'static str {
        match self {
            Self::Own => "LaunchTemplate",
            Self::MixedPolicy => "MixedInstancesPolicy.LaunchTemplate.\
                                  LaunchTemplateSpecification",
            Self::MixedPolicyOverride => "MixedInstancesPolicy.LaunchTemplate.Overrides.\
                                          member.1.LaunchTemplateSpecification",
        }
    }
}

/// Which half of the specification the request states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    /// `LaunchTemplateId`, carrying the id EC2 assigned the account's template.
    HeldId,
    /// `LaunchTemplateName`, carrying the name EC2 holds that template under.
    HeldName,
    /// `LaunchTemplateId`, of the published form, for a template nothing created.
    AbsentId,
    /// `LaunchTemplateName`, for a template nothing created.
    AbsentName,
}

/// One row: where the group names the template, which half it states, and whether the group
/// must end up reporting both halves.
struct Row {
    case: &'static str,
    group: &'static str,
    source: Source,
    named: Named,
    /// Whether the group must report the OTHER half too: a template the account holds is
    /// resolved, one it does not hold is left exactly as the request stated it.
    resolved: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a group that names its own launch template by id reports its name too",
        group: "fleet-own-by-id",
        source: Source::Own,
        named: Named::HeldId,
        resolved: true,
    },
    Row {
        case: "a group that names its own launch template by name reports its id too",
        group: "fleet-own-by-name",
        source: Source::Own,
        named: Named::HeldName,
        resolved: true,
    },
    Row {
        case: "the launch template of a mixed instances policy, named by id",
        group: "fleet-policy-by-id",
        source: Source::MixedPolicy,
        named: Named::HeldId,
        resolved: true,
    },
    Row {
        case: "the launch template an override of that policy names, by name",
        group: "fleet-override-by-name",
        source: Source::MixedPolicyOverride,
        named: Named::HeldName,
        resolved: true,
    },
    Row {
        case: "an id of the published form the account holds no template for is left as it \
               was stated",
        group: "fleet-absent-id",
        source: Source::Own,
        named: Named::AbsentId,
        resolved: false,
    },
    Row {
        case: "a name the account holds no template under is left as it was stated",
        group: "fleet-absent-name",
        source: Source::Own,
        named: Named::AbsentName,
        resolved: false,
    },
];

/// One emulator with both services on one engine: the group and the launch template belong
/// to ONE account, and one engine is what makes the two views that account.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    router
}

/// Create the account's launch template over Amazon EC2's own wire, and answer the id that
/// service assigned it — never one this suite spells out.
async fn create_launch_template(router: &MultiServiceRouter) -> String {
    let (status, body) = ec2_query(
        router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", TEMPLATE_NAME),
            p("LaunchTemplateData.ImageId", IMAGE_ID),
            p("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateLaunchTemplate must succeed, got {status}: {body}"
    );
    let id = tag_value(&body, "launchTemplateId");
    assert!(
        !id.is_empty(),
        "CreateLaunchTemplate must report the template's id, got {body}"
    );
    id
}

/// The `(member, value)` the row's request states the template by.
fn stated(named: Named, held_id: &str) -> (&'static str, String) {
    match named {
        Named::HeldId => ("LaunchTemplateId", held_id.to_string()),
        Named::HeldName => ("LaunchTemplateName", TEMPLATE_NAME.to_string()),
        Named::AbsentId => ("LaunchTemplateId", ABSENT_TEMPLATE_ID.to_string()),
        Named::AbsentName => ("LaunchTemplateName", ABSENT_TEMPLATE_NAME.to_string()),
    }
}

/// The `(LaunchTemplateId, LaunchTemplateName)` the group reports for the specification the
/// row's source carries, read off `DescribeAutoScalingGroups`.
fn reported_specification(body: &str) -> (String, String) {
    (
        tag_value(body, "LaunchTemplateId"),
        tag_value(body, "LaunchTemplateName"),
    )
}

#[tokio::test]
async fn a_group_reports_both_halves_of_the_launch_template_it_resolves() {
    for row in ROWS {
        let case = row.case;
        let router = emulator();
        let held_id = create_launch_template(&router).await;
        let (member, value) = stated(row.named, &held_id);

        let mut params = vec![
            p("AutoScalingGroupName", row.group),
            p("MinSize", "0"),
            p("MaxSize", "1"),
            p("AvailabilityZones.member.1", "us-west-2a"),
            p(&format!("{}.{member}", row.source.prefix()), value.clone()),
        ];
        if row.source == Source::MixedPolicyOverride {
            // An override names the instance type it overrides, beside the template it
            // launches that type from.
            params.push(p(
                "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.InstanceType",
                INSTANCE_TYPE,
            ));
        }
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &borrowed(&params),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateAutoScalingGroup must succeed, got {status}: {body}"
        );

        let (status, described) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", row.group)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAutoScalingGroups must answer 200, got {status}: {described}"
        );
        let (reported_id, reported_name) = reported_specification(&described);
        if row.resolved {
            assert_eq!(
                (reported_id.as_str(), reported_name.as_str()),
                (held_id.as_str(), TEMPLATE_NAME),
                "case {case:?}: the group must report both halves of the template it \
                 resolved, got {described}"
            );
        } else {
            // Nothing is invented for a template the account does not hold: the half the
            // request stated is reported, and the other stays absent.
            let expected = match row.named {
                Named::AbsentId => (value.as_str(), ""),
                _ => ("", value.as_str()),
            };
            assert_eq!(
                (reported_id.as_str(), reported_name.as_str()),
                expected,
                "case {case:?}: a template the account does not hold must be reported exactly \
                 as the request stated it, got {described}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_onto_another_launch_template_resolves_it_the_same_way() {
    let case = "a group updated from one launch template onto another, named by id";
    let router = emulator();
    let first = create_launch_template(&router).await;

    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "fleet-updated"),
            ("MinSize", "0"),
            ("MaxSize", "1"),
            ("AvailabilityZones.member.1", "us-west-2a"),
            ("LaunchTemplate.LaunchTemplateName", TEMPLATE_NAME),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateAutoScalingGroup must succeed, got {status}: {body}"
    );

    // A second template of the same account, which the update moves the group onto by ID.
    let (status, body) = ec2_query(
        &router,
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", "asg-fleet-template-two"),
            p("LaunchTemplateData.ImageId", IMAGE_ID),
            p("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the second CreateLaunchTemplate must succeed, got {status}: {body}"
    );
    let second = tag_value(&body, "launchTemplateId");
    assert_ne!(
        second, first,
        "case {case:?}: the two templates must be two resources, got {second:?}"
    );

    let (status, body) = query(
        &router,
        "UpdateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "fleet-updated"),
            ("LaunchTemplate.LaunchTemplateId", second.as_str()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateAutoScalingGroup must succeed, got {status}: {body}"
    );

    let (_, described) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "fleet-updated")],
    )
    .await;
    assert_eq!(
        reported_specification(&described),
        (second.clone(), "asg-fleet-template-two".to_string()),
        "case {case:?}: the updated group must report both halves of the template it is now \
         on, got {described}"
    );
    // One specification, not two: the group reports the template it is on and no trace of
    // the one it left.
    assert_eq!(
        tag_values(&described, "LaunchTemplateName"),
        vec!["asg-fleet-template-two".to_string()],
        "case {case:?}: expected exactly one launch template on the group, got {described}"
    );
}
