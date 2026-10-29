// SPDX-License-Identifier: CC0-1.0

//! The stack's OWN configuration record after a failed update was rolled back, end to end
//! over the awsQuery wire.
//!
//! RULE: a stack reports the configuration of the last revision that was really DEPLOYED —
//! "if the stack update fails, CloudFormation rolls back the stack to the previous stack
//! configuration" — so every member a read of a rolled-back stack answers with (its
//! `Parameters`, `Tags`, `Capabilities`, its template and the description that template
//! carries) is the one in force before the failed update, and the values that update
//! submitted are discarded with the rest of its revision. The record therefore never
//! contradicts the resources the rollback put back.
//!
//! The rows span what the rule generalises over: WHICH configuration members the rejected
//! update restated (the parameter alone, under the stack's own template; the parameter with
//! tags; a whole revised template body with a description of its own; the capabilities
//! acknowledged for the update), and WHICH operation drove the failed update (`UpdateStack`,
//! and the execution of an `UPDATE` change set, which applies its revision the same way).
//! The second test is the other half of the rule, for the one update that keeps its failed
//! state (`DisableRollback`): the rejected configuration stands while the stack is parked at
//! `UPDATE_FAILED`, and the rollback a client then asks for is what puts the deployed
//! revision back. The third is the consequence a client depends on: `UsePreviousValue`
//! resolves against the restored revision, so a follow-up update re-submits the value that
//! is really deployed rather than the rejected one.
//!
//! The refusal is Amazon RDS's own: a stack parameter wired into an `AWS::RDS::DBParameterGroup`
//! resolves to a value outside the domain the engine family publishes for that parameter, so
//! the update fails in the service that owns the resource — exactly as a client's would.

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, get_template, member, rds, router,
    wait_for_the_transition, xml_blocks, xml_text, xml_texts,
};

/// The engine family every group here is created in.
const FAMILY: &str = "mysql8.0";

/// The parameter every row wires its stack parameter into.
const TUNED: &str = "innodb_lock_wait_timeout";

/// A value inside the domain `FAMILY` publishes for [`TUNED`], so the create and the
/// restored revision both deploy.
const ACCEPTED: &str = "300";

/// A value OUTSIDE that domain, which is what makes Amazon RDS refuse the update: the
/// declaration states no bound of its own, so the refusal comes from the owning service.
const REJECTED: &str = "0";

/// One stack configuration: the revision a request submits, and — read back — the one the
/// stack reports.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Configuration {
    /// The template body, or `None` where the request reuses the stack's
    /// (`UsePreviousTemplate`).
    template: Option<String>,
    parameters: Vec<(String, String)>,
    tags: Vec<(String, String)>,
    capabilities: Vec<String>,
}

/// The template body a row deploys: one `AWS::RDS::DBParameterGroup` whose tuned parameter
/// reads the stack parameter, under the stack description the revision carries.
fn template(group: &str, description: &str) -> String {
    Value::Object(serde_json::Map::from_iter([
        ("Description".to_string(), json!(description)),
        (
            "Parameters".to_string(),
            json!({"LockWait": {"Type": "Number"}}),
        ),
        (
            "Resources".to_string(),
            json!({"DbParams": {
                "Type": "AWS::RDS::DBParameterGroup",
                "Properties": {
                    "Description": "a rollback probe group",
                    "Family": FAMILY,
                    "DBParameterGroupName": group,
                    "Parameters": {TUNED: {"Ref": "LockWait"}},
                }
            }}),
        ),
    ]))
    .to_string()
}

/// How a row submits the update that fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Driver {
    /// `UpdateStack` with the revision.
    UpdateStack,
    /// `CreateChangeSet` with the revision, then `ExecuteChangeSet`.
    ChangeSet,
}

/// One row of the table: the revision the stack is created with, the one the rejected update
/// submits, and how that update is driven.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// `DBParameterGroupName`, so the row reads its group back through RDS's own API.
    group: &'static str,
    deployed: Configuration,
    rejected: Configuration,
    driver: Driver,
}

fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn words(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|entry| (*entry).to_string()).collect()
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the rejected parameter value alone, under the stack's own template",
            stack: "rollback-config-parameter",
            group: "rollback-config-parameter-group",
            deployed: Configuration {
                template: Some(template(
                    "rollback-config-parameter-group",
                    "the deployed revision",
                )),
                parameters: pairs(&[("LockWait", ACCEPTED)]),
                tags: pairs(&[("owner", "the-deployed-revision")]),
                capabilities: Vec::new(),
            },
            rejected: Configuration {
                template: None,
                parameters: pairs(&[("LockWait", REJECTED)]),
                tags: Vec::new(),
                capabilities: Vec::new(),
            },
            driver: Driver::UpdateStack,
        },
        Row {
            case: "the rejected parameter value and the tags submitted with it",
            stack: "rollback-config-tags",
            group: "rollback-config-tags-group",
            deployed: Configuration {
                template: Some(template(
                    "rollback-config-tags-group",
                    "the deployed revision",
                )),
                parameters: pairs(&[("LockWait", ACCEPTED)]),
                tags: pairs(&[("owner", "the-deployed-revision"), ("tier", "gold")]),
                capabilities: Vec::new(),
            },
            rejected: Configuration {
                template: None,
                parameters: pairs(&[("LockWait", REJECTED)]),
                tags: pairs(&[("owner", "the-rejected-revision")]),
                capabilities: Vec::new(),
            },
            driver: Driver::UpdateStack,
        },
        Row {
            case: "a whole revised template body, with a description of its own",
            stack: "rollback-config-template",
            group: "rollback-config-template-group",
            deployed: Configuration {
                template: Some(template(
                    "rollback-config-template-group",
                    "the deployed revision",
                )),
                parameters: pairs(&[("LockWait", ACCEPTED)]),
                tags: Vec::new(),
                capabilities: Vec::new(),
            },
            rejected: Configuration {
                template: Some(template(
                    "rollback-config-template-group",
                    "the revision nothing deployed",
                )),
                parameters: pairs(&[("LockWait", REJECTED)]),
                tags: Vec::new(),
                capabilities: Vec::new(),
            },
            driver: Driver::UpdateStack,
        },
        Row {
            case: "the capabilities acknowledged for the rejected update",
            stack: "rollback-config-capabilities",
            group: "rollback-config-capabilities-group",
            deployed: Configuration {
                template: Some(template(
                    "rollback-config-capabilities-group",
                    "the deployed revision",
                )),
                parameters: pairs(&[("LockWait", ACCEPTED)]),
                tags: Vec::new(),
                capabilities: Vec::new(),
            },
            rejected: Configuration {
                template: None,
                parameters: pairs(&[("LockWait", REJECTED)]),
                tags: Vec::new(),
                capabilities: words(&["CAPABILITY_IAM", "CAPABILITY_AUTO_EXPAND"]),
            },
            driver: Driver::UpdateStack,
        },
        Row {
            case: "the failed update is the execution of an UPDATE change set",
            stack: "rollback-config-change-set",
            group: "rollback-config-change-set-group",
            deployed: Configuration {
                template: Some(template(
                    "rollback-config-change-set-group",
                    "the deployed revision",
                )),
                parameters: pairs(&[("LockWait", ACCEPTED)]),
                tags: pairs(&[("owner", "the-deployed-revision")]),
                capabilities: Vec::new(),
            },
            rejected: Configuration {
                template: Some(template(
                    "rollback-config-change-set-group",
                    "the revision nothing deployed",
                )),
                parameters: pairs(&[("LockWait", REJECTED)]),
                tags: pairs(&[("owner", "the-rejected-revision")]),
                capabilities: Vec::new(),
            },
            driver: Driver::ChangeSet,
        },
    ]
}

/// The members a revision is submitted as: its template (or the instruction to reuse the
/// stack's), its parameters, its tags and its capabilities, indexed from 1 as an AWS SDK
/// sends them.
fn revision_members(revision: &Configuration) -> Vec<(String, String)> {
    let mut members = match &revision.template {
        Some(body) => vec![member("TemplateBody", body)],
        None => vec![member("UsePreviousTemplate", "true")],
    };
    for (index, (key, value)) in revision.parameters.iter().enumerate() {
        let position = index + 1;
        members.push(member(
            &format!("Parameters.member.{position}.ParameterKey"),
            key,
        ));
        members.push(member(
            &format!("Parameters.member.{position}.ParameterValue"),
            value,
        ));
    }
    for (index, (key, value)) in revision.tags.iter().enumerate() {
        let position = index + 1;
        members.push(member(&format!("Tags.member.{position}.Key"), key));
        members.push(member(&format!("Tags.member.{position}.Value"), value));
    }
    for (index, capability) in revision.capabilities.iter().enumerate() {
        let position = index + 1;
        members.push(member(
            &format!("Capabilities.member.{position}"),
            capability,
        ));
    }
    members
}

/// The configuration the stack REPORTS right now, read through `DescribeStacks` and
/// `GetTemplate` as a client reads it, beside the status it is in and the description its
/// template carries.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> (Configuration, String, Option<String>) {
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks must answer 200, got {code} {body}"
    );
    let parameter_keys = xml_blocks(&body, "Parameters")
        .into_iter()
        .next()
        .unwrap_or_default();
    let mut parameters: Vec<(String, String)> = xml_texts(&parameter_keys, "ParameterKey")
        .into_iter()
        .zip(xml_texts(&parameter_keys, "ParameterValue"))
        .collect();
    parameters.sort();
    let tag_block = xml_blocks(&body, "Tags")
        .into_iter()
        .next()
        .unwrap_or_default();
    let mut tags: Vec<(String, String)> = xml_texts(&tag_block, "Key")
        .into_iter()
        .zip(xml_texts(&tag_block, "Value"))
        .collect();
    tags.sort();
    let capability_block = xml_blocks(&body, "Capabilities")
        .into_iter()
        .next()
        .unwrap_or_default();
    let mut capabilities = xml_texts(&capability_block, "member");
    capabilities.sort();
    let (code, template_body) = get_template(router, stack, None).await;
    assert_eq!(
        code, 200,
        "case {case:?}: GetTemplate must answer 200, got {code} {template_body}"
    );
    (
        Configuration {
            template: xml_text(&template_body, "TemplateBody"),
            parameters,
            tags,
            capabilities,
        },
        xml_text(&body, "StackStatus").unwrap_or_default(),
        xml_text(&body, "Description"),
    )
}

/// The revision as the stack must report it: the same members, in the order a read answers
/// them in.
fn as_reported(revision: &Configuration) -> Configuration {
    let mut parameters = revision.parameters.clone();
    parameters.sort();
    let mut tags = revision.tags.clone();
    tags.sort();
    let mut capabilities = revision.capabilities.clone();
    capabilities.sort();
    Configuration {
        template: revision.template.clone(),
        parameters,
        tags,
        capabilities,
    }
}

/// The value Amazon RDS holds for a group's tuned parameter, read through its own API.
async fn value_in_rds(router: &MultiServiceRouter, case: &str, group: &str) -> String {
    let (code, body) = rds(
        router,
        "DescribeDBParameters",
        &[("DBParameterGroupName", group), ("Source", "user")],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeDBParameters({group}) must answer 200, got {code} {body}"
    );
    xml_texts(&body, "ParameterName")
        .into_iter()
        .zip(xml_texts(&body, "ParameterValue"))
        .find(|(name, _)| name == TUNED)
        .map(|(_, value)| value)
        .unwrap_or_default()
}

/// Create the row's stack with the revision it deploys, and answer once it is complete.
async fn deploy(router: &MultiServiceRouter, case: &str, stack: &str, deployed: &Configuration) {
    let body = deployed
        .template
        .clone()
        .expect("a create carries a template body");
    let mut extra = revision_members(deployed);
    // The template body is the create's own member, so it is not restated here.
    extra.retain(|(key, _)| key != "TemplateBody");
    let (code, answered) = create_stack(router, stack, &body, &extra).await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must answer 200, got {code} {answered}"
    );
    wait_for_the_transition().await;
    let (_, created, _) = reported(router, case, stack).await;
    assert_eq!(
        created,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the create to complete before the update, got {created:?}"
    );
}

/// Submit a revision as an update that the owning service will refuse, through the driver
/// the row names, with whatever extra members the case carries.
async fn submit(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    revision: &Configuration,
    driver: Driver,
    extra: &[(String, String)],
) {
    match driver {
        Driver::UpdateStack => {
            let mut members = vec![member("StackName", stack)];
            members.extend(revision_members(revision));
            members.extend_from_slice(extra);
            let (code, answered) = call(router, "UpdateStack", &members).await;
            assert_eq!(
                code, 200,
                "case {case:?}: UpdateStack must answer 200, got {code} {answered}"
            );
        }
        Driver::ChangeSet => {
            let mut members = vec![
                member("StackName", stack),
                member("ChangeSetName", "rejected-revision"),
                member("ChangeSetType", "UPDATE"),
            ];
            members.extend(revision_members(revision));
            let (code, answered) = call(router, "CreateChangeSet", &members).await;
            assert_eq!(
                code, 200,
                "case {case:?}: CreateChangeSet must answer 200, got {code} {answered}"
            );
            let change_set = xml_text(&answered, "Id").unwrap_or_default();
            wait_for_the_transition().await;
            let mut members = vec![member("ChangeSetName", &change_set)];
            members.extend_from_slice(extra);
            let (code, answered) = call(router, "ExecuteChangeSet", &members).await;
            assert_eq!(
                code, 200,
                "case {case:?}: ExecuteChangeSet must answer 200, got {code} {answered}"
            );
        }
    }
}

/// The description the row's deployed template carries, which is what the stack reports for
/// the revision in force.
fn deployed_description(deployed: &Configuration) -> Option<String> {
    let body = deployed.template.as_deref()?;
    serde_json::from_str::<Value>(body)
        .ok()?
        .get("Description")?
        .as_str()
        .map(str::to_string)
}

#[tokio::test]
async fn a_rolled_back_update_leaves_the_stack_reporting_the_deployed_revision() {
    for row in rows() {
        let case = row.case;
        let router = router();
        deploy(&router, case, row.stack, &row.deployed).await;
        let (before, _, description_before) = reported(&router, case, row.stack).await;
        assert_eq!(
            before,
            as_reported(&row.deployed),
            "case {case:?}: expected the create to put its own revision on the stack, got \
             {before:?}"
        );

        submit(&router, case, row.stack, &row.rejected, row.driver, &[]).await;
        wait_for_the_transition().await;

        let (after, settled, description_after) = reported(&router, case, row.stack).await;
        assert_eq!(
            settled,
            status::UPDATE_ROLLBACK_COMPLETE,
            "case {case:?}: expected the refused update to roll the stack back, got {settled:?}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: expected the rolled-back stack to report the revision it really \
             deployed, got {after:?}"
        );
        assert_eq!(
            description_after,
            description_before,
            "case {case:?}: expected the description of the deployed template, got \
             {description_after:?}"
        );
        assert_eq!(
            description_after,
            deployed_description(&row.deployed),
            "case {case:?}: expected the stack to describe the template it deploys, got \
             {description_after:?}"
        );
        // ...and the record agrees with the resource the rollback put back, which is the
        // whole point of restoring it.
        let held = value_in_rds(&router, case, row.group).await;
        assert_eq!(
            held, ACCEPTED,
            "case {case:?}: expected Amazon RDS to hold the deployed value for {TUNED}, got \
             {held:?}"
        );
    }
}

/// `DisableRollback` is the one update that KEEPS its failed state: the configuration it
/// submitted stands while the stack is parked at `UPDATE_FAILED`, and the rollback a client
/// then asks for is what puts the deployed revision back — the same restore, reached through
/// `RollbackStack`.
#[tokio::test]
async fn a_rollback_a_client_asks_for_restores_the_kept_updates_previous_configuration() {
    let case = "a failed update kept by DisableRollback, then rolled back on request";
    let stack = "rollback-config-kept";
    let group = "rollback-config-kept-group";
    let deployed = Configuration {
        template: Some(template(group, "the deployed revision")),
        parameters: pairs(&[("LockWait", ACCEPTED)]),
        tags: pairs(&[("owner", "the-deployed-revision")]),
        capabilities: Vec::new(),
    };
    let rejected = Configuration {
        template: Some(template(group, "the revision nothing deployed")),
        parameters: pairs(&[("LockWait", REJECTED)]),
        tags: pairs(&[("owner", "the-rejected-revision")]),
        capabilities: Vec::new(),
    };
    let router = router();
    deploy(&router, case, stack, &deployed).await;

    submit(
        &router,
        case,
        stack,
        &rejected,
        Driver::UpdateStack,
        &[member("DisableRollback", "true")],
    )
    .await;
    wait_for_the_transition().await;

    // The caller asked for the failed state to be kept, so the stack is parked at
    // UPDATE_FAILED carrying what that update submitted.
    let (kept, failed, _) = reported(&router, case, stack).await;
    assert_eq!(
        failed,
        status::UPDATE_FAILED,
        "case {case:?}: expected DisableRollback to park the failed update, got {failed:?}"
    );
    assert_eq!(
        kept,
        as_reported(&rejected),
        "case {case:?}: expected the kept update's own configuration to stand, got {kept:?}"
    );

    let (code, body) = call(&router, "RollbackStack", &[member("StackName", stack)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: RollbackStack must answer 200, got {code} {body}"
    );
    wait_for_the_transition().await;

    let (restored, settled, description) = reported(&router, case, stack).await;
    assert_eq!(
        settled,
        status::UPDATE_ROLLBACK_COMPLETE,
        "case {case:?}: expected the requested rollback to settle the stack, got {settled:?}"
    );
    assert_eq!(
        restored,
        as_reported(&deployed),
        "case {case:?}: expected the requested rollback to put the deployed revision back, got \
         {restored:?}"
    );
    assert_eq!(
        description,
        deployed_description(&deployed),
        "case {case:?}: expected the description of the deployed template, got {description:?}"
    );
    let held = value_in_rds(&router, case, group).await;
    assert_eq!(
        held, ACCEPTED,
        "case {case:?}: expected Amazon RDS to hold the deployed value for {TUNED}, got {held:?}"
    );
}

/// The consequence a client depends on: `UsePreviousValue` — "use the existing parameter
/// value that the stack is using for a given parameter key" — resolves against the RESTORED
/// revision, so a follow-up update re-submits the value that is really deployed and
/// completes, rather than re-submitting the one the owning service refused.
#[tokio::test]
async fn use_previous_value_after_a_rollback_resolves_to_the_deployed_value() {
    let case = "a follow-up update asking for the previous value after a rollback";
    let stack = "rollback-config-previous-value";
    let group = "rollback-config-previous-value-group";
    let deployed = Configuration {
        template: Some(template(group, "the deployed revision")),
        parameters: pairs(&[("LockWait", ACCEPTED)]),
        tags: Vec::new(),
        capabilities: Vec::new(),
    };
    let rejected = Configuration {
        template: None,
        parameters: pairs(&[("LockWait", REJECTED)]),
        tags: Vec::new(),
        capabilities: Vec::new(),
    };
    let router = router();
    deploy(&router, case, stack, &deployed).await;
    submit(&router, case, stack, &rejected, Driver::UpdateStack, &[]).await;
    wait_for_the_transition().await;
    let (_, settled, _) = reported(&router, case, stack).await;
    assert_eq!(
        settled,
        status::UPDATE_ROLLBACK_COMPLETE,
        "case {case:?}: expected the refused update to roll the stack back, got {settled:?}"
    );

    // A revision that changes the template's description and asks for the parameter value the
    // stack holds: the value it resolves to is the one really deployed, which the owning
    // service accepts.
    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &template(group, "a later revision")),
            member("Parameters.member.1.ParameterKey", "LockWait"),
            member("Parameters.member.1.UsePreviousValue", "true"),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: UpdateStack must answer 200, got {code} {body}"
    );
    wait_for_the_transition().await;

    let (later, completed, _) = reported(&router, case, stack).await;
    assert_eq!(
        completed,
        status::UPDATE_COMPLETE,
        "case {case:?}: expected the follow-up update to deploy the previous value, got \
         {completed:?}"
    );
    assert_eq!(
        later.parameters,
        pairs(&[("LockWait", ACCEPTED)]),
        "case {case:?}: expected UsePreviousValue to resolve to the deployed value, got {:?}",
        later.parameters
    );
    let held = value_in_rds(&router, case, group).await;
    assert_eq!(
        held, ACCEPTED,
        "case {case:?}: expected Amazon RDS to hold the deployed value for {TUNED}, got {held:?}"
    );
}
