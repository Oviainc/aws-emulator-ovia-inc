// SPDX-License-Identifier: CC0-1.0

//! The IAM instance profile a launch template names is resolved when Amazon EC2 Auto
//! Scaling CONSUMES the template.
//!
//! A launch template's `IamInstanceProfile` is a REFERENCE to an IAM resource. Amazon EC2
//! stores whatever well-formed value `CreateLaunchTemplate` was given without resolving
//! it, so the reference is resolved by the service that consumes the template: a group
//! launches its instances under that profile, so a request that would build a group on a
//! template naming an instance profile the account does not hold is refused — HTTP 400,
//! error code `ValidationError`, a message naming the offending launch parameter and the
//! value as supplied — and no group is stored.
//!
//! The table below spans what that rule generalises over: whether the reference is given
//! as an ARN or as a name, whether the profile it names is one the account holds, the path
//! the held profile lives under, the casing the name is written in (IAM entity names are
//! not distinguished by case), the IAM entity type and account an ARN names, where the
//! group names the template (`LaunchTemplate`, the launch template of a
//! `MixedInstancesPolicy`, or the template one of that policy's overrides names), and
//! which selector picks the template version. The recorded case — an absent profile named
//! by ARN — is one row of it.
//!
//! All three services are mounted on ONE engine and driven over their own wire protocols —
//! awsQuery for Auto Scaling and IAM, ec2Query for Amazon EC2 — the way the emulator serves
//! them, so the profiles and templates these assertions resolve against are the ones a
//! client created.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_ec2_amazonec2 as ec2_service;
use service_com_amazonaws_iam_awsidentitymanagementv20100508 as iam_service;

use crate::support::{
    borrowed, ec2_query, iam_query, p, query, seed_subnets, tag_value, tag_values, xml_unescape,
};

/// The instance profile every row's account holds.
const HELD_PROFILE: &str = "asg-instance-profile";

/// An instance profile name no row ever creates, so no account in this suite holds one.
const ABSENT_PROFILE: &str = "asg-instance-profile-never-created";

/// The image and instance type every launch template in this suite carries, so a row's
/// template differs from another's only in the reference under test.
const IMAGE_ID: &str = "ami-00000000000000021";
const INSTANCE_TYPE: &str = "t3.micro";

/// How the launch template names the instance profile its instances run under.
#[derive(Debug, Clone, Copy)]
enum Reference {
    /// The template carries no `IamInstanceProfile`: there is no reference to resolve.
    Absent,
    /// `Arn`, carrying the ARN IAM minted for the profile the account holds.
    HeldArn,
    /// `Name`, carrying the name IAM holds that profile under.
    HeldName,
    /// `Name`, carrying that same name upper-cased.
    HeldNameUpperCased,
    /// `Arn`, built for a profile of this account that was never created.
    AbsentArn,
    /// The same, under a path, so a refusal is not a property of the default path.
    AbsentArnUnderPath,
    /// `Name`, naming a profile that was never created.
    AbsentName,
    /// `Arn`, naming an instance profile of ANOTHER account, whose IAM this emulator
    /// does not hold.
    AnotherAccountsArn,
    /// `Arn`, naming a different IAM entity type of this account: a role is not an
    /// instance profile even where the two share a name.
    ThisAccountsRoleArn,
    /// `Arn`, carrying a value that is not an ARN at all.
    NotAnArn,
}

/// Which launch parameter the reference is carried under, and `None` for a template that
/// carries no reference.
impl Reference {
    fn member(self) -> Option<&'static str> {
        match self {
            Self::Absent => None,
            Self::HeldName | Self::HeldNameUpperCased | Self::AbsentName => Some("Name"),
            _ => Some("Arn"),
        }
    }

    /// The value the template carries, built from the account's own state: the ARN IAM
    /// minted for the held profile, or a value naming something the account does not hold.
    fn value(self, held_arn: &str) -> Option<String> {
        let account = ::engine::emulator_account_id();
        let value = match self {
            Self::Absent => return None,
            Self::HeldArn => held_arn.to_string(),
            Self::HeldName => HELD_PROFILE.to_string(),
            Self::HeldNameUpperCased => HELD_PROFILE.to_ascii_uppercase(),
            Self::AbsentArn => format!("arn:aws:iam::{account}:instance-profile/{ABSENT_PROFILE}"),
            Self::AbsentArnUnderPath => {
                format!("arn:aws:iam::{account}:instance-profile/service-role/{ABSENT_PROFILE}")
            }
            Self::AbsentName => ABSENT_PROFILE.to_string(),
            Self::AnotherAccountsArn => {
                format!("arn:aws:iam::123456789012:instance-profile/{HELD_PROFILE}")
            }
            Self::ThisAccountsRoleArn => format!("arn:aws:iam::{account}:role/{HELD_PROFILE}"),
            Self::NotAnArn => format!("instance-profile/{HELD_PROFILE}"),
        };
        Some(value)
    }
}

/// Where the group names the launch template under test.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// `LaunchTemplate`, naming the template by name.
    ByName,
    /// `LaunchTemplate`, naming the template by id.
    ById,
    /// `MixedInstancesPolicy`, whose own launch template is the one under test.
    MixedPolicy,
    /// `MixedInstancesPolicy` whose own launch template carries no reference and whose
    /// OVERRIDE names the template under test — "some instance types might require a
    /// launch template with a different AMI"
    /// (`LaunchTemplateOverrides$LaunchTemplateSpecification`).
    MixedPolicyOverride,
}

/// One row: an account, a launch template, and the group the row tries to build on it.
struct Row {
    case: &'static str,
    /// The `Path` the held instance profile is created under.
    held_path: &'static str,
    reference: Reference,
    source: Source,
    /// The `Version` the group's specification carries; `None` leaves it out, which the
    /// model documents as `$Default`.
    version: Option<&'static str>,
    /// Whether the create must be ACCEPTED: the reference resolves, or there is none.
    accepted: bool,
}

/// The parameter a refusal names for a reference carried under `member`, and the sentence
/// it ends with — Amazon EC2's own wording for a launch parameter naming an instance
/// profile that does not resolve.
fn expected_refusal(member: &str, value: &str) -> String {
    let (parameter, detail) = if member == "Arn" {
        ("iamInstanceProfile.arn", "Invalid IAM Instance Profile ARN")
    } else {
        (
            "iamInstanceProfile.name",
            "Invalid IAM Instance Profile name",
        )
    };
    format!(
        "You must use a valid fully-formed launch template. Value ({value}) for parameter \
         {parameter} is invalid. {detail}"
    )
}

/// One emulator with all three services on one engine: the group, the launch template and
/// the instance profile belong to ONE account, and one engine is what makes the three
/// views that account.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(ec2_service::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(iam_service::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Create the instance profile the account holds, through IAM's own API, and answer the
/// ARN IAM minted for it — never one this suite spells out.
async fn create_instance_profile(router: &MultiServiceRouter, path: &str) -> String {
    let (status, body) = iam_query(
        router,
        "CreateInstanceProfile",
        &[p("InstanceProfileName", HELD_PROFILE), p("Path", path)],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateInstanceProfile under {path} expected 200, got {status}: {body}"
    );
    let arn = tag_value(&body, "Arn");
    assert!(
        !arn.is_empty(),
        "CreateInstanceProfile must report the profile's ARN, got {body}"
    );
    arn
}

/// Create one Amazon EC2 launch template over the wire, optionally carrying an
/// `IamInstanceProfile` reference, and answer its id.
async fn create_launch_template(
    router: &MultiServiceRouter,
    name: &str,
    profile: Option<(&str, &str)>,
) -> String {
    let mut params = vec![
        p("LaunchTemplateName", name),
        p("LaunchTemplateData.ImageId", IMAGE_ID),
        p("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
    ];
    if let Some((member, value)) = profile {
        params.push(p(
            &format!("LaunchTemplateData.IamInstanceProfile.{member}"),
            value,
        ));
    }
    let (status, body) = ec2_query(router, "CreateLaunchTemplate", &params).await;
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

/// The `LaunchTemplate` or `MixedInstancesPolicy` parameters that name the template under
/// test, as `source` names it.
fn source_params(
    source: Source,
    template_name: &str,
    template_id: &str,
    plain_template_name: &str,
    version: Option<&str>,
) -> Vec<(String, String)> {
    let (prefix, mut params) = match source {
        Source::ByName => (
            "LaunchTemplate".to_string(),
            vec![p("LaunchTemplate.LaunchTemplateName", template_name)],
        ),
        Source::ById => (
            "LaunchTemplate".to_string(),
            vec![p("LaunchTemplate.LaunchTemplateId", template_id)],
        ),
        Source::MixedPolicy => {
            let prefix =
                "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification".to_string();
            (
                prefix.clone(),
                vec![p(&format!("{prefix}.LaunchTemplateName"), template_name)],
            )
        }
        Source::MixedPolicyOverride => {
            let prefix = "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1\
                          .LaunchTemplateSpecification"
                .to_string();
            (
                prefix.clone(),
                vec![
                    p(
                        "MixedInstancesPolicy.LaunchTemplate.LaunchTemplateSpecification\
                         .LaunchTemplateName",
                        plain_template_name,
                    ),
                    p(
                        "MixedInstancesPolicy.LaunchTemplate.Overrides.member.1.InstanceType",
                        INSTANCE_TYPE,
                    ),
                    p(&format!("{prefix}.LaunchTemplateName"), template_name),
                ],
            )
        }
    };
    if let Some(version) = version {
        params.push(p(&format!("{prefix}.Version"), version));
    }
    params
}

/// Every launch template a group is built from has its `IamInstanceProfile` resolved
/// against the account's own IAM instance profiles: the group is created when the
/// reference resolves, and refused — storing no group — when it does not.
#[tokio::test]
async fn a_group_is_refused_when_its_launch_template_names_an_absent_instance_profile() {
    let rows = [
        Row {
            case: "the held profile by ARN, template by name, no version selector",
            held_path: "/",
            reference: Reference::HeldArn,
            source: Source::ByName,
            version: None,
            accepted: true,
        },
        Row {
            case: "the held profile by ARN under a path, template by id, $Default",
            held_path: "/service-role/",
            reference: Reference::HeldArn,
            source: Source::ById,
            version: Some("$Default"),
            accepted: true,
        },
        Row {
            case: "the held profile by name, through a mixed instances policy, $Latest",
            held_path: "/",
            reference: Reference::HeldName,
            source: Source::MixedPolicy,
            version: Some("$Latest"),
            accepted: true,
        },
        Row {
            case: "the held profile by name in another casing, template by name, version 1",
            held_path: "/eng/tools/",
            reference: Reference::HeldNameUpperCased,
            source: Source::ByName,
            version: Some("1"),
            accepted: true,
        },
        Row {
            case: "a template carrying no instance profile at all",
            held_path: "/",
            reference: Reference::Absent,
            source: Source::ByName,
            version: None,
            accepted: true,
        },
        Row {
            case: "a profile never created, by ARN, template by id",
            held_path: "/",
            reference: Reference::AbsentArn,
            source: Source::ById,
            version: None,
            accepted: false,
        },
        Row {
            case: "a profile never created, by ARN under a path, through a mixed policy",
            held_path: "/",
            reference: Reference::AbsentArnUnderPath,
            source: Source::MixedPolicy,
            version: Some("$Default"),
            accepted: false,
        },
        Row {
            case: "a profile never created, by name, template by name, $Latest",
            held_path: "/service-role/",
            reference: Reference::AbsentName,
            source: Source::ByName,
            version: Some("$Latest"),
            accepted: false,
        },
        Row {
            case: "a profile never created, by name, named by a policy override",
            held_path: "/",
            reference: Reference::AbsentName,
            source: Source::MixedPolicyOverride,
            version: None,
            accepted: false,
        },
        Row {
            case: "another account's instance profile ARN",
            held_path: "/",
            reference: Reference::AnotherAccountsArn,
            source: Source::ByName,
            version: None,
            accepted: false,
        },
        Row {
            case: "an ARN of this account naming a role, not an instance profile",
            held_path: "/",
            reference: Reference::ThisAccountsRoleArn,
            source: Source::ById,
            version: Some("1"),
            accepted: false,
        },
        Row {
            case: "a value in Arn that is not an ARN at all",
            held_path: "/",
            reference: Reference::NotAnArn,
            source: Source::ByName,
            version: None,
            accepted: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One emulator per row: each row is its own account, so one row's profiles and
        // templates never explain another's.
        let (router, engine) = emulator();
        let subnets = seed_subnets(&engine, 1);
        let subnet = subnets
            .first()
            .map(|subnet| subnet.id.clone())
            .expect("one seeded subnet");
        let held_arn = create_instance_profile(&router, row.held_path).await;

        // Amazon EC2 stores whatever reference the template was given, WITHOUT resolving
        // it — the half of the rule that is EC2's.
        let member = row.reference.member();
        let value = row.reference.value(&held_arn);
        let template_name = format!("lt-{index}");
        let plain_template_name = format!("lt-{index}-plain");
        let template_id = create_launch_template(
            &router,
            &template_name,
            member.zip(value.as_deref()),
        )
        .await;
        create_launch_template(&router, &plain_template_name, None).await;
        if let (Some(member), Some(value)) = (member, value.as_deref()) {
            let (status, body) = ec2_query(
                &router,
                "DescribeLaunchTemplateVersions",
                &[p("LaunchTemplateId", template_id.clone())],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: DescribeLaunchTemplateVersions expected 200, got {status}: {body}",
                row.case
            );
            let stored = tag_value(&body, &member.to_ascii_lowercase());
            assert_eq!(
                stored, value,
                "case {:?}: Amazon EC2 must store the reference verbatim, got {stored:?} in \
                 {body}",
                row.case
            );
        }

        let name = format!("asg-{index}");
        let mut params = vec![
            p("AutoScalingGroupName", name.clone()),
            p("MinSize", "0"),
            p("MaxSize", "1"),
            p("DesiredCapacity", "0"),
            p("VPCZoneIdentifier", subnet),
        ];
        params.extend(source_params(
            row.source,
            &template_name,
            &template_id,
            &plain_template_name,
            row.version,
        ));
        let (status, body) =
            query(&router, "CreateAutoScalingGroup", &borrowed(&params)).await;

        if row.accepted {
            assert_eq!(
                status, 200,
                "case {:?}: CreateAutoScalingGroup expected 200, got {status}: {body}",
                row.case
            );
        } else {
            assert_eq!(
                status, 400,
                "case {:?}: CreateAutoScalingGroup expected 400, got {status}: {body}",
                row.case
            );
            let code = tag_value(&body, "Code");
            assert_eq!(
                code, "ValidationError",
                "case {:?}: expected the error code ValidationError, got {code:?} in {body}",
                row.case
            );
            let message = xml_unescape(&tag_value(&body, "Message"));
            let expected = expected_refusal(
                member.expect("a refused row carries a reference"),
                &value.clone().expect("a refused row carries a value"),
            );
            assert_eq!(
                message, expected,
                "case {:?}: expected the message {expected:?}, got {message:?}",
                row.case
            );
        }

        // What the account holds after the call: the group a resolved reference created,
        // and NO group where the reference did not resolve.
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
        let reported = tag_values(&body, "AutoScalingGroupName");
        let expected: Vec<String> = if row.accepted {
            vec![name.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported, expected,
            "case {:?}: expected DescribeAutoScalingGroups to report {expected:?}, got \
             {reported:?} in {body}",
            row.case
        );
    }
}

/// The same resolution guards an UPDATE that moves a group onto another launch template:
/// the group keeps the launch source it was created with, because a refused update stores
/// nothing.
#[tokio::test]
async fn an_update_onto_a_template_with_an_absent_instance_profile_is_refused() {
    // (case, the reference the new template carries, where the update names it)
    let rows = [
        (
            "by ARN, as the group's own LaunchTemplate",
            Reference::AbsentArn,
            Source::ByName,
        ),
        (
            "by name, as the group's own LaunchTemplate",
            Reference::AbsentName,
            Source::ById,
        ),
        (
            "by ARN under a path, through a mixed instances policy",
            Reference::AbsentArnUnderPath,
            Source::MixedPolicy,
        ),
    ];

    for (index, (case, reference, source)) in rows.iter().enumerate() {
        let (router, engine) = emulator();
        let subnets = seed_subnets(&engine, 1);
        let subnet = subnets
            .first()
            .map(|subnet| subnet.id.clone())
            .expect("one seeded subnet");
        let held_arn = create_instance_profile(&router, "/").await;

        // The group is created on a template whose reference RESOLVES.
        let good_template = format!("lt-good-{index}");
        create_launch_template(&router, &good_template, Some(("Arn", &held_arn))).await;
        let name = format!("asg-update-{index}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", name.as_str()),
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("DesiredCapacity", "0"),
                ("VPCZoneIdentifier", subnet.as_str()),
                ("LaunchTemplate.LaunchTemplateName", good_template.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateAutoScalingGroup expected 200, got {status}: {body}"
        );

        let bad_template = format!("lt-bad-{index}");
        let plain_template = format!("lt-plain-{index}");
        let member = reference.member().expect("a refused row has a reference");
        let value = reference.value(&held_arn).expect("a refused row has a value");
        let bad_id = create_launch_template(&router, &bad_template, Some((member, &value))).await;
        create_launch_template(&router, &plain_template, None).await;

        let mut params = vec![p("AutoScalingGroupName", name.clone())];
        params.extend(source_params(
            *source,
            &bad_template,
            &bad_id,
            &plain_template,
            None,
        ));
        let (status, body) =
            query(&router, "UpdateAutoScalingGroup", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: UpdateAutoScalingGroup expected 400, got {status}: {body}"
        );
        let code = tag_value(&body, "Code");
        assert_eq!(
            code, "ValidationError",
            "case {case:?}: expected the error code ValidationError, got {code:?} in {body}"
        );
        let message = xml_unescape(&tag_value(&body, "Message"));
        let expected = expected_refusal(member, &value);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );

        // The group still launches from the template it was created with.
        let (status, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAutoScalingGroups expected 200, got {status}: {body}"
        );
        let templates = tag_values(&body, "LaunchTemplateName");
        assert_eq!(
            templates,
            vec![good_template.clone()],
            "case {case:?}: expected the group to keep {good_template:?}, got {templates:?} in \
             {body}"
        );
    }
}
