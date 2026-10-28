// SPDX-License-Identifier: CC0-1.0

//! The certificate a container service's `publicDomainNames` are keyed by,
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a container service's `publicDomainNames` is keyed by the
//! name of the SSL/TLS certificate that secures the domains listed under it — "You
//! must create and validate an SSL/TLS certificate before you can use public
//! domain names with your container service" — so whichever operation records
//! those names, the create as much as the update, refuses a key naming a
//! certificate this account holds none of with Lightsail's `DoesNotExist`
//! envelope and a key naming a certificate the certificate authority has not
//! issued with the modelled `InvalidInputException`, writes nothing when it
//! refuses, and serves the request when every key names an `ISSUED` certificate
//! of the account. The sibling suites `distribution_create_certificate_validation.rs`
//! and `distribution_certificate_attachment.rs` pin the same exists-and-issued
//! rule where a distribution records a certificate.

use serde_json::{json, Map, Value};

// The harness, the wire error code, the certificate stand-up and the issuance
// wait this test target already exposes: a sibling suite drives the service
// through the same front door, and waits for the same clock, rather than
// standing up a second one.
use crate::distribution_certificate_attachment::{create_certificate, issue_certificate};
use crate::instance_lifecycle::{error_code, Emulator};

/// The `CertificateStatus` a freshly requested certificate is in, from the
/// modelled enum.
const PENDING_VALIDATION: &str = "PENDING_VALIDATION";

/// The value Lightsail puts in a not-found body's `code` member, which a
/// Lightsail SDK client branches on rather than on the HTTP status.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The power and scale every service of this suite is created with: the rule
/// under test is about the certificate, not the capacity.
const POWER: &str = "nano";
const SCALE: i64 = 1;

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// The `publicDomainNames` map `domains` describes, as the wire carries it.
///
/// `pub(crate)` so a sibling suite in this test target builds the same member
/// the same way rather than carrying a second copy of the conversion.
pub(crate) fn public_domain_names(domains: &[(&str, &[&str])]) -> Value {
    let mut map = Map::new();
    for (certificate, names) in domains {
        map.insert((*certificate).to_string(), json!(names));
    }
    Value::Object(map)
}

/// The `CreateContainerService` body for `name` carrying `domains`, or none at
/// all when `domains` is empty.
fn create_body(name: &str, domains: &[(&str, &[&str])]) -> Value {
    let mut body = json!({"serviceName": name, "power": POWER, "scale": SCALE});
    if !domains.is_empty() {
        body["publicDomainNames"] = public_domain_names(domains);
    }
    body
}

/// Read the container service `name` of `region` back through
/// `GetContainerServices`, answering the HTTP status and the body — so a case can
/// assert either the record or the refusal of a service that was never stored.
async fn read_service(emulator: &Emulator, region: &str, name: &str) -> (u16, Value) {
    emulator
        .call_in(
            Some(region),
            "GetContainerServices",
            json!({"serviceName": name}),
        )
        .await
}

/// The `publicDomainNames` the stored service `name` of `region` reports.
///
/// `pub(crate)` for the same reason as [`public_domain_names`].
pub(crate) async fn reported_domains(
    emulator: &Emulator,
    case: &str,
    region: &str,
    name: &str,
) -> Value {
    let (status, body) = read_service(emulator, region, name).await;
    assert_eq!(
        status, 200,
        "case {case}: expected the stored service {name} to be readable, got {status} {body:?}"
    );
    body["containerServices"][0]["publicDomainNames"].clone()
}

/// The domains `service` reports under `certificate`, sorted.
fn domains_of(service: &Value, certificate: &str) -> Vec<String> {
    let mut domains: Vec<String> = service[certificate]
        .as_array()
        .map(|domains| {
            domains
                .iter()
                .map(|domain| domain.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    domains.sort();
    domains
}

/// Assert `reported` is exactly the public domain names `want` describes.
fn assert_domains(case: &str, want: &[(&str, &[&str])], reported: &Value) {
    assert_eq!(
        reported.as_object().map(Map::len),
        Some(want.len()),
        "case {case}: expected exactly {} certificate(s) keying the domains, got {reported:?}",
        want.len()
    );
    for (certificate, names) in want {
        let mut expected: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        let got = domains_of(reported, certificate);
        assert_eq!(
            got, expected,
            "case {case}: expected the domains of '{certificate}' to be {expected:?}, got {got:?}"
        );
    }
}

/// Assert the refusal `body` is Lightsail's `DoesNotExist` envelope for the
/// certificate `certificate_name`.
fn assert_certificate_does_not_exist(case: &str, certificate_name: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    let code = error_code(body);
    assert_eq!(
        code, "NotFoundException",
        "case {case}: expected the wire error code NotFoundException, got {code} {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some(DOES_NOT_EXIST),
        "case {case}: expected the body code {DOES_NOT_EXIST}, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(format!("The Certificate does not exist: {certificate_name}").as_str()),
        "case {case}: expected the resource-type/name message, got {body:?}"
    );
}

/// Assert the refusal `body` is the modelled `InvalidInputException` for a
/// certificate that is still `PENDING_VALIDATION`.
fn assert_certificate_not_issued(case: &str, certificate_name: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    let code = error_code(body);
    assert_eq!(
        code, "InvalidInputException",
        "case {case}: expected the wire error code InvalidInputException, got {code} {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(
            format!(
                "The certificate {certificate_name} is {PENDING_VALIDATION}: you must create and \
                 validate an SSL/TLS certificate before you can use public domain names with \
                 your container service."
            )
            .as_str()
        ),
        "case {case}: expected the message to name the certificate, the status it is in and the \
         rule, got {body:?}"
    );
}

// ---------------------------------------------------------------------------
// The refusal table: a key naming a certificate the account does not hold
// ---------------------------------------------------------------------------

/// The certificate this account DOES hold in every case below, and the domains it
/// covers.
const HELD: &str = "held-cert";
const HELD_DOMAINS: &[&str] = &["held.example.com", "www.held.example.com"];

/// The name of a resource of ANOTHER type this account holds, so a case can pin
/// that a certificate is addressed as a certificate and not as any named
/// resource.
const HELD_BUCKET: &str = "held-domain-bucket";

/// One request whose `publicDomainNames` is keyed by a certificate this account
/// does not hold.
///
/// The rows differ along everything the rule generalises over: the region the
/// request is made in, a name nothing was ever created under, a name that
/// addresses a resource of another type, the held certificate's name in different
/// letter case (a certificate is addressed by its exact name), the shortest name
/// the modelled `ResourceName` pattern `^\w[\w\-]*\w$` admits, and a map one key
/// of which DOES name the held certificate — so a partly resolvable map is
/// refused whole.
struct Ghost {
    case: &'static str,
    region: &'static str,
    service_name: &'static str,
    /// The public domain names the request carries.
    domains: &'static [(&'static str, &'static [&'static str])],
    /// The key the refusal must name: the first one, in map order, that names no
    /// certificate of this account.
    refused_certificate: &'static str,
}

/// The table.
const GHOSTS: &[Ghost] = &[
    Ghost {
        case: "a name no certificate was ever created under",
        region: "us-east-1",
        service_name: "ghost-cert-service",
        domains: &[("ghost-cert", &["ghost.example.com"])],
        refused_certificate: "ghost-cert",
    },
    Ghost {
        case: "the name of a bucket of this account",
        region: "us-west-2",
        service_name: "bucket-named-service",
        domains: &[(HELD_BUCKET, &["bucket.example.com"])],
        refused_certificate: HELD_BUCKET,
    },
    Ghost {
        case: "the held certificate's name in different letter case",
        region: "eu-west-1",
        service_name: "letter-case-service",
        domains: &[("Held-Cert", &["held.example.com"])],
        refused_certificate: "Held-Cert",
    },
    Ghost {
        case: "the shortest name the modelled ResourceName pattern admits",
        region: "ap-northeast-1",
        service_name: "short-name-service",
        domains: &[("ab", &["short.example.com"])],
        refused_certificate: "ab",
    },
    Ghost {
        case: "two keys, one of them the held certificate",
        region: "us-east-2",
        service_name: "half-held-service",
        domains: &[
            (HELD, &["held.example.com"]),
            ("second-ghost-cert", &["second.example.com"]),
        ],
        refused_certificate: "second-ghost-cert",
    },
];

/// RULE: a create whose public domain names are keyed by a certificate this
/// account holds none of is refused with the `DoesNotExist` envelope and stores
/// no container service at all.
#[tokio::test]
async fn creating_a_service_keyed_by_a_certificate_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    // The certificate this account holds, issued, and covering exactly the
    // domains the cases key by it.
    issue_certificate(&emulator, HELD, HELD_DOMAINS).await;
    emulator
        .ok(
            "CreateBucket",
            json!({"bucketName": HELD_BUCKET, "bundleId": "small_1_0"}),
        )
        .await;

    for ghost in GHOSTS {
        let case = ghost.case;
        let (status, body) = emulator
            .call_in(
                Some(ghost.region),
                "CreateContainerService",
                create_body(ghost.service_name, ghost.domains),
            )
            .await;
        assert_certificate_does_not_exist(case, ghost.refused_certificate, status, &body);

        // A refused create stores nothing: the name is still free.
        let (status, read) = read_service(&emulator, ghost.region, ghost.service_name).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a refused create to store no container service, and reading \
             {} answered {status} {read:?}",
            ghost.service_name
        );
        assert_eq!(
            read["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case {case}: expected reading {} to answer {DOES_NOT_EXIST}, got {read:?}",
            ghost.service_name
        );
    }
}

/// RULE: the same check guards the update, which records the domains on a service
/// that already exists — a refused update leaves the domains the service already
/// served, and leaves it in the state it was in.
#[tokio::test]
async fn updating_a_service_keyed_by_a_certificate_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    issue_certificate(&emulator, HELD, HELD_DOMAINS).await;
    emulator
        .ok(
            "CreateBucket",
            json!({"bucketName": HELD_BUCKET, "bundleId": "small_1_0"}),
        )
        .await;

    for ghost in GHOSTS {
        let case = ghost.case;
        // The service the update is made against: created with the held
        // certificate's domains, so the refusal has something to leave alone.
        let held_domains: &[(&str, &[&str])] = &[(HELD, HELD_DOMAINS)];
        let (status, created) = emulator
            .call_in(
                Some(ghost.region),
                "CreateContainerService",
                create_body(ghost.service_name, held_domains),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case}: expected the service to be created, got {status} {created:?}"
        );
        let before = created["containerService"]["state"].clone();

        let mut body = json!({"serviceName": ghost.service_name});
        body["publicDomainNames"] = public_domain_names(ghost.domains);
        let (status, refused) = emulator
            .call_in(Some(ghost.region), "UpdateContainerService", body)
            .await;
        assert_certificate_does_not_exist(case, ghost.refused_certificate, status, &refused);

        // The refused update wrote nothing: the domains and the lifecycle are
        // the ones the create left.
        let (status, read) = read_service(&emulator, ghost.region, ghost.service_name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the service to still be readable, got {status} {read:?}"
        );
        let service = &read["containerServices"][0];
        assert_domains(case, held_domains, &service["publicDomainNames"]);
        assert_eq!(
            service["state"], before,
            "case {case}: a refused update must not move the service, got {:?}",
            service["state"]
        );
    }
}

/// RULE: only a certificate the authority has ISSUED may key a container
/// service's public domain names, so a create and an update naming one that is
/// still being validated are both refused with the modelled
/// `InvalidInputException` and write nothing.
///
/// The pending window is the issuance the clock owes a fresh certificate, so the
/// case does not ASSUME a call lands inside it: each attempt requests a fresh
/// certificate (whose create answer is asserted pending) for a fresh service
/// name, and an attempt the clock beat is retried rather than counted.
#[tokio::test]
async fn a_certificate_the_authority_has_not_issued_cannot_key_public_domain_names() {
    /// How many attempts may be beaten by the clock before the case fails. Each
    /// attempt is a handful of in-process calls against a sub-second window, so
    /// one is normally enough.
    const ATTEMPTS: usize = 10;

    let emulator = Emulator::new();
    let region = "us-east-1";

    // The service the UPDATE case is made against, settled before any attempt.
    let settled_service = "pending-update-service";
    let (status, created) = emulator
        .call_in(
            Some(region),
            "CreateContainerService",
            create_body(settled_service, &[]),
        )
        .await;
    assert_eq!(
        status, 201,
        "case standing up the service to update: expected HTTP 201, got {status} {created:?}"
    );

    let mut observed_create = false;
    let mut observed_update = false;
    for attempt in 0..ATTEMPTS {
        let case = format!("attempt {attempt}");
        let certificate_name = format!("pending-cert-{attempt}");
        let service_name = format!("pending-service-{attempt}");
        let requested = create_certificate(
            &emulator,
            &certificate_name,
            "pending.example.com",
            &["www.pending.example.com"],
        )
        .await;
        // The create's own answer, so no clock has run yet.
        assert_eq!(
            requested["certificateDetail"]["status"].as_str(),
            Some(PENDING_VALIDATION),
            "case {case}: expected a freshly requested certificate to be {PENDING_VALIDATION}, \
             got {requested:?}"
        );

        let domains: &[(&str, &[&str])] = &[(certificate_name.as_str(), &["pending.example.com"])];
        if !observed_create {
            let (status, body) = emulator
                .call_in(
                    Some(region),
                    "CreateContainerService",
                    create_body(&service_name, domains),
                )
                .await;
            // The clock issued this certificate between the two calls, so this
            // attempt says nothing about the pending window: try again with a
            // fresh pair.
            if status != 201 {
                assert_certificate_not_issued(&case, &certificate_name, status, &body);
                let (status, read) = read_service(&emulator, region, &service_name).await;
                assert_eq!(
                    status, 400,
                    "case {case}: expected a refused create to store no container service, and \
                     reading {service_name} answered {status} {read:?}"
                );
                observed_create = true;
            }
        }

        if !observed_update {
            // What the service serves going INTO the attempt: an earlier
            // attempt whose certificate the clock issued first was served, and
            // left its own domains on the service, so the refusal is judged
            // against what stands rather than against an empty map.
            let before = reported_domains(&emulator, &case, region, settled_service).await;
            let mut body = json!({"serviceName": settled_service});
            body["publicDomainNames"] = public_domain_names(domains);
            let (status, refused) = emulator
                .call_in(Some(region), "UpdateContainerService", body)
                .await;
            if status != 200 {
                assert_certificate_not_issued(&case, &certificate_name, status, &refused);
                let reported = reported_domains(&emulator, &case, region, settled_service).await;
                assert_eq!(
                    reported, before,
                    "case {case}: a refused update must leave the domains the service already \
                     served, got {reported:?}"
                );
                observed_update = true;
            }
        }

        if observed_create && observed_update {
            break;
        }
    }
    assert!(
        observed_create,
        "case the pending window on the create: expected one of {ATTEMPTS} attempts to name a \
         certificate the authority had not issued yet, and every one of them was issued first"
    );
    assert!(
        observed_update,
        "case the pending window on the update: expected one of {ATTEMPTS} attempts to name a \
         certificate the authority had not issued yet, and every one of them was issued first"
    );
}

// ---------------------------------------------------------------------------
// The served table: every key an issued certificate of the account
// ---------------------------------------------------------------------------

/// One request the service must serve, and what the stored record then reports.
///
/// The rows differ along what the rule generalises over: the region, how many
/// certificates key the domains (including none at all), and how many domains
/// each of them carries — up to the documented four in total.
struct Served {
    case: &'static str,
    region: &'static str,
    service_name: &'static str,
    /// The certificates to stand up, with the domains each covers, which are
    /// also the domains the request keys by them.
    domains: &'static [(&'static str, &'static [&'static str])],
    /// What the service's domains are REPLACED by in a second step, an update —
    /// the same rule on the other operation that records them.
    replacement: &'static [(&'static str, &'static [&'static str])],
}

/// The table.
const SERVED: &[Served] = &[
    Served {
        case: "no public domain names at all, then one certificate's single domain",
        region: "us-east-1",
        service_name: "default-domain-service",
        domains: &[],
        replacement: &[("served-one-cert", &["one.example.com"])],
    },
    Served {
        case: "one certificate covering two domains, then another certificate's one",
        region: "us-west-2",
        service_name: "two-domain-service",
        domains: &[("served-two-cert", &["two.example.com", "www.two.example.com"])],
        replacement: &[("served-swap-cert", &["swap.example.org"])],
    },
    Served {
        case: "two certificates carrying the documented four domains, then none",
        region: "eu-west-1",
        service_name: "four-domain-service",
        domains: &[
            ("served-a-cert", &["a.example.com", "b.example.com"]),
            ("served-b-cert", &["c.example.org", "d.example.org"]),
        ],
        replacement: &[],
    },
];

/// RULE: a request whose every key names an issued certificate of this account is
/// served, and the domains it carried are what every later read of the service
/// reports — through the create that records them and through the update that
/// replaces them.
#[tokio::test]
async fn public_domain_names_keyed_by_issued_certificates_are_served() {
    let emulator = Emulator::new();

    for served in SERVED {
        let case = served.case;
        for (certificate, domains) in served.domains.iter().chain(served.replacement) {
            issue_certificate(&emulator, certificate, domains).await;
        }

        let (status, created) = emulator
            .call_in(
                Some(served.region),
                "CreateContainerService",
                create_body(served.service_name, served.domains),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {case}: expected HTTP 201, got {status} {created:?}"
        );
        assert_domains(
            case,
            served.domains,
            &created["containerService"]["publicDomainNames"],
        );
        // The round trip: what the create stored is what a later read reports.
        let reported = reported_domains(&emulator, case, served.region, served.service_name).await;
        assert_domains(case, served.domains, &reported);

        // And the update records the replacement the same way.
        let mut body = json!({"serviceName": served.service_name});
        body["publicDomainNames"] = public_domain_names(served.replacement);
        let (status, updated) = emulator
            .call_in(Some(served.region), "UpdateContainerService", body)
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the update to be served, got {status} {updated:?}"
        );
        assert_domains(
            case,
            served.replacement,
            &updated["containerService"]["publicDomainNames"],
        );
        let reported = reported_domains(&emulator, case, served.region, served.service_name).await;
        assert_domains(case, served.replacement, &reported);
    }
}
