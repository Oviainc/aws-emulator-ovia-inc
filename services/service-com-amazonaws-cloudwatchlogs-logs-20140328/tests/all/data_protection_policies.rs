// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for one log group's data protection policy:
//! `PutDataProtectionPolicy` / `GetDataProtectionPolicy` /
//! `DeleteDataProtectionPolicy`, and the `dataProtectionStatus` `DescribeLogGroups`
//! reports for the group that holds one.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables below span the classes the rules generalise over (how many data
//! identifiers a policy lists, which kind of findings destination its Audit action
//! names, which order the two blocks are written in, which form of log-group identifier
//! the request uses, how long the document is, and which documented rule a refused
//! document breaks); any single policy document an issue happens to mention is one row.

use serde_json::{json, Value};

use crate::harness::{now_ms, Logs};

/// The prefix every AWS-managed data identifier ARN carries. Source: the identifier ARNs
/// listed in "Types of data that you can mask"
/// (<https://docs.aws.amazon.com/AmazonCloudWatch/latest/logs/mask-sensitive-log-data-types.html>),
/// which `PutDataProtectionPolicy$policyDocument` links to for its `DataIdentifier`
/// array.
const DATA_IDENTIFIER_PREFIX: &str = "arn:aws:dataprotection::aws:data-identifier/";

/// The longest `policyDocument` the operation admits. Source:
/// `PutDataProtectionPolicy$policyDocument` — "The JSON specified in policyDocument can
/// be up to 30,720 characters."
const MAX_POLICY_DOCUMENT_CHARS: usize = 30_720;

/// Policy rows, differing in how many data identifiers the policy lists and which ones.
/// The managed identifier names come from the catalogue cited on
/// [`DATA_IDENTIFIER_PREFIX`]; none of them is the one the issue cited.
const POLICY_ROWS: &[(&str, &[&str])] = &[
    ("one-identifier", &["EmailAddress"]),
    ("two-identifiers", &["CreditCardNumber", "AwsSecretKey"]),
    (
        "five-identifiers",
        &[
            "Address",
            "DriversLicense-US",
            "Name",
            "PhoneNumber-US",
            "Ssn-US",
        ],
    ),
];

/// A data identifier ARN for one managed identifier name.
fn data_identifier(name: &str) -> String {
    format!("{DATA_IDENTIFIER_PREFIX}{name}")
}

/// The findings destinations an Audit action can name. Source:
/// `PutDataProtectionPolicy$policyDocument` — "You can optionally use that
/// FindingsDestination object to list one or more destinations to send audit findings
/// to. If you specify destinations such as log groups, Firehose streams, and S3 buckets,
/// they must already exist." The empty object is the "optionally" case.
fn findings_destinations(findings_log_group: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("no-destination", json!({})),
        (
            "log-group-destination",
            json!({"CloudWatchLogs": {"LogGroup": findings_log_group}}),
        ),
        (
            "firehose-destination",
            json!({"Firehose": {"DeliveryStream": "audit-findings-stream"}}),
        ),
        (
            "s3-destination",
            json!({"S3": {"Bucket": "audit-findings-bucket"}}),
        ),
    ]
}

/// One Audit block: a `DataIdentifier` array plus an `Operation` with an `Audit` action
/// carrying a `FindingsDestination`, as `PutDataProtectionPolicy$policyDocument`
/// documents the first block.
fn audit_block(identifiers: &[String], findings_destination: &Value) -> Value {
    json!({
        "Sid": "audit-sensitive-data",
        "DataIdentifier": identifiers,
        "Operation": {"Audit": {"FindingsDestination": findings_destination}},
    })
}

/// One Deidentify block: the same `DataIdentifier` array plus an `Operation` with a
/// `Deidentify` action carrying the empty `MaskConfig`, as the second block is
/// documented.
fn deidentify_block(identifiers: &[String]) -> Value {
    json!({
        "Sid": "mask-sensitive-data",
        "DataIdentifier": identifiers,
        "Operation": {"Deidentify": {"MaskConfig": {}}},
    })
}

/// A whole policy document: the optional `Name` / `Description` / `Version` fields the
/// member documents, plus the two blocks. `audit_first` writes them in the documented
/// order; the reverse order is the same policy written differently.
fn policy_document(
    identifier_names: &[&str],
    findings_destination: &Value,
    audit_first: bool,
    description: &str,
) -> String {
    let identifiers: Vec<String> = identifier_names.iter().map(|name| data_identifier(name)).collect();
    let audit = audit_block(&identifiers, findings_destination);
    let deidentify = deidentify_block(&identifiers);
    let statement = if audit_first {
        vec![audit, deidentify]
    } else {
        vec![deidentify, audit]
    };
    json!({
        "Name": "emulator-data-protection-policy",
        "Description": description,
        "Version": "2021-06-01",
        "Statement": statement,
    })
    .to_string()
}

/// The simplest well-formed document, for a case whose subject is not the document.
fn simple_document() -> String {
    policy_document(&["EmailAddress"], &json!({}), true, "one identifier")
}

/// One string member of a response body.
fn member(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the response must carry {key}, got {body}"))
        .to_string()
}

/// The `lastUpdatedTime` of a response body.
fn last_updated_time(body: &Value) -> i64 {
    body.get("lastUpdatedTime")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("the response must carry lastUpdatedTime, got {body}"))
}

/// The `dataProtectionStatus` `DescribeLogGroups` reports for one group, or `None` when
/// the group reports none.
async fn data_protection_status(logs: &Logs, group: &str) -> Option<String> {
    let described = logs
        .describe_group(group)
        .await
        .unwrap_or_else(|| panic!("case {group}: the log group must exist"));
    described
        .get("dataProtectionStatus")
        .map(|status| {
            status
                .as_str()
                .unwrap_or_else(|| panic!("case {group}: dataProtectionStatus is a string, got {status}"))
                .to_string()
        })
}

/// A put stores the policy on the group it names, a get reports it back byte-for-byte,
/// the group reports `ACTIVATED` while it holds one, and a delete removes it — for every
/// document shape, findings destination and log-group identifier form.
#[tokio::test]
async fn policy_round_trips_through_put_get_and_delete() {
    let logs = Logs::new();
    logs.ok(
        "CreateLogGroup",
        &json!({"logGroupName": "audit-findings-group"}),
    )
    .await;

    for (row, identifiers) in POLICY_ROWS {
        for (destination_case, destination) in findings_destinations("audit-findings-group") {
            for audit_first in [true, false] {
                for selector in ["name", "logGroupArn"] {
                    let case = format!("{row}/{destination_case}/audit-first={audit_first}/{selector}");
                    let group = format!("round-trip-{row}-{destination_case}-{audit_first}-{selector}");
                    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
                        .await;
                    assert_eq!(
                        data_protection_status(&logs, &group).await,
                        None,
                        "case {case:?}: a group that never held a policy reports no dataProtectionStatus"
                    );

                    let (_, log_group_arn) = logs.group_arns(&group).await;
                    let identifier = if selector == "name" {
                        group.clone()
                    } else {
                        log_group_arn
                    };
                    let document = policy_document(identifiers, &destination, audit_first, row);

                    let before = now_ms();
                    let put = logs
                        .ok(
                            "PutDataProtectionPolicy",
                            &json!({
                                "logGroupIdentifier": identifier,
                                "policyDocument": document,
                            }),
                        )
                        .await;
                    let after = now_ms();
                    assert_eq!(
                        member(&put, "logGroupIdentifier"),
                        identifier,
                        "case {case:?}: the put echoes the identifier the request named, got {put}"
                    );
                    assert_eq!(
                        member(&put, "policyDocument"),
                        document,
                        "case {case:?}: the put reports the document it stored, got {put}"
                    );
                    let put_time = last_updated_time(&put);
                    assert!(
                        (before..=after).contains(&put_time),
                        "case {case:?}: lastUpdatedTime is the put's wall clock in [{before}, {after}], got {put_time}"
                    );

                    // Read the policy back through the other identifier form, so what is
                    // asserted is the stored policy and not the request that wrote it.
                    let read_identifier = if selector == "name" {
                        logs.group_arns(&group).await.1
                    } else {
                        group.clone()
                    };
                    let got = logs
                        .ok(
                            "GetDataProtectionPolicy",
                            &json!({"logGroupIdentifier": read_identifier}),
                        )
                        .await;
                    assert_eq!(
                        member(&got, "policyDocument"),
                        document,
                        "case {case:?}: the get reports the stored document byte-for-byte, got {got}"
                    );
                    assert_eq!(
                        member(&got, "logGroupIdentifier"),
                        read_identifier,
                        "case {case:?}: the get echoes the identifier the request named, got {got}"
                    );
                    assert_eq!(
                        last_updated_time(&got),
                        put_time,
                        "case {case:?}: the get reports the put's lastUpdatedTime, got {got}"
                    );
                    assert_eq!(
                        data_protection_status(&logs, &group).await.as_deref(),
                        Some("ACTIVATED"),
                        "case {case:?}: a group holding a policy reports ACTIVATED"
                    );

                    logs.ok(
                        "DeleteDataProtectionPolicy",
                        &json!({"logGroupIdentifier": identifier}),
                    )
                    .await;
                    assert_eq!(
                        logs.err(
                            "GetDataProtectionPolicy",
                            &json!({"logGroupIdentifier": identifier})
                        )
                        .await,
                        "ResourceNotFoundException",
                        "case {case:?}: the deleted policy is gone"
                    );
                    assert_eq!(
                        data_protection_status(&logs, &group).await.as_deref(),
                        Some("DELETED"),
                        "case {case:?}: a group whose policy was deleted reports DELETED"
                    );
                }
            }
        }
    }
}

/// A put on a group that already holds a policy replaces it: the group holds exactly
/// one, and the get reports the latest.
#[tokio::test]
async fn a_put_replaces_the_policy_the_group_holds() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": "replaced"}))
        .await;

    let mut previous_time = 0;
    for (row, identifiers) in POLICY_ROWS {
        let document = policy_document(identifiers, &json!({}), true, row);
        let put = logs
            .ok(
                "PutDataProtectionPolicy",
                &json!({"logGroupIdentifier": "replaced", "policyDocument": document}),
            )
            .await;
        let got = logs
            .ok(
                "GetDataProtectionPolicy",
                &json!({"logGroupIdentifier": "replaced"}),
            )
            .await;
        assert_eq!(
            member(&got, "policyDocument"),
            document,
            "case {row:?}: the group holds the document the latest put stored, got {got}"
        );
        let updated = last_updated_time(&put);
        assert!(
            updated >= previous_time,
            "case {row:?}: lastUpdatedTime never moves backwards: got {updated} after {previous_time}"
        );
        previous_time = updated;
        assert_eq!(
            data_protection_status(&logs, "replaced").await.as_deref(),
            Some("ACTIVATED"),
            "case {row:?}: the group still holds a policy"
        );
    }
}

/// Reading a policy reports it without changing it: two gets of one policy agree, and
/// the group's status is untouched.
#[tokio::test]
async fn reading_a_policy_does_not_change_it() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": "read-twice"}))
        .await;
    let document = simple_document();
    logs.ok(
        "PutDataProtectionPolicy",
        &json!({"logGroupIdentifier": "read-twice", "policyDocument": document}),
    )
    .await;

    let first = logs
        .ok(
            "GetDataProtectionPolicy",
            &json!({"logGroupIdentifier": "read-twice"}),
        )
        .await;
    let second = logs
        .ok(
            "GetDataProtectionPolicy",
            &json!({"logGroupIdentifier": "read-twice"}),
        )
        .await;
    assert_eq!(
        first, second,
        "case read-twice: a second get reports what the first did, got {first} then {second}"
    );
    assert_eq!(
        data_protection_status(&logs, "read-twice").await.as_deref(),
        Some("ACTIVATED"),
        "case read-twice: reading the policy leaves the group holding it"
    );
}

/// The document is stored byte-for-byte at every length up to the documented cap, and a
/// document one character past the cap is refused.
#[tokio::test]
async fn documents_are_preserved_up_to_the_documented_character_cap() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": "large-document"}))
        .await;

    // A document of exactly `chars` characters: the padding goes in the `Description`
    // field the member documents as optional.
    let document_of_length = |chars: usize| -> String {
        let empty = policy_document(&["EmailAddress"], &json!({}), true, "");
        let padding = chars
            .checked_sub(empty.chars().count())
            .unwrap_or_else(|| panic!("case {chars}: the padded document must reach {chars} characters"));
        policy_document(
            &["EmailAddress"],
            &json!({}),
            true,
            &"d".repeat(padding),
        )
    };

    // Each published bound of the cap, and one past it.
    for chars in [512, MAX_POLICY_DOCUMENT_CHARS - 1, MAX_POLICY_DOCUMENT_CHARS] {
        let document = document_of_length(chars);
        assert_eq!(
            document.chars().count(),
            chars,
            "case {chars}: the test's own document is {chars} characters"
        );
        logs.ok(
            "PutDataProtectionPolicy",
            &json!({"logGroupIdentifier": "large-document", "policyDocument": document}),
        )
        .await;
        let got = logs
            .ok(
                "GetDataProtectionPolicy",
                &json!({"logGroupIdentifier": "large-document"}),
            )
            .await;
        assert_eq!(
            member(&got, "policyDocument"),
            document,
            "case {chars}: a document of {chars} characters is stored byte-for-byte"
        );
    }

    let over = document_of_length(MAX_POLICY_DOCUMENT_CHARS + 1);
    assert_eq!(
        logs.err(
            "PutDataProtectionPolicy",
            &json!({"logGroupIdentifier": "large-document", "policyDocument": over})
        )
        .await,
        "InvalidParameterException",
        "case {}: a document past the {MAX_POLICY_DOCUMENT_CHARS}-character cap is refused",
        MAX_POLICY_DOCUMENT_CHARS + 1
    );
}

/// A document that breaks one of the rules `PutDataProtectionPolicy$policyDocument`
/// states is refused, and nothing is stored.
#[tokio::test]
async fn a_malformed_document_is_refused() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": "malformed"}))
        .await;

    let email = vec![data_identifier("EmailAddress")];
    let name = vec![data_identifier("Name")];
    let none: Vec<String> = Vec::new();
    // One row per sentence of the member's documentation: the document is JSON, it holds
    // the two blocks, the Audit action carries a FindingsDestination, the Deidentify
    // action carries an EMPTY MaskConfig, and the two DataIdentifier arrays match.
    let rows: Vec<(&str, String)> = vec![
        ("not-json", "this is not a policy".to_string()),
        ("json-but-not-an-object", json!([1, 2, 3]).to_string()),
        ("no-statement", json!({"Name": "p", "Version": "2021-06-01"}).to_string()),
        (
            "empty-statement",
            json!({"Statement": []}).to_string(),
        ),
        (
            "audit-block-only",
            json!({"Statement": [audit_block(&email, &json!({}))]}).to_string(),
        ),
        (
            "deidentify-block-only",
            json!({"Statement": [deidentify_block(&email)]}).to_string(),
        ),
        (
            "audit-without-findings-destination",
            json!({"Statement": [
                {"DataIdentifier": email, "Operation": {"Audit": {}}},
                deidentify_block(&email),
            ]})
            .to_string(),
        ),
        (
            "non-empty-mask-config",
            json!({"Statement": [
                audit_block(&email, &json!({})),
                {
                    "DataIdentifier": email,
                    "Operation": {"Deidentify": {"MaskConfig": {"MaskWith": "#"}}},
                },
            ]})
            .to_string(),
        ),
        (
            "mismatched-data-identifiers",
            json!({"Statement": [
                audit_block(&email, &json!({})),
                deidentify_block(&name),
            ]})
            .to_string(),
        ),
        (
            "data-identifiers-only-in-one-block",
            json!({"Statement": [
                audit_block(&email, &json!({})),
                {"Operation": {"Deidentify": {"MaskConfig": {}}}},
            ]})
            .to_string(),
        ),
        (
            "empty-data-identifier-array",
            json!({"Statement": [
                audit_block(&none, &json!({})),
                deidentify_block(&none),
            ]})
            .to_string(),
        ),
    ];

    for (case, document) in rows {
        let code = logs
            .err(
                "PutDataProtectionPolicy",
                &json!({"logGroupIdentifier": "malformed", "policyDocument": document}),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: a document breaking a documented rule is refused InvalidParameterException, got {code}"
        );
        assert_eq!(
            logs.err(
                "GetDataProtectionPolicy",
                &json!({"logGroupIdentifier": "malformed"})
            )
            .await,
            "ResourceNotFoundException",
            "case {case:?}: a refused put stores nothing"
        );
        assert_eq!(
            data_protection_status(&logs, "malformed").await,
            None,
            "case {case:?}: a refused put leaves the group's dataProtectionStatus unreported"
        );
    }

    // "If you specify destinations such as log groups, Firehose streams, and S3 buckets,
    // they must already exist."
    let document = policy_document(
        &["EmailAddress"],
        &json!({"CloudWatchLogs": {"LogGroup": "not-created-yet"}}),
        true,
        "missing destination",
    );
    assert_eq!(
        logs.err(
            "PutDataProtectionPolicy",
            &json!({"logGroupIdentifier": "malformed", "policyDocument": document})
        )
        .await,
        "InvalidParameterException",
        "case missing-findings-destination: a findings destination log group that does not exist is refused"
    );
}

/// Every one of the three operations answers `ResourceNotFoundException` for a log group
/// that does not exist, and the two readers for a group that holds no policy.
#[tokio::test]
async fn a_missing_group_or_policy_is_not_found() {
    let logs = Logs::new();
    logs.ok("CreateLogGroup", &json!({"logGroupName": "no-policy"}))
        .await;
    let (_, missing_group_arn) = {
        logs.ok("CreateLogGroup", &json!({"logGroupName": "deleted-group"}))
            .await;
        let arns = logs.group_arns("deleted-group").await;
        logs.ok("DeleteLogGroup", &json!({"logGroupName": "deleted-group"}))
            .await;
        arns
    };

    let document = simple_document();
    // Rows: the three operations, against a group that was never created, a group that
    // was deleted (named by its ARN), and an existing group holding no policy.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "put/never-created",
            "PutDataProtectionPolicy",
            json!({"logGroupIdentifier": "never-created", "policyDocument": document}),
        ),
        (
            "put/deleted-group-arn",
            "PutDataProtectionPolicy",
            json!({"logGroupIdentifier": missing_group_arn, "policyDocument": document}),
        ),
        (
            "get/never-created",
            "GetDataProtectionPolicy",
            json!({"logGroupIdentifier": "never-created"}),
        ),
        (
            "get/no-policy",
            "GetDataProtectionPolicy",
            json!({"logGroupIdentifier": "no-policy"}),
        ),
        (
            "delete/never-created",
            "DeleteDataProtectionPolicy",
            json!({"logGroupIdentifier": "never-created"}),
        ),
        (
            "delete/no-policy",
            "DeleteDataProtectionPolicy",
            json!({"logGroupIdentifier": "no-policy"}),
        ),
    ];

    for (case, operation, body) in cases {
        let code = logs.err(operation, &body).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: a missing resource is refused ResourceNotFoundException, got {code}"
        );
    }
}
