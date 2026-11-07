// SPDX-License-Identifier: CC0-1.0

//! The VALIDATION PATH an early-validation event reports, end to end over the awsQuery wire.
//!
//! RULE: `OperationEvent$ValidationPath` is "the path within the resource where the validation
//! was applied". A pre-deployment validation CloudFormation runs over a submitted template is
//! applied to one resource of it, and a template declares its resources under `Resources`
//! keyed by their logical id — so the path is that element's: `/Resources/<logical id>`.
//!
//! The rows differ along what the rule generalises over: WHICH logical id the failing resource
//! is declared under and which resource TYPE it is, each read through the same event.

use crate::change_set_lifecycle::{create_change_set, settle_the_store};
use crate::harness::{call, member, router_with_engine, xml_blocks, xml_text};
use crate::stack_resource_provisioning::s3;
use smithy_server_runtime::MultiServiceRouter;

/// One row: a resource whose name something already holds, declared under this logical id.
struct Row {
    case: &'static str,
    stack: &'static str,
    change_set: &'static str,
    logical_id: &'static str,
    bucket: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a bucket declared under the logical id Bucket",
            stack: "validation-path-bucket",
            change_set: "cs-validation-path-bucket",
            logical_id: "Bucket",
            bucket: "validation-path-taken-one",
        },
        Row {
            case: "another bucket, under a different logical id",
            stack: "validation-path-store",
            change_set: "cs-validation-path-store",
            logical_id: "ObjectStore",
            bucket: "validation-path-taken-two",
        },
    ]
}

/// Create one bucket as a client's own `CreateBucket` does, so the name is taken.
async fn put_bucket(router: &MultiServiceRouter, name: &str) {
    let created = s3(router, "PUT", &format!("/{name}"), b"").await;
    assert_eq!(
        created.status, 200,
        "the bucket that takes the name must be created first, got {} {}",
        created.status, created.body
    );
}

#[tokio::test]
async fn a_validation_event_names_the_template_element_it_was_applied_to() {
    let (router, engine) = router_with_engine();
    for row in rows() {
        let case = row.case;
        put_bucket(&router, row.bucket).await;
        let template = format!(
            r#"{{"Resources":{{"{}":{{"Type":"AWS::S3::Bucket",
                "Properties":{{"BucketName":"{}"}}}}}}}}"#,
            row.logical_id, row.bucket
        );
        let (code, body) = create_change_set(
            &router,
            row.stack,
            row.change_set,
            &[
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", &template),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the change set is created and then validated, got {code} {body}"
        );
        settle_the_store(&router, &engine).await;

        let (code, body) = call(
            &router,
            "DescribeEvents",
            &[
                member("StackName", row.stack),
                member("ChangeSetName", row.change_set),
            ],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the operation's events must be readable, got {code} {body}"
        );
        let validation: Vec<(String, String, String)> = xml_blocks(&body, "member")
            .iter()
            .filter(|event| xml_text(event, "ValidationName").is_some())
            .map(|event| {
                (
                    xml_text(event, "EventType").unwrap_or_default(),
                    xml_text(event, "LogicalResourceId").unwrap_or_default(),
                    xml_text(event, "ValidationPath").unwrap_or_default(),
                )
            })
            .collect();
        let expected = vec![(
            "VALIDATION_ERROR".to_string(),
            row.logical_id.to_string(),
            format!("/Resources/{}", row.logical_id),
        )];
        assert_eq!(
            validation, expected,
            "case {case:?}: expected the path of the element the validation ran on, got \
             {validation:?}"
        );
    }
}
