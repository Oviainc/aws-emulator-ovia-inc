// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::Bedrock::Guardrail` resources its template declares — in the Amazon Bedrock
//! control plane that owns them — and the identifiers it publishes for one are the ones
//! that service MINTED.
//!
//! The rule under test: **a stack creates every resource its template declares by calling
//! the OWNING service's own create API, and reports back the identifiers that service
//! assigned, so every handle the stack publishes resolves in that service**. Amazon Bedrock
//! mints a guardrail's `guardrailId` (`^[a-z0-9]+$`) and the ARN around it, which is what
//! every guardrail API takes as its `guardrailIdentifier` — a value the guardrail's NAME
//! (`^[0-9a-zA-Z-_]+$`) does not have to satisfy and in general does not — so a stack that
//! published the name would publish an identifier `GetGuardrail` refuses outright. Two
//! crates have to agree for that to hold (CloudFormation makes the call, the Bedrock
//! control plane serves it), so the suite lives here: a crate-local test can only ever see
//! one side of it.
//!
//! The rows differ along what the rule generalises over: the template spelling (JSON with
//! long-form intrinsics, YAML with the short-form tags), which return value the output
//! reads back (`Ref`, `Fn::GetAtt GuardrailId`, `Fn::GetAtt GuardrailArn`,
//! `Fn::GetAtt Version`), where the name comes from (a literal, a stack parameter), which
//! policies the declaration carries (none, a word policy, a content policy and a topic
//! policy), how many guardrails one stack declares, and whether the declaration carries the
//! type's own `Tags`.
//!
//! The deletion half and the refusal half are the same rule: deleting the stack deletes the
//! guardrails it created, and a declaration the control plane REFUSES leaves the stack
//! rolled back rather than publishing an identifier nothing created.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery percent-encoder and the XML readers every awsQuery suite in this crate
// shares, written once in `all/query_wire.rs`.
use crate::query_wire::{form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stack and its
/// guardrails belong to.
const REGION: &str = "us-east-1";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms)
/// to come due.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// The signing name the whole Bedrock family shares; the route decides which member serves
/// a request.
const BEDROCK: &str = "bedrock";

/// The ARN prefix a guardrail of this Region carries (`GuardrailArn`:
/// `arn:aws:bedrock:<region>:<account>:guardrail/<id>`).
fn guardrail_arn_prefix() -> String {
    format!("arn:aws:bedrock:{REGION}:")
}

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
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
    router.register_service(
        service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::EmulatedService::new(
            engine,
        ),
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

/// Call one CloudFormation action over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
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
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// `GetGuardrail` on one `guardrailIdentifier`, over the control plane's own restJson1 wire
/// (`GET /guardrails/{guardrailIdentifier}`).
async fn get_guardrail(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/guardrails/{identifier}"))
        .header("host", format!("bedrock.{REGION}.amazonaws.com"))
        .header("authorization", authorization(BEDROCK))
        .body(Bytes::new())
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

/// `ListGuardrails`, for the "nothing was created" half of the refusal case.
async fn list_guardrails(router: &MultiServiceRouter) -> (u16, Value) {
    let request = http::Request::builder()
        .method("GET")
        .uri("/guardrails")
        .header("host", format!("bedrock.{REGION}.amazonaws.com"))
        .header("authorization", authorization(BEDROCK))
        .body(Bytes::new())
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
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// A word policy, for a declaration that carries one.
fn word_policy() -> Value {
    json!({"WordsConfig": [{"Text": "forbidden-phrase"}]})
}

/// A content policy and a topic policy, for a declaration that carries both.
fn content_policy() -> Value {
    json!({"FiltersConfig": [{
        "Type": "VIOLENCE",
        "InputStrength": "HIGH",
        "OutputStrength": "HIGH",
    }]})
}

fn topic_policy() -> Value {
    json!({"TopicsConfig": [{
        "Name": "InvestmentAdvice",
        "Definition": "Any advice about buying or selling securities.",
        "Type": "DENY",
    }]})
}

/// What a row expects of one guardrail the stack declares.
struct Declared {
    /// The output key whose value is the guardrail's minted id.
    id_output: &'static str,
    /// The output key whose value is the guardrail's ARN, where the row publishes one.
    arn_output: Option<&'static str>,
    /// The output key whose value is the guardrail's version, where the row publishes one.
    version_output: Option<&'static str>,
    /// The name the declaration asked for, which the guardrail has to carry.
    name: &'static str,
}

/// One row: a template, the parameters the create passes, and the guardrails it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    parameters: Vec<(&'static str, &'static str)>,
    declared: Vec<Declared>,
}

fn rows() -> Vec<Row> {
    vec![
        // The issue's own stack: one guardrail named by the template, its minted id read
        // back through `Fn::GetAtt GuardrailId` and its ARN through `Ref`, in JSON with
        // long-form intrinsics.
        Row {
            case: "one guardrail, GuardrailId and Ref read back, JSON spelling",
            stack_name: "bedrock-guardrail-id-stack",
            template: json!({
                "Resources": {
                    "Guardrail": {
                        "Type": "AWS::Bedrock::Guardrail",
                        "Properties": {
                            "Name": "cfn-guardrail-one",
                            "BlockedInputMessaging": "Blocked by the stack's guardrail.",
                            "BlockedOutputsMessaging": "Blocked by the stack's guardrail.",
                            "WordPolicyConfig": word_policy(),
                        }
                    }
                },
                "Outputs": {
                    "GuardrailId": {"Value": {"Fn::GetAtt": ["Guardrail", "GuardrailId"]}},
                    "GuardrailRef": {"Value": {"Ref": "Guardrail"}},
                    "GuardrailVersion": {"Value": {"Fn::GetAtt": ["Guardrail", "Version"]}}
                }
            })
            .to_string(),
            parameters: Vec::new(),
            declared: vec![Declared {
                id_output: "GuardrailId",
                arn_output: Some("GuardrailRef"),
                version_output: Some("GuardrailVersion"),
                name: "cfn-guardrail-one",
            }],
        },
        // The same type in YAML with short-form tags, named from a stack PARAMETER, with a
        // content policy, a topic policy and the type's own `Tags`, and its ARN read back
        // through `Fn::GetAtt GuardrailArn`.
        Row {
            case: "a parameterised name, two policies and declared tags, YAML spelling",
            stack_name: "bedrock-guardrail-getatt-stack",
            template: "\
Parameters:
  GuardrailName:
    Type: String
Resources:
  Guardrail:
    Type: AWS::Bedrock::Guardrail
    Properties:
      Name: !Ref GuardrailName
      Description: The guardrail this stack deploys.
      BlockedInputMessaging: Blocked input.
      BlockedOutputsMessaging: Blocked output.
      Tags:
        - Key: team
          Value: platform
      ContentPolicyConfig:
        FiltersConfig:
          - Type: VIOLENCE
            InputStrength: HIGH
            OutputStrength: HIGH
      TopicPolicyConfig:
        TopicsConfig:
          - Name: InvestmentAdvice
            Definition: Any advice about buying or selling securities.
            Type: DENY
Outputs:
  GuardrailId:
    Value: !GetAtt Guardrail.GuardrailId
  GuardrailArn:
    Value: !GetAtt Guardrail.GuardrailArn
"
            .to_string(),
            parameters: vec![("GuardrailName", "cfn-guardrail-two_9")],
            declared: vec![Declared {
                id_output: "GuardrailId",
                arn_output: Some("GuardrailArn"),
                version_output: None,
                name: "cfn-guardrail-two_9",
            }],
        },
        // Two guardrails in one stack, one of them carrying no policy at all: each is
        // created in its own right and each publishes its OWN minted id.
        Row {
            case: "two guardrails in one stack, one with no policy",
            stack_name: "bedrock-guardrail-pair-stack",
            template: json!({
                "Resources": {
                    "First": {
                        "Type": "AWS::Bedrock::Guardrail",
                        "Properties": {
                            "Name": "cfn-guardrail-three",
                            "BlockedInputMessaging": "Blocked input.",
                            "BlockedOutputsMessaging": "Blocked output.",
                            "ContentPolicyConfig": content_policy(),
                            "TopicPolicyConfig": topic_policy(),
                        }
                    },
                    "Second": {
                        "Type": "AWS::Bedrock::Guardrail",
                        "Properties": {
                            "Name": "cfn-guardrail-four",
                            "BlockedInputMessaging": "Blocked input.",
                            "BlockedOutputsMessaging": "Blocked output.",
                        }
                    }
                },
                "Outputs": {
                    "FirstId": {"Value": {"Fn::GetAtt": ["First", "GuardrailId"]}},
                    "SecondId": {"Value": {"Fn::GetAtt": ["Second", "GuardrailId"]}}
                }
            })
            .to_string(),
            parameters: Vec::new(),
            declared: vec![
                Declared {
                    id_output: "FirstId",
                    arn_output: None,
                    version_output: None,
                    name: "cfn-guardrail-three",
                },
                Declared {
                    id_output: "SecondId",
                    arn_output: None,
                    version_output: None,
                    name: "cfn-guardrail-four",
                },
            ],
        },
    ]
}

/// Whether `id` has the form `GuardrailId` publishes: `^[a-z0-9]+$`, `@length(max: 64)`.
fn is_guardrail_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
}

#[tokio::test]
async fn a_deployed_stack_publishes_the_guardrail_identifiers_bedrock_minted() {
    for row in rows() {
        let router = build_router();
        let case = row.case;
        let body = create_and_settle(
            &router,
            case,
            row.stack_name,
            &row.template,
            &row.parameters,
        )
        .await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {body}"
        );
        let outputs = outputs_of(&body);

        for declared in &row.declared {
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
                is_guardrail_id(&id),
                "case {case:?}: expected the published GuardrailId to have the modelled \
                 ^[a-z0-9]+$ form, got {id:?}"
            );
            assert_ne!(
                id, declared.name,
                "case {case:?}: expected the minted id rather than the guardrail's name"
            );

            // The identifier the stack published is one the OWNING service resolves, and
            // the guardrail it resolves to is the one the template declared.
            let (status, reported) = get_guardrail(&router, &id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected Amazon Bedrock to resolve the published id {id:?}, \
                 got {status} with {reported}"
            );
            assert_eq!(
                reported["name"].as_str(),
                Some(declared.name),
                "case {case:?}: expected the guardrail to carry the declared name, got \
                 {reported}"
            );
            assert_eq!(
                reported["guardrailId"].as_str(),
                Some(id.as_str()),
                "case {case:?}: expected the guardrail to report the published id, got \
                 {reported}"
            );
            let arn = reported["guardrailArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let prefix = guardrail_arn_prefix();
            assert!(
                arn.starts_with(&prefix) && arn.ends_with(&format!("guardrail/{id}")),
                "case {case:?}: expected an ARN of the Bedrock namespace naming the minted \
                 id, got {arn:?}"
            );
            // The ARN is also a `guardrailIdentifier`, so it resolves the same guardrail.
            let (status, by_arn) = get_guardrail(&router, &arn.replace(':', "%3A").replace('/', "%2F")).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the minted ARN to resolve the guardrail too, got \
                 {status} with {by_arn}"
            );

            if let Some(key) = declared.arn_output {
                let published = outputs
                    .get(key)
                    .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"));
                assert_eq!(
                    published, &arn,
                    "case {case:?}: expected the published ARN to be the one Amazon Bedrock \
                     minted ({arn:?}), got {published:?}"
                );
            }
            if let Some(key) = declared.version_output {
                let published = outputs
                    .get(key)
                    .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"));
                assert_eq!(
                    published.as_str(),
                    reported["version"].as_str().unwrap_or_default(),
                    "case {case:?}: expected the published version to be the one the \
                     guardrail carries, got {published:?} against {reported}"
                );
            }
        }
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_guardrails_it_created() {
    let router = build_router();
    let case = "two guardrails deleted with their stack";
    let stack_name = "bedrock-guardrail-deleted-stack";
    let template = json!({
        "Resources": {
            "First": {
                "Type": "AWS::Bedrock::Guardrail",
                "Properties": {
                    "Name": "deleted-guardrail-one",
                    "BlockedInputMessaging": "Blocked input.",
                    "BlockedOutputsMessaging": "Blocked output.",
                }
            },
            "Second": {
                "Type": "AWS::Bedrock::Guardrail",
                "Properties": {
                    "Name": "deleted-guardrail-two",
                    "BlockedInputMessaging": "Blocked input.",
                    "BlockedOutputsMessaging": "Blocked output.",
                }
            }
        },
        "Outputs": {
            "FirstId": {"Value": {"Fn::GetAtt": ["First", "GuardrailId"]}},
            "SecondId": {"Value": {"Fn::GetAtt": ["Second", "GuardrailId"]}}
        }
    })
    .to_string();

    let body = create_and_settle(&router, case, stack_name, &template, &[]).await;
    let outputs = outputs_of(&body);
    let ids: Vec<String> = ["FirstId", "SecondId"]
        .iter()
        .map(|key| {
            outputs
                .get(*key)
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
                .clone()
        })
        .collect();
    for id in &ids {
        let (status, reported) = get_guardrail(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Amazon Bedrock to hold {id}, got {status} with \
             {reported}"
        );
    }

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    for id in &ids {
        let (status, answer) = get_guardrail(&router, id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected {id} to be gone from Amazon Bedrock, got {status} with \
             {answer}"
        );
    }
}

#[tokio::test]
async fn a_declaration_the_control_plane_refuses_fails_the_stack() {
    let router = build_router();
    let case = "a Name outside the modelled pattern";
    let stack_name = "bedrock-guardrail-refused-stack";
    // `GuardrailName` is `^[0-9a-zA-Z-_]+$` in Amazon Bedrock's own model, so
    // `CreateGuardrail` refuses this declaration — which is what the stack must report
    // rather than completing with an identifier nothing created.
    let template = json!({
        "Resources": {
            "Guardrail": {
                "Type": "AWS::Bedrock::Guardrail",
                "Properties": {
                    "Name": "not a guardrail name",
                    "BlockedInputMessaging": "Blocked input.",
                    "BlockedOutputsMessaging": "Blocked output.",
                }
            }
        },
        "Outputs": {"GuardrailId": {"Value": {"Fn::GetAtt": ["Guardrail", "GuardrailId"]}}}
    })
    .to_string();

    let body = create_and_settle(&router, case, stack_name, &template, &[]).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back rather than publish an \
         identifier Amazon Bedrock never created, got {body}"
    );
    assert!(
        outputs_of(&body).is_empty(),
        "case {case:?}: expected no output for a resource nothing created, got {body}"
    );

    // And Amazon Bedrock holds no guardrail at all: nothing was created.
    let (status, listed) = list_guardrails(&router).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListGuardrails to answer, got {status} with {listed}"
    );
    let count = listed["guardrails"].as_array().map(Vec::len).unwrap_or_default();
    assert_eq!(
        count, 0,
        "case {case:?}: expected Amazon Bedrock to hold no guardrail, got {listed}"
    );
}
