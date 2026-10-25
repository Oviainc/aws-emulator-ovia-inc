// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ImportCertificate`: a client brings its own certificate and
//! private key, and every fact ACM afterwards reports about that certificate is read
//! out of the certificate itself.
//!
//! Every assertion goes through the same router the emulator serves production traffic
//! on, and every certificate the service is fed is built by `rcgen` — something other
//! than the code under test — so a response that is not computed from the imported
//! body cannot pass. Each case writes with `ImportCertificate` and reads back with
//! `DescribeCertificate`, `ListCertificates`, `SearchCertificates` or
//! `ListTagsForCertificate`.
//!
//! The tables below span the classes the rule generalises over: the key families and
//! curves ACM declares, certificates carrying one name and several, a name in the
//! subject and a name only in the extension, wildcard and plain names, validity
//! windows open and closed, key-usage extensions present and absent, and — for the
//! refusals — each guideline this operation documents for a third-party certificate.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use x509_cert::der::DecodePem;

use crate::certificate_lifecycle::{
    call, error_code, error_message, list_all_arns, request_certificate, router, ACCOUNT, REGION,
};

/// One imported certificate, as the test generated it.
struct Generated {
    /// The PEM body handed to `ImportCertificate` as `Certificate`.
    certificate: String,
    /// The PEM body handed to it as `PrivateKey`.
    private_key: String,
    /// The certificate's own serial, as the detail must report it: lowercase hex of
    /// the X.509 serial number's bytes.
    serial: String,
    /// The certificate's own validity window, in whole epoch seconds.
    not_before: f64,
    not_after: f64,
}

/// What one generated certificate is built from: the subject common name (if any),
/// the subject organization (if any), the DNS subject alternative names, the signature
/// algorithm its key pair uses, whether it asserts key usages, and its validity window
/// as `(not_before, not_after)` years.
struct Blueprint<'a> {
    common_name: Option<&'a str>,
    organization: Option<&'a str>,
    dns_names: &'a [&'a str],
    algorithm: &'static rcgen::SignatureAlgorithm,
    key_usages: bool,
    validity: (i32, i32),
}

impl Default for Blueprint<'_> {
    fn default() -> Self {
        Self {
            common_name: Some("import.example.com"),
            organization: None,
            dns_names: &[],
            // The default `rcgen` key pair, an ECDSA P-256 one.
            algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
            key_usages: false,
            validity: (2020, 2100),
        }
    }
}

/// Generate a real self-signed certificate and its private key from `blueprint`.
fn generate(blueprint: &Blueprint<'_>) -> Generated {
    let key = rcgen::KeyPair::generate_for(blueprint.algorithm).expect("a key pair");
    generate_signed_by(blueprint, &key)
}

/// Generate a real self-signed certificate from `blueprint`, signed by `key`, and
/// return it beside THAT key's PEM.
fn generate_signed_by(blueprint: &Blueprint<'_>, key: &rcgen::KeyPair) -> Generated {
    let names: Vec<String> = blueprint
        .dns_names
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let mut params = rcgen::CertificateParams::new(names).expect("certificate parameters");
    let mut subject = rcgen::DistinguishedName::new();
    if let Some(common_name) = blueprint.common_name {
        subject.push(rcgen::DnType::CommonName, common_name);
    }
    if let Some(organization) = blueprint.organization {
        subject.push(rcgen::DnType::OrganizationName, organization);
    }
    params.distinguished_name = subject;
    params.not_before = rcgen::date_time_ymd(blueprint.validity.0, 1, 1);
    params.not_after = rcgen::date_time_ymd(blueprint.validity.1, 1, 1);
    if blueprint.key_usages {
        params.key_usages = vec![
            rcgen::KeyUsagePurpose::DigitalSignature,
            rcgen::KeyUsagePurpose::KeyEncipherment,
        ];
        params.extended_key_usages = vec![
            rcgen::ExtendedKeyUsagePurpose::ServerAuth,
            rcgen::ExtendedKeyUsagePurpose::ClientAuth,
        ];
    }
    let certificate = params.self_signed(key).expect("a self-signed certificate").pem();
    let parsed = x509_cert::Certificate::from_pem(&certificate).expect("the certificate parses");
    Generated {
        serial: hex::encode(parsed.tbs_certificate.serial_number.as_bytes()),
        not_before: epoch_seconds(parsed.tbs_certificate.validity.not_before.to_system_time()),
        not_after: epoch_seconds(parsed.tbs_certificate.validity.not_after.to_system_time()),
        certificate,
        private_key: key.serialize_pem(),
    }
}

/// One `ImportCertificate` case: what it exercises, the certificate to generate, and
/// what the detail must then report — the `DomainName`, the `SubjectAlternativeNames`,
/// the `KeyAlgorithm`, the `SignatureAlgorithm` and the distinguished name.
type ImportCase<'a> = (
    &'a str,
    Blueprint<'a>,
    &'a str,
    &'a [&'a str],
    &'a str,
    &'a str,
    &'a str,
);

/// An instant as the whole epoch seconds a timestamp member serialises to.
fn epoch_seconds(at: std::time::SystemTime) -> f64 {
    at.duration_since(std::time::UNIX_EPOCH)
        .expect("a validity bound after the epoch")
        .as_secs() as f64
}

/// A PEM body as the base64 a blob member carries on the wire.
fn blob(pem: &str) -> Value {
    json!(aws_smithy_types::base64::encode(pem.as_bytes()))
}

/// The `ImportCertificate` request body for one generated certificate.
fn import_body(generated: &Generated) -> Value {
    json!({
        "Certificate": blob(&generated.certificate),
        "PrivateKey": blob(&generated.private_key),
    })
}

/// Import one certificate, requiring success, and return the ARN it was stored under.
async fn import(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, response) = call(router, "ImportCertificate", body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ImportCertificate must answer 200, got {status} {response}"
    );
    response["CertificateArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no CertificateArn in {response}"))
        .to_string()
}

/// `DescribeCertificate`'s detail for `arn`, requiring success.
async fn describe(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, response) = call(
        router,
        "DescribeCertificate",
        &json!({ "CertificateArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeCertificate must answer 200, got {status} {response}"
    );
    response["Certificate"].clone()
}

/// The string members of a detail, as a `Vec<&str>` for comparison.
fn names_of(detail: &Value, member: &str, case: &str) -> Vec<String> {
    detail[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {detail}"))
        .iter()
        .map(|name| name.as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: an import stores ONE certificate record read out of the X.509 body the
/// request carried — the names it covers, its subject and issuer, its serial, its own
/// validity window, its key and signature algorithms and its key-usage extensions —
/// typed `IMPORTED`, already `ISSUED` and with a client-provided key pair; and every
/// read projects that record.
#[tokio::test]
async fn describe_reports_the_imported_certificate() {
    // One row per class the rule generalises over: key family and curve, the number of
    // names, where the name comes from (subject, extension or both), the form of the
    // name, and whether the certificate asserts key usages.
    //
    // `KeyAlgorithm` is reported in the X.509 descriptor spelling the detail uses
    // (`EC-prime256v1`), and `SignatureAlgorithm` as `<digest>WITH<family>`. Each
    // certificate here is self-signed, so its issuer distinguished name is its
    // subject's.
    let cases: &[ImportCase<'_>] = &[
        (
            "a subject common name only, P-256",
            Blueprint::default(),
            "import.example.com",
            &["import.example.com"],
            "EC-prime256v1",
            "SHA256WITHECDSA",
            "CN=import.example.com",
        ),
        (
            "a common name and three DNS names, P-256",
            Blueprint {
                common_name: Some("one.example.com"),
                dns_names: &["two.example.com", "three.example.com", "four.example.com"],
                ..Blueprint::default()
            },
            "one.example.com",
            &[
                "one.example.com",
                "two.example.com",
                "three.example.com",
                "four.example.com",
            ],
            "EC-prime256v1",
            "SHA256WITHECDSA",
            "CN=one.example.com",
        ),
        (
            "a common name repeated in the extension is covered once",
            Blueprint {
                common_name: Some("same.example.com"),
                dns_names: &["same.example.com", "other.example.com"],
                ..Blueprint::default()
            },
            "same.example.com",
            &["same.example.com", "other.example.com"],
            "EC-prime256v1",
            "SHA256WITHECDSA",
            "CN=same.example.com",
        ),
        (
            "no common name: the first DNS name is the primary one",
            Blueprint {
                common_name: None,
                organization: Some("Example Org"),
                dns_names: &["only.example.net", "second.example.net"],
                ..Blueprint::default()
            },
            "only.example.net",
            &["only.example.net", "second.example.net"],
            "EC-prime256v1",
            "SHA256WITHECDSA",
            "O=Example Org",
        ),
        (
            "a wildcard name, P-384, asserting key usages",
            Blueprint {
                common_name: Some("*.wild.example.org"),
                dns_names: &["*.wild.example.org"],
                algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
                key_usages: true,
                ..Blueprint::default()
            },
            "*.wild.example.org",
            &["*.wild.example.org"],
            "EC-secp384r1",
            "SHA384WITHECDSA",
            "CN=*.wild.example.org",
        ),
        (
            "a validity window of its own, P-384",
            Blueprint {
                common_name: Some("window.example.com"),
                algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
                validity: (2021, 2061),
                ..Blueprint::default()
            },
            "window.example.com",
            &["window.example.com"],
            "EC-secp384r1",
            "SHA384WITHECDSA",
            "CN=window.example.com",
        ),
    ];

    let router = router();
    for (case, blueprint, domain, names, key_algorithm, signature_algorithm, distinguished_name) in
        cases
    {
        let generated = generate(blueprint);
        let arn = import(&router, case, &import_body(&generated)).await;
        assert!(
            arn.starts_with(&format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/")),
            "case {case:?}: expected an ACM certificate ARN in {REGION}, got {arn:?}"
        );

        let detail = describe(&router, case, &arn).await;
        assert_eq!(
            detail["CertificateArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the detail to report the ARN it was stored under, got {detail}"
        );
        assert_eq!(
            detail["DomainName"].as_str(),
            Some(*domain),
            "case {case:?}: expected DomainName {domain:?}, got {detail}"
        );
        assert_eq!(
            names_of(&detail, "SubjectAlternativeNames", case),
            names.iter().map(|name| (*name).to_string()).collect::<Vec<_>>(),
            "case {case:?}: expected the certificate's own names, got {detail}"
        );
        for (member, expected) in [
            ("Type", "IMPORTED"),
            ("Status", "ISSUED"),
            ("RenewalEligibility", "INELIGIBLE"),
            ("CertificateKeyPairOrigin", "CUSTOMER_PROVIDED"),
            ("KeyAlgorithm", key_algorithm),
            ("SignatureAlgorithm", signature_algorithm),
        ] {
            assert_eq!(
                detail[member].as_str(),
                Some(expected),
                "case {case:?}: expected {member} {expected:?}, got {detail}"
            );
        }
        // A self-signed certificate is its own issuer, so both distinguished names are
        // the subject's — and each is the certificate's own, not one ACM composed.
        for member in ["Subject", "Issuer"] {
            assert_eq!(
                detail[member].as_str(),
                Some(*distinguished_name),
                "case {case:?}: expected {member} {distinguished_name:?}, got {detail}"
            );
        }
        assert_eq!(
            detail["Serial"].as_str(),
            Some(generated.serial.as_str()),
            "case {case:?}: expected the certificate's own serial {:?}, got {detail}",
            generated.serial
        );
        assert_eq!(
            detail["NotBefore"].as_f64(),
            Some(generated.not_before),
            "case {case:?}: expected the certificate's own NotBefore {}, got {detail}",
            generated.not_before
        );
        assert_eq!(
            detail["NotAfter"].as_f64(),
            Some(generated.not_after),
            "case {case:?}: expected the certificate's own NotAfter {}, got {detail}",
            generated.not_after
        );
        // "This value exists only when the certificate type is IMPORTED"
        // (`CertificateDetail$ImportedAt`), and the two instants that belong to a
        // certificate ACM issued or was asked to issue do not.
        assert!(
            detail["ImportedAt"].is_number(),
            "case {case:?}: expected an ImportedAt instant, got {detail}"
        );
        for absent in ["CreatedAt", "IssuedAt", "DomainValidationOptions", "FailureReason"] {
            assert!(
                detail[absent].is_null(),
                "case {case:?}: {absent} does not belong to an imported certificate, got {detail}"
            );
        }

        let key_usages: Vec<String> = detail["KeyUsages"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no KeyUsages in {detail}"))
            .iter()
            .map(|usage| usage["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        let extended: Vec<String> = detail["ExtendedKeyUsages"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no ExtendedKeyUsages in {detail}"))
            .iter()
            .map(|usage| usage["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        let (expected_usages, expected_extended): (Vec<&str>, Vec<&str>) = if blueprint.key_usages {
            (
                vec!["DIGITAL_SIGNATURE", "KEY_ENCIPHERMENT"],
                vec![
                    "TLS_WEB_SERVER_AUTHENTICATION",
                    "TLS_WEB_CLIENT_AUTHENTICATION",
                ],
            )
        } else {
            (Vec::new(), Vec::new())
        };
        assert_eq!(
            key_usages, expected_usages,
            "case {case:?}: expected the certificate's own key usages, got {detail}"
        );
        assert_eq!(
            extended, expected_extended,
            "case {case:?}: expected the certificate's own extended key usages, got {detail}"
        );
    }
}

/// RULE: the stored import is one record, so the list and search operations report the
/// same facts the detail does — and a search selects it on them.
#[tokio::test]
async fn the_import_is_listed_and_searchable_on_what_it_stored() {
    let router = router();
    let generated = generate(&Blueprint {
        common_name: Some("searchable.example.com"),
        dns_names: &["alias.searchable.example.com"],
        ..Blueprint::default()
    });
    let case = "an imported EC certificate";
    let arn = import(&router, case, &import_body(&generated)).await;

    // `ListCertificates` filters on the key algorithms the request names; an EC
    // certificate is listed when the request asks for its algorithm.
    let listed = list_all_arns(
        &router,
        &json!({ "Includes": { "keyTypes": ["EC_prime256v1"] } }),
    )
    .await;
    assert!(
        listed.contains(&arn),
        "case {case:?}: expected the imported certificate to be listed, got {listed:?}"
    );

    // Each filter selects on a value only the imported certificate carries.
    let searches: &[(&str, Value)] = &[
        (
            "its type",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "Type": "IMPORTED" } } }),
        ),
        (
            "its key pair origin",
            json!({ "Filter": { "AcmCertificateMetadataFilter": { "CertificateKeyPairOrigin": "CUSTOMER_PROVIDED" } } }),
        ),
        (
            "its own serial",
            json!({ "Filter": { "X509AttributeFilter": { "SerialNumber": generated.serial } } }),
        ),
        (
            "its key algorithm",
            json!({ "Filter": { "X509AttributeFilter": { "KeyAlgorithm": "EC_prime256v1" } } }),
        ),
    ];
    for (selector, body) in searches {
        let (status, response) = call(&router, "SearchCertificates", body).await;
        assert_eq!(
            status, 200,
            "case {selector:?}: SearchCertificates must answer 200, got {status} {response}"
        );
        let found: Vec<&str> = response["Results"]
            .as_array()
            .unwrap_or_else(|| panic!("case {selector:?}: no Results in {response}"))
            .iter()
            .map(|result| result["CertificateArn"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            found,
            vec![arn.as_str()],
            "case {selector:?}: expected the search on {selector} to select the import, got {response}"
        );
        let result = &response["Results"][0];
        assert_eq!(
            result["X509Attributes"]["SerialNumber"].as_str(),
            Some(generated.serial.as_str()),
            "case {selector:?}: expected the stored serial in the result, got {result}"
        );
        assert!(
            result["CertificateMetadata"]["AcmCertificateMetadata"]["ImportedAt"].is_number(),
            "case {selector:?}: expected an ImportedAt instant in the result, got {result}"
        );
    }
}

/// RULE: a request that names an imported certificate's ARN replaces THAT certificate
/// instead of creating a second one, and the ACM resource's own facts — the ARN and
/// the tags it carries — survive the replacement.
#[tokio::test]
async fn a_reimport_replaces_the_certificate_under_the_same_arn() {
    let router = router();
    let first = generate(&Blueprint {
        common_name: Some("first.example.com"),
        ..Blueprint::default()
    });
    let case = "reimport";
    let mut body = import_body(&first);
    body["Tags"] = json!([{ "Key": "owner", "Value": "platform" }]);
    let arn = import(&router, case, &body).await;

    // Replacing it with a certificate of another curve, another name set and another
    // validity window: every one of those facts must follow the new certificate.
    let second = generate(&Blueprint {
        common_name: Some("second.example.com"),
        dns_names: &["extra.second.example.com"],
        algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
        validity: (2022, 2062),
        key_usages: true,
        ..Blueprint::default()
    });
    let mut replacement = import_body(&second);
    replacement["CertificateArn"] = json!(arn);
    let returned = import(&router, case, &replacement).await;
    assert_eq!(
        returned, arn,
        "case {case:?}: a reimport must answer the ARN it replaced, got {returned:?}"
    );

    let detail = describe(&router, case, &arn).await;
    assert_eq!(
        detail["DomainName"].as_str(),
        Some("second.example.com"),
        "case {case:?}: expected the replacement's name, got {detail}"
    );
    assert_eq!(
        names_of(&detail, "SubjectAlternativeNames", case),
        vec![
            "second.example.com".to_string(),
            "extra.second.example.com".to_string()
        ],
        "case {case:?}: expected the replacement's name set, got {detail}"
    );
    assert_eq!(
        detail["Serial"].as_str(),
        Some(second.serial.as_str()),
        "case {case:?}: expected the replacement's serial, got {detail}"
    );
    assert_eq!(
        detail["NotAfter"].as_f64(),
        Some(second.not_after),
        "case {case:?}: expected the replacement's validity window, got {detail}"
    );
    assert_eq!(
        detail["KeyAlgorithm"].as_str(),
        Some("EC-secp384r1"),
        "case {case:?}: expected the replacement's key algorithm, got {detail}"
    );

    // One resource, not two.
    let listed = list_all_arns(
        &router,
        &json!({ "Includes": { "keyTypes": ["EC_prime256v1", "EC_secp384r1"] } }),
    )
    .await;
    assert_eq!(
        listed,
        vec![arn.clone()],
        "case {case:?}: a reimport must not create a second certificate, got {listed:?}"
    );

    // The tags belong to the ACM resource, and a reimport may not carry any.
    let (status, tags) = call(
        &router,
        "ListTagsForCertificate",
        &json!({ "CertificateArn": arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForCertificate must answer 200, got {status} {tags}"
    );
    assert_eq!(
        tags["Tags"],
        json!([{ "Key": "owner", "Value": "platform" }]),
        "case {case:?}: expected the resource's tags to survive the reimport, got {tags}"
    );
}

/// RULE: each guideline this operation documents for a third-party certificate is a
/// refusal of the request, under the modelled error for what was wrong with it — never
/// a stored certificate and never a 501.
#[tokio::test]
async fn an_import_that_breaks_a_guideline_is_refused() {
    let router = router();

    // A certificate whose validity window is closed, one whose window has not opened,
    // one of an algorithm outside the `KeyAlgorithm` enum (Ed25519), and the key and
    // chain bodies in each form the guidelines exclude.
    let expired = generate(&Blueprint {
        common_name: Some("expired.example.com"),
        validity: (2000, 2001),
        ..Blueprint::default()
    });
    let not_yet = generate(&Blueprint {
        common_name: Some("future.example.com"),
        validity: (2990, 2999),
        ..Blueprint::default()
    });
    let unsupported = generate(&Blueprint {
        common_name: Some("ed25519.example.com"),
        algorithm: &rcgen::PKCS_ED25519,
        ..Blueprint::default()
    });
    let valid = generate(&Blueprint::default());
    let other_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .expect("another P-256 key pair");
    let other_algorithm =
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P384_SHA384).expect("a P-384 key pair");
    // The same key, framed as an encrypted private key (RFC 7468 section 11), which is
    // the one form of key an import may not carry.
    let encrypted = format!(
        "-----BEGIN ENCRYPTED PRIVATE KEY-----\n{}\n-----END ENCRYPTED PRIVATE KEY-----\n",
        aws_smithy_types::base64::encode(valid.private_key.as_bytes())
    );

    let cases: &[(&str, Value, &str)] = &[
        (
            "the Certificate is not PEM",
            json!({
                "Certificate": blob("this is not a certificate"),
                "PrivateKey": blob(&valid.private_key),
            }),
            "InvalidParameterException",
        ),
        (
            "the Certificate is a PEM block that is not a certificate",
            json!({
                "Certificate": blob("-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----\n"),
                "PrivateKey": blob(&valid.private_key),
            }),
            "InvalidParameterException",
        ),
        (
            "the certificate's validity window has closed",
            import_body(&expired),
            "InvalidParameterException",
        ),
        (
            "the certificate's validity window has not opened",
            import_body(&not_yet),
            "InvalidParameterException",
        ),
        (
            "the certificate's key algorithm is not one ACM declares",
            import_body(&unsupported),
            "InvalidParameterException",
        ),
        (
            "the PrivateKey is not PEM",
            json!({
                "Certificate": blob(&valid.certificate),
                "PrivateKey": blob("not a key"),
            }),
            "InvalidParameterException",
        ),
        (
            "the PrivateKey is encrypted",
            json!({
                "Certificate": blob(&valid.certificate),
                "PrivateKey": blob(&encrypted),
            }),
            "InvalidParameterException",
        ),
        (
            // `rcgen` writes the optional public key into the PKCS#8 EC key it
            // serialises, so this row's key is compared against the certificate's own.
            // A key that OMITS it (SEC1 appendix C.4 allows that) is held to its
            // algorithm alone, which is the documented limit of the match: the service
            // derives no public key from a secret scalar.
            "the PrivateKey is another key pair's",
            json!({
                "Certificate": blob(&valid.certificate),
                "PrivateKey": blob(&other_key.serialize_pem()),
            }),
            "InvalidParameterException",
        ),
        (
            "the PrivateKey is of another algorithm",
            json!({
                "Certificate": blob(&valid.certificate),
                "PrivateKey": blob(&other_algorithm.serialize_pem()),
            }),
            "InvalidParameterException",
        ),
        (
            "the CertificateChain is not PEM",
            json!({
                "Certificate": blob(&valid.certificate),
                "PrivateKey": blob(&valid.private_key),
                "CertificateChain": blob("not a chain"),
            }),
            "InvalidParameterException",
        ),
        (
            "a tag key reserved for Amazon Web Services",
            json!({
                "Certificate": blob(&valid.certificate),
                "PrivateKey": blob(&valid.private_key),
                "Tags": [{ "Key": "aws:owner", "Value": "no" }],
            }),
            "InvalidTagException",
        ),
        (
            "the CertificateArn to replace is not an ACM certificate ARN",
            json!({
                "Certificate": blob(&valid.certificate),
                "PrivateKey": blob(&valid.private_key),
                "CertificateArn": format!("arn:aws:acm:{REGION}:{ACCOUNT}:not-a-certificate"),
            }),
            "InvalidArnException",
        ),
        (
            "the CertificateArn to replace names no stored certificate",
            json!({
                "Certificate": blob(&valid.certificate),
                "PrivateKey": blob(&valid.private_key),
                "CertificateArn": format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555"),
            }),
            "ResourceNotFoundException",
        ),
    ];

    for (case, body, expected) in cases {
        let (status, response) = call(&router, "ImportCertificate", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the import to be refused with 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            *expected,
            "case {case:?}: expected {expected}, got {response}"
        );
        assert!(
            !error_message(&response).is_empty(),
            "case {case:?}: a refusal must say what was wrong, got {response}"
        );
    }

    // Nothing above was stored: every refusal left the certificate store as it was.
    let listed = list_all_arns(
        &router,
        &json!({ "Includes": { "keyTypes": ["EC_prime256v1", "EC_secp384r1", "RSA_2048"] } }),
    )
    .await;
    assert!(
        listed.is_empty(),
        "case {:?}: a refused import must store nothing, got {listed:?}",
        "refusals store nothing"
    );
}

/// RULE: `CertificateArn` names "an imported certificate to replace", so a reimport
/// onto a certificate ACM was asked to issue is refused — and tags, which belong to
/// the resource and not to the body, cannot be applied by one.
#[tokio::test]
async fn a_reimport_is_refused_where_it_does_not_apply() {
    let router = router();
    let requested = request_certificate(
        &router,
        &json!({ "DomainName": "requested.example.com", "ValidationMethod": "DNS" }),
    )
    .await;
    let generated = generate(&Blueprint::default());
    let imported = import(&router, "the certificate to replace", &import_body(&generated)).await;

    let replacement = generate(&Blueprint {
        common_name: Some("replacement.example.com"),
        ..Blueprint::default()
    });
    let cases: &[(&str, Value)] = &[
        (
            "a certificate ACM was asked to issue is not an imported one",
            json!({
                "Certificate": blob(&replacement.certificate),
                "PrivateKey": blob(&replacement.private_key),
                "CertificateArn": requested,
            }),
        ),
        (
            "a reimport may not carry tags",
            json!({
                "Certificate": blob(&replacement.certificate),
                "PrivateKey": blob(&replacement.private_key),
                "CertificateArn": imported,
                "Tags": [{ "Key": "owner", "Value": "platform" }],
            }),
        ),
    ];
    for (case, body) in cases {
        let (status, response) = call(&router, "ImportCertificate", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal with 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {response}"
        );
    }

    // Neither refusal touched the certificate it named.
    let detail = describe(&router, "the certificate to replace", &imported).await;
    assert_eq!(
        detail["Serial"].as_str(),
        Some(generated.serial.as_str()),
        "case {:?}: a refused reimport must leave the stored certificate as it was, got {detail}",
        "a reimport may not carry tags"
    );
    let requested_detail = describe(&router, "the requested certificate", &requested).await;
    assert_eq!(
        requested_detail["Type"].as_str(),
        Some("AMAZON_ISSUED"),
        "case {:?}: a refused reimport must not retype the certificate, got {requested_detail}",
        "a certificate ACM was asked to issue is not an imported one"
    );
}

/// RULE: a PEM-encoded certificate chain is accepted beside the certificate when its
/// first certificate is the one that issued it — whatever else the chain carries above
/// that — and the certificate stored is still the leaf the request named; a chain
/// beginning with some other authority's certificate is refused.
#[tokio::test]
async fn a_chain_is_accepted_beside_the_certificate() {
    let router = router();
    // A self-signed leaf is its own issuer, so the certificate that issued it — the one
    // a chain has to begin with — is the leaf's own.
    let leaf = generate(&Blueprint {
        common_name: Some("leaf.example.com"),
        ..Blueprint::default()
    });
    let root = generate(&Blueprint {
        common_name: Some("root.example.com"),
        ..Blueprint::default()
    });

    // One block and several: a chain is "the PEM encoded certificate chain"
    // (`ImportCertificateRequest$CertificateChain` doc), however many certificates it
    // carries above the issuing one.
    let chains: &[(&str, String)] = &[
        ("the issuing certificate alone", leaf.certificate.clone()),
        (
            "the issuing certificate and a root above it",
            format!("{}{}", leaf.certificate, root.certificate),
        ),
    ];
    for (case, chain) in chains {
        let mut body = import_body(&leaf);
        body["CertificateChain"] = blob(chain);
        let arn = import(&router, case, &body).await;
        let detail = describe(&router, case, &arn).await;
        assert_eq!(
            detail["Serial"].as_str(),
            Some(leaf.serial.as_str()),
            "case {case:?}: the stored certificate must be the leaf, got {detail}"
        );
        assert_eq!(
            detail["DomainName"].as_str(),
            Some("leaf.example.com"),
            "case {case:?}: the stored certificate must be the leaf, got {detail}"
        );
    }

    // A chain of another authority entirely, and one that merely begins with the wrong
    // certificate: both say nothing about the certificate they were sent with.
    let refused: &[(&str, String)] = &[
        ("another authority's certificate", root.certificate.clone()),
        (
            "the issuing certificate below a foreign one",
            format!("{}{}", root.certificate, leaf.certificate),
        ),
    ];
    for (case, chain) in refused {
        let mut body = import_body(&leaf);
        body["CertificateChain"] = blob(chain);
        let (status, response) = call(&router, "ImportCertificate", &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the import to be refused with 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {response}"
        );
    }
}
