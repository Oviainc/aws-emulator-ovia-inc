// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::CertificateManager::AcmeEndpoint` really
//! creates an ACME endpoint in AWS Certificate Manager, and the identifier the stack hands
//! back is the one ACM answers to.
//!
//! The rule under test: **a resource type CloudFormation publishes may be declared in a
//! template, and a declared resource of a type this emulator provisions is created through
//! the OWNING service's own API — so the stack's physical resource id is the identifier
//! that service assigned, the service's own read resolves it to the resource the template
//! configured, and deleting the stack deletes it there.** Two crates have to agree for that
//! to hold (CloudFormation makes the call; ACM serves it and mints the ARN), so the suite
//! lives here rather than in either crate.
//!
//! The rows differ along what the rule generalises over: the `Contact` the template sets
//! (both values the model declares, plus a template that sets none so ACM's own default
//! applies), whether the template carries `Tags`, whether it is written in JSON or YAML,
//! the Region it deploys in, and which intrinsic the outputs read the resource's identity
//! with (`Ref`, `Fn::GetAtt … AcmeEndpointArn`, `Fn::GetAtt … Arn`).
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the endpoint ARN itself, which is ACM's to mint: what
//! is checked about it is that ONE value appears everywhere the stack reports the
//! resource's identity, that it has the form `AcmeEndpointArn`'s own `@pattern` requires,
//! and that ACM's `DescribeAcmeEndpoint` resolves it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's own clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) and
/// for the `CREATING` -> `ACTIVE` transition ACM owes a new ACME endpoint
/// (`ACME_ENDPOINT_ACTIVATION_DELAY`, 500 ms) to come due.
const PAST_BOTH_SPANS: Duration = Duration::from_millis(900);

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The `AcmeEndpointStatus` of an endpoint that is serving.
const ACTIVE: &str = "ACTIVE";

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
        service_com_amazonaws_acm_certificatemanager::EmulatedService::new(engine),
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

/// Call one ACM operation over its awsJson1_1 wire, signed for `region`.
async fn acm(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("acm.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("CertificateManager.{operation}"))
        .header("authorization", authorization("acm", region))
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

/// Whether `arn` satisfies `AcmeEndpointArn`'s modelled `@pattern`
/// (`^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+$`) and its
/// modelled length (1..200) for `region` and the account this emulator mints.
fn is_well_formed_endpoint_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:acm:{region}:{ACCOUNT}:acme-endpoint/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        return false;
    };
    arn.len() <= 200
        && !id.is_empty()
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// One row of the table: a template, and what the rule says the ACME endpoint it declares
/// must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the endpoint is created in.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The `Contact` `DescribeAcmeEndpoint` must report, which the template's property
    /// decides — or ACM's own modelled default for a template that sets none.
    expected_contact: &'static str,
    /// The `AuthorizationBehavior` the template declares.
    expected_behavior: &'static str,
    /// The tags `ListTagsForResource` must report for the endpoint, which the template's
    /// `Tags` decide.
    expected_tags: &'static [(&'static str, &'static str)],
    /// The output keys whose value must be the ARN ACM minted, each written with a
    /// different intrinsic over the same resource.
    identity_outputs: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a PRE_APPROVED endpoint, contact NOT_REQUIRED, tagged, JSON, Ref and GetAtt",
        region: "us-west-2",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "an ACM ACME endpoint",
                "Resources": {
                    "Endpoint": {
                        "Type": "AWS::CertificateManager::AcmeEndpoint",
                        "Properties": {
                            "AuthorizationBehavior": "PRE_APPROVED",
                            "Contact": "NOT_REQUIRED",
                            "CertificateAuthority": { "PublicCertificateAuthority": {} },
                            "Tags": [
                                { "Key": "Owner", "Value": "platform" },
                                { "Key": "Tier", "Value": "prod" }
                            ]
                        }
                    }
                },
                "Outputs": {
                    "EndpointRef": { "Value": { "Ref": "Endpoint" } },
                    "EndpointArn": {
                        "Value": { "Fn::GetAtt": ["Endpoint", "AcmeEndpointArn"] }
                    },
                    "EndpointConventionalArn": {
                        "Value": { "Fn::GetAtt": ["Endpoint", "Arn"] }
                    }
                }
            })
            .to_string()
        },
        expected_contact: "NOT_REQUIRED",
        expected_behavior: "PRE_APPROVED",
        // `TagKey`'s pattern admits no digits, so the keys here are letters.
        expected_tags: &[("Owner", "platform"), ("Tier", "prod")],
        identity_outputs: &["EndpointRef", "EndpointArn", "EndpointConventionalArn"],
    },
    Row {
        case: "contact REQUIRED, untagged, YAML, Ref only",
        region: "eu-west-1",
        template: || {
            [
                "Resources:",
                "  Endpoint:",
                "    Type: AWS::CertificateManager::AcmeEndpoint",
                "    Properties:",
                "      AuthorizationBehavior: PRE_APPROVED",
                "      Contact: REQUIRED",
                "      CertificateAuthority:",
                "        PublicCertificateAuthority: {}",
                "Outputs:",
                "  EndpointRef:",
                "    Value: !Ref Endpoint",
                "",
            ]
            .join("\n")
        },
        expected_contact: "REQUIRED",
        expected_behavior: "PRE_APPROVED",
        expected_tags: &[],
        identity_outputs: &["EndpointRef"],
    },
    Row {
        case: "no Contact declared, so ACM's own default applies",
        region: "ap-southeast-2",
        template: || {
            json!({
                "Resources": {
                    "Endpoint": {
                        "Type": "AWS::CertificateManager::AcmeEndpoint",
                        "Properties": {
                            "AuthorizationBehavior": "PRE_APPROVED",
                            "CertificateAuthority": { "PublicCertificateAuthority": {} }
                        }
                    }
                },
                "Outputs": {
                    "EndpointRef": { "Value": { "Ref": "Endpoint" } },
                    "EndpointArn": {
                        "Value": { "Fn::GetAtt": ["Endpoint", "AcmeEndpointArn"] }
                    }
                }
            })
            .to_string()
        },
        // `CreateAcmeEndpointRequest$Contact` carries the modelled default `REQUIRED`.
        expected_contact: "REQUIRED",
        expected_behavior: "PRE_APPROVED",
        expected_tags: &[],
        identity_outputs: &["EndpointRef", "EndpointArn"],
    },
];

/// Every row: a template declaring the type is accepted, the id the stack reports for its
/// `AWS::CertificateManager::AcmeEndpoint` is the ARN ACM minted, `DescribeAcmeEndpoint`
/// resolves it to the endpoint the template configured, and deleting the stack deletes it.
#[tokio::test]
async fn a_stack_provisions_the_acm_acme_endpoint_its_template_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("acme-endpoint-stack-{index}");
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
        tokio::time::sleep(PAST_BOTH_SPANS).await;
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
        let endpoint_arn = opt_element(&resources, "PhysicalResourceId").unwrap_or_else(|| {
            panic!("case {case:?}: no <PhysicalResourceId> in {resources}")
        });
        assert!(
            is_well_formed_endpoint_arn(&endpoint_arn, region),
            "case {case:?}: expected an ACM ACME endpoint ARN in {region}, got {endpoint_arn:?}"
        );

        let outputs = outputs_of(&described);
        for key in row.identity_outputs {
            assert_eq!(
                outputs.get(*key),
                Some(&endpoint_arn),
                "case {case:?}: expected output {key:?} to be the endpoint ARN \
                 {endpoint_arn:?}, got {:?}",
                outputs.get(*key)
            );
        }

        // And ACM itself resolves that ARN to the endpoint the template configured.
        let (status, answer) = acm(
            &router,
            region,
            "DescribeAcmeEndpoint",
            &json!({ "AcmeEndpointArn": endpoint_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeAcmeEndpoint on the ARN the stack \
             reported, got {status} with body {answer}"
        );
        let endpoint = &answer["AcmeEndpoint"];
        assert_eq!(
            endpoint["AcmeEndpointArn"].as_str(),
            Some(endpoint_arn.as_str()),
            "case {case:?}: expected the endpoint to report its own ARN, got {answer}"
        );
        assert_eq!(
            endpoint["Status"].as_str(),
            Some(ACTIVE),
            "case {case:?}: expected the endpoint to be {ACTIVE} once the stack completed, got \
             {answer}"
        );
        assert_eq!(
            endpoint["AuthorizationBehavior"].as_str(),
            Some(row.expected_behavior),
            "case {case:?}: expected AuthorizationBehavior {:?}, got {answer}",
            row.expected_behavior
        );
        assert_eq!(
            endpoint["Contact"].as_str(),
            Some(row.expected_contact),
            "case {case:?}: expected Contact {:?}, got {answer}",
            row.expected_contact
        );
        assert!(
            endpoint["CertificateAuthority"]["PublicCertificateAuthority"].is_object(),
            "case {case:?}: expected the declared certificate authority, got {answer}"
        );

        // The tags the template declared reached ACM through its own tagging surface. The
        // three `aws:cloudformation:*` tags CloudFormation records are NOT among them: ACM
        // refuses a reserved tag key to any caller, so they are left off rather than sent
        // to be refused.
        let (status, tags) = acm(
            &router,
            region,
            "ListTagsForResource",
            &json!({ "ResourceArn": endpoint_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {status} with body \
             {tags}"
        );
        let reported: BTreeMap<String, String> = tags["Tags"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|tag| {
                (
                    tag["Key"].as_str().unwrap_or_default().to_string(),
                    tag["Value"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let expected: BTreeMap<String, String> = row
            .expected_tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly the tags the template declared, got {tags}"
        );

        // Deleting the stack deletes the endpoint ACM holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = acm(
            &router,
            region,
            "DescribeAcmeEndpoint",
            &json!({ "AcmeEndpointArn": endpoint_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the endpoint to be gone once the stack deleted it, got \
             {status} with body {gone}"
        );
        assert_eq!(
            gone["__type"]
                .as_str()
                .unwrap_or_default()
                .split('#')
                .next_back(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException for the deleted endpoint, got \
             {gone}"
        );
    }
}

/// A template that declares the type is ACCEPTED, where a template declaring a type
/// CloudFormation publishes no resource type for is still refused at submit time.
///
/// This is the other half of the rule: the catalogue is what a template may declare, so
/// adding a type must not weaken the refusal that keeps a template real CloudFormation
/// rejects from deploying here.
#[tokio::test]
async fn the_acme_endpoint_type_is_recognised_and_a_neighbouring_misspelling_is_not() {
    let router = build_router();
    let region = "us-east-1";
    let declared = |resource_type: &str| {
        json!({
            "Resources": {
                "Endpoint": {
                    "Type": resource_type,
                    "Properties": {
                        "AuthorizationBehavior": "PRE_APPROVED",
                        "CertificateAuthority": { "PublicCertificateAuthority": {} }
                    }
                }
            }
        })
        .to_string()
    };

    // (case, the type the template declares, whether CreateStack must accept it)
    let rows: &[(&str, &str, bool)] = &[
        (
            "the published type",
            "AWS::CertificateManager::AcmeEndpoint",
            true,
        ),
        (
            "the type under a namespace that publishes no resource types",
            "AWS::Acme::Endpoint",
            false,
        ),
        (
            "a misspelling of the published type",
            "AWS::CertificateManager::ACMEEndpoint",
            false,
        ),
    ];

    for (index, (case, resource_type, accepted)) in rows.iter().enumerate() {
        let stack_name = format!("acme-type-stack-{index}");
        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &declared(resource_type)),
            ],
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected {resource_type} to be accepted, got {status} with body \
                 {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected {resource_type} to be refused, got {status} with body {body}"
        );
        assert_eq!(
            opt_element(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected the ValidationError code, got {body}"
        );
        let message = opt_element(&body, "Message").unwrap_or_default();
        assert!(
            message.contains("Unrecognized resource types") && message.contains(resource_type),
            "case {case:?}: expected the unrecognised-type refusal naming {resource_type}, got \
             {message:?}"
        );
    }
}
