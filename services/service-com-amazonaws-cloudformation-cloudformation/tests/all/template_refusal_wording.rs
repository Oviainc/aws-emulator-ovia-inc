// SPDX-License-Identifier: CC0-1.0

//! How CloudFormation WORDS the refusal of a template it will not deploy, end to end over
//! the awsQuery wire.
//!
//! A refusal's message is the only machine-readable description of what was wrong, so the
//! three rules below are about its construction rather than about the status and the code:
//!
//! * **a dependency CYCLE is reported as the chain it is — each logical id followed by the
//!   resource it depends on, beginning at the member whose id sorts first** — so the list
//!   comes from the request's own ids and dependency relation, and the same body is refused
//!   with the same list on every run;
//! * **a section past its per-template quota is counted by the noun that limit is worded
//!   with, the count taken from the request and the maximum from the ceiling
//!   `DescribeAccountLimits` advertises** — and the noun belongs to the one limit, so the
//!   outputs limit and the parameters limit do not borrow each other's;
//! * **a function no template may use is refused by naming it and then ONE fixed roster of
//!   supported functions** — fixed text, independent of the template, which is why the
//!   roster never advertises the function the same response is refusing.
//!
//! Each table spans the class its rule generalises over: cycles differing in length, in the
//! relation that closes them, in where the first-sorting member stands among the
//! declarations, in the presence of resources outside the cycle and in both template
//! spellings; the quota rows differ in the section metered and in how far past the ceiling
//! the body is; the roster rows differ in the offending function, in the section it appears
//! in, and in which operation the body was submitted to.

use crate::stack_lifecycle::{
    call, create_stack, member, router, wait_for_the_transition, xml_text,
};
use crate::template_validation::refusal;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    account_limit_name, template::MAX_PARAMETERS_PER_TEMPLATE,
};
use smithy_server_runtime::MultiServiceRouter;

/// A template that is well-formed and deploys, used as the stack the `UpdateStack` and
/// `CreateChangeSet` rows submit their bodies against.
const DEPLOYABLE: &str =
    r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"wording-topic"}}}}"#;

// ---------------------------------------------------------------------------
// A dependency cycle names its resources in declaration order
// ---------------------------------------------------------------------------

/// One cyclic template, and the logical ids its refusal must name, in order.
struct Cycle {
    case: &'static str,
    template: &'static str,
    /// The cycle's members as the refusal must list them: from the member whose id sorts
    /// first, then the resource that one depends on, and so on round the cycle.
    chain: &'static [&'static str],
}

/// The class the rule covers: a template no creation order satisfies.
const CYCLES: &[Cycle] = &[
    Cycle {
        // The member that sorts first is the one declared LAST here, and the chain from it
        // runs the other way round the pair than the declarations do.
        case: "two resources whose dependency chain runs against their declaration order",
        template: r#"{"Resources":{"Zulu":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Fn::GetAtt":["Alfa","Value"]}}},"Alfa":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Fn::GetAtt":["Zulu","Value"]}}}}}"#,
        chain: &["Alfa", "Zulu"],
    },
    Cycle {
        case: "two resources that Ref each other",
        template: r#"{"Resources":{"Alfa":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Ref":"Bravo"}}},"Bravo":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Ref":"Alfa"}}}}}"#,
        chain: &["Alfa", "Bravo"],
    },
    Cycle {
        // Three resources closed through `DependsOn`: the chain follows the dependency, so
        // it is NOT the order the three are declared in.
        case: "three resources whose DependsOn closes the cycle",
        template: r#"{"Resources":{"Charlie":{"Type":"AWS::SNS::Topic","DependsOn":"Alfa"},"Bravo":{"Type":"AWS::SNS::Topic","DependsOn":"Charlie"},"Alfa":{"Type":"AWS::SNS::Topic","DependsOn":"Bravo"}}}"#,
        chain: &["Alfa", "Bravo", "Charlie"],
    },
    Cycle {
        case: "four resources, the cycle closed through Fn::Sub and Fn::GetAtt",
        template: r#"{"Resources":{"Delta":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Fn::Sub":"${Charlie}"}}},"Charlie":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Fn::GetAtt":["Bravo","Value"]}}},"Bravo":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Ref":"Alfa"}}},"Alfa":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Fn::Sub":"${Delta}"}}}}}"#,
        chain: &["Alfa", "Delta", "Charlie", "Bravo"],
    },
    Cycle {
        // Only the resources that FORM the cycle are named, however many the template
        // declares beside them and wherever they stand.
        case: "a cycle among resources declared between two that are not in it",
        template: r#"{"Resources":{"Standalone":{"Type":"AWS::SNS::Topic"},"Yankee":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Ref":"Xray"}}},"Xray":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Ref":"Yankee"}}},"Trailing":{"Type":"AWS::SNS::Topic"}}}"#,
        chain: &["Xray", "Yankee"],
    },
    Cycle {
        // The same rule over the other spelling a body may be submitted in.
        case: "a YAML body whose cycle is spelled with short-form intrinsics",
        template: "Resources:\n  Oscar:\n    Type: AWS::SSM::Parameter\n    Properties:\n      Type: String\n      Value: !Ref November\n  November:\n    Type: AWS::SSM::Parameter\n    Properties:\n      Type: String\n      Value: !Ref Oscar\n",
        chain: &["November", "Oscar"],
    },
];

/// The refusal a cyclic template is answered with: the fixed text, then the cycle's logical
/// ids comma-and-space separated, in the order the chain runs.
fn expected_cycle_message(chain: &[&str]) -> String {
    format!(
        "Circular dependency between resources: [{}]",
        chain.join(", ")
    )
}

#[tokio::test]
async fn a_cycle_is_reported_as_the_chain_it_is() {
    let router = router();
    for row in CYCLES {
        let case = row.case;
        let expected = expected_cycle_message(row.chain);
        let (status, body) = create_stack(&router, "cycle-refused", row.template, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the cyclic template refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {}",
            refusal(&body)
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // The list is a function of the request, so the same body answers the same list
        // again rather than whatever order a walk happened to take this time.
        let (_, again) = create_stack(&router, "cycle-refused", row.template, &[]).await;
        let repeated = xml_text(&again, "Message").unwrap_or_default();
        assert_eq!(
            repeated, message,
            "case {case:?}: expected the same refusal on a second submit, got {repeated:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A section past its quota is counted by that limit's own noun
// ---------------------------------------------------------------------------

/// The limit `DescribeAccountLimits` advertises under `name`, as the count it is.
async fn advertised_limit(router: &MultiServiceRouter, name: &str) -> usize {
    let (status, body) = call(router, "DescribeAccountLimits", &[]).await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected DescribeAccountLimits to answer, got {status} {body}"
    );
    // The limits are reported as `<Name>`/`<Value>` pairs; the pair under test is found by
    // its name so the row does not depend on the order they are reported in.
    let marker = format!("<Name>{name}</Name>");
    let position = body
        .find(&marker)
        .unwrap_or_else(|| panic!("case {name:?}: expected the limit to be advertised, got {body}"));
    xml_text(&body[position..], "Value")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("case {name:?}: expected a numeric limit, got {body}"))
}

/// A template declaring `count` outputs beside one resource.
fn outputs_template(count: usize) -> String {
    let outputs: serde_json::Map<String, serde_json::Value> = (0..count)
        .map(|index| {
            (
                format!("Out{index:04}"),
                serde_json::json!({"Value": format!("value-{index}")}),
            )
        })
        .collect();
    serde_json::json!({
        "Resources": {"Handle": {"Type": "AWS::CloudFormation::WaitConditionHandle"}},
        "Outputs": outputs,
    })
    .to_string()
}

/// A template declaring `count` parameters beside one resource.
fn parameters_template(count: usize) -> String {
    let parameters: serde_json::Map<String, serde_json::Value> = (0..count)
        .map(|index| {
            (
                format!("Param{index:04}"),
                serde_json::json!({"Type": "String", "Default": "value"}),
            )
        })
        .collect();
    serde_json::json!({
        "Parameters": parameters,
        "Resources": {"Handle": {"Type": "AWS::CloudFormation::WaitConditionHandle"}},
    })
    .to_string()
}

#[tokio::test]
async fn an_over_quota_section_is_counted_by_its_own_limits_noun() {
    let router = router();
    // The ceiling the account is held to is the one the account advertises, read from the
    // service rather than spelled here, so the refusal's second number is pinned to it.
    let outputs_limit = advertised_limit(&router, account_limit_name::STACK_OUTPUTS).await;

    // How far past the ceiling the body is comes from the request: one entry past it, a few
    // past it, and a whole page past it.
    for over_by in [1usize, 3, 100] {
        let declared = outputs_limit + over_by;
        let expected = format!(
            "Template format error: Outputs count {declared} is greater than max allowed \
             {outputs_limit}"
        );
        let (status, body) = create_stack(
            &router,
            "outputs-over-quota",
            &outputs_template(declared),
            &[],
        )
        .await;
        assert_eq!(
            status, 400,
            "case \"{declared} outputs\": expected the body refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case \"{declared} outputs\": expected ValidationError, got {}",
            refusal(&body)
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert_eq!(
            message, expected,
            "case \"{declared} outputs\": expected {expected:?}, got {message:?}"
        );
    }

    // The noun belongs to the ONE limit: the parameters limit is counted in the singular
    // where the outputs limit is counted by the section's plural, so neither refusal may be
    // reworded into the other's shape.
    let declared = MAX_PARAMETERS_PER_TEMPLATE + 1;
    let expected = format!(
        "Template format error: Parameter count {declared} is greater than max allowed {}",
        MAX_PARAMETERS_PER_TEMPLATE
    );
    let (status, body) = create_stack(
        &router,
        "parameters-over-quota",
        &parameters_template(declared),
        &[],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"{declared} parameters\": expected the body refused, got {status} {body}"
    );
    let message = xml_text(&body, "Message").unwrap_or_default();
    assert_eq!(
        message, expected,
        "case \"{declared} parameters\": expected {expected:?}, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// One fixed roster of supported functions
// ---------------------------------------------------------------------------

/// The roster the "unsupported function" refusal advertises, in the order the service lists
/// it.
///
/// Recorded behaviour: the roster is not in the Smithy model, and it is FIXED text — nothing
/// of the request reaches it, so the same list answers every refusal of every template.
const ADVERTISED_ROSTER: &str = "Fn::GetStackOutput, Fn::AccountIdFromAlias, \
                                 Fn::GetConstantAZs, Fn::Base64, Fn::GetAtt, Fn::GetAZs, \
                                 Fn::ImportValue, Fn::Join, Fn::Split, Fn::FindInMap, \
                                 Fn::Select, Ref, Fn::Equals, Fn::If, Fn::Not, Condition, \
                                 Fn::And, Fn::Or, Fn::Contains, Fn::EachMemberEquals, \
                                 Fn::EachMemberIn, Fn::ValueOf, Fn::ValueOfAll, Fn::RefAll, \
                                 Fn::Sub, Fn::Cidr";

/// One template using a function no template may use, and the function its refusal must
/// name.
struct Unsupported {
    case: &'static str,
    template: &'static str,
    function: &'static str,
}

/// The class the rule covers: an intrinsic the template may not use, wherever it stands.
const UNSUPPORTED: &[Unsupported] = &[
    Unsupported {
        case: "a function no template may use, in the Outputs section",
        template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"X":{"Value":{"Fn::NoSuchIntrinsic":["alpha","beta"]}}}}"#,
        function: "Fn::NoSuchIntrinsic",
    },
    Unsupported {
        case: "a function no template may use, inside a resource's properties",
        template: r#"{"Resources":{"Param":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":{"Fn::Bogus":"anything"}}}}}"#,
        function: "Fn::Bogus",
    },
    Unsupported {
        case: "a misspelled documented function",
        template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"X":{"Value":{"Fn::Jion":["-",["a","b"]]}}}}"#,
        function: "Fn::Jion",
    },
    Unsupported {
        // The language-extensions directives are functions only the transform that defines
        // them makes available, so a template that does not declare it may not use one —
        // and the roster this response advertises must not list the function it refuses.
        case: "Fn::Length in a template that does not declare the transform",
        template: r#"{"Parameters":{"Items":{"Type":"CommaDelimitedList","Default":"a,b"}},"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"Count":{"Value":{"Fn::Length":{"Ref":"Items"}}}}}"#,
        function: "Fn::Length",
    },
    Unsupported {
        case: "Fn::ToJsonString in a template that does not declare the transform",
        template: r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}},"Outputs":{"Rendered":{"Value":{"Fn::ToJsonString":{"a":1}}}}}"#,
        function: "Fn::ToJsonString",
    },
];

/// The refusal a template using `function` is answered with.
fn expected_roster_message(function: &str) -> String {
    format!(
        "Template Error: Encountered unsupported function: {function} Supported functions \
         are: [{ADVERTISED_ROSTER}]"
    )
}

#[tokio::test]
async fn an_unsupported_function_is_refused_with_one_fixed_roster() {
    let router = router();
    // The stack the revision-submitting operations are held against, deployed from a
    // template that resolves.
    let (status, body) = create_stack(&router, "roster-holder", DEPLOYABLE, &[]).await;
    assert_eq!(
        status, 200,
        "expected the holder stack to be created, got {status} {body}"
    );
    wait_for_the_transition().await;

    for row in UNSUPPORTED {
        let case = row.case;
        let expected = expected_roster_message(row.function);
        // Every operation that takes a template body gives the same verdict in the same
        // words — the one that submits a body for deployment and the one whose whole answer
        // IS a verdict on it.
        for (action, members) in [
            (
                "CreateStack",
                vec![
                    member("StackName", "roster-refused"),
                    member("TemplateBody", row.template),
                ],
            ),
            (
                "UpdateStack",
                vec![
                    member("StackName", "roster-holder"),
                    member("TemplateBody", row.template),
                ],
            ),
            (
                "CreateChangeSet",
                vec![
                    member("StackName", "roster-holder"),
                    member("ChangeSetName", "roster-refused"),
                    member("TemplateBody", row.template),
                ],
            ),
            (
                "ValidateTemplate",
                vec![member("TemplateBody", row.template)],
            ),
        ] {
            let (status, body) = call(&router, action, &members).await;
            assert_eq!(
                status, 400,
                "case {case:?} through {action}: expected the body refused, got {status} \
                 {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?} through {action}: expected ValidationError, got {}",
                refusal(&body)
            );
            let message = xml_text(&body, "Message").unwrap_or_default();
            assert_eq!(
                message, expected,
                "case {case:?} through {action}: expected {expected:?}, got {message:?}"
            );
            // The invariant behind the fixed roster: a response may not advertise the very
            // function it is refusing.
            let advertised = message
                .split_once('[')
                .map(|(_, roster)| roster.to_string())
                .unwrap_or_default();
            assert!(
                !advertised.contains(row.function),
                "case {case:?} through {action}: the roster must not list the refused \
                 function, got {advertised:?}"
            );
        }
    }
}
