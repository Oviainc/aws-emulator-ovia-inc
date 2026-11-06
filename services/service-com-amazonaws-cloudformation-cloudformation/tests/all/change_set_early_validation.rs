// SPDX-License-Identifier: CC0-1.0

//! The PRE-DEPLOYMENT validation a change set is held to while it is being created, end to
//! end over the awsQuery wire.
//!
//! RULE: CloudFormation validates a change set BEFORE offering it for execution — a resource
//! the plan would CREATE under a physical name the service that owns it already holds cannot
//! be created, so the change set itself fails: it settles `Status` `FAILED` with
//! `ExecutionStatus` `UNAVAILABLE` and a `StatusReason` naming the validation that failed,
//! the stack it was created for keeps the status it had, and nothing is provisioned.
//! `DisableValidation` turns that stage off, which defers exactly the same collision to
//! execution time.
//!
//! The rows differ along what the rule generalises over: whether the colliding name is the
//! one the template SPELLS or one CloudFormation generates, WHICH resource type owns it (an
//! `AWS::S3::Bucket` and an `AWS::SNS::Topic`, each read back through its own service), who
//! holds the name already (a client's own call and another stack), what the plan does with
//! the named resource (adds it, or keeps one the stack already owns), and whether the request
//! turned the stage off.

use smithy_server_runtime::MultiServiceRouter;

use crate::change_set_lifecycle::{
    create_change_set, describe_change_set, execute_change_set, settle_the_store,
};
use crate::harness::{call, create_stack, member, router_with_engine};
use crate::stack_lifecycle::{describe_stack, sns, xml_blocks, xml_text};
use crate::stack_resource_provisioning::s3;
use service_com_amazonaws_cloudformation_cloudformation::state::{
    change_set_status, execution_status, status,
};

/// The resource-level status the event about a resource whose identifier is already taken
/// carries. Source: `crate::state::early_validation::NAME_CONFLICT_VALIDATION`.
const NAME_CONFLICT_VALIDATION: &str = "NAME_CONFLICT_VALIDATION";

/// The ARN Amazon SNS identifies the taken topic by, which is what the create this
/// validation stands in for would have produced from the name the template states.
const TAKEN_TOPIC_ARN: &str = "arn:aws:sns:us-east-1:000000000000:early-validation-taken-topic";

/// The `StatusReason` a change set whose early validation failed carries. Source: the
/// validation CloudFormation names for the existence check,
/// `crate::state::early_validation::RESOURCE_EXISTENCE_CHECK`, in the sentence that module
/// owns.
const EXISTENCE_CHECK_FAILED: &str =
    "The following hook(s)/validation failed: \
     [AWS::EarlyValidation::ResourceExistenceCheck]. To troubleshoot Early Validation errors, \
     use the DescribeEvents API for detailed failure information.";

/// A template declaring one bucket, named or left to CloudFormation's own naming.
fn bucket_template(name: Option<&str>) -> String {
    let properties = match name {
        Some(name) => format!(r#"{{"BucketName":"{name}"}}"#),
        None => "{}".to_string(),
    };
    format!(r#"{{"Resources":{{"Data":{{"Type":"AWS::S3::Bucket","Properties":{properties}}}}}}}"#)
}

/// A template declaring one topic under the name it is given.
fn topic_template(name: &str) -> String {
    format!(
        r#"{{"Resources":{{"Events":{{"Type":"AWS::SNS::Topic",
            "Properties":{{"TopicName":"{name}"}}}}}}}}"#
    )
}

/// Create one bucket as a client's own `CreateBucket` does, so the name is taken by
/// something no stack owns.
async fn put_bucket(router: &MultiServiceRouter, name: &str) {
    let created = s3(router, "PUT", &format!("/{name}"), b"").await;
    assert_eq!(
        created.status, 200,
        "the bucket that takes the name must be created first, got {} {}",
        created.status, created.body
    );
}

/// Create one topic through Amazon SNS's own wire, for the same reason.
async fn create_topic(router: &MultiServiceRouter, name: &str) {
    let (code, body) = sns(router, "CreateTopic", &[("Name", name)]).await;
    assert_eq!(
        code, 200,
        "the topic that takes the name must be created first, got {code} {body}"
    );
}

/// What a row expects of the change set its template is planned into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    /// The validation failed the change set, reporting this resource's identifier as taken:
    /// its logical id, its type and the identifier the owning service already holds.
    Failed {
        logical_id: &'static str,
        resource_type: &'static str,
        identifier: &'static str,
    },
    /// The plan is offered for execution.
    Available,
}

/// One row: a template, a name somebody already holds, and what the change set must settle
/// as.
struct Row {
    case: &'static str,
    stack: &'static str,
    change_set: &'static str,
    template: String,
    /// Whether the request turns the validation stage off.
    disable_validation: bool,
    expected: Expected,
}

#[tokio::test]
async fn early_validation_fails_a_change_set_that_would_create_a_resource_that_exists() {
    let (router, engine) = router_with_engine();
    put_bucket(&router, "early-validation-taken").await;
    create_topic(&router, "early-validation-taken-topic").await;

    let rows: Vec<Row> = vec![
        Row {
            case: "a bucket name a client's own call already holds",
            stack: "early-validation-bucket",
            change_set: "cs-bucket",
            template: bucket_template(Some("early-validation-taken")),
            disable_validation: false,
            expected: Expected::Failed {
                logical_id: "Data",
                resource_type: "AWS::S3::Bucket",
                // A bucket is identified by its name.
                identifier: "early-validation-taken",
            },
        },
        Row {
            case: "a topic name Amazon SNS already holds",
            stack: "early-validation-topic",
            change_set: "cs-topic",
            template: topic_template("early-validation-taken-topic"),
            disable_validation: false,
            expected: Expected::Failed {
                logical_id: "Events",
                resource_type: "AWS::SNS::Topic",
                // ...while a topic is identified by its ARN, which is what the create would
                // have produced from the name the template states.
                identifier: TAKEN_TOPIC_ARN,
            },
        },
        Row {
            case: "the same colliding bucket name with the validation turned off",
            stack: "early-validation-disabled",
            change_set: "cs-disabled",
            template: bucket_template(Some("early-validation-taken")),
            disable_validation: true,
            expected: Expected::Available,
        },
        Row {
            case: "a bucket name nothing holds",
            stack: "early-validation-free",
            change_set: "cs-free",
            template: bucket_template(Some("early-validation-free-name")),
            disable_validation: false,
            expected: Expected::Available,
        },
        Row {
            case: "a bucket whose name CloudFormation generates",
            stack: "early-validation-generated",
            change_set: "cs-generated",
            template: bucket_template(None),
            disable_validation: false,
            expected: Expected::Available,
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut members = vec![
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", &row.template),
        ];
        if row.disable_validation {
            members.push(member("DisableValidation", "true"));
        }
        let (code, body) =
            create_change_set(&router, row.stack, row.change_set, &members).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set is created and then validated, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;
        let (code, body) = describe_change_set(&router, row.change_set, Some(row.stack)).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set must be readable, got {code} {body}"
        );
        let settled = (
            xml_text(&body, "Status").unwrap_or_default(),
            xml_text(&body, "ExecutionStatus").unwrap_or_default(),
            xml_text(&body, "StatusReason"),
        );
        let expected = match row.expected {
            Expected::Failed { .. } => (
                change_set_status::FAILED.to_string(),
                execution_status::UNAVAILABLE.to_string(),
                Some(EXISTENCE_CHECK_FAILED.to_string()),
            ),
            Expected::Available => (
                change_set_status::CREATE_COMPLETE.to_string(),
                execution_status::AVAILABLE.to_string(),
                None,
            ),
        };
        assert_eq!(
            settled, expected,
            "case {case:?}: expected {expected:?}, got {settled:?}"
        );

        // "To troubleshoot Early Validation errors, use the DescribeEvents API": that call
        // carries the DETAIL of a failed validation — which resource, which validation, and
        // which identifier of which type is already there — and carries no such event where
        // the validation passed.
        let (code, body) = call(
            &router,
            "DescribeEvents",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the operation's events must be readable, got {code} {body}"
        );
        let validation_events: Vec<(String, String, String, String)> = xml_blocks(&body, "member")
            .iter()
            .filter(|event| xml_text(event, "ValidationName").is_some())
            .map(|event| {
                (
                    xml_text(event, "LogicalResourceId").unwrap_or_default(),
                    xml_text(event, "EventType").unwrap_or_default(),
                    xml_text(event, "ValidationName").unwrap_or_default(),
                    xml_text(event, "ValidationStatusReason").unwrap_or_default(),
                )
            })
            .collect();
        let expected_events = match row.expected {
            Expected::Failed {
                logical_id,
                resource_type,
                identifier,
            } => vec![(
                logical_id.to_string(),
                "VALIDATION_ERROR".to_string(),
                NAME_CONFLICT_VALIDATION.to_string(),
                format!(
                    "Resource of type '{resource_type}' with identifier '{identifier}' already \
                     exists."
                ),
            )],
            Expected::Available => Vec::new(),
        };
        assert_eq!(
            validation_events, expected_events,
            "case {case:?}: expected the validation events {expected_events:?}, got \
             {validation_events:?}"
        );

        // ...and that event is not part of the resource-transition stream: nothing was done
        // to the resource, so it has no status there.
        let (code, body) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the stack's events must be readable, got {code} {body}"
        );
        assert!(
            !body.contains(NAME_CONFLICT_VALIDATION),
            "case {case:?}: expected no validation event among the stack's resource \
             transitions, got {body}"
        );

        // Either way the stack the change set was created for is still in review with
        // nothing provisioned: a change set changes nothing until it is executed.
        let (code, body) = describe_stack(&router, row.stack).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set's stack must be readable, got {code} {body}"
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::REVIEW_IN_PROGRESS),
            "case {case:?}: expected the stack to stay in review, got {body}"
        );
    }

    // The failed change set cannot be executed, and the colliding bucket still belongs to
    // whoever held it: the validation deployed nothing.
    let (code, body) = execute_change_set(&router, "cs-bucket", Some("early-validation-bucket"))
        .await;
    assert_eq!(
        code, 400,
        "case \"executing a failed change set\": expected a refusal, got {code} {body}"
    );

    // ...while the change set created with the validation turned off IS offered for
    // execution: turning the stage off is what defers the collision past the plan.
    let (code, body) = execute_change_set(
        &router,
        "cs-disabled",
        Some("early-validation-disabled"),
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"the deferred collision\": the unvalidated plan is executable, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
}

/// A change set over a stack that ALREADY owns the named resource is not a collision: the
/// name is held because the stack holds it.
#[tokio::test]
async fn a_change_set_that_keeps_a_resource_the_stack_owns_passes_validation() {
    let (router, engine) = router_with_engine();
    let stack = "early-validation-owned";
    let template = bucket_template(Some("early-validation-owned-data"));
    let (code, body) = create_stack(&router, stack, &template, &[]).await;
    assert_eq!(
        code, 200,
        "the stack that owns the name must be created first, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;

    // An UPDATE change set restating the same bucket with a new property keeps it, so the
    // plan creates nothing under a name anything holds.
    let revised = r#"{"Resources":{"Data":{"Type":"AWS::S3::Bucket","Properties":{
        "BucketName":"early-validation-owned-data",
        "VersioningConfiguration":{"Status":"Enabled"}}}}}"#;
    let (code, body) = create_change_set(
        &router,
        stack,
        "cs-owned",
        &[member("TemplateBody", revised)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"a modification of the stack's own bucket\": got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (code, body) = describe_change_set(&router, "cs-owned", Some(stack)).await;
    assert_eq!(
        code, 200,
        "case \"a modification of the stack's own bucket\": the change set must be readable, \
         got {code} {body}"
    );
    assert_eq!(
        (
            xml_text(&body, "Status").unwrap_or_default(),
            xml_text(&body, "ExecutionStatus").unwrap_or_default(),
            xml_text(&body, "StatusReason"),
        ),
        (
            change_set_status::CREATE_COMPLETE.to_string(),
            execution_status::AVAILABLE.to_string(),
            None,
        ),
        "case \"a modification of the stack's own bucket\": expected the plan to be offered, \
         got {body}"
    );

    // ...while a SECOND stack asking for the same name is a collision the validation fails.
    let (code, body) = create_change_set(
        &router,
        "early-validation-second",
        "cs-second",
        &[
            member("ChangeSetType", "CREATE"),
            member("TemplateBody", &template),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"a second stack asking for the name\": got {code} {body}"
    );
    settle_the_store(&router, &engine).await;
    let (code, body) = describe_change_set(&router, "cs-second", Some("early-validation-second"))
        .await;
    assert_eq!(
        code, 200,
        "case \"a second stack asking for the name\": the change set must be readable, got \
         {code} {body}"
    );
    assert_eq!(
        (
            xml_text(&body, "Status").unwrap_or_default(),
            xml_text(&body, "ExecutionStatus").unwrap_or_default(),
            xml_text(&body, "StatusReason"),
        ),
        (
            change_set_status::FAILED.to_string(),
            execution_status::UNAVAILABLE.to_string(),
            Some(EXISTENCE_CHECK_FAILED.to_string()),
        ),
        "case \"a second stack asking for the name\": expected the validation to fail it, got \
         {body}"
    );
}
