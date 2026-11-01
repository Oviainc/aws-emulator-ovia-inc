// SPDX-License-Identifier: CC0-1.0

//! The domains a distribution publishes for the SSL/TLS certificate attached to
//! it, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a distribution that has an SSL/TLS certificate attached
//! publishes as its `alternativeDomainNames` exactly the domains that
//! certificate covers — its `domainName` first, then the alternate domains it
//! covers, in the order the certificate holds them — so the two members move
//! together whichever operation made the attachment: the create that names a
//! certificate for a new distribution ("the name of the SSL/TLS certificate that
//! you want to attach to the distribution"), the attach that sets one later, and
//! the update that does. A distribution with no certificate attached publishes
//! no alternate domain name of its own.
//!
//! The sibling suites pin the refusals of a certificate a distribution may not
//! serve (`distribution_create_certificate_validation.rs`) and the release of
//! the attachment (`distribution_certificate_detach.rs`).

use serde_json::{json, Value};

// The harness, the instance stand-up, the certificate stand-up, the issuance
// wait and the distribution read this test target already exposes: a sibling
// suite drives the service through the same front door, and waits for the same
// clock, rather than standing up a second one.
use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{
    certificate, create_certificate, distribution, string_list, wait_for_issued_certificate,
};
use crate::instance_lifecycle::Emulator;

/// The availability zone the origin instance of every case is created in.
const ORIGIN_ZONE: &str = "us-east-1a";

/// The origin every distribution of this suite pulls from.
const ORIGIN_NAME: &str = "domains-origin";

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Create the distribution `name` on `bundle_id`, naming `certificate_name` when
/// one is given, and answer the `distribution` the create reports.
async fn create_distribution(
    emulator: &Emulator,
    name: &str,
    bundle_id: &str,
    certificate_name: Option<&str>,
) -> Value {
    let mut body = json!({
        "distributionName": name,
        "bundleId": bundle_id,
        "origin": {"name": ORIGIN_NAME},
        "defaultCacheBehavior": {"behavior": "cache"},
    });
    if let Some(certificate_name) = certificate_name {
        body["certificateName"] = json!(certificate_name);
    }
    emulator.ok("CreateDistribution", body).await["distribution"].clone()
}

/// Every domain the certificate `name` covers, read off the certificate itself:
/// the domain it is for, then the alternate domains it covers.
///
/// Read through `GetCertificates` rather than assumed, so each case compares the
/// distribution's published domains against what the certificate record itself
/// says it secures.
async fn certificate_domains(emulator: &Emulator, name: &str) -> Vec<String> {
    let record = certificate(emulator, name).await;
    let detail = &record["certificateDetail"];
    let mut domains: Vec<String> = detail["domainName"]
        .as_str()
        .map(|domain| vec![domain.to_string()])
        .unwrap_or_default();
    domains.extend(string_list(&detail["subjectAlternativeNames"]));
    domains
}

/// How many resources the certificate `name` reports itself in use by.
async fn in_use_resource_count(emulator: &Emulator, name: &str) -> Option<i64> {
    certificate(emulator, name).await["certificateDetail"]["inUseResourceCount"].as_i64()
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One certificate a distribution is created with, and the domains that
/// attachment then publishes.
///
/// The rows differ along everything the rule generalises over: how many domains
/// the certificate covers (its primary domain alone, one alternate, several),
/// the ORDER the request listed the alternates in (the published list follows
/// the certificate, not the alphabet), a request whose alternates repeat the
/// primary domain and each other ("Lightsail will de-dupe the names for you", so
/// the published list is the de-duped one), and the bundle the distribution runs
/// on.
struct Row {
    case: &'static str,
    certificate_name: &'static str,
    /// The certificate's `domainName`.
    domain: &'static str,
    /// The `subjectAlternativeNames` the certificate request carried.
    alternatives: &'static [&'static str],
    distribution_name: &'static str,
    bundle_id: &'static str,
    /// The `alternativeDomainNames` a distribution the certificate is attached
    /// to publishes, in order.
    published: &'static [&'static str],
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "a certificate covering its primary domain alone",
        certificate_name: "domains-single-cert",
        domain: "single.example.com",
        alternatives: &[],
        distribution_name: "domains-single-cdn",
        bundle_id: "small_1_0",
        published: &["single.example.com"],
    },
    Row {
        case: "a certificate covering one alternate domain",
        certificate_name: "domains_www_cert",
        domain: "shop.example.net",
        alternatives: &["www.shop.example.net"],
        distribution_name: "domains-www-cdn",
        bundle_id: "medium_1_0",
        published: &["shop.example.net", "www.shop.example.net"],
    },
    Row {
        case: "a certificate covering three alternate domains, listed out of alphabetical order",
        certificate_name: "domains-many-cert",
        domain: "zeta.example.org",
        alternatives: &[
            "img.zeta.example.org",
            "api.zeta.example.org",
            "cdn.zeta.example.org",
        ],
        distribution_name: "domains-many-cdn",
        bundle_id: "large_1_0",
        published: &[
            "zeta.example.org",
            "img.zeta.example.org",
            "api.zeta.example.org",
            "cdn.zeta.example.org",
        ],
    },
    Row {
        case: "a certificate whose request repeated its primary domain and an alternate",
        certificate_name: "domains-repeat-cert",
        domain: "repeat.example.com",
        alternatives: &[
            "repeat.example.com",
            "www.repeat.example.com",
            "www.repeat.example.com",
        ],
        distribution_name: "domains-repeat-cdn",
        bundle_id: "small_1_0",
        published: &["repeat.example.com", "www.repeat.example.com"],
    },
];

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// RULE: a create that names a certificate makes the attachment, so the
/// distribution publishes exactly the domains that certificate covers, every
/// later read reports them, and the three doors onto the attachment — the
/// create, the attach and the update — report the same domains for the same
/// certificate.
#[tokio::test]
async fn a_distribution_publishes_the_domains_of_the_certificate_attached_to_it() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, ORIGIN_NAME, ORIGIN_ZONE).await;

    for row in ROWS {
        let case = row.case;
        create_certificate(&emulator, row.certificate_name, row.domain, row.alternatives).await;
        wait_for_issued_certificate(&emulator, row.certificate_name).await;
        let covered = certificate_domains(&emulator, row.certificate_name).await;
        assert_eq!(
            covered, row.published,
            "case {case}: expected the certificate to cover {:?}, got {covered:?}",
            row.published
        );

        // The create that names the certificate.
        let created = create_distribution(
            &emulator,
            row.distribution_name,
            row.bundle_id,
            Some(row.certificate_name),
        )
        .await;
        assert_eq!(
            created["certificateName"].as_str(),
            Some(row.certificate_name),
            "case {case}: expected the created distribution to name the certificate, got \
             {created:?}"
        );
        let published = string_list(&created["alternativeDomainNames"]);
        assert_eq!(
            published, covered,
            "case {case}: expected the create to publish the certificate's domains {covered:?}, \
             got {created:?}"
        );

        // The round trip: what the create stored is what a later read reports.
        let stored = distribution(&emulator, row.distribution_name).await;
        let stored_domains = string_list(&stored["alternativeDomainNames"]);
        assert_eq!(
            stored_domains, covered,
            "case {case}: expected a later read to report the certificate's domains {covered:?}, \
             got {stored:?}"
        );
        assert_eq!(
            stored["certificateName"].as_str(),
            Some(row.certificate_name),
            "case {case}: expected a later read to name the certificate, got {stored:?}"
        );

        // The other two doors onto the same attachment, each on a distribution
        // created without a certificate: both publish what the create did.
        let attach_name = format!("{}-attach", row.distribution_name);
        let bare = create_distribution(&emulator, &attach_name, row.bundle_id, None).await;
        assert_eq!(
            bare["certificateName"],
            Value::Null,
            "case {case}: expected a create naming no certificate to report none, got {bare:?}"
        );
        assert!(
            string_list(&bare["alternativeDomainNames"]).is_empty(),
            "case {case}: expected a distribution with no certificate attached to publish no \
             alternate domain name, got {bare:?}"
        );
        emulator
            .ok(
                "AttachCertificateToDistribution",
                json!({
                    "distributionName": attach_name,
                    "certificateName": row.certificate_name,
                }),
            )
            .await;
        let attached = distribution(&emulator, &attach_name).await;
        let attached_domains = string_list(&attached["alternativeDomainNames"]);
        assert_eq!(
            attached_domains, covered,
            "case {case}: expected the attach to publish the certificate's domains {covered:?}, \
             got {attached:?}"
        );

        let update_name = format!("{}-update", row.distribution_name);
        create_distribution(&emulator, &update_name, row.bundle_id, None).await;
        emulator
            .ok(
                "UpdateDistribution",
                json!({
                    "distributionName": update_name,
                    "certificateName": row.certificate_name,
                }),
            )
            .await;
        let updated = distribution(&emulator, &update_name).await;
        let updated_domains = string_list(&updated["alternativeDomainNames"]);
        assert_eq!(
            updated_domains, covered,
            "case {case}: expected the update to publish the certificate's domains {covered:?}, \
             got {updated:?}"
        );

        // The three distributions report the same attachment, so the
        // certificate counts all three.
        let counted = in_use_resource_count(&emulator, row.certificate_name).await;
        assert_eq!(
            counted,
            Some(3),
            "case {case}: expected the certificate counted by the three distributions it is \
             attached to, got {counted:?}"
        );
    }
}
