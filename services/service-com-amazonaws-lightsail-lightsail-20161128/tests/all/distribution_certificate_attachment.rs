// SPDX-License-Identifier: CC0-1.0

//! Attaching a Lightsail SSL/TLS certificate to a content delivery network
//! distribution, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `AttachCertificateToDistribution` records the ONE
//! `ISSUED` certificate the request names on the ONE stored distribution of that
//! name, so every later read of that distribution reports it as
//! `certificateName` and reports every domain that certificate covers as the
//! distribution's `alternativeDomainNames`, the certificate counts that
//! distribution in its `inUseResourceCount`, and the call answers one
//! operation-log entry for the distribution it changed. An attach the two
//! resources' state does not allow — a distribution or a certificate the account
//! does not hold, or a certificate the certificate authority has not issued yet —
//! is refused and attaches nothing.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

// The harness, the wire error code, the minted-id check and the instance
// stand-up this test target already exposes: a sibling suite drives the service
// through the same front door rather than standing up a second one.
use crate::disk_attachment::create_running_instance;
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The `operationType` the log entry of an attach carries, from the modelled
/// `OperationType` enum.
const OPERATION_TYPE: &str = "AttachCertificateToDistribution";

/// The `ResourceType` a distribution reports, from the modelled enum.
const RESOURCE_TYPE_DISTRIBUTION: &str = "Distribution";

/// The one Region a distribution and a certificate live in, from the modelled
/// `LightsailDistribution$location` documentation ("all distributions are
/// located in the `us-east-1` Region").
const DISTRIBUTION_REGION: &str = "us-east-1";

/// The `CertificateStatus` a certificate is requested in, from the modelled
/// enum.
const PENDING_VALIDATION: &str = "PENDING_VALIDATION";

/// The `CertificateStatus` a certificate has to be in to be attachable, from
/// this operation's own `certificateName` documentation ("Only certificates with
/// a status of ISSUED can be attached to a distribution").
const ISSUED: &str = "ISSUED";

/// The `status` of a distribution whose configuration is still rolling out, and
/// the one it settles into. See `crate::state::DISTRIBUTION_STATUS_IN_PROGRESS`.
const IN_PROGRESS: &str = "InProgress";
/// See [`IN_PROGRESS`].
const DEPLOYED: &str = "Deployed";

/// The value Lightsail puts in a not-found body's `code` member, which a
/// Lightsail SDK client branches on rather than on the HTTP status.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The algorithm and the issuing certificate authority of a Lightsail
/// certificate (`crate::state::CERTIFICATE_KEY_ALGORITHM`,
/// `crate::state::CERTIFICATE_ISSUER_CA`).
const KEY_ALGORITHM: &str = "RSA_2048";
/// See [`KEY_ALGORITHM`].
const ISSUER_CA: &str = "Amazon";

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Request the certificate `name` for `domain`, with `alternatives` as its
/// alternate domains, answering the `certificate` the create reports.
///
/// `pub(crate)` so a sibling suite that needs a certificate of the account
/// stands one up the same way rather than re-typing the call.
pub(crate) async fn create_certificate(
    emulator: &Emulator,
    name: &str,
    domain: &str,
    alternatives: &[&str],
) -> Value {
    let mut body = json!({"certificateName": name, "domainName": domain});
    if !alternatives.is_empty() {
        body["subjectAlternativeNames"] = json!(alternatives);
    }
    emulator.ok("CreateCertificate", body).await["certificate"].clone()
}

/// The stored certificate `name`, in full, as `GetCertificates` reports it.
///
/// `pub(crate)` for the same reason as [`create_certificate`].
pub(crate) async fn certificate(emulator: &Emulator, name: &str) -> Value {
    let body = emulator
        .ok(
            "GetCertificates",
            json!({"certificateName": name, "includeCertificateDetails": true}),
        )
        .await;
    body["certificates"]
        .as_array()
        .and_then(|certificates| certificates.first())
        .cloned()
        .unwrap_or(Value::Null)
}

/// Poll `GetCertificates` until the certificate `name` reports `ISSUED`,
/// answering the whole record.
///
/// The issuance is owed by the clock, not by the poll: it arrives without anyone
/// asking more than once.
///
/// `pub(crate)` so the ONE issuance wait of this test target serves every suite
/// whose case needs an issued certificate.
pub(crate) async fn wait_for_issued_certificate(emulator: &Emulator, name: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reported = certificate(emulator, name).await;
        if reported["certificateDetail"]["status"].as_str() == Some(ISSUED) {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for certificate {name}: expected {ISSUED} within 5s, got {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The stored distribution `name`, as `GetDistributions` reports it.
///
/// `pub(crate)` so a sibling suite in this test target reads a distribution back
/// through the same front door rather than carrying a second copy of the call.
pub(crate) async fn distribution(emulator: &Emulator, name: &str) -> Value {
    let body = emulator
        .ok("GetDistributions", json!({"distributionName": name}))
        .await;
    body["distributions"]
        .as_array()
        .and_then(|distributions| distributions.first())
        .cloned()
        .unwrap_or(Value::Null)
}

/// Poll `GetDistributions` until the distribution `name` reports `want`,
/// answering the whole record.
///
/// `pub(crate)` for the same reason as [`distribution`].
pub(crate) async fn wait_for_distribution_status(
    emulator: &Emulator,
    name: &str,
    want: &str,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reported = distribution(emulator, name).await;
        if reported["status"].as_str() == Some(want) {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for distribution {name}: expected '{want}' within 5s, got {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The certificate names a distribution's `alternativeDomainNames` reports.
fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or("<not a string>").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// What kind of Lightsail resource an origin is, from the modelled `Origin`
/// documentation: "An origin can be a Lightsail instance, bucket, container
/// service, or load balancer."
#[derive(Clone, Copy, Debug)]
enum OriginKind {
    Instance,
    Bucket,
    LoadBalancer,
}

impl OriginKind {
    /// The `ResourceType` the distribution reports for an origin of this kind,
    /// from the modelled `ResourceType` enum.
    fn resource_type(self) -> &'static str {
        match self {
            Self::Instance => "Instance",
            Self::Bucket => "Bucket",
            Self::LoadBalancer => "LoadBalancer",
        }
    }
}

/// Stand up the origin resource `name` of `kind` in `region`.
async fn create_origin(emulator: &Emulator, kind: OriginKind, name: &str, region: &str) {
    match kind {
        OriginKind::Instance => {
            create_running_instance(emulator, name, &format!("{region}a")).await;
        }
        OriginKind::Bucket => {
            emulator
                .ok_at(
                    None,
                    Some(region),
                    "CreateBucket",
                    json!({"bucketName": name, "bundleId": "small_1_0"}),
                )
                .await;
        }
        OriginKind::LoadBalancer => {
            emulator
                .ok_at(
                    None,
                    Some(region),
                    "CreateLoadBalancer",
                    json!({"loadBalancerName": name, "instancePort": 80}),
                )
                .await;
        }
    }
}

// ---------------------------------------------------------------------------
// The attach table
// ---------------------------------------------------------------------------

/// One distribution, the certificate attached to it, and the origin it pulls
/// from.
///
/// The rows differ along everything the rule generalises over: how many domains
/// the certificate covers (and so how many alternative domain names the attach
/// publishes), the kind of resource the origin is and the Region it is in, the
/// bundle the distribution was created with, whether the distribution has
/// finished deploying when the attach is made, and whether private origin access
/// is enabled.
struct Row {
    case: &'static str,
    certificate_name: &'static str,
    domain: &'static str,
    alternatives: &'static [&'static str],
    distribution_name: &'static str,
    bundle_id: &'static str,
    origin_kind: OriginKind,
    origin_name: &'static str,
    origin_region: &'static str,
    /// Whether the origin's Region is named in the request. An origin that names
    /// none is looked for in the distribution's own Region.
    name_origin_region: bool,
    /// Whether the distribution is waited into `Deployed` before the attach: the
    /// operation's documentation makes the attach conditional on the
    /// CERTIFICATE's status, not on the distribution having settled.
    wait_deployed: bool,
    /// The domains the attach then publishes on the distribution, in order.
    alternative_domain_names: &'static [&'static str],
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "one domain, an instance origin in the distribution's own Region",
        certificate_name: "single-domain-cert",
        domain: "example.com",
        alternatives: &[],
        distribution_name: "single-domain-cdn",
        bundle_id: "small_1_0",
        origin_kind: OriginKind::Instance,
        origin_name: "cdn-origin-instance",
        origin_region: DISTRIBUTION_REGION,
        name_origin_region: false,
        wait_deployed: true,
        alternative_domain_names: &["example.com"],
    },
    Row {
        case: "one alternate domain, an instance origin in another Region",
        certificate_name: "www_cert",
        domain: "shop.example.net",
        alternatives: &["www.shop.example.net"],
        distribution_name: "www-cdn",
        bundle_id: "medium_1_0",
        origin_kind: OriginKind::Instance,
        origin_name: "cdn-origin-west",
        origin_region: "us-west-2",
        name_origin_region: true,
        wait_deployed: true,
        alternative_domain_names: &["shop.example.net", "www.shop.example.net"],
    },
    Row {
        case: "three alternate domains, a bucket origin",
        certificate_name: "many-domains-cert",
        domain: "alpha.example.org",
        alternatives: &["beta.example.org", "gamma.example.org", "delta.example.org"],
        distribution_name: "many-domains-cdn",
        bundle_id: "large_1_0",
        origin_kind: OriginKind::Bucket,
        origin_name: "cdn-origin-bucket",
        origin_region: DISTRIBUTION_REGION,
        name_origin_region: true,
        wait_deployed: true,
        alternative_domain_names: &[
            "alpha.example.org",
            "beta.example.org",
            "gamma.example.org",
            "delta.example.org",
        ],
    },
    Row {
        case: "attached while the distribution is still deploying, a load balancer origin",
        certificate_name: "early-cert",
        domain: "early.example.com",
        alternatives: &["cdn.early.example.com"],
        distribution_name: "early-cdn",
        bundle_id: "small_1_0",
        origin_kind: OriginKind::LoadBalancer,
        origin_name: "cdn-origin-lb",
        origin_region: "eu-west-1",
        name_origin_region: true,
        wait_deployed: false,
        alternative_domain_names: &["early.example.com", "cdn.early.example.com"],
    },
    Row {
        case: "an alternate domain that repeats the certificate's domain is published once",
        certificate_name: "repeated-domain-cert",
        domain: "repeat.example.com",
        alternatives: &["repeat.example.com", "other.example.com"],
        distribution_name: "repeated-domain-cdn",
        bundle_id: "small_1_0",
        origin_kind: OriginKind::Instance,
        origin_name: "cdn-origin-repeat",
        origin_region: DISTRIBUTION_REGION,
        name_origin_region: false,
        wait_deployed: true,
        alternative_domain_names: &["repeat.example.com", "other.example.com"],
    },
];

/// The `CreateDistribution` body `row` describes.
fn create_distribution_body(row: &Row) -> Value {
    let mut origin = json!({"name": row.origin_name, "protocolPolicy": "http-only"});
    if row.name_origin_region {
        origin["regionName"] = json!(row.origin_region);
    }
    if matches!(row.origin_kind, OriginKind::Bucket) {
        // "You can enable private origin access only when the distribution's
        // origin is a Lightsail bucket."
        return json!({
            "distributionName": row.distribution_name,
            "bundleId": row.bundle_id,
            "origin": origin,
            "defaultCacheBehavior": {"behavior": "cache"},
            "enablePrivateOriginAccess": true,
        });
    }
    json!({
        "distributionName": row.distribution_name,
        "bundleId": row.bundle_id,
        "origin": origin,
        "defaultCacheBehavior": {"behavior": "dont-cache"},
    })
}

/// RULE: an attach records the named ISSUED certificate on the one stored
/// distribution, so every later read reports it and the domains it covers, and
/// the certificate counts that distribution among the resources it is in use by.
#[tokio::test]
async fn attaching_a_certificate_records_it_on_the_distribution() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        // A certificate is requested pending validation, so it is not
        // attachable the instant it is created.
        let created = create_certificate(
            &emulator,
            row.certificate_name,
            row.domain,
            row.alternatives,
        )
        .await;
        assert_eq!(
            created["certificateDetail"]["status"].as_str(),
            Some(PENDING_VALIDATION),
            "case {case}: expected a freshly requested certificate to be \
             {PENDING_VALIDATION}, got {created:?}"
        );
        assert_eq!(
            created["certificateDetail"]["issuedAt"],
            Value::Null,
            "case {case}: expected no issuance date before the certificate is issued, got \
             {created:?}"
        );
        let issued = wait_for_issued_certificate(&emulator, row.certificate_name).await;
        let detail = issued["certificateDetail"].clone();
        assert_eq!(
            detail["keyAlgorithm"].as_str(),
            Some(KEY_ALGORITHM),
            "case {case}: expected the {KEY_ALGORITHM} key algorithm, got {detail:?}"
        );
        assert_eq!(
            detail["issuerCA"].as_str(),
            Some(ISSUER_CA),
            "case {case}: expected {ISSUER_CA} as the issuing authority, got {detail:?}"
        );
        for member in ["issuedAt", "notBefore", "notAfter"] {
            assert!(
                detail[member].as_f64().is_some(),
                "case {case}: expected an issued certificate to report {member}, got {detail:?}"
            );
        }
        assert!(
            detail["notAfter"].as_f64() > detail["notBefore"].as_f64(),
            "case {case}: expected the validity window to end after it starts, got {detail:?}"
        );
        assert!(
            detail["serialNumber"]
                .as_str()
                .is_some_and(|serial| !serial.is_empty()),
            "case {case}: expected a serial number, got {detail:?}"
        );
        assert_eq!(
            detail["inUseResourceCount"].as_i64(),
            Some(0),
            "case {case}: expected a certificate attached to nothing to be in use by 0 \
             resources, got {detail:?}"
        );

        create_origin(
            &emulator,
            row.origin_kind,
            row.origin_name,
            row.origin_region,
        )
        .await;
        let created = emulator
            .ok("CreateDistribution", create_distribution_body(row))
            .await["distribution"]
            .clone();
        assert_eq!(
            created["name"].as_str(),
            Some(row.distribution_name),
            "case {case}: expected the created distribution to be named \
             {}, got {created:?}",
            row.distribution_name
        );
        assert_eq!(
            created["bundleId"].as_str(),
            Some(row.bundle_id),
            "case {case}: expected the bundle the create asked for, got {created:?}"
        );
        // The create's own answer, so no clock has run yet: a new distribution's
        // configuration is still rolling out to the edge locations.
        assert_eq!(
            created["status"].as_str(),
            Some(IN_PROGRESS),
            "case {case}: expected a new distribution to be {IN_PROGRESS}, got {created:?}"
        );
        assert_eq!(
            created["origin"]["resourceType"].as_str(),
            Some(row.origin_kind.resource_type()),
            "case {case}: expected the origin to be a {}, got {created:?}",
            row.origin_kind.resource_type()
        );
        assert_eq!(
            created["origin"]["regionName"].as_str(),
            Some(row.origin_region),
            "case {case}: expected the origin in {}, got {created:?}",
            row.origin_region
        );
        assert_eq!(
            created["location"]["regionName"].as_str(),
            Some(DISTRIBUTION_REGION),
            "case {case}: expected every distribution in {DISTRIBUTION_REGION}, got {created:?}"
        );
        assert_eq!(
            created["certificateName"],
            Value::Null,
            "case {case}: expected a distribution created with no certificate to report none, \
             got {created:?}"
        );
        assert!(
            string_list(&created["alternativeDomainNames"]).is_empty(),
            "case {case}: expected no alternative domain names before an attach, got {created:?}"
        );
        if row.wait_deployed {
            wait_for_distribution_status(&emulator, row.distribution_name, DEPLOYED).await;
        }

        let answered = emulator
            .ok(
                "AttachCertificateToDistribution",
                json!({
                    "distributionName": row.distribution_name,
                    "certificateName": row.certificate_name,
                }),
            )
            .await;
        let operation = answered["operation"].clone();
        assert_eq!(
            operation["operationType"].as_str(),
            Some(OPERATION_TYPE),
            "case {case}: expected the {OPERATION_TYPE} operation type, got {operation:?}"
        );
        assert_eq!(
            operation["resourceType"].as_str(),
            Some(RESOURCE_TYPE_DISTRIBUTION),
            "case {case}: expected the entry to be about the distribution, got {operation:?}"
        );
        assert_eq!(
            operation["resourceName"].as_str(),
            Some(row.distribution_name),
            "case {case}: expected the entry to name {}, got {operation:?}",
            row.distribution_name
        );
        assert_eq!(
            operation["status"].as_str(),
            Some("Started"),
            "case {case}: expected a Started operation, got {operation:?}"
        );
        assert_eq!(
            operation["isTerminal"].as_bool(),
            Some(false),
            "case {case}: expected a non-terminal operation, got {operation:?}"
        );
        assert_eq!(
            operation["location"]["regionName"].as_str(),
            Some(DISTRIBUTION_REGION),
            "case {case}: expected the entry located where the distribution is, got \
             {operation:?}"
        );
        let id = operation["id"].as_str().unwrap_or_default();
        assert!(
            is_uuid_shaped(id),
            "case {case}: expected a uuid-shaped operation id, got {id:?}"
        );

        // The attachment, as the DISTRIBUTION reports it: the certificate's
        // domains are the ones the distribution now accepts HTTPS traffic for.
        // The rolling-out status is asserted on the log entry answered above
        // (`isTerminal` false), which the call itself built, rather than on this
        // read — by the time a read lands, the clock may already have owed the
        // distribution its deployment.
        let attached = distribution(&emulator, row.distribution_name).await;
        assert_eq!(
            attached["certificateName"].as_str(),
            Some(row.certificate_name),
            "case {case}: expected the attached certificate to be reported, got {attached:?}"
        );
        let published = string_list(&attached["alternativeDomainNames"]);
        assert_eq!(
            published, row.alternative_domain_names,
            "case {case}: expected the domains {:?}, got {published:?}",
            row.alternative_domain_names
        );
        // And it survives the deployment the clock owed the distribution.
        let deployed =
            wait_for_distribution_status(&emulator, row.distribution_name, DEPLOYED).await;
        assert_eq!(
            deployed["certificateName"].as_str(),
            Some(row.certificate_name),
            "case {case}: expected the attachment to survive the deployment, got {deployed:?}"
        );
        assert_eq!(
            string_list(&deployed["alternativeDomainNames"]),
            published,
            "case {case}: expected the same domains after the deployment, got {deployed:?}"
        );
        // And as the CERTIFICATE reports it: one more resource it is in use by.
        let in_use = certificate(&emulator, row.certificate_name).await;
        assert_eq!(
            in_use["certificateDetail"]["inUseResourceCount"].as_i64(),
            Some(1),
            "case {case}: expected the certificate in use by the distribution, got {in_use:?}"
        );
    }

    // Every distribution the table created, as the LISTING reports them: the
    // same attachment the named read reports, for every one of them.
    let listed = emulator.ok("GetDistributions", json!({})).await["distributions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        ROWS.len(),
        "case the listing: expected {} distributions, got {listed:?}",
        ROWS.len()
    );
    for row in ROWS {
        let found = listed
            .iter()
            .find(|entry| entry["name"].as_str() == Some(row.distribution_name))
            .unwrap_or_else(|| {
                panic!(
                    "case the listing: expected {} among {listed:?}",
                    row.distribution_name
                )
            });
        assert_eq!(
            found["certificateName"].as_str(),
            Some(row.certificate_name),
            "case the listing of {}: expected the attached certificate, got {found:?}",
            row.distribution_name
        );
        assert_eq!(
            string_list(&found["alternativeDomainNames"]),
            row.alternative_domain_names,
            "case the listing of {}: expected the certificate's domains, got {found:?}",
            row.distribution_name
        );
    }
}

/// RULE: a distribution holds ONE attached certificate, so attaching the one it
/// already has leaves that certificate attached and attaching another replaces
/// it — the domains the distribution publishes and the resource counts of both
/// certificates follow the attachment.
#[tokio::test]
async fn attaching_replaces_the_one_attached_certificate() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "swap-origin", "us-east-1a").await;
    for (name, domain, alternatives) in [
        ("first-cert", "first.example.com", &["www.first.example.com"]),
        (
            "second-cert",
            "second.example.com",
            &["www.second.example.com"],
        ),
    ] {
        create_certificate(&emulator, name, domain, alternatives).await;
        wait_for_issued_certificate(&emulator, name).await;
    }
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": "swap-cdn",
                "bundleId": "small_1_0",
                "origin": {"name": "swap-origin"},
                "defaultCacheBehavior": {"behavior": "cache"},
            }),
        )
        .await;

    // Each step: the certificate attached, the domains the distribution then
    // publishes, and the resource count each certificate reports.
    let steps: &[(&str, &str, &[&str], i64, i64)] = &[
        (
            "the first attach",
            "first-cert",
            &["first.example.com", "www.first.example.com"],
            1,
            0,
        ),
        (
            "attaching the same certificate again",
            "first-cert",
            &["first.example.com", "www.first.example.com"],
            1,
            0,
        ),
        (
            "attaching another certificate replaces it",
            "second-cert",
            &["second.example.com", "www.second.example.com"],
            0,
            1,
        ),
    ];
    for (case, certificate_name, domains, first_count, second_count) in steps {
        emulator
            .ok(
                "AttachCertificateToDistribution",
                json!({"distributionName": "swap-cdn", "certificateName": certificate_name}),
            )
            .await;
        let attached = distribution(&emulator, "swap-cdn").await;
        assert_eq!(
            attached["certificateName"].as_str(),
            Some(*certificate_name),
            "case {case}: expected {certificate_name} attached, got {attached:?}"
        );
        let published = string_list(&attached["alternativeDomainNames"]);
        assert_eq!(
            published, *domains,
            "case {case}: expected the domains {domains:?}, got {published:?}"
        );
        for (name, want) in [("first-cert", first_count), ("second-cert", second_count)] {
            let reported = certificate(&emulator, name).await;
            assert_eq!(
                reported["certificateDetail"]["inUseResourceCount"].as_i64(),
                Some(*want),
                "case {case}: expected {name} in use by {want} resources, got {reported:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The refusal table
// ---------------------------------------------------------------------------

/// One `AttachCertificateToDistribution` request the service must refuse, and
/// the error envelope it must refuse it with.
struct Refusal {
    case: &'static str,
    distribution_name: &'static str,
    certificate_name: &'static str,
    /// The wire error code (`__type`).
    code: &'static str,
    /// The body's `code` member, for the envelope that carries one.
    body_code: Option<&'static str>,
}

/// The table. Every row is refused with HTTP 400 and Lightsail's `DoesNotExist`
/// envelope: the modelled `NotFoundException` is answered at 400 with
/// `DoesNotExist` in the body. (The certificate-not-yet-issued refusal is its
/// own test, because it depends on the issuance window rather than on the names
/// alone.)
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a distribution name the account does not hold",
        distribution_name: "ghost-cdn",
        certificate_name: "guard-cert",
        code: "NotFoundException",
        body_code: Some(DOES_NOT_EXIST),
    },
    Refusal {
        case: "a certificate name the account does not hold",
        distribution_name: "guard-cdn",
        certificate_name: "ghost-cert",
        code: "NotFoundException",
        body_code: Some(DOES_NOT_EXIST),
    },
    Refusal {
        case: "neither resource exists",
        distribution_name: "ghost-cdn",
        certificate_name: "ghost-cert",
        code: "NotFoundException",
        body_code: Some(DOES_NOT_EXIST),
    },
];

/// RULE: an attach the two resources' state does not allow is refused with the
/// modelled error for its cause and attaches NOTHING — the distribution keeps
/// the certificate it already had.
#[tokio::test]
async fn a_refused_attach_attaches_nothing() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "guard-origin", "us-east-1a").await;
    create_certificate(&emulator, "guard-cert", "guard.example.com", &[]).await;
    wait_for_issued_certificate(&emulator, "guard-cert").await;
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": "guard-cdn",
                "bundleId": "small_1_0",
                "origin": {"name": "guard-origin"},
                "defaultCacheBehavior": {"behavior": "cache"},
            }),
        )
        .await;
    // One attachment the refusals must leave exactly as it is.
    emulator
        .ok(
            "AttachCertificateToDistribution",
            json!({"distributionName": "guard-cdn", "certificateName": "guard-cert"}),
        )
        .await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let (status, body) = emulator
            .call(
                "AttachCertificateToDistribution",
                json!({
                    "distributionName": refusal.distribution_name,
                    "certificateName": refusal.certificate_name,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, refusal.code,
            "case {case}: expected the wire error code {}, got {code} {body:?}",
            refusal.code
        );
        assert_eq!(
            body["code"].as_str(),
            refusal.body_code,
            "case {case}: expected the body code {:?}, got {body:?}",
            refusal.body_code
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case}: expected a message saying what was refused, got {body:?}"
        );
        let guarded = distribution(&emulator, "guard-cdn").await;
        assert_eq!(
            guarded["certificateName"].as_str(),
            Some("guard-cert"),
            "case {case}: expected the refusal to change no attachment, got {guarded:?}"
        );
        assert_eq!(
            string_list(&guarded["alternativeDomainNames"]),
            vec!["guard.example.com".to_string()],
            "case {case}: expected the refusal to publish no domain, got {guarded:?}"
        );
    }
}

/// RULE: the certificate counts every distribution whose `certificateName`
/// names it — including one `CreateDistribution` named at create time — and the
/// domains it covers are published on the distribution by the attach, which is
/// the operation that makes the attachment.
#[tokio::test]
async fn a_certificate_named_at_create_is_counted_and_published_by_the_attach() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "create-origin", "us-east-1a").await;
    create_certificate(
        &emulator,
        "create-time-cert",
        "create.example.com",
        &["img.create.example.com"],
    )
    .await;
    wait_for_issued_certificate(&emulator, "create-time-cert").await;

    let case = "named at create";
    let created = emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": "create-time-cdn",
                "bundleId": "small_1_0",
                "origin": {"name": "create-origin"},
                "defaultCacheBehavior": {"behavior": "cache"},
                "certificateName": "create-time-cert",
            }),
        )
        .await["distribution"]
        .clone();
    assert_eq!(
        created["certificateName"].as_str(),
        Some("create-time-cert"),
        "case {case}: expected the certificate the create named, got {created:?}"
    );
    let in_use = certificate(&emulator, "create-time-cert").await;
    assert_eq!(
        in_use["certificateDetail"]["inUseResourceCount"].as_i64(),
        Some(1),
        "case {case}: expected the certificate counted by the distribution naming it, got \
         {in_use:?}"
    );

    // The attach is what publishes the certificate's domains on the
    // distribution, so they appear once it is made.
    let case = "attached after the create";
    emulator
        .ok(
            "AttachCertificateToDistribution",
            json!({
                "distributionName": "create-time-cdn",
                "certificateName": "create-time-cert",
            }),
        )
        .await;
    let stored = distribution(&emulator, "create-time-cdn").await;
    assert_eq!(
        stored["certificateName"].as_str(),
        Some("create-time-cert"),
        "case {case}: expected the attached certificate, got {stored:?}"
    );
    assert_eq!(
        string_list(&stored["alternativeDomainNames"]),
        vec![
            "create.example.com".to_string(),
            "img.create.example.com".to_string()
        ],
        "case {case}: expected the certificate's domains, got {stored:?}"
    );
    let in_use = certificate(&emulator, "create-time-cert").await;
    assert_eq!(
        in_use["certificateDetail"]["inUseResourceCount"].as_i64(),
        Some(1),
        "case {case}: expected the one distribution still counted once, got {in_use:?}"
    );
}

/// RULE: only an ISSUED certificate can be attached, so an attach made while
/// the certificate authority has not issued the certificate yet is refused with
/// the modelled `OperationFailureException` and attaches nothing.
///
/// The pending window is the issuance the clock owes a fresh certificate, so the
/// case does not ASSUME an attach lands inside it: each attempt requests a fresh
/// certificate (whose create answer is asserted pending) against a fresh
/// distribution, and an attempt the clock beat is retried rather than counted.
#[tokio::test]
async fn attaching_a_certificate_the_authority_has_not_issued_is_refused() {
    /// How many attempts may be beaten by the clock before the case fails. Each
    /// attempt is a handful of in-process calls against a 500 ms window, so one
    /// is normally enough.
    const ATTEMPTS: usize = 10;

    let emulator = Emulator::new();
    create_running_instance(&emulator, "pending-origin", "us-east-1a").await;

    let mut observed = false;
    for attempt in 0..ATTEMPTS {
        let case = format!("attempt {attempt}");
        let certificate_name = format!("pending-cert-{attempt}");
        let distribution_name = format!("pending-cdn-{attempt}");
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
            "case {case}: expected a freshly requested certificate to be \
             {PENDING_VALIDATION}, got {created:?}"
        );
        emulator
            .ok(
                "CreateDistribution",
                json!({
                    "distributionName": distribution_name,
                    "bundleId": "small_1_0",
                    "origin": {"name": "pending-origin"},
                    "defaultCacheBehavior": {"behavior": "cache"},
                }),
            )
            .await;
        let (status, body) = emulator
            .call(
                "AttachCertificateToDistribution",
                json!({
                    "distributionName": distribution_name,
                    "certificateName": certificate_name,
                }),
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
                .is_some_and(|message| message.contains(ISSUED)),
            "case {case}: expected a message naming the {ISSUED} requirement, got {body:?}"
        );
        let refused = distribution(&emulator, &distribution_name).await;
        assert_eq!(
            refused["certificateName"],
            Value::Null,
            "case {case}: expected the refusal to attach nothing, got {refused:?}"
        );
        assert!(
            string_list(&refused["alternativeDomainNames"]).is_empty(),
            "case {case}: expected the refusal to publish no domain, got {refused:?}"
        );
        observed = true;
        break;
    }
    assert!(
        observed,
        "case the pending window: expected one of {ATTEMPTS} attempts to attach a certificate \
         the authority had not issued yet, and every one of them was issued first"
    );
}

/// RULE: Lightsail serves the distribution and certificate APIs from the
/// N. Virginia endpoint alone — the modelled `InvalidInputException`
/// documentation says to set the Region to us-east-1 "to create, view, or edit
/// these resources" — so an attach signed for any other Region is refused and
/// attaches nothing.
#[tokio::test]
async fn an_attach_outside_the_distribution_region_is_refused() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, "region-origin", "us-east-1a").await;
    create_certificate(&emulator, "region-cert", "region.example.com", &[]).await;
    wait_for_issued_certificate(&emulator, "region-cert").await;
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": "region-cdn",
                "bundleId": "small_1_0",
                "origin": {"name": "region-origin"},
                "defaultCacheBehavior": {"behavior": "cache"},
            }),
        )
        .await;

    // Rows spanning the Regions a caller might sign for: another Region this
    // service serves resources in, and one it does not.
    for region in ["us-west-2", "eu-central-1", "mars-central-1"] {
        let (status, body) = emulator
            .call_in(
                Some(region),
                "AttachCertificateToDistribution",
                json!({"distributionName": "region-cdn", "certificateName": "region-cert"}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case signed for {region}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case signed for {region}: expected InvalidInputException, got {body:?}"
        );
        let guarded = distribution(&emulator, "region-cdn").await;
        assert_eq!(
            guarded["certificateName"],
            Value::Null,
            "case signed for {region}: expected the refusal to attach nothing, got {guarded:?}"
        );
    }

    // And the same call in the distribution Region is served.
    emulator
        .ok_at(
            None,
            Some(DISTRIBUTION_REGION),
            "AttachCertificateToDistribution",
            json!({"distributionName": "region-cdn", "certificateName": "region-cert"}),
        )
        .await;
    let attached = distribution(&emulator, "region-cdn").await;
    assert_eq!(
        attached["certificateName"].as_str(),
        Some("region-cert"),
        "case signed for {DISTRIBUTION_REGION}: expected the attach to be served, got \
         {attached:?}"
    );
}
