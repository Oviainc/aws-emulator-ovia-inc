// SPDX-License-Identifier: CC0-1.0

//! `GetLoadBalancerTlsCertificates` — reporting the SSL/TLS certificates a
//! Lightsail load balancer holds — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: the operation answers, from the persisted load balancer
//! the request names in the Region it is made in, one `LoadBalancerTlsCertificate`
//! per certificate that load balancer holds, in the order they were added —
//! whether `CreateLoadBalancer` or `CreateLoadBalancerTlsCertificate` asked for
//! them — each reporting the name, domain, de-duped alternative domains and tags
//! its request named, the members the service assigned from the certificate's own
//! minted id (ARN, support code, serial) and its validation state:
//! `PENDING_VALIDATION` with no issuance reported until the issuance the clock
//! owes it falls due, `ISSUED` with an issuer and a validity window afterwards. A
//! load balancer holding no certificate answers the empty list, a certificate an
//! `AttachLoadBalancerTlsCertificate` attached is the one reported attached, a
//! deleted certificate is reported no more, and a load balancer name this account
//! holds nothing under in the Region of the request is refused with the service's
//! `DoesNotExist` envelope.
//!
//! The rows differ along everything the rule generalises over: the Region, which
//! call asked for the certificate, how many certificates the load balancer holds
//! (none, one, the two it may hold), how many alternative domains each covers
//! (none, one, the nine the documentation admits), how many entries the
//! alternative-name list carries before Lightsail de-dupes it, and how many tags
//! the certificate carries.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

// The harness, the wire error code, the minted-id check and the minted-serial
// derivation this test target already exposes: a sibling suite drives the
// service through the same front door, and checks a minted serial the same way,
// rather than standing up a second one.
use crate::certificate_request::serial_number_of;
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The operation under test.
const OPERATION: &str = "GetLoadBalancerTlsCertificates";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN and
/// support code carries.
const ACCOUNT: &str = "000000000000";

/// The `ResourceType` an SSL/TLS certificate of a load balancer reports, from
/// the modelled `ResourceType` enum ("`LoadBalancerTlsCertificate` - An SSL/TLS
/// certificate associated with a Lightsail load balancer").
const CERTIFICATE_RESOURCE_TYPE: &str = "LoadBalancerTlsCertificate";

/// The `LoadBalancerTlsCertificateStatus` a certificate whose domains are still
/// being validated is in, and the one the certificate authority issues it into,
/// from the modelled enum.
const PENDING_VALIDATION: &str = "PENDING_VALIDATION";
/// See [`PENDING_VALIDATION`].
const ISSUED: &str = "ISSUED";

/// The `LoadBalancerTlsCertificateDomainStatus` of a domain whose validation is
/// still outstanding, and of one that has been validated, from the modelled
/// enum.
const DOMAIN_PENDING: &str = "PENDING_VALIDATION";
/// See [`DOMAIN_PENDING`].
const DOMAIN_SUCCESS: &str = "SUCCESS";

/// The `LoadBalancerTlsCertificateRenewalStatus` of a certificate Lightsail is
/// managing the renewal of, from the modelled enum. A certificate still being
/// validated for the first time has no managed renewal yet, so it reports no
/// renewal summary at all.
const RENEWAL_PENDING_AUTO_RENEWAL: &str = "PENDING_AUTO_RENEWAL";

/// The type of DNS record a domain of the certificate is validated through, from
/// the modelled `LoadBalancerTlsCertificateDomainValidationRecord$type`
/// documentation ("The type of validation record. For example, `CNAME` for
/// domain validation").
const VALIDATION_RECORD_TYPE: &str = "CNAME";

/// The algorithm the certificate's key pair is generated with, the algorithm it
/// is signed with, and the authority that issues it.
const KEY_ALGORITHM: &str = "RSA_2048";
/// See [`KEY_ALGORITHM`].
const SIGNATURE_ALGORITHM: &str = "SHA256WITHRSA";
/// See [`KEY_ALGORITHM`].
const ISSUER: &str = "Amazon";

/// How long an issued certificate is valid for, in seconds: AWS documents that
/// an ACM certificate — which a Lightsail SSL/TLS certificate is — is "valid for
/// 13 months (395 days)".
const VALIDITY_SECONDS: f64 = 395.0 * 24.0 * 60.0 * 60.0;

/// The wire error code of a request naming something the account does not hold,
/// and the value Lightsail puts in that body's `code` member — which a Lightsail
/// SDK client branches on rather than on the HTTP status.
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The `ResourceType` a load balancer reports, from the same enum — what the
/// refusal of an unknown load balancer names.
const LOAD_BALANCER_RESOURCE_TYPE: &str = "LoadBalancer";

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Now, in epoch seconds, for the window a reported `createdAt` must fall in.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// The certificates the load balancer `name` in `region` holds, as this
/// operation reports them.
async fn certificates(emulator: &Emulator, region: &str, name: &str) -> Vec<Value> {
    let body = emulator
        .ok_at(
            None,
            Some(region),
            OPERATION,
            json!({"loadBalancerName": name}),
        )
        .await;
    body["tlsCertificates"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a tlsCertificates array for {name}, got {body:?}"))
        .clone()
}

/// The one certificate named `certificate` the load balancer `name` in `region`
/// holds.
async fn certificate(emulator: &Emulator, region: &str, name: &str, certificate: &str) -> Value {
    certificates(emulator, region, name)
        .await
        .into_iter()
        .find(|reported| reported["name"].as_str() == Some(certificate))
        .unwrap_or_else(|| panic!("expected {name} to hold a certificate named {certificate}"))
}

/// Poll until the certificate `certificate` of the load balancer `name` is
/// reported `ISSUED`, answering it.
///
/// The issuance is owed by the clock, not by the poll: it arrives without anyone
/// asking more than once.
async fn wait_for_issued(
    emulator: &Emulator,
    region: &str,
    name: &str,
    certificate_name: &str,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reported = certificate(emulator, region, name, certificate_name).await;
        if reported["status"].as_str() == Some(ISSUED) {
            return reported;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for {certificate_name} on {name}: expected {ISSUED} within 5s, \
             got {reported:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The read table
// ---------------------------------------------------------------------------

/// One certificate a row asks for, and what the rule says the read then reports
/// for it.
#[derive(Debug, Clone, Copy)]
struct Certificate {
    name: &'static str,
    domain: &'static str,
    /// The alternative domains the request carries, as sent — repeats and the
    /// primary domain among them, which Lightsail de-dupes away.
    alternatives: &'static [&'static str],
    /// Every domain the certificate ends up covering, primary domain first:
    /// what `subjectAlternativeNames`, the validation records and the renewal
    /// options each carry one entry for.
    covered: &'static [&'static str],
    /// The tags the request carries, as (key, value) pairs.
    tags: &'static [(&'static str, &'static str)],
}

impl Certificate {
    /// The tags this certificate must report, as the wire carries them.
    fn expected_tags(&self) -> Value {
        Value::Array(
            self.tags
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect(),
        )
    }

    /// The `CreateLoadBalancerTlsCertificate` body that adds this certificate to
    /// `load_balancer`.
    fn add_request(&self, load_balancer: &str) -> Value {
        let mut body = serde_json::Map::new();
        body.insert("loadBalancerName".to_string(), json!(load_balancer));
        body.insert("certificateName".to_string(), json!(self.name));
        body.insert("certificateDomainName".to_string(), json!(self.domain));
        if !self.alternatives.is_empty() {
            body.insert(
                "certificateAlternativeNames".to_string(),
                json!(self.alternatives),
            );
        }
        if !self.tags.is_empty() {
            body.insert("tags".to_string(), self.expected_tags());
        }
        Value::Object(body)
    }
}

/// One load balancer, the certificate `CreateLoadBalancer` was asked for (if
/// any), and the certificates added to it afterwards in order.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    load_balancer: &'static str,
    instance_port: i64,
    created_with: Option<Certificate>,
    added: &'static [Certificate],
}

/// The nine alternative domains the documentation admits, for the row that
/// covers all of them, and the domains that certificate then covers.
const NINE_ALTERNATIVES: &[&str] = &[
    "a.apac.example.com",
    "b.apac.example.com",
    "c.apac.example.com",
    "d.apac.example.com",
    "e.apac.example.com",
    "f.apac.example.com",
    "g.apac.example.com",
    "h.apac.example.com",
    "i.apac.example.com",
];
/// See [`NINE_ALTERNATIVES`]: the primary domain first, then those nine.
const NINE_COVERED: &[&str] = &[
    "apac.example.com",
    "a.apac.example.com",
    "b.apac.example.com",
    "c.apac.example.com",
    "d.apac.example.com",
    "e.apac.example.com",
    "f.apac.example.com",
    "g.apac.example.com",
    "h.apac.example.com",
    "i.apac.example.com",
];

const ROWS: &[Row] = &[
    Row {
        case: "one certificate with no alternative domains and no tags",
        region: "us-east-1",
        load_balancer: "shop-lb",
        instance_port: 80,
        created_with: None,
        added: &[Certificate {
            name: "shop-cert",
            domain: "example.com",
            alternatives: &[],
            covered: &["example.com"],
            tags: &[],
        }],
    },
    Row {
        case: "the two certificates one load balancer may hold, one tagged",
        region: "eu-west-1",
        load_balancer: "eu-edge-lb",
        instance_port: 8080,
        created_with: None,
        added: &[
            Certificate {
                name: "eu-edge-primary",
                domain: "eu.example.com",
                alternatives: &["www.eu.example.com"],
                covered: &["eu.example.com", "www.eu.example.com"],
                tags: &[("tier", "edge"), ("owner", "platform")],
            },
            Certificate {
                name: "eu-edge-backup",
                domain: "eu.example.net",
                alternatives: &["www.eu.example.net", "shop.eu.example.net"],
                covered: &[
                    "eu.example.net",
                    "www.eu.example.net",
                    "shop.eu.example.net",
                ],
                tags: &[],
            },
        ],
    },
    Row {
        case: "the nine alternative domains the documentation admits",
        region: "ap-northeast-1",
        load_balancer: "apac-lb-01",
        instance_port: 3000,
        created_with: None,
        added: &[Certificate {
            name: "apac-cert-01",
            domain: "apac.example.com",
            alternatives: NINE_ALTERNATIVES,
            covered: NINE_COVERED,
            tags: &[("env", "prod")],
        }],
    },
    Row {
        case: "the certificate CreateLoadBalancer was asked for, beside one added later",
        region: "us-west-2",
        load_balancer: "west-lb-2",
        instance_port: 65535,
        created_with: Some(Certificate {
            name: "west-cert-2026-01",
            domain: "west.example.org",
            // The list repeats a name and names the primary domain, both of
            // which Lightsail de-dupes away: "Lightsail will de-dupe the names
            // for you."
            alternatives: &[
                "cdn.west.example.org",
                "cdn.west.example.org",
                "west.example.org",
            ],
            covered: &["west.example.org", "cdn.west.example.org"],
            tags: &[],
        }),
        added: &[Certificate {
            name: "west_cert_backup",
            domain: "backup.west.example.org",
            alternatives: &[],
            covered: &["backup.west.example.org"],
            tags: &[("role", "backup")],
        }],
    },
];

impl Row {
    /// The `CreateLoadBalancer` request this row's load balancer is created by.
    fn create_load_balancer(&self) -> Value {
        let mut body = serde_json::Map::new();
        body.insert("loadBalancerName".to_string(), json!(self.load_balancer));
        body.insert("instancePort".to_string(), json!(self.instance_port));
        if let Some(certificate) = self.created_with {
            body.insert("certificateName".to_string(), json!(certificate.name));
            body.insert(
                "certificateDomainName".to_string(),
                json!(certificate.domain),
            );
            if !certificate.alternatives.is_empty() {
                body.insert(
                    "certificateAlternativeNames".to_string(),
                    json!(certificate.alternatives),
                );
            }
        }
        Value::Object(body)
    }

    /// Every certificate this row's load balancer ends up holding, in the order
    /// they were added.
    fn expected(&self) -> Vec<Certificate> {
        self.created_with
            .into_iter()
            .chain(self.added.iter().copied())
            .collect()
    }
}

/// Assert everything the rule says a read reports for `expected`, except the
/// validation state (which the clock moves) — so the same assertions hold on
/// every read. `created_after` is an instant before the certificate was asked
/// for, which its creation time must not precede.
fn assert_identity(
    case: &str,
    reported: &Value,
    row: &Row,
    expected: &Certificate,
    created_after: f64,
) -> String {
    assert_eq!(
        reported["name"].as_str(),
        Some(expected.name),
        "case {case}: expected the requested certificate name, got {reported:?}"
    );
    assert_eq!(
        reported["loadBalancerName"].as_str(),
        Some(row.load_balancer),
        "case {case}: a certificate is reported on the load balancer that holds it, \
         got {reported:?}"
    );
    assert_eq!(
        reported["domainName"].as_str(),
        Some(expected.domain),
        "case {case}: expected the requested domain, got {reported:?}"
    );
    assert_eq!(
        reported["resourceType"].as_str(),
        Some(CERTIFICATE_RESOURCE_TYPE),
        "case {case}: expected the {CERTIFICATE_RESOURCE_TYPE} resource type, got {reported:?}"
    );
    assert_eq!(
        reported["tags"],
        expected.expected_tags(),
        "case {case}: expected the tags the request named, got {:?}",
        reported["tags"]
    );

    // Server-assigned identity: every member computed from the certificate's own
    // minted id, which the ARN carries.
    let arn = reported["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an arn, got {reported:?}"));
    let arn_prefix = format!(
        "arn:aws:lightsail:{}:{ACCOUNT}:{CERTIFICATE_RESOURCE_TYPE}/",
        row.region
    );
    let id = arn
        .strip_prefix(&arn_prefix)
        .unwrap_or_else(|| panic!("case {case}: expected an arn '{arn_prefix}<id>', got {arn:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped certificate id, got {id:?}"
    );
    assert_eq!(
        reported["supportCode"].as_str(),
        Some(format!("{ACCOUNT}/{id}").as_str()),
        "case {case}: expected the account/id support code, got {:?}",
        reported["supportCode"]
    );
    assert_eq!(
        reported["serial"].as_str(),
        Some(serial_number_of(id).as_str()),
        "case {case}: expected a serial number built from this certificate's id, got {:?}",
        reported["serial"]
    );
    let created_at = reported["createdAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a createdAt, got {reported:?}"));
    assert!(
        created_at >= created_after - 300.0 && created_at <= now_epoch_seconds() + 300.0,
        "case {case}: expected the creation time to be now, got {created_at}"
    );

    // A load balancer is regional, and so is the certificate it holds: Lightsail
    // spreads the load balancer across the Region's Availability Zones, so
    // neither names one.
    assert_eq!(
        reported["location"]["regionName"].as_str(),
        Some(row.region),
        "case {case}: expected the Region the load balancer lives in, got {:?}",
        reported["location"]
    );
    assert!(
        reported["location"]["availabilityZone"].is_null(),
        "case {case}: a load balancer certificate is regional, so it must report no \
         availability zone, got {:?}",
        reported["location"]
    );

    // The X.509 members: the subject names the primary domain, and the subject
    // alternative names cover every domain the certificate secures.
    assert_eq!(
        reported["subject"].as_str(),
        Some(format!("CN={}", expected.domain).as_str()),
        "case {case}: expected the subject to name the certificate's domain, got {:?}",
        reported["subject"]
    );
    assert_eq!(
        reported["keyAlgorithm"].as_str(),
        Some(KEY_ALGORITHM),
        "case {case}: expected the {KEY_ALGORITHM} key algorithm, got {reported:?}"
    );
    assert_eq!(
        reported["signatureAlgorithm"].as_str(),
        Some(SIGNATURE_ALGORITHM),
        "case {case}: expected the {SIGNATURE_ALGORITHM} signature algorithm, got {reported:?}"
    );
    let alternative_names: Vec<&str> = reported["subjectAlternativeNames"]
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    assert_eq!(
        alternative_names, expected.covered,
        "case {case}: expected every domain the certificate covers, de-duped, got {:?}",
        reported["subjectAlternativeNames"]
    );

    // Nothing here fails or revokes a certificate.
    for member in ["failureReason", "revocationReason", "revokedAt"] {
        assert!(
            reported[member].is_null(),
            "case {case}: expected no {member} on a certificate that neither failed nor was \
             revoked, got {reported:?}"
        );
    }
    id.to_string()
}

/// Assert the validation state the rule says a certificate reports: one record
/// per domain it covers, each in `domain_status`, and the certificate itself in
/// `status`. `renewal_status` is the status its managed renewal is in, or `None`
/// where the certificate has no managed renewal yet and must report no renewal
/// summary — in which case the renewal options are not reported either.
fn assert_validation(
    case: &str,
    reported: &Value,
    expected: &Certificate,
    status: &str,
    domain_status: &str,
    renewal_status: Option<&str>,
) {
    assert_eq!(
        reported["status"].as_str(),
        Some(status),
        "case {case}: expected the certificate to be {status}, got {reported:?}"
    );
    let records = reported["domainValidationRecords"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        records.len(),
        expected.covered.len(),
        "case {case}: expected one validation record per domain the certificate covers \
         ({:?}), got {records:?}",
        expected.covered
    );
    for (record, domain) in records.iter().zip(expected.covered) {
        assert_eq!(
            record["domainName"].as_str(),
            Some(*domain),
            "case {case}: expected a validation record for {domain}, got {record:?}"
        );
        assert_eq!(
            record["name"].as_str(),
            Some(*domain),
            "case {case}: expected the record to name the fully qualified domain {domain}, \
             got {record:?}"
        );
        assert_eq!(
            record["type"].as_str(),
            Some(VALIDATION_RECORD_TYPE),
            "case {case}: expected a {VALIDATION_RECORD_TYPE} validation record, got {record:?}"
        );
        assert_eq!(
            record["validationStatus"].as_str(),
            Some(domain_status),
            "case {case}: expected the validation of {domain} to be {domain_status}, \
             got {record:?}"
        );
        // This emulator publishes no validation record in any DNS zone, so there
        // is no value a caller could create and no automatic creation of one to
        // report.
        for member in ["value", "dnsRecordCreationState"] {
            assert!(
                record[member].is_null(),
                "case {case}: expected no {member} on a record nothing published, got {record:?}"
            );
        }
    }
    let Some(renewal_status) = renewal_status else {
        assert!(
            reported["renewalSummary"].is_null(),
            "case {case}: nothing renews a certificate that was never issued, so it must \
             report no renewal summary, got {:?}",
            reported["renewalSummary"]
        );
        return;
    };
    let renewal = reported["renewalSummary"].clone();
    assert_eq!(
        renewal["renewalStatus"].as_str(),
        Some(renewal_status),
        "case {case}: expected the renewal to be {renewal_status}, got {renewal:?}"
    );
    let options: Vec<(&str, &str)> = renewal["domainValidationOptions"]
        .as_array()
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    Some((
                        option["domainName"].as_str()?,
                        option["validationStatus"].as_str()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let want: Vec<(&str, &str)> = expected
        .covered
        .iter()
        .map(|domain| (*domain, domain_status))
        .collect();
    assert_eq!(
        options, want,
        "case {case}: expected one renewal option per covered domain, got {renewal:?}"
    );
}

/// Every row: what the creates stored is what this operation reports, in the
/// order the certificates were added, pending validation at first and issued
/// once the clock has made the issuance due.
#[tokio::test(flavor = "current_thread")]
async fn get_load_balancer_tls_certificates_reports_every_stored_certificate() {
    let emulator = Emulator::new();
    for row in ROWS {
        let case = row.case;
        let created_after = now_epoch_seconds();
        emulator
            .ok_at(
                None,
                Some(row.region),
                "CreateLoadBalancer",
                row.create_load_balancer(),
            )
            .await;
        for certificate in row.added {
            emulator
                .ok_at(
                    None,
                    Some(row.region),
                    "CreateLoadBalancerTlsCertificate",
                    certificate.add_request(row.load_balancer),
                )
                .await;
        }

        // A read straight after the creates: every certificate the load balancer
        // holds, in the order it was added, and none of them validated yet.
        let expected = row.expected();
        let reported = certificates(&emulator, row.region, row.load_balancer).await;
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {case}: expected one entry per certificate held, got {reported:?}"
        );
        let mut identities = Vec::new();
        for (reported, expected) in reported.iter().zip(&expected) {
            let case = format!("{case} / {}", expected.name);
            let id = assert_identity(&case, reported, row, expected, created_after);
            assert_eq!(
                reported["isAttached"].as_bool(),
                Some(false),
                "case {case}: nothing has attached this certificate, got {reported:?}"
            );
            assert_validation(
                &case,
                reported,
                expected,
                PENDING_VALIDATION,
                DOMAIN_PENDING,
                None,
            );
            for member in ["issuedAt", "issuer", "notBefore", "notAfter"] {
                assert!(
                    reported[member].is_null(),
                    "case {case}: expected no {member} before the certificate is issued, \
                     got {reported:?}"
                );
            }
            identities.push((id, reported["createdAt"].clone()));
        }

        // The issuance the clock owes every one of them: it arrives without
        // anyone asking, and leaves everything the request stored alone.
        for ((id, created_at), expected) in identities.iter().zip(&expected) {
            let case = format!("{case} / {}", expected.name);
            let issued =
                wait_for_issued(&emulator, row.region, row.load_balancer, expected.name).await;
            assert_validation(
                &case,
                &issued,
                expected,
                ISSUED,
                DOMAIN_SUCCESS,
                Some(RENEWAL_PENDING_AUTO_RENEWAL),
            );
            assert_eq!(
                issued["issuer"].as_str(),
                Some(ISSUER),
                "case {case}: expected {ISSUER} as the issuer, got {issued:?}"
            );
            assert_eq!(
                issued["arn"].as_str(),
                Some(format!("arn:aws:lightsail:{}:{ACCOUNT}:{CERTIFICATE_RESOURCE_TYPE}/{id}", row.region).as_str()),
                "case {case}: expected issuance to leave the stored ARN alone, got {issued:?}"
            );
            assert_eq!(
                issued["createdAt"], *created_at,
                "case {case}: expected issuance to leave the creation time alone, got {issued:?}"
            );
            let not_before = issued["notBefore"].as_f64().unwrap_or_else(|| {
                panic!("case {case}: expected an issued certificate to report notBefore, got {issued:?}")
            });
            let not_after = issued["notAfter"].as_f64().unwrap_or_else(|| {
                panic!("case {case}: expected an issued certificate to report notAfter, got {issued:?}")
            });
            assert_eq!(
                issued["issuedAt"].as_f64(),
                Some(not_before),
                "case {case}: a certificate is first valid when it is issued, got {issued:?}"
            );
            assert!(
                (not_after - not_before - VALIDITY_SECONDS).abs() < 1.0,
                "case {case}: expected a validity window of {VALIDITY_SECONDS} seconds, got \
                 {not_before} to {not_after}"
            );

            // Reading changes nothing: a second read of a settled certificate
            // reports exactly what the first one did.
            let again = certificate(&emulator, row.region, row.load_balancer, expected.name).await;
            assert_eq!(
                again, issued,
                "case {case}: a read must not change what the next read reports, got {again:?}"
            );
        }
    }
}

/// A load balancer that holds no certificate reports the empty list — not a
/// refusal, and not a certificate it does not hold.
#[tokio::test(flavor = "current_thread")]
async fn a_load_balancer_holding_no_certificate_reports_an_empty_list() {
    let emulator = Emulator::new();
    let case = "a load balancer created without a certificate";
    let region = "us-east-2";
    emulator
        .ok_at(
            None,
            Some(region),
            "CreateLoadBalancer",
            json!({"loadBalancerName": "bare-lb", "instancePort": 80}),
        )
        .await;
    let reported = certificates(&emulator, region, "bare-lb").await;
    assert!(
        reported.is_empty(),
        "case {case}: expected no certificates, got {reported:?}"
    );

    // And once its only certificate is deleted, it holds none again.
    let case = "a load balancer whose only certificate was deleted";
    emulator
        .ok_at(
            None,
            Some(region),
            "CreateLoadBalancerTlsCertificate",
            json!({
                "loadBalancerName": "bare-lb",
                "certificateName": "bare-cert",
                "certificateDomainName": "bare.example.com",
            }),
        )
        .await;
    assert_eq!(
        certificates(&emulator, region, "bare-lb").await.len(),
        1,
        "case {case}: the certificate must be reported before it is deleted"
    );
    emulator
        .ok_at(
            None,
            Some(region),
            "DeleteLoadBalancerTlsCertificate",
            json!({"loadBalancerName": "bare-lb", "certificateName": "bare-cert"}),
        )
        .await;
    let reported = certificates(&emulator, region, "bare-lb").await;
    assert!(
        reported.is_empty(),
        "case {case}: expected the deleted certificate to be reported no more, got {reported:?}"
    );
}

/// A load balancer terminates HTTPS with ONE certificate, so the read reports
/// exactly the attached one as attached — and follows the attachment when it
/// moves to the other certificate.
#[tokio::test(flavor = "current_thread")]
async fn the_read_reports_which_certificate_is_attached() {
    let emulator = Emulator::new();
    let region = "ca-central-1";
    let load_balancer = "attach-lb";
    emulator
        .ok_at(
            None,
            Some(region),
            "CreateLoadBalancer",
            json!({"loadBalancerName": load_balancer, "instancePort": 80}),
        )
        .await;
    for (name, domain) in [
        ("attach-primary", "attach.example.com"),
        ("attach-backup", "attach.example.net"),
    ] {
        emulator
            .ok_at(
                None,
                Some(region),
                "CreateLoadBalancerTlsCertificate",
                json!({
                    "loadBalancerName": load_balancer,
                    "certificateName": name,
                    "certificateDomainName": domain,
                }),
            )
            .await;
    }

    // Each attachment in turn: the certificate named is the one reported
    // attached, and every other certificate of the load balancer is not.
    for attached in ["attach-primary", "attach-backup", "attach-primary"] {
        let case = format!("attaching {attached}");
        emulator
            .ok_at(
                None,
                Some(region),
                "AttachLoadBalancerTlsCertificate",
                json!({"loadBalancerName": load_balancer, "certificateName": attached}),
            )
            .await;
        let reported: Vec<(String, bool)> = certificates(&emulator, region, load_balancer)
            .await
            .iter()
            .map(|certificate| {
                (
                    certificate["name"].as_str().unwrap_or_default().to_string(),
                    certificate["isAttached"].as_bool().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            reported,
            vec![
                (
                    "attach-primary".to_string(),
                    attached == "attach-primary"
                ),
                ("attach-backup".to_string(), attached == "attach-backup"),
            ],
            "case {case}: exactly the attached certificate must be reported attached, \
             got {reported:?}"
        );
    }
}

/// One `GetLoadBalancerTlsCertificates` request that names a load balancer the
/// account does not hold where the request is made.
struct Refusal {
    case: &'static str,
    /// The Region the read is made in.
    region: &'static str,
    name: &'static str,
}

/// The load balancer the refusal table creates, and the Region it creates it in.
const REFUSAL_LOAD_BALANCER: &str = "sa-lb";
/// See [`REFUSAL_LOAD_BALANCER`].
const REFUSAL_REGION: &str = "sa-east-1";

const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a load balancer nothing ever created",
        region: REFUSAL_REGION,
        name: "never-created-lb",
    },
    Refusal {
        case: "a load balancer held in another Region",
        region: "eu-central-1",
        name: REFUSAL_LOAD_BALANCER,
    },
    Refusal {
        case: "a load balancer that was deleted",
        region: REFUSAL_REGION,
        name: "deleted-lb",
    },
];

/// Every refusal row: a well-formed name that names no load balancer of this
/// account in the Region of the request is the service's `DoesNotExist`
/// envelope, never an empty list.
#[tokio::test(flavor = "current_thread")]
async fn an_unknown_load_balancer_is_refused() {
    let emulator = Emulator::new();
    for (name, region) in [
        (REFUSAL_LOAD_BALANCER, REFUSAL_REGION),
        ("deleted-lb", REFUSAL_REGION),
    ] {
        emulator
            .ok_at(
                None,
                Some(region),
                "CreateLoadBalancer",
                json!({"loadBalancerName": name, "instancePort": 80}),
            )
            .await;
    }
    emulator
        .ok_at(
            None,
            Some(REFUSAL_REGION),
            "DeleteLoadBalancer",
            json!({"loadBalancerName": "deleted-lb"}),
        )
        .await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let (status, body) = emulator
            .call_in(
                Some(refusal.region),
                OPERATION,
                json!({"loadBalancerName": refusal.name}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the not-found wire status, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            NOT_FOUND,
            "case {case}: expected the {NOT_FOUND} wire error, got {body:?}"
        );
        assert_eq!(
            body["code"].as_str(),
            Some(DOES_NOT_EXIST),
            "case {case}: expected the {DOES_NOT_EXIST} body code, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(
                format!(
                    "The {LOAD_BALANCER_RESOURCE_TYPE} does not exist: {}",
                    refusal.name
                )
                .as_str()
            ),
            "case {case}: expected the message to name the resource type and the requested \
             name, got {body:?}"
        );
    }
}
