// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that declares an `AWS::ACMPCA::CertificateAuthority` really
//! creates the private CA in Amazon Web Services Private CA, publishes the ARN that service
//! minted as the resource's identity, and exports it through the stack's outputs.
//!
//! The rules under test, both of which two crates have to agree on (CloudFormation makes the
//! calls; Amazon Web Services Private CA serves them and mints the ARN), so the suite lives
//! here rather than in either crate:
//!
//! 1. **a declared resource of a type this emulator provisions is created through the OWNING
//!    service's own API, so the stack's `PhysicalResourceId` — and what `Ref`/`Fn::GetAtt`
//!    resolve to — is the identifier that service assigned**, which for this type is the
//!    certificate-authority ARN, **and that service's own read resolves it to the resource
//!    the template configured.**
//! 2. **a stack that finished deploying reports a VALUE for every output its template
//!    declares**: each `Outputs` entry of a `CREATE_COMPLETE` stack carries an `OutputValue`,
//!    so a client reading the stack's outputs can discover what the stack created.
//!
//! The rows differ along what the rules generalise over: the Region the stack deploys in,
//! the form the template is written in (JSON or YAML), which key and signing algorithm the
//! CA is declared with, whether it is a root or a subordinate CA, how the outputs name the
//! resource (`Ref`, `Fn::GetAtt` of `Arn`, `Fn::GetAtt` of `CertificateAuthorityArn`,
//! `Fn::Sub` over one of them), and whether the declaration carries tags, a usage mode and a
//! revocation configuration.
//!
//! Every expected value is computed here from what the row declares, never read back out of
//! the answer being checked — except the CA ARN itself, which that service mints: what is
//! checked about it is that it has the form `DescribeCertificateAuthority`'s own
//! `CertificateAuthorityArn` `@pattern` requires, that it is the id the stack reports and
//! exports, and that `acm-pca DescribeCertificateAuthority` resolves it.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{elements, form_encode, opt_element, outputs_of};

/// How long the suite goes on polling for an outcome that is already due, and how often: a
/// loaded machine can be slower than the emulator's own spans without the rule being any
/// different.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const POLLS: usize = 40;

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The `ResourceStatus`/`StackStatus` values the rules above are stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
///
/// Request authentication is off, as in every other suite here; the in-process callback
/// CloudFormation makes signs its request either way (the credential scope is what routes
/// it).
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

/// The `(PhysicalResourceId, ResourceStatus)` the stack reports for ONE of its resources,
/// read through `DescribeStackResource` so the answer carries that resource alone.
async fn stack_resource(
    router: &MultiServiceRouter,
    region: &str,
    stack: &str,
    logical_id: &str,
) -> (Option<String>, Option<String>, String) {
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
    (
        opt_element(&body, "PhysicalResourceId"),
        opt_element(&body, "ResourceStatus"),
        body,
    )
}

/// Poll the stack until it reports `CREATE_COMPLETE`, or answer the last status it reported,
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
        let reported = opt_element(&body, "StackStatus").unwrap_or_default();
        if reported == CREATE_COMPLETE {
            return (reported, body);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let (_, body) =
        cloudformation(router, region, "DescribeStacks", &[("StackName", stack)]).await;
    (
        opt_element(&body, "StackStatus").unwrap_or_default(),
        body,
    )
}

/// Whether `arn` satisfies `DescribeCertificateAuthority`'s `CertificateAuthorityArn`
/// modelled `@pattern` (`^arn:[\w+=/,.@-]+:acm-pca:[\w+=/,.@-]*:[0-9]*:[\w+=,.@-]+(/[\w+=,.@-]+)*$`)
/// and its modelled length (5..200) for `region` and the account this emulator mints.
fn is_well_formed_ca_arn(arn: &str, region: &str) -> bool {
    let prefix = format!("arn:aws:acm-pca:{region}:{ACCOUNT}:certificate-authority/");
    let Some(id) = arn.strip_prefix(&prefix) else {
        return false;
    };
    (5..=200).contains(&arn.len())
        && !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+=,.@-".contains(c))
}

/// One row of the table: a template, and what the rules say the CA it declares must then be.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the CA is created in.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML).
    template: fn() -> String,
    /// The stack's own tags, which CloudFormation propagates to the resource.
    stack_tags: &'static [(&'static str, &'static str)],
    /// What `DescribeCertificateAuthority` must report for the CA, computed from what the
    /// template declares: its kind, its key and signing algorithms, its subject and its
    /// usage mode.
    expected_type: &'static str,
    expected_key_algorithm: &'static str,
    expected_signing_algorithm: &'static str,
    expected_subject: fn() -> Value,
    expected_usage_mode: &'static str,
    /// The tags `ListTags` must report for the CA: the declaration's own beside the stack's.
    expected_tags: &'static [(&'static str, &'static str)],
    /// Every output key the template declares, each of which must carry the CA's ARN.
    identity_outputs: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "a tagged RSA root CA exported by Ref and by GetAtt, JSON",
        region: "us-west-2",
        template: || {
            json!({
                "AWSTemplateFormatVersion": "2010-09-09",
                "Description": "a private certificate authority",
                "Resources": {
                    "Ca": {
                        "Type": "AWS::ACMPCA::CertificateAuthority",
                        "Properties": {
                            "Type": "ROOT",
                            "KeyAlgorithm": "RSA_2048",
                            "SigningAlgorithm": "SHA256WITHRSA",
                            "Subject": {
                                "CommonName": "root.acme-widgets.internal",
                                "Country": "US",
                                "Organization": "acme-widgets"
                            },
                            "Tags": [{ "Key": "Owner", "Value": "platform" }]
                        }
                    }
                },
                "Outputs": {
                    "CertificateAuthorityArn": { "Value": { "Ref": "Ca" } },
                    "CaArn": { "Value": { "Fn::GetAtt": ["Ca", "Arn"] } },
                    "CaTypedArn": {
                        "Value": { "Fn::GetAtt": ["Ca", "CertificateAuthorityArn"] }
                    }
                }
            })
            .to_string()
        },
        stack_tags: &[("Team", "pki")],
        expected_type: "ROOT",
        expected_key_algorithm: "RSA_2048",
        expected_signing_algorithm: "SHA256WITHRSA",
        expected_subject: || {
            json!({
                "CommonName": "root.acme-widgets.internal",
                "Country": "US",
                "Organization": "acme-widgets"
            })
        },
        // "The default value is GENERAL_PURPOSE" (`UsageMode`), which this row declares
        // none of.
        expected_usage_mode: "GENERAL_PURPOSE",
        expected_tags: &[("Owner", "platform"), ("Team", "pki")],
        identity_outputs: &["CertificateAuthorityArn", "CaArn", "CaTypedArn"],
    },
    Row {
        case: "an untagged EC subordinate CA with a revocation configuration, YAML",
        region: "eu-west-1",
        template: || {
            [
                "Resources:",
                "  Ca:",
                "    Type: AWS::ACMPCA::CertificateAuthority",
                "    Properties:",
                "      Type: SUBORDINATE",
                "      KeyAlgorithm: EC_prime256v1",
                "      SigningAlgorithm: SHA384WITHECDSA",
                "      UsageMode: SHORT_LIVED_CERTIFICATE",
                "      Subject:",
                "        CommonName: issuing.acme-widgets.internal",
                "      RevocationConfiguration:",
                "        CrlConfiguration:",
                "          Enabled: false",
                "Outputs:",
                "  CaArn:",
                "    Value: !Ref Ca",
                "  CaArnBySub:",
                "    Value: !Sub '${Ca}'",
                "",
            ]
            .join("\n")
        },
        stack_tags: &[],
        expected_type: "SUBORDINATE",
        expected_key_algorithm: "EC_prime256v1",
        expected_signing_algorithm: "SHA384WITHECDSA",
        expected_subject: || json!({ "CommonName": "issuing.acme-widgets.internal" }),
        expected_usage_mode: "SHORT_LIVED_CERTIFICATE",
        expected_tags: &[],
        identity_outputs: &["CaArn", "CaArnBySub"],
    },
];

/// Every row: the id the stack reports for its `AWS::ACMPCA::CertificateAuthority` is the
/// ARN that service minted, every output of the completed stack carries a value, and the
/// outputs over the CA carry that same ARN — which `DescribeCertificateAuthority` then
/// resolves to the CA the template configured.
#[tokio::test]
async fn a_stack_publishes_the_arn_private_ca_minted_for_its_certificate_authority() {
    let router = build_router();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("private-ca-stack-{index}");
        let template = (row.template)();

        let mut members: Vec<(String, String)> = vec![
            ("StackName".to_string(), stack_name.clone()),
            ("TemplateBody".to_string(), template),
        ];
        for (position, (key, value)) in row.stack_tags.iter().enumerate() {
            members.push((
                format!("Tags.member.{}.Key", position + 1),
                (*key).to_string(),
            ));
            members.push((
                format!("Tags.member.{}.Value", position + 1),
                (*value).to_string(),
            ));
        }
        let borrowed: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let (status, body) = cloudformation(&router, region, "CreateStack", &borrowed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        // The FIRST observation of the resource: the identifier is already the one that
        // service minted, whatever status the resource is still in.
        let (physical_id, resource_status, body) =
            stack_resource(&router, region, &stack_name, "Ca").await;
        let first_seen = physical_id.unwrap_or_else(|| {
            panic!("case {case:?}: the resource reports no PhysicalResourceId: {body}")
        });
        assert!(
            is_well_formed_ca_arn(&first_seen, region),
            "case {case:?}: expected a certificate-authority ARN of {region}, got \
             {first_seen:?} ({body})"
        );
        assert!(
            resource_status.is_some(),
            "case {case:?}: the resource reports no ResourceStatus: {body}"
        );

        // The stack finishes, and the resource is reported created under that same id.
        let (stack_status, described) = wait_for_completion(&router, region, &stack_name).await;
        assert_eq!(
            stack_status, CREATE_COMPLETE,
            "case {case:?}: expected the stack to complete, got {stack_status:?} ({described})"
        );
        let (physical_id, resource_status, body) =
            stack_resource(&router, region, &stack_name, "Ca").await;
        assert_eq!(
            physical_id.as_deref(),
            Some(first_seen.as_str()),
            "case {case:?}: the resource's id must not change, got {physical_id:?} ({body})"
        );
        assert_eq!(
            resource_status.as_deref(),
            Some(CREATE_COMPLETE),
            "case {case:?}: expected the resource created, got {resource_status:?} ({body})"
        );

        // Every output the template declares carries a value, and the ones naming the CA
        // carry the ARN that service minted.
        let keys = elements(&described, "OutputKey");
        let values = elements(&described, "OutputValue");
        assert_eq!(
            keys.len(),
            row.identity_outputs.len(),
            "case {case:?}: expected one entry per declared output, got {keys:?} ({described})"
        );
        assert_eq!(
            values.len(),
            keys.len(),
            "case {case:?}: every output of a completed stack carries an OutputValue, got \
             {keys:?} against {values:?} ({described})"
        );
        let outputs = outputs_of(&described);
        for key in row.identity_outputs {
            assert_eq!(
                outputs.get(*key).map(String::as_str),
                Some(first_seen.as_str()),
                "case {case:?}: expected output {key:?} to carry {first_seen:?}, got \
                 {outputs:?}"
            );
        }

        // The owning service resolves that id to the CA the template configured.
        let (status, reported) = acm_pca(
            &router,
            region,
            "DescribeCertificateAuthority",
            &json!({ "CertificateAuthorityArn": first_seen }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeCertificateAuthority to resolve {first_seen:?}, \
             got {status} {reported:?}"
        );
        let authority = reported
            .get("CertificateAuthority")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            authority.get("Arn").and_then(Value::as_str),
            Some(first_seen.as_str()),
            "case {case:?}: expected the CA to report its own ARN, got {authority:?}"
        );
        assert_eq!(
            authority.get("Type").and_then(Value::as_str),
            Some(row.expected_type),
            "case {case:?}: expected a {} CA, got {authority:?}",
            row.expected_type
        );
        assert_eq!(
            authority.get("UsageMode").and_then(Value::as_str),
            Some(row.expected_usage_mode),
            "case {case:?}: expected usage mode {}, got {authority:?}",
            row.expected_usage_mode
        );
        let configuration = authority
            .get("CertificateAuthorityConfiguration")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            configuration.get("KeyAlgorithm").and_then(Value::as_str),
            Some(row.expected_key_algorithm),
            "case {case:?}: expected key algorithm {}, got {configuration:?}",
            row.expected_key_algorithm
        );
        assert_eq!(
            configuration.get("SigningAlgorithm").and_then(Value::as_str),
            Some(row.expected_signing_algorithm),
            "case {case:?}: expected signing algorithm {}, got {configuration:?}",
            row.expected_signing_algorithm
        );
        assert_eq!(
            configuration.get("Subject").cloned().unwrap_or(Value::Null),
            (row.expected_subject)(),
            "case {case:?}: expected the declared subject, got {configuration:?}"
        );

        // And the tags the stack gave the CA are the ones that service reports for it: the
        // declaration's own beside the stack's, under the ARN the stack published.
        let (status, listed) = acm_pca(
            &router,
            region,
            "ListTags",
            &json!({ "CertificateAuthorityArn": first_seen }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTags to answer for {first_seen:?}, got {status} \
             {listed:?}"
        );
        let mut reported_tags: Vec<(String, String)> = listed
            .get("Tags")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|tag| {
                Some((
                    tag.get("Key").and_then(Value::as_str)?.to_string(),
                    tag.get("Value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ))
            })
            .collect();
        reported_tags.sort();
        let mut expected_tags: Vec<(String, String)> = row
            .expected_tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        expected_tags.sort();
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected the declared and stack tags, got {reported_tags:?}"
        );
    }
}
