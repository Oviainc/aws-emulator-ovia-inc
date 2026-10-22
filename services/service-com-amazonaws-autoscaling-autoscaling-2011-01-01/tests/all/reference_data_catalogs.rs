// SPDX-License-Identifier: CC0-1.0

//! The zero-input reference-data reads of Amazon EC2 Auto Scaling, over the
//! awsQuery wire.
//!
//! THE RULE UNDER TEST: an operation whose modelled input is `smithy.api#Unit` has
//! no member a client can get wrong and no 4xx bound to it, so a well-formed call
//! is always answered HTTP 200 with the service's own data — the whole published
//! catalog for the reference-data describes, and for `DescribeAccountLimits` the
//! quotas this service enforces beside the counts computed from the store.
//!
//! The catalog table below is the class, not one row of it: its rows differ in the
//! size of the list (1, 2, 3, 5, 8, 9 and 20 entries), in its shape (a list of
//! strings against a list of structures) and in how many members the answer
//! carries, and every one of them is asserted through the same rule.
//!
//! Every assertion drives the service the way the router does — one
//! `MultiServiceRouter` holding this service over one shared engine — so what is
//! under test is the HTTP behaviour (status, response element, XML members), not a
//! handler called directly. The wire helpers are shared, from `support`.

use crate::support::{new_engine, query, router_with, tag_value, tag_values};

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use autoscaling::state;

/// The launch source every group in this suite is created from: a launch
/// TEMPLATE, because no account can create a launch configuration
/// (`CreateLaunchConfiguration` is refused with `UnsupportedOperation`).
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// One zero-input reference-data read: the action, the element each entry of its
/// list is reported in, and the catalog the service publishes for it.
struct CatalogCase {
    action: &'static str,
    /// The element an entry's VALUE lands in: a list of structures names the
    /// structure's own member (`<member><ProcessName>…</ProcessName></member>`), a
    /// list of strings the generic `<member>` the query protocol wraps it in.
    entry_tag: &'static str,
    /// The catalog, from `src/state.rs`, where each list's doc comment names the
    /// model member whose documentation publishes its membership and ordering.
    expected: &'static [&'static str],
}

/// Every reference-data catalog this service publishes, one row per operation.
const CATALOGS: &[CatalogCase] = &[
    CatalogCase {
        action: "DescribeScalingProcessTypes",
        entry_tag: "ProcessName",
        expected: state::SCALING_PROCESS_TYPES,
    },
    CatalogCase {
        action: "DescribeTerminationPolicyTypes",
        entry_tag: "member",
        expected: state::TERMINATION_POLICY_TYPES,
    },
    CatalogCase {
        action: "DescribeAdjustmentTypes",
        entry_tag: "AdjustmentType",
        expected: state::ADJUSTMENT_TYPES,
    },
    CatalogCase {
        action: "DescribeAutoScalingNotificationTypes",
        entry_tag: "member",
        expected: state::AUTO_SCALING_NOTIFICATION_TYPES,
    },
    CatalogCase {
        action: "DescribeLifecycleHookTypes",
        entry_tag: "member",
        expected: state::LIFECYCLE_HOOK_TYPES,
    },
    CatalogCase {
        action: "DescribeMetricCollectionTypes",
        entry_tag: "Metric",
        expected: state::METRIC_COLLECTION_TYPES,
    },
];

/// `DescribeAccountLimits` and every reference-data catalog answer 200 with their
/// published list — the top-level `<{Action}Response>` and `<{Action}Result>`
/// elements present, and the entries exactly the catalog, in order.
#[tokio::test]
async fn every_zero_input_reference_read_answers_its_published_catalog() {
    let router = router_with(new_engine());

    for case in CATALOGS {
        let (status, body) = query(&router, case.action, &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: a Unit-input read models no 4xx, expected 200, got {status}: {body}",
            case.action
        );
        for element in [
            format!("<{}Response", case.action),
            format!("<{}Result>", case.action),
        ] {
            assert!(
                body.contains(&element),
                "case {:?}: expected the response to carry {element:?}, got {body}",
                case.action
            );
        }
        let reported = tag_values(&body, case.entry_tag);
        assert_eq!(
            reported,
            case.expected
                .iter()
                .map(|entry| (*entry).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected the whole published catalog, got {reported:?}",
            case.action
        );
    }

    // The one catalog read that carries a second list: the granularity metrics
    // collection accepts, which the model documents as the single value `1Minute`.
    let (status, body) = query(&router, "DescribeMetricCollectionTypes", &[]).await;
    assert_eq!(
        status, 200,
        "case \"DescribeMetricCollectionTypes\": expected 200, got {status}: {body}"
    );
    let granularities = tag_values(&body, "Granularity");
    assert_eq!(
        granularities,
        state::METRIC_GRANULARITIES
            .iter()
            .map(|entry| (*entry).to_string())
            .collect::<Vec<String>>(),
        "case \"DescribeMetricCollectionTypes\": expected the published granularities, \
         got {granularities:?}"
    );
}

/// A catalog is SERVICE data: it does not depend on the store, and reading it
/// changes nothing a later caller sees. Same six rows, read before and after the
/// store has gained a group.
#[tokio::test]
async fn a_catalog_read_is_independent_of_state_and_changes_none_of_it() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    let mut first = Vec::new();
    for case in CATALOGS {
        let (status, body) = query(&router, case.action, &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status}: {body}",
            case.action
        );
        first.push(tag_values(&body, case.entry_tag));
    }

    let (status, body) = query(
        &router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", "asg-catalog"),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", "1"),
            ("DesiredCapacity", "0"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(status, 200, "CreateAutoScalingGroup: got {status}: {body}");

    for (case, before) in CATALOGS.iter().zip(&first) {
        // Read it twice more: the second read proves the first did not consume or
        // reorder anything.
        for attempt in 1..=2 {
            let (status, body) = query(&router, case.action, &[]).await;
            assert_eq!(
                status, 200,
                "case {:?} attempt {attempt}: expected 200, got {status}: {body}",
                case.action
            );
            let after = tag_values(&body, case.entry_tag);
            assert_eq!(
                &after, before,
                "case {:?} attempt {attempt}: a catalog must not depend on the store, \
                 expected {before:?}, got {after:?}",
                case.action
            );
        }
    }

    // And the store the catalog reads left alone still holds what was created.
    let (status, body) = query(
        &router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", "asg-catalog")],
    )
    .await;
    assert_eq!(status, 200, "DescribeAutoScalingGroups: got {status}: {body}");
    assert_eq!(
        tag_value(&body, "AutoScalingGroupName"),
        "asg-catalog",
        "a catalog read must leave the store alone, got {body}"
    );
}

/// The published catalog and the values the mutating operations accept are one
/// list: every termination policy `CreateAutoScalingGroup` validates against is in
/// the catalog `DescribeTerminationPolicyTypes` publishes, and a group can be
/// created naming any of them. The catalog also publishes the custom-policy TYPE
/// name, which a group names by a Lambda function ARN instead.
#[tokio::test]
async fn the_published_catalog_is_the_list_the_mutating_operations_accept() {
    let engine = new_engine();
    let router = router_with(engine.clone());

    let (status, body) = query(&router, "DescribeTerminationPolicyTypes", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeTerminationPolicyTypes: expected 200, got {status}: {body}"
    );
    let published = tag_values(&body, "member");

    for policy in state::TERMINATION_POLICIES {
        assert!(
            published.iter().any(|entry| entry == policy),
            "case {policy:?}: a policy CreateAutoScalingGroup accepts must be published, \
             got {published:?}"
        );
    }
    assert!(
        published
            .iter()
            .any(|entry| entry == state::TERMINATION_POLICY_TYPE_LAMBDA),
        "case {:?}: the custom-policy type name must be published, got {published:?}",
        state::TERMINATION_POLICY_TYPE_LAMBDA
    );

    for (index, policy) in published
        .iter()
        .filter(|entry| *entry != state::TERMINATION_POLICY_TYPE_LAMBDA)
        .enumerate()
    {
        let name = format!("asg-policy-{index}");
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", &name),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("DesiredCapacity", "0"),
                ("AvailabilityZones.member.1", "us-west-2a"),
                ("TerminationPolicies.member.1", policy),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {policy:?}: a published termination policy type must be accepted, \
             got {status}: {body}"
        );
    }

    // Two entries of the metric catalog the issue that prompted this suite did not
    // name, asserted by value so a truncated catalog is caught.
    let (_, body) = query(&router, "DescribeMetricCollectionTypes", &[]).await;
    let metrics = tag_values(&body, "Metric");
    for metric in ["WarmPoolWarmedCapacity", "GroupAndWarmPoolTotalCapacity"] {
        assert!(
            metrics.iter().any(|entry| entry == metric),
            "case {metric:?}: expected the published metric catalog to carry it, got {metrics:?}"
        );
    }

    // Two reads share one modelled list shape (`AutoScalingNotificationTypes`) and
    // still publish different catalogs: an SNS event a group notifies about is not
    // a transition a lifecycle hook is created for.
    let (_, body) = query(&router, "DescribeAutoScalingNotificationTypes", &[]).await;
    let notification_types = tag_values(&body, "member");
    let (_, body) = query(&router, "DescribeLifecycleHookTypes", &[]).await;
    let hook_types = tag_values(&body, "member");
    for hook_type in &hook_types {
        assert!(
            !notification_types.iter().any(|entry| entry == hook_type),
            "case {hook_type:?}: a lifecycle transition is published by \
             DescribeLifecycleHookTypes, not as a notification type, got {notification_types:?}"
        );
    }
}

/// `DescribeAccountLimits` reports the quotas this service publishes beside counts
/// COMPUTED from the store: a table over how many groups exist, walked by creating
/// and then deleting them, with the maxima unchanged throughout. The launch
/// configuration count stays 0 because no account can create one.
#[tokio::test]
async fn account_limits_count_what_the_store_holds() {
    let router = router_with(new_engine());

    // The maxima the answer must carry: the ceiling `CreateAutoScalingGroup`
    // answers `LimitExceededFault` past, and the launch configuration quota the
    // service publishes for the account (which nothing can reach, since
    // `CreateLaunchConfiguration` is refused outright).
    let expected_max_groups = state::MAX_GROUPS.to_string();
    let expected_max_configurations = state::MAX_LAUNCH_CONFIGURATIONS.to_string();

    // Each row: what to do next, then the counts the answer must carry.
    let mut case_number = 0;
    let mut assert_counts = |body: &str, configurations: usize, groups: usize, case: &str| {
        case_number += 1;
        for (tag, expected) in [
            ("NumberOfLaunchConfigurations", configurations.to_string()),
            ("NumberOfAutoScalingGroups", groups.to_string()),
            ("MaxNumberOfAutoScalingGroups", expected_max_groups.clone()),
            (
                "MaxNumberOfLaunchConfigurations",
                expected_max_configurations.clone(),
            ),
        ] {
            let actual = tag_value(body, tag);
            assert_eq!(
                actual, expected,
                "case {case_number} ({case}): expected {tag} = {expected}, got {actual:?} in {body}"
            );
        }
    };

    let (status, body) = query(&router, "DescribeAccountLimits", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeAccountLimits: a Unit-input read models no 4xx, got {status}: {body}"
    );
    assert!(
        body.contains("<DescribeAccountLimitsResponse")
            && body.contains("<DescribeAccountLimitsResult>"),
        "DescribeAccountLimits: expected the modelled response element, got {body}"
    );
    assert_counts(&body, 0, 0, "an empty store");

    // Launch configurations: a refused create adds nothing to count.
    for attempt in 1..=3usize {
        let name = format!("lc-{attempt}");
        let (status, body) = query(
            &router,
            "CreateLaunchConfiguration",
            &[
                ("LaunchConfigurationName", name.as_str()),
                ("ImageId", "ami-12345678"),
                ("InstanceType", "t3.micro"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "attempt {attempt}: CreateLaunchConfiguration is refused, got {status}: {body}"
        );
        let (status, body) = query(&router, "DescribeAccountLimits", &[]).await;
        assert_eq!(status, 200, "DescribeAccountLimits: got {status}: {body}");
        assert_counts(&body, 0, 0, "after a refused launch configuration create");
    }

    // Groups: the count follows each create, and the two counts are independent.
    for expected in 1..=2usize {
        let (status, body) = query(
            &router,
            "CreateAutoScalingGroup",
            &[
                ("AutoScalingGroupName", &format!("asg-{expected}")),
                LAUNCH_SOURCE,
                ("MinSize", "0"),
                ("MaxSize", "1"),
                ("DesiredCapacity", "0"),
                ("AvailabilityZones.member.1", "us-west-2a"),
            ],
        )
        .await;
        assert_eq!(status, 200, "CreateAutoScalingGroup: got {status}: {body}");
        let (status, body) = query(&router, "DescribeAccountLimits", &[]).await;
        assert_eq!(status, 200, "DescribeAccountLimits: got {status}: {body}");
        assert_counts(&body, 0, expected, "after creating a group");
    }

    // A delete moves the count back down: the numbers are computed, not counted up.
    let (status, body) = query(
        &router,
        "DeleteAutoScalingGroup",
        &[("AutoScalingGroupName", "asg-1")],
    )
    .await;
    assert_eq!(status, 200, "DeleteAutoScalingGroup: got {status}: {body}");
    // And a delete of a launch configuration is refused, leaving the count at 0.
    let (status, body) = query(
        &router,
        "DeleteLaunchConfiguration",
        &[("LaunchConfigurationName", "lc-3")],
    )
    .await;
    assert_eq!(
        status, 400,
        "DeleteLaunchConfiguration on a name nothing holds: got {status}: {body}"
    );
    let (status, body) = query(&router, "DescribeAccountLimits", &[]).await;
    assert_eq!(status, 200, "DescribeAccountLimits: got {status}: {body}");
    assert_counts(&body, 0, 1, "after deleting a group");
}
