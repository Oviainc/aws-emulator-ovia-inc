// SPDX-License-Identifier: CC0-1.0

//! Where a log group's data protection policy meets the GROUP's own lifecycle and the
//! `logGroupIdentifier` member's own constraint — the complement of
//! `tests/all/data_protection_policies.rs`, which holds the put/get/delete loop and the
//! document's documented rules.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rules under test:
//!
//! * a policy is held by the ONE log group it was put on ("The PutDataProtectionPolicy
//!   operation applies to only the specified log group"), so its neighbours are untouched
//!   and it is gone when that group is deleted — a group re-created under the same name
//!   holds none;
//! * `LogGroupIdentifier`'s `@pattern` `^[\w#+=/:,.@-]*$` admits no `*`, so a log group's
//!   `arn` form (which ends in `:*`) is not one of the identifiers these operations take,
//!   while its `logGroupArn` form is;
//! * `DescribeLogGroups` under a `logGroupNamePattern` reports the reduced log-group
//!   projection, which carries no `dataProtectionStatus` for a group that holds a
//!   policy — the same group reports `ACTIVATED` without the pattern.
//!
//! The tables span the classes those rules generalise over (which group of a set holds a
//! policy, which identifier form names it, which operation is called).

use serde_json::{json, Value};

use crate::harness::Logs;

/// The prefix every AWS-managed data identifier ARN carries. Source: the identifier ARNs
/// listed in "Types of data that you can mask"
/// (<https://docs.aws.amazon.com/AmazonCloudWatch/latest/logs/mask-sensitive-log-data-types.html>),
/// which `PutDataProtectionPolicy$policyDocument` links to for its `DataIdentifier`
/// array.
const DATA_IDENTIFIER_PREFIX: &str = "arn:aws:dataprotection::aws:data-identifier/";

/// A policy document over one managed data identifier, in the form
/// `PutDataProtectionPolicy$policyDocument` documents: the Audit block with its
/// `FindingsDestination`, then the Deidentify block with the empty `MaskConfig`, both
/// listing the same `DataIdentifier` array.
fn policy_document(identifier_name: &str) -> String {
    let identifiers = vec![format!("{DATA_IDENTIFIER_PREFIX}{identifier_name}")];
    json!({
        "Name": "emulator-data-protection-policy",
        "Version": "2021-06-01",
        "Statement": [
            {
                "Sid": "audit-sensitive-data",
                "DataIdentifier": identifiers,
                "Operation": {"Audit": {"FindingsDestination": {}}},
            },
            {
                "Sid": "mask-sensitive-data",
                "DataIdentifier": identifiers,
                "Operation": {"Deidentify": {"MaskConfig": {}}},
            },
        ],
    })
    .to_string()
}

/// The `policyDocument` the service reports for one log group.
async fn stored_document(logs: &Logs, group: &str) -> String {
    let got = logs
        .ok(
            "GetDataProtectionPolicy",
            &json!({"logGroupIdentifier": group}),
        )
        .await;
    got.get("policyDocument")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {group}: the get must carry policyDocument, got {got}"))
        .to_string()
}

/// The `dataProtectionStatus` `DescribeLogGroups` reports for one group, or `None` when
/// it reports none.
async fn data_protection_status(logs: &Logs, group: &str) -> Option<String> {
    let described = logs
        .describe_group(group)
        .await
        .unwrap_or_else(|| panic!("case {group}: the log group must exist"));
    described
        .get("dataProtectionStatus")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A policy is held by the one group it was put on: each group reports its own document,
/// and a group nobody put on holds none and reports no status.
#[tokio::test]
async fn a_policy_is_held_by_the_one_group_it_was_put_on() {
    let logs = Logs::new();
    // Rows: three groups, two of which hold DIFFERENT policies, one of which holds none.
    let rows = [
        ("/scope/email", Some("EmailAddress")),
        ("/scope/address", Some("Address")),
        ("/scope/untouched", None),
    ];
    for (group, _) in rows {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    for (group, identifier_name) in rows {
        if let Some(identifier_name) = identifier_name {
            logs.ok(
                "PutDataProtectionPolicy",
                &json!({
                    "logGroupIdentifier": group,
                    "policyDocument": policy_document(identifier_name),
                }),
            )
            .await;
        }
    }

    for (group, identifier_name) in rows {
        match identifier_name {
            Some(identifier_name) => {
                let expected = policy_document(identifier_name);
                let stored = stored_document(&logs, group).await;
                assert_eq!(
                    stored, expected,
                    "case {group}: expected only this group's own document, got {stored}"
                );
                assert_eq!(
                    data_protection_status(&logs, group).await.as_deref(),
                    Some("ACTIVATED"),
                    "case {group}: a group holding a policy reports ACTIVATED"
                );
            }
            None => {
                let code = logs
                    .err(
                        "GetDataProtectionPolicy",
                        &json!({"logGroupIdentifier": group}),
                    )
                    .await;
                assert_eq!(
                    code, "ResourceNotFoundException",
                    "case {group}: a group nobody put on holds no policy, got {code}"
                );
                assert_eq!(
                    data_protection_status(&logs, group).await,
                    None,
                    "case {group}: a group that never held a policy reports no dataProtectionStatus"
                );
            }
        }
    }
}

/// The policy lives on its log group, so deleting the group takes it along: a group
/// re-created under the same name holds none and reports no status.
#[tokio::test]
async fn deleting_the_log_group_takes_its_policy_with_it() {
    let logs = Logs::new();
    // Rows: the identifier form the policy was put through, so neither the name form nor
    // the ARN form leaves a policy behind the group's deletion.
    for selector in ["logGroupName", "logGroupArn"] {
        let group = format!("/lifecycle/{selector}");
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
        let identifier = if selector == "logGroupName" {
            group.clone()
        } else {
            logs.group_arns(&group).await.1
        };
        logs.ok(
            "PutDataProtectionPolicy",
            &json!({
                "logGroupIdentifier": identifier,
                "policyDocument": policy_document("EmailAddress"),
            }),
        )
        .await;
        logs.ok("DeleteLogGroup", &json!({"logGroupName": group}))
            .await;

        let code = logs
            .err(
                "GetDataProtectionPolicy",
                &json!({"logGroupIdentifier": identifier}),
            )
            .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {selector}: the group is gone, so there is no policy to report, got {code}"
        );

        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
        let code = logs
            .err(
                "GetDataProtectionPolicy",
                &json!({"logGroupIdentifier": identifier}),
            )
            .await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {selector}: a new group under an old name holds no policy, got {code}"
        );
        assert_eq!(
            data_protection_status(&logs, &group).await,
            None,
            "case {selector}: a re-created group reports no dataProtectionStatus"
        );
    }
}

/// A log group's `arn` form ends in `:*`, which `LogGroupIdentifier`'s `@pattern`
/// (`^[\w#+=/:,.@-]*$`) does not admit: every operation taking a `logGroupIdentifier`
/// refuses it on that constraint, while the group's `logGroupArn` form is accepted.
#[tokio::test]
async fn the_wildcard_arn_form_is_outside_the_member_pattern() {
    let logs = Logs::new();
    let group = "/wildcard-arn/group";
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let (arn, log_group_arn) = logs.group_arns(group).await;
    let document = policy_document("EmailAddress");

    // The `logGroupArn` form is the ARN these operations take: the put through it is what
    // makes the refusals below about the FORM of the other ARN and not about the group.
    logs.ok(
        "PutDataProtectionPolicy",
        &json!({"logGroupIdentifier": log_group_arn, "policyDocument": document}),
    )
    .await;

    let rows = [
        (
            "GetDataProtectionPolicy",
            json!({"logGroupIdentifier": arn}),
        ),
        (
            "PutDataProtectionPolicy",
            json!({"logGroupIdentifier": arn, "policyDocument": document}),
        ),
        (
            "DeleteDataProtectionPolicy",
            json!({"logGroupIdentifier": arn}),
        ),
    ];
    for (operation, body) in rows {
        let (code, message) = logs.refusal(operation, &body).await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {operation}: expected the member's constraint refusal, got {code} {message:?}"
        );
        assert!(
            message.contains("logGroupIdentifier"),
            "case {operation}: expected the refusal to name the member it judged, got {message:?}"
        );
    }

    // The refused requests judged the identifier, so the stored policy is untouched.
    let stored = stored_document(&logs, group).await;
    assert_eq!(
        stored, document,
        "case after the refusals: the group still holds the document it was put, got {stored}"
    );
}

/// `DescribeLogGroups` under a `logGroupNamePattern` reports the reduced log-group
/// projection, which carries no `dataProtectionStatus` — for a group holding a policy and
/// for one that does not.
#[tokio::test]
async fn a_pattern_search_reports_no_data_protection_status() {
    let logs = Logs::new();
    // Rows: one group that holds a policy and one that does not, both matched by the one
    // pattern.
    let rows = [("/pattern/holder", true), ("/pattern/bare", false)];
    for (group, holds_policy) in rows {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
        if holds_policy {
            logs.ok(
                "PutDataProtectionPolicy",
                &json!({
                    "logGroupIdentifier": group,
                    "policyDocument": policy_document("EmailAddress"),
                }),
            )
            .await;
        }
    }

    let page = logs
        .ok(
            "DescribeLogGroups",
            &json!({"logGroupNamePattern": "pattern"}),
        )
        .await;
    let reported = page
        .get("logGroups")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("DescribeLogGroups must return logGroups, got {page}"))
        .clone();

    for (group, holds_policy) in rows {
        let found = reported
            .iter()
            .find(|entry| entry.get("logGroupName").and_then(Value::as_str) == Some(group))
            .unwrap_or_else(|| panic!("case {group}: the pattern search must report it, got {page}"));
        assert_eq!(
            found.get("dataProtectionStatus"),
            None,
            "case {group}: a pattern search reports no dataProtectionStatus, got {found}"
        );
        let without_pattern = data_protection_status(&logs, group).await;
        let expected = if holds_policy { Some("ACTIVATED") } else { None };
        assert_eq!(
            without_pattern.as_deref(),
            expected,
            "case {group}: without a pattern the same group reports {expected:?}, got {without_pattern:?}"
        );
    }
}
