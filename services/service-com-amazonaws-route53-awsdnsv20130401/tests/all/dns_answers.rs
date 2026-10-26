// SPDX-License-Identifier: CC0-1.0

//! What `TestDNSAnswer` answers, and where the answer comes from.
//!
//! The rule every suite below asserts: the operation simulates one DNS query
//! against the hosted zone the request names and answers it FROM the record sets
//! that zone persists — the response code from whether the zone holds the
//! queried name and the queried type at all, the record data from the record set
//! the routing policy at that name and type answers with, and the name server
//! from the zone's own delegation set. Nothing in the answer is canned: a zone
//! that holds no record for a name answers that it holds none, and a zone whose
//! records change answers differently afterwards.
//!
//! Each table spans what the rule generalises over rather than the one request
//! that exposed it: the record type and the number of values at a name, the
//! three outcomes a query has (an answer, a name without that type, a name the
//! zone does not hold), the routing policy the group at a name carries, the
//! source of a record set's data (its own values, an alias inside the zone, an
//! alias to a resource outside it), and the refusals the operation models.
//!
//! The harness (`router`, `call`, `Reply`) and the request builders come from
//! `hosted_zone_records` and `record_set_validation`, so these suites drive the
//! service through the same router a socket-served request goes through.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, create_zone_body, create_zone_ok, router, Reply};
use crate::record_set_validation::{change, message, submit, Rrset};

/// The zone the answering suites query inside.
const ZONE: &str = "example.com";

/// The one input error this service's model declares for an input it cannot use.
const INVALID_INPUT: &str = "InvalidInput";

/// The response code of a query the zone answers, and of a name it holds
/// records at but none of the queried type.
const NOERROR: &str = "NOERROR";

/// The response code of a query for a name the zone holds no record at.
const NXDOMAIN: &str = "NXDOMAIN";

/// Ask `TestDNSAnswer` one question, with the query string already rendered.
async fn ask(router: &MultiServiceRouter, query: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/testdnsanswer?{query}"),
        "",
    )
    .await
}

/// Ask for one name and type in the zone `zone_id`.
async fn ask_for(
    router: &MultiServiceRouter,
    zone_id: &str,
    name: &str,
    rr_type: &str,
) -> Reply {
    ask(
        router,
        &format!("hostedzoneid={zone_id}&recordname={name}&recordtype={rr_type}"),
    )
    .await
}

/// The record data of an answer, in the order the response lists it.
fn record_data(reply: &Reply) -> Vec<&str> {
    reply.all("RecordData/RecordDataEntry")
}

/// The response code of an answer.
fn response_code(reply: &Reply) -> &str {
    reply.first("ResponseCode").unwrap_or("")
}

/// Assert the query was answered at all.
fn assert_served(case: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200, got {} with body {}",
        reply.status, reply.body
    );
}

/// Seed one record set into a zone and assert the batch was accepted.
async fn seed(router: &MultiServiceRouter, zone_id: &str, record: &Rrset) {
    let reply = submit(router, zone_id, &change("CREATE", record)).await;
    assert_eq!(
        reply.status, 200,
        "seeding {record:?}: expected 200, got {} with body {}",
        reply.status, reply.body
    );
}

// ---------------------------------------------------------------------------
// 1. An answer is the record data the zone holds
// ---------------------------------------------------------------------------

/// One record set seeded into the zone, and the query that must read it back.
struct AnswerCase {
    label: &'static str,
    /// The record set's name, or `None` to query a record the zone already
    /// holds (its default apex `SOA` / `NS`) rather than seeding one.
    seed_name: Option<&'static str>,
    rr_type: &'static str,
    ttl: i64,
    values: &'static [&'static str],
    /// The name the query spells, which need not be the spelling the record
    /// was created with.
    query_name: &'static str,
    /// The name the answer reports, in the one canonical form this service
    /// stores and returns a name in.
    answer_name: &'static str,
}

#[tokio::test]
async fn an_answer_carries_the_record_data_the_zone_holds_at_that_name_and_type() {
    // The rule: a query for a name and type the zone holds a record set at is
    // answered NOERROR with that record set's values, one `RecordDataEntry` per
    // value, and echoes the name in its canonical form and the type asked for.
    // The rows span the record type (across the modelled `RRType` value set),
    // the number of values at one name (one, several), the depth of the name
    // (apex, one label, two), the spelling the query uses (trailing dot, upper
    // case) and whether the record set was seeded by a change batch or created
    // with the zone.
    let cases = [
        AnswerCase {
            label: "a single-value A record",
            seed_name: Some("www.example.com"),
            rr_type: "A",
            ttl: 60,
            values: &["192.0.2.1"],
            query_name: "www.example.com",
            answer_name: "www.example.com.",
        },
        AnswerCase {
            label: "an A record with three values, queried without the trailing dot",
            seed_name: Some("many.example.com."),
            rr_type: "A",
            ttl: 300,
            values: &["192.0.2.10", "192.0.2.11", "192.0.2.12"],
            query_name: "many.example.com",
            answer_name: "many.example.com.",
        },
        AnswerCase {
            label: "an AAAA record queried in upper case",
            seed_name: Some("six.example.com"),
            rr_type: "AAAA",
            ttl: 60,
            values: &["2001:db8::1"],
            query_name: "SIX.EXAMPLE.COM",
            answer_name: "six.example.com.",
        },
        AnswerCase {
            label: "a two-label name under the zone",
            seed_name: Some("deep.sub.example.com"),
            rr_type: "A",
            ttl: 60,
            values: &["192.0.2.20"],
            query_name: "deep.sub.example.com",
            answer_name: "deep.sub.example.com.",
        },
        AnswerCase {
            label: "a TXT record, whose values are character strings",
            seed_name: Some("text.example.com"),
            rr_type: "TXT",
            ttl: 120,
            values: &["\"v=spf1 -all\"", "\"second string\""],
            query_name: "text.example.com",
            answer_name: "text.example.com.",
        },
        AnswerCase {
            label: "an MX record, whose value is a compound of preference and host",
            seed_name: Some("example.com"),
            rr_type: "MX",
            ttl: 60,
            values: &["10 mail.example.net.", "20 backup.example.net."],
            query_name: "example.com.",
            answer_name: "example.com.",
        },
        AnswerCase {
            label: "a CNAME record, whose value is a domain name",
            seed_name: Some("alias-name.example.com"),
            rr_type: "CNAME",
            ttl: 60,
            values: &["target.example.net."],
            query_name: "alias-name.example.com",
            answer_name: "alias-name.example.com.",
        },
        AnswerCase {
            label: "the NS record the zone was created with",
            seed_name: None,
            rr_type: "NS",
            ttl: 0,
            values: &[],
            query_name: "example.com",
            answer_name: "example.com.",
        },
        AnswerCase {
            label: "the SOA record the zone was created with",
            seed_name: None,
            rr_type: "SOA",
            ttl: 0,
            values: &[],
            query_name: "example.com",
            answer_name: "example.com.",
        },
    ];

    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-answers").await;
    // The zone's four authoritative name servers, as the zone itself reports
    // them: the answer must name the one that served the query, and it is one
    // of these.
    let zone = call(
        &router,
        "GET",
        &format!("/2013-04-01/hostedzone/{zone_id}"),
        "",
    )
    .await;
    let name_servers: Vec<String> = zone
        .all("NameServers/NameServer")
        .iter()
        .map(|server| (*server).to_string())
        .collect();
    assert!(
        !name_servers.is_empty(),
        "the zone must report its name servers, got {}",
        zone.body
    );

    for case in &cases {
        if let Some(name) = case.seed_name {
            seed(
                &router,
                &zone_id,
                &Rrset::new(name, case.rr_type)
                    .ttl(case.ttl)
                    .values(case.values),
            )
            .await;
        }
        let reply = ask_for(&router, &zone_id, case.query_name, case.rr_type).await;
        assert_served(case.label, &reply);
        assert_eq!(
            response_code(&reply),
            NOERROR,
            "case {:?}: expected {NOERROR}, got {} with body {}",
            case.label,
            response_code(&reply),
            reply.body
        );
        assert_eq!(
            reply.first("RecordName"),
            Some(case.answer_name),
            "case {:?}: expected the name in canonical form, got {:?}",
            case.label,
            reply.first("RecordName")
        );
        assert_eq!(
            reply.first("RecordType"),
            Some(case.rr_type),
            "case {:?}: expected the type asked for, got {:?}",
            case.label,
            reply.first("RecordType")
        );
        assert_eq!(
            reply.first("Protocol"),
            Some("UDP"),
            "case {:?}: expected the simulated query's transport, got {:?}",
            case.label,
            reply.first("Protocol")
        );
        let nameserver = reply.first("Nameserver").unwrap_or("").to_string();
        assert!(
            name_servers.contains(&nameserver),
            "case {:?}: the answering name server must be one of the zone's \
             ({name_servers:?}), got {nameserver:?}",
            case.label
        );
        if case.seed_name.is_some() {
            assert_eq!(
                record_data(&reply),
                case.values,
                "case {:?}: expected the values the zone holds, got {:?}",
                case.label,
                record_data(&reply)
            );
        } else {
            // The zone's own apex records: their values are minted with the
            // zone, so what is asserted is that the answer carries them.
            assert!(
                !record_data(&reply).is_empty(),
                "case {:?}: the default apex record must answer with its data, got {}",
                case.label,
                reply.body
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. A name or type the zone does not hold
// ---------------------------------------------------------------------------

/// One query the zone has no record data for, and the response code it gets.
struct EmptyCase {
    label: &'static str,
    query_name: &'static str,
    rr_type: &'static str,
    code: &'static str,
}

#[tokio::test]
async fn a_name_the_zone_does_not_hold_answers_nxdomain_and_a_missing_type_answers_no_data() {
    // The rule: the response code says which of the two the query ran into — a
    // name the zone holds records at, but none of the queried type, is a valid
    // query with no data (`NOERROR`, no `RecordDataEntry`), while a name the
    // zone holds no record at — including one outside the zone the name servers
    // are authoritative for — does not exist (`NXDOMAIN`). The rows span both
    // outcomes, the depth of the name, and whether the name is inside the zone
    // at all.
    let cases = [
        EmptyCase {
            label: "a name the zone holds, queried for a type it has no record of",
            query_name: "www.example.com",
            rr_type: "TXT",
            code: NOERROR,
        },
        EmptyCase {
            label: "the apex, queried for a type the zone has no record of",
            query_name: "example.com",
            rr_type: "CAA",
            code: NOERROR,
        },
        EmptyCase {
            label: "a name the zone holds no record at",
            query_name: "absent.example.com",
            rr_type: "A",
            code: NXDOMAIN,
        },
        EmptyCase {
            label: "a deeper name the zone holds no record at",
            query_name: "a.b.c.example.com",
            rr_type: "A",
            code: NXDOMAIN,
        },
        EmptyCase {
            label: "a name outside the zone",
            query_name: "www.example.net",
            rr_type: "A",
            code: NXDOMAIN,
        },
        EmptyCase {
            label: "a name whose suffix only looks like the zone's",
            query_name: "www.evil-example.com",
            rr_type: "A",
            code: NXDOMAIN,
        },
    ];

    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-empty").await;
    seed(
        &router,
        &zone_id,
        &Rrset::new("www.example.com", "A")
            .ttl(60)
            .values(&["192.0.2.1"]),
    )
    .await;

    for case in &cases {
        let reply = ask_for(&router, &zone_id, case.query_name, case.rr_type).await;
        assert_served(case.label, &reply);
        assert_eq!(
            response_code(&reply),
            case.code,
            "case {:?}: expected {}, got {} with body {}",
            case.label,
            case.code,
            response_code(&reply),
            reply.body
        );
        assert!(
            record_data(&reply).is_empty(),
            "case {:?}: a query with no data must carry none, got {:?}",
            case.label,
            record_data(&reply)
        );
        assert_eq!(
            reply.first("RecordType"),
            Some(case.rr_type),
            "case {:?}: expected the type asked for, got {:?}",
            case.label,
            reply.first("RecordType")
        );
    }

    // The answer is read off state, not off the name's shape: deleting the one
    // record set turns the `NOERROR` above into `NXDOMAIN` for the same query.
    let deleted = submit(
        &router,
        &zone_id,
        &change(
            "DELETE",
            &Rrset::new("www.example.com", "A")
                .ttl(60)
                .values(&["192.0.2.1"]),
        ),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "the delete must be accepted, got {} with body {}",
        deleted.status, deleted.body
    );
    let after = ask_for(&router, &zone_id, "www.example.com", "TXT").await;
    assert_eq!(
        response_code(&after),
        NXDOMAIN,
        "once the only record at the name is deleted the name no longer exists, got {}",
        after.body
    );
}

// ---------------------------------------------------------------------------
// 3. Weighted routing
// ---------------------------------------------------------------------------

/// One weighted group, and which of its values may answer a query.
struct WeightCase {
    label: &'static str,
    name: &'static str,
    /// One `(SetIdentifier, Weight, value)` per member of the group.
    members: &'static [(&'static str, i64, &'static str)],
    /// The values a query may be answered with.
    answerable: &'static [&'static str],
    /// Whether the group must answer with more than one distinct value over the
    /// draws below — true where more than one member carries a positive share,
    /// or where every share is zero.
    varies: bool,
}

/// How many times each weighted group is queried.
///
/// A proportional draw is a draw: one query says little, so each row is asked
/// repeatedly. With the smallest group that must vary (three members of equal
/// share), the chance of this many draws all falling to one member is below
/// 10^-19, so the "varies" assertion is not a flake.
const DRAWS: usize = 40;

#[tokio::test]
async fn weighted_routing_answers_in_proportion_and_never_with_a_zero_weight_record() {
    // The rule (`Weight`'s own documentation): among the record sets at one name
    // and type, Route 53 answers "based on the ratio of a resource's weight to
    // the total"; a record set whose weight is 0 never answers, unless every
    // weight in the group is 0, in which case the group answers with equal
    // probability. The rows span the shares (a positive beside a zero, several
    // positives, all zeros), and the size of the group (one, two, three).
    let cases = [
        WeightCase {
            label: "a zero-weight sibling never answers",
            name: "blue-green.example.com",
            members: &[("blue", 100, "192.0.2.31"), ("green", 0, "192.0.2.32")],
            answerable: &["192.0.2.31"],
            varies: false,
        },
        WeightCase {
            label: "two zero-weight siblings never answer",
            name: "canary.example.com",
            members: &[
                ("live", 255, "192.0.2.33"),
                ("dark", 0, "192.0.2.34"),
                ("off", 0, "192.0.2.35"),
            ],
            answerable: &["192.0.2.33"],
            varies: false,
        },
        WeightCase {
            label: "unequal positive shares both answer",
            name: "uneven.example.com",
            members: &[("one", 1, "192.0.2.36"), ("nine", 9, "192.0.2.37")],
            answerable: &["192.0.2.36", "192.0.2.37"],
            varies: true,
        },
        WeightCase {
            label: "a group whose every weight is zero answers with equal probability",
            name: "all-zero.example.com",
            members: &[
                ("a", 0, "192.0.2.38"),
                ("b", 0, "192.0.2.39"),
                ("c", 0, "192.0.2.40"),
            ],
            answerable: &["192.0.2.38", "192.0.2.39", "192.0.2.40"],
            varies: true,
        },
        WeightCase {
            label: "a lone zero-weight record set is the whole group, so it answers",
            name: "lone-zero.example.com",
            members: &[("only", 0, "192.0.2.41")],
            answerable: &["192.0.2.41"],
            varies: false,
        },
    ];

    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-weighted").await;

    for case in &cases {
        for (identifier, weight, value) in case.members {
            seed(
                &router,
                &zone_id,
                &Rrset::new(case.name, "A")
                    .set_identifier(identifier)
                    .routing(&format!("<Weight>{weight}</Weight>"))
                    .ttl(60)
                    .values(&[value]),
            )
            .await;
        }

        let mut seen: Vec<String> = Vec::new();
        for draw in 0..DRAWS {
            let reply = ask_for(&router, &zone_id, case.name, "A").await;
            assert_served(case.label, &reply);
            let data = record_data(&reply);
            assert_eq!(
                data.len(),
                1,
                "case {:?} draw {draw}: a weighted group answers with one member, got {data:?}",
                case.label
            );
            let answered = data[0].to_string();
            assert!(
                case.answerable.contains(&answered.as_str()),
                "case {:?} draw {draw}: expected one of {:?}, got {answered:?}",
                case.label,
                case.answerable
            );
            if !seen.contains(&answered) {
                seen.push(answered);
            }
        }
        assert!(
            seen.len() > 1 || !case.varies,
            "case {:?}: every member with a share must be able to answer, only saw {seen:?}",
            case.label
        );
        assert!(
            seen.len() == 1 || case.varies,
            "case {:?}: only the members with a share may answer, saw {seen:?}",
            case.label
        );
    }
}

// ---------------------------------------------------------------------------
// 4. The other routing policies
// ---------------------------------------------------------------------------

/// One routing group, and the values a query must be answered with.
struct PolicyCase {
    label: &'static str,
    name: &'static str,
    /// One `(SetIdentifier, routing elements, value)` per member of the group.
    members: &'static [(&'static str, &'static str, &'static str)],
    /// The values the answer must carry, in the order it must carry them.
    expect: &'static [&'static str],
}

#[tokio::test]
async fn each_routing_policy_answers_from_the_member_its_own_rule_selects() {
    // The rule: which member of a group answers is decided by the policy the
    // group carries — the healthy `PRIMARY` of a failover pair, up to eight
    // members of a multivalue answer group, the `*` member of a group routed by
    // a location this service cannot place (`GeoLocation`'s own documentation:
    // "queries from IP addresses that aren't mapped to a location" go to the `*`
    // record, and a group without one answers "no answer"), and the member in
    // the Region of the resolver the checking tool simulates. The rows span the
    // policies, the size of the group, and whether the member the rule names is
    // present at all.
    let cases = [
        PolicyCase {
            label: "a failover pair answers from its primary",
            name: "failover.example.com",
            members: &[
                ("primary", "<Failover>PRIMARY</Failover>", "192.0.2.51"),
                ("secondary", "<Failover>SECONDARY</Failover>", "192.0.2.52"),
            ],
            expect: &["192.0.2.51"],
        },
        PolicyCase {
            label: "a failover group with only a secondary answers from it",
            name: "secondary-only.example.com",
            members: &[("secondary", "<Failover>SECONDARY</Failover>", "192.0.2.53")],
            expect: &["192.0.2.53"],
        },
        PolicyCase {
            label: "a multivalue answer group of three answers with all three",
            name: "multi.example.com",
            members: &[
                ("one", "<MultiValueAnswer>true</MultiValueAnswer>", "192.0.2.61"),
                ("three", "<MultiValueAnswer>true</MultiValueAnswer>", "192.0.2.63"),
                ("two", "<MultiValueAnswer>true</MultiValueAnswer>", "192.0.2.62"),
            ],
            // The members answer in `SetIdentifier` order, which is the order
            // the zone holds them in.
            expect: &["192.0.2.61", "192.0.2.63", "192.0.2.62"],
        },
        PolicyCase {
            label: "a geolocation group answers from its default member",
            name: "geo.example.com",
            members: &[
                (
                    "row",
                    "<GeoLocation><CountryCode>*</CountryCode></GeoLocation>",
                    "192.0.2.71",
                ),
                (
                    "us",
                    "<GeoLocation><CountryCode>US</CountryCode></GeoLocation>",
                    "192.0.2.72",
                ),
                (
                    "eu",
                    "<GeoLocation><ContinentCode>EU</ContinentCode></GeoLocation>",
                    "192.0.2.73",
                ),
            ],
            expect: &["192.0.2.71"],
        },
        PolicyCase {
            label: "a geolocation group with no default member gives no answer",
            name: "geo-no-default.example.com",
            members: &[
                (
                    "us",
                    "<GeoLocation><CountryCode>US</CountryCode></GeoLocation>",
                    "192.0.2.74",
                ),
                (
                    "de",
                    "<GeoLocation><CountryCode>DE</CountryCode></GeoLocation>",
                    "192.0.2.75",
                ),
            ],
            expect: &[],
        },
        PolicyCase {
            label: "a latency group answers from the simulated resolver's own Region",
            name: "latency.example.com",
            members: &[
                ("east", "<Region>us-east-1</Region>", "192.0.2.81"),
                ("west", "<Region>us-west-2</Region>", "192.0.2.82"),
            ],
            expect: &["192.0.2.81"],
        },
        PolicyCase {
            label: "a latency group holding no member in that Region still answers",
            name: "latency-elsewhere.example.com",
            members: &[
                ("ap", "<Region>ap-south-1</Region>", "192.0.2.83"),
                ("eu", "<Region>eu-west-1</Region>", "192.0.2.84"),
            ],
            expect: &["192.0.2.83"],
        },
    ];

    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-policies").await;

    for case in &cases {
        for (identifier, routing, value) in case.members {
            seed(
                &router,
                &zone_id,
                &Rrset::new(case.name, "A")
                    .set_identifier(identifier)
                    .routing(routing)
                    .ttl(60)
                    .values(&[value]),
            )
            .await;
        }
        let reply = ask_for(&router, &zone_id, case.name, "A").await;
        assert_served(case.label, &reply);
        assert_eq!(
            response_code(&reply),
            NOERROR,
            "case {:?}: expected {NOERROR}, got {}",
            case.label,
            reply.body
        );
        assert_eq!(
            record_data(&reply),
            case.expect,
            "case {:?}: expected {:?}, got {:?}",
            case.label,
            case.expect,
            record_data(&reply)
        );
    }
}

#[tokio::test]
async fn a_multivalue_answer_group_answers_with_at_most_eight_of_its_members() {
    // The same rule at its documented bound: "Route 53 responds to DNS queries
    // with up to eight healthy records; if you have eight or fewer healthy
    // records, Route 53 responds to all DNS queries with all the healthy
    // records." The rows are the group sizes around that bound.
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-multivalue").await;

    for size in [7usize, 8, 9, 12] {
        let name = format!("multi-{size}.example.com");
        for member in 0..size {
            let value = format!("192.0.2.{}", 100 + member);
            seed(
                &router,
                &zone_id,
                &Rrset::new(&name, "A")
                    .set_identifier(&format!("m{member:02}"))
                    .routing("<MultiValueAnswer>true</MultiValueAnswer>")
                    .ttl(60)
                    .values(&[value.as_str()]),
            )
            .await;
        }
        let reply = ask_for(&router, &zone_id, &name, "A").await;
        assert_served(&name, &reply);
        let answered = record_data(&reply);
        assert_eq!(
            answered.len(),
            size.min(8),
            "a group of {size} must answer with {} value(s), got {answered:?}",
            size.min(8)
        );
    }
}

// ---------------------------------------------------------------------------
// 5. Alias record sets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_alias_answers_with_the_data_of_what_it_targets() {
    // The rule (`RecordDataEntry`'s own documentation): an alias record set that
    // targets another record set answers with "one value from the referenced
    // resource record set", resolved through that record set's own routing, and
    // one that targets an Amazon Web Services resource outside this emulator
    // answers with "a domain name for the … resource". The cases span a target
    // with one value, a target with several, a chain of aliases, a target that
    // is itself a routing group, and a target outside the zone.
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-alias").await;

    seed(
        &router,
        &zone_id,
        &Rrset::new("one.example.com", "A")
            .ttl(60)
            .values(&["192.0.2.91"]),
    )
    .await;
    seed(
        &router,
        &zone_id,
        &Rrset::new("several.example.com", "A")
            .ttl(60)
            .values(&["192.0.2.92", "192.0.2.93"]),
    )
    .await;
    for (identifier, weight, value) in [("live", 10, "192.0.2.94"), ("dark", 0, "192.0.2.95")] {
        seed(
            &router,
            &zone_id,
            &Rrset::new("group.example.com", "A")
                .set_identifier(identifier)
                .routing(&format!("<Weight>{weight}</Weight>"))
                .ttl(60)
                .values(&[value]),
        )
        .await;
    }

    let aliases = [
        ("to-one.example.com", "one.example.com"),
        ("to-several.example.com", "several.example.com"),
        ("to-group.example.com", "group.example.com"),
        ("to-alias.example.com", "to-one.example.com"),
    ];
    for (name, target) in aliases {
        seed(
            &router,
            &zone_id,
            &Rrset::new(name, "A").alias(&zone_id, target),
        )
        .await;
    }
    // An alias to a resource this emulator holds no hosted zone for: the
    // documented answer is a domain name for that resource.
    seed(
        &router,
        &zone_id,
        &Rrset::new("to-outside.example.com", "A")
            .alias("Z2FDTNDATAQYW2", "d111abcdef8.cloudfront.net"),
    )
    .await;

    let cases: [(&str, &str, &[&str]); 5] = [
        ("an alias to a single-value record", "to-one.example.com", &["192.0.2.91"]),
        (
            "an alias to a record with several values",
            "to-several.example.com",
            &["192.0.2.92", "192.0.2.93"],
        ),
        (
            "an alias to a weighted group answers through that group's rule",
            "to-group.example.com",
            &["192.0.2.94"],
        ),
        ("an alias to another alias", "to-alias.example.com", &["192.0.2.91"]),
        (
            "an alias to a resource outside this emulator",
            "to-outside.example.com",
            &["d111abcdef8.cloudfront.net."],
        ),
    ];

    for (label, name, expect) in cases {
        let reply = ask_for(&router, &zone_id, name, "A").await;
        assert_served(label, &reply);
        assert_eq!(
            response_code(&reply),
            NOERROR,
            "case {label:?}: expected {NOERROR}, got {}",
            reply.body
        );
        assert_eq!(
            record_data(&reply),
            expect,
            "case {label:?}: expected {expect:?}, got {:?}",
            record_data(&reply)
        );
    }
}

#[tokio::test]
async fn an_alias_walk_is_bounded_whatever_the_zone_holds() {
    // The other half of the alias rule: an alias answers from the ONE record set
    // it references, so the walk follows one record set per hop and ends when it
    // returns to a (zone, name, type) it already resolved. Whatever arrangement
    // of alias record sets a zone holds — a self-reference, a two-record cycle, a
    // cycle through a multivalue group, or a chain longer than the bound — one
    // query is answered promptly and with bounded record data, rather than
    // expanding into the product of the groups along the chain.
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-alias-bound").await;

    // A record set that aliases its own name and type.
    seed(
        &router,
        &zone_id,
        &Rrset::new("self.example.com", "A").alias(&zone_id, "self.example.com"),
    )
    .await;
    // Two record sets that alias each other.
    seed(
        &router,
        &zone_id,
        &Rrset::new("ping.example.com", "A").alias(&zone_id, "pong.example.com"),
    )
    .await;
    seed(
        &router,
        &zone_id,
        &Rrset::new("pong.example.com", "A").alias(&zone_id, "ping.example.com"),
    )
    .await;
    // A multivalue answer group of eight alias members, every one of them
    // aliasing the group's own name: the shape whose fan-out would multiply.
    for member in 0..8 {
        seed(
            &router,
            &zone_id,
            &Rrset::new("fan.example.com", "A")
                .set_identifier(&format!("m{member}"))
                .routing("<MultiValueAnswer>true</MultiValueAnswer>")
                .alias(&zone_id, "fan.example.com"),
        )
        .await;
    }
    // A chain of twelve aliases, longer than the hop bound, ending nowhere.
    for hop in 0..12 {
        seed(
            &router,
            &zone_id,
            &Rrset::new(&format!("hop{hop}.example.com"), "A")
                .alias(&zone_id, &format!("hop{}.example.com", hop + 1)),
        )
        .await;
    }

    for name in [
        "self.example.com",
        "ping.example.com",
        "fan.example.com",
        "hop0.example.com",
    ] {
        let reply = ask_for(&router, &zone_id, name, "A").await;
        assert_served(name, &reply);
        assert_eq!(
            response_code(&reply),
            NOERROR,
            "case {name:?}: the name exists, so the query is valid, got {}",
            reply.body
        );
        assert!(
            record_data(&reply).len() <= 8,
            "case {name:?}: an unresolvable alias arrangement must answer with bounded \
             data, got {:?}",
            record_data(&reply)
        );
    }

    // A chain exactly at the bound still answers, so the bound is not a cap on
    // ordinary use: eight hops to a record set with values.
    seed(
        &router,
        &zone_id,
        &Rrset::new("end.example.com", "A")
            .ttl(60)
            .values(&["192.0.2.99"]),
    )
    .await;
    for hop in 0..8 {
        let target = if hop == 7 {
            "end.example.com".to_string()
        } else {
            format!("chain{}.example.com", hop + 1)
        };
        seed(
            &router,
            &zone_id,
            &Rrset::new(&format!("chain{hop}.example.com"), "A").alias(&zone_id, &target),
        )
        .await;
    }
    let chained = ask_for(&router, &zone_id, "chain0.example.com", "A").await;
    assert_served("a chain at the hop bound", &chained);
    assert_eq!(
        record_data(&chained),
        vec!["192.0.2.99"],
        "a chain within the hop bound must answer with the target's value, got {:?}",
        record_data(&chained)
    );
}

// ---------------------------------------------------------------------------
// 6. The refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_zone_id_that_names_no_zone_is_refused_nosuchhostedzone() {
    // The rule: the zone is looked up in state, and an id that names none is
    // refused `NoSuchHostedZone` (404) naming the id that was looked for. The
    // rows span the id's form and the spelling the request used.
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-missing").await;
    let neighbour = format!("{zone_id}X");

    for id in ["Z00000000000000", "BOGUS", "0", neighbour.as_str()] {
        let reply = ask_for(&router, id, "www.example.com", "A").await;
        assert_eq!(
            reply.status, 404,
            "case {id:?}: expected 404, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "NoSuchHostedZone",
            "case {id:?}: expected NoSuchHostedZone, got {}",
            reply.body
        );
        assert_eq!(
            message(&reply),
            format!("No hosted zone found with ID: {id}"),
            "case {id:?}: the refusal must name the id that was looked for, got {:?}",
            message(&reply)
        );
    }
}

#[tokio::test]
async fn a_private_hosted_zone_cannot_be_queried() {
    // The operation's own documentation: "This call only supports querying
    // public hosted zones." A private zone answers inside the VPCs it is
    // associated with, and no simulated resolver is in one, so the query is
    // refused rather than answered from the private zone's records.
    let router = router();
    let created = call(
        &router,
        "POST",
        "/2013-04-01/hostedzone",
        &create_zone_body(
            "internal.example.com",
            "ref-private",
            "<HostedZoneConfig><PrivateZone>true</PrivateZone></HostedZoneConfig>\
             <VPC><VPCRegion>us-east-1</VPCRegion><VPCId>vpc-1a2b3c4d</VPCId></VPC>",
        ),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "the private zone must be created, got {} with body {}",
        created.status, created.body
    );
    let zone_id = created
        .first("HostedZone/Id")
        .unwrap_or_else(|| panic!("the create must report the zone; body {}", created.body))
        .trim_start_matches("/hostedzone/")
        .to_string();

    let reply = ask_for(&router, &zone_id, "internal.example.com", "SOA").await;
    assert_eq!(
        reply.status, 400,
        "expected 400, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        INVALID_INPUT,
        "expected {INVALID_INPUT}, got {}",
        reply.body
    );
    assert!(
        message(&reply).contains(&zone_id) && message(&reply).contains("public hosted zones"),
        "the refusal must name the zone and the rule, got {:?}",
        message(&reply)
    );
}

/// One EDNS0 client subnet a query names, and whether it may be simulated from.
struct SubnetCase {
    label: &'static str,
    /// The `edns0clientsubnetip` and `edns0clientsubnetmask` query parameters.
    query: &'static str,
    /// 200 for a subnet the query is simulated from, 400 for one it is not.
    status: u16,
}

#[tokio::test]
async fn an_edns0_client_subnet_mask_outside_its_documented_range_is_refused() {
    // The rule (`EDNS0ClientSubnetMask`'s own documentation): "The range of
    // valid values depends on whether edns0clientsubnetip is an IPv4 or an IPv6
    // address: IPv4: Specify a value between 0 and 32. IPv6: Specify a value
    // between 0 and 128." The rows span both families, each published bound,
    // one past each bound, a mask that is not a number at all, and a mask sent
    // with no address for it to be bits of.
    let cases = [
        SubnetCase {
            label: "an IPv4 subnet at the bottom of the range",
            query: "&edns0clientsubnetip=192.0.2.44&edns0clientsubnetmask=0",
            status: 200,
        },
        SubnetCase {
            label: "an IPv4 subnet inside the range",
            query: "&edns0clientsubnetip=192.0.2.44&edns0clientsubnetmask=24",
            status: 200,
        },
        SubnetCase {
            label: "an IPv4 subnet at the top of the range",
            query: "&edns0clientsubnetip=192.0.2.44&edns0clientsubnetmask=32",
            status: 200,
        },
        SubnetCase {
            label: "an IPv4 subnet one bit past the top of the range",
            query: "&edns0clientsubnetip=192.0.2.44&edns0clientsubnetmask=33",
            status: 400,
        },
        SubnetCase {
            label: "an IPv6 subnet at the top of its own range",
            query: "&edns0clientsubnetip=2001:db8:85a3::8a2e:370:7334&edns0clientsubnetmask=128",
            status: 200,
        },
        SubnetCase {
            label: "an IPv6 subnet inside the IPv6 range but past the IPv4 one",
            query: "&edns0clientsubnetip=2001:db8:85a3::8a2e:370:7334&edns0clientsubnetmask=64",
            status: 200,
        },
        SubnetCase {
            label: "an IPv6 subnet one bit past the top of its range",
            query: "&edns0clientsubnetip=2001:db8:85a3::8a2e:370:7334&edns0clientsubnetmask=129",
            status: 400,
        },
        SubnetCase {
            label: "a mask that is not a number",
            query: "&edns0clientsubnetip=192.0.2.44&edns0clientsubnetmask=abc",
            status: 400,
        },
        SubnetCase {
            label: "a mask with no address to be bits of",
            query: "&edns0clientsubnetmask=24",
            status: 200,
        },
        SubnetCase {
            label: "an address with no mask at all",
            query: "&edns0clientsubnetip=192.0.2.44",
            status: 200,
        },
        SubnetCase {
            label: "a resolver address and a client subnet together",
            query: "&resolverip=198.51.100.5&edns0clientsubnetip=192.0.2.44\
                    &edns0clientsubnetmask=24",
            status: 200,
        },
    ];

    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-subnet").await;
    seed(
        &router,
        &zone_id,
        &Rrset::new("www.example.com", "A")
            .ttl(60)
            .values(&["192.0.2.1"]),
    )
    .await;

    for case in &cases {
        let reply = ask(
            &router,
            &format!(
                "hostedzoneid={zone_id}&recordname=www.example.com&recordtype=A{}",
                case.query
            ),
        )
        .await;
        assert_eq!(
            reply.status, case.status,
            "case {:?}: expected {}, got {} with body {}",
            case.label, case.status, reply.status, reply.body
        );
        if case.status == 200 {
            // A subnet the query may be simulated from changes nothing about
            // the answer the zone's records give.
            assert_eq!(
                record_data(&reply),
                vec!["192.0.2.1"],
                "case {:?}: expected the record the zone holds, got {:?}",
                case.label,
                record_data(&reply)
            );
        } else {
            assert_eq!(
                reply.error_code(),
                INVALID_INPUT,
                "case {:?}: expected {INVALID_INPUT}, got {}",
                case.label,
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_record_type_outside_the_modelled_value_set_is_refused() {
    // The queried type is a modelled enum (`RRType`), so a value outside its
    // value set is refused as the input fault it is — `InvalidInput`, 400 —
    // rather than answered, and nothing in the zone is read for it. The rows
    // span a type that does not exist, a modelled type in the wrong case, and
    // the DNS query type `ANY`, which `RRType` does not name.
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-bad-type").await;

    for rr_type in ["NOTAREALTYPE", "a", "ANY", "ALIAS"] {
        let reply = ask_for(&router, &zone_id, "www.example.com", rr_type).await;
        assert_eq!(
            reply.status, 400,
            "case {rr_type:?}: expected 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            INVALID_INPUT,
            "case {rr_type:?}: expected {INVALID_INPUT}, got {}",
            reply.body
        );
        assert!(
            message(&reply).to_lowercase().contains("recordtype"),
            "case {rr_type:?}: the refusal must name the member it was about, got {:?}",
            message(&reply)
        );
    }

    // A type the value set DOES name is served against the same zone.
    let served_type = ask_for(&router, &zone_id, ZONE, "NS").await;
    assert_eq!(
        served_type.status, 200,
        "a modelled record type must be served, got {} with body {}",
        served_type.status, served_type.body
    );
}
