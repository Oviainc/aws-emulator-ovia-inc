// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a log group's deletion protection: `PutLogGroupDeletionProtection`,
//! the `deletionProtectionEnabled` `DescribeLogGroups` reports, and the refusal
//! `DeleteLogGroup` answers while a group is protected.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables below span the classes the rule generalises over: the forms a
//! `logGroupIdentifier` takes (name, both ARN forms), the state the group starts in
//! (protected at creation or not), the sequences of puts applied to it, and whether the
//! group holds streams and events when the delete is refused. Any single group an issue
//! happens to mention is one row.

use serde_json::{json, Value};

use crate::harness::{base_ms, Logs};

/// Log group names spanning the forms `LogGroupName`'s pattern
/// (`^[\.\-_/#A-Za-z0-9]+$`) admits: a path, a dotted name, a single character, and one
/// carrying every punctuation class the pattern allows.
const NAME_FORMS: [&str; 4] = [
    "/aws/lambda/protected-fn",
    "service.api.Protected",
    "p",
    "mixed-1_2.3/4#5",
];

/// The `deletionProtectionEnabled` the service reports for one log group.
async fn protection_of(logs: &Logs, identifier: &str) -> Option<bool> {
    logs.describe_group(identifier)
        .await
        .unwrap_or_else(|| panic!("case {identifier}: the log group must still exist"))
        .get("deletionProtectionEnabled")
        .and_then(Value::as_bool)
}

/// Put a deletion-protection state through one identifier form, asserting the empty 200
/// the `Unit` output carries.
async fn put_protection(logs: &Logs, identifier: &str, enabled: bool) {
    let body = logs
        .ok(
            "PutLogGroupDeletionProtection",
            &json!({"logGroupIdentifier": identifier, "deletionProtectionEnabled": enabled}),
        )
        .await;
    let members = match &body {
        Value::Null => 0,
        Value::Object(map) => map.len(),
        other => panic!("case {identifier}: a Unit output is an empty body, got {other}"),
    };
    assert_eq!(
        members, 0,
        "case {identifier} enabled {enabled}: a Unit output carries no members, got {body}"
    );
}

/// A protected log group refuses every delete with a 400 `InvalidParameterException` and
/// survives it; disabling protection lets the same delete through. Crossed over the
/// identifier form the protection was put through, the state the group was created in,
/// and whether the group holds a stream with events.
#[tokio::test]
async fn protection_blocks_the_delete_until_it_is_disabled() {
    let logs = Logs::new();

    // identifier form × created-protected × holds data. `LogGroupIdentifier`'s pattern
    // (`^[\w#+=/:,.@-]*$`) admits no `*`, so the `LogGroup$arn` form — the one carrying
    // the trailing `:*` — is not an identifier a put can name; the refusals test pins
    // that.
    let cases: [(&str, bool, bool); 4] = [
        ("name", false, false),
        ("name", true, true),
        ("logGroupArn", false, true),
        ("logGroupArn", true, false),
    ];

    for (index, (form, created_protected, holds_data)) in cases.into_iter().enumerate() {
        let name = format!("{}/case-{index}", NAME_FORMS[index % NAME_FORMS.len()]);
        let case = format!("{form} created_protected={created_protected} data={holds_data}");
        logs.ok(
            "CreateLogGroup",
            &json!({"logGroupName": name, "deletionProtectionEnabled": created_protected}),
        )
        .await;
        if holds_data {
            logs.ok(
                "CreateLogStream",
                &json!({"logGroupName": name, "logStreamName": "stream-1"}),
            )
            .await;
            logs.ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": name,
                    "logStreamName": "stream-1",
                    "logEvents": [{"timestamp": base_ms(), "message": "kept"}],
                }),
            )
            .await;
        }

        // "Indicates whether deletion protection is enabled for this log group": the
        // state the group was created in is the state it reports.
        assert_eq!(
            protection_of(&logs, &name).await,
            Some(created_protected),
            "case {case}: a new group reports the deletion protection it was created with"
        );

        let (_arn, log_group_arn) = logs.group_arns(&name).await;
        let identifier = if form == "name" {
            name.clone()
        } else {
            log_group_arn
        };

        // Enable protection through this identifier form, then read it back through the
        // NAME, so the flag is stored on the group and not under the string that named it.
        put_protection(&logs, &identifier, true).await;
        assert_eq!(
            protection_of(&logs, &name).await,
            Some(true),
            "case {case}: protection enabled through {form} is reported on the group"
        );

        // "deletion protection blocks all deletion operations until it is explicitly
        // disabled".
        let (status, body) = logs
            .call("DeleteLogGroup", &json!({"logGroupName": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a delete of a protected group is refused, got {status} {body}"
        );
        assert_eq!(
            crate::harness::error_code(&body),
            "InvalidParameterException",
            "case {case}: the refusal carries the modelled error code, got {body}"
        );

        // The refused delete removed nothing: the group, its streams and its events are
        // all still readable.
        assert_eq!(
            protection_of(&logs, &name).await,
            Some(true),
            "case {case}: a refused delete leaves the group in place"
        );
        if holds_data {
            let streams = logs
                .ok("DescribeLogStreams", &json!({"logGroupName": name}))
                .await;
            assert_eq!(
                streams["logStreams"].as_array().map(Vec::len),
                Some(1),
                "case {case}: a refused delete leaves the group's streams in place, got {streams}"
            );
            let page = logs
                .ok(
                    "GetLogEvents",
                    &json!({"logGroupName": name, "logStreamName": "stream-1"}),
                )
                .await;
            assert_eq!(
                page["events"].as_array().map(Vec::len),
                Some(1),
                "case {case}: a refused delete leaves the group's events in place, got {page}"
            );
        }

        // Explicitly disabling protection is what unblocks the delete — the documented
        // way to remove a protected log group.
        put_protection(&logs, &identifier, false).await;
        assert_eq!(
            protection_of(&logs, &name).await,
            Some(false),
            "case {case}: protection disabled through {form} is reported on the group"
        );
        logs.ok("DeleteLogGroup", &json!({"logGroupName": name}))
            .await;
        assert_eq!(
            logs.describe_group(&name).await,
            None,
            "case {case}: the group is gone once protection is disabled and it is deleted"
        );
    }
}

/// A put records the state it names, however many times it is repeated, and changes
/// nothing else about the group.
#[tokio::test]
async fn a_later_put_replaces_the_state_and_nothing_else() {
    let logs = Logs::new();
    let group = "repeated.puts/group";
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let before = logs
        .describe_group(group)
        .await
        .expect("the group was just created");

    // Both transitions, and each state put twice in a row, so a repeated put is pinned
    // as a no-op rather than a toggle.
    for enabled in [true, true, false, false, true, false] {
        put_protection(&logs, group, enabled).await;
        let after = logs
            .describe_group(group)
            .await
            .expect("a put does not delete the group");
        assert_eq!(
            after.get("deletionProtectionEnabled").and_then(Value::as_bool),
            Some(enabled),
            "case enabled {enabled}: a put records exactly the state it names, got {after}"
        );
        for member in [
            "logGroupName",
            "creationTime",
            "arn",
            "logGroupArn",
            "logGroupClass",
            "storedBytes",
            "metricFilterCount",
            "retentionInDays",
        ] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case enabled {enabled}: a put leaves {member} unchanged, got {after}"
            );
        }
    }
}

/// Deletion protection belongs to the group it was put on: protecting one group blocks
/// only that group's delete, whatever order the groups were created in.
#[tokio::test]
async fn protection_is_per_log_group() {
    let logs = Logs::new();
    let groups = ["sibling-a", "sibling.b", "sibling/c", "sibling#d"];
    for group in groups {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    // Protect every other group, so a protected and an unprotected group sit on both
    // sides of each other in name order.
    let protected: Vec<&str> = groups.iter().copied().step_by(2).collect();
    for group in &protected {
        put_protection(&logs, group, true).await;
    }

    for group in groups {
        let expected = protected.contains(&group);
        assert_eq!(
            protection_of(&logs, group).await,
            Some(expected),
            "case {group}: a put on one group changes no other group's state"
        );
        let (status, body) = logs
            .call("DeleteLogGroup", &json!({"logGroupName": group}))
            .await;
        if expected {
            assert_eq!(
                status, 400,
                "case {group}: the protected group refuses its delete, got {status} {body}"
            );
            assert_eq!(
                crate::harness::error_code(&body),
                "InvalidParameterException",
                "case {group}: the refusal carries the modelled error code, got {body}"
            );
            assert!(
                logs.describe_group(group).await.is_some(),
                "case {group}: the protected group survives its refused delete"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {group}: an unprotected group is deleted, got {status} {body}"
            );
            assert_eq!(
                logs.describe_group(group).await,
                None,
                "case {group}: the unprotected group is gone after its delete"
            );
        }
    }
}

/// A put naming a log group that does not exist is refused with
/// `ResourceNotFoundException`, in every identifier form, and creates nothing.
#[tokio::test]
async fn a_put_on_an_unknown_log_group_is_refused() {
    let logs = Logs::new();
    let present = "present-group";
    logs.ok("CreateLogGroup", &json!({"logGroupName": present}))
        .await;
    let (arn, log_group_arn) = logs.group_arns(present).await;

    let unknown = [
        ("a name nobody created", "no-such-group".to_string()),
        (
            "a name that differs only in case",
            "Present-Group".to_string(),
        ),
        (
            "an ARN form naming no group",
            log_group_arn.replace(present, "absent/group"),
        ),
    ];

    for (case, identifier) in unknown {
        for enabled in [true, false] {
            let code = logs
                .err(
                    "PutLogGroupDeletionProtection",
                    &json!({
                        "logGroupIdentifier": identifier,
                        "deletionProtectionEnabled": enabled,
                    }),
                )
                .await;
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case} ({identifier}) enabled {enabled}: protection is set on a group that exists, got {code}"
            );
        }
        assert_eq!(
            logs.describe_group(&identifier).await,
            None,
            "case {case} ({identifier}): a refused put does not create the group"
        );
    }

    // `LogGroupIdentifier`'s pattern admits no `*`, so the `LogGroup$arn` form is
    // refused by the modelled constraint before this operation's own checks run.
    let (status, body) = logs
        .call(
            "PutLogGroupDeletionProtection",
            &json!({"logGroupIdentifier": arn, "deletionProtectionEnabled": true}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case the wildcard ARN form ({arn}): it is outside logGroupIdentifier's pattern, got {status} {body}"
    );
    assert_eq!(
        crate::harness::error_code(&body),
        "ValidationException",
        "case the wildcard ARN form ({arn}): a modelled constraint refusal, got {body}"
    );

    // The group that does exist is untouched by the refusals.
    assert_eq!(
        protection_of(&logs, present).await,
        Some(false),
        "case after the refusals: a refused put persists nothing"
    );
}
