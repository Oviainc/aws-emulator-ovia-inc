// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for three facts ACM reports about an IMPORTED certificate, each
//! read out of the DER the request carried:
//!
//! 1. `CertificateDetail$Issuer` is "the name of the certificate authority that issued
//!    and signed the certificate" — the common-name VALUE the certificate's issuer
//!    field carries, alone, with no attribute-type prefix and none of the other
//!    relative distinguished names that field places the authority under;
//! 2. a public key whose algorithm, size or curve is outside the `KeyAlgorithm` values
//!    the model declares is material ACM can report no certificate under, so the import
//!    is refused as supplied input failing the service's constraints (HTTP 400
//!    `ValidationException`) rather than as an invalid input parameter;
//! 3. a certificate carrying NO key-usage extension restricts or asserts nothing, and
//!    that absence is itself a reportable value — the sentinel its own enum declares for
//!    it: `ANY` for the Key Usage extension (a key no extension restricts is open to
//!    every usage) and `NONE` for the Extended Key Usage extension — in every projection
//!    of the certificate AND in the filters that select on those values.
//!
//! Every certificate is built outside the code under test: by `rcgen`, or — for a key
//! size no signing backend will mint — by assembling the DER here. Every assertion goes
//! through the same router the emulator serves production traffic on, and each table
//! spans the class its rule generalises over rather than one recorded request.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use x509_cert::der::asn1::{Any, BitString, UintRef};
use x509_cert::der::pem::LineEnding;
use x509_cert::der::{DecodePem, Encode, EncodePem};
use x509_cert::spki::{AlgorithmIdentifierOwned, ObjectIdentifier, SubjectPublicKeyInfoOwned};

use crate::certificate_distinguished_names::{import_body as signed_import_body, Authority};
use crate::certificate_import::{blob, generate, import, import_body, Blueprint};
use crate::certificate_lifecycle::{
    call, describe, error_code, error_message, list_all_arns, router, KEY_ALGORITHMS,
};

/// The `ExtendedKeyUsageName` a certificate that carries no Extended Key Usage
/// extension is reported and selected under
/// (`smithy_query(query="shape", shape="com.amazonaws.acm#ExtendedKeyUsageName")`, whose
/// values are the nine key purposes plus `ANY`, `CUSTOM` and this sentinel).
const NONE_SENTINEL: &str = "NONE";

/// The `KeyUsageName` a certificate that carries no Key Usage extension is reported and
/// selected under (`smithy_query(query="shape",
/// shape="com.amazonaws.acm#KeyUsageName")`, whose values are the nine RFC 5280 key
/// usages plus `CUSTOM` and this sentinel).
const ANY_SENTINEL: &str = "ANY";

/// The refusal ACM answers for key material it has no `KeyAlgorithm` to report a
/// certificate under — a fixed sentence about the key TYPE, whichever algorithm, size or
/// curve the refused certificate carries.
const UNSUPPORTED_KEY: &str = "ACM does not support this key type.";

/// `rsaEncryption`, the algorithm identifier an RSA public key is carried under (RFC
/// 8017 appendix C).
const RSA_ENCRYPTION: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");

/// One issuer case: what it exercises, the certificate to mint, and the `Issuer` ACM
/// must then report for it.
struct IssuerCase<'a> {
    case: &'a str,
    blueprint: Blueprint<'a>,
    issuer: &'a str,
    /// The `Subject` the same detail must report, which stays the WHOLE subject
    /// distinguished name — the two members are different facts and this suite must not
    /// be satisfiable by reporting one of them twice.
    subject: &'a str,
}

/// RULE: `Issuer` is the issuing authority's NAME — the common-name value the
/// certificate's own issuer field carries, alone. A self-signed import is its own
/// issuer and so reports its own common name; an issuer field that names no common name
/// has only the field itself for a name.
#[tokio::test]
async fn the_issuer_is_the_issuing_authoritys_common_name() {
    // One row per class the rule generalises over: a common name alone; a common name
    // beside another relative distinguished name (so a reported DN would show); a
    // wildcard name; a name carried only on a certificate whose issuer field names no
    // common name at all.
    let cases = &[
        IssuerCase {
            case: "a common name alone",
            blueprint: Blueprint {
                common_name: Some("apex.issuer-probe.example.com"),
                ..Blueprint::default()
            },
            issuer: "apex.issuer-probe.example.com",
            subject: "CN=apex.issuer-probe.example.com",
        },
        IssuerCase {
            case: "a common name beside an organization",
            blueprint: Blueprint {
                common_name: Some("paired.issuer-probe.example.com"),
                organization: Some("Issuer Probe Organization"),
                ..Blueprint::default()
            },
            issuer: "paired.issuer-probe.example.com",
            subject: "CN=paired.issuer-probe.example.com,O=Issuer Probe Organization",
        },
        IssuerCase {
            case: "a wildcard common name beside an organization",
            blueprint: Blueprint {
                common_name: Some("*.wild.issuer-probe.example.com"),
                organization: Some("Issuer Probe Organization"),
                ..Blueprint::default()
            },
            issuer: "*.wild.issuer-probe.example.com",
            subject: "CN=*.wild.issuer-probe.example.com,O=Issuer Probe Organization",
        },
        IssuerCase {
            case: "no common name: the issuer field has only its own name",
            blueprint: Blueprint {
                common_name: None,
                organization: Some("Nameless Issuer Probe"),
                dns_names: &["nameless.issuer-probe.example.com"],
                ..Blueprint::default()
            },
            issuer: "O=Nameless Issuer Probe",
            subject: "O=Nameless Issuer Probe",
        },
    ];

    let router = router();
    for IssuerCase {
        case,
        blueprint,
        issuer,
        subject,
    } in cases
    {
        let generated = generate(blueprint);
        let arn = import(&router, case, &import_body(&generated)).await;
        let detail = describe(&router, &arn).await;

        let reported = detail["Issuer"].as_str().unwrap_or_default();
        assert_eq!(
            reported, *issuer,
            "case {case:?}: expected Issuer {issuer:?}, got {reported:?}"
        );
        let reported_subject = detail["Subject"].as_str().unwrap_or_default();
        assert_eq!(
            reported_subject, *subject,
            "case {case:?}: expected Subject {subject:?}, got {reported_subject:?}"
        );
        // An authority NAMED by the certificate is named on its own: no attribute-type
        // prefix and no second component, whatever else the issuer field carries.
        if let Some(common_name) = blueprint.common_name {
            assert!(
                !reported.contains('=') && !reported.contains(','),
                "case {case:?}: expected the bare common name {common_name:?} with no \
                 distinguished-name syntax, got {reported:?}"
            );
        }
    }
}

/// RULE, for a certificate signed by someone else: the reported `Issuer` is the SIGNING
/// authority's common name — not the certificate's own name, and not the authority's
/// whole distinguished name.
#[tokio::test]
async fn the_issuer_of_a_signed_leaf_is_the_authoritys_common_name() {
    let case = "a leaf signed by an authority whose name carries a second component";
    // The authority minted by the shared helper the distinguished-name suite owns, so
    // only one suite builds a certificate authority.
    let authority = Authority::new(&[
        (rcgen::DnType::CommonName, "ca.issuer-probe.example.net"),
        (rcgen::DnType::OrganizationName, "Issuer Probe Authority"),
    ]);
    let leaf = authority.sign(
        &[(rcgen::DnType::CommonName, "leaf.issuer-probe.example.net")],
        &["leaf.issuer-probe.example.net"],
    );

    let router = router();
    let arn = import(&router, case, &signed_import_body(&leaf)).await;
    let detail = describe(&router, &arn).await;

    let issuer = detail["Issuer"].as_str().unwrap_or_default();
    assert_eq!(
        issuer, "ca.issuer-probe.example.net",
        "case {case:?}: expected the authority's common name, got {issuer:?}"
    );
    let subject = detail["Subject"].as_str().unwrap_or_default();
    assert_eq!(
        subject, "CN=leaf.issuer-probe.example.net",
        "case {case:?}: expected the leaf's own subject, got {subject:?}"
    );
}

/// A certificate whose public key is an `rsaEncryption` key of `bits` bits.
///
/// Assembled here rather than generated: no signing backend will mint — or sign with —
/// an RSA key of a size outside the ones ACM declares, and the key the certificate
/// CARRIES is the one fact these rows are about. A real `rcgen` certificate's
/// `SubjectPublicKeyInfo` is replaced with such a key and the result re-encoded as DER,
/// so the import reads the key out of a well-formed certificate.
fn rsa_certificate(bits: usize) -> String {
    let template = generate(&Blueprint {
        common_name: Some("key-size.example.com"),
        ..Blueprint::default()
    });
    let mut certificate =
        x509_cert::Certificate::from_pem(&template.certificate).expect("the template parses");

    // A modulus of exactly `bits` bits: the leading byte carries the top bit, and the
    // last is odd as a product of two odd primes is.
    let mut modulus = vec![0xff_u8; bits / 8];
    *modulus.last_mut().expect("a non-empty modulus") = 0x8f;
    // 65537, the public exponent every generator picks (RFC 8017 section 3.1).
    let exponent: [u8; 3] = [0x01, 0x00, 0x01];
    let key = pkcs1::RsaPublicKey {
        modulus: UintRef::new(&modulus).expect("a positive modulus"),
        public_exponent: UintRef::new(&exponent).expect("a positive exponent"),
    }
    .to_der()
    .expect("the RSA public key encodes");

    certificate.tbs_certificate.subject_public_key_info = SubjectPublicKeyInfoOwned {
        algorithm: AlgorithmIdentifierOwned {
            oid: RSA_ENCRYPTION,
            parameters: Some(Any::null()),
        },
        subject_public_key: BitString::from_bytes(&key).expect("a public-key bit string"),
    };
    certificate
        .to_pem(LineEnding::LF)
        .expect("the certificate re-encodes")
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

/// RULE: a public key of an algorithm, a size or a curve outside the `KeyAlgorithm`
/// values the model declares is material ACM can report no certificate under, so the
/// import is refused as supplied input that failed the service's constraints — HTTP 400
/// `ValidationException` under ACM's sentence for an unsupported key type — and nothing
/// is stored.
#[tokio::test]
async fn a_key_outside_the_declared_algorithms_is_refused_as_supplied_input() {
    // An Ed25519 key, so that a whole family outside the declared ones is covered, and
    // each published RSA bound with one size past it: below the smallest declared size,
    // between two of them, and above the largest.
    let ed25519 = generate(&Blueprint {
        common_name: Some("ed25519.example.com"),
        algorithm: &rcgen::PKCS_ED25519,
        ..Blueprint::default()
    });
    let valid = generate(&Blueprint::default());

    let refusals: &[(&str, Value)] = &[
        (
            "an Ed25519 key, a family no KeyAlgorithm names",
            import_body(&ed25519),
        ),
        (
            "an RSA key below the smallest declared size",
            json!({
                "Certificate": blob(&rsa_certificate(512)),
                "PrivateKey": blob(&valid.private_key),
            }),
        ),
        (
            "an RSA key between two declared sizes",
            json!({
                "Certificate": blob(&rsa_certificate(1536)),
                "PrivateKey": blob(&valid.private_key),
            }),
        ),
        (
            "an RSA key above the largest declared size",
            json!({
                "Certificate": blob(&rsa_certificate(8192)),
                "PrivateKey": blob(&valid.private_key),
            }),
        ),
    ];

    let router = router();
    for (case, body) in refusals {
        refused(&router, case, body, UNSUPPORTED_KEY).await;
    }

    // Each size the enum DOES declare, including both published bounds, gets past this
    // refusal: the import is then held to the guideline that the private key must match
    // the certificate, which is a different sentence about different material.
    for bits in [1024, 2048, 3072, 4096] {
        let case = format!("an RSA key of {bits} bits, a size the enum declares");
        refused(
            &router,
            &case,
            &json!({
                "Certificate": blob(&rsa_certificate(bits)),
                "PrivateKey": blob(&valid.private_key),
            }),
            "Key provided does not match the certificate.",
        )
        .await;
    }

    let stored =
        list_all_arns(&router, &json!({ "Includes": { "keyTypes": KEY_ALGORITHMS } })).await;
    assert!(
        stored.is_empty(),
        "a refused import must store nothing, got {stored:?}"
    );
}

/// One key-usage case: what it exercises, the certificate to mint, and the
/// `KeyUsageName` and `ExtendedKeyUsageName` values ACM must then report for it.
struct PurposeCase<'a> {
    case: &'a str,
    blueprint: Blueprint<'a>,
    usages: &'a [&'a str],
    extended: &'a [&'a str],
}

/// One imported certificate of that table: the case, the ARN it was stored under and
/// the names each key-usage member must report for it.
struct Imported<'a> {
    case: &'a str,
    arn: String,
    usages: &'a [&'a str],
    extended: &'a [&'a str],
}

/// The `Name` of each entry a detail's `member` list carries — `KeyUsages` and
/// `ExtendedKeyUsages` are both lists of objects whose `Name` is the enum value.
fn reported_names(case: &str, detail: &Value, member: &str) -> Vec<String> {
    detail[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {detail}"))
        .iter()
        .map(|usage| usage["Name"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Every ARN `SearchCertificates` reports for `filter`, paging to the end.
async fn searched_arns(router: &MultiServiceRouter, filter: &Value) -> Vec<String> {
    let mut arns: Vec<String> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut body = json!({ "FilterStatement": { "Filter": filter } });
        if let Some(token) = next.as_ref() {
            body["NextToken"] = json!(token);
        }
        let (status, response) = call(router, "SearchCertificates", &body).await;
        assert_eq!(
            status, 200,
            "case {body}: SearchCertificates must succeed, got {status} {response}"
        );
        for result in response["Results"]
            .as_array()
            .unwrap_or_else(|| panic!("case {body}: no Results in {response}"))
        {
            arns.push(
                result["CertificateArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {body}: a result carried no ARN"))
                    .to_string(),
            );
        }
        next = response["NextToken"].as_str().map(str::to_string);
        if next.is_none() {
            break;
        }
    }
    arns.sort();
    arns
}

/// One `ListCertificates` page for the named key-usage filter, over every key algorithm
/// so the listing's own default key-type filter cannot hide a row.
fn listing(member: &str, names: &[&str]) -> Value {
    json!({
        "Includes": {
            "keyTypes": KEY_ALGORITHMS,
            member: names,
        }
    })
}

/// Every ARN `ListCertificates` reports for that filter, paging to the end.
async fn listed_for(router: &MultiServiceRouter, member: &str, names: &[&str]) -> Vec<String> {
    let mut arns = list_all_arns(router, &listing(member, names)).await;
    arns.sort();
    arns
}

/// The ARNs of the imported certificates whose `member` names include `name`, sorted as
/// the listing and the search report them.
fn expected_arns(imported: &[Imported<'_>], member: &str, name: &str) -> Vec<String> {
    let mut arns: Vec<String> = imported
        .iter()
        .filter(|entry| names_of(entry, member).contains(&name))
        .map(|entry| entry.arn.clone())
        .collect();
    arns.sort();
    arns
}

/// The names one imported certificate must report for `member`.
fn names_of<'a>(entry: &Imported<'a>, member: &str) -> &'a [&'a str] {
    if member == "keyUsage" {
        entry.usages
    } else {
        entry.extended
    }
}

/// RULE: a certificate carrying no key-usage extension restricts or asserts nothing, and
/// that absence is the sentinel its own enum declares for it — `ANY` for Key Usage and
/// `NONE` for Extended Key Usage — reported by every projection of the certificate and
/// selected by every filter that names it, exactly as a value the certificate does carry
/// is.
#[tokio::test]
async fn the_absence_of_a_key_usage_extension_is_its_own_sentinel() {
    // One row per class the rule generalises over: both extensions absent, on
    // certificates of two key families and with one name and several; the extended
    // purposes present with one purpose, with two, and with the catch-all purpose; and
    // both extensions present together.
    let cases = &[
        PurposeCase {
            case: "no extension, P-256, one name",
            blueprint: Blueprint {
                common_name: Some("bare-one.purpose.example.com"),
                ..Blueprint::default()
            },
            usages: &[ANY_SENTINEL],
            extended: &[NONE_SENTINEL],
        },
        PurposeCase {
            case: "no extension, P-384, three names",
            blueprint: Blueprint {
                common_name: Some("bare-two.purpose.example.com"),
                dns_names: &[
                    "alias-one.purpose.example.com",
                    "alias-two.purpose.example.com",
                ],
                algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
                ..Blueprint::default()
            },
            usages: &[ANY_SENTINEL],
            extended: &[NONE_SENTINEL],
        },
        PurposeCase {
            case: "no extension, P-256, an organization in the subject",
            blueprint: Blueprint {
                common_name: Some("bare-three.purpose.example.com"),
                organization: Some("Purpose Probe Organization"),
                ..Blueprint::default()
            },
            usages: &[ANY_SENTINEL],
            extended: &[NONE_SENTINEL],
        },
        PurposeCase {
            case: "extended purposes present, key usage absent: server authentication",
            blueprint: Blueprint {
                common_name: Some("server.purpose.example.com"),
                extended_key_usages: &[rcgen::ExtendedKeyUsagePurpose::ServerAuth],
                ..Blueprint::default()
            },
            usages: &[ANY_SENTINEL],
            extended: &["TLS_WEB_SERVER_AUTHENTICATION"],
        },
        PurposeCase {
            case: "extended purposes present: client authentication and code signing",
            blueprint: Blueprint {
                common_name: Some("client.purpose.example.com"),
                extended_key_usages: &[
                    rcgen::ExtendedKeyUsagePurpose::ClientAuth,
                    rcgen::ExtendedKeyUsagePurpose::CodeSigning,
                ],
                ..Blueprint::default()
            },
            usages: &[ANY_SENTINEL],
            extended: &["TLS_WEB_CLIENT_AUTHENTICATION", "CODE_SIGNING"],
        },
        PurposeCase {
            case: "extended purposes present: the catch-all purpose",
            blueprint: Blueprint {
                common_name: Some("any.purpose.example.com"),
                extended_key_usages: &[rcgen::ExtendedKeyUsagePurpose::Any],
                ..Blueprint::default()
            },
            usages: &[ANY_SENTINEL],
            extended: &["ANY"],
        },
        PurposeCase {
            case: "both extensions present",
            blueprint: Blueprint {
                common_name: Some("both.purpose.example.com"),
                key_usages: true,
                ..Blueprint::default()
            },
            usages: &["DIGITAL_SIGNATURE", "KEY_ENCIPHERMENT"],
            extended: &[
                "TLS_WEB_SERVER_AUTHENTICATION",
                "TLS_WEB_CLIENT_AUTHENTICATION",
            ],
        },
    ];

    let router = router();
    let mut imported: Vec<Imported<'_>> = Vec::new();
    for PurposeCase {
        case,
        blueprint,
        usages,
        extended,
    } in cases
    {
        let generated = generate(blueprint);
        let arn = import(&router, case, &import_body(&generated)).await;
        imported.push(Imported {
            case,
            arn,
            usages,
            extended,
        });
    }

    // The detail of each certificate reports exactly the values it carries, and the
    // sentinel exactly where the extension is absent — so a reader can tell "restricted
    // to nothing" apart from a record whose extensions are not known.
    for entry in &imported {
        let detail = describe(&router, &entry.arn).await;
        for (member, names) in [
            ("KeyUsages", entry.usages),
            ("ExtendedKeyUsages", entry.extended),
        ] {
            let reported = reported_names(entry.case, &detail, member);
            assert_eq!(
                reported,
                names.iter().map(|name| (*name).to_string()).collect::<Vec<_>>(),
                "case {:?}: expected {member} {names:?}, got {detail}",
                entry.case
            );
        }
        // The extended sentinel stands for an extension the certificate does not carry,
        // so its identifier is reported as the empty one rather than left out.
        if entry.extended == [NONE_SENTINEL] {
            let oid = detail["ExtendedKeyUsages"][0]["OID"].as_str();
            assert_eq!(
                oid,
                Some(""),
                "case {:?}: the sentinel names no extension value, so its OID is the \
                 empty identifier, got {oid:?}",
                entry.case
            );
        }
    }

    // Every certificate reported under a name is selected by the filter that names it —
    // in the listing and in the search alike, and for both members: one stored value,
    // one vocabulary.
    for (member, filter, names) in [
        ("keyUsage", "KeyUsage", vec![ANY_SENTINEL, "DIGITAL_SIGNATURE"]),
        (
            "extendedKeyUsage",
            "ExtendedKeyUsage",
            vec![
                NONE_SENTINEL,
                "TLS_WEB_SERVER_AUTHENTICATION",
                "TLS_WEB_CLIENT_AUTHENTICATION",
                "CODE_SIGNING",
                "ANY",
            ],
        ),
    ] {
        for name in names {
            let expected = expected_arns(&imported, member, name);
            assert!(
                !expected.is_empty(),
                "case {member}={name:?}: this table must carry a certificate for every \
                 name it filters on"
            );

            let listed = listed_for(&router, member, &[name]).await;
            assert_eq!(
                listed, expected,
                "case {member}={name:?}: expected ListCertificates to report \
                 {expected:?}, got {listed:?}"
            );
            let searched =
                searched_arns(&router, &json!({ "X509AttributeFilter": { filter: name } })).await;
            assert_eq!(
                searched, expected,
                "case {member}={name:?}: expected SearchCertificates to report \
                 {expected:?}, got {searched:?}"
            );
        }
    }

    // Each filter takes one or more values, so naming a sentinel beside a value selects
    // the union: the certificates the extension is absent on AND the one carrying that
    // value.
    for (member, name) in [
        ("keyUsage", "DIGITAL_SIGNATURE"),
        ("extendedKeyUsage", "CODE_SIGNING"),
    ] {
        let sentinel = if member == "keyUsage" {
            ANY_SENTINEL
        } else {
            NONE_SENTINEL
        };
        let mut union = expected_arns(&imported, member, sentinel);
        union.extend(expected_arns(&imported, member, name));
        union.sort();
        let listed = listed_for(&router, member, &[sentinel, name]).await;
        assert_eq!(
            listed, union,
            "case {member}=[{sentinel:?}, {name:?}]: expected {union:?}, got {listed:?}"
        );
    }

    // And the summary of a certificate a sentinel describes names it too, so the
    // listing's own projection agrees with the detail's.
    let bare = imported
        .iter()
        .find(|entry| entry.extended == [NONE_SENTINEL])
        .expect("a certificate with no extension");
    let page = call(
        &router,
        "ListCertificates",
        &listing("extendedKeyUsage", &[NONE_SENTINEL]),
    )
    .await
    .1;
    let summary = page["CertificateSummaryList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {:?}: no CertificateSummaryList in {page}", bare.case))
        .iter()
        .find(|summary| summary["CertificateArn"].as_str() == Some(bare.arn.as_str()))
        .unwrap_or_else(|| panic!("case {:?}: {} is not listed in {page}", bare.case, bare.arn))
        .clone();
    for (member, expected) in [
        ("KeyUsages", json!([ANY_SENTINEL])),
        ("ExtendedKeyUsages", json!([NONE_SENTINEL])),
    ] {
        assert_eq!(
            summary[member], expected,
            "case {:?}: expected the summary's {member} to name the sentinel ({expected}), \
             got {summary}",
            bare.case
        );
    }
}
