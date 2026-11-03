// SPDX-License-Identifier: CC0-1.0

//! The imported-certificate plane end to end: `ImportCertificate`,
//! `DescribeCertificate`, `UpdateCertificate` and `DeleteCertificate`.
//!
//! RULE under test: an import STORES one certificate record under a newly minted
//! `CertificateId` and every fact a certificate reader then reports comes from that
//! record — the certificate message the request framed, the chain beside it, the
//! `Serial`/`NotBeforeDate`/`NotAfterDate` read OUT of that material, the `Usage` the
//! request named, the `Type` the presence of a `PrivateKey` decides, the description
//! and the tags — with `ActiveDate`/`InactiveDate` resolved from the request or, where
//! it names no bound, from the certificate's own `NotBeforeDate`/`NotAfterDate` (the
//! model states that default for `ImportCertificate`'s members and for
//! `UpdateCertificate`'s, so both writes share it), and `Status` DERIVED from that
//! window and the clock. A write whose material cannot be read as a certificate is
//! refused, an id the store does not hold is refused by every reader and writer of this
//! resource, and a delete removes exactly one record.
//!
//! The rows differ along what the rule generalises over: every value of
//! `CertificateUsageType`, a private key present and absent, a chain present and
//! absent, a `Description` absent and at both modelled `len 1..200` bounds, 0/1/3 tags,
//! each form of requested window (neither bound, each bound alone, both), certificates
//! whose OWN validity period is current, not yet started and already over, the serial
//! the issuing CA assigned (a generator-chosen one, one with a leading zero byte and a
//! 20-octet one), and sub-second timestamp precision.
//!
//! Every certificate the service is fed is built by `rcgen` — something other than the
//! code under test — and every call goes over the real wire path through the shared
//! [`crate::harness`], so a response that is not computed from what was imported cannot
//! pass.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_transfer_transferservice::state::{
    Certificate, CERTIFICATE_STATUS_ACTIVE, CERTIFICATE_STATUS_INACTIVE,
    CERTIFICATE_STATUS_PENDING_ROTATION, CERTIFICATE_TYPE_CERTIFICATE,
    CERTIFICATE_TYPE_WITH_PRIVATE_KEY, MAX_STORED_CERTIFICATES,
};
use x509_cert::der::DecodePem;

use crate::harness::{
    is_modelled_certificate_id, member_of, Answer, Emulator, ACCOUNT, REGION,
};

/// `CertificateUsageType`'s whole modelled value set
/// (`smithy_query(query="shape",shape="CertificateUsageType")` reports
/// `[SIGNING, ENCRYPTION, TLS]`).
const USAGES: [&str; 3] = ["SIGNING", "ENCRYPTION", "TLS"];

/// How close two timestamp members must be to count as the same instant: one
/// microsecond, far below the millisecond precision a caller sends a `CertDate` with
/// and far above the rounding of one epoch-seconds float.
const INSTANT_TOLERANCE: f64 = 1e-6;

/// One certificate the test generated, and what a reader must therefore report about
/// it.
struct Generated {
    /// The PEM text handed to `ImportCertificate` as `Certificate`.
    certificate: String,
    /// The certificate MESSAGE inside that text — the PEM without the line ending
    /// that closes it — which is what the record holds and a reader reports.
    message: String,
    /// The PEM text of its key pair, handed over as `PrivateKey` where a row asks for
    /// one.
    private_key: String,
    /// The certificate's own serial, as `CertSerial` must carry it: the X.509 serial
    /// number's bytes, each as two lowercase hex digits, unseparated.
    serial: String,
    /// Its own validity period, in epoch seconds.
    not_before: f64,
    not_after: f64,
}

/// Generate a real self-signed certificate valid over `[from, to]` (whole years), with
/// `serial` as its X.509 serial number when the row chooses one.
fn generate(validity: (i32, i32), serial: Option<&[u8]>) -> Generated {
    let key = rcgen::KeyPair::generate().expect("a key pair");
    let mut params =
        rcgen::CertificateParams::new(vec!["as2.example.com".to_string()]).expect("parameters");
    let mut subject = rcgen::DistinguishedName::new();
    subject.push(rcgen::DnType::CommonName, "as2.example.com");
    params.distinguished_name = subject;
    params.not_before = rcgen::date_time_ymd(validity.0, 1, 1);
    params.not_after = rcgen::date_time_ymd(validity.1, 1, 1);
    if let Some(serial) = serial {
        params.serial_number = Some(rcgen::SerialNumber::from(serial.to_vec()));
    }
    let certificate = params
        .self_signed(&key)
        .expect("a self-signed certificate")
        .pem();
    // The expectations are read back out of the GENERATED certificate, so they are the
    // certificate's own values and not the handler's.
    let parsed = x509_cert::Certificate::from_pem(certificate.as_bytes())
        .expect("the generated certificate parses");
    let validity = &parsed.tbs_certificate.validity;
    Generated {
        serial: parsed
            .tbs_certificate
            .serial_number
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        not_before: epoch_seconds(validity.not_before.to_system_time()),
        not_after: epoch_seconds(validity.not_after.to_system_time()),
        message: certificate.trim_end().to_string(),
        certificate,
        private_key: key.serialize_pem(),
    }
}

/// A certificate that is valid NOW: the window every row uses unless it is about a
/// window that has not opened or has closed.
fn current_certificate() -> Generated {
    generate((2020, 2100), None)
}

/// An instant as the epoch seconds a timestamp member carries on the wire.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("an instant after the Unix epoch")
        .as_secs_f64()
}

/// `now + offset` seconds, as the epoch seconds a request sends a `CertDate` as.
fn offset_from_now(offset: f64) -> f64 {
    epoch_seconds(SystemTime::now()) + offset
}

/// Assert two timestamp members name the same instant.
fn assert_instant(case: &str, member: &str, actual: &Value, expected: f64) {
    let got = actual.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected {member} to be a timestamp, got {actual:?}")
    });
    assert!(
        (got - expected).abs() < INSTANT_TOLERANCE,
        "case {case:?}: expected {member} {expected}, got {got}"
    );
}

/// Import one certificate and hand back the answer.
async fn import(emulator: &Emulator, request: Value) -> Answer {
    emulator.call("ImportCertificate", request).await
}

/// Import one certificate that must succeed and hand back the `CertificateId` the
/// service minted.
async fn import_ok(emulator: &Emulator, case: &str, request: Value) -> String {
    let answer = import(emulator, request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected ImportCertificate 200, got {} with body {}",
        answer.status, answer.body
    );
    let certificate_id = answer
        .get("CertificateId")
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: ImportCertificate returned no CertificateId: {}", answer.body)
        })
        .to_string();
    assert!(
        is_modelled_certificate_id(&certificate_id),
        "case {case:?}: expected a modelled CertificateId, got {certificate_id:?}"
    );
    certificate_id
}

/// Read one certificate back and hand back the `Certificate` structure.
async fn describe(emulator: &Emulator, case: &str, certificate_id: &str) -> Value {
    let answer = emulator
        .call("DescribeCertificate", json!({"CertificateId": certificate_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeCertificate 200, got {} with body {}",
        answer.status, answer.body
    );
    member_of(&answer.body, "Certificate").clone()
}

/// One import case: what it exercises, the material to generate, and the request
/// members and expectations that go with it.
struct ImportCase {
    name: &'static str,
    /// The validity period of the certificate to generate, in whole years.
    validity: (i32, i32),
    /// The X.509 serial to assign it, `None` leaving the generator's own.
    serial: Option<&'static [u8]>,
    usage: &'static str,
    with_private_key: bool,
    with_chain: bool,
    description: Option<String>,
    tags: Vec<(&'static str, &'static str)>,
    /// `ActiveDate`/`InactiveDate` to send, as offsets in seconds from the call;
    /// `None` sends the member not at all.
    active_offset: Option<f64>,
    inactive_offset: Option<f64>,
    /// The `Status` the resolved window puts the record in.
    status: &'static str,
}

impl ImportCase {
    /// A row that sends neither window bound and a certificate that is valid now.
    fn new(name: &'static str, usage: &'static str) -> Self {
        Self {
            name,
            validity: (2020, 2100),
            serial: None,
            usage,
            with_private_key: false,
            with_chain: false,
            description: None,
            tags: Vec::new(),
            active_offset: None,
            inactive_offset: None,
            status: CERTIFICATE_STATUS_ACTIVE,
        }
    }
}

/// A `Description` of exactly 200 characters: the upper bound of `Description`'s
/// modelled `len 1..200`, whose pattern `^[\u0021-\u007E]+$` `d` satisfies.
fn description_at_max_length() -> String {
    "d".repeat(200)
}

/// The rows: the member values and window forms an import can carry.
fn import_cases() -> Vec<ImportCase> {
    vec![
        ImportCase {
            with_private_key: true,
            description: Some("as2-signing".to_string()),
            ..ImportCase::new("signing key pair, no window requested", USAGES[0])
        },
        ImportCase {
            with_chain: true,
            tags: vec![("env", "test")],
            active_offset: Some(-3_600.0),
            inactive_offset: Some(3_600.0),
            ..ImportCase::new("encryption certificate inside a requested window", USAGES[1])
        },
        ImportCase {
            with_private_key: true,
            with_chain: true,
            // `Description`'s modelled upper bound.
            description: Some(description_at_max_length()),
            tags: vec![("env", "test"), ("owner", "as2"), ("tier", "1")],
            active_offset: Some(3_600.0),
            status: CERTIFICATE_STATUS_PENDING_ROTATION,
            ..ImportCase::new("tls certificate whose requested window has not opened", USAGES[2])
        },
        ImportCase {
            // `Description`'s modelled lower bound, `len 1`.
            description: Some("x".to_string()),
            inactive_offset: Some(-1.0),
            status: CERTIFICATE_STATUS_INACTIVE,
            ..ImportCase::new("signing certificate whose requested window has closed", USAGES[0])
        },
        ImportCase {
            // The CA's own dates are the window when the request names none, so a
            // certificate that is not valid yet is not active yet.
            validity: (2090, 2100),
            status: CERTIFICATE_STATUS_PENDING_ROTATION,
            ..ImportCase::new("certificate whose own validity has not begun", USAGES[1])
        },
        ImportCase {
            validity: (2000, 2001),
            status: CERTIFICATE_STATUS_INACTIVE,
            ..ImportCase::new("certificate whose own validity is over", USAGES[2])
        },
        ImportCase {
            // A serial with a LEADING ZERO byte: the rendering keeps it, so the
            // reported value is the CA's serial and not an integer re-rendered.
            serial: Some(&[0x00, 0x7f, 0xff]),
            with_private_key: true,
            ..ImportCase::new("certificate carrying a short serial", USAGES[0])
        },
        ImportCase {
            // 20 octets: the longest serial RFC 5280 section 4.1.2.2 allows, which is
            // 40 of `CertSerial`'s modelled `len 0..48` digits.
            serial: Some(&[
                0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89,
                0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67,
            ]),
            ..ImportCase::new("certificate carrying a 20-octet serial", USAGES[1])
        },
    ]
}

/// The `Tags` request member for `pairs`.
fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// RULE: every member a certificate reader reports is the stored record's, with
/// `Status` derived from the resolved window and the clock.
#[tokio::test]
async fn an_imported_certificate_is_reported_by_its_own_reader() {
    let emulator = Emulator::new();

    for case in import_cases() {
        let name = case.name;
        let generated = generate(case.validity, case.serial);
        // The chain is a second certificate, so a row that sends one can tell the
        // chain it stored apart from the certificate it stored.
        let chain = generate((2020, 2100), None).certificate;

        let mut request = json!({
            "Usage": case.usage,
            "Certificate": &generated.certificate,
        });
        if case.with_private_key {
            request["PrivateKey"] = json!(&generated.private_key);
        }
        if case.with_chain {
            request["CertificateChain"] = json!(&chain);
        }
        if let Some(description) = &case.description {
            request["Description"] = json!(description);
        }
        if !case.tags.is_empty() {
            request["Tags"] = tag_list(&case.tags);
        }
        let active = case.active_offset.map(offset_from_now);
        let inactive = case.inactive_offset.map(offset_from_now);
        if let Some(active) = active {
            request["ActiveDate"] = json!(active);
        }
        if let Some(inactive) = inactive {
            request["InactiveDate"] = json!(inactive);
        }

        let certificate_id = import_ok(&emulator, name, request).await;
        let described = describe(&emulator, name, &certificate_id).await;

        assert_eq!(
            member_of(&described, "CertificateId").as_str(),
            Some(certificate_id.as_str()),
            "case {name:?}: expected the minted CertificateId, got {described}"
        );
        assert_eq!(
            member_of(&described, "Arn").as_str(),
            Some(
                format!("arn:aws:transfer:{REGION}:{ACCOUNT}:certificate/{certificate_id}")
                    .as_str()
            ),
            "case {name:?}: expected the certificate's own ARN, got {described}"
        );
        assert_eq!(
            member_of(&described, "Usage").as_str(),
            Some(case.usage),
            "case {name:?}: expected Usage {:?}, got {described}",
            case.usage
        );
        let expected_type = if case.with_private_key {
            CERTIFICATE_TYPE_WITH_PRIVATE_KEY
        } else {
            CERTIFICATE_TYPE_CERTIFICATE
        };
        assert_eq!(
            member_of(&described, "Type").as_str(),
            Some(expected_type),
            "case {name:?}: expected Type {expected_type:?}, got {described}"
        );
        assert_eq!(
            member_of(&described, "Status").as_str(),
            Some(case.status),
            "case {name:?}: expected Status {:?}, got {described}",
            case.status
        );
        // The material, as the record holds it.
        assert_eq!(
            member_of(&described, "Certificate").as_str(),
            Some(generated.message.as_str()),
            "case {name:?}: expected the imported certificate back unchanged, got {described}"
        );
        let expected_chain = if case.with_chain {
            Some(chain.trim_end())
        } else {
            None
        };
        assert_eq!(
            member_of(&described, "CertificateChain").as_str(),
            expected_chain,
            "case {name:?}: expected CertificateChain {expected_chain:?}, got {described}"
        );
        // Read OUT of the material: the CA's serial and validity period.
        assert_eq!(
            member_of(&described, "Serial").as_str(),
            Some(generated.serial.as_str()),
            "case {name:?}: expected Serial {:?}, got {described}",
            generated.serial
        );
        assert_instant(
            name,
            "NotBeforeDate",
            member_of(&described, "NotBeforeDate"),
            generated.not_before,
        );
        assert_instant(
            name,
            "NotAfterDate",
            member_of(&described, "NotAfterDate"),
            generated.not_after,
        );
        // The window: the request's bound where it named one, the CA's date where it
        // did not.
        assert_instant(
            name,
            "ActiveDate",
            member_of(&described, "ActiveDate"),
            active.unwrap_or(generated.not_before),
        );
        assert_instant(
            name,
            "InactiveDate",
            member_of(&described, "InactiveDate"),
            inactive.unwrap_or(generated.not_after),
        );
        assert_eq!(
            member_of(&described, "Description").as_str(),
            case.description.as_deref(),
            "case {name:?}: expected Description {:?}, got {described}",
            case.description
        );
        // The tag set is a collection the certificate always holds, so an import that
        // carried no tags reads back the EMPTY list rather than no member.
        let expected_tags = tag_list(&case.tags);
        assert_eq!(
            member_of(&described, "Tags"),
            &expected_tags,
            "case {name:?}: expected Tags {expected_tags:?}, got {described}"
        );

        // A READ changes nothing a later caller sees: the same read answers the same.
        let again = describe(&emulator, name, &certificate_id).await;
        assert_eq!(
            member_of(&again, "Certificate"),
            member_of(&described, "Certificate"),
            "case {name:?}: a second read reported different material: {again}"
        );
        assert_eq!(
            member_of(&again, "ActiveDate"),
            member_of(&described, "ActiveDate"),
            "case {name:?}: a second read reported a different window: {again}"
        );
    }
}

/// RULE: the instants a request states are stored as sent — a `CertDate` keeps its
/// sub-second precision through the import and the read back.
///
/// The rows are fractions of a second a caller can send: none, a half, a quarter, an
/// eighth and a millisecond-precision value.
#[tokio::test]
async fn a_requested_window_keeps_its_sub_second_precision() {
    let emulator = Emulator::new();
    let generated = current_certificate();

    for fraction in [0.0, 0.5, 0.25, 0.125, 0.123] {
        let case = format!("fraction {fraction}");
        // A whole second well inside the certificate's own validity period, plus the
        // row's fraction.
        let active = 1_762_000_000.0 + fraction;
        let inactive = 1_793_000_000.0 + fraction;
        let certificate_id = import_ok(
            &emulator,
            &case,
            json!({
                "Usage": USAGES[0],
                "Certificate": &generated.certificate,
                "ActiveDate": active,
                "InactiveDate": inactive,
            }),
        )
        .await;

        let described = describe(&emulator, &case, &certificate_id).await;
        assert_instant(
            &case,
            "ActiveDate",
            member_of(&described, "ActiveDate"),
            active,
        );
        assert_instant(
            &case,
            "InactiveDate",
            member_of(&described, "InactiveDate"),
            inactive,
        );
    }
}

/// One material-framing case: what it exercises, the `Certificate` and
/// `CertificateChain` the request carries, and the `Certificate` and `CertificateChain`
/// the reader must then report.
struct MaterialCase {
    name: &'static str,
    body: String,
    sent_chain: Option<String>,
    expected_certificate: String,
    expected_chain: Option<String>,
}

/// RULE: what a certificate reader serves back is the certificate MESSAGE the request
/// framed and, separately, the chain: the record holds the two as the model reports
/// them, `Certificate` may carry both ("You can provide both the certificate and its
/// chain in this parameter"), and the line ending or explanatory text a member frames
/// the material with is framing rather than material (RFC 7468 section 5.2 allows it
/// around an encapsulated message).
///
/// The rows are the framings one certificate's PEM can arrive in: as the generator
/// wrote it, without its trailing newline, with CRLF line endings, with explanatory
/// text around the message, with the chain appended to the same member, and with the
/// chain in its own member.
#[tokio::test]
async fn the_imported_material_is_served_back_as_the_record_holds_it() {
    let emulator = Emulator::new();
    let generated = current_certificate();
    let chain = generate((2020, 2100), None).certificate;
    let message = generated.message.as_str();
    let chain_message = chain.trim_end();
    let pem = generated.certificate.as_str();

    // (case, the `Certificate` body, the `CertificateChain` body, the `Certificate`
    // the reader must report, the `CertificateChain` it must report)
    let rows: [MaterialCase; 6] = [
        MaterialCase {
            name: "as generated",
            body: pem.to_string(),
            sent_chain: None,
            expected_certificate: message.to_string(),
            expected_chain: None,
        },
        MaterialCase {
            name: "without a trailing newline",
            body: message.to_string(),
            sent_chain: None,
            expected_certificate: message.to_string(),
            expected_chain: None,
        },
        MaterialCase {
            name: "with CRLF line endings",
            body: pem.replace('\n', "\r\n"),
            sent_chain: None,
            expected_certificate: message.replace('\n', "\r\n"),
            expected_chain: None,
        },
        MaterialCase {
            name: "with explanatory text around the message",
            body: format!("subject=CN=as2.example.com\n{pem}trailing text\n"),
            sent_chain: None,
            expected_certificate: message.to_string(),
            expected_chain: None,
        },
        MaterialCase {
            name: "with the chain appended to the same member",
            body: format!("{pem}{chain}"),
            sent_chain: None,
            expected_certificate: message.to_string(),
            expected_chain: Some(chain_message.to_string()),
        },
        MaterialCase {
            name: "with the chain in its own member",
            body: pem.to_string(),
            sent_chain: Some(chain.clone()),
            expected_certificate: message.to_string(),
            expected_chain: Some(chain_message.to_string()),
        },
    ];

    for row in rows {
        let MaterialCase {
            name: case,
            body,
            sent_chain,
            expected_certificate,
            expected_chain,
        } = row;
        let mut request = json!({"Usage": USAGES[0], "Certificate": &body});
        if let Some(sent_chain) = &sent_chain {
            request["CertificateChain"] = json!(sent_chain);
        }
        let certificate_id = import_ok(&emulator, case, request).await;
        let described = describe(&emulator, case, &certificate_id).await;
        assert_eq!(
            member_of(&described, "Certificate").as_str(),
            Some(expected_certificate.as_str()),
            "case {case:?}: expected the certificate the body carried, got {described}"
        );
        assert_eq!(
            member_of(&described, "CertificateChain").as_str(),
            expected_chain.as_deref(),
            "case {case:?}: expected CertificateChain {expected_chain:?}, got {described}"
        );
        // Whatever the framing, the serial and validity period are the LEADING
        // certificate's own.
        assert_eq!(
            member_of(&described, "Serial").as_str(),
            Some(generated.serial.as_str()),
            "case {case:?}: expected the certificate's own Serial, got {described}"
        );
        assert_instant(
            case,
            "NotBeforeDate",
            member_of(&described, "NotBeforeDate"),
            generated.not_before,
        );
        assert_instant(
            case,
            "NotAfterDate",
            member_of(&described, "NotAfterDate"),
            generated.not_after,
        );
    }
}

/// RULE: material this service cannot read as a PEM-encoded X.509 certificate is
/// REFUSED — the serial and validity period a reader reports are the certificate's own,
/// so there is nothing to report for a body that carries no certificate — and the
/// refusal stores nothing.
///
/// The rows differ along WHY the body carries no certificate: it is not PEM at all, it
/// frames a PEM message of another type, its framing is truncated, and its base64
/// content is not a certificate.
#[tokio::test]
async fn a_body_that_is_not_a_certificate_is_refused() {
    let emulator = Emulator::new();
    let generated = current_certificate();
    let truncated: String = generated
        .certificate
        .lines()
        .take(2)
        .collect::<Vec<_>>()
        .join("\n");

    // (case, the `Certificate` body to send)
    let rows: [(&str, String); 5] = [
        ("text framing no PEM message", "not a certificate".to_string()),
        (
            "a PEM message of another type",
            generated.private_key.clone(),
        ),
        ("a truncated certificate", truncated),
        (
            "base64 content that is not a certificate",
            "-----BEGIN CERTIFICATE-----\nZm9vYmFy\n-----END CERTIFICATE-----\n".to_string(),
        ),
        (
            "the PEM boundaries with no content",
            "-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n".to_string(),
        ),
    ];

    for (case, body) in rows {
        let answer = import(
            &emulator,
            json!({"Usage": USAGES[0], "Certificate": body}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {}",
            answer.body
        );
        assert_eq!(
            member_of(&answer.body, "Message").as_str(),
            Some("Certificate must be a PEM-encoded X.509 certificate"),
            "case {case:?}: unexpected Message in {}",
            answer.body
        );
    }

    // Nothing was stored by any refused import.
    let listed = emulator.call("ListCertificates", json!({})).await;
    assert_eq!(
        listed.get("Certificates").as_array().map(Vec::len),
        Some(0),
        "a refused import must store nothing, got {}",
        listed.body
    );
}

/// One update case: the members the request carries and what the record must then
/// report.
struct UpdateCase {
    name: &'static str,
    /// `ActiveDate`/`InactiveDate` to send as offsets in seconds from the call, and
    /// the `Description`; a `None` sends that member not at all.
    active_offset: Option<f64>,
    inactive_offset: Option<f64>,
    description: Option<&'static str>,
}

/// RULE: an update RE-RESOLVES the window from the request and the certificate's own
/// dates — a bound it names is set, a bound it does not name takes the certificate's
/// `NotBeforeDate`/`NotAfterDate`, the default the model states for these members —
/// while `Description`, which has no documented default, changes only when the request
/// carries one. The answer names the certificate updated, and the read back shows it.
#[tokio::test]
async fn an_update_re_resolves_the_window_and_replaces_only_what_it_names() {
    let emulator = Emulator::new();
    let generated = current_certificate();

    let rows = [
        UpdateCase {
            name: "both bounds",
            active_offset: Some(-60.0),
            inactive_offset: Some(60.0),
            description: None,
        },
        UpdateCase {
            name: "the active bound alone",
            active_offset: Some(-120.0),
            inactive_offset: None,
            description: None,
        },
        UpdateCase {
            name: "the inactive bound alone",
            active_offset: None,
            inactive_offset: Some(120.0),
            description: None,
        },
        UpdateCase {
            name: "a description alone",
            active_offset: None,
            inactive_offset: None,
            description: Some("renamed"),
        },
        UpdateCase {
            name: "nothing but the identifier",
            active_offset: None,
            inactive_offset: None,
            description: None,
        },
    ];

    for case in rows {
        let name = case.name;
        // Each row starts from the same imported state: a window the import named
        // explicitly and a description, so an update that re-resolves a bound is
        // visible.
        let imported_active = offset_from_now(-7_200.0);
        let imported_inactive = offset_from_now(7_200.0);
        let certificate_id = import_ok(
            &emulator,
            name,
            json!({
                "Usage": USAGES[0],
                "Certificate": &generated.certificate,
                "Description": "imported",
                "ActiveDate": imported_active,
                "InactiveDate": imported_inactive,
            }),
        )
        .await;

        let mut request = json!({"CertificateId": &certificate_id});
        let active = case.active_offset.map(offset_from_now);
        let inactive = case.inactive_offset.map(offset_from_now);
        if let Some(active) = active {
            request["ActiveDate"] = json!(active);
        }
        if let Some(inactive) = inactive {
            request["InactiveDate"] = json!(inactive);
        }
        if let Some(description) = case.description {
            request["Description"] = json!(description);
        }
        let answer = emulator.call("UpdateCertificate", request).await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected UpdateCertificate 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.get("CertificateId").as_str(),
            Some(certificate_id.as_str()),
            "case {name:?}: expected the updated CertificateId back, got {}",
            answer.body
        );

        let described = describe(&emulator, name, &certificate_id).await;
        assert_instant(
            name,
            "ActiveDate",
            member_of(&described, "ActiveDate"),
            active.unwrap_or(generated.not_before),
        );
        assert_instant(
            name,
            "InactiveDate",
            member_of(&described, "InactiveDate"),
            inactive.unwrap_or(generated.not_after),
        );
        assert_eq!(
            member_of(&described, "Description").as_str(),
            Some(case.description.unwrap_or("imported")),
            "case {name:?}: unexpected Description in {described}"
        );
        // Neither write touches the material or what was read out of it.
        assert_eq!(
            member_of(&described, "Certificate").as_str(),
            Some(generated.message.as_str()),
            "case {name:?}: the update changed the stored material: {described}"
        );
        assert_eq!(
            member_of(&described, "Serial").as_str(),
            Some(generated.serial.as_str()),
            "case {name:?}: the update changed the stored serial: {described}"
        );
        assert_eq!(
            member_of(&described, "Status").as_str(),
            Some(CERTIFICATE_STATUS_ACTIVE),
            "case {name:?}: expected the record to stay inside its window: {described}"
        );
    }
}

/// RULE: a delete removes exactly the record its `CertificateId` names, and afterwards
/// every reader and writer of that id answers the not-found refusal — `Resource` the id
/// the request named, `ResourceType` the resource name capitalized and a fixed `Message`
/// carrying no identifier.
///
/// The rows are the operations addressed at a `CertificateId` the store does not hold,
/// and WHY it does not hold it: never imported (three id shapes), or deleted.
#[tokio::test]
async fn a_deleted_certificate_is_gone_from_every_certificate_operation() {
    let emulator = Emulator::new();
    let generated = current_certificate();

    // Two records, so the delete can be shown to remove exactly one.
    let kept = import_ok(
        &emulator,
        "kept",
        json!({"Usage": USAGES[0], "Certificate": &generated.certificate}),
    )
    .await;
    let doomed = import_ok(
        &emulator,
        "doomed",
        json!({
            "Usage": USAGES[1],
            "Certificate": &generated.certificate,
            "CertificateChain": &generated.certificate,
            "Tags": tag_list(&[("env", "test")]),
        }),
    )
    .await;

    let deleted = emulator
        .call("DeleteCertificate", json!({"CertificateId": &doomed}))
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteCertificate 200, got {} with body {}",
        deleted.status, deleted.body
    );

    // The other record is untouched, and it is the only one listed.
    let listed = emulator.call("ListCertificates", json!({})).await;
    let ids: Vec<String> = listed
        .get("Certificates")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            member_of(item, "CertificateId")
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        ids,
        vec![kept.clone()],
        "expected only the kept certificate to remain, got {}",
        listed.body
    );

    // Syntactically valid `CertificateId`s (`len 22..22`, `^cert-([0-9a-f]{17})$`) no
    // import ever minted: the low end of the id space, a mid value and the high end.
    let never_minted = [
        "cert-00000000000000000".to_string(),
        "cert-0000000000000dead".to_string(),
        "cert-fffffffffffffffff".to_string(),
    ];
    let mut missing: Vec<(String, String)> = never_minted
        .iter()
        .map(|id| (format!("never imported {id}"), id.clone()))
        .collect();
    missing.push(("deleted".to_string(), doomed.clone()));

    for (why, certificate_id) in missing {
        for (op, request) in [
            ("DescribeCertificate", json!({"CertificateId": &certificate_id})),
            ("DeleteCertificate", json!({"CertificateId": &certificate_id})),
            (
                "UpdateCertificate",
                json!({"CertificateId": &certificate_id, "Description": "whatever"}),
            ),
        ] {
            let case = format!("{op} on a certificate {why}");
            let answer = emulator.call(op, request).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {}",
                answer.body
            );
            assert_eq!(
                member_of(&answer.body, "Resource").as_str(),
                Some(certificate_id.as_str()),
                "case {case:?}: expected the requested id in Resource, got {}",
                answer.body
            );
            assert_eq!(
                member_of(&answer.body, "ResourceType").as_str(),
                Some("Certificate"),
                "case {case:?}: unexpected ResourceType in {}",
                answer.body
            );
            let message = member_of(&answer.body, "Message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !message.is_empty() && !message.contains(&certificate_id),
                "case {case:?}: expected a fixed Message carrying no identifier, got {message:?}"
            );
        }
    }
}

/// RULE: a certificate's tags are the tag plane's like every other taggable resource's
/// — the set the import carried is what `ListTagsForResource` reports for the
/// certificate's ARN, a `TagResource` write on that ARN is what `DescribeCertificate`
/// then reports, and `UntagResource` detaches exactly the keys it names.
#[tokio::test]
async fn a_certificates_tags_are_reachable_through_the_tag_plane() {
    let emulator = Emulator::new();
    let generated = current_certificate();

    let certificate_id = import_ok(
        &emulator,
        "tagged import",
        json!({
            "Usage": USAGES[2],
            "Certificate": &generated.certificate,
            "Tags": tag_list(&[("env", "test"), ("owner", "as2")]),
        }),
    )
    .await;
    let arn = format!("arn:aws:transfer:{REGION}:{ACCOUNT}:certificate/{certificate_id}");

    let listed = emulator
        .call("ListTagsForResource", json!({"Arn": &arn}))
        .await;
    assert_eq!(
        listed.get("Tags"),
        &tag_list(&[("env", "test"), ("owner", "as2")]),
        "expected the imported tag set through the tag plane, got {}",
        listed.body
    );

    let tagged = emulator
        .call(
            "TagResource",
            json!({"Arn": &arn, "Tags": tag_list(&[("owner", "ops"), ("tier", "1")])}),
        )
        .await;
    assert_eq!(
        tagged.status, 200,
        "expected TagResource 200, got {} with body {}",
        tagged.status, tagged.body
    );
    let described = describe(&emulator, "after TagResource", &certificate_id).await;
    assert_eq!(
        member_of(&described, "Tags"),
        &tag_list(&[("env", "test"), ("owner", "ops"), ("tier", "1")]),
        "expected the written tag set from the certificate's own reader, got {described}"
    );

    let untagged = emulator
        .call("UntagResource", json!({"Arn": &arn, "TagKeys": ["env", "tier"]}))
        .await;
    assert_eq!(
        untagged.status, 200,
        "expected UntagResource 200, got {} with body {}",
        untagged.status, untagged.body
    );
    let described = describe(&emulator, "after UntagResource", &certificate_id).await;
    assert_eq!(
        member_of(&described, "Tags"),
        &tag_list(&[("owner", "ops")]),
        "expected only the keys the untag left, got {described}"
    );
}

/// RULE: every import mints a NEW identifier — the same material imported twice is two
/// independent records, each reporting its own usage, window and tags.
#[tokio::test]
async fn importing_the_same_material_twice_stores_two_records() {
    let emulator = Emulator::new();
    let generated = current_certificate();

    let first = import_ok(
        &emulator,
        "first import",
        json!({
            "Usage": USAGES[0],
            "Certificate": &generated.certificate,
            "Description": "first",
        }),
    )
    .await;
    let second = import_ok(
        &emulator,
        "second import",
        json!({
            "Usage": USAGES[1],
            "Certificate": &generated.certificate,
            "Description": "second",
        }),
    )
    .await;
    assert_ne!(
        first, second,
        "two imports must mint two identifiers, got {first:?} twice"
    );

    for (case, certificate_id, usage, description) in [
        ("first import", &first, USAGES[0], "first"),
        ("second import", &second, USAGES[1], "second"),
    ] {
        let described = describe(&emulator, case, certificate_id).await;
        assert_eq!(
            member_of(&described, "Usage").as_str(),
            Some(usage),
            "case {case:?}: expected Usage {usage:?}, got {described}"
        );
        assert_eq!(
            member_of(&described, "Description").as_str(),
            Some(description),
            "case {case:?}: expected Description {description:?}, got {described}"
        );
    }
}

/// RULE: the certificate store is BOUNDED — at capacity an import is refused with the
/// modelled `ServiceUnavailableException` rather than growing the store, and a delete
/// releases the capacity again.
#[tokio::test]
async fn an_import_past_the_store_bound_is_refused_until_a_delete_releases_it() {
    let emulator = Emulator::new();
    let generated = current_certificate();

    // Fill the store to its bound. These records are seeded directly — the bound is
    // about the COUNT, and the import path is what the rest of this suite drives.
    let filler = {
        let store = emulator.store();
        let mut guard = store.lock();
        let mut last = String::new();
        while guard.certificates.len() < MAX_STORED_CERTIFICATES {
            let certificate_id = guard.mint_id("cert");
            guard.certificates.insert(
                certificate_id.clone(),
                Certificate {
                    certificate_id: certificate_id.clone(),
                    usage: USAGES[0].to_string(),
                    certificate_type: CERTIFICATE_TYPE_CERTIFICATE.to_string(),
                    ..Certificate::default()
                },
            );
            last = certificate_id;
        }
        last
    };

    let refused = import(
        &emulator,
        json!({"Usage": USAGES[0], "Certificate": &generated.certificate}),
    )
    .await;
    assert_eq!(
        refused.status, 503,
        "expected HTTP 503 at the store bound, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ServiceUnavailableException",
        "expected ServiceUnavailableException at the store bound, got {}",
        refused.body
    );

    let deleted = emulator
        .call("DeleteCertificate", json!({"CertificateId": &filler}))
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteCertificate 200, got {} with body {}",
        deleted.status, deleted.body
    );

    let accepted = import_ok(
        &emulator,
        "after a delete released capacity",
        json!({"Usage": USAGES[0], "Certificate": &generated.certificate}),
    )
    .await;
    let described = describe(&emulator, "after a delete released capacity", &accepted).await;
    assert_eq!(
        member_of(&described, "Certificate").as_str(),
        Some(generated.message.as_str()),
        "expected the import that fit to be stored, got {described}"
    );
}

/// RULE: `Status` follows the CLOCK across the stored window without any caller
/// touching the record — a certificate whose window opens shortly is
/// `PENDING_ROTATION` and reads as `ACTIVE` once the clock has passed that bound.
#[tokio::test]
async fn a_certificates_status_follows_the_clock_into_its_window() {
    let emulator = Emulator::new();
    let generated = current_certificate();
    let opens_in = Duration::from_millis(600);

    let certificate_id = import_ok(
        &emulator,
        "window opening shortly",
        json!({
            "Usage": USAGES[0],
            "Certificate": &generated.certificate,
            "ActiveDate": epoch_seconds(SystemTime::now() + opens_in),
        }),
    )
    .await;

    let before = describe(&emulator, "before the window opens", &certificate_id).await;
    assert_eq!(
        member_of(&before, "Status").as_str(),
        Some(CERTIFICATE_STATUS_PENDING_ROTATION),
        "expected PENDING_ROTATION before the window opens, got {before}"
    );

    tokio::time::sleep(opens_in + Duration::from_millis(400)).await;
    let after = describe(&emulator, "inside the window", &certificate_id).await;
    assert_eq!(
        member_of(&after, "Status").as_str(),
        Some(CERTIFICATE_STATUS_ACTIVE),
        "expected ACTIVE once the clock is inside the window, got {after}"
    );
}
