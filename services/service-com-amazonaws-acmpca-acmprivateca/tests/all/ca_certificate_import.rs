// SPDX-License-Identifier: CC0-1.0

//! What `ImportCertificateAuthorityCertificate` takes, and what it refuses.
//!
//! Two rules, in the order the service applies them.
//!
//! The SHAPE rule: the certificate member of a request is published with a PEM shape, and a
//! body outside that shape is refused by request validation, naming the member and quoting
//! the expression it must match — before anything tries to read a certificate out of it,
//! which is what the modelled malformed-certificate failure is for.
//!
//! The IDENTITY rule: a private CA signs with the key pair IT holds, so the certificate it
//! takes must be the one issued for the certification request it published — the same
//! entity as that request's subject, over the same public key. A certificate for another
//! subject, or over another key, is refused as the mismatch it is and the CA keeps waiting
//! for its certificate. The entity is what counts, not the bytes the name was written in:
//! the same attributes in another order or another case are the same entity.

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::state::MALFORMED_CERTIFICATE_MESSAGE;
use std::str::FromStr;
use x509_cert::name::Name;

use crate::private_ca_lifecycle::{
    blob, ca_certificate_for, certificate_over, csr_pem, pem, pem_of_bytes, request_of,
    root_ca_request, string, wait_for_the_clock, Emulator,
};
use crate::resource_policy::error_message;

/// The refusal a value outside the certificate member's published PEM shape carries: the
/// count envelope, the member's wire name, and the expression the value must match.
///
/// Written out here rather than built from the service's own constant, so a change to
/// either has to be made in both.
const SHAPE_REFUSAL: &str = concat!(
    "1 validation error detected: Value at 'certificate' failed to satisfy constraint: ",
    r"Member must satisfy regular expression pattern: -----BEGIN CERTIFICATE-----\r?\n",
    r"([A-Za-z0-9/+]{64}\r?\n)*[A-Za-z0-9/+]{1,64}={0,2}\r?\n-----END CERTIFICATE-----",
    r"(\r?\n)?."
);

/// A CA waiting for its certificate, and the certification request it published.
///
/// The subject carries six X.500 attributes, so a row can diverge from it in one attribute
/// or in the order they are written.
async fn pending_ca(emulator: &Emulator, case: &str, common_name: &str) -> (String, String) {
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &create_request(common_name))
        .await;
    let arn = string(&created, "CertificateAuthorityArn", case);
    // A test may wait for a due instant; a handler may not.
    wait_for_the_clock();
    let reported = emulator
        .ok(
            case,
            "GetCertificateAuthorityCsr",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    (arn, string(&reported, "Csr", case))
}

/// A `CreateCertificateAuthority` body whose subject is the six-attribute one above.
fn create_request(common_name: &str) -> Value {
    let mut body = root_ca_request(common_name);
    body["CertificateAuthorityConfiguration"]["Subject"] = json!({
        "CommonName": common_name,
        "Country": "US",
        "State": "WA",
        "Locality": "Seattle",
        "Organization": "Example Inc",
        "OrganizationalUnit": "PKI",
    });
    body
}

/// An `ImportCertificateAuthorityCertificate` body.
fn import(arn: &str, certificate: &str) -> Value {
    json!({"CertificateAuthorityArn": arn, "Certificate": blob(certificate)})
}

/// A body outside the published PEM shape is refused by request validation, naming the
/// member and the expression; one INSIDE the shape reaches the certificate decode and is
/// refused as malformed instead.
///
/// The rows vary along what the expression says: the armour, its casing, the base64
/// alphabet, the line length, where padding may appear, and whether the block is closed.
#[tokio::test]
async fn a_certificate_body_is_held_to_the_published_pem_shape_first() {
    let emulator = Emulator::new();
    let case = "shape";
    let (arn, csr) = pending_ca(&emulator, case, "shape-root").await;
    let own = ca_certificate_for(&csr);
    let body = crate::private_ca_lifecycle::certificate_der(&own, case);
    let base64 = ::aws_smithy_types::base64::encode(&body);

    // Each row is a body the published expression does not match.
    let outside: &[(&str, String)] = &[
        ("no armour at all", "a certificate".to_string()),
        ("the certification request armour", csr.clone()),
        (
            "a body outside the base64 alphabet",
            "-----BEGIN CERTIFICATE-----\nnot base64 at all!!\n-----END CERTIFICATE-----\n"
                .to_string(),
        ),
        (
            "an empty body",
            "-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n".to_string(),
        ),
        (
            "a block that is never closed",
            format!("-----BEGIN CERTIFICATE-----\n{}\n", &base64[..40]),
        ),
        (
            "lines longer than the 64 the expression allows",
            format!(
                "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
                base64
                    .as_bytes()
                    .chunks(65)
                    .map(|line| String::from_utf8_lossy(line).to_string())
                    .collect::<Vec<String>>()
                    .join("\n")
            ),
        ),
        (
            "padding before the last line",
            format!(
                "-----BEGIN CERTIFICATE-----\n{}=\n{}\n-----END CERTIFICATE-----\n",
                &base64[..63],
                &base64[63..100]
            ),
        ),
        (
            "armour in the wrong case",
            own.replace("-----BEGIN CERTIFICATE-----", "-----begin certificate-----"),
        ),
        (
            "text before the armour",
            format!("certificate follows\n{own}"),
        ),
    ];
    for (row, certificate) in outside {
        let refused = emulator
            .refused(
                row,
                "ImportCertificateAuthorityCertificate",
                &import(&arn, certificate),
                "ValidationException",
            )
            .await;
        let message = error_message(&refused, row);
        assert_eq!(
            message, SHAPE_REFUSAL,
            "case {row:?}: expected the member's shape refusal, got {message:?}"
        );
    }

    // Inside the shape, and not a certificate: the decode's own refusal, which is a
    // different error class and a different sentence.
    let inside: &[(&str, String)] = &[
        ("base64 that is not der", pem("CERTIFICATE", "a certificate")),
        (
            "a truncated certificate",
            pem_of_bytes("CERTIFICATE", &body[..body.len() / 2]),
        ),
    ];
    for (row, certificate) in inside {
        let refused = emulator
            .refused(
                row,
                "ImportCertificateAuthorityCertificate",
                &import(&arn, certificate),
                "MalformedCertificateException",
            )
            .await;
        let message = error_message(&refused, row);
        assert_eq!(
            message, MALFORMED_CERTIFICATE_MESSAGE,
            "case {row:?}: expected the malformed-certificate sentence, got {message:?}"
        );
    }

    // And the CA took nothing from any of them: it still waits for its certificate.
    let described = emulator
        .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
        .await;
    assert_eq!(
        string(&described["CertificateAuthority"], "Status", case),
        "PENDING_CERTIFICATE",
        "case {case:?}: a refused import must leave the CA waiting for its certificate"
    );
}

/// The certificate a CA takes is the one its own request asks for: another subject or
/// another key is the mismatch, told apart by which field diverges.
#[tokio::test]
async fn only_the_certificate_the_cas_own_request_asks_for_is_taken() {
    let emulator = Emulator::new();

    let (arn, csr) = pending_ca(&emulator, "the CA under test", "import-root").await;
    // A second CA, created with the SAME subject, is where a different key of the same
    // algorithm comes from.
    let (_other, other_csr) = pending_ca(&emulator, "another CA", "import-root").await;
    // And a third, with its own subject, for the certificate that belongs to another CA
    // entirely.
    let (_foreign, foreign_csr) = pending_ca(&emulator, "a foreign CA", "foreign-root").await;

    let own = request_of(&csr, "the CA under test");
    let other = request_of(&other_csr, "another CA");
    let foreign = request_of(&foreign_csr, "a foreign CA");

    // The subject of the CA's own request, written the way another writer might: the same
    // attributes, in the reverse order, with the common name in another case.
    let reordered = Name::from_str("OU=PKI,O=Example Inc,L=Seattle,ST=WA,C=US,CN=IMPORT-ROOT")
        .expect("an RFC 4514 distinguished name");
    // The same subject with one attribute dropped is a different entity.
    let shortened = Name::from_str("CN=import-root,C=US,ST=WA,L=Seattle,O=Example Inc")
        .expect("an RFC 4514 distinguished name");

    // (case, the certificate presented, the sentence the refusal carries)
    let refused: Vec<(&str, String, &str)> = vec![
        (
            "the CA's subject over another key of the same algorithm",
            certificate_over(&own.info.subject, &other.info.public_key, &own.algorithm),
            "The certificate subject public key info field contains a different public key \
             or key algorithm than the CA certificate signing request.",
        ),
        (
            "the CA's own key under another subject",
            certificate_over(&foreign.info.subject, &own.info.public_key, &own.algorithm),
            "The certificate subject contains a different subject than the CA certificate \
             signing request.",
        ),
        (
            "another CA's certificate entirely",
            ca_certificate_for(&foreign_csr),
            "The certificate subject contains a different subject than the CA certificate \
             signing request.",
        ),
        (
            "the CA's own key under a subject missing an attribute",
            certificate_over(&shortened, &own.info.public_key, &own.algorithm),
            "The certificate subject contains a different subject than the CA certificate \
             signing request.",
        ),
    ];

    for (case, certificate, sentence) in &refused {
        let answer = emulator
            .refused(
                case,
                "ImportCertificateAuthorityCertificate",
                &import(&arn, certificate),
                "CertificateMismatchException",
            )
            .await;
        let message = error_message(&answer, case);
        assert_eq!(
            message, *sentence,
            "case {case:?}: expected the mismatch sentence for the field that diverges, \
             got {message:?}"
        );
        // Nothing was installed: the CA still waits, and still has no certificate to hand
        // out.
        let described = emulator
            .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
            .await;
        assert_eq!(
            string(&described["CertificateAuthority"], "Status", case),
            "PENDING_CERTIFICATE",
            "case {case:?}: a refused import must leave the CA waiting for its certificate"
        );
        emulator
            .refused(
                case,
                "GetCertificateAuthorityCertificate",
                &json!({"CertificateAuthorityArn": arn}),
                "InvalidStateException",
            )
            .await;
    }

    // The subject written another way is the same entity, so that certificate IS the CA's
    // own and the import takes it — which is what proves the refusals above are about the
    // entity and the key rather than about the bytes of a name.
    let case = "the CA's subject in another order and another case";
    let accepted = certificate_over(&reordered, &own.info.public_key, &own.algorithm);
    emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &import(&arn, &accepted),
        )
        .await;
    let described = emulator
        .ok(case, "DescribeCertificateAuthority", &json!({"CertificateAuthorityArn": arn}))
        .await;
    assert_eq!(
        string(&described["CertificateAuthority"], "Status", case),
        "ACTIVE",
        "case {case:?}: the certificate of the CA's own request activates it"
    );
    // And the CA now signs with it, which is what an activated CA is for.
    let case = "a leaf from the activated CA";
    let issued = emulator
        .ok(
            case,
            "IssueCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "Csr": blob(&csr_pem("an-end-entity")),
                "SigningAlgorithm": "SHA256WITHRSA",
                "Validity": {"Type": "DAYS", "Value": 30},
            }),
        )
        .await;
    assert!(
        string(&issued, "CertificateArn", case).starts_with(&format!("{arn}/certificate/")),
        "case {case:?}: an ACTIVE CA issues certificates of its own"
    );
}
