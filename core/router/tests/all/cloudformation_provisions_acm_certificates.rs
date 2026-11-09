// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::CertificateManager::Certificate` really
//! requests the certificate in AWS Certificate Manager, hands back the ARN ACM minted, and
//! does not report the resource created until ACM has issued it.
//!
//! The rules under test, both of which two crates have to agree on (CloudFormation makes the
//! calls; ACM serves them, mints the ARN and decides the certificate's status), so the suite
//! lives here rather than in either crate:
//!
//! 1. **a declared resource of a type this emulator provisions is created through the OWNING
//!    service's own API, so the stack's `PhysicalResourceId` is the identifier that service
//!    assigned** — for a type whose provider identifies the resource by its ARN that is the
//!    ARN — **and that service's own read resolves it to the resource the template
//!    configured.**
//! 2. **CloudFormation does not report a resource created while the service that owns it
//!    still reports the resource short of the state it is usable in: the resource is reported
//!    `CREATE_IN_PROGRESS`, under the identifier that service already assigned, until that
//!    service reports it ready, and the resource (and the stack) fails if it never does.**
//!
//! The rows differ along what the rules generalise over: WHICH outcome ACM owes the request
//! — a private request, which "If you are requesting a private certificate, domain
//! validation is not required" issues; a public one on a domain whose owner could publish
//! the evidence, which ACM waits on; and a public one on a domain nobody can be delegated
//! control of, which ACM fails — the validation method (`DNS`, `EMAIL`, none at all), how
//! many names the certificate covers, whether `DomainValidationOptions` and `Tags` are
//! declared, whether the template is written in JSON or YAML, and the Region it deploys in.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the certificate ARN itself, which is ACM's to mint: what
//! is checked about it is that it has the form `DescribeCertificate`'s own `CertificateArn`
//! `@pattern` requires, that it is the id the stack reports from the first observation
//! onwards, and that ACM's `DescribeCertificate` resolves it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::stabilising::CREATE_STABILISATION_WINDOW;
use service_com_amazonaws_cloudformation_cloudformation::state::ComAmazonawsCloudformationCloudformationState;
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element};

/// Long enough for the stack's own clock-owed span (`STATUS_TRANSITION_DELAY`, 500 ms) and
/// for the outcome ACM owes a requested certificate it needs no published evidence for
/// (`VALIDATION_SETTLE_DELAY`, 750 ms) to come due, with room for both to be observed.
const PAST_BOTH_SPANS: Duration = Duration::from_millis(1_400);

/// How long the suite goes on polling for an outcome that is already due, and how often: a
/// loaded machine can be slower than the spans above without the rule being any different.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLLS: usize = 40;

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The `CertificateStatus` values the rows expect, from the model's own enum.
const PENDING_VALIDATION: &str = "PENDING_VALIDATION";
const ISSUED: &str = "ISSUED";

/// The `ResourceStatus` values the rules above are stated in, and the stack status a rolled
/// back create reaches.
const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const CREATE_FAILED: &str = "CREATE_FAILED";
const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them, and the engine itself — which one case needs in
/// order to put the clock of a wait that is bounded in minutes behind it.
///
/// Request authentication is off, as in every other suite here; the in-process callback
/// CloudFormation makes signs its request either way (the credential scope is what routes
/// it).
fn build_router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_acm_certificatemanager::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
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

/// The `(PhysicalResourceId, ResourceStatus)` the stack reports for ONE of its resources,
/// read through `DescribeStackResource` so the answer carries that resource alone.
async fn stack_resource(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    logical_id: &str,
) -> (u16, Option<String>, Option<String>, String) {
    let (status, body) = cloudformation(
        router,
        region,
        "DescribeStackResource",
        &[("StackName", stack), ("LogicalResourceId", logical_id)],
    )
    .await;
    let physical_id = opt_element(&body, "PhysicalResourceId");
    let resource_status = opt_element(&body, "ResourceStatus");
    (status, physical_id, resource_status, body)
}

/// The `StackStatus` a stack reports right now.
async fn stack_status(router: &MultiServiceRouter, region: &str, stack: &str) -> (String, String) {
    let (status, body) = cloudformation(router, region, "DescribeStacks", &[("StackName", stack)])
        .await;
    assert_eq!(
        status, 200,
        "DescribeStacks {stack:?}: expected HTTP 200, got {status} with body {body}"
    );
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// Poll the stack until it reports `wanted`, or answer the last status it reported.
async fn wait_for_stack_status(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    wanted: &str,
) -> (String, String) {
    let mut last = stack_status(router, region, stack).await;
    for _ in 0..POLLS {
        if last.0 == wanted {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = stack_status(router, region, stack).await;
    }
    last
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// Whether `arn` satisfies `DescribeCertificate`'s `CertificateArn` modelled `@pattern`
/// (`^arn:[\w+=/,.@-]+:acm:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$`) and its
/// modelled length (20..2048) for `region` and the account this emulator mints.
fn is_well_formed_certificate_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:acm:{region}:{ACCOUNT}:certificate/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        return false;
    };
    (20..=2048).contains(&arn.len())
        && !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+=,.@-".contains(c))
}

/// What ACM owes the request a row makes, which is what decides whether the resource's
/// creation ever finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owed {
    /// ACM issues the certificate on its own, so the resource's creation finishes and the
    /// stack completes.
    Issued,
    /// ACM waits for evidence this emulator never observes, so the resource's creation
    /// stays open for the whole documented validation window.
    Pending,
    /// ACM fails the request, so the resource's creation fails rather than waiting.
    Failed,
}

/// One row of the table: a template, and what the rules say the certificate it declares must
/// then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the certificate is requested in.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// What ACM owes the request the template makes.
    owed: Owed,
    /// The `DomainName` `DescribeCertificate` must report, which the template declares.
    expected_domain: &'static str,
    /// The whole name set the certificate must cover: the declared `DomainName` first, then
    /// the declared `SubjectAlternativeNames`.
    expected_names: &'static [&'static str],
    /// The tags `ListTagsForCertificate` must report, which the template's `Tags` decide.
    expected_tags: &'static [(&'static str, &'static str)],
    /// The output keys whose value must be the ARN ACM minted, read once the stack has
    /// completed (a stack still in progress reports no outputs).
    identity_outputs: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a public DNS-validated certificate for one domain, tagged, JSON",
        region: "us-west-2",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "an ACM certificate",
                "Resources": {
                    "Certificate": {
                        "Type": "AWS::CertificateManager::Certificate",
                        "Properties": {
                            "DomainName": "www.acme-widgets.com",
                            "ValidationMethod": "DNS",
                            "Tags": [
                                { "Key": "Owner", "Value": "platform" },
                                { "Key": "Tier", "Value": "prod" }
                            ]
                        }
                    }
                },
                "Outputs": {
                    "CertificateRef": { "Value": { "Ref": "Certificate" } }
                }
            })
            .to_string()
        },
        owed: Owed::Pending,
        expected_domain: "www.acme-widgets.com",
        expected_names: &["www.acme-widgets.com"],
        // `TagKey`'s pattern admits no digits, so the keys here are letters.
        expected_tags: &[("Owner", "platform"), ("Tier", "prod")],
        identity_outputs: &["CertificateRef"],
    },
    Row {
        case: "a private certificate, three names, untagged, JSON, which ACM issues itself",
        region: "eu-west-1",
        template: || {
            json!({
                "Resources": {
                    "Certificate": {
                        "Type": "AWS::CertificateManager::Certificate",
                        "Properties": {
                            "DomainName": "internal.acme-widgets.com",
                            "SubjectAlternativeNames": [
                                "api.internal.acme-widgets.com",
                                "db.internal.acme-widgets.com"
                            ],
                            "KeyAlgorithm": "RSA_2048",
                            "CertificateAuthorityArn":
                                "arn:aws:acm-pca:eu-west-1:000000000000:certificate-authority/\
                                 11111111-2222-3333-4444-555555555555"
                        }
                    }
                },
                "Outputs": {
                    "CertificateRef": { "Value": { "Ref": "Certificate" } }
                }
            })
            .to_string()
        },
        owed: Owed::Issued,
        expected_domain: "internal.acme-widgets.com",
        expected_names: &[
            "internal.acme-widgets.com",
            "api.internal.acme-widgets.com",
            "db.internal.acme-widgets.com",
        ],
        expected_tags: &[],
        identity_outputs: &["CertificateRef"],
    },
    Row {
        case: "a public EMAIL-validated certificate with a validation domain, YAML",
        region: "ap-southeast-2",
        template: || {
            [
                "Resources:",
                "  Certificate:",
                "    Type: AWS::CertificateManager::Certificate",
                "    Properties:",
                "      DomainName: shop.acme-widgets.com",
                "      ValidationMethod: EMAIL",
                "      DomainValidationOptions:",
                "        - DomainName: shop.acme-widgets.com",
                "          ValidationDomain: acme-widgets.com",
                "Outputs:",
                "  CertificateRef:",
                "    Value: !Ref Certificate",
                "",
            ]
            .join("\n")
        },
        owed: Owed::Pending,
        expected_domain: "shop.acme-widgets.com",
        expected_names: &["shop.acme-widgets.com"],
        expected_tags: &[],
        identity_outputs: &["CertificateRef"],
    },
    Row {
        case: "a public certificate for a domain nobody can be delegated, which ACM fails",
        region: "us-east-1",
        template: || {
            json!({
                "Resources": {
                    "Certificate": {
                        "Type": "AWS::CertificateManager::Certificate",
                        "Properties": {
                            // Reserved by IANA for documentation, so no requester can ever
                            // publish the evidence ACM asks for.
                            "DomainName": "www.example.com",
                            "ValidationMethod": "DNS"
                        }
                    }
                }
            })
            .to_string()
        },
        owed: Owed::Failed,
        expected_domain: "www.example.com",
        expected_names: &["www.example.com"],
        expected_tags: &[],
        identity_outputs: &[],
    },
];

/// Every row: the id the stack reports for its `AWS::CertificateManager::Certificate` is the
/// ARN ACM minted, from the FIRST observation onwards; the resource is reported
/// `CREATE_IN_PROGRESS` while ACM has not issued the certificate; and what becomes of the
/// resource is what ACM decides about the request.
#[tokio::test]
async fn a_stack_holds_its_acm_certificate_in_progress_under_the_arn_acm_minted() {
    let (router, _engine) = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("certificate-stack-{index}");
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

        // The FIRST observation of the resource, before any span has come due: the
        // identifier is already the one ACM minted, and the resource is reported
        // CREATE_IN_PROGRESS, because ACM has not issued the certificate.
        let (status, physical_id, resource_status, body) =
            stack_resource(&router, region, &stack_name, "Certificate").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResource, got {status} with body \
             {body}"
        );
        let certificate_arn = physical_id.unwrap_or_default();
        assert!(
            is_well_formed_certificate_arn(&certificate_arn, region),
            "case {case:?}: expected an ACM certificate ARN in {region}, got \
             {certificate_arn:?} in {body}"
        );
        assert_eq!(
            resource_status.as_deref(),
            Some(CREATE_IN_PROGRESS),
            "case {case:?}: expected the certificate resource to still be \
             {CREATE_IN_PROGRESS} while ACM has not issued it, got {body}"
        );

        // And ACM itself resolves that ARN to the certificate the template configured.
        let (status, answer) = acm(
            &router,
            region,
            "DescribeCertificate",
            &json!({ "CertificateArn": certificate_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeCertificate on the ARN the stack \
             reported, got {status} with body {answer}"
        );
        let certificate = &answer["Certificate"];
        assert_eq!(
            certificate["CertificateArn"].as_str(),
            Some(certificate_arn.as_str()),
            "case {case:?}: expected the certificate to report its own ARN, got {answer}"
        );
        assert_eq!(
            certificate["DomainName"].as_str(),
            Some(row.expected_domain),
            "case {case:?}: expected DomainName {:?}, got {answer}",
            row.expected_domain
        );
        let names: Vec<String> = certificate["SubjectAlternativeNames"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|name| name.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names, row.expected_names,
            "case {case:?}: expected the certificate to cover exactly the declared names \
             {:?}, got {answer}",
            row.expected_names
        );

        // The tags the template declared reached ACM through its own tagging surface. The
        // three `aws:cloudformation:*` tags CloudFormation records are NOT among them: ACM
        // refuses a reserved tag key to any caller, so they are left off rather than sent
        // to be refused.
        let (status, tags) = acm(
            &router,
            region,
            "ListTagsForCertificate",
            &json!({ "CertificateArn": certificate_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForCertificate, got {status} with \
             body {tags}"
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

        // What becomes of the resource is what ACM decides about the request.
        match row.owed {
            Owed::Issued => {
                let (status, described) =
                    wait_for_stack_status(&router, region, &stack_name, "CREATE_COMPLETE").await;
                assert_eq!(
                    status, "CREATE_COMPLETE",
                    "case {case:?}: expected the stack to complete once ACM issued the \
                     certificate, got {described}"
                );
                let (_, _, resource_status, body) =
                    stack_resource(&router, region, &stack_name, "Certificate").await;
                assert_eq!(
                    resource_status.as_deref(),
                    Some(CREATE_COMPLETE),
                    "case {case:?}: expected the certificate resource to be \
                     {CREATE_COMPLETE} once ACM issued it, got {body}"
                );
                let (_, answer) = acm(
                    &router,
                    region,
                    "DescribeCertificate",
                    &json!({ "CertificateArn": certificate_arn }),
                )
                .await;
                assert_eq!(
                    answer["Certificate"]["Status"].as_str(),
                    Some(ISSUED),
                    "case {case:?}: expected ACM to report the certificate {ISSUED} once the \
                     stack completed, got {answer}"
                );
                // The outputs a completed stack reports resolve the resource to that same
                // ARN, which is what `Ref` of this type returns.
                let outputs = outputs_of(&described);
                for key in row.identity_outputs {
                    assert_eq!(
                        outputs.get(*key),
                        Some(&certificate_arn),
                        "case {case:?}: expected output {key:?} to be the certificate ARN \
                         {certificate_arn:?}, got {:?}",
                        outputs.get(*key)
                    );
                }
            }
            Owed::Pending => {
                // Both spans have come due and the certificate is still unissued, so the
                // resource and the stack are still in progress — however long ago the call
                // that started the create returned.
                tokio::time::sleep(PAST_BOTH_SPANS).await;
                let (status, described) = stack_status(&router, region, &stack_name).await;
                assert_eq!(
                    status, CREATE_IN_PROGRESS,
                    "case {case:?}: expected the stack to still be {CREATE_IN_PROGRESS} while \
                     the certificate is unissued, got {described}"
                );
                let (_, physical_id, resource_status, body) =
                    stack_resource(&router, region, &stack_name, "Certificate").await;
                assert_eq!(
                    resource_status.as_deref(),
                    Some(CREATE_IN_PROGRESS),
                    "case {case:?}: expected the certificate resource to still be \
                     {CREATE_IN_PROGRESS}, got {body}"
                );
                assert_eq!(
                    physical_id.as_deref(),
                    Some(certificate_arn.as_str()),
                    "case {case:?}: expected the resource to go on reporting the ARN ACM \
                     minted, got {body}"
                );
                let (_, answer) = acm(
                    &router,
                    region,
                    "DescribeCertificate",
                    &json!({ "CertificateArn": certificate_arn }),
                )
                .await;
                assert_eq!(
                    answer["Certificate"]["Status"].as_str(),
                    Some(PENDING_VALIDATION),
                    "case {case:?}: expected ACM to still report the certificate \
                     {PENDING_VALIDATION}, got {answer}"
                );
            }
            Owed::Failed => {
                // ACM fails the request, so nothing is going to issue the certificate: the
                // resource's creation fails rather than waiting the window out, and the
                // failed create is rolled back — which removes the certificate it had
                // requested.
                let (status, described) =
                    wait_for_stack_status(&router, region, &stack_name, ROLLBACK_COMPLETE).await;
                assert_eq!(
                    status, ROLLBACK_COMPLETE,
                    "case {case:?}: expected the failed create to be rolled back once ACM \
                     failed the certificate, got {described}"
                );
                let (_, events) = cloudformation(
                    &router,
                    region,
                    "DescribeStackEvents",
                    &[("StackName", &stack_name)],
                )
                .await;
                let reasons = elements(&events, "ResourceStatusReason");
                assert!(
                    reasons
                        .iter()
                        .any(|reason| reason.contains(&certificate_arn)),
                    "case {case:?}: expected an event naming the certificate ACM failed, got \
                     {reasons:?}"
                );
                assert!(
                    elements(&events, "ResourceStatus")
                        .iter()
                        .any(|status| status == CREATE_FAILED),
                    "case {case:?}: expected the resource's own {CREATE_FAILED} event, got \
                     {events}"
                );
            }
        }

        // Deleting the stack deletes the certificate ACM holds for it — whether the stack
        // had completed or was still waiting for the certificate to be issued.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        let (status, gone) = acm(
            &router,
            region,
            "DescribeCertificate",
            &json!({ "CertificateArn": certificate_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the certificate to be gone once the stack deleted it, got \
             {status} with body {gone}"
        );
        assert_eq!(
            gone["__type"]
                .as_str()
                .unwrap_or_default()
                .split('#')
                .next_back(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException for the deleted certificate, got \
             {gone}"
        );
    }
}

/// The wait for a resource to be ready is BOUNDED: a certificate ACM never issues leaves the
/// resource `CREATE_FAILED` naming the state ACM was still reporting, and the failed create
/// is rolled back — which removes the certificate it had requested — rather than leaving the
/// stack in progress for ever.
///
/// The window is counted in minutes, so this case puts the barrier's own clock behind it
/// rather than sleeping it out: the wait that is then over is the same wait, at the same
/// instant it would have run out on its own.
#[tokio::test]
async fn a_certificate_acm_never_issues_fails_the_resource_when_the_wait_runs_out() {
    let (router, engine) = build_router();
    let region = "us-east-2";
    let stack_name = "certificate-wait-runs-out";
    let template = json!({
        "Resources": {
            "Certificate": {
                "Type": "AWS::CertificateManager::Certificate",
                "Properties": {
                    "DomainName": "payments.acme-widgets.com",
                    "ValidationMethod": "DNS"
                }
            }
        }
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
        "expected HTTP 200 from CreateStack, got {status} with body {body}"
    );

    let (_, physical_id, resource_status, body) =
        stack_resource(&router, region, stack_name, "Certificate").await;
    let certificate_arn = physical_id.unwrap_or_default();
    assert_eq!(
        resource_status.as_deref(),
        Some(CREATE_IN_PROGRESS),
        "expected the certificate resource to be {CREATE_IN_PROGRESS} while it is unissued, \
         got {body}"
    );

    // The wait has now run out, at the instant it would have anyway. The wait runs from the
    // instant the CALL it is waiting for was made, so that is the instant a case reaching past
    // its deadline moves — beside the instant the operation behind it started, which is what
    // the resource's own in-progress reporting is stamped with.
    {
        let state = engine.state::<ComAmazonawsCloudformationCloudformationState>();
        let mut store = state.lock();
        for stack in &mut store.stacks {
            for barrier in &mut stack.stabilisations {
                barrier.asked_at -= CREATE_STABILISATION_WINDOW + Duration::from_secs(1);
                barrier.opened_at -= CREATE_STABILISATION_WINDOW + Duration::from_secs(1);
            }
        }
    }

    let (status, described) =
        wait_for_stack_status(&router, region, stack_name, ROLLBACK_COMPLETE).await;
    assert_eq!(
        status, ROLLBACK_COMPLETE,
        "expected the failed create to be rolled back once the wait for the certificate ran \
         out, got {described}"
    );
    let (_, events) = cloudformation(
        &router,
        region,
        "DescribeStackEvents",
        &[("StackName", stack_name)],
    )
    .await;
    let reasons = elements(&events, "ResourceStatusReason");
    assert!(
        reasons.iter().any(|reason| {
            reason.contains(&certificate_arn) && reason.contains(PENDING_VALIDATION)
        }),
        "expected an event naming the certificate and the state ACM was still reporting, got \
         {reasons:?}"
    );
    assert!(
        reasons
            .iter()
            .any(|reason| reason.contains("failed to create: [Certificate]")),
        "expected the stack's own failure to name the resource that failed, got {reasons:?}"
    );
    // The rollback removed the certificate the failed create had requested.
    let (status, gone) = acm(
        &router,
        region,
        "DescribeCertificate",
        &json!({ "CertificateArn": certificate_arn }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected the rollback to have deleted the certificate the failed create requested, \
         got {status} with body {gone}"
    );
    assert_eq!(
        gone["__type"]
            .as_str()
            .unwrap_or_default()
            .split('#')
            .next_back(),
        Some("ResourceNotFoundException"),
        "expected ResourceNotFoundException for the rolled-back certificate, got {gone}"
    );
}
