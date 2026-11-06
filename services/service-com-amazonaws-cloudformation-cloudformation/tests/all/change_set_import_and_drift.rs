// SPDX-License-Identifier: CC0-1.0

//! Adopting resources that already exist, and reverting drift, end to end over the awsQuery
//! wire.
//!
//! RULE: a physical resource name identifies ONE resource, and what a change set does about a
//! name that is already taken is what the request asked for. `ChangeSetType=IMPORT` and
//! `ImportExistingResources=true` ADOPT such a resource — the plan reports it with the
//! `Import` action and the identifier it already carries, and executing the plan records it
//! as a resource the stack manages without creating anything — while a name another STACK
//! manages is nobody else's to adopt, so the change set fails with CloudFormation's own
//! collision reason, `<physical-id> already exists in stack <stack-arn>`. That same reason is
//! what a deployment reports for a resource it cannot create because the stack it is
//! deploying already holds the name. An IMPORT is held to the rule CloudFormation states for
//! it: every imported resource must carry a `DeletionPolicy`.
//!
//! RULE: `DeploymentMode=REVERT_DRIFT` compares the template against the resource's ACTUAL
//! state rather than against the previous template, so a resource written out of band is a
//! `Modify` whose before value is the live one (`ACTUAL_STATE`) and whose after value is the
//! template's (`TEMPLATE`) — and executing it puts the template's value back.
//!
//! The rows of each table differ along what the rule generalises over: who holds the name
//! (nobody, another stack, the deploying stack itself), how many resources are named, whether
//! the request asks for adoption at all, which `DeletionPolicy` the template states, which
//! failure option the change set carries, and which property drifted.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::{
    create_change_set, describe_change_set, execute_change_set, settle_the_store,
};
use crate::dynamic_references::{put_parameter, ssm};
use crate::harness::{call, create_stack, member, router_with_engine, xml_blocks, xml_text, xml_texts};
use crate::stack_lifecycle::describe_stack;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    change_action, change_set_status, execution_status, status, NO_CHANGES_REASON,
};

/// The `StatusReason` a change set whose early validation failed carries. Source: the
/// sentence `crate::state::early_validation::failed_reason` words for the existence check.
const EXISTENCE_CHECK_FAILED: &str =
    "The following hook(s)/validation failed: \
     [AWS::EarlyValidation::ResourceExistenceCheck]. To troubleshoot Early Validation errors, \
     use the DescribeEvents API for detailed failure information.";

/// A template declaring one `AWS::SSM::Parameter` under the name it is given, with the
/// resource attributes the case needs. The two required properties of the type are its `Type`
/// and its `Value` (source: the CloudFormation resource reference for
/// `AWS::SSM::Parameter`).
fn parameter_template(name: &str, value: &str, attributes: &str) -> String {
    format!(
        r#"{{"Resources":{{"Param":{{"Type":"AWS::SSM::Parameter"{attributes},
            "Properties":{{"Name":"{name}","Type":"String","Value":"{value}"}}}}}}}}"#
    )
}

/// The parameter value Systems Manager holds under `name`, or `None` where it holds none.
async fn parameter_value(router: &MultiServiceRouter, name: &str) -> Option<String> {
    let (status_code, body) = ssm(router, "GetParameter", &json!({"Name": name})).await;
    if status_code != 200 {
        return None;
    }
    body.pointer("/Parameter/Value")
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
}

/// The `<member>` block of `DescribeStackResources` for one logical id.
async fn stack_resource(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> Option<String> {
    let (_, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack)],
    )
    .await;
    xml_blocks(&body, "member")
        .into_iter()
        .find(|block| xml_text(block, "LogicalResourceId").as_deref() == Some(logical_id))
}

/// What one auto-import row expects of the change set its template is planned into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Adoption {
    /// The named resource is adopted: the plan reports it with this action and the identifier
    /// it already exists under.
    Adopted,
    /// The change set fails, with this reason.
    Failed(&'static str),
}

/// RULE: with `ImportExistingResources=true` a resource the plan would create under a name
/// that already exists is ADOPTED — unless another stack manages that name, which fails the
/// change set naming the stack that holds it. Without the member, the same collision fails
/// the change set through the early-validation stage instead.
#[tokio::test]
async fn an_existing_name_is_adopted_only_when_no_stack_manages_it() {
    let (router, engine) = router_with_engine();

    // A name a CLIENT holds: written through Systems Manager's own wire, so no stack manages
    // it.
    put_parameter(&router, "unmanaged", "/adopt/unmanaged", "String", "live").await;
    // ...and a name ANOTHER STACK manages, which that stack created.
    let owner_template = parameter_template("/adopt/owned", "owned", "");
    let (code, body) = create_stack(&router, "adopt-owner", &owner_template, &[]).await;
    assert_eq!(
        code, 200,
        "the owning stack must be created first, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (_, body) = describe_stack(&router, "adopt-owner").await;
    let owner_stack_id = xml_text(&body, "StackId").expect("the owning stack's id");

    /// `(case, stack, change set, template, whether adoption is asked for, expectation)`.
    type Row = (&'static str, &'static str, String, bool, Adoption);
    let rows: Vec<Row> = vec![
        (
            // The template declares a DIFFERENT value from the one the live parameter
            // carries: adopting a resource does not modify it.
            "a name nothing manages, with adoption asked for",
            "adopt-unmanaged",
            parameter_template("/adopt/unmanaged", "declared", ""),
            true,
            Adoption::Adopted,
        ),
        (
            "a name another stack manages, with adoption asked for",
            "adopt-managed",
            parameter_template("/adopt/owned", "owned", ""),
            true,
            Adoption::Failed("already exists in stack"),
        ),
        (
            "the same unmanaged name, with adoption NOT asked for",
            "adopt-plain",
            parameter_template("/adopt/unmanaged", "declared", ""),
            false,
            Adoption::Failed(EXISTENCE_CHECK_FAILED),
        ),
    ];

    for (case, stack, template, adopting, expected) in &rows {
        let mut members = vec![
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", template),
        ];
        if *adopting {
            members.push(member("ImportExistingResources", "true"));
        }
        let (code, body) = create_change_set(&router, stack, "cs", &members).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set must be created, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (code, body) = describe_change_set(&router, "cs", Some(stack)).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set must be readable, got {code} {body}"
        );
        match expected {
            Adoption::Adopted => {
                assert_eq!(
                    (
                        xml_text(&body, "Status").unwrap_or_default(),
                        xml_text(&body, "ExecutionStatus").unwrap_or_default(),
                        xml_texts(&body, "Action"),
                        xml_texts(&body, "PhysicalResourceId"),
                    ),
                    (
                        change_set_status::CREATE_COMPLETE.to_string(),
                        execution_status::AVAILABLE.to_string(),
                        vec![change_action::IMPORT.to_string()],
                        vec!["/adopt/unmanaged".to_string()],
                    ),
                    "case {case:?}: expected an executable plan that imports the existing \
                     name, got {body}"
                );
            }
            Adoption::Failed(reason) => {
                let status_reason = xml_text(&body, "StatusReason").unwrap_or_default();
                assert_eq!(
                    (
                        xml_text(&body, "Status").unwrap_or_default(),
                        xml_text(&body, "ExecutionStatus").unwrap_or_default(),
                        status_reason.contains(reason),
                    ),
                    (
                        change_set_status::FAILED.to_string(),
                        execution_status::UNAVAILABLE.to_string(),
                        true,
                    ),
                    "case {case:?}: expected a failed plan whose reason carries {reason:?}, \
                     got {body}"
                );
                // The owner's own stack arn is what a collision with a managed name names.
                if *reason == "already exists in stack" {
                    assert_eq!(
                        status_reason,
                        format!("/adopt/owned already exists in stack {owner_stack_id}"),
                        "case {case:?}: the reason must name the stack that manages the name, \
                         got {status_reason:?}"
                    );
                }
                // Either way the stack this plan was for is still in review with nothing
                // provisioned.
                let (_, stack_body) = describe_stack(&router, stack).await;
                assert_eq!(
                    xml_text(&stack_body, "StackStatus").as_deref(),
                    Some(status::REVIEW_IN_PROGRESS),
                    "case {case:?}: expected the stack to stay in review, got {stack_body}"
                );
            }
        }
    }

    // Executing the adopting plan brings the existing parameter under the stack's
    // management: the resource is reported with the name it already had, the stack completes,
    // and the parameter the client wrote is still there.
    let (code, body) = execute_change_set(&router, "cs", Some("adopt-unmanaged")).await;
    assert_eq!(
        code, 200,
        "the adopting plan must be executable, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (_, body) = describe_stack(&router, "adopt-unmanaged").await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "the adopting execution must complete the stack, got {body}"
    );
    let resource = stack_resource(&router, "adopt-unmanaged", "Param")
        .await
        .expect("the adopted resource must be reported by the stack");
    assert_eq!(
        (
            xml_text(&resource, "PhysicalResourceId").unwrap_or_default(),
            xml_text(&resource, "ResourceStatus").unwrap_or_default(),
        ),
        (
            "/adopt/unmanaged".to_string(),
            "UPDATE_COMPLETE".to_string(),
        ),
        "the adopted resource must be reported under the name it already had, got {resource}"
    );
    assert_eq!(
        parameter_value(&router, "/adopt/unmanaged").await.as_deref(),
        Some("live"),
        "the adopted parameter must be left exactly as it was: importing a resource brings \
         it under management without modifying it"
    );
    // The stack that OWNS the other name still owns it: the failed plan took nothing.
    assert_eq!(
        parameter_value(&router, "/adopt/owned").await.as_deref(),
        Some("owned"),
        "the owning stack must keep its resource"
    );
}

/// RULE: an IMPORT change set adopts exactly the resources `ResourcesToImport` names, and is
/// refused unless the template gives every one of them a `DeletionPolicy` — the refusal
/// listing the logical ids that lack one, with no stack created for it.
#[tokio::test]
async fn an_import_change_set_needs_a_deletion_policy_on_every_imported_resource() {
    let (router, engine) = router_with_engine();
    put_parameter(&router, "import", "/import/one", "String", "held").await;
    put_parameter(&router, "import", "/import/two", "String", "held").await;

    /// `(case, stack, the DeletionPolicy attribute the template states, the refusal)`.
    type Row = (&'static str, &'static str, &'static str, Option<String>);
    let rows: Vec<Row> = vec![
        (
            "Retain on the imported resource",
            "import-retain",
            r#","DeletionPolicy":"Retain""#,
            None,
        ),
        (
            "Delete stated explicitly, which is still a stated policy",
            "import-delete",
            r#","DeletionPolicy":"Delete""#,
            None,
        ),
        (
            "Snapshot, the third documented value",
            "import-snapshot",
            r#","DeletionPolicy":"Snapshot""#,
            None,
        ),
        (
            "no DeletionPolicy at all",
            "import-bare",
            "",
            Some(
                "The following resources to import [Param] must have DeletionPolicy \
                 attribute specified in the template."
                    .to_string(),
            ),
        ),
    ];

    for (case, stack, attributes, refusal) in &rows {
        let template = parameter_template("/import/one", "held", attributes);
        let (code, body) = create_change_set(
            &router,
            stack,
            "cs",
            &[
                member("ChangeSetType", "IMPORT"),
                member("TemplateBody", &template),
                member("ResourcesToImport.member.1.ResourceType", "AWS::SSM::Parameter"),
                member("ResourcesToImport.member.1.LogicalResourceId", "Param"),
                member(
                    "ResourcesToImport.member.1.ResourceIdentifier.entry.1.key",
                    "Name",
                ),
                member(
                    "ResourcesToImport.member.1.ResourceIdentifier.entry.1.value",
                    "/import/one",
                ),
            ],
        )
        .await;
        match refusal {
            None => {
                assert_eq!(
                    code, 200,
                    "case {case:?}: the import must be accepted, got {code} {body}"
                );
                settle_the_store(&router, &engine).await;
                let (_, body) = describe_change_set(&router, "cs", Some(stack)).await;
                assert_eq!(
                    (
                        xml_text(&body, "Status").unwrap_or_default(),
                        xml_texts(&body, "Action"),
                        xml_texts(&body, "PhysicalResourceId"),
                    ),
                    (
                        change_set_status::CREATE_COMPLETE.to_string(),
                        vec![change_action::IMPORT.to_string()],
                        vec!["/import/one".to_string()],
                    ),
                    "case {case:?}: expected one Import of the existing parameter, got {body}"
                );
            }
            Some(message) => {
                assert_eq!(
                    code, 400,
                    "case {case:?}: the import must be refused, got {code} {body}"
                );
                assert_eq!(
                    (
                        xml_text(&body, "Code").unwrap_or_default(),
                        xml_text(&body, "Message").unwrap_or_default(),
                    ),
                    ("ValidationError".to_string(), message.clone()),
                    "case {case:?}: expected the modelled refusal, got {body}"
                );
                // "No stack is created": the name the refused request addressed resolves to
                // nothing.
                let (code, body) = describe_stack(&router, stack).await;
                assert_eq!(
                    code, 400,
                    "case {case:?}: the refused import must have created no stack, got \
                     {code} {body}"
                );
            }
        }
    }

    // The refusal names EVERY imported resource that lacks the attribute, in the order the
    // request names them.
    let two = r#"{"Resources":{
        "Kept":{"Type":"AWS::SSM::Parameter","DeletionPolicy":"Retain",
                "Properties":{"Name":"/import/one","Type":"String","Value":"held"}},
        "Bare":{"Type":"AWS::SSM::Parameter",
                "Properties":{"Name":"/import/two","Type":"String","Value":"held"}}}}"#;
    let (code, body) = create_change_set(
        &router,
        "import-two",
        "cs",
        &[
            member("ChangeSetType", "IMPORT"),
            member("TemplateBody", two),
            member("ResourcesToImport.member.1.ResourceType", "AWS::SSM::Parameter"),
            member("ResourcesToImport.member.1.LogicalResourceId", "Kept"),
            member("ResourcesToImport.member.1.ResourceIdentifier.entry.1.key", "Name"),
            member(
                "ResourcesToImport.member.1.ResourceIdentifier.entry.1.value",
                "/import/one",
            ),
            member("ResourcesToImport.member.2.ResourceType", "AWS::SSM::Parameter"),
            member("ResourcesToImport.member.2.LogicalResourceId", "Bare"),
            member("ResourcesToImport.member.2.ResourceIdentifier.entry.1.key", "Name"),
            member(
                "ResourcesToImport.member.2.ResourceIdentifier.entry.1.value",
                "/import/two",
            ),
        ],
    )
    .await;
    assert_eq!(
        code, 400,
        "case \"one of two imported resources without the attribute\": got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Message").unwrap_or_default(),
        "The following resources to import [Bare] must have DeletionPolicy attribute \
         specified in the template.",
        "case \"one of two imported resources without the attribute\": the refusal must name \
         only the resource that lacks it, got {body}"
    );

    // An import of a resource the owning service does NOT hold is no import at all: the
    // plan cannot be carried out, so the change set fails rather than offering one whose
    // execution would record a resource nobody has. Asserted through the same Systems
    // Manager parameters the rows above use, under a name nothing was ever written to.
    let absent = parameter_template("/import/absent", "held", r#","DeletionPolicy":"Retain""#);
    let (code, body) = create_change_set(
        &router,
        "import-absent",
        "cs",
        &[
            member("ChangeSetType", "IMPORT"),
            member("TemplateBody", &absent),
            member("ResourcesToImport.member.1.ResourceType", "AWS::SSM::Parameter"),
            member("ResourcesToImport.member.1.LogicalResourceId", "Param"),
            member("ResourcesToImport.member.1.ResourceIdentifier.entry.1.key", "Name"),
            member(
                "ResourcesToImport.member.1.ResourceIdentifier.entry.1.value",
                "/import/absent",
            ),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"a resource to import that nothing holds\": the change set is created, got \
         {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (_, body) = describe_change_set(&router, "cs", Some("import-absent")).await;
    assert_eq!(
        (
            xml_text(&body, "Status").unwrap_or_default(),
            xml_text(&body, "ExecutionStatus").unwrap_or_default(),
            xml_text(&body, "StatusReason").unwrap_or_default(),
        ),
        (
            change_set_status::FAILED.to_string(),
            execution_status::UNAVAILABLE.to_string(),
            "Resource of type 'AWS::SSM::Parameter' with identifier '/import/absent' was not \
             found."
                .to_string(),
        ),
        "case \"a resource to import that nothing holds\": expected a failed plan naming the \
         identifier nothing holds, got {body}"
    );

    // ...and one another STACK already manages is that stack's: importing it would take over
    // a resource CloudFormation manages elsewhere.
    let owner_template = parameter_template("/import/owned", "owned", "");
    let (code, body) = create_stack(&router, "import-owner", &owner_template, &[]).await;
    assert_eq!(
        code, 200,
        "the owning stack must be created first, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (_, body) = describe_stack(&router, "import-owner").await;
    let owner_stack_id = xml_text(&body, "StackId").expect("the owning stack's id");
    let managed = parameter_template("/import/owned", "owned", r#","DeletionPolicy":"Retain""#);
    let (code, body) = create_change_set(
        &router,
        "import-managed",
        "cs",
        &[
            member("ChangeSetType", "IMPORT"),
            member("TemplateBody", &managed),
            member("ResourcesToImport.member.1.ResourceType", "AWS::SSM::Parameter"),
            member("ResourcesToImport.member.1.LogicalResourceId", "Param"),
            member("ResourcesToImport.member.1.ResourceIdentifier.entry.1.key", "Name"),
            member(
                "ResourcesToImport.member.1.ResourceIdentifier.entry.1.value",
                "/import/owned",
            ),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"a resource another stack manages\": the change set is created, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (_, body) = describe_change_set(&router, "cs", Some("import-managed")).await;
    assert_eq!(
        (
            xml_text(&body, "Status").unwrap_or_default(),
            xml_text(&body, "StatusReason").unwrap_or_default(),
        ),
        (
            change_set_status::FAILED.to_string(),
            format!("/import/owned already exists in stack {owner_stack_id}"),
        ),
        "case \"a resource another stack manages\": expected a failed plan naming that stack, \
         got {body}"
    );

    // Executing an accepted import adopts the resource as it is: the stack reaches
    // IMPORT_COMPLETE and the parameter it adopted is untouched.
    let (code, body) = execute_change_set(&router, "cs", Some("import-retain")).await;
    assert_eq!(
        code, 200,
        "the import must be executable, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (_, body) = describe_stack(&router, "import-retain").await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::IMPORT_COMPLETE),
        "an executed import completes as an import, got {body}"
    );
    let resource = stack_resource(&router, "import-retain", "Param")
        .await
        .expect("the imported resource must be reported by the stack");
    assert_eq!(
        (
            xml_text(&resource, "PhysicalResourceId").unwrap_or_default(),
            xml_text(&resource, "ResourceStatus").unwrap_or_default(),
        ),
        ("/import/one".to_string(), "IMPORT_COMPLETE".to_string()),
        "the imported resource must be reported as imported, under the identifier it \
         already had, got {resource}"
    );
    assert_eq!(
        parameter_value(&router, "/import/one").await.as_deref(),
        Some("held"),
        "the imported parameter must still be there"
    );

    // `DisableValidation` turns the existence reads OFF, so nothing is established about any
    // identifier and nothing is reported missing: the plan is offered unvalidated, which is
    // what turning the stage off is for.
    let unvalidated =
        parameter_template("/import/unread", "held", r#","DeletionPolicy":"Retain""#);
    let (code, body) = create_change_set(
        &router,
        "import-unvalidated",
        "cs",
        &[
            member("ChangeSetType", "IMPORT"),
            member("TemplateBody", &unvalidated),
            member("DisableValidation", "true"),
            member("ResourcesToImport.member.1.ResourceType", "AWS::SSM::Parameter"),
            member("ResourcesToImport.member.1.LogicalResourceId", "Param"),
            member("ResourcesToImport.member.1.ResourceIdentifier.entry.1.key", "Name"),
            member(
                "ResourcesToImport.member.1.ResourceIdentifier.entry.1.value",
                "/import/unread",
            ),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"an unvalidated import\": the change set is created, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (_, body) = describe_change_set(&router, "cs", Some("import-unvalidated")).await;
    assert_eq!(
        (
            xml_text(&body, "Status").unwrap_or_default(),
            xml_text(&body, "ExecutionStatus").unwrap_or_default(),
            xml_texts(&body, "Action"),
        ),
        (
            change_set_status::CREATE_COMPLETE.to_string(),
            execution_status::AVAILABLE.to_string(),
            vec![change_action::IMPORT.to_string()],
        ),
        "case \"an unvalidated import\": an unanswered read is not evidence of absence, so \
         the plan is offered, got {body}"
    );

    // An import whose identifier the TEMPLATE does not state is identified by the request's
    // own `ResourceIdentifier`, and it is that identifier the existence check reads back.
    put_parameter(&router, "identified", "/import/identified", "String", "held").await;
    let unnamed = r#"{"Resources":{"Param":{"Type":"AWS::SSM::Parameter",
        "DeletionPolicy":"Retain",
        "Properties":{"Type":"String","Value":"held"}}}}"#;
    let (code, body) = create_change_set(
        &router,
        "import-identified",
        "cs",
        &[
            member("ChangeSetType", "IMPORT"),
            member("TemplateBody", unnamed),
            member("ResourcesToImport.member.1.ResourceType", "AWS::SSM::Parameter"),
            member("ResourcesToImport.member.1.LogicalResourceId", "Param"),
            member("ResourcesToImport.member.1.ResourceIdentifier.entry.1.key", "Name"),
            member(
                "ResourcesToImport.member.1.ResourceIdentifier.entry.1.value",
                "/import/identified",
            ),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"an identifier the template does not state\": the change set is created, got \
         {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (_, body) = describe_change_set(&router, "cs", Some("import-identified")).await;
    assert_eq!(
        (
            xml_text(&body, "Status").unwrap_or_default(),
            xml_texts(&body, "Action"),
            xml_texts(&body, "PhysicalResourceId"),
        ),
        (
            change_set_status::CREATE_COMPLETE.to_string(),
            vec![change_action::IMPORT.to_string()],
            vec!["/import/identified".to_string()],
        ),
        "case \"an identifier the template does not state\": the request's own identifier is \
         the one adopted, got {body}"
    );
}

/// RULE: two resources of ONE stack that resolve to one physical name collide, and
/// CloudFormation reports the collision itself — the failing resource's `CREATE_FAILED` event
/// carries `<physical-id> already exists in stack <stack-arn>`, with no resource-handler
/// wrapper around it — and what becomes of the stack is the `OnStackFailure` the change set
/// was created with.
#[tokio::test]
async fn an_intra_stack_name_collision_is_reported_in_cloudformations_own_words() {
    let (router, engine) = router_with_engine();

    /// `(case, stack, OnStackFailure, the stack's terminal status, whether the name resolves)`
    type Row = (&'static str, &'static str, &'static str, &'static str, bool);
    let rows: Vec<Row> = vec![
        (
            "DELETE takes the failed stack away",
            "collide-delete",
            "DELETE",
            status::DELETE_COMPLETE,
            false,
        ),
        (
            "ROLLBACK removes what the create made",
            "collide-rollback",
            "ROLLBACK",
            status::ROLLBACK_COMPLETE,
            true,
        ),
        (
            "DO_NOTHING keeps the failed state",
            "collide-nothing",
            "DO_NOTHING",
            status::CREATE_FAILED,
            true,
        ),
    ];

    for (case, stack, on_stack_failure, terminal, resolves) in &rows {
        // Two declarations of one stack resolving to ONE name: the second takes the first's
        // name through a `Ref`-free restatement of it, which is what a template that names
        // both resources the same way is.
        let name = format!("/collide/{stack}");
        let template = format!(
            r#"{{"Resources":{{
                "A":{{"Type":"AWS::SSM::Parameter",
                      "Properties":{{"Name":"{name}","Type":"String","Value":"first"}}}},
                "B":{{"Type":"AWS::SSM::Parameter",
                      "Properties":{{"Name":"{name}","Type":"String","Value":"second"}}}}}}}}"#
        );
        let (code, body) = create_change_set(
            &router,
            stack,
            "cs",
            &[
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", &template),
                member("OnStackFailure", on_stack_failure),
                // The collision is between two resources of this very plan, so the
                // pre-deployment existence check has nothing to report about either name.
                member("DisableValidation", "true"),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set must be created, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (_, body) = describe_stack(&router, stack).await;
        let stack_id = xml_text(&body, "StackId").expect("the stack's id");
        let (code, body) = execute_change_set(&router, "cs", Some(stack)).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the plan must be executable, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        settle_the_store(&router, &engine).await;

        // The failing resource's own event carries CloudFormation's collision reason.
        let (_, events) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", &stack_id)],
        )
        .await;
        let reason = xml_blocks(&events, "member")
            .into_iter()
            .find(|event| {
                xml_text(event, "LogicalResourceId").as_deref() == Some("B")
                    && xml_text(event, "ResourceStatus").as_deref() == Some("CREATE_FAILED")
            })
            .and_then(|event| xml_text(&event, "ResourceStatusReason"));
        assert_eq!(
            reason.as_deref(),
            Some(format!("{name} already exists in stack {stack_id}").as_str()),
            "case {case:?}: expected CloudFormation's own collision reason, got {reason:?}"
        );

        // ...and what became of the stack is the failure option the change set carried.
        let (_, body) = describe_stack(&router, &stack_id).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(*terminal),
            "case {case:?}: expected the stack to settle {terminal}, got {body}"
        );
        let (code, body) = describe_stack(&router, stack).await;
        let resolved = code == 200;
        assert_eq!(
            resolved, *resolves,
            "case {case:?}: expected the stack name to resolve: {resolves}, got {code} {body}"
        );
    }
}

/// RULE: the `DisableRollback` a change set reports is the flag in force on it — the one the
/// request stated, else the one the deployment MODE it runs in puts in force, the express mode
/// being the mode that keeps what a failed operation provisioned.
#[tokio::test]
async fn the_deployment_config_a_change_set_reports_is_the_one_in_force() {
    let (router, engine) = router_with_engine();
    let template = parameter_template("/config/base", "one", "");
    let (code, body) = create_stack(&router, "config-stack", &template, &[]).await;
    assert_eq!(code, 200, "the stack must be created, got {code} {body}");
    settle_the_store(&router, &engine).await;

    /// `(case, change set, the DeploymentConfig members, the reported Mode and
    /// DisableRollback)`.
    type Row = (
        &'static str,
        &'static str,
        Vec<(String, String)>,
        (&'static str, &'static str),
    );
    let rows: Vec<Row> = vec![
        (
            "EXPRESS alone",
            "cs-express",
            vec![member("DeploymentConfig.Mode", "EXPRESS")],
            ("EXPRESS", "true"),
        ),
        (
            "EXPRESS with the flag stated false",
            "cs-express-false",
            vec![
                member("DeploymentConfig.Mode", "EXPRESS"),
                member("DeploymentConfig.DisableRollback", "false"),
            ],
            ("EXPRESS", "false"),
        ),
        (
            "STANDARD alone",
            "cs-standard",
            vec![member("DeploymentConfig.Mode", "STANDARD")],
            ("STANDARD", "false"),
        ),
        (
            "no deployment configuration at all",
            "cs-none",
            Vec::new(),
            ("STANDARD", "false"),
        ),
    ];

    for (index, (case, change_set, config, expected)) in rows.iter().enumerate() {
        // Each row submits a template of its own, so the plan carries a change and the change
        // set completes.
        let revised = parameter_template("/config/base", &format!("revision-{index}"), "");
        let mut members = vec![member("TemplateBody", &revised)];
        members.extend_from_slice(config);
        let (code, body) =
            create_change_set(&router, "config-stack", change_set, &members).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set must be created, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (_, body) = describe_change_set(&router, change_set, Some("config-stack")).await;
        let reported = xml_blocks(&body, "DeploymentConfig")
            .into_iter()
            .next()
            .unwrap_or_default();
        assert_eq!(
            (
                xml_text(&reported, "Mode").unwrap_or_default(),
                xml_text(&reported, "DisableRollback").unwrap_or_default(),
            ),
            (expected.0.to_string(), expected.1.to_string()),
            "case {case:?}: expected the configuration in force {expected:?}, got {body}"
        );
    }
}

/// RULE: a `REVERT_DRIFT` change set compares the template against the resource's ACTUAL
/// state, so an out-of-band write is a `Modify` whose target reports the live value from
/// `ACTUAL_STATE` and the template's from `TEMPLATE`; the STANDARD change set over the same
/// unchanged template is the one that has nothing to change. Executing the drift-aware plan
/// puts the template's value back.
#[tokio::test]
async fn a_revert_drift_change_set_plans_against_the_resources_actual_state() {
    let (router, engine) = router_with_engine();

    /// `(case, stack, the property written out of band, the value written, the value the
    /// template declares)`.
    type Row = (&'static str, &'static str, &'static str, &'static str);
    let rows: Vec<Row> = vec![
        ("a value written out of band", "drift-value", "drifted", "declared"),
        ("a longer value written out of band", "drift-long", "a-much-longer-value", "short"),
    ];

    for (case, stack, live, declared) in &rows {
        let name = format!("/drift/{stack}");
        let template = parameter_template(&name, declared, "");
        let (code, body) = create_stack(&router, stack, &template, &[]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the stack must be created, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;

        // The resource is written OUT OF BAND, through the owning service's own wire.
        let (code, written) = ssm(
            &router,
            "PutParameter",
            &json!({"Name": name, "Type": "String", "Value": live, "Overwrite": true}),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the out-of-band write must land, got {code} {written}"
        );

        // The STANDARD change set over the unchanged template has nothing to change.
        let (code, body) = create_change_set(
            &router,
            stack,
            "cs-standard",
            &[member("UsePreviousTemplate", "true")],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the standard change set must be created, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (_, body) = describe_change_set(&router, "cs-standard", Some(stack)).await;
        assert_eq!(
            (
                xml_text(&body, "Status").unwrap_or_default(),
                xml_text(&body, "StatusReason").unwrap_or_default(),
            ),
            (
                change_set_status::FAILED.to_string(),
                NO_CHANGES_REASON.to_string(),
            ),
            "case {case:?}: the template-only comparison must find nothing, got {body}"
        );

        // ...while the drift-aware one reports the live value against the declared one.
        let (code, body) = create_change_set(
            &router,
            stack,
            "cs-revert",
            &[
                member("UsePreviousTemplate", "true"),
                member("DeploymentMode", "REVERT_DRIFT"),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the drift-aware change set must be created, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (_, body) = describe_change_set(&router, "cs-revert", Some(stack)).await;
        assert_eq!(
            (
                xml_text(&body, "Status").unwrap_or_default(),
                xml_text(&body, "DeploymentMode").unwrap_or_default(),
                xml_texts(&body, "Action"),
                xml_text(&body, "Replacement").unwrap_or_default(),
                xml_text(&body, "ResourceDriftStatus").unwrap_or_default(),
            ),
            (
                change_set_status::CREATE_COMPLETE.to_string(),
                "REVERT_DRIFT".to_string(),
                vec![change_action::MODIFY.to_string()],
                "False".to_string(),
                "MODIFIED".to_string(),
            ),
            "case {case:?}: expected a drift-aware modification, got {body}"
        );
        let target = xml_blocks(&body, "Target")
            .into_iter()
            .next()
            .unwrap_or_default();
        assert_eq!(
            (
                xml_text(&target, "Name").unwrap_or_default(),
                xml_text(&target, "BeforeValue").unwrap_or_default(),
                xml_text(&target, "BeforeValueFrom").unwrap_or_default(),
                xml_text(&target, "AfterValue").unwrap_or_default(),
                xml_text(&target, "AfterValueFrom").unwrap_or_default(),
            ),
            (
                "Value".to_string(),
                (*live).to_string(),
                "ACTUAL_STATE".to_string(),
                (*declared).to_string(),
                "TEMPLATE".to_string(),
            ),
            "case {case:?}: expected the live value against the template's, got {target}"
        );

        // Executing it puts the template's value back, and the stack completes its update.
        let (code, body) = execute_change_set(&router, "cs-revert", Some(stack)).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the drift-aware plan must be executable, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (_, body) = describe_stack(&router, stack).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::UPDATE_COMPLETE),
            "case {case:?}: the revert must complete the stack, got {body}"
        );
        assert_eq!(
            parameter_value(&router, &name).await.as_deref(),
            Some(*declared),
            "case {case:?}: the revert must put the template's value back"
        );
    }
}

/// RULE: a template parameter whose type puts its value in Parameter Store is resolved while
/// the change set is PLANNED — a name the store does not answer for is the request's own
/// refusal, naming the names it could not read and creating no stack, and a name it holds is
/// reported as the parameter's `ResolvedValue` beside the name as its `ParameterValue`.
#[tokio::test]
async fn an_ssm_typed_parameter_is_resolved_while_the_change_set_is_planned() {
    let (router, engine) = router_with_engine();
    put_parameter(&router, "held", "/planned/held", "String", "7").await;

    /// `(case, stack, the names the template's parameters default to, the refusal)`.
    type Row = (&'static str, &'static str, Vec<&'static str>, Option<String>);
    let rows: Vec<Row> = vec![
        ("one name the store holds", "planned-held", vec!["/planned/held"], None),
        (
            "one name the store does not hold",
            "planned-absent",
            vec!["/planned/absent"],
            Some(
                "Unable to fetch parameters [/planned/absent] from parameter store for this \
                 account."
                    .to_string(),
            ),
        ),
        (
            "two names the store does not hold, in the order they are addressed",
            "planned-two",
            vec!["/planned/absent-a", "/planned/absent-b"],
            Some(
                "Unable to fetch parameters [/planned/absent-a, /planned/absent-b] from \
                 parameter store for this account."
                    .to_string(),
            ),
        ),
        (
            "one held name beside one that is not",
            "planned-mixed",
            vec!["/planned/held", "/planned/absent-c"],
            Some(
                "Unable to fetch parameters [/planned/absent-c] from parameter store for this \
                 account."
                    .to_string(),
            ),
        ),
    ];

    for (case, stack, names, refusal) in &rows {
        let declarations: Vec<String> = names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                format!(
                    r#""P{index}":{{"Type":"AWS::SSM::Parameter::Value<String>",
                        "Default":"{name}"}}"#
                )
            })
            .collect();
        let template = format!(
            r#"{{"Parameters":{{{}}},
                "Resources":{{"Marker":{{
                    "Type":"AWS::CloudFormation::WaitConditionHandle"}}}}}}"#,
            declarations.join(",")
        );
        let (code, body) = create_change_set(
            &router,
            stack,
            "cs",
            &[
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", &template),
            ],
        )
        .await;
        match refusal {
            None => {
                assert_eq!(
                    code, 200,
                    "case {case:?}: the plan must be accepted, got {code} {body}"
                );
                settle_the_store(&router, &engine).await;
                let (_, body) = describe_change_set(&router, "cs", Some(stack)).await;
                let parameter = xml_blocks(&body, "member")
                    .into_iter()
                    .find(|block| xml_text(block, "ParameterKey").is_some())
                    .unwrap_or_default();
                assert_eq!(
                    (
                        xml_text(&parameter, "ParameterValue").unwrap_or_default(),
                        xml_text(&parameter, "ResolvedValue").unwrap_or_default(),
                    ),
                    ("/planned/held".to_string(), "7".to_string()),
                    "case {case:?}: the plan must report the name and the value in force, \
                     got {body}"
                );
                let (_, body) = describe_stack(&router, stack).await;
                assert_eq!(
                    xml_text(&body, "StackStatus").as_deref(),
                    Some(status::REVIEW_IN_PROGRESS),
                    "case {case:?}: the accepted plan leaves its stack in review, got {body}"
                );
            }
            Some(message) => {
                assert_eq!(
                    code, 400,
                    "case {case:?}: the plan must be refused, got {code} {body}"
                );
                assert_eq!(
                    (
                        xml_text(&body, "Code").unwrap_or_default(),
                        xml_text(&body, "Message").unwrap_or_default(),
                    ),
                    ("ValidationError".to_string(), message.clone()),
                    "case {case:?}: expected the modelled refusal, got {body}"
                );
                let (code, body) = describe_stack(&router, stack).await;
                assert_eq!(
                    code, 400,
                    "case {case:?}: the refused plan must have created no stack, got \
                     {code} {body}"
                );
            }
        }
    }

    // The rule covers every parameter the change set would DEPLOY with, not only the ones
    // the request restates: an UPDATE change set that reuses the stack's template and passes
    // no parameters still resolves the name the stack carries, and is refused once Parameter
    // Store stops answering for it.
    let deployed = r#"{"Parameters":{"P":{"Type":"AWS::SSM::Parameter::Value<String>",
            "Default":"/planned/carried"}},
        "Resources":{"Marker":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;
    put_parameter(&router, "carried", "/planned/carried", "String", "3").await;
    let (code, body) = create_stack(&router, "planned-carried", deployed, &[]).await;
    assert_eq!(
        code, 200,
        "case \"a parameter the stack carries\": the stack must be created, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (code, body) = create_change_set(
        &router,
        "planned-carried",
        "cs-carried",
        &[member("UsePreviousTemplate", "true"), member("Description", "reuse")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"a parameter the stack carries\": the plan must be accepted while the store \
         answers, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (_, body) = describe_change_set(&router, "cs-carried", Some("planned-carried")).await;
    let parameter = xml_blocks(&body, "member")
        .into_iter()
        .find(|block| xml_text(block, "ParameterKey").is_some())
        .unwrap_or_default();
    assert_eq!(
        (
            xml_text(&parameter, "ParameterValue").unwrap_or_default(),
            xml_text(&parameter, "ResolvedValue").unwrap_or_default(),
        ),
        ("/planned/carried".to_string(), "3".to_string()),
        "case \"a parameter the stack carries\": the plan must report the name it addresses \
         and the value in force, got {body}"
    );
    // The parameter is deleted out of band, through Systems Manager's own wire: the next
    // plan over the same template addresses a name the store no longer answers for.
    let (code, deleted) = ssm(
        &router,
        "DeleteParameter",
        &json!({"Name": "/planned/carried"}),
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"a parameter the stack carries\": the delete must land, got {code} {deleted}"
    );
    let (code, body) = create_change_set(
        &router,
        "planned-carried",
        "cs-carried-gone",
        &[member("UsePreviousTemplate", "true"), member("Description", "gone")],
    )
    .await;
    assert_eq!(
        (
            code,
            xml_text(&body, "Code").unwrap_or_default(),
            xml_text(&body, "Message").unwrap_or_default(),
        ),
        (
            400,
            "ValidationError".to_string(),
            "Unable to fetch parameters [/planned/carried] from parameter store for this \
             account."
                .to_string(),
        ),
        "case \"a parameter the stack carries\": a name the store no longer answers for must \
         refuse the plan, got {code} {body}"
    );
}
