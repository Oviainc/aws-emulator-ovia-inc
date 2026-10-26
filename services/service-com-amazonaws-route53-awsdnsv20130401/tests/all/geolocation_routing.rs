// SPDX-License-Identifier: CC0-1.0

//! Which member of a geolocation group `TestDNSAnswer` answers a query from.
//!
//! The rule every table below asserts: the operation resolves the query it
//! simulates the way the zone's name servers would, so a group of record sets
//! that differ only in the LOCATION they route by is resolved against where the
//! query appears to come from — the client subnet the request names
//! (`EDNS0ClientSubnetIP` with `EDNS0ClientSubnetMask`), or the resolver's own
//! address when it names no client subnet. The member whose location covers that
//! origin most specifically answers: a country record set in preference to a
//! continent one that covers the same country, and the `*` default only for a
//! query no sibling's location covers. A query whose address places nowhere is
//! one "from [an IP address] that [isn't] mapped to a location", which the same
//! `*` default answers — and where the group holds no default, the name still
//! exists, so the answer is `NOERROR` with no record data at all rather than a
//! sibling the location rules do not pick.
//!
//! Each table spans what that rule generalises over rather than the one query
//! that exposed it: the continent and country the client subnet places in, the
//! granularity the sibling names its location at (country, country with a
//! subdivision, continent, the `*` default), the bits of the address the mask
//! includes, the form of the address (IPv4, IPv6, an address no public
//! delegation record places), which of the two origin inputs carries it, and
//! whether a covering sibling or a default exists at all.
//!
//! The harness (`router`, `call`, `Reply`) and the change builders (`Rrset`,
//! `change`, `submit`) come from `hosted_zone_records` and
//! `record_set_validation`, so every assertion is about what a Route 53 client
//! sees over HTTP.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, create_zone_ok, router, Reply};
use crate::record_set_validation::{change, list, submit, Rrset};

/// The zone every group below lives in.
const ZONE: &str = "example.com";

/// The response code of a query the zone answers, and of a query it holds a name
/// for but no record the location rules pick.
const NOERROR: &str = "NOERROR";

/// Seed one record set and assert the batch was accepted.
async fn seed(router: &MultiServiceRouter, zone_id: &str, record: &Rrset) {
    let reply = submit(router, zone_id, &change("CREATE", record)).await;
    assert_eq!(
        reply.status, 200,
        "seeding {record:?}: expected 200, got {} with body {}",
        reply.status, reply.body
    );
}

/// Delete one record set and assert the batch was accepted.
async fn delete(router: &MultiServiceRouter, zone_id: &str, record: &Rrset) {
    let reply = submit(router, zone_id, &change("DELETE", record)).await;
    assert_eq!(
        reply.status, 200,
        "deleting {record:?}: expected 200, got {} with body {}",
        reply.status, reply.body
    );
}

/// A `<GeoLocation>` naming a country.
fn in_country(code: &str) -> String {
    format!("<GeoLocation><CountryCode>{code}</CountryCode></GeoLocation>")
}

/// A `<GeoLocation>` naming a subdivision of a country — the only pairing
/// `GeoLocation$SubdivisionCode` admits ("If you specify subdivisioncode, you
/// must also specify US for CountryCode").
fn in_subdivision(country: &str, subdivision: &str) -> String {
    format!(
        "<GeoLocation><CountryCode>{country}</CountryCode>\
         <SubdivisionCode>{subdivision}</SubdivisionCode></GeoLocation>"
    )
}

/// A `<GeoLocation>` naming a continent.
fn on_continent(code: &str) -> String {
    format!("<GeoLocation><ContinentCode>{code}</ContinentCode></GeoLocation>")
}

/// Ask `TestDNSAnswer` for one name and type, with `origin` carrying whatever the
/// request says about where the query comes from.
async fn ask(
    router: &MultiServiceRouter,
    zone_id: &str,
    name: &str,
    origin: &str,
) -> Reply {
    call(
        router,
        "GET",
        &format!(
            "/2013-04-01/testdnsanswer?hostedzoneid={zone_id}&recordname={name}\
             &recordtype=A{origin}"
        ),
        "",
    )
    .await
}

/// The record data of an answer, in the order the response lists it.
fn record_data(reply: &Reply) -> Vec<&str> {
    reply.all("RecordData/RecordDataEntry")
}

/// Assert one answer: served, `NOERROR`, and exactly the record data expected.
fn assert_answer(case: &str, reply: &Reply, expected: &[&str]) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.first("ResponseCode"),
        Some(NOERROR),
        "case {case:?}: expected {NOERROR}, got {:?}",
        reply.first("ResponseCode")
    );
    assert_eq!(
        record_data(reply),
        expected.to_vec(),
        "case {case:?}: expected {expected:?}, got {:?}",
        record_data(reply)
    );
}

// ---------------------------------------------------------------------------
// 1. The covering location, most specific first
// ---------------------------------------------------------------------------

/// One query against the group seeded below, and the value it must be answered
/// with.
struct OriginCase {
    label: &'static str,
    /// The query-string members that say where the query comes from, appended to
    /// a request that already names the zone, the name and the type.
    origin: &'static str,
    expect: &'static str,
}

/// The name the group of siblings sits at.
const GROUP: &str = "geo.example.com";

/// The value of each member of that group, one per location it routes by.
const DE: &str = "203.0.113.80";
const JP: &str = "203.0.113.81";
const US: &str = "203.0.113.82";
const US_WA: &str = "203.0.113.83";
const EU: &str = "203.0.113.84";
const ASIA: &str = "203.0.113.85";
const DEFAULT: &str = "203.0.113.86";

#[tokio::test]
async fn a_geolocation_group_answers_from_the_member_whose_location_covers_the_client_subnet() {
    // The rule: the member whose location covers the query's origin answers,
    // most specific cover first, and the `*` default answers every origin no
    // sibling covers — including one this emulator cannot place.
    //
    // The client subnets below are addresses of blocks the public delegation
    // records place in the country named in each label (the source of the
    // emulator's own table of them, `state::client_location`). Rows span six
    // continents and eight countries, the three granularities a sibling names a
    // location at, the mask (named, omitted, a /32 host, a /8 wider than any one
    // delegation), an IPv6 subnet, an address no delegation record places, a
    // resolver address standing in for an absent client subnet, and a request
    // that says nothing about its origin at all.
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-geo-cover").await;
    for (identifier, location, value) in [
        ("de", in_country("DE"), DE),
        ("jp", in_country("JP"), JP),
        ("us", in_country("US"), US),
        // A subdivision of the US, which is more specific than the `us` sibling
        // and covers only a query placed in that subdivision.
        ("us-wa", in_subdivision("US", "WA"), US_WA),
        ("eu", on_continent("EU"), EU),
        ("as", on_continent("AS"), ASIA),
        ("default", in_country("*"), DEFAULT),
    ] {
        seed(
            &router,
            &zone_id,
            &Rrset::new(GROUP, "A")
                .set_identifier(identifier)
                .routing(&location)
                .ttl(60)
                .values(&[value]),
        )
        .await;
    }

    let cases = [
        OriginCase {
            label: "a German client subnet takes the DE record, not the EU sibling",
            origin: "&edns0clientsubnetip=62.155.0.0&edns0clientsubnetmask=24",
            expect: DE,
        },
        OriginCase {
            label: "a second German block takes the same DE record",
            origin: "&edns0clientsubnetip=217.0.1.0&edns0clientsubnetmask=24",
            expect: DE,
        },
        OriginCase {
            label: "a German client subnet with no mask takes the DE record",
            origin: "&edns0clientsubnetip=62.155.0.0",
            expect: DE,
        },
        OriginCase {
            label: "a German host address, masked to its /24, takes the DE record",
            origin: "&edns0clientsubnetip=62.155.4.9&edns0clientsubnetmask=24",
            expect: DE,
        },
        OriginCase {
            label: "a Dutch client subnet has no country sibling and takes the EU sibling",
            origin: "&edns0clientsubnetip=193.0.0.0&edns0clientsubnetmask=24",
            expect: EU,
        },
        OriginCase {
            label: "a Swedish client subnet takes the EU sibling",
            origin: "&edns0clientsubnetip=192.36.148.0&edns0clientsubnetmask=24",
            expect: EU,
        },
        OriginCase {
            label: "a British client subnet takes the EU sibling",
            origin: "&edns0clientsubnetip=212.58.224.0&edns0clientsubnetmask=24",
            expect: EU,
        },
        OriginCase {
            label: "a Japanese client subnet takes the JP record, not the AS sibling",
            origin: "&edns0clientsubnetip=210.130.0.0&edns0clientsubnetmask=24",
            expect: JP,
        },
        OriginCase {
            label: "a second Japanese block takes the same JP record",
            origin: "&edns0clientsubnetip=202.12.27.0&edns0clientsubnetmask=24",
            expect: JP,
        },
        OriginCase {
            label: "a Chinese client subnet has no country sibling and takes the AS sibling",
            origin: "&edns0clientsubnetip=223.5.5.0&edns0clientsubnetmask=24",
            expect: ASIA,
        },
        OriginCase {
            label: "a Korean client subnet takes the AS sibling",
            origin: "&edns0clientsubnetip=168.126.63.0&edns0clientsubnetmask=24",
            expect: ASIA,
        },
        OriginCase {
            label: "a US client subnet takes the US record, not the subdivision sibling",
            origin: "&edns0clientsubnetip=8.8.8.0&edns0clientsubnetmask=24",
            expect: US,
        },
        OriginCase {
            label: "a single US host address takes the US record",
            origin: "&edns0clientsubnetip=208.67.222.222&edns0clientsubnetmask=32",
            expect: US,
        },
        OriginCase {
            label: "a Canadian client subnet has no sibling for its country or continent",
            origin: "&edns0clientsubnetip=149.112.121.0&edns0clientsubnetmask=24",
            expect: DEFAULT,
        },
        OriginCase {
            label: "a Brazilian client subnet has no South American sibling",
            origin: "&edns0clientsubnetip=200.160.0.0&edns0clientsubnetmask=24",
            expect: DEFAULT,
        },
        OriginCase {
            label: "a South African client subnet has no African sibling",
            origin: "&edns0clientsubnetip=196.25.0.0&edns0clientsubnetmask=24",
            expect: DEFAULT,
        },
        OriginCase {
            label: "an Australian client subnet has no Oceanian sibling",
            origin: "&edns0clientsubnetip=1.128.0.0&edns0clientsubnetmask=24",
            expect: DEFAULT,
        },
        OriginCase {
            label: "a client subnet wider than any one delegation places nowhere",
            origin: "&edns0clientsubnetip=62.0.0.0&edns0clientsubnetmask=8",
            expect: DEFAULT,
        },
        OriginCase {
            label: "a documentation address no delegation record places",
            origin: "&edns0clientsubnetip=192.0.2.0&edns0clientsubnetmask=24",
            expect: DEFAULT,
        },
        OriginCase {
            label: "an IPv6 client subnet",
            origin: "&edns0clientsubnetip=2001:db8::&edns0clientsubnetmask=32",
            expect: DEFAULT,
        },
        OriginCase {
            label: "a resolver address stands in when no client subnet is named",
            origin: "&resolverip=210.130.0.0",
            expect: JP,
        },
        OriginCase {
            label: "a client subnet is what places the query when both are named",
            origin: "&resolverip=210.130.0.0&edns0clientsubnetip=62.155.0.0",
            expect: DE,
        },
        OriginCase {
            label: "a request that says nothing about where it comes from",
            origin: "",
            expect: DEFAULT,
        },
    ];

    for case in &cases {
        let reply = ask(&router, &zone_id, GROUP, case.origin).await;
        assert_answer(case.label, &reply, &[case.expect]);
        assert_eq!(
            reply.first("RecordName"),
            Some("geo.example.com."),
            "case {:?}: the answer must echo the name asked for, got {:?}",
            case.label,
            reply.first("RecordName")
        );
        assert_eq!(
            reply.first("RecordType"),
            Some("A"),
            "case {:?}: the answer must echo the type asked for, got {:?}",
            case.label,
            reply.first("RecordType")
        );
    }

    // Two queries differing ONLY in their client subnet are answered
    // differently, which is the whole point of the group.
    let german = ask(
        &router,
        &zone_id,
        GROUP,
        "&edns0clientsubnetip=62.155.0.0&edns0clientsubnetmask=24",
    )
    .await;
    let japanese = ask(
        &router,
        &zone_id,
        GROUP,
        "&edns0clientsubnetip=210.130.0.0&edns0clientsubnetmask=24",
    )
    .await;
    assert_ne!(
        record_data(&german),
        record_data(&japanese),
        "two client subnets in different countries must not get the same answer, got {:?}",
        record_data(&german)
    );
}

// ---------------------------------------------------------------------------
// 2. No covering location and no default is no answer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_group_with_no_covering_member_and_no_default_answers_with_no_record_data() {
    // The rule's other half: the name exists, so a query no sibling's location
    // covers is answered `NOERROR` with an empty `RecordData` rather than from a
    // sibling the location rules do not pick. The rows span a query placed in a
    // country the group holds no sibling for, one placed on a continent it holds
    // none for, an unplaceable address, and a request that names no origin —
    // while the two covered countries keep answering.
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-geo-nodefault").await;
    let name = "nodefault.example.com";
    for (identifier, location, value) in [
        ("de", in_country("DE"), DE),
        ("jp", in_country("JP"), JP),
    ] {
        seed(
            &router,
            &zone_id,
            &Rrset::new(name, "A")
                .set_identifier(identifier)
                .routing(&location)
                .ttl(60)
                .values(&[value]),
        )
        .await;
    }

    for (label, origin) in [
        (
            "a US client subnet, which no sibling covers",
            "&edns0clientsubnetip=8.8.8.0&edns0clientsubnetmask=24",
        ),
        (
            "a Brazilian client subnet, whose continent no sibling covers",
            "&edns0clientsubnetip=200.160.0.0&edns0clientsubnetmask=24",
        ),
        (
            "an address no delegation record places",
            "&edns0clientsubnetip=192.0.2.0&edns0clientsubnetmask=24",
        ),
        ("a request naming no origin at all", ""),
    ] {
        let reply = ask(&router, &zone_id, name, origin).await;
        assert_answer(label, &reply, &[]);
    }

    for (label, origin, expect) in [
        (
            "the DE sibling still answers its own country",
            "&edns0clientsubnetip=62.155.0.0&edns0clientsubnetmask=24",
            DE,
        ),
        (
            "the JP sibling still answers its own country",
            "&edns0clientsubnetip=210.130.0.0&edns0clientsubnetmask=24",
            JP,
        ),
    ] {
        let reply = ask(&router, &zone_id, name, origin).await;
        assert_answer(label, &reply, &[expect]);
    }
}

// ---------------------------------------------------------------------------
// 3. Deleting the default changes what the uncovered query is answered
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_the_default_member_turns_an_uncovered_query_into_an_empty_answer() {
    // The answer is computed from the record sets the zone holds AT THE TIME of
    // the query, so removing the `*` default moves every origin it was covering
    // from its value to no record data at all, and leaves the origins the
    // country siblings cover exactly as they were.
    let router = router();
    let zone_id = create_zone_ok(&router, ZONE, "ref-geo-delete").await;
    let name = "removed.example.com";
    let default = Rrset::new(name, "A")
        .set_identifier("default")
        .routing(&in_country("*"))
        .ttl(60)
        .values(&[DEFAULT]);
    for (identifier, location, value) in [
        ("de", in_country("DE"), DE),
        ("jp", in_country("JP"), JP),
    ] {
        seed(
            &router,
            &zone_id,
            &Rrset::new(name, "A")
                .set_identifier(identifier)
                .routing(&location)
                .ttl(60)
                .values(&[value]),
        )
        .await;
    }
    seed(&router, &zone_id, &default).await;

    let uncovered = "&edns0clientsubnetip=8.8.8.0&edns0clientsubnetmask=24";
    let covered = "&edns0clientsubnetip=62.155.0.0&edns0clientsubnetmask=24";
    assert_answer(
        "while the default is in place, an uncovered query takes it",
        &ask(&router, &zone_id, name, uncovered).await,
        &[DEFAULT],
    );

    delete(&router, &zone_id, &default).await;

    assert_answer(
        "with the default gone, the same uncovered query gets no record data",
        &ask(&router, &zone_id, name, uncovered).await,
        &[],
    );
    assert_answer(
        "with the default gone, a covered query is unchanged",
        &ask(&router, &zone_id, name, covered).await,
        &[DE],
    );

    // The surviving group is exactly the two country siblings, read back through
    // the listing. No other record set in this zone carries a `SetIdentifier` or
    // a `GeoLocation`, so the whole listing names just this group.
    let listed = list(&router, &zone_id).await;
    assert_eq!(
        listed.all("SetIdentifier"),
        vec!["de", "jp"],
        "the deleted member must be gone from the group, got {:?}",
        listed.all("SetIdentifier")
    );
    assert_eq!(
        listed.all("GeoLocation/CountryCode"),
        vec!["DE", "JP"],
        "the surviving group must be the two country siblings, got {:?}",
        listed.all("GeoLocation/CountryCode")
    );
}
