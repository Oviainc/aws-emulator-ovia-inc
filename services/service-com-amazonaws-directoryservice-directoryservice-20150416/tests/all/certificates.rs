// SPDX-License-Identifier: CC0-1.0

//! The Directory Service certificate registrations, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body, so
//! nothing here reaches into the state struct and a response that is not computed from
//! persisted state cannot pass.
//!
//! The rule under test is that `RegisterCertificate` reports back the CERTIFICATE's own
//! values: every row of the table registers a *different* self-signed certificate —
//! built here with `rcgen`, so each row's subject common name and `notAfter` bound are
//! chosen by the test and known independently of the emulator — and asserts that
//! `DescribeCertificate` reports exactly those, that the registration is identified by a
//! freshly minted `CertificateId`, and that it settles out of the transient state the
//! clock owes a transition from. The remaining suites assert the invariants: a
//! certificate this emulator cannot decode is refused, a directory registers one
//! certificate once, a registration needs a directory that is in service, and the
//! per-directory guard holds.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, ok, refused, service, string, PAST_ONE_SPAN,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The `CertificateState` a registration carries until the transition the register owes
/// falls due, and the one it reaches after: the first two values of the modelled
/// `CertificateState` enum (`smithy_query(query="shape",shape="CertificateState")` lists
/// `Registering`, `Registered`, `RegisterFailed`, `Deregistering`, `Deregistered`,
/// `DeregisterFailed`).
pub(crate) const STATE_REGISTERING: &str = "Registering";
pub(crate) const STATE_REGISTERED: &str = "Registered";

/// The values of the modelled `CertificateType` enum
/// (`smithy_query(query="shape",shape="CertificateType")`), and the one a request that
/// names none performs: "The default value is ClientLDAPS."
pub(crate) const TYPE_CLIENT_LDAPS: &str = "ClientLDAPS";
pub(crate) const TYPE_CLIENT_CERT_AUTH: &str = "ClientCertAuth";

/// This emulator's documented per-directory registration guard
/// (`state::MAX_CERTIFICATES_PER_DIRECTORY`).
pub(crate) const CERTIFICATE_GUARD: usize = 10;

/// A self-signed certificate, PEM-encoded, with `common_name` as its subject's common
/// name (none at all when `common_name` is `None`) and `(year, month, day)` midnight UTC
/// as its `notAfter` bound.
pub(crate) fn certificate_pem(
    common_name: Option<&str>,
    not_after: (i32, u8, u8),
) -> (String, i64) {
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new())
        .expect("the certificate parameters are well formed");
    let mut subject = rcgen::DistinguishedName::new();
    if let Some(name) = common_name {
        subject.push(rcgen::DnType::CommonName, name);
    }
    params.distinguished_name = subject;
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    let (year, month, day) = not_after;
    let expiry = rcgen::date_time_ymd(year, month, day);
    params.not_after = expiry;
    let key = rcgen::KeyPair::generate().expect("a key pair");
    let certificate = params.self_signed(&key).expect("a self-signed certificate");
    (certificate.pem(), expiry.unix_timestamp())
}

/// The modelled `CertificateId` pattern, `^c-[0-9a-f]{10}$`, as an assertion.
pub(crate) fn assert_certificate_id(id: &str, case: &str) {
    let suffix = id.strip_prefix("c-").unwrap_or_default();
    assert!(
        suffix.len() == 10
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "case {case:?}: expected a CertificateId matching ^c-[0-9a-f]{{10}}$, got {id:?}"
    );
}

/// `RegisterCertificate` with `body`, asserting it was accepted and returning the id.
pub(crate) async fn register(router: &MultiServiceRouter, body: Value, case: &str) -> String {
    let answer = ok(router, "RegisterCertificate", body, case).await;
    let id = string(&answer, "CertificateId", case);
    assert_certificate_id(&id, case);
    id
}

/// The `Certificate` a `DescribeCertificate` answer reports.
pub(crate) async fn describe(
    router: &MultiServiceRouter,
    directory_id: &str,
    certificate_id: &str,
    case: &str,
) -> Value {
    let answer = ok(
        router,
        "DescribeCertificate",
        json!({ "DirectoryId": directory_id, "CertificateId": certificate_id }),
        case,
    )
    .await;
    answer["Certificate"].clone()
}

/// Seconds since the epoch, the unit an `awsJson1_1` timestamp is rendered in.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A Managed Microsoft AD directory past the stage transition its create owes, so it is
/// in service for the registrations the cases make.
///
/// Managed Microsoft AD is one of the directory types client-side LDAPS is offered for
/// (`state::LDAPS_SUPPORTED_DIRECTORY_TYPES`), which is the function a `ClientLDAPS`
/// certificate performs, so it is the directory those registrations are made with.
pub(crate) async fn directory_in_service(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
) -> String {
    let id = create_microsoft_ad(router, minimal_microsoft_ad(name), case).await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    id
}

/// An AD CONNECTOR directory past that same transition.
///
/// Client authentication — what a `ClientCertAuth` certificate performs — is offered for
/// this directory type and no other (`state::CLIENT_AUTHENTICATION_DIRECTORY_TYPES`), so
/// every row that registers such a certificate is made against one of these, while the
/// rows about a certificate securing LDAP calls use [`directory_in_service`].
pub(crate) async fn connector_in_service(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
) -> String {
    let id = crate::ad_connector::connect_directory(
        router,
        crate::ad_connector::minimal_connect(name),
        case,
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    id
}

/// One row of the registration table: a certificate to register and what the stored
/// registration must then report.
struct CertificateCase {
    case: &'static str,
    /// The subject common name the certificate itself states, if any.
    common_name: Option<&'static str>,
    /// The `notAfter` bound the certificate itself states.
    not_after: (i32, u8, u8),
    /// The `Type` member the request carries, if any.
    requested_type: Option<&'static str>,
    /// The `ClientCertAuthSettings` the request carries, if any.
    settings: Option<Value>,
    /// The `Type` the registration must report: the requested one, or the default.
    expected_type: &'static str,
}

#[tokio::test]
async fn a_registered_certificate_reports_the_certificates_own_values() {
    let router = service();
    let directory_id = directory_in_service(&router, "ldaps.example.com", "registration").await;
    // Client authentication is offered for AD Connector alone, so the rows that register a
    // certificate performing it are made against a directory of that type.
    let connector_id = connector_in_service(&router, "smartcard.example.com", "registration").await;

    // The rows differ along everything a registration varies in: the common name the
    // certificate states (several forms, and one certificate with no common name at
    // all), the expiry the certificate states (near and far), the function the
    // certificate performs (both modelled `CertificateType` values, and a request that
    // names none), and whether client certificate authentication settings came with it.
    let cases = vec![
        CertificateCase {
            case: "a secure-LDAP certificate, type named",
            common_name: Some("ldaps.example.com"),
            not_after: (2030, 1, 2),
            requested_type: Some(TYPE_CLIENT_LDAPS),
            settings: None,
            expected_type: TYPE_CLIENT_LDAPS,
        },
        CertificateCase {
            case: "no type named: the documented default",
            common_name: Some("corp.example.net"),
            not_after: (2027, 6, 15),
            requested_type: None,
            settings: None,
            expected_type: TYPE_CLIENT_LDAPS,
        },
        CertificateCase {
            case: "client certificate authentication, with an OCSP responder",
            common_name: Some("smartcard.example.org"),
            not_after: (2045, 12, 31),
            requested_type: Some(TYPE_CLIENT_CERT_AUTH),
            settings: Some(json!({ "OCSPUrl": "http://ocsp.example.org/status" })),
            expected_type: TYPE_CLIENT_CERT_AUTH,
        },
        CertificateCase {
            case: "a common name of one character",
            common_name: Some("a"),
            not_after: (2026, 2, 28),
            requested_type: Some(TYPE_CLIENT_CERT_AUTH),
            settings: None,
            expected_type: TYPE_CLIENT_CERT_AUTH,
        },
        CertificateCase {
            case: "a certificate whose subject states no common name",
            common_name: None,
            not_after: (2031, 11, 9),
            requested_type: None,
            settings: None,
            expected_type: TYPE_CLIENT_LDAPS,
        },
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut registered: Vec<(&CertificateCase, String, String, i64)> = Vec::new();

    for row in &cases {
        let case = row.case;
        // The directory a row is registered with is the one its certificate's function is
        // offered for.
        let target = if row.expected_type == TYPE_CLIENT_CERT_AUTH {
            &connector_id
        } else {
            &directory_id
        };
        let (pem, expires_at) = certificate_pem(row.common_name, row.not_after);
        let mut request = json!({ "DirectoryId": target, "CertificateData": pem });
        if let Some(wanted) = row.requested_type {
            request["Type"] = json!(wanted);
        }
        if let Some(settings) = &row.settings {
            request["ClientCertAuthSettings"] = settings.clone();
        }

        let registered_from = epoch_seconds(SystemTime::now());
        let certificate_id = register(&router, request, case).await;
        let registered_until = epoch_seconds(SystemTime::now());
        assert!(
            minted.insert(certificate_id.clone()),
            "case {case:?}: expected a fresh CertificateId, got the already minted \
             {certificate_id:?}"
        );

        let reported = describe(&router, target, &certificate_id, case).await;
        assert_eq!(
            reported["CertificateId"],
            json!(&certificate_id),
            "case {case:?}: expected the registration to name its own id, got {reported}"
        );
        // The register answers while the registration is still being made, so the
        // transient state is what a caller acting straight after it observes.
        assert_eq!(
            reported["State"],
            json!(STATE_REGISTERING),
            "case {case:?}: expected the transient state right after the register, got {reported}"
        );
        assert_eq!(
            reported["CommonName"].as_str(),
            row.common_name,
            "case {case:?}: expected the certificate's own common name, got {reported}"
        );
        assert_eq!(
            reported["ExpiryDateTime"].as_f64(),
            Some(expires_at as f64),
            "case {case:?}: expected the certificate's own notAfter bound \
             ({expires_at}), got {reported}"
        );
        assert_eq!(
            reported["Type"],
            json!(row.expected_type),
            "case {case:?}: expected the function the certificate performs, got {reported}"
        );
        assert_eq!(
            reported.get("ClientCertAuthSettings").cloned(),
            row.settings.clone(),
            "case {case:?}: expected the client certificate authentication settings to \
             round-trip, got {reported}"
        );
        let registered_at = reported["RegisteredDateTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a RegisteredDateTime timestamp, got {reported}")
        });
        assert!(
            (registered_from - 1.0..=registered_until + 1.0).contains(&registered_at),
            "case {case:?}: expected RegisteredDateTime within the registering window \
             [{registered_from}, {registered_until}], got {registered_at}"
        );

        // A read does not change what the next caller sees.
        let again = describe(&router, target, &certificate_id, case).await;
        assert_eq!(
            again, reported,
            "case {case:?}: expected a repeated DescribeCertificate to report the same \
             registration, got {again}"
        );

        registered.push((row, target.clone(), certificate_id, expires_at));
    }

    // The transition the register owed has fallen due: every registration has settled,
    // and nothing else about it moved.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (row, target, certificate_id, expires_at) in &registered {
        let case = row.case;
        let settled = describe(&router, target, certificate_id, case).await;
        assert_eq!(
            settled["State"],
            json!(STATE_REGISTERED),
            "case {case:?}: expected the settled state once the transition fell due, got {settled}"
        );
        assert_eq!(
            settled["CommonName"].as_str(),
            row.common_name,
            "case {case:?}: expected the common name to survive the transition, got {settled}"
        );
        assert_eq!(
            settled["ExpiryDateTime"].as_f64(),
            Some(*expires_at as f64),
            "case {case:?}: expected the expiry to survive the transition, got {settled}"
        );
    }
}

#[tokio::test]
async fn a_certificate_the_emulator_cannot_decode_is_refused() {
    let router = service();
    let directory_id = directory_in_service(&router, "invalid.example.com", "invalid").await;

    let (valid, _) = certificate_pem(Some("valid.example.com"), (2030, 5, 5));
    // A PEM body line this certificate's own encoding produced, dropped: the framing is
    // still a PEM certificate block, but what it carries is no longer a certificate.
    let truncated: String = valid
        .lines()
        .enumerate()
        .filter(|(index, _)| *index != 1)
        .map(|(_, line)| format!("{line}\n"))
        .collect();
    let private_key = rcgen::KeyPair::generate()
        .expect("a key pair")
        .serialize_pem();

    // The rows differ along the way the input fails to be a PEM-encoded X.509
    // certificate: not PEM at all, PEM framing around something that is not base64,
    // around base64 that is not DER, around a truncated certificate, and a PEM of
    // another kind of object entirely.
    let cases: Vec<(&str, String)> = vec![
        ("not PEM at all", "this is not a certificate".to_string()),
        (
            "PEM framing around something that is not base64",
            "-----BEGIN CERTIFICATE-----\nnot base64 at all!\n-----END CERTIFICATE-----\n"
                .to_string(),
        ),
        (
            "PEM framing around base64 that is not DER",
            "-----BEGIN CERTIFICATE-----\naGVsbG8=\n-----END CERTIFICATE-----\n".to_string(),
        ),
        ("a certificate with a body line missing", truncated),
        ("a PEM of another kind of object", private_key),
        (
            "only the PEM framing",
            "-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n".to_string(),
        ),
    ];

    for (case, certificate_data) in &cases {
        refused(
            &router,
            "RegisterCertificate",
            json!({ "DirectoryId": &directory_id, "CertificateData": certificate_data }),
            "InvalidCertificateException",
            case,
        )
        .await;
    }

    // The refusals stored nothing, so the certificate that DOES decode is registered as
    // the directory's first one and reports its own values.
    let certificate_id = register(
        &router,
        json!({ "DirectoryId": &directory_id, "CertificateData": &valid }),
        "a certificate that decodes",
    )
    .await;
    let reported = describe(
        &router,
        &directory_id,
        &certificate_id,
        "a certificate that decodes",
    )
    .await;
    assert_eq!(
        reported["CommonName"],
        json!("valid.example.com"),
        "case \"a certificate that decodes\": expected the certificate's own common name, \
         got {reported}"
    );

    // ...and they consumed nothing either: the id the first accepted registration is
    // identified by is the one it gets on a service where no call was refused at all.
    let untouched = service();
    let fresh_directory = directory_in_service(&untouched, "invalid.example.com", "invalid").await;
    let first_id = register(
        &untouched,
        json!({ "DirectoryId": &fresh_directory, "CertificateData": &valid }),
        "no refusal before the first registration",
    )
    .await;
    assert_eq!(
        certificate_id, first_id,
        "case \"a certificate that decodes\": expected the refused registers to consume no \
         CertificateId, got {certificate_id} where an unrefused service minted {first_id}"
    );
}

#[tokio::test]
async fn a_directory_registers_one_certificate_once() {
    let router = service();
    let first = directory_in_service(&router, "one.example.com", "duplicates").await;
    let second = directory_in_service(&router, "two.example.com", "duplicates").await;

    let shared_name = "shared.example.com";
    let (certificate, _) = certificate_pem(Some(shared_name), (2029, 3, 4));
    // A DIFFERENT certificate stating the SAME common name: what a directory registers
    // at most one of is the certificate, not the name it states.
    let (namesake, _) = certificate_pem(Some(shared_name), (2029, 3, 4));
    assert_ne!(
        certificate, namesake,
        "case \"duplicates\": the two rows must be different certificates"
    );

    let registered = register(
        &router,
        json!({ "DirectoryId": &first, "CertificateData": &certificate }),
        "the first registration",
    )
    .await;

    // The same certificate again, with and without the members the first call carried.
    for (case, body) in [
        (
            "the same certificate again",
            json!({ "DirectoryId": &first, "CertificateData": &certificate }),
        ),
        (
            "the same certificate again, with the function it performs named",
            json!({
                "DirectoryId": &first,
                "CertificateData": &certificate,
                "Type": TYPE_CLIENT_LDAPS
            }),
        ),
    ] {
        refused(
            &router,
            "RegisterCertificate",
            body,
            "CertificateAlreadyExistsException",
            case,
        )
        .await;
    }

    // A different certificate of the same name is a second registration...
    let second_registration = register(
        &router,
        json!({ "DirectoryId": &first, "CertificateData": &namesake }),
        "a namesake certificate",
    )
    .await;
    assert_ne!(
        registered, second_registration,
        "case \"a namesake certificate\": expected a distinct CertificateId"
    );

    // ...and the SAME certificate registered with ANOTHER directory is that directory's
    // own registration, because a certificate is registered for a directory.
    let elsewhere = register(
        &router,
        json!({ "DirectoryId": &second, "CertificateData": &certificate }),
        "the same certificate, another directory",
    )
    .await;
    let reported = describe(
        &router,
        &second,
        &elsewhere,
        "the same certificate, another directory",
    )
    .await;
    assert_eq!(
        reported["CommonName"],
        json!(shared_name),
        "case \"the same certificate, another directory\": expected the certificate's own \
         common name, got {reported}"
    );

    // A registration is not visible through a directory it was not made for.
    for (case, directory_id, certificate_id) in [
        (
            "the other directory's registration",
            &first,
            &elsewhere,
        ),
        ("this directory's registration", &second, &registered),
    ] {
        refused(
            &router,
            "DescribeCertificate",
            json!({ "DirectoryId": directory_id, "CertificateId": certificate_id }),
            "CertificateDoesNotExistException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn a_registration_needs_a_directory_that_is_in_service() {
    let router = service();

    let (certificate, _) = certificate_pem(Some("early.example.com"), (2032, 7, 7));

    // Both creates leave the directory in the transient stage its create owes a
    // transition out of, so both rows exercise the window before it falls due. The two
    // types are the ones client-side LDAPS — the function this certificate performs — is
    // offered for, so the stage is the only thing that can refuse the call.
    let connector = crate::ad_connector::connect_directory(
        &router,
        crate::ad_connector::minimal_connect("pending.example.com"),
        "an AD Connector directory still being built",
    )
    .await;
    let managed = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("pending.corp.example.com"),
        "a Managed Microsoft AD directory still being built",
    )
    .await;

    let rows = [
        ("an AD Connector directory still being built", &connector),
        (
            "a Managed Microsoft AD directory still being built",
            &managed,
        ),
    ];
    for (case, directory_id) in rows {
        refused(
            &router,
            "RegisterCertificate",
            json!({ "DirectoryId": directory_id, "CertificateData": &certificate }),
            "DirectoryUnavailableException",
            case,
        )
        .await;
    }

    // The refusal was about the stage, so the very same call is admitted once the
    // transition the create owed has fallen due.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (case, directory_id) in rows {
        let certificate_id = register(
            &router,
            json!({ "DirectoryId": directory_id, "CertificateData": &certificate }),
            case,
        )
        .await;
        let reported = describe(&router, directory_id, &certificate_id, case).await;
        assert_eq!(
            reported["CommonName"],
            json!("early.example.com"),
            "case {case:?}: expected the registration once the directory is in service, \
             got {reported}"
        );
    }
}

#[tokio::test]
async fn an_absent_directory_or_certificate_is_refused_by_both_operations() {
    let router = service();

    let existing = directory_in_service(&router, "present.example.com", "absent").await;
    let (certificate, _) = certificate_pem(Some("present.example.com"), (2033, 8, 8));
    let kept = register(
        &router,
        json!({ "DirectoryId": &existing, "CertificateData": &certificate }),
        "absent: the surviving registration",
    )
    .await;

    // A directory this account never created, in several forms the modelled
    // `DirectoryId` pattern `^d-[0-9a-f]{10}$` admits...
    let mut absent_directories: Vec<String> = vec![
        "d-0000000000".to_string(),
        "d-00000000ff".to_string(),
        "d-abcdef0123".to_string(),
        "d-ffffffffff".to_string(),
    ];
    // ...and one that existed until it was deleted, so the refusal follows the store
    // rather than the shape of the id.
    let doomed = directory_in_service(&router, "doomed.example.com", "absent").await;
    crate::harness::delete_directory_and_wait(&router, &doomed, "absent: delete").await;
    absent_directories.push(doomed);

    for directory_id in &absent_directories {
        for (operation, body) in [
            (
                "RegisterCertificate",
                json!({ "DirectoryId": directory_id, "CertificateData": &certificate }),
            ),
            (
                "DescribeCertificate",
                json!({ "DirectoryId": directory_id, "CertificateId": &kept }),
            ),
        ] {
            let case = format!("{operation} against {directory_id}");
            refused(
                &router,
                operation,
                body.clone(),
                "DirectoryDoesNotExistException",
                &case,
            )
            .await;
            // The refusal is built from the request: it quotes the id the caller asked
            // for, so the caller can fix the request from the message alone.
            let (_, parsed) = call(&router, operation, body).await;
            let message = parsed["message"]
                .as_str()
                .or_else(|| parsed["Message"].as_str())
                .unwrap_or_default();
            assert!(
                message.contains(&format!("'{directory_id}'")),
                "case {case:?}: expected the message to name the requested directory, got {parsed}"
            );
        }
    }

    // A certificate id this directory holds no registration for, in several forms the
    // modelled `CertificateId` pattern `^c-[0-9a-f]{10}$` admits.
    for certificate_id in [
        "c-0000000000",
        "c-00000000ff",
        "c-abcdef0123",
        "c-ffffffffff",
    ] {
        refused(
            &router,
            "DescribeCertificate",
            json!({ "DirectoryId": &existing, "CertificateId": certificate_id }),
            "CertificateDoesNotExistException",
            &format!("DescribeCertificate against {certificate_id}"),
        )
        .await;
    }

    // Every refusal above stored nothing and consumed nothing: the registration the
    // surviving directory holds is still reported.
    let reported = describe(&router, &existing, &kept, "absent: the surviving registration").await;
    assert_eq!(
        reported["CertificateId"],
        json!(&kept),
        "case \"absent\": expected the surviving registration, got {reported}"
    );
}

#[tokio::test]
async fn registrations_are_capped_by_the_documented_per_directory_guard() {
    let router = service();
    let directory_id = directory_in_service(&router, "busy.example.com", "guard").await;

    let mut filled: Vec<String> = Vec::new();
    for index in 0..CERTIFICATE_GUARD {
        let (certificate, _) = certificate_pem(
            Some(&format!("filler-{index}.example.com")),
            (2030, 1, 1),
        );
        filled.push(
            register(
                &router,
                json!({ "DirectoryId": &directory_id, "CertificateData": certificate }),
                "guard: filling",
            )
            .await,
        );
    }

    // One more certificate past the guard is refused, and the stored registrations are
    // untouched.
    let (one_too_many, _) = certificate_pem(Some("one-too-many.example.com"), (2030, 1, 1));
    refused(
        &router,
        "RegisterCertificate",
        json!({ "DirectoryId": &directory_id, "CertificateData": one_too_many }),
        "CertificateLimitExceededException",
        "one past the per-directory guard",
    )
    .await;

    for certificate_id in &filled {
        let reported = describe(
            &router,
            &directory_id,
            certificate_id,
            "guard: after the refusal",
        )
        .await;
        assert_eq!(
            reported["CertificateId"],
            json!(certificate_id),
            "case \"guard: after the refusal\": expected every accepted registration to \
             survive, got {reported}"
        );
    }
}
