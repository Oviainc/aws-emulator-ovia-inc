// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::BedrockAgentCore::WorkloadIdentity` resources its template declares — in the
//! Amazon Bedrock AgentCore control plane that owns them.
//!
//! The rule under test: **a stack creates every resource its template declares by calling
//! the OWNING service's own create API, so the handle the stack publishes for a resource
//! resolves in that service**. A workload identity is addressed by its NAME ("Ref returns
//! the name"), so the name a deployed stack publishes has to be one
//! `GetWorkloadIdentity` resolves, rather than the control plane answering
//! `ResourceNotFoundException` for an identity its own stack just reported as created.
//! Two crates have to agree for that to hold (CloudFormation makes the call, the AgentCore
//! control plane serves it), so the suite lives here: a crate-local test can only ever see
//! one side of it.
//!
//! The rows differ along what the rule generalises over: the template spelling (JSON with
//! long-form intrinsics, YAML with the short-form tags), how the output reads the resource
//! back (`Ref`, `Fn::GetAtt Name`, `Fn::GetAtt WorkloadIdentityArn`), where the name comes
//! from (a literal, a stack parameter, and no `Name` property at all — the one
//! CloudFormation generates), how many identities one stack declares, how many allowed
//! OAuth2 return URLs the declaration carries (none, one, two), and whether it carries the
//! type's own `Tags`.
//!
//! The deletion half, the update half and the refusal half are the same rule: deleting the
//! stack deletes the identities it created, an update applies a changed declaration through
//! that service's own update API, and a declaration the control plane REFUSES leaves the
//! stack rolled back rather than publishing a name nothing created.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery percent-encoder and the XML readers every awsQuery suite in this crate
// shares, written once in `all/query_wire.rs` so no suite re-derives the encoding or the
// reading its assertions depend on.
use crate::query_wire::{form_encode, opt_element, outputs_of};

/// The region every request here is signed for, and so the region the stack and its
/// workload identities belong to.
const REGION: &str = "us-east-1";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms)
/// to come due.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// The signing name the AgentCore control plane shares with the AgentCore data plane
/// (`sigv4Name: bedrock-agentcore`): the route decides which of the two serves a request.
const AGENTCORE: &str = "bedrock-agentcore";

/// The ARN namespace the control plane stamps on a workload identity
/// (`awsService.arnNamespace: bedrock-agentcore`), in the Region this suite signs for.
fn agentcore_arn_prefix() -> String {
    format!("arn:aws:bedrock-agentcore:{REGION}:")
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
        service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService::new(
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

/// Call one AgentCore control-plane operation over its own restJson1 wire: a POST to the
/// URI its `@http` binding names, under the signing name both AgentCore planes share.
async fn agentcore(router: &MultiServiceRouter, uri: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", format!("bedrock-agentcore-control.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization(AGENTCORE))
        .body(Bytes::from(body.to_string()))
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

/// `GetWorkloadIdentity` on one name.
async fn get_identity(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    agentcore(
        router,
        "/identities/GetWorkloadIdentity",
        json!({ "name": name }),
    )
    .await
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

/// What a row expects of one workload identity the stack declares.
struct Declared {
    /// The output key whose value is the identity's NAME.
    name_output: &'static str,
    /// The output key whose value is the identity's ARN, where the row publishes one.
    arn_output: Option<&'static str>,
    /// The name the identity must carry, or `None` where the template declared none and
    /// CloudFormation generated it (so only the stack can say what it is).
    name: Option<String>,
    /// The allowed OAuth2 return URLs the control plane must report for it: `None` where
    /// the declaration carried none, so the identity carries none either.
    return_urls: Option<Value>,
}

/// One row: a template, the parameters the create passes, and the identities it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    parameters: Vec<(&'static str, &'static str)>,
    declared: Vec<Declared>,
}

fn rows() -> Vec<Row> {
    vec![
        // The issue's own stack: one identity named by the template, read back through
        // `Ref`, in JSON with long-form intrinsics, declaring no return URLs.
        Row {
            case: "one identity, Ref read back, JSON spelling, no return URLs",
            stack_name: "agentcore-wi-ref-stack",
            template: json!({
                "Resources": {
                    "Identity": {
                        "Type": "AWS::BedrockAgentCore::WorkloadIdentity",
                        "Properties": { "Name": "stack-identity-one" }
                    }
                },
                "Outputs": {
                    "IdentityName": {"Value": {"Ref": "Identity"}},
                    "IdentityArn": {
                        "Value": {"Fn::GetAtt": ["Identity", "WorkloadIdentityArn"]}
                    }
                }
            })
            .to_string(),
            parameters: Vec::new(),
            declared: vec![Declared {
                name_output: "IdentityName",
                arn_output: Some("IdentityArn"),
                name: Some("stack-identity-one".to_string()),
                return_urls: None,
            }],
        },
        // The same type in YAML with short-form tags, named from a stack PARAMETER, read
        // back through `Fn::GetAtt Name`, with two allowed return URLs and the type's own
        // `Tags` — which the create carries beside the ones CloudFormation propagates. The
        // tag set itself is not read back here: `ListTagsForResource` is still one of that
        // service's scaffolds, so what this row holds to is that a declaration carrying
        // tags is created rather than refused.
        Row {
            case: "a parameterised name, two return URLs and declared tags, YAML spelling",
            stack_name: "agentcore-wi-getatt-stack",
            template: "\
Parameters:
  IdentityName:
    Type: String
Resources:
  Identity:
    Type: AWS::BedrockAgentCore::WorkloadIdentity
    Properties:
      Name: !Ref IdentityName
      Tags:
        team: platform
      AllowedResourceOauth2ReturnUrls:
        - https://app.example.com/oauth2/callback
        - https://app.example.com/oauth2/second
Outputs:
  IdentityName:
    Value: !GetAtt Identity.Name
  IdentityArn:
    Value: !GetAtt Identity.WorkloadIdentityArn
"
            .to_string(),
            parameters: vec![("IdentityName", "stack.identity_two-9")],
            declared: vec![Declared {
                name_output: "IdentityName",
                arn_output: Some("IdentityArn"),
                name: Some("stack.identity_two-9".to_string()),
                return_urls: Some(json!([
                    "https://app.example.com/oauth2/callback",
                    "https://app.example.com/oauth2/second"
                ])),
            }],
        },
        // Two identities in one stack: one the template names with a single return URL, and
        // one it does NOT name at all — whose physical name CloudFormation generates, and
        // which the owning service has to hold under exactly that generated name.
        Row {
            case: "two identities, one named with one return URL and one CloudFormation names",
            stack_name: "agentcore-wi-pair-stack",
            template: json!({
                "Resources": {
                    "Named": {
                        "Type": "AWS::BedrockAgentCore::WorkloadIdentity",
                        "Properties": {
                            "Name": "stack-identity-three",
                            "AllowedResourceOauth2ReturnUrls": [
                                "https://app.example.com/oauth2/only"
                            ]
                        }
                    },
                    "Unnamed": {
                        "Type": "AWS::BedrockAgentCore::WorkloadIdentity",
                        "Properties": {}
                    }
                },
                "Outputs": {
                    "NamedName": {"Value": {"Ref": "Named"}},
                    "UnnamedName": {"Value": {"Fn::GetAtt": ["Unnamed", "Name"]}}
                }
            })
            .to_string(),
            parameters: Vec::new(),
            declared: vec![
                Declared {
                    name_output: "NamedName",
                    arn_output: None,
                    name: Some("stack-identity-three".to_string()),
                    return_urls: Some(json!(["https://app.example.com/oauth2/only"])),
                },
                Declared {
                    name_output: "UnnamedName",
                    arn_output: None,
                    name: None,
                    return_urls: None,
                },
            ],
        },
    ]
}

#[tokio::test]
async fn every_declared_workload_identity_is_created_in_the_agentcore_control_plane() {
    let router = build_router();

    for row in rows() {
        let body = create_and_settle(
            &router,
            row.case,
            row.stack_name,
            &row.template,
            &row.parameters,
        )
        .await;
        assert_eq!(
            opt_element(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {:?}: expected the stack to complete, got {body}",
            row.case
        );
        let outputs = outputs_of(&body);
        let mut seen: Vec<String> = Vec::new();

        for declared in &row.declared {
            let case = format!("{} [{}]", row.case, declared.name_output);
            let name = outputs
                .get(declared.name_output)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no {} output in {body}", declared.name_output)
                })
                .clone();
            if let Some(expected) = &declared.name {
                assert_eq!(
                    &name, expected,
                    "case {case:?}: expected the declared name, got {name:?}"
                );
            }
            assert!(
                !seen.contains(&name),
                "case {case:?}: expected each declared resource to carry its own name, \
                 got {name:?} twice"
            );
            seen.push(name.clone());

            // The control plane itself resolves the name the stack published, and what it
            // holds is what the template declared.
            let (status, reported) = get_identity(&router, &name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the control plane to resolve the name the stack \
                 published, got {status} with {reported}"
            );
            assert_eq!(
                reported["name"].as_str(),
                Some(name.as_str()),
                "case {case:?}: expected the identity to report the same name, got {reported}"
            );
            let arn = reported["workloadIdentityArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let prefix = agentcore_arn_prefix();
            assert!(
                arn.starts_with(&prefix) && arn.ends_with(&name),
                "case {case:?}: expected an ARN in the AgentCore namespace naming the \
                 identity, got {arn:?}"
            );
            match &declared.return_urls {
                Some(expected) => assert_eq!(
                    &reported["allowedResourceOauth2ReturnUrls"],
                    expected,
                    "case {case:?}: expected the declared return URLs, got {reported}"
                ),
                None => assert!(
                    reported
                        .get("allowedResourceOauth2ReturnUrls")
                        .is_none_or(Value::is_null),
                    "case {case:?}: expected no return URLs for a declaration carrying \
                     none, got {reported}"
                ),
            }
            for instant in ["createdTime", "lastUpdatedTime"] {
                assert!(
                    reported[instant].is_number(),
                    "case {case:?}: expected the identity to report {instant}, got {reported}"
                );
            }

            // And the ARN the stack published, where it publishes one, is the one that
            // service minted.
            if let Some(key) = declared.arn_output {
                let published = outputs
                    .get(key)
                    .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"));
                assert_eq!(
                    published, &arn,
                    "case {case:?}: expected the published ARN to be the one the control \
                     plane minted ({arn:?}), got {published:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_workload_identities_it_created() {
    let router = build_router();
    let case = "two identities deleted with their stack";
    let stack_name = "agentcore-wi-deleted-stack";
    let template = json!({
        "Resources": {
            "First": {
                "Type": "AWS::BedrockAgentCore::WorkloadIdentity",
                "Properties": { "Name": "deleted-identity-one" }
            },
            "Second": {
                "Type": "AWS::BedrockAgentCore::WorkloadIdentity",
                "Properties": { "Name": "deleted-identity-two" }
            }
        },
        "Outputs": {
            "FirstName": {"Value": {"Ref": "First"}},
            "SecondName": {"Value": {"Ref": "Second"}}
        }
    })
    .to_string();

    let body = create_and_settle(&router, case, stack_name, &template, &[]).await;
    let outputs = outputs_of(&body);
    let names: Vec<String> = ["FirstName", "SecondName"]
        .iter()
        .map(|key| {
            outputs
                .get(*key)
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
                .clone()
        })
        .collect();
    for name in &names {
        let (status, reported) = get_identity(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the control plane to hold {name}, got {status} with \
             {reported}"
        );
    }

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    for name in &names {
        let (status, answer) = get_identity(&router, name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected {name} to be gone from the control plane, got {status} \
             with {answer}"
        );
        assert_eq!(
            answer["__type"]
                .as_str()
                .unwrap_or_default()
                .rsplit('#')
                .next(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException for {name}, got {answer}"
        );
    }
}

#[tokio::test]
async fn an_update_applies_the_changed_return_urls_to_the_identity_it_still_declares() {
    let router = build_router();
    let case = "an update that re-states one identity's return URLs and drops another";
    let stack_name = "agentcore-wi-updated-stack";
    let before = json!({
        "Resources": {
            "Kept": {
                "Type": "AWS::BedrockAgentCore::WorkloadIdentity",
                "Properties": {
                    "Name": "updated-identity",
                    "AllowedResourceOauth2ReturnUrls": ["https://before.example.com/cb"]
                }
            },
            "Dropped": {
                "Type": "AWS::BedrockAgentCore::WorkloadIdentity",
                "Properties": { "Name": "dropped-identity" }
            }
        },
        "Outputs": {
            "KeptName": {"Value": {"Ref": "Kept"}},
            "DroppedName": {"Value": {"Ref": "Dropped"}}
        }
    })
    .to_string();
    let after = json!({
        "Resources": {
            "Kept": {
                "Type": "AWS::BedrockAgentCore::WorkloadIdentity",
                "Properties": {
                    "Name": "updated-identity",
                    "AllowedResourceOauth2ReturnUrls": [
                        "https://after.example.com/first",
                        "https://after.example.com/second"
                    ]
                }
            }
        },
        "Outputs": { "KeptName": {"Value": {"Ref": "Kept"}} }
    })
    .to_string();

    let body = create_and_settle(&router, case, stack_name, &before, &[]).await;
    let outputs = outputs_of(&body);
    let kept = outputs
        .get("KeptName")
        .unwrap_or_else(|| panic!("case {case:?}: no KeptName output in {body}"))
        .clone();
    let dropped = outputs
        .get("DroppedName")
        .unwrap_or_else(|| panic!("case {case:?}: no DroppedName output in {body}"))
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
    let (status, body) =
        cloudformation(&router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "case {case:?}: expected the update to complete, got {body}"
    );

    // The identity the new template still declares is the SAME one, carrying the list the
    // update declared: that member replaces the existing list.
    let (status, reported) = get_identity(&router, &kept).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the control plane to still hold {kept}, got {status} with \
         {reported}"
    );
    assert_eq!(
        reported["allowedResourceOauth2ReturnUrls"],
        json!([
            "https://after.example.com/first",
            "https://after.example.com/second"
        ]),
        "case {case:?}: expected the updated return URLs, got {reported}"
    );

    // And the one the update dropped is gone from the control plane.
    let (status, answer) = get_identity(&router, &dropped).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the dropped identity to be deleted in the control plane, \
         got {status} with {answer}"
    );
}

#[tokio::test]
async fn a_declaration_the_control_plane_refuses_fails_the_stack() {
    let router = build_router();
    let case = "a Name shorter than the modelled minimum";
    let stack_name = "agentcore-wi-refused-stack";
    // `name` is `@length(min: 3, max: 255)` in the AgentCore control plane's own model, so
    // `CreateWorkloadIdentity` refuses this declaration — which is what the stack must
    // report rather than completing with a name nothing created.
    let template = json!({
        "Resources": {
            "Identity": {
                "Type": "AWS::BedrockAgentCore::WorkloadIdentity",
                "Properties": { "Name": "no" }
            }
        },
        "Outputs": {"IdentityName": {"Value": {"Ref": "Identity"}}}
    })
    .to_string();

    let body = create_and_settle(&router, case, stack_name, &template, &[]).await;
    assert_eq!(
        opt_element(&body, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back rather than publish a \
         name the control plane never created, got {body}"
    );
    let reason = opt_element(&body, "StackStatusReason").unwrap_or_default();
    assert!(
        reason.contains("Identity"),
        "case {case:?}: expected the reason to name the resource that failed, got {reason:?}"
    );
    assert!(
        outputs_of(&body).is_empty(),
        "case {case:?}: expected no output for a resource nothing created, got {body}"
    );

    // And the control plane holds no workload identity at all: nothing was created.
    let (status, listed) = agentcore(
        &router,
        "/identities/ListWorkloadIdentities",
        json!({}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListWorkloadIdentities to answer, got {status} with {listed}"
    );
    let count = listed["workloadIdentities"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        count, 0,
        "case {case:?}: expected the control plane to hold no workload identity, got {listed}"
    );
}
