// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the distinguished names ACM reports about a certificate:
//! `CertificateDetail$Subject` ("the name of the entity that is associated with the
//! public key contained in the certificate") and `CertificateDetail$Issuer` ("the name
//! of the certificate authority that issued and signed the certificate").
//!
//! RULE under test: a reported distinguished name lists the certificate's relative
//! distinguished names in the order its DER carries them, each written `Type=Value` and
//! separated by commas — so the name begins with the component the certificate encodes
//! FIRST, whichever component that is.
//!
//! Every certificate here is built by `rcgen` — something other than the code under
//! test — with its subject components pushed in a chosen order, and every assertion
//! goes through the same router the emulator serves production traffic on. The table
//! spans the class the rule generalises over: one component and several, the common
//! name first and last, and the same six components in both encoding orders.

use serde_json::json;

use crate::certificate_import::{blob, import};
use crate::certificate_lifecycle::{call, describe, router};

/// One row: what it exercises and the subject components, in the order the generated
/// certificate's DER will carry them.
type NameCase<'a> = (&'a str, &'a [(rcgen::DnType, &'a str)]);

/// The attribute-type short name RFC 4514 section 3 lists for each X.500 attribute type
/// a certificate name carries. Written out here so the expected string is built from
/// the test's own table rather than from the code under test.
fn short_name(attribute: &rcgen::DnType) -> &'static str {
    match attribute {
        rcgen::DnType::CommonName => "CN",
        rcgen::DnType::OrganizationalUnitName => "OU",
        rcgen::DnType::OrganizationName => "O",
        rcgen::DnType::LocalityName => "L",
        rcgen::DnType::StateOrProvinceName => "ST",
        rcgen::DnType::CountryName => "C",
        other => panic!("no short name is tabled for {other:?}"),
    }
}

/// The distinguished name `components` must be reported as: each component rendered
/// `Type=Value`, comma-separated, in the order the certificate encodes them.
fn rendered(components: &[(rcgen::DnType, &str)]) -> String {
    components
        .iter()
        .map(|(attribute, value)| format!("{}={value}", short_name(attribute)))
        .collect::<Vec<_>>()
        .join(",")
}

/// `rcgen`'s subject name for `components`, pushed in the order given — which is the
/// order the generated certificate's DER carries them in.
fn subject_name(components: &[(rcgen::DnType, &str)]) -> rcgen::DistinguishedName {
    let mut name = rcgen::DistinguishedName::new();
    for (attribute, value) in components {
        name.push(attribute.clone(), *value);
    }
    name
}

/// The `CertificateParams` of a certificate whose subject is `components` and which
/// covers `dns_names`, valid across any instant a test runs at.
fn params(components: &[(rcgen::DnType, &str)], dns_names: &[&str]) -> rcgen::CertificateParams {
    let names: Vec<String> = dns_names.iter().map(|name| (*name).to_string()).collect();
    let mut params = rcgen::CertificateParams::new(names).expect("certificate parameters");
    params.distinguished_name = subject_name(components);
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(2100, 1, 1);
    params
}

/// One generated certificate: the two PEM bodies an import carries.
struct Minted {
    certificate: String,
    private_key: String,
}

/// The `ImportCertificate` request body for one generated certificate.
fn import_body(minted: &Minted) -> serde_json::Value {
    json!({
        "Certificate": blob(&minted.certificate),
        "PrivateKey": blob(&minted.private_key),
    })
}

/// Generate a real self-signed certificate whose subject is `components`.
fn self_signed(components: &[(rcgen::DnType, &str)], dns_names: &[&str]) -> Minted {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("a key pair");
    let certificate = params(components, dns_names)
        .self_signed(&key)
        .expect("a self-signed certificate")
        .pem();
    minted(certificate, &key)
}

/// A certificate authority a leaf can be signed by: its own subject components, the
/// certificate itself and the key that signs with it.
struct Authority {
    certificate: rcgen::Certificate,
    key: rcgen::KeyPair,
}

impl Authority {
    /// Generate a real certificate authority whose subject is `components`.
    fn new(components: &[(rcgen::DnType, &str)]) -> Self {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("a key pair");
        let mut params = params(components, &[]);
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let certificate = params.self_signed(&key).expect("a certificate authority");
        Self { certificate, key }
    }

    /// Generate a real leaf certificate whose subject is `components`, signed by this
    /// authority — so the leaf's issuer name is this authority's subject name.
    fn sign(&self, components: &[(rcgen::DnType, &str)], dns_names: &[&str]) -> Minted {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).expect("a key pair");
        let certificate = params(components, dns_names)
            .signed_by(&key, &self.certificate, &self.key)
            .expect("a signed certificate")
            .pem();
        minted(certificate, &key)
    }
}

/// One generated certificate: its PEM body beside the PEM of the key it was generated
/// for.
fn minted(certificate: String, key: &rcgen::KeyPair) -> Minted {
    Minted {
        certificate,
        private_key: key.serialize_pem(),
    }
}

/// RULE: `DescribeCertificate` reports a certificate's `Subject` with its relative
/// distinguished names in the order the certificate's own DER carries them, written
/// `Type=Value` and comma-separated — so the reported name begins with the component
/// the certificate encodes first.
#[tokio::test]
async fn the_subject_is_reported_in_the_order_the_certificate_encodes_it() {
    // One row per class the rule generalises over: a single component; two components
    // either way round; and six components in the conventional most-general-first
    // encoding and in the opposite one.
    let cases: &[NameCase<'_>] = &[
        (
            "a common name alone",
            &[(rcgen::DnType::CommonName, "alone.example.com")],
        ),
        (
            "an organization alone",
            &[(rcgen::DnType::OrganizationName, "Example Organization")],
        ),
        (
            "a common name encoded before an organizational unit",
            &[
                (rcgen::DnType::CommonName, "cn-first.example.com"),
                (rcgen::DnType::OrganizationalUnitName, "Example Unit"),
            ],
        ),
        (
            "an organizational unit encoded before a common name",
            &[
                (rcgen::DnType::OrganizationalUnitName, "Example Unit"),
                (rcgen::DnType::CommonName, "ou-first.example.com"),
            ],
        ),
        (
            "six components, most general first",
            &[
                (rcgen::DnType::CountryName, "US"),
                (rcgen::DnType::StateOrProvinceName, "Washington"),
                (rcgen::DnType::LocalityName, "Seattle"),
                (rcgen::DnType::OrganizationName, "Example Organization"),
                (rcgen::DnType::OrganizationalUnitName, "Example Unit"),
                (rcgen::DnType::CommonName, "general-first.example.com"),
            ],
        ),
        (
            "the same six components, most specific first",
            &[
                (rcgen::DnType::CommonName, "specific-first.example.com"),
                (rcgen::DnType::OrganizationalUnitName, "Example Unit"),
                (rcgen::DnType::OrganizationName, "Example Organization"),
                (rcgen::DnType::LocalityName, "Seattle"),
                (rcgen::DnType::StateOrProvinceName, "Washington"),
                (rcgen::DnType::CountryName, "US"),
            ],
        ),
    ];

    let router = router();
    for (case, components) in cases {
        // Every certificate carries a DNS name of its own, so a row whose subject names
        // no common name is still a certificate ACM accepts.
        let generated = self_signed(components, &["named.example.com"]);
        let arn = import(&router, case, &import_body(&generated)).await;
        let detail = describe(&router, &arn).await;

        let expected = rendered(components);
        // A self-signed certificate is its own issuer, so both names are this subject.
        for member in ["Subject", "Issuer"] {
            let actual = detail[member].as_str().unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected:?}, got {actual:?}"
            );
        }
        let (first, value) = &components[0];
        let leading = format!("{}={value}", short_name(first));
        let subject = detail["Subject"].as_str().unwrap_or_default();
        assert!(
            subject.starts_with(&leading),
            "case {case:?}: expected the Subject to begin with the component the \
             certificate encodes first ({leading:?}), got {subject:?}"
        );
    }
}

/// RULE: the same ordering holds for `Issuer`, which is the SIGNING authority's name
/// read out of the certificate — so a leaf signed by another authority reports two
/// different names, each in its own encoding order.
#[tokio::test]
async fn the_issuer_is_the_signing_authoritys_name_in_its_own_order() {
    // The two names are ordered differently, so neither can be the other's rendering
    // and neither can be read off the wrong field.
    let authority_name: &[(rcgen::DnType, &str)] = &[
        (rcgen::DnType::CountryName, "US"),
        (rcgen::DnType::OrganizationName, "Example Authority"),
        (rcgen::DnType::CommonName, "ca.example.com"),
    ];
    let leaf_name: &[(rcgen::DnType, &str)] = &[
        (rcgen::DnType::CommonName, "signed.example.com"),
        (rcgen::DnType::OrganizationalUnitName, "Example Unit"),
    ];

    let case = "a leaf signed by an authority whose name is ordered differently";
    let router = router();
    let authority = Authority::new(authority_name);
    let leaf = authority.sign(leaf_name, &["signed.example.com"]);
    let authority_pem = authority.certificate.pem();

    // The chain "consists of the certificate of the issuing CA"
    // (`GetCertificate` doc), so the authority's own certificate is imported beside the
    // leaf — and must come back from `GetCertificate` byte for byte.
    let mut body = import_body(&leaf);
    body["CertificateChain"] = blob(&authority_pem);
    let arn = import(&router, case, &body).await;

    let detail = describe(&router, &arn).await;
    let expected_subject = rendered(leaf_name);
    let expected_issuer = rendered(authority_name);
    let subject = detail["Subject"].as_str().unwrap_or_default();
    let issuer = detail["Issuer"].as_str().unwrap_or_default();
    assert_eq!(
        subject, expected_subject,
        "case {case:?}: expected Subject {expected_subject:?}, got {subject:?}"
    );
    assert_eq!(
        issuer, expected_issuer,
        "case {case:?}: expected Issuer {expected_issuer:?}, got {issuer:?}"
    );

    let (status, response) = call(
        &router,
        "GetCertificate",
        &json!({ "CertificateArn": &arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCertificate must answer 200, got {status} {response}"
    );
    assert_eq!(
        response["Certificate"].as_str(),
        Some(leaf.certificate.as_str()),
        "case {case:?}: expected the leaf back as imported, got {response}"
    );
    assert_eq!(
        response["CertificateChain"].as_str(),
        Some(authority_pem.as_str()),
        "case {case:?}: expected the chain back byte for byte as imported, got {response}"
    );
}
