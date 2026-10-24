// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a log group's retention: `PutRetentionPolicy`,
//! `DeleteRetentionPolicy`, and the retention `DescribeLogGroups` reports for the group it
//! was set on.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables below span the class the rule generalises over: the whole documented
//! catalogue of retentions, the forms a log group name takes, repeated puts and deletes on
//! one group, and the values outside the catalogue. Any single `retentionInDays` an issue
//! happens to mention is one row.
//!
//! The delete rule under test: `DeleteRetentionPolicy` puts the log group it names back in
//! the state a group with no retention policy is in — no `retentionInDays` reported and
//! its log events no longer expiring — whatever retention it held, changing nothing else
//! about the group, and refusing only a log group that does not exist.

use serde_json::{json, Value};

use crate::harness::{group_with_stream, now_ms, Logs, REGION};

/// Milliseconds in one day, the unit `retentionInDays` is expressed in.
const DAY: i64 = 24 * 60 * 60 * 1_000;

/// The published past bound of the ingestion window, from the `PutLogEvents` doc: "Events
/// older than 14 days or preceding the log group's retention period are rejected while
/// processing remaining valid events." A retention shorter than this is the binding bound.
const MAX_EVENT_AGE: i64 = 14 * DAY;

/// How far clear of a bound a case that is not testing the bound itself is placed, so no
/// case is decided by how long a request took.
const MARGIN: i64 = 5_000;

/// The members of a described log group that a retention call must leave alone.
const UNTOUCHED_MEMBERS: [&str; 7] = [
    "logGroupName",
    "creationTime",
    "arn",
    "logGroupArn",
    "logGroupClass",
    "storedBytes",
    "metricFilterCount",
];

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
        for member in UNTOUCHED_MEMBERS {
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

/// Whatever retention from the documented catalogue a group holds, deleting its retention
/// policy clears it: the group reports no `retentionInDays` afterwards, the call is
/// answered with an empty 200, and nothing else about the group moves. Crossed with the
/// forms a log group name takes, so neither the value nor the name shape is what makes a
/// case pass.
#[tokio::test]
async fn deleting_the_retention_clears_every_documented_retention() {
    let logs = Logs::new();

    for (index, days) in RETENTIONS.iter().enumerate() {
        let form = NAME_FORMS[index % NAME_FORMS.len()];
        let group = format!("{form}/cleared-{index}");
        create_group(&logs, &group).await;
        logs.ok(
            "PutRetentionPolicy",
            &json!({"logGroupName": group, "retentionInDays": days}),
        )
        .await;
        assert_eq!(
            retention_of(&logs, &group).await,
            Some(*days),
            "case {group}: the retention to be deleted is set first"
        );
        let before = group_of(&logs, &group).await;

        let (status, body) = logs
            .call("DeleteRetentionPolicy", &json!({"logGroupName": group}))
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

        let after = group_of(&logs, &group).await;
        assert_eq!(
            after.get("retentionInDays"),
            None,
            "case {group} retentionInDays {days}: a group without a retention policy reports no retentionInDays, got {after}"
        );
        for member in UNTOUCHED_MEMBERS {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {group} retentionInDays {days}: a delete leaves {member} unchanged, got {after}"
            );
        }
    }
}

/// Deleting a retention a group does not hold is a success too: the call names an end
/// state the group is already in. Spans the ways a group comes to hold none — never given
/// one, and one whose retention a previous delete already cleared — and repeated deletes.
#[tokio::test]
async fn deleting_a_retention_a_group_does_not_hold_is_a_success() {
    // (case, the retention to put before the deletes, how many deletes to issue)
    let cases: [(&str, Option<i64>, usize); 5] = [
        ("a group nobody put a retention on", None, 1),
        ("repeated deletes on a group with no retention", None, 4),
        ("a delete after one that cleared the retention", Some(RETENTIONS[0]), 2),
        ("many deletes after the longest retention", Some(RETENTIONS[21]), 3),
        ("many deletes after a mid-catalogue retention", Some(RETENTIONS[11]), 5),
    ];

    for (case, put_first, deletes) in cases {
        let logs = Logs::new();
        let group = "idempotent.delete/group";
        create_group(&logs, group).await;
        if let Some(days) = put_first {
            logs.ok(
                "PutRetentionPolicy",
                &json!({"logGroupName": group, "retentionInDays": days}),
            )
            .await;
        }
        let before = group_of(&logs, group).await;

        for round in 1..=deletes {
            let (status, body) = logs
                .call("DeleteRetentionPolicy", &json!({"logGroupName": group}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} delete {round}: expected HTTP 200, got {status} {body}"
            );
            let after = group_of(&logs, group).await;
            assert_eq!(
                after.get("retentionInDays"),
                None,
                "case {case:?} delete {round}: the group holds no retention, got {after}"
            );
            for member in UNTOUCHED_MEMBERS {
                assert_eq!(
                    after.get(member),
                    before.get(member),
                    "case {case:?} delete {round}: {member} is unchanged, got {after}"
                );
            }
        }
    }
}

/// The retention a delete clears belongs to the group it named: every other group keeps
/// the retention put on it, whichever group of the set is deleted.
#[tokio::test]
async fn deleting_one_groups_retention_leaves_every_other_group() {
    let groups = ["peer-a", "peer.b", "peer/c", "peer#d"];
    // One distinct retention per group, taken from the documented catalogue.
    let assigned: Vec<(&str, i64)> = groups
        .iter()
        .zip([RETENTIONS[0], RETENTIONS[4], RETENTIONS[13], RETENTIONS[21]])
        .map(|(group, days)| (*group, days))
        .collect();

    // Delete each group's retention in turn, from a fresh store each time, so the case is
    // not decided by the order the groups were created in.
    for (deleted, _) in &assigned {
        let logs = Logs::new();
        for (group, days) in &assigned {
            create_group(&logs, group).await;
            logs.ok(
                "PutRetentionPolicy",
                &json!({"logGroupName": group, "retentionInDays": days}),
            )
            .await;
        }

        logs.ok("DeleteRetentionPolicy", &json!({"logGroupName": deleted}))
            .await;

        for (group, days) in &assigned {
            let expected = if group == deleted { None } else { Some(*days) };
            assert_eq!(
                retention_of(&logs, group).await,
                expected,
                "case deleting {deleted}: expected {group} to report {expected:?}"
            );
        }
    }
}

/// A delete and a put alternate without limit: the group's retention follows the last call
/// in the sequence, whatever retentions the sequence walks through.
#[tokio::test]
async fn a_retention_can_be_put_again_after_it_was_deleted() {
    let logs = Logs::new();
    let group = "re-put.retention/group";
    create_group(&logs, group).await;

    // `Some(days)` is a put of that retention, `None` a delete. The sequence walks the
    // catalogue up and down with deletes interleaved, and ends on a delete.
    let mut sequence: Vec<Option<i64>> = Vec::new();
    for days in RETENTIONS {
        sequence.push(Some(days));
        sequence.push(None);
    }
    for days in RETENTIONS.iter().rev() {
        sequence.push(Some(*days));
    }
    sequence.push(Some(RETENTIONS[0]));
    sequence.push(None);

    for (step, call) in sequence.iter().enumerate() {
        match call {
            Some(days) => {
                logs.ok(
                    "PutRetentionPolicy",
                    &json!({"logGroupName": group, "retentionInDays": days}),
                )
                .await;
            }
            None => {
                logs.ok("DeleteRetentionPolicy", &json!({"logGroupName": group}))
                    .await;
            }
        }
        assert_eq!(
            retention_of(&logs, group).await,
            *call,
            "case step {step} ({call:?}): the group reports what the last call left"
        );
    }
}

/// What a delete refuses: a log group that does not exist — the policy it would clear is a
/// property of a group. A refused delete creates nothing and clears nothing.
#[tokio::test]
async fn deleting_the_retention_of_an_unknown_log_group_is_refused() {
    let logs = Logs::new();
    let group = "refused-delete";
    create_group(&logs, group).await;
    logs.ok(
        "PutRetentionPolicy",
        &json!({"logGroupName": group, "retentionInDays": RETENTIONS[7]}),
    )
    .await;

    for (case, name) in [
        ("a name nobody created", "no-such-group"),
        ("a name that differs only in case", "Refused-Delete"),
        ("a name that extends an existing one", "refused-delete/child"),
        ("a name that is a prefix of an existing one", "refused"),
    ] {
        assert_eq!(
            logs.err("DeleteRetentionPolicy", &json!({"logGroupName": name}))
                .await,
            "ResourceNotFoundException",
            "case {case} ({name}): a retention policy is deleted on a log group that exists"
        );
        assert_eq!(
            logs.describe_group(name).await,
            None,
            "case {case} ({name}): a refused delete does not create the group"
        );
    }

    assert_eq!(
        retention_of(&logs, group).await,
        Some(RETENTIONS[7]),
        "case after the refusals: the existing group keeps its retention"
    );
}

/// The retention a delete clears stops binding ingestion: "Log events do not expire if
/// they belong to log groups without a retention policy." An event inside the 14-day
/// ingestion window but before the group's retention period is refused as expired while
/// the retention is set, and ingested once it is deleted. Rows span retentions shorter
/// than the ingestion window, which are the ones that bind.
#[tokio::test]
async fn events_before_a_deleted_retention_are_ingested_again() {
    // Retentions from the documented catalogue that are shorter than the 14-day ingestion
    // window, so the retention — not the window — is what refuses the event.
    for days in [RETENTIONS[0], RETENTIONS[1], RETENTIONS[2], RETENTIONS[3]] {
        let logs = Logs::new();
        let group = format!("expiry-{days}");
        let stream = "stream";
        group_with_stream(&logs, &group, stream, Some(days)).await;

        let timestamp = now_ms() - days * DAY - MARGIN;
        assert!(
            timestamp > now_ms() - MAX_EVENT_AGE,
            "case retention {days}: the event is inside the 14-day ingestion window"
        );

        let response = logs
            .ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": group,
                    "logStreamName": stream,
                    "logEvents": [{"timestamp": timestamp, "message": "before the retention"}],
                }),
            )
            .await;
        assert_eq!(
            response.get("rejectedLogEventsInfo"),
            Some(&json!({"expiredLogEventEndIndex": 1})),
            "case retention {days}: an event before the retention period is expired, got {response}"
        );

        logs.ok("DeleteRetentionPolicy", &json!({"logGroupName": group}))
            .await;

        let response = logs
            .ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": group,
                    "logStreamName": stream,
                    "logEvents": [{"timestamp": timestamp, "message": "after the delete"}],
                }),
            )
            .await;
        assert_eq!(
            response.get("rejectedLogEventsInfo"),
            None,
            "case retention {days}: with no retention policy the same event is ingested, got {response}"
        );

        let page = logs
            .ok(
                "GetLogEvents",
                &json!({"logGroupName": group, "logStreamName": stream, "startFromHead": true}),
            )
            .await;
        let messages: Vec<&str> = page
            .get("events")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("GetLogEvents must return events, got {page}"))
            .iter()
            .filter_map(|event| event.get("message").and_then(Value::as_str))
            .collect();
        assert_eq!(
            messages,
            vec!["after the delete"],
            "case retention {days}: exactly the event ingested after the delete is readable"
        );
    }
}
