// SPDX-License-Identifier: CC0-1.0

//! The stack policy a stack ALREADY carries, validated against the template an update
//! submits, end to end over the awsQuery wire.
//!
//! RULE: "stack policies can only be applied to logical ids referenced in the template", so a
//! stored statement protecting a logical id the submitted template does not declare could
//! never be evaluated: CloudFormation refuses the `UpdateStack` outright with 400
//! `ValidationError` — `Error validating existing stack policy: Unknown logical id
//! '<the pattern the statement named>' in statement {} - stack policies can only be applied to
//! logical ids referenced in the template` — and the stack keeps the status, the events, the
//! template and the policy it had.
//!
//! The rows differ along what the rule generalises over: whether the statement naming the
//! absent id is the policy's only one or sits beside others, whether its effect is `Deny` or
//! `Allow`, WHICH logical id it names, and whether it names it exactly or by a WILDCARD (which
//! names no id in particular, so it matches whatever the template declares and is not
//! refused) — beside the updates the rule leaves alone.

use crate::change_set_lifecycle::settle_the_store;
use crate::harness::{call, create_stack, member, router_with_engine};
use crate::stack_lifecycle::{describe_stack, xml_text, xml_texts};

/// The blanket permission a documented stack policy carries to allow anything.
const ALLOW_EVERYTHING: &str =
    r#"{"Effect":"Allow","Action":"Update:*","Principal":"*","Resource":"*"}"#;

/// One policy document from the statements it carries, in document order.
fn policy_of(statements: &[&str]) -> String {
    format!(r#"{{"Statement":[{}]}}"#, statements.join(","))
}

/// The template the stack is created with: two parameters, one of which the updates drop.
fn created_template(stack: &str) -> String {
    format!(
        r#"{{"Resources":{{
             "Keep":{{"Type":"AWS::SSM::Parameter",
               "Properties":{{"Name":"{stack}-keep","Type":"String","Value":"v1"}}}},
             "Other":{{"Type":"AWS::SSM::Parameter",
               "Properties":{{"Type":"String","Value":"v1"}}}}}}}}"#
    )
}

/// The template the update submits: the second parameter alone, so `Keep` is gone.
const WITHOUT_KEEP: &str = r#"{"Resources":{
    "Other":{"Type":"AWS::SSM::Parameter","Properties":{"Type":"String","Value":"v1"}}}}"#;

/// One row: the stored policy, and the pattern the refusal must name — or `None` where the
/// update is one CloudFormation does not refuse on account of the policy.
struct Row {
    case: &'static str,
    stack: &'static str,
    policy: String,
    unknown: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a Deny naming the logical id the update drops, beside a blanket Allow",
            stack: "stored-policy-deny-keep",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:Delete","Principal":"*",
                    "Resource":"LogicalResourceId/Keep"}"#,
            ]),
            unknown: Some("LogicalResourceId/Keep"),
        },
        Row {
            case: "that statement on its own",
            stack: "stored-policy-deny-only",
            policy: policy_of(&[
                r#"{"Effect":"Deny","Action":"Update:*","Principal":"*",
                    "Resource":"LogicalResourceId/Keep"}"#,
            ]),
            unknown: Some("LogicalResourceId/Keep"),
        },
        Row {
            case: "an ALLOW naming a logical id the template never declared",
            stack: "stored-policy-allow-absent",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Allow","Action":"Update:*","Principal":"*",
                    "Resource":"LogicalResourceId/NeverDeclared"}"#,
            ]),
            unknown: Some("LogicalResourceId/NeverDeclared"),
        },
        Row {
            case: "a WILDCARD covering the dropped id, which names no id in particular",
            stack: "stored-policy-wildcard",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:Delete","Principal":"*",
                    "Resource":"LogicalResourceId/Ke*"}"#,
            ]),
            unknown: None,
        },
        Row {
            case: "a policy that names only the resource the template keeps",
            stack: "stored-policy-kept-id",
            policy: policy_of(&[
                ALLOW_EVERYTHING,
                r#"{"Effect":"Deny","Action":"Update:Replace","Principal":"*",
                    "Resource":"LogicalResourceId/Other"}"#,
            ]),
            unknown: None,
        },
        Row {
            case: "a policy that names no logical id at all",
            stack: "stored-policy-no-ids",
            policy: policy_of(&[ALLOW_EVERYTHING]),
            unknown: None,
        },
    ]
}

#[tokio::test]
async fn an_update_is_refused_when_the_stored_policy_names_an_absent_logical_id() {
    let (router, engine) = router_with_engine();
    for row in rows() {
        let case = row.case;
        let (code, body) = create_stack(
            &router,
            row.stack,
            &created_template(row.stack),
            &[member("StackPolicyBody", &row.policy)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the stack must be created first, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (code, body) = describe_stack(&router, row.stack).await;
        let created_status = xml_text(&body, "StackStatus").unwrap_or_default();
        assert_eq!(
            created_status, "CREATE_COMPLETE",
            "case {case:?}: the stack must complete before the update, got {code} {body}"
        );
        let (code, body) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the events must be readable, got {code} {body}"
        );
        let events_before = xml_texts(&body, "EventId").len();

        let (code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", row.stack),
                member("TemplateBody", WITHOUT_KEEP),
            ],
        )
        .await;
        let Some(pattern) = row.unknown else {
            assert_eq!(
                code, 200,
                "case {case:?}: an update the stored policy does not refuse is accepted, got \
                 {code} {body}"
            );
            settle_the_store(&router, &engine).await;
            continue;
        };
        assert_eq!(
            code, 400,
            "case {case:?}: expected the update to be refused, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected a ValidationError, got {body}"
        );
        let expected = format!(
            "Error validating existing stack policy: Unknown logical id '{pattern}' in \
             statement {{}} - stack policies can only be applied to logical ids referenced \
             in the template"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {body}"
        );

        // Nothing was done: the stack keeps its status, its events and its policy.
        settle_the_store(&router, &engine).await;
        let (code, body) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&body, "StackStatus").unwrap_or_default(),
            created_status,
            "case {case:?}: expected the stack to keep its status, got {code} {body}"
        );
        let (code, body) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the events must be readable, got {code} {body}"
        );
        assert_eq!(
            xml_texts(&body, "EventId").len(),
            events_before,
            "case {case:?}: expected no new event, got {body}"
        );
        let (code, body) = call(
            &router,
            "GetStackPolicy",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the policy must be readable, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "StackPolicyBody").as_deref(),
            Some(row.policy.as_str()),
            "case {case:?}: expected the stack to keep its policy, got {body}"
        );
    }
}
