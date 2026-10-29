// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares the three resources a private CA is ACTIVATED by
//! really activates it: the CA's own certification request is signed by the CA, the
//! certificate that comes back is imported into it, and the CA the stack publishes can then
//! issue certificates.
//!
//! The rule under test, which two crates have to agree on (CloudFormation makes the calls;
//! Amazon Web Services Private CA serves them, builds the certification request and signs
//! it), so the suite lives here rather than in either crate:
//!
//! **a stack reports `CREATE_COMPLETE` only once every resource it declares has really been
//! created in the service that owns it** — so an
//! `AWS::ACMPCA::Certificate` of a completed stack is a certificate that service really
//! issued (its `Fn::GetAtt CertificateSigningRequest`/`Certificate` chain resolving to the
//! values that service produced, not to placeholders), and an
//! `AWS::ACMPCA::CertificateAuthorityActivation` is a CA that really holds that certificate
//! and is in the status the declaration asked for.
//!
//! The rows differ along what the rule generalises over: the Region the stack deploys in,
//! the form the template is written in (JSON with the long-form intrinsics, YAML with the
//! short-form tags), the CA's key and signing algorithms, the validity the certificate is
//! asked for (in days, in years) and the status the activation asks for (`ACTIVE`, and
//! `DISABLED` — a CA that holds its certificate and may not use it).
//!
//! Every expected value is computed here from what the row declares. The two values that
//! service OWNS — the CA's ARN and the certificate's — are checked for the form their
//! models require, for being what the stack reports and exports, and for resolving through
//! that service's own reads. The leaf certification request the `ACTIVE` rows have the
//! deployed CA sign is one the emulator built for ANOTHER CA, read over that service's own
//! wire, so the bytes the deployed CA is asked to sign are not built by this suite.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{form_encode, opt_element, outputs_of};

/// How long the suite goes on polling for an outcome that is already due, and how often.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLLS: usize = 40;

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The `StackStatus` a deployment that went through reports.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The PEM delimiters an X.509 certificate body is framed by (RFC 7468), which is the form
/// `GetCertificate` answers in.
const CERTIFICATE_BEGIN: &str = "-----BEGIN CERTIFICATE-----";
const CERTIFICATE_END: &str = "-----END CERTIFICATE-----";

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
        service_com_amazonaws_acmpca_acmprivateca::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in `region`, which
/// is what routes the request and what each handler reads its Region from.
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

/// Call one Amazon Web Services Private CA operation over its awsJson1_1 wire, signed for
/// `region`.
async fn acm_pca(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("acm-pca.{region}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("ACMPrivateCA.{operation}"))
        .header("authorization", authorization("acm-pca", region))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// Poll the stack until it reports `CREATE_COMPLETE`, or answer the last status it reported
/// with the `DescribeStacks` body that reported it.
async fn wait_for_completion(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
) -> (String, String) {
    for _ in 0..POLLS {
        let (status, body) =
            cloudformation(router, region, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            status, 200,
            "DescribeStacks {stack}: expected HTTP 200, got {status} with body {body}"
        );
        if opt_element(&body, "StackStatus").unwrap_or_default() == CREATE_COMPLETE {
            return (CREATE_COMPLETE.to_string(), body);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let (_, body) = cloudformation(router, region, "DescribeStacks", &[("StackName", stack)]).await;
    (opt_element(&body, "StackStatus").unwrap_or_default(), body)
}

/// The `PhysicalResourceId` the stack reports for ONE of its resources.
async fn stack_resource_id(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    logical_id: &str,
) -> (Option<String>, String) {
    let (status, body) = cloudformation(
        router,
        region,
        "DescribeStackResource",
        &[("StackName", stack), ("LogicalResourceId", logical_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackResource {logical_id}: expected HTTP 200, got {status} with body {body}"
    );
    (opt_element(&body, "PhysicalResourceId"), body)
}

/// Whether `arn` satisfies the `Arn` pattern every acm-pca operation's member is modelled
/// with, for a certificate authority of `region` in the account this emulator mints.
fn is_well_formed_ca_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:acm-pca:{region}:{ACCOUNT}:certificate-authority/");
    arn.strip_prefix(&prefix).is_some_and(|id| {
        !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+=,.@-".contains(c))
    })
}

/// A certificate ARN carries the ARN of the CA that issued it as its prefix, then the
/// serial that service allocated (`<ca arn>/certificate/<serial>`).
fn is_certificate_of(certificate_arn: &str, ca_arn: &str) -> bool {
    certificate_arn
        .strip_prefix(&format!("{ca_arn}/certificate/"))
        .is_some_and(|serial| !serial.is_empty() && serial.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Whether `body` is one PEM-framed certificate.
fn is_certificate_pem(body: &str) -> bool {
    body.trim_start().starts_with(CERTIFICATE_BEGIN) && body.trim_end().ends_with(CERTIFICATE_END)
}

/// One row of the table: a template, and what the rule says the CA it declares must then
/// be.
struct Row {
    case: &'static str,
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The `CertificateAuthorityStatus` the activation asks for, which is the status the CA
    /// must be in once the stack has deployed.
    expected_status: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "an RSA root CA activated ACTIVE, JSON",
        region: "us-west-2",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a private certificate authority and its activation",
                "Resources": {
                    "Ca": {
                        "Type": "AWS::ACMPCA::CertificateAuthority",
                        "Properties": {
                            "Type": "ROOT",
                            "KeyAlgorithm": "RSA_2048",
                            "SigningAlgorithm": "SHA256WITHRSA",
                            "Subject": {"CommonName": "root.activated.internal"}
                        }
                    },
                    "CaCert": {
                        "Type": "AWS::ACMPCA::Certificate",
                        "Properties": {
                            "CertificateAuthorityArn": {"Ref": "Ca"},
                            "CertificateSigningRequest": {
                                "Fn::GetAtt": ["Ca", "CertificateSigningRequest"]
                            },
                            "SigningAlgorithm": "SHA256WITHRSA",
                            "TemplateArn": "arn:aws:acm-pca:::template/RootCACertificate/V1",
                            "Validity": {"Type": "DAYS", "Value": 3650}
                        }
                    },
                    "CaActivation": {
                        "Type": "AWS::ACMPCA::CertificateAuthorityActivation",
                        "Properties": {
                            "CertificateAuthorityArn": {"Ref": "Ca"},
                            "Certificate": {"Fn::GetAtt": ["CaCert", "Certificate"]},
                            "Status": "ACTIVE"
                        }
                    }
                },
                "Outputs": {
                    "CaArn": {"Value": {"Ref": "Ca"}},
                    "CertificateArn": {"Value": {"Ref": "CaCert"}}
                }
            })
            .to_string()
        },
        expected_status: "ACTIVE",
    },
    Row {
        case: "an EC root CA activated DISABLED, YAML",
        region: "eu-west-1",
        template: || {
            [
                "Resources:",
                "  Ca:",
                "    Type: AWS::ACMPCA::CertificateAuthority",
                "    Properties:",
                "      Type: ROOT",
                "      KeyAlgorithm: EC_prime256v1",
                "      SigningAlgorithm: SHA384WITHECDSA",
                "      Subject:",
                "        CommonName: root.disabled.internal",
                "  CaCert:",
                "    Type: AWS::ACMPCA::Certificate",
                "    Properties:",
                "      CertificateAuthorityArn: !Ref Ca",
                "      CertificateSigningRequest: !GetAtt Ca.CertificateSigningRequest",
                "      SigningAlgorithm: SHA384WITHECDSA",
                "      TemplateArn: arn:aws:acm-pca:::template/RootCACertificate/V1",
                "      Validity:",
                "        Type: YEARS",
                "        Value: 10",
                "  CaActivation:",
                "    Type: AWS::ACMPCA::CertificateAuthorityActivation",
                "    Properties:",
                "      CertificateAuthorityArn: !Ref Ca",
                "      Certificate: !GetAtt CaCert.Certificate",
                "      Status: DISABLED",
                "Outputs:",
                "  CaArn:",
                "    Value: !Ref Ca",
                "  CertificateArn:",
                "    Value: !Ref CaCert",
                "",
            ]
            .join("\n")
        },
        expected_status: "DISABLED",
    },
];

/// A certification request this emulator built for a CA OTHER than the deployed one, read
/// over that service's own wire: the bytes the deployed CA is asked to sign as a leaf.
async fn borrowed_certification_request(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    common_name: &str,
) -> String {
    let (status, created) = acm_pca(
        router,
        region,
        "CreateCertificateAuthority",
        &json!({
            "CertificateAuthorityConfiguration": {
                "KeyAlgorithm": "RSA_2048",
                "SigningAlgorithm": "SHA256WITHRSA",
                "Subject": {"CommonName": common_name}
            },
            "CertificateAuthorityType": "SUBORDINATE"
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected a second CA to be created, got {status} {created:?}"
    );
    let arn = created
        .get("CertificateAuthorityArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no CertificateAuthorityArn in {created:?}"))
        .to_string();

    // The request is available once that service has finished creating the CA, which is
    // owed on its own clock: the suite polls for it exactly as a client would.
    for _ in 0..POLLS {
        let (status, reported) = acm_pca(
            router,
            region,
            "GetCertificateAuthorityCsr",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
        if status == 200 {
            return reported
                .get("Csr")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no Csr in {reported:?}"))
                .to_string();
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: the second CA never published a certification request");
}

/// The body of the certificate `certificate_arn` names, polled until that service has
/// finished issuing it.
async fn issued_certificate(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    ca_arn: &str,
    certificate_arn: &str,
) -> String {
    for _ in 0..POLLS {
        let (status, reported) = acm_pca(
            router,
            region,
            "GetCertificate",
            &json!({
                "CertificateAuthorityArn": ca_arn,
                "CertificateArn": certificate_arn,
            }),
        )
        .await;
        if status == 200 {
            return reported
                .get("Certificate")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no Certificate in {reported:?}"))
                .to_string();
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!("case {case:?}: {certificate_arn} was never served");
}

/// Every row: the stack completes, and the CA it published holds the certificate the stack
/// had it sign and is in the status the activation asked for — with the `ACTIVE` ones able
/// to sign a leaf request straight away.
#[tokio::test]
async fn a_stack_activates_the_private_ca_it_declares() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("activated-ca-stack-{index}");
        let template = (row.template)();

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", stack_name.as_str()),
                ("TemplateBody", template.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        let (stack_status, described) = wait_for_completion(&router, region, &stack_name).await;
        assert_eq!(
            stack_status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to complete, got {stack_status:?} ({described})"
        );

        // What the stack published: the CA's ARN and the ARN of the certificate its
        // `AWS::ACMPCA::Certificate` had that CA issue.
        let outputs = outputs_of(&described);
        let ca_arn = outputs
            .get("CaArn")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no CaArn output in {described}"));
        let certificate_arn = outputs
            .get("CertificateArn")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no CertificateArn output in {described}"));
        assert!(
            is_well_formed_ca_arn(&ca_arn, region),
            "case {case:?}: expected a certificate-authority ARN of {region}, got {ca_arn:?}"
        );
        assert!(
            is_certificate_of(&certificate_arn, &ca_arn),
            "case {case:?}: expected a certificate ARN of {ca_arn:?}, got {certificate_arn:?}"
        );

        // And those are the identifiers the stack reports for the two resources.
        for (logical_id, expected) in [("Ca", &ca_arn), ("CaCert", &certificate_arn)] {
            let (reported, body) = stack_resource_id(&router, region, &stack_name, logical_id).await;
            assert_eq!(
                reported.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {logical_id} to report {expected:?}, got {reported:?} \
                 ({body})"
            );
        }

        // The activation really happened: the CA holds a certificate and is in the status
        // the declaration asked for.
        let (status, reported) = acm_pca(
            &router,
            region,
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": ca_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeCertificateAuthority to resolve {ca_arn:?}, got \
             {status} {reported:?}"
        );
        let authority = reported
            .get("CertificateAuthority")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            authority.get("Status").and_then(Value::as_str),
            Some(row.expected_status),
            "case {case:?}: expected the CA in {}, got {authority:?}",
            row.expected_status
        );

        // The certificate the stack had the CA sign is the one the CA was activated with,
        // and both are PEM-framed certificates that service serves.
        let signed = issued_certificate(&router, region, case, &ca_arn, &certificate_arn).await;
        assert!(
            is_certificate_pem(&signed),
            "case {case:?}: expected the issued certificate as a PEM body, got {signed:?}"
        );
        let (status, held) = acm_pca(
            &router,
            region,
            "GetCertificateAuthorityCertificate",
            &json!({"CertificateAuthorityArn": ca_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the CA to hold its certificate, got {status} {held:?}"
        );
        let imported = held
            .get("Certificate")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            imported, signed,
            "case {case:?}: expected the CA to hold the certificate the stack had it sign, \
             got {imported:?}"
        );

        // An activated CA signs: a leaf request this suite did not build is answered with a
        // certificate of its own. A CA the activation DISABLED may not, which is the other
        // half of the status the declaration asked for.
        let leaf = borrowed_certification_request(
            &router,
            region,
            case,
            &format!("leaf-{index}.activated.internal"),
        )
        .await;
        let signing_algorithm = authority
            .get("CertificateAuthorityConfiguration")
            .and_then(|configuration| configuration.get("SigningAlgorithm"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let (status, issued) = acm_pca(
            &router,
            region,
            "IssueCertificate",
            &json!({
                "CertificateAuthorityArn": ca_arn,
                "Csr": aws_smithy_types::base64::encode(leaf.as_bytes()),
                "SigningAlgorithm": signing_algorithm,
                "TemplateArn": "arn:aws:acm-pca:::template/EndEntityCertificate/V1",
                "Validity": {"Type": "DAYS", "Value": 30},
            }),
        )
        .await;
        if row.expected_status == "ACTIVE" {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the activated CA to issue a leaf, got {status} \
                 {issued:?}"
            );
            let leaf_arn = issued
                .get("CertificateArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no CertificateArn in {issued:?}"))
                .to_string();
            let leaf_pem = issued_certificate(&router, region, case, &ca_arn, &leaf_arn).await;
            assert!(
                is_certificate_pem(&leaf_pem),
                "case {case:?}: expected a leaf certificate as a PEM body, got {leaf_pem:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a {} CA to refuse an issuance, got {status} {issued:?}",
                row.expected_status
            );
        }
    }
}
