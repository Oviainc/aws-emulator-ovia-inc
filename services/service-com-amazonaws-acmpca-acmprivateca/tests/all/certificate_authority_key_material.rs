// SPDX-License-Identifier: CC0-1.0

//! The key pair a private CA holds, as the certification request it publishes shows it.
//!
//! The rule under test: a private CA is created with a key pair of the `KeyAlgorithm` its
//! configuration names, and the PKCS#10 certification request it publishes is over THAT key
//! pair — a `SubjectPublicKeyInfo` any X.509 tooling can read as a key of that algorithm and
//! of that size, under the subject the configuration carries, signed with the private half.
//! Everything the CA then signs is signed with the same key, so a certificate it issues
//! verifies against the public key its request published.
//!
//! The rows span the `KeyAlgorithm` members the provider mints — both RSA sizes and all
//! three NIST curves, not only the `RSA_2048` the issue cited — each with the
//! `SigningAlgorithm` of its own family, and each is checked by VERIFYING a signature rather
//! than by inspecting bytes: a key that is not a real key verifies nothing.

use aws_lc_rs::signature::{
    UnparsedPublicKey, VerificationAlgorithm, ECDSA_P256_SHA256_ASN1, ECDSA_P384_SHA384_ASN1,
    ECDSA_P521_SHA512_ASN1, RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_2048_8192_SHA384,
};
use serde_json::{json, Value};
use x509_cert::der::asn1::UintRef;
use x509_cert::der::{Encode, Reader, SliceReader};
use x509_cert::spki::SubjectPublicKeyInfoOwned;

use crate::private_ca_lifecycle::{
    blob, certificate_of, csr_pem, request_of, string, wait_for_the_clock, Emulator,
};

/// The template that produces a ROOT CA's own certificate.
const ROOT_CA_TEMPLATE: &str = "arn:aws:acm-pca:::template/RootCACertificate/V1";

/// The object identifiers a `SubjectPublicKeyInfo` names a key algorithm by, from
/// `smithy_query(query="shape",shape="KeyAlgorithm")`'s members: RSA keys are
/// `rsaEncryption` (RFC 8017 appendix C) and EC keys are `id-ecPublicKey` with the named
/// curve as the algorithm parameters (RFC 5480 section 2.1.1).
const RSA_ENCRYPTION: &str = "1.2.840.113549.1.1.1";
const EC_PUBLIC_KEY: &str = "1.2.840.10045.2.1";

/// What one `KeyAlgorithm` member must produce.
struct Row {
    /// The case name every assertion below names.
    case: &'static str,
    /// The `KeyAlgorithm` the CA is created with.
    key_algorithm: &'static str,
    /// A `SigningAlgorithm` of that key's family.
    signing_algorithm: &'static str,
    /// The algorithm identifier the published `SubjectPublicKeyInfo` must carry.
    key_algorithm_oid: &'static str,
    /// The named curve an EC key's algorithm parameters must name, by its object
    /// identifier (RFC 5480 section 2.1.1.1).
    curve_oid: Option<&'static str>,
    /// The number of bits an RSA key's modulus must have.
    modulus_bits: Option<usize>,
    /// The number of bytes an EC key's uncompressed point must take: one byte of form plus
    /// two coordinates of the curve's size (RFC 5480 section 2.2).
    point_bytes: Option<usize>,
    /// The algorithm a signature made with this key verifies under.
    verification: &'static dyn VerificationAlgorithm,
    /// The fewest DER bytes a request over this key, with the six-attribute subject below,
    /// can take — the key material, the name and the signature. The `RSA_2048` figure is
    /// the one a recorded request of that algorithm is measured against; the others are the
    /// same bound scaled to the key.
    min_der_bytes: usize,
}

/// Every `KeyAlgorithm` member the in-process provider mints a key pair for, with the key
/// shape each one publishes.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "rsa 2048",
            key_algorithm: "RSA_2048",
            signing_algorithm: "SHA256WITHRSA",
            key_algorithm_oid: RSA_ENCRYPTION,
            curve_oid: None,
            modulus_bits: Some(2048),
            point_bytes: None,
            verification: &RSA_PKCS1_2048_8192_SHA256,
            min_der_bytes: 300,
        },
        Row {
            case: "rsa 3072",
            key_algorithm: "RSA_3072",
            signing_algorithm: "SHA384WITHRSA",
            key_algorithm_oid: RSA_ENCRYPTION,
            curve_oid: None,
            modulus_bits: Some(3072),
            point_bytes: None,
            verification: &RSA_PKCS1_2048_8192_SHA384,
            min_der_bytes: 420,
        },
        Row {
            case: "ec prime256v1",
            key_algorithm: "EC_prime256v1",
            signing_algorithm: "SHA256WITHECDSA",
            key_algorithm_oid: EC_PUBLIC_KEY,
            curve_oid: Some("1.2.840.10045.3.1.7"),
            modulus_bits: None,
            point_bytes: Some(65),
            verification: &ECDSA_P256_SHA256_ASN1,
            min_der_bytes: 240,
        },
        Row {
            case: "ec secp384r1",
            key_algorithm: "EC_secp384r1",
            signing_algorithm: "SHA384WITHECDSA",
            key_algorithm_oid: EC_PUBLIC_KEY,
            curve_oid: Some("1.3.132.0.34"),
            modulus_bits: None,
            point_bytes: Some(97),
            verification: &ECDSA_P384_SHA384_ASN1,
            min_der_bytes: 280,
        },
        Row {
            case: "ec secp521r1",
            key_algorithm: "EC_secp521r1",
            signing_algorithm: "SHA512WITHECDSA",
            key_algorithm_oid: EC_PUBLIC_KEY,
            curve_oid: Some("1.3.132.0.35"),
            modulus_bits: None,
            point_bytes: Some(133),
            verification: &ECDSA_P521_SHA512_ASN1,
            min_der_bytes: 320,
        },
    ]
}

/// The subject a CA of every row is created with: six X.500 attributes, so the name in the
/// published request is one a client reads several relative distinguished names out of.
fn subject(common_name: &str) -> Value {
    json!({
        "CommonName": common_name,
        "Country": "US",
        "State": "WA",
        "Locality": "Seattle",
        "Organization": "Example Inc",
        "OrganizationalUnit": "PKI",
    })
}

/// `CreateCertificateAuthority` body for one row.
fn create_request(row: &Row, common_name: &str) -> Value {
    json!({
        "CertificateAuthorityConfiguration": {
            "KeyAlgorithm": row.key_algorithm,
            "SigningAlgorithm": row.signing_algorithm,
            "Subject": subject(common_name),
        },
        "CertificateAuthorityType": "ROOT",
    })
}

/// The bit length of the modulus of a PKCS#1 `RSAPublicKey` (RFC 8017 appendix A.1.1), read
/// with the DER decoder the service itself uses.
fn rsa_modulus_bits(pkcs1: &[u8], case: &str) -> usize {
    let mut reader = SliceReader::new(pkcs1)
        .unwrap_or_else(|error| panic!("case {case:?}: the RSA key is readable DER: {error}"));
    let modulus_bytes = reader
        .sequence(|reader| {
            let modulus: UintRef = reader.decode()?;
            let _exponent: UintRef = reader.decode()?;
            Ok(modulus.as_bytes().len())
        })
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the RSA key is an RSAPublicKey sequence: {error}")
        });
    modulus_bytes * 8
}

/// Everything about one published key: it names the algorithm the CA was created with, it
/// is of the size that algorithm names, and the request it sits in is signed by its private
/// half.
fn assert_published_key(row: &Row, csr: &str, case: &str) -> SubjectPublicKeyInfoOwned {
    let request = request_of(csr, case);

    let key = &request.info.public_key;
    assert_eq!(
        key.algorithm.oid.to_string(),
        row.key_algorithm_oid,
        "case {case:?}: the published key must name the algorithm the CA was created with"
    );
    if let Some(curve) = row.curve_oid {
        let parameters = key
            .algorithm
            .parameters
            .as_ref()
            .unwrap_or_else(|| panic!("case {case:?}: an EC key names its curve"));
        assert_eq!(
            parameters.value(),
            x509_cert::spki::ObjectIdentifier::new(curve)
                .expect("a curve object identifier")
                .as_bytes(),
            "case {case:?}: the published key must name the curve of its KeyAlgorithm"
        );
    }
    let key_bytes = key.subject_public_key.raw_bytes();
    if let Some(bits) = row.modulus_bits {
        assert_eq!(
            rsa_modulus_bits(key_bytes, case),
            bits,
            "case {case:?}: the published key must be of the size its KeyAlgorithm names"
        );
    }
    if let Some(bytes) = row.point_bytes {
        assert_eq!(
            key_bytes.len(),
            bytes,
            "case {case:?}: the published EC key must be the uncompressed point of its curve, \
             got {} bytes",
            key_bytes.len()
        );
    }

    // The request is signed with the private half of that key: this is what tells a real
    // key pair from a placeholder of the right shape.
    let signed = request
        .info
        .to_der()
        .unwrap_or_else(|error| panic!("case {case:?}: the request body re-encodes: {error}"));
    UnparsedPublicKey::new(row.verification, key_bytes)
        .verify(&signed, request.signature.raw_bytes())
        .unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the published certification request must be signed by the key \
                 it publishes, got {error:?}"
            )
        });

    // The subject is the configured one, read back attribute by attribute.
    let name = request.info.subject.to_string();
    for attribute in ["CN=", "C=US", "ST=WA", "L=Seattle", "O=Example Inc", "OU=PKI"] {
        assert!(
            name.contains(attribute),
            "case {case:?}: the published request carries the configured subject, \
             {attribute:?} is missing from {name:?}"
        );
    }
    // A request of this shape cannot be small: an RSA key alone is hundreds of bytes, and a
    // six-attribute name adds its own.
    let der = request
        .to_der()
        .unwrap_or_else(|error| panic!("case {case:?}: the request re-encodes: {error}"));
    assert!(
        der.len() > row.min_der_bytes,
        "case {case:?}: a certification request over a real key with this subject cannot be \
         {} bytes",
        der.len()
    );
    key.clone()
}

/// Every key algorithm the service mints: the published request is over a real key pair of
/// that algorithm, and the CA signs with its private half.
#[tokio::test]
async fn a_cas_published_request_is_over_the_key_pair_it_signs_with() {
    let emulator = Emulator::new();
    let rows = rows();

    // Create every CA first, then wait for the clock ONCE: the certification request is a
    // product of creation and is published when creation settles.
    let mut authorities: Vec<String> = Vec::with_capacity(rows.len());
    for row in &rows {
        let created = emulator
            .ok(
                row.case,
                "CreateCertificateAuthority",
                &create_request(row, &format!("key-{}", row.key_algorithm)),
            )
            .await;
        authorities.push(string(&created, "CertificateAuthorityArn", row.case));
    }
    wait_for_the_clock();

    for (row, arn) in rows.iter().zip(&authorities) {
        let case = row.case;
        let csr = string(
            &emulator
                .ok(
                    case,
                    "GetCertificateAuthorityCsr",
                    &json!({"CertificateAuthorityArn": arn}),
                )
                .await,
            "Csr",
            case,
        );
        let published = assert_published_key(row, &csr, case);

        // The certificate the CA signs for its own request carries that same key, which is
        // what makes the CA's certificate and its request one identity.
        let issued = emulator
            .ok(
                case,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Csr": blob(&csr),
                    "SigningAlgorithm": row.signing_algorithm,
                    "TemplateArn": ROOT_CA_TEMPLATE,
                    "Validity": {"Type": "YEARS", "Value": 3},
                }),
            )
            .await;
        let certificate_arn = string(&issued, "CertificateArn", case);
        wait_for_the_clock();
        let served = emulator
            .ok(
                case,
                "GetCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "CertificateArn": certificate_arn,
                }),
            )
            .await;
        let root = certificate_of(&string(&served, "Certificate", case), case);
        assert_eq!(
            root.tbs_certificate.subject_public_key_info, published,
            "case {case:?}: the CA's own certificate must carry the key its request published"
        );

        // And it is signed with the private half: the signature over the certificate body
        // verifies under the published key.
        let signed = root
            .tbs_certificate
            .to_der()
            .unwrap_or_else(|error| panic!("case {case:?}: the certificate body re-encodes: {error}"));
        UnparsedPublicKey::new(row.verification, published.subject_public_key.raw_bytes())
            .verify(&signed, root.signature.raw_bytes())
            .unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: the certificate the CA signed must verify under the CA's \
                     own key, got {error:?}"
                )
            });

        // The CA takes that certificate, and then signs a leaf with the same key: the
        // leaf's signature verifies under the CA's published key too.
        emulator
            .ok(
                case,
                "ImportCertificateAuthorityCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Certificate": blob(&string(&served, "Certificate", case)),
                }),
            )
            .await;
        let leaf_request = csr_pem(&format!("leaf-{}", row.key_algorithm));
        let leaf_issued = emulator
            .ok(
                case,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": arn,
                    "Csr": blob(&leaf_request),
                    "SigningAlgorithm": row.signing_algorithm,
                    "Validity": {"Type": "DAYS", "Value": 30},
                }),
            )
            .await;
        let leaf_arn = string(&leaf_issued, "CertificateArn", case);
        wait_for_the_clock();
        let leaf_served = emulator
            .ok(
                case,
                "GetCertificate",
                &json!({"CertificateAuthorityArn": arn, "CertificateArn": leaf_arn}),
            )
            .await;
        let leaf = certificate_of(&string(&leaf_served, "Certificate", case), case);
        let leaf_signed = leaf
            .tbs_certificate
            .to_der()
            .unwrap_or_else(|error| panic!("case {case:?}: the leaf body re-encodes: {error}"));
        UnparsedPublicKey::new(row.verification, published.subject_public_key.raw_bytes())
            .verify(&leaf_signed, leaf.signature.raw_bytes())
            .unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: a leaf the CA issued must verify under the CA's own key, \
                     got {error:?}"
                )
            });
    }
}

/// A CA created with a curve and a digest that is not that curve's own: the key pair is
/// still real and still the configured algorithm's, and the CA still publishes it and signs
/// with it.
///
/// `CreateCertificateAuthority` accepts the pairing — the rule it holds a configuration to
/// is the algorithm FAMILY — and the in-process provider offers no algorithm for it, so the
/// signatures such a CA makes are the stand-in that `state::key_material` documents as its
/// one narrower limit. What must hold either way is asserted here: the published key is the
/// real EC key of the configured curve, and the certificate the CA signs for its own request
/// carries that same key.
#[tokio::test]
async fn a_curve_paired_with_another_digest_still_publishes_its_real_key() {
    let emulator = Emulator::new();
    let case = "prime256v1 with a SHA-384 signature";
    let body = json!({
        "CertificateAuthorityConfiguration": {
            "KeyAlgorithm": "EC_prime256v1",
            "SigningAlgorithm": "SHA384WITHECDSA",
            "Subject": subject("mixed-digest-root"),
        },
        "CertificateAuthorityType": "ROOT",
    });
    let arn = string(
        &emulator.ok(case, "CreateCertificateAuthority", &body).await,
        "CertificateAuthorityArn",
        case,
    );
    wait_for_the_clock();

    let csr = string(
        &emulator
            .ok(
                case,
                "GetCertificateAuthorityCsr",
                &json!({"CertificateAuthorityArn": arn}),
            )
            .await,
        "Csr",
        case,
    );
    let request = request_of(&csr, case);
    let key = &request.info.public_key;
    assert_eq!(
        key.algorithm.oid.to_string(),
        EC_PUBLIC_KEY,
        "case {case:?}: the published key names the algorithm the CA was created with"
    );
    assert_eq!(
        key.subject_public_key.raw_bytes().len(),
        65,
        "case {case:?}: the published key is the uncompressed point of prime256v1"
    );

    let issued = emulator
        .ok(
            case,
            "IssueCertificate",
            &json!({
                "CertificateAuthorityArn": arn,
                "Csr": blob(&csr),
                "SigningAlgorithm": "SHA384WITHECDSA",
                "TemplateArn": ROOT_CA_TEMPLATE,
                "Validity": {"Type": "YEARS", "Value": 3},
            }),
        )
        .await;
    let certificate_arn = string(&issued, "CertificateArn", case);
    wait_for_the_clock();
    let served = emulator
        .ok(
            case,
            "GetCertificate",
            &json!({"CertificateAuthorityArn": arn, "CertificateArn": certificate_arn}),
        )
        .await;
    let root = certificate_of(&string(&served, "Certificate", case), case);
    assert_eq!(
        root.tbs_certificate.subject_public_key_info, *key,
        "case {case:?}: the CA's own certificate carries the key its request published"
    );
}
