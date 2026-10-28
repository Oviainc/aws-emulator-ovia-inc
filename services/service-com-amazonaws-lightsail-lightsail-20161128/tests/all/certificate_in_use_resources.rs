// SPDX-License-Identifier: CC0-1.0

//! What a Lightsail SSL/TLS certificate counts as a resource it is attached to,
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness, the certificate stand-up and the issuance wait are the
//! ones the sibling suites already expose.
//!
//! The rule under test: `Certificate$inUseResourceCount` is "the number of
//! Lightsail resources that the certificate is attached to", and a resource is
//! attached to a certificate exactly when its own record references it — a
//! distribution through its `certificateName`, a container service through the
//! certificate name its `publicDomainNames` are keyed by — so EVERY such
//! resource is counted, and `DeleteCertificate` is refused for as long as the
//! count is above zero. That keeps the delete the exact inverse of the write
//! path: a certificate a container service's domains are keyed by cannot be
//! deleted out from under it, so the configuration the service reports stays one
//! `UpdateContainerService` still accepts.
//!
//! So the cases are TABLES: one row per set of resources referencing the one
//! certificate — how many container services key it, which Regions they live in,
//! whether the create or a later update recorded the key, whether a distribution
//! references it too, and how the reference is given back (deleting the service,
//! or updating it to drop the key) — and a second table over how many
//! certificates key the domains of a single container service, since the count
//! is per CERTIFICATE and not per service. `certificate_delete.rs` pins the same
//! rule where only distributions reference the certificate.

use serde_json::json;

// The harness, the wire error code, the origin stand-up, the certificate
// readers, the in-use refusal and the public-domain-name helpers this test
// target already exposes: a sibling suite drives the service through the same
// front door, and asserts the same refusal, rather than standing up a second
// one.
use crate::certificate_delete::assert_in_use_refusal;
use crate::container_service_public_domain_certificate::{
    public_domain_names, reported_domains,
};
use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{certificate, issue_certificate};
use crate::instance_lifecycle::{error_code, Emulator};

/// The operation whose in-use gate is under test, by the name its `X-Amz-Target`
/// carries.
const DELETE: &str = "DeleteCertificate";

/// The `ResourceType` a certificate reports, from the modelled enum — what its
/// `DoesNotExist` refusal names.
const RESOURCE_TYPE: &str = "Certificate";

/// The wire error code of Lightsail's rejection of a name that addresses
/// nothing, and the `code` its body carries.
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The one Region a Lightsail certificate and a distribution live in, from the
/// modelled `CreateCertificate` documentation ("Only certificates created in the
/// us-east-1 Amazon Web Services Region can be attached to Lightsail
/// distributions").
const CERTIFICATE_REGION: &str = "us-east-1";

/// The Availability Zone the origin instance of a distribution is created in:
/// the first zone of the certificate Region.
const ORIGIN_ZONE: &str = "us-east-1a";

/// The instance every distribution of this suite pulls its content from.
const ORIGIN: &str = "in-use-count-origin";

/// The bundle every distribution of this suite is created with: the rule under
/// test is about the certificate, not the capacity.
const DISTRIBUTION_BUNDLE: &str = "small_1_0";

/// The power and scale every container service of this suite is created with,
/// for the same reason.
const POWER: &str = "nano";
const SCALE: i64 = 1;

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// The `inUseResourceCount` the certificate `name` reports through
/// `GetCertificates`, which is the member the delete's gate and the client both
/// read.
async fn in_use_count(emulator: &Emulator, case: &str, name: &str) -> i64 {
    let record = certificate(emulator, name).await;
    record["certificateDetail"]["inUseResourceCount"]
        .as_i64()
        .unwrap_or_else(|| {
            panic!("case {case}: expected {name} to report an inUseResourceCount, got {record:?}")
        })
}

/// Require that the certificate `name` reports being attached to `want`
/// resources.
async fn assert_in_use(emulator: &Emulator, case: &str, name: &str, want: i64) {
    let got = in_use_count(emulator, case, name).await;
    assert_eq!(
        got, want,
        "case {case}: expected {name} to be attached to {want} resource(s), got {got}"
    );
}

/// Require that `DeleteCertificate` serves `name` and that the certificate is
/// then gone from every read.
async fn assert_delete_served(emulator: &Emulator, case: &str, name: &str) {
    let answered = emulator.ok(DELETE, json!({"certificateName": name})).await;
    let entries = answered["operations"].as_array().unwrap_or_else(|| {
        panic!("case {case}: expected an operations array from {DELETE}, got {answered:?}")
    });
    assert_eq!(
        entries.len(),
        1,
        "case {case}: expected exactly one log entry for the one certificate deleted, got \
         {answered:?}"
    );
    // The record is gone, so every later read of it is the service's
    // `DoesNotExist` rejection rather than a certificate attached to nothing.
    let (status, refused) = emulator
        .call("GetCertificates", json!({"certificateName": name}))
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected reading the deleted certificate {name} to be refused, got \
         {status} {refused:?}"
    );
    let code = error_code(&refused);
    assert_eq!(
        code, NOT_FOUND,
        "case {case}: expected the {NOT_FOUND} wire code for a deleted certificate, got {code} \
         {refused:?}"
    );
    assert_eq!(
        refused["code"].as_str(),
        Some(DOES_NOT_EXIST),
        "case {case}: expected the {DOES_NOT_EXIST} body code, got {refused:?}"
    );
    let message = refused["message"].as_str().unwrap_or("<no message>");
    assert!(
        message.contains(RESOURCE_TYPE) && message.contains(name),
        "case {case}: expected a message naming the {RESOURCE_TYPE} and '{name}', got {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// The table: every kind of resource that can reference a certificate
// ---------------------------------------------------------------------------

/// Which operation recorded the certificate key on a container service: both
/// write the same reference.
#[derive(Clone, Copy, Debug)]
enum RecordedBy {
    Create,
    Update,
}

/// How a container service's reference to the certificate is given back.
#[derive(Clone, Copy, Debug)]
enum Release {
    /// The whole service goes, taking the domains it served with it.
    DeleteTheService,
    /// The service stays and its `publicDomainNames` are replaced by none.
    DropTheKey,
}

/// One container service referencing the certificate: where it lives, the
/// domains it serves under the certificate, and which operation recorded them.
struct Service {
    region: &'static str,
    name: &'static str,
    domains: &'static [&'static str],
    recorded_by: RecordedBy,
}

/// One certificate and the resources of the account that reference it.
///
/// The rows differ along everything the rule generalises over: how many
/// container services key the certificate, the Regions they live in (including
/// one that is not the Region the certificate API is served from), which
/// operation recorded the key, how many domains are served under it, whether a
/// distribution references the same certificate too, and how the references are
/// given back.
struct Row {
    case: &'static str,
    certificate_name: &'static str,
    /// The domains the certificate covers; the first is its `domainName`.
    domains: &'static [&'static str],
    services: &'static [Service],
    /// The distributions whose `certificateName` names it, created in the
    /// certificate Region.
    distributions: &'static [&'static str],
    release: Release,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "one container service in the certificate Region, keyed at create",
        certificate_name: "cs-create-cert",
        domains: &["create.example.com"],
        services: &[Service {
            region: CERTIFICATE_REGION,
            name: "cs-create-service",
            domains: &["create.example.com"],
            recorded_by: RecordedBy::Create,
        }],
        distributions: &[],
        release: Release::DeleteTheService,
    },
    Row {
        case: "one container service outside the certificate Region, keyed by a later update",
        certificate_name: "cs-update-cert",
        domains: &["update.example.com", "www.update.example.com"],
        services: &[Service {
            region: "us-west-2",
            name: "cs-update-service",
            domains: &["update.example.com", "www.update.example.com"],
            recorded_by: RecordedBy::Update,
        }],
        distributions: &[],
        release: Release::DropTheKey,
    },
    Row {
        case: "two container services in different Regions keyed by the one certificate",
        certificate_name: "cs-pair-cert",
        domains: &["pair.example.com", "www.pair.example.com"],
        services: &[
            Service {
                region: CERTIFICATE_REGION,
                name: "cs-pair-east-service",
                domains: &["pair.example.com"],
                recorded_by: RecordedBy::Create,
            },
            Service {
                region: "eu-west-1",
                name: "cs-pair-europe-service",
                domains: &["www.pair.example.com"],
                recorded_by: RecordedBy::Update,
            },
        ],
        distributions: &[],
        release: Release::DropTheKey,
    },
    Row {
        case: "a container service and a distribution referencing the one certificate",
        certificate_name: "cs-and-cdn-cert",
        domains: &["mixed.example.com"],
        services: &[Service {
            region: "ap-northeast-1",
            name: "cs-mixed-service",
            domains: &["mixed.example.com"],
            recorded_by: RecordedBy::Create,
        }],
        distributions: &["cs-mixed-cdn"],
        release: Release::DeleteTheService,
    },
    Row {
        case: "no resource references it at all",
        certificate_name: "cs-unused-cert",
        domains: &["unused.example.com"],
        services: &[],
        distributions: &[],
        release: Release::DropTheKey,
    },
];

/// Stand `service` up and record the certificate key on it, the way `row` says.
async fn stand_up_service(
    emulator: &Emulator,
    case: &str,
    certificate_name: &str,
    service: &Service,
) {
    let keyed: &[(&str, &[&str])] = &[(certificate_name, service.domains)];
    let mut body = json!({"serviceName": service.name, "power": POWER, "scale": SCALE});
    if matches!(service.recorded_by, RecordedBy::Create) {
        body["publicDomainNames"] = public_domain_names(keyed);
    }
    let (status, created) = emulator
        .call_in(Some(service.region), "CreateContainerService", body)
        .await;
    assert_eq!(
        status, 201,
        "case {case}: expected {} to be created, got {status} {created:?}",
        service.name
    );
    if matches!(service.recorded_by, RecordedBy::Update) {
        let mut body = json!({"serviceName": service.name});
        body["publicDomainNames"] = public_domain_names(keyed);
        let (status, updated) = emulator
            .call_in(Some(service.region), "UpdateContainerService", body)
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the update of {} to be served, got {status} {updated:?}",
            service.name
        );
    }
    // The reference really is on the record every read reports.
    let reported = reported_domains(emulator, case, service.region, service.name).await;
    assert!(
        reported.get(certificate_name).is_some(),
        "case {case}: expected {} to report its domains under {certificate_name}, got {reported:?}",
        service.name
    );
}

/// RULE: a certificate counts every resource of the account whose record
/// references it — a distribution naming it and a container service keyed by it
/// alike — and `DeleteCertificate` is refused for as long as that count is above
/// zero, so the delete is served only once nothing references the certificate
/// any more.
#[tokio::test]
async fn a_certificate_counts_every_resource_that_references_it() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, ORIGIN, ORIGIN_ZONE).await;

    for row in ROWS {
        let case = row.case;
        issue_certificate(&emulator, row.certificate_name, row.domains).await;
        // Nothing references it yet, whatever else this account holds.
        assert_in_use(&emulator, case, row.certificate_name, 0).await;

        for service in row.services {
            stand_up_service(&emulator, case, row.certificate_name, service).await;
        }
        for distribution in row.distributions {
            emulator
                .ok(
                    "CreateDistribution",
                    json!({
                        "distributionName": distribution,
                        "bundleId": DISTRIBUTION_BUNDLE,
                        "origin": {"name": ORIGIN},
                        "defaultCacheBehavior": {"behavior": "cache"},
                        "certificateName": row.certificate_name,
                    }),
                )
                .await;
        }

        let referencing = row.services.len() + row.distributions.len();
        let mut remaining =
            i64::try_from(referencing).expect("a table row names a handful of resources");
        assert_in_use(&emulator, case, row.certificate_name, remaining).await;
        if remaining == 0 {
            // A certificate nothing references is deleted, which is the floor
            // the counted cases are measured against.
            assert_delete_served(&emulator, case, row.certificate_name).await;
            continue;
        }
        assert_in_use_refusal(&emulator, case, row.certificate_name, remaining).await;

        // The refused delete is what keeps the service's own configuration one
        // the write path still accepts: re-sending the very same public domain
        // names is served, because the certificate they are keyed by is still
        // held and issued.
        for service in row.services {
            let keyed: &[(&str, &[&str])] = &[(row.certificate_name, service.domains)];
            let mut body = json!({"serviceName": service.name});
            body["publicDomainNames"] = public_domain_names(keyed);
            let (status, answered) = emulator
                .call_in(Some(service.region), "UpdateContainerService", body)
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected re-sending the domains {} already serves to be accepted, \
                 got {status} {answered:?}",
                service.name
            );
        }

        // Every reference given back drops the count by one, and the delete
        // stays refused while any is left.
        for service in row.services {
            match row.release {
                Release::DeleteTheService => {
                    let (status, answered) = emulator
                        .call_in(
                            Some(service.region),
                            "DeleteContainerService",
                            json!({"serviceName": service.name}),
                        )
                        .await;
                    assert_eq!(
                        status, 204,
                        "case {case}: expected {} to be deleted, got {status} {answered:?}",
                        service.name
                    );
                }
                Release::DropTheKey => {
                    let mut body = json!({"serviceName": service.name});
                    body["publicDomainNames"] = public_domain_names(&[]);
                    let (status, answered) = emulator
                        .call_in(Some(service.region), "UpdateContainerService", body)
                        .await;
                    assert_eq!(
                        status, 200,
                        "case {case}: expected the domains of {} to be dropped, got {status} \
                         {answered:?}",
                        service.name
                    );
                }
            }
            remaining -= 1;
            assert_in_use(&emulator, case, row.certificate_name, remaining).await;
            if remaining > 0 {
                assert_in_use_refusal(&emulator, case, row.certificate_name, remaining).await;
            }
        }
        for distribution in row.distributions {
            emulator
                .ok("DeleteDistribution", json!({"distributionName": distribution}))
                .await;
            remaining -= 1;
            assert_in_use(&emulator, case, row.certificate_name, remaining).await;
            if remaining > 0 {
                assert_in_use_refusal(&emulator, case, row.certificate_name, remaining).await;
            }
        }

        // Nothing references the certificate any more, so the same delete that
        // was refused is now served.
        assert_delete_served(&emulator, case, row.certificate_name).await;
    }
}

// ---------------------------------------------------------------------------
// The count is per certificate, not per service
// ---------------------------------------------------------------------------

/// The container service whose domains two certificates key, and where it lives.
const SHARED_SERVICE: &str = "two-certificate-service";
const SHARED_SERVICE_REGION: &str = "us-east-2";

/// The two certificates keying that one service, and the domain each secures.
const KEYED: &[(&str, &str)] = &[
    ("shared-first-cert", "first.example.com"),
    ("shared-second-cert", "second.example.com"),
];

/// RULE: the count is a property of the CERTIFICATE, so each certificate keying
/// a group of one service's public domain names counts that service once, and a
/// certificate whose key the service drops is released while the others are
/// untouched.
#[tokio::test]
async fn each_certificate_keying_one_service_counts_it_once() {
    let emulator = Emulator::new();

    let mut keyed: Vec<(&str, &[&str])> = Vec::new();
    for (certificate_name, domain) in KEYED {
        issue_certificate(&emulator, certificate_name, std::slice::from_ref(domain)).await;
        keyed.push((*certificate_name, std::slice::from_ref(domain)));
    }

    let mut body = json!({
        "serviceName": SHARED_SERVICE,
        "power": POWER,
        "scale": SCALE,
    });
    body["publicDomainNames"] = public_domain_names(&keyed);
    let (status, created) = emulator
        .call_in(
            Some(SHARED_SERVICE_REGION),
            "CreateContainerService",
            body,
        )
        .await;
    assert_eq!(
        status, 201,
        "case standing up the shared service: expected HTTP 201, got {status} {created:?}"
    );

    // Each certificate is attached to the one service, so each reports 1 and
    // each delete is refused.
    for (certificate_name, _) in KEYED {
        let case = format!("{certificate_name} keying the shared service");
        assert_in_use(&emulator, &case, certificate_name, 1).await;
        assert_in_use_refusal(&emulator, &case, certificate_name, 1).await;
    }

    // Dropping the FIRST key releases that certificate alone: the second is
    // still keying the service's remaining domains.
    let (first, _) = KEYED[0];
    let (second, second_domain) = KEYED[1];
    let second_domains: &[&str] = std::slice::from_ref(&second_domain);
    let remaining: &[(&str, &[&str])] = &[(second, second_domains)];
    let mut body = json!({"serviceName": SHARED_SERVICE});
    body["publicDomainNames"] = public_domain_names(remaining);
    let (status, updated) = emulator
        .call_in(
            Some(SHARED_SERVICE_REGION),
            "UpdateContainerService",
            body,
        )
        .await;
    assert_eq!(
        status, 200,
        "case dropping the first key: expected the update to be served, got {status} {updated:?}"
    );

    let case = format!("{first} after its key was dropped");
    assert_in_use(&emulator, &case, first, 0).await;
    assert_delete_served(&emulator, &case, first).await;

    let case = format!("{second} while it still keys the shared service");
    assert_in_use(&emulator, &case, second, 1).await;
    assert_in_use_refusal(&emulator, &case, second, 1).await;

    // And the service is still reporting the domains it serves under the
    // certificate that stayed.
    let reported = reported_domains(
        &emulator,
        "the shared service after the drop",
        SHARED_SERVICE_REGION,
        SHARED_SERVICE,
    )
    .await;
    assert!(
        reported.get(second).is_some() && reported.get(first).is_none(),
        "case the shared service after the drop: expected its domains under {second} alone, got \
         {reported:?}"
    );
}
