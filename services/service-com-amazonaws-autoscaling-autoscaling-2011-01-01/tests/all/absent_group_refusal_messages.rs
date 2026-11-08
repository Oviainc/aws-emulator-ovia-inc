// SPDX-License-Identifier: CC0-1.0

//! The refusal a mutation naming an Auto Scaling group that does not exist answers,
//! over the awsQuery wire.
//!
//! The rule under test: a refusal message carries a value from the request ONLY where
//! the service fills that part of the message from the request — so
//! `UpdateAutoScalingGroup`'s absent-group `ValidationError` message is one constant
//! sentence whose trailing detail field is the unfilled token, identical for every
//! group name and every set of members the update carries, while the sibling mutations
//! that DO name the group in that field keep naming it (each in the detail form IT
//! serves — see `client_error_refusal_wording.rs` for the whole set).
//!
//! The tables below therefore span what the rule generalises over — the name's length,
//! character set, case and spelling (including a name spelled like the message's own
//! detail token), whether the name never existed or was created and deleted, and which
//! members the update supplies — and assert the whole message of every row, so a row
//! cannot pass by refusing for a different reason.

use crate::support::{new_engine, query, router_with, tag_value, xml_unescape};

/// The launch source every group in this suite is created from — a launch TEMPLATE,
/// because no account can create a launch configuration (see
/// `launch_configurations_are_unavailable.rs`).
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// The whole message Amazon EC2 Auto Scaling answers an `UpdateAutoScalingGroup` that
/// names no stored group with: fixed text, the detail after the dash left unfilled.
const ABSENT_GROUP_MESSAGE: &str = "AutoScalingGroup name not found - null";

/// Names a client can submit for a group that does not exist. They differ in length,
/// character set, case, separators and spelling; `AutoScalingGroupName` is
/// `XmlStringMaxLen255` (len 1..255, pattern
/// `^[\u{0020}-\u{D7FF}\u{E000}-\u{FFFD}\u{D800}\u{DC00}-\u{DBFF}\u{DFFF}\r\n\t]*$`,
/// `smithy_query(query="operation", operation="UpdateAutoScalingGroup")`), so every row
/// here is a name the boundary validator accepts and only the handler can refuse.
fn absent_group_names() -> Vec<(&'static str, String)> {
    let mut names: Vec<(&'static str, String)> = [
        ("the shortest name the model allows", "a"),
        (
            "a name of the shape the recorded calls used",
            "sim2real-behav-as-grp-0123456789ab",
        ),
        ("an upper-case name", "ASG-UPPER-CASE"),
        (
            "a name carrying every separator the model allows",
            "asg.name_with-separators 1",
        ),
        ("a name spelled like the message's own detail token", "null"),
        ("a name spelled like the error code", "ValidationError"),
        (
            "a name carrying XML metacharacters",
            "<asg & 'name' \"quoted\">",
        ),
        ("a non-ASCII name", "グループ-ünïcode"),
        ("a name that is only digits", "1234567890123456"),
    ]
    .into_iter()
    .map(|(case, name)| (case, name.to_string()))
    .collect();
    // The longest name the member's length constraint admits.
    names.push(("the longest name the model allows", "z".repeat(255)));
    names
}

/// The member sets an update can carry beyond the group name, including ones whose own
/// bound checks would refuse the request if the group existed: the absent group is
/// refused first, so the message is the same for all of them.
fn update_member_sets() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        ("the name alone", vec![]),
        ("a new MaxSize", vec![("MaxSize", "2")]),
        ("a new MinSize", vec![("MinSize", "3")]),
        ("a new DesiredCapacity", vec![("DesiredCapacity", "1")]),
        (
            "every bound at once",
            vec![("MinSize", "1"), ("MaxSize", "5"), ("DesiredCapacity", "2")],
        ),
        (
            "a new HealthCheckType",
            vec![("HealthCheckType", "ELB"), ("HealthCheckGracePeriod", "60")],
        ),
        (
            "bounds no group could hold, which only a stored group is checked against",
            vec![("MinSize", "-1"), ("MaxSize", "-7")],
        ),
        (
            "new placement",
            vec![("AvailabilityZones.member.1", "us-west-2a")],
        ),
    ]
}

/// One sibling mutation this suite drives: the case, the action, and the members the
/// request carries beyond the group name.
type SiblingCase<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);

/// Post one update and return the status, the wire code and the message.
async fn update(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    members: &[(&str, &str)],
) -> (u16, String, String) {
    let mut params: Vec<(&str, &str)> = vec![("AutoScalingGroupName", name)];
    params.extend_from_slice(members);
    let (status, body) = query(router, "UpdateAutoScalingGroup", &params).await;
    (
        status,
        tag_value(&body, "Code"),
        xml_unescape(&tag_value(&body, "Message")),
    )
}

/// Every name, crossed with every member set: HTTP 400, `ValidationError`, and the one
/// constant message — no part of it taken from the request.
#[tokio::test]
async fn an_update_of_an_absent_group_answers_one_constant_message() {
    let router = router_with(new_engine());

    for (name_case, name) in absent_group_names() {
        for (members_case, members) in update_member_sets() {
            let case = format!("{name_case} with {members_case}");
            let (status, code, message) = update(&router, &name, &members).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {message:?}"
            );
            assert_eq!(
                code, "ValidationError",
                "case {case:?}: expected the ValidationError code, got {code:?}"
            );
            // Equality with the constant is what says no part of the request reached
            // the message: every row submits a different name and every row reports
            // this same sentence.
            assert_eq!(
                message, ABSENT_GROUP_MESSAGE,
                "case {case:?}: expected the message {ABSENT_GROUP_MESSAGE:?}, got {message:?}"
            );
        }
    }
}

/// A group that EXISTS is still updated — the refusal did not swallow the happy path —
/// and once it is deleted the same name answers the constant refusal, which is the
/// sequence a teardown probe walks.
#[tokio::test]
async fn a_stored_group_updates_and_its_deleted_name_is_refused() {
    let router = router_with(new_engine());

    // Names a create accepts: `CreateAutoScalingGroup` admits only ASCII 33..=126
    // without a colon, so these are the creatable corner of the table above.
    for name in ["asg-1", "null", "ASG.name_with-separators"] {
        let (status, body) = query(
            &router,
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
            "case {name:?}: the create must succeed, got {status} with {body:?}"
        );

        let (status, code, message) = update(&router, name, &[("MaxSize", "4")]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the update of a stored group must succeed, got {status} with \
             {code:?} {message:?}"
        );

        // Read the update back through the implemented reader: the new bound is stored,
        // not just accepted.
        let (status, body) = query(
            &router,
            "DescribeAutoScalingGroups",
            &[("AutoScalingGroupNames.member.1", name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the read-back must succeed, got {status} with {body:?}"
        );
        assert_eq!(
            tag_value(&body, "MaxSize"),
            "4",
            "case {name:?}: expected the updated MaxSize 4, got {body:?}"
        );

        let (status, body) = query(
            &router,
            "DeleteAutoScalingGroup",
            &[("AutoScalingGroupName", name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the delete must succeed, got {status} with {body:?}"
        );

        let (status, code, message) = update(&router, name, &[("MaxSize", "2")]).await;
        assert_eq!(
            status, 400,
            "case {name:?}: the update of the deleted name must be refused, got {status} with \
             {message:?}"
        );
        assert_eq!(
            code, "ValidationError",
            "case {name:?}: expected the ValidationError code, got {code:?}"
        );
        assert_eq!(
            message, ABSENT_GROUP_MESSAGE,
            "case {name:?}: expected the message {ABSENT_GROUP_MESSAGE:?}, got {message:?}"
        );
    }
}

/// The scope of the rule: the sibling mutations whose absent-group message the service
/// DOES fill with the group name keep naming it, so each of their refusals still says
/// which name was missing.
#[tokio::test]
async fn the_sibling_mutations_still_name_the_missing_group() {
    let router = router_with(new_engine());

    let siblings: [SiblingCase<'_>; 2] = [
        (
            "a delete naming no such group",
            "DeleteAutoScalingGroup",
            vec![],
        ),
        (
            "a forced delete naming no such group",
            "DeleteAutoScalingGroup",
            vec![("ForceDelete", "true")],
        ),
    ];

    for (case, action, extra) in siblings {
        for (name_case, name) in absent_group_names() {
            let case = format!("{case} ({name_case})");
            let mut params: Vec<(&str, &str)> = vec![("AutoScalingGroupName", name.as_str())];
            params.extend_from_slice(&extra);
            let (status, body) = query(&router, action, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body:?}"
            );
            assert_eq!(
                tag_value(&body, "Code"),
                "ValidationError",
                "case {case:?}: expected the ValidationError code, got {body:?}"
            );
            let message = xml_unescape(&tag_value(&body, "Message"));
            // The WHOLE message, not a substring: a name spelled like a part of the
            // sentence would satisfy a `contains` check even against a handler that had
            // stopped naming the group.
            let expected =
                format!("AutoScalingGroup name not found - AutoScalingGroup '{name}' not found");
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?}"
            );
        }
    }
}
