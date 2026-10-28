// SPDX-License-Identifier: CC0-1.0

//! An UPDATE whose only change is the stack's own `Parameters`, end to end over the awsQuery
//! wire.
//!
//! RULE: a stack UPDATE deploys the declaration the stack's template EVALUATES to against the
//! parameter values then in force — "specify a new stack configuration … CloudFormation
//! updates the stack" — so a change confined to `UpdateStack`'s `Parameters` under
//! `UsePreviousTemplate` reaches the service that owns each affected resource as the same
//! in-place change a revised template body would: the stack settles `UPDATE_COMPLETE`, the
//! resource keeps the physical id it was created under, and the owning service reports the
//! newly RESOLVED values. The same rule decides a refusal: a resolved value the owning service
//! rejects fails that resource's update and the stack is rolled back with the last accepted
//! value still in force, never an `UPDATE_COMPLETE` over a write that service refused.
//!
//! The rows span what the rule generalises over: how the declaration READS the parameter
//! (`Ref` and an `Fn::Sub` variable), how many parameters the template declares and how many
//! of them the update restates (one, and one changed beside one asked for with
//! `UsePreviousValue`), the engine family, and the resource KIND — the instance-scoped
//! `AWS::RDS::DBParameterGroup` and its cluster-scoped sibling, whose owning API addresses a
//! group and reports its parameters through members of its own.
//!
//! Amazon RDS is the owning service here because a parameter group is the resource whose whole
//! configuration is the values a template states for it, so what a resolved value changed is
//! observable through that service's own parameter report. `crate::state::provisioner` is the
//! layer under test, reached through the deployment that drives it.

use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::rds_stack_resources::{group_settings, physical_id};
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, rds, resource_event_reason, router,
    wait_for_the_transition, xml_blocks, xml_text, xml_texts,
};

/// How one entry of an update's `Parameters` states the value for its key.
#[derive(Debug, Clone, Copy)]
enum Restated {
    /// `ParameterValue`: the value the update states for the key.
    To(&'static str),
    /// `UsePreviousValue`: "During a stack update, use the existing parameter value that the
    /// stack is using for a given parameter key" — the key is restated, its value left as the
    /// stack holds it.
    ThePreviousValue,
}

/// One row of the table: the template the stack is created with and KEEPS, the parameter
/// values it is created with, the ones the update restates, and what the owning service must
/// report at each step.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The ONE template body of the row: the create deploys it and the update reuses it
    /// (`UsePreviousTemplate`), so the only thing that changes is a parameter value.
    template: &'static str,
    created_with: &'static [(&'static str, &'static str)],
    updated_with: &'static [(&'static str, Restated)],
    /// The member Amazon RDS addresses this group KIND by, and the action that reports its
    /// parameters.
    name_member: &'static str,
    parameters_describe: &'static str,
    /// The settings the group must carry after the create and after the update, each in the
    /// name order the parameter report answers in.
    after_create: &'static [(&'static str, &'static str)],
    after_update: &'static [(&'static str, &'static str)],
    /// The stack's own `Parameters` once the update has settled, in the order it reports them.
    stack_parameters: &'static [(&'static str, &'static str)],
}

/// The parameter every row states is one its family PUBLISHES, so each stated value is inside
/// the domain that family admits — which is what the refusal case below reads off
/// `DescribeEngineDefaultParameters` rather than restating.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one Ref-resolved value restated",
            stack: "param-driven-single",
            template: r#"{"Parameters":{"LockWait":{"Type":"String"}},
                "Resources":{"DbParams":{"Type":"AWS::RDS::DBParameterGroup",
                  "Properties":{"Description":"a parameter-driven group","Family":"mysql8.0",
                    "Parameters":{"innodb_lock_wait_timeout":{"Ref":"LockWait"}}}}}}"#,
            created_with: &[("LockWait", "120")],
            updated_with: &[("LockWait", Restated::To("300"))],
            name_member: "DBParameterGroupName",
            parameters_describe: "DescribeDBParameters",
            after_create: &[("innodb_lock_wait_timeout", "120")],
            after_update: &[("innodb_lock_wait_timeout", "300")],
            stack_parameters: &[("LockWait", "300")],
        },
        Row {
            case: "one value changed beside one asked for by UsePreviousValue",
            stack: "param-driven-pair",
            template: r#"{"Parameters":{"MaxConns":{"Type":"String"},
                  "Flush":{"Type":"String"}},
                "Resources":{"DbParams":{"Type":"AWS::RDS::DBParameterGroup",
                  "Properties":{"Description":"a parameter-driven pair","Family":"mysql8.0",
                    "Parameters":{"max_connections":{"Ref":"MaxConns"},
                      "innodb_flush_log_at_trx_commit":{"Ref":"Flush"}}}}}}"#,
            created_with: &[("MaxConns", "150"), ("Flush", "1")],
            updated_with: &[
                ("MaxConns", Restated::To("400")),
                ("Flush", Restated::ThePreviousValue),
            ],
            name_member: "DBParameterGroupName",
            parameters_describe: "DescribeDBParameters",
            after_create: &[
                ("innodb_flush_log_at_trx_commit", "1"),
                ("max_connections", "150"),
            ],
            after_update: &[
                ("innodb_flush_log_at_trx_commit", "1"),
                ("max_connections", "400"),
            ],
            stack_parameters: &[("MaxConns", "400"), ("Flush", "1")],
        },
        Row {
            case: "a value the declaration reads through an Fn::Sub variable",
            stack: "param-driven-substituted",
            template: r#"{"Parameters":{"Seconds":{"Type":"String"}},
                "Resources":{"DbParams":{"Type":"AWS::RDS::DBParameterGroup",
                  "Properties":{"Description":"a substituted group","Family":"mysql8.0",
                    "Parameters":{"wait_timeout":{"Fn::Sub":"${Seconds}"}}}}}}"#,
            created_with: &[("Seconds", "600")],
            updated_with: &[("Seconds", Restated::To("1200"))],
            name_member: "DBParameterGroupName",
            parameters_describe: "DescribeDBParameters",
            after_create: &[("wait_timeout", "600")],
            after_update: &[("wait_timeout", "1200")],
            stack_parameters: &[("Seconds", "1200")],
        },
        Row {
            case: "the CLUSTER-scoped group kind, through its own modify and report",
            stack: "param-driven-cluster",
            template: r#"{"Parameters":{"Format":{"Type":"String"}},
                "Resources":{"DbParams":{"Type":"AWS::RDS::DBClusterParameterGroup",
                  "Properties":{"Description":"a parameter-driven cluster group",
                    "Family":"aurora-mysql8.0",
                    "Parameters":{"binlog_format":{"Ref":"Format"}}}}}}"#,
            created_with: &[("Format", "ROW")],
            updated_with: &[("Format", Restated::To("MIXED"))],
            name_member: "DBClusterParameterGroupName",
            parameters_describe: "DescribeDBClusterParameters",
            after_create: &[("binlog_format", "ROW")],
            after_update: &[("binlog_format", "MIXED")],
            stack_parameters: &[("Format", "MIXED")],
        },
    ]
}

/// The `Parameters` members a CREATE carries for `values`, indexed from 1 as an AWS SDK sends
/// them.
fn created_members(values: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut members = Vec::new();
    for (index, (key, value)) in values.iter().enumerate() {
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
    members
}

/// The members an UPDATE carries: the stack's own template, and each restated parameter either
/// with its new value or asking for the one the stack already holds.
fn updated_members(stack: &str, restated: &[(&str, Restated)]) -> Vec<(String, String)> {
    let mut members = vec![
        member("StackName", stack),
        member("UsePreviousTemplate", "true"),
    ];
    for (index, (key, how)) in restated.iter().enumerate() {
        let position = index + 1;
        members.push(member(
            &format!("Parameters.member.{position}.ParameterKey"),
            key,
        ));
        match how {
            Restated::To(value) => members.push(member(
                &format!("Parameters.member.{position}.ParameterValue"),
                value,
            )),
            Restated::ThePreviousValue => members.push(member(
                &format!("Parameters.member.{position}.UsePreviousValue"),
                "true",
            )),
        }
    }
    members
}

/// The settings the group `group` carries, asserted to be answered 200.
async fn settings_of(
    router: &MultiServiceRouter,
    case: &str,
    parameters_describe: &str,
    name_member: &str,
    group: &str,
    what: &str,
) -> Vec<(String, String)> {
    let (code, body, settings) =
        group_settings(router, parameters_describe, name_member, group).await;
    assert_eq!(
        code, 200,
        "case {case:?}: {parameters_describe}({group}) must answer 200 {what}, got {code} \
         {body}"
    );
    settings
}

/// The pairs a row states, as the settings report is compared against.
fn expected(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// The `(ParameterKey, ParameterValue)` pairs a `DescribeStacks` body reports.
fn reported_parameters(xml: &str) -> Vec<(String, String)> {
    xml_texts(xml, "ParameterKey")
        .into_iter()
        .zip(xml_texts(xml, "ParameterValue"))
        .collect()
}

/// The `AllowedValues` Amazon RDS publishes for `parameter` in the family `family`, read
/// through `DescribeEngineDefaultParameters` as a client reads it — the source of the domain a
/// value is inside or outside of, so this suite restates no range of its own.
async fn published_allowed_values(
    router: &MultiServiceRouter,
    case: &str,
    family: &str,
    parameter: &str,
) -> String {
    let (code, body) = rds(
        router,
        "DescribeEngineDefaultParameters",
        &[("DBParameterGroupFamily", family)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeEngineDefaultParameters({family}) must answer 200, got {code} \
         {body}"
    );
    let published = xml_blocks(&body, "Parameter")
        .into_iter()
        .find(|entry| xml_text(entry, "ParameterName").as_deref() == Some(parameter))
        .and_then(|entry| xml_text(&entry, "AllowedValues"));
    published.unwrap_or_else(|| {
        panic!(
            "case {case:?}: the {family} family must publish {parameter} with its allowed \
             values, got {body}"
        )
    })
}

/// The `StackStatus` the stack reports right now, asserted to be answered 200.
async fn stack_status(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks must answer 200, got {code} {body}"
    );
    xml_text(&body, "StackStatus").unwrap_or_default()
}

#[tokio::test]
async fn a_changed_stack_parameter_under_the_previous_template_is_applied_in_the_owning_service()
{
    for row in rows() {
        let router = router();
        let (code, body) = create_stack(
            &router,
            row.stack,
            row.template,
            &created_members(row.created_with),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must answer 200, got {code} {body}",
            row.case
        );
        wait_for_the_transition().await;
        let created = stack_status(&router, row.case, row.stack).await;
        assert_eq!(
            created,
            status::CREATE_COMPLETE,
            "case {:?}: expected the create to complete before the update, got {created:?}",
            row.case
        );
        let group = physical_id(&router, row.stack, "DbParams").await;
        assert_eq!(
            settings_of(
                &router,
                row.case,
                row.parameters_describe,
                row.name_member,
                &group,
                "after the create",
            )
            .await,
            expected(row.after_create),
            "case {:?}: expected the create to resolve its declaration onto the group",
            row.case
        );

        let (code, body) = call(
            &router,
            "UpdateStack",
            &updated_members(row.stack, row.updated_with),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {:?}: UpdateStack must answer 200, got {code} {body}",
            row.case
        );
        wait_for_the_transition().await;

        let settled = stack_status(&router, row.case, row.stack).await;
        assert_eq!(
            settled,
            status::UPDATE_COMPLETE,
            "case {:?}: expected the stack to complete the update, got {settled:?}",
            row.case
        );
        // The group was changed IN PLACE: the handle a DB instance would name it by, and that
        // the stack publishes, is the one the create reported.
        let after = physical_id(&router, row.stack, "DbParams").await;
        assert_eq!(
            after, group,
            "case {:?}: expected the group to keep the id it was created under, got {after:?}",
            row.case
        );
        let held = settings_of(
            &router,
            row.case,
            row.parameters_describe,
            row.name_member,
            &group,
            "after the update",
        )
        .await;
        assert_eq!(
            held,
            expected(row.after_update),
            "case {:?}: expected Amazon RDS to report the values the update resolved to, got \
             {held:?}",
            row.case
        );

        // And the stack reports the parameter set it deployed, so a later update resolves
        // `UsePreviousValue` against the values now in force.
        let (code, body) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {:?}: DescribeStacks must answer 200, got {code} {body}",
            row.case
        );
        let reported = reported_parameters(&body);
        assert_eq!(
            reported,
            expected(row.stack_parameters),
            "case {:?}: expected the stack to carry the parameters the update deployed, got \
             {reported:?}",
            row.case
        );
    }
}

/// A resolved value the owning service REFUSES fails that resource's update, so the stack is
/// rolled back with the last accepted value still in force and the refusal is reported as the
/// resource's own reason — the other half of the same rule, reached through the same
/// changed-parameter path.
///
/// The value is one below the lower bound of the range the family publishes for the parameter,
/// which is what puts it outside the domain the engine admits.
#[tokio::test]
async fn a_refused_parameter_value_rolls_the_stack_back_to_the_last_accepted_one() {
    let case = "a stack parameter resolved below the published lower bound";
    let stack = "param-driven-refused";
    let family = "mysql8.0";
    let parameter = "innodb_lock_wait_timeout";
    // One below the lower bound of the range the family publishes for the parameter, which the
    // assertion below reads off `DescribeEngineDefaultParameters`.
    let refused = "0";
    let accepted = "300";
    let template = r#"{"Parameters":{"LockWait":{"Type":"String"}},
        "Resources":{"DbParams":{"Type":"AWS::RDS::DBParameterGroup",
          "Properties":{"Description":"a parameter-driven group","Family":"mysql8.0",
            "Parameters":{"innodb_lock_wait_timeout":{"Ref":"LockWait"}}}}}}"#;
    let router = router();

    let (code, body) = create_stack(
        &router,
        stack,
        template,
        &created_members(&[("LockWait", accepted)]),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must answer 200, got {code} {body}"
    );
    wait_for_the_transition().await;
    let created = stack_status(&router, case, stack).await;
    assert_eq!(
        created,
        status::CREATE_COMPLETE,
        "case {case:?}: expected the create to complete before the update, got {created:?}"
    );
    let group = physical_id(&router, stack, "DbParams").await;

    let (code, body) = call(
        &router,
        "UpdateStack",
        &updated_members(stack, &[("LockWait", Restated::To(refused))]),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: UpdateStack must answer 200, got {code} {body}"
    );
    wait_for_the_transition().await;

    let settled = stack_status(&router, case, stack).await;
    assert_eq!(
        settled,
        status::UPDATE_ROLLBACK_COMPLETE,
        "case {case:?}: expected a refused value to fail the update and roll it back, got \
         {settled:?}"
    );
    let held = settings_of(
        &router,
        case,
        "DescribeDBParameters",
        "DBParameterGroupName",
        &group,
        "after the refused update",
    )
    .await;
    assert_eq!(
        held,
        expected(&[(parameter, accepted)]),
        "case {case:?}: expected the last accepted value to still be in force, got {held:?}"
    );
    // The refusal is the owning service's own, reported on the resource it was refused for:
    // the submitted value, the range the family publishes for the parameter, and the parameter
    // the declaration named.
    let reason = resource_event_reason(
        &router,
        stack,
        "DbParams",
        resource_status::UPDATE_FAILED,
    )
    .await;
    let domain = published_allowed_values(&router, case, family, parameter).await;
    let refusal =
        format!("Value: {refused} is outside of range: {domain} for parameter: {parameter}");
    assert!(
        reason.contains(&refusal),
        "case {case:?}: expected the resource's reason to carry Amazon RDS's refusal \
         {refusal:?} of the resolved value, got {reason:?}"
    );
}
