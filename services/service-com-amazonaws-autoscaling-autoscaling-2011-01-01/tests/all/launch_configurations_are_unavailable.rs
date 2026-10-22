// SPDX-License-Identifier: CC0-1.0

//! The launch configuration surface, end to end over the awsQuery wire.
//!
//! Launch configurations are not a resource this account can create: every
//! `CreateLaunchConfiguration` call is refused with HTTP 400 and the error code
//! `UnsupportedOperation` (a `Sender` fault), whatever the request carries. The
//! rest of the surface follows from that — `DescribeLaunchConfigurations` reports
//! the required-but-empty `LaunchConfigurations` element for every page it can be
//! asked for, `DeleteLaunchConfiguration` answers `ValidationError` for every
//! name, and a group that names a launch configuration cannot be created either.
//!
//! Each assertion below spans a SET of requests, because none of this behaviour
//! depends on any one request's values.

use std::sync::Arc;
use std::time::SystemTime;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;

use crate::support::{new_engine, query, router_with, tag_values};

/// Whether the response carries the `LaunchConfigurations` element at all — the
/// member is `@required`, so it must be PRESENT and empty, never absent.
fn carries_the_launch_configurations_element(xml: &str) -> bool {
    xml.contains("<LaunchConfigurations>") || xml.contains("<LaunchConfigurations/>")
}

/// One request, named by the case it stands for.
type RequestCase<'a> = (&'static str, Vec<(&'a str, &'a str)>);

/// Every `CreateLaunchConfiguration` request is refused with HTTP 400 /
/// `UnsupportedOperation`, whatever it carries, and stores nothing.
///
/// The rows differ along everything a request can differ along: which optional
/// members it sets, how many, their size, the launch source it names (an image
/// and instance type, an instance to copy), and a name at each end of the
/// modelled `XmlStringMaxLen255` length range.
#[tokio::test]
async fn every_launch_configuration_create_is_refused() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    let longest_name = "n".repeat(255);
    let cases: Vec<RequestCase<'_>> = vec![
        (
            "the required member alone",
            vec![("LaunchConfigurationName", "lc-bare")],
        ),
        (
            "the usual create: a name, an image and an instance type",
            vec![
                ("LaunchConfigurationName", "lc-usual"),
                ("ImageId", "ami-0123456789abcdef0"),
                ("InstanceType", "t3.micro"),
            ],
        ),
        (
            "a different image and a different instance type",
            vec![
                ("LaunchConfigurationName", "lc-other"),
                ("ImageId", "ami-00000000000000000"),
                ("InstanceType", "m5.24xlarge"),
            ],
        ),
        (
            "copied from a running EC2 instance instead",
            vec![
                ("LaunchConfigurationName", "lc-from-instance"),
                ("InstanceId", "i-0123456789abcdef0"),
            ],
        ),
        (
            "every optional member the operation takes, set at once",
            vec![
                ("LaunchConfigurationName", "lc-full"),
                ("ImageId", "ami-0123456789abcdef0"),
                ("InstanceType", "t3.micro"),
                ("KeyName", "a-key-pair"),
                ("SecurityGroups.member.1", "sg-01234567"),
                ("SecurityGroups.member.2", "sg-89abcdef"),
                ("ClassicLinkVPCId", "vpc-01234567"),
                ("ClassicLinkVPCSecurityGroups.member.1", "sg-0f0f0f0f"),
                ("UserData", "#!/bin/sh\necho hello\n"),
                ("KernelId", "aki-01234567"),
                ("RamdiskId", "ari-01234567"),
                ("BlockDeviceMappings.member.1.DeviceName", "/dev/xvda"),
                ("BlockDeviceMappings.member.1.Ebs.VolumeSize", "8"),
                ("InstanceMonitoring.Enabled", "false"),
                ("SpotPrice", "0.010"),
                ("IamInstanceProfile", "an-instance-profile"),
                ("EbsOptimized", "true"),
                ("AssociatePublicIpAddress", "true"),
                ("PlacementTenancy", "dedicated"),
                ("MetadataOptions.HttpTokens", "required"),
            ],
        ),
        (
            // `LaunchConfigurationName` is `XmlStringMaxLen255`, so this is the
            // longest name the boundary validator lets through.
            "a name at the top of the modelled length range",
            vec![
                ("LaunchConfigurationName", longest_name.as_str()),
                ("ImageId", "ami-0123456789abcdef0"),
                ("InstanceType", "t3.micro"),
            ],
        ),
        (
            "a name that would be rejected as a group name, with a colon in it",
            vec![
                ("LaunchConfigurationName", "lc:with:colons"),
                ("ImageId", "ami-0123456789abcdef0"),
                ("InstanceType", "t3.micro"),
            ],
        ),
    ];

    for (case, params) in &cases {
        let (status, body) = query(&router, "CreateLaunchConfiguration", params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert!(
            body.contains("<Code>UnsupportedOperation</Code>"),
            "case {case:?}: expected the UnsupportedOperation code, got {body}"
        );
        assert!(
            body.contains("<Type>Sender</Type>"),
            "case {case:?}: a refused create is a Sender fault, got {body}"
        );
        assert!(
            body.contains("launch templates"),
            "case {case:?}: the refusal must point the caller at launch templates, got {body}"
        );
    }

    // Not one of them stored anything: the describe reports the empty element,
    // and the store itself holds no launch configuration.
    let (status, body) = query(&router, "DescribeLaunchConfigurations", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeLaunchConfigurations after the refusals: got {status}: {body}"
    );
    assert!(
        tag_values(&body, "LaunchConfigurationName").is_empty(),
        "a refused create must store nothing, got {body}"
    );
    let state = autoscaling::state::service_state(&*engine).expect("the service store");
    assert!(
        state.lock().launch_configurations.is_empty(),
        "a refused create must leave the store empty, got {:?}",
        state.lock().launch_configurations
    );

    // And each name the refused creates used is still unknown to every reader.
    for (case, params) in &cases {
        let name = params
            .iter()
            .find(|(key, _)| *key == "LaunchConfigurationName")
            .map(|(_, value)| *value)
            .expect("every case names the configuration");
        let (status, body) = query(
            &router,
            "DescribeLaunchConfigurations",
            &[("LaunchConfigurationNames.member.1", name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: describing the refused name got {status}: {body}"
        );
        assert!(
            tag_values(&body, "LaunchConfigurationName").is_empty(),
            "case {case:?}: the refused name must describe as absent, got {body}"
        );
        assert!(
            carries_the_launch_configurations_element(&body),
            "case {case:?}: the required LaunchConfigurations element must be present, got {body}"
        );
    }
}

/// `DescribeLaunchConfigurations` answers 200 with the required-but-empty
/// `LaunchConfigurations` element for every request shape — including a
/// `NextToken` the service never issued, which for this operation is the name the
/// next page resumes after and so can never be invalid.
#[tokio::test]
async fn describing_launch_configurations_always_answers_the_empty_element() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    // (case, the request's parameters)
    let cases: Vec<RequestCase<'_>> = vec![
        ("no parameters at all", vec![]),
        ("one name", vec![("LaunchConfigurationNames.member.1", "lc-1")]),
        (
            "several names",
            vec![
                ("LaunchConfigurationNames.member.1", "lc-1"),
                ("LaunchConfigurationNames.member.2", "lc-2"),
                ("LaunchConfigurationNames.member.3", "lc-3"),
            ],
        ),
        ("the smallest page", vec![("MaxRecords", "1")]),
        ("the documented default page", vec![("MaxRecords", "50")]),
        ("the documented maximum page", vec![("MaxRecords", "100")]),
        (
            "a token that is not a number",
            vec![("NextToken", "not-a-token")],
        ),
        (
            "a token in the shape this service's offset tokens take",
            vec![("NextToken", "next-9999")],
        ),
        ("a bare offset", vec![("NextToken", "9999")]),
        ("a bare zero", vec![("NextToken", "0")]),
        (
            "a token that is a launch configuration name",
            vec![("NextToken", "lc-1")],
        ),
        (
            "a token another operation issued, with a page size",
            vec![("NextToken", "next-20"), ("MaxRecords", "20")],
        ),
        (
            "a token and a name filter together",
            vec![
                ("NextToken", "not-a-token"),
                ("LaunchConfigurationNames.member.1", "lc-1"),
            ],
        ),
    ];

    for (case, params) in &cases {
        let (status, body) = query(&router, "DescribeLaunchConfigurations", params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert!(
            carries_the_launch_configurations_element(&body),
            "case {case:?}: the required LaunchConfigurations element must be present, got {body}"
        );
        assert!(
            tag_values(&body, "LaunchConfigurationName").is_empty(),
            "case {case:?}: the list must be empty, got {body}"
        );
        assert!(
            tag_values(&body, "NextToken").is_empty(),
            "case {case:?}: a single empty page carries no NextToken, got {body}"
        );
    }

    // A `MaxRecords` outside the documented range (1..=100) is still refused:
    // that is a bad request, not a page the service cannot fill.
    for (case, max_records) in [
        ("below the smallest page", "0"),
        ("a negative page size", "-1"),
        ("one past the documented maximum", "101"),
        ("far past the documented maximum", "4096"),
    ] {
        let (status, body) = query(
            &router,
            "DescribeLaunchConfigurations",
            &[("MaxRecords", max_records)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert!(
            body.contains("<Code>ValidationError</Code>"),
            "case {case:?}: expected the ValidationError code, got {body}"
        );
    }
}

/// One row of the paging table: the request's parameters, the names the page must
/// carry in order, and the `NextToken` that must follow it (empty for the last
/// page).
struct PageCase {
    case: &'static str,
    params: Vec<(&'static str, &'static str)>,
    expected: Vec<&'static str>,
    next_token: &'static str,
}

/// Seed the store this service reads with launch configurations under `names`.
///
/// No operation can create one, so the rows are written straight into the state
/// the handler reads — the point of the test is the CURSOR, and the cursor is a
/// property of the collection, not of how it was filled.
fn seed_launch_configurations(engine: &Arc<engine::InMemoryEngine>, names: &[&str]) {
    let state = autoscaling::state::service_state(&**engine).expect("the service store");
    let mut state = state.lock();
    for name in names {
        state.launch_configurations.insert(
            (*name).to_string(),
            autoscaling::state::StoredLaunchConfiguration {
                name: (*name).to_string(),
                arn: engine::emulator_arn(
                    "autoscaling",
                    "us-west-2",
                    &format!("launchConfiguration:seeded:launchConfigurationName/{name}"),
                ),
                image_id: "ami-0123456789abcdef0".to_string(),
                instance_type: "t3.micro".to_string(),
                key_name: None,
                security_groups: Vec::new(),
                classic_link_vpc_id: None,
                classic_link_vpc_security_groups: Vec::new(),
                user_data: None,
                kernel_id: None,
                ramdisk_id: None,
                spot_price: None,
                iam_instance_profile: None,
                block_device_mappings: Vec::new(),
                metadata_options: None,
                instance_monitoring_enabled: true,
                ebs_optimized: false,
                associate_public_ip: None,
                placement_tenancy: None,
                created_time: SystemTime::now(),
            },
        );
    }
}

/// `DescribeLaunchConfigurations`'s `NextToken` is the name the next page resumes
/// AFTER: a page is bounded by `MaxRecords`, the token it issues is the last name
/// on that page, the next call resumes strictly after it, and a token that is not
/// a stored name is a position in the same order rather than a refusal.
///
/// The store is seeded directly because no request can fill it (see
/// `every_launch_configuration_create_is_refused`); what is under test here is
/// the cursor, which the empty store cannot exercise.
#[tokio::test]
async fn the_name_cursor_pages_the_launch_configurations_it_is_given() {
    let engine = new_engine();
    let router = router_with(engine.clone());
    seed_launch_configurations(&engine, &["lc-a", "lc-b", "lc-c", "lc-d", "lc-e"]);

    let cases = [
        PageCase {
            case: "the whole collection on one page",
            params: vec![],
            expected: vec!["lc-a", "lc-b", "lc-c", "lc-d", "lc-e"],
            next_token: "",
        },
        PageCase {
            case: "the first page of two names",
            params: vec![("MaxRecords", "2")],
            expected: vec!["lc-a", "lc-b"],
            next_token: "lc-b",
        },
        PageCase {
            case: "the page after the token the first one issued",
            params: vec![("MaxRecords", "2"), ("NextToken", "lc-b")],
            expected: vec!["lc-c", "lc-d"],
            next_token: "lc-d",
        },
        PageCase {
            case: "the last, short page carries no token",
            params: vec![("MaxRecords", "2"), ("NextToken", "lc-d")],
            expected: vec!["lc-e"],
            next_token: "",
        },
        PageCase {
            case: "one name per page",
            params: vec![("MaxRecords", "1")],
            expected: vec!["lc-a"],
            next_token: "lc-a",
        },
        PageCase {
            case: "a name filter is paged the same way",
            params: vec![
                ("LaunchConfigurationNames.member.1", "lc-c"),
                ("LaunchConfigurationNames.member.2", "lc-e"),
                ("MaxRecords", "1"),
            ],
            expected: vec!["lc-c"],
            next_token: "lc-c",
        },
        PageCase {
            case: "the filtered page resumes after its own token",
            params: vec![
                ("LaunchConfigurationNames.member.1", "lc-c"),
                ("LaunchConfigurationNames.member.2", "lc-e"),
                ("NextToken", "lc-c"),
            ],
            expected: vec!["lc-e"],
            next_token: "",
        },
        PageCase {
            case: "a token that is not a stored name positions in the same order",
            params: vec![("MaxRecords", "2"), ("NextToken", "lc-bb")],
            expected: vec!["lc-c", "lc-d"],
            next_token: "lc-d",
        },
        PageCase {
            case: "a token past every name is the empty page, not a refusal",
            params: vec![("NextToken", "zzzz")],
            expected: vec![],
            next_token: "",
        },
        PageCase {
            case: "a token before every name is the first page",
            params: vec![("MaxRecords", "2"), ("NextToken", "aaaa")],
            expected: vec!["lc-a", "lc-b"],
            next_token: "lc-b",
        },
    ];

    for case in &cases {
        let (status, body) = query(&router, "DescribeLaunchConfigurations", &case.params).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status}: {body}",
            case.case
        );
        let reported = tag_values(&body, "LaunchConfigurationName");
        assert_eq!(
            reported,
            case.expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected {:?}, got {reported:?} in {body}",
            case.case,
            case.expected
        );
        let token = tag_values(&body, "NextToken").first().cloned().unwrap_or_default();
        assert_eq!(
            token, case.next_token,
            "case {:?}: expected the NextToken {:?}, got {token:?} in {body}",
            case.case, case.next_token
        );
        assert!(
            carries_the_launch_configurations_element(&body),
            "case {:?}: the required LaunchConfigurations element must be present, got {body}",
            case.case
        );
    }

    // Walking the tokens visits every seeded configuration exactly once, in name
    // order — the invariant the cursor exists for.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..6 {
        let mut params = vec![("MaxRecords", "2")];
        if let Some(token) = token.as_deref() {
            params.push(("NextToken", token));
        }
        let (status, body) = query(&router, "DescribeLaunchConfigurations", &params).await;
        assert_eq!(status, 200, "walking the pages: got {status}: {body}");
        seen.extend(tag_values(&body, "LaunchConfigurationName"));
        match tag_values(&body, "NextToken").first().cloned() {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert_eq!(
        seen,
        ["lc-a", "lc-b", "lc-c", "lc-d", "lc-e"]
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<String>>(),
        "walking the pages must visit every configuration once, in name order, got {seen:?}"
    );
}

/// `DeleteLaunchConfiguration` answers HTTP 400 / `ValidationError` for every
/// name, because no name can be the name of a stored launch configuration.
#[tokio::test]
async fn deleting_a_launch_configuration_refuses_every_name() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    let longest_name = "n".repeat(255);
    for (case, name) in [
        ("a plain name", "lc-1"),
        ("a name a create tried to use", "lc-usual"),
        ("a name with punctuation", "lc.1_2-3+4"),
        ("a single character", "x"),
        ("a name at the top of the modelled length range", longest_name.as_str()),
    ] {
        let (status, body) = query(
            &router,
            "DeleteLaunchConfiguration",
            &[("LaunchConfigurationName", name)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert!(
            body.contains("<Code>ValidationError</Code>"),
            "case {case:?}: expected the ValidationError code, got {body}"
        );
        assert!(
            body.contains("<Type>Sender</Type>"),
            "case {case:?}: an unknown name is a Sender fault, got {body}"
        );
        assert!(
            body.contains(name),
            "case {case:?}: the refusal must name the configuration asked for, got {body}"
        );
    }
}

/// A group cannot be built on a launch configuration either: the name it would
/// have to point at can never exist, so the create is refused with
/// `ValidationError` and no group is stored. A launch TEMPLATE is the launch
/// source that works.
#[tokio::test]
async fn a_group_cannot_be_created_from_a_launch_configuration() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    for (case, configuration) in [
        ("a name nothing ever created", "lc-missing"),
        ("a name a refused create used", "lc-usual"),
        ("a name with punctuation", "lc.1_2-3"),
    ] {
        let group = format!("asg-for-{}", configuration.replace(['.', '+'], "-"));
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", group.as_str()),
                ("LaunchConfigurationName", configuration),
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert!(
            body.contains("<Code>ValidationError</Code>"),
            "case {case:?}: expected the ValidationError code, got {body}"
        );

        let (status, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", group.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAutoScalingGroups got {status}: {body}"
        );
        assert!(
            tag_values(&body, "AutoScalingGroupName").is_empty(),
            "case {case:?}: a refused create must store no group, got {body}"
        );
    }

    // The same group, built on a launch template, is created and reads back.
    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-from-template"),
            ("LaunchTemplate.LaunchTemplateName", "lt-1"),
            ("LaunchTemplate.Version", "3"),
            ("MinSize", "0"),
            ("MaxSize", "1"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "a group built on a launch template: got {status}: {body}"
    );
    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-from-template")],
    )
    .await;
    assert_eq!(status, 200, "DescribeAutoScalingGroups: got {status}: {body}");
    assert_eq!(
        tag_values(&body, "LaunchTemplateName"),
        vec!["lt-1".to_string()],
        "the group must report the launch template it was created with, got {body}"
    );
    assert!(
        tag_values(&body, "LaunchConfigurationName").is_empty(),
        "a template-backed group reports no launch configuration, got {body}"
    );
}
