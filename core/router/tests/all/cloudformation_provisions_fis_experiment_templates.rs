// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::FIS::ExperimentTemplate` resources its template declares — in AWS FIS.
//!
//! The rule under test: **a stack publishes, for every output its template declares, the
//! identifier of the resource the OWNING SERVICE created — and where that service ASSIGNS
//! the identifier, the published value is the one it minted.** An experiment template's id
//! is FIS's to mint (`Ref` of the type returns it, and the type has no name property), so
//! the `TemplateId` output of a deployed stack has to carry an id FIS itself resolves:
//! `GetExperimentTemplate` and `StartExperiment` on the stack's own output succeed, rather
//! than FIS answering `ResourceNotFoundException` for an id its own stack just published.
//! Two crates have to agree for that to hold (CloudFormation makes the call, FIS serves
//! it), so the suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: the template spelling (JSON with
//! long-form intrinsics, YAML with the short-form tags), how the output reads the resource
//! back (`Ref` and `Fn::GetAtt Id`), how many experiment templates one stack declares,
//! where a property's value comes from (a literal, and a stack parameter through `Ref`),
//! and how deep the declared properties go (a bare wait action, and a target carrying the
//! maps and the filter list the type nests).
//!
//! The deletion half, the update half and the refusal half are the same rule: deleting the
//! stack deletes the templates it created, an update keeps the id of a resource it still
//! declares and deletes the one it drops, and a declaration FIS REFUSES leaves the stack
//! `CREATE_FAILED` naming the resource rather than publishing an id nothing minted.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The percent-encoder every awsQuery suite in this crate shares, written once in
// `all/query_wire.rs` so no suite re-derives the encoding its assertions depend on.
use crate::query_wire::form_encode;

/// The region every request here is signed for, and so the region the stack and its
/// experiment templates belong to.
const REGION: &str = "us-east-1";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms)
/// to come due.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// A wait inside the 1..720 minutes FIS documents for `aws:fis:wait`.
const WAIT_DURATION: &str = "PT5M";

/// How many characters follow the `EXT` prefix of an experiment-template id FIS mints
/// (FIS's own `ID_SUFFIX_LEN`).
const TEMPLATE_ID_SUFFIX_LEN: usize = 12;

/// The emulator with the three services this suite exercises, mounted on ONE engine
/// exactly as `core/router/src/lib.rs` mounts them. IAM is there because the experiment's
/// execution role is a real role: `StartExperiment` checks its trust policy against the
/// account's IAM records.
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
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService::new(engine),
    );
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

/// Call one awsQuery operation of a service that speaks it (CloudFormation, IAM).
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
    let host = if service == "iam" {
        "iam.amazonaws.com".to_string()
    } else {
        format!("{service}.{REGION}.amazonaws.com")
    };
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// Call one FIS operation over its own restJson1 wire.
async fn fis(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("the test body serializes")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("fis.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("fis"))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Every `<tag>` text of an XML body, in document order, with the serializer's escaping
/// undone.
///
/// `query_wire::element` reads the FIRST element of a name; this suite needs every
/// `OutputKey`/`OutputValue` of the answer (and their unescaped text) to read the stack's
/// outputs as the key/value map a client sees, which is what [`outputs_of`] builds.
fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(
            after[..end]
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&amp;", "&"),
        );
        rest = &after[end + close.len()..];
    }
    found
}

fn xml_text(xml: &str, tag: &str) -> Option<String> {
    xml_texts(xml, tag).into_iter().next()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as — the
/// `{o["OutputKey"]: o["OutputValue"]}` read that raises `KeyError` on a stack reporting no
/// `Outputs` member at all.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    xml_texts(xml, "OutputKey")
        .into_iter()
        .zip(xml_texts(xml, "OutputValue"))
        .collect()
}

/// Create the IAM role the experiment templates below run as, trusting
/// `fis.amazonaws.com`, and answer its ARN.
async fn execution_role(router: &MultiServiceRouter, name: &str) -> String {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "fis.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
    .to_string();
    let (status, body) = query(
        router,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", document.as_str()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected CreateRole to persist the execution role, got {status} with {body}"
    );
    xml_text(&body, "Arn").unwrap_or_else(|| panic!("no role Arn in {body}"))
}

/// Whether `value` is an experiment-template id of the form FIS mints: `EXT` followed by
/// [`TEMPLATE_ID_SUFFIX_LEN`] uppercase base-36 characters.
fn is_experiment_template_id(value: &str) -> bool {
    value.strip_prefix("EXT").is_some_and(|suffix| {
        suffix.len() == TEMPLATE_ID_SUFFIX_LEN
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
    })
}

/// Create a stack, wait past its transition, and answer the `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
    parameters: &[(&str, &str)],
) -> String {
    let mut members = vec![("StackName", stack_name), ("TemplateBody", template)];
    let positional: Vec<(String, String)> = parameters
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            vec![
                (
                    format!("Parameters.member.{position}.ParameterKey"),
                    (*key).to_string(),
                ),
                (
                    format!("Parameters.member.{position}.ParameterValue"),
                    (*value).to_string(),
                ),
            ]
        })
        .collect();
    for (key, value) in &positional {
        members.push((key.as_str(), value.as_str()));
    }
    let (status, body) = cloudformation(router, "CreateStack", &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// What the row expects of one experiment template the stack declares.
struct Declared {
    /// The output key whose value is the template's id.
    id_output: &'static str,
    /// The logical id the template declares the resource under, which CloudFormation
    /// records on it as the `aws:cloudformation:logical-id` tag.
    logical_id: &'static str,
    /// The `description` FIS must report for it, which is what the template declared.
    description: &'static str,
    /// The `actionId` of the one action the declaration carries.
    action: (&'static str, &'static str),
    /// The target name the declaration carries, where it carries one, and what FIS must
    /// report for it.
    target: Option<(&'static str, Value)>,
    /// The tags FIS must report for it.
    tags: Value,
}

/// One row: a template, the parameters the create passes, and the templates it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    parameters: Vec<(&'static str, &'static str)>,
    declared: Vec<Declared>,
}

fn rows(role_arn: &str) -> Vec<Row> {
    vec![
        // The issue's own stack: one experiment template read back through `Ref`, in JSON
        // with long-form intrinsics.
        Row {
            case: "one experiment template, Ref read back, JSON spelling",
            stack_name: "fis-ref-stack",
            template: json!({
                "Resources": {
                    "FisTemplate": {
                        "Type": "AWS::FIS::ExperimentTemplate",
                        "Properties": {
                            "Description": "a wait experiment",
                            "RoleArn": role_arn,
                            "StopConditions": [{"Source": "none"}],
                            "Targets": {},
                            "Actions": {
                                "waitAction": {
                                    "ActionId": "aws:fis:wait",
                                    "Parameters": {"duration": WAIT_DURATION}
                                }
                            }
                        }
                    }
                },
                "Outputs": {"TemplateId": {"Value": {"Ref": "FisTemplate"}}}
            })
            .to_string(),
            parameters: Vec::new(),
            declared: vec![Declared {
                id_output: "TemplateId",
                logical_id: "FisTemplate",
                description: "a wait experiment",
                action: ("waitAction", "aws:fis:wait"),
                target: None,
                tags: json!({}),
            }],
        },
        // The same type in YAML with short-form tags, read back through `Fn::GetAtt Id`,
        // with the role coming from a stack PARAMETER, tags declared as the type's map, and
        // a target carrying the maps and the filter list the type nests.
        Row {
            case: "a parameterised role, tags and a nested target, YAML spelling",
            stack_name: "fis-getatt-stack",
            template: "\
Parameters:
  ExperimentRole:
    Type: String
Resources:
  Experiment:
    Type: AWS::FIS::ExperimentTemplate
    Properties:
      Description: an instance experiment
      RoleArn: !Ref ExperimentRole
      StopConditions:
        - Source: none
      Tags:
        team: platform
      Targets:
        Instances:
          ResourceType: aws:ec2:instance
          SelectionMode: COUNT(1)
          ResourceTags:
            Name: web
          Filters:
            - Path: State.Name
              Values:
                - running
          Parameters:
            availabilityZoneIdentifier: us-east-1a
      Actions:
        stopInstances:
          ActionId: aws:ec2:stop-instances
          Description: stop one instance
          Parameters:
            startInstancesAfterDuration: PT2M
          Targets:
            Instances: Instances
Outputs:
  TemplateId:
    Value: !GetAtt Experiment.Id
"
            .to_string(),
            parameters: vec![("ExperimentRole", "")],
            declared: vec![Declared {
                id_output: "TemplateId",
                logical_id: "Experiment",
                description: "an instance experiment",
                action: ("stopInstances", "aws:ec2:stop-instances"),
                target: Some((
                    "Instances",
                    json!({
                        "resourceType": "aws:ec2:instance",
                        "selectionMode": "COUNT(1)",
                        "resourceTags": {"Name": "web"},
                        "filters": [{"path": "State.Name", "values": ["running"]}],
                        "parameters": {"availabilityZoneIdentifier": "us-east-1a"}
                    }),
                )),
                tags: json!({"team": "platform"}),
            }],
        },
        // Two experiment templates in one stack, each with its own output: every declared
        // output carries the id of the resource it names.
        Row {
            case: "two experiment templates, one output each",
            stack_name: "fis-pair-stack",
            template: json!({
                "Resources": {
                    "First": {
                        "Type": "AWS::FIS::ExperimentTemplate",
                        "Properties": {
                            "Description": "the first experiment",
                            "RoleArn": role_arn,
                            "StopConditions": [{"Source": "none"}],
                            "Targets": {},
                            "Actions": {
                                "waitAction": {
                                    "ActionId": "aws:fis:wait",
                                    "Parameters": {"duration": WAIT_DURATION}
                                }
                            }
                        }
                    },
                    "Second": {
                        "Type": "AWS::FIS::ExperimentTemplate",
                        "Properties": {
                            "Description": "the second experiment",
                            "RoleArn": role_arn,
                            "StopConditions": [{"Source": "none"}],
                            "Targets": {},
                            "Actions": {
                                "waitAction": {
                                    "ActionId": "aws:fis:wait",
                                    "Parameters": {"duration": WAIT_DURATION}
                                }
                            }
                        }
                    }
                },
                "Outputs": {
                    "FirstId": {"Value": {"Ref": "First"}},
                    "SecondId": {"Value": {"Fn::GetAtt": ["Second", "Id"]}}
                }
            })
            .to_string(),
            parameters: Vec::new(),
            declared: vec![
                Declared {
                    id_output: "FirstId",
                    logical_id: "First",
                    description: "the first experiment",
                    action: ("waitAction", "aws:fis:wait"),
                    target: None,
                    tags: json!({}),
                },
                Declared {
                    id_output: "SecondId",
                    logical_id: "Second",
                    description: "the second experiment",
                    action: ("waitAction", "aws:fis:wait"),
                    target: None,
                    tags: json!({}),
                },
            ],
        },
    ]
}

#[tokio::test]
async fn every_declared_output_carries_the_experiment_template_id_fis_minted() {
    let router = build_router();
    let role_arn = execution_role(&router, "fis-stack-outputs").await;

    for row in rows(&role_arn) {
        // The one row whose role comes from a parameter passes the ARN that was just
        // created, so no row depends on an ARN spelled by hand.
        let parameters: Vec<(&str, &str)> = row
            .parameters
            .iter()
            .map(|(key, value)| {
                (
                    *key,
                    if value.is_empty() {
                        role_arn.as_str()
                    } else {
                        *value
                    },
                )
            })
            .collect();
        let body = create_and_settle(
            &router,
            row.case,
            row.stack_name,
            &row.template,
            &parameters,
        )
        .await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {:?}: expected the stack to complete, got {body}",
            row.case
        );
        let outputs = outputs_of(&body);
        let mut seen: Vec<String> = Vec::new();

        for declared in &row.declared {
            let case = format!("{} [{}]", row.case, declared.id_output);
            let id = outputs
                .get(declared.id_output)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: no {} output in {body}",
                        declared.id_output
                    )
                })
                .clone();
            assert!(
                is_experiment_template_id(&id),
                "case {case:?}: expected an id of the form FIS mints, got {id:?}"
            );
            assert!(
                !seen.contains(&id),
                "case {case:?}: expected each declared resource to carry its own id, \
                 got {id:?} twice"
            );
            seen.push(id.clone());

            // FIS itself resolves the id the stack published, and what it holds is what
            // the template declared.
            let (status, reported) =
                fis(&router, "GET", &format!("/experimentTemplates/{id}"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected FIS to resolve the id the stack published, \
                 got {status} with {reported}"
            );
            let template = &reported["experimentTemplate"];
            assert_eq!(
                template["id"].as_str(),
                Some(id.as_str()),
                "case {case:?}: expected FIS to report the same id, got {reported}"
            );
            assert_eq!(
                template["description"].as_str(),
                Some(declared.description),
                "case {case:?}: expected the declared description, got {reported}"
            );
            assert_eq!(
                template["roleArn"].as_str(),
                Some(role_arn.as_str()),
                "case {case:?}: expected the declared role, got {reported}"
            );
            let (action_name, action_id) = declared.action;
            assert_eq!(
                template["actions"][action_name]["actionId"].as_str(),
                Some(action_id),
                "case {case:?}: expected the declared action under its own name, \
                 got {reported}"
            );
            assert_eq!(
                template["stopConditions"][0]["source"].as_str(),
                Some("none"),
                "case {case:?}: expected the declared stop condition, got {reported}"
            );
            if let Some((target_name, expected)) = &declared.target {
                assert_eq!(
                    &template["targets"][*target_name],
                    expected,
                    "case {case:?}: expected the declared target as FIS spells it, \
                     got {reported}"
                );
            }
            // The tags the declaration asked for, PLUS the three CloudFormation records on
            // every resource it provisions: `aws:cloudformation:stack-name`,
            // `aws:cloudformation:stack-id` and `aws:cloudformation:logical-id` (these rows
            // supply no stack-level `Tags`, which would be here too). The whole set is held
            // to by
            // `services/service-com-amazonaws-cloudformation-cloudformation/tests/all/stack_tag_propagation.rs`.
            let mut expected_tags = declared.tags.as_object().cloned().unwrap_or_default();
            expected_tags.insert(
                "aws:cloudformation:stack-name".to_string(),
                json!(row.stack_name),
            );
            expected_tags.insert(
                "aws:cloudformation:stack-id".to_string(),
                json!(xml_text(&body, "StackId").unwrap_or_default()),
            );
            expected_tags.insert(
                "aws:cloudformation:logical-id".to_string(),
                json!(declared.logical_id),
            );
            assert_eq!(
                template["tags"],
                Value::Object(expected_tags),
                "case {case:?}: expected the declared tags beside the aws:cloudformation \
                 ones, got {reported}"
            );
        }
    }
}

/// The issue's own workflow: deploy the template, read the id out of the stack's outputs,
/// and start the experiment with it.
#[tokio::test]
async fn the_id_the_stack_reports_starts_an_experiment() {
    let router = build_router();
    let role_arn = execution_role(&router, "fis-start-from-stack").await;
    let case = "deploy, read the output, start the experiment";
    let template = json!({
        "Resources": {
            "FisTemplate": {
                "Type": "AWS::FIS::ExperimentTemplate",
                "Properties": {
                    "Description": "a wait experiment",
                    "RoleArn": role_arn,
                    "StopConditions": [{"Source": "none"}],
                    "Targets": {},
                    "Actions": {
                        "waitAction": {
                            "ActionId": "aws:fis:wait",
                            "Parameters": {"duration": WAIT_DURATION}
                        }
                    }
                }
            }
        },
        "Outputs": {"TemplateId": {"Value": {"Ref": "FisTemplate"}}}
    })
    .to_string();

    let body = create_and_settle(&router, case, "fis-start-stack", &template, &[]).await;
    let id = outputs_of(&body)
        .get("TemplateId")
        .unwrap_or_else(|| panic!("case {case:?}: no TemplateId output in {body}"))
        .clone();

    let (status, started) = fis(
        &router,
        "POST",
        "/experiments",
        Some(json!({
            "clientToken": "from-the-stack-output",
            "experimentTemplateId": id
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the experiment to start from the stack's own output, \
         got {status} with {started}"
    );
    assert_eq!(
        started["experiment"]["experimentTemplateId"].as_str(),
        Some(id.as_str()),
        "case {case:?}: expected the experiment to name the stack's template, got {started}"
    );
    assert_eq!(
        started["experiment"]["roleArn"].as_str(),
        Some(role_arn.as_str()),
        "case {case:?}: expected the experiment to run as the declared role, got {started}"
    );

    // Stop it again, so the suite leaves no active experiment behind.
    let experiment = started["experiment"]["id"].as_str().unwrap_or_default();
    let (status, stopped) = fis(
        &router,
        "DELETE",
        &format!("/experiments/{experiment}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the stop to be accepted, got {status} with {stopped}"
    );
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_experiment_templates_it_created() {
    let router = build_router();
    let role_arn = execution_role(&router, "fis-deleted-stack").await;
    let case = "two experiment templates deleted with their stack";
    let stack_name = "fis-deleted-stack";
    let template = json!({
        "Resources": {
            "Kept": {
                "Type": "AWS::FIS::ExperimentTemplate",
                "Properties": {
                    "Description": "the first experiment",
                    "RoleArn": role_arn,
                    "StopConditions": [{"Source": "none"}],
                    "Targets": {},
                    "Actions": {"waitAction": {
                        "ActionId": "aws:fis:wait",
                        "Parameters": {"duration": WAIT_DURATION}
                    }}
                }
            },
            "Other": {
                "Type": "AWS::FIS::ExperimentTemplate",
                "Properties": {
                    "Description": "the second experiment",
                    "RoleArn": role_arn,
                    "StopConditions": [{"Source": "none"}],
                    "Targets": {},
                    "Actions": {"waitAction": {
                        "ActionId": "aws:fis:wait",
                        "Parameters": {"duration": WAIT_DURATION}
                    }}
                }
            }
        },
        "Outputs": {
            "KeptId": {"Value": {"Ref": "Kept"}},
            "OtherId": {"Value": {"Ref": "Other"}}
        }
    })
    .to_string();

    let body = create_and_settle(&router, case, stack_name, &template, &[]).await;
    let outputs = outputs_of(&body);
    let ids: Vec<String> = ["KeptId", "OtherId"]
        .iter()
        .map(|key| {
            outputs
                .get(*key)
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
                .clone()
        })
        .collect();
    for id in &ids {
        let (status, reported) =
            fis(&router, "GET", &format!("/experimentTemplates/{id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected FIS to hold {id}, got {status} with {reported}"
        );
    }

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    for id in &ids {
        let (status, answer) =
            fis(&router, "GET", &format!("/experimentTemplates/{id}"), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the template {id} to be gone from FIS, \
             got {status} with {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException for {id}, got {answer}"
        );
    }
}

#[tokio::test]
async fn an_update_keeps_the_id_it_still_declares_and_deletes_what_it_drops() {
    let router = build_router();
    let role_arn = execution_role(&router, "fis-updated-stack").await;
    let case = "an update that drops one of two experiment templates";
    let stack_name = "fis-updated-stack";
    let experiment = |description: &str| {
        json!({
            "Type": "AWS::FIS::ExperimentTemplate",
            "Properties": {
                "Description": description,
                "RoleArn": role_arn,
                "StopConditions": [{"Source": "none"}],
                "Targets": {},
                "Actions": {"waitAction": {
                    "ActionId": "aws:fis:wait",
                    "Parameters": {"duration": WAIT_DURATION}
                }}
            }
        })
    };
    let before = json!({
        "Resources": {
            "Kept": experiment("the kept experiment"),
            "Dropped": experiment("the dropped experiment")
        },
        "Outputs": {
            "KeptId": {"Value": {"Ref": "Kept"}},
            "DroppedId": {"Value": {"Ref": "Dropped"}}
        }
    })
    .to_string();
    let after = json!({
        "Resources": { "Kept": experiment("the kept experiment") },
        "Outputs": { "KeptId": {"Value": {"Ref": "Kept"}} }
    })
    .to_string();

    let body = create_and_settle(&router, case, stack_name, &before, &[]).await;
    let outputs = outputs_of(&body);
    let kept = outputs
        .get("KeptId")
        .unwrap_or_else(|| panic!("case {case:?}: no KeptId output in {body}"))
        .clone();
    let dropped = outputs
        .get("DroppedId")
        .unwrap_or_else(|| panic!("case {case:?}: no DroppedId output in {body}"))
        .clone();

    let (status, body) = cloudformation(
        &router,
        "UpdateStack",
        &[("StackName", stack_name), ("TemplateBody", after.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) = cloudformation(&router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {body}"
    );

    // The resource the new template still declares is the SAME experiment template: its id
    // is the one FIS minted for it, which no re-deploy can recompute.
    let outputs = outputs_of(&body);
    assert_eq!(
        outputs.get("KeptId").map(String::as_str),
        Some(kept.as_str()),
        "case {case:?}: expected the kept resource to keep its minted id, got {body}"
    );
    assert!(
        !outputs.contains_key("DroppedId"),
        "case {case:?}: expected no output for the dropped resource, got {body}"
    );
    let (status, reported) = fis(
        &router,
        "GET",
        &format!("/experimentTemplates/{kept}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected FIS to still hold the kept template, \
         got {status} with {reported}"
    );

    // And the one the update dropped is gone from FIS.
    let (status, answer) = fis(
        &router,
        "GET",
        &format!("/experimentTemplates/{dropped}"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the dropped template to be deleted in FIS, \
         got {status} with {answer}"
    );
}

#[tokio::test]
async fn a_declaration_fis_refuses_fails_the_stack() {
    let router = build_router();
    let case = "a roleArn shorter than the modelled minimum";
    let stack_name = "fis-refused-stack";
    // `RoleArn` is `@length(min: 20)` in FIS's own model, so `CreateExperimentTemplate`
    // refuses this declaration — which is what the stack must report rather than
    // completing with an id nothing minted.
    let template = json!({
        "Resources": {
            "FisTemplate": {
                "Type": "AWS::FIS::ExperimentTemplate",
                "Properties": {
                    "Description": "a refused experiment",
                    "RoleArn": "arn:aws:iam::0:r",
                    "StopConditions": [{"Source": "none"}],
                    "Targets": {},
                    "Actions": {"waitAction": {
                        "ActionId": "aws:fis:wait",
                        "Parameters": {"duration": WAIT_DURATION}
                    }}
                }
            }
        },
        "Outputs": {"TemplateId": {"Value": {"Ref": "FisTemplate"}}}
    })
    .to_string();

    let body = create_and_settle(&router, case, stack_name, &template, &[]).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back rather than publish \
         an id FIS never minted, got {body}"
    );
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("FisTemplate"),
        "case {case:?}: expected the reason to name the resource that failed, got {reason:?}"
    );
    assert!(
        outputs_of(&body).is_empty(),
        "case {case:?}: expected no output for a resource nothing created, got {body}"
    );

    // And FIS holds no experiment template at all: nothing was created.
    let (status, listed) = fis(&router, "GET", "/experimentTemplates", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListExperimentTemplates to answer, got {status} with {listed}"
    );
    let count = listed["experimentTemplates"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        count, 0,
        "case {case:?}: expected FIS to hold no experiment template, got {listed}"
    );
}
