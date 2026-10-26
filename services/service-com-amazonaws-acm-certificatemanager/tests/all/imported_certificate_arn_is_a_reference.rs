// SPDX-License-Identifier: CC0-1.0

//! What a client gets back from `ImportCertificate`, viewed as the *reference* another
//! service is then handed: not merely a 200, but a certificate ARN whose every field is
//! one the constraints on a certificate reference admit, naming the Region the request
//! was signed for and this account, distinct for every import, and resolving through
//! ACM's own read path.
//!
//! RULE: an import stores one certificate and answers the ARN it is stored under, so
//! the value returned is a well-formed ACM certificate ARN for the request's own Region
//! and account, unique per stored certificate and stable for a replacement under an ARN
//! the request names — and that ARN reads back as the certificate that was imported.
//!
//! The rows span what the rule generalises over: the key curve, how many names the
//! certificate covers and in which form (a subject name, a wildcard, several DNS names,
//! a name only in the extension), whether the import carries a chain or tags, and the
//! Region the request is signed for — with a final reimport under the ARN the first row
//! was answered, the one case in which an import must NOT mint a new reference. That
//! last row is here for the reference's *stability across Regions*; what a replacement
//! does to the stored certificate is `certificate_import`'s
//! `a_reimport_replaces_the_certificate_under_the_same_arn`.
//!
//! Every certificate fed to the service is built by `rcgen`, and every ARN asserted on
//! is the one the import answered rather than a value written into this file.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::certificate_import::{blob, generate, import_body, import_in_region, Blueprint};
use crate::certificate_lifecycle::{describe_in_region, router, ACCOUNT};

/// What the models require of a value carried as a certificate reference, as the fields
/// of an ARN. Both shapes are read from the models rather than written from memory:
///
/// * `smithy_query(query="shape", shape="com.amazonaws.acm#Arn")` — the shape
///   `ImportCertificateResponse$CertificateArn` targets: `@length` 20..2048 and a
///   pattern whose service field is literally `acm`, whose account field is digits, and
///   whose resource is one or more `/`-separated segments.
/// * `smithy_query(query="shape", shape="com.amazonaws.appsync#CertificateArn",
///   model_path="services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice\
///   /smithy-model/com-amazonaws-appsync-awsdeepdishcontrolplaneservice.json")` — the
///   shape a consuming service's `certificateArn` member targets, the narrower of the
///   two: `@length` 20..2048 and a pattern admitting only a lowercase partition, the
///   `acm` or `iam` service, a lowercase-or-digit Region, an account of EXACTLY twelve
///   digits, and a `certificate` or `server-certificate` resource whose id is drawn from
///   `[0-9A-Za-z_/-]`.
///
/// The lower `@length` bound is the one an ACM certificate ARN can come near, so it is
/// the bound checked here; the 2048 upper bound is unreachable for a six-field ARN
/// ending in one id and is not asserted.
const MINIMUM_REFERENCE_LENGTH: usize = 20;

/// Why `arn` is not a value both shapes above admit, or `None` when it is one.
///
/// Returning the reason rather than a bool is what lets the counter-examples below
/// assert that this check has teeth: a row proves the ARN is admitted, and each
/// counter-example proves a value the shapes exclude is rejected, naming which field
/// excluded it.
fn not_a_certificate_reference(arn: &str) -> Option<String> {
    if arn.len() < MINIMUM_REFERENCE_LENGTH {
        return Some(format!(
            "shorter than the {MINIMUM_REFERENCE_LENGTH}-character minimum"
        ));
    }
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    if fields.len() != 6 {
        return Some(format!("has {} colon-separated fields, not six", fields.len()));
    }
    if fields[0] != "arn" {
        return Some(format!("does not begin with \"arn\" but {:?}", fields[0]));
    }
    if fields[1].is_empty() || !fields[1].chars().all(|c| c.is_ascii_lowercase() || c == '-') {
        return Some(format!("partition {:?} is not a lowercase partition", fields[1]));
    }
    if fields[2] != "acm" {
        return Some(format!("service {:?} is not \"acm\"", fields[2]));
    }
    if !fields[3]
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Some(format!("region {:?} is not a Region name", fields[3]));
    }
    if fields[4].len() != 12 || !fields[4].chars().all(|c| c.is_ascii_digit()) {
        return Some(format!("account {:?} is not twelve digits", fields[4]));
    }
    let Some(id) = fields[5].strip_prefix("certificate/") else {
        return Some(format!("resource {:?} is not a certificate", fields[5]));
    };
    if id.is_empty() {
        return Some("names no certificate id".to_string());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '/' || c == '-')
    {
        return Some(format!("certificate id {id:?} carries an excluded character"));
    }
    None
}

/// Values the two shapes do NOT admit, one per field the check above reads, so no row
/// can pass against a check that accepts everything.
const NOT_REFERENCES: &[&str] = &[
    "certificate/11111111-2222-3333-4444-555555555555",
    "arn:aws:iam::123456789012:certificate/11111111-2222-3333-4444-555555555555",
    "arn:AWS:acm:us-west-2:123456789012:certificate/11111111-2222-3333-4444-5555",
    "arn:aws:acm:us-west-2:12345:certificate/11111111-2222-3333-4444-555555555555",
    "arn:aws:acm:us-west-2:123456789012:acme-endpoint/11111111-2222-3333-4444-5555",
    "arn:aws:acm:us-west-2:123456789012:certificate/",
];

/// One import: the Region it is signed for and the certificate it carries.
struct Row {
    case: &'static str,
    region: &'static str,
    blueprint: Blueprint<'static>,
    /// Whether the request carries the issuing certificate as its chain.
    with_chain: bool,
    /// The tags the request carries, if any — part of the ACM resource, not of the
    /// reference, so they must not change what the ARN looks like.
    tags: Option<Value>,
}

/// The rows, built here rather than as a `const` because each blueprint borrows the
/// names its certificate covers.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a P-256 certificate with one subject name",
            region: "us-west-2",
            blueprint: Blueprint {
                common_name: Some("reference.example.com"),
                ..Blueprint::default()
            },
            with_chain: false,
            tags: None,
        },
        Row {
            case: "a P-384 wildcard certificate imported with its chain",
            region: "us-east-1",
            blueprint: Blueprint {
                common_name: Some("*.reference.example.com"),
                dns_names: &["*.reference.example.com"],
                algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
                ..Blueprint::default()
            },
            with_chain: true,
            tags: None,
        },
        Row {
            case: "a certificate covering several DNS names, imported with tags",
            region: "eu-central-1",
            blueprint: Blueprint {
                common_name: Some("one.reference.example.net"),
                dns_names: &["one.reference.example.net", "two.reference.example.net"],
                validity: (2021, 2061),
                ..Blueprint::default()
            },
            with_chain: false,
            tags: Some(json!([{ "Key": "owner", "Value": "platform" }])),
        },
        Row {
            case: "a certificate whose only name is in the extension, carrying key usages",
            region: "ap-southeast-2",
            blueprint: Blueprint {
                common_name: None,
                organization: Some("Reference Test Org"),
                dns_names: &["extension-only.reference.example.org"],
                key_usages: true,
                ..Blueprint::default()
            },
            with_chain: false,
            tags: None,
        },
    ]
}

/// The check above has teeth: every value the two shapes exclude is rejected, so the
/// admissibility a row asserts cannot be vacuous.
#[test]
fn a_value_the_shapes_exclude_is_not_a_certificate_reference() {
    for not_one in NOT_REFERENCES {
        assert!(
            not_a_certificate_reference(not_one).is_some(),
            "case {not_one:?}: expected a value the modelled shapes exclude to be rejected"
        );
    }
}

/// Assert `arn` is a value the modelled constraints admit, and that it names `region`
/// and this account.
fn assert_is_a_certificate_reference(case: &str, region: &str, arn: &str) {
    if let Some(why) = not_a_certificate_reference(arn) {
        panic!("case {case:?}: expected a certificate reference, but {arn:?} {why}");
    }

    // The two fields the request itself decides: a certificate is a regional resource of
    // one account, so its reference names the Region the call was signed for and the
    // account the emulator serves.
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    for (what, actual, expected) in [("region", fields[3], region), ("account", fields[4], ACCOUNT)]
    {
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the ARN's {what} to be {expected:?}, got {actual:?} in \
             {arn:?}"
        );
    }
}

/// RULE: `ImportCertificate` answers the ARN the certificate it stored is stored under —
/// a reference the modelled constraints on a certificate ARN admit, naming the request's
/// own Region and account, distinct for every stored certificate, and resolvable through
/// `DescribeCertificate` as the certificate that was imported.
#[tokio::test]
async fn an_import_answers_a_usable_certificate_reference() {
    let router = router();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut first: Option<(String, &'static str)> = None;

    for row in rows() {
        let case = row.case;
        let generated = generate(&row.blueprint);
        let mut body = import_body(&generated);
        if row.with_chain {
            // A self-signed leaf is its own issuer, so the certificate a chain has to
            // begin with is the leaf's own.
            body["CertificateChain"] = blob(&generated.certificate);
        }
        if let Some(tags) = row.tags.as_ref() {
            body["Tags"] = tags.clone();
        }

        let arn = import_in_region(&router, case, row.region, &body).await;
        assert_is_a_certificate_reference(case, row.region, &arn);
        assert!(
            seen.insert(arn.clone()),
            "case {case:?}: every import stores its own certificate, so its ARN must be \
             distinct, got {arn:?} again"
        );

        // The reference resolves, and what it resolves to is the certificate the request
        // carried rather than anything the service invented.
        let detail = describe_in_region(&router, row.region, &arn).await;
        for (member, expected) in [
            ("CertificateArn", arn.as_str()),
            ("Type", "IMPORTED"),
            ("Status", "ISSUED"),
            ("Serial", generated.serial.as_str()),
        ] {
            assert_eq!(
                detail[member].as_str(),
                Some(expected),
                "case {case:?}: expected {member} {expected:?} under the ARN the import \
                 answered, got {detail}"
            );
        }
        assert_eq!(
            detail["NotAfter"].as_f64(),
            Some(generated.not_after),
            "case {case:?}: expected the imported certificate's own validity end, got \
             {detail}"
        );

        if first.is_none() {
            first = Some((arn, row.region));
        }
    }

    // The one import that must NOT mint a reference: a request naming an ARN replaces
    // the certificate stored under it, so the reference a consuming service already
    // holds keeps pointing at the replacement.
    let (held, region) = first.expect("the table has rows");
    let case = "a reimport under the ARN the first import answered";
    let replacement = generate(&Blueprint {
        common_name: Some("reference.example.com"),
        algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
        validity: (2022, 2062),
        ..Blueprint::default()
    });
    let mut body = import_body(&replacement);
    body["CertificateArn"] = json!(held.as_str());
    let answered = import_in_region(&router, case, region, &body).await;
    assert_eq!(
        answered, held,
        "case {case:?}: expected the ARN that was replaced, got {answered:?}"
    );
    assert_is_a_certificate_reference(case, region, &answered);

    let detail = describe_in_region(&router, region, &answered).await;
    assert_eq!(
        detail["Serial"].as_str(),
        Some(replacement.serial.as_str()),
        "case {case:?}: expected the replacement's own serial under the held ARN, got \
         {detail}"
    );
}
