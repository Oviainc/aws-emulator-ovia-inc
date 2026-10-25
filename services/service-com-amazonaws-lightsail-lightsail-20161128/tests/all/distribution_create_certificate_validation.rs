// SPDX-License-Identifier: CC0-1.0

//! The certificate a `CreateDistribution` request names, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a distribution's `certificateName` names the SSL/TLS
//! certificate that distribution serves HTTPS traffic with, so whichever
//! operation records it — the create that names one on a new distribution as
//! much as the attach that sets one later — refuses a name this account holds no
//! certificate under with Lightsail's `DoesNotExist` envelope and a certificate
//! the certificate authority has not issued with the modelled
//! `OperationFailureException`, and a refused create stores no distribution at
//! all. The sibling suite `distribution_certificate_attachment.rs` pins the same
//! rule on the attach.

use serde_json::{json, Value};

// The harness, the certificate stand-up, the issuance wait and the instance
// stand-up this test target already exposes: a sibling suite drives the service
// through the same front door, and waits for the same clock, rather than
// standing up a second one.
use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{
    certificate, create_certificate, wait_for_issued_certificate,
};
use crate::instance_lifecycle::{error_code, Emulator};

/// The `CertificateStatus` a freshly requested certificate is in, from the
/// modelled enum.
const PENDING_VALIDATION: &str = "PENDING_VALIDATION";

/// The `CertificateStatus` a certificate has to be in to be the one a
/// distribution serves traffic with, from
/// `AttachCertificateToDistribution$certificateName` ("Only certificates with a
/// status of ISSUED can be attached to a distribution").
const ISSUED: &str = "ISSUED";

/// The value Lightsail puts in a not-found body's `code` member, which a
/// Lightsail SDK client branches on rather than on the HTTP status.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The availability zone the origin instances of this suite are created in.
const ORIGIN_ZONE: &str = "us-east-1a";

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// How many resources the certificate `name` reports itself in use by.
async fn in_use_resource_count(emulator: &Emulator, name: &str) -> Option<i64> {
    certificate(emulator, name).await["certificateDetail"]["inUseResourceCount"].as_i64()
}

/// The `CreateDistribution` body for the distribution `name` pulling from
/// `origin`, on `bundle_id`, naming `certificate_name` when one is given.
fn create_distribution_body(
    name: &str,
    bundle_id: &str,
    origin: &str,
    certificate_name: Option<&str>,
) -> Value {
    let mut body = json!({
        "distributionName": name,
        "bundleId": bundle_id,
        "origin": {"name": origin},
        "defaultCacheBehavior": {"behavior": "cache"},
    });
    if let Some(certificate_name) = certificate_name {
        body["certificateName"] = json!(certificate_name);
    }
    body
}

/// Read the distribution `name` back through `GetDistributions`, answering the
/// HTTP status and the body, so a case can assert either the record or the
/// refusal of a distribution that was never stored.
async fn read_distribution(emulator: &Emulator, name: &str) -> (u16, Value) {
    emulator
        .call("GetDistributions", json!({"distributionName": name}))
        .await
}

/// Every distribution name the account holds, from the unfiltered listing.
async fn all_distribution_names(emulator: &Emulator) -> Vec<String> {
    let body = emulator.ok("GetDistributions", json!({})).await;
    body["distributions"]
        .as_array()
        .map(|distributions| {
            distributions
                .iter()
                .map(|distribution| {
                    distribution["name"]
                        .as_str()
                        .unwrap_or("<not a string>")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert that the create of `distribution_name` stored nothing: the name is
/// neither readable nor listed.
async fn assert_nothing_stored(emulator: &Emulator, case: &str, distribution_name: &str) {
    let (status, body) = read_distribution(emulator, distribution_name).await;
    assert_eq!(
        status, 400,
        "case {case}: expected a refused create to store no distribution, and reading \
         {distribution_name} answered {status} {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some(DOES_NOT_EXIST),
        "case {case}: expected reading {distribution_name} to answer {DOES_NOT_EXIST}, got \
         {body:?}"
    );
    let listed = all_distribution_names(emulator).await;
    assert!(
        !listed.iter().any(|name| name == distribution_name),
        "case {case}: expected {distribution_name} to be listed nowhere, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// The refusal table: a certificate name the account holds nothing under
// ---------------------------------------------------------------------------

/// One `CreateDistribution` request naming a certificate this account does not
/// hold.
///
/// The rows differ along everything the rule generalises over: a name nothing
/// was ever created under, a name that addresses a resource of ANOTHER type,
/// the held certificate's name in different letter case (a certificate is
/// addressed by its exact name), and the shortest name the modelled
/// `ResourceName` pattern `^\w[\w\-]*\w$` admits.
struct Ghost {
    case: &'static str,
    certificate_name: &'static str,
    distribution_name: &'static str,
    bundle_id: &'static str,
    /// Whether the case goes on to create a certificate UNDER that name, to
    /// pin that the refused create left no distribution counting against it.
    /// Only for a name that addresses nothing else of the account.
    create_certificate_after: bool,
}

/// The table. Every row is refused with HTTP 400 and Lightsail's `DoesNotExist`
/// envelope: the modelled `NotFoundException` is answered at 400 with
/// `DoesNotExist` in the body. (The certificate-not-yet-issued refusal is its
/// own test below, because it depends on the issuance window rather than on the
/// names alone.)
const GHOSTS: &[Ghost] = &[
    Ghost {
        case: "a name no certificate was ever created under",
        certificate_name: "ghost-cert",
        distribution_name: "ghost-cdn",
        bundle_id: "small_1_0",
        create_certificate_after: true,
    },
    Ghost {
        case: "the name of an instance of this account",
        certificate_name: "validation-origin",
        distribution_name: "instance-named-cdn",
        bundle_id: "medium_1_0",
        create_certificate_after: false,
    },
    Ghost {
        case: "the name of a bucket of this account",
        certificate_name: "validation-bucket",
        distribution_name: "bucket-named-cdn",
        bundle_id: "large_1_0",
        create_certificate_after: false,
    },
    Ghost {
        case: "a held certificate's name in different letter case",
        certificate_name: "Held-Cert",
        distribution_name: "letter-case-cdn",
        bundle_id: "small_1_0",
        create_certificate_after: true,
    },
    Ghost {
        case: "the shortest name the modelled ResourceName pattern admits",
        certificate_name: "ab",
        distribution_name: "short-name-cdn",
        bundle_id: "small_1_0",
        create_certificate_after: true,
    },
];

/// RULE: a create naming a certificate this account holds none of is refused
/// with the `DoesNotExist` envelope and stores no distribution, so no
/// certificate later created under that name counts a distribution it was never
/// attached to.
#[tokio::test]
async fn creating_a_distribution_naming_a_certificate_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "validation-origin", ORIGIN_ZONE).await;
    emulator
        .ok(
            "CreateBucket",
            json!({"bucketName": "validation-bucket", "bundleId": "small_1_0"}),
        )
        .await;
    create_certificate(&emulator, "held-cert", "held.example.com", &[]).await;
    wait_for_issued_certificate(&emulator, "held-cert").await;

    for ghost in GHOSTS {
        let case = ghost.case;
        let (status, body) = emulator
            .call(
                "CreateDistribution",
                create_distribution_body(
                    ghost.distribution_name,
                    ghost.bundle_id,
                    "validation-origin",
                    Some(ghost.certificate_name),
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "NotFoundException",
            "case {case}: expected the wire error code NotFoundException, got {code} {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case {case}: expected the body code {DOES_NOT_EXIST}, got {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains(ghost.certificate_name)),
            "case {case}: expected a message naming the certificate the request asked for, got \
             {body:?}"
        );
        assert_nothing_stored(&emulator, case, ghost.distribution_name).await;
    }

    // The certificate this account DOES hold was never named by a create that
    // was served, so nothing is in use by it.
    let held = in_use_resource_count(&emulator, "held-cert").await;
    assert_eq!(
        held,
        Some(0),
        "case the held certificate: expected a certificate attached to nothing to be in use by \
         0 resources, got {held:?}"
    );
    // And a certificate created AFTER a refused create that named it counts
    // nothing either: the refusal left no distribution naming it.
    for ghost in GHOSTS.iter().filter(|ghost| ghost.create_certificate_after) {
        let case = ghost.case;
        create_certificate(
            &emulator,
            ghost.certificate_name,
            &format!("{}.example.com", ghost.certificate_name),
            &[],
        )
        .await;
        let counted = in_use_resource_count(&emulator, ghost.certificate_name).await;
        assert_eq!(
            counted,
            Some(0),
            "case {case}: expected a certificate created after the refused create to be in use \
             by 0 resources, got {counted:?}"
        );
    }
}

/// RULE: only an ISSUED certificate is one a distribution serves traffic with,
/// so a create naming a certificate the authority has not issued yet is refused
/// with the modelled `OperationFailureException` and stores no distribution.
///
/// The pending window is the issuance the clock owes a fresh certificate, so the
/// case does not ASSUME a create lands inside it: each attempt requests a fresh
/// certificate (whose create answer is asserted pending) for a fresh
/// distribution name, and an attempt the clock beat is retried rather than
/// counted.
#[tokio::test]
async fn creating_a_distribution_naming_a_certificate_the_authority_has_not_issued_is_refused() {
    /// How many attempts may be beaten by the clock before the case fails. Each
    /// attempt is a handful of in-process calls against a sub-second window, so
    /// one is normally enough.
    const ATTEMPTS: usize = 10;

    let emulator = Emulator::new();
    create_running_instance(&emulator, "pending-create-origin", ORIGIN_ZONE).await;

    let mut observed = false;
    for attempt in 0..ATTEMPTS {
        let case = format!("attempt {attempt}");
        let certificate_name = format!("pending-create-cert-{attempt}");
        let distribution_name = format!("pending-create-cdn-{attempt}");
        let created = create_certificate(
            &emulator,
            &certificate_name,
            "pending.example.com",
            &["www.pending.example.com"],
        )
        .await;
        // The create's own answer, so no clock has run yet.
        assert_eq!(
            created["certificateDetail"]["status"].as_str(),
            Some(PENDING_VALIDATION),
            "case {case}: expected a freshly requested certificate to be {PENDING_VALIDATION}, \
             got {created:?}"
        );
        let (status, body) = emulator
            .call(
                "CreateDistribution",
                create_distribution_body(
                    &distribution_name,
                    "small_1_0",
                    "pending-create-origin",
                    Some(&certificate_name),
                ),
            )
            .await;
        if status == 200 {
            // The clock issued this certificate between the two calls, so this
            // attempt says nothing about the pending window: try again with a
            // fresh pair.
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "OperationFailureException",
            "case {case}: expected OperationFailureException, got {code} {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains(ISSUED)
                    && message.contains(&certificate_name)),
            "case {case}: expected a message naming the certificate and the {ISSUED} \
             requirement, got {body:?}"
        );
        assert_nothing_stored(&emulator, &case, &distribution_name).await;
        let counted = in_use_resource_count(&emulator, &certificate_name).await;
        assert_eq!(
            counted,
            Some(0),
            "case {case}: expected the refused create to leave the certificate in use by 0 \
             resources, got {counted:?}"
        );
        observed = true;
        break;
    }
    assert!(
        observed,
        "case the pending window: expected one of {ATTEMPTS} attempts to name a certificate the \
         authority had not issued yet, and every one of them was issued first"
    );
}

// ---------------------------------------------------------------------------
// The served table: the certificate the account holds, issued
// ---------------------------------------------------------------------------

/// One `CreateDistribution` request the service must serve, and what the stored
/// distribution then reports.
///
/// The rows differ along what the rule generalises over: whether the request
/// names a certificate at all, and how many domains the certificate it names
/// covers.
struct Served {
    case: &'static str,
    distribution_name: &'static str,
    bundle_id: &'static str,
    /// The certificate the request names, with the domain it is for and its
    /// alternate domains — `None` for a request that names none.
    certificate: Option<(&'static str, &'static str, &'static [&'static str])>,
}

/// The table.
const SERVED: &[Served] = &[
    Served {
        case: "no certificate named",
        distribution_name: "plain-cdn",
        bundle_id: "small_1_0",
        certificate: None,
    },
    Served {
        case: "an issued certificate covering one domain",
        distribution_name: "one-domain-cdn",
        bundle_id: "medium_1_0",
        certificate: Some(("served-one-cert", "one.example.com", &[])),
    },
    Served {
        case: "an issued certificate covering three domains",
        distribution_name: "three-domain-cdn",
        bundle_id: "large_1_0",
        certificate: Some((
            "served-three-cert",
            "three.example.org",
            &["img.three.example.org", "api.three.example.org"],
        )),
    },
];

/// RULE: the same check serves the create that names a certificate this account
/// holds and has had issued — the stored distribution reports that name on every
/// later read, and the certificate counts the one distribution naming it.
#[tokio::test]
async fn creating_a_distribution_naming_an_issued_certificate_is_served() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "served-origin", ORIGIN_ZONE).await;

    for served in SERVED {
        let case = served.case;
        if let Some((name, domain, alternatives)) = served.certificate {
            create_certificate(&emulator, name, domain, alternatives).await;
            wait_for_issued_certificate(&emulator, name).await;
        }
        let created = emulator
            .ok(
                "CreateDistribution",
                create_distribution_body(
                    served.distribution_name,
                    served.bundle_id,
                    "served-origin",
                    served.certificate.map(|(name, _, _)| name),
                ),
            )
            .await["distribution"]
            .clone();
        let want = served.certificate.map(|(name, _, _)| name);
        assert_eq!(
            created["certificateName"].as_str(),
            want,
            "case {case}: expected the certificate the create named to be {want:?}, got \
             {created:?}"
        );
        // The domains a certificate covers are published on the distribution by
        // `AttachCertificateToDistribution`, which is the operation that makes
        // the attachment, so a create publishes none — pinned in
        // `distribution_certificate_attachment.rs`.
        let published = created["alternativeDomainNames"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            published, 0,
            "case {case}: expected a create to publish no alternate domain name, got {created:?}"
        );

        // The round trip: what the create stored is what a later read reports.
        let (status, body) = read_distribution(&emulator, served.distribution_name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the stored distribution to be readable, got {status} {body:?}"
        );
        let stored = body["distributions"]
            .as_array()
            .and_then(|distributions| distributions.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            stored["certificateName"].as_str(),
            want,
            "case {case}: expected the read to report the certificate {want:?}, got {stored:?}"
        );

        if let Some((name, _, _)) = served.certificate {
            let counted = in_use_resource_count(&emulator, name).await;
            assert_eq!(
                counted,
                Some(1),
                "case {case}: expected the certificate counted by the one distribution naming \
                 it, got {counted:?}"
            );
        }
    }
}
