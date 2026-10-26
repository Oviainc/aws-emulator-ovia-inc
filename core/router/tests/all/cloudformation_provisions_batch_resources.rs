// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares `AWS::Batch::ComputeEnvironment` and
//! `AWS::Batch::JobQueue` really creates those resources in AWS Batch, and the handles the
//! stack hands back are the ARNs that service answers to.
//!
//! The rule under test: **a resource of a type this emulator provisions is created in the
//! service that OWNS it, through that service's own public API, and the identifier the
//! stack publishes for it is the one that type's published return values document — "Ref
//! returns the compute environment ARN", "Ref returns the job queue ARN" — so `Ref` and
//! `Fn::GetAtt` resolve to a value `batch describe-compute-environments` and `batch
//! describe-job-queues` resolve to the resource the template declared.** Two crates have to
//! agree for that to hold (CloudFormation makes the calls, AWS Batch serves them), so the
//! suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES each
//! resource or leaves CloudFormation to generate its name, whether the compute environment
//! is `UNMANAGED` or `MANAGED` (and so whether it declares `ComputeResources`), whether the
//! queue declares `Tags`, a `Priority` and a `State`, whether the template is written in
//! JSON or YAML, and which intrinsic the outputs read each resource with (`Ref`,
//! `Fn::GetAtt`, `Fn::Sub`).
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the GENERATED names of the row that declares none,
//! which are CloudFormation's to compute: what is checked about those is that the ARNs the
//! stack publishes are those names' Batch ARNs and that AWS Batch resolves them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span and for the two Batch resources the stack
/// waits on to leave `CREATING`, with margin for a slow machine.
const PAST_STACK_SPAN: Duration = Duration::from_millis(2_500);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

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
        service_com_amazonaws_batch_awsbatchv20160810::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
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

/// Call one AWS Batch operation over its restJson1 wire, signed for `region`.
async fn batch(
    router: &MultiServiceRouter,
    region: &str,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", format!("batch.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("batch", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// `ListTagsForResource` for `arn`, which AWS Batch binds to `GET /v1/tags/{resourceArn}`
/// — the ARN travels in the URI rather than in a body.
async fn batch_tags(router: &MultiServiceRouter, region: &str, arn: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/v1/tags/{}", form_encode(arn)))
        .header("host", format!("batch.{region}.amazonaws.com"))
        .header("authorization", authorization("batch", region))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The ARN a Batch resource of `kind` and `name` carries in `region`: the same total
/// function of the Region and the name both services compute it from.
fn batch_arn(region: &str, kind: &str, name: &str) -> String {
    format!("arn:aws:batch:{region}:{ACCOUNT}:{kind}/{name}")
}

/// One row of the table: a template, and what the rule says the two resources it declares
/// must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the resources are created in and
    /// the one their ARNs carry.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The compute environment name the template declares, or `None` for a template that
    /// names none — whose environment carries the name CloudFormation generated.
    environment_name: Option<&'static str>,
    /// The job queue name the template declares, or `None` as above.
    queue_name: Option<&'static str>,
    /// The `CEType` the environment must report.
    environment_type: &'static str,
    /// The `priority` the queue must report.
    queue_priority: i64,
    /// The `JQState` the queue must report.
    queue_state: &'static str,
    /// The tags the queue's own declaration asks for, beside the three CloudFormation
    /// records on every resource it provisions.
    queue_tags: &'static [(&'static str, &'static str)],
    /// The output keys whose value must be the compute environment's ARN.
    environment_outputs: &'static [&'static str],
    /// The output keys whose value must be the job queue's ARN.
    queue_outputs: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named UNMANAGED environment and a tagged queue in JSON, read with Ref and \
               both attributes",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "an AWS Batch compute environment and job queue",
                "Resources": {
                    "Environment": {
                        "Type": "AWS::Batch::ComputeEnvironment",
                        "Properties": {
                            "ComputeEnvironmentName": "orders-ce",
                            "Type": "UNMANAGED",
                            "State": "ENABLED",
                        },
                    },
                    "Queue": {
                        "Type": "AWS::Batch::JobQueue",
                        "Properties": {
                            "JobQueueName": "orders-jq",
                            "Priority": 7,
                            "State": "ENABLED",
                            "ComputeEnvironmentOrder": [
                                { "Order": 1, "ComputeEnvironment": { "Ref": "Environment" } }
                            ],
                            "Tags": { "Owner": "orders", "Tier": "prod" },
                        },
                    },
                },
                "Outputs": {
                    "EnvironmentRef": { "Value": { "Ref": "Environment" } },
                    "EnvironmentArn": {
                        "Value": { "Fn::GetAtt": ["Environment", "ComputeEnvironmentArn"] }
                    },
                    "QueueRef": { "Value": { "Ref": "Queue" } },
                    "QueueArn": { "Value": { "Fn::GetAtt": ["Queue", "JobQueueArn"] } },
                },
            })
            .to_string()
        },
        environment_name: Some("orders-ce"),
        queue_name: Some("orders-jq"),
        environment_type: "UNMANAGED",
        queue_priority: 7,
        queue_state: "ENABLED",
        queue_tags: &[("Owner", "orders"), ("Tier", "prod")],
        environment_outputs: &["EnvironmentRef", "EnvironmentArn"],
        queue_outputs: &["QueueRef", "QueueArn"],
    },
    Row {
        case: "a MANAGED environment with ComputeResources and a DISABLED queue in YAML, \
               read with Fn::Sub",
        region: "eu-west-1",
        template: || {
            [
                "Resources:",
                "  Environment:",
                "    Type: AWS::Batch::ComputeEnvironment",
                "    Properties:",
                "      ComputeEnvironmentName: billing-ce",
                "      Type: MANAGED",
                "      ServiceRole: arn:aws:iam::000000000000:role/AWSBatchServiceRole",
                "      ComputeResources:",
                "        Type: FARGATE",
                "        MaxvCpus: 16",
                "        Subnets:",
                "          - subnet-0123456789abcdef0",
                "        SecurityGroupIds:",
                "          - sg-0123456789abcdef0",
                "  Queue:",
                "    Type: AWS::Batch::JobQueue",
                "    Properties:",
                "      JobQueueName: billing-jq",
                "      Priority: 1",
                "      State: DISABLED",
                "      ComputeEnvironmentOrder:",
                "        - Order: 1",
                "          ComputeEnvironment: !Ref Environment",
                "Outputs:",
                "  EnvironmentRef:",
                "    Value: !Ref Environment",
                "  QueueSub:",
                "    Value: !Sub '${Queue}'",
                "",
            ]
            .join("\n")
        },
        environment_name: Some("billing-ce"),
        queue_name: Some("billing-jq"),
        environment_type: "MANAGED",
        queue_priority: 1,
        queue_state: "DISABLED",
        queue_tags: &[],
        environment_outputs: &["EnvironmentRef"],
        queue_outputs: &["QueueSub"],
    },
    Row {
        case: "neither resource is named, so CloudFormation generates both names",
        region: "ap-northeast-1",
        template: || {
            json!({
                "Resources": {
                    "Environment": {
                        "Type": "AWS::Batch::ComputeEnvironment",
                        "Properties": { "Type": "UNMANAGED" },
                    },
                    "Queue": {
                        "Type": "AWS::Batch::JobQueue",
                        "Properties": {
                            "Priority": 0,
                            "ComputeEnvironmentOrder": [
                                { "Order": 1, "ComputeEnvironment": { "Ref": "Environment" } }
                            ],
                        },
                    },
                },
                "Outputs": {
                    "EnvironmentRef": { "Value": { "Ref": "Environment" } },
                    "QueueRef": { "Value": { "Ref": "Queue" } },
                },
            })
            .to_string()
        },
        environment_name: None,
        queue_name: None,
        environment_type: "UNMANAGED",
        queue_priority: 0,
        // `CreateJobQueue$state`: "If the job queue state is ENABLED … The default value is
        // ENABLED", so a declaration that states none gets that.
        queue_state: "ENABLED",
        queue_tags: &[],
        environment_outputs: &["EnvironmentRef"],
        queue_outputs: &["QueueRef"],
    },
];

/// Every row: the ARNs the stack reports for its two `AWS::Batch::*` resources are those
/// resources' own, AWS Batch resolves each of them — by the ARN and by the NAME, both of
/// which its readers accept — to the resource the template declared, the queue orders the
/// environment the template pointed it at, and deleting the stack deletes both in Batch.
#[tokio::test]
async fn a_stack_provisions_the_batch_compute_environment_and_job_queue_it_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("batch-stack-{index}");
        let template = (row.template)();

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
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let outputs = outputs_of(&described);
        let environment_arn = outputs
            .get(row.environment_outputs[0])
            .unwrap_or_else(|| panic!("case {case:?}: no environment output in {described}"))
            .clone();
        let queue_arn = outputs
            .get(row.queue_outputs[0])
            .unwrap_or_else(|| panic!("case {case:?}: no queue output in {described}"))
            .clone();

        // "Ref returns the compute environment ARN" / "the job queue ARN": each published
        // handle is that ARN, and the name inside it is the one the template asked for —
        // or, for a template that named nothing, the one CloudFormation generated.
        let environment_name = match row.environment_name {
            Some(declared) => declared.to_string(),
            None => environment_arn
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string(),
        };
        let queue_name = match row.queue_name {
            Some(declared) => declared.to_string(),
            None => queue_arn.rsplit('/').next().unwrap_or_default().to_string(),
        };
        for (kind, name, arn) in [
            ("compute-environment", &environment_name, &environment_arn),
            ("job-queue", &queue_name, &queue_arn),
        ] {
            let expected = batch_arn(region, kind, name);
            assert_eq!(
                arn, &expected,
                "case {case:?}: expected the {kind} handle to be {expected}, got {arn:?}"
            );
            assert!(
                !name.is_empty() && name.len() <= 128,
                "case {case:?}: expected a {kind} name inside the documented 1..128, got \
                 {name:?}"
            );
        }
        for key in row.environment_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&environment_arn),
                "case {case:?}: expected output {key:?} to be {environment_arn}, got {:?}",
                outputs.get(*key)
            );
        }
        for key in row.queue_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&queue_arn),
                "case {case:?}: expected output {key:?} to be {queue_arn}, got {:?}",
                outputs.get(*key)
            );
        }

        // And AWS Batch itself resolves each handle — by the ARN and by the NAME, both of
        // which its readers document ("name or full Amazon Resource Name (ARN)").
        for identifier in [environment_arn.clone(), environment_name.clone()] {
            let (status, answered) = batch(
                &router,
                region,
                "/v1/describecomputeenvironments",
                &json!({ "computeEnvironments": [identifier] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeComputeEnvironments on \
                 {identifier}, got {status} with body {answered}"
            );
            let environment = &answered["computeEnvironments"][0];
            assert_eq!(
                environment["computeEnvironmentArn"].as_str(),
                Some(environment_arn.as_str()),
                "case {case:?}: expected {identifier} to resolve to {environment_arn}, got \
                 {answered}"
            );
            assert_eq!(
                environment["computeEnvironmentName"].as_str(),
                Some(environment_name.as_str()),
                "case {case:?}: expected the declared name {environment_name}, got {answered}"
            );
            assert_eq!(
                environment["type"].as_str(),
                Some(row.environment_type),
                "case {case:?}: expected the declared CEType, got {answered}"
            );
            assert_eq!(
                environment["status"].as_str(),
                Some("VALID"),
                "case {case:?}: a stack that completed leaves its environment VALID, got \
                 {answered}"
            );
        }

        for identifier in [queue_arn.clone(), queue_name.clone()] {
            let (status, answered) = batch(
                &router,
                region,
                "/v1/describejobqueues",
                &json!({ "jobQueues": [identifier] }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from DescribeJobQueues on {identifier}, got \
                 {status} with body {answered}"
            );
            let queues = answered["jobQueues"].as_array().cloned().unwrap_or_default();
            assert_eq!(
                queues.len(),
                1,
                "case {case:?}: expected exactly one jobQueue for {identifier}, got {answered}"
            );
            let queue = &queues[0];
            assert_eq!(
                queue["jobQueueArn"].as_str(),
                Some(queue_arn.as_str()),
                "case {case:?}: expected {identifier} to resolve to {queue_arn}, got {answered}"
            );
            assert_eq!(
                queue["jobQueueName"].as_str(),
                Some(queue_name.as_str()),
                "case {case:?}: expected the declared name {queue_name}, got {answered}"
            );
            assert_eq!(
                queue["priority"].as_i64(),
                Some(row.queue_priority),
                "case {case:?}: expected the declared priority, got {answered}"
            );
            assert_eq!(
                queue["state"].as_str(),
                Some(row.queue_state),
                "case {case:?}: expected the declared state, got {answered}"
            );
            // The queue orders the environment the template pointed it at, by the ARN that
            // environment carries — which is what makes the two resources one deployment.
            assert_eq!(
                queue["computeEnvironmentOrder"][0]["computeEnvironment"].as_str(),
                Some(environment_arn.as_str()),
                "case {case:?}: expected the queue to order {environment_arn}, got {answered}"
            );
        }

        // The tags the queue's declaration asked for, plus the three CloudFormation records
        // on every resource it provisions, read through Batch's own tagging call.
        let (status, listed) = batch_tags(&router, region, &queue_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} with body \
             {listed}"
        );
        let reported = listed["tags"].as_object().cloned().unwrap_or_default();
        for (key, value) in row.queue_tags {
            assert_eq!(
                reported.get(*key).and_then(Value::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {listed}"
            );
        }
        let logical_stack_id = opt_element(&described, "StackId")
            .unwrap_or_else(|| panic!("case {case:?}: no <StackId> in {described}"));
        for (key, value) in [
            ("aws:cloudformation:stack-name", stack_name.as_str()),
            ("aws:cloudformation:stack-id", logical_stack_id.as_str()),
            ("aws:cloudformation:logical-id", "Queue"),
        ] {
            assert_eq!(
                reported.get(key).and_then(Value::as_str),
                Some(value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {listed}"
            );
        }

        // Deleting the stack deletes both resources in AWS Batch.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        for (uri, member, identifier) in [
            ("/v1/describejobqueues", "jobQueues", queue_arn.clone()),
            (
                "/v1/describecomputeenvironments",
                "computeEnvironments",
                environment_arn.clone(),
            ),
        ] {
            let request = match member {
                "jobQueues" => json!({ "jobQueues": [identifier] }),
                _ => json!({ "computeEnvironments": [identifier] }),
            };
            let (status, gone) = batch(&router, region, uri, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from the read after the stack's deletion, got \
                 {status} with body {gone}"
            );
            assert_eq!(
                gone[member].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: expected AWS Batch to hold no {member} once the stack deleted \
                 it, got {gone}"
            );
        }
    }
}

/// A stack whose template declares a job queue AWS Batch REFUSES does not report a resource
/// nothing created: the stack fails with the refusal the owning service gave.
///
/// The refusal here is Batch's own: `CreateJobQueue` will not attach a compute environment
/// that does not exist ("Compute environments must be created and valid before attaching
/// them to a job queue"), which a template reaches by ordering an environment no resource
/// of the stack declares.
#[tokio::test]
async fn a_job_queue_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let case = "a queue ordering a compute environment nothing created";
    let region = "us-east-2";
    let stack_name = "batch-queue-clash";

    let template = json!({
        "Resources": {
            "Queue": {
                "Type": "AWS::Batch::JobQueue",
                "Properties": {
                    "JobQueueName": "orphan-jq",
                    "Priority": 1,
                    "ComputeEnvironmentOrder": [
                        { "Order": 1, "ComputeEnvironment": "no-such-compute-environment" }
                    ],
                },
            },
        },
    })
    .to_string();
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let (_, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back, got {described}"
    );
    let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Queue"),
        "case {case:?}: expected the failure to name the resource, got {reason:?}"
    );

    // And nothing was left behind in AWS Batch for the queue the stack could not create.
    let (status, gone) = batch(
        &router,
        region,
        "/v1/describejobqueues",
        &json!({ "jobQueues": ["orphan-jq"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeJobQueues, got {status} with body {gone}"
    );
    assert_eq!(
        gone["jobQueues"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: expected no job queue for a stack that failed, got {gone}"
    );
}
