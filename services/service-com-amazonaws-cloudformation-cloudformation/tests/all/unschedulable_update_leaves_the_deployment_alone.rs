// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateStack` whose body CloudFormation cannot resolve does to a stack that is
//! already deployed, end to end over the awsQuery wire.
//!
//! The rule under test: a template whose own declarations cannot be resolved is refused
//! SYNCHRONOUSLY — HTTP 400, `ValidationError`, and a message that names what cannot be
//! resolved — and that refusal is a no-op on the deployment. The stack keeps the template it
//! holds, its status, its resource set and the values its resources were provisioned with in
//! the services that OWN them, it records no update time, and a body the same rules DO accept
//! still applies afterwards and reaches `UPDATE_COMPLETE` with the new value provisioned.
//!
//! Two halves, then, and the second is what makes the first a statement about the refusal
//! rather than about a stack that can no longer be updated at all.
//!
//! The table spans the class the rule generalises over — every way a submitted body can name
//! something that cannot be resolved:
//!
//! * a dependency cycle between two resources, through `Fn::Sub` and through `Ref`, with the
//!   two rows differing in how the order the refusal LISTS the cycle's members in relates to
//!   the way those ids sort: one pair is listed against their sort order and the other with
//!   it, so no rule that merely sorts the ids, or reverses them, answers both;
//! * a `Ref` and an `Fn::Sub` variable naming nothing the body declares;
//! * an `Fn::GetAtt` reading an attribute off a resource the body does not declare.
//!
//! Every row carries the WHOLE message its refusal must answer with, so the fixed text, the
//! separator and the variable part taken from the submitted body are all pinned.

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, get_template, member, router, wait_for_the_transition,
    xml_text, xml_texts,
};
use crate::update_rollback_recovery::parameter_value;

/// The stack every case is submitted against.
const STACK: &str = "unschedulable-update";

/// The Parameter Store name the first declared resource takes, and so the physical id a
/// reference to it resolves to.
const NAME_A: &str = "/unschedulable-update/a";

/// The Parameter Store name the second declared resource takes.
const NAME_B: &str = "/unschedulable-update/b";

/// The value the first resource is deployed with.
const VALUE_A: &str = "a-value";

/// The value the SECOND resource is deployed with: computed by the deployment, since its
/// declaration reads the first resource's physical id rather than spelling it.
fn deployed_value_b() -> String {
    format!("b-of-{NAME_A}")
}

/// The acyclic body the stack is created from: two `AWS::SSM::Parameter` resources declared
/// in this order, the second one's value substituting a reference to the first.
fn deployable() -> String {
    json!({
        "Resources": {
            "ParamA": parameter(NAME_A, json!(VALUE_A)),
            "ParamB": parameter(NAME_B, json!({"Fn::Sub": "b-of-${ParamA}"})),
        },
    })
    .to_string()
}

/// One `AWS::SSM::Parameter` declaration, whose `Value` a case chooses.
fn parameter(name: &str, value: Value) -> Value {
    json!({
        "Type": "AWS::SSM::Parameter",
        "Properties": {"Name": name, "Type": "String", "Value": value},
    })
}

/// One body an `UpdateStack` submits that CloudFormation cannot resolve, and the whole
/// message its refusal must carry.
struct Unresolvable {
    case: &'static str,
    body: fn() -> String,
    message: &'static str,
}

/// The class the rule covers: every way a submitted body can name something unresolvable.
const UNRESOLVABLE: &[Unresolvable] = &[
    Unresolvable {
        // The cycle the issue's own body closes: the two resources substitute each other, and
        // the members are listed against the way their ids sort.
        case: "a cycle the two resources' Fn::Sub values close",
        body: || {
            json!({
                "Resources": {
                    "ParamA": parameter(NAME_A, json!({"Fn::Sub": "a-of-${ParamB}"})),
                    "ParamB": parameter(NAME_B, json!({"Fn::Sub": "b-of-${ParamA}"})),
                },
            })
            .to_string()
        },
        message: "Circular dependency between resources: [ParamB, ParamA]",
    },
    Unresolvable {
        // A cycle whose members come back the other way about relative to their sort order,
        // so the pair of rows pins the order to the ids themselves rather than to sorting
        // them, reversing them, or to where the body declares them.
        case: "a cycle two Refs close, listed with the way those ids sort",
        body: || {
            json!({
                "Resources": {
                    "Left": parameter(NAME_A, json!({"Ref": "Right"})),
                    "Right": parameter(NAME_B, json!({"Ref": "Left"})),
                },
            })
            .to_string()
        },
        message: "Circular dependency between resources: [Left, Right]",
    },
    Unresolvable {
        case: "a Ref naming nothing the body declares",
        body: || {
            json!({
                "Resources": {
                    "ParamA": parameter(NAME_A, json!(VALUE_A)),
                    "ParamB": parameter(NAME_B, json!({"Ref": "DoesNotExist"})),
                },
            })
            .to_string()
        },
        message: "Template format error: Unresolved resource dependencies [DoesNotExist] in \
                  the Resources block of the template",
    },
    Unresolvable {
        // The same unresolved dependency reached through a substitution variable: one rule,
        // two spellings of the reference.
        case: "an Fn::Sub variable naming nothing the body declares",
        body: || {
            json!({
                "Resources": {
                    "ParamA": parameter(NAME_A, json!(VALUE_A)),
                    "ParamB": parameter(NAME_B, json!({"Fn::Sub": "b-of-${DoesNotExist}"})),
                },
            })
            .to_string()
        },
        message: "Template format error: Unresolved resource dependencies [DoesNotExist] in \
                  the Resources block of the template",
    },
    Unresolvable {
        // An attribute read off a resource the body does not declare is reported for the
        // FUNCTION instance that read it, which is the other wording this class carries.
        case: "an Fn::GetAtt reading an attribute off a resource the body does not declare",
        body: || {
            json!({
                "Resources": {
                    "ParamA": parameter(NAME_A, json!(VALUE_A)),
                    "ParamB": parameter(NAME_B, json!({"Fn::GetAtt": ["DoesNotExist", "Value"]})),
                },
            })
            .to_string()
        },
        message: "Template error: instance of Fn::GetAtt references undefined resource \
                  DoesNotExist",
    },
];

/// Every `LogicalResourceId` `ListStackResources` reports for the stack, sorted, so a row
/// compares the SET the deployment holds rather than one response's order.
async fn deployed_logical_ids(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let (status_code, body) =
        call(router, "ListStackResources", &[member("StackName", STACK)]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected ListStackResources to answer, got {status_code} {body}"
    );
    let mut ids = xml_texts(&body, "LogicalResourceId");
    ids.sort();
    ids
}

/// The body `GetTemplate` reports for the stack: the template the stack really holds, which a
/// refused submission may not have replaced.
async fn stored_template(case: &str, router: &MultiServiceRouter) -> String {
    let (status_code, body) = get_template(router, STACK, None).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected GetTemplate to answer, got {status_code} {body}"
    );
    xml_text(&body, "TemplateBody").unwrap_or_default()
}

#[tokio::test]
async fn an_update_whose_template_cannot_be_resolved_leaves_the_deployment_exactly_as_it_was() {
    let router = router();
    let (status_code, created) = create_stack(&router, STACK, &deployable(), &[]).await;
    assert_eq!(
        status_code, 200,
        "the stack under test must be created, got {status_code} {created}"
    );
    wait_for_the_transition().await;

    // The baseline the refusals are measured against, read back through the stack and through
    // the service that OWNS the resources it deployed.
    let (status_code, described) = describe_stack(&router, STACK).await;
    assert_eq!(
        status_code, 200,
        "the created stack must be readable, got {status_code} {described}"
    );
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "the stack under test must have completed, got {described}"
    );
    let template_before = stored_template("baseline", &router).await;
    let ids_before = deployed_logical_ids("baseline", &router).await;
    assert_eq!(
        ids_before,
        vec!["ParamA".to_string(), "ParamB".to_string()],
        "the deployment must report the two resources the body declares, got {ids_before:?}"
    );
    assert_eq!(
        parameter_value(&router, NAME_A).await.as_deref(),
        Some(VALUE_A),
        "the first resource must be provisioned with the value the body declares"
    );
    assert_eq!(
        parameter_value(&router, NAME_B).await,
        Some(deployed_value_b()),
        "the second resource's value must be the one the deployment computed from the \
         reference"
    );

    for row in UNRESOLVABLE {
        let case = row.case;
        let body = (row.body)();
        let (status_code, refusal) = call(
            &router,
            "UpdateStack",
            &[member("StackName", STACK), member("TemplateBody", &body)],
        )
        .await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the body refused, got {status_code} {refusal}"
        );
        assert_eq!(
            xml_text(&refusal, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {refusal}"
        );
        let message = xml_text(&refusal, "Message").unwrap_or_default();
        assert_eq!(
            message, row.message,
            "case {case:?}: expected {:?}, got {message:?}",
            row.message
        );
        assert!(
            xml_text(&refusal, "StackId").is_none(),
            "case {case:?}: a refused update must answer no StackId, got {refusal}"
        );

        // The refusal wrote NOTHING: the stack is as its create left it, down to the template
        // it holds and the resource set it reports.
        let (status_code, after) = describe_stack(&router, STACK).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: the stack must still be readable, got {status_code} {after}"
        );
        assert_eq!(
            xml_text(&after, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {case:?}: expected the stack left in {}, got {after}",
            status::CREATE_COMPLETE
        );
        assert_eq!(
            xml_text(&after, "LastUpdatedTime"),
            None,
            "case {case:?}: a refused update records no update time, got {after}"
        );
        assert_eq!(
            stored_template(case, &router).await,
            template_before,
            "case {case:?}: expected the stack to keep the template it was created with"
        );
        assert_eq!(
            deployed_logical_ids(case, &router).await,
            ids_before,
            "case {case:?}: expected the resource set unchanged, got a different set"
        );

        // And nothing moved in the service that owns those resources either.
        assert_eq!(
            parameter_value(&router, NAME_A).await.as_deref(),
            Some(VALUE_A),
            "case {case:?}: expected the provisioned value untouched"
        );
        assert_eq!(
            parameter_value(&router, NAME_B).await,
            Some(deployed_value_b()),
            "case {case:?}: expected the computed value untouched"
        );
    }

    // The other half of the rule: the refusals left a stack an acceptable body still updates,
    // and the new value really reaches the service that owns the resource.
    let recovered = json!({
        "Resources": {
            "ParamA": parameter(NAME_A, json!("a-value-2")),
            "ParamB": parameter(NAME_B, json!({"Fn::Sub": "b-of-${ParamA}"})),
        },
    })
    .to_string();
    let (status_code, updated) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", STACK),
            member("TemplateBody", &recovered),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "the recovery update must be accepted, got {status_code} {updated}"
    );
    wait_for_the_transition().await;
    let (_, settled) = describe_stack(&router, STACK).await;
    assert_eq!(
        xml_text(&settled, "StackStatus").as_deref(),
        Some(status::UPDATE_COMPLETE),
        "expected the recovery update to complete, got {settled}"
    );
    assert_eq!(
        parameter_value(&router, NAME_A).await.as_deref(),
        Some("a-value-2"),
        "expected the recovery update's value provisioned"
    );
    assert_eq!(
        parameter_value(&router, NAME_B).await,
        Some(deployed_value_b()),
        "expected the resource the recovery update did not change left as it was"
    );
}
