// SPDX-License-Identifier: CC0-1.0

//! A parameter declared under an SSM PARAMETER TYPE is deployed with the value Parameter
//! Store holds under the name it supplies.
//!
//! RULE: "AWS::SSM::Parameter::Value<String>: The parameter value corresponds to an existing
//! Systems Manager parameter in Parameter Store" — what the request (or the declaration's own
//! `Default`) supplies for such a parameter is the NAME of a parameter, and "when you use an
//! SSM parameter type, CloudFormation retrieves the value of the Systems Manager parameter
//! and uses it for your stack". So the VALUE in force for that parameter — the value the
//! template's `Rules` assertions are checked against, and the value its own processing reads
//! — is the one the store answers, while the stack goes on reporting the name the request
//! supplied, because that is what `Parameter$ParameterValue` is ("the input value associated
//! with the parameter").
//!
//! A `Rules` assertion that does not hold for the values in force is not a bad request:
//! "Rules … validates a parameter or a combination of parameters passed to a template during
//! a stack creation or stack update", so the create is accepted and the stack it opens fails
//! and rolls back carrying the rule's own `AssertDescription`. That is the sequence a
//! deployment guarded by a bootstrap-version rule depends on.
//!
//! The rows span what the rule generalises over: which rule function reads the value
//! (`Fn::Contains` over a list of versions, and `Fn::Equals`), whether the name comes from
//! the declaration's `Default` or from the request's own `Parameters`, whether the stored
//! value satisfies the assertion or violates it, and a parameter of a NON-SSM type beside it
//! whose submitted value is its value.

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, member, router, wait_for_the_transition, xml_text, xml_texts,
};
use crate::update_rollback_recovery::{reason_of, ssm, status_of};

/// The name of the Parameter Store parameter every row resolves through.
const VERSION_PARAMETER: &str = "/cfn-ssm-types/version";

/// The description the rule states, which is the reason a stack whose assertion fails
/// reports.
const ASSERT_DESCRIPTION: &str = "a newer bootstrap version is required";

/// How CloudFormation words a `Rules` assertion that does not hold.
fn assertion_failure(description: &str) -> String {
    format!("Parameter validation failed: assertion error: {description}")
}

/// Write one Parameter Store parameter, as a client does before it deploys a template that
/// reads it.
async fn put_parameter(router: &MultiServiceRouter, case: &str, name: &str, value: &str) {
    let (status_code, body) = ssm(
        router,
        "PutParameter",
        &json!({"Name": name, "Type": "String", "Value": value, "Overwrite": true}),
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected PutParameter({name}) to succeed, got {status_code} {body}"
    );
}

/// One row: the stored value the deployment resolves, the rule the template asserts, and
/// whether that assertion holds for it.
struct SsmTypedParameter {
    case: &'static str,
    stack: &'static str,
    /// The value Parameter Store holds under [`VERSION_PARAMETER`] when the create runs.
    stored: &'static str,
    /// The rule's `Assert` expression, over `{"Ref": "Version"}`.
    assertion: Value,
    /// Whether the request passes the parameter's NAME itself, rather than leaving the
    /// declaration's `Default` to supply it.
    passes_the_name: bool,
    /// Whether the assertion holds for `stored`, which decides whether the create completes
    /// or rolls back.
    holds: bool,
}

/// The class the rule covers: any template whose parameter is declared under an SSM
/// parameter type and whose rules read that parameter.
fn ssm_typed_parameters() -> Vec<SsmTypedParameter> {
    vec![
        SsmTypedParameter {
            case: "Fn::Contains excludes the stored version, name from the Default",
            stack: "ssm-type-excluded",
            stored: "5",
            assertion: json!({
                "Fn::Not": [{"Fn::Contains": [["1", "2", "3", "4", "5"], {"Ref": "Version"}]}],
            }),
            passes_the_name: false,
            holds: false,
        },
        SsmTypedParameter {
            case: "Fn::Contains admits the stored version, name from the Default",
            stack: "ssm-type-admitted",
            stored: "6",
            assertion: json!({
                "Fn::Not": [{"Fn::Contains": [["1", "2", "3", "4", "5"], {"Ref": "Version"}]}],
            }),
            passes_the_name: false,
            holds: true,
        },
        SsmTypedParameter {
            case: "Fn::Equals against the stored value, name passed by the request",
            stack: "ssm-type-equals",
            stored: "21",
            assertion: json!({"Fn::Equals": [{"Ref": "Version"}, "21"]}),
            passes_the_name: true,
            holds: true,
        },
        SsmTypedParameter {
            case: "Fn::Equals against another value, name passed by the request",
            stack: "ssm-type-differs",
            stored: "21",
            assertion: json!({"Fn::Equals": [{"Ref": "Version"}, "99"]}),
            passes_the_name: true,
            holds: false,
        },
    ]
}

/// The template a row deploys: a parameter declared under the SSM parameter type, a plain
/// `String` parameter beside it whose submitted value IS its value, the rule asserting over
/// the first, and one resource so the create has something to deploy.
fn template(assertion: &Value) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Parameters": {
            "Version": {
                "Type": "AWS::SSM::Parameter::Value<String>",
                "Default": VERSION_PARAMETER,
            },
            "Plain": {"Type": "String", "Default": "plainly"},
        },
        "Rules": {
            "CheckVersion": {
                "Assertions": [{
                    "Assert": assertion,
                    "AssertDescription": ASSERT_DESCRIPTION,
                }],
            },
        },
        "Resources": {
            "Handle": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
        },
    })
    .to_string()
}

/// The `(ParameterKey, ParameterValue)` pairs a stack reports, read as the parallel member
/// lists the query serializer writes.
async fn reported_parameters(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> Vec<(String, String)> {
    let (status_code, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStacks to succeed, got {status_code} {body}"
    );
    let keys = xml_texts(&body, "ParameterKey");
    let values = xml_texts(&body, "ParameterValue");
    keys.into_iter().zip(values).collect()
}

#[tokio::test]
async fn an_ssm_typed_parameter_is_deployed_with_the_value_the_store_holds() {
    for row in ssm_typed_parameters() {
        let case = row.case;
        let router = router();
        put_parameter(&router, case, VERSION_PARAMETER, row.stored).await;

        let extra = if row.passes_the_name {
            vec![
                member("Parameters.member.1.ParameterKey", "Version"),
                member("Parameters.member.1.ParameterValue", VERSION_PARAMETER),
            ]
        } else {
            Vec::new()
        };
        let body_of_the_template = template(&row.assertion);
        let (status_code, body) =
            create_stack(&router, row.stack, &body_of_the_template, &extra).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} {body}"
        );
        wait_for_the_transition().await;
        // A failed deployment is rolled back, which takes one more transition than the
        // create itself.
        wait_for_the_transition().await;

        let settled = status_of(&router, row.stack).await;
        let expected = if row.holds {
            status::CREATE_COMPLETE
        } else {
            status::ROLLBACK_COMPLETE
        };
        assert_eq!(
            settled, expected,
            "case {case:?}: with {VERSION_PARAMETER} holding {:?}, expected the create to \
             settle in {expected:?}, got {settled:?}",
            row.stored
        );
        let reason = reason_of(&router, row.stack).await;
        if row.holds {
            assert!(
                reason.is_empty(),
                "case {case:?}: expected a completed create to report no reason, got \
                 {reason:?}"
            );
        } else {
            assert_eq!(
                reason,
                assertion_failure(ASSERT_DESCRIPTION),
                "case {case:?}: expected the stack to report the rule's own description, got \
                 {reason:?}"
            );
        }

        // Whatever the rule decided, the stack reports the parameter as the request supplied
        // it: `ParameterValue` is "the input value associated with the parameter", which for
        // an SSM parameter type is the NAME of the parameter the value was read from.
        let reported = reported_parameters(&router, case, row.stack).await;
        assert!(
            reported.contains(&("Version".to_string(), VERSION_PARAMETER.to_string())),
            "case {case:?}: expected the stack to report the submitted parameter name, got \
             {reported:?}"
        );
        assert!(
            reported.contains(&("Plain".to_string(), "plainly".to_string())),
            "case {case:?}: expected a parameter of a plain type to report its own value, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_name_the_parameter_store_does_not_hold_leaves_the_submitted_value_in_force() {
    // One unanswered read is no evidence about the parameter: the name stands as the value in
    // force, so a rule reading it is judged on that rather than on a value nothing answered.
    let case = "an SSM parameter type naming a parameter the store does not hold";
    let stack = "ssm-type-absent";
    let router = router();
    let assertion = json!({"Fn::Equals": [{"Ref": "Version"}, VERSION_PARAMETER]});
    let body_of_the_template = template(&assertion);
    let (status_code, body) = create_stack(&router, stack, &body_of_the_template, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected CreateStack to be accepted, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
    let settled = status_of(&router, stack).await;
    assert_eq!(
        settled,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the submitted name to be the value in force, got {settled:?}"
    );
    let (status_code, body) = call(&router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStacks to succeed, got {status_code} {body}"
    );
    let reported = xml_text(&body, "StackName");
    assert_eq!(
        reported,
        Some(stack.to_string()),
        "case {case:?}: expected the stack to be reported, got {reported:?}"
    );
}
