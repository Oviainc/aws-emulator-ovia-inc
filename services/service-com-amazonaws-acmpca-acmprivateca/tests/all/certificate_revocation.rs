// SPDX-License-Identifier: CC0-1.0

//! Revoking a certificate a private CA issued.
//!
//! The rule under test: `RevokeCertificate` records a TERMINAL revocation on the one
//! certificate the named CA issued under the serial number the request carries — the
//! serial names it by the hexadecimal VALUE it denotes, so the separators, the digit case
//! and the leading zeros it is spelled with are no part of it — and only while the CA is in
//! a status it can still sign and publish a revocation from. The one certificate that is
//! never revoked is the CA's OWN self-signed certificate, and a second revocation of a
//! certificate already revoked is refused as a request already completed, leaving what the
//! first call recorded untouched.
//!
//! The rows differ along what the rule generalises over: the spelling the serial arrives
//! in, the `RevocationReason` the request names (every member the model declares), the
//! status the CA is in (every member `CertificateAuthorityStatus` declares), which way a
//! serial names no certificate of the CA, and whether the certificate is the CA's own.
//!
//! Every call goes over the wire through the harness in [`super::private_ca_lifecycle`].

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_acmpca_acmprivateca::ops::types::{
    CERTIFICATE_AUTHORITY_STATUS_VALUES, REVOCATION_REASON_VALUES,
};
use service_com_amazonaws_acmpca_acmprivateca::state::{
    not_in_state_to_have_issued_message, CA_NOT_FOUND_MESSAGE,
    CERTIFICATE_ALREADY_REVOKED_MESSAGE, CERTIFICATE_NOT_FOUND_MESSAGE, INVALID_ARN_MESSAGE,
    ROOT_SELF_SIGNED_REVOCATION_MESSAGE, STATUS_ACTIVE, STATUS_CREATING, STATUS_DELETED,
    STATUS_DISABLED, STATUS_EXPIRED, STATUS_FAILED, STATUS_PENDING_CERTIFICATE,
};
use x509_cert::der::DecodePem;
use x509_cert::Certificate;

use crate::delete_certificate_authority::force_status;
use crate::private_ca_lifecycle::{
    active_ca, blob, own_ca_certificate, root_ca_request, string, wait_for_the_clock,
    Emulator, ACCOUNT, REGION,
};
use crate::resource_policy::error_message;

/// The template that produces a ROOT CA's own certificate, the one step of the documented
/// root-CA procedure that is about which KIND of certificate is asked for.
const ROOT_CA_TEMPLATE: &str = "arn:aws:acm-pca:::template/RootCACertificate/V1";

/// A `RevokeCertificate` request body.
fn revocation(ca_arn: &str, serial: &str, reason: &str) -> Value {
    json!({
        "CertificateAuthorityArn": ca_arn,
        "CertificateSerial": serial,
        "RevocationReason": reason,
    })
}

/// `serial`'s octets, separated by `separator`.
fn join_octets(serial: &str, separator: char) -> String {
    serial
        .as_bytes()
        .chunks(2)
        .map(|octet| String::from_utf8_lossy(octet).to_string())
        .collect::<Vec<String>>()
        .join(&separator.to_string())
}

/// One way a serial number can be spelled: the name of the spelling and the function
/// that writes a serial that way.
type SerialSpelling = (&'static str, fn(&str) -> String);

/// The spellings of ONE serial number this suite drives, each a different way the same
/// hexadecimal value arrives in a request.
///
/// `RevokeCertificateRequest$CertificateSerial`'s docs have the serial read off
/// `openssl x509 -text`, which prints it as colon-separated octets; hexadecimal digits
/// are case-insensitive; and a leading zero changes no value.
const SERIAL_SPELLINGS: &[SerialSpelling] = &[
    ("as the certificate's ARN carries it", |serial| {
        serial.to_string()
    }),
    ("in upper case", |serial| serial.to_ascii_uppercase()),
    ("as colon-separated octets", |serial| {
        join_octets(serial, ':')
    }),
    (
        "as colon-separated octets behind the leading zero octet openssl prints",
        |serial| format!("00:{}", join_octets(serial, ':')),
    ),
    ("with leading zeros", |serial| format!("000{serial}")),
    ("as upper-case colon-separated octets", |serial| {
        join_octets(&serial.to_ascii_uppercase(), ':')
    }),
];

/// A real PKCS#10 certification request for a CA of this suite to sign: the request a
/// freshly created CA publishes, built by the service from a key of its own.
async fn certification_request(emulator: &Emulator, case: &str, common_name: &str) -> String {
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &root_ca_request(common_name))
        .await;
    let arn = string(&created, "CertificateAuthorityArn", case);
    // A test may wait for a due instant; a handler may not.
    wait_for_the_clock();
    let published = emulator
        .ok(
            case,
            "GetCertificateAuthorityCsr",
            &json!({"CertificateAuthorityArn": arn}),
        )
        .await;
    string(&published, "Csr", case)
}

/// The serial a certificate ARN carries, which is the serial the issuing CA allocated.
fn serial_in(certificate_arn: &str, case: &str) -> String {
    certificate_arn
        .rsplit_once("/certificate/")
        .map(|(_, serial)| serial.to_string())
        .unwrap_or_else(|| panic!("case {case:?}: no serial in {certificate_arn:?}"))
}

/// Have `ca_arn` issue a certificate over `csr`, and answer the serial it allocated.
///
/// Every call carries an idempotency token of its own, which is how a caller asks one CA
/// for SEVERAL certificates over one certification request: "If you change the idempotency
/// token for each call, Amazon Web Services Private CA recognizes that you are requesting
/// multiple certificates" (`IssueCertificateRequest$IdempotencyToken`). A byte-identical
/// request repeated without one is the earlier certificate replayed, not a second
/// certificate.
async fn issued_serial(emulator: &Emulator, case: &str, ca_arn: &str, csr: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let token = format!(
        "issuance-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let issued = emulator
        .ok(
            case,
            "IssueCertificate",
            &json!({
                "CertificateAuthorityArn": ca_arn,
                "Csr": blob(csr),
                "SigningAlgorithm": "SHA256WITHRSA",
                "Validity": {"Type": "DAYS", "Value": 30},
                "IdempotencyToken": token,
            }),
        )
        .await;
    serial_in(&string(&issued, "CertificateArn", case), case)
}

/// The serial number a PEM certificate carries, hex as this service spells one.
fn serial_of(certificate: &str, case: &str) -> String {
    let parsed = Certificate::from_pem(certificate)
        .unwrap_or_else(|error| panic!("case {case:?}: expected an X.509 certificate: {error}"));
    hex::encode(parsed.tbs_certificate.serial_number.as_bytes())
}

/// What the store holds about the revocation of the certificate `serial` names under
/// `ca_arn`: the reason recorded and the instant it was recorded at, or `None` while the
/// certificate is not revoked.
fn stored_revocation(
    emulator: &Emulator,
    ca_arn: &str,
    serial: &str,
    case: &str,
) -> Option<(String, SystemTime)> {
    let state = emulator.state();
    let state = state.lock();
    let record = state
        .certificate_authorities
        .get(ca_arn)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no CA at {ca_arn}"));
    let certificate = record
        .certificate_with_serial(serial)
        .unwrap_or_else(|| panic!("case {case:?}: the CA holds no certificate of serial {serial}"));
    certificate
        .revocation
        .as_ref()
        .map(|revocation| (revocation.reason.clone(), revocation.revoked_at))
}

/// The status the store holds for `ca_arn`.
fn stored_status(emulator: &Emulator, ca_arn: &str, case: &str) -> String {
    let state = emulator.state();
    let state = state.lock();
    state
        .certificate_authorities
        .get(ca_arn)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no CA at {ca_arn}"))
        .status
        .clone()
}

/// Every `RevocationReason` the model declares, each named through another spelling of the
/// serial: the revocation is recorded as the reason the request carried, and a second
/// revocation of the same certificate — spelled differently again — is the
/// already-completed refusal, which leaves the first call's record alone.
#[tokio::test]
async fn a_revocation_is_recorded_once_whatever_spelling_names_the_serial() {
    let emulator = Emulator::new();
    let (ca_arn, _certificate) = active_ca(&emulator, "revocation-reasons").await;
    let csr = certification_request(&emulator, "revocation-reasons", "leaf-subject").await;

    // One row per declared reason, cycling through every spelling above, so each reason
    // and each spelling is driven at least once.
    for (index, reason) in REVOCATION_REASON_VALUES.iter().enumerate() {
        let (spelling, spell) = SERIAL_SPELLINGS[index % SERIAL_SPELLINGS.len()];
        let case = format!("{reason} named by a serial {spelling}");
        let serial = issued_serial(&emulator, &case, &ca_arn, &csr).await;
        assert!(
            stored_revocation(&emulator, &ca_arn, &serial, &case).is_none(),
            "case {case:?}: expected a freshly issued certificate to carry no revocation"
        );

        let answered = emulator
            .ok(
                &case,
                "RevokeCertificate",
                &revocation(&ca_arn, &spell(&serial), reason),
            )
            .await;
        let members = answered.as_object().map_or(0, serde_json::Map::len);
        assert_eq!(
            members, 0,
            "case {case:?}: expected an empty response body, got {answered:?}"
        );

        let Some((recorded, revoked_at)) = stored_revocation(&emulator, &ca_arn, &serial, &case)
        else {
            panic!("case {case:?}: expected the revocation to be recorded on the certificate")
        };
        assert_eq!(
            &recorded, reason,
            "case {case:?}: expected the reason the request named, got {recorded:?}"
        );

        // The second revocation of that certificate, through yet another spelling of the
        // same value and naming another reason: refused, and nothing it named changes.
        let (other_spelling, other_spell) = SERIAL_SPELLINGS[(index + 1) % SERIAL_SPELLINGS.len()];
        let repeat = format!("{case}, revoked again {other_spelling}");
        let refused = emulator
            .refused(
                &repeat,
                "RevokeCertificate",
                &revocation(&ca_arn, &other_spell(&serial), "KEY_COMPROMISE"),
                "RequestAlreadyProcessedException",
            )
            .await;
        let message = error_message(&refused, &repeat);
        assert_eq!(
            message, CERTIFICATE_ALREADY_REVOKED_MESSAGE,
            "case {repeat:?}: expected the empty message this refusal carries, got {message:?}"
        );
        let still = stored_revocation(&emulator, &ca_arn, &serial, &repeat);
        assert_eq!(
            still,
            Some((recorded.clone(), revoked_at)),
            "case {repeat:?}: expected the first call's revocation to stand, got {still:?}"
        );
    }
}

/// A revoked certificate is still the certificate the CA issued: revocation records that
/// it may no longer be trusted, it does not withdraw the body the CA signed.
#[tokio::test]
async fn a_revoked_certificate_is_still_served() {
    let emulator = Emulator::new();
    let case = "a revoked certificate";
    let (ca_arn, _certificate) = active_ca(&emulator, "revoked-still-served").await;
    let csr = certification_request(&emulator, case, "served-leaf").await;
    let serial = issued_serial(&emulator, case, &ca_arn, &csr).await;
    let certificate_arn = format!("{ca_arn}/certificate/{serial}");

    emulator
        .ok(
            case,
            "RevokeCertificate",
            &revocation(&ca_arn, &serial, "CESSATION_OF_OPERATION"),
        )
        .await;
    // Issuance completes on the clock, as it does for a certificate nobody revoked.
    wait_for_the_clock();
    let served = emulator
        .ok(
            case,
            "GetCertificate",
            &json!({
                "CertificateAuthorityArn": ca_arn,
                "CertificateArn": certificate_arn,
            }),
        )
        .await;
    let body = string(&served, "Certificate", case);
    assert!(
        body.contains("-----BEGIN CERTIFICATE-----"),
        "case {case:?}: expected the revoked certificate to still be served, got {body:?}"
    );
}

/// A serial no certificate of the CA carries names no certificate at all — however the
/// serial is spelled, and whichever other authority's certificate carries it.
#[tokio::test]
async fn a_serial_no_certificate_of_the_ca_carries_is_not_found() {
    let emulator = Emulator::new();
    let case = "unknown serials";
    let (ca_arn, _certificate) = active_ca(&emulator, "unknown-serials").await;
    let (other_arn, _other_certificate) = active_ca(&emulator, "another-authority").await;
    let csr = certification_request(&emulator, case, "unknown-serial-leaf").await;
    let mine = issued_serial(&emulator, case, &ca_arn, &csr).await;
    let theirs = issued_serial(&emulator, case, &other_arn, &csr).await;

    let rows: Vec<(String, String)> = vec![
        (
            "another authority's certificate".to_string(),
            theirs.clone(),
        ),
        (
            "a hexadecimal serial nothing was issued under".to_string(),
            "0a1b2c3d4e5f6071".to_string(),
        ),
        ("zero".to_string(), "0".to_string()),
        (
            "the empty serial the member's length allows".to_string(),
            String::new(),
        ),
        (
            "a serial that is not hexadecimal at all".to_string(),
            "not-a-serial".to_string(),
        ),
        (
            "separators with no digit between them".to_string(),
            ":::".to_string(),
        ),
        (
            "octets separated by spaces rather than by the colon openssl prints".to_string(),
            join_octets(&mine, ' '),
        ),
        (
            "one digit more than the issued serial".to_string(),
            format!("{mine}0"),
        ),
    ];

    for (case, serial) in &rows {
        let refused = emulator
            .refused(
                case,
                "RevokeCertificate",
                &revocation(&ca_arn, serial, "UNSPECIFIED"),
                "ResourceNotFoundException",
            )
            .await;
        let message = error_message(&refused, case);
        assert_eq!(
            message, CERTIFICATE_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the service's certificate not-found sentence, got \
             {message:?}"
        );
    }

    // None of those refusals revoked anything: the certificate the CA DID issue still
    // revokes, and the other authority's is untouched.
    let case = "the certificate the CA issued";
    emulator
        .ok(
            case,
            "RevokeCertificate",
            &revocation(&ca_arn, &mine, "SUPERSEDED"),
        )
        .await;
    let recorded = stored_revocation(&emulator, &ca_arn, &mine, case);
    assert!(
        recorded.is_some_and(|(reason, _)| reason == "SUPERSEDED"),
        "case {case:?}: expected the revocation to be recorded after the refusals"
    );
    let case = "the other authority's certificate";
    assert!(
        stored_revocation(&emulator, &other_arn, &theirs, case).is_none(),
        "case {case:?}: expected a revocation of one authority to leave another's alone"
    );
}

/// The CA's ARN resolves before the serial does: an ARN that names no private CA resource
/// is the ARN refusal, and a well-formed ARN naming no authority this service holds is a
/// not-found about the CA rather than about a certificate.
#[tokio::test]
async fn the_cas_arn_resolves_before_the_serial() {
    let emulator = Emulator::new();
    // An authority exists, so each refusal is about the ARN in the request.
    let (_existing, _certificate) = active_ca(&emulator, "arn-before-serial").await;

    let rows: Vec<(&str, String, &str, &str)> = vec![
        (
            "an ARN naming no private CA resource",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority"),
            "InvalidArnException",
            INVALID_ARN_MESSAGE,
        ),
        (
            "an ARN of another resource type",
            format!("arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate/0123456789abcdef"),
            "InvalidArnException",
            INVALID_ARN_MESSAGE,
        ),
        (
            "an id no authority was minted with",
            format!(
                "arn:aws:acm-pca:{REGION}:{ACCOUNT}:certificate-authority/\
                 7b9f4c1e-0d2a-4f6b-9c3e-5a8d1f2b4c60"
            ),
            "ResourceNotFoundException",
            CA_NOT_FOUND_MESSAGE,
        ),
        (
            "another account's authority",
            format!("arn:aws:acm-pca:{REGION}:111122223333:certificate-authority/abcdef"),
            "ResourceNotFoundException",
            CA_NOT_FOUND_MESSAGE,
        ),
    ];

    for (case, arn, code, expected) in &rows {
        let refused = emulator
            .refused(
                case,
                "RevokeCertificate",
                &revocation(arn, "0a1b2c3d", "KEY_COMPROMISE"),
                code,
            )
            .await;
        let message = error_message(&refused, case);
        assert_eq!(
            &message, expected,
            "case {case:?}: expected the sentence of the stage that refused, got {message:?}"
        );
    }
}

/// Every `CertificateAuthorityStatus` the model declares, against whether a CA resting in
/// it revokes a certificate it HOLDS: "ACTIVE — the CA is active and can be used to issue
/// and revoke certificates", a disabled CA "can no longer issue certificates, but it
/// continues to revoke them and to publish CRLs", and every other status either has no
/// certificate of its own yet, cannot sign with it any more, or is waiting to be restored
/// (the private CA lifecycle).
///
/// One table, read by every suite below that drives a status, so the two cannot drift when
/// the model gains a member.
const REVOCATION_BY_STATUS: &[(&str, bool)] = &[
    (STATUS_ACTIVE, true),
    (STATUS_DISABLED, true),
    (STATUS_CREATING, false),
    (STATUS_PENDING_CERTIFICATE, false),
    (STATUS_EXPIRED, false),
    (STATUS_FAILED, false),
    (STATUS_DELETED, false),
];

/// Assert [`REVOCATION_BY_STATUS`] spans the whole enum, so a member the model gains cannot
/// go untested.
fn the_table_spans_the_status_enum() {
    let mut covered: Vec<&str> = REVOCATION_BY_STATUS
        .iter()
        .map(|(status, _)| *status)
        .collect();
    covered.sort_unstable();
    let mut declared: Vec<&str> = CERTIFICATE_AUTHORITY_STATUS_VALUES.to_vec();
    declared.sort_unstable();
    assert_eq!(
        covered, declared,
        "the table must span every CertificateAuthorityStatus the model declares, got \
         {covered:?} against {declared:?}"
    );
}

/// Every `CertificateAuthorityStatus` the model declares, against whether a CA in it
/// revokes one of its certificates — and a refused revocation leaves both the certificate
/// and the CA exactly as they were.
#[tokio::test]
async fn revocation_is_allowed_only_from_the_statuses_a_ca_can_still_sign_in() {
    let emulator = Emulator::new();
    let csr = certification_request(&emulator, "status table", "status-leaf").await;

    the_table_spans_the_status_enum();

    for (index, (status, accepted)) in REVOCATION_BY_STATUS.iter().enumerate() {
        let case = format!("revocation from {status}");
        let (ca_arn, _certificate) = active_ca(&emulator, &format!("status-{index}")).await;
        let serial = issued_serial(&emulator, &case, &ca_arn, &csr).await;
        force_status(&emulator, &ca_arn, status, &case);

        if *accepted {
            emulator
                .ok(
                    &case,
                    "RevokeCertificate",
                    &revocation(&ca_arn, &serial, "KEY_COMPROMISE"),
                )
                .await;
            let recorded = stored_revocation(&emulator, &ca_arn, &serial, &case);
            assert!(
                recorded.is_some_and(|(reason, _)| reason == "KEY_COMPROMISE"),
                "case {case:?}: expected the revocation to be recorded"
            );
        } else {
            let refused = emulator
                .refused(
                    &case,
                    "RevokeCertificate",
                    &revocation(&ca_arn, &serial, "KEY_COMPROMISE"),
                    "InvalidStateException",
                )
                .await;
            let message = error_message(&refused, &case);
            assert_eq!(
                message,
                not_in_state_to_have_issued_message(&ca_arn),
                "case {case:?}: expected the sentence naming the CA the request carried, got \
                 {message:?}"
            );
            let recorded = stored_revocation(&emulator, &ca_arn, &serial, &case);
            assert!(
                recorded.is_none(),
                "case {case:?}: a refused revocation must record none, got {recorded:?}"
            );
            let held = stored_status(&emulator, &ca_arn, &case);
            assert_eq!(
                &held, status,
                "case {case:?}: a refused revocation must leave the CA's status alone, got \
                 {held:?}"
            );
        }
    }
}

/// The serial a revocation names is resolved on the CA BEFORE the CA's own lifecycle
/// status is judged: a serial the CA carries no certificate for is the certificate's
/// not-found refusal in every status the CA can rest in, and the state refusal is reserved
/// for a revocation of a certificate the CA does hold.
///
/// The rows differ along both of the things the rule generalises over: the status the CA
/// rests in (every member `CertificateAuthorityStatus` declares) and the way the serial
/// names no certificate of that CA (a value nothing was issued under, no value at all, a
/// value that is not hexadecimal, a sibling value of the one the CA holds, and another
/// authority's certificate).
#[tokio::test]
async fn an_unknown_serial_is_not_found_whatever_status_the_ca_rests_in() {
    let emulator = Emulator::new();
    let (ca_arn, _certificate) = active_ca(&emulator, "unknown-serial-statuses").await;
    let (other_arn, _other_certificate) = active_ca(&emulator, "another-authority").await;
    let csr = certification_request(&emulator, "unknown serials by status", "by-status-leaf").await;
    let theirs = issued_serial(&emulator, "another authority", &other_arn, &csr).await;

    the_table_spans_the_status_enum();

    for (status, accepted) in REVOCATION_BY_STATUS {
        // A certificate of this CA per row, so the row that accepts a revocation does not
        // leave the next one asking about a certificate already revoked.
        let row = format!("a revocation against a {status} CA");
        let held = issued_serial(&emulator, &row, &ca_arn, &csr).await;
        force_status(&emulator, &ca_arn, status, &row);

        // Every way a serial can name no certificate of this CA.
        let unknown: Vec<(String, String)> = vec![
            (
                "a hexadecimal serial nothing was issued under".to_string(),
                "0a1b2c3d4e5f6071".to_string(),
            ),
            (
                "the empty serial the member's length allows".to_string(),
                String::new(),
            ),
            (
                "a serial that is not hexadecimal at all".to_string(),
                "not-a-serial".to_string(),
            ),
            (
                "one digit more than the serial the CA holds".to_string(),
                format!("{held}0"),
            ),
            (
                "another authority's certificate".to_string(),
                theirs.clone(),
            ),
        ];

        for (named, serial) in &unknown {
            let case = format!("{row} naming {named}");
            let refused = emulator
                .refused(
                    &case,
                    "RevokeCertificate",
                    &revocation(&ca_arn, serial, "UNSPECIFIED"),
                    "ResourceNotFoundException",
                )
                .await;
            let message = error_message(&refused, &case);
            assert_eq!(
                message, CERTIFICATE_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the service's certificate not-found sentence, got \
                 {message:?}"
            );
        }

        // None of those refusals touched anything: neither the certificate the CA does
        // hold nor the status it rests in.
        let recorded = stored_revocation(&emulator, &ca_arn, &held, &row);
        assert!(
            recorded.is_none(),
            "case {row:?}: a refused revocation must record none, got {recorded:?}"
        );
        let other = stored_revocation(&emulator, &other_arn, &theirs, &row);
        assert!(
            other.is_none(),
            "case {row:?}: a refusal must leave another authority's certificate alone, got \
             {other:?}"
        );
        let held_status = stored_status(&emulator, &ca_arn, &row);
        assert_eq!(
            &held_status, status,
            "case {row:?}: a refused revocation must leave the CA's status alone, got \
             {held_status:?}"
        );

        // The same request for the serial this CA DOES hold is where the status decides:
        // accepted where the CA can still sign a revocation, and the state refusal
        // naming the CA everywhere else.
        let case = format!("{row} naming the serial it holds");
        if *accepted {
            emulator
                .ok(
                    &case,
                    "RevokeCertificate",
                    &revocation(&ca_arn, &held, "KEY_COMPROMISE"),
                )
                .await;
            let recorded = stored_revocation(&emulator, &ca_arn, &held, &case);
            assert!(
                recorded.is_some_and(|(reason, _)| reason == "KEY_COMPROMISE"),
                "case {case:?}: expected the revocation to be recorded"
            );
        } else {
            let refused = emulator
                .refused(
                    &case,
                    "RevokeCertificate",
                    &revocation(&ca_arn, &held, "KEY_COMPROMISE"),
                    "InvalidStateException",
                )
                .await;
            let message = error_message(&refused, &case);
            assert_eq!(
                message,
                not_in_state_to_have_issued_message(&ca_arn),
                "case {case:?}: expected the sentence naming the CA the request carried, got \
                 {message:?}"
            );
        }

        // The next row starts from a CA that can issue the certificate it needs.
        force_status(&emulator, &ca_arn, STATUS_ACTIVE, &row);
    }
}

/// The documented prohibition on revoking a root CA's own self-signed certificate is about
/// WHICH certificate the request names, so it is the answer in every status the CA rests in:
/// a certificate that can never be revoked is not one a change of the CA's status makes
/// revocable.
///
/// The rows differ along the status the CA rests in (every member
/// `CertificateAuthorityStatus` declares) and the spelling the serial arrives in.
#[tokio::test]
async fn a_root_cas_own_certificate_is_refused_in_every_status() {
    let emulator = Emulator::new();
    let case = "a root CA activated with its own certificate";
    let (ca_arn, certificate) = active_ca(&emulator, "own-certificate-statuses").await;
    let own = serial_of(&certificate, case);

    // The whole enum, so a member the model gains cannot go untested.
    let mut statuses: Vec<&str> = CERTIFICATE_AUTHORITY_STATUS_VALUES.to_vec();
    statuses.sort_unstable();

    for (index, status) in statuses.iter().enumerate() {
        let (spelling, spell) = SERIAL_SPELLINGS[index % SERIAL_SPELLINGS.len()];
        let case = format!("the CA's own certificate under {status}, named {spelling}");
        force_status(&emulator, &ca_arn, status, &case);
        let refused = emulator
            .refused(
                &case,
                "RevokeCertificate",
                &revocation(&ca_arn, &spell(&own), "KEY_COMPROMISE"),
                "InvalidRequestException",
            )
            .await;
        let message = error_message(&refused, &case);
        assert_eq!(
            message, ROOT_SELF_SIGNED_REVOCATION_MESSAGE,
            "case {case:?}: expected the refusal's own sentence, got {message:?}"
        );
        let held = stored_status(&emulator, &ca_arn, &case);
        assert_eq!(
            &held, status,
            "case {case:?}: a refused revocation must leave the CA's status alone, got \
             {held:?}"
        );
    }
}

/// The certificate a CA holds as its OWN is the one certificate it never revokes, however
/// that certificate reached it — while a CA whose certificate was signed by its parent
/// holds no such certificate at all, so the same serial is simply one it did not issue.
#[tokio::test]
async fn a_root_cas_own_self_signed_certificate_is_never_revoked() {
    let emulator = Emulator::new();

    // A root CA activated through the documented procedure: it signs its OWN certification
    // request under the root CA certificate template, and imports what comes back.
    let case = "a root CA that signed its own certificate";
    let created = emulator
        .ok(
            case,
            "CreateCertificateAuthority",
            &root_ca_request("self-signing-root"),
        )
        .await;
    let root = string(&created, "CertificateAuthorityArn", case);
    wait_for_the_clock();
    let published = emulator
        .ok(
            case,
            "GetCertificateAuthorityCsr",
            &json!({"CertificateAuthorityArn": root}),
        )
        .await;
    let root_csr = string(&published, "Csr", case);
    let issued = emulator
        .ok(
            case,
            "IssueCertificate",
            &json!({
                "CertificateAuthorityArn": root,
                "Csr": blob(&root_csr),
                "SigningAlgorithm": "SHA256WITHRSA",
                "TemplateArn": ROOT_CA_TEMPLATE,
                "Validity": {"Type": "YEARS", "Value": 10},
            }),
        )
        .await;
    let root_certificate_arn = string(&issued, "CertificateArn", case);
    let root_serial = serial_in(&root_certificate_arn, case);
    wait_for_the_clock();
    let served = emulator
        .ok(
            case,
            "GetCertificate",
            &json!({
                "CertificateAuthorityArn": root,
                "CertificateArn": root_certificate_arn,
            }),
        )
        .await;
    let root_certificate = string(&served, "Certificate", case);
    emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({
                "CertificateAuthorityArn": root,
                "Certificate": blob(&root_certificate),
            }),
        )
        .await;

    // A root CA activated with a certificate built outside this emulator: its own all the
    // same, so the rule is about the certificate the CA HOLDS rather than about which call
    // produced it.
    let case = "a root CA activated with an imported certificate";
    let (imported_root, imported_certificate) = active_ca(&emulator, "imported-root").await;
    let imported_serial = serial_of(&imported_certificate, case);

    // A subordinate CA's certificate is signed by its parent, which is the CA that can
    // revoke it; the subordinate itself never issued it.
    let case = "a subordinate CA";
    let mut body = root_ca_request("subordinate-authority");
    body["CertificateAuthorityType"] = json!("SUBORDINATE");
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &body)
        .await;
    let subordinate = string(&created, "CertificateAuthorityArn", case);
    wait_for_the_clock();
    let subordinate_certificate =
        own_ca_certificate(&emulator, case, &subordinate).await;
    emulator
        .ok(
            case,
            "ImportCertificateAuthorityCertificate",
            &json!({
                "CertificateAuthorityArn": subordinate,
                "Certificate": blob(&subordinate_certificate),
            }),
        )
        .await;
    let subordinate_serial = serial_of(&subordinate_certificate, case);

    // (case, the CA asked, the serial named, the code it is refused with)
    let rows: Vec<(&str, &String, String, &str)> = vec![
        (
            "the certificate a root CA signed itself",
            &root,
            root_serial.clone(),
            "InvalidRequestException",
        ),
        (
            "that same value as colon-separated octets",
            &root,
            join_octets(&root_serial, ':'),
            "InvalidRequestException",
        ),
        (
            "the certificate a root CA imported as its own",
            &imported_root,
            imported_serial,
            "InvalidRequestException",
        ),
        (
            "the certificate a subordinate CA holds, signed by its parent",
            &subordinate,
            subordinate_serial,
            "ResourceNotFoundException",
        ),
    ];

    for (case, ca_arn, serial, code) in &rows {
        let refused = emulator
            .refused(
                case,
                "RevokeCertificate",
                &revocation(ca_arn, serial, "CERTIFICATE_AUTHORITY_COMPROMISE"),
                code,
            )
            .await;
        let message = error_message(&refused, case);
        let expected = if *code == "InvalidRequestException" {
            ROOT_SELF_SIGNED_REVOCATION_MESSAGE
        } else {
            CERTIFICATE_NOT_FOUND_MESSAGE
        };
        assert_eq!(
            message, expected,
            "case {case:?}: expected the sentence of the refusal, got {message:?}"
        );
    }

    // The root's own certificate is one of the certificates it issued, and the refusal
    // left it unrevoked.
    let case = "the root CA's own certificate after the refusal";
    let recorded = stored_revocation(&emulator, &root, &root_serial, case);
    assert!(
        recorded.is_none(),
        "case {case:?}: expected no revocation on the CA's own certificate, got {recorded:?}"
    );

    // The positive control: an end-entity certificate of the SAME root CA revokes.
    let case = "an end-entity certificate of the same root CA";
    let leaf_csr = certification_request(&emulator, case, "leaf-of-root").await;
    let leaf = issued_serial(&emulator, case, &root, &leaf_csr).await;
    emulator
        .ok(case, "RevokeCertificate", &revocation(&root, &leaf, "SUPERSEDED"))
        .await;
    let recorded = stored_revocation(&emulator, &root, &leaf, case);
    assert!(
        recorded.is_some_and(|(reason, _)| reason == "SUPERSEDED"),
        "case {case:?}: expected an end-entity certificate of the root to revoke"
    );
}
