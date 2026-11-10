// SPDX-License-Identifier: CC0-1.0

//! `DeregisterCertificate` — the registrations a directory stops holding — driven end to
//! end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body. The
//! harness (`call`, `ok`, `refused`) is the one `directory_lifecycle.rs` defines and the
//! certificate helpers are the ones `certificates.rs` defines, so nothing here reaches
//! into the state struct and what the store holds is always read back through
//! `DescribeCertificate` and `ListCertificates`.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * a deregister deletes from the system exactly the registration the request names, by
//!   putting it into the modelled transient `Deregistering` state the clock then takes
//!   out of the directory — over rows differing in the function the certificate performs,
//!   in whether the register's own transition has settled, in how many registrations the
//!   directory holds and in whether the registration came with client certificate
//!   authentication settings;
//! * a certificate securing live LDAP calls cannot be deregistered until LDAP security is
//!   disabled, while one performing client certificate authentication is unaffected by
//!   that switch;
//! * a request naming a directory or a registration the store does not hold, or a
//!   directory that is not in service, is refused and deletes nothing;
//! * the deregister is idempotent while the clock still owes the teardown, and once the
//!   registration is gone the same certificate can be registered again.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificates::{
    assert_certificate_id, certificate_pem, connector_in_service, describe,
    directory_in_service, register, STATE_REGISTERED, STATE_REGISTERING, TYPE_CLIENT_CERT_AUTH,
    TYPE_CLIENT_LDAPS,
};
use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, refused, service, PAST_ONE_SPAN,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The `CertificateState` a registration carries between the deregister and the teardown
/// that deregister owes: the `Deregistering` value of the modelled `CertificateState`
/// enum (`smithy_query(query="shape",shape="CertificateState")` lists `Registering`,
/// `Registered`, `RegisterFailed`, `Deregistering`, `Deregistered`, `DeregisterFailed`).
const STATE_DEREGISTERING: &str = "Deregistering";

/// The only value of the modelled `LDAPSType` enum
/// (`smithy_query(query="shape",shape="LDAPSType")` answers `["Client"]`).
const LDAPS_TYPE: &str = "Client";

/// `DeregisterCertificate`, asserting it was accepted and answered the modelled empty
/// result.
async fn deregister(
    router: &MultiServiceRouter,
    directory_id: &str,
    certificate_id: &str,
    case: &str,
) {
    let answer = ok(
        router,
        "DeregisterCertificate",
        json!({ "DirectoryId": directory_id, "CertificateId": certificate_id }),
        case,
    )
    .await;
    // The operation models no output members, so the answer carries none.
    assert_eq!(
        answer,
        json!({}),
        "case {case:?}: expected an empty DeregisterCertificateResult, got {answer}"
    );
}

/// Every registration `ListCertificates` reports for `directory_id`, keyed by
/// `CertificateId`. The per-directory guard is below the default page size, so one page
/// is the whole listing.
async fn listed(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> Vec<(String, Value)> {
    let answer = ok(
        router,
        "ListCertificates",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    answer["CertificatesInfo"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a CertificatesInfo list, got {answer}"))
        .iter()
        .map(|entry| {
            let id = entry["CertificateId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a CertificateId in {entry}")
                })
                .to_string();
            (id, entry.clone())
        })
        .collect()
}

/// The ids `ListCertificates` reports for `directory_id`, as a set.
async fn listed_ids(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> BTreeSet<String> {
    listed(router, directory_id, case)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// Register a certificate stating `common_name` with `directory_id`, performing
/// `certificate_type` when one is named.
async fn register_one(
    router: &MultiServiceRouter,
    directory_id: &str,
    common_name: &str,
    certificate_type: Option<&str>,
    settings: Option<Value>,
    case: &str,
) -> (String, String) {
    let (pem, _) = certificate_pem(Some(common_name), (2030, 1, 1));
    let mut body = json!({ "DirectoryId": directory_id, "CertificateData": &pem });
    if let Some(wanted) = certificate_type {
        body["Type"] = json!(wanted);
    }
    if let Some(settings) = settings {
        body["ClientCertAuthSettings"] = settings;
    }
    let id = register(router, body, case).await;
    (id, pem)
}

/// One row of the deregistration table: the registration to delete and the registrations
/// the directory holds beside it.
struct DeregisterCase {
    name: &'static str,
    /// The `Type` member the register carried, if any.
    certificate_type: Option<&'static str>,
    /// The `ClientCertAuthSettings` the register carried, if any.
    settings: Option<Value>,
    /// Whether the state transition the register owes is allowed to fall due before the
    /// deregister: a row that does not wait deregisters a registration that is still
    /// being made.
    settled: bool,
    /// How many other registrations the directory holds.
    bystanders: usize,
}

#[tokio::test]
async fn a_deregister_deletes_exactly_the_named_registration_from_the_system() {
    let router = service();

    // The rows differ along everything a deregistration varies in: the function the
    // certificate performs (both modelled `CertificateType` values, and a register that
    // named none), whether the register's own transition had settled first, how many
    // registrations the directory holds beside the one being deleted (none, one, several),
    // and whether client certificate authentication settings came with it.
    let cases = [
        DeregisterCase {
            name: "a secure-LDAP registration among several",
            certificate_type: Some(TYPE_CLIENT_LDAPS),
            settings: None,
            settled: true,
            bystanders: 3,
        },
        DeregisterCase {
            name: "the directory's only registration, no type named",
            certificate_type: None,
            settings: None,
            settled: true,
            bystanders: 0,
        },
        DeregisterCase {
            name: "a client certificate authentication registration with an OCSP responder",
            certificate_type: Some(TYPE_CLIENT_CERT_AUTH),
            settings: Some(json!({ "OCSPUrl": "http://ocsp.example.org/status" })),
            settled: true,
            bystanders: 1,
        },
        DeregisterCase {
            name: "a registration whose register has not settled yet",
            certificate_type: Some(TYPE_CLIENT_CERT_AUTH),
            settings: None,
            settled: false,
            bystanders: 1,
        },
        DeregisterCase {
            name: "a secure-LDAP registration whose register has not settled yet",
            certificate_type: Some(TYPE_CLIENT_LDAPS),
            settings: None,
            settled: false,
            bystanders: 0,
        },
    ];

    // Every row gets its own directory, and they are all created before anything waits:
    // the stage transition each create owes falls due on the clock, so one wait covers
    // the whole table. A row whose certificate performs client authentication gets a
    // directory of the type that function is offered for.
    let mut ids: Vec<String> = Vec::new();
    for (row, case) in cases.iter().enumerate() {
        let name = format!("deregister-{row}.example.com");
        ids.push(if case.certificate_type == Some(TYPE_CLIENT_CERT_AUTH) {
            crate::ad_connector::connect_directory(
                &router,
                crate::ad_connector::minimal_connect(&name),
                case.name,
            )
            .await
        } else {
            create_microsoft_ad(&router, minimal_microsoft_ad(&name), case.name).await
        });
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (case, directory_id) in cases.iter().zip(&ids) {
        let mut bystanders: BTreeSet<String> = BTreeSet::new();
        for index in 0..case.bystanders {
            let (id, _) = register_one(
                &router,
                directory_id,
                &format!("bystander-{index}.example.com"),
                None,
                None,
                case.name,
            )
            .await;
            bystanders.insert(id);
        }
        let (target, _pem) = register_one(
            &router,
            directory_id,
            "doomed.example.com",
            case.certificate_type,
            case.settings.clone(),
            case.name,
        )
        .await;
        if case.settled {
            tokio::time::sleep(PAST_ONE_SPAN).await;
        }

        let before = listed_ids(&router, directory_id, case.name).await;
        let mut expected_before = bystanders.clone();
        expected_before.insert(target.clone());
        assert_eq!(
            before, expected_before,
            "case {:?}: expected the directory to hold every registration before the \
             deregister, got {before:?}",
            case.name
        );

        deregister(&router, directory_id, &target, case.name).await;

        // The certificate is not out of the system the instant the call answers: the
        // registration carries the modelled transient state until the teardown the
        // deregister owes falls due, and is still reported until then.
        let pending = describe(&router, directory_id, &target, case.name).await;
        assert_eq!(
            pending["State"],
            json!(STATE_DEREGISTERING),
            "case {:?}: expected the transient state right after the deregister, got {pending}",
            case.name
        );
        assert_eq!(
            pending["CommonName"],
            json!("doomed.example.com"),
            "case {:?}: expected the registration's own values to survive the deregister, \
             got {pending}",
            case.name
        );
        let still_listed = listed(&router, directory_id, case.name).await;
        let still_listed_ids: BTreeSet<String> =
            still_listed.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(
            still_listed_ids, expected_before,
            "case {:?}: expected a registration still being deregistered to be listed, \
             got {still_listed_ids:?}",
            case.name
        );
        for (id, entry) in &still_listed {
            if id == &target {
                assert_eq!(
                    entry["State"],
                    json!(STATE_DEREGISTERING),
                    "case {:?}: expected the listing to agree with the describe, got {entry}",
                    case.name
                );
            } else {
                assert_ne!(
                    entry["State"],
                    json!(STATE_DEREGISTERING),
                    "case {:?}: expected the registrations the request did not name to be \
                     untouched, got {entry}",
                    case.name
                );
            }
        }

        // Nothing but time passes here: the teardown falls due on the clock, not on a
        // later call.
        tokio::time::sleep(PAST_ONE_SPAN).await;
        refused(
            &router,
            "DescribeCertificate",
            json!({ "DirectoryId": directory_id, "CertificateId": &target }),
            "CertificateDoesNotExistException",
            case.name,
        )
        .await;
        let after = listed_ids(&router, directory_id, case.name).await;
        assert_eq!(
            after, bystanders,
            "case {:?}: expected exactly the registrations the deregister did not name, \
             got {after:?}",
            case.name
        );
        for bystander in &bystanders {
            let reported = describe(&router, directory_id, bystander, case.name).await;
            assert_eq!(
                reported["State"],
                json!(STATE_REGISTERED),
                "case {:?}: expected a registration the deregister did not name to stay in \
                 service, got {reported}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn a_certificate_securing_live_ldap_calls_cannot_be_deregistered() {
    let router = service();
    // LDAP secure calls can only be activated for a directory type that serves an LDAPS
    // endpoint, and this case also registers a certificate performing client
    // authentication — so the switch is exercised on an AD Connector directory, the type
    // both functions are offered for.
    let directory_id = connector_in_service(&router, "ldaps.corp.example.com", "in use").await;

    let (securing, _) = register_one(
        &router,
        &directory_id,
        "ldaps.corp.example.com",
        Some(TYPE_CLIENT_LDAPS),
        None,
        "in use",
    )
    .await;
    let (other_securing, _) = register_one(
        &router,
        &directory_id,
        "ldaps-spare.corp.example.com",
        Some(TYPE_CLIENT_LDAPS),
        None,
        "in use",
    )
    .await;
    let (smartcard, _) = register_one(
        &router,
        &directory_id,
        "smartcard.corp.example.com",
        Some(TYPE_CLIENT_CERT_AUTH),
        None,
        "in use",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // While LDAP security is being activated, and once it is active, every registration
    // performing ClientLDAPS secures those calls: the rows cover both statuses and both
    // such registrations.
    ok(
        &router,
        "EnableLDAPS",
        json!({ "DirectoryId": &directory_id, "Type": LDAPS_TYPE }),
        "in use",
    )
    .await;
    for (case, certificate_id) in [
        ("LDAP security still being activated", &securing),
        (
            "another secure-LDAP registration, still being activated",
            &other_securing,
        ),
    ] {
        refused(
            &router,
            "DeregisterCertificate",
            json!({ "DirectoryId": &directory_id, "CertificateId": certificate_id }),
            "CertificateInUseException",
            case,
        )
        .await;
    }

    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (case, certificate_id) in [
        ("LDAP security active", &securing),
        ("another secure-LDAP registration, LDAP security active", &other_securing),
    ] {
        refused(
            &router,
            "DeregisterCertificate",
            json!({ "DirectoryId": &directory_id, "CertificateId": certificate_id }),
            "CertificateInUseException",
            case,
        )
        .await;
        // The refusal deleted nothing: the registration is still in service.
        let reported = describe(&router, &directory_id, certificate_id, case).await;
        assert_eq!(
            reported["State"],
            json!(STATE_REGISTERED),
            "case {case:?}: expected a refused deregister to leave the registration in \
             service, got {reported}"
        );
    }

    // A certificate performing client certificate authentication serves no LDAP security
    // connection, so the switch does not hold it.
    deregister(
        &router,
        &directory_id,
        &smartcard,
        "client certificate authentication, LDAP security active",
    )
    .await;

    // The refusal was about the switch, so the very same call is admitted once LDAP
    // security is disabled.
    ok(
        &router,
        "DisableLDAPS",
        json!({ "DirectoryId": &directory_id, "Type": LDAPS_TYPE }),
        "in use",
    )
    .await;
    for (case, certificate_id) in [
        ("LDAP security disabled", &securing),
        (
            "another secure-LDAP registration, LDAP security disabled",
            &other_securing,
        ),
    ] {
        deregister(&router, &directory_id, certificate_id, case).await;
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let left = listed_ids(&router, &directory_id, "in use").await;
    assert!(
        left.is_empty(),
        "case \"in use\": expected every registration to be gone once the teardowns fell \
         due, got {left:?}"
    );
}

#[tokio::test]
async fn an_absent_directory_or_registration_is_refused_and_deletes_nothing() {
    let router = service();

    let existing = directory_in_service(&router, "present.example.com", "absent").await;
    let (kept, _) = register_one(
        &router,
        &existing,
        "present.example.com",
        None,
        None,
        "absent: the surviving registration",
    )
    .await;
    let elsewhere = directory_in_service(&router, "other.example.com", "absent").await;
    let (held_elsewhere, _) = register_one(
        &router,
        &elsewhere,
        "other.example.com",
        None,
        None,
        "absent: the other directory's registration",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // A directory this account never created, in several forms the modelled `DirectoryId`
    // pattern `^d-[0-9a-f]{10}$` admits...
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
        let case = format!("DeregisterCertificate against {directory_id}");
        let body = json!({ "DirectoryId": directory_id, "CertificateId": &kept });
        refused(
            &router,
            "DeregisterCertificate",
            body.clone(),
            "DirectoryDoesNotExistException",
            &case,
        )
        .await;
        // The refusal is built from the request: it quotes the id the caller asked for,
        // so the caller can fix the request from the message alone.
        let (_, parsed) = call(&router, "DeregisterCertificate", body).await;
        let message = parsed["message"]
            .as_str()
            .or_else(|| parsed["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains(&format!("'{directory_id}'")),
            "case {case:?}: expected the message to name the requested directory, got {parsed}"
        );
    }

    // A certificate id this directory holds no registration for, in several forms the
    // modelled `CertificateId` pattern `^c-[0-9a-f]{10}$` admits, and the id of a
    // registration another directory holds: a registration is reachable only through the
    // directory it was made for.
    let mut absent_certificates: Vec<String> = vec![
        "c-0000000000".to_string(),
        "c-00000000ff".to_string(),
        "c-abcdef0123".to_string(),
        "c-ffffffffff".to_string(),
    ];
    assert_certificate_id(&held_elsewhere, "absent: the other directory's registration");
    absent_certificates.push(held_elsewhere.clone());
    for certificate_id in &absent_certificates {
        refused(
            &router,
            "DeregisterCertificate",
            json!({ "DirectoryId": &existing, "CertificateId": certificate_id }),
            "CertificateDoesNotExistException",
            &format!("DeregisterCertificate against {certificate_id}"),
        )
        .await;
    }

    // A directory still being built by the transition its create owes is not in service,
    // so a deregister against it is refused exactly as a register is.
    let pending = create_directory(
        &router,
        minimal_create("pending.example.com"),
        "a directory still being built",
    )
    .await;
    refused(
        &router,
        "DeregisterCertificate",
        json!({ "DirectoryId": &pending, "CertificateId": &kept }),
        "DirectoryUnavailableException",
        "a directory still being built",
    )
    .await;

    // Every refusal above deleted nothing: both stored registrations are still reported,
    // still in service.
    for (case, directory_id, certificate_id) in [
        ("absent: the surviving registration", &existing, &kept),
        (
            "absent: the other directory's registration",
            &elsewhere,
            &held_elsewhere,
        ),
    ] {
        let reported = describe(&router, directory_id, certificate_id, case).await;
        assert_eq!(
            reported["State"],
            json!(STATE_REGISTERED),
            "case {case:?}: expected the stored registration to be untouched, got {reported}"
        );
    }
}

#[tokio::test]
async fn a_repeat_deregister_keeps_the_teardown_the_first_one_started() {
    let router = service();
    let directory_id = directory_in_service(&router, "repeat.example.com", "repeat").await;
    let (certificate_id, pem) = register_one(
        &router,
        &directory_id,
        "repeat.example.com",
        Some(TYPE_CLIENT_LDAPS),
        None,
        "repeat",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    deregister(&router, &directory_id, &certificate_id, "repeat").await;
    // A repeat inside the span names a registration that is still being deregistered, so
    // it is answered the same way and the pending teardown is not restarted.
    deregister(
        &router,
        &directory_id,
        &certificate_id,
        "a repeat inside the span",
    )
    .await;
    let pending = describe(
        &router,
        &directory_id,
        &certificate_id,
        "a repeat inside the span",
    )
    .await;
    assert_eq!(
        pending["State"],
        json!(STATE_DEREGISTERING),
        "case \"a repeat inside the span\": expected the registration to still be \
         deregistering, got {pending}"
    );

    // The repeat left the pending teardown where it was: the registration is gone once
    // the span the first deregister started has elapsed.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let left = listed_ids(&router, &directory_id, "repeat").await;
    assert!(
        left.is_empty(),
        "case \"repeat\": expected the registration to be gone once the clock says so, \
         got {left:?}"
    );
    // And a registration that is gone is one this directory does not hold.
    refused(
        &router,
        "DeregisterCertificate",
        json!({ "DirectoryId": &directory_id, "CertificateId": &certificate_id }),
        "CertificateDoesNotExistException",
        "a deregister repeated after the span",
    )
    .await;

    // The certificate itself left the system with the registration, so the very same
    // certificate can be registered again — under a fresh id, in the transient state a
    // register answers in.
    let again = register(
        &router,
        json!({ "DirectoryId": &directory_id, "CertificateData": &pem }),
        "the same certificate registered again",
    )
    .await;
    assert_ne!(
        again, certificate_id,
        "case \"the same certificate registered again\": expected a fresh CertificateId"
    );
    let reported = describe(
        &router,
        &directory_id,
        &again,
        "the same certificate registered again",
    )
    .await;
    assert_eq!(
        reported["State"],
        json!(STATE_REGISTERING),
        "case \"the same certificate registered again\": expected the transient state a \
         register answers in, got {reported}"
    );
    assert_eq!(
        reported["CommonName"],
        json!("repeat.example.com"),
        "case \"the same certificate registered again\": expected the certificate's own \
         common name, got {reported}"
    );
}
