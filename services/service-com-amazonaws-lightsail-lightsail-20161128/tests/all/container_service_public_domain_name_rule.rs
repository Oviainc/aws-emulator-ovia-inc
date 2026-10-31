// SPDX-License-Identifier: CC0-1.0

//! The domain names a container service's `publicDomainNames` may list, driven
//! over the wire through BOTH operations that record them.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: every name a request lists under a key of
//! `publicDomainNames` has to be a DNS domain name — the member carries "the
//! public domain names to use with the container service, such as example.com and
//! www.example.com" — and the map may carry no more of them than a container
//! service accepts, whichever operation records them: the
//! `CreateContainerService` that stores the first set as much as the
//! `UpdateContainerService` that replaces it. A request that breaks either is
//! refused with the modelled `InvalidInputException` and records nothing; one that
//! satisfies both is served, and the service reports exactly the de-duped names on
//! every later read.
//!
//! The sibling suite `container_service_public_domain_certificate.rs` pins the
//! other half of the same member's rule — the certificate each group of names is
//! keyed by — and `load_balancer_certificate_domain_rule.rs` pins the same
//! is-this-a-domain-name rule where a load balancer's certificate carries the
//! domains.

use serde_json::{json, Map, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state::MAX_PUBLIC_DOMAIN_NAMES;

// The harness, the wire error code, the certificate stand-up and the read of a
// stored service's domains this test target already exposes: sibling suites drive
// the service through the same front door, and wait for the same clock, rather
// than standing up a second one.
use crate::container_service_public_domain_certificate::reported_domains;
use crate::distribution_certificate_attachment::issue_certificate;
use crate::instance_lifecycle::{error_code, Emulator};

/// The Region every case is made in: the rule is about the names, not about where
/// the service lives.
const REGION: &str = "us-east-1";

/// The value Lightsail puts in a not-found body's `code` member, which is what a
/// read of a service a refused create never stored answers.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The power and scale every service of this suite is created with: the rule under
/// test is about the domain names, not the capacity.
const POWER: &str = "nano";
const SCALE: i64 = 1;

/// The two certificates this account holds, issued, that every case keys its names
/// by — so the only thing a refusal can be about is the names themselves.
const HELD: &str = "domain-rule-cert";
const SECOND_HELD: &str = "domain-rule-second-cert";

/// A domain the held certificates are requested for. The certificate's own
/// coverage is the sibling suite's rule; one domain each is enough to stand them
/// up.
const HELD_DOMAIN: &str = "domain-rule.example.com";
const SECOND_HELD_DOMAIN: &str = "domain-rule.example.org";

/// A name this account holds NO certificate under, so one case can pin that the
/// half of the rule the request decides on its own is answered before the store is
/// asked which certificates the account holds.
const GHOST: &str = "domain-rule-ghost-cert";

/// The most characters one label of a domain name may carry, and the most a whole
/// name may carry in the dotted presentation form a request uses — RFC 1035 §2.3.4
/// caps a label at 63 octets and an encoded name at 255 octets, which is 253
/// characters written out.
const MAX_LABEL_CHARS: usize = 63;
const MAX_DOMAIN_NAME_CHARS: usize = 253;

// ---------------------------------------------------------------------------
// The names the rows are built from
// ---------------------------------------------------------------------------

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

/// `count` distinct domain names, in a stable order.
fn distinct(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("name{index}.example.com"))
        .collect()
}

/// The `publicDomainNames` member `domains` describes, as the wire carries it.
///
/// Rows build their names at run time (a label at its published bound, a name one
/// character past it), so this takes the owned form the sibling suite's
/// `&'static` builder cannot.
fn public_domain_names(domains: &[(String, Vec<String>)]) -> Value {
    let mut map = Map::new();
    for (certificate, names) in domains {
        map.insert(certificate.clone(), json!(names));
    }
    Value::Object(map)
}

/// One group of names keyed by `certificate`.
fn keyed(certificate: &str, names: &[&str]) -> (String, Vec<String>) {
    (
        certificate.to_string(),
        names.iter().map(|name| (*name).to_string()).collect(),
    )
}

/// The `publicDomainNames` `reported` carries, as a sorted map of sorted name
/// lists — the form a row's expectation is written in.
fn reported_map(reported: &Value) -> Vec<(String, Vec<String>)> {
    let mut map: Vec<(String, Vec<String>)> = reported
        .as_object()
        .map(|entries| {
            entries
                .iter()
                .map(|(certificate, names)| {
                    let mut names: Vec<String> = names
                        .as_array()
                        .map(|names| {
                            names
                                .iter()
                                .map(|name| name.as_str().unwrap_or("<not a string>").to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    names.sort();
                    (certificate.clone(), names)
                })
                .collect()
        })
        .unwrap_or_default();
    map.sort();
    map
}

/// `want` in the same sorted form [`reported_map`] answers in.
fn sorted(want: &[(String, Vec<String>)]) -> Vec<(String, Vec<String>)> {
    let mut want: Vec<(String, Vec<String>)> = want
        .iter()
        .map(|(certificate, names)| {
            let mut names = names.clone();
            names.sort();
            names.dedup();
            (certificate.clone(), names)
        })
        .collect();
    want.sort();
    want
}

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// Which operation records the names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carrier {
    /// `CreateContainerService`, storing the first set of names of a new service.
    OnCreate,
    /// `UpdateContainerService`, replacing the set a service already serves.
    OnUpdate,
}

impl Carrier {
    /// Both operations that record the member, so every row is driven through
    /// each of them.
    const ALL: [Carrier; 2] = [Carrier::OnCreate, Carrier::OnUpdate];

    /// The operation name this carrier calls.
    fn operation(self) -> &'static str {
        match self {
            Carrier::OnCreate => "CreateContainerService",
            Carrier::OnUpdate => "UpdateContainerService",
        }
    }

    /// The HTTP status a served call through this carrier answers.
    fn served_status(self) -> u16 {
        match self {
            Carrier::OnCreate => 201,
            Carrier::OnUpdate => 200,
        }
    }
}

/// The domains the service every UPDATE case acts on serves going into the case,
/// so a refusal has something to leave alone.
fn baseline() -> Vec<(String, Vec<String>)> {
    vec![keyed(HELD, &["baseline.example.com"])]
}

/// Record `domains` on the service `service_name` through `carrier`, answering the
/// HTTP status and the body.
///
/// A create carries the whole record; an update is made against a service stood up
/// with [`baseline`] first, which is what makes "the refusal wrote nothing"
/// observable.
async fn record(
    emulator: &Emulator,
    carrier: Carrier,
    service_name: &str,
    domains: &[(String, Vec<String>)],
) -> (u16, Value) {
    match carrier {
        Carrier::OnCreate => {
            let mut body = json!({"serviceName": service_name, "power": POWER, "scale": SCALE});
            body["publicDomainNames"] = public_domain_names(domains);
            emulator
                .call_in(Some(REGION), carrier.operation(), body)
                .await
        }
        Carrier::OnUpdate => {
            let mut created =
                json!({"serviceName": service_name, "power": POWER, "scale": SCALE});
            created["publicDomainNames"] = public_domain_names(&baseline());
            let (status, body) = emulator
                .call_in(Some(REGION), "CreateContainerService", created)
                .await;
            assert_eq!(
                status, 201,
                "case standing up {service_name} to update: expected HTTP 201, got {status} \
                 {body:?}"
            );
            let mut body = json!({"serviceName": service_name});
            body["publicDomainNames"] = public_domain_names(domains);
            emulator
                .call_in(Some(REGION), carrier.operation(), body)
                .await
        }
    }
}

/// One emulator holding the two issued certificates every case keys its names by.
async fn emulator_with_certificates() -> Emulator {
    let emulator = Emulator::new();
    issue_certificate(&emulator, HELD, &[HELD_DOMAIN]).await;
    issue_certificate(&emulator, SECOND_HELD, &[SECOND_HELD_DOMAIN]).await;
    emulator
}

// ---------------------------------------------------------------------------
// The refusal table
// ---------------------------------------------------------------------------

/// One `publicDomainNames` the service refuses, whichever operation records it.
struct Refused {
    case: &'static str,
    /// The map the request carries.
    domains: fn() -> Vec<(String, Vec<String>)>,
    /// Every string the refusal's message has to carry, so a caller can fix the
    /// request from the text alone: the member, and the entry (or the count) that
    /// is wrong.
    message_names: fn() -> Vec<String>,
}

/// The table. Every row breaks the one rule both operations share, and the rows
/// differ along what that rule generalises over: how the name offends (a space and
/// punctuation, a wildcard, a missing parent zone, an empty name, an empty label, a
/// trailing dot, a character no DNS label carries, a leading or trailing hyphen, a
/// label or a whole name past its published length), WHERE the offending name sits
/// (the only entry, a later entry of one list, a later key's list), how many names
/// the map carries, and whether the count is past the maximum.
const REFUSED: &[Refused] = &[
    Refused {
        case: "a name carrying a space and punctuation",
        domains: || vec![keyed(HELD, &["not a domain!!"])],
        message_names: || vec!["publicDomainNames".to_string(), "not a domain!!".to_string()],
    },
    Refused {
        case: "a wildcard name, which the certificate documentation does not support",
        domains: || vec![keyed(HELD, &["*.example.com"])],
        message_names: || vec!["publicDomainNames".to_string(), "*.example.com".to_string()],
    },
    Refused {
        case: "a name with no parent zone",
        domains: || vec![keyed(HELD, &["localhost"])],
        message_names: || vec!["publicDomainNames".to_string(), "localhost".to_string()],
    },
    Refused {
        case: "an empty name",
        domains: || vec![keyed(HELD, &[""])],
        message_names: || vec!["publicDomainNames".to_string()],
    },
    Refused {
        case: "a name with an empty label",
        domains: || vec![keyed(HELD, &["www..example.com"])],
        message_names: || {
            vec![
                "publicDomainNames".to_string(),
                "www..example.com".to_string(),
            ]
        },
    },
    Refused {
        case: "a name with a trailing dot, whose last label is empty",
        domains: || vec![keyed(HELD, &["example.com."])],
        message_names: || vec!["publicDomainNames".to_string(), "example.com.".to_string()],
    },
    Refused {
        case: "a name carrying a character no DNS label carries",
        domains: || vec![keyed(HELD, &["under_score.example.com"])],
        message_names: || {
            vec![
                "publicDomainNames".to_string(),
                "under_score.example.com".to_string(),
            ]
        },
    },
    Refused {
        case: "a name whose label begins with a hyphen",
        domains: || vec![keyed(HELD, &["-leading.example.com"])],
        message_names: || {
            vec![
                "publicDomainNames".to_string(),
                "-leading.example.com".to_string(),
            ]
        },
    },
    Refused {
        case: "a name whose label ends with a hyphen",
        domains: || vec![keyed(HELD, &["trailing-.example.com"])],
        message_names: || {
            vec![
                "publicDomainNames".to_string(),
                "trailing-.example.com".to_string(),
            ]
        },
    },
    Refused {
        case: "a label one character past the longest a DNS label may carry",
        domains: || {
            vec![keyed(
                HELD,
                &[domain_with_label_of_length(MAX_LABEL_CHARS + 1).as_str()],
            )]
        },
        message_names: || vec!["publicDomainNames".to_string()],
    },
    Refused {
        case: "a name one character past the longest a domain name may carry",
        domains: || {
            vec![keyed(
                HELD,
                &[domain_of_length(MAX_DOMAIN_NAME_CHARS + 1).as_str()],
            )]
        },
        message_names: || vec!["publicDomainNames".to_string()],
    },
    Refused {
        case: "a later entry of one certificate's list",
        domains: || vec![keyed(HELD, &["good.example.com", "zzz not a domain"])],
        message_names: || {
            vec![
                "publicDomainNames".to_string(),
                "zzz not a domain".to_string(),
            ]
        },
    },
    Refused {
        case: "the list of the second of two certificates",
        domains: || {
            vec![
                keyed(HELD, &["good.example.com"]),
                keyed(SECOND_HELD, &["also not a domain"]),
            ]
        },
        message_names: || {
            vec![
                "publicDomainNames".to_string(),
                "also not a domain".to_string(),
            ]
        },
    },
    Refused {
        case: "a malformed name keyed by a certificate the account does not hold either",
        domains: || vec![keyed(GHOST, &["not a domain!!"])],
        message_names: || vec!["publicDomainNames".to_string(), "not a domain!!".to_string()],
    },
    Refused {
        case: "one name more than a container service accepts, under one certificate",
        domains: || vec![(HELD.to_string(), distinct(MAX_PUBLIC_DOMAIN_NAMES + 1))],
        message_names: || {
            vec![
                "publicDomainNames".to_string(),
                (MAX_PUBLIC_DOMAIN_NAMES + 1).to_string(),
            ]
        },
    },
    Refused {
        case: "one name more than a container service accepts, split across two certificates",
        domains: || {
            vec![
                (HELD.to_string(), distinct(MAX_PUBLIC_DOMAIN_NAMES)),
                (
                    SECOND_HELD.to_string(),
                    vec!["extra.example.com".to_string()],
                ),
            ]
        },
        message_names: || {
            vec![
                "publicDomainNames".to_string(),
                (MAX_PUBLIC_DOMAIN_NAMES + 1).to_string(),
            ]
        },
    },
];

/// RULE: a `publicDomainNames` listing a name that is not a DNS domain name, or
/// carrying more names than a container service accepts, is refused with the
/// modelled `InvalidInputException` and records nothing — whichever of the two
/// operations carries the member.
#[tokio::test(flavor = "current_thread")]
async fn a_public_domain_name_that_is_not_a_domain_name_is_refused_by_both_operations() {
    let emulator = emulator_with_certificates().await;

    for carrier in Carrier::ALL {
        let operation = carrier.operation();
        for (index, row) in REFUSED.iter().enumerate() {
            let case = format!("{operation}: {}", row.case);
            // A fresh name per row, so a create that is refused is judged against
            // a name nothing was ever stored under.
            let service_name = format!("domain-rule-{}-{index}", operation.to_lowercase());
            let domains = (row.domains)();
            let (status, body) = record(&emulator, carrier, &service_name, &domains).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} {body:?}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, "InvalidInputException",
                "case {case}: expected the modelled InvalidInputException, got {code} {body:?}"
            );
            let message = body["message"].as_str().unwrap_or_default();
            for named in (row.message_names)() {
                assert!(
                    message.contains(&named),
                    "case {case}: expected a message naming {named:?}, got {message:?}"
                );
            }

            match carrier {
                Carrier::OnCreate => {
                    // A refused create stores nothing: the name is still free.
                    let (status, read) = emulator
                        .call_in(
                            Some(REGION),
                            "GetContainerServices",
                            json!({"serviceName": service_name}),
                        )
                        .await;
                    assert_eq!(
                        status, 400,
                        "case {case}: expected a refused create to store no container service, \
                         and reading {service_name} answered {status} {read:?}"
                    );
                    assert_eq!(
                        read["code"].as_str(),
                        Some(DOES_NOT_EXIST),
                        "case {case}: expected reading {service_name} to answer \
                         {DOES_NOT_EXIST}, got {read:?}"
                    );
                }
                Carrier::OnUpdate => {
                    // A refused update leaves the names the service already
                    // served.
                    let reported =
                        reported_domains(&emulator, &case, REGION, &service_name).await;
                    let got = reported_map(&reported);
                    let want = sorted(&baseline());
                    assert_eq!(
                        got, want,
                        "case {case}: expected a refused update to leave {want:?}, got {got:?}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The served table
// ---------------------------------------------------------------------------

/// One `publicDomainNames` the service records, and what every later read of the
/// service then reports.
struct Served {
    case: &'static str,
    domains: fn() -> Vec<(String, Vec<String>)>,
    /// What the stored service reports, which differs from the request only where
    /// the service de-dupes it.
    reported: fn() -> Vec<(String, Vec<String>)>,
}

/// The table. The rows differ along what the rule generalises over: how many
/// certificates key the names and how many names each carries (up to the maximum a
/// container service accepts), a label and a whole name at their published bounds,
/// a subdomain of a subdomain, a label opening with a digit, and a list whose
/// repeats de-dupe to within the maximum.
const SERVED: &[Served] = &[
    Served {
        case: "one certificate's single name",
        domains: || vec![keyed(HELD, &["svc.example.com"])],
        reported: || vec![keyed(HELD, &["svc.example.com"])],
    },
    Served {
        case: "the most names a container service accepts, under one certificate",
        domains: || vec![(HELD.to_string(), distinct(MAX_PUBLIC_DOMAIN_NAMES))],
        reported: || vec![(HELD.to_string(), distinct(MAX_PUBLIC_DOMAIN_NAMES))],
    },
    Served {
        case: "the most names a container service accepts, split across two certificates",
        domains: || {
            vec![
                keyed(HELD, &["a.example.com", "b.example.com"]),
                keyed(SECOND_HELD, &["c.example.org", "d.example.org"]),
            ]
        },
        reported: || {
            vec![
                keyed(HELD, &["a.example.com", "b.example.com"]),
                keyed(SECOND_HELD, &["c.example.org", "d.example.org"]),
            ]
        },
    },
    Served {
        case: "a subdomain of a subdomain and a label opening with a digit",
        domains: || {
            vec![keyed(
                HELD,
                &["cdn.shop.example.com", "1-digit.example.com"],
            )]
        },
        reported: || {
            vec![keyed(
                HELD,
                &["cdn.shop.example.com", "1-digit.example.com"],
            )]
        },
    },
    Served {
        case: "a label at the longest a DNS label may carry",
        domains: || {
            vec![keyed(
                HELD,
                &[domain_with_label_of_length(MAX_LABEL_CHARS).as_str()],
            )]
        },
        reported: || {
            vec![keyed(
                HELD,
                &[domain_with_label_of_length(MAX_LABEL_CHARS).as_str()],
            )]
        },
    },
    Served {
        case: "a name at the longest a domain name may carry",
        domains: || {
            vec![keyed(
                HELD,
                &[domain_of_length(MAX_DOMAIN_NAME_CHARS).as_str()],
            )]
        },
        reported: || {
            vec![keyed(
                HELD,
                &[domain_of_length(MAX_DOMAIN_NAME_CHARS).as_str()],
            )]
        },
    },
    Served {
        case: "repeats that de-dupe to within the maximum",
        domains: || {
            let mut names = distinct(MAX_PUBLIC_DOMAIN_NAMES);
            names.extend(distinct(MAX_PUBLIC_DOMAIN_NAMES));
            vec![(HELD.to_string(), names)]
        },
        reported: || vec![(HELD.to_string(), distinct(MAX_PUBLIC_DOMAIN_NAMES))],
    },
];

/// RULE: a `publicDomainNames` whose every name is a DNS domain name and whose
/// de-duped names are within the maximum a container service accepts is served by
/// either operation, and the service reports exactly those names on a later read.
#[tokio::test(flavor = "current_thread")]
async fn well_formed_public_domain_names_are_served_by_both_operations() {
    let emulator = emulator_with_certificates().await;

    for carrier in Carrier::ALL {
        let operation = carrier.operation();
        for (index, row) in SERVED.iter().enumerate() {
            let case = format!("{operation}: {}", row.case);
            let service_name = format!("domain-served-{}-{index}", operation.to_lowercase());
            let domains = (row.domains)();
            let (status, body) = record(&emulator, carrier, &service_name, &domains).await;
            assert_eq!(
                status,
                carrier.served_status(),
                "case {case}: expected HTTP {}, got {status} {body:?}",
                carrier.served_status()
            );
            let want = sorted(&(row.reported)());
            let answered = reported_map(&body["containerService"]["publicDomainNames"]);
            assert_eq!(
                answered, want,
                "case {case}: expected the answered record to carry {want:?}, got {answered:?}"
            );
            // The round trip: what was recorded is what a later read reports.
            let reported = reported_domains(&emulator, &case, REGION, &service_name).await;
            let got = reported_map(&reported);
            assert_eq!(
                got, want,
                "case {case}: expected the stored service to report {want:?}, got {got:?}"
            );
        }
    }
}
