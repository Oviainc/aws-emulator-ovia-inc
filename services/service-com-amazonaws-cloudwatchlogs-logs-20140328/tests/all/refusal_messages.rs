// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal messages whose producing rule is known, so the text
//! is part of the contract and not merely one recorded string:
//!
//! * a refusal that a resource policy the request names is not there is BUILT FROM THE
//!   REQUEST — it carries the requested `policyName`, so a caller deleting policies in a
//!   loop can attribute the refusal to one name (`DeleteResourcePolicy`);
//! * a refusal of a value outside the set a member's own documentation enumerates
//!   declares the value invalid and then LISTS THAT SET, so the refusal tells a caller
//!   which values it may use (`PutRetentionPolicy$retentionInDays`,
//!   `PutSubscriptionFilter$emitSystemFields`).
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! message that is not computed from the request and the stored policy cannot pass.
//!
//! The tables below span the classes the rules generalise over: the forms a policy name
//! takes, the scope the delete addresses (account-scoped, resource-scoped), and the
//! values on either side of each published bound of a documented catalogue. Any single
//! policy name or retention an issue happens to mention is one row.

use serde_json::{json, Value};

use crate::harness::{error_code, Logs};

/// Every retention a log group can be set to. Source: the model's `Days` shape — "The
/// number of days to retain the log events in the specified log group. Possible values
/// are: 1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192,
/// 2557, 2922, 3288, and 3653."
const RETENTIONS: [i64; 22] = [
    1, 3, 5, 7, 14, 30, 60, 90, 120, 150, 180, 365, 400, 545, 731, 1096, 1827, 2192, 2557, 2922,
    3288, 3653,
];

/// The system fields a subscription filter may emit. Source:
/// `PutSubscriptionFilter$emitSystemFields` — "Valid values are @aws.account,
/// @aws.region, and @source.log".
const SYSTEM_FIELDS: [&str; 3] = ["@aws.account", "@aws.region", "@source.log"];

/// Policy name forms spanning the class `PolicyName` admits: the model gives the shape
/// no pattern and no length bound, so the class is "any string" — a single character, a
/// dotted name, a path-like name, one carrying spaces, one carrying non-ASCII, a long
/// one, and one differing from another row only in case.
fn policy_name_forms() -> Vec<String> {
    vec![
        "p".to_string(),
        "service.api.PutLogEvents".to_string(),
        "team/platform/logs-policy".to_string(),
        "a policy with spaces".to_string(),
        "policy-ünïcode-✓".to_string(),
        "l".repeat(200),
        "P".to_string(),
    ]
}

/// A minimal valid policy document — "This is formatted as a JSON string".
fn policy_document() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "s",
            "Effect": "Allow",
            "Principal": {"Service": "route53.amazonaws.com"},
            "Action": ["logs:PutLogEvents"],
            "Resource": "*",
        }],
    })
    .to_string()
}

/// The `message` member of an error body.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a refusal must carry a message, got {body}"))
        .to_string()
}

/// Call one operation that must be refused as a client error, answering its wire error
/// code and its message.
async fn refusal(logs: &Logs, operation: &str, body: &Value) -> (String, String) {
    let (status, parsed) = logs.call(operation, body).await;
    assert_eq!(
        status, 400,
        "case {operation} {body}: expected HTTP 400, got {status} {parsed}"
    );
    (error_code(&parsed), message_of(&parsed))
}

/// Create a log group and answer the `logGroupArn` the service reports for it — the
/// identifier a resource-scoped policy is addressed by.
async fn group_arn(logs: &Logs, group: &str) -> String {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let page = logs
        .ok(
            "DescribeLogGroups",
            &json!({"logGroupIdentifiers": [group]}),
        )
        .await;
    page.get("logGroups")
        .and_then(Value::as_array)
        .and_then(|groups| groups.first())
        .and_then(|found| found.get("logGroupArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {group}: a log group carries a logGroupArn, got {page}"))
        .to_string()
}

/// Every refusal that a resource policy the request names is not there carries that
/// name: over the forms a policy name takes, over the account-scoped and the
/// resource-scoped delete, and whether the name was never used or was just deleted.
#[tokio::test]
async fn policy_not_found_refusals_name_the_requested_policy() {
    let logs = Logs::new();
    let document = policy_document();

    // One log group carrying a policy under a name none of the rows use, and one
    // carrying none at all, so the resource-scoped delete is exercised both ways.
    let attached_arn = group_arn(&logs, "policy-carrying-group").await;
    logs.ok(
        "PutResourcePolicy",
        &json!({
            "policyName": "the-attached-one",
            "policyDocument": document,
            "resourceArn": attached_arn,
        }),
    )
    .await;
    let bare_arn = group_arn(&logs, "policy-free-group").await;

    let mut seen: Vec<String> = Vec::new();
    for name in policy_name_forms() {
        let expected = format!("Policy with name [{name}] does not exist.");

        // The delete addresses the policy by `policyName` alone (account-scoped), by a
        // resource that carries a DIFFERENT policy, and by a resource that carries none.
        let cases = [
            ("account-scoped, never created", json!({"policyName": name})),
            (
                "resource-scoped, another policy attached",
                json!({"policyName": name, "resourceArn": attached_arn}),
            ),
            (
                "resource-scoped, nothing attached",
                json!({"policyName": name, "resourceArn": bare_arn}),
            ),
        ];
        for (case, body) in cases {
            let (code, message) = refusal(&logs, "DeleteResourcePolicy", &body).await;
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case} ({name}): a policy that is not there is a not-found, got {code}"
            );
            assert_eq!(
                message, expected,
                "case {case} ({name}): expected {expected:?}, got {message:?}"
            );
            assert!(
                message.contains(&name),
                "case {case} ({name}): the refusal names the requested policy, got {message:?}"
            );
        }

        // A name that WAS used refuses the same way once its policy is gone, so the
        // message follows the request and not the history of the name.
        logs.ok(
            "PutResourcePolicy",
            &json!({"policyName": name, "policyDocument": document}),
        )
        .await;
        logs.ok("DeleteResourcePolicy", &json!({"policyName": name}))
            .await;
        let (code, message) = refusal(&logs, "DeleteResourcePolicy", &json!({"policyName": name}))
            .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case deleted twice ({name}): a policy that is gone cannot be deleted again"
        );
        assert_eq!(
            message, expected,
            "case deleted twice ({name}): expected {expected:?}, got {message:?}"
        );
        seen.push(message);
    }

    // Distinct requests produce distinct refusals: a fixed string could not.
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        seen.len(),
        "case all rows: each requested name yields its own refusal, got {seen:?}"
    );
}

/// A resource-scoped delete that names NO policy has no request member to build the
/// refusal from, so it is refused as a not-found without naming one.
#[tokio::test]
async fn a_delete_that_names_no_policy_is_still_refused() {
    let logs = Logs::new();
    let arn = group_arn(&logs, "nameless-delete-group").await;

    let (code, message) = refusal(&logs, "DeleteResourcePolicy", &json!({"resourceArn": arn})).await;
    assert_eq!(
        code, "ResourceNotFoundException",
        "case no policy attached, no policyName: nothing is there to delete, got {message:?}"
    );
    assert_eq!(
        message, "The specified resource policy does not exist.",
        "case no policy attached, no policyName: the service's sentence names the resource kind, \
         got {message:?}"
    );

    // With a policy attached, the same request is refused for the missing revision
    // instead — the revision guard, not the policy's identity.
    logs.ok(
        "PutResourcePolicy",
        &json!({
            "policyName": "attached",
            "policyDocument": policy_document(),
            "resourceArn": arn,
        }),
    )
    .await;
    let (code, message) = refusal(&logs, "DeleteResourcePolicy", &json!({"resourceArn": arn})).await;
    assert_eq!(
        code, "InvalidParameterException",
        "case policy attached, no expectedRevisionId: the revision is required, got {message:?}"
    );
}

/// Every refusal of a `retentionInDays` outside the documented catalogue declares the
/// retention invalid and lists that catalogue, in the documented order — the same
/// message whatever value was submitted, because the valid set is what it states.
#[tokio::test]
async fn invalid_retention_refusals_list_the_documented_values() {
    let logs = Logs::new();
    let group = "retention-refusal-group";
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;

    let expected = format!(
        "Invalid retention value. Valid values are: [{}]",
        RETENTIONS
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Each published bound of the catalogue and one past it, the neighbours of values
    // inside it, a non-positive value, and the extreme an `integer` admits.
    let lowest = RETENTIONS[0];
    let highest = RETENTIONS[RETENTIONS.len() - 1];
    let mut rejected = vec![
        lowest - 1,
        highest + 1,
        0,
        -1,
        2,
        4,
        6,
        29,
        100,
        364,
        366,
        3652,
        i64::from(i32::MAX),
    ];
    rejected.extend((lowest..=RETENTIONS[3]).filter(|days| !RETENTIONS.contains(days)));

    for days in rejected {
        let (code, message) = refusal(
            &logs,
            "PutRetentionPolicy",
            &json!({"logGroupName": group, "retentionInDays": days}),
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case retentionInDays {days}: a value outside the catalogue is a parameter error"
        );
        assert_eq!(
            message, expected,
            "case retentionInDays {days}: expected {expected:?}, got {message:?}"
        );
        // Every published value is readable out of the refusal — including entries no
        // issue cites, such as 2557.
        for valid in RETENTIONS {
            assert!(
                message.contains(&valid.to_string()),
                "case retentionInDays {days}: the refusal lists {valid}, got {message:?}"
            );
        }
    }

    // The catalogue in the message is the one the service accepts: every value it lists
    // is accepted, so the message is not a list that stands apart from the behaviour.
    for days in RETENTIONS {
        logs.ok(
            "PutRetentionPolicy",
            &json!({"logGroupName": group, "retentionInDays": days}),
        )
        .await;
    }
}

/// The same rule on another member whose documentation enumerates its values:
/// `emitSystemFields` is refused with the documented set listed.
#[tokio::test]
async fn invalid_system_field_refusals_list_the_documented_values() {
    let logs = Logs::new();
    let group = "system-field-refusal-group";
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let destination = "arn:aws:kinesis:eu-west-2:123456789012:stream/a-stream";
    // Every destination but a Lambda function is delivered to through a role CloudWatch
    // Logs assumes, so a put of a Kinesis destination names one.
    let role = "arn:aws:iam::123456789012:role/CWLtoKinesisRole";

    let expected = format!(
        "Invalid emitSystemFields value. Valid values are: [{}]",
        SYSTEM_FIELDS.join(", ")
    );

    // A field nobody documents, one that only looks like a documented one, and a
    // documented one buried in a list beside an undocumented one.
    for fields in [
        vec!["@aws.principal"],
        vec!["aws.account"],
        vec![""],
        vec![SYSTEM_FIELDS[2], "@source.host"],
    ] {
        let (code, message) = refusal(
            &logs,
            "PutSubscriptionFilter",
            &json!({
                "logGroupName": group,
                "filterName": "f",
                "filterPattern": "",
                "destinationArn": destination,
                "roleArn": role,
                "emitSystemFields": fields,
            }),
        )
        .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case emitSystemFields {fields:?}: an undocumented field is a parameter error"
        );
        assert_eq!(
            message, expected,
            "case emitSystemFields {fields:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The documented set itself is accepted, so the refusal above is the catalogue and
    // not a blanket rejection.
    logs.ok(
        "PutSubscriptionFilter",
        &json!({
            "logGroupName": group,
            "filterName": "f",
            "filterPattern": "",
            "destinationArn": destination,
            "roleArn": role,
            "emitSystemFields": SYSTEM_FIELDS,
        }),
    )
    .await;
}

/// Every operation of this service refuses a resource that is not there with the same
/// sentence, naming the resource KIND that is missing — the form the model documents for
/// `ResourceNotFoundException` ("The specified resource does not exist."). The table
/// spans the operations that can refuse one, and both kinds of resource they address.
/// The one operation whose own sentence differs — the data protection policy read, which
/// names the group and its owner — is asserted after the table, as that boundary.
#[tokio::test]
async fn a_missing_resource_is_refused_with_the_services_sentence() {
    let logs = Logs::new();
    let missing = "never-created-group";
    let missing_arn = format!("arn:aws:logs:eu-west-2:000000000000:log-group:{missing}");
    let destination = "arn:aws:kinesis:eu-west-2:123456789012:stream/a-stream";
    let event = json!([{"timestamp": crate::harness::now_ms(), "message": "e"}]);

    // One log group that DOES exist, so the log-stream rows below are refused for the
    // stream and not for the group.
    let group = "stream-less-group";
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "log group",
            "DeleteLogGroup",
            json!({"logGroupName": missing}),
        ),
        (
            "log group",
            "PutRetentionPolicy",
            json!({"logGroupName": missing, "retentionInDays": RETENTIONS[3]}),
        ),
        (
            "log group",
            "CreateLogStream",
            json!({"logGroupName": missing, "logStreamName": "s"}),
        ),
        (
            "log group",
            "DescribeLogStreams",
            json!({"logGroupName": missing}),
        ),
        (
            "log group",
            "DescribeSubscriptionFilters",
            json!({"logGroupName": missing}),
        ),
        (
            "log group",
            "PutSubscriptionFilter",
            json!({
                "logGroupName": missing,
                "filterName": "f",
                "filterPattern": "",
                "destinationArn": destination,
                "roleArn": "arn:aws:iam::123456789012:role/CWLtoKinesisRole",
            }),
        ),
        (
            "log group",
            "GetLogEvents",
            json!({"logGroupName": missing, "logStreamName": "s"}),
        ),
        (
            "log group",
            "PutLogEvents",
            json!({"logGroupName": missing, "logStreamName": "s", "logEvents": event}),
        ),
        (
            "log group",
            "PutResourcePolicy",
            json!({
                "policyName": "p",
                "policyDocument": policy_document(),
                "resourceArn": missing_arn,
            }),
        ),
        (
            "log group",
            "DeleteResourcePolicy",
            json!({
                "policyName": "p",
                "resourceArn": missing_arn,
                "expectedRevisionId": "00000000000000000000000000000000",
            }),
        ),
        (
            "log group",
            "DeleteRetentionPolicy",
            json!({"logGroupName": missing}),
        ),
        (
            "log group",
            "DeleteLogStream",
            json!({"logGroupName": missing, "logStreamName": "s"}),
        ),
        (
            "log group",
            "AssociateKmsKey",
            json!({
                "logGroupName": missing,
                "kmsKeyId": "arn:aws:kms:eu-west-2:000000000000:key/aaaabbbb-cccc-dddd-eeee-ffff00001111",
            }),
        ),
        (
            "log group",
            "DisassociateKmsKey",
            json!({"logGroupName": missing}),
        ),
        (
            "log group",
            "PutBearerTokenAuthentication",
            json!({
                "logGroupIdentifier": missing,
                "bearerTokenAuthenticationEnabled": true,
            }),
        ),
        // The same sentence answers each identifier form a request can name a missing
        // group by: the ARN form, and a name that differs from a group that DOES exist
        // only in case.
        (
            "log group",
            "PutBearerTokenAuthentication",
            json!({
                "logGroupIdentifier": missing_arn,
                "bearerTokenAuthenticationEnabled": true,
            }),
        ),
        (
            "log group",
            "PutBearerTokenAuthentication",
            json!({
                "logGroupIdentifier": "Stream-Less-Group",
                "bearerTokenAuthenticationEnabled": true,
            }),
        ),
        (
            "log group",
            "PutLogGroupDeletionProtection",
            json!({"logGroupIdentifier": missing, "deletionProtectionEnabled": true}),
        ),
        (
            "log group",
            "DeleteDataProtectionPolicy",
            json!({"logGroupIdentifier": missing}),
        ),
        (
            "log stream",
            "GetLogEvents",
            json!({"logGroupName": group, "logStreamName": "no-such-stream"}),
        ),
        (
            "log stream",
            "PutLogEvents",
            json!({
                "logGroupName": group,
                "logStreamName": "no-such-stream",
                "logEvents": event,
            }),
        ),
    ];

    for (kind, operation, body) in rows {
        let expected = format!("The specified {kind} does not exist.");
        let (code, message) = refusal(&logs, operation, &body).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {operation} ({kind}): a missing resource is a not-found, got {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {operation} ({kind}): expected {expected:?}, got {message:?}"
        );
    }

    // The boundary of that sentence: the data protection policy READ is the one operation
    // that words a missing log group differently, because such a policy is readable across
    // accounts — it names the group that was looked for AND the account that would own it.
    let (code, message) = refusal(
        &logs,
        "GetDataProtectionPolicy",
        &json!({"logGroupIdentifier": missing}),
    )
    .await;
    let expected = format!(
        "Log group '{missing}' for owner '{}' cannot be found",
        engine::emulator_account_id()
    );
    assert_eq!(
        code, "ResourceNotFoundException",
        "case GetDataProtectionPolicy (log group): a missing resource is a not-found, got {message:?}"
    );
    assert_eq!(
        message, expected,
        "case GetDataProtectionPolicy (log group): expected {expected:?}, got {message:?}"
    );
}
