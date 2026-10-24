// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::GuardDuty::Detector` really creates a
//! detector in Amazon GuardDuty, and the id the stack hands back is the one GuardDuty
//! answers to.
//!
//! The rule under test: **a stack's physical resource id is the identifier the service
//! that owns the resource assigned — for a type whose identifier that service MINTS, the
//! one its create answered with — so `Ref` and `Fn::GetAtt` resolve to a value the owning
//! service's own read resolves to the resource the template configured.** Two crates have
//! to agree for that to hold (CloudFormation makes the call; GuardDuty serves it and mints
//! the id), so the suite lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: the publishing frequency the
//! template sets (each of the three the model admits, plus a template that sets none),
//! whether the detector is enabled at all, whether the template carries `Tags` and a
//! `DataSources` block, whether it is written in JSON or YAML, and which intrinsic the
//! outputs read the resource with (`Ref`, `Fn::GetAtt … Id`, `Fn::Sub`). Each row deploys
//! in its own Region, because GuardDuty allows one detector per account per Region.
//!
//! Every expected value is computed here from what the row declares, never read back out
//! of the answer being checked — except the detector id itself, which is GuardDuty's to
//! mint: what is checked about it is that ONE value appears everywhere the stack reports
//! the resource's identity, that it has the form GuardDuty mints, and that GuardDuty's own
//! `GetDetector` resolves it.

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

/// The length and alphabet of a GuardDuty detector id: 32 lowercase hexadecimal
/// characters (`DETECTOR_ID_LEN` in that service's own state module).
const DETECTOR_ID_LEN: usize = 32;

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
        service_com_amazonaws_guardduty_guarddutyapiservice::EmulatedService::new(engine),
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

/// Call one GuardDuty operation over its restJson1 wire, signed for `region`.
async fn guardduty(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("guardduty.{region}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("guardduty", region))
        .body(Bytes::new())
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

/// One output whose value is COMPUTED from the detector's id: the key, and the value the
/// rule says it must carry once GuardDuty has minted that id.
type DerivedOutput = (&'static str, fn(&str) -> String);

/// One row of the table: a template, and what the rule says the detector it declares must
/// then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in. One detector per account per Region, so each row has
    /// its own — which also exercises that the stack's Region is the one the detector is
    /// created in.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The `Status` `GetDetector` must report, which the template's `Enable` decides.
    expected_status: &'static str,
    /// The `FindingPublishingFrequency` `GetDetector` must report. A template that sets
    /// none gets GuardDuty's own default, which is not the template's value to decide.
    expected_frequency: &'static str,
    /// The tags `GetDetector` must report, which the template's `Tags` decide.
    expected_tags: &'static [(&'static str, &'static str)],
    /// The output keys whose value must be the detector's id, each written with a
    /// different intrinsic over the same resource.
    detector_id_outputs: &'static [&'static str],
    /// Other outputs the stack must report, as `(key, value computed from the detector
    /// id)`.
    derived_outputs: &'static [DerivedOutput],
    /// The `S3Logs` status `GetDetector`'s deprecated `dataSources` view must report, for
    /// a row that declares one.
    expected_s3_logs: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "an enabled detector, SIX_HOURS, tagged, JSON, Ref and Fn::GetAtt Id",
        region: "us-east-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a GuardDuty detector",
                "Resources": {
                    "Detector": {
                        "Type": "AWS::GuardDuty::Detector",
                        "Properties": {
                            "Enable": true,
                            "FindingPublishingFrequency": "SIX_HOURS",
                            "Tags": [
                                { "Key": "Owner", "Value": "security" },
                                { "Key": "Tier", "Value": "prod" }
                            ]
                        }
                    }
                },
                "Outputs": {
                    "DetectorId": { "Value": { "Ref": "Detector" } },
                    "DetectorIdAttribute": { "Value": { "Fn::GetAtt": ["Detector", "Id"] } }
                }
            })
            .to_string()
        },
        expected_status: "ENABLED",
        expected_frequency: "SIX_HOURS",
        // `TagKey`'s pattern admits no digits (`^(?!aws:)[a-zA-Z+-=._:/]+$`), so the keys
        // here are letters.
        expected_tags: &[("Owner", "security"), ("Tier", "prod")],
        detector_id_outputs: &["DetectorId", "DetectorIdAttribute"],
        derived_outputs: &[],
        expected_s3_logs: None,
    },
    Row {
        case: "an enabled detector, FIFTEEN_MINUTES, untagged, YAML, Ref only",
        region: "us-west-2",
        template: || {
            [
                "Resources:",
                "  Detector:",
                "    Type: AWS::GuardDuty::Detector",
                "    Properties:",
                "      Enable: true",
                "      FindingPublishingFrequency: FIFTEEN_MINUTES",
                "Outputs:",
                "  DetectorId:",
                "    Value: !Ref Detector",
                "",
            ]
            .join("\n")
        },
        expected_status: "ENABLED",
        expected_frequency: "FIFTEEN_MINUTES",
        expected_tags: &[],
        detector_id_outputs: &["DetectorId"],
        derived_outputs: &[],
        expected_s3_logs: None,
    },
    Row {
        case: "an enabled detector, ONE_HOUR, a DataSources block, an Fn::Sub output",
        region: "eu-west-1",
        template: || {
            json!({
                "Resources": {
                    "Detector": {
                        "Type": "AWS::GuardDuty::Detector",
                        "Properties": {
                            "Enable": true,
                            "FindingPublishingFrequency": "ONE_HOUR",
                            "DataSources": {
                                "S3Logs": { "Enable": false },
                                "Kubernetes": { "AuditLogs": { "Enable": true } }
                            }
                        }
                    }
                },
                "Outputs": {
                    "DetectorId": { "Value": { "Ref": "Detector" } },
                    "DetectorArn": {
                        "Value": {
                            "Fn::Sub": [
                                "arn:aws:guardduty:${AWS::Region}:${AWS::AccountId}:detector/${Id}",
                                { "Id": { "Ref": "Detector" } }
                            ]
                        }
                    }
                }
            })
            .to_string()
        },
        expected_status: "ENABLED",
        expected_frequency: "ONE_HOUR",
        expected_tags: &[],
        detector_id_outputs: &["DetectorId"],
        derived_outputs: &[(
            "DetectorArn",
            |id| format!("arn:aws:guardduty:eu-west-1:000000000000:detector/{id}"),
        )],
        // The template switched S3 data events OFF, which the deprecated data-source view
        // reports off the feature that governs it.
        expected_s3_logs: Some("DISABLED"),
    },
    Row {
        case: "a detector the template leaves disabled, no frequency declared",
        region: "ap-northeast-1",
        template: || {
            [
                "Resources:",
                "  Detector:",
                "    Type: AWS::GuardDuty::Detector",
                "    Properties:",
                "      Enable: false",
                "Outputs:",
                "  DetectorId:",
                "    Value: !GetAtt Detector.Id",
                "",
            ]
            .join("\n")
        },
        expected_status: "DISABLED",
        // Not the template's value to decide: GuardDuty's own default for a create that
        // names no frequency.
        expected_frequency: "SIX_HOURS",
        expected_tags: &[],
        detector_id_outputs: &["DetectorId"],
        derived_outputs: &[],
        expected_s3_logs: None,
    },
];

/// Every row: the id the stack reports for its `AWS::GuardDuty::Detector` is the id
/// GuardDuty minted, `GetDetector` resolves it to the detector the template configured,
/// and deleting the stack deletes it there.
#[tokio::test]
async fn a_stack_provisions_the_guardduty_detector_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("guardduty-stack-{index}");
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

        // The physical resource id of the declared resource: what a client reads out of
        // DescribeStackResources, and what every identity output must carry.
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
        let detector_id = opt_element(&resources, "PhysicalResourceId").unwrap_or_else(|| {
            panic!("case {case:?}: no <PhysicalResourceId> in {resources}")
        });
        assert_eq!(
            detector_id.len(),
            DETECTOR_ID_LEN,
            "case {case:?}: expected a {DETECTOR_ID_LEN}-character GuardDuty detector id, got \
             {detector_id:?}"
        );
        assert!(
            detector_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "case {case:?}: expected a lowercase hexadecimal detector id, got {detector_id:?}"
        );

        let outputs = outputs_of(&described);
        for key in row.detector_id_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&detector_id),
                "case {case:?}: expected output {key:?} to be the detector id \
                 {detector_id:?}, got {:?}",
                outputs.get(*key)
            );
        }
        for (key, expected) in row.derived_outputs {
            let expected = expected(&detector_id);
            assert_eq!(
                outputs.get(*key),
                Some(&expected),
                "case {case:?}: expected output {key:?} to be {expected:?}, got {:?}",
                outputs.get(*key)
            );
        }

        // And GuardDuty itself resolves that id to the detector the template configured.
        let (status, detector) =
            guardduty(&router, region, "GET", &format!("/detector/{detector_id}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetDetector on the id the stack reported, \
             got {status} with body {detector}"
        );
        assert_eq!(
            detector.get("status").and_then(Value::as_str),
            Some(row.expected_status),
            "case {case:?}: expected status {:?}, got {detector}",
            row.expected_status
        );
        assert_eq!(
            detector
                .get("findingPublishingFrequency")
                .and_then(Value::as_str),
            Some(row.expected_frequency),
            "case {case:?}: expected findingPublishingFrequency {:?}, got {detector}",
            row.expected_frequency
        );
        assert!(
            detector
                .get("serviceRole")
                .and_then(Value::as_str)
                .is_some_and(|role| role.starts_with("arn:aws:iam::")),
            "case {case:?}: expected the detector's required serviceRole, got {detector}"
        );
        let reported_tags = detector.get("tags").cloned().unwrap_or(Value::Null);
        for (key, value) in row.expected_tags {
            assert_eq!(
                reported_tags.get(*key).and_then(Value::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported_tags}"
            );
        }
        assert_eq!(
            reported_tags.as_object().map(serde_json::Map::len),
            Some(row.expected_tags.len()),
            "case {case:?}: expected exactly the {} tag(s) the template declared, got \
             {reported_tags}",
            row.expected_tags.len()
        );
        if let Some(expected) = row.expected_s3_logs {
            assert_eq!(
                detector
                    .get("dataSources")
                    .and_then(|sources| sources.get("s3Logs"))
                    .and_then(|s3| s3.get("status"))
                    .and_then(Value::as_str),
                Some(expected),
                "case {case:?}: expected dataSources.s3Logs.status {expected:?}, got {detector}"
            );
        }

        // Deleting the stack deletes the detector GuardDuty holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) =
            guardduty(&router, region, "GET", &format!("/detector/{detector_id}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from GetDetector once the stack deleted the \
             detector, got {status} with body {gone}"
        );
    }
}

/// A second stack in a Region that already has a detector is NOT reported as created:
/// GuardDuty allows one detector per account per Region, and its refusal is what the stack
/// carries.
///
/// The rule this pins is the other half of the one above — nothing is passed off as
/// created when the owning service refused to create it — and it is where the detector id
/// the FIRST stack reported must still resolve afterwards.
#[tokio::test]
async fn a_second_detector_in_one_region_fails_the_stack() {
    let router = build_router();
    let case = "two stacks declaring a detector in one Region";
    let region = "us-east-2";
    let template = json!({
        "Resources": { "Detector": { "Type": "AWS::GuardDuty::Detector",
                                     "Properties": { "Enable": true } } },
        "Outputs": { "DetectorId": { "Value": { "Ref": "Detector" } } }
    })
    .to_string();

    for stack_name in ["detector-first", "detector-second"] {
        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[("StackName", stack_name), ("TemplateBody", &template)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack for {stack_name:?}, got {status} \
             with body {body}"
        );
    }
    tokio::time::sleep(PAST_STACK_SPAN).await;

    let (_, first) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", "detector-first")]).await;
    assert_eq!(
        opt_element(&first, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the first stack to complete, got {first}"
    );
    let detector_id = outputs_of(&first)
        .get("DetectorId")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no DetectorId output in {first}"));

    let (_, second) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", "detector-second")])
            .await;
    assert_eq!(
        opt_element(&second, "StackStatus").as_deref(),
        Some("CREATE_FAILED"),
        "case {case:?}: expected the second stack to fail, got {second}"
    );
    let reason = opt_element(&second, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Detector"),
        "case {case:?}: expected the failure to name the resource, got {reason:?}"
    );

    // The detector the first stack created is untouched by the second stack's failure.
    let (status, detector) =
        guardduty(&router, region, "GET", &format!("/detector/{detector_id}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the first stack's detector to still resolve, got {status} with \
         body {detector}"
    );
}

/// Re-deploying a detector stack is a NO-OP, and an update that changes the detector's
/// configuration is refused rather than silently dropped.
///
/// This is the identity half of the rule: a type whose identifier the owning service mints
/// has no identifier the template can recompute, so what decides whether a deployment keeps
/// the resource is its logical id, its type and its PROPERTIES — never a value
/// CloudFormation invented. The cases are the four things an update of such a stack can be:
/// the body already in force (refused before anything deploys), the same declaration
/// written differently so the deployment really runs (a YAML spelling, and the same
/// declaration under a new `Description`), and a template that really changes the
/// configuration.
#[tokio::test]
async fn re_deploying_a_detector_stack_keeps_the_detector_it_already_created() {
    let router = build_router();
    let region = "eu-central-1";
    let stack_name = "redeployed-detector";
    let json_body = json!({
        "Resources": {
            "Detector": {
                "Type": "AWS::GuardDuty::Detector",
                "Properties": { "Enable": true, "FindingPublishingFrequency": "ONE_HOUR" }
            }
        },
        "Outputs": { "DetectorId": { "Value": { "Ref": "Detector" } } }
    })
    .to_string();

    let (status, body) = cloudformation(
        &router,
        region,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", &json_body)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the first deployment\": expected HTTP 200 from CreateStack, got {status} with \
         body {body}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (_, created) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    let detector_id = outputs_of(&created)
        .get("DetectorId")
        .cloned()
        .unwrap_or_else(|| panic!("case \"the first deployment\": no DetectorId output in {created}"));

    // An update whose body is byte-for-byte the one in force is refused before anything is
    // deployed — "No updates are to be performed." — which is CloudFormation's own answer
    // and leaves the detector exactly as it is.
    let case = "an update with the body already in force";
    let (status, answer) = cloudformation(
        &router,
        region,
        "UpdateStack",
        &[("StackName", stack_name), ("TemplateBody", &json_body)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from UpdateStack, got {status} with body {answer}"
    );
    assert!(
        answer.contains("No updates are to be performed"),
        "case {case:?}: expected CloudFormation's no-update refusal, got {answer}"
    );

    // Each row below is the SAME declaration written differently, so the body is new and
    // the deployment really runs — and every one of them must still be a re-deploy the
    // owning service is not asked to create a second time.
    let unchanged: &[(&str, String)] = &[
        (
            "the same declaration in YAML",
            [
                "Resources:",
                "  Detector:",
                "    Type: AWS::GuardDuty::Detector",
                "    Properties:",
                "      Enable: true",
                "      FindingPublishingFrequency: ONE_HOUR",
                "Outputs:",
                "  DetectorId:",
                "    Value: !Ref Detector",
                "",
            ]
            .join("\n"),
        ),
        (
            "the same declaration with a Description added",
            json!({
                "Description": "the same detector, described",
                "Resources": {
                    "Detector": {
                        "Type": "AWS::GuardDuty::Detector",
                        "Properties": {
                            "Enable": true,
                            "FindingPublishingFrequency": "ONE_HOUR"
                        }
                    }
                },
                "Outputs": { "DetectorId": { "Value": { "Ref": "Detector" } } }
            })
            .to_string(),
        ),
    ];
    for (case, body) in unchanged {
        let (status, answer) = cloudformation(
            &router,
            region,
            "UpdateStack",
            &[("StackName", stack_name), ("TemplateBody", body)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {answer}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (_, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("UPDATE_COMPLETE"),
            "case {case:?}: expected the re-deploy to complete, got {described}"
        );
        assert_eq!(
            outputs_of(&described).get("DetectorId"),
            Some(&detector_id),
            "case {case:?}: expected the re-deploy to keep detector {detector_id:?}, got \
             {described}"
        );
        // One resource record for the one logical resource, carrying the minted id — not a
        // second record under a recomputed one.
        let (_, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", stack_name)],
        )
        .await;
        assert_eq!(
            elements(&resources, "PhysicalResourceId"),
            vec![detector_id.clone()],
            "case {case:?}: expected exactly one resource record carrying the minted id, got \
             {resources}"
        );
        let (status, detector) =
            guardduty(&router, region, "GET", &format!("/detector/{detector_id}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the detector to still resolve after the re-deploy, got \
             {status} with body {detector}"
        );
        assert_eq!(
            detector
                .get("findingPublishingFrequency")
                .and_then(Value::as_str),
            Some("ONE_HOUR"),
            "case {case:?}: expected the detector to keep its configuration, got {detector}"
        );
    }

    // An update that really changes the configuration is a different resource state, and
    // this emulator does not change one in place: the stack fails and names the resource
    // rather than reporting a change nothing applied.
    let case = "an update that changes the publishing frequency";
    let changed = json!({
        "Resources": {
            "Detector": {
                "Type": "AWS::GuardDuty::Detector",
                "Properties": { "Enable": true, "FindingPublishingFrequency": "SIX_HOURS" }
            }
        },
        "Outputs": { "DetectorId": { "Value": { "Ref": "Detector" } } }
    })
    .to_string();
    let (status, answer) = cloudformation(
        &router,
        region,
        "UpdateStack",
        &[("StackName", stack_name), ("TemplateBody", &changed)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {answer}"
    );
    tokio::time::sleep(PAST_STACK_SPAN).await;
    let (_, described) =
        cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        opt_element(&described, "StackStatus").as_deref(),
        Some("UPDATE_FAILED"),
        "case {case:?}: expected the stack to fail rather than report a change nothing \
         applied, got {described}"
    );
    let (status, detector) =
        guardduty(&router, region, "GET", &format!("/detector/{detector_id}")).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the detector to survive the failed update, got {status} with \
         body {detector}"
    );
    assert_eq!(
        detector
            .get("findingPublishingFrequency")
            .and_then(Value::as_str),
        Some("ONE_HOUR"),
        "case {case:?}: expected the detector to keep the configuration it has, got {detector}"
    );
}
