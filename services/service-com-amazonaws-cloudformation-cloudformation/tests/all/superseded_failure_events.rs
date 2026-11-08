// SPDX-License-Identifier: CC0-1.0

//! WHICH events report a failed stack operation, end to end over the awsQuery wire.
//!
//! RULE under test: **a stack-level event reports a failed status only where the stack RESTS
//! in it.** A create or update CloudFormation rolls back never rests in `CREATE_FAILED` /
//! `UPDATE_FAILED` — it goes from the operation's own `*_IN_PROGRESS` straight into
//! `ROLLBACK_IN_PROGRESS` / `UPDATE_ROLLBACK_IN_PROGRESS`, whose event carries the failure
//! and, for a rolled-back create, the clause saying the rollback was asked for — so the only
//! `*_FAILED` event such an operation leaves is the one of the RESOURCE that failed. A caller
//! that asked for the failed state to be kept (`OnFailure=DO_NOTHING`, `DisableRollback`)
//! gets a stack that does rest in the failed status, and then the stack's own event reports
//! it too.
//!
//! Why it matters: deployment tooling reads the event stream to find WHAT failed — the
//! `*_FAILED` events, whose `LogicalResourceId` and `ResourceType` name the resources to fix.
//! A stack-level `*_FAILED` event for an operation that rolled back puts the stack itself in
//! that set, which names no resource anybody can fix and hides the layering an operator is
//! taught to use.
//!
//! `stack_failure_reason.rs` owns the REASON those events carry; this suite owns which of
//! them exist at all.
//!
//! The rows differ along what the rule generalises over: the action that failed (a create, an
//! update) and what the request asked for on failure (the documented default rollback,
//! `OnFailure=DO_NOTHING`, `DisableRollback=true`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::REGION;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, resource_event_reason, router,
    wait_for_the_transition, xml_blocks, xml_text,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary, resource_status, rollback_requested, status,
};

/// The logical id of the resource every row fails on, and its type.
const FAILING: &str = "RoleAttachment";
const FAILING_TYPE: &str = "AWS::Cognito::IdentityPoolRoleAttachment";

/// The type of the stack itself, which a stack-level event is reported under.
const STACK_TYPE: &str = "AWS::CloudFormation::Stack";

/// An IAM role ARN in the form `SetIdentityPoolRoles` takes, so the declaration below is
/// refused for the ROLE MAPPING it states rather than for the roles beside it.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/superseded-failure-auth";

/// The provider a role mapping is keyed by.
const PROVIDER: &str = "graph.facebook.com";

/// A template whose identity pool is fine and whose role attachment cannot be applied: a
/// `RoleMapping` of type `Rules` states no `RulesConfiguration`, which the owning service
/// refuses ("If you specify Rules as the role mapping type, RulesConfiguration is required",
/// `RoleMapping$RulesConfiguration`), so the resource fails and the operation fails with it.
fn template_with_an_invalid_role_mapping(pool_name: &str) -> String {
    json!({
        "Resources": {
            "Pool": {
                "Type": "AWS::Cognito::IdentityPool",
                "Properties": {
                    "IdentityPoolName": pool_name,
                    "AllowUnauthenticatedIdentities": true
                }
            },
            FAILING: {
                "Type": FAILING_TYPE,
                "Properties": {
                    "IdentityPoolId": {"Ref": "Pool"},
                    "Roles": {"authenticated": ROLE_ARN},
                    "RoleMappings": {
                        PROVIDER: {"Type": "Rules", "AmbiguousRoleResolution": "Deny"}
                    }
                }
            }
        }
    })
    .to_string()
}

/// The same template with a role mapping the owning service DOES apply: a `Token` mapping
/// needs no rules at all, so the attachment is created and the stack completes.
fn template_with_a_valid_role_mapping(pool_name: &str) -> String {
    json!({
        "Resources": {
            "Pool": {
                "Type": "AWS::Cognito::IdentityPool",
                "Properties": {
                    "IdentityPoolName": pool_name,
                    "AllowUnauthenticatedIdentities": true
                }
            },
            FAILING: {
                "Type": FAILING_TYPE,
                "Properties": {
                    "IdentityPoolId": {"Ref": "Pool"},
                    "Roles": {"authenticated": ROLE_ARN},
                    "RoleMappings": {
                        PROVIDER: {"Type": "Token", "AmbiguousRoleResolution": "Deny"}
                    }
                }
            }
        }
    })
    .to_string()
}

/// Every event of `stack` whose `ResourceStatus` is a FAILED one, as the pairs deployment
/// tooling reads them: `(LogicalResourceId, ResourceType)`, sorted so the assertion does not
/// depend on the order the stream reports them in.
async fn failed_events(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> Vec<(String, String)> {
    let (code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackEvents({stack}) must answer 200, got {code} {body}"
    );
    let mut failed: Vec<(String, String)> = xml_blocks(&body, "member")
        .iter()
        .filter(|entry| {
            xml_text(entry, "ResourceStatus")
                .is_some_and(|reported| reported.ends_with("_FAILED"))
        })
        .map(|entry| {
            (
                xml_text(entry, "LogicalResourceId").unwrap_or_default(),
                xml_text(entry, "ResourceType").unwrap_or_default(),
            )
        })
        .collect();
    failed.sort();
    failed
}

/// One Amazon Cognito Identity request, over the awsJson1.1 wire it speaks — the owning
/// service of the two resource types the templates above declare, mounted beside
/// CloudFormation on the same engine.
async fn cognito_identity(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://cognito-identity.{REGION}.amazonaws.com/"))
        .header("host", format!("cognito-identity.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityService.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/cognito-identity/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The names of the identity pools the owning service holds.
async fn pools_in_cognito_identity(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (code, body) = cognito_identity(router, "ListIdentityPools", &json!({"MaxResults": 60}))
        .await;
    assert_eq!(
        code, 200,
        "case {case:?}: ListIdentityPools must answer 200, got {code} with {body}"
    );
    body["IdentityPools"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListIdentityPools answers a list: {body}"))
        .iter()
        .map(|pool| {
            pool["IdentityPoolName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every `ResourceStatus` the stack's own events carry, newest first.
async fn stack_statuses(router: &MultiServiceRouter, case: &str, stack: &str) -> Vec<String> {
    let (code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackEvents({stack}) must answer 200, got {code} {body}"
    );
    xml_blocks(&body, "member")
        .iter()
        .filter(|entry| xml_text(entry, "LogicalResourceId").as_deref() == Some(stack))
        .filter_map(|entry| xml_text(entry, "ResourceStatus"))
        .collect()
}

/// One create row: what the request asked for on failure, and whether the stack rests in the
/// failed status because of it.
struct CreateRow {
    case: &'static str,
    stack: &'static str,
    /// The members the request carries beside the template.
    on_failure: Option<(&'static str, &'static str)>,
    /// Whether the stack is left in `CREATE_FAILED`.
    rests_in_the_failure: bool,
}

const CREATE_ROWS: &[CreateRow] = &[
    CreateRow {
        // "ROLLBACK — Roll back the stack creation. This is the default."
        case: "a failed create the request let CloudFormation roll back",
        stack: "superseded-rolled-back",
        on_failure: None,
        rests_in_the_failure: false,
    },
    CreateRow {
        case: "a failed create whose request asked for the failed state to be kept",
        stack: "superseded-do-nothing",
        on_failure: Some(("OnFailure", "DO_NOTHING")),
        rests_in_the_failure: true,
    },
    CreateRow {
        case: "a failed create whose request disabled rollback",
        stack: "superseded-rollback-disabled",
        on_failure: Some(("DisableRollback", "true")),
        rests_in_the_failure: true,
    },
];

/// A failed CREATE leaves a `CREATE_FAILED` event for the resource that failed, and one for
/// the STACK only where the stack is left resting in that status.
#[tokio::test]
async fn a_rolled_back_create_reports_no_stack_level_failure_event() {
    for row in CREATE_ROWS {
        let router = router();
        let template = template_with_an_invalid_role_mapping(row.stack);
        let extra: Vec<(String, String)> = row
            .on_failure
            .map(|(name, value)| vec![member(name, value)])
            .unwrap_or_default();
        let (code, body) = create_stack(&router, row.stack, &template, &extra).await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must be accepted — the stack reports the outcome — got \
             {code} {body}",
            row.case
        );
        // The failure, and then the rollback it owes, each one transition away.
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let (_, described) = describe_stack(&router, row.stack).await;
        let expected_status = if row.rests_in_the_failure {
            status::CREATE_FAILED
        } else {
            status::ROLLBACK_COMPLETE
        };
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(expected_status),
            "case {:?}: expected the stack to settle at {expected_status:?}, got {described}",
            row.case
        );

        // The resource the owning service refused has its own event, carrying that refusal:
        // that is what says WHAT to fix, and it is there whatever became of the stack.
        let diagnosis = resource_event_reason(
            &router,
            row.stack,
            FAILING,
            resource_status::CREATE_FAILED,
        )
        .await;
        assert!(
            diagnosis.contains("Rules"),
            "case {:?}: expected the failed resource's event to carry the owning service's \
             refusal, got {diagnosis:?}",
            row.case
        );

        // ...and the stack itself is in that set only where it rests in the failed status.
        let summary = resource_failure_summary(failed_action::CREATE, [FAILING]);
        let mut expected = vec![(FAILING.to_string(), FAILING_TYPE.to_string())];
        if row.rests_in_the_failure {
            expected.push((row.stack.to_string(), STACK_TYPE.to_string()));
        }
        expected.sort();
        let reported = failed_events(&router, row.case, row.stack).await;
        assert_eq!(
            reported, expected,
            "case {:?}: expected exactly {expected:?} to carry a failed status, got \
             {reported:?}",
            row.case
        );

        let statuses = stack_statuses(&router, row.case, row.stack).await;
        if row.rests_in_the_failure {
            assert!(
                statuses.contains(&status::CREATE_FAILED.to_string())
                    && !statuses.contains(&status::ROLLBACK_IN_PROGRESS.to_string()),
                "case {:?}: expected the kept failure and no rollback, got {statuses:?}",
                row.case
            );
            continue;
        }
        for owed in [status::ROLLBACK_IN_PROGRESS, status::ROLLBACK_COMPLETE] {
            assert!(
                statuses.contains(&owed.to_string()),
                "case {:?}: expected the stack's own {owed} event, got {statuses:?}",
                row.case
            );
        }
        // The rollback's own event is where the failure the stack never rested in is
        // reported, with the clause saying the rollback was asked for.
        let rollback_reason = resource_event_reason(
            &router,
            row.stack,
            row.stack,
            status::ROLLBACK_IN_PROGRESS,
        )
        .await;
        assert_eq!(
            rollback_reason,
            rollback_requested(&summary),
            "case {:?}: expected the rollback's event to carry the failure, got \
             {rollback_reason:?}",
            row.case
        );
        // ...and the rollback is not a report: the pool the failed create HAD made is gone
        // from the service that owns it, so nothing the rolled-back stack created is left
        // behind for a later caller to reach.
        let pools = pools_in_cognito_identity(&router, row.case).await;
        assert!(
            !pools.contains(&row.stack.to_string()),
            "case {:?}: expected the rollback to remove the pool the create made, Amazon \
             Cognito Identity still holds {pools:?}",
            row.case
        );
    }
}

/// One update row: whether the request disabled rollback, and so whether the stack rests in
/// `UPDATE_FAILED`.
struct UpdateRow {
    case: &'static str,
    stack: &'static str,
    disable_rollback: bool,
}

const UPDATE_ROWS: &[UpdateRow] = &[
    UpdateRow {
        case: "a failed update CloudFormation rolled back",
        stack: "superseded-update-rolled-back",
        disable_rollback: false,
    },
    UpdateRow {
        case: "a failed update whose request disabled rollback",
        stack: "superseded-update-kept",
        disable_rollback: true,
    },
];

/// The same rule on an UPDATE: the resource the change could not be applied to carries the
/// `UPDATE_FAILED` event, and the stack carries one only where it rests in that status.
#[tokio::test]
async fn a_rolled_back_update_reports_no_stack_level_failure_event() {
    for row in UPDATE_ROWS {
        let router = router();
        let created = template_with_a_valid_role_mapping(row.stack);
        let (code, body) = create_stack(&router, row.stack, &created, &[]).await;
        assert_eq!(
            code, 200,
            "case {:?}: CreateStack must answer 200, got {code} {body}",
            row.case
        );
        wait_for_the_transition().await;
        let (_, described) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the create must complete before the update, got {described}",
            row.case
        );

        let mut members = vec![
            member("StackName", row.stack),
            member(
                "TemplateBody",
                &template_with_an_invalid_role_mapping(row.stack),
            ),
        ];
        if row.disable_rollback {
            members.push(member("DisableRollback", "true"));
        }
        let (code, body) = call(&router, "UpdateStack", &members).await;
        assert_eq!(
            code, 200,
            "case {:?}: UpdateStack must be accepted — the stack reports the outcome — got \
             {code} {body}",
            row.case
        );
        wait_for_the_transition().await;
        wait_for_the_transition().await;

        let mut expected = vec![(FAILING.to_string(), FAILING_TYPE.to_string())];
        if row.disable_rollback {
            expected.push((row.stack.to_string(), STACK_TYPE.to_string()));
        }
        expected.sort();
        let reported = failed_events(&router, row.case, row.stack).await;
        assert_eq!(
            reported, expected,
            "case {:?}: expected exactly {expected:?} to carry a failed status, got \
             {reported:?}",
            row.case
        );

        let statuses = stack_statuses(&router, row.case, row.stack).await;
        let rested = statuses.contains(&status::UPDATE_FAILED.to_string());
        assert_eq!(
            rested, row.disable_rollback,
            "case {:?}: expected the stack's own {} event exactly where the failed state is \
             kept, got {statuses:?}",
            row.case,
            status::UPDATE_FAILED
        );
        if !row.disable_rollback {
            assert!(
                statuses.contains(&status::UPDATE_ROLLBACK_IN_PROGRESS.to_string()),
                "case {:?}: expected the rollback's own event, got {statuses:?}",
                row.case
            );
        }
    }
}
