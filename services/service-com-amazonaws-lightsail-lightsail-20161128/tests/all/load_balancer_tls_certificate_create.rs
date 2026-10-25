// SPDX-License-Identifier: CC0-1.0

//! `CreateLoadBalancerTlsCertificate` — adding an SSL/TLS certificate to a
//! Lightsail load balancer — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a `CreateLoadBalancerTlsCertificate` call adds ONE
//! certificate record — the name, the primary domain and the de-duped
//! alternative domains the request asked for — to the load balancer its
//! `loadBalancerName` names in the Region the request is made in, UNATTACHED, and
//! answers one terminal operation-log entry for it; the load balancer then
//! reports a summary for that certificate and is otherwise exactly as it was,
//! still serving plain `HTTP` on port 80 because nothing has attached the
//! certificate yet. A request that names a load balancer the account does not
//! hold in that Region, a certificate name the load balancer already holds, a
//! domain that is not a domain name, more alternative domains than one
//! certificate may cover, or that would take the load balancer or the account
//! past the certificates it may hold, creates nothing and is refused.
//!
//! The rows differ along everything the rule generalises over: the Region, the
//! load balancer's instance port, the certificate's name form and domain, how
//! many alternative domains it covers (none, one, the nine the documentation
//! admits), how many certificates the load balancer already holds (none, one
//! from `CreateLoadBalancer`, one from an earlier call of this operation), and
//! how many entries the alternative-name list carries before Lightsail de-dupes
//! it.

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state::MAX_DOMAIN_NAME_LIST_ENTRIES;

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The operation under test.
const OPERATION: &str = "CreateLoadBalancerTlsCertificate";

/// The `ResourceType` an SSL/TLS certificate of a load balancer reports, from
/// the modelled `ResourceType` enum.
const CERTIFICATE_RESOURCE_TYPE: &str = "LoadBalancerTlsCertificate";

/// The `ResourceType` a load balancer reports, from the same enum.
const LOAD_BALANCER_RESOURCE_TYPE: &str = "LoadBalancer";

/// The public port a load balancer with no attached certificate listens on, from
/// the modelled `LoadBalancer$publicPorts` documentation ("For HTTP, use port
/// 80").
const HTTP_PORT: i64 = 80;

/// The protocol a load balancer with no ATTACHED certificate serves, from the
/// modelled `LoadBalancer$protocol` documentation ("You can't just have
/// `HTTP_HTTPS`, but you can have just `HTTP`").
const HTTP_ONLY: &str = "HTTP";

/// The most certificates one load balancer may hold at a time, from the modelled
/// `CreateLoadBalancerTlsCertificate$certificateName` documentation: "Each
/// Lightsail load balancer can have up to 2 certificates associated with it at
/// one time."
const CERTIFICATES_PER_LOAD_BALANCER: usize = 2;

/// The most certificates the account may hold at a time, from the same
/// documentation: "You can have up to 10 certificates in your account at one
/// time."
const CERTIFICATES_PER_ACCOUNT: usize = 10;

/// The most tags one resource may carry, from the modelled `TagResource`
/// documentation ("Each resource can have a maximum of 50 tags").
const MAX_TAGS: usize = 50;

/// The members of a load balancer that an added certificate must leave exactly
/// as they were. `state` is left out on purpose: a load balancer settles from
/// `provisioning` into `active` on the clock, whatever else is called.
const UNCHANGED_MEMBERS: &[&str] = &[
    "name",
    "arn",
    "supportCode",
    "createdAt",
    "dnsName",
    "instancePort",
    "healthCheckPath",
    "ipAddressType",
    "location",
    "configurationOptions",
    "instanceHealthSummary",
];

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// The stored load balancer `name` in `region`, as `GetLoadBalancer` reports it.
async fn read(emulator: &Emulator, region: &str, name: &str) -> Value {
    emulator
        .ok_at(
            None,
            Some(region),
            "GetLoadBalancer",
            json!({"loadBalancerName": name}),
        )
        .await["loadBalancer"]
        .clone()
}

/// The certificate summaries a load balancer reports, in report order, as
/// (name, `isAttached`) pairs.
fn summaries(balancer: &Value) -> Vec<(String, bool)> {
    balancer["tlsCertificateSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| {
                    (
                        summary["name"].as_str().unwrap_or_default().to_string(),
                        summary["isAttached"].as_bool().unwrap_or(true),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `CreateLoadBalancerTlsCertificate` body naming `certificate` for
/// `load_balancer`, covering `domain` and `alternatives`.
fn request(load_balancer: &str, certificate: &str, domain: &str, alternatives: &[&str]) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("loadBalancerName".to_string(), json!(load_balancer));
    body.insert("certificateName".to_string(), json!(certificate));
    body.insert("certificateDomainName".to_string(), json!(domain));
    if !alternatives.is_empty() {
        body.insert(
            "certificateAlternativeNames".to_string(),
            json!(alternatives),
        );
    }
    Value::Object(body)
}

/// Check the one operation-log entry a successful call answers.
fn assert_create_operation(case: &str, body: &Value, region: &str, certificate: &str) {
    let operations = body["operations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {body:?}"));
    assert_eq!(
        operations.len(),
        1,
        "case {case}: one certificate was added, so one operation is answered, \
         got {operations:?}"
    );
    let operation = &operations[0];
    assert_eq!(
        operation["operationType"].as_str(),
        Some(OPERATION),
        "case {case}: expected the {OPERATION} operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some(CERTIFICATE_RESOURCE_TYPE),
        "case {case}: expected the certificate resource type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(certificate),
        "case {case}: expected the created certificate's name, got {operation:?}"
    );
    assert_eq!(
        operation["status"].as_str(),
        Some("Succeeded"),
        "case {case}: the certificate is on the record, so the operation succeeded, \
         got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case {case}: the certificate is on the record, so the operation is terminal, \
         got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some(region),
        "case {case}: expected the Region the load balancer lives in, got {operation:?}"
    );
    let id = operation["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an operation id, got {operation:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a UUID-shaped operation id, got {id:?}"
    );
}

// ---------------------------------------------------------------------------
// The create table
// ---------------------------------------------------------------------------

/// One certificate a row asks for.
#[derive(Debug, Clone, Copy)]
struct Certificate {
    name: &'static str,
    domain: &'static str,
    /// The alternative domains the request carries, as sent.
    alternatives: &'static [&'static str],
}

/// One load balancer, the certificate `CreateLoadBalancer` was asked for (if
/// any), and the certificates this operation then adds to it in order.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    load_balancer: &'static str,
    instance_port: i64,
    /// The certificate `CreateLoadBalancer` itself was asked for, which this
    /// operation's certificates are added BESIDE.
    created_with: Option<Certificate>,
    added: &'static [Certificate],
}

/// The nine alternative domains the documentation admits, for the row that uses
/// all of them.
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

const ROWS: &[Row] = &[
    Row {
        case: "one certificate with no alternative names, on a bare load balancer",
        region: "us-east-1",
        load_balancer: "shop-lb",
        instance_port: HTTP_PORT,
        created_with: None,
        added: &[Certificate {
            name: "shop-cert",
            domain: "example.com",
            alternatives: &[],
        }],
    },
    Row {
        case: "the two certificates one load balancer may hold, added one at a time",
        region: "eu-west-1",
        load_balancer: "eu-edge-lb",
        instance_port: 8080,
        created_with: None,
        added: &[
            Certificate {
                name: "eu-edge-primary",
                domain: "eu.example.com",
                alternatives: &["www.eu.example.com"],
            },
            Certificate {
                name: "eu-edge-backup",
                domain: "eu.example.net",
                alternatives: &["www.eu.example.net", "shop.eu.example.net"],
            },
        ],
    },
    Row {
        case: "the nine alternative names the documentation admits",
        region: "ap-northeast-1",
        load_balancer: "apac-lb-01",
        instance_port: 3000,
        created_with: None,
        added: &[Certificate {
            name: "apac-cert-01",
            domain: "apac.example.com",
            alternatives: NINE_ALTERNATIVES,
        }],
    },
    Row {
        case: "a second certificate beside the one CreateLoadBalancer was asked for",
        region: "us-west-2",
        load_balancer: "west-lb-2",
        instance_port: 65535,
        created_with: Some(Certificate {
            name: "west-cert-2026-01",
            domain: "west.example.org",
            alternatives: &["cdn.west.example.org"],
        }),
        added: &[Certificate {
            name: "west_cert_backup",
            domain: "backup.west.example.org",
            alternatives: &[],
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
}

/// Every row: add this row's certificates to this row's load balancer, and check
/// each one lands unattached on the record, leaves the rest of the load balancer
/// alone, is reported by the operation log, and can be deleted again.
#[tokio::test(flavor = "current_thread")]
async fn create_load_balancer_tls_certificate_adds_an_unattached_certificate() {
    let emulator = Emulator::new();
    for row in ROWS {
        let case = row.case;
        emulator
            .ok_at(
                None,
                Some(row.region),
                "CreateLoadBalancer",
                row.create_load_balancer(),
            )
            .await;
        let before = read(&emulator, row.region, row.load_balancer).await;

        // The certificates the load balancer holds as each call lands: the one
        // `CreateLoadBalancer` was asked for comes first, then each added one in
        // the order it was added.
        let mut expected: Vec<(String, bool)> = row
            .created_with
            .iter()
            .map(|certificate| (certificate.name.to_string(), false))
            .collect();

        for certificate in row.added {
            let case = format!("{case} / {}", certificate.name);
            let answered = emulator
                .ok_at(
                    None,
                    Some(row.region),
                    OPERATION,
                    request(
                        row.load_balancer,
                        certificate.name,
                        certificate.domain,
                        certificate.alternatives,
                    ),
                )
                .await;
            assert_create_operation(&case, &answered, row.region, certificate.name);

            expected.push((certificate.name.to_string(), false));
            let after = read(&emulator, row.region, row.load_balancer).await;
            assert_eq!(
                summaries(&after),
                expected,
                "case {case}: expected every certificate held so far, unattached, got {:?}",
                after["tlsCertificateSummaries"]
            );

            // Nothing has ATTACHED a certificate, so the load balancer still
            // serves plain HTTP on port 80 alone.
            assert_eq!(
                after["protocol"].as_str(),
                Some(HTTP_ONLY),
                "case {case}: with no attached certificate the protocol must be {HTTP_ONLY}, \
                 got {:?}",
                after["protocol"]
            );
            assert_eq!(
                after["publicPorts"]
                    .as_array()
                    .map(|ports| ports.iter().filter_map(Value::as_i64).collect::<Vec<_>>()),
                Some(vec![HTTP_PORT]),
                "case {case}: with no attached certificate the only public port must be \
                 {HTTP_PORT}, got {:?}",
                after["publicPorts"]
            );
            for member in UNCHANGED_MEMBERS {
                assert_eq!(
                    after[member], before[member],
                    "case {case}: adding a certificate must leave '{member}' as it was, \
                     got {:?}",
                    after[member]
                );
            }

            // The entry the call answered is the entry the log holds for that
            // certificate, so a caller that polls the log by resource finds it.
            let logged = emulator
                .ok_at(
                    None,
                    Some(row.region),
                    "GetOperationsForResource",
                    json!({"resourceName": certificate.name}),
                )
                .await;
            let types: Vec<&str> = logged["operations"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {logged:?}"))
                .iter()
                .filter_map(|operation| operation["operationType"].as_str())
                .collect();
            assert_eq!(
                types,
                vec![OPERATION],
                "case {case}: expected the create entry in the certificate's log, got {logged:?}"
            );
        }

        // Each added certificate is a record of its own: deleting the first one
        // leaves every other certificate of the load balancer where it is.
        let deleted = row.added[0].name;
        emulator
            .ok_at(
                None,
                Some(row.region),
                "DeleteLoadBalancerTlsCertificate",
                json!({
                    "loadBalancerName": row.load_balancer,
                    "certificateName": deleted,
                }),
            )
            .await;
        expected.retain(|(name, _)| name != deleted);
        let after_delete = read(&emulator, row.region, row.load_balancer).await;
        assert_eq!(
            summaries(&after_delete),
            expected,
            "case {case}: deleting '{deleted}' must leave the other certificates alone, \
             got {:?}",
            after_delete["tlsCertificateSummaries"]
        );
    }
}

// ---------------------------------------------------------------------------
// De-duping the alternative names
// ---------------------------------------------------------------------------

/// The primary domain every alternative-name row's certificate is for.
const DEDUPE_DOMAIN: &str = "primary.example.com";

/// Nine distinct alternative domains of [`DEDUPE_DOMAIN`] — the most one
/// certificate may cover.
const NINE: &[&str] = &[
    "a.primary.example.com",
    "b.primary.example.com",
    "c.primary.example.com",
    "d.primary.example.com",
    "e.primary.example.com",
    "f.primary.example.com",
    "g.primary.example.com",
    "h.primary.example.com",
    "i.primary.example.com",
];

/// Ten distinct ones: one past the maximum.
const TEN: &[&str] = &[
    "a.primary.example.com",
    "b.primary.example.com",
    "c.primary.example.com",
    "d.primary.example.com",
    "e.primary.example.com",
    "f.primary.example.com",
    "g.primary.example.com",
    "h.primary.example.com",
    "i.primary.example.com",
    "j.primary.example.com",
];

/// Eleven entries naming nine distinct alternative domains: one repeat and the
/// primary domain itself, neither of which adds a domain to the certificate.
const ELEVEN_WITH_NINE_DISTINCT: &[&str] = &[
    "a.primary.example.com",
    "b.primary.example.com",
    "c.primary.example.com",
    "d.primary.example.com",
    "e.primary.example.com",
    "f.primary.example.com",
    "g.primary.example.com",
    "h.primary.example.com",
    "i.primary.example.com",
    "a.primary.example.com",
    DEDUPE_DOMAIN,
];

/// Twelve entries naming ONE distinct alternative domain.
const TWELVE_OF_ONE: &[&str] = &[
    "www.primary.example.com",
    "www.primary.example.com",
    "www.primary.example.com",
    "www.primary.example.com",
    "www.primary.example.com",
    "www.primary.example.com",
    "www.primary.example.com",
    "www.primary.example.com",
    "www.primary.example.com",
    "www.primary.example.com",
    "www.primary.example.com",
    "www.primary.example.com",
];

/// Twelve entries naming ten distinct alternative domains: de-duping does not
/// bring them within the maximum.
const TWELVE_WITH_TEN_DISTINCT: &[&str] = &[
    "a.primary.example.com",
    "b.primary.example.com",
    "c.primary.example.com",
    "d.primary.example.com",
    "e.primary.example.com",
    "f.primary.example.com",
    "g.primary.example.com",
    "h.primary.example.com",
    "i.primary.example.com",
    "j.primary.example.com",
    "j.primary.example.com",
    DEDUPE_DOMAIN,
];

/// One alternative-name list, and whether the certificate it describes is one
/// Lightsail accepts.
#[derive(Debug, Clone, Copy)]
struct AlternativesRow {
    case: &'static str,
    load_balancer: &'static str,
    certificate: &'static str,
    sent: &'static [&'static str],
    accepted: bool,
}

const ALTERNATIVES_ROWS: &[AlternativesRow] = &[
    AlternativesRow {
        case: "no alternative names at all",
        load_balancer: "dedupe-lb-1",
        certificate: "dedupe-cert-1",
        sent: &[],
        accepted: true,
    },
    AlternativesRow {
        case: "the nine distinct names the documentation admits",
        load_balancer: "dedupe-lb-2",
        certificate: "dedupe-cert-2",
        sent: NINE,
        accepted: true,
    },
    AlternativesRow {
        case: "ten distinct names, one past the maximum",
        load_balancer: "dedupe-lb-3",
        certificate: "dedupe-cert-3",
        sent: TEN,
        accepted: false,
    },
    AlternativesRow {
        case: "eleven entries that de-dupe to the nine the maximum admits",
        load_balancer: "dedupe-lb-4",
        certificate: "dedupe-cert-4",
        sent: ELEVEN_WITH_NINE_DISTINCT,
        accepted: true,
    },
    AlternativesRow {
        case: "twelve entries naming one alternative domain",
        load_balancer: "dedupe-lb-5",
        certificate: "dedupe-cert-5",
        sent: TWELVE_OF_ONE,
        accepted: true,
    },
    AlternativesRow {
        case: "twelve entries that still name ten distinct domains",
        load_balancer: "dedupe-lb-6",
        certificate: "dedupe-cert-6",
        sent: TWELVE_WITH_TEN_DISTINCT,
        accepted: false,
    },
];

/// The alternative domains are de-duped — against each other and against the
/// primary domain — BEFORE the nine-name maximum is applied, so a list whose
/// repeats Lightsail itself would drop is accepted and one that names ten
/// distinct domains is not.
#[tokio::test(flavor = "current_thread")]
async fn alternative_names_are_de_duped_before_the_documented_maximum() {
    let emulator = Emulator::new();
    let region = "us-east-2";
    for row in ALTERNATIVES_ROWS {
        let case = row.case;
        emulator
            .ok_at(
                None,
                Some(region),
                "CreateLoadBalancer",
                json!({"loadBalancerName": row.load_balancer, "instancePort": HTTP_PORT}),
            )
            .await;
        let (status, body) = emulator
            .call_in(
                Some(region),
                OPERATION,
                request(row.load_balancer, row.certificate, DEDUPE_DOMAIN, row.sent),
            )
            .await;

        let balancer = read(&emulator, region, row.load_balancer).await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case}: {} entries name at most 9 distinct alternative domains, \
                 so the certificate is created, got {status} {body:?}",
                row.sent.len()
            );
            assert_eq!(
                summaries(&balancer),
                vec![(row.certificate.to_string(), false)],
                "case {case}: expected the created certificate on the load balancer, got {:?}",
                balancer["tlsCertificateSummaries"]
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: more than 9 distinct alternative domains is refused, \
                 got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {case}: expected the modelled InvalidInputException, got {body:?}"
            );
            assert!(
                summaries(&balancer).is_empty(),
                "case {case}: a refused create must add nothing, got {:?}",
                balancer["tlsCertificateSummaries"]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The bound on the list the de-duping reads
// ---------------------------------------------------------------------------

/// How many entries an alternative-name list may carry is bounded BEFORE any
/// entry is read — whether an entry is a repeat is only known once it has been
/// examined — so a list at the bound whose entries all name one domain is
/// accepted and one entry past it is refused.
#[tokio::test(flavor = "current_thread")]
async fn the_alternative_name_list_is_bounded_before_it_is_read() {
    let emulator = Emulator::new();
    let region = "ca-central-1";
    // (case, how many entries the list carries, whether it is accepted)
    let cases = [
        (
            "the most entries the list may carry",
            MAX_DOMAIN_NAME_LIST_ENTRIES,
            true,
        ),
        (
            "one entry past the bound",
            MAX_DOMAIN_NAME_LIST_ENTRIES + 1,
            false,
        ),
    ];
    for (index, (case, entries, accepted)) in cases.into_iter().enumerate() {
        let load_balancer = format!("entries-lb-{index}");
        let certificate = format!("entries-cert-{index}");
        emulator
            .ok_at(
                None,
                Some(region),
                "CreateLoadBalancer",
                json!({"loadBalancerName": load_balancer, "instancePort": HTTP_PORT}),
            )
            .await;
        // Every entry names the SAME alternative domain, so de-duping leaves
        // one: what decides each case is the entry count alone.
        let sent = vec!["www.primary.example.com"; entries];
        let (status, body) = emulator
            .call_in(
                Some(region),
                OPERATION,
                request(&load_balancer, &certificate, DEDUPE_DOMAIN, &sent),
            )
            .await;
        let balancer = read(&emulator, region, &load_balancer).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case}: {entries} entries name one distinct alternative domain, so the \
                 certificate is created, got {status} {body:?}"
            );
            assert_eq!(
                summaries(&balancer),
                vec![(certificate.clone(), false)],
                "case {case}: expected the created certificate, got {:?}",
                balancer["tlsCertificateSummaries"]
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case}: {entries} entries is more than the list may carry, \
                 got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {case}: expected the modelled InvalidInputException, got {body:?}"
            );
            assert!(
                summaries(&balancer).is_empty(),
                "case {case}: a refused create must add nothing, got {:?}",
                balancer["tlsCertificateSummaries"]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------
/// The load balancer every refusal row is checked against, and the one
/// certificate it already holds.
const HELD_REGION: &str = "us-east-1";
const HELD_LOAD_BALANCER: &str = "held-lb";
const HELD_CERTIFICATE: &str = "held-cert";
const HELD_DOMAIN: &str = "held.example.com";

/// What a refused request must be answered with.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// Lightsail's `DoesNotExist` envelope, naming this `ResourceType` and this
    /// name.
    DoesNotExist(&'static str, &'static str),
    /// The modelled `InvalidInputException`.
    InvalidInput,
}

/// One `CreateLoadBalancerTlsCertificate` request the service refuses.
#[derive(Debug, Clone, Copy)]
struct Refusal {
    case: &'static str,
    region: &'static str,
    load_balancer: &'static str,
    certificate: &'static str,
    domain: &'static str,
    alternatives: &'static [&'static str],
    /// How many tags the request carries.
    tags: usize,
    expected: Expected,
}

const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a load balancer name the account does not hold",
        region: HELD_REGION,
        load_balancer: "absent-lb",
        certificate: "new-cert",
        domain: "new.example.com",
        alternatives: &[],
        tags: 0,
        expected: Expected::DoesNotExist(LOAD_BALANCER_RESOURCE_TYPE, "absent-lb"),
    },
    Refusal {
        case: "the held load balancer, in a Region it does not live in",
        region: "eu-central-1",
        load_balancer: HELD_LOAD_BALANCER,
        certificate: "new-cert",
        domain: "new.example.com",
        alternatives: &[],
        tags: 0,
        expected: Expected::DoesNotExist(LOAD_BALANCER_RESOURCE_TYPE, HELD_LOAD_BALANCER),
    },
    Refusal {
        case: "a certificate name the load balancer already holds",
        region: HELD_REGION,
        load_balancer: HELD_LOAD_BALANCER,
        certificate: HELD_CERTIFICATE,
        domain: "other.example.com",
        alternatives: &[],
        tags: 0,
        expected: Expected::InvalidInput,
    },
    Refusal {
        case: "a wildcard primary domain, which Lightsail does not support",
        region: HELD_REGION,
        load_balancer: HELD_LOAD_BALANCER,
        certificate: "wildcard-cert",
        domain: "*.example.com",
        alternatives: &[],
        tags: 0,
        expected: Expected::InvalidInput,
    },
    Refusal {
        case: "a wildcard alternative domain, which Lightsail does not support",
        region: HELD_REGION,
        load_balancer: HELD_LOAD_BALANCER,
        certificate: "wildcard-alt-cert",
        domain: "example.com",
        alternatives: &["*.example.com"],
        tags: 0,
        expected: Expected::InvalidInput,
    },
    Refusal {
        case: "a primary domain with no parent zone",
        region: HELD_REGION,
        load_balancer: HELD_LOAD_BALANCER,
        certificate: "bare-domain-cert",
        domain: "example",
        alternatives: &[],
        tags: 0,
        expected: Expected::InvalidInput,
    },
    Refusal {
        case: "an alternative domain with no parent zone",
        region: HELD_REGION,
        load_balancer: HELD_LOAD_BALANCER,
        certificate: "bare-alt-cert",
        domain: "example.com",
        alternatives: &["localhost"],
        tags: 0,
        expected: Expected::InvalidInput,
    },
    Refusal {
        case: "one tag more than a resource may carry",
        region: HELD_REGION,
        load_balancer: HELD_LOAD_BALANCER,
        certificate: "over-tagged-cert",
        domain: "tagged.example.com",
        alternatives: &[],
        tags: MAX_TAGS + 1,
        expected: Expected::InvalidInput,
    },
];

/// A create that names a load balancer the account does not hold, a certificate
/// name it already holds, or a domain that is not a domain name, adds nothing.
#[tokio::test(flavor = "current_thread")]
async fn create_load_balancer_tls_certificate_refuses_what_it_cannot_create() {
    let emulator = Emulator::new();
    emulator
        .ok_at(
            None,
            Some(HELD_REGION),
            "CreateLoadBalancer",
            json!({"loadBalancerName": HELD_LOAD_BALANCER, "instancePort": HTTP_PORT}),
        )
        .await;
    emulator
        .ok_at(
            None,
            Some(HELD_REGION),
            OPERATION,
            request(HELD_LOAD_BALANCER, HELD_CERTIFICATE, HELD_DOMAIN, &[]),
        )
        .await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let mut body = request(
            refusal.load_balancer,
            refusal.certificate,
            refusal.domain,
            refusal.alternatives,
        );
        if refusal.tags > 0 {
            let tags: Vec<Value> = (0..refusal.tags)
                .map(|index| json!({"key": format!("key-{index}"), "value": "v"}))
                .collect();
            body["tags"] = json!(tags);
        }
        let (status, answered) = emulator.call_in(Some(refusal.region), OPERATION, body).await;
        assert_eq!(
            status, 400,
            "case {case}: Lightsail refuses this request with 400, got {status} {answered:?}"
        );
        match refusal.expected {
            Expected::DoesNotExist(resource_type, name) => {
                assert_eq!(
                    error_code(&answered),
                    "NotFoundException",
                    "case {case}: expected the modelled NotFoundException, got {answered:?}"
                );
                assert_eq!(
                    answered["code"].as_str(),
                    Some("DoesNotExist"),
                    "case {case}: expected the DoesNotExist body code, got {answered:?}"
                );
                let message = answered["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(resource_type) && message.contains(name),
                    "case {case}: expected a message naming the {resource_type} '{name}', \
                     got {message:?}"
                );
            }
            Expected::InvalidInput => {
                assert_eq!(
                    error_code(&answered),
                    "InvalidInputException",
                    "case {case}: expected the modelled InvalidInputException, got {answered:?}"
                );
                assert!(
                    !answered["message"]
                        .as_str()
                        .unwrap_or_default()
                        .is_empty(),
                    "case {case}: expected a message saying what was refused, got {answered:?}"
                );
            }
        }

        // Every refused create adds nothing: the held load balancer still holds
        // the one certificate it held.
        let held = read(&emulator, HELD_REGION, HELD_LOAD_BALANCER).await;
        assert_eq!(
            summaries(&held),
            vec![(HELD_CERTIFICATE.to_string(), false)],
            "case {case}: a refused create must add nothing, got {:?}",
            held["tlsCertificateSummaries"]
        );
    }
}

// ---------------------------------------------------------------------------
// The documented quotas
// ---------------------------------------------------------------------------

/// A load balancer holds at most the two certificates the documentation admits,
/// and the call that would add a third adds nothing.
#[tokio::test(flavor = "current_thread")]
async fn a_load_balancer_holds_at_most_the_documented_two_certificates() {
    let emulator = Emulator::new();
    let region = "ap-south-1";
    let load_balancer = "quota-lb";
    emulator
        .ok_at(
            None,
            Some(region),
            "CreateLoadBalancer",
            json!({"loadBalancerName": load_balancer, "instancePort": HTTP_PORT}),
        )
        .await;

    let mut held: Vec<(String, bool)> = Vec::new();
    for index in 0..CERTIFICATES_PER_LOAD_BALANCER {
        let certificate = format!("quota-cert-{index}");
        let domain = format!("quota{index}.example.com");
        emulator
            .ok_at(
                None,
                Some(region),
                OPERATION,
                request(load_balancer, &certificate, &domain, &[]),
            )
            .await;
        held.push((certificate, false));
    }

    let (status, body) = emulator
        .call_in(
            Some(region),
            OPERATION,
            request(load_balancer, "quota-cert-extra", "extra.example.com", &[]),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a third certificate on one load balancer: expected a refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case a third certificate on one load balancer: expected the modelled \
         InvalidInputException, got {body:?}"
    );
    let balancer = read(&emulator, region, load_balancer).await;
    assert_eq!(
        summaries(&balancer),
        held,
        "case a third certificate on one load balancer: the refused create must add nothing, \
         got {:?}",
        balancer["tlsCertificateSummaries"]
    );
}

/// The account holds at most the ten certificates the documentation admits,
/// counted across every load balancer and Region, and the call that would add an
/// eleventh adds nothing.
#[tokio::test(flavor = "current_thread")]
async fn the_account_holds_at_most_the_documented_ten_certificates() {
    let emulator = Emulator::new();
    // Load balancers in TWO Regions, each filled with the two certificates it
    // may hold, so what the quota counts is the account's certificates and not
    // one Region's or one load balancer's.
    const PLACEMENTS: &[(&str, &str)] = &[
        ("us-east-1", "account-lb-a"),
        ("us-east-1", "account-lb-b"),
        ("eu-west-2", "account-lb-c"),
        ("eu-west-2", "account-lb-d"),
        ("us-east-1", "account-lb-e"),
    ];
    assert_eq!(
        PLACEMENTS.len() * CERTIFICATES_PER_LOAD_BALANCER,
        CERTIFICATES_PER_ACCOUNT,
        "case filling the account quota: the placements must hold exactly the \
         {CERTIFICATES_PER_ACCOUNT} certificates the account may have"
    );

    let mut created = 0usize;
    for (region, load_balancer) in PLACEMENTS {
        emulator
            .ok_at(
                None,
                Some(region),
                "CreateLoadBalancer",
                json!({"loadBalancerName": load_balancer, "instancePort": HTTP_PORT}),
            )
            .await;
        for _ in 0..CERTIFICATES_PER_LOAD_BALANCER {
            let certificate = format!("account-cert-{created}");
            let domain = format!("account{created}.example.com");
            emulator
                .ok_at(
                    None,
                    Some(region),
                    OPERATION,
                    request(load_balancer, &certificate, &domain, &[]),
                )
                .await;
            created += 1;
        }
    }
    assert_eq!(
        created, CERTIFICATES_PER_ACCOUNT,
        "case filling the account quota: expected {CERTIFICATES_PER_ACCOUNT} certificates, \
         got {created}"
    );

    // An eleventh certificate, on a load balancer that holds none of its own, is
    // still the account's eleventh.
    let region = PLACEMENTS[0].0;
    let fresh = "account-lb-spare";
    emulator
        .ok_at(
            None,
            Some(region),
            "CreateLoadBalancer",
            json!({"loadBalancerName": fresh, "instancePort": HTTP_PORT}),
        )
        .await;
    let (status, body) = emulator
        .call_in(
            Some(region),
            OPERATION,
            request(fresh, "account-cert-extra", "extra.example.com", &[]),
        )
        .await;
    assert_eq!(
        status, 400,
        "case the account's eleventh certificate: expected a refusal, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case the account's eleventh certificate: expected the modelled InvalidInputException, \
         got {body:?}"
    );
    let spare = read(&emulator, region, fresh).await;
    assert!(
        summaries(&spare).is_empty(),
        "case the account's eleventh certificate: the refused create must add nothing, got {:?}",
        spare["tlsCertificateSummaries"]
    );

    // And the ten the account does hold are all still there.
    let mut counted = 0usize;
    for (region, load_balancer) in PLACEMENTS {
        let balancer = read(&emulator, region, load_balancer).await;
        counted += summaries(&balancer).len();
    }
    assert_eq!(
        counted, CERTIFICATES_PER_ACCOUNT,
        "case the account's eleventh certificate: the {CERTIFICATES_PER_ACCOUNT} certificates \
         the account holds must all still be reported, got {counted}"
    );
}
