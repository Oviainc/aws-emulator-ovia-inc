// SPDX-License-Identifier: CC0-1.0

//! How a request that states a parameter twice over, and one that re-creates a change-set
//! name under different attributes, are refused — over the awsQuery wire.
//!
//! Two rules, each held over a TABLE rather than over one recorded request:
//!
//! 1. **One value per parameter key.** `Parameter$UsePreviousValue` is "during a stack update,
//!    use the existing parameter value that the stack is using for a given parameter key. If
//!    you specify `true`, do not specify a parameter value"
//!    (`smithy_query(query="shape", shape="Parameter")`), so an entry asking for the previous
//!    value AND supplying a non-empty one of its own states two values for one key and is
//!    refused `400 ValidationError` reading `Invalid input for parameter key <Key>. Cannot
//!    specify usePreviousValue as true and non empty value for a parameter` — the key the
//!    entry carried is the sentence's only variable part. An EMPTY value states no value, so
//!    it is not in conflict, and the previous value is what the operation resolves.
//! 2. **A re-created change-set name names the ATTRIBUTE that differs.** "The name must be
//!    unique among all change sets that are associated with the specified stack": a name the
//!    stack already carries is answered with the change set that is there when the request
//!    submits the same attributes, and otherwise refused `400 AlreadyExistsException` reading
//!    `ChangeSet <name> cannot be created due to a mismatch with existing attribute
//!    <attribute>`. The attribute is named as the SERVICE names it — `Template` for the
//!    template the change set would put on the stack, which three request members
//!    (`TemplateBody`, `TemplateURL`, `UsePreviousTemplate`) are three ways of stating, so no
//!    one of them is what the refusal can name.
//!
//! Rule 1's rows span the operations that resolve a `Parameters` list against previous values
//! (the stack ones and the stack-set one, which share the resolution), WHICH entry of the list
//! carries the conflict, and the two sides of "non empty" — a value and the empty string.
//! Rule 2's rows span the attribute that differs and, for the template, the MEMBER the second
//! request states it with.
//!
//! The sentences are spelled out here rather than imported from the crate, because the
//! sentence IS the requirement: a reworded message must fail this suite.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, member, router, wait_for_the_transition, xml_text, xml_texts,
};

/// The template the stacks here are deployed from: one parameter, really reaching a resource,
/// so a parameter the request carries has somewhere to go.
const TEMPLATE: &str = r#"{"Parameters":{"Carried":{"Type":"String","Default":"deployed"},
     "Other":{"Type":"String","Default":"other-deployed"}},
     "Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":{"Ref":"Carried"}}}}}"#;

/// A second revision of it, so an update or a change set has something to change.
const REVISED: &str = r#"{"Parameters":{"Carried":{"Type":"String","Default":"deployed"},
     "Other":{"Type":"String","Default":"other-deployed"}},
     "Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":{"Ref":"Other"}}}}}"#;

const STACK: &str = "parameter-conflicts";
const STACK_SET: &str = "parameter-conflict-set";

/// `Invalid input for parameter key <Key>. Cannot specify usePreviousValue as true and non
/// empty value for a parameter` — the refusal an entry stating two values gets.
fn use_previous_value_conflict(parameter_key: &str) -> String {
    format!(
        "Invalid input for parameter key {parameter_key}. Cannot specify usePreviousValue as \
         true and non empty value for a parameter"
    )
}

/// `ChangeSet <name> cannot be created due to a mismatch with existing attribute <attribute>`.
fn change_set_mismatch(change_set_name: &str, attribute: &str) -> String {
    format!(
        "ChangeSet {change_set_name} cannot be created due to a mismatch with existing \
         attribute {attribute}"
    )
}

/// The operations that resolve a `Parameters` list against the values a record already
/// carries — the one resolution, reached from every one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvesParameters {
    UpdateStack,
    CreateChangeSet,
    UpdateStackSet,
}

const RESOLVES_PARAMETERS: &[ResolvesParameters] = &[
    ResolvesParameters::UpdateStack,
    ResolvesParameters::CreateChangeSet,
    ResolvesParameters::UpdateStackSet,
];

/// One `Parameters` list a row sends: each entry is a key, the value it states (`None` for an
/// entry that states none at all) and whether it asks for the previous value.
type Entry = (&'static str, Option<&'static str>, bool);

/// The entry rows, and the key the refusal must name — `None` for a list that is admitted.
const PARAMETER_LISTS: &[(&str, &[Entry], Option<&str>)] = &[
    (
        "one entry asking for the previous value AND stating one",
        &[("Carried", Some("supplied"), true)],
        Some("Carried"),
    ),
    (
        "the conflict on the SECOND entry, so the whole list is checked",
        &[
            ("Carried", Some("supplied"), false),
            ("Other", Some("supplied"), true),
        ],
        Some("Other"),
    ),
    (
        "both entries in conflict, which names the first one found",
        &[
            ("Carried", Some("supplied"), true),
            ("Other", Some("supplied"), true),
        ],
        Some("Carried"),
    ),
    (
        "an EMPTY value beside the previous-value instruction, which states no value",
        &[("Carried", Some(""), true)],
        None,
    ),
    (
        "the previous-value instruction on its own",
        &[("Carried", None, true)],
        None,
    ),
    (
        "a value on its own",
        &[("Carried", Some("supplied"), false)],
        None,
    ),
];

/// The `Parameters.member.N.*` members one list is sent as.
fn parameter_members(entries: &[Entry]) -> Vec<(String, String)> {
    let mut members = Vec::new();
    for (index, (key, value, use_previous)) in entries.iter().enumerate() {
        let at = format!("Parameters.member.{}", index + 1);
        members.push(member(&format!("{at}.ParameterKey"), key));
        if let Some(value) = value {
            members.push(member(&format!("{at}.ParameterValue"), value));
        }
        if *use_previous {
            members.push(member(&format!("{at}.UsePreviousValue"), "true"));
        }
    }
    members
}

/// The stack every stack-scoped row acts on, created with both parameters at their defaults
/// and settled.
async fn settled_stack(router: &MultiServiceRouter) {
    let (status_code, body) = create_stack(router, STACK, TEMPLATE, &[]).await;
    assert_eq!(
        status_code, 200,
        "the stack under test must be created, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
}

/// Send one `Parameters` list through `op` on a fresh router.
async fn parameters_through(
    op: ResolvesParameters,
    entries: &[Entry],
) -> (u16, String) {
    let router = router();
    let parameters = parameter_members(entries);
    match op {
        ResolvesParameters::UpdateStack => {
            settled_stack(&router).await;
            let mut members = vec![
                member("StackName", STACK),
                member("TemplateBody", REVISED),
            ];
            members.extend(parameters);
            call(&router, "UpdateStack", &members).await
        }
        ResolvesParameters::CreateChangeSet => {
            settled_stack(&router).await;
            let mut members = vec![
                member("StackName", STACK),
                member("ChangeSetName", "parameter-plan"),
                member("TemplateBody", REVISED),
            ];
            members.extend(parameters);
            call(&router, "CreateChangeSet", &members).await
        }
        ResolvesParameters::UpdateStackSet => {
            let (status_code, body) = call(
                &router,
                "CreateStackSet",
                &[
                    member("StackSetName", STACK_SET),
                    member("TemplateBody", TEMPLATE),
                ],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "the stack set under test must be created, got {status_code} {body}"
            );
            let mut members = vec![
                member("StackSetName", STACK_SET),
                member("TemplateBody", REVISED),
            ];
            members.extend(parameters);
            call(&router, "UpdateStackSet", &members).await
        }
    }
}

#[tokio::test]
async fn a_parameter_stating_two_values_is_refused_by_every_operation_that_resolves_one() {
    for op in RESOLVES_PARAMETERS {
        for (case, entries, conflicting_key) in PARAMETER_LISTS {
            let (status_code, body) = parameters_through(*op, entries).await;
            match conflicting_key {
                Some(key) => {
                    assert_eq!(
                        status_code, 400,
                        "case {op:?} {case:?}: expected the request to be refused, got \
                         {status_code} {body}"
                    );
                    let expected = use_previous_value_conflict(key);
                    assert_eq!(
                        (
                            xml_text(&body, "Code").unwrap_or_default(),
                            xml_text(&body, "Message").unwrap_or_default()
                        ),
                        ("ValidationError".to_string(), expected.clone()),
                        "case {op:?} {case:?}: expected the refusal to read {expected:?}, got \
                         {body}"
                    );
                }
                None => assert_eq!(
                    status_code, 200,
                    "case {op:?} {case:?}: expected the request to be accepted, got \
                     {status_code} {body}"
                ),
            }
        }
    }
}

#[tokio::test]
async fn an_empty_value_beside_the_instruction_resolves_to_the_previous_value() {
    // The other side of rule 1: "non empty" is what conflicts, so an empty value leaves the
    // previous-value instruction to decide — and what it resolves to is the value the stack
    // is already using, read back through `DescribeStacks`.
    let router = router();
    settled_stack(&router).await;
    let mut members = vec![
        member("StackName", STACK),
        member("TemplateBody", REVISED),
    ];
    members.extend(parameter_members(&[
        ("Carried", Some(""), true),
        ("Other", Some("restated"), false),
    ]));
    let (status_code, body) = call(&router, "UpdateStack", &members).await;
    assert_eq!(
        status_code, 200,
        "the update must be accepted, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let (status_code, described) = call(
        &router,
        "DescribeStacks",
        &[member("StackName", STACK)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DescribeStacks must answer 200, got {status_code} {described}"
    );
    let values: Vec<String> = xml_texts(&described, "ParameterValue");
    assert!(
        values.contains(&"deployed".to_string()),
        "expected the carried parameter to keep the value the stack was using, got {values:?}"
    );
    assert!(
        values.contains(&"restated".to_string()),
        "expected the restated parameter to take the request's value, got {values:?}"
    );
}

/// One row of the change-set name-reuse table: what the second request states differently,
/// the members it states it with, and the attribute the refusal must name.
struct ReuseRow {
    case: &'static str,
    members: fn() -> Vec<(String, String)>,
    attribute: &'static str,
}

/// A third revision, for the row whose second request submits another body.
const THIRD: &str = r#"{"Parameters":{"Carried":{"Type":"String","Default":"deployed"},
     "Other":{"Type":"String","Default":"other-deployed"}},
     "Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":"stated-outright"}}}}"#;

const REUSE_ROWS: &[ReuseRow] = &[
    ReuseRow {
        case: "another template body",
        members: || vec![member("TemplateBody", THIRD)],
        attribute: "Template",
    },
    ReuseRow {
        // The SAME attribute reached through another member: the stack's own template is not
        // the body the stored change set carries, so what differs is the Template — and the
        // refusal cannot name a member this request did not even send.
        case: "the stack's own template, reused instead of a body",
        members: || vec![member("UsePreviousTemplate", "true")],
        attribute: "Template",
    },
    ReuseRow {
        case: "another parameter value",
        members: || {
            let mut members = vec![member("TemplateBody", REVISED)];
            members.extend(parameter_members(&[("Carried", Some("different"), false)]));
            members
        },
        attribute: "Parameters",
    },
    ReuseRow {
        case: "another description",
        members: || {
            vec![
                member("TemplateBody", REVISED),
                member("Description", "another plan"),
            ]
        },
        attribute: "Description",
    },
    ReuseRow {
        case: "a tag the stored change set does not carry",
        members: || {
            vec![
                member("TemplateBody", REVISED),
                member("Tags.member.1.Key", "env"),
                member("Tags.member.1.Value", "dev"),
            ]
        },
        attribute: "Tags",
    },
];

#[tokio::test]
async fn a_re_created_change_set_name_names_the_attribute_that_differs() {
    for row in REUSE_ROWS {
        let router = router();
        settled_stack(&router).await;
        let change_set = "reused-plan";
        let (status_code, body) = call(
            &router,
            "CreateChangeSet",
            &[
                member("StackName", STACK),
                member("ChangeSetName", change_set),
                member("TemplateBody", REVISED),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {:?}: the first change set must be created, got {status_code} {body}",
            row.case
        );

        let mut members = vec![
            member("StackName", STACK),
            member("ChangeSetName", change_set),
        ];
        members.extend((row.members)());
        let (status_code, body) = call(&router, "CreateChangeSet", &members).await;
        assert_eq!(
            status_code, 400,
            "case {:?}: re-creating the name under other attributes must be refused, got \
             {status_code} {body}",
            row.case
        );
        let expected = change_set_mismatch(change_set, row.attribute);
        assert_eq!(
            (
                xml_text(&body, "Code").unwrap_or_default(),
                xml_text(&body, "Message").unwrap_or_default()
            ),
            ("AlreadyExistsException".to_string(), expected.clone()),
            "case {:?}: expected the refusal to read {expected:?}, got {body}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_same_attributes_under_the_same_name_answer_the_change_set_that_is_there() {
    // The other side of rule 2: the comparison is what decides, so re-submitting the SAME
    // plan under the same name is the same change set rather than a conflict.
    let router = router();
    settled_stack(&router).await;
    let change_set = "idempotent-plan";
    let members = vec![
        member("StackName", STACK),
        member("ChangeSetName", change_set),
        member("TemplateBody", REVISED),
    ];
    let (status_code, first) = call(&router, "CreateChangeSet", &members).await;
    assert_eq!(
        status_code, 200,
        "the first change set must be created, got {status_code} {first}"
    );
    let (status_code, second) = call(&router, "CreateChangeSet", &members).await;
    assert_eq!(
        status_code, 200,
        "the identical request must be answered with the change set that is there, got \
         {status_code} {second}"
    );
    assert_eq!(
        xml_text(&second, "Id"),
        xml_text(&first, "Id"),
        "expected the same change set id, got {second}"
    );
}
