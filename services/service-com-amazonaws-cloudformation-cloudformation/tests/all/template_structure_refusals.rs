// SPDX-License-Identifier: CC0-1.0

//! What a body that is no TEMPLATE, and a parameter value that is no NUMBER, are answered
//! with by every operation that takes a template body, end to end over the awsQuery wire.
//!
//! Two rules, each about the REQUEST rather than about a deployment, and each asserted over
//! the class it generalises over rather than over one body:
//!
//! * **a body whose document is not a mapping of template sections is no template**: the
//!   top level itself, or a section the format states as a mapping of what it declares, and
//!   every operation that takes a body — the three that submit one for deployment and the one
//!   whose whole answer is a verdict on it — refuses it with HTTP 400 `ValidationError` and
//!   the one structural wording, before anything is written. A body that IS such a mapping is
//!   held to the rules about its sections instead, so an empty section and a document the
//!   reader will not read at all are refused for their own faults;
//! * **a value in force for a `Number` parameter that is no number is refused** with the
//!   sentence that names the parameter, whatever spelling makes it no number and whether the
//!   request supplied it or the declaration defaulted it.

use service_com_amazonaws_cloudformation_cloudformation::state::template::{
    NO_RESOURCES_DECLARED, UNSUPPORTED_STRUCTURE,
};

use crate::harness::{call, create_stack, member, router, wait_for_the_transition, xml_text};

/// A template that deploys, declaring one resource no owning service has to be asked about:
/// the stack the `UpdateStack` and `CreateChangeSet` rows submit their bodies against.
const DEPLOYABLE: &str =
    r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// The stack those two operations are held against.
const HOLDER: &str = "structure-holder";

/// The `Code` and `Message` of a refusal, for an assertion's own message.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (xml_text(body, "Code"), xml_text(body, "Message"))
}

/// One body that is no mapping of template sections.
struct StructureCase {
    case: &'static str,
    body: &'static str,
}

/// The class the structural rule covers: the top level stated as each kind of document that
/// is not a mapping, in both template spellings, and then each section the format states as a
/// mapping stated as something else.
const UNSUPPORTED: &[StructureCase] = &[
    StructureCase {
        case: "a JSON sequence at the top level",
        body: "[]",
    },
    StructureCase {
        case: "a JSON sequence of resource declarations at the top level",
        body: r#"[{"Type":"AWS::CloudFormation::WaitConditionHandle"}]"#,
    },
    StructureCase {
        case: "a JSON string at the top level",
        body: r#""Resources""#,
    },
    StructureCase {
        case: "a JSON number at the top level",
        body: "42",
    },
    StructureCase {
        case: "a JSON boolean at the top level",
        body: "true",
    },
    StructureCase {
        case: "a YAML sequence at the top level",
        body: "- Resources\n- Outputs\n",
    },
    StructureCase {
        case: "a YAML scalar at the top level",
        body: "not-a-template",
    },
    StructureCase {
        case: "a Resources section stated as a sequence",
        body: r#"{"Resources":[{"Type":"AWS::CloudFormation::WaitConditionHandle"}]}"#,
    },
    StructureCase {
        case: "a Resources section stated as text",
        body: r#"{"Resources":"Handle"}"#,
    },
    StructureCase {
        case: "a Resources section stated as a YAML sequence",
        body: "Resources:\n  - Type: AWS::CloudFormation::WaitConditionHandle\n",
    },
    StructureCase {
        case: "a Parameters section stated as a sequence",
        body: r#"{"Parameters":["P"],
            "Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
    },
    StructureCase {
        case: "an Outputs section stated as text",
        body: r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
            "Outputs":"none"}"#,
    },
    StructureCase {
        case: "a Mappings section stated as a number",
        body: r#"{"Mappings":1,
            "Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
    },
    StructureCase {
        case: "a Conditions section stated as a sequence",
        body: r#"{"Conditions":["IsProd"],
            "Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
    },
    StructureCase {
        case: "a Metadata section stated as text",
        body: r#"{"Metadata":"about this template",
            "Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
    },
    StructureCase {
        case: "a Rules section stated as a sequence",
        body: r#"{"Rules":[{"Assertions":[]}],
            "Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
    },
];

/// The name a refused create submits its body under: no row creates a stack, so one name
/// serves them all and a stack found under it would itself be the failure.
const REFUSED_NAME: &str = "structure-refused";

/// The four operations that take a template body, each with the members that carry one.
fn submissions(body: &str) -> Vec<(&'static str, Vec<(String, String)>)> {
    vec![
        (
            "CreateStack",
            vec![
                member("StackName", REFUSED_NAME),
                member("TemplateBody", body),
            ],
        ),
        (
            "UpdateStack",
            vec![member("StackName", HOLDER), member("TemplateBody", body)],
        ),
        (
            "CreateChangeSet",
            vec![
                member("StackName", HOLDER),
                member("ChangeSetName", "structure-change-set"),
                member("TemplateBody", body),
            ],
        ),
        ("ValidateTemplate", vec![member("TemplateBody", body)]),
    ]
}

#[tokio::test]
async fn a_body_that_is_no_mapping_of_sections_is_refused_for_its_structure() {
    let router = router();
    let (status, body) = create_stack(&router, HOLDER, DEPLOYABLE, &[]).await;
    assert_eq!(
        status, 200,
        "the stack the revision-submitting rows are held against must be created first, \
         got {status} {body}"
    );
    wait_for_the_transition().await;

    for row in UNSUPPORTED {
        let case = row.case;
        for (action, members) in submissions(row.body) {
            let (status, body) = call(&router, action, &members).await;
            assert_eq!(
                status, 400,
                "case {case:?} through {action}: expected the body refused, got {status} {body}"
            );
            let answered = refusal(&body);
            let expected = (
                Some("ValidationError".to_string()),
                Some(UNSUPPORTED_STRUCTURE.to_string()),
            );
            assert_eq!(
                answered, expected,
                "case {case:?} through {action}: expected {expected:?}, got {answered:?}"
            );
        }
        // Nothing was written: the refusal is of the REQUEST, so no stack exists under the
        // name the create submitted the body under...
        let (status, body) = call(
            &router,
            "DescribeStacks",
            &[member("StackName", REFUSED_NAME)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create leaves no stack, got {status} {body}"
        );
        // ...and the stack the update and the change set were submitted against still
        // carries the template it was created from.
        let (status, body) = call(&router, "GetTemplate", &[member("StackName", HOLDER)]).await;
        let served = xml_text(&body, "TemplateBody");
        assert_eq!(
            (status, served.as_deref()),
            (200, Some(DEPLOYABLE)),
            "case {case:?}: the holder stack's template must be untouched, got {status} {body}"
        );
    }
}

/// One body that IS a mapping of template sections, with the refusal due for its own fault —
/// `None` where the body deploys. `refused` is the text the message must open with, since a
/// body the reader stopped inside is refused with the position it stopped at.
struct SectionCase {
    case: &'static str,
    body: &'static str,
    refused: Option<&'static str>,
}

/// The other side of the rule: a document the structural refusal does NOT answer, so the
/// faults it is weighed against each keep their own wording.
const SECTIONS: &[SectionCase] = &[
    SectionCase {
        case: "a mapping declaring no section at all",
        body: "{}",
        refused: Some(NO_RESOURCES_DECLARED),
    },
    SectionCase {
        case: "a Resources section stated as nothing",
        body: r#"{"Resources":null}"#,
        refused: Some(NO_RESOURCES_DECLARED),
    },
    SectionCase {
        case: "a Resources section stated as an empty mapping",
        body: r#"{"Resources":{}}"#,
        refused: Some(NO_RESOURCES_DECLARED),
    },
    SectionCase {
        case: "a body the reader will not read at all",
        body: r#"{"Resources": "#,
        refused: Some("Template format error: YAML not well-formed."),
    },
    SectionCase {
        case: "a mapping of sections that declares a resource",
        body: DEPLOYABLE,
        refused: None,
    },
    SectionCase {
        case: "a mapping whose Description is text and whose Transform is a list",
        body: r#"{"Description":"a template","Transform":["AWS::LanguageExtensions"],
            "Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#,
        refused: None,
    },
];

#[tokio::test]
async fn a_mapping_of_sections_is_held_to_the_rules_about_its_sections() {
    let router = router();
    for row in SECTIONS {
        let case = row.case;
        let (status, body) =
            call(&router, "ValidateTemplate", &[member("TemplateBody", row.body)]).await;
        match row.refused {
            Some(opening) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the body refused, got {status} {body}"
                );
                let (code, message) = refusal(&body);
                assert_eq!(
                    code.as_deref(),
                    Some("ValidationError"),
                    "case {case:?}: expected ValidationError, got {code:?} {message:?}"
                );
                let reported = message.unwrap_or_default();
                assert!(
                    reported.starts_with(opening),
                    "case {case:?}: expected a refusal opening {opening:?}, got {reported:?}"
                );
            }
            None => assert_eq!(
                status, 200,
                "case {case:?}: expected a verdict on the body, got {status} {body}"
            ),
        }
    }
}

/// One `Number`-typed declaration, the value in force for it, and whether that value is a
/// number.
struct NumberCase {
    case: &'static str,
    declaration: &'static str,
    /// The value the request passes, or `None` to leave the declaration's own `Default` in
    /// force.
    value: Option<&'static str>,
    a_number: bool,
}

/// The class the `Number` rule covers: every spelling that is no number — a word, a number
/// with text on it, several numbers in one scalar value, a non-finite float spelling — beside
/// the spellings that ARE one, for a scalar `Number` and for the list type whose elements are
/// each one, and with the value supplied by the request or defaulted by the declaration.
///
/// `Type` and its spellings come from the `Parameters` section's own documentation
/// ("Number: an integer or float"), read through `smithy_query(query="doc",
/// shape="com.amazonaws.cloudformation#Parameter")`'s neighbouring template contract.
const NUMBERS: &[NumberCase] = &[
    NumberCase {
        case: "a word where a number was declared",
        declaration: r#"{"Type":"Number"}"#,
        value: Some("not-a-number"),
        a_number: false,
    },
    NumberCase {
        case: "a number with text after it",
        declaration: r#"{"Type":"Number"}"#,
        value: Some("12abc"),
        a_number: false,
    },
    NumberCase {
        case: "two numbers in one scalar value",
        declaration: r#"{"Type":"Number"}"#,
        value: Some("1,2"),
        a_number: false,
    },
    NumberCase {
        case: "a non-finite float spelling",
        declaration: r#"{"Type":"Number"}"#,
        value: Some("inf"),
        a_number: false,
    },
    NumberCase {
        case: "a float spelling that is not a number at all",
        declaration: r#"{"Type":"Number"}"#,
        value: Some("NaN"),
        a_number: false,
    },
    NumberCase {
        case: "an integer",
        declaration: r#"{"Type":"Number"}"#,
        value: Some("42"),
        a_number: true,
    },
    NumberCase {
        case: "a negative float",
        declaration: r#"{"Type":"Number"}"#,
        value: Some("-3.5"),
        a_number: true,
    },
    NumberCase {
        case: "one element of a List<Number> that is no number",
        declaration: r#"{"Type":"List<Number>"}"#,
        value: Some("1,two,3"),
        a_number: false,
    },
    NumberCase {
        case: "a List<Number> whose every element is a number",
        declaration: r#"{"Type":"List<Number>"}"#,
        value: Some("1,2,3"),
        a_number: true,
    },
    NumberCase {
        case: "a declared Default that is no number, with nothing supplied",
        declaration: r#"{"Type":"Number","Default":"eight"}"#,
        value: None,
        a_number: false,
    },
    NumberCase {
        case: "a declared Default that is a number, with nothing supplied",
        declaration: r#"{"Type":"Number","Default":"8"}"#,
        value: None,
        a_number: true,
    },
];

/// A template declaring one parameter under `declaration` beside one resource that needs no
/// owning service.
fn parameterised(declaration: &str) -> String {
    format!(
        r#"{{"Parameters":{{"Size":{declaration}}},
            "Resources":{{"Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}}}}"#
    )
}

#[tokio::test]
async fn a_number_parameter_given_no_number_is_refused_by_name() {
    let router = router();
    // The sentence names the parameter the value is for; nothing else of the request reaches
    // it, so one expectation answers every row.
    let expected_message = "Parameter 'Size' must be a number.";
    for (index, row) in NUMBERS.iter().enumerate() {
        let case = row.case;
        let name = format!("number-{index}");
        let template = parameterised(row.declaration);
        let passed = match row.value {
            Some(value) => vec![
                member("Parameters.member.1.ParameterKey", "Size"),
                member("Parameters.member.1.ParameterValue", value),
            ],
            None => Vec::new(),
        };
        let (status, body) = create_stack(&router, &name, &template, &passed).await;
        if row.a_number {
            assert_eq!(
                status, 200,
                "case {case:?}: the declaration admits the value, got {status} {body}"
            );
            wait_for_the_transition().await;
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: the value must be refused, got {status} {body}"
        );
        let answered = refusal(&body);
        let expected = (
            Some("ValidationError".to_string()),
            Some(expected_message.to_string()),
        );
        assert_eq!(
            answered, expected,
            "case {case:?}: expected {expected:?}, got {answered:?}"
        );
        // Nothing was created: the value is weighed before any stack exists under the name.
        let (status, body) = call(&router, "DescribeStacks", &[member("StackName", &name)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create leaves no stack, got {status} {body}"
        );
    }
}
