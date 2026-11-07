// SPDX-License-Identifier: CC0-1.0

//! A `DeleteStack` CloudFormation CANCELS because another live stack still imports one of
//! the stack's exported output values destroys NOTHING — in the services that own the
//! stack's resources.
//!
//! The rule under test: **an exported output value another live stack still imports pins the
//! stack that publishes it, so the accepted `DeleteStack` is cancelled: the stack goes back
//! to the healthy status it held carrying
//! `Delete canceled. Cannot delete export <export> as it is in use by <importing stack>.`,
//! every resource it created is still held by the service that owns it, and once no live
//! stack imports that value the identical call deletes the stack and those resources with
//! it.** The second half of that sentence is what needs two crates to agree (CloudFormation
//! decides, the owning service holds the resource), so the suite lives here: a
//! CloudFormation-only test can see the status and the reason but not whether the queue or
//! the topic behind them survived.
//!
//! Why it matters: a terminal `DELETE_FAILED` conventionally says the stack's resources are
//! stranded and the delete should be retried — forced, if need be — so automation following
//! that convention would drive the delete through and destroy a resource another live stack
//! still imports.
//!
//! The rows differ along what the rule generalises over: which service OWNS the exported
//! resource (Amazon SQS, Amazon SNS), which handle the export publishes (`Ref`, or an
//! `Fn::GetAtt` attribute), how the importing stack consumed the import (a property of a
//! resource it really creates, or its own output), how many stacks import it, and how many
//! resources the exporting stack created. A stack nothing imports from is the control row.
//!
//! The awsQuery form encoder and the XML element readers are the test crate's own
//! ([`crate::query_wire`]), not another copy.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, entries_of, form_encode, opt_element, outputs_of};

/// The Region every request here is signed for, and so the Region the stacks, their queues
/// and their topics live in.
const REGION: &str = "us-east-1";

/// Long enough for a stack operation's clock-owed transition (`STATUS_TRANSITION_DELAY`,
/// 500 ms) to come due. Neither a queue nor a topic has a transient status of its own.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// The emulator with the four services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one awsQuery operation of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one CloudFormation operation (API version 2010-05-15).
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// One CloudFormation call that must be answered.
async fn cloudformation_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = cloudformation(router, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudFormation {action}, got {status} with {body}"
    );
    body
}

/// Call one Amazon SNS operation (API version 2010-03-31).
async fn sns(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "sns", "2010-03-31", action, members).await
}

/// Call one awsJson RPC operation of a service addressed by its signing name and
/// `X-Amz-Target` prefix.
async fn json_rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", format!("application/x-amz-json-{json_version}"))
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .header("authorization", authorization(signing_name))
        .body(Bytes::from(request.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Call one Amazon SQS operation (`sigv4Name: sqs`, service shape `AmazonSQS`, awsJson1_0).
async fn sqs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    json_rpc(router, "sqs", "AmazonSQS", "1.0", operation, request).await
}

/// Call one AWS Systems Manager operation (`sigv4Name: ssm`, service shape `AmazonSSM`).
async fn ssm(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    json_rpc(router, "ssm", "AmazonSSM", "1.1", operation, request).await
}

/// The error code an awsJson answer carries, as a client branches on it.
fn json_error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|code| code.rsplit(['#', '/']).next().unwrap_or(code).to_string())
        .unwrap_or_default()
}

/// The awsQuery error `<Code>` of a refusal, or `""` on a success envelope.
fn query_error_code(xml: &str) -> String {
    opt_element(xml, "Code").unwrap_or_default()
}

/// A `GetTopicAttributes` answer's `Attributes`, as the map a client reads it as: each
/// `<entry>`'s own key paired with its own value ([`crate::query_wire::entries_of`]), so an
/// entry carrying only half of the pair contributes nothing instead of misaligning the rest.
fn attributes_of(xml: &str) -> BTreeMap<String, String> {
    entries_of(xml)
        .into_iter()
        .map(|(key, value)| (key, value.to_string()))
        .collect()
}

/// The service that owns the resource an export publishes a handle for.
#[derive(Clone, Copy, Debug)]
enum Owner {
    /// Amazon SQS, where the handle is a queue url.
    SqsQueue,
    /// Amazon SNS, where the handle is a topic ARN.
    SnsTopic,
}

/// Where the importing stack's template resolved its `Fn::ImportValue`.
#[derive(Clone, Copy, Debug)]
enum ImportedIn {
    /// A property of an `AWS::SSM::Parameter` the importing stack really creates, so the
    /// imported value is readable in AWS Systems Manager.
    AResourceProperty,
    /// An output value of the importing stack.
    AnOutput,
}

/// The importing stack's template: the import resolved where the row says.
fn importer_template(export_name: &str, where_imported: ImportedIn) -> String {
    match where_imported {
        ImportedIn::AResourceProperty => format!(
            r#"{{"Resources":{{"Held":{{"Type":"AWS::SSM::Parameter","Properties":
                {{"Type":"String","Value":{{"Fn::ImportValue":"{export_name}"}}}}}}}},
               "Outputs":{{"HeldName":{{"Value":{{"Ref":"Held"}}}}}}}}"#
        ),
        ImportedIn::AnOutput => format!(
            r#"{{"Resources":{{"Handle":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}},
               "Outputs":{{"Echoed":{{"Value":{{"Fn::ImportValue":"{export_name}"}}}}}}}}"#
        ),
    }
}

/// One row: an exporting stack whose export is pinned, and the stacks that pin it.
struct Row {
    case: &'static str,
    /// The exporting stack's name, unique across this suite.
    exporter: &'static str,
    /// Its template: the resources it really creates, the exported output and the
    /// non-exported outputs carrying the handles this row probes.
    template: &'static str,
    /// The service that owns the resources behind `probe_outputs`.
    owner: Owner,
    /// The export name the pin is on, as the template declares it.
    export_name: &'static str,
    /// The output key the export is declared on, whose value is what the importing stacks
    /// resolved.
    export_output: &'static str,
    /// The output keys carrying the handles of the resources the exporting stack created,
    /// which the cancelled delete must leave untouched in `owner`.
    probe_outputs: &'static [&'static str],
    /// The importing stacks, each with where its template resolved the import.
    importers: &'static [(&'static str, ImportedIn)],
}

const ROWS: &[Row] = &[
    Row {
        case: "an SQS queue whose url is the exported value, imported by a parameter's property",
        exporter: "queue-url-exporter",
        template: r#"{"Resources":{"Shared":{"Type":"AWS::SQS::Queue",
               "Properties":{"QueueName":"pinned-shared-queue"}}},
             "Outputs":{"SharedUrl":{"Value":{"Ref":"Shared"},
                 "Export":{"Name":"pinned-queue-url"}},
               "ProbeUrl":{"Value":{"Ref":"Shared"}}}}"#,
        owner: Owner::SqsQueue,
        export_name: "pinned-queue-url",
        export_output: "SharedUrl",
        probe_outputs: &["ProbeUrl"],
        importers: &[("queue-url-importer", ImportedIn::AResourceProperty)],
    },
    Row {
        case: "an SNS topic whose ARN is the exported value, imported by the importer's output",
        exporter: "topic-arn-exporter",
        template: r#"{"Resources":{"Shared":{"Type":"AWS::SNS::Topic",
               "Properties":{"TopicName":"pinned-shared-topic"}}},
             "Outputs":{"SharedArn":{"Value":{"Ref":"Shared"},
                 "Export":{"Name":"pinned-topic-arn"}},
               "ProbeArn":{"Value":{"Ref":"Shared"}}}}"#,
        owner: Owner::SnsTopic,
        export_name: "pinned-topic-arn",
        export_output: "SharedArn",
        probe_outputs: &["ProbeArn"],
        importers: &[("topic-arn-importer", ImportedIn::AnOutput)],
    },
    Row {
        case: "two queues, the second one's Fn::GetAtt ARN exported, imported by two stacks",
        exporter: "queue-arn-exporter",
        template: r#"{"Resources":{
               "First":{"Type":"AWS::SQS::Queue","Properties":{"QueueName":"pinned-first-queue"}},
               "Second":{"Type":"AWS::SQS::Queue","Properties":{"QueueName":"pinned-second-queue"}}},
             "Outputs":{"SecondArn":{"Value":{"Fn::GetAtt":["Second","Arn"]},
                 "Export":{"Name":"pinned-queue-arn"}},
               "FirstProbeUrl":{"Value":{"Ref":"First"}},
               "SecondProbeUrl":{"Value":{"Ref":"Second"}}}}"#,
        owner: Owner::SqsQueue,
        export_name: "pinned-queue-arn",
        export_output: "SecondArn",
        probe_outputs: &["FirstProbeUrl", "SecondProbeUrl"],
        importers: &[
            ("queue-arn-importer", ImportedIn::AResourceProperty),
            ("queue-arn-second-importer", ImportedIn::AnOutput),
        ],
    },
];

/// The reason a cancelled deletion carries, as AWS words it: the fixed text around the
/// export and the stack that still imports it, ending with its period. Written out here
/// rather than read off the emulator's own formatter, so the assertion pins the wording.
fn in_use_reason(export_name: &str, importing_stack_name: &str) -> String {
    format!(
        "Delete canceled. Cannot delete export {export_name} as it is in use by \
         {importing_stack_name}."
    )
}

/// Create a stack, wait past its transition, and answer its `(StackId, Outputs)` once it
/// reports `CREATE_COMPLETE`.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> (String, BTreeMap<String, String>) {
    let created = cloudformation_ok(
        router,
        case,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    let stack_id = opt_element(&created, "StackId")
        .unwrap_or_else(|| panic!("case {case:?}: no StackId for {stack_name} in {created}"));
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let described = describe(router, case, stack_name).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected {stack_name} to complete, got {described}"
    );
    (stack_id, outputs_of(&described))
}

/// One `DescribeStacks` body, by stack name or by stack id.
async fn describe(router: &MultiServiceRouter, case: &str, name_or_id: &str) -> String {
    cloudformation_ok(router, case, "DescribeStacks", &[("StackName", name_or_id)]).await
}

/// The `(StackStatus, StackStatusReason)` one stack reports.
async fn status_of(router: &MultiServiceRouter, case: &str, name_or_id: &str) -> (String, String) {
    let body = describe(router, case, name_or_id).await;
    (
        opt_element(&body, "StackStatus").unwrap_or_default(),
        opt_element(&body, "StackStatusReason").unwrap_or_default(),
    )
}

/// The export names `ListExports` reports.
async fn exported_names(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let body = cloudformation_ok(router, case, "ListExports", &[]).await;
    elements(&body, "Name")
}

/// The owning service still holds the resource at `handle`, and it still works: a queue
/// takes a message and gives it back, a topic reports its own ARN.
async fn must_still_be_held(
    router: &MultiServiceRouter,
    case: &str,
    owner: Owner,
    handle: &str,
) {
    match owner {
        Owner::SqsQueue => {
            let payload = format!("sent after the canceled delete to {handle}");
            let (status, sent) = sqs(
                router,
                "SendMessage",
                &json!({"QueueUrl": handle, "MessageBody": payload}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected SQS to still take a message for {handle}, got {status} \
                 with {sent}"
            );
            let (status, received) = sqs(
                router,
                "ReceiveMessage",
                &json!({"QueueUrl": handle, "MaxNumberOfMessages": 10}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected SQS to still serve {handle}, got {status} with {received}"
            );
            let bodies: Vec<String> = received
                .get("Messages")
                .and_then(Value::as_array)
                .map(|messages| {
                    messages
                        .iter()
                        .filter_map(|message| message.get("Body").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                bodies.contains(&payload),
                "case {case:?}: expected {payload:?} back from {handle}, got {bodies:?}"
            );
        }
        Owner::SnsTopic => {
            let (status, body) = sns(router, "GetTopicAttributes", &[("TopicArn", handle)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected SNS to still hold {handle}, got {status} with {body}"
            );
            let attributes = attributes_of(&body);
            assert_eq!(
                attributes.get("TopicArn").map(String::as_str),
                Some(handle),
                "case {case:?}: expected SNS to report {handle}, got {attributes:?}"
            );
        }
    }
}

/// The owning service no longer holds the resource at `handle`, which is what the delete
/// that DID go through has to have done.
async fn must_no_longer_be_held(
    router: &MultiServiceRouter,
    case: &str,
    owner: Owner,
    handle: &str,
) {
    match owner {
        Owner::SqsQueue => {
            let (status, answer) =
                sqs(router, "ReceiveMessage", &json!({"QueueUrl": handle})).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the queue at {handle} to be gone, got {status} with \
                 {answer}"
            );
            assert_eq!(
                json_error_code(&answer),
                "QueueDoesNotExist",
                "case {case:?}: expected QueueDoesNotExist for {handle}, got {answer}"
            );
        }
        Owner::SnsTopic => {
            let (status, answer) = sns(router, "GetTopicAttributes", &[("TopicArn", handle)]).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the topic at {handle} to be gone, got {status} with \
                 {answer}"
            );
            assert_eq!(
                query_error_code(&answer),
                "NotFound",
                "case {case:?}: expected NotFound for {handle}, got {answer}"
            );
        }
    }
}

#[tokio::test]
async fn a_canceled_delete_leaves_every_exported_resource_in_its_owning_service() {
    for row in ROWS {
        let case = row.case;
        let router = build_router();
        let (exporter_id, exporter_outputs) =
            create_and_settle(&router, case, row.exporter, row.template).await;
        let exported_value = exporter_outputs
            .get(row.export_output)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: no {} in {exporter_outputs:?}",
                    row.export_output
                )
            });
        let handles: Vec<String> = row
            .probe_outputs
            .iter()
            .map(|key| {
                exporter_outputs
                    .get(*key)
                    .cloned()
                    .unwrap_or_else(|| panic!("case {case:?}: no {key} in {exporter_outputs:?}"))
            })
            .collect();

        // Each importing stack really resolved the import, which is the pin.
        for (importer, where_imported) in row.importers {
            let (_, outputs) = create_and_settle(
                &router,
                case,
                importer,
                &importer_template(row.export_name, *where_imported),
            )
            .await;
            match where_imported {
                ImportedIn::AResourceProperty => {
                    let parameter_name = outputs.get("HeldName").cloned().unwrap_or_else(|| {
                        panic!("case {case:?}: no HeldName for {importer} in {outputs:?}")
                    });
                    let (status, read) =
                        ssm(&router, "GetParameter", &json!({"Name": parameter_name})).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected Systems Manager to hold {importer}'s parameter, \
                         got {status} with {read}"
                    );
                    let held = read
                        .pointer("/Parameter/Value")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    assert_eq!(
                        held, exported_value,
                        "case {case:?}: expected the imported value {exported_value:?} in the \
                         parameter, got {held:?}"
                    );
                }
                ImportedIn::AnOutput => {
                    let echoed = outputs.get("Echoed").cloned().unwrap_or_default();
                    assert_eq!(
                        echoed, exported_value,
                        "case {case:?}: expected {importer} to import {exported_value:?}, got \
                         {echoed:?}"
                    );
                }
            }
        }

        let listed = cloudformation_ok(
            &router,
            case,
            "ListImports",
            &[("ExportName", row.export_name)],
        )
        .await;
        for (importer, _) in row.importers {
            assert!(
                elements(&listed, "member").contains(&(*importer).to_string()),
                "case {case:?}: expected ListImports to report {importer}, got {listed}"
            );
        }

        // The CALL is accepted: the model declares no error for a pinned export.
        cloudformation_ok(
            &router,
            case,
            "DeleteStack",
            &[("StackName", row.exporter)],
        )
        .await;
        tokio::time::sleep(PAST_THE_STACK_SPAN).await;

        // CANCELLED: the stack is back in the healthy status it held, saying which export
        // and which importer stood in the way.
        let (settled, reason) = status_of(&router, case, row.exporter).await;
        assert_eq!(
            settled, "CREATE_COMPLETE",
            "case {case:?}: expected the cancelled delete to restore CREATE_COMPLETE, got \
             {settled:?}"
        );
        // The reason's EXPORT half is asserted on its own, so a reason naming another
        // export cannot pass by belonging to the set of accepted importers below.
        let expected_head = format!(
            "Delete canceled. Cannot delete export {} as it is in use by ",
            row.export_name
        );
        assert!(
            reason.starts_with(&expected_head),
            "case {case:?}: expected the StackStatusReason to open {expected_head:?}, got \
             {reason:?}"
        );
        let accepted: Vec<String> = row
            .importers
            .iter()
            .map(|(importer, _)| in_use_reason(row.export_name, importer))
            .collect();
        assert!(
            accepted.contains(&reason),
            "case {case:?}: expected one of {accepted:?} as the StackStatusReason, got {reason:?}"
        );
        let stream = cloudformation_ok(
            &router,
            case,
            "DescribeStackEvents",
            &[("StackName", row.exporter)],
        )
        .await;
        assert!(
            stream.contains("Delete canceled") && stream.contains(row.export_name),
            "case {case:?}: expected the cancellation and its export on the event stream, got \
             {stream}"
        );

        // NOTHING was destroyed: the export is still published, and every resource the stack
        // created is still held — and still working — in the service that owns it.
        let published = exported_names(&router, case).await;
        assert!(
            published.contains(&row.export_name.to_string()),
            "case {case:?}: expected {:?} to survive the cancelled delete, got {published:?}",
            row.export_name
        );
        for handle in &handles {
            must_still_be_held(&router, case, row.owner, handle).await;
        }

        // Once the last importer is gone the identical call deletes the stack, and the
        // resources go with it.
        for (importer, _) in row.importers {
            cloudformation_ok(&router, case, "DeleteStack", &[("StackName", importer)]).await;
        }
        tokio::time::sleep(PAST_THE_STACK_SPAN).await;
        cloudformation_ok(
            &router,
            case,
            "DeleteStack",
            &[("StackName", row.exporter)],
        )
        .await;
        tokio::time::sleep(PAST_THE_STACK_SPAN).await;

        // Addressed by STACK ID, since a completed deletion leaves the live set.
        let (settled, _) = status_of(&router, case, &exporter_id).await;
        assert_eq!(
            settled, "DELETE_COMPLETE",
            "case {case:?}: expected the retried delete to complete, got {settled:?}"
        );
        let published = exported_names(&router, case).await;
        assert!(
            !published.contains(&row.export_name.to_string()),
            "case {case:?}: expected {:?} to go with its stack, got {published:?}",
            row.export_name
        );
        for handle in &handles {
            must_no_longer_be_held(&router, case, row.owner, handle).await;
        }
    }
}

/// The control row: a stack whose exports no live stack imports is deleted, and the queue it
/// created is gone from Amazon SQS — so the cancellation above is the pin at work and not a
/// delete path that never destroys anything.
#[tokio::test]
async fn a_delete_nothing_imports_from_deletes_the_queue_it_created() {
    let case = "no live stack imports what the exporter publishes";
    let router = build_router();
    let template = r#"{"Resources":{"Free":{"Type":"AWS::SQS::Queue",
           "Properties":{"QueueName":"unpinned-queue"}}},
         "Outputs":{"FreeUrl":{"Value":{"Ref":"Free"},"Export":{"Name":"unpinned-queue-url"}}}}"#;
    let (exporter_id, outputs) = create_and_settle(&router, case, "free-exporter", template).await;
    let url = outputs
        .get("FreeUrl")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no FreeUrl in {outputs:?}"));

    // Another stack publishes another export, and a third imports THAT one: an import of a
    // name this stack does not publish pins nothing of it.
    let other_exporter = r#"{"Resources":{"Handle":
           {"Type":"AWS::CloudFormation::WaitConditionHandle"}},
         "Outputs":{"Other":{"Value":"other-value",
             "Export":{"Name":"another-export"}}}}"#;
    create_and_settle(&router, case, "other-exporter", other_exporter).await;
    create_and_settle(
        &router,
        case,
        "unrelated-importer",
        &importer_template("another-export", ImportedIn::AnOutput),
    )
    .await;

    cloudformation_ok(
        &router,
        case,
        "DeleteStack",
        &[("StackName", "free-exporter")],
    )
    .await;
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    let (settled, reason) = status_of(&router, case, &exporter_id).await;
    assert_eq!(
        settled, "DELETE_COMPLETE",
        "case {case:?}: expected an unpinned delete to complete, got {settled:?}"
    );
    assert!(
        !reason.contains("Delete canceled"),
        "case {case:?}: expected no cancellation reason, got {reason:?}"
    );
    must_no_longer_be_held(&router, case, Owner::SqsQueue, &url).await;
}
