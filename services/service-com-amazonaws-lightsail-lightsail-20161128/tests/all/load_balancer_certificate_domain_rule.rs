// SPDX-License-Identifier: CC0-1.0

//! The domains a Lightsail load balancer's SSL/TLS certificate covers, driven
//! over the wire through BOTH operations that describe one.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: one certificate record of a load balancer is described
//! by `certificateName`, `certificateDomainName` and
//! `certificateAlternativeNames`, so whichever operation carries those members —
//! the `CreateLoadBalancer` that describes the first certificate of a new load
//! balancer as much as the `CreateLoadBalancerTlsCertificate` that adds one to a
//! load balancer that already exists — the primary domain and every alternative
//! name have to be a DNS domain name (which is what refuses the wildcards the
//! modelled `CreateLoadBalancerTlsCertificate$certificateAlternativeNames`
//! documentation says are not supported), and what survives Lightsail's own
//! de-duping has to be within the nine alternative names that same documentation
//! admits "(in addition to the 1 primary domain)". A request that breaks either
//! is refused with the modelled `InvalidInputException` and stores no
//! certificate; one that satisfies both is served, and the certificate reports
//! exactly the de-duped domains on every later read.
//!
//! The rows differ along what the rule generalises over: which operation carries
//! the members, where the offending name sits (the primary domain or one of the
//! alternatives), how it offends (a wildcard, a missing parent zone, an empty
//! label, a character no DNS label carries, a leading or trailing hyphen, a
//! label or a whole name past its published length), how many alternative
//! domains the request names, and how many entries the list carries before
//! Lightsail de-dupes it.

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state::MAX_DOMAIN_NAME_LIST_ENTRIES;

// The harness this test target already exposes: a sibling suite drives the
// service through the same front door rather than standing up a second one.
use crate::instance_lifecycle::{error_code, Emulator};

/// The operation that adds a certificate to a load balancer that already exists.
const ADD_CERTIFICATE: &str = "CreateLoadBalancerTlsCertificate";

/// The operation that creates a load balancer, optionally describing its first
/// certificate.
const CREATE_LOAD_BALANCER: &str = "CreateLoadBalancer";

/// The operation every accepted case reads the stored certificate back through.
const READ_CERTIFICATES: &str = "GetLoadBalancerTlsCertificates";

/// The port every load balancer of this suite forwards to. The rule under test
/// is about the certificate members, so one port serves every row.
const INSTANCE_PORT: i64 = 80;

/// The load balancer every case acts on, and the certificate it describes.
const LOAD_BALANCER: &str = "cert-rule-lb";
const CERTIFICATE: &str = "cert-rule-cert";

/// The Region every case is made in. Each case stands up its own emulator, so
/// one Region is enough and neither the per-Region load balancer cap nor the
/// account-wide certificate cap is in play.
const REGION: &str = "us-east-1";

/// The most alternative domains one certificate may cover, from the modelled
/// `CreateLoadBalancerTlsCertificate$certificateAlternativeNames` documentation:
/// "You can have a maximum of 9 alternative names (in addition to the 1 primary
/// domain)."
const MAX_ALTERNATIVE_NAMES: usize = 9;

/// The most characters one label of a domain name may carry (RFC 1035 §2.3.4).
const MAX_LABEL_CHARS: usize = 63;

/// The most characters a domain name may carry in the dotted presentation form a
/// request uses — RFC 1035 §2.3.4 caps the encoded name at 255 octets.
const MAX_DOMAIN_NAME_CHARS: usize = 253;

// ---------------------------------------------------------------------------
// The inputs the rows are built from
// ---------------------------------------------------------------------------

/// `count` distinct alternative domains, in a stable order.
fn distinct(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("alt{index}.example.com"))
        .collect()
}

/// `count` entries, every one of them `name` — a list Lightsail's own de-duping
/// reduces to a single domain.
fn repeated(name: &str, count: usize) -> Vec<String> {
    (0..count).map(|_| name.to_string()).collect()
}

/// A domain name of exactly `chars` characters, built from labels well inside
/// [`MAX_LABEL_CHARS`] so only the name's OVERALL length is ever at issue.
fn domain_of_length(chars: usize) -> String {
    /// How many characters each label of the built name carries.
    const LABEL: usize = 9;
    let mut built = String::new();
    while built.len() < chars {
        let room = chars - built.len();
        if built.is_empty() {
            built.push_str(&"a".repeat(room.min(LABEL)));
        } else if room >= 2 {
            // A dot plus at least one character: a label is never empty.
            built.push('.');
            built.push_str(&"a".repeat((room - 1).min(LABEL)));
        } else {
            // One character short of the target: lengthen the last label rather
            // than open a new one, which would need a dot it has no room for.
            built.push('a');
        }
    }
    built
}

/// A domain name whose FIRST label carries `chars` characters.
fn domain_with_label_of_length(chars: usize) -> String {
    format!("{}.example.com", "a".repeat(chars))
}

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Which operation carries the certificate members.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carrier {
    /// `CreateLoadBalancer`, describing the first certificate of the load
    /// balancer it creates.
    OnCreate,
    /// `CreateLoadBalancerTlsCertificate`, adding one to a load balancer that
    /// already exists.
    OnAdd,
}

impl Carrier {
    /// Both operations that carry the members, so every row is driven through
    /// each of them.
    const ALL: [Carrier; 2] = [Carrier::OnCreate, Carrier::OnAdd];

    /// The operation name this carrier calls.
    fn operation(self) -> &'static str {
        match self {
            Carrier::OnCreate => CREATE_LOAD_BALANCER,
            Carrier::OnAdd => ADD_CERTIFICATE,
        }
    }
}

/// What a case observed: the refusal or acceptance of the request, and the
/// domains every certificate the load balancer then holds covers.
struct Observed {
    status: u16,
    body: Value,
    /// One entry per stored certificate: the domains it covers, primary first,
    /// as `subjectAlternativeNames` reports them.
    covered: Vec<Vec<String>>,
}

/// Describe a certificate for `domain` with `alternatives` through `carrier`,
/// against a fresh emulator, and read back what it stored.
async fn describe(carrier: Carrier, domain: &str, alternatives: &[String]) -> Observed {
    let emulator = Emulator::new();
    let (status, body) = match carrier {
        Carrier::OnCreate => {
            emulator
                .call_in(
                    Some(REGION),
                    CREATE_LOAD_BALANCER,
                    json!({
                        "loadBalancerName": LOAD_BALANCER,
                        "instancePort": INSTANCE_PORT,
                        "certificateName": CERTIFICATE,
                        "certificateDomainName": domain,
                        "certificateAlternativeNames": alternatives,
                    }),
                )
                .await
        }
        Carrier::OnAdd => {
            // The load balancer the certificate is added to, described with no
            // certificate of its own.
            emulator
                .ok_at(
                    None,
                    Some(REGION),
                    CREATE_LOAD_BALANCER,
                    json!({
                        "loadBalancerName": LOAD_BALANCER,
                        "instancePort": INSTANCE_PORT,
                    }),
                )
                .await;
            emulator
                .call_in(
                    Some(REGION),
                    ADD_CERTIFICATE,
                    json!({
                        "loadBalancerName": LOAD_BALANCER,
                        "certificateName": CERTIFICATE,
                        "certificateDomainName": domain,
                        "certificateAlternativeNames": alternatives,
                    }),
                )
                .await
        }
    };
    let covered = covered_domains(&emulator).await;
    Observed {
        status,
        body,
        covered,
    }
}

/// The domains every certificate the load balancer holds covers — empty when the
/// account holds no such load balancer at all, which is what a refused
/// `CreateLoadBalancer` leaves behind.
async fn covered_domains(emulator: &Emulator) -> Vec<Vec<String>> {
    let (status, body) = emulator
        .call_in(
            Some(REGION),
            READ_CERTIFICATES,
            json!({"loadBalancerName": LOAD_BALANCER}),
        )
        .await;
    if status != 200 {
        return Vec::new();
    }
    body["tlsCertificates"]
        .as_array()
        .map(|certificates| {
            certificates
                .iter()
                .map(|certificate| {
                    certificate["subjectAlternativeNames"]
                        .as_array()
                        .map(|names| {
                            names
                                .iter()
                                .map(|name| {
                                    name.as_str().unwrap_or("<not a string>").to_string()
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The refusal table
// ---------------------------------------------------------------------------

/// One certificate description the service refuses, whichever operation carries
/// it.
struct Refused {
    case: &'static str,
    domain: fn() -> String,
    alternatives: fn() -> Vec<String>,
    /// The member the refusal's message must name, so a caller can fix the
    /// request from the text alone.
    names: &'static str,
}

/// The table. Every row breaks the one rule the two operations share.
const REFUSED: &[Refused] = &[
    Refused {
        case: "a wildcard primary domain, which Lightsail does not support",
        domain: || "*.example.com".to_string(),
        alternatives: Vec::new,
        names: "certificateDomainName",
    },
    Refused {
        case: "a wildcard alternative domain, which Lightsail does not support",
        domain: || "example.com".to_string(),
        alternatives: || vec!["*.example.com".to_string()],
        names: "certificateAlternativeNames",
    },
    Refused {
        case: "a primary domain with no parent zone",
        domain: || "example".to_string(),
        alternatives: Vec::new,
        names: "certificateDomainName",
    },
    Refused {
        case: "an alternative domain with no parent zone",
        domain: || "example.com".to_string(),
        alternatives: || vec!["localhost".to_string()],
        names: "certificateAlternativeNames",
    },
    Refused {
        case: "an empty primary domain",
        domain: String::new,
        alternatives: Vec::new,
        names: "certificateDomainName",
    },
    Refused {
        case: "a primary domain with an empty label",
        domain: || "www..example.com".to_string(),
        alternatives: Vec::new,
        names: "certificateDomainName",
    },
    Refused {
        case: "an alternative domain carrying a character no DNS label carries",
        domain: || "example.com".to_string(),
        alternatives: || vec!["under_score.example.com".to_string()],
        names: "certificateAlternativeNames",
    },
    Refused {
        case: "a primary domain whose label begins with a hyphen",
        domain: || "-leading.example.com".to_string(),
        alternatives: Vec::new,
        names: "certificateDomainName",
    },
    Refused {
        case: "an alternative domain whose label ends with a hyphen",
        domain: || "example.com".to_string(),
        alternatives: || vec!["trailing-.example.com".to_string()],
        names: "certificateAlternativeNames",
    },
    Refused {
        case: "a primary label one character past the longest a DNS label may carry",
        domain: || domain_with_label_of_length(MAX_LABEL_CHARS + 1),
        alternatives: Vec::new,
        names: "certificateDomainName",
    },
    Refused {
        case: "a primary domain one character past the longest a domain name may carry",
        domain: || domain_of_length(MAX_DOMAIN_NAME_CHARS + 1),
        alternatives: Vec::new,
        names: "certificateDomainName",
    },
    Refused {
        case: "an alternative domain one character past the longest a domain name may carry",
        domain: || "example.com".to_string(),
        alternatives: || vec![domain_of_length(MAX_DOMAIN_NAME_CHARS + 1)],
        names: "certificateAlternativeNames",
    },
    Refused {
        case: "one alternative domain more than a certificate may cover",
        domain: || "example.com".to_string(),
        alternatives: || distinct(MAX_ALTERNATIVE_NAMES + 1),
        names: "certificateAlternativeNames",
    },
    Refused {
        case: "repeats that still leave one alternative domain more than a certificate may cover",
        domain: || "example.com".to_string(),
        alternatives: || {
            let mut names = distinct(MAX_ALTERNATIVE_NAMES + 1);
            names.extend(distinct(MAX_ALTERNATIVE_NAMES + 1));
            names
        },
        names: "certificateAlternativeNames",
    },
    Refused {
        case: "the primary domain among names that still exceed the maximum",
        domain: || "example.com".to_string(),
        alternatives: || {
            let mut names = vec!["example.com".to_string()];
            names.extend(distinct(MAX_ALTERNATIVE_NAMES + 1));
            names
        },
        names: "certificateAlternativeNames",
    },
    Refused {
        case: "one entry more than the alternative-name list may carry",
        domain: || "example.com".to_string(),
        alternatives: || repeated("one.example.com", MAX_DOMAIN_NAME_LIST_ENTRIES + 1),
        names: "certificateAlternativeNames",
    },
];

/// RULE: a certificate description whose domains are not domain names, or whose
/// de-duped alternative domains exceed the documented maximum, is refused with
/// the modelled `InvalidInputException` and stores no certificate — whichever of
/// the two operations carries those members.
#[tokio::test(flavor = "current_thread")]
async fn a_load_balancer_certificate_description_is_refused_by_both_operations() {
    for carrier in Carrier::ALL {
        let operation = carrier.operation();
        for row in REFUSED {
            let case = format!("{operation}: {}", row.case);
            let domain = (row.domain)();
            let alternatives = (row.alternatives)();
            let observed = describe(carrier, &domain, &alternatives).await;
            assert_eq!(
                observed.status, 400,
                "case {case}: expected HTTP 400, got {} {:?}",
                observed.status, observed.body
            );
            assert_eq!(
                error_code(&observed.body),
                "InvalidInputException",
                "case {case}: expected the modelled InvalidInputException, got {:?}",
                observed.body
            );
            let message = observed.body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains(row.names),
                "case {case}: expected a message naming '{}', got {message:?}",
                row.names
            );
            assert!(
                observed.covered.is_empty(),
                "case {case}: expected a refused description to store no certificate, got {:?}",
                observed.covered
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The served table
// ---------------------------------------------------------------------------

/// One certificate description the service serves, and the domains the stored
/// certificate then covers — the primary domain first, then the alternative
/// domains Lightsail's de-duping left.
struct Served {
    case: &'static str,
    domain: fn() -> String,
    alternatives: fn() -> Vec<String>,
    covered: fn() -> Vec<String>,
}

/// The table.
const SERVED: &[Served] = &[
    Served {
        case: "no alternative domain at all",
        domain: || "example.com".to_string(),
        alternatives: Vec::new,
        covered: || vec!["example.com".to_string()],
    },
    Served {
        case: "one alternative domain",
        domain: || "example.com".to_string(),
        alternatives: || vec!["www.example.com".to_string()],
        covered: || vec!["example.com".to_string(), "www.example.com".to_string()],
    },
    Served {
        case: "the nine alternative domains the documentation admits",
        domain: || "example.com".to_string(),
        alternatives: || distinct(MAX_ALTERNATIVE_NAMES),
        covered: || {
            let mut covered = vec!["example.com".to_string()];
            covered.extend(distinct(MAX_ALTERNATIVE_NAMES));
            covered
        },
    },
    Served {
        case: "repeats and the primary domain that de-dupe to the nine admitted",
        domain: || "example.com".to_string(),
        alternatives: || {
            let mut names = distinct(MAX_ALTERNATIVE_NAMES);
            names.push("example.com".to_string());
            names.extend(distinct(MAX_ALTERNATIVE_NAMES));
            names
        },
        covered: || {
            let mut covered = vec!["example.com".to_string()];
            covered.extend(distinct(MAX_ALTERNATIVE_NAMES));
            covered
        },
    },
    Served {
        case: "the most entries the list may carry, every one of them one domain",
        domain: || "example.com".to_string(),
        alternatives: || repeated("one.example.com", MAX_DOMAIN_NAME_LIST_ENTRIES),
        covered: || vec!["example.com".to_string(), "one.example.com".to_string()],
    },
    Served {
        case: "a primary label at the longest a DNS label may carry",
        domain: || domain_with_label_of_length(MAX_LABEL_CHARS),
        alternatives: Vec::new,
        covered: || vec![domain_with_label_of_length(MAX_LABEL_CHARS)],
    },
    Served {
        case: "a primary domain at the longest a domain name may carry",
        domain: || domain_of_length(MAX_DOMAIN_NAME_CHARS),
        alternatives: Vec::new,
        covered: || vec![domain_of_length(MAX_DOMAIN_NAME_CHARS)],
    },
    Served {
        case: "an alternative domain at the longest a domain name may carry",
        domain: || "example.com".to_string(),
        alternatives: || vec![domain_of_length(MAX_DOMAIN_NAME_CHARS)],
        covered: || {
            vec![
                "example.com".to_string(),
                domain_of_length(MAX_DOMAIN_NAME_CHARS),
            ]
        },
    },
    Served {
        case: "a subdomain of a subdomain as an alternative domain",
        domain: || "example.org".to_string(),
        alternatives: || {
            vec![
                "shop.example.org".to_string(),
                "cdn.shop.example.org".to_string(),
                "1-digit.example.org".to_string(),
            ]
        },
        covered: || {
            vec![
                "example.org".to_string(),
                "shop.example.org".to_string(),
                "cdn.shop.example.org".to_string(),
                "1-digit.example.org".to_string(),
            ]
        },
    },
];

/// RULE: a certificate description whose domains are domain names and whose
/// de-duped alternative domains are within the documented maximum is served by
/// either operation, and the stored certificate reports exactly the de-duped
/// domains on a later read.
#[tokio::test(flavor = "current_thread")]
async fn a_load_balancer_certificate_description_is_served_by_both_operations() {
    for carrier in Carrier::ALL {
        let operation = carrier.operation();
        for row in SERVED {
            let case = format!("{operation}: {}", row.case);
            let domain = (row.domain)();
            let alternatives = (row.alternatives)();
            let observed = describe(carrier, &domain, &alternatives).await;
            assert_eq!(
                observed.status, 200,
                "case {case}: expected the description to be served, got {} {:?}",
                observed.status, observed.body
            );
            let want = vec![(row.covered)()];
            assert_eq!(
                observed.covered, want,
                "case {case}: expected the stored certificate to cover {want:?}, got {:?}",
                observed.covered
            );
        }
    }
}
