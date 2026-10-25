// SPDX-License-Identifier: CC0-1.0

//! Each rule `ChangeResourceRecordSets` refuses a record set over is reported
//! as ITS OWN refusal.
//!
//! One name and type are served either by a single simple record set, which
//! carries no `SetIdentifier`, or by a group of record sets with "a routing
//! policy other than simple" (`SetIdentifier`'s own documentation), each told
//! from its siblings by that identifier. Several different rules can refuse a
//! change at one such name and type, and each is a different thing for the
//! caller to put right, so each answers with its own error code and its own
//! message:
//!
//! * a SIMPLE record set written where a group already answers cannot be
//!   created at all, because other record sets hold that name and type;
//! * a record set WITH a routing policy written where one simple record set
//!   already answers conflicts with what is there;
//! * a change naming a record set the zone already holds is a duplicate of
//!   that one record set, named by its identifier;
//! * a routing policy written with no `SetIdentifier` is a fault in the
//!   REQUEST, not in what the zone holds, so it answers `InvalidInput`.
//!
//! Both suites drive the service through the real router with the harness
//! `hosted_zone_records` owns, and write their changes with the builder
//! `record_set_validation` owns, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use crate::hosted_zone_records::{create_zone_ok, router, Reply};
use crate::record_name_canonical_form::count_named;
use crate::record_set_validation::{change, list, message, submit, Rrset};

/// The zone both tables below work inside.
const ZONE: &str = "example.com";

/// The ONE canonical form of a name, which a refusal and a listing report it
/// in whatever spelling the change was written with: fully qualified by the
/// root dot, and "a name's letters come back folded to lower case however they
/// were sent" (`Name`'s own documentation).
fn canonical_name(name: &str) -> String {
    format!("{}.", name.trim_end_matches('.').to_lowercase())
}

/// Assert one refused batch: the status, the error code, the whole message, and
/// that the refusal is reported as the one problem it is.
fn assert_refusal(case: &str, reply: &Reply, code: &str, expected: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: expected 400, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        code,
        "case {case:?}: expected {code}, got {}",
        reply.body
    );
    let actual = message(reply);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the refusal {expected:?}, got {actual:?}"
    );
}

// ---------------------------------------------------------------------------
// 1. A simple record set written where a routing group already answers
// ---------------------------------------------------------------------------

/// One name and type already held by a routing group, and the simple record set
/// that may not join it.
struct GroupCase {
    label: &'static str,
    /// The name the group answers for, as the refused change spells it — which
    /// is not always the canonical form the refusal reports.
    name: &'static str,
    rr_type: &'static str,
    /// A record data value that suits `rr_type`, since a value is parsed
    /// against the syntax of the type it was submitted for.
    value: &'static str,
    /// The routing-policy element(s) each seeded sibling carries.
    routing: &'static str,
    /// One identifier per seeded sibling, so the list's length is the group's
    /// size.
    identifiers: &'static [&'static str],
}

/// The rows span what the rule generalises over: the routing policy that makes
/// the group (the members `ResourceRecordSet` declares for it —
/// `smithy_query(query="shape",shape="ResourceRecordSet")`: `Weight`, `Region`,
/// `GeoLocation`, `Failover`, `MultiValueAnswer`), the record type (from the
/// modelled `RRType` value set), the number of siblings already in the group,
/// and the spelling of the name the change was written with.
const GROUP_CASES: &[GroupCase] = &[
    GroupCase {
        label: "a weighted A group of two",
        name: "weighted.example.com",
        rr_type: "A",
        value: "192.0.2.10",
        routing: "<Weight>10</Weight>",
        identifiers: &["blue", "green"],
    },
    GroupCase {
        label: "a weighted A group of one",
        name: "lone-weight.example.com",
        rr_type: "A",
        value: "192.0.2.11",
        routing: "<Weight>0</Weight>",
        identifiers: &["only"],
    },
    GroupCase {
        label: "a weighted A group of three",
        name: "three-weights.example.com",
        rr_type: "A",
        value: "192.0.2.12",
        routing: "<Weight>255</Weight>",
        identifiers: &["blue", "green", "canary"],
    },
    GroupCase {
        label: "a latency AAAA group",
        name: "latency.example.com",
        rr_type: "AAAA",
        value: "2001:db8::1",
        routing: "<Region>us-west-2</Region>",
        identifiers: &["west", "east"],
    },
    GroupCase {
        label: "a failover CNAME group",
        name: "failover.example.com",
        rr_type: "CNAME",
        value: "target.example.net.",
        routing: "<Failover>PRIMARY</Failover>",
        identifiers: &["primary", "secondary"],
    },
    GroupCase {
        label: "a geolocation TXT group",
        name: "geo.example.com",
        rr_type: "TXT",
        value: "\"owned\"",
        routing: "<GeoLocation><CountryCode>US</CountryCode></GeoLocation>",
        identifiers: &["us", "row"],
    },
    GroupCase {
        label: "a multivalue answer MX group, named without its trailing dot",
        name: "MULTI.example.com.",
        rr_type: "MX",
        value: "10 mail.example.net.",
        routing: "<MultiValueAnswer>true</MultiValueAnswer>",
        identifiers: &["one", "two"],
    },
];

#[tokio::test]
async fn a_simple_record_set_is_refused_where_a_routing_group_already_answers() {
    // The rule: a name and type are served either by one simple record set or
    // by a group of record sets with a routing policy other than simple, so a
    // simple record set written where a group already answers cannot be
    // created — refused with `InvalidChangeBatch`, stating that other record
    // sets hold that name and type, and naming neither a `SetIdentifier` (the
    // rule is about the name and type, not about one sibling) nor the zone.
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-simple-vs-group").await;

    for case in GROUP_CASES {
        let label = case.label;
        let group: String = case
            .identifiers
            .iter()
            .map(|identifier| {
                change(
                    "CREATE",
                    &Rrset::new(case.name, case.rr_type)
                        .set_identifier(identifier)
                        .routing(case.routing)
                        .ttl(60)
                        .values(&[case.value]),
                )
            })
            .collect();
        let seeded = submit(&router, &zone_id, &group).await;
        assert_eq!(
            seeded.status, 200,
            "case {label}: the group must be created, got {} with body {}",
            seeded.status, seeded.body
        );

        let canonical = canonical_name(case.name);
        let simple = Rrset::new(case.name, case.rr_type)
            .ttl(60)
            .values(&[case.value]);
        let expected = format!(
            "[RRSet with DNS name {canonical}, type {} cannot be created as other RRSets exist \
             with the same name and type.]",
            case.rr_type
        );
        for action in ["CREATE", "UPSERT"] {
            let case = format!("{label} ({action})");
            let refused = submit(&router, &zone_id, &change(action, &simple)).await;
            assert_refusal(&case, &refused, "InvalidChangeBatch", &expected);
            // "Each error message indicates one error in the change batch":
            // one refused change, one entry.
            let entries = refused.all("messages/member");
            assert_eq!(
                entries.len(),
                1,
                "case {case:?}: expected one message entry, got {entries:?}"
            );
        }

        // A refused batch lands nothing: the group is exactly as it was, and
        // no record set without an identifier joined it.
        assert_eq!(
            count_named(&router, &zone_id, &canonical).await,
            case.identifiers.len(),
            "case {label}: the refusals must leave the group of {} as it was",
            case.identifiers.len()
        );

        // The control: another identifier IS free to join the group, so what
        // was refused is the mixing of the two kinds and not the name itself.
        let joining = Rrset::new(case.name, case.rr_type)
            .set_identifier("joining")
            .routing(case.routing)
            .ttl(60)
            .values(&[case.value]);
        let accepted = submit(&router, &zone_id, &change("CREATE", &joining)).await;
        assert_eq!(
            accepted.status, 200,
            "case {label}: a further sibling must be accepted, got {} with body {}",
            accepted.status, accepted.body
        );
        assert_eq!(
            count_named(&router, &zone_id, &canonical).await,
            case.identifiers.len() + 1,
            "case {label}: the accepted sibling must be listed beside the group"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. One name, several rules, several refusals
// ---------------------------------------------------------------------------

/// One change refused at a name the zone already answers for, and the refusal
/// it must answer with.
struct RuleCase {
    label: &'static str,
    /// The change, already rendered.
    batch: String,
    code: &'static str,
    expected: String,
}

#[tokio::test]
async fn the_rules_refusing_a_record_set_are_each_reported_distinctly() {
    // The rule: the refusals a record set can draw at one name and type are
    // distinct rules with distinct remedies — delete the group, pick another
    // identifier, supply the missing member — so each answers with its own
    // code and its own message rather than one shared wording. The rows are
    // the rules that can refuse a change at a name and type a zone already
    // answers for: a simple record set beside a group, a group member beside a
    // simple record set, a duplicate of a record set already there, and a
    // routing policy with no identifier at all.
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-refusal-rules").await;

    // A weighted group at one name, and a simple record set at another.
    let grouped = format!("grouped.{ZONE}");
    let plain = format!("plain.{ZONE}");
    let sibling = |identifier: &str, weight: u32| {
        Rrset::new(&grouped, "A")
            .set_identifier(identifier)
            .routing(&format!("<Weight>{weight}</Weight>"))
            .ttl(60)
            .values(&["192.0.2.40"])
    };
    let simple_at_plain = Rrset::new(&plain, "A").ttl(60).values(&["192.0.2.41"]);
    let seed = format!(
        "{}{}",
        change("CREATE", &sibling("blue", 10)),
        change("CREATE", &simple_at_plain),
    );
    let seeded = submit(&router, &zone_id, &seed).await;
    assert_eq!(
        seeded.status, 200,
        "the seed batch must be applied, got {} with body {}",
        seeded.status, seeded.body
    );

    let simple_at_grouped = Rrset::new(&grouped, "A").ttl(60).values(&["192.0.2.42"]);
    let weighted_at_plain = Rrset::new(&plain, "A")
        .set_identifier("joining")
        .routing("<Weight>20</Weight>")
        .ttl(60)
        .values(&["192.0.2.43"]);
    let unidentified_at_plain = Rrset::new(&plain, "A")
        .routing("<Weight>30</Weight>")
        .ttl(60)
        .values(&["192.0.2.44"]);

    let cases = vec![
        RuleCase {
            label: "a simple record set where a weighted group answers",
            batch: change("CREATE", &simple_at_grouped),
            code: "InvalidChangeBatch",
            expected: format!(
                "[RRSet with DNS name {}, type A cannot be created as other RRSets exist with \
                 the same name and type.]",
                simple_at_grouped.canonical()
            ),
        },
        RuleCase {
            label: "a weighted record set where a simple record set answers",
            batch: change("CREATE", &weighted_at_plain),
            code: "InvalidChangeBatch",
            expected: format!(
                "[RRSet of type A with DNS name {} is not permitted as it conflicts with other \
                 records with the same DNS name and type in zone {ZONE}.]",
                weighted_at_plain.canonical()
            ),
        },
        RuleCase {
            label: "a duplicate of the sibling the group already holds",
            batch: change("CREATE", &sibling("blue", 10)),
            code: "InvalidChangeBatch",
            expected: format!(
                "[Tried to create resource record set [name='{}', type='A', \
                 set-identifier='blue'] but it already exists]",
                sibling("blue", 10).canonical()
            ),
        },
        RuleCase {
            label: "a weighted record set carrying no identifier",
            batch: change("CREATE", &unidentified_at_plain),
            code: "InvalidInput",
            expected: format!(
                "Invalid request: Missing field 'SetIdentifier' in Change with [Action=CREATE, \
                 Name={}, Type=A, SetIdentifier=null]",
                unidentified_at_plain.canonical()
            ),
        },
    ];

    let mut seen: Vec<(&str, String)> = Vec::new();
    for case in &cases {
        let refused = submit(&router, &zone_id, &case.batch).await;
        assert_refusal(case.label, &refused, case.code, &case.expected);
        seen.push((case.label, message(&refused)));
    }

    // No two of these rules answer with the same message: a client can tell
    // which one it broke.
    for (index, (label, refusal)) in seen.iter().enumerate() {
        for (other_label, other) in &seen[index + 1..] {
            assert_ne!(
                refusal, other,
                "cases {label:?} and {other_label:?} are different rules, but both answered \
                 {refusal:?}"
            );
        }
    }

    // None of the refusals touched the zone: the group kept its one sibling
    // and the simple record set is still the only record set at its name.
    assert_eq!(
        count_named(&router, &zone_id, &simple_at_grouped.canonical()).await,
        1,
        "the refused batches must leave the group as it was"
    );
    assert_eq!(
        count_named(&router, &zone_id, &simple_at_plain.canonical()).await,
        1,
        "the refused batches must leave the simple record set as it was"
    );

    // The control: a second identifier joins the group and round-trips through
    // the listing, so only the mixed states above were refused.
    let accepted = submit(&router, &zone_id, &change("CREATE", &sibling("green", 20))).await;
    assert_eq!(
        accepted.status, 200,
        "a second sibling must be accepted, got {} with body {}",
        accepted.status, accepted.body
    );
    let after = list(&router, &zone_id).await;
    assert_eq!(
        after.all("ResourceRecordSet/SetIdentifier"),
        vec!["blue", "green"],
        "the group must list both of its siblings, got {}",
        after.body
    );
}
