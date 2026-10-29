// SPDX-License-Identifier: CC0-1.0

//! The `ResourceArns` rule, end to end, through BOTH static-membership operations.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The one rule under test, asserted over a TABLE of lists crossed with both
//! operations that take one rather than one row: a static-membership call acts on a
//! `ResourceArns` list of at most the modelled maximum whose every entry is shaped
//! like a resource ARN, and refuses any other list the same way whichever of the two
//! operations is called — which is what `crate::state::check_resource_arns` decides
//! in one place for both of them.

use serde_json::{json, Value};

use service_com_amazonaws_resourcegroups_ardi::state::{check_resource_arns, MAX_RESOURCE_ARNS};

use super::resource_group_lifecycle::{error_code, static_group, Emulator};
use super::ungroup_resources::{group_with_members, listed_members, sorted, succeeded};

/// ARN forms the modelled `ResourceArn` admits, one per branch of its pattern
/// `^arn:aws(-[a-z]+)*:[a-z0-9\-]*:([a-z]{2}(-[a-z]+)+-\d{1})?:([0-9]{12})?:.+$`
/// (source: `smithy_query(query="shape",shape="ResourceArn")`): a `type/id`
/// resource, a `type:id` resource, a bare-name resource, a global resource with
/// neither region nor account, another partition, a service carrying digits and
/// hyphens, and a multi-level resource path.
const ARN_FORMS: [&str; 7] = [
    "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0",
    "arn:aws:lambda:eu-west-1:000000000000:function:report",
    "arn:aws:sqs:us-east-1:000000000000:orders-queue",
    "arn:aws:s3:::arns-rule-probe-bucket",
    "arn:aws-us-gov:dynamodb:us-gov-west-1:000000000000:table/Orders",
    "arn:aws:route53-recovery-control:us-west-2:000000000000:cluster/abcdef",
    "arn:aws:s3:us-west-2:000000000000:accesspoint/reports/object/key",
];

/// Strings the modelled `ResourceArn` pattern (same source as [`ARN_FORMS`]) does
/// NOT admit, one per field of the ARN the pattern constrains: a bare resource id, a
/// group name, the empty string, a five-field ARN with no resource segment, an empty
/// resource, a region that is not shaped like a region, an account that is not
/// twelve digits, a partition outside `aws(-[a-z]+)*`, an uppercase service, and an
/// ARN whose partition and service are empty.
const NOT_ARNS: [&str; 10] = [
    "i-0123456789abcdef0",
    "arns-rule-group",
    "",
    "arn:aws:ec2:us-east-1:000000000000",
    "arn:aws:ec2:us-east-1:000000000000:",
    "arn:aws:ec2:nowhere:000000000000:instance/i-0abc",
    "arn:aws:ec2:us-east-1:12345:instance/i-0abc",
    "arn:AWS:ec2:us-east-1:000000000000:instance/i-0abc",
    "arn:aws:EC2:us-east-1:000000000000:instance/i-0abc",
    "arn:::ec2::instance/i-0abc",
];

/// `count` distinct instance ARNs — the list length itself under test, so the
/// entries differ only in their resource id.
fn instance_arns(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("arn:aws:ec2:us-east-1:000000000000:instance/i-{index:017x}"))
        .collect()
}

/// The `message` an error response carries.
fn message(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body}"))
        .to_string()
}

/// One row of the accepted table: a whole `ResourceArns` list.
struct Accepted {
    label: &'static str,
    arns: Vec<String>,
}

/// Lists the rule admits, differing in entry count, ARN form and repetition.
fn accepted_lists() -> Vec<Accepted> {
    vec![
        Accepted {
            label: "one ARN, the modelled minimum",
            arns: vec![ARN_FORMS[0].to_string()],
        },
        Accepted {
            label: "every ARN form the modelled pattern admits",
            arns: ARN_FORMS.iter().map(|arn| (*arn).to_string()).collect(),
        },
        Accepted {
            label: "ten ARNs, the modelled maximum",
            arns: instance_arns(MAX_RESOURCE_ARNS),
        },
        Accepted {
            label: "the same ARN twice",
            arns: vec![ARN_FORMS[1].to_string(), ARN_FORMS[1].to_string()],
        },
    ]
}

/// One row of the refused table.
struct Refused {
    label: String,
    arns: Vec<String>,
}

/// Lists the rule refuses: one past the modelled maximum, well past it, and every
/// shape of entry that is not a resource ARN — alone, and beside an ARN that is one.
fn refused_lists() -> Vec<Refused> {
    let mut cases = vec![
        Refused {
            label: format!("{} ARNs, one past the modelled maximum", MAX_RESOURCE_ARNS + 1),
            arns: instance_arns(MAX_RESOURCE_ARNS + 1),
        },
        Refused {
            label: format!("{} ARNs, well past the modelled maximum", MAX_RESOURCE_ARNS * 3),
            arns: instance_arns(MAX_RESOURCE_ARNS * 3),
        },
    ];
    for not_an_arn in NOT_ARNS {
        cases.push(Refused {
            label: format!("the single entry {not_an_arn:?}, which is not an ARN"),
            arns: vec![not_an_arn.to_string()],
        });
        cases.push(Refused {
            label: format!("an ARN beside {not_an_arn:?}, which is not an ARN"),
            arns: vec![ARN_FORMS[0].to_string(), not_an_arn.to_string()],
        });
    }
    cases
}

/// Every list the rule admits is acted on by BOTH operations: the grouping call
/// records exactly its entries as the group's memberships, and the ungrouping call
/// drops exactly those memberships again.
#[tokio::test]
async fn both_static_membership_operations_act_on_every_resource_arns_list_the_rule_admits() {
    for case in accepted_lists() {
        let label = case.label;
        let emulator = Emulator::new();
        let group = "arns-rule-group";
        emulator.ok("/groups", &static_group(group)).await;
        // One entry repeated is one membership, so the memberships a list records
        // are its entries deduplicated.
        let expected = sorted(&case.arns);

        let grouped = emulator
            .ok(
                "/group-resources",
                &json!({ "Group": group, "ResourceArns": case.arns }),
            )
            .await;
        assert_eq!(
            succeeded(label, &grouped),
            expected,
            "case {label:?}: GroupResources must report every entry as grouped, got {grouped}",
        );
        let stored = listed_members(&emulator, label, group).await;
        assert_eq!(
            stored, expected,
            "case {label:?}: the group must hold exactly the entries grouped, got {stored:?}",
        );

        let ungrouped = emulator
            .ok(
                "/ungroup-resources",
                &json!({ "Group": group, "ResourceArns": case.arns }),
            )
            .await;
        assert_eq!(
            succeeded(label, &ungrouped),
            expected,
            "case {label:?}: UngroupResources must report every membership it dropped, \
             got {ungrouped}",
        );
        let left = listed_members(&emulator, label, group).await;
        assert!(
            left.is_empty(),
            "case {label:?}: ungrouping every entry must leave no membership, got {left:?}",
        );
    }
}

/// A list the rule refuses is refused by BOTH operations, with the same status, the
/// same error code and the same sentence — and a refused call changes no membership.
#[tokio::test]
async fn both_static_membership_operations_refuse_the_same_resource_arns_lists_identically() {
    let emulator = Emulator::new();
    let group = "arns-rule-group";
    let members = instance_arns(2);
    group_with_members(&emulator, group, &members).await;

    for case in refused_lists() {
        let label = &case.label;
        // (the status, the error code, the message) of each operation, which the rule
        // requires to be the same answer.
        let mut answers: Vec<(u16, String, String)> = Vec::new();
        for path in ["/group-resources", "/ungroup-resources"] {
            let (status, answered) = emulator
                .call(path, &json!({ "Group": group, "ResourceArns": case.arns }))
                .await;
            let code = error_code(&answered).to_string();
            assert_eq!(
                (status, code.as_str()),
                (400, "BadRequestException"),
                "case {label:?} at {path}: expected 400 BadRequestException, \
                 got {status} {answered}",
            );
            let text = message(label, &answered);
            assert!(
                text.to_lowercase().contains("resourcearns"),
                "case {label:?} at {path}: the refusal must name the member it refused, \
                 got {text:?}",
            );
            answers.push((status, code, text));
        }
        assert_eq!(
            answers[0], answers[1],
            "case {label:?}: both operations must refuse the list with one answer",
        );
    }

    let stored = listed_members(&emulator, "after every refusal", group).await;
    assert_eq!(
        stored,
        sorted(&members),
        "a refused list must change no membership, got {stored:?}",
    );
}

/// The one shared rule itself, over the whole domain of a `ResourceArns` list — the
/// list lengths on either side of the modelled maximum, and every ARN form and
/// non-ARN form of an entry.
///
/// The boundary validator refuses a list outside the modelled length or an entry
/// outside the modelled pattern before either handler runs (the suite above asserts
/// that over the wire, for both of them), so this is the one place the shared rule's
/// own sentences are observable.
#[test]
fn the_shared_resource_arns_rule_spans_the_list_bound_and_every_entry_form() {
    // Every length the modelled list admits, from its published minimum to its
    // published maximum.
    for count in 1..=MAX_RESOURCE_ARNS {
        let arns = instance_arns(count);
        let answered = check_resource_arns(&arns);
        assert_eq!(
            answered,
            Ok(()),
            "case {count} ARNs: a list inside the modelled length is accepted, \
             got {answered:?}",
        );
    }
    // One past the published maximum, and well past it.
    for count in [MAX_RESOURCE_ARNS + 1, MAX_RESOURCE_ARNS * 3] {
        let arns = instance_arns(count);
        let answered = check_resource_arns(&arns);
        assert_eq!(
            answered,
            Err(format!(
                "ResourceArns carries {count} entries, which is more than the \
                 {MAX_RESOURCE_ARNS} this operation accepts"
            )),
            "case {count} ARNs: a list past the modelled maximum is refused with the \
             one shared sentence, got {answered:?}",
        );
    }
    // The published minimum is a boundary-only rule: this rule bounds how much work
    // one request can ask for, so it says nothing about a list with nothing in it.
    let empty: Vec<String> = Vec::new();
    let answered = check_resource_arns(&empty);
    assert_eq!(
        answered,
        Ok(()),
        "case no ARNs: the shared rule bounds only the maximum, got {answered:?}",
    );

    for arn in ARN_FORMS {
        let entries = vec![arn.to_string()];
        let answered = check_resource_arns(&entries);
        assert_eq!(
            answered,
            Ok(()),
            "case {arn:?}: an entry the modelled pattern admits is accepted, \
             got {answered:?}",
        );
    }
    for not_an_arn in NOT_ARNS {
        let expected = Err(format!(
            "ResourceArns contains {not_an_arn}, which is not a resource ARN"
        ));
        // Refused wherever the entry sits in the list: alone, after an ARN, and
        // before one.
        let lists = [
            vec![not_an_arn.to_string()],
            vec![ARN_FORMS[0].to_string(), not_an_arn.to_string()],
            vec![not_an_arn.to_string(), ARN_FORMS[0].to_string()],
        ];
        for entries in lists {
            let answered = check_resource_arns(&entries);
            assert_eq!(
                answered, expected,
                "case {entries:?}: an entry the modelled pattern does not admit is \
                 refused with the one shared sentence, got {answered:?}",
            );
        }
    }
}
