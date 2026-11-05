// SPDX-License-Identifier: CC0-1.0

//! The order in which an operation that takes a private CA's ARN resolves what it was
//! asked about, and the sentence each stage refuses with.
//!
//! The rule under test: an operation whose input carries a private CA's ARN resolves the
//! ARN's SHAPE first, then the AUTHORITY that ARN names, and only then the sub-resource of
//! that authority it was asked for — so an ARN that names no private CA resource is HTTP
//! 400 `InvalidArnException`, a well-formed ARN naming an authority this service does not
//! hold is HTTP 400 `ResourceNotFoundException` naming the CA, and only an authority that
//! EXISTS reports the thing asked of it (its policy, a grant on it, a certificate it
//! issued) as the absent one, with that sub-resource's own sentence.
//!
//! The rows differ along what the rule generalises over: which operation is called (reads,
//! writes and deletes, over either spelling of the ARN member), which way the ARN is
//! malformed, which way a well-formed ARN names no authority (a fresh id, another id form,
//! another account, Region or partition) and which sub-resource the second stage is about.
//!
//! Every call goes over the wire through the harness in [`super::private_ca_lifecycle`].

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::state::{
    CA_NOT_FOUND_MESSAGE, CERTIFICATE_NOT_FOUND_MESSAGE, INVALID_ARN_MESSAGE,
    PERMISSION_NOT_FOUND_MESSAGE, POLICY_NOT_FOUND_MESSAGE,
};

use crate::private_ca_lifecycle::{active_ca, Emulator, ACCOUNT, REGION};
use crate::resource_policy::error_message;

/// A principal an `acm-pca` permission can name, for the operations that take one. Any
/// value does: the rows that carry it are about the CA, which is resolved first.
const PRINCIPAL: &str = "acm.amazonaws.com";

/// A well-formed resource policy OF `resource_arn`, for the operation that attaches one.
fn policy_for(resource_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": ACCOUNT},
            "Action": "acm-pca:IssueCertificate",
            "Resource": resource_arn
        }]
    })
    .to_string()
}

/// The request body `operation` takes for the CA (or resource) ARN `arn`.
///
/// Two spellings of the same member: the operations over a CA take
/// `CertificateAuthorityArn` and the ones over its resource-based policy take
/// `ResourceArn`, and both name the private CA whose ARN is resolved.
fn request(operation: &str, arn: &str) -> Value {
    match operation {
        "GetPolicy" | "DeletePolicy" => json!({"ResourceArn": arn}),
        "PutPolicy" => json!({"ResourceArn": arn, "Policy": policy_for(arn)}),
        "DeletePermission" => {
            json!({"CertificateAuthorityArn": arn, "Principal": PRINCIPAL})
        }
        _ => json!({"CertificateAuthorityArn": arn}),
    }
}

/// The operations whose input carries the ARN of a private CA and nothing else that has to
/// resolve first: reads of the CA, reads and writes of its policy, and a revocation.
const ARN_TAKING_OPERATIONS: [&str; 9] = [
    "DescribeCertificateAuthority",
    "ListTags",
    "ListPermissions",
    "GetCertificateAuthorityCsr",
    "GetCertificateAuthorityCertificate",
    "GetPolicy",
    "PutPolicy",
    "DeletePolicy",
    "DeletePermission",
];

/// Stage one: an ARN that matches the modelled `Arn` pattern but names no private CA
/// RESOURCE is an ARN error, before any lookup — the same answer from every operation.
#[tokio::test]
async fn an_arn_that_names_no_private_ca_resource_is_refused_before_any_lookup() {
    let emulator = Emulator::new();
    // A CA exists, so the refusals below are about the ARN in the request rather than
    // about an empty store.
    let (_existing, _certificate) = active_ca(&emulator, "arn-stage-neighbour").await;

    // Each row matches `Arn`'s modelled pattern
    // (`^arn:[\w+=/,.@-]+:acm-pca:[\w+=/,.@-]*:[0-9]*:[\w+=,.@-]+(/[\w+=,.@-]+)*$`) and its
    // modelled length, so the framework hands it to the handler, and none of them names a
    // `certificate-authority/<id>` of this service.
    let rows: Vec<(&str, String)> = vec![
        (
            "no resource id at all",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority"),
        ),
        (
            "another resource type",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate/0123456789abcdef"),
        ),
        (
            "an account that is not twelve digits",
            format!("arn:aws:acm-pca:{REGION}:12345:certificate-authority/abcdef"),
        ),
        (
            "an empty account",
            format!("arn:aws:acm-pca:{REGION}::certificate-authority/abcdef"),
        ),
    ];

    for (case, arn) in &rows {
        for operation in ARN_TAKING_OPERATIONS {
            let body = emulator
                .refused(case, operation, &request(operation, arn), "InvalidArnException")
                .await;
            let message = error_message(&body, case);
            assert_eq!(
                message, INVALID_ARN_MESSAGE,
                "case {case:?}: expected {operation} to answer the service's ARN sentence, \
                 got {message:?}"
            );
        }
    }
}

/// Stage two: a well-formed ARN that names an authority this service does not hold is a
/// not-found about THE CA, whatever the operation was going to do with it.
#[tokio::test]
async fn a_well_formed_arn_naming_no_authority_is_refused_about_the_ca() {
    let emulator = Emulator::new();
    let (_existing, _certificate) = active_ca(&emulator, "missing-ca-neighbour").await;

    let rows: Vec<(&str, String)> = vec![
        (
            "an id no CA was minted with",
            format!(
                "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
                 7b9f4c1e-0d2a-4f6b-9c3e-5a8d1f2b4c60"
            ),
        ),
        (
            "an id of another shape entirely",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/not-a-uuid"),
        ),
        (
            "another account's CA",
            format!("arn:aws:acm-pca:{REGION}:111122223333:certificate-authority/abcdef"),
        ),
        (
            "another Region's CA",
            format!("arn:aws:acm-pca:us-gov-west-1:{ACCOUNT}:certificate-authority/abcdef"),
        ),
        (
            "another partition's CA",
            format!("arn:aws-cn:acm-pca:cn-north-1:{ACCOUNT}:certificate-authority/abcdef"),
        ),
    ];

    for (case, arn) in &rows {
        for operation in ARN_TAKING_OPERATIONS {
            let body = emulator
                .refused(
                    case,
                    operation,
                    &request(operation, arn),
                    "ResourceNotFoundException",
                )
                .await;
            let message = error_message(&body, case);
            assert_eq!(
                message, CA_NOT_FOUND_MESSAGE,
                "case {case:?}: expected {operation} to answer the service's CA not-found \
                 sentence, got {message:?}"
            );
        }
    }
}

/// Stage three: once the authority resolves, the absence reported is the SUB-RESOURCE's,
/// each with its own sentence — so a client can tell a CA that is gone from one that
/// simply does not carry what was asked of it.
#[tokio::test]
async fn a_resolvable_authority_reports_the_sub_resource_as_the_absent_one() {
    let emulator = Emulator::new();
    let (arn, _certificate) = active_ca(&emulator, "sub-resource-absences").await;

    // One row per sub-resource an operation of this service resolves ON a CA: the policy
    // attached to it, a grant held on it, and a certificate it issued.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "the policy of a CA that carries none",
            "GetPolicy",
            request("GetPolicy", &arn),
            POLICY_NOT_FOUND_MESSAGE,
        ),
        (
            "a grant the CA does not hold",
            "DeletePermission",
            request("DeletePermission", &arn),
            PERMISSION_NOT_FOUND_MESSAGE,
        ),
        (
            "a certificate the CA never issued",
            "GetCertificate",
            json!({
                "CertificateAuthorityArn": arn,
                "CertificateArn": format!("{arn}/certificate/0123456789abcdef"),
            }),
            CERTIFICATE_NOT_FOUND_MESSAGE,
        ),
    ];

    for (case, operation, body, expected) in &rows {
        let answered = emulator
            .refused(case, operation, body, "ResourceNotFoundException")
            .await;
        let message = error_message(&answered, case);
        assert_eq!(
            &message, expected,
            "case {case:?}: expected {operation} to name the sub-resource it was asked \
             for, got {message:?}"
        );
    }

    // And the authority itself is still there: the refusals above were about what it does
    // not carry, not about the CA. The delete of a sub-resource that is not there says the
    // same thing by succeeding: `DeletePolicy` is delete-if-exists, so a resolved authority
    // leaves it nothing to report.
    emulator
        .ok(
            "deleting the policy of a CA that carries none",
            "DeletePolicy",
            &request("DeletePolicy", &arn),
        )
        .await;
    let described = emulator
        .ok(
            "the authority itself",
            "DescribeCertificateAuthority",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    assert_eq!(
        described
            .get("CertificateAuthority")
            .and_then(|authority| authority.get("Arn"))
            .and_then(Value::as_str),
        Some(arn.as_str()),
        "expected the CA to still resolve to itself, got {described:?}"
    );
}
