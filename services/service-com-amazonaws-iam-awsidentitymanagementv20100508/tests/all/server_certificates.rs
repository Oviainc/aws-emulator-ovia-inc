// SPDX-License-Identifier: CC0-1.0

//! `UploadServerCertificate` and its reporting half `GetServerCertificate`, end to end
//! over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of a certificate that was
//! PERSISTED, never a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.
//!
//! The rules under test are that an upload PERSISTS the certificate under the name it was
//! given, with an ARN DERIVED from that name and its path, an id minted once, and the
//! expiration READ OUT of the certificate body itself; that the read REPORTS exactly what
//! was persisted — the metadata, the PEM material byte for byte, and the tags sorted by
//! key — and never the private key; and that material which is not a certificate, a key
//! that is not the certificate's, and a second upload for a name the account already
//! holds are all refused with nothing persisted.
//!
//! Each suite is a TABLE whose rows differ along what those rules generalise over (the
//! path's depth, the name's case and alphabet, the key algorithm, the validity window,
//! whether a chain is carried, the tag count and the order the tags arrive in), so no
//! single request is the specification. Every certificate the service is fed is built by
//! `rcgen` — something other than the code under test — so an `Expiration` that is not
//! computed from the uploaded body cannot pass.

use std::collections::{BTreeMap, BTreeSet};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use x509_cert::der::DecodePem;

use crate::role_lifecycle::{borrowed, optional, param, reported_tags};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// One generated key pair with the self-signed certificate it belongs to.
pub(crate) struct Material {
    /// The PEM handed to `UploadServerCertificate` as `CertificateBody`.
    pub(crate) certificate: String,
    /// The PEM handed to it as `PrivateKey`.
    pub(crate) private_key: String,
    /// The certificate's own `notAfter` (RFC 5280 section 4.1.2.5), in whole epoch
    /// seconds — read out of the generated DER, not out of the service.
    not_after: i64,
}

/// Generate a real key pair and a self-signed certificate for it, valid from 1 January
/// `validity.0` to 1 January `validity.1`.
pub(crate) fn material(
    algorithm: &'static rcgen::SignatureAlgorithm,
    validity: (i32, i32),
    common_name: &str,
) -> Material {
    let key = rcgen::KeyPair::generate_for(algorithm).expect("a key pair");
    certificate_for(&key, validity, common_name)
}

/// Generate a real self-signed certificate for `key`, and report it beside THAT key's
/// PEM.
fn certificate_for(key: &rcgen::KeyPair, validity: (i32, i32), common_name: &str) -> Material {
    let mut params = rcgen::CertificateParams::new(vec![common_name.to_string()])
        .expect("certificate parameters");
    let mut subject = rcgen::DistinguishedName::new();
    subject.push(rcgen::DnType::CommonName, common_name);
    params.distinguished_name = subject;
    params.not_before = rcgen::date_time_ymd(validity.0, 1, 1);
    params.not_after = rcgen::date_time_ymd(validity.1, 1, 1);
    let certificate = params
        .self_signed(key)
        .expect("a self-signed certificate")
        .pem();
    let parsed = x509_cert::Certificate::from_pem(&certificate).expect("the certificate parses");
    Material {
        not_after: parsed
            .tbs_certificate
            .validity
            .not_after
            .to_unix_duration()
            .as_secs() as i64,
        certificate,
        private_key: key.serialize_pem(),
    }
}

/// One `UploadServerCertificate` request, with the members the row exercises.
struct UploadCase {
    /// `ServerCertificateName` — "do not include the path in this value"
    /// (`UploadServerCertificate$ServerCertificateName`), spelled across the
    /// `^[\w+=,.@-]+$` alphabet the member declares.
    name: &'static str,
    /// `Path`, or `None` for a request that omits it: "this parameter is optional. If it
    /// is not included, it defaults to a slash (/)" (`$Path`).
    path: Option<&'static str>,
    /// The signature algorithm the generated key pair uses.
    algorithm: &'static rcgen::SignatureAlgorithm,
    /// The generated certificate's validity window, as `(not_before, not_after)` years.
    validity: (i32, i32),
    /// Whether the request carries a `CertificateChain`.
    chain: bool,
    /// The tags the request carries, in request order.
    tags: Vec<(&'static str, &'static str)>,
}

/// The table every suite here is written against: the default path, a one-segment path, a
/// nested path, the `/cloudfront/` path `$Path`'s note calls for and the root given
/// explicitly; names spanning the member's alphabet and both cases; three key algorithms;
/// validity windows of one year, ten and twenty, every one of them opening and closing far
/// enough ahead that no row depends on the wall clock; a chain carried and omitted; tag
/// counts from none to three, arriving in an order that is not the sorted one.
fn upload_cases() -> Vec<UploadCase> {
    vec![
        UploadCase {
            name: "web-server",
            path: None,
            algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
            validity: (2020, 2030),
            chain: false,
            tags: Vec::new(),
        },
        UploadCase {
            name: "CloudFront_Cert.1",
            path: Some("/cloudfront/test/"),
            algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
            validity: (2021, 2031),
            chain: true,
            tags: vec![("env", "prod"), ("alpha", "1")],
        },
        UploadCase {
            name: "edge+east=1",
            path: Some("/edge/east/"),
            algorithm: &rcgen::PKCS_ECDSA_P256_SHA256,
            validity: (2024, 2044),
            chain: false,
            tags: vec![("zeta", "last"), ("alpha", "first"), ("mu", "middle")],
        },
        UploadCase {
            name: "LEGACY-cert_9",
            path: Some("/legacy/"),
            algorithm: &rcgen::PKCS_ED25519,
            validity: (2029, 2039),
            chain: true,
            tags: vec![("owner", "platform")],
        },
        UploadCase {
            name: "svc.team@2",
            path: Some("/"),
            algorithm: &rcgen::PKCS_ECDSA_P384_SHA384,
            validity: (2040, 2041),
            chain: false,
            tags: Vec::new(),
        },
    ]
}

impl UploadCase {
    /// The path this request's certificate ends up under: the one it named, or the slash
    /// an omitted `Path` defaults to.
    fn expected_path(&self) -> &'static str {
        self.path.unwrap_or("/")
    }

    /// The material this row uploads: its certificate and key, and the chain it carries.
    fn material(&self) -> (Material, Option<String>) {
        let leaf = material(self.algorithm, self.validity, "server.example.com");
        let chain = self.chain.then(|| {
            material(self.algorithm, self.validity, "issuer.example.com").certificate
        });
        (leaf, chain)
    }

    fn params(&self, leaf: &Material, chain: Option<&String>) -> Vec<(String, String)> {
        let mut params = vec![
            param("ServerCertificateName", self.name),
            param("CertificateBody", &leaf.certificate),
            param("PrivateKey", &leaf.private_key),
        ];
        if let Some(path) = self.path {
            params.push(param("Path", path));
        }
        if let Some(chain) = chain {
            params.push(param("CertificateChain", chain));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("Tags.member.{position}.Key"), key));
            params.push(param(&format!("Tags.member.{position}.Value"), value));
        }
        params
    }

    fn expected_tags(&self) -> BTreeMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }
}

/// A server certificate ARN is `arn:aws:iam::<account>:server-certificate<path><name>` —
/// IAM is global, so the region segment is EMPTY, and the path already begins and ends
/// with a slash.
///
/// `pub(crate)` because every suite that drives a server-certificate call checks the ARN
/// the service spells against the same grammar.
pub(crate) fn expected_arn(path: &str, name: &str) -> String {
    format!(
        "arn:aws:iam::{}:server-certificate{path}{name}",
        account_id()
    )
}

/// The instant a reported `dateType` member names, in whole epoch seconds.
fn reported_seconds(text: &str) -> i64 {
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| panic!("{text:?} is not a wire date-time: {error}"))
        .secs()
}

/// `idType` (which `ServerCertificateMetadata$ServerCertificateId` targets) is len 16..128
/// with pattern `^[\w]+$`, and the "IAM identifiers" page of the IAM User Guide lists
/// `ASCA` as the unique ID prefix of a certificate.
fn assert_certificate_id_shape(case: &str, value: &str) {
    assert!(
        (16..=128).contains(&value.len())
            && value
                .chars()
                .all(|character| character.is_alphanumeric() || character == '_'),
        "case {case:?}: ServerCertificateId must be 16..128 word characters, got {value:?}"
    );
    assert!(
        value.starts_with("ASCA"),
        "case {case:?}: ServerCertificateId must carry a certificate's ASCA prefix, got \
         {value:?}"
    );
}

// ---------------------------------------------------------------------------
// Upload, then read back
// ---------------------------------------------------------------------------

/// Every row uploads a certificate that `GetServerCertificate` then reports identically:
/// the name and path the request chose, an ARN derived from both, the id the upload
/// minted, the instant it was uploaded, the expiration read out of the certificate, the
/// PEM material byte for byte, and the tags sorted by key. No response carries the
/// private key.
#[tokio::test]
async fn an_uploaded_server_certificate_round_trips_through_get_server_certificate() {
    let router = router();
    let mut minted_ids: BTreeSet<String> = BTreeSet::new();

    for case in upload_cases() {
        let (leaf, chain) = case.material();
        let params = case.params(&leaf, chain.as_ref());
        let (status, uploaded) =
            call(&router, "UploadServerCertificate", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {uploaded}",
            case.name
        );

        let path = case.expected_path();
        assert_eq!(
            element(&uploaded, "Path"),
            path,
            "case {:?}: the upload must report the path it was given, got {uploaded}",
            case.name
        );
        assert_eq!(
            element(&uploaded, "ServerCertificateName"),
            case.name,
            "case {:?}: the upload must report the name it was given, got {uploaded}",
            case.name
        );
        let arn = element(&uploaded, "Arn");
        assert_eq!(
            arn,
            expected_arn(path, case.name),
            "case {:?}: the ARN must be derived from the path and the name, got {arn:?}",
            case.name
        );
        // `arnType` is modelled len 20..2048 — the constraint on the value this mints.
        assert!(
            arn.len() >= 20 && arn.len() <= 2048,
            "case {:?}: the ARN must be 20..2048 characters, got {} in {arn:?}",
            case.name,
            arn.len()
        );
        let id = element(&uploaded, "ServerCertificateId");
        assert_certificate_id_shape(case.name, &id);
        assert!(
            minted_ids.insert(id.clone()),
            "case {:?}: ServerCertificateId {id:?} was already minted for another \
             certificate",
            case.name
        );
        // "The date on which the certificate is set to expire"
        // (`ServerCertificateMetadata$Expiration`) is the uploaded certificate's own
        // `notAfter`, which `rcgen` chose and this suite read out of the DER.
        let expiration = reported_seconds(&element(&uploaded, "Expiration"));
        assert_eq!(
            expiration, leaf.not_after,
            "case {:?}: the Expiration must be the certificate's own notAfter ({}), got \
             {expiration}",
            case.name, leaf.not_after
        );
        let upload_date = reported_seconds(&element(&uploaded, "UploadDate"));
        assert!(
            upload_date > 0,
            "case {:?}: the upload must report when it happened, got {upload_date}",
            case.name
        );
        assert_eq!(
            reported_tags(&uploaded),
            case.expected_tags(),
            "case {:?}: the upload must report the tags it was given, got {uploaded}",
            case.name
        );
        assert!(
            optional(&uploaded, "PrivateKey").is_none(),
            "case {:?}: no response may carry the private key, got {uploaded}",
            case.name
        );

        // The read reports the SAME record, resolved by the name the upload gave it.
        let read_params = vec![param("ServerCertificateName", case.name)];
        let (status, read) = call(&router, "GetServerCertificate", &borrowed(&read_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from GetServerCertificate, got {status} with body \
             {read}",
            case.name
        );
        for (member, expected) in [
            ("Path", path.to_string()),
            ("ServerCertificateName", case.name.to_string()),
            ("Arn", arn.clone()),
            ("ServerCertificateId", id.clone()),
        ] {
            assert_eq!(
                element(&read, member),
                expected,
                "case {:?}: member {member}: the read must report what was persisted, got \
                 {read}",
                case.name
            );
        }
        assert_eq!(
            reported_seconds(&element(&read, "Expiration")),
            expiration,
            "case {:?}: the read must report the stored expiration, got {read}",
            case.name
        );
        assert_eq!(
            reported_seconds(&element(&read, "UploadDate")),
            upload_date,
            "case {:?}: the read must report the stored upload date, got {read}",
            case.name
        );
        assert_eq!(
            element(&read, "CertificateBody"),
            leaf.certificate,
            "case {:?}: the read must report the certificate body byte for byte, got {read}",
            case.name
        );
        assert_eq!(
            optional(&read, "CertificateChain"),
            chain,
            "case {:?}: the read must report the chain the upload carried, got {read}",
            case.name
        );
        assert_eq!(
            reported_tags(&read),
            case.expected_tags(),
            "case {:?}: the read must report the stored tags, got {read}",
            case.name
        );
        assert!(
            optional(&read, "PrivateKey").is_none(),
            "case {:?}: the read must not report the private key, got {read}",
            case.name
        );
    }
}

/// "The returned list of tags is sorted by tag key" (`UploadServerCertificate$Tags`): the
/// upload and the read both report the keys in ascending order, whatever order the
/// request carried them in.
#[tokio::test]
async fn reported_tags_are_sorted_by_key() {
    let router = router();

    for case in upload_cases() {
        let mut sorted_keys: Vec<String> = case
            .tags
            .iter()
            .map(|(key, _)| (*key).to_string())
            .collect();
        sorted_keys.sort();

        let (leaf, chain) = case.material();
        let params = case.params(&leaf, chain.as_ref());
        let (status, uploaded) =
            call(&router, "UploadServerCertificate", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture certificate must upload, got {status}: {uploaded}",
            case.name
        );
        assert_eq!(
            elements(&uploaded, "Key"),
            sorted_keys,
            "case {:?}: the upload must report its tags sorted by key, got {uploaded}",
            case.name
        );

        let read_params = vec![param("ServerCertificateName", case.name)];
        let (_, read) = call(&router, "GetServerCertificate", &borrowed(&read_params)).await;
        assert_eq!(
            elements(&read, "Key"),
            sorted_keys,
            "case {:?}: the read must report the stored tags sorted by key, got {read}",
            case.name
        );
    }
}

/// Reading is a READ: two `GetServerCertificate` calls report the same record, and the
/// second one is not affected by the first.
#[tokio::test]
async fn reading_a_server_certificate_changes_nothing_a_later_caller_sees() {
    let router = router();

    for case in upload_cases() {
        let (leaf, chain) = case.material();
        let params = case.params(&leaf, chain.as_ref());
        let (status, uploaded) =
            call(&router, "UploadServerCertificate", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture certificate must upload, got {status}: {uploaded}",
            case.name
        );

        let read_params = vec![param("ServerCertificateName", case.name)];
        let (_, first) = call(&router, "GetServerCertificate", &borrowed(&read_params)).await;
        let (_, second) = call(&router, "GetServerCertificate", &borrowed(&read_params)).await;
        for member in [
            "Path",
            "ServerCertificateName",
            "Arn",
            "ServerCertificateId",
            "UploadDate",
            "Expiration",
            "CertificateBody",
        ] {
            assert_eq!(
                element(&first, member),
                element(&second, member),
                "case {:?}: member {member}: two reads must report the same value",
                case.name
            );
        }
        assert_eq!(
            reported_tags(&first),
            reported_tags(&second),
            "case {:?}: two reads must report the same tags",
            case.name
        );
    }
}

// ---------------------------------------------------------------------------
// A name is uploaded at most once
// ---------------------------------------------------------------------------

/// IAM entity names are unique within the account and "are not distinguished by case", so
/// a second upload under a held name — however it is spelled — is `EntityAlreadyExists`
/// (HTTP 409), and the held certificate keeps the record the FIRST upload wrote.
#[tokio::test]
async fn a_second_upload_for_a_held_name_is_refused_and_changes_nothing() {
    let router = router();

    for case in upload_cases() {
        let (leaf, chain) = case.material();
        let params = case.params(&leaf, chain.as_ref());
        let (status, uploaded) =
            call(&router, "UploadServerCertificate", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture certificate must upload, got {status}: {uploaded}",
            case.name
        );
        let held_id = element(&uploaded, "ServerCertificateId");
        let held_arn = element(&uploaded, "Arn");

        // Rows: the same spelling, and the case-folded and upper-cased spellings of it.
        for spelling in [
            case.name.to_string(),
            case.name.to_ascii_lowercase(),
            case.name.to_ascii_uppercase(),
        ] {
            let second = material(case.algorithm, (2022, 2042), "second.example.com");
            let params = vec![
                param("ServerCertificateName", &spelling),
                param("CertificateBody", &second.certificate),
                param("PrivateKey", &second.private_key),
            ];
            let (status, body) =
                call(&router, "UploadServerCertificate", &borrowed(&params)).await;
            assert_eq!(
                status, 409,
                "case {spelling:?}: expected HTTP 409, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "EntityAlreadyExists",
                "case {spelling:?}: expected EntityAlreadyExists, got {body}"
            );
        }

        let read_params = vec![param("ServerCertificateName", case.name)];
        let (status, read) = call(&router, "GetServerCertificate", &borrowed(&read_params)).await;
        assert_eq!(status, 200, "case {:?}: the held certificate must survive a refused upload, got {status}: {read}", case.name);
        assert_eq!(
            element(&read, "ServerCertificateId"),
            held_id,
            "case {:?}: a refused upload must not re-mint the held id, got {read}",
            case.name
        );
        assert_eq!(
            element(&read, "Arn"),
            held_arn,
            "case {:?}: a refused upload must not change the held ARN, got {read}",
            case.name
        );
        assert_eq!(
            element(&read, "CertificateBody"),
            leaf.certificate,
            "case {:?}: a refused upload must not replace the held body, got {read}",
            case.name
        );
    }
}

// ---------------------------------------------------------------------------
// Material that is not a certificate, and a key that is not its own
// ---------------------------------------------------------------------------

/// One row of the refusal table: what the row exercises, the material members the
/// request carries, and the awsQuery error code the upload must answer with.
struct RefusedCase {
    case: &'static str,
    material: Vec<(String, String)>,
    expected: &'static str,
}

/// The material an upload carries has to be what its members say it is: a body that is
/// not one PEM-encoded X.509 certificate, or a chain that is not PEM-encoded
/// certificates, is `MalformedCertificate` (HTTP 400); a key that is not the
/// certificate's is `KeyPairMismatch` (HTTP 400). Either way nothing is persisted.
#[tokio::test]
async fn material_that_is_not_the_certificate_and_its_key_is_refused() {
    let router = router();

    let valid = material(&rcgen::PKCS_ECDSA_P256_SHA256, (2020, 2040), "valid.example.com");
    let other_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .expect("a second key pair of the same algorithm");
    let other_algorithm =
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).expect("a key pair of another algorithm");
    // An Ed25519 certificate with a FOREIGN Ed25519 key: the pair is matched on the public
    // key the PKCS#8 document states (RFC 5958 section 2), not on the algorithm alone.
    let edwards = material(&rcgen::PKCS_ED25519, (2030, 2050), "edwards.example.com");
    let other_edwards =
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).expect("a second Ed25519 key pair");

    // Rows: a body that is not PEM at all, PEM framing around something that is not DER,
    // a PEM private key where a certificate belongs, a key that is not PEM, a key of
    // another key pair of the same algorithm (for an EC and for an Edwards certificate), a
    // key of another algorithm, and a chain that is not PEM.
    let cases: Vec<RefusedCase> = vec![
        RefusedCase {
            case: "a CertificateBody that is not PEM",
            material: vec![
                param("CertificateBody", "not a certificate at all"),
                param("PrivateKey", &valid.private_key),
            ],
            expected: "MalformedCertificate",
        },
        RefusedCase {
            case: "a CertificateBody whose PEM frames no certificate",
            material: vec![
                param(
                    "CertificateBody",
                    "-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----\n",
                ),
                param("PrivateKey", &valid.private_key),
            ],
            expected: "MalformedCertificate",
        },
        RefusedCase {
            case: "a CertificateBody carrying the private key instead",
            material: vec![
                param("CertificateBody", &valid.private_key),
                param("PrivateKey", &valid.private_key),
            ],
            expected: "MalformedCertificate",
        },
        RefusedCase {
            case: "a PrivateKey that is not PEM",
            material: vec![
                param("CertificateBody", &valid.certificate),
                param("PrivateKey", "not a key"),
            ],
            expected: "MalformedCertificate",
        },
        RefusedCase {
            case: "a PrivateKey of another key pair of the same algorithm",
            material: vec![
                param("CertificateBody", &valid.certificate),
                param("PrivateKey", &other_pair.serialize_pem()),
            ],
            expected: "KeyPairMismatch",
        },
        RefusedCase {
            case: "a PrivateKey of another algorithm",
            material: vec![
                param("CertificateBody", &valid.certificate),
                param("PrivateKey", &other_algorithm.serialize_pem()),
            ],
            expected: "KeyPairMismatch",
        },
        RefusedCase {
            case: "a PrivateKey of another key pair of the same Edwards algorithm",
            material: vec![
                param("CertificateBody", &edwards.certificate),
                param("PrivateKey", &other_edwards.serialize_pem()),
            ],
            expected: "KeyPairMismatch",
        },
        RefusedCase {
            case: "a CertificateChain that is not PEM",
            material: vec![
                param("CertificateBody", &valid.certificate),
                param("PrivateKey", &valid.private_key),
                param("CertificateChain", "not a chain"),
            ],
            expected: "MalformedCertificate",
        },
    ];

    for (index, row) in cases.into_iter().enumerate() {
        let RefusedCase {
            case,
            material,
            expected,
        } = row;
        let name = format!("refused-{index}");
        let mut params = vec![param("ServerCertificateName", &name)];
        params.extend(material);
        let (status, body) = call(&router, "UploadServerCertificate", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );

        let read_params = vec![param("ServerCertificateName", &name)];
        let (status, read) = call(&router, "GetServerCertificate", &borrowed(&read_params)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused upload must persist nothing, got {status}: {read}"
        );
    }
}

/// A name the account holds no certificate for is `NoSuchEntity` (HTTP 404) — never an
/// empty success.
#[tokio::test]
async fn reading_what_was_never_uploaded_is_refused() {
    let router = router();
    let held = material(&rcgen::PKCS_ECDSA_P256_SHA256, (2020, 2040), "held.example.com");
    let params = vec![
        param("ServerCertificateName", "held"),
        param("CertificateBody", &held.certificate),
        param("PrivateKey", &held.private_key),
    ];
    let (status, body) = call(&router, "UploadServerCertificate", &borrowed(&params)).await;
    assert_eq!(status, 200, "the fixture certificate must upload, got {body}");

    // Rows: a name nothing holds, a near-miss of the held one, and the held name with a
    // path-like suffix — which is not part of the name a certificate is reached by.
    for name in ["never", "held-2", "held.1"] {
        let params = vec![param("ServerCertificateName", name)];
        let (status, body) = call(&router, "GetServerCertificate", &borrowed(&params)).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected HTTP 404, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {body}"
        );
        assert!(
            optional(&body, "CertificateBody").is_none(),
            "case {name:?}: a refused read reports no certificate, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Tags AWS reserves, and the account's certificate quota
// ---------------------------------------------------------------------------

/// AWS reserves the `aws:` tag-key prefix ("Tagging IAM resources", IAM User Guide), and
/// "if any one of the tags is invalid … the entire request fails and the resource is not
/// created" (`UploadServerCertificate$Tags`): the upload is `InvalidInput` (HTTP 400) and
/// nothing is persisted, wherever in the request the reserved key sits.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_uploads_nothing() {
    let router = router();
    let valid = material(
        &rcgen::PKCS_ECDSA_P256_SHA256,
        (2030, 2050),
        "tagged.example.com",
    );

    // Rows: the reserved key alone, and beside a valid one, in either order.
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("reserved-only", vec![("aws:cloudformation:stack", "s")]),
        (
            "reserved-after-valid",
            vec![("env", "dev"), ("aws:created-by", "me")],
        ),
        (
            "reserved-before-valid",
            vec![("aws:created-by", "me"), ("env", "dev")],
        ),
    ];
    for (case, tags) in cases {
        let name = format!("tag-{case}");
        let mut params = vec![
            param("ServerCertificateName", &name),
            param("CertificateBody", &valid.certificate),
            param("PrivateKey", &valid.private_key),
        ];
        for (index, (key, value)) in tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("Tags.member.{position}.Key"), key));
            params.push(param(&format!("Tags.member.{position}.Value"), value));
        }
        let (status, body) = call(&router, "UploadServerCertificate", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected InvalidInput, got {body}"
        );

        let read_params = vec![param("ServerCertificateName", &name)];
        let (status, read) = call(&router, "GetServerCertificate", &borrowed(&read_params)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused upload must persist nothing, got {status}: {read}"
        );
    }
}

/// An account holds a bounded number of server certificates — AWS's "IAM and STS quotas"
/// page gives "Server certificates stored in IAM per account: 20" — so the upload past it
/// is `LimitExceeded` (HTTP 409), persists nothing, and leaves every held certificate
/// readable.
#[tokio::test]
async fn an_upload_past_the_account_quota_is_refused_and_changes_nothing() {
    let router = router();
    // The quota `state::MAX_SERVER_CERTIFICATES_PER_ACCOUNT` records.
    const QUOTA: usize = 20;

    for index in 0..QUOTA {
        let held = material(
            &rcgen::PKCS_ECDSA_P256_SHA256,
            (2030, 2050),
            "quota.example.com",
        );
        let name = format!("quota-{index}");
        let params = vec![
            param("ServerCertificateName", &name),
            param("CertificateBody", &held.certificate),
            param("PrivateKey", &held.private_key),
        ];
        let (status, body) = call(&router, "UploadServerCertificate", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: an upload within the quota must succeed, got {status}: {body}"
        );
    }

    let extra = material(
        &rcgen::PKCS_ECDSA_P256_SHA256,
        (2030, 2050),
        "extra.example.com",
    );
    let name = format!("quota-{QUOTA}");
    let params = vec![
        param("ServerCertificateName", &name),
        param("CertificateBody", &extra.certificate),
        param("PrivateKey", &extra.private_key),
    ];
    let (status, body) = call(&router, "UploadServerCertificate", &borrowed(&params)).await;
    assert_eq!(
        status, 409,
        "case {name:?}: expected HTTP 409 past the quota, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "LimitExceeded",
        "case {name:?}: expected LimitExceeded, got {body}"
    );

    let read_params = vec![param("ServerCertificateName", &name)];
    let (status, read) = call(&router, "GetServerCertificate", &borrowed(&read_params)).await;
    assert_eq!(
        status, 404,
        "case {name:?}: a refused upload must persist nothing, got {status}: {read}"
    );
    for index in [0, QUOTA - 1] {
        let name = format!("quota-{index}");
        let read_params = vec![param("ServerCertificateName", &name)];
        let (status, read) = call(&router, "GetServerCertificate", &borrowed(&read_params)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a refused upload must leave the held certificates readable, got \
             {status}: {read}"
        );
    }
}
