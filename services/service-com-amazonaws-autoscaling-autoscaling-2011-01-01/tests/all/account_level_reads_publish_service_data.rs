// SPDX-License-Identifier: CC0-1.0

//! What the account-level reads of Amazon EC2 Auto Scaling PUBLISH, over the
//! awsQuery wire.
//!
//! THE RULE UNDER TEST: a reporting operation answers with every member the
//! service's own response carries, including the ones the on-disk model does not
//! declare, projected from this service's state and catalogs — so
//! `DescribeAccountLimits` reports the account's API call-rate bucket beside the
//! four counts, and `DescribeAutoScalingNotificationTypes` reports the whole set of
//! event types a group can be notified about rather than the subset the model's
//! prose enumerates. The last suite is the same fidelity rule on the refusal side:
//! a request the service refuses carries the whole sentence the service answers that
//! class with.
//!
//! Each suite drives the service the way the router does — one `MultiServiceRouter`
//! holding this service over one shared engine — so what is under test is the HTTP
//! behaviour (status, elements, element text), not a handler called directly. The
//! wire helpers are shared, from `support`.

use crate::support::{new_engine, query, router_with, tag_value, tag_values};

use autoscaling::state;
use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;

/// The launch source every group in this suite is created from: a launch TEMPLATE,
/// because no account can create a launch configuration
/// (`CreateLaunchConfiguration` is refused with `UnsupportedOperation`).
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// Every event type the notification catalog publishes, in the order it publishes
/// them.
///
/// Written out here rather than read from
/// `state::AUTO_SCALING_NOTIFICATION_TYPES`, so the assertion is against the
/// catalog the service is expected to publish rather than against whatever the
/// service happens to hold. Sources: the model's own documentation for
/// `NotificationConfiguration$NotificationType`
/// (`smithy_query(query="doc", shape="NotificationConfiguration", member="NotificationType")`),
/// which enumerates the four instance-lifecycle events and the test notification,
/// plus the two root-volume replacement events the service also notifies about and
/// that prose does not list.
const PUBLISHED_NOTIFICATION_TYPES: [&str; 7] = [
    "autoscaling:EC2_INSTANCE_LAUNCH",
    "autoscaling:EC2_INSTANCE_LAUNCH_ERROR",
    "autoscaling:EC2_INSTANCE_TERMINATE",
    "autoscaling:EC2_INSTANCE_TERMINATE_ERROR",
    "autoscaling:EC2_ROOT_VOLUME_REPLACEMENT",
    "autoscaling:EC2_ROOT_VOLUME_REPLACEMENT_ERROR",
    "autoscaling:TEST_NOTIFICATION",
];

/// What one row of the walk does to the store before the account's limits are read
/// back.
enum StoreAction {
    /// Nothing: the store is read as it stands.
    Nothing,
    /// Create groups until the store holds this many.
    GrowGroupsTo(usize),
    /// Delete the group of this name.
    DeleteGroup(&'static str),
    /// Attempt a launch configuration create, which this service refuses.
    RefusedLaunchConfiguration,
}

/// One step of the walk through store states: what to do to the store next, and the
/// group count the next `DescribeAccountLimits` answer must carry.
struct StoreStep {
    /// What this row does to the store before reading the limits back.
    name: &'static str,
    action: StoreAction,
    /// The groups the store holds once this row has run.
    groups: usize,
}

/// The rows differ along what the rule generalises over: how much the store holds
/// when the account's limits are read (nothing, one group, several), and whether the
/// last write grew it, shrank it or was refused outright.
const STORE_STEPS: &[StoreStep] = &[
    StoreStep {
        name: "an empty store",
        action: StoreAction::Nothing,
        groups: 0,
    },
    StoreStep {
        name: "one group",
        action: StoreAction::GrowGroupsTo(1),
        groups: 1,
    },
    StoreStep {
        name: "three groups",
        action: StoreAction::GrowGroupsTo(3),
        groups: 3,
    },
    StoreStep {
        name: "two groups, after a delete",
        action: StoreAction::DeleteGroup("asg-3"),
        groups: 2,
    },
    StoreStep {
        name: "two groups, after a refused launch configuration create",
        action: StoreAction::RefusedLaunchConfiguration,
        groups: 2,
    },
];

/// Create `name` as a group of the smallest capacity, so a row only moves the group
/// COUNT and never launches an instance.
async fn create_group(router: &smithy_server_runtime::MultiServiceRouter, name: &str) {
    let (status, body) = query(
        router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", name),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", "1"),
            ("DesiredCapacity", "0"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateAutoScalingGroup must be accepted, got {status}: {body}"
    );
}

/// The text inside the answer's `<ThrottlingLimits>` element, or `None` when the
/// answer carries none.
///
/// The opening tag is read up to its `>` rather than matched whole: the framework
/// stamps the model's `xmlns` on the first nested element of a response, so the
/// element arrives as `<ThrottlingLimits xmlns="…">`.
fn throttling_limits_subtree(xml: &str) -> Option<String> {
    let start = xml.find("<ThrottlingLimits")?;
    let after_tag = start + xml[start..].find('>')? + 1;
    let end = xml[after_tag..].find("</ThrottlingLimits>")?;
    Some(xml[after_tag..after_tag + end].to_string())
}

/// The call-rate bucket an answer reports, read off the wire: the text of each
/// sub-member of `ThrottlingLimits/AccountMaxCallRate`.
fn reported_call_rate(subtree: &str) -> (String, String) {
    (
        tag_value(subtree, "RefillRate"),
        tag_value(subtree, "BucketSize"),
    )
}

/// Assert one `DescribeAccountLimits` answer against the rule: the four counts, and
/// the account's call-rate bucket nested under `ThrottlingLimits`.
fn assert_account_limits(body: &str, case: &str, groups: usize) {
    for (tag, expected) in [
        ("MaxNumberOfAutoScalingGroups", state::MAX_GROUPS.to_string()),
        (
            "MaxNumberOfLaunchConfigurations",
            state::MAX_LAUNCH_CONFIGURATIONS.to_string(),
        ),
        ("NumberOfAutoScalingGroups", groups.to_string()),
        // No account can create a launch configuration, so this count stays 0.
        ("NumberOfLaunchConfigurations", "0".to_string()),
    ] {
        let actual = tag_value(body, tag);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {tag} = {expected:?}, got {actual:?} in {body}"
        );
    }

    // The bucket is reported as a SUBTREE, not as two loose members: a client
    // reading its own call budget walks ThrottlingLimits -> AccountMaxCallRate, so
    // both sub-members have to sit inside both elements.
    let subtree = throttling_limits_subtree(body)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ThrottlingLimits element, got {body}"));
    assert!(
        subtree.contains("<AccountMaxCallRate>")
            && subtree.contains("<RefillRate>")
            && subtree.contains("<BucketSize>"),
        "case {case:?}: expected AccountMaxCallRate/{{RefillRate,BucketSize}} inside \
         ThrottlingLimits, got {subtree:?}"
    );

    let (refill_rate, bucket_size) = reported_call_rate(&subtree);
    // A rate is a decimal: it carries a fractional part whatever its value, which is
    // what distinguishes the published rate from a count.
    assert!(
        refill_rate.contains('.'),
        "case {case:?}: expected RefillRate as a decimal, got {refill_rate:?}"
    );
    let parsed_rate = refill_rate
        .parse::<f64>()
        .unwrap_or_else(|error| panic!("case {case:?}: RefillRate {refill_rate:?}: {error}"));
    assert!(
        (parsed_rate - state::ACCOUNT_MAX_CALL_RATE_REFILL_RATE).abs() < f64::EPSILON,
        "case {case:?}: expected RefillRate = {}, got {parsed_rate}",
        state::ACCOUNT_MAX_CALL_RATE_REFILL_RATE
    );
    assert!(
        parsed_rate > 0.0,
        "case {case:?}: a call-rate bucket refills at a positive rate, got {parsed_rate}"
    );

    // A bucket size is a COUNT of calls: an integer, with no fractional part.
    assert_eq!(
        bucket_size,
        state::ACCOUNT_MAX_CALL_RATE_BUCKET_SIZE.to_string(),
        "case {case:?}: expected BucketSize = {}, got {bucket_size:?} in {body}",
        state::ACCOUNT_MAX_CALL_RATE_BUCKET_SIZE
    );
}

/// `DescribeAccountLimits` reports the account's API call-rate bucket on every
/// answer, whatever the store holds, beside the counts computed from the store.
#[tokio::test]
async fn account_limits_report_the_accounts_call_rate_bucket() {
    let router = router_with(new_engine());

    let mut created = 0usize;
    for (index, step) in STORE_STEPS.iter().enumerate() {
        match step.action {
            StoreAction::Nothing => {}
            StoreAction::GrowGroupsTo(target) => {
                while created < target {
                    created += 1;
                    create_group(&router, &format!("asg-{created}")).await;
                }
            }
            StoreAction::DeleteGroup(name) => {
                let (status, body) = query(
                    &router,
                    "DeleteAutoScalingGroup",
                    &[("AutoScalingGroupName", name)],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: DeleteAutoScalingGroup got {status}: {body}",
                    step.name
                );
            }
            StoreAction::RefusedLaunchConfiguration => {
                let (status, body) = query(
                    &router,
                    "CreateLaunchConfiguration",
                    &[
                        ("LaunchConfigurationName", "lc-1"),
                        ("ImageId", "ami-12345678"),
                        ("InstanceType", "t3.micro"),
                    ],
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {:?}: CreateLaunchConfiguration is refused, got {status}: {body}",
                    step.name
                );
            }
        }

        // Read it twice: the bucket is a published figure of the account, so a read
        // neither consumes it nor moves it.
        let mut answers = Vec::new();
        for attempt in 1..=2 {
            let (status, body) = query(&router, "DescribeAccountLimits", &[]).await;
            assert_eq!(
                status, 200,
                "case {:?} attempt {attempt}: a Unit-input read models no 4xx, got {status}: {body}",
                step.name
            );
            assert_account_limits(&body, step.name, step.groups);
            answers.push(reported_call_rate(
                &throttling_limits_subtree(&body).unwrap_or_default(),
            ));
        }
        assert_eq!(
            answers[0], answers[1],
            "case {} ({:?}): reading the limits must not move the call-rate bucket, got {answers:?}",
            index + 1,
            step.name
        );
    }
}

/// `DescribeAutoScalingNotificationTypes` publishes the whole catalog of event types
/// a group can be notified about, in the order the service publishes them.
#[tokio::test]
async fn the_notification_catalog_publishes_every_event_type() {
    let router = router_with(new_engine());

    let (status, body) = query(&router, "DescribeAutoScalingNotificationTypes", &[]).await;
    assert_eq!(
        status, 200,
        "a Unit-input read models no 4xx, expected 200, got {status}: {body}"
    );
    let published = tag_values(&body, "member");

    assert_eq!(
        published,
        PUBLISHED_NOTIFICATION_TYPES
            .iter()
            .map(|entry| (*entry).to_string())
            .collect::<Vec<String>>(),
        "expected the whole published catalog in order, got {published:?}"
    );

    // The invariants of the catalog, each asserted over every entry rather than over
    // the one the published list happens to start with: an event type is named in
    // this service's own namespace, appears once, and the catalog is published in
    // ASCII order.
    for (index, entry) in published.iter().enumerate() {
        assert!(
            entry.starts_with("autoscaling:"),
            "case {index} ({entry:?}): an event type is named in the service's own \
             namespace, got {entry:?}"
        );
        assert_eq!(
            published.iter().filter(|other| *other == entry).count(),
            1,
            "case {index} ({entry:?}): an event type is published once, got {published:?}"
        );
    }
    let mut sorted = published.clone();
    sorted.sort();
    assert_eq!(
        published, sorted,
        "expected the catalog in ASCII order, got {published:?}"
    );

    // The entry this suite's rule was found on sits before the test notification,
    // which is last: a client offering the catalog as a choice gets the service's own
    // ordering.
    let last = published
        .last()
        .unwrap_or_else(|| panic!("the catalog must not be empty, got {published:?}"));
    assert_eq!(
        last, "autoscaling:TEST_NOTIFICATION",
        "expected the test notification last, got {published:?}"
    );

    // And the catalog the handler serves is the one this service holds, so nothing
    // publishes a second copy of the membership.
    assert_eq!(
        published,
        state::AUTO_SCALING_NOTIFICATION_TYPES
            .iter()
            .map(|entry| (*entry).to_string())
            .collect::<Vec<String>>(),
        "expected the service's own catalog, got {published:?}"
    );

    // A lifecycle transition is a SEPARATE catalog: neither of the two the service
    // publishes for lifecycle hooks is an event a group notifies about.
    let (_, hooks) = query(&router, "DescribeLifecycleHookTypes", &[]).await;
    for hook_type in tag_values(&hooks, "member") {
        assert!(
            !published.contains(&hook_type),
            "case {hook_type:?}: a lifecycle transition is not a notification type, \
             got {published:?}"
        );
    }
}

/// A `CreateAutoScalingGroup` request that names no launch source is refused with the
/// sentence Amazon EC2 Auto Scaling answers that class with, whole: the same fixed
/// text whatever else the request carries, since no member of it varies the sentence.
#[tokio::test]
async fn a_request_with_no_launch_source_is_refused_with_its_fixed_sentence() {
    let router = router_with(new_engine());

    // The rows differ in what the request carries BESIDE the missing launch source:
    // the required members alone, a placement, a desired capacity, a health check and
    // a tag. None of them names a launch template, a launch configuration or a mixed
    // instances policy, so every row is the same refusal.
    let cases: [(&str, Vec<(&str, &str)>); 5] = [
        ("the required members alone", vec![]),
        (
            "a placement",
            vec![("AvailabilityZones.member.1", "us-west-2a")],
        ),
        ("a desired capacity", vec![("DesiredCapacity", "1")]),
        ("a health check", vec![("HealthCheckType", "EC2")]),
        (
            "a tag",
            vec![("Tags.member.1.Key", "env"), ("Tags.member.1.Value", "test")],
        ),
    ];

    for (case, extra) in &cases {
        let mut params = vec![
            ("AutoScalingGroupName", "asg-no-launch-source"),
            ("MinSize", "0"),
            ("MaxSize", "1"),
        ];
        params.extend(extra.iter().copied());
        let (status, body) = query(&router, "CreateAutoScalingGroup", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request naming no launch source is refused, got {status}: {body}"
        );
        let code = tag_value(&body, "Code");
        assert_eq!(
            code, "ValidationError",
            "case {case:?}: expected the error code ValidationError, got {code:?} in {body}"
        );
        let message = tag_value(&body, "Message");
        assert_eq!(
            message,
            "Valid requests must contain either LaunchTemplate, LaunchConfigurationName, \
             InstanceId or MixedInstancesPolicy parameter.",
            "case {case:?}: expected the refusal's whole sentence, got {message:?}"
        );
    }
}
