// SPDX-License-Identifier: CC0-1.0

//! WHICH certificate of WHICH directory is not there: the `CertificateDoesNotExistException`
//! a registration lookup is refused with names both ids the lookup was made with.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body
//! (`crate::harness`), so nothing here reaches into the state struct and an answer that is
//! not computed from persisted state cannot pass.
//!
//! The rule under test: a refusal of this service states what the CALLER has to act on, so
//! the lookup that resolves no certificate reports the certificate id the request named,
//! the fixed member label of the directory it was looked for in, and that directory's id —
//! never the error shape's own documentation sentence, which is prose about the error class
//! rather than the message the service sends.
//!
//! It is asserted over a TABLE spanning the ways a certificate can fail to resolve — an id
//! nothing ever registered, an id of ANOTHER directory's registration, and the documented
//! deregister-then-read path where the registration really was there — against BOTH
//! operations that resolve one (`DescribeCertificate`, `DeregisterCertificate`), so no row
//! can pass by matching one recorded pair of ids.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificates::{certificate_pem, directory_in_service, register, TYPE_CLIENT_LDAPS};
use crate::directory_lifecycle::PAST_ONE_SPAN;
use crate::harness::{call, ok, service};

/// The operations that resolve a registration of a directory, and are therefore refused by
/// this one diagnostic: `DescribeCertificate` ("displays information about the certificate
/// registered for secure LDAP or client certificate authentication") and
/// `DeregisterCertificate` ("deletes from the system the certificate that was registered
/// for secure LDAP or client certificate authentication").
const LOOKUPS: [&str; 2] = ["DescribeCertificate", "DeregisterCertificate"];

/// Well-formed `CertificateId`s (`^c-[0-9a-f]{10}$`) no registration here ever mints, varied
/// along their digits so no row can pass by matching one recorded id.
const ABSENT_CERTIFICATE_IDS: [&str; 3] = ["c-0123456789", "c-0000000000", "c-ffffffffff"];

/// What this service's refusal envelope appends to every diagnostic it words: the call's own
/// correlation handle, whose value is per call.
const REQUEST_ID_SEPARATOR: &str = " : RequestId: ";

/// The diagnostic Directory Service refuses a registration lookup with, as
/// `crate::state::no_such_certificate` builds it: the certificate id the request named, the
/// fixed `directoryId:` member label and the directory the lookup was made in.
fn expected_diagnostic(certificate_id: &str, directory_id: &str) -> String {
    format!("Certificate {certificate_id} does not exist for directoryId: {directory_id}")
}

/// One lookup of `certificate_id` in `directory_id` through `operation`, asserted to be
/// refused with `CertificateDoesNotExistException` carrying that diagnostic and the call's
/// request id.
async fn assert_refused(
    router: &MultiServiceRouter,
    operation: &str,
    directory_id: &str,
    certificate_id: &str,
    case: &str,
) {
    let (status, parsed) = call(
        router,
        operation,
        json!({ "DirectoryId": directory_id, "CertificateId": certificate_id }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains("CertificateDoesNotExistException"),
        "case {case:?}: expected CertificateDoesNotExistException from {operation}, got {parsed}"
    );
    let message = parsed["Message"].as_str().unwrap_or_default();
    let request_id = parsed["RequestId"].as_str().unwrap_or_default();
    assert!(
        !request_id.is_empty(),
        "case {case:?}: expected {operation} to report a RequestId, got {parsed}"
    );
    assert_eq!(
        message,
        format!(
            "{diagnostic}{REQUEST_ID_SEPARATOR}{request_id}",
            diagnostic = expected_diagnostic(certificate_id, directory_id)
        ),
        "case {case:?}: expected the diagnostic naming both ids, got {message:?}"
    );
}

/// A registration of a fresh self-signed certificate in `directory_id`, in service, as its
/// id.
async fn registered(router: &MultiServiceRouter, directory_id: &str, case: &str) -> String {
    let (pem, _) = certificate_pem(Some("lookup.example.com"), (2033, 8, 9));
    let certificate_id = register(
        router,
        json!({
            "DirectoryId": directory_id,
            "CertificateData": pem,
            "Type": TYPE_CLIENT_LDAPS
        }),
        case,
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    certificate_id
}

/// RULE: a registration lookup that resolves nothing is refused with the certificate id the
/// request named, the `directoryId:` label and the directory it was looked for in — for
/// every way the lookup can fail and through either operation that makes it.
#[tokio::test]
async fn a_registration_lookup_that_resolves_nothing_names_both_ids() {
    let router = service();
    let subject = directory_in_service(&router, "lookup.example.com", "subject").await;
    let other = directory_in_service(&router, "other.example.com", "other").await;
    // A registration of the OTHER directory: its id is one the account really minted, and
    // it still resolves to nothing in the subject directory, because a certificate is
    // registered FOR one directory.
    let elsewhere = registered(&router, &other, "a registration of another directory").await;

    // The rows differ along WHY the lookup resolves nothing: an id nothing ever registered
    // (three of them, differing in their digits), an id another directory's registration
    // answers to, and the directory the lookup is made in.
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for certificate_id in ABSENT_CERTIFICATE_IDS {
        rows.push((
            format!("{certificate_id} — never registered"),
            subject.clone(),
            certificate_id.to_string(),
        ));
    }
    rows.push((
        "a registration of another directory".to_string(),
        subject.clone(),
        elsewhere.clone(),
    ));
    rows.push((
        "an absent id in the other directory".to_string(),
        other.clone(),
        ABSENT_CERTIFICATE_IDS[0].to_string(),
    ));

    for (case, directory_id, certificate_id) in rows {
        for operation in LOOKUPS {
            assert_refused(
                &router,
                operation,
                &directory_id,
                &certificate_id,
                &format!("{case} through {operation}"),
            )
            .await;
        }
    }
}

/// RULE: once the registration a deregister removed is out of the system, the read of it is
/// refused with that same diagnostic — the documented deregister-then-read path, where the
/// ids the message names are ones the account really minted.
#[tokio::test]
async fn a_deregistered_certificate_is_reported_gone_by_both_ids() {
    let router = service();
    let case = "deregistered";
    let directory_id = directory_in_service(&router, "deregistered.example.com", case).await;
    let certificate_id = registered(&router, &directory_id, case).await;

    // While the teardown is still owed the registration is reported in its transient state
    // rather than as absent: the refusal below is the one served AFTER the clock has taken
    // it out of the directory.
    ok(
        &router,
        "DeregisterCertificate",
        json!({ "DirectoryId": &directory_id, "CertificateId": &certificate_id }),
        case,
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for operation in LOOKUPS {
        assert_refused(
            &router,
            operation,
            &directory_id,
            &certificate_id,
            &format!("{case} through {operation}"),
        )
        .await;
    }

    // The listing agrees: the directory holds no registration at all, which is why the
    // lookup resolves nothing.
    let listing: Value = ok(
        &router,
        "ListCertificates",
        json!({ "DirectoryId": &directory_id }),
        case,
    )
    .await;
    assert_eq!(
        listing["CertificatesInfo"],
        json!([]),
        "case {case:?}: expected the deregistered directory to hold no registration, got \
         {listing}"
    );
}
