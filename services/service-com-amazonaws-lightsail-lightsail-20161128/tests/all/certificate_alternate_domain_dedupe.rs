// SPDX-License-Identifier: CC0-1.0

//! The alternate domains a `CreateCertificate` request names are de-duped before
//! they are stored, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: an entry a certificate's alternative-name list repeats,
//! and an entry that already IS the certificate's primary domain, add no domain
//! to the certificate — so what is stored and reported is the list with those
//! dropped, in the order the request first named them, and the documented
//! maximum of alternate domains is applied to that reduced list rather than to
//! the entries the request sent. One certificate therefore never disagrees with
//! itself: it reports one domain-validation record per domain it covers, which is
//! its primary domain followed by exactly the alternate domains it reports.
//!
//! The sibling suite `load_balancer_tls_certificate_create.rs` pins the same rule
//! for the certificates a load balancer holds; both certificate kinds go through
//! the one owner of the reduction.

use serde_json::{json, Value};

// The entry bound the de-duping is affordable under, as the service itself
// declares it, rather than a second copy of the number in this suite.
use service_com_amazonaws_lightsail_lightsail_20161128::state::MAX_DOMAIN_NAME_LIST_ENTRIES;

// The harness, the wire error code and the certificate read this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::distribution_certificate_attachment::certificate_entry;
use crate::instance_lifecycle::{error_code, Emulator};

/// Assert that `name` names no certificate of this account: the read is refused
/// with Lightsail's `DoesNotExist` envelope.
async fn assert_nothing_stored(emulator: &Emulator, case: &str, name: &str) {
    let (status, body) = emulator
        .call("GetCertificates", json!({"certificateName": name}))
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected a refused create to store no certificate, and reading {name:?} \
         answered {status} {body:?}"
    );
    assert_eq!(
        body["code"].as_str(),
        Some(DOES_NOT_EXIST),
        "case {case}: expected reading {name:?} to answer {DOES_NOT_EXIST}, got {body:?}"
    );
}

/// The wire error code of the refusal a request the model admits but the service
/// does not serve answers with.
const INVALID_INPUT: &str = "InvalidInputException";

/// The value Lightsail puts in a not-found body's `code` member, which a
/// Lightsail SDK client branches on rather than on the HTTP status.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The most alternate domains one certificate may cover, from the modelled
/// `CreateCertificate$subjectAlternativeNames` documentation ("You can specify a
/// maximum of nine alternate domains (in addition to the primary domain
/// name)").
const MAX_ALTERNATE_DOMAINS: usize = 9;

/// The primary domain every certificate in this suite is for.
const PRIMARY: &str = "primary.example.com";

/// Nine distinct alternate domains of [`PRIMARY`] — the most one certificate may
/// cover.
const NINE_DISTINCT: &[&str] = &[
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

/// Ten distinct ones: one past the documented maximum, which no de-duping
/// reduces.
const TEN_DISTINCT: &[&str] = &[
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

/// Ten entries naming the nine distinct alternate domains above: the last
/// repeats the first.
const TEN_WITH_NINE_DISTINCT: &[&str] = &[
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
];

/// Eleven entries naming those same nine: one repeat and the certificate's own
/// domain, neither of which adds a domain to the certificate.
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
    PRIMARY,
];

/// Twelve entries that still name ten distinct alternate domains: de-duping does
/// not bring them within the documented maximum.
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
    PRIMARY,
];

/// Nine copies of the certificate's own domain: nine entries that add no
/// alternate domain at all.
const NINE_COPIES_OF_PRIMARY: &[&str] = &[
    PRIMARY, PRIMARY, PRIMARY, PRIMARY, PRIMARY, PRIMARY, PRIMARY, PRIMARY, PRIMARY,
];

/// What the service does with one `subjectAlternativeNames` list.
#[derive(Debug, Clone, Copy)]
enum Outcome {
    /// Served, storing exactly these alternate domains in this order.
    Stored(&'static [&'static str]),
    /// Refused with the modelled `InvalidInputException`, storing nothing: the
    /// de-duped list names more alternate domains than the documented maximum.
    Refused,
}

/// One `CreateCertificate` request, and what the certificate it describes then
/// covers.
///
/// The rows differ along everything the rule generalises over: how many entries
/// the list carries, how many of them are repeats, whether the certificate's own
/// domain is among them, the order the distinct names first appear in, and
/// whether what the de-duping leaves is within the documented maximum.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    certificate_name: &'static str,
    sent: &'static [&'static str],
    outcome: Outcome,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "no alternate domain at all",
        certificate_name: "dedupe-none-cert",
        sent: &[],
        outcome: Outcome::Stored(&[]),
    },
    Row {
        case: "an entry that repeats an earlier entry",
        certificate_name: "dedupe-repeat-cert",
        sent: &[
            "www.primary.example.com",
            "img.primary.example.com",
            "www.primary.example.com",
        ],
        outcome: Outcome::Stored(&["www.primary.example.com", "img.primary.example.com"]),
    },
    Row {
        case: "the certificate's own domain among the alternate domains",
        certificate_name: "dedupe-primary-cert",
        sent: &[PRIMARY, "blog.primary.example.com"],
        outcome: Outcome::Stored(&["blog.primary.example.com"]),
    },
    Row {
        case: "nine copies of the certificate's own domain, which add no alternate domain",
        certificate_name: "dedupe-primary-nine-cert",
        sent: NINE_COPIES_OF_PRIMARY,
        outcome: Outcome::Stored(&[]),
    },
    Row {
        case: "repeats and the primary domain interleaved, so order is the first naming",
        certificate_name: "dedupe-order-cert",
        sent: &[
            "c.primary.example.com",
            "b.primary.example.com",
            "c.primary.example.com",
            "a.primary.example.com",
            PRIMARY,
            "b.primary.example.com",
        ],
        outcome: Outcome::Stored(&[
            "c.primary.example.com",
            "b.primary.example.com",
            "a.primary.example.com",
        ]),
    },
    Row {
        // The published bound: nine alternate domains "in addition to the
        // primary domain name".
        case: "the documented nine distinct alternate domains",
        certificate_name: "dedupe-nine-cert",
        sent: NINE_DISTINCT,
        outcome: Outcome::Stored(NINE_DISTINCT),
    },
    Row {
        case: "ten entries that de-dupe to the nine the maximum admits",
        certificate_name: "dedupe-ten-entries-cert",
        sent: TEN_WITH_NINE_DISTINCT,
        outcome: Outcome::Stored(NINE_DISTINCT),
    },
    Row {
        case: "eleven entries, a repeat and the primary domain among them, de-duping to nine",
        certificate_name: "dedupe-eleven-entries-cert",
        sent: ELEVEN_WITH_NINE_DISTINCT,
        outcome: Outcome::Stored(NINE_DISTINCT),
    },
    Row {
        // One past the published bound, on the list the de-duping leaves.
        case: "ten distinct alternate domains, one past the documented nine",
        certificate_name: "dedupe-ten-distinct-cert",
        sent: TEN_DISTINCT,
        outcome: Outcome::Refused,
    },
    Row {
        case: "twelve entries that still name ten distinct alternate domains",
        certificate_name: "dedupe-twelve-entries-cert",
        sent: TWELVE_WITH_TEN_DISTINCT,
        outcome: Outcome::Refused,
    },
];

/// The `CreateCertificate` body for the certificate `name` for [`PRIMARY`] with
/// `alternatives` as its alternate domains.
fn create_body(name: &str, alternatives: &[&str]) -> Value {
    json!({
        "certificateName": name,
        "domainName": PRIMARY,
        "subjectAlternativeNames": alternatives,
    })
}

/// The string list `value` reports, with a placeholder for an entry that is not
/// a string at all, so a failure prints what came back.
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

/// The domains the certificate `detail` reports a validation record for, in the
/// order it reports them.
fn validated_domains(detail: &Value) -> Vec<String> {
    detail["domainValidationRecords"]
        .as_array()
        .map(|records| {
            records
                .iter()
                .map(|record| {
                    record["domainName"]
                        .as_str()
                        .unwrap_or("<not a string>")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Assert that the certificate `detail` covers exactly `alternates` in addition
/// to [`PRIMARY`], and that it does not disagree with itself: one
/// domain-validation record per domain it covers, primary domain first.
fn assert_covers(case: &str, detail: &Value, alternates: &[&str]) {
    assert_eq!(
        string_list(&detail["subjectAlternativeNames"]),
        alternates,
        "case {case}: expected the de-duped alternate domains, got {:?}",
        detail["subjectAlternativeNames"]
    );
    let mut covered = vec![PRIMARY.to_string()];
    covered.extend(alternates.iter().map(|name| (*name).to_string()));
    assert_eq!(
        validated_domains(detail),
        covered,
        "case {case}: expected one validation record per domain the certificate covers, got {:?}",
        detail["domainValidationRecords"]
    );
}

/// RULE: a repeat, and an entry that already is the certificate's own domain, add
/// no alternate domain — so the stored list is the request's with those dropped,
/// in the order it first named them — and the documented maximum of nine
/// alternate domains is applied to that reduced list, so a request past it is
/// refused with the modelled `InvalidInputException` and stores nothing.
#[tokio::test]
async fn alternate_domains_are_de_duped_before_they_are_stored_and_bounded() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let (status, answer) = emulator
            .call(
                "CreateCertificate",
                create_body(row.certificate_name, row.sent),
            )
            .await;

        match row.outcome {
            Outcome::Stored(alternates) => {
                assert_eq!(
                    status, 200,
                    "case {case}: {} entries name {} alternate domains, within the \
                     {MAX_ALTERNATE_DOMAINS} documented, so the certificate is created, got \
                     {status} {answer:?}",
                    row.sent.len(),
                    alternates.len()
                );
                assert!(
                    alternates.len() <= MAX_ALTERNATE_DOMAINS,
                    "case {case}: the row expects {} alternate domains, more than the \
                     {MAX_ALTERNATE_DOMAINS} documented",
                    alternates.len()
                );
                let created = answer["certificate"]["certificateDetail"].clone();
                assert_covers(case, &created, alternates);

                // The round trip: what the create stored is what a later read
                // reports, so the de-duping happened on the way IN rather than
                // in one response.
                let (read_status, read) = certificate_entry(&emulator, row.certificate_name).await;
                assert_eq!(
                    read_status, 200,
                    "case {case}: expected the created certificate to be readable, got \
                     {read_status} {read:?}"
                );
                assert_covers(case, &read["certificateDetail"], alternates);
            }
            Outcome::Refused => {
                assert_eq!(
                    status, 400,
                    "case {case}: more than {MAX_ALTERNATE_DOMAINS} distinct alternate domains is \
                     refused, got {status} {answer:?}"
                );
                let code = error_code(&answer);
                assert_eq!(
                    code, INVALID_INPUT,
                    "case {case}: expected the wire error code {INVALID_INPUT}, got {code} \
                     {answer:?}"
                );
                assert!(
                    answer["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("subjectAlternativeNames")),
                    "case {case}: expected a message naming subjectAlternativeNames, got \
                     {answer:?}"
                );
                assert_nothing_stored(&emulator, case, row.certificate_name).await;
            }
        }
    }
}

/// RULE: how many ENTRIES an alternative-name list may carry is bounded before
/// any entry is read — whether an entry is a repeat is only known once it has
/// been examined — so a list at the bound whose entries all name one domain is
/// served, storing that one alternate domain, and one entry past the bound is
/// refused.
#[tokio::test]
async fn the_alternate_domain_list_is_bounded_before_it_is_read() {
    let emulator = Emulator::new();
    let only = "www.primary.example.com";
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
        let name = format!("entries-cert-{index}");
        // Every entry names the SAME alternate domain, so the de-duping leaves
        // one: what decides each case is the entry count alone.
        let sent = vec![only; entries];
        let (status, answer) = emulator
            .call("CreateCertificate", create_body(&name, &sent))
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case}: {entries} entries name one alternate domain, so the certificate is \
                 created, got {status} {answer:?}"
            );
            assert_covers(case, &answer["certificate"]["certificateDetail"], &[only]);
        } else {
            assert_eq!(
                status, 400,
                "case {case}: {entries} entries is more than the list may carry, got {status} \
                 {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, INVALID_INPUT,
                "case {case}: expected the wire error code {INVALID_INPUT}, got {code} {answer:?}"
            );
            assert_nothing_stored(&emulator, case, &name).await;
        }
    }
}
