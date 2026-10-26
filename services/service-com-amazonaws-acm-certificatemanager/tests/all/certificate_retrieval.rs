// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetCertificate`: the PEM material ACM hands back for a
//! certificate ARN.
//!
//! RULE: `GetCertificate` reports the certificate material the addressed ACM resource
//! holds — the certificate itself, and the chain of trust it is served with, which is
//! the chain the resource was stored with or the certificate itself when it was stored
//! without one — as the call that stored it carried it, with newline line endings; a
//! resource that holds no certificate is refused with the documented
//! `RequestInProgressException`, and the read never writes anything a later caller can
//! see.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every certificate the service is fed is
//! built by `rcgen` — something other than the code under test — so a body that is not
//! the stored one cannot pass.
//!
//! The table below spans the classes the rule generalises over: the key families and
//! curves the import tests cover, certificates carrying one name and several, a chain
//! absent, a chain of one certificate and a chain of two, both line endings the PEM
//! `@pattern` admits, and the material replaced under an ARN by a reimport.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_import::{blob, generate, import, import_body, Blueprint};
use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, hold_pending_validations, request_certificate,
    router, router_with_engine, ACCOUNT, REGION,
};

/// The `RequestInProgressException` sentence the rule requires. The model documents the
/// whole error class in one sentence — "The certificate request is in process and the
/// certificate in your account has not yet been issued"
/// (`smithy_query(query="doc", shape="com.amazonaws.acm#RequestInProgressException")`) —
/// and that is what the refusal reports.
const REQUEST_IN_PROGRESS_MESSAGE: &str =
    "The certificate request is in process and the certificate in your account has not yet \
     been issued.";

/// One retrieval case: what it exercises, the certificate to generate, how many
/// certificates the chain the import carries holds (`0` for an import with no chain),
/// and whether the import sends its PEM bodies with CRLF line endings.
struct Case<'a> {
    case: &'a str,
    blueprint: Blueprint<'a>,
    chain_len: usize,
    crlf: bool,
}

/// The chain body an import carries for a case, for a self-signed `leaf`.
///
/// A self-signed leaf is its own issuer, so the certificate a chain has to begin with is
/// the leaf's own; anything above that is some other authority's certificate, which a
/// chain may carry.
fn chain_body(leaf_pem: &str, chain_len: usize) -> Option<String> {
    if chain_len == 0 {
        return None;
    }
    let mut chain = leaf_pem.to_string();
    for _ in 1..chain_len {
        let above = generate(&Blueprint {
            common_name: Some("chain-root.example.com"),
            ..Blueprint::default()
        });
        chain.push_str(&above.certificate);
    }
    Some(chain)
}

/// `GetCertificate` for `arn`, as `(status, body)`.
async fn get_certificate(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "GetCertificate", &json!({ "CertificateArn": arn })).await
}

/// One PEM body with CRLF line endings — the other form the `CertificateBody`
/// `@pattern` admits (`\u{000D}?\u{000A}`).
fn as_crlf(pem: &str) -> String {
    pem.replace('\n', "\r\n")
}

/// RULE: the material `GetCertificate` reports is the material the resource was stored
/// from, with newline line endings, and the chain of trust it reports is the stored
/// chain or — for a resource stored without one — the certificate itself.
#[tokio::test]
async fn get_certificate_reports_the_stored_material() {
    let router = router();

    // One row per class the rule generalises over: the key family and curve, the number
    // of names the certificate covers, where those names come from, the shape of the
    // chain stored beside it, and the line endings the import carried.
    let cases = [
        Case {
            case: "a P-256 certificate with no chain",
            blueprint: Blueprint::default(),
            chain_len: 0,
            crlf: false,
        },
        Case {
            case: "a P-256 certificate with a one-certificate chain",
            blueprint: Blueprint {
                common_name: Some("chained.example.com"),
                ..Blueprint::default()
            },
            chain_len: 1,
            crlf: false,
        },
        Case {
            case: "a P-384 certificate with a two-certificate chain",
            blueprint: Blueprint {
                common_name: Some("p384.example.com"),
                algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
                ..Blueprint::default()
            },
            chain_len: 2,
            crlf: false,
        },
        Case {
            case: "a three-name certificate with a one-certificate chain",
            blueprint: Blueprint {
                common_name: Some("one.example.net"),
                dns_names: &["two.example.net", "three.example.net"],
                ..Blueprint::default()
            },
            chain_len: 1,
            crlf: false,
        },
        Case {
            case: "a certificate named only by its extension, no chain",
            blueprint: Blueprint {
                common_name: None,
                // A self-signed certificate's issuer is its own subject, which an import
                // requires to be non-empty, so the subject carries an organization
                // instead of the common name this row leaves out.
                organization: Some("Extension Only Example"),
                dns_names: &["extension-only.example.org"],
                ..Blueprint::default()
            },
            chain_len: 0,
            crlf: false,
        },
        Case {
            case: "a CRLF-encoded certificate with no chain",
            blueprint: Blueprint {
                common_name: Some("crlf.example.com"),
                ..Blueprint::default()
            },
            chain_len: 0,
            crlf: true,
        },
        Case {
            case: "a CRLF-encoded certificate with a two-certificate chain",
            blueprint: Blueprint {
                common_name: Some("crlf-chained.example.com"),
                ..Blueprint::default()
            },
            chain_len: 2,
            crlf: true,
        },
    ];

    for Case {
        case,
        blueprint,
        chain_len,
        crlf,
    } in &cases
    {
        let leaf = generate(blueprint);
        let chain = chain_body(&leaf.certificate, *chain_len);
        // `rcgen` writes PEM with newline endings, so a CRLF row is that same material
        // with the carriage returns put back in — the other form the PEM `@pattern`
        // admits, and the one the read must report without them.
        let mut body = import_body(&leaf);
        if *crlf {
            body["Certificate"] = blob(&as_crlf(&leaf.certificate));
        }
        if let Some(chain) = chain.as_deref() {
            body["CertificateChain"] = blob(&if *crlf {
                as_crlf(chain)
            } else {
                chain.to_string()
            });
        }
        let arn = import(&router, case, &body).await;

        let (status, response) = get_certificate(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCertificate must answer 200, got {status} {response}"
        );
        let returned = response["Certificate"].as_str();
        assert_eq!(
            returned,
            Some(leaf.certificate.as_str()),
            "case {case:?}: expected the imported certificate body, got {returned:?}"
        );
        // A resource stored without a chain holds no issuing authority's certificate,
        // so the chain of trust it is served with is the certificate itself.
        let expected_chain = chain.as_deref().unwrap_or(leaf.certificate.as_str());
        let returned_chain = response["CertificateChain"].as_str();
        assert_eq!(
            returned_chain,
            Some(expected_chain),
            "case {case:?}: expected the chain {expected_chain:?}, got {returned_chain:?}"
        );
    }
}

/// RULE: the read reports the resource, so it is the same for every caller: two reads of
/// one certificate answer the same bytes, and the facts the other read operations report
/// about it are unchanged by having read it.
#[tokio::test]
async fn reading_a_certificate_changes_nothing() {
    let router = router();
    let leaf = generate(&Blueprint {
        common_name: Some("repeat.example.com"),
        ..Blueprint::default()
    });
    let mut body = import_body(&leaf);
    body["CertificateChain"] = blob(&leaf.certificate);
    let arn = import(&router, "a certificate read twice", &body).await;

    let before = describe(&router, &arn).await;
    let mut answers = Vec::new();
    for round in 0..3 {
        let (status, response) = get_certificate(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case read {round}: GetCertificate must answer 200, got {status} {response}"
        );
        answers.push(response);
    }
    for (round, answer) in answers.iter().enumerate() {
        assert_eq!(
            answer, &answers[0],
            "case read {round}: every read must answer the same material, got {answer}"
        );
    }
    let after = describe(&router, &arn).await;
    for member in ["Status", "Type", "Serial", "ImportedAt", "InUseBy"] {
        assert_eq!(
            after[member], before[member],
            "case {member}: reading the certificate must not change what is reported, \
             got {after}"
        );
    }
}

/// RULE: the material belongs to the ARN, so a reimport under one ARN replaces what that
/// ARN answers with — including dropping a chain the replacement does not carry — while
/// every other ARN keeps its own.
#[tokio::test]
async fn a_reimport_replaces_the_material_under_its_arn() {
    let router = router();

    let first = generate(&Blueprint {
        common_name: Some("rotating.example.com"),
        ..Blueprint::default()
    });
    let mut body = import_body(&first);
    body["CertificateChain"] = blob(&first.certificate);
    let arn = import(&router, "the first import", &body).await;

    // One more authority's certificate, to sit above the issuing one in the chains the
    // rows that carry a chain send — so a chain the read reports is never just the
    // certificate it was sent with.
    let above = generate(&Blueprint {
        common_name: Some("above.example.com"),
        ..Blueprint::default()
    });

    // A second resource, untouched by the reimport, so the replacement is proven to be
    // scoped to the ARN it named.
    let other = generate(&Blueprint {
        common_name: Some("settled.example.com"),
        ..Blueprint::default()
    });
    let other_arn = import(&router, "an unrelated import", &import_body(&other)).await;

    // Each row replaces the material under the same ARN and says what the read must
    // answer afterwards: a different key family, a different name, a chain added and a
    // chain dropped.
    let replacements = [
        (
            "a P-384 replacement carrying a chain",
            generate(&Blueprint {
                common_name: Some("rotating.example.com"),
                algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
                ..Blueprint::default()
            }),
            true,
        ),
        (
            "a replacement for a different name, with no chain",
            generate(&Blueprint {
                common_name: Some("renamed.example.com"),
                ..Blueprint::default()
            }),
            false,
        ),
        (
            "a replacement carrying a chain again",
            generate(&Blueprint {
                common_name: Some("renamed.example.com"),
                dns_names: &["alt.renamed.example.com"],
                ..Blueprint::default()
            }),
            true,
        ),
    ];

    for (case, replacement, with_chain) in &replacements {
        // A self-signed replacement is its own issuer, so a chain it carries begins
        // with its own certificate and holds one more above it.
        let sent_chain = with_chain
            .then(|| format!("{}{}", replacement.certificate, above.certificate));
        let mut body = import_body(replacement);
        body["CertificateArn"] = json!(arn);
        if let Some(chain) = sent_chain.as_deref() {
            body["CertificateChain"] = blob(chain);
        }
        let (status, response) = call(&router, "ImportCertificate", &body).await;
        assert_eq!(
            (status, response["CertificateArn"].as_str()),
            (200, Some(arn.as_str())),
            "case {case:?}: a reimport must keep the ARN, got {status} {response}"
        );

        let (status, response) = get_certificate(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCertificate must answer 200, got {status} {response}"
        );
        let returned = response["Certificate"].as_str();
        assert_eq!(
            returned,
            Some(replacement.certificate.as_str()),
            "case {case:?}: expected the replacement body, got {returned:?}"
        );
        // A reimport that carries no chain leaves the resource holding none, so the
        // chain of trust reported for it is the replacement itself — never the chain
        // the ARN answered with before.
        let expected_chain = sent_chain
            .as_deref()
            .unwrap_or(replacement.certificate.as_str());
        let returned_chain = response["CertificateChain"].as_str();
        assert_eq!(
            returned_chain,
            Some(expected_chain),
            "case {case:?}: expected the chain {expected_chain:?}, got {returned_chain:?}"
        );

        let (status, response) = get_certificate(&router, &other_arn).await;
        assert_eq!(
            (status, response["Certificate"].as_str()),
            (200, Some(other.certificate.as_str())),
            "case {case:?}: the unrelated certificate must be untouched, got {status} {response}"
        );
        assert_eq!(
            response["CertificateChain"].as_str(),
            Some(other.certificate.as_str()),
            "case {case:?}: the unrelated certificate was imported with no chain, so it is \
             its own chain of trust, got {response}"
        );
    }
}

/// RULE: a resource this service holds no certificate for is refused with HTTP 400
/// `RequestInProgressException` and the documented sentence — never with an answer
/// carrying no material.
#[tokio::test]
async fn a_certificate_with_no_material_reports_the_in_progress_refusal() {
    let (router, engine) = router_with_engine();

    // Each row is a request ACM has not issued a certificate for. Pinning the owed
    // outcome keeps every one of them in that state for the rest of the test, so the
    // refusal is not a race with the validation delay.
    let cases: &[(&str, Value)] = &[
        (
            "a request validated by DNS",
            json!({ "DomainName": "pending-dns.example.com", "ValidationMethod": "DNS" }),
        ),
        (
            "a request validated by email",
            json!({ "DomainName": "pending-email.example.com", "ValidationMethod": "EMAIL" }),
        ),
        (
            "a request validated by HTTP",
            json!({ "DomainName": "pending-http.example.com", "ValidationMethod": "HTTP" }),
        ),
        (
            "a wildcard request with several names",
            json!({
                "DomainName": "*.pending.example.net",
                "SubjectAlternativeNames": ["alt.pending.example.net"],
                "ValidationMethod": "DNS"
            }),
        ),
    ];

    let mut arns = Vec::new();
    for (case, body) in cases {
        arns.push((case, request_certificate(&router, body).await));
    }
    hold_pending_validations(&engine);

    for (case, arn) in &arns {
        let (status, response) = get_certificate(&router, arn).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "RequestInProgressException"),
            "case {case:?}: expected 400 RequestInProgressException, got {status} {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            message, REQUEST_IN_PROGRESS_MESSAGE,
            "case {case:?}: expected message {REQUEST_IN_PROGRESS_MESSAGE:?}, got {message:?}"
        );
    }
}

/// RULE: an ARN that names no certificate at all is refused before any lookup
/// (`InvalidArnException`), and a certificate ARN with no stored record behind it is
/// refused as missing (`ResourceNotFoundException`) — the same two sentences every
/// certificate-ARN lookup in this service renders.
#[tokio::test]
async fn an_unresolvable_arn_is_refused_by_its_class() {
    let router = router();

    // A stored certificate, so each refusal is drawn by the ARN not resolving rather
    // than by the operation always failing.
    let leaf = generate(&Blueprint {
        common_name: Some("resolves.example.com"),
        ..Blueprint::default()
    });
    let stored = import(&router, "a stored certificate", &import_body(&leaf)).await;

    // Each row: the class of ARN, the ARN itself, the error it draws and the sentence
    // that error must carry. Every ARN satisfies the member's modelled length (20..2048)
    // and `@pattern`, so each one reaches the handler.
    let cases: &[(&str, String, &str, String)] = &[
        (
            "a resource type that is not a certificate",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/abcdefghij"),
            "InvalidArnException",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/abcdefghij is not a valid ARN"),
        ),
        (
            "an ACM ARN with no resource type",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate"),
            "InvalidArnException",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate is not a valid ARN"),
        ),
        (
            "a nested resource below a certificate",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/abcdef/extra"),
            "InvalidArnException",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/abcdef/extra is not a valid ARN"),
        ),
        (
            "a certificate id with nothing behind it",
            format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/11111111-2222-3333-4444-555555555555"),
            "ResourceNotFoundException",
            format!(
                "Could not find certificate arn:aws:acm:{REGION}:{ACCOUNT}:certificate/\
                 11111111-2222-3333-4444-555555555555."
            ),
        ),
        (
            "a stored certificate's id in another region",
            stored.replace(REGION, "eu-central-1"),
            "ResourceNotFoundException",
            format!(
                "Could not find certificate {}.",
                stored.replace(REGION, "eu-central-1")
            ),
        ),
        (
            "a stored certificate's id under another account",
            stored.replace(ACCOUNT, "123456789012"),
            "ResourceNotFoundException",
            format!(
                "Could not find certificate {}.",
                stored.replace(ACCOUNT, "123456789012")
            ),
        ),
    ];

    for (case, arn, expected_code, expected_message) in cases {
        let (status, response) = get_certificate(&router, arn).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, *expected_code),
            "case {case:?}: expected 400 {expected_code} for {arn}, got {status} {response}"
        );
        let message = error_message(&response);
        assert_eq!(
            &message, expected_message,
            "case {case:?}: expected message {expected_message:?}, got {message:?}"
        );
    }

    // A certificate the account no longer holds is missing, not retryable.
    let (status, response) = call(
        &router,
        "DeleteCertificate",
        &json!({ "CertificateArn": stored }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case delete: DeleteCertificate must succeed, got {status} {response}"
    );
    let (status, response) = get_certificate(&router, &stored).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ResourceNotFoundException"),
        "case a deleted certificate: expected 400 ResourceNotFoundException, \
         got {status} {response}"
    );
}
