// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::CodeBuild::ReportGroup` resources its template declares — in CodeBuild, with the
//! configuration the declaration carries.
//!
//! The rule under test: **a stack reports a resource as created only where the service
//! that owns it created it, through that service's own public API and with every declared
//! property applied, so the handle the stack publishes addresses a resource that service
//! really holds — and deleting the stack releases it there.** For a report group the
//! handle is its NAME ("Ref returns the report group name") and its `Arn` attribute, so
//! right after the create waiter returns `BatchGetReportGroups` answers for that ARN —
//! rather than CodeBuild reporting the ARN of its own completed stack in
//! `reportGroupsNotFound` — and `PutResourcePolicy` / `GetResourcePolicy` against the same
//! ARN have a resource to attach a policy to. Two crates have to agree for that to hold
//! (CloudFormation makes the calls, CodeBuild serves them), so the suite lives here: a
//! crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! group or leaves the name to CloudFormation, whether it is written in JSON or YAML,
//! which `Type` it declares, which export configuration it carries (no export, and an S3
//! destination with every member of that structure), whether it declares tags, and the
//! Region the stack is deployed in.
//!
//! The awsQuery form encoder and the XML element readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the deployment it completed. A report group has no transient
/// status of its own: `CreateReportGroup` answers with one that is `ACTIVE` at once.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The bounds CodeBuild holds a report group's name to (`ReportGroupName`,
/// `@length(min: 2, max: 128)` in that service's own model), which a GENERATED name is
/// held to as well.
const MIN_REPORT_GROUP_NAME: usize = 2;
const MAX_REPORT_GROUP_NAME: usize = 128;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them. Request authentication is off, as in every
/// other suite here; the in-process callback CloudFormation makes signs its request either
/// way (the credential scope is what routes it).
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`,
/// which is what routes the request and what each handler reads its Region from. The
/// signature is never checked: this router runs with request authentication off.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire, signed for `region`.
async fn cloudformation(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{region}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation", region))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one CodeBuild operation over its awsJson1_1 wire, signed for `region`.
async fn codebuild(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("codebuild.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CodeBuild_20161006.{operation}"))
        .header("authorization", authorization("codebuild", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The ARN a report group of `name` carries in `region`: the same total function of the
/// Region, the account and the name CodeBuild itself keys a report group by.
fn report_group_arn(region: &str, name: &str) -> String {
    format!("arn:aws:codebuild:{region}:{ACCOUNT}:report-group/{name}")
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The report group CodeBuild holds for `arn`, or `None` where it reports the ARN in
/// `reportGroupsNotFound` — the answer's own partition, read the way a client reads it.
async fn held_report_group(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
) -> (Option<Value>, Vec<String>) {
    let (status, answer) = codebuild(
        router,
        region,
        "BatchGetReportGroups",
        &json!({ "reportGroupArns": [arn] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "BatchGetReportGroups {arn:?}: expected HTTP 200, got {status} with body {answer}"
    );
    let held = answer
        .get("reportGroups")
        .and_then(Value::as_array)
        .and_then(|groups| groups.first())
        .cloned();
    let not_found = answer
        .get("reportGroupsNotFound")
        .and_then(Value::as_array)
        .map(|arns| {
            arns.iter()
                .filter_map(|arn| arn.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    (held, not_found)
}

/// One row of the table: a template, and what the rule says the report group it declares
/// must then be in CodeBuild.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the group is created in and the
    /// one its ARN carries.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: &'static str,
    /// The name the template declares, or `None` for a template that names nothing —
    /// whose group carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The `type` `BatchGetReportGroups` must report.
    expected_type: &'static str,
    /// The `exportConfig` it must report, built from what the row declares.
    expected_export_config: fn() -> Value,
    /// The tags the group's own declaration asks for.
    expected_tags: &'static [(&'static str, &'static str)],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named TEST group in JSON with no export and tags",
        region: "us-west-2",
        template: r#"{
            "AWSTemplateFormatVersion": "2010-09-09",
            "Description": "a CodeBuild report group",
            "Resources": {
                "Reports": {
                    "Type": "AWS::CodeBuild::ReportGroup",
                    "Properties": {
                        "Name": "unit-test-reports",
                        "Type": "TEST",
                        "ExportConfig": { "ExportConfigType": "NO_EXPORT" },
                        "Tags": [
                            { "Key": "Owner", "Value": "orders" },
                            { "Key": "Tier", "Value": "prod" }
                        ]
                    }
                }
            },
            "Outputs": {
                "GroupRef": { "Value": { "Ref": "Reports" } },
                "GroupArn": { "Value": { "Fn::GetAtt": ["Reports", "Arn"] } }
            }
        }"#,
        declared_name: Some("unit-test-reports"),
        expected_type: "TEST",
        expected_export_config: || json!({"exportConfigType": "NO_EXPORT"}),
        expected_tags: &[("Owner", "orders"), ("Tier", "prod")],
    },
    Row {
        case: "a named CODE_COVERAGE group in YAML exporting to S3, every destination \
               member declared",
        region: "us-east-1",
        template: "Resources:\n  \
                   Reports:\n    \
                   Type: AWS::CodeBuild::ReportGroup\n    \
                   Properties:\n      \
                   Name: coverage-reports\n      \
                   Type: CODE_COVERAGE\n      \
                   ExportConfig:\n        \
                   ExportConfigType: S3\n        \
                   S3Destination:\n          \
                   Bucket: coverage-raw-data\n          \
                   BucketOwner: '000000000000'\n          \
                   Path: coverage/runs\n          \
                   Packaging: ZIP\n          \
                   EncryptionKey: arn:aws:kms:us-east-1:000000000000:alias/aws/s3\n          \
                   EncryptionDisabled: false\n\
                   Outputs:\n  \
                   GroupRef:\n    \
                   Value: !Ref Reports\n  \
                   GroupArn:\n    \
                   Value: !GetAtt Reports.Arn\n",
        declared_name: Some("coverage-reports"),
        expected_type: "CODE_COVERAGE",
        expected_export_config: || {
            json!({
                "exportConfigType": "S3",
                "s3Destination": {
                    "bucket": "coverage-raw-data",
                    "bucketOwner": "000000000000",
                    "path": "coverage/runs",
                    "packaging": "ZIP",
                    "encryptionKey": "arn:aws:kms:us-east-1:000000000000:alias/aws/s3",
                    "encryptionDisabled": false,
                },
            })
        },
        expected_tags: &[],
    },
    Row {
        case: "a group the template does not name, whose reports go with it on delete",
        region: "eu-west-1",
        template: r#"{
            "Resources": {
                "Reports": {
                    "Type": "AWS::CodeBuild::ReportGroup",
                    "Properties": {
                        "Type": "TEST",
                        "ExportConfig": { "ExportConfigType": "NO_EXPORT" },
                        "DeleteReports": true,
                        "Tags": { "Owner": "platform" }
                    }
                }
            },
            "Outputs": {
                "GroupRef": { "Value": { "Ref": "Reports" } },
                "GroupArn": { "Value": { "Fn::GetAtt": ["Reports", "Arn"] } }
            }
        }"#,
        declared_name: None,
        expected_type: "TEST",
        expected_export_config: || json!({"exportConfigType": "NO_EXPORT"}),
        expected_tags: &[("Owner", "platform")],
    },
];

/// Every row: the name the stack reports for its `AWS::CodeBuild::ReportGroup` is a report
/// group CodeBuild really holds under the ARN the stack publishes, configured as the
/// template declared it; a resource policy can be attached to that ARN and read back; and
/// deleting the stack releases the group there.
#[tokio::test]
async fn a_stack_provisions_the_codebuild_report_group_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("codebuild-report-group-stack-{index}");

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", row.template),
                ("Tags.member.1.Key", "Stack"),
                ("Tags.member.1.Value", "codebuild-suite"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_id = opt_element(&resources, "PhysicalResourceId")
            .unwrap_or_else(|| panic!("case {case:?}: no <PhysicalResourceId> in {resources}"));

        // "Ref returns the report group name": the physical id IS the name the template
        // asked for, or the one CloudFormation generated for a template that named
        // nothing — which is held to the name CodeBuild accepts.
        if let Some(declared) = row.declared_name {
            assert_eq!(
                physical_id, declared,
                "case {case:?}: expected the physical id to be the declared name \
                 {declared:?}, got {physical_id:?}"
            );
        }
        let name_length = physical_id.chars().count();
        assert!(
            (MIN_REPORT_GROUP_NAME..=MAX_REPORT_GROUP_NAME).contains(&name_length),
            "case {case:?}: expected a name inside CodeBuild's own ReportGroupName rule, got \
             {physical_id:?}"
        );

        let arn = report_group_arn(region, &physical_id);
        let outputs = outputs_of(&described);
        assert_eq!(
            outputs.get("GroupRef"),
            Some(&physical_id),
            "case {case:?}: expected Ref to be the report group name {physical_id:?}, got {:?}",
            outputs.get("GroupRef")
        );
        assert_eq!(
            outputs.get("GroupArn"),
            Some(&arn),
            "case {case:?}: expected Fn::GetAtt Arn to be {arn}, got {:?}",
            outputs.get("GroupArn")
        );

        // And CodeBuild itself resolves that ARN to the report group the template
        // declared.
        let (held, not_found) = held_report_group(&router, region, &arn).await;
        assert!(
            not_found.is_empty(),
            "case {case:?}: expected the stack's own ARN to be found, got \
             reportGroupsNotFound {not_found:?}"
        );
        let held = held.unwrap_or_else(|| {
            panic!("case {case:?}: expected BatchGetReportGroups to report the group for {arn}")
        });
        assert_eq!(
            held.get("arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the group's ARN to be {arn}, got {held}"
        );
        assert_eq!(
            held.get("name").and_then(Value::as_str),
            Some(physical_id.as_str()),
            "case {case:?}: expected the group to carry the stack's name, got {held}"
        );
        assert_eq!(
            held.get("type").and_then(Value::as_str),
            Some(row.expected_type),
            "case {case:?}: expected the declared type, got {held}"
        );
        let expected_export_config = (row.expected_export_config)();
        assert_eq!(
            held.get("exportConfig"),
            Some(&expected_export_config),
            "case {case:?}: expected the declared export configuration \
             {expected_export_config}, got {held}"
        );
        assert_eq!(
            held.get("status").and_then(Value::as_str),
            Some("ACTIVE"),
            "case {case:?}: a created report group is usable at once, got {held}"
        );

        // The tags the declaration asked for are on the group, beside the stack's own and
        // the three CloudFormation records itself — CodeBuild's own `Tag` shape admits
        // that key namespace, so nothing is left off.
        let reported: BTreeMap<String, String> = held
            .get("tags")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no 'tags' list in {held}"))
            .iter()
            .filter_map(|tag| {
                Some((
                    tag.get("key")?.as_str()?.to_string(),
                    tag.get("value")?.as_str()?.to_string(),
                ))
            })
            .collect();
        for (key, value) in row.expected_tags {
            assert_eq!(
                reported.get(*key).map(String::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported:?}"
            );
        }
        assert_eq!(
            reported.get("Stack").map(String::as_str),
            Some("codebuild-suite"),
            "case {case:?}: expected the stack's own tag to be propagated, got {reported:?}"
        );
        assert_eq!(
            reported
                .get("aws:cloudformation:logical-id")
                .map(String::as_str),
            Some("Reports"),
            "case {case:?}: expected CloudFormation's own record of the logical id, got \
             {reported:?}"
        );

        // What the stack provisioned is addressable as a resource of CodeBuild's own: a
        // resource policy attaches to that ARN and reads back verbatim.
        let policy = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::222222222222:root"},
                "Action": ["codebuild:BatchGetReportGroups"],
                "Resource": arn,
            }],
        })
        .to_string();
        let (status, stored) = codebuild(
            &router,
            region,
            "PutResourcePolicy",
            &json!({ "resourceArn": arn, "policy": policy }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from PutResourcePolicy on the stack's group, got \
             {status} with body {stored}"
        );
        let (status, read) = codebuild(
            &router,
            region,
            "GetResourcePolicy",
            &json!({ "resourceArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetResourcePolicy, got {status} with body \
             {read}"
        );
        assert_eq!(
            read.get("policy").and_then(Value::as_str),
            Some(policy.as_str()),
            "case {case:?}: expected the stored policy document back, got {read}"
        );

        // Deleting the stack releases the report group CodeBuild holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (held, not_found) = held_report_group(&router, region, &arn).await;
        assert!(
            held.is_none() && not_found == vec![arn.clone()],
            "case {case:?}: expected the group to be gone once the stack deleted it, got \
             {held:?} with reportGroupsNotFound {not_found:?}"
        );
    }
}

/// A declaration CodeBuild REFUSES does not report a resource nothing created: the stack
/// fails with that service's own refusal, and the report group the declaration named is
/// not in CodeBuild afterwards.
///
/// The rows differ along WHAT is refused: a name a client already holds, an S3 export that
/// names no bucket, and a report type the model does not declare.
#[tokio::test]
async fn a_report_group_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let region = "us-east-2";

    // The report group a client already holds, which the first row re-declares.
    let (status, created) = codebuild(
        &router,
        region,
        "CreateReportGroup",
        &json!({
            "name": "already-there",
            "type": "TEST",
            "exportConfig": {"exportConfigType": "NO_EXPORT"},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the client's own CreateReportGroup to succeed, got {status} with body {created}"
    );

    // (case, the resource's declared properties, the name the declaration asks for, and
    // whether a group of that name must still be held once the stack has failed — true
    // only where a client created it)
    let rows: Vec<(&str, Value, &str, bool)> = vec![
        (
            "a report group name a client already holds",
            json!({
                "Name": "already-there",
                "Type": "TEST",
                "ExportConfig": {"ExportConfigType": "NO_EXPORT"},
            }),
            "already-there",
            true,
        ),
        (
            "an S3 export configuration that names no bucket",
            json!({
                "Name": "exports-nowhere",
                "Type": "TEST",
                "ExportConfig": {"ExportConfigType": "S3", "S3Destination": {"Path": "runs"}},
            }),
            "exports-nowhere",
            false,
        ),
        (
            "a report type the model does not declare",
            json!({
                "Name": "not-a-report-type",
                "Type": "SPREADSHEET",
                "ExportConfig": {"ExportConfigType": "NO_EXPORT"},
            }),
            "not-a-report-type",
            false,
        ),
    ];

    for (index, (case, properties, name, held_by_a_client)) in rows.into_iter().enumerate() {
        let stack_name = format!("codebuild-report-group-refusal-{index}");
        let template = json!({
            "Resources": { "Reports": {
                "Type": "AWS::CodeBuild::ReportGroup",
                "Properties": properties,
            }}
        })
        .to_string();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", &stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        let stack_status = opt_element(&described, "StackStatus").unwrap_or_default();
        assert!(
            stack_status.contains("ROLLBACK") || stack_status.contains("FAILED"),
            "case {case:?}: expected the stack not to report success, got {stack_status:?} in \
             {described}"
        );

        // Nothing the refused declaration asked for is in CodeBuild — bar the group a
        // client created, which the stack never owned.
        let arn = report_group_arn(region, name);
        let (held, not_found) = held_report_group(&router, region, &arn).await;
        assert_eq!(
            held.is_some(),
            held_by_a_client,
            "case {case:?}: expected the group present={held_by_a_client} after the failed \
             stack, got {held:?} with reportGroupsNotFound {not_found:?}"
        );
    }

    // The client's own group is still exactly the one it created, under its own ARN.
    let client_arn = report_group_arn(region, "already-there");
    let (held, not_found) = held_report_group(&router, region, &client_arn).await;
    let held = held.unwrap_or_else(|| {
        panic!(
            "expected the client's own group to still be reported for {client_arn}, got \
             reportGroupsNotFound {not_found:?}"
        )
    });
    assert_eq!(
        held.get("name").and_then(Value::as_str),
        Some("already-there"),
        "expected the client's own group to keep its name, got {held}"
    );
    assert_eq!(
        held.get("type").and_then(Value::as_str),
        Some("TEST"),
        "expected the client's own group to keep the type it was created with, got {held}"
    );
}
