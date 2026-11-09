// SPDX-License-Identifier: CC0-1.0

//! What a stack operation holds its REQUEST to before it writes anything, end to end over
//! the awsQuery wire.
//!
//! Four rules, each about the request rather than about the deployment:
//!
//! * a stack NAME already in use in the Region is refused with the modelled
//!   `AlreadyExistsException` before the template the request submits is weighed at all, and
//!   the stack that holds the name is left as it was;
//! * a parameter VALUE in force is held to the constraints its own declaration states —
//!   `Type`, `MinValue`/`MaxValue`, `MinLength`/`MaxLength`, `AllowedValues` and
//!   `AllowedPattern` — and a value outside them refuses the call with `ValidationError`,
//!   worded by the declaration's own `ConstraintDescription` where it states one;
//! * the resource TYPES a request states in `ResourceTypes` bound the template it submits,
//!   and the member cannot be stated beside `Capabilities`;
//! * a `TemplateBody` past its documented maximum is refused with the constraint-violation
//!   message that QUOTES the offending value, bounded.
//!
//! ...and two rules about what a stack operation does before it provisions anything:
//!
//! * with pre-deployment validation on (the default), a declared property value the type's
//!   own contract does not admit fails the operation before the resource is created, so no
//!   resource-level event is recorded for it and nothing is asked of the owning service,
//!   while `DisableValidation` defers the same problem to provisioning;
//! * a pagination token a read of ONE stack's resources handed out is scoped to that stack:
//!   presenting it on another stack fails that check distinctly from presenting a value this
//!   service never issued.
//!
//! Every table spans the class its rule generalises over rather than the one recorded case.

use service_com_amazonaws_cloudformation_cloudformation::state::{
    self, status, PAGE_SIZE, TEMPLATE_BODY_MAX_BYTES,
};

use crate::harness::{
    call, create_stack, member, router, router_with_engine, wait_for_the_transition, xml_blocks,
    xml_text, xml_texts,
};

/// A template declaring one resource of a type no owning service has to be asked about, so
/// a case about the REQUEST does not depend on a deployment.
const PLAIN_TEMPLATE: &str = r#"{"Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// A template whose IAM resource a create has to acknowledge with `CAPABILITY_IAM`, used to
/// show that the NAME is weighed before the template is.
const IAM_TEMPLATE: &str = r#"{"Resources":{"Role":{"Type":"AWS::IAM::Role","Properties":
    {"AssumeRolePolicyDocument":{"Version":"2012-10-17","Statement":[]}}}}}"#;

/// A template whose `Transform` names a macro a create has to acknowledge with
/// `CAPABILITY_AUTO_EXPAND`, for the same reason.
const MACRO_TEMPLATE: &str = r#"{"Transform":["MyMacro"],
    "Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// A template whose own parameter declaration refuses the value the request passes, again
/// for the same reason.
const CONSTRAINED_TEMPLATE: &str = r#"{"Parameters":{"Size":{"Type":"Number","MinValue":2}},
    "Resources":{"Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// The `Code` and `Message` of a refusal.
fn refusal(body: &str) -> (Option<String>, Option<String>) {
    (xml_text(body, "Code"), xml_text(body, "Message"))
}

/// One row of the duplicate-name table: a template a second create submits, and the extra
/// members it carries.
struct DuplicateCase {
    case: &'static str,
    template: &'static str,
    extra: Vec<(String, String)>,
}

#[tokio::test]
async fn a_name_already_in_use_is_refused_as_already_existing_whatever_the_template_submits() {
    let router = router();
    let name = "topstack";
    let (status_code, created) = create_stack(&router, name, PLAIN_TEMPLATE, &[]).await;
    assert_eq!(
        status_code, 200,
        "the stack that holds the name must be created first, got {status_code} {created}"
    );
    let original_id = xml_text(&created, "StackId").expect("CreateStack answers a StackId");
    wait_for_the_transition().await;

    let rows: Vec<DuplicateCase> = vec![
        DuplicateCase {
            case: "the same template again",
            template: PLAIN_TEMPLATE,
            extra: vec![],
        },
        DuplicateCase {
            case: "a template whose IAM resource the request did not acknowledge",
            template: IAM_TEMPLATE,
            extra: vec![],
        },
        DuplicateCase {
            case: "a template whose macro the request did not acknowledge",
            template: MACRO_TEMPLATE,
            extra: vec![],
        },
        DuplicateCase {
            case: "a template whose parameter value its own declaration refuses",
            template: CONSTRAINED_TEMPLATE,
            extra: vec![
                member("Parameters.member.1.ParameterKey", "Size"),
                member("Parameters.member.1.ParameterValue", "1"),
            ],
        },
        DuplicateCase {
            case: "a template declaring a type the request's own ResourceTypes excludes",
            template: PLAIN_TEMPLATE,
            extra: vec![member("ResourceTypes.member.1", "AWS::SNS::Topic")],
        },
    ];

    for row in rows {
        let case = row.case;
        let (code, body) = create_stack(&router, name, row.template, &row.extra).await;
        assert_eq!(
            code, 400,
            "case {case:?}: a duplicate name is refused, got {code} {body}"
        );
        let answered = refusal(&body);
        let expected = (
            Some("AlreadyExistsException".to_string()),
            Some(format!("Stack [{name}] already exists")),
        );
        assert_eq!(
            answered, expected,
            "case {case:?}: expected {expected:?}, got {answered:?}"
        );
    }

    // ...and the stack that holds the name is untouched: same id, same status, same
    // template.
    let (code, body) = call(&router, "DescribeStacks", &[member("StackName", name)]).await;
    assert_eq!(
        code, 200,
        "the original stack must still be readable, got {code} {body}"
    );
    let still = (
        xml_text(&body, "StackId"),
        xml_text(&body, "StackStatus"),
        xml_texts(&body, "StackName").len(),
    );
    let expected = (
        Some(original_id),
        Some(status::CREATE_COMPLETE.to_string()),
        1,
    );
    assert_eq!(
        still, expected,
        "case \"the stack that holds the name\": expected {expected:?}, got {still:?}"
    );
}

/// One row of the parameter-constraint table: a declaration, the value a request passes for
/// it, and the refusal that value is answered with (`None` where the declaration admits it).
struct ParameterCase {
    case: &'static str,
    declaration: &'static str,
    value: &'static str,
    refused: Option<&'static str>,
}

/// The class the parameter rule covers: every constraint a declaration may state, a value
/// inside and outside each one, the bounds themselves, a list-typed parameter whose value is
/// several values, and a declaration that words its own refusal.
fn parameter_cases() -> Vec<ParameterCase> {
    vec![
        ParameterCase {
            case: "a number below MinValue",
            declaration: r#"{"Type":"Number","MinValue":2,"MaxValue":8}"#,
            value: "1",
            refused: Some("Parameter 'P' must be a number not less than 2"),
        },
        ParameterCase {
            case: "a number above MaxValue",
            declaration: r#"{"Type":"Number","MinValue":2,"MaxValue":8}"#,
            value: "9",
            refused: Some("Parameter 'P' must be a number not greater than 8"),
        },
        ParameterCase {
            case: "a number at the lower bound",
            declaration: r#"{"Type":"Number","MinValue":2,"MaxValue":8}"#,
            value: "2",
            refused: None,
        },
        ParameterCase {
            case: "a number at the upper bound",
            declaration: r#"{"Type":"Number","MinValue":2,"MaxValue":8}"#,
            value: "8",
            refused: None,
        },
        ParameterCase {
            case: "a Number parameter passed something that is no number",
            declaration: r#"{"Type":"Number"}"#,
            value: "eight",
            refused: Some("Parameter 'P' must be a number."),
        },
        ParameterCase {
            case: "a value outside AllowedValues",
            declaration: r#"{"Type":"String","AllowedValues":["dev","prod"]}"#,
            value: "staging",
            refused: Some("Parameter 'P' must be one of AllowedValues"),
        },
        ParameterCase {
            case: "a value AllowedValues admits",
            declaration: r#"{"Type":"String","AllowedValues":["dev","prod"]}"#,
            value: "prod",
            refused: None,
        },
        ParameterCase {
            case: "a value the AllowedPattern does not match",
            declaration: r#"{"Type":"String","AllowedPattern":"[a-z]+"}"#,
            value: "Bad1",
            refused: Some("Parameter 'P' must match pattern [a-z]+"),
        },
        ParameterCase {
            case: "a value the AllowedPattern matches only part of",
            declaration: r#"{"Type":"String","AllowedPattern":"[a-z]+"}"#,
            value: "fine1",
            refused: Some("Parameter 'P' must match pattern [a-z]+"),
        },
        ParameterCase {
            case: "a value the AllowedPattern matches whole",
            declaration: r#"{"Type":"String","AllowedPattern":"[a-z]+"}"#,
            value: "fine",
            refused: None,
        },
        ParameterCase {
            case: "a ConstraintDescription in place of the pattern clause",
            declaration: r#"{"Type":"String","AllowedPattern":"[a-z]+",
                "ConstraintDescription":"Use lowercase letters only."}"#,
            value: "Bad1",
            refused: Some("Parameter P failed to satisfy constraint: Use lowercase letters only."),
        },
        ParameterCase {
            case: "a ConstraintDescription in place of a numeric clause",
            declaration: r#"{"Type":"Number","MinValue":2,
                "ConstraintDescription":"Pick a size from 2 to 8."}"#,
            value: "1",
            refused: Some("Parameter P failed to satisfy constraint: Pick a size from 2 to 8."),
        },
        ParameterCase {
            case: "a value shorter than MinLength",
            declaration: r#"{"Type":"String","MinLength":3,"MaxLength":6}"#,
            value: "ab",
            refused: Some("Parameter 'P' must have length not less than 3"),
        },
        ParameterCase {
            case: "a value longer than MaxLength",
            declaration: r#"{"Type":"String","MinLength":3,"MaxLength":6}"#,
            value: "abcdefg",
            refused: Some("Parameter 'P' must have length not greater than 6"),
        },
        ParameterCase {
            case: "a value at each length bound",
            declaration: r#"{"Type":"String","MinLength":3,"MaxLength":6}"#,
            value: "abcdef",
            refused: None,
        },
        ParameterCase {
            case: "one element of a list-typed value outside AllowedValues",
            declaration: r#"{"Type":"CommaDelimitedList","AllowedValues":["a","b"]}"#,
            value: "a,c",
            refused: Some("Parameter 'P' must be one of AllowedValues"),
        },
        ParameterCase {
            case: "every element of a list-typed value admitted",
            declaration: r#"{"Type":"CommaDelimitedList","AllowedValues":["a","b"]}"#,
            value: "a,b",
            refused: None,
        },
        ParameterCase {
            case: "a declaration that constrains nothing",
            declaration: r#"{"Type":"String"}"#,
            value: "anything at all",
            refused: None,
        },
    ]
}

/// A template declaring one parameter under `declaration` and one resource that needs no
/// owning service.
fn parameterised_template(declaration: &str) -> String {
    format!(
        r#"{{"Parameters":{{"P":{declaration}}},
            "Resources":{{"Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}}}}"#
    )
}

/// The members one parameter value is passed as.
fn passing(value: &str) -> Vec<(String, String)> {
    vec![
        member("Parameters.member.1.ParameterKey", "P"),
        member("Parameters.member.1.ParameterValue", value),
    ]
}

#[tokio::test]
async fn a_parameter_value_is_held_to_the_constraints_its_declaration_states() {
    let router = router();
    for (index, row) in parameter_cases().into_iter().enumerate() {
        let case = row.case;
        let name = format!("param-{index}");
        let (code, body) = create_stack(
            &router,
            &name,
            &parameterised_template(row.declaration),
            &passing(row.value),
        )
        .await;
        match row.refused {
            Some(message) => {
                assert_eq!(
                    code, 400,
                    "case {case:?}: the value must be refused, got {code} {body}"
                );
                let answered = refusal(&body);
                let expected = (
                    Some("ValidationError".to_string()),
                    Some(message.to_string()),
                );
                assert_eq!(
                    answered, expected,
                    "case {case:?}: expected {expected:?}, got {answered:?}"
                );
                // Nothing was created: the refusal is of the REQUEST, before any stack
                // exists under that name.
                let (read, read_body) =
                    call(&router, "DescribeStacks", &[member("StackName", &name)]).await;
                assert_eq!(
                    read, 400,
                    "case {case:?}: a refused create leaves no stack, got {read} {read_body}"
                );
            }
            None => assert_eq!(
                code, 200,
                "case {case:?}: the declaration admits the value, got {code} {body}"
            ),
        }
    }
}

#[tokio::test]
async fn an_update_is_held_to_the_same_parameter_constraints_as_a_create() {
    let router = router();
    let template = parameterised_template(r#"{"Type":"Number","MinValue":2,"MaxValue":8}"#);
    let (code, body) = create_stack(&router, "param-update", &template, &passing("4")).await;
    assert_eq!(
        code, 200,
        "the stack an update is applied to must exist first, got {code} {body}"
    );
    wait_for_the_transition().await;

    // (case, the value the update passes, the refusal it is answered with)
    let rows: Vec<(&str, &str, Option<&str>)> = vec![
        (
            "an update passing a value below MinValue",
            "1",
            Some("Parameter 'P' must be a number not less than 2"),
        ),
        (
            "an update passing a value above MaxValue",
            "9",
            Some("Parameter 'P' must be a number not greater than 8"),
        ),
        ("an update passing a value in range", "6", None),
    ];
    for (case, value, refused) in rows {
        let mut members = vec![
            member("StackName", "param-update"),
            member("TemplateBody", &template),
        ];
        members.extend(passing(value));
        let (code, body) = call(&router, "UpdateStack", &members).await;
        match refused {
            Some(message) => {
                assert_eq!(
                    code, 400,
                    "case {case:?}: the value must be refused, got {code} {body}"
                );
                let answered = refusal(&body);
                let expected = (
                    Some("ValidationError".to_string()),
                    Some(message.to_string()),
                );
                assert_eq!(
                    answered, expected,
                    "case {case:?}: expected {expected:?}, got {answered:?}"
                );
            }
            None => {
                assert_eq!(
                    code, 200,
                    "case {case:?}: the declaration admits the value, got {code} {body}"
                );
                wait_for_the_transition().await;
            }
        }
    }
}

/// A template declaring one resource of `resource_type`, needing no properties.
fn template_of_type(resource_type: &str) -> String {
    format!(r#"{{"Resources":{{"R":{{"Type":"{resource_type}"}}}}}}"#)
}

#[tokio::test]
async fn the_resource_types_a_request_states_bound_the_template_it_submits() {
    let router = router();
    // (case, the ResourceTypes entries the request states, the type its template declares,
    // the refusal it is answered with)
    let rows: Vec<(&str, &[&str], &str, Option<String>)> = vec![
        (
            "a list that names the declared type",
            &["AWS::CloudFormation::WaitConditionHandle"],
            "AWS::CloudFormation::WaitConditionHandle",
            None,
        ),
        (
            "a list that excludes the declared type",
            &["AWS::CloudFormation::WaitConditionHandle"],
            "AWS::SNS::Topic",
            Some(
                "Resource type AWS::SNS::Topic is not allowed by parameter ResourceTypes \
                 [AWS::CloudFormation::WaitConditionHandle]"
                    .to_string(),
            ),
        ),
        (
            "a list naming the service of the declared type",
            &["AWS::SNS::*"],
            "AWS::SNS::Topic",
            None,
        ),
        (
            "a list naming another service",
            &["AWS::SQS::*"],
            "AWS::SNS::Topic",
            Some(
                "Resource type AWS::SNS::Topic is not allowed by parameter ResourceTypes \
                 [AWS::SQS::*]"
                    .to_string(),
            ),
        ),
        (
            "a list naming every published resource",
            &["AWS::*"],
            "AWS::SNS::Topic",
            None,
        ),
        (
            "more than one entry, one of which covers the type",
            &["AWS::SQS::*", "AWS::SNS::Topic"],
            "AWS::SNS::Topic",
            None,
        ),
        (
            "more than one entry, none of which covers the type",
            &["AWS::SQS::*", "AWS::S3::Bucket"],
            "AWS::SNS::Topic",
            Some(
                "Resource type AWS::SNS::Topic is not allowed by parameter ResourceTypes \
                 [AWS::SQS::*, AWS::S3::Bucket]"
                    .to_string(),
            ),
        ),
        (
            "no list at all, which bounds nothing",
            &[],
            "AWS::SNS::Topic",
            None,
        ),
    ];

    for (index, (case, allowed, resource_type, refused)) in rows.into_iter().enumerate() {
        let name = format!("rtypes-{index}");
        let extra: Vec<(String, String)> = allowed
            .iter()
            .enumerate()
            .map(|(position, entry)| {
                member(&format!("ResourceTypes.member.{}", position + 1), entry)
            })
            .collect();
        let (code, body) =
            create_stack(&router, &name, &template_of_type(resource_type), &extra).await;
        match &refused {
            Some(message) => {
                assert_eq!(
                    code, 400,
                    "case {case:?}: the create must be refused, got {code} {body}"
                );
                let answered = refusal(&body);
                let expected = (
                    Some("ValidationError".to_string()),
                    Some(message.clone()),
                );
                assert_eq!(
                    answered, expected,
                    "case {case:?}: expected {expected:?}, got {answered:?}"
                );
            }
            None => {
                assert_eq!(
                    code, 200,
                    "case {case:?}: the list covers the template, got {code} {body}"
                );
                wait_for_the_transition().await;
            }
        }
    }

    // "Only one of the Capabilities and ResourceType parameters can be specified": the pair
    // is refused for the REQUEST SHAPE, whatever the template says.
    let (code, body) = create_stack(
        &router,
        "rtypes-both",
        &template_of_type("AWS::SNS::Topic"),
        &[
            member("ResourceTypes.member.1", "AWS::*"),
            member("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        code, 400,
        "case \"both bounds stated\": the request must be refused, got {code} {body}"
    );
    let answered = refusal(&body);
    let expected = (
        Some("ValidationError".to_string()),
        Some("ResourceTypes and Capabilities cannot be specified at the same time.".to_string()),
    );
    assert_eq!(
        answered, expected,
        "case \"both bounds stated\": expected {expected:?}, got {answered:?}"
    );
}

#[tokio::test]
async fn an_update_is_held_to_the_resource_types_its_request_states() {
    let router = router();
    let (code, body) = create_stack(&router, "rtypes-update", PLAIN_TEMPLATE, &[]).await;
    assert_eq!(
        code, 200,
        "the stack the update changes must exist first, got {code} {body}"
    );
    wait_for_the_transition().await;

    // The update ADDS a resource of a type its own list excludes.
    let revised = r#"{"Resources":{
        "Handle":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
        "Topic":{"Type":"AWS::SNS::Topic"}}}"#;
    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "rtypes-update"),
            member("TemplateBody", revised),
            member("ResourceTypes.member.1", "AWS::CloudFormation::*"),
        ],
    )
    .await;
    assert_eq!(
        code, 400,
        "case \"an update adding an excluded type\": the update must be refused, got {code} {body}"
    );
    let answered = refusal(&body);
    let expected = (
        Some("ValidationError".to_string()),
        Some(
            "Resource type AWS::SNS::Topic is not allowed by parameter ResourceTypes \
             [AWS::CloudFormation::*]"
                .to_string(),
        ),
    );
    assert_eq!(
        answered, expected,
        "case \"an update adding an excluded type\": expected {expected:?}, got {answered:?}"
    );

    // The same update under a list that covers both types is carried out.
    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "rtypes-update"),
            member("TemplateBody", revised),
            member("ResourceTypes.member.1", "AWS::*"),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"an update whose list covers both types\": expected it to be carried out, got \
         {code} {body}"
    );
}

/// A JSON template body of exactly `bytes` characters, padded with `pad` — a single
/// character, so a body of any size is one unbroken run of text or, where the padding is a
/// space, a body carrying separators.
///
/// The padding goes in `Metadata`, a section whose content the template format bounds by
/// nothing, rather than in `Description`, which a template may carry only 1024 bytes of.
fn template_of_size(bytes: usize, pad: char) -> String {
    const WRAPPER: usize = 23;
    format!(
        r#"{{"Metadata":{{"Pad":"{}"}}}}"#,
        pad.to_string().repeat(bytes - WRAPPER)
    )
}

/// The value a refusal of `body` quotes, as the RULE states it: the body itself where it
/// fits the quoted width, else as much of its leading text as fits beside the marker naming
/// how many characters it holds in all, cut at a separator where the kept text has one.
///
/// Spelled here from the rule rather than taken from the handler, so the suite would catch
/// the handler quoting something else.
fn quoted(body: &str) -> String {
    let characters = body.chars().count();
    if characters <= state::QUOTED_TEMPLATE_PREFIX {
        return body.to_string();
    }
    let kept: String = body.chars().take(state::QUOTED_TEMPLATE_PREFIX).collect();
    let kept = match kept.rfind(' ') {
        Some(last) => kept[..last + 1].to_string(),
        None => kept,
    };
    format!("{kept}...<{characters} chars total>")
}

#[tokio::test]
async fn an_inline_template_body_past_its_maximum_is_refused_quoting_the_value() {
    let router = router();
    // (case, the body's size, the character it is padded with, whether it is within the
    // bound)
    let rows: Vec<(&str, usize, char, bool)> = vec![
        ("a small body", 64, 'd', true),
        ("at the published maximum", TEMPLATE_BODY_MAX_BYTES, 'd', true),
        (
            "one character past it",
            TEMPLATE_BODY_MAX_BYTES + 1,
            'd',
            false,
        ),
        ("far past it", TEMPLATE_BODY_MAX_BYTES * 2, 'd', false),
        (
            "a body past it whose text carries separators",
            TEMPLATE_BODY_MAX_BYTES + 1,
            ' ',
            false,
        ),
    ];
    for (index, (case, size, pad, within)) in rows.into_iter().enumerate() {
        let body_text = template_of_size(size, pad);
        let (code, body) =
            create_stack(&router, &format!("sized-{index}"), &body_text, &[]).await;
        if within {
            assert_eq!(
                code, 200,
                "case {case:?}: a {size}-character body is within the bound, got {code} {body}"
            );
            wait_for_the_transition().await;
            continue;
        }
        assert_eq!(
            code, 400,
            "case {case:?}: a {size}-character body is past the bound, got {code} {body}"
        );
        let (answered_code, answered_message) = refusal(&body);
        assert_eq!(
            answered_code.as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
        let message = answered_message.unwrap_or_default();
        // The constraint sentence is fixed; the variable parts are the value the request
        // sent — quoted, and bounded, since the member is one a caller sizes — and the
        // count of characters it holds in all.
        let expected = format!(
            "1 validation error detected: Value '{}' at 'templateBody' failed to satisfy \
             constraint: Member must have length less than or equal to {TEMPLATE_BODY_MAX_BYTES}",
            quoted(&body_text)
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            message.contains(&format!("<{size} chars total>"))
                && message.len() < state::QUOTED_TEMPLATE_PREFIX + 200,
            "case {case:?}: the refusal must quote the value's leading text and its total \
             length, bounded; got {message:?}"
        );
    }

    // One refusal pinned to the LITERAL message a known body is answered with, so the suite
    // states the whole sentence once rather than only agreeing with how it is built: a body
    // of 51201 `d` characters under the `{"Metadata":{"Pad":"…"}}` wrapper, quoted to the
    // width a refusal keeps — which the sentence below states, so a changed width fails here.
    assert_eq!(
        state::QUOTED_TEMPLATE_PREFIX, 100,
        "the quoted width is the one the recorded refusals of an over-long body show"
    );
    let body_text = template_of_size(TEMPLATE_BODY_MAX_BYTES + 1, 'd');
    let (code, body) = create_stack(&router, "sized-literal", &body_text, &[]).await;
    assert_eq!(
        code, 400,
        "case \"the pinned refusal\": the body is past the bound, got {code} {body}"
    );
    let pinned = format!(
        "1 validation error detected: Value '{{\"Metadata\":{{\"Pad\":\"{}...<51201 chars \
         total>' at 'templateBody' failed to satisfy constraint: Member must have length less \
         than or equal to 51200",
        // The quoted value is the body's leading text: the wrapper, and `d`s up to the width.
        "d".repeat(state::QUOTED_TEMPLATE_PREFIX - r#"{"Metadata":{"Pad":""#.len())
    );
    assert_eq!(
        xml_text(&body, "Message").as_deref(),
        Some(pinned.as_str()),
        "case \"the pinned refusal\": expected {pinned:?}, got {body}"
    );
}

/// One row of the property-validation table: a declaration whose property value its type's
/// contract does not admit, and the property that fails.
struct PropertyCase {
    case: &'static str,
    logical_id: &'static str,
    resource_type: &'static str,
    properties: &'static str,
    property: &'static str,
    admitted: &'static str,
}

/// The class the pre-deployment property rule covers: types whose contract publishes the
/// values a property admits, including one the issue that filed the rule did not name.
fn property_cases() -> Vec<PropertyCase> {
    vec![
        PropertyCase {
            case: "a parameter type Systems Manager does not publish",
            logical_id: "Param",
            resource_type: "AWS::SSM::Parameter",
            properties: r#"{"Name":"validated-param","Type":"NotAType","Value":"x"}"#,
            property: "Type",
            admitted: "String, StringList, SecureString",
        },
        PropertyCase {
            case: "a canned ACL Amazon S3 does not publish",
            logical_id: "Data",
            resource_type: "AWS::S3::Bucket",
            properties: r#"{"BucketName":"validated-bucket","AccessControl":"EverybodyRead"}"#,
            property: "AccessControl",
            admitted: "AuthenticatedRead, AwsExecRead, BucketOwnerFullControl, \
                       BucketOwnerRead, LogDeliveryWrite, Private, PublicRead, \
                       PublicReadWrite",
        },
    ]
}

/// The template one property case deploys.
fn property_template(row: &PropertyCase) -> String {
    format!(
        r#"{{"Resources":{{"{}":{{"Type":"{}","Properties":{}}}}}}}"#,
        row.logical_id, row.resource_type, row.properties
    )
}

#[tokio::test]
async fn a_property_value_the_type_does_not_admit_fails_before_anything_is_provisioned() {
    for (index, row) in property_cases().into_iter().enumerate() {
        let case = row.case;
        let (router, _engine) = router_with_engine();
        let name = format!("validated-{index}");
        let (code, body) = create_stack(&router, &name, &property_template(&row), &[]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the create itself is accepted, got {code} {body}"
        );
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        // The resource was never asked for, so the stack's own event stream carries no
        // event about it at all.
        let (code, body) =
            call(&router, "DescribeStackEvents", &[member("StackName", &name)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the stack's events must be readable, got {code} {body}"
        );
        let resource_events: Vec<String> = xml_blocks(&body, "member")
            .iter()
            .filter(|event| {
                xml_text(event, "LogicalResourceId").as_deref() == Some(row.logical_id)
            })
            .map(|event| xml_text(event, "ResourceStatus").unwrap_or_default())
            .collect();
        assert!(
            resource_events.is_empty(),
            "case {case:?}: validation must stop the create before {} is provisioned, got \
             {resource_events:?}",
            row.logical_id
        );

        // ...and the validation result IS reported, as the detail `DescribeEvents` carries:
        // which validation failed, on which resource, and what the property admits.
        let (code, body) = call(&router, "DescribeEvents", &[member("StackName", &name)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the operation's events must be readable, got {code} {body}"
        );
        let validations: Vec<(String, String, String, String, String)> = xml_blocks(&body, "member")
            .iter()
            .filter(|event| xml_text(event, "ValidationName").is_some())
            .map(|event| {
                (
                    xml_text(event, "LogicalResourceId").unwrap_or_default(),
                    xml_text(event, "ResourceType").unwrap_or_default(),
                    xml_text(event, "ValidationName").unwrap_or_default(),
                    xml_text(event, "ValidationStatus").unwrap_or_default(),
                    xml_text(event, "ValidationStatusReason").unwrap_or_default(),
                )
            })
            .collect();
        let expected = vec![(
            row.logical_id.to_string(),
            row.resource_type.to_string(),
            "PROPERTY_VALIDATION".to_string(),
            "FAILED".to_string(),
            format!(
                "Property validation failure: [Value for property {{/{}}} of resource [{}] \
                 must be one of [{}]]",
                row.property, row.logical_id, row.admitted
            ),
        )];
        assert_eq!(
            validations, expected,
            "case {case:?}: expected {expected:?}, got {validations:?}"
        );

        // The stack itself did not deploy: it carries the validation's own reason.
        let (code, body) = call(&router, "DescribeStacks", &[member("StackName", &name)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the stack must be readable, got {code} {body}"
        );
        let settled = xml_text(&body, "StackStatus").unwrap_or_default();
        assert_eq!(
            settled,
            status::ROLLBACK_COMPLETE,
            "case {case:?}: a failed create rolls back, got {settled:?} {body}"
        );
    }
}

#[tokio::test]
async fn disable_validation_defers_the_same_property_error_to_provisioning() {
    let (router, _engine) = router_with_engine();
    let row = &property_cases()[0];
    let (code, body) = create_stack(
        &router,
        "unvalidated",
        &property_template(row),
        &[member("DisableValidation", "true")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"validation disabled\": the create is accepted, got {code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    // With the stage off, the resource IS asked for, and the owning service's own refusal
    // is what the resource's events report.
    let (code, body) = call(
        &router,
        "DescribeStackEvents",
        &[member("StackName", "unvalidated")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"validation disabled\": the stack's events must be readable, got {code} {body}"
    );
    let statuses: Vec<String> = xml_blocks(&body, "member")
        .iter()
        .filter(|event| xml_text(event, "LogicalResourceId").as_deref() == Some(row.logical_id))
        .map(|event| xml_text(event, "ResourceStatus").unwrap_or_default())
        .collect();
    assert!(
        statuses.contains(&status::CREATE_FAILED.to_string()),
        "case \"validation disabled\": the resource must be provisioned and fail there, got \
         {statuses:?}"
    );
    // ...and no validation result is reported, because no validation ran.
    let (code, body) = call(
        &router,
        "DescribeEvents",
        &[member("StackName", "unvalidated")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"validation disabled\": the operation's events must be readable, got {code} {body}"
    );
    let validations: Vec<String> = xml_blocks(&body, "member")
        .iter()
        .filter_map(|event| xml_text(event, "ValidationName"))
        .collect();
    assert!(
        validations.is_empty(),
        "case \"validation disabled\": no validation ran, so none is reported, got \
         {validations:?}"
    );
}

/// A template declaring `count` resources of a type no owning service has to be asked
/// about, named `R000`.. so the pages are stable.
fn handles(count: usize) -> String {
    let declarations: Vec<String> = (0..count)
        .map(|index| {
            format!(r#""R{index:03}":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}"#)
        })
        .collect();
    format!(r#"{{"Resources":{{{}}}}}"#, declarations.join(","))
}

/// List one stack's resources, optionally resuming at `token`.
async fn list_resources(
    router: &smithy_server_runtime::MultiServiceRouter,
    stack: &str,
    token: Option<&str>,
) -> (u16, String) {
    let mut members = vec![member("StackName", stack)];
    if let Some(token) = token {
        members.push(member("NextToken", token));
    }
    call(router, "ListStackResources", &members).await
}

#[tokio::test]
async fn a_pagination_token_is_scoped_to_the_stack_whose_resources_it_pages() {
    let router = router();
    // Two stacks, each with more resources than one page carries, so each read really hands
    // out a token.
    for name in ["paged-here", "paged-there"] {
        let (code, body) = create_stack(&router, name, &handles(PAGE_SIZE + 1), &[]).await;
        assert_eq!(
            code, 200,
            "the stacks whose resources are paged must be created first, got {code} {body}"
        );
    }
    wait_for_the_transition().await;

    let (code, body) = list_resources(&router, "paged-here", None).await;
    assert_eq!(
        code, 200,
        "the first page of one stack's resources must be served, got {code} {body}"
    );
    let token =
        xml_text(&body, "NextToken").expect("a stack of more than one page hands out a token");

    // (case, the stack the token is presented on, the token, the refusal it is answered
    // with — `None` where it resumes the walk)
    let rows: Vec<(&str, &str, String, Option<String>)> = vec![
        (
            "the stack the token was minted for",
            "paged-here",
            token.clone(),
            None,
        ),
        (
            "another stack's collection of the same kind",
            "paged-there",
            token.clone(),
            Some("Hash mismatch.".to_string()),
        ),
        (
            "a value this service never issued",
            "paged-here",
            "not-a-token-we-issued".to_string(),
            Some("not-a-token-we-issued is not a valid NextToken".to_string()),
        ),
        (
            "a cursor of another collection entirely",
            "paged-here",
            state::encode_next_token(state::PageOf::StackSummaries, 0),
            Some(format!(
                "{} is not a valid NextToken",
                state::encode_next_token(state::PageOf::StackSummaries, 0)
            )),
        ),
        (
            "a cursor of this collection carrying no scope at all",
            "paged-here",
            state::encode_next_token(state::PageOf::StackResourceSummaries, 0),
            Some(format!(
                "{} is not a valid NextToken",
                state::encode_next_token(state::PageOf::StackResourceSummaries, 0)
            )),
        ),
    ];
    for (case, stack, presented, refused) in rows {
        let (code, body) = list_resources(&router, stack, Some(&presented)).await;
        match refused {
            Some(message) => {
                assert_eq!(
                    code, 400,
                    "case {case:?}: the token must be refused, got {code} {body}"
                );
                let answered = refusal(&body);
                let expected = (Some("ValidationError".to_string()), Some(message));
                assert_eq!(
                    answered, expected,
                    "case {case:?}: expected {expected:?}, got {answered:?}"
                );
            }
            None => {
                assert_eq!(
                    code, 200,
                    "case {case:?}: the stack's own token resumes its walk, got {code} {body}"
                );
                // The page it resumes is the one past the first, which is what the token
                // was minted for.
                let resumed = xml_texts(&body, "LogicalResourceId");
                assert_eq!(
                    resumed.len(),
                    1,
                    "case {case:?}: expected the last resource alone, got {resumed:?}"
                );
            }
        }
    }
}
