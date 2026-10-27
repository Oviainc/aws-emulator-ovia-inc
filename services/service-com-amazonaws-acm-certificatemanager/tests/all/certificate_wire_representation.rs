// SPDX-License-Identifier: CC0-1.0

//! How ACM REPRESENTS one stored certificate on the wire, end to end over the wire
//! (`POST /`, `X-Amz-Target: CertificateManager.<Op>`, awsJson1_1).
//!
//! Three rules under test, each about a member every reader of one certificate must
//! agree on:
//!
//! 1. the key algorithm is reported as the X.509 algorithm descriptor — the model
//!    enum's token with its separator written as a HYPHEN — by every projection that
//!    REPORTS a certificate, so `DescribeCertificate` and `ListCertificates` never
//!    disagree about one certificate's `KeyAlgorithm`;
//! 2. a serial number is reported as the bytes of the certificate's own X.509 serial,
//!    each as exactly two lowercase hex digits, joined by colons;
//! 3. each entry of the Extended Key Usage list carries BOTH the `ExtendedKeyUsageName`
//!    and the dotted-decimal object identifier that names the purpose
//!    (`ExtendedKeyUsage` is "a name and an object identifier (OID)").
//!
//! The tables below span what each rule generalises over: every `KeyAlgorithm` the
//! model declares and both ways a record acquires one (requested and imported), serials
//! of one byte and of twenty, with an interior zero byte and with the high bit set, and
//! key purposes inside the catalogue the model names, outside it, and absent
//! altogether. Every certificate the service is fed is built by `rcgen` — something
//! other than the code under test — so a response that is not computed from the stored
//! certificate cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_import::{generate, import, import_body, Blueprint};
use crate::certificate_lifecycle::{
    call, describe, hold_pending_validations, request_certificate, router, router_with_engine,
    KEY_ALGORITHMS,
};
use crate::certificate_validation_state_machine::{make_validation_due_now, PRIVATE_CA_ARN};

/// Every `KeyAlgorithm` the model declares beside the X.509 algorithm descriptor it is
/// reported as: the token with its one separator written as a hyphen
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#KeyAlgorithm")`).
const ALGORITHM_DESCRIPTORS: &[(&str, &str)] = &[
    ("RSA_1024", "RSA-1024"),
    ("RSA_2048", "RSA-2048"),
    ("RSA_3072", "RSA-3072"),
    ("RSA_4096", "RSA-4096"),
    ("EC_prime256v1", "EC-prime256v1"),
    ("EC_secp384r1", "EC-secp384r1"),
    ("EC_secp521r1", "EC-secp521r1"),
];

/// The `CertificateSummary` `ListCertificates` reports for `arn` under `filters`, or
/// `None` when the listing does not carry it.
async fn summary(
    router: &MultiServiceRouter,
    case: &str,
    filters: &Value,
    arn: &str,
) -> Option<Value> {
    let (status, response) = call(router, "ListCertificates", filters).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListCertificates must succeed, got {status} {response}"
    );
    response["CertificateSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no CertificateSummaryList in {response}"))
        .iter()
        .find(|summary| summary["CertificateArn"].as_str() == Some(arn))
        .cloned()
}

/// Every key algorithm, as the listing that reports all of them is asked for.
fn every_key_type() -> Value {
    json!({ "Includes": { "keyTypes": KEY_ALGORITHMS } })
}

/// The `{Name, OID}` pairs a detail reports for its Extended Key Usage list.
fn extended_key_usages(case: &str, detail: &Value) -> Vec<(String, String)> {
    detail["ExtendedKeyUsages"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no ExtendedKeyUsages in {detail}"))
        .iter()
        .map(|usage| {
            (
                usage["Name"].as_str().unwrap_or_default().to_string(),
                usage["OID"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// RULE 1: one certificate has ONE key algorithm, and every projection that reports it
/// states the X.509 descriptor — so the detail of a certificate and the summary the
/// same certificate is listed in always agree, whichever algorithm it carries and
/// however it came to carry it.
#[tokio::test]
async fn the_detail_and_the_summary_report_one_key_algorithm() {
    let router = router();

    // One row per modelled `KeyAlgorithm`, each requested by name so the record's
    // algorithm is the row's own.
    for (token, descriptor) in ALGORITHM_DESCRIPTORS {
        let arn = request_certificate(
            &router,
            &json!({
                "DomainName": format!("{}.algorithm.probe-domain.com", descriptor.to_lowercase()),
                "ValidationMethod": "DNS",
                "KeyAlgorithm": token,
            }),
        )
        .await;

        let detail = describe(&router, &arn).await;
        let listed = summary(&router, token, &every_key_type(), &arn)
            .await
            .unwrap_or_else(|| panic!("case {token}: {arn} is not listed"));
        assert_eq!(
            (detail["KeyAlgorithm"].as_str(), listed["KeyAlgorithm"].as_str()),
            (Some(*descriptor), Some(*descriptor)),
            "case {token}: the detail and the summary must both report {descriptor}, got \
             {detail} and {listed}"
        );

        // The filter still selects on the ENUM token, which is what a request is
        // written in: the row is listed when its own token is asked for and not when
        // another is.
        let other = ALGORITHM_DESCRIPTORS
            .iter()
            .map(|(other, _)| *other)
            .find(|other| other != token)
            .expect("another modelled algorithm");
        assert!(
            summary(
                &router,
                token,
                &json!({ "Includes": { "keyTypes": [token] } }),
                &arn
            )
            .await
            .is_some(),
            "case {token}: a certificate must be listed under its own keyTypes filter"
        );
        assert_eq!(
            summary(
                &router,
                token,
                &json!({ "Includes": { "keyTypes": [other] } }),
                &arn
            )
            .await,
            None,
            "case {token}: a certificate must not be listed under the {other} filter"
        );
    }

    // The same rule for a record whose algorithm was read out of an imported
    // certificate rather than taken from a request: the two curves `rcgen` generates
    // key pairs for.
    let imports: &[(&str, &'static rcgen::SignatureAlgorithm, &str)] = &[
        (
            "an imported P-256 certificate",
            &rcgen::PKCS_ECDSA_P256_SHA256,
            "EC-prime256v1",
        ),
        (
            "an imported P-384 certificate",
            &rcgen::PKCS_ECDSA_P384_SHA384,
            "EC-secp384r1",
        ),
    ];
    for (case, algorithm, descriptor) in imports {
        let generated = generate(&Blueprint {
            common_name: Some(&format!("{}.imported.probe-domain.com", descriptor.to_lowercase())),
            algorithm,
            ..Blueprint::default()
        });
        let arn = import(&router, case, &import_body(&generated)).await;
        let detail = describe(&router, &arn).await;
        let listed = summary(&router, case, &every_key_type(), &arn)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: {arn} is not listed"));
        assert_eq!(
            (detail["KeyAlgorithm"].as_str(), listed["KeyAlgorithm"].as_str()),
            (Some(*descriptor), Some(*descriptor)),
            "case {case:?}: the detail and the summary must both report {descriptor}, got \
             {detail} and {listed}"
        );
    }
}

/// RULE 2: the serial a certificate is reported under is its own X.509 serial number,
/// rendered byte-wise: two lowercase hex digits per byte — so a byte below `0x10` keeps
/// its leading zero — joined by colons, and the same rendering is what
/// `SearchCertificates` reports and selects on.
#[tokio::test]
async fn an_imported_certificates_serial_is_rendered_byte_wise() {
    // Rows differ along what the rendering generalises over: the number of bytes, a
    // byte below 0x10, an interior zero byte, and a leading byte with its high bit set
    // (which DER itself pads, so the reported value is the certificate's own encoding).
    let cases: &[(&str, &[u8])] = &[
        ("a one-byte serial", &[0x2a]),
        ("a serial whose first byte is below 0x10", &[0x0a, 0x1b, 0x2c]),
        ("a serial with an interior zero byte", &[0x0a, 0x00, 0x7f]),
        ("a serial whose leading byte has the high bit set", &[0x7f, 0xff, 0x01]),
        (
            "a twenty-byte serial",
            &[
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
                0x0f, 0x10, 0x11, 0x12, 0x13, 0x14,
            ],
        ),
    ];

    let router = router();
    for (case, serial) in cases {
        let generated = generate(&Blueprint {
            common_name: Some("serial.probe-domain.com"),
            serial: Some(serial),
            ..Blueprint::default()
        });
        let arn = import(&router, case, &import_body(&generated)).await;
        let detail = describe(&router, &arn).await;
        let reported = detail["Serial"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Serial in {detail}"))
            .to_string();

        // Every group is exactly two lowercase hex digits, and there is one group per
        // byte of the certificate's own serial.
        let groups: Vec<&str> = reported.split(':').collect();
        assert_eq!(
            groups.len(),
            generated.serial_bytes.len(),
            "case {case:?}: expected one colon-separated group per serial byte, got \
             {reported:?}"
        );
        assert!(
            groups.iter().all(|group| group.len() == 2
                && group
                    .chars()
                    .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase())),
            "case {case:?}: every group must be two lowercase hex digits, got {reported:?}"
        );
        // And the groups are the bytes themselves, in order.
        assert_eq!(
            hex::decode(reported.replace(':', "")).ok(),
            Some(generated.serial_bytes.clone()),
            "case {case:?}: expected the certificate's own serial bytes, got {reported:?}"
        );
        assert_eq!(
            reported, generated.serial,
            "case {case:?}: expected the serial {:?}, got {reported:?}",
            generated.serial
        );

        // `SearchCertificates` reports and selects on the same rendering, so a client
        // can feed back what it read.
        let (status, response) = call(
            &router,
            "SearchCertificates",
            &json!({
                "FilterStatement": { "Filter": { "X509AttributeFilter": { "SerialNumber": reported } } }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: SearchCertificates must succeed, got {status} {response}"
        );
        let found: Vec<&str> = response["Results"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no Results in {response}"))
            .iter()
            .map(|result| result["CertificateArn"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            found,
            vec![arn.as_str()],
            "case {case:?}: the reported serial must select the certificate it came \
             from, got {response}"
        );
        assert_eq!(
            response["Results"][0]["X509Attributes"]["SerialNumber"].as_str(),
            Some(generated.serial.as_str()),
            "case {case:?}: expected the same rendering in the search result, got {response}"
        );
    }
}

/// One Extended Key Usage case: what it exercises, the purposes the generated
/// certificate asserts, and the `{Name, OID}` pairs the detail must then report.
type PurposeCase<'a> = (
    &'a str,
    &'a [rcgen::ExtendedKeyUsagePurpose],
    &'a [(&'a str, &'a str)],
);

/// RULE 3: each Extended Key Usage entry is the purpose the certificate asserts, as a
/// `{Name, OID}` pair — the identifier the X.509 extension carries and the
/// `ExtendedKeyUsageName` it is named by — including for a purpose outside the names
/// the model declares, which is reported as `CUSTOM` beside its own identifier.
#[tokio::test]
async fn extended_key_usages_report_a_name_and_an_object_identifier() {
    // One row per class the rule generalises over: a single purpose, several, a purpose
    // the issue's own example does not name, the catch-all purpose, a purpose outside
    // the modelled names, and no extension at all. Each expected OID is the one
    // `ExtendedKeyUsage$OID`'s documentation pairs the name with.
    let cases: &[PurposeCase<'_>] = &[
        (
            "server authentication alone",
            &[rcgen::ExtendedKeyUsagePurpose::ServerAuth],
            &[("TLS_WEB_SERVER_AUTHENTICATION", "1.3.6.1.5.5.7.3.1")],
        ),
        (
            "client authentication and code signing",
            &[
                rcgen::ExtendedKeyUsagePurpose::ClientAuth,
                rcgen::ExtendedKeyUsagePurpose::CodeSigning,
            ],
            &[
                ("TLS_WEB_CLIENT_AUTHENTICATION", "1.3.6.1.5.5.7.3.2"),
                ("CODE_SIGNING", "1.3.6.1.5.5.7.3.3"),
            ],
        ),
        (
            "email protection, time stamping and OCSP signing",
            &[
                rcgen::ExtendedKeyUsagePurpose::EmailProtection,
                rcgen::ExtendedKeyUsagePurpose::TimeStamping,
                rcgen::ExtendedKeyUsagePurpose::OcspSigning,
            ],
            &[
                ("EMAIL_PROTECTION", "1.3.6.1.5.5.7.3.4"),
                ("TIME_STAMPING", "1.3.6.1.5.5.7.3.8"),
                ("OCSP_SIGNING", "1.3.6.1.5.5.7.3.9"),
            ],
        ),
        (
            // `anyExtendedKeyUsage`, RFC 5280 section 4.2.1.12.
            "the catch-all purpose",
            &[rcgen::ExtendedKeyUsagePurpose::Any],
            &[("ANY", "2.5.29.37.0")],
        ),
        (
            // A purpose no `ExtendedKeyUsageName` names, so the only thing that still
            // says which purpose it is is its identifier.
            "a purpose outside the modelled names",
            &[rcgen::ExtendedKeyUsagePurpose::Other(vec![
                1, 3, 6, 1, 4, 1, 311, 10, 3, 4,
            ])],
            &[("CUSTOM", "1.3.6.1.4.1.311.10.3.4")],
        ),
        (
            "no extended key usage extension",
            &[],
            &[],
        ),
    ];

    let router = router();
    for (case, purposes, expected) in cases {
        let generated = generate(&Blueprint {
            common_name: Some("purpose.probe-domain.com"),
            extended_key_usages: purposes,
            ..Blueprint::default()
        });
        let arn = import(&router, case, &import_body(&generated)).await;
        let detail = describe(&router, &arn).await;
        let reported = extended_key_usages(case, &detail);
        let wanted: Vec<(String, String)> = expected
            .iter()
            .map(|(name, oid)| ((*name).to_string(), (*oid).to_string()))
            .collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the purposes the certificate asserts, got {detail}"
        );

        // The summary of the same certificate carries the NAMES alone — the member is
        // a list of `ExtendedKeyUsageName` — and they are the names of those very
        // pairs.
        let listed = summary(&router, case, &every_key_type(), &arn)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: {arn} is not listed"));
        let names: Vec<String> = listed["ExtendedKeyUsages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|name| name.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names,
            wanted.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>(),
            "case {case:?}: expected the summary to name the same purposes, got {listed}"
        );

        // And the listing filter still selects on a name, so a purpose read off the
        // pair finds the certificate it came from.
        if let Some((name, _)) = wanted.first() {
            assert!(
                summary(
                    &router,
                    case,
                    &json!({
                        "Includes": {
                            "keyTypes": KEY_ALGORITHMS,
                            "extendedKeyUsage": [name],
                        }
                    }),
                    &arn,
                )
                .await
                .is_some(),
                "case {case:?}: a certificate must be listed under the {name} filter"
            );
        }
    }
}

/// RULE 3 for a certificate ACM issued itself: the purposes a TLS certificate carries
/// come into existence with the certificate, and are reported as the same `{Name, OID}`
/// pairs an imported certificate's are.
#[tokio::test]
async fn an_issued_certificates_extended_key_usages_carry_their_identifiers() {
    let (router, engine) = router_with_engine();
    let case = "a private certificate carried to ISSUED";
    let arn = request_certificate(
        &router,
        &json!({
            "DomainName": "issued.purpose.probe-domain.com",
            "ValidationMethod": "DNS",
            "CertificateAuthorityArn": PRIVATE_CA_ARN,
        }),
    )
    .await;

    // The transition the clock owes this request is held out while the pending state is
    // observed, so the row cannot race the settle delay.
    hold_pending_validations(&engine);

    // Before issuance the certificate asserts no purpose, which is the EMPTY list.
    let pending = describe(&router, &arn).await;
    assert_eq!(
        extended_key_usages(case, &pending),
        Vec::<(String, String)>::new(),
        "case {case:?}: a pending certificate asserts no purpose, got {pending}"
    );

    make_validation_due_now(&engine);
    let detail = describe(&router, &arn).await;
    assert_eq!(
        detail["Status"].as_str(),
        Some("ISSUED"),
        "case {case:?}: the certificate must be issued first, got {detail}"
    );
    assert_eq!(
        extended_key_usages(case, &detail),
        vec![
            (
                "TLS_WEB_SERVER_AUTHENTICATION".to_string(),
                "1.3.6.1.5.5.7.3.1".to_string()
            ),
            (
                "TLS_WEB_CLIENT_AUTHENTICATION".to_string(),
                "1.3.6.1.5.5.7.3.2".to_string()
            ),
        ],
        "case {case:?}: expected the purposes of an issued TLS certificate, got {detail}"
    );
}
