// SPDX-License-Identifier: CC0-1.0

//! A constraint broken by the ELEMENTS of a list member, end to end over the awsQuery wire.
//!
//! RULE: a value outside a declared constraint is refused at the member that HOLDS it. For a
//! list that is the LIST member, not the element the walk found the value on: the clause
//! quotes the value that member holds — the whole list, as the request sent it, neither
//! sorted nor deduplicated — and states the element shape's own constraint nested inside
//! `Member must satisfy constraint: [ … ]`. An enum's value set is listed in the order
//! CloudFormation's own validator lists it, which for `StackStatus` is neither the model's
//! declaration order nor a sorted one.
//!
//! The rows differ along what the rule generalises over: WHICH list member the request
//! carries (the status filter of a read, the capabilities of a deployment), how many of its
//! elements are outside the set and in which positions, the ORDER the request sends them in,
//! and how many elements the list has — plus the lists that satisfy the model, which are
//! refused by nothing.

use crate::harness::{call, member, router, xml_text};

/// `StackStatus`, in the order CloudFormation's own input validation lists its value set.
/// Source: the service's own refusal of an unknown filter value, which this suite is the
/// wire-level reading of (`crate::service`'s `STACK_STATUS_VALUE_SET`).
const STACK_STATUS_SET: &str = "REVIEW_IN_PROGRESS, CREATE_FAILED, UPDATE_ROLLBACK_FAILED, \
                                UPDATE_ROLLBACK_IN_PROGRESS, CREATE_IN_PROGRESS, \
                                IMPORT_ROLLBACK_IN_PROGRESS, \
                                UPDATE_ROLLBACK_COMPLETE_CLEANUP_IN_PROGRESS, \
                                ROLLBACK_IN_PROGRESS, IMPORT_IN_PROGRESS, DELETE_COMPLETE, \
                                UPDATE_COMPLETE, UPDATE_IN_PROGRESS, DELETE_FAILED, \
                                IMPORT_COMPLETE, UPDATE_FAILED, DELETE_IN_PROGRESS, \
                                ROLLBACK_COMPLETE, ROLLBACK_FAILED, IMPORT_ROLLBACK_COMPLETE, \
                                UPDATE_COMPLETE_CLEANUP_IN_PROGRESS, CREATE_COMPLETE, \
                                IMPORT_ROLLBACK_FAILED, UPDATE_ROLLBACK_COMPLETE";

/// `Capability`, in the order the model declares it: a set this service's own refusals do not
/// re-order, so the clause lists it as declared.
/// Source: `smithy_query(query="shape", shape="Capability")`.
const CAPABILITY_SET: &str = "CAPABILITY_IAM, CAPABILITY_NAMED_IAM, CAPABILITY_AUTO_EXPAND";

/// The whole refusal one bad list is answered with.
fn expected_message(list: &[&str], path: &str, value_set: &str) -> String {
    format!(
        "1 validation error detected: Value '[{}]' at '{path}' failed to satisfy constraint: \
         Member must satisfy constraint: [Member must satisfy enum value set: [{value_set}]]",
        list.join(", ")
    )
}

/// One row: an operation, the list member it carries, the elements sent (in order), and
/// whether the model admits every one of them.
struct Row {
    case: &'static str,
    action: &'static str,
    /// The request members that are not the list, so each operation is otherwise
    /// well-formed.
    other: &'static [(&'static str, &'static str)],
    list_member: &'static str,
    /// The violation path the refusal states, which is the list member in lowerCamel.
    path: &'static str,
    value_set: &'static str,
    sent: &'static [&'static str],
    refused: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an unknown status after a known one",
            action: "ListStacks",
            other: &[],
            list_member: "StackStatusFilter",
            path: "stackStatusFilter",
            value_set: STACK_STATUS_SET,
            sent: &["CREATE_COMPLETE", "ACTIVE"],
            refused: true,
        },
        Row {
            case: "the same two values in the other order",
            action: "ListStacks",
            other: &[],
            list_member: "StackStatusFilter",
            path: "stackStatusFilter",
            value_set: STACK_STATUS_SET,
            sent: &["ACTIVE", "CREATE_COMPLETE"],
            refused: true,
        },
        Row {
            case: "one unknown status on its own",
            action: "ListStacks",
            other: &[],
            list_member: "StackStatusFilter",
            path: "stackStatusFilter",
            value_set: STACK_STATUS_SET,
            sent: &["ACTIVE"],
            refused: true,
        },
        Row {
            case: "two unknown statuses among three elements",
            action: "ListStacks",
            other: &[],
            list_member: "StackStatusFilter",
            path: "stackStatusFilter",
            value_set: STACK_STATUS_SET,
            sent: &["ACTIVE", "UPDATE_COMPLETE", "RETIRED"],
            refused: true,
        },
        Row {
            case: "the same status twice, which is neither deduplicated nor sorted",
            action: "ListStacks",
            other: &[],
            list_member: "StackStatusFilter",
            path: "stackStatusFilter",
            value_set: STACK_STATUS_SET,
            sent: &["UPDATE_COMPLETE", "ACTIVE", "UPDATE_COMPLETE"],
            refused: true,
        },
        Row {
            case: "a filter every element of which the model admits",
            action: "ListStacks",
            other: &[],
            list_member: "StackStatusFilter",
            path: "stackStatusFilter",
            value_set: STACK_STATUS_SET,
            sent: &["CREATE_COMPLETE", "UPDATE_COMPLETE"],
            refused: false,
        },
        Row {
            // The same rule at ANOTHER operation's list member, whose element enum this
            // service does not re-order: a deployment's capabilities.
            case: "an unknown capability on a deployment",
            action: "CreateStack",
            other: &[
                ("StackName", "list-element-refusal"),
                (
                    "TemplateBody",
                    r#"{"Resources":{"Data":{"Type":"AWS::S3::Bucket","Properties":{}}}}"#,
                ),
            ],
            list_member: "Capabilities",
            path: "capabilities",
            value_set: CAPABILITY_SET,
            sent: &["CAPABILITY_IAM", "CAPABILITY_EVERYTHING"],
            refused: true,
        },
    ]
}

#[tokio::test]
async fn a_constraint_broken_by_a_lists_elements_is_stated_at_the_list() {
    let router = router();
    for row in rows() {
        let case = row.case;
        let mut members: Vec<(String, String)> = row
            .other
            .iter()
            .map(|(key, value)| member(key, value))
            .collect();
        for (index, value) in row.sent.iter().enumerate() {
            members.push(member(
                &format!("{}.member.{}", row.list_member, index + 1),
                value,
            ));
        }
        let (code, body) = call(&router, row.action, &members).await;
        if !row.refused {
            assert_eq!(
                code, 200,
                "case {case:?}: a list the model admits is refused by nothing, got {code} \
                 {body}"
            );
            continue;
        }
        assert_eq!(
            code, 400,
            "case {case:?}: expected the request to be refused, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected CloudFormation's own refusal code, got {body}"
        );
        let expected = expected_message(row.sent, row.path, row.value_set);
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {body}"
        );
        // The refusal is still this service's own envelope: one `<RequestId>` beside the
        // `<Error>`.
        assert_eq!(
            xml_text(&body, "RequestId").map(|id| id.len()),
            Some(36),
            "case {case:?}: expected the envelope's request id, got {body}"
        );
    }
}
