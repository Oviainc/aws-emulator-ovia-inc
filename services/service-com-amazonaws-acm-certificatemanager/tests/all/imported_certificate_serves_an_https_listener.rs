// SPDX-License-Identifier: CC0-1.0

//! The REFERENCE a client gets back when it brings its own certificate: the ARN
//! `ImportCertificate` answers, which is what an integrated service is then handed — an
//! Elastic Load Balancing v2 HTTPS or TLS listener's `Certificates[].CertificateArn`, an
//! API Gateway custom domain's `certificateArn`, a CloudFront distribution's
//! `ACMCertificateArn`.
//!
//! RULE: an imported certificate is an ordinary ACM resource, so the ARN the import
//! answers is a certificate reference like any other: it has the form the model gives an
//! ACM ARN (`ImportCertificateResponse$CertificateArn` targets `Arn`, length 20..2048,
//! pattern `^arn:[\w+=/,.@-]+:acm:[\w+=/,.@-]*:[0-9]+:[\w+=,.@-]+(/[\w+=,.@-]+)*$` —
//! `smithy_query(query="shape", shape="com.amazonaws.acm#Arn")`), it names the Region the
//! caller signed for and this account, it is a NEW reference for every import — the same
//! certificate imported twice is two resources — and it is the key every ACM reader
//! resolves that certificate under, reporting the facts of the certificate stored there
//! rather than anything this service invented.
//!
//! Every assertion goes through the same router the emulator serves production traffic
//! on, and every certificate the service is fed is built by `rcgen` — something other
//! than the code under test — so a response that is not computed from the imported body
//! cannot pass.
//!
//! The rows span what the rule generalises over: the Region the request is signed for
//! (four, including the one the rest of this crate's suites use), the certificate's key
//! curve, how many names it covers and in which form (one subject name, several DNS
//! names, a wildcard), and whether it asserts key usages. The ARN's own shape is asserted
//! against the model's pattern rather than against a value written here.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_import::{generate, import_body, Blueprint, Generated};
use crate::certificate_lifecycle::{call_in_region, router, ACCOUNT};

/// The modelled bounds on an ACM ARN (`com.amazonaws.acm#Arn`, `@length(min: 20, max:
/// 2048)`).
const ARN_LENGTH: std::ops::RangeInclusive<usize> = 20..=2048;

/// One row: the Region the import is signed for, the certificate to import, and the name
/// ACM must report as that certificate's `DomainName`.
struct Row {
    case: &'static str,
    region: &'static str,
    certificate: Blueprint<'static>,
    domain_name: &'static str,
}

/// The rows, built here rather than as a `const` because each blueprint carries the names
/// its certificate covers.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a single-name P-256 certificate imported in us-west-2",
            region: "us-west-2",
            certificate: Blueprint {
                common_name: Some("reference.imported.example.com"),
                dns_names: &["reference.imported.example.com"],
                ..Blueprint::default()
            },
            domain_name: "reference.imported.example.com",
        },
        Row {
            case: "a multi-name P-384 certificate asserting key usages imported in \
                   eu-west-1",
            region: "eu-west-1",
            certificate: Blueprint {
                common_name: Some("listener.imported.example.net"),
                dns_names: &[
                    "listener.imported.example.net",
                    "alt.imported.example.net",
                ],
                algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
                key_usages: true,
                ..Blueprint::default()
            },
            domain_name: "listener.imported.example.net",
        },
        Row {
            case: "a wildcard certificate imported in ap-southeast-2",
            region: "ap-southeast-2",
            certificate: Blueprint {
                common_name: Some("*.imported.example.org"),
                dns_names: &["*.imported.example.org"],
                ..Blueprint::default()
            },
            domain_name: "*.imported.example.org",
        },
        Row {
            case: "a certificate whose subject names an organization, imported in \
                   us-east-1",
            region: "us-east-1",
            certificate: Blueprint {
                common_name: Some("ops.imported.example.com"),
                organization: Some("Imported Certificates"),
                validity: (2021, 2051),
                ..Blueprint::default()
            },
            domain_name: "ops.imported.example.com",
        },
    ]
}

/// Import one certificate signed for `region`, requiring success, and answer the ARN it
/// was stored under.
async fn import_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    generated: &Generated,
) -> String {
    let (status, response) =
        call_in_region(router, region, "ImportCertificate", &import_body(generated)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ImportCertificate must answer 200, got {status} {response}"
    );
    response["CertificateArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no CertificateArn in {response}"))
        .to_string()
}

/// `DescribeCertificate`'s detail for `arn`, read in `region`, requiring success.
async fn describe_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    arn: &str,
) -> Value {
    let (status, response) = call_in_region(
        router,
        region,
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

/// The five fields the model's ARN pattern divides an ACM ARN into, after the `arn:`
/// prefix: `arn:<partition>:acm:<region>:<account>:<resource>`.
fn arn_fields(arn: &str, case: &str) -> Vec<String> {
    let fields: Vec<String> = arn.splitn(6, ':').map(str::to_string).collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: an ARN carries six colon-separated fields, got {arn:?}"
    );
    fields
}

/// Whether `id` is a UUID as ACM writes a certificate id: five lowercase-hex groups of
/// 8-4-4-4-12 characters.
fn is_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8usize, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()))
}

/// RULE: the ARN `ImportCertificate` answers is a certificate reference like any other —
/// the modelled ACM ARN form, in the Region the caller signed for and in this account, a
/// new reference for every import, and the key ACM's readers resolve that certificate
/// under.
#[tokio::test]
async fn the_import_answers_the_arn_an_integrated_service_references() {
    let router = router();
    // Every ARN handed out, so the uniqueness of the reference is asserted across the
    // whole run rather than within one row.
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for row in rows() {
        let case = row.case;
        let region = row.region;

        let generated = generate(&row.certificate);
        let arn = import_in(&router, case, region, &generated).await;

        // 1. The reference has the shape the model gives an ACM ARN.
        assert!(
            ARN_LENGTH.contains(&arn.len()),
            "case {case:?}: expected an ARN of {}..={} characters, got {} in {arn:?}",
            ARN_LENGTH.start(),
            ARN_LENGTH.end(),
            arn.len()
        );
        // The characters the whole pattern admits, across every field.
        assert!(
            arn.chars().all(|character| character.is_ascii_alphanumeric()
                || "+=/,.@-_:*".contains(character)),
            "case {case:?}: the ARN carries a character the modelled pattern excludes, \
             got {arn:?}"
        );

        let fields = arn_fields(&arn, case);
        for (index, (what, expected)) in [
            ("the ARN prefix", "arn"),
            ("the partition", "aws"),
            ("the service namespace", "acm"),
            // The Region and the account are not this test's constants: the Region is
            // the one the request was signed for, and the account is the emulator's.
            ("the Region", region),
            ("the account", ACCOUNT),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                fields[index], expected,
                "case {case:?}: expected {what} {expected:?} in the ARN, got {arn:?}"
            );
        }

        // 2. The resource names a CERTIFICATE, under an opaque id of its own.
        let resource = fields[5].clone();
        let id = resource.strip_prefix("certificate/").unwrap_or_else(|| {
            panic!("case {case:?}: expected a certificate/<id> resource, got {resource:?}")
        });
        assert!(
            is_uuid(id),
            "case {case:?}: expected the certificate id to be a UUID, got {id:?}"
        );
        assert!(
            !arn.contains(row.domain_name),
            "case {case:?}: the reference is opaque, not built from the names the \
             certificate covers, got {arn:?}"
        );

        // 3. It is a NEW reference: no earlier import handed out this one, and importing
        //    the SAME certificate again is a second resource with its own ARN.
        assert!(
            minted.insert(arn.clone()),
            "case {case:?}: every import answers its own reference, but {arn:?} was \
             already handed out"
        );
        let again = import_in(&router, case, region, &generated).await;
        assert!(
            minted.insert(again.clone()),
            "case {case:?}: importing the same certificate again must answer a second \
             reference, got {again:?} already handed out"
        );
        assert_ne!(
            again, arn,
            "case {case:?}: an import that names no CertificateArn mints its own ARN"
        );

        // 4. The reference resolves, in the Region it names, to the certificate that was
        //    imported under it — and the second import is a separate resource reporting
        //    the same certificate's facts.
        for (which, reference) in [("the first import", &arn), ("the repeat", &again)] {
            let detail = describe_in(&router, case, region, reference).await;
            for (member, expected) in [
                ("CertificateArn", reference.as_str()),
                ("Type", "IMPORTED"),
                ("Status", "ISSUED"),
                ("Serial", generated.serial.as_str()),
                ("DomainName", row.domain_name),
            ] {
                assert_eq!(
                    detail[member].as_str(),
                    Some(expected),
                    "case {case:?}: expected {member} {expected:?} under {which}'s \
                     reference, got {detail}"
                );
            }
            assert_eq!(
                detail["NotAfter"].as_f64(),
                Some(generated.not_after),
                "case {case:?}: expected the certificate's own validity window under \
                 {which}'s reference, got {detail}"
            );
        }

        // 5. The certificate material ACM hands back under the reference is the material
        //    the client imported, byte for byte.
        let (status, response) = call_in_region(
            &router,
            region,
            "GetCertificate",
            &json!({ "CertificateArn": arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCertificate must answer 200, got {status} {response}"
        );
        assert_eq!(
            response["Certificate"].as_str(),
            Some(generated.certificate.as_str()),
            "case {case:?}: expected the imported PEM under the reference, got {response}"
        );
    }

    assert_eq!(
        minted.len(),
        rows().len() * 2,
        "expected one reference per import, got {minted:?}"
    );
}
