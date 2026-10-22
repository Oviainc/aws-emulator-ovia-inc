// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a log group's retention: `PutRetentionPolicy` and the retention
//! `DescribeLogGroups` reports for the group it was set on.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables below span the class the rule generalises over: the whole documented
//! catalogue of retentions, the forms a log group name takes, repeated puts on one
//! group, and the values outside the catalogue. Any single `retentionInDays` an issue
//! happens to mention is one row.

use serde_json::{json, Value};

use crate::harness::{Logs, REGION};

/// Every retention a log group can be set to. Source: the model's `Days` shape — "The
/// number of days to retain the log events in the specified log group. Possible values
/// are: 1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192,
/// 2557, 2922, 3288, and 3653."
const RETENTIONS: [i64; 22] = [
    1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922,
    3288, 3653,
];

/// Log group names spanning the forms `LogGroupName`'s pattern
/// (`^[\.\-_/#A-Za-z0-9]+$`) admits: a path, a dotted name, a single character, and one
/// carrying every punctuation class the pattern allows.
const NAME_FORMS: [&str; 4] = [
    "/aws/bedrock-agentcore/runtimes/test-DEFAULT",
    "service.api.Logs",
    "g",
    "mixed-1_2.3/4#5",
];

/// The one log group the service reports under `name`, read back through
/// `DescribeLogGroups`.
async fn group_of(logs: &Logs, name: &str) -> Value {
    let page = logs
        .ok(
            "DescribeLogGroups",
            &json!({"logGroupIdentifiers": [name]}),
        )
        .await;
    let groups = page
        .get("logGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("DescribeLogGroups must return logGroups, got {page}"))
        .clone();
    assert_eq!(
        groups.len(),
        1,
        "case {name}: exactly one log group is reported under that name, got {groups:?}"
    );
    groups.into_iter().next().expect("the one group")
}

/// The `retentionInDays` the service reports for one log group, absent when the group has
/// no retention policy.
async fn retention_of(logs: &Logs, name: &str) -> Option<i64> {
    group_of(logs, name)
        .await
        .get("retentionInDays")
        .and_then(Value::as_i64)
}

/// Create a log group, which is what a retention policy is set on.
async fn create_group(logs: &Logs, name: &str) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": name}))
        .await;
}

/// Every retention in the documented catalogue is accepted on every form of log group
/// name, answered with an empty 200, and reported back by `DescribeLogGroups` for that
/// group only.
#[tokio::test]
async fn every_documented_retention_round_trips() {
    let logs = Logs::new();

    for (index, days) in RETENTIONS.iter().enumerate() {
        // Cross the catalogue with the name forms, so neither the value nor the name
        // shape can be what makes a case pass.
        let form = NAME_FORMS[index % NAME_FORMS.len()];
        let group = format!("{form}/retention-{index}");
        create_group(&logs, &group).await;

        // "Log events do not expire if they belong to log groups without a retention
        // policy": a brand-new group reports none.
        assert_eq!(
            retention_of(&logs, &group).await,
            None,
            "case {group}: a group with no retention policy reports no retentionInDays"
        );

        let (status, body) = logs
            .call(
                "PutRetentionPolicy",
                &json!({"logGroupName": group, "retentionInDays": days}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {group} retentionInDays {days}: expected HTTP 200, got {status} {body}"
        );
        // The output shape is `Unit`, so a success carries no members.
        let members = match &body {
            Value::Null => 0,
            Value::Object(map) => map.len(),
            other => panic!("case {group}: a Unit output is an empty body, got {other}"),
        };
        assert_eq!(
            members, 0,
            "case {group} retentionInDays {days}: a Unit output carries no members, got {body}"
        );

        assert_eq!(
            retention_of(&logs, &group).await,
            Some(*days),
            "case {group}: the retention just set must be reported back"
        );
    }
}

/// A put replaces whatever retention the group was at, and touches nothing else about
/// the group: its creation time, ARNs, class and stored bytes survive every put.
#[tokio::test]
async fn a_later_put_replaces_the_retention_and_nothing_else() {
    let logs = Logs::new();
    let group = "replaced.retention/group";
    create_group(&logs, group).await;

    let before = group_of(&logs, group).await;
    // Walk the catalogue down and then back up, so a case is never only an increase, and
    // repeat one value to pin that a repeated put is not a change.
    let sequence: Vec<i64> = RETENTIONS
        .iter()
        .rev()
        .copied()
        .chain(RETENTIONS.iter().copied())
        .chain([RETENTIONS[0], RETENTIONS[0]])
        .collect();

    for days in sequence {
        logs.ok(
            "PutRetentionPolicy",
            &json!({"logGroupName": group, "retentionInDays": days}),
        )
        .await;
        let after = group_of(&logs, group).await;
        assert_eq!(
            after.get("retentionInDays").and_then(Value::as_i64),
            Some(days),
            "case retentionInDays {days}: a put replaces the retention the group was at, got {after}"
        );
        for member in [
            "logGroupName",
            "creationTime",
            "arn",
            "logGroupArn",
            "logGroupClass",
            "storedBytes",
            "metricFilterCount",
        ] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case retentionInDays {days}: a put leaves {member} unchanged, got {after}"
            );
        }
    }
}

/// The retention belongs to the group it was set on: a put on one group changes no other
/// group's retention, whatever order the groups were created in.
#[tokio::test]
async fn retention_is_per_log_group() {
    let logs = Logs::new();
    let groups = ["sibling-a", "sibling.b", "sibling/c"];
    for group in groups {
        create_group(&logs, group).await;
    }

    // One distinct retention per group, taken from the documented catalogue.
    let assigned: Vec<(&str, i64)> = groups
        .iter()
        .zip([RETENTIONS[1], RETENTIONS[9], RETENTIONS[21]])
        .map(|(group, days)| (*group, days))
        .collect();
    for (group, days) in &assigned {
        logs.ok(
            "PutRetentionPolicy",
            &json!({"logGroupName": group, "retentionInDays": days}),
        )
        .await;
    }
    for (group, days) in &assigned {
        assert_eq!(
            retention_of(&logs, group).await,
            Some(*days),
            "case {group}: each group keeps the retention put on it"
        );
    }

    // A group that was never given a retention still has none.
    create_group(&logs, "sibling-d").await;
    assert_eq!(
        retention_of(&logs, "sibling-d").await,
        None,
        "case sibling-d: a group nobody put a retention on reports none"
    );
}

/// What a put refuses: a log group that does not exist, and a `retentionInDays` outside
/// the documented catalogue — including each neighbour of a published value, the values
/// one past each end of the catalogue, and a non-positive one. A refused put stores
/// nothing.
#[tokio::test]
async fn requests_that_cannot_be_stored_are_refused() {
    let logs = Logs::new();
    let group = "refused-retention";
    create_group(&logs, group).await;
    logs.ok(
        "PutRetentionPolicy",
        &json!({"logGroupName": group, "retentionInDays": RETENTIONS[3]}),
    )
    .await;

    // "Sets the retention of the specified log group": there is no such group.
    for (case, name) in [
        ("a name nobody created", "no-such-group"),
        ("a name that differs only in case", "Refused-Retention"),
    ] {
        assert_eq!(
            logs.err(
                "PutRetentionPolicy",
                &json!({"logGroupName": name, "retentionInDays": RETENTIONS[0]}),
            )
            .await,
            "ResourceNotFoundException",
            "case {case} ({name}): a retention is set on a log group that exists"
        );
    }

    // Each published bound of the catalogue, and one past it, plus the neighbours of
    // values inside it. `Days` is a plain integer in the model, so only this operation
    // can refuse them.
    let lowest = RETENTIONS[0];
    let highest = RETENTIONS[RETENTIONS.len() - 1];
    let mut rejected = vec![
        0,
        -1,
        lowest - 1,
        highest + 1,
        highest * 2,
        i64::from(i32::MAX),
    ];
    // Every integer up to the catalogue's third entry that the catalogue does not list,
    // so a neighbour of a published value on either side is covered.
    rejected.extend((lowest..=RETENTIONS[2]).filter(|days| !RETENTIONS.contains(days)));
    rejected.extend([100, 364, 366, 3652]);

    for days in rejected {
        assert_eq!(
            logs.err(
                "PutRetentionPolicy",
                &json!({"logGroupName": group, "retentionInDays": days}),
            )
            .await,
            "InvalidParameterException",
            "case retentionInDays {days}: it is not one of the documented retentions"
        );
    }

    // Every published value IS accepted, so the refusal above is the catalogue and not a
    // blanket rejection.
    for days in RETENTIONS {
        logs.ok(
            "PutRetentionPolicy",
            &json!({"logGroupName": group, "retentionInDays": days}),
        )
        .await;
    }

    // A refused put changes nothing: the group is still at the last retention a put
    // accepted, and the group nobody could name was not created.
    assert_eq!(
        retention_of(&logs, group).await,
        Some(highest),
        "case after the refusals: a refused put persists nothing"
    );
    let page = logs
        .ok(
            "DescribeLogGroups",
            &json!({"logGroupIdentifiers": ["no-such-group"]}),
        )
        .await;
    assert_eq!(
        page.get("logGroups").and_then(Value::as_array),
        Some(&Vec::new()),
        "case after the refusals: a put on an unknown group does not create it, got {page}"
    );
}

/// A retention put through the name is readable back through the group's ARN, the other
/// identifier `DescribeLogGroups` accepts, so it is stored on the group itself and not
/// under the string the put named.
#[tokio::test]
async fn retention_is_readable_through_the_group_arn() {
    let logs = Logs::new();
    let group = "arn-addressed-group";
    create_group(&logs, group).await;
    logs.ok(
        "PutRetentionPolicy",
        &json!({"logGroupName": group, "retentionInDays": RETENTIONS[5]}),
    )
    .await;

    let arn = group_of(&logs, group)
        .await
        .get("logGroupArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {group}: a log group carries a logGroupArn"))
        .to_string();
    assert!(
        arn.contains(REGION),
        "case {group}: the group's ARN names the request's region, got {arn}"
    );
    assert_eq!(
        retention_of(&logs, &arn).await,
        Some(RETENTIONS[5]),
        "case {group}: the retention is readable back through the group's ARN"
    );
}
