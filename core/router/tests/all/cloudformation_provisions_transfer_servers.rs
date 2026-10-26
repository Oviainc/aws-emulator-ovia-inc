// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::Transfer::Server` really creates a
//! server in AWS Transfer Family, and the id the stack hands back is the one that
//! service answers to.
//!
//! The rule under test: **a resource of a type this emulator provisions is created in the
//! service that OWNS it, through that service's own public API, and the identifier the
//! stack publishes for it is the one that type's published return values document — for a
//! Transfer Family server, "Ref returns the Amazon Resource Name (ARN) of the server" with
//! `Fn::GetAtt … ServerId` returning "the service-assigned ID of the server that is
//! created" — so a client that reads the stack's output and calls
//! `transfer DescribeServer` with it gets the server the template declared, and can then
//! drive it with `StopServer`/`StartServer`.** Two crates have to agree for that to hold
//! (CloudFormation makes the call, Transfer Family serves it), so the suite lives here
//! rather than in either crate.
//!
//! The rows differ along what the rule generalises over: the Region, whether the template
//! is written in JSON or YAML, which optional properties the declaration carries
//! (`Protocols`, `EndpointType`, `IdentityProviderType`, `SecurityPolicyName`,
//! `PreAuthenticationLoginBanner`, `Tags`) and which intrinsic the outputs read the
//! resource with (`Ref`, `Fn::GetAtt … Arn`, `Fn::GetAtt … ServerId`).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`).
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) and
/// for the created server's own start (`SERVER_START_SPAN`, 500 ms) to come due.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The two `State` values a created server reports once its start has settled, which is
/// what a post-deploy read must see ("`State` in (OFFLINE, ONLINE)").
const SETTLED_STATES: [&str; 2] = ["OFFLINE", "ONLINE"];

/// The emulator with the two services this suite exercises, mounted on ONE engine. The
/// in-process callback CloudFormation makes signs its own request; the credential scope
/// is what routes it.
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
        service_com_amazonaws_transfer_transferservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`,
/// which is what routes the request and what each handler reads its Region from.
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

/// Call one AWS Transfer Family operation over its awsJson1_1 wire, signed for `region`.
async fn transfer(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("transfer.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TransferService.{operation}"))
        .header("authorization", authorization("transfer", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
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

/// One row of the table: a template, and what the rule says the server it declares must
/// then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the server is created in and
    /// the one its ARN carries.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The members `DescribeServer` must report for the created server, as
    /// `(member, value)` pairs the declaration asked for.
    expected: &'static [(&'static str, &'static str)],
    /// The tags `transfer ListTagsForResource` must report for the server's ARN, beside
    /// the three CloudFormation records on every resource it provisions.
    expected_tags: &'static [(&'static str, &'static str)],
}

const ROWS: &[Row] = &[
    Row {
        case: "the plain SFTP server of an IaC deployment, in YAML, read with GetAtt",
        region: "us-east-1",
        template: || {
            [
                "Resources:",
                "  Server:",
                "    Type: AWS::Transfer::Server",
                "    Properties:",
                "      Protocols:",
                "        - SFTP",
                "      EndpointType: PUBLIC",
                "      IdentityProviderType: SERVICE_MANAGED",
                "Outputs:",
                "  ServerId:",
                "    Value: !GetAtt Server.ServerId",
                "  ServerArn:",
                "    Value: !GetAtt Server.Arn",
                "  ServerRef:",
                "    Value: !Ref Server",
                "",
            ]
            .join("\n")
        },
        expected: &[
            ("Domain", "S3"),
            ("EndpointType", "PUBLIC"),
            ("IdentityProviderType", "SERVICE_MANAGED"),
        ],
        expected_tags: &[],
    },
    Row {
        case: "a tagged server naming a security policy and a banner, in JSON",
        region: "eu-west-1",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Resources": {
                    "Server": {
                        "Type": "AWS::Transfer::Server",
                        "Properties": {
                            "Protocols": ["SFTP"],
                            "EndpointType": "PUBLIC",
                            "IdentityProviderType": "SERVICE_MANAGED",
                            "SecurityPolicyName": "TransferSecurityPolicy-2024-01",
                            "PreAuthenticationLoginBanner": "authorised use only",
                            "Tags": [
                                {"Key": "Owner", "Value": "platform"},
                                {"Key": "Tier", "Value": "prod"}
                            ]
                        }
                    }
                },
                "Outputs": {
                    "ServerId": {"Value": {"Fn::GetAtt": ["Server", "ServerId"]}},
                    "ServerArn": {"Value": {"Fn::GetAtt": ["Server", "Arn"]}},
                    "ServerRef": {"Value": {"Ref": "Server"}}
                }
            })
            .to_string()
        },
        expected: &[
            ("EndpointType", "PUBLIC"),
            ("IdentityProviderType", "SERVICE_MANAGED"),
            ("SecurityPolicyName", "TransferSecurityPolicy-2024-01"),
            ("PreAuthenticationLoginBanner", "authorised use only"),
        ],
        expected_tags: &[("Owner", "platform"), ("Tier", "prod")],
    },
    Row {
        case: "a server whose template declares no property at all",
        region: "ap-northeast-1",
        template: || {
            [
                "Resources:",
                "  Server:",
                "    Type: AWS::Transfer::Server",
                "Outputs:",
                "  ServerId:",
                "    Value: !GetAtt Server.ServerId",
                "  ServerArn:",
                "    Value: !GetAtt Server.Arn",
                "  ServerRef:",
                "    Value: !Ref Server",
                "",
            ]
            .join("\n")
        },
        // Every member the model documents a default for is what the server is created
        // with, exactly as a client's own `CreateServer` would have left it.
        expected: &[
            ("Domain", "S3"),
            ("EndpointType", "PUBLIC"),
            ("IdentityProviderType", "SERVICE_MANAGED"),
        ],
        expected_tags: &[],
    },
];

/// Every row: the `ServerId` the stack publishes is a server AWS Transfer Family holds,
/// `DescribeServer` reports it as the template declared it, `StopServer`/`StartServer`
/// drive it, and deleting the stack deletes it there.
#[tokio::test]
async fn a_stack_provisions_the_transfer_family_server_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("transfer-server-stack-{index}");
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

        // The id the deployment hands a client, read exactly as a client reads it.
        let outputs = outputs_of(&described);
        let server_id = outputs
            .get("ServerId")
            .unwrap_or_else(|| panic!("case {case:?}: no ServerId output in {described}"))
            .clone();
        assert!(
            server_id.len() == 19
                && server_id.starts_with("s-")
                && server_id[2..]
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "case {case:?}: expected a ServerId matching ^s-([0-9a-f]{{17}})$, got {server_id:?}"
        );
        let arn = format!("arn:aws:transfer:{region}:{ACCOUNT}:server/{server_id}");
        for key in ["ServerArn", "ServerRef"] {
            assert_eq!(
                outputs.get(key),
                Some(&arn),
                "case {case:?}: expected output {key:?} to be the created server's ARN {arn:?}, \
                 got {:?}",
                outputs.get(key)
            );
        }

        // And Transfer Family itself resolves that id to the server the template declared.
        let (status, answer) = transfer(
            &router,
            region,
            "DescribeServer",
            &json!({"ServerId": &server_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeServer on the id the stack \
             published, got {status} with body {answer}"
        );
        let server = answer.get("Server").cloned().unwrap_or(Value::Null);
        assert_eq!(
            server.get("Arn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the server to report the ARN the stack published, got \
             {server}"
        );
        for (member, expected) in row.expected {
            assert_eq!(
                server.get(*member).and_then(Value::as_str),
                Some(*expected),
                "case {case:?}: expected {member} to be {expected:?}, got {server}"
            );
        }
        let state = server
            .get("State")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            SETTLED_STATES.contains(&state),
            "case {case:?}: expected the deployed server's State in {SETTLED_STATES:?}, got \
             {state:?}"
        );

        // The tags the declaration asked for, plus the three CloudFormation records,
        // read through Transfer Family's own tagging call.
        let (status, listed) = transfer(
            &router,
            region,
            "ListTagsForResource",
            &json!({"Arn": &arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} with body \
             {listed}"
        );
        let reported: BTreeMap<String, String> = listed
            .get("Tags")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|tag| {
                Some((
                    tag.get("Key").and_then(Value::as_str)?.to_string(),
                    tag.get("Value").and_then(Value::as_str)?.to_string(),
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
        let stack_id = opt_element(&described, "StackId")
            .unwrap_or_else(|| panic!("case {case:?}: no <StackId> in {described}"));
        for (key, value) in [
            ("aws:cloudformation:stack-name", stack_name.as_str()),
            ("aws:cloudformation:stack-id", stack_id.as_str()),
            ("aws:cloudformation:logical-id", "Server"),
        ] {
            assert_eq!(
                reported.get(key).map(String::as_str),
                Some(value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported:?}"
            );
        }

        // The deployed server is a real server: it can be stopped and started again.
        let (status, stopped) = transfer(
            &router,
            region,
            "StopServer",
            &json!({"ServerId": &server_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StopServer on the deployed server, got \
             {status} with body {stopped}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (_, offline) = transfer(
            &router,
            region,
            "DescribeServer",
            &json!({"ServerId": &server_id}),
        )
        .await;
        assert_eq!(
            offline
                .get("Server")
                .and_then(|server| server.get("State"))
                .and_then(Value::as_str),
            Some("OFFLINE"),
            "case {case:?}: expected the stopped server to settle OFFLINE, got {offline}"
        );
        let (status, started) = transfer(
            &router,
            region,
            "StartServer",
            &json!({"ServerId": &server_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StartServer on the deployed server, got \
             {status} with body {started}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (_, online) = transfer(
            &router,
            region,
            "DescribeServer",
            &json!({"ServerId": &server_id}),
        )
        .await;
        assert_eq!(
            online
                .get("Server")
                .and_then(|server| server.get("State"))
                .and_then(Value::as_str),
            Some("ONLINE"),
            "case {case:?}: expected the started server to settle ONLINE, got {online}"
        );

        // Deleting the stack deletes the server Transfer Family holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = transfer(
            &router,
            region,
            "DescribeServer",
            &json!({"ServerId": &server_id}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the deleted server to stop resolving, got {status} with \
             body {gone}"
        );
        assert!(
            gone.get("__type")
                .and_then(Value::as_str)
                .is_some_and(|code| code.ends_with("ResourceNotFoundException")),
            "case {case:?}: expected ResourceNotFoundException once the stack deleted the \
             server, got {gone}"
        );
    }
}
