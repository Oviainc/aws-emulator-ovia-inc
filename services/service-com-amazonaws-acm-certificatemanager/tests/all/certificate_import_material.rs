// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `ImportCertificate` holds the MATERIAL it is handed to:
//! the `CertificateChain` body and the correspondence between `PrivateKey` and the
//! certificate's public key.
//!
//! RULES under test:
//!
//! * a `CertificateChain` is material the service stores to serve back beside the
//!   certificate, not a claim the import is checked against — so a chain is accepted
//!   whatever authority its certificates belong to and whatever they assert, is handed
//!   back by `GetCertificate` as it was sent, and never contributes a fact the record
//!   reports (`Issuer` stays the name the imported certificate's own DER carries);
//! * the only things a chain is held to are being a PEM encoding of certificates and
//!   carrying no certificate whose validity window has run out, each refused with the
//!   modelled constraint failure for supplied input under its own fixed sentence;
//! * a `PrivateKey` that is well formed but belongs to another key pair is the same
//!   class of refusal, under its own fixed sentence, and leaves the certificate stored
//!   at the ARN the request named exactly as it was.
//!
//! Every certificate here is built by `rcgen` — something other than the code under
//! test — and every assertion goes through the same router the emulator serves
//! production traffic on. The tables span the classes the rules generalise over: a
//! chain of an unrelated authority, of the leaf itself, of a certificate that is
//! explicitly not a CA, and of several certificates in either order; the chain defects
//! in each form (no PEM framing at all, PEM framing whose content is not a
//! certificate, a PEM of another type, a type mixed in below a certificate, an expired
//! member first and last); and key mismatches within one curve and across curves, on a
//! fresh import and on a reimport.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_import::{blob, import};
use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, list_all_arns, router,
};

/// The fixed sentence each material refusal this suite exercises carries. They are the
/// only thing telling the refusals apart, so each is asserted in full.
const CHAIN_EXPIRED: &str = "A certificate in the chain is expired. You can Import a \
                             certificate only during its validity period.";
const CHAIN_NOT_PEM: &str = "The certificate chain is not PEM-encoded or is not valid.";
const CHAIN_WRONG_PEM: &str = "Passed PEM is not a certificate chain.";
const KEY_MISMATCH: &str = "Key provided does not match the certificate.";

/// What one generated certificate is built from: its subject common name, whether it
/// asserts `BasicConstraints` ca=true, its validity window as `(not_before, not_after)`
/// years, and the algorithm its key pair uses.
struct Spec<'a> {
    common_name: &'a str,
    ca: bool,
    validity: (i32, i32),
    algorithm: &'static rcgen::SignatureAlgorithm,
}

impl Default for Spec<'_> {
    fn default() -> Self {
        Self {
            common_name: "material.example.com",
            ca: false,
            // A window open across any instant a test runs at.
            validity: (2020, 2100),
            algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
        }
    }
}

/// One generated certificate: the two PEM bodies an import carries.
struct Minted {
    certificate: String,
    private_key: String,
}

/// The `CertificateParams` `spec` describes.
fn params(spec: &Spec<'_>) -> rcgen::CertificateParams {
    let mut params = rcgen::CertificateParams::new(vec![spec.common_name.to_string()])
        .expect("certificate parameters");
    let mut subject = rcgen::DistinguishedName::new();
    subject.push(rcgen::DnType::CommonName, spec.common_name);
    params.distinguished_name = subject;
    params.not_before = rcgen::date_time_ymd(spec.validity.0, 1, 1);
    params.not_after = rcgen::date_time_ymd(spec.validity.1, 1, 1);
    params.is_ca = if spec.ca {
        rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained)
    } else {
        // `BasicConstraints` present with ca=false, so the chain member really does
        // assert that it is not a certificate authority.
        rcgen::IsCa::ExplicitNoCa
    };
    params
}

/// Generate a real self-signed certificate from `spec`, beside the key it belongs to.
fn self_signed(spec: &Spec<'_>) -> Minted {
    let key = rcgen::KeyPair::generate_for(spec.algorithm).expect("a key pair");
    let certificate = params(spec)
        .self_signed(&key)
        .expect("a self-signed certificate")
        .pem();
    Minted {
        certificate,
        private_key: key.serialize_pem(),
    }
}

/// A certificate authority a leaf can be signed by, beside the PEM of its own
/// certificate.
struct Authority {
    certificate: rcgen::Certificate,
    key: rcgen::KeyPair,
    pem: String,
    /// The authority's subject common name, which is therefore the `Issuer` ACM must
    /// report for every leaf it signs.
    common_name: String,
}

impl Authority {
    /// Generate a real certificate authority named `common_name`.
    fn new(common_name: &str) -> Self {
        let spec = Spec {
            common_name,
            ca: true,
            ..Spec::default()
        };
        let key = rcgen::KeyPair::generate_for(spec.algorithm).expect("a key pair");
        let certificate = params(&spec)
            .self_signed(&key)
            .expect("a certificate authority");
        let pem = certificate.pem();
        Self {
            certificate,
            key,
            pem,
            common_name: common_name.to_string(),
        }
    }

    /// Generate a real leaf certificate from `spec`, SIGNED by this authority — so the
    /// leaf's own DER names this authority as its issuer.
    fn sign(&self, spec: &Spec<'_>) -> Minted {
        let key = rcgen::KeyPair::generate_for(spec.algorithm).expect("a key pair");
        let certificate = params(spec)
            .signed_by(&key, &self.certificate, &self.key)
            .expect("a signed certificate")
            .pem();
        Minted {
            certificate,
            private_key: key.serialize_pem(),
        }
    }
}

/// The `ImportCertificate` request body for one generated certificate.
fn import_body(minted: &Minted) -> Value {
    json!({
        "Certificate": blob(&minted.certificate),
        "PrivateKey": blob(&minted.private_key),
    })
}

/// `GetCertificate`'s answer for `arn`, requiring success.
async fn get_certificate(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let (status, response) = call(router, "GetCertificate", &json!({ "CertificateArn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCertificate must answer 200, got {status} {response}"
    );
    response
}

/// Every certificate ARN the store holds under the key algorithms this suite imports
/// under, so a refusal can be shown to have stored nothing. The default listing
/// reports only RSA key types, so the EC ones are named explicitly.
async fn stored_arns(router: &MultiServiceRouter) -> Vec<String> {
    list_all_arns(
        router,
        &json!({ "Includes": { "keyTypes": ["EC_prime256v1", "EC_secp384r1"] } }),
    )
    .await
}

/// Require that `body` is refused with HTTP 400 `ValidationException` and exactly
/// `message`.
async fn refused(router: &MultiServiceRouter, case: &str, body: &Value, message: &str) {
    let (status, response) = call(router, "ImportCertificate", body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the import to be refused with 400, got {status} {response}"
    );
    let code = error_code(&response);
    assert_eq!(
        code, "ValidationException",
        "case {case:?}: expected ValidationException, got {code:?} in {response}"
    );
    let actual = error_message(&response);
    assert_eq!(
        actual, message,
        "case {case:?}: expected the message {message:?}, got {actual:?}"
    );
}

/// RULE: the chain is stored and served back as sent, whatever its certificates are,
/// and contributes nothing to what the record reports — `Issuer` is the authority that
/// signed the IMPORTED certificate, read out of its own DER.
#[tokio::test]
async fn a_chain_is_stored_as_sent_and_decides_nothing() {
    let router = router();

    // Two unrelated authorities. The leaf is signed by the first, so `Issuer` must name
    // it however the chain is composed.
    let signer = Authority::new("signer.example.com");
    let stranger = Authority::new("stranger.example.com");
    let leaf = signer.sign(&Spec {
        common_name: "chained.example.com",
        ..Spec::default()
    });
    // A certificate that explicitly asserts it is NOT a certificate authority, so a
    // chain carrying it could not be a chain of trust at all.
    let not_a_ca = self_signed(&Spec {
        common_name: "end-entity.example.com",
        ..Spec::default()
    });

    // One row per chain shape the rule generalises over: the authority that really
    // signed the leaf, an authority that did not, the leaf itself, a non-CA
    // certificate, and several certificates in both orders.
    let chains: &[(&str, String)] = &[
        ("the authority that signed the leaf", signer.pem.clone()),
        ("an unrelated authority's certificate", stranger.pem.clone()),
        ("the leaf's own certificate", leaf.certificate.clone()),
        (
            "a certificate asserting it is not a CA",
            not_a_ca.certificate.clone(),
        ),
        (
            "an unrelated authority below a non-CA certificate",
            format!("{}{}", not_a_ca.certificate, stranger.pem),
        ),
        (
            "an unrelated authority above the signing one",
            format!("{}{}", signer.pem, stranger.pem),
        ),
    ];

    for (case, chain) in chains {
        let mut body = import_body(&leaf);
        body["CertificateChain"] = blob(chain);
        let arn = import(&router, case, &body).await;

        let detail = describe(&router, &arn).await;
        let status = detail["Status"].as_str();
        assert_eq!(
            status,
            Some("ISSUED"),
            "case {case:?}: an imported certificate is already issued, got {status:?} in {detail}"
        );
        let expected_issuer = signer.common_name.clone();
        let issuer = detail["Issuer"].as_str().unwrap_or_default();
        assert_eq!(
            issuer, expected_issuer,
            "case {case:?}: expected the Issuer read from the leaf's DER \
             ({expected_issuer:?}), got {issuer:?}"
        );
        let domain = detail["DomainName"].as_str();
        assert_eq!(
            domain,
            Some("chained.example.com"),
            "case {case:?}: the stored certificate must be the leaf, got {domain:?} in {detail}"
        );

        let served = get_certificate(&router, case, &arn).await;
        assert_eq!(
            served["Certificate"].as_str(),
            Some(leaf.certificate.as_str()),
            "case {case:?}: expected the leaf back as imported, got {served}"
        );
        assert_eq!(
            served["CertificateChain"].as_str(),
            Some(chain.as_str()),
            "case {case:?}: expected the chain back as sent, got {served}"
        );
    }
}

/// RULE: a chain is held to being a PEM encoding of certificates and to its members'
/// validity windows, and nothing else — each failure the modelled constraint failure
/// for supplied input, under its own fixed sentence, with nothing stored.
#[tokio::test]
async fn a_chain_that_is_not_servable_is_refused() {
    let router = router();

    let leaf = self_signed(&Spec {
        common_name: "refused-chain.example.com",
        ..Spec::default()
    });
    let valid_ca = Authority::new("valid-ca.example.com");
    // An authority whose own validity window closed before any instant a test runs at.
    let expired_ca = Authority::new("expired-ca.example.com");
    let expired = params(&Spec {
        common_name: "expired-ca.example.com",
        ca: true,
        validity: (2000, 2001),
        ..Spec::default()
    })
    .self_signed(&expired_ca.key)
    .expect("an expired certificate authority")
    .pem();

    // One row per form the two held-to properties can fail in: no PEM framing at all,
    // framing whose content is not a certificate, a well-formed PEM of another type
    // alone and mixed in below a certificate, and an expired member in first and last
    // position.
    let cases: &[(&str, String, &str)] = &[
        ("text that frames no PEM message", "not a chain".to_string(), CHAIN_NOT_PEM),
        (
            "a CERTIFICATE block whose content is not base64",
            "-----BEGIN CERTIFICATE-----\nnot-base64!!!\n-----END CERTIFICATE-----\n".to_string(),
            CHAIN_NOT_PEM,
        ),
        (
            "a CERTIFICATE block whose content is not a certificate",
            "-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----\n".to_string(),
            CHAIN_NOT_PEM,
        ),
        (
            "a PEM private key",
            leaf.private_key.clone(),
            CHAIN_WRONG_PEM,
        ),
        (
            "a PEM private key below a certificate",
            format!("{}{}", valid_ca.pem, leaf.private_key),
            CHAIN_WRONG_PEM,
        ),
        (
            "an expired certificate alone",
            expired.clone(),
            CHAIN_EXPIRED,
        ),
        (
            "an expired certificate below a valid one",
            format!("{}{}", valid_ca.pem, expired),
            CHAIN_EXPIRED,
        ),
        (
            "an expired certificate above a valid one",
            format!("{}{}", expired, valid_ca.pem),
            CHAIN_EXPIRED,
        ),
    ];

    for (case, chain, message) in cases {
        let mut body = import_body(&leaf);
        body["CertificateChain"] = blob(chain);
        refused(&router, case, &body, message).await;
    }

    let listed = stored_arns(&router).await;
    assert!(
        listed.is_empty(),
        "case {:?}: a refused import must store nothing, got {listed:?}",
        "every chain refusal above"
    );
}

/// RULE: a `PrivateKey` that does not belong to the certificate's public key is the
/// same class of refusal under its own fixed sentence — on a fresh import and on a
/// reimport, where the certificate already stored at the named ARN is left byte for
/// byte as it was.
#[tokio::test]
async fn a_key_that_is_not_the_certificates_is_refused() {
    let router = router();

    // The certificate the reimports target, imported with the key that really does
    // belong to it: the positive control.
    let stored = self_signed(&Spec {
        common_name: "rotating.example.com",
        ..Spec::default()
    });
    let arn = import(&router, "the certificate to replace", &import_body(&stored)).await;
    let before = get_certificate(&router, "the certificate to replace", &arn).await;
    assert_eq!(
        before["Certificate"].as_str(),
        Some(stored.certificate.as_str()),
        "case {:?}: expected the imported body back, got {before}",
        "the certificate to replace"
    );

    // One row per key-mismatch class: another key pair on the same curve, and a key
    // pair of a different curve, against certificates of each curve.
    let p256 = self_signed(&Spec {
        common_name: "rotating.example.com",
        ..Spec::default()
    });
    let p384 = self_signed(&Spec {
        common_name: "rotating.example.com",
        algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
        ..Spec::default()
    });
    let other_p256 = self_signed(&Spec::default());
    let other_p384 = self_signed(&Spec {
        algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
        ..Spec::default()
    });

    let cases: &[(&str, &Minted, &Minted)] = &[
        (
            "a P-256 certificate with another P-256 key pair's key",
            &p256,
            &other_p256,
        ),
        (
            "a P-256 certificate with a P-384 key",
            &p256,
            &other_p384,
        ),
        (
            "a P-384 certificate with another P-384 key pair's key",
            &p384,
            &other_p384,
        ),
        (
            "a P-384 certificate with a P-256 key",
            &p384,
            &other_p256,
        ),
    ];

    for (case, certificate, key) in cases {
        // A reimport onto the stored ARN, and a fresh import carrying no ARN: the
        // refusal belongs to the material, not to which of the two the request is.
        let fresh = json!({
            "Certificate": blob(&certificate.certificate),
            "PrivateKey": blob(&key.private_key),
        });
        refused(&router, case, &fresh, KEY_MISMATCH).await;

        let mut reimport = fresh.clone();
        reimport["CertificateArn"] = json!(arn);
        let reimport_case = format!("{case}, onto an existing ARN");
        refused(&router, &reimport_case, &reimport, KEY_MISMATCH).await;

        // The refused reimport changed nothing: the ARN still answers the material it
        // was imported with.
        let after = get_certificate(&router, &reimport_case, &arn).await;
        assert_eq!(
            after, before,
            "case {reimport_case:?}: a refused reimport must leave the stored material \
             byte for byte as it was, got {after}"
        );
    }

    // Nothing above was stored beside the control.
    let listed = stored_arns(&router).await;
    assert_eq!(
        listed,
        vec![arn.clone()],
        "case {:?}: only the control import may be stored, got {listed:?}",
        "every key refusal above"
    );
}
