// SPDX-License-Identifier: CC0-1.0

//! How a reporting operation renders a Lightsail SSL/TLS certificate, driven
//! over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness, the certificate stand-up and the issuance wait are the
//! ones the sibling suites already expose.
//!
//! The rule under test: a reported certificate is assembled ONE way — the stored
//! record paired with `Certificate$inUseResourceCount`, "the number of Lightsail
//! resources that the certificate is attached to", which is counted from the
//! whole store rather than from the record — so `GetCertificates` reports the
//! same certificate identically whether the request NAMES it or the response
//! pages the account's certificates, with and without
//! `includeCertificateDetails`.
//!
//! So the case is a TABLE whose rows differ along everything that assembly
//! generalises over: how many domains the certificate covers, whether it carries
//! tags, and how many resources of the account reference it and of which kinds (a
//! distribution naming it in `certificateName`, a container service whose
//! `publicDomainNames` are keyed by it). The account holds more certificates than
//! one page carries, so the rows are read back from different pages of the
//! listing and the comparison covers a page the walk reached through a page
//! token.

use std::collections::BTreeMap;

use serde_json::{json, Value};

// The harness, the origin stand-up, the certificate stand-up with its issuance
// wait, and the public-domain-names conversion this test target already exposes:
// a sibling suite drives the service through the same front door rather than
// standing up a second one.
use crate::container_service_public_domain_certificate::public_domain_names;
use crate::disk_attachment::create_running_instance;
use crate::distribution_certificate_attachment::{issue_certificate, wait_for_issued_certificate};
use crate::instance_lifecycle::Emulator;

/// The Availability Zone the origin instance of every distribution of this suite
/// is created in: the first zone of the one Region the certificate API serves.
const ORIGIN_ZONE: &str = "us-east-1a";

/// The instance every distribution of this suite pulls its content from.
const ORIGIN: &str = "parity-origin";

/// The bundle every distribution of this suite is created with, and the power and
/// scale of every container service: the rule under test is about how a
/// certificate is reported, not about capacity.
const DISTRIBUTION_BUNDLE: &str = "small_1_0";
/// See [`DISTRIBUTION_BUNDLE`].
const POWER: &str = "nano";
/// See [`DISTRIBUTION_BUNDLE`].
const SCALE: i64 = 1;

/// How many certificates one page of `GetCertificates` carries
/// (`crate::state::CERTIFICATE_PAGE_SIZE`), so the suite can hold more than that
/// and read its rows off more than one page.
const PAGE_SIZE: usize = 10;

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One container service referencing the certificate of a row: where it lives
/// and the domains it serves under that certificate.
struct Service {
    region: &'static str,
    name: &'static str,
    domains: &'static [&'static str],
}

/// One certificate, the way it was requested and the resources of the account
/// that reference it.
struct Row {
    case: &'static str,
    /// The certificate's name. Every row sorts after every filler below, so the
    /// rows land on the pages the listing reaches last.
    certificate_name: &'static str,
    /// The domains the certificate covers; the first is its `domainName` and the
    /// rest its alternate domains.
    domains: &'static [&'static str],
    /// The `(key, value)` tags the request named, if any.
    tags: &'static [(&'static str, &'static str)],
    /// The distributions whose `certificateName` names it.
    distributions: &'static [&'static str],
    /// The container services whose `publicDomainNames` it keys.
    services: &'static [Service],
}

impl Row {
    /// How many resources of the account reference this row's certificate, which
    /// is what its `inUseResourceCount` has to report.
    fn in_use_resource_count(&self) -> i64 {
        i64::try_from(self.distributions.len() + self.services.len())
            .expect("a table row names a handful of resources")
    }
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "one domain, no tags, nothing referencing it",
        certificate_name: "parity-row-unused",
        domains: &["unused.example.com"],
        tags: &[],
        distributions: &[],
        services: &[],
    },
    Row {
        case: "three domains and tags, one distribution referencing it",
        certificate_name: "parity-row-tagged",
        domains: &[
            "tagged.example.com",
            "www.tagged.example.com",
            "api.tagged.example.com",
        ],
        tags: &[("owner", "parity"), ("stage", "test")],
        distributions: &["parity-tagged-cdn"],
        services: &[],
    },
    Row {
        case: "a distribution and a container service of another Region referencing it",
        certificate_name: "parity-row-mixed",
        domains: &["mixed.example.com", "www.mixed.example.com"],
        tags: &[("owner", "parity")],
        distributions: &["parity-mixed-cdn"],
        services: &[Service {
            region: "eu-west-1",
            name: "parity-mixed-service",
            domains: &["mixed.example.com"],
        }],
    },
];

/// The certificates the account holds beside the rows, so the listing spans more
/// than one page and the rows are reported from pages the walk reaches through a
/// page token. Their names sort BEFORE every row's.
const FILLERS: usize = 8;

/// The name of filler `index`.
fn filler_name(index: usize) -> String {
    format!("parity-filler-{index:02}")
}

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// The certificate `name` as `GetCertificates` reports it when the request NAMES
/// it — the single-record read.
async fn named_certificate(emulator: &Emulator, name: &str, include_details: bool) -> Value {
    let (status, body) = emulator
        .call(
            "GetCertificates",
            json!({
                "certificateName": name,
                "includeCertificateDetails": include_details,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case reading the named certificate {name} (details {include_details}): expected HTTP \
         200, got {status} {body:?}"
    );
    body["certificates"]
        .as_array()
        .and_then(|certificates| certificates.first())
        .cloned()
        .unwrap_or(Value::Null)
}

/// Every certificate the account holds as the LISTING reports it, keyed by name,
/// plus how many pages the walk took.
async fn listed_certificates(
    emulator: &Emulator,
    include_details: bool,
) -> (BTreeMap<String, Value>, usize) {
    let mut listed: BTreeMap<String, Value> = BTreeMap::new();
    let mut pages = 0;
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({"includeCertificateDetails": include_details});
        if let Some(token) = &token {
            request["pageToken"] = json!(token);
        }
        let (status, body) = emulator.call("GetCertificates", request).await;
        assert_eq!(
            status, 200,
            "case listing certificates (details {include_details}): expected HTTP 200, got \
             {status} {body:?}"
        );
        pages += 1;
        let page = body["certificates"].as_array().cloned().unwrap_or_default();
        for entry in page {
            let name = entry["certificateName"]
                .as_str()
                .unwrap_or("<not a string>")
                .to_string();
            listed.insert(name, entry);
        }
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            pages < 100,
            "case listing certificates (details {include_details}): the walk did not terminate \
             after {pages} pages"
        );
    }
    (listed, pages)
}

/// Request the certificate of `row` and wait until the certificate authority has
/// issued it, so the resources that reference it can be stood up.
async fn stand_up_certificate(emulator: &Emulator, row: &Row) {
    let (domain, alternatives) = row
        .domains
        .split_first()
        .expect("a certificate secures at least one domain");
    let mut body = json!({"certificateName": row.certificate_name, "domainName": domain});
    if !alternatives.is_empty() {
        body["subjectAlternativeNames"] = json!(alternatives);
    }
    if !row.tags.is_empty() {
        body["tags"] = Value::Array(
            row.tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        );
    }
    let (status, created) = emulator.call("CreateCertificate", body).await;
    assert_eq!(
        status, 200,
        "case {}: expected the certificate to be requested, got {status} {created:?}",
        row.case
    );
}

/// Stand up every resource of `row` that references its certificate.
async fn stand_up_references(emulator: &Emulator, row: &Row) {
    for distribution in row.distributions {
        let (status, created) = emulator
            .call(
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
        assert_eq!(
            status, 200,
            "case {}: expected the distribution {distribution} to be created, got {status} \
             {created:?}",
            row.case
        );
    }
    for service in row.services {
        let keyed: &[(&str, &[&str])] = &[(row.certificate_name, service.domains)];
        let mut body = json!({"serviceName": service.name, "power": POWER, "scale": SCALE});
        body["publicDomainNames"] = public_domain_names(keyed);
        let (status, created) = emulator
            .call_in(Some(service.region), "CreateContainerService", body)
            .await;
        assert_eq!(
            status, 201,
            "case {}: expected the container service {} to be created, got {status} {created:?}",
            row.case, service.name
        );
    }
}

/// RULE: a certificate is assembled one way for every reporting operation — the
/// stored record paired with the number of resources of the account that
/// reference it — so the single-record read and the listing report the very same
/// certificate, whether or not the request asks for the full detail.
#[tokio::test]
async fn a_certificate_reads_the_same_whether_it_is_named_or_listed() {
    let emulator = Emulator::new();
    create_running_instance(&emulator, ORIGIN, ORIGIN_ZONE).await;

    // The account holds more certificates than one page carries, so the rows are
    // reported from pages the walk reaches through a page token.
    for index in 0..FILLERS {
        let name = filler_name(index);
        let domain = format!("filler-{index:02}.example.com");
        issue_certificate(&emulator, &name, &[domain.as_str()]).await;
    }
    for row in ROWS {
        stand_up_certificate(&emulator, row).await;
    }
    // The issuance is owed by the clock: waiting for it leaves every certificate
    // of the account in a settled status, so the two reads below cannot straddle
    // a transition the clock made between them.
    for row in ROWS {
        wait_for_issued_certificate(&emulator, row.certificate_name).await;
    }
    for row in ROWS {
        stand_up_references(&emulator, row).await;
    }

    let held = FILLERS + ROWS.len();
    assert!(
        held > PAGE_SIZE,
        "case the stand-up: expected the account to hold more than one page of {PAGE_SIZE} \
         certificates, got {held}"
    );

    for include_details in [false, true] {
        let (listed, pages) = listed_certificates(&emulator, include_details).await;
        assert!(
            pages > 1,
            "case listing certificates (details {include_details}): expected the {held} held \
             certificates to span more than one page, got {pages}"
        );
        assert_eq!(
            listed.len(),
            held,
            "case listing certificates (details {include_details}): expected every held \
             certificate to be listed once, got {:?}",
            listed.keys().collect::<Vec<&String>>()
        );

        for row in ROWS {
            let case = row.case;
            let named = named_certificate(&emulator, row.certificate_name, include_details).await;
            let from_page = listed.get(row.certificate_name).cloned().unwrap_or(Value::Null);
            assert_eq!(
                from_page, named,
                "case {case} (details {include_details}): expected the listed certificate to be \
                 the one the named read reports ({named:?}), got {from_page:?}"
            );

            // And both report the assembly's own member: the resources of the
            // account that reference the certificate, counted from their
            // records, reported only with the full detail.
            let want = row.in_use_resource_count();
            for (path, reported) in [("named", &named), ("listed", &from_page)] {
                let detail = &reported["certificateDetail"];
                if include_details {
                    let got = detail["inUseResourceCount"].as_i64();
                    assert_eq!(
                        got,
                        Some(want),
                        "case {case} ({path} read): expected an inUseResourceCount of {want}, got \
                         {reported:?}"
                    );
                } else {
                    assert!(
                        detail.is_null(),
                        "case {case} ({path} read): expected no certificateDetail without \
                         includeCertificateDetails, got {reported:?}"
                    );
                }
            }
        }
    }
}
