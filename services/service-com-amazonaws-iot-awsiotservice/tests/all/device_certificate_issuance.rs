// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of AWS IoT device-certificate issuance:
//! `CreateKeysAndCertificate` and the `DescribeCertificate` reader that proves what it
//! persisted.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what an
//! AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: an operation documented to GENERATE key material computes it
//! with real cryptography and records the half that outlives the call, so the material
//! it answers is independently verifiable and the record a later reader reports is the
//! one the create wrote. Here that means `CreateKeysAndCertificate` issues a 2048-bit
//! RSA key pair and an X.509 certificate over its public key — the private key matching
//! the certificate's `SubjectPublicKeyInfo`, the certificate carrying a real
//! `sha256WithRSAEncryption` signature that verifies under that key, and the
//! `certificateId` being the SHA-256 digest of the certificate's own DER — records the
//! certificate (and never the private key) under that id, and `DescribeCertificate`
//! reports it as a `CertificateDescription`: the same PEM, the ARN
//! (`arn:aws:iot:<region>:<account>:cert/<certificateId>`), the owning account, the
//! status `setAsActive` resolved to, `DEFAULT` mode, customer version 1, and the
//! validity the certificate itself carries. Two calls issue two different certificates;
//! an id the store does not hold is refused `ResourceNotFoundException` (404).
//!
//! The table spans what that rule generalises over: whether the request carries
//! `setAsActive` at all and which value it carries (the member is modelled `default
//! false`, `smithy_query(query="operation",operation="CreateKeysAndCertificate")`), and
//! the region the call is signed for, which is the region the certificate's ARN is
//! scoped to.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use service_com_amazonaws_iot_awsiotservice::state::{
    CERTIFICATE_MODE_DEFAULT, CERTIFICATE_STATUS_ACTIVE, CERTIFICATE_STATUS_INACTIVE,
    FIRST_CERTIFICATE_CUSTOMER_VERSION,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;
use x509_cert::der::{Decode, DecodePem, Encode};
use x509_cert::spki::SubjectPublicKeyInfoOwned;
use x509_cert::Certificate;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// The modulus length, in bytes, of the "2048-bit RSA key pair"
/// `CreateKeysAndCertificate` documents.
const RSA_2048_MODULUS_BYTES: usize = 256;

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and `region`: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to. The signature itself is never
/// checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `certificateDescription` member of a `DescribeCertificate` response.
    fn description(&self) -> Option<&Value> {
        self.body.get("certificateDescription")
    }

    /// A member of the reported `CertificateDescription`.
    fn member(&self, name: &str) -> Option<&Value> {
        self.description().and_then(|value| value.get(name))
    }

    /// A string member of the reported `CertificateDescription`.
    fn member_str(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    /// A timestamp member as the epoch seconds the restJson1 protocol renders a
    /// timestamp as.
    fn member_epoch(&self, name: &str) -> Option<f64> {
        self.member(name).and_then(Value::as_f64)
    }

    /// The `keyPair` member of a `CreateKeysAndCertificate` response.
    fn key_pair(&self, name: &str) -> Option<&str> {
        self.body
            .get("keyPair")
            .and_then(|pair| pair.get(name))
            .and_then(Value::as_str)
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, region: &str) -> Reply {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
            .header("content-type", "application/json")
            .body(bytes::Bytes::new())
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// `CreateKeysAndCertificate`: `POST /keys-and-certificate`, with `setAsActive`
    /// bound as an `@httpQuery` parameter rather than a body member.
    async fn create(&self, query: Option<bool>, region: &str) -> Reply {
        let path = match query {
            None => "/keys-and-certificate".to_string(),
            Some(value) => format!("/keys-and-certificate?setAsActive={value}"),
        };
        self.call("POST", &path, region).await
    }

    async fn describe(&self, certificate_id: &str, region: &str) -> Reply {
        self.call("GET", &format!("/certificates/{certificate_id}"), region)
            .await
    }
}

/// The ARN a certificate issued in `region` must carry.
fn expected_arn(region: &str, certificate_id: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:cert/{certificate_id}")
}

/// Epoch seconds now, for bounding a reported timestamp.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// One issue request: a label, the `setAsActive` query parameter it carries (or none),
/// the region it is signed for, and the certificate status that resolves to.
struct Case {
    label: &'static str,
    set_as_active: Option<bool>,
    region: &'static str,
    status: &'static str,
}

/// The table every case list below is built from. It spans what the rule generalises
/// over: the three forms `setAsActive` can take on the wire — absent (the modelled
/// `default false`), explicitly `false`, explicitly `true` — and the region the call is
/// signed for.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no setAsActive at all",
            set_as_active: None,
            region: REGION,
            status: CERTIFICATE_STATUS_INACTIVE,
        },
        Case {
            label: "setAsActive=false",
            set_as_active: Some(false),
            region: REGION,
            status: CERTIFICATE_STATUS_INACTIVE,
        },
        Case {
            label: "setAsActive=true",
            set_as_active: Some(true),
            region: REGION,
            status: CERTIFICATE_STATUS_ACTIVE,
        },
        Case {
            label: "setAsActive=true in another region",
            set_as_active: Some(true),
            region: "eu-west-1",
            status: CERTIFICATE_STATUS_ACTIVE,
        },
        Case {
            label: "no setAsActive in another region",
            set_as_active: None,
            region: "ap-southeast-2",
            status: CERTIFICATE_STATUS_INACTIVE,
        },
    ]
}

/// The certificate `pem` parses as, and the key material it names.
struct ParsedCertificate {
    certificate: Certificate,
    /// The DER of the whole certificate, which its id is the digest of.
    der: Vec<u8>,
    /// The PKCS#1 `RSAPublicKey` DER inside the certificate's
    /// `SubjectPublicKeyInfo` — the form an RSA verification key is given in.
    public_key: Vec<u8>,
}

/// Parse `pem` as an X.509 certificate, independently of the encoder the service used.
fn parse_certificate(case: &str, pem: &str) -> ParsedCertificate {
    let certificate = Certificate::from_pem(pem)
        .unwrap_or_else(|why| panic!("case {case:?}: the certificatePem must parse: {why} ({pem})"));
    let der = certificate
        .to_der()
        .unwrap_or_else(|why| panic!("case {case:?}: the parsed certificate re-encodes: {why}"));
    let public_key = certificate
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .raw_bytes()
        .to_vec();
    ParsedCertificate {
        certificate,
        der,
        public_key,
    }
}

#[tokio::test]
async fn create_keys_and_certificate_issues_a_real_rsa_2048_key_pair_and_certificate() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        let issued = emulator.create(case.set_as_active, case.region).await;
        assert_eq!(
            issued.status, 200,
            "case {label:?}: CreateKeysAndCertificate must succeed, got {issued:?}"
        );

        let certificate_id = issued.text("certificateId").unwrap_or_else(|| {
            panic!("case {label:?}: the response must carry a certificateId, got {issued:?}")
        });
        let certificate_pem = issued.text("certificatePem").unwrap_or_else(|| {
            panic!("case {label:?}: the response must carry a certificatePem, got {issued:?}")
        });

        // The id is the SHA-256 digest of the certificate's own DER, hex encoded, so it
        // satisfies the modelled `length 64..64, pattern ^(0x)?[a-fA-F0-9]+$` and names
        // the certificate that was actually issued.
        let parsed = parse_certificate(label, &certificate_pem);
        let digest = hex::encode(Sha256::digest(&parsed.der));
        assert_eq!(
            certificate_id, digest,
            "case {label:?}: expected the certificateId to be the certificate's SHA-256 digest \
             {digest}, got {certificate_id}"
        );
        assert_eq!(
            certificate_id.len(),
            64,
            "case {label:?}: expected a 64-character certificateId, got {certificate_id}"
        );
        assert!(
            certificate_id
                .chars()
                .all(|character| character.is_ascii_hexdigit()),
            "case {label:?}: expected a hex certificateId, got {certificate_id}"
        );

        assert_eq!(
            issued.text("certificateArn").as_deref(),
            Some(expected_arn(case.region, &certificate_id).as_str()),
            "case {label:?}: expected the certificate ARN for region {}, got {issued:?}",
            case.region
        );

        // The certificate is signed `sha256WithRSAEncryption` by the key it names, so
        // its own public key verifies the signature over its `TBSCertificate`: the
        // signature is real RSA, not bytes shaped like one.
        let tbs = parsed
            .certificate
            .tbs_certificate
            .to_der()
            .unwrap_or_else(|why| panic!("case {label:?}: the TBSCertificate re-encodes: {why}"));
        let signature = parsed
            .certificate
            .signature
            .as_bytes()
            .unwrap_or_else(|| panic!("case {label:?}: the signature is whole bytes"));
        let verifier = aws_lc_rs::signature::UnparsedPublicKey::new(
            &aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256,
            &parsed.public_key,
        );
        verifier.verify(&tbs, signature).unwrap_or_else(|why| {
            panic!("case {label:?}: the certificate's own signature must verify: {why}")
        });

        // The key pair: the public half is the certificate's `SubjectPublicKeyInfo`, and
        // the private half is the private key of that same modulus — a 2048-bit one.
        let public_key_pem = issued.key_pair("PublicKey").unwrap_or_else(|| {
            panic!("case {label:?}: the response must carry keyPair.PublicKey, got {issued:?}")
        });
        let announced = SubjectPublicKeyInfoOwned::from_pem(public_key_pem)
            .unwrap_or_else(|why| panic!("case {label:?}: keyPair.PublicKey must parse: {why}"));
        assert_eq!(
            announced.subject_public_key.raw_bytes(),
            parsed.public_key.as_slice(),
            "case {label:?}: keyPair.PublicKey must be the public key the certificate names, \
             got {public_key_pem}"
        );

        let private_key_pem = issued.key_pair("PrivateKey").unwrap_or_else(|| {
            panic!("case {label:?}: the response must carry keyPair.PrivateKey, got {issued:?}")
        });
        // `KeyPair$PrivateKey` is a PEM PKCS#1 RSA private key, so the block's own
        // label says what it holds and the body decodes as that structure.
        let (pem_label, private_der) =
            x509_cert::der::pem::decode_vec(private_key_pem.as_bytes()).unwrap_or_else(|why| {
                panic!("case {label:?}: keyPair.PrivateKey must be a PEM block: {why}")
            });
        assert_eq!(
            pem_label, "RSA PRIVATE KEY",
            "case {label:?}: expected an RSA PRIVATE KEY block, got {pem_label}"
        );
        let private_key = pkcs1::RsaPrivateKey::from_der(&private_der)
            .unwrap_or_else(|why| panic!("case {label:?}: keyPair.PrivateKey must parse: {why}"));
        assert_eq!(
            private_key.modulus.as_bytes().len(),
            RSA_2048_MODULUS_BYTES,
            "case {label:?}: expected a {}-byte (2048-bit) modulus, got {}",
            RSA_2048_MODULUS_BYTES,
            private_key.modulus.as_bytes().len()
        );
        let certificate_key = pkcs1::RsaPublicKey::from_der(&parsed.public_key)
            .unwrap_or_else(|why| panic!("case {label:?}: the certificate's RSA key parses: {why}"));
        assert_eq!(
            private_key.modulus.as_bytes(),
            certificate_key.modulus.as_bytes(),
            "case {label:?}: keyPair.PrivateKey must be the private half of the certificate's key"
        );
    }
}

#[tokio::test]
async fn describe_certificate_reports_the_issued_certificate() {
    let emulator = Emulator::new();

    for case in cases() {
        let label = case.label;
        let before = now_epoch_seconds();
        let issued = emulator.create(case.set_as_active, case.region).await;
        assert_eq!(
            issued.status, 200,
            "case {label:?}: CreateKeysAndCertificate must succeed, got {issued:?}"
        );
        let certificate_id = issued.text("certificateId").unwrap_or_else(|| {
            panic!("case {label:?}: the response must carry a certificateId, got {issued:?}")
        });

        let read = emulator.describe(&certificate_id, case.region).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: DescribeCertificate of the issued id must succeed, got {read:?}"
        );
        assert_eq!(
            read.member_str("certificateId"),
            Some(certificate_id.as_str()),
            "case {label:?}: the description must report the issued id, got {read:?}"
        );
        assert_eq!(
            read.member_str("certificateArn"),
            Some(expected_arn(case.region, &certificate_id).as_str()),
            "case {label:?}: the description must report the issued ARN, got {read:?}"
        );
        assert_eq!(
            read.member_str("certificatePem"),
            issued.text("certificatePem").as_deref(),
            "case {label:?}: the description must report the issued certificate, got {read:?}"
        );
        assert_eq!(
            read.member_str("status"),
            Some(case.status),
            "case {label:?}: expected status {:?}, got {read:?}",
            case.status
        );
        assert_eq!(
            read.member_str("ownedBy"),
            Some(ACCOUNT_ID),
            "case {label:?}: the description must report the owning account, got {read:?}"
        );
        assert_eq!(
            read.member_str("certificateMode"),
            Some(CERTIFICATE_MODE_DEFAULT),
            "case {label:?}: a service-generated certificate is in DEFAULT mode, got {read:?}"
        );
        assert_eq!(
            read.member("customerVersion").and_then(Value::as_i64),
            Some(i64::from(FIRST_CERTIFICATE_CUSTOMER_VERSION)),
            "case {label:?}: expected customer version {FIRST_CERTIFICATE_CUSTOMER_VERSION}, \
             got {read:?}"
        );
        assert!(
            read.member_str("generationId")
                .is_some_and(|id| !id.is_empty()),
            "case {label:?}: the description must report a generationId, got {read:?}"
        );
        // Nothing has transferred the certificate to another account and no registered
        // CA signed it, so the members that describe those carry nothing.
        for absent in ["caCertificateId", "previousOwnedBy", "transferData"] {
            assert!(
                read.member(absent).is_none(),
                "case {label:?}: {absent} must be absent on a freshly issued certificate, \
                 got {read:?}"
            );
        }

        // `creationDate` is the instant of the issue: it lies between the instant before
        // the create and the instant after this read. Nothing has updated the
        // certificate, so `lastModifiedDate` is that same instant.
        let created_at = read.member_epoch("creationDate").unwrap_or_else(|| {
            panic!("case {label:?}: the description reported no creationDate: {read:?}")
        });
        let after = now_epoch_seconds();
        assert!(
            (before - 1.0..=after + 1.0).contains(&created_at),
            "case {label:?}: creationDate {created_at} is outside [{before}, {after}], got {read:?}"
        );
        assert_eq!(
            read.member_epoch("lastModifiedDate"),
            Some(created_at),
            "case {label:?}: a certificate nothing has updated was last modified when it was \
             issued ({created_at}), got {read:?}"
        );

        // The reported validity is the validity the certificate itself carries, to the
        // second.
        let pem = issued.text("certificatePem").unwrap_or_default();
        let parsed = parse_certificate(label, &pem);
        let validity = read.member("validity").cloned().unwrap_or(Value::Null);
        for (member, expected) in [
            (
                "notBefore",
                parsed.certificate.tbs_certificate.validity.not_before,
            ),
            (
                "notAfter",
                parsed.certificate.tbs_certificate.validity.not_after,
            ),
        ] {
            let reported = validity
                .get(member)
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    panic!("case {label:?}: the description reported no validity.{member}: {read:?}")
                });
            let expected = expected.to_unix_duration().as_secs_f64();
            assert!(
                (reported - expected).abs() < 1.0,
                "case {label:?}: expected validity.{member} {expected}, got {reported}"
            );
        }

        // The private key is issued once, by the create, and recorded nowhere: no
        // member of the description carries it.
        let rendered = read.body.to_string();
        assert!(
            !rendered.contains("PRIVATE KEY"),
            "case {label:?}: a certificate description must carry no private key, got {read:?}"
        );
    }
}

#[tokio::test]
async fn each_call_issues_a_different_certificate_and_key_pair() {
    let emulator = Emulator::new();

    // Each call "Creates a 2048-bit RSA key pair and issues an X.509 certificate": a
    // second call mints a second identity rather than re-issuing the first one.
    let mut ids = Vec::new();
    let mut private_keys = Vec::new();
    for case in cases() {
        let label = case.label;
        let issued = emulator.create(case.set_as_active, case.region).await;
        assert_eq!(
            issued.status, 200,
            "case {label:?}: CreateKeysAndCertificate must succeed, got {issued:?}"
        );
        let id = issued.text("certificateId").unwrap_or_else(|| {
            panic!("case {label:?}: the response must carry a certificateId, got {issued:?}")
        });
        assert!(
            !ids.contains(&id),
            "case {label:?}: certificateId {id} was already issued to another call"
        );
        let private_key = issued
            .key_pair("PrivateKey")
            .unwrap_or_else(|| {
                panic!("case {label:?}: the response must carry keyPair.PrivateKey, got {issued:?}")
            })
            .to_string();
        assert!(
            !private_keys.contains(&private_key),
            "case {label:?}: the private key of certificate {id} repeats an earlier one"
        );
        ids.push(id);
        private_keys.push(private_key);
    }

    // Every certificate issued is still readable: one issue overwrote none of the
    // others.
    for id in &ids {
        let read = emulator.describe(id, REGION).await;
        assert_eq!(
            read.status, 200,
            "case {id:?}: every issued certificate stays readable, got {read:?}"
        );
    }
}

#[tokio::test]
async fn reading_a_certificate_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    // `DescribeCertificate` reports the certificate; it must not re-time, re-mint or
    // otherwise rewrite it, so two reads of one certificate are byte-identical.
    for case in cases() {
        let label = case.label;
        let issued = emulator.create(case.set_as_active, case.region).await;
        let id = issued.text("certificateId").unwrap_or_else(|| {
            panic!("case {label:?}: the response must carry a certificateId, got {issued:?}")
        });

        let first = emulator.describe(&id, case.region).await;
        let second = emulator.describe(&id, case.region).await;
        assert_eq!(
            first.status, second.status,
            "case {label:?}: both reads must answer the same status, got {second:?}"
        );
        assert_eq!(
            first.body, second.body,
            "case {label:?}: a read must not change the certificate, got {first:?} then {second:?}"
        );
    }
}

#[tokio::test]
async fn describing_an_unissued_certificate_is_refused_not_found() {
    let emulator = Emulator::new();

    // One certificate exists, so the refusals below are about the id and not about an
    // empty store.
    let issued = emulator.create(Some(true), REGION).await;
    let existing = issued
        .text("certificateId")
        .expect("the issued certificate has an id");

    // `CertificateId` is modelled `length 64..64, pattern ^(0x)?[a-fA-F0-9]+$`
    // (`smithy_query(query="operation",operation="DescribeCertificate")`), so every id
    // below is a legal one the store simply does not hold — a hex digest of the right
    // length, the same with the `0x` prefix the pattern allows, and the existing id
    // with its last character changed.
    let mut neighbour = existing.clone();
    neighbour.pop();
    neighbour.push(if existing.ends_with('0') { '1' } else { '0' });
    for id in [
        "0".repeat(64),
        "f".repeat(64),
        format!("0x{}", "a".repeat(62)),
        neighbour,
    ] {
        let read = emulator.describe(&id, REGION).await;
        assert_eq!(
            read.status, 404,
            "case {id:?}: DescribeCertificate of an unissued id must be refused 404, got {read:?}"
        );
        assert_eq!(
            read.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {id:?}: expected ResourceNotFoundException, got {read:?}"
        );
        assert!(
            read.description().is_none(),
            "case {id:?}: a refusal carries no certificateDescription, got {read:?}"
        );
    }

    // The certificate that does exist is still readable: the refusals wrote nothing.
    let read = emulator.describe(&existing, REGION).await;
    assert_eq!(
        read.status, 200,
        "the issued certificate must still be readable, got {read:?}"
    );
}
