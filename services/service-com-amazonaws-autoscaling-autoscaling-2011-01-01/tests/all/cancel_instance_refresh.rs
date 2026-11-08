// SPDX-License-Identifier: CC0-1.0

//! Cancelling the instance refresh a group has in progress.
//!
//! The rule under test: a cancellation acts on the refresh its group has IN PROGRESS —
//! it answers that refresh's own id, moves it to `Cancelling` and the clock then brings
//! it to `Cancelled`, starting no further replacement and rolling none back, so the
//! progress the refresh had already made is exactly what a later read reports; a group
//! whose refresh is not in progress has nothing to cancel and is refused with
//! `ActiveInstanceRefreshNotFound`.
//!
//! The tables below span what that rule generalises over: how many instances the group
//! holds (none, one, several), whether the request asks to wait for in-flight launches
//! and terminations (absent, so the model's default, or either value the boolean
//! admits), and whether any replacement had already been made when the cancellation
//! arrived. A second table spans the states a group's refreshes can be in when a
//! cancellation arrives and nothing is in progress.

use std::time::Duration;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;

use crate::support::{new_engine, query, router_with, tag_value, tag_values, xml_unescape};

/// The launch template the groups in this suite launch from.
const LAUNCH_TEMPLATE: &str = "lt-cancel";

/// The zone the groups in this suite place their instances in.
const ZONE: &str = "us-west-2a";

/// One span of this service's clock, plus a margin: enough for one instance
/// replacement, one lifecycle transition or one cancellation to have come due.
/// Derived from the service's own spans, so this and the implementation cannot drift
/// apart.
fn one_span() -> Duration {
    autoscaling::state::INSTANCE_REFRESH_REPLACEMENT
        .max(autoscaling::state::LIFECYCLE_TRANSITION)
        .max(autoscaling::state::INSTANCE_REFRESH_CANCELLATION)
        + Duration::from_millis(150)
}

/// Wait until the clock has certainly brought `steps` spans due.
async fn past_spans(steps: u32) {
    tokio::time::sleep(one_span() * (steps + 1)).await;
}

/// Create one group holding `capacity` instances and wait until they are placed.
async fn create_group(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    capacity: i32,
) {
    let capacity = capacity.to_string();
    let (status, body) = query(
        router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", name),
            ("LaunchTemplate.LaunchTemplateName", LAUNCH_TEMPLATE),
            ("MinSize", "0"),
            ("MaxSize", capacity.as_str()),
            ("DesiredCapacity", capacity.as_str()),
            ("AvailabilityZones.member.1", ZONE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateAutoScalingGroup for {name} expected 200, got {status}: {body}"
    );
    past_spans(0).await;
}

/// Start one refresh on `name` and answer the id the service tracks it by.
async fn start_refresh(router: &smithy_server_runtime::MultiServiceRouter, name: &str) -> String {
    let (status, body) = query(
        router,
        "StartInstanceRefresh",
        &[("AutoScalingGroupName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "StartInstanceRefresh for {name} expected 200, got {status}: {body}"
    );
    let id = tag_value(&body, "InstanceRefreshId");
    assert!(
        !id.is_empty(),
        "a started refresh carries its id, got {body}"
    );
    id
}

/// What `DescribeInstanceRefreshes` reports for the newest refresh of a group.
#[derive(Debug, Clone)]
struct ReportedRefresh {
    instance_refresh_id: String,
    status: String,
    percentage_complete: String,
    instances_to_update: String,
    end_time: String,
}

/// Describe one group's refreshes and read the newest one out of the answer.
async fn newest_refresh(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> ReportedRefresh {
    let (status, body) = query(
        router,
        "DescribeInstanceRefreshes",
        &[("AutoScalingGroupName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstanceRefreshes for {name} expected 200, got {status}: {body}"
    );
    ReportedRefresh {
        instance_refresh_id: tag_value(&body, "InstanceRefreshId"),
        status: tag_value(&body, "Status"),
        percentage_complete: tag_value(&body, "PercentageComplete"),
        instances_to_update: tag_value(&body, "InstancesToUpdate"),
        end_time: tag_value(&body, "EndTime"),
    }
}

/// The instance ids one group reports.
async fn group_instance_ids(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> Vec<String> {
    let (status, body) = query(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAutoScalingGroups for {name} expected 200, got {status}: {body}"
    );
    tag_values(&body, "InstanceId")
}

/// One row of the cancellation table.
struct Row {
    case: &'static str,
    /// How many instances the group holds when the refresh starts.
    capacity: i32,
    /// The `WaitForTransitioningInstances` the request names, or `None` to leave it to
    /// the model's default.
    wait: Option<&'static str>,
    /// How many of this service's spans elapse between starting the refresh and
    /// cancelling it: `0` cancels one that has replaced nothing yet.
    spans_before_cancel: u32,
}

/// Every row: the cancellation acts on the refresh in progress, reports it
/// `Cancelling` and then `Cancelled`, and keeps the progress it had made.
#[tokio::test]
async fn cancelling_a_refresh_in_progress_ends_it_without_rolling_it_back() {
    let rows = [
        Row {
            case: "a group holding no instance, the wait left to the model's default",
            capacity: 0,
            wait: None,
            spans_before_cancel: 0,
        },
        Row {
            case: "one instance, waiting for in-flight launches and terminations",
            capacity: 1,
            wait: Some("true"),
            spans_before_cancel: 0,
        },
        Row {
            case: "one instance, not waiting for in-flight launches and terminations",
            capacity: 1,
            wait: Some("false"),
            spans_before_cancel: 0,
        },
        Row {
            case: "eight instances, cancelled before any replacement came due",
            capacity: 8,
            wait: None,
            spans_before_cancel: 0,
        },
        Row {
            case: "eight instances, cancelled once replacements had been made",
            capacity: 8,
            wait: Some("false"),
            spans_before_cancel: 1,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One emulator per row: each row is its own account, so the ids of one row
        // never explain another's.
        let router = router_with(new_engine());
        let name = format!("asg-cancel-{index}");
        create_group(&router, &name, row.capacity).await;
        let started = start_refresh(&router, &name).await;
        if row.spans_before_cancel > 0 {
            tokio::time::sleep(one_span() * row.spans_before_cancel).await;
        }

        let mut params: Vec<(&str, &str)> = vec![("AutoScalingGroupName", name.as_str())];
        if let Some(wait) = row.wait {
            params.push(("WaitForTransitioningInstances", wait));
        }
        let (status, body) = query(&router, "CancelInstanceRefresh", &params).await;
        assert_eq!(
            status, 200,
            "case {:?}: CancelInstanceRefresh expected 200, got {status}: {body}",
            row.case
        );
        assert_eq!(
            tag_value(&body, "InstanceRefreshId"),
            started,
            "case {:?}: the answer names the refresh that was in progress, got {body}",
            row.case
        );

        // The cancellation is owed a span of its own, so a caller reading straight
        // after it still sees the transient status.
        let cancelling = newest_refresh(&router, &name).await;
        assert_eq!(
            cancelling.status, "Cancelling",
            "case {:?}: a just-cancelled refresh is being cancelled, got {cancelling:?}",
            row.case
        );
        assert_eq!(
            cancelling.instance_refresh_id, started,
            "case {:?}: the refresh keeps the id it was started under, got {cancelling:?}",
            row.case
        );

        // A refresh being cancelled is not in progress, so there is nothing left to
        // cancel — and it still holds its group, so no new refresh may start on it.
        let (status, body) = query(&router, "CancelInstanceRefresh", &params).await;
        assert_eq!(
            status, 400,
            "case {:?}: a second cancellation must be refused, got {status}: {body}",
            row.case
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ActiveInstanceRefreshNotFound",
            "case {:?}: expected the ActiveInstanceRefreshNotFound code, got {body}",
            row.case
        );
        let (status, body) = query(
            &router,
            "StartInstanceRefresh",
            &[("AutoScalingGroupName", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: a refresh being cancelled still holds its group, got {status}: {body}",
            row.case
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "InstanceRefreshInProgress",
            "case {:?}: expected the InstanceRefreshInProgress code, got {body}",
            row.case
        );

        // Once the cancellation has come due the refresh is cancelled, with exactly
        // the progress it had when it was cancelled: nothing was rolled back and
        // nothing further was replaced.
        // Two spans: the cancellation's own, and the one an in-flight launch or
        // termination it may have waited for.
        past_spans(2).await;
        let cancelled = newest_refresh(&router, &name).await;
        assert_eq!(
            cancelled.status, "Cancelled",
            "case {:?}: expected the refresh to be cancelled, got {cancelled:?}",
            row.case
        );
        assert!(
            !cancelled.end_time.is_empty(),
            "case {:?}: a cancelled refresh reports when it ended, got {cancelled:?}",
            row.case
        );
        assert_eq!(
            cancelled.instances_to_update, cancelling.instances_to_update,
            "case {:?}: a cancellation replaces nothing further, got {cancelled:?} \
             after {cancelling:?}",
            row.case
        );
        assert_eq!(
            cancelled.percentage_complete, cancelling.percentage_complete,
            "case {:?}: a cancellation rolls nothing back, got {cancelled:?} \
             after {cancelling:?}",
            row.case
        );

        // What the refresh had done when it was cancelled: nothing at all when no
        // replacement had come due, and part of the group when some had.
        let to_update: i32 = cancelling
            .instances_to_update
            .parse()
            .unwrap_or_else(|_| panic!("case {:?}: {cancelling:?} reports a count", row.case));
        let complete: i32 = cancelling
            .percentage_complete
            .parse()
            .unwrap_or_else(|_| panic!("case {:?}: {cancelling:?} reports a percentage", row.case));
        if row.spans_before_cancel == 0 {
            assert_eq!(
                to_update, row.capacity,
                "case {:?}: a refresh cancelled before replacing anything still had \
                 every instance to update, got {cancelling:?}",
                row.case
            );
            assert_eq!(
                complete, 0,
                "case {:?}: a refresh cancelled before replacing anything made no \
                 progress, got {cancelling:?}",
                row.case
            );
        } else {
            assert!(
                (1..row.capacity).contains(&to_update),
                "case {:?}: expected between 1 and {} instances left to update, \
                 got {cancelling:?}",
                row.case,
                row.capacity - 1
            );
            assert!(
                (1..100).contains(&complete),
                "case {:?}: expected the progress the replacements made, got {cancelling:?}",
                row.case
            );
        }

        // The group holds its capacity: a cancellation takes no instance away.
        let held = group_instance_ids(&router, &name).await;
        assert_eq!(
            held.len(),
            usize::try_from(row.capacity).expect("a capacity is a count"),
            "case {:?}: the group holds its capacity after the cancellation, got {held:?}",
            row.case
        );

        // A cancelled refresh is terminal, so the group is free to be refreshed again.
        let next = start_refresh(&router, &name).await;
        assert_ne!(
            next, started,
            "case {:?}: the refresh started after the cancellation is its own, got {next}",
            row.case
        );
    }
}

/// One row of the refusal table: the state the group's refreshes are in when the
/// cancellation arrives.
struct Refusal {
    case: &'static str,
    /// Whether a refresh is started at all before the cancellation.
    start_refresh: bool,
    /// Whether that refresh is left to finish before the cancellation.
    let_it_finish: bool,
}

/// A group with no refresh in progress has nothing to cancel, whichever way it got
/// there, and a group the account does not hold is refused by its name.
#[tokio::test]
async fn a_group_with_no_refresh_in_progress_has_nothing_to_cancel() {
    let rows = [
        Refusal {
            case: "a group nobody has ever refreshed",
            start_refresh: false,
            let_it_finish: false,
        },
        Refusal {
            case: "a group whose only refresh has succeeded",
            start_refresh: true,
            let_it_finish: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let router = router_with(new_engine());
        let name = format!("asg-nothing-to-cancel-{index}");
        create_group(&router, &name, 1).await;
        if row.start_refresh {
            start_refresh(&router, &name).await;
        }
        if row.let_it_finish {
            past_spans(3).await;
            let finished = newest_refresh(&router, &name).await;
            assert_eq!(
                finished.status, "Successful",
                "case {:?}: the refresh must have finished first, got {finished:?}",
                row.case
            );
        }

        let (status, body) = query(
            &router,
            "CancelInstanceRefresh",
            &[("AutoScalingGroupName", name.as_str())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status}: {body}",
            row.case
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ActiveInstanceRefreshNotFound",
            "case {:?}: expected the ActiveInstanceRefreshNotFound code, got {body}",
            row.case
        );
        let expected = format!("No active Instance Refresh found for Auto Scaling group {name}");
        assert_eq!(
            xml_unescape(&tag_value(&body, "Message")),
            expected,
            "case {:?}: expected the message {expected:?}, got {body}",
            row.case
        );
    }

    // A group the account does not hold is refused by its name, whatever the name
    // looks like: names differing in length, case and character set, each of which the
    // boundary validator admits, so only the handler can refuse them.
    let router = router_with(new_engine());
    for name in [
        "a",
        "asg-does-not-exist",
        "ASG-UPPER-CASE",
        "asg.name_with-separators 1",
        "グループ-ünïcode",
    ] {
        let (status, body) = query(
            &router,
            "CancelInstanceRefresh",
            &[("AutoScalingGroupName", name)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            tag_value(&body, "Code"),
            "ValidationError",
            "case {name:?}: expected the ValidationError code, got {body}"
        );
        let expected = format!(
            "AutoScalingGroup name not found - Auto Scaling group with name {name} not found"
        );
        assert_eq!(
            xml_unescape(&tag_value(&body, "Message")),
            expected,
            "case {name:?}: expected the message {expected:?}, got {body}"
        );
    }
}
