// SPDX-License-Identifier: CC0-1.0

//! The `CertificateDetail` that `DescribeCertificate` projects from a stored
//! certificate record.
//!
//! The wire harness is shared with [`crate::certificate_lifecycle`], so every call
//! here goes through `MultiServiceRouter::dispatch` — the path the emulator serves
//! real traffic on — and every assertion is on what `DescribeCertificate` reports
//! after a `RequestCertificate` wrote the record.
//!
//! The tables span the classes the rules generalise over: every modelled
//! `KeyAlgorithm` (so the reported spelling is not read off one algorithm), every
//! `ValidationMethod`, `Options` present in each of its forms and absent, wildcard
//! and plain names, and several name counts.

use serde_json::{json, Value};

use crate::certificate_lifecycle::{
    call, describe, hold_pending_validations, request_certificate, router, router_with_engine,
    KEY_ALGORITHMS,
};

/// Every modelled `KeyAlgorithm`, beside the X.509 algorithm descriptor a certificate
/// detail reports it as and the algorithm the certificate is signed with.
///
/// The first column is the model's own enum
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#KeyAlgorithm")` →
/// `RSA_1024`, `RSA_2048`, `RSA_3072`, `RSA_4096`, `EC_prime256v1`, `EC_secp384r1`,
/// `EC_secp521r1`), which is the spelling a request and an `Includes.keyTypes` filter
/// are written in. The second is the descriptor the detail reports — family and
/// parameter joined by a hyphen. The third is the `<hash>WITH<family>` signature
/// algorithm, whose spellings are the sibling Private CA model's `SigningAlgorithm`
/// values (`smithy_query(query="shape", shape="com.amazonaws.acmpca#SigningAlgorithm")`
/// → `SHA256WITHRSA`, `SHA256WITHECDSA`, …).
const ALGORITHM_FORMS: &[(&str, &str, &str)] = &[
    ("EC_prime256v1", "EC-prime256v1", "SHA256WITHECDSA"),
    ("EC_secp384r1", "EC-secp384r1", "SHA256WITHECDSA"),
    ("EC_secp521r1", "EC-secp521r1", "SHA256WITHECDSA"),
    ("RSA_1024", "RSA-1024", "SHA256WITHRSA"),
    ("RSA_2048", "RSA-2048", "SHA256WITHRSA"),
    ("RSA_3072", "RSA-3072", "SHA256WITHRSA"),
    ("RSA_4096", "RSA-4096", "SHA256WITHRSA"),
];

/// The key algorithm a `RequestCertificate` that names none gets: "RSA is the default
/// key algorithm for ACM certificates" (`RequestCertificateRequest$KeyAlgorithm` doc).
const DEFAULT_KEY_ALGORITHM: &str = "RSA_2048";

/// One row of the metadata table: what the case is, the `RequestCertificate` body, the
/// domain names the certificate then covers (`DomainName` first), the validation
/// method the certificate ends up with, the `Issuer` the detail must report (absent
/// for a certificate signed by a private CA this service holds no subject name for),
/// and the two `Options` members it must report.
struct DetailCase<'a> {
    label: &'a str,
    request: Value,
    names: &'a [&'a str],
    method: &'a str,
    issuer: Option<&'a str>,
    transparency_logging: &'a str,
    export: &'a str,
}

/// The mailbox list a name validated by `method` must report: email validation names
/// the messages ACM has sent — none while the initial dispatch is still owed, so the
/// empty list — while a name validated by DNS or HTTP has no mailbox list at all.
fn expected_validation_emails(method: &str) -> Value {
    if method == "EMAIL" {
        json!([])
    } else {
        Value::Null
    }
}

/// The `(descriptor, signature algorithm)` pair [`ALGORITHM_FORMS`] holds for a
/// requested key algorithm, or for the default when the request named none. Shared
/// with the other suites, so every assertion about the described spelling reads it
/// from this one literal table rather than re-deriving the transform.
pub(crate) fn algorithm_forms(requested: Option<&str>) -> (&'static str, &'static str) {
    let wanted = requested.unwrap_or(DEFAULT_KEY_ALGORITHM);
    ALGORITHM_FORMS
        .iter()
        .find(|(modelled, _, _)| *modelled == wanted)
        .map(|(_, descriptor, signature)| (*descriptor, *signature))
        .unwrap_or_else(|| panic!("{wanted} is not a modelled KeyAlgorithm"))
}

/// The domain whose control ACM proves for `name`: the name itself, with a leading
/// wildcard label removed (`*.example.com` is validated through `example.com`).
///
/// Deliberately the test's own expectation rather than a call into
/// `crate::state::validation_target`: a test that reuses the production helper would
/// assert the implementation against itself.
fn validation_target(name: &str) -> &str {
    name.strip_prefix("*.").unwrap_or(name)
}

/// RULE: for a certificate ACM issues, the detail reports every member the record
/// decides — the X.509 identity fixed when the request was accepted (`Subject`,
/// `Issuer`, `SignatureAlgorithm`), the key-usage extension lists as lists (empty
/// while the certificate is pending), the certificate's full `Options`, and the
/// validation domain of each name — while only members that come into existence with
/// the certificate itself stay absent.
#[tokio::test]
async fn the_detail_reports_the_metadata_the_record_decides() {
    let cases: &[DetailCase<'_>] = &[
        DetailCase {
            label: "DNS validation, no Options",
            request: json!({ "DomainName": "dns.detail.example.com", "ValidationMethod": "DNS" }),
            names: &["dns.detail.example.com"],
            method: "DNS",
            issuer: Some("Amazon"),
            transparency_logging: "ENABLED",
            export: "DISABLED",
        },
        DetailCase {
            label: "no method named: ACM validates by email",
            request: json!({ "DomainName": "mail.detail.example.com" }),
            names: &["mail.detail.example.com"],
            method: "EMAIL",
            issuer: Some("Amazon"),
            transparency_logging: "ENABLED",
            export: "DISABLED",
        },
        DetailCase {
            label: "HTTP validation with two subject alternative names",
            request: json!({
                "DomainName": "http.detail.example.com",
                "SubjectAlternativeNames": ["a.http.detail.example.com", "b.http.detail.example.org"],
                "ValidationMethod": "HTTP",
            }),
            names: &[
                "http.detail.example.com",
                "a.http.detail.example.com",
                "b.http.detail.example.org",
            ],
            method: "HTTP",
            issuer: Some("Amazon"),
            transparency_logging: "ENABLED",
            export: "DISABLED",
        },
        DetailCase {
            label: "Options naming only Export=ENABLED",
            request: json!({
                "DomainName": "export.detail.example.com",
                "ValidationMethod": "DNS",
                "Options": { "Export": "ENABLED" },
            }),
            names: &["export.detail.example.com"],
            method: "DNS",
            issuer: Some("Amazon"),
            transparency_logging: "ENABLED",
            export: "ENABLED",
        },
        DetailCase {
            label: "Options naming both preferences",
            request: json!({
                "DomainName": "both.detail.example.com",
                "ValidationMethod": "EMAIL",
                "Options": {
                    "CertificateTransparencyLoggingPreference": "DISABLED",
                    "Export": "DISABLED",
                },
            }),
            names: &["both.detail.example.com"],
            method: "EMAIL",
            issuer: Some("Amazon"),
            transparency_logging: "DISABLED",
            export: "DISABLED",
        },
        DetailCase {
            label: "Options naming only the validation method",
            request: json!({
                "DomainName": "vm.detail.example.com",
                "Options": { "ValidationMethod": "DNS" },
            }),
            names: &["vm.detail.example.com"],
            method: "DNS",
            issuer: Some("Amazon"),
            transparency_logging: "ENABLED",
            export: "DISABLED",
        },
        DetailCase {
            label: "a wildcard name, an EC key pair and three names",
            request: json!({
                "DomainName": "*.wild.detail.example.com",
                "SubjectAlternativeNames": [
                    "wild.detail.example.com",
                    "*.inner.wild.detail.example.com",
                ],
                "ValidationMethod": "DNS",
                "KeyAlgorithm": "EC_secp384r1",
            }),
            names: &[
                "*.wild.detail.example.com",
                "wild.detail.example.com",
                "*.inner.wild.detail.example.com",
            ],
            method: "DNS",
            issuer: Some("Amazon"),
            transparency_logging: "ENABLED",
            export: "DISABLED",
        },
        DetailCase {
            label: "an RSA_4096 key pair",
            request: json!({
                "DomainName": "rsa4096.detail.example.com",
                "ValidationMethod": "HTTP",
                "KeyAlgorithm": "RSA_4096",
            }),
            names: &["rsa4096.detail.example.com"],
            method: "HTTP",
            issuer: Some("Amazon"),
            transparency_logging: "ENABLED",
            export: "DISABLED",
        },
        DetailCase {
            // A certificate ACM issues from a private CA is signed by that CA, whose
            // subject name lives in Private CA's own store, so this service reports no
            // issuer for it.
            label: "issued from a private certificate authority",
            request: json!({
                "DomainName": "private.detail.example.com",
                "ValidationMethod": "DNS",
                "CertificateAuthorityArn":
                    "arn:aws:acm-pca:us-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
            }),
            names: &["private.detail.example.com"],
            method: "DNS",
            issuer: None,
            transparency_logging: "ENABLED",
            export: "DISABLED",
        },
    ];

    let (router, engine) = router_with_engine();
    for case in cases {
        let label = case.label;
        let arn = request_certificate(&router, &case.request).await;
        // This suite is about the record a request creates, so every transition the
        // clock owes it — the validation outcome and the initial validation mail — is
        // held out instead of racing the wall clock.
        hold_pending_validations(&engine);
        let detail = describe(&router, &arn).await;

        let domain = case.request["DomainName"].as_str().unwrap_or_default();
        let expected_subject = format!("CN={domain}");
        assert_eq!(
            detail["Subject"].as_str(),
            Some(expected_subject.as_str()),
            "case {label:?}: expected Subject {expected_subject}, got {detail}"
        );
        assert_eq!(
            detail["Issuer"].as_str(),
            case.issuer,
            "case {label:?}: expected Issuer {:?}, got {detail}",
            case.issuer
        );

        let requested_algorithm = case.request["KeyAlgorithm"].as_str();
        let (descriptor, signature) = algorithm_forms(requested_algorithm);
        assert_eq!(
            detail["KeyAlgorithm"].as_str(),
            Some(descriptor),
            "case {label:?}: expected KeyAlgorithm {descriptor}, got {detail}"
        );
        assert_eq!(
            detail["SignatureAlgorithm"].as_str(),
            Some(signature),
            "case {label:?}: expected SignatureAlgorithm {signature}, got {detail}"
        );

        // The key-usage extensions of a pending certificate: known, and known to be
        // none — the EMPTY list, not a missing member.
        for member in ["KeyUsages", "ExtendedKeyUsages"] {
            assert_eq!(
                detail[member],
                json!([]),
                "case {label:?}: expected {member} to be the empty list, got {detail}"
            );
        }

        // Options reports both certificate preferences, defaulted where the request
        // named none, plus the validation-method preference ONLY when the request's own
        // `Options` named one — the method each name is validated by is reported per
        // domain, below.
        let options = detail["Options"]
            .as_object()
            .unwrap_or_else(|| panic!("case {label:?}: no Options object in {detail}"));
        let mut members: Vec<&str> = options.keys().map(String::as_str).collect();
        members.sort_unstable();
        let requested_option_method = case.request["Options"]["ValidationMethod"].as_str();
        let mut expected_members = vec!["CertificateTransparencyLoggingPreference", "Export"];
        if requested_option_method.is_some() {
            expected_members.push("ValidationMethod");
        }
        expected_members.sort_unstable();
        assert_eq!(
            members, expected_members,
            "case {label:?}: expected exactly {expected_members:?} in Options, got {detail}"
        );
        assert_eq!(
            options.get("ValidationMethod").and_then(Value::as_str),
            requested_option_method,
            "case {label:?}: expected Options.ValidationMethod {requested_option_method:?}, got {detail}"
        );
        assert_eq!(
            options["CertificateTransparencyLoggingPreference"],
            json!(case.transparency_logging),
            "case {label:?}: expected CertificateTransparencyLoggingPreference {}, got {detail}",
            case.transparency_logging
        );
        assert_eq!(
            options["Export"],
            json!(case.export),
            "case {label:?}: expected Export {}, got {detail}",
            case.export
        );

        // One validation per name, each naming the domain whose control is proven, and
        // none claiming an email ACM has not sent.
        let validations = detail["DomainValidationOptions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {label:?}: no DomainValidationOptions in {detail}"));
        assert_eq!(
            validations.len(),
            case.names.len(),
            "case {label:?}: expected one validation per name, got {detail}"
        );
        for (validation, name) in validations.iter().zip(case.names) {
            assert_eq!(
                validation["DomainName"].as_str(),
                Some(*name),
                "case {label:?}: expected the validation of {name}, got {validation}"
            );
            assert_eq!(
                validation["ValidationMethod"].as_str(),
                Some(case.method),
                "case {label:?}: expected ValidationMethod {}, got {validation}",
                case.method
            );
            let target = validation_target(name);
            assert_eq!(
                validation["ValidationDomain"].as_str(),
                Some(target),
                "case {label:?}: expected ValidationDomain {target}, got {validation}"
            );
            let expected_emails = expected_validation_emails(case.method);
            assert_eq!(
                validation["ValidationEmails"], expected_emails,
                "case {label:?}: expected ValidationEmails {expected_emails}, got {validation}"
            );
        }

        // Members that come into existence only with the certificate itself.
        for absent in ["Serial", "IssuedAt", "ImportedAt", "NotBefore", "NotAfter"] {
            assert!(
                detail[absent].is_null(),
                "case {label:?}: {absent} must be absent before issuance, got {detail}"
            );
        }

        // A describe reports; it does not change what the next describe reports.
        let again = describe(&router, &arn).await;
        assert_eq!(
            again, detail,
            "case {label:?}: two describes of one certificate must agree"
        );
    }
}

/// RULE: a certificate's detail reports its key algorithm as the X.509 algorithm
/// descriptor (family and parameter joined by a hyphen) and its signature algorithm as
/// SHA-256 with that key's family, for every algorithm the model declares — while the
/// stored value a `ListCertificates` filter selects on keeps the model enum's spelling.
#[tokio::test]
async fn every_modelled_key_algorithm_is_described_as_its_x509_descriptor() {
    // The table must cover the model's enum exactly: a value added to the model is a
    // row missing here, not a silently untested algorithm.
    let mut catalogued: Vec<&str> = ALGORITHM_FORMS
        .iter()
        .map(|(modelled, _, _)| *modelled)
        .collect();
    catalogued.sort_unstable();
    let mut modelled: Vec<&str> = KEY_ALGORITHMS.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        catalogued, modelled,
        "the table must hold one row per modelled KeyAlgorithm, got {catalogued:?}"
    );

    let router = router();
    for (requested, descriptor, signature) in ALGORITHM_FORMS {
        let domain = format!("{}.algo.example.com", descriptor.to_lowercase());
        let arn = request_certificate(
            &router,
            &json!({
                "DomainName": domain,
                "ValidationMethod": "DNS",
                "KeyAlgorithm": requested,
            }),
        )
        .await;

        let detail = describe(&router, &arn).await;
        assert_eq!(
            detail["KeyAlgorithm"].as_str(),
            Some(*descriptor),
            "case {requested}: expected KeyAlgorithm {descriptor}, got {detail}"
        );
        assert_eq!(
            detail["SignatureAlgorithm"].as_str(),
            Some(*signature),
            "case {requested}: expected SignatureAlgorithm {signature}, got {detail}"
        );

        // The value the filter selects on is the stored one, in the model's spelling:
        // asking for this algorithm by name finds exactly this certificate, and its
        // summary reports that spelling.
        let (status, listed) = call(
            &router,
            "ListCertificates",
            &json!({ "Includes": { "keyTypes": [requested] } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {requested}: ListCertificates must succeed, got {status} {listed}"
        );
        let summaries = listed["CertificateSummaryList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {requested}: no CertificateSummaryList in {listed}"));
        let summary = summaries
            .iter()
            .find(|summary| summary["CertificateArn"].as_str() == Some(arn.as_str()))
            .unwrap_or_else(|| panic!("case {requested}: {arn} is missing from {listed}"));
        assert_eq!(
            summary["KeyAlgorithm"].as_str(),
            Some(*requested),
            "case {requested}: a summary reports the spelling the filter uses, got {summary}"
        );
    }

    // A request that names no algorithm gets the documented default, described in the
    // same spelling.
    let arn = request_certificate(
        &router,
        &json!({ "DomainName": "default.algo.example.com", "ValidationMethod": "DNS" }),
    )
    .await;
    let detail = describe(&router, &arn).await;
    let (descriptor, signature) = algorithm_forms(None);
    assert_eq!(
        (
            detail["KeyAlgorithm"].as_str(),
            detail["SignatureAlgorithm"].as_str()
        ),
        (Some(descriptor), Some(signature)),
        "case no KeyAlgorithm named: expected {descriptor} signed {signature}, got {detail}"
    );
}

/// RULE: the validation domain reported for a name is the one the request named for
/// that name in `DomainValidationOptions`, else the name itself with any wildcard
/// label removed — whatever validation method the certificate uses.
#[tokio::test]
async fn a_request_named_validation_domain_applies_to_that_name_only() {
    // Every modelled `ValidationMethod`
    // (`smithy_query(query="shape", shape="com.amazonaws.acm#ValidationMethod")`),
    // plus the case where the request names none.
    let methods: &[Option<&str>] = &[Some("DNS"), Some("EMAIL"), Some("HTTP"), None];

    let (router, engine) = router_with_engine();
    for (index, method) in methods.iter().enumerate() {
        let case = format!("ValidationMethod={method:?}");
        // The first name is validated through a domain the request chose — a
        // superdomain of it, which is the only kind `DomainValidationOption` accepts —
        // while the other two carry no option of their own.
        let chosen = format!("vd{index}.example.com");
        let domain = format!("named.{chosen}");
        let plain = format!("plain.{chosen}");
        let wildcard = format!("*.wild.{chosen}");
        let mut body = json!({
            "DomainName": domain,
            "SubjectAlternativeNames": [plain, wildcard],
            "DomainValidationOptions": [
                { "DomainName": domain, "ValidationDomain": chosen },
            ],
        });
        if let Some(method) = method {
            body["ValidationMethod"] = json!(method);
        }

        let arn = request_certificate(&router, &body).await;
        // The row is about the validation domains the request fixed, so the clock-owed
        // mail dispatch is held out and no mailbox has been written to yet.
        hold_pending_validations(&engine);
        let detail = describe(&router, &arn).await;
        let validations = detail["DomainValidationOptions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: no DomainValidationOptions in {detail}"));

        let expected: [(&str, &str); 3] = [
            (&domain, &chosen),
            (&plain, &plain),
            (&wildcard, validation_target(&wildcard)),
        ];
        let expected_emails = expected_validation_emails(method.unwrap_or("EMAIL"));
        assert_eq!(
            validations.len(),
            expected.len(),
            "case {case}: expected one validation per name, got {detail}"
        );
        for (validation, (name, validation_domain)) in validations.iter().zip(expected) {
            assert_eq!(
                validation["DomainName"].as_str(),
                Some(name),
                "case {case}: expected the validation of {name}, got {validation}"
            );
            assert_eq!(
                validation["ValidationDomain"].as_str(),
                Some(validation_domain),
                "case {case}: expected ValidationDomain {validation_domain}, got {validation}"
            );
            assert_eq!(
                validation["ValidationEmails"], expected_emails,
                "case {case}: expected ValidationEmails {expected_emails}, got {validation}"
            );
        }
    }
}
