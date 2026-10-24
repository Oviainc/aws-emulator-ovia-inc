// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::StepFunctions::Activity` really creates
//! an activity in AWS Step Functions, and the handle the stack hands back is the ARN that
//! service answers to.
//!
//! The rule under test: **a resource of a type this emulator provisions is created in the
//! service that OWNS it, through that service's own public API, and the identifier the
//! stack publishes for it is the one that type's published return values document — for an
//! activity, "Ref returns the ARN of the activity" — so `Ref` and `Fn::GetAtt` resolve to a
//! value `sfn DescribeActivity` resolves to the activity the template declared.** Two
//! crates have to agree for that to hold (CloudFormation makes the call, Step Functions
//! serves it), so the suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! activity or leaves CloudFormation to generate its name, whether it declares `Tags` and
//! an `EncryptionConfiguration` (and which encryption type), whether it is written in JSON
//! or YAML, and which intrinsic the outputs read the resource with (`Ref`,
//! `Fn::GetAtt … Arn`, `Fn::GetAtt … Name`, `Fn::Sub`).
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the GENERATED name of the row that declares none,
//! which is CloudFormation's to compute: what is checked about it is that the ARN the stack
//! publishes is that name's activity ARN and that Step Functions resolves it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) to come
/// due, so the stack reports the template it deployed.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// A KMS key ARN of the form Step Functions' `kmsKeyId` documents ("an alias, alias ARN,
/// key ID, or key ARN"), for the customer-managed row.
const KMS_KEY_ARN: &str = "arn:aws:kms:eu-west-1:000000000000:key/12345678-1234-1234-1234-123456789012";

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
        service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(engine),
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

/// Call one Step Functions operation over its awsJson1_0 wire, signed for `region`.
async fn sfn(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("states.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AWSStepFunctions.{operation}"))
        .header("authorization", authorization("states", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as. Each
/// `member` of the list carries exactly one `OutputKey` and one `OutputValue`, so the two
/// document-ordered lists pair up.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The ARN an activity of `name` carries in `region`: the same total function of the Region
/// and the name both services compute it from.
fn activity_arn(region: &str, name: &str) -> String {
    format!("arn:aws:states:{region}:{ACCOUNT}:activity:{name}")
}

/// One row of the table: a template, and what the rule says the activity it declares must
/// then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the activity is created in and
    /// the one its ARN carries.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The name the template declares, or `None` for a template that names nothing — whose
    /// activity carries the name CloudFormation generated.
    declared_name: Option<&'static str>,
    /// The tags `sfn ListTagsForResource` must report for the activity's ARN, beside the
    /// three CloudFormation records on every resource it provisions.
    expected_tags: &'static [(&'static str, &'static str)],
    /// The members `DescribeActivity`'s `encryptionConfiguration` must carry.
    expected_encryption: &'static [(&'static str, &'static str)],
    /// The output keys whose value must be the activity's ARN, each written with a
    /// different intrinsic over the same resource.
    arn_outputs: &'static [&'static str],
    /// The output keys whose value must be the activity's NAME.
    name_outputs: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a named, tagged activity in JSON, read with Ref and both attributes",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a Step Functions activity",
                "Resources": {
                    "Activity": {
                        "Type": "AWS::StepFunctions::Activity",
                        "Properties": {
                            "Name": "orders-worker",
                            "Tags": [
                                { "Key": "Owner", "Value": "orders" },
                                { "Key": "Tier", "Value": "prod" }
                            ]
                        }
                    }
                },
                "Outputs": {
                    "ActivityRef": { "Value": { "Ref": "Activity" } },
                    "ActivityArn": { "Value": { "Fn::GetAtt": ["Activity", "Arn"] } },
                    "ActivityName": { "Value": { "Fn::GetAtt": ["Activity", "Name"] } }
                }
            })
            .to_string()
        },
        declared_name: Some("orders-worker"),
        expected_tags: &[("Owner", "orders"), ("Tier", "prod")],
        // A create that configured no encryption is encrypted under the
        // Amazon-Web-Services-owned key, which the describe reports.
        expected_encryption: &[("type", "AWS_OWNED_KEY")],
        arn_outputs: &["ActivityRef", "ActivityArn"],
        name_outputs: &["ActivityName"],
    },
    Row {
        case: "a named activity in YAML, untagged, the owned key spelled out",
        region: "us-west-2",
        template: || {
            [
                "Resources:",
                "  Activity:",
                "    Type: AWS::StepFunctions::Activity",
                "    Properties:",
                "      Name: billing-worker",
                "      EncryptionConfiguration:",
                "        Type: AWS_OWNED_KEY",
                "Outputs:",
                "  ActivityRef:",
                "    Value: !Ref Activity",
                "",
            ]
            .join("\n")
        },
        declared_name: Some("billing-worker"),
        expected_tags: &[],
        expected_encryption: &[("type", "AWS_OWNED_KEY")],
        arn_outputs: &["ActivityRef"],
        name_outputs: &[],
    },
    Row {
        case: "a customer-managed key, an Fn::Sub output over the ARN",
        region: "eu-west-1",
        template: || {
            json!({
                "Resources": {
                    "Activity": {
                        "Type": "AWS::StepFunctions::Activity",
                        "Properties": {
                            "Name": "secure-worker",
                            "EncryptionConfiguration": {
                                "Type": "CUSTOMER_MANAGED_KMS_KEY",
                                "KmsKeyId": KMS_KEY_ARN,
                                "KmsDataKeyReusePeriodSeconds": 300
                            },
                            "Tags": [{ "Key": "Owner", "Value": "security" }]
                        }
                    }
                },
                "Outputs": {
                    "ActivityRef": { "Value": { "Ref": "Activity" } },
                    "ActivitySub": {
                        "Value": { "Fn::Sub": ["${Arn}", { "Arn": { "Ref": "Activity" } }] }
                    }
                }
            })
            .to_string()
        },
        declared_name: Some("secure-worker"),
        expected_tags: &[("Owner", "security")],
        expected_encryption: &[
            ("type", "CUSTOMER_MANAGED_KMS_KEY"),
            ("kmsKeyId", KMS_KEY_ARN),
        ],
        arn_outputs: &["ActivityRef", "ActivitySub"],
        name_outputs: &[],
    },
    Row {
        case: "an activity the template does not name, so CloudFormation generates one",
        region: "ap-northeast-1",
        template: || {
            [
                "Resources:",
                "  Activity:",
                "    Type: AWS::StepFunctions::Activity",
                "Outputs:",
                "  ActivityRef:",
                "    Value: !Ref Activity",
                "  ActivityName:",
                "    Value: !GetAtt Activity.Name",
                "",
            ]
            .join("\n")
        },
        declared_name: None,
        expected_tags: &[],
        expected_encryption: &[("type", "AWS_OWNED_KEY")],
        arn_outputs: &["ActivityRef"],
        name_outputs: &["ActivityName"],
    },
];

/// Every row: the ARN the stack reports for its `AWS::StepFunctions::Activity` is the
/// activity's own, `sfn DescribeActivity` resolves it to the activity the template
/// declared, the tags CloudFormation gives the resource are on it, and deleting the stack
/// deletes it there.
#[tokio::test]
async fn a_stack_provisions_the_step_functions_activity_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("sfn-activity-stack-{index}");
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

        // The stack reports the template it deployed once the create COMPLETES.
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

        // "Ref returns the ARN of the activity": the physical id is that ARN, and the name
        // inside it is the one the template asked for — or, for a template that named
        // nothing, the one CloudFormation generated.
        let name = match row.declared_name {
            Some(declared) => declared.to_string(),
            None => physical_id
                .rsplit(':')
                .next()
                .unwrap_or_default()
                .to_string(),
        };
        let arn = activity_arn(region, &name);
        assert_eq!(
            physical_id, arn,
            "case {case:?}: expected the physical id to be the activity ARN {arn}, got \
             {physical_id:?}"
        );
        assert!(
            !name.is_empty() && name.len() <= 80,
            "case {case:?}: expected a name inside the modelled 1..80, got {name:?}"
        );

        let outputs = outputs_of(&described);
        for key in row.arn_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&arn),
                "case {case:?}: expected output {key:?} to be the activity ARN {arn:?}, got {:?}",
                outputs.get(*key)
            );
        }
        for key in row.name_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&name),
                "case {case:?}: expected output {key:?} to be the activity name {name:?}, got \
                 {:?}",
                outputs.get(*key)
            );
        }

        // And Step Functions itself resolves that ARN to the activity the template declared.
        let (status, activity) = sfn(
            &router,
            region,
            "DescribeActivity",
            &json!({ "activityArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeActivity on the ARN the stack \
             reported, got {status} with body {activity}"
        );
        assert_eq!(
            activity.get("activityArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the describe to report {arn}, got {activity}"
        );
        assert_eq!(
            activity.get("name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: expected the describe to report the name {name:?}, got {activity}"
        );
        assert!(
            activity.get("creationDate").is_some(),
            "case {case:?}: expected the describe to report a creationDate, got {activity}"
        );
        let encryption = activity
            .get("encryptionConfiguration")
            .cloned()
            .unwrap_or(Value::Null);
        for (member, expected) in row.expected_encryption {
            assert_eq!(
                encryption.get(*member).and_then(Value::as_str),
                Some(*expected),
                "case {case:?}: expected encryptionConfiguration.{member} to be {expected:?}, \
                 got {encryption}"
            );
        }

        // The tags the declaration asked for, plus the three CloudFormation records on
        // every resource it provisions, read through Step Functions' own tagging call.
        let (status, listed) = sfn(
            &router,
            region,
            "ListTagsForResource",
            &json!({ "resourceArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} with body \
             {listed}"
        );
        let reported: BTreeMap<String, String> = listed
            .get("tags")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no 'tags' array in {listed}"))
            .iter()
            .filter_map(|tag| {
                Some((
                    tag.get("key").and_then(Value::as_str)?.to_string(),
                    tag.get("value").and_then(Value::as_str)?.to_string(),
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
        let logical_id = opt_element(&resources, "LogicalResourceId")
            .unwrap_or_else(|| panic!("case {case:?}: no <LogicalResourceId> in {resources}"));
        let stack_id = opt_element(&described, "StackId")
            .unwrap_or_else(|| panic!("case {case:?}: no <StackId> in {described}"));
        for (key, value) in [
            ("aws:cloudformation:stack-name", stack_name.as_str()),
            ("aws:cloudformation:stack-id", stack_id.as_str()),
            ("aws:cloudformation:logical-id", logical_id.as_str()),
        ] {
            assert_eq!(
                reported.get(key).map(String::as_str),
                Some(value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported:?}"
            );
        }
        assert_eq!(
            reported.len(),
            row.expected_tags.len() + 3,
            "case {case:?}: expected exactly the {} tag(s) the template declared beside the \
             three CloudFormation records, got {reported:?}",
            row.expected_tags.len()
        );

        // Deleting the stack deletes the activity Step Functions holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = sfn(
            &router,
            region,
            "DescribeActivity",
            &json!({ "activityArn": arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from DescribeActivity once the stack deleted the \
             activity, got {status} with body {gone}"
        );
        assert!(
            gone.get("__type")
                .and_then(Value::as_str)
                .is_some_and(|code| code.ends_with("ActivityDoesNotExist")),
            "case {case:?}: expected ActivityDoesNotExist once the activity was deleted, got \
             {gone}"
        );
    }
}

/// A stack whose template declares an activity Step Functions REFUSES does not report a
/// resource nothing created: the stack fails, with the refusal the owning service gave, and
/// the activity a client already had is untouched.
///
/// The refusal here is Step Functions' own `ActivityAlreadyExists`, reached by pointing a
/// template at a name a client already created with DIFFERENT encryption settings — the one
/// difference that create's idempotency does not absorb ("EncryptionConfiguration may not be
/// updated").
#[tokio::test]
async fn an_activity_the_owning_service_refuses_fails_the_stack() {
    let router = build_router();
    let case = "a template naming an activity that exists with other encryption settings";
    let region = "us-east-2";
    let name = "already-there";

    let (status, created) = sfn(
        &router,
        region,
        "CreateActivity",
        &json!({
            "name": name,
            "encryptionConfiguration": {
                "type": "CUSTOMER_MANAGED_KMS_KEY",
                "kmsKeyId": "arn:aws:kms:us-east-2:000000000000:key/12345678-1234-1234-1234-123456789012",
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the client's own CreateActivity to succeed, got {status} with \
         body {created}"
    );

    let template = json!({
        "Resources": {
            "Activity": {
                "Type": "AWS::StepFunctions::Activity",
                "Properties": { "Name": name }
            }
        }
    })
    .to_string();
    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", "activity-clash"), ("TemplateBody", &template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let (_, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", "activity-clash")]).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("CREATE_FAILED"),
        "case {case:?}: expected the stack to fail, got {described}"
    );
    let reason = opt_element(&described, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Activity"),
        "case {case:?}: expected the failure to name the resource, got {reason:?}"
    );

    // The activity the client created is still exactly as it was.
    let (status, activity) = sfn(
        &router,
        region,
        "DescribeActivity",
        &json!({ "activityArn": activity_arn(region, name) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the client's activity to still resolve, got {status} with body \
         {activity}"
    );
    assert_eq!(
        activity
            .get("encryptionConfiguration")
            .and_then(|configuration| configuration.get("type"))
            .and_then(Value::as_str),
        Some("CUSTOMER_MANAGED_KMS_KEY"),
        "case {case:?}: expected the client's own encryption settings to be untouched, got \
         {activity}"
    );
}
