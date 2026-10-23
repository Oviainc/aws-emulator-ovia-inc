// SPDX-License-Identifier: CC0-1.0

//! The service-role check every stack operation that takes a `RoleARN` makes, across the
//! two services it spans.
//!
//! RULE: a stack operation acts as the `RoleARN` the request names
//! (`CreateStackInput$RoleARN`: "The Amazon Resource Name (ARN) of an IAM role that
//! CloudFormation assumes to create the stack. CloudFormation uses the role's credentials
//! to make calls on your behalf"), so it resolves that role against the account's IAM
//! records and refuses the call — writing nothing — with HTTP 400, the wire code
//! `ValidationError` and a message naming the submitted ARN, when the role's trust policy
//! withholds `sts:AssumeRole` from `cloudformation.amazonaws.com`.
//!
//! The role is resolved against the records the REAL `CreateRole` wrote, on the same engine
//! that serves CloudFormation, so what the check reads is the document IAM persisted and
//! not a copy. The rows differ along what the rule generalises over:
//!
//! * the form of the trust policy — which service principal it names, whether it names
//!   several, whether the principal and the action are strings or lists, which statement
//!   carries the grant, the effect, whether an explicit `Deny` stands beside an `Allow`,
//!   which action is named, which wildcards stand in for it, the casing, and whether the
//!   principal is a service at all;
//! * the OPERATION the role is handed to (`CreateStack`, `UpdateStack`, `DeleteStack`,
//!   `CreateChangeSet`) — every one of them refuses it the same way;
//! * whether the IAM store holds the role at all — an ARN it holds no role for is no
//!   evidence of a withheld trust, so the call goes ahead.
//!
//! The issue's own case (a role whose only `Principal.Service` is `lambda.amazonaws.com`)
//! is one row of the first table. Its reproduction declared one `AWS::SQS::Queue`; the
//! templates here declare resource types this emulator leaves declared-only instead,
//! because the role is resolved BEFORE anything is written or deployed — so what the
//! template declares is immaterial to the rule — while a type this emulator DOES provision
//! is created in its owning service, which a router carrying only CloudFormation and IAM
//! does not mount (that path is
//! `core/router/tests/all/cloudformation_provisions_sqs_queues.rs`).

use std::sync::Arc;

use service_com_amazonaws_cloudformation_cloudformation::state::{status, SERVICE_PRINCIPAL};
use service_com_amazonaws_cloudformation_cloudformation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{call, iam, member, wait_for_the_transition, xml_text};

/// The template the stacks here are created from: one `AWS::CloudFormation::WaitConditionHandle`.
///
/// The type is CloudFormation's OWN, so no other service owns its resources and nothing is
/// created outside this emulator's CloudFormation store — which is what lets a stack
/// complete on a router carrying CloudFormation and IAM alone. A type whose resources
/// another service owns is created in that service
/// (`core/router/tests/all/cloudformation_provisions_sqs_queues.rs` drives that path).
const STACK_TEMPLATE: &str =
    r#"{"Resources":{"TheHandle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// A second template, for the update that has to change something about the stack: the same
/// CloudFormation-owned type under another logical id.
const UPDATED_TEMPLATE: &str =
    r#"{"Resources":{"TheOtherHandle":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// One emulator carrying BOTH services over ONE shared engine, exactly as the production
/// router builds them: the IAM store CloudFormation resolves a `RoleARN` against is the one
/// `CreateRole` writes.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(EmulatedService::new(engine));
    router
}

/// Create a role carrying `document` as its trust policy, answered with the ARN IAM
/// persisted for it.
async fn create_role(router: &MultiServiceRouter, name: &str, document: &str) -> String {
    let (status, created) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected CreateRole to persist the role, got {status}: {created}"
    );
    xml_text(&created, "Arn")
        .unwrap_or_else(|| panic!("case {name:?}: CreateRole reported no Arn: {created}"))
}

/// A trust policy with one statement, written as a client sends it.
fn policy(effect: &str, action: &str, principal: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{principal}}},\"Action\":{action}}}]}}"
    )
}

/// Assert a response is the service's refusal of `role_arn`: HTTP 400, the wire code
/// `ValidationError`, and a message built from the ARN the request submitted.
fn assert_refused(case: &str, status: u16, body: &str, role_arn: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: a role CloudFormation cannot assume must be refused, got {status}: {body}"
    );
    let code = xml_text(body, "Code");
    assert_eq!(
        code.as_deref(),
        Some("ValidationError"),
        "case {case:?}: expected the wire code ValidationError, got {code:?}: {body}"
    );
    let message = xml_text(body, "Message").unwrap_or_default();
    assert_eq!(
        message,
        format!("Role {role_arn} is invalid or cannot be assumed"),
        "case {case:?}: the refusal is built from the request's own RoleARN, got {message:?}"
    );
}

/// What a `DescribeStacks` read reports for `name`: the status and the `RoleARN`, or `None`
/// when the service answers that the stack does not exist.
///
/// The absent case is held to the service's own not-found refusal — HTTP 400
/// `ValidationError` saying the stack does not exist — so a 400 for any OTHER reason fails
/// the read instead of being read as "there is no such stack".
async fn described(router: &MultiServiceRouter, name: &str) -> Option<(String, Option<String>)> {
    let (status, body) = call(router, "DescribeStacks", &[member("StackName", name)]).await;
    if status == 400 {
        let code = xml_text(&body, "Code");
        assert_eq!(
            code.as_deref(),
            Some("ValidationError"),
            "stack {name:?}: expected the not-found refusal, got code {code:?}: {body}"
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        assert!(
            message.contains("does not exist"),
            "stack {name:?}: expected a does-not-exist message, got {message:?}"
        );
        return None;
    }
    assert_eq!(
        status, 200,
        "expected DescribeStacks to answer for {name:?}, got {status}: {body}"
    );
    Some((
        xml_text(&body, "StackStatus").unwrap_or_default(),
        xml_text(&body, "RoleARN"),
    ))
}

/// One row of the trust-policy table: the document a role carries, and whether a stack
/// operation handed that role is accepted.
struct Row {
    case: &'static str,
    document: String,
    accepted: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "cloudformation alone",
            document: policy("Allow", "\"sts:AssumeRole\"", "\"cloudformation.amazonaws.com\""),
            accepted: true,
        },
        Row {
            case: "cloudformation among several services",
            document: policy(
                "Allow",
                "[\"sts:TagSession\",\"sts:AssumeRole\"]",
                "[\"lambda.amazonaws.com\",\"cloudformation.amazonaws.com\"]",
            ),
            accepted: true,
        },
        Row {
            case: "the grant carried by the second statement",
            document: "{\"Version\":\"2012-10-17\",\"Statement\":[\
                       {\"Effect\":\"Allow\",\"Principal\":{\"Service\":\"lambda.amazonaws.com\"},\
                       \"Action\":\"sts:AssumeRole\"},\
                       {\"Effect\":\"Allow\",\
                       \"Principal\":{\"Service\":\"cloudformation.amazonaws.com\"},\
                       \"Action\":\"sts:AssumeRole\"}]}"
                .to_string(),
            accepted: true,
        },
        Row {
            case: "one statement written as an object rather than a list",
            document: "{\"Version\":\"2012-10-17\",\"Statement\":\
                       {\"Effect\":\"Allow\",\
                       \"Principal\":{\"Service\":\"cloudformation.amazonaws.com\"},\
                       \"Action\":\"sts:AssumeRole\"}}"
                .to_string(),
            accepted: true,
        },
        Row {
            case: "the action named through an sts wildcard",
            document: policy("Allow", "\"sts:*\"", "\"cloudformation.amazonaws.com\""),
            accepted: true,
        },
        Row {
            case: "every action",
            document: policy("Allow", "\"*\"", "\"cloudformation.amazonaws.com\""),
            accepted: true,
        },
        Row {
            // IAM compares a principal and an action case-insensitively.
            case: "the action and the principal in another casing",
            document: policy("Allow", "\"STS:ASSUMEROLE\"", "\"CloudFormation.amazonaws.com\""),
            accepted: true,
        },
        Row {
            // The issue's own case.
            case: "lambda only",
            document: policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
            accepted: false,
        },
        Row {
            case: "ec2 only",
            document: policy("Allow", "\"sts:AssumeRole\"", "\"ec2.amazonaws.com\""),
            accepted: false,
        },
        Row {
            case: "cloudformation explicitly denied",
            document: policy("Deny", "\"sts:AssumeRole\"", "\"cloudformation.amazonaws.com\""),
            accepted: false,
        },
        Row {
            // IAM's deny precedence: an explicit `Deny` wins over any `Allow`.
            case: "cloudformation both allowed and denied",
            document: "{\"Version\":\"2012-10-17\",\"Statement\":[\
                       {\"Effect\":\"Allow\",\
                       \"Principal\":{\"Service\":\"cloudformation.amazonaws.com\"},\
                       \"Action\":\"sts:AssumeRole\"},\
                       {\"Effect\":\"Deny\",\
                       \"Principal\":{\"Service\":\"cloudformation.amazonaws.com\"},\
                       \"Action\":\"sts:AssumeRole\"}]}"
                .to_string(),
            accepted: false,
        },
        Row {
            // A trust policy governs `sts:AssumeRole`; a statement naming some other
            // action grants nothing here.
            case: "cloudformation allowed an action that is not assuming the role",
            document: policy("Allow", "\"s3:GetObject\"", "\"cloudformation.amazonaws.com\""),
            accepted: false,
        },
        Row {
            case: "an account principal rather than a service one",
            document: "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                       \"Principal\":{\"AWS\":\"arn:aws:iam::000000000000:root\"},\
                       \"Action\":\"sts:AssumeRole\"}]}"
                .to_string(),
            accepted: false,
        },
    ]
}

#[tokio::test]
async fn create_stack_refuses_a_role_whose_trust_policy_withholds_cloudformation() {
    let router = emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let role_arn = create_role(&router, &format!("cfn-trust-{index}"), &row.document).await;
        let stack_name = format!("trust-create-{index}");

        let (status, body) = call(
            &router,
            "CreateStack",
            &[
                member("StackName", &stack_name),
                member("TemplateBody", STACK_TEMPLATE),
                member("RoleARN", &role_arn),
            ],
        )
        .await;

        if row.accepted {
            assert_eq!(
                status, 200,
                "case {:?}: a role that grants {SERVICE_PRINCIPAL} must be accepted, \
                 got {status}: {body}",
                row.case
            );
            let stack_id = xml_text(&body, "StackId").unwrap_or_default();
            assert!(
                stack_id.contains(&format!(":stack/{stack_name}/")),
                "case {:?}: expected a stack id for that stack, got {stack_id:?}",
                row.case
            );
            // Round trip: the stack reads back through DescribeStacks carrying the role
            // it was created with.
            let reported = described(&router, &stack_name).await;
            assert_eq!(
                reported.as_ref().and_then(|(_, role)| role.as_deref()),
                Some(role_arn.as_str()),
                "case {:?}: the created stack must report its RoleARN, got {reported:?}",
                row.case
            );
        } else {
            assert_refused(row.case, status, &body, &role_arn);
            // A refused create writes NOTHING: no stack of that name exists.
            let reported = described(&router, &stack_name).await;
            assert!(
                reported.is_none(),
                "case {:?}: a refused create must leave no stack, got {reported:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn every_stack_operation_taking_a_role_arn_refuses_one_cloudformation_cannot_assume() {
    let router = emulator();
    let bad_role = create_role(
        &router,
        "cfn-ops-lambda-only",
        &policy("Allow", "\"sts:AssumeRole\"", "\"lambda.amazonaws.com\""),
    )
    .await;
    let good_role = create_role(
        &router,
        "cfn-ops-trusted",
        &policy("Allow", "\"sts:AssumeRole\"", "\"cloudformation.amazonaws.com\""),
    )
    .await;

    let stack_name = "trust-operations";
    let (status, created) = call(
        &router,
        "CreateStack",
        &[
            member("StackName", stack_name),
            member("TemplateBody", STACK_TEMPLATE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the baseline stack to be created, got {status}: {created}"
    );
    wait_for_the_transition().await;
    // What the baseline stack reports once its create has settled: the state every refused
    // call below has to leave exactly as it found it. Snapshotted rather than spelled out,
    // so what is asserted is "the refusal wrote nothing" rather than one particular settled
    // status.
    let baseline = described(&router, stack_name).await;
    assert_eq!(
        baseline,
        Some((status::CREATE_COMPLETE.to_string(), None)),
        "expected the baseline stack to settle with no role of its own, got {baseline:?}"
    );

    // Every operation that takes a `RoleARN`, each handed the role CloudFormation cannot
    // assume: refused the same way, and leaving the stack exactly as it was.
    let refusals: [(&str, Vec<(String, String)>); 4] = [
        (
            "CreateStack",
            vec![
                member("StackName", "trust-operations-second"),
                member("TemplateBody", STACK_TEMPLATE),
                member("RoleARN", &bad_role),
            ],
        ),
        (
            "UpdateStack",
            vec![
                member("StackName", stack_name),
                member("TemplateBody", UPDATED_TEMPLATE),
                member("RoleARN", &bad_role),
            ],
        ),
        (
            "CreateChangeSet",
            vec![
                member("StackName", stack_name),
                member("ChangeSetName", "trust-change-set"),
                member("TemplateBody", UPDATED_TEMPLATE),
                member("RoleARN", &bad_role),
            ],
        ),
        (
            "DeleteStack",
            vec![
                member("StackName", stack_name),
                member("RoleARN", &bad_role),
            ],
        ),
    ];

    for (action, members) in &refusals {
        let (status, body) = call(&router, action, members).await;
        assert_refused(action, status, &body, &bad_role);

        let reported = described(&router, stack_name).await;
        assert_eq!(
            reported, baseline,
            "case {action:?}: a refused call must leave the stack untouched, got {reported:?}"
        );
    }

    // The positive control, operation by operation: the SAME calls with a role whose trust
    // names cloudformation.amazonaws.com are accepted.
    let (status, body) = call(
        &router,
        "CreateChangeSet",
        &[
            member("StackName", stack_name),
            member("ChangeSetName", "trust-change-set"),
            member("TemplateBody", UPDATED_TEMPLATE),
            member("RoleARN", &good_role),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateChangeSet with a trusted role must be accepted, got {status}: {body}"
    );

    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack_name),
            member("TemplateBody", UPDATED_TEMPLATE),
            member("RoleARN", &good_role),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateStack with a trusted role must be accepted, got {status}: {body}"
    );
    wait_for_the_transition().await;
    let reported = described(&router, stack_name).await;
    // What the rule is about: the accepted update put the trusted role on the stack, and the
    // stack went through an UPDATE rather than staying as the create left it. How that
    // update ENDED is the deployment's business, not this rule's, so the status is held only
    // to being one of the update's own.
    assert_eq!(
        reported.as_ref().and_then(|(_, role)| role.as_deref()),
        Some(good_role.as_str()),
        "the accepted update must put the trusted role on the stack, got {reported:?}"
    );
    let updated_status = reported
        .as_ref()
        .map(|(stack_status, _)| stack_status.as_str())
        .unwrap_or_default();
    assert!(
        updated_status.starts_with("UPDATE_"),
        "the accepted update must have run, got {updated_status:?}"
    );

    let (status, body) = call(
        &router,
        "DeleteStack",
        &[
            member("StackName", stack_name),
            member("RoleARN", &good_role),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteStack with a trusted role must be accepted, got {status}: {body}"
    );
    let reported = described(&router, stack_name).await;
    assert_eq!(
        reported.as_ref().map(|(stack_status, _)| stack_status.as_str()),
        Some(status::DELETE_IN_PROGRESS),
        "the accepted delete must start the deletion, got {reported:?}"
    );
}

/// RULE (the complement): a `RoleARN` the account's IAM store holds no role for is not
/// evidence that the trust is missing — the role may live in another account, or in an IAM
/// this emulator was never told about — so the operation goes ahead and the stack carries
/// the ARN the request named.
#[tokio::test]
async fn a_role_arn_the_iam_store_holds_no_role_for_is_accepted() {
    let router = emulator();

    // The rows differ along the shape of the unheld ARN: this account's own namespace, a
    // role with a path, another account, and another partition.
    let rows = [
        ("this account", "arn:aws:iam::000000000000:role/not-created"),
        (
            "a role with a path",
            "arn:aws:iam::000000000000:role/team/not-created",
        ),
        ("another account", "arn:aws:iam::123456789012:role/remote"),
        (
            "another partition",
            "arn:aws-us-gov:iam::000000000000:role/remote",
        ),
    ];

    for (index, (case, role_arn)) in rows.iter().enumerate() {
        let stack_name = format!("trust-unheld-{index}");
        let (status, body) = call(
            &router,
            "CreateStack",
            &[
                member("StackName", &stack_name),
                member("TemplateBody", STACK_TEMPLATE),
                member("RoleARN", role_arn),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to go ahead, got {status}: {body}"
        );
        let reported = described(&router, &stack_name).await;
        assert_eq!(
            reported.as_ref().and_then(|(_, role)| role.as_deref()),
            Some(*role_arn),
            "case {case:?}: the stack runs as the role the request named, got {reported:?}"
        );
    }
}
