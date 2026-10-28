// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetHostedZoneLimit`.
//!
//! The rule under test: for the `HostedZoneLimitType` named in the request the
//! operation reports the per-zone maximum in force for that kind of entity,
//! beside the number of entities of that kind THE NAMED ZONE holds right now,
//! counted from the service's state at the moment of the call — and the VPC
//! association quota exists on a private hosted zone only, so asking a public
//! zone for it is the modelled `HostedZoneNotPrivate` refusal.
//!
//! Every rule is asserted over a SET of inputs rather than one request: the
//! tables below span the whole limit catalog (both values `HostedZoneLimitType`
//! accepts, taken from the model), several record-set populations (the two
//! default records a new zone carries, each of four added record sets, and one
//! taken away again), several association populations (the creation VPC, three
//! more in both id forms and several regions, and one disassociated again), two
//! zones holding DIFFERENT populations, and both spellings of a zone id.
//!
//! The harness (`router`, `call`, `Reply`) and the request builders come from
//! the suites that own the zone, record-set and association operations, so
//! these tests drive the service through the same router a socket-served
//! request goes through.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::HOSTED_ZONE_LIMIT_TYPE_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::{
    HOSTED_ZONE_LIMIT_MAX_RRSETS, HOSTED_ZONE_LIMIT_MAX_VPCS_ASSOCIATED, MAX_RECORD_SETS_PER_ZONE,
    MAX_VPCS_PER_HOSTED_ZONE,
};

use crate::account_limits::reported;
use crate::hosted_zone_records::{
    call, change_batch_body, change_element, create_zone_ok, router, RecordRow, Reply,
};
use crate::refusal_message_rules::message_of;
use crate::vpc_associations::{associations, create_private_zone, get_zone, vpc_block};
use crate::vpc_disassociation::{associate_ok, disassociate};

/// Ask one zone for one of its limits.
async fn get_zone_limit(router: &MultiServiceRouter, zone_id: &str, limit_type: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/hostedzonelimit/{zone_id}/{limit_type}"),
        "",
    )
    .await
}

/// Apply one change to one zone's record sets.
async fn change_record(
    router: &MultiServiceRouter,
    zone_id: &str,
    action: &str,
    row: &RecordRow,
) -> Reply {
    call(
        router,
        "POST",
        &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
        &change_batch_body(&change_element(action, row), None),
    )
    .await
}

/// Apply one change and assert it was accepted.
async fn change_record_ok(
    router: &MultiServiceRouter,
    zone_id: &str,
    action: &str,
    row: &RecordRow,
) {
    let reply = change_record(router, zone_id, action, row).await;
    assert_eq!(
        reply.status, 200,
        "case {action} {}/{}: expected 200 from ChangeResourceRecordSets, got {} with body {}",
        row.name, row.rr_type, reply.status, reply.body
    );
}

/// How many record sets `ListResourceRecordSets` reports for one zone, which is
/// what the record-set count has to agree with.
async fn listed_record_count(router: &MultiServiceRouter, zone_id: &str) -> usize {
    let listed = call(
        router,
        "GET",
        &format!("/2013-04-01/hostedzone/{zone_id}/rrset"),
        "",
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListResourceRecordSets, got {} with body {}",
        listed.status, listed.body
    );
    listed.all("ResourceRecordSet/Name").len()
}

/// How many VPCs `GetHostedZone` reports one zone as associated with, which is
/// what the association count has to agree with.
async fn read_association_count(router: &MultiServiceRouter, zone_id: &str) -> usize {
    let reply = get_zone(router, zone_id).await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from GetHostedZone, got {} with body {}",
        reply.status, reply.body
    );
    associations(&reply).len()
}

/// The record sets a new zone is created with: the default SOA and NS records
/// at its apex, which every record-set count starts from.
const DEFAULT_RECORD_SETS: usize = 2;

/// One row of the per-zone limit catalog: a value of `HostedZoneLimitType` and
/// the maximum in force for it.
struct LimitRow {
    limit_type: &'static str,
    value: usize,
}

/// The whole per-zone limit catalog, each entry paired with the quota in force
/// for it. Checked against the model's own `HostedZoneLimitType` value list
/// below, so a limit type this table forgets fails the test rather than going
/// unasserted.
const CATALOG: &[LimitRow] = &[
    LimitRow {
        limit_type: HOSTED_ZONE_LIMIT_MAX_RRSETS,
        value: MAX_RECORD_SETS_PER_ZONE,
    },
    LimitRow {
        limit_type: HOSTED_ZONE_LIMIT_MAX_VPCS_ASSOCIATED,
        value: MAX_VPCS_PER_HOSTED_ZONE,
    },
];

#[tokio::test]
async fn the_table_below_covers_every_modelled_hosted_zone_limit_type() {
    // `HOSTED_ZONE_LIMIT_TYPE_VALUES` is the model's own `HostedZoneLimitType`
    // value list, so this is what keeps the catalog above from drifting from it.
    let mut covered: Vec<&str> = CATALOG.iter().map(|row| row.limit_type).collect();
    covered.sort_unstable();
    let mut modelled: Vec<&str> = HOSTED_ZONE_LIMIT_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        covered, modelled,
        "every HostedZoneLimitType must be covered: expected {modelled:?}, got {covered:?}"
    );
}

#[tokio::test]
async fn every_limit_type_reports_its_maximum_for_a_zone_that_holds_only_its_defaults() {
    let router = router();
    // A PRIVATE zone, so both limits apply to it, and nothing has been added to
    // it: the record-set count is its two default records and the association
    // count is the one VPC it was created with.
    let zone = create_private_zone(
        &router,
        "fresh.example.com",
        "zone-limit-fresh",
        "vpc-1a2b3c4d",
    )
    .await;

    for row in CATALOG {
        let reply = get_zone_limit(&router, &zone, row.limit_type).await;
        let (reported_type, value, count) = reported(&reply, row.limit_type);
        assert_eq!(
            reported_type, row.limit_type,
            "case {:?}: Limit/Type must echo the limit requested, got {reported_type:?}; body {}",
            row.limit_type, reply.body
        );
        assert_eq!(
            value,
            row.value.to_string(),
            "case {:?}: expected Limit/Value {}, got {value:?}; body {}",
            row.limit_type,
            row.value,
            reply.body
        );
        let expected = if row.limit_type == HOSTED_ZONE_LIMIT_MAX_RRSETS {
            DEFAULT_RECORD_SETS
        } else {
            1
        };
        assert_eq!(
            count,
            expected.to_string(),
            "case {:?}: expected Count {expected}, got {count:?}; body {}",
            row.limit_type,
            reply.body
        );
    }
}

/// The apex of every zone this suite adds record sets to.
///
/// A record set is only permitted inside the zone whose name it falls under, so
/// the rows below name the zone that carries them and every zone they are
/// created in is this one.
const RECORD_ZONE: &str = "limits.example.com";

/// The record sets added one at a time, each row differing in name, record
/// type, TTL and how many values it carries.
const RECORD_ROWS: &[RecordRow] = &[
    RecordRow {
        name: "alpha.limits.example.com",
        rr_type: "A",
        ttl: 60,
        values: &["192.0.2.10"],
    },
    RecordRow {
        name: "beta.limits.example.com",
        rr_type: "AAAA",
        ttl: 300,
        values: &["2001:db8::10", "2001:db8::11"],
    },
    RecordRow {
        name: "mail.limits.example.com",
        rr_type: "MX",
        ttl: 3600,
        values: &["10 inbound.limits.example.com."],
    },
    RecordRow {
        name: "notes.limits.example.com",
        rr_type: "TXT",
        ttl: 120,
        values: &["\"one\"", "\"two\"", "\"three\""],
    },
];

#[tokio::test]
async fn the_record_set_count_follows_the_record_sets_the_zone_holds() {
    let router = router();
    let zone = create_zone_ok(&router, RECORD_ZONE, "zone-limit-records").await;

    // Several populations, not one: the count is asserted on the zone's two
    // default records, after each of four creates, and after a delete takes one
    // away again.
    for (index, row) in RECORD_ROWS.iter().enumerate() {
        let expected = DEFAULT_RECORD_SETS + index;
        let case = format!("{expected} record set(s) held");
        let reply = get_zone_limit(&router, &zone, HOSTED_ZONE_LIMIT_MAX_RRSETS).await;
        let (_, value, count) = reported(&reply, &case);
        assert_eq!(
            count,
            expected.to_string(),
            "case {case:?}: expected Count {expected}, got {count:?}; body {}",
            reply.body
        );
        // The reported maximum is the zone's quota, so it does not move as the
        // population does.
        assert_eq!(
            value,
            MAX_RECORD_SETS_PER_ZONE.to_string(),
            "case {case:?}: expected Limit/Value {MAX_RECORD_SETS_PER_ZONE}, got {value:?}; \
             body {}",
            reply.body
        );
        // And the count is the same population `ListResourceRecordSets` reports.
        let listed = listed_record_count(&router, &zone).await;
        assert_eq!(
            count,
            listed.to_string(),
            "case {case:?}: the count must agree with ListResourceRecordSets ({listed} record \
             sets), got {count:?}"
        );

        change_record_ok(&router, &zone, "CREATE", row).await;
    }

    // A record set the zone no longer holds is no longer counted.
    let removed = &RECORD_ROWS[RECORD_ROWS.len() - 1];
    change_record_ok(&router, &zone, "DELETE", removed).await;
    let expected = DEFAULT_RECORD_SETS + RECORD_ROWS.len() - 1;
    let reply = get_zone_limit(&router, &zone, HOSTED_ZONE_LIMIT_MAX_RRSETS).await;
    let (_, _, count) = reported(&reply, "after a delete");
    assert_eq!(
        count,
        expected.to_string(),
        "case \"after a delete\": expected Count {expected}, got {count:?}; body {}",
        reply.body
    );
}

/// One VPC to associate: its id (in either id form) and the region it was
/// created in, each a value of the modelled `VPCRegion` enum
/// (smithy_query(query="shape",shape="VPCRegion")).
struct VpcRow {
    vpc_id: &'static str,
    region: &'static str,
}

/// The VPCs associated one at a time, each row differing in id form and region.
const VPC_ROWS: &[VpcRow] = &[
    VpcRow {
        vpc_id: "vpc-5e6f7a8b",
        region: "eu-west-1",
    },
    VpcRow {
        vpc_id: "vpc-0a1b2c3d4e5f60718",
        region: "ap-southeast-2",
    },
    VpcRow {
        vpc_id: "vpc-deadbeef",
        region: "us-gov-west-1",
    },
];

#[tokio::test]
async fn the_association_count_follows_the_vpcs_the_private_zone_holds() {
    let router = router();
    let creation_vpc = "vpc-9a8b7c6d";
    let zone = create_private_zone(
        &router,
        "private.limits.example.com",
        "zone-limit-vpcs",
        creation_vpc,
    )
    .await;

    // Several populations, not one: the count is asserted on the VPC the zone
    // was created with, after each of three associations, and after a
    // disassociation takes one away again.
    for (index, row) in VPC_ROWS.iter().enumerate() {
        let expected = 1 + index;
        let case = format!("{expected} association(s) held");
        let reply = get_zone_limit(&router, &zone, HOSTED_ZONE_LIMIT_MAX_VPCS_ASSOCIATED).await;
        let (_, value, count) = reported(&reply, &case);
        assert_eq!(
            count,
            expected.to_string(),
            "case {case:?}: expected Count {expected}, got {count:?}; body {}",
            reply.body
        );
        assert_eq!(
            value,
            MAX_VPCS_PER_HOSTED_ZONE.to_string(),
            "case {case:?}: expected Limit/Value {MAX_VPCS_PER_HOSTED_ZONE}, got {value:?}; \
             body {}",
            reply.body
        );
        // And the count is the same association set `GetHostedZone` reports.
        let read = read_association_count(&router, &zone).await;
        assert_eq!(
            count,
            read.to_string(),
            "case {case:?}: the count must agree with GetHostedZone ({read} VPCs), got {count:?}"
        );

        associate_ok(&router, &zone, row.vpc_id, row.region).await;
    }

    // A VPC the zone is no longer associated with is no longer counted.
    let removed = &VPC_ROWS[VPC_ROWS.len() - 1];
    let reply = disassociate(
        &router,
        &zone,
        &vpc_block(Some(removed.vpc_id), Some(removed.region)),
        None,
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from DisassociateVPCFromHostedZone, got {} with body {}",
        reply.status, reply.body
    );
    let expected = VPC_ROWS.len();
    let reply = get_zone_limit(&router, &zone, HOSTED_ZONE_LIMIT_MAX_VPCS_ASSOCIATED).await;
    let (_, _, count) = reported(&reply, "after a disassociation");
    assert_eq!(
        count,
        expected.to_string(),
        "case \"after a disassociation\": expected Count {expected}, got {count:?}; body {}",
        reply.body
    );
}

#[tokio::test]
async fn each_zone_reports_its_own_population_for_the_kind_that_was_asked_about() {
    // Two private zones holding DIFFERENT populations, so a count read off the
    // account rather than off the named zone, or off the wrong entity kind,
    // fails here.
    let router = router();
    let busy = create_private_zone(
        &router,
        RECORD_ZONE,
        "zone-limit-busy",
        "vpc-11112222333344445",
    )
    .await;
    let quiet = create_private_zone(
        &router,
        "quiet.example.com",
        "zone-limit-quiet",
        "vpc-55556666777788889",
    )
    .await;

    // The busy zone takes three record sets and two more VPCs; the quiet one
    // keeps what it was created with.
    for row in &RECORD_ROWS[..3] {
        change_record_ok(&router, &busy, "CREATE", row).await;
    }
    for row in &VPC_ROWS[..2] {
        associate_ok(&router, &busy, row.vpc_id, row.region).await;
    }

    let expectations = [
        (busy.as_str(), HOSTED_ZONE_LIMIT_MAX_RRSETS, DEFAULT_RECORD_SETS + 3),
        (busy.as_str(), HOSTED_ZONE_LIMIT_MAX_VPCS_ASSOCIATED, 3),
        (quiet.as_str(), HOSTED_ZONE_LIMIT_MAX_RRSETS, DEFAULT_RECORD_SETS),
        (quiet.as_str(), HOSTED_ZONE_LIMIT_MAX_VPCS_ASSOCIATED, 1),
    ];
    for (zone, limit_type, expected) in expectations {
        let case = format!("{limit_type} on {zone}");
        let reply = get_zone_limit(&router, zone, limit_type).await;
        let (_, _, count) = reported(&reply, &case);
        assert_eq!(
            count,
            expected.to_string(),
            "case {case:?}: expected Count {expected}, got {count:?}; body {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_zone_id_is_read_in_either_spelling() {
    // The bare id and the `/hostedzone/`-prefixed id a response hands back name
    // the same zone, so both report the same thing.
    let router = router();
    let zone = create_zone_ok(&router, RECORD_ZONE, "zone-limit-spelling").await;
    change_record_ok(&router, &zone, "CREATE", &RECORD_ROWS[0]).await;

    let bare = get_zone_limit(&router, &zone, HOSTED_ZONE_LIMIT_MAX_RRSETS).await;
    let from_bare = reported(&bare, "the bare id");
    let prefixed = get_zone_limit(
        &router,
        &format!("%2Fhostedzone%2F{zone}"),
        HOSTED_ZONE_LIMIT_MAX_RRSETS,
    )
    .await;
    let from_prefixed = reported(&prefixed, "the /hostedzone/-prefixed id");
    assert_eq!(
        from_prefixed, from_bare,
        "case \"the /hostedzone/-prefixed id\": expected what the bare id reported \
         ({from_bare:?}), got {from_prefixed:?}; body {}",
        prefixed.body
    );
    assert_eq!(
        from_bare.2,
        (DEFAULT_RECORD_SETS + 1).to_string(),
        "the zone holds its defaults plus one record set, got Count {:?}; body {}",
        from_bare.2,
        bare.body
    );
}

#[tokio::test]
async fn the_vpc_limit_belongs_to_a_private_zone_only() {
    // The rule: a VPC association is a private-zone thing, so a PUBLIC zone has
    // no VPC quota to report and the request is refused — while the record-set
    // quota, which every zone has, is reported for the very same zone. The rows
    // differ in the zone's name shape and in how the request spells its id.
    let router = router();
    let rows = [
        ("public-one.example.com", "zone-limit-public-one", false),
        ("public-two.example.net.", "zone-limit-public-two", false),
        (
            "deep.sub.public-three.example.org",
            "zone-limit-public-three",
            true,
        ),
    ];

    for (name, caller_reference, prefixed) in rows {
        let zone = create_zone_ok(&router, name, caller_reference).await;
        let requested = if prefixed {
            format!("%2Fhostedzone%2F{zone}")
        } else {
            zone.clone()
        };
        let case = format!("{name} asked for its VPC limit");

        let refused = get_zone_limit(&router, &requested, HOSTED_ZONE_LIMIT_MAX_VPCS_ASSOCIATED)
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "HostedZoneNotPrivate",
            "case {case:?}: expected HostedZoneNotPrivate, got body {}",
            refused.body
        );
        let got = message_of(&refused, &case);
        let expected = format!(
            "The specified hosted zone is a public hosted zone, not a private hosted zone: \
             {zone}"
        );
        assert_eq!(
            got, expected,
            "case {case:?}: expected message {expected:?}, got {got:?}"
        );

        // The SAME zone still reports the quota a public zone does have.
        let allowed = get_zone_limit(&router, &requested, HOSTED_ZONE_LIMIT_MAX_RRSETS).await;
        let (_, value, count) = reported(&allowed, &case);
        assert_eq!(
            value,
            MAX_RECORD_SETS_PER_ZONE.to_string(),
            "case {case:?}: expected Limit/Value {MAX_RECORD_SETS_PER_ZONE}, got {value:?}; \
             body {}",
            allowed.body
        );
        assert_eq!(
            count,
            DEFAULT_RECORD_SETS.to_string(),
            "case {case:?}: expected Count {DEFAULT_RECORD_SETS}, got {count:?}; body {}",
            allowed.body
        );
    }

    // A PRIVATE zone created in the same store answers the VPC limit, so the
    // refusal above is about the zone being public and nothing else.
    let private = create_private_zone(
        &router,
        "private-control.example.com",
        "zone-limit-private-control",
        "vpc-1234abcd",
    )
    .await;
    let reply = get_zone_limit(&router, &private, HOSTED_ZONE_LIMIT_MAX_VPCS_ASSOCIATED).await;
    let (_, _, count) = reported(&reply, "a private zone asked for its VPC limit");
    assert_eq!(
        count, "1",
        "a private zone created with one VPC must report Count 1, got {count:?}; body {}",
        reply.body
    );
}

#[tokio::test]
async fn an_id_that_names_no_zone_is_refused_for_every_limit_type() {
    let router = router();
    // One zone that DOES exist, so the store is not empty and the refusal is
    // about the id that was asked about.
    let existing = create_zone_ok(&router, "exists.example.com", "zone-limit-exists").await;

    // The ids differ in form: a well-formed `Z…` id nothing minted, the
    // prefixed spelling of one, a deleted zone's id, and a short id.
    let deleted = create_zone_ok(&router, "deleted.example.com", "zone-limit-deleted").await;
    let removed = call(
        &router,
        "DELETE",
        &format!("/2013-04-01/hostedzone/{deleted}"),
        "",
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "expected 200 from DeleteHostedZone, got {} with body {}",
        removed.status, removed.body
    );

    let rows = [
        ("ZNOSUCHZONE123".to_string(), "ZNOSUCHZONE123".to_string()),
        (
            format!("%2Fhostedzone%2F{}", "ZABSENT9999999"),
            "ZABSENT9999999".to_string(),
        ),
        (deleted.clone(), deleted.clone()),
        ("Z1".to_string(), "Z1".to_string()),
    ];

    for (requested, bare) in rows {
        for row in CATALOG {
            let case = format!("{} of {requested}", row.limit_type);
            let reply = get_zone_limit(&router, &requested, row.limit_type).await;
            assert_eq!(
                reply.status, 404,
                "case {case:?}: expected HTTP 404, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "NoSuchHostedZone",
                "case {case:?}: expected NoSuchHostedZone, got body {}",
                reply.body
            );
            let got = message_of(&reply, &case);
            let expected = format!("No hosted zone found with ID: {bare}");
            assert_eq!(
                got, expected,
                "case {case:?}: expected message {expected:?}, got {got:?}"
            );
            // The refusal is about the id asked about, not the zone that does
            // exist.
            assert!(
                !got.contains(&existing),
                "case {case:?}: the message must not name the zone {existing}, got {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn reading_a_limit_leaves_the_zone_exactly_as_it_was() {
    // The rule is a report, so repeating it answers the same thing and consumes
    // nothing: the record sets and the associations after any number of reads
    // are still the ones the zone held.
    let router = router();
    let zone = create_private_zone(
        &router,
        RECORD_ZONE,
        "zone-limit-idempotent",
        "vpc-abcd1234",
    )
    .await;
    change_record_ok(&router, &zone, "CREATE", &RECORD_ROWS[0]).await;
    associate_ok(&router, &zone, VPC_ROWS[0].vpc_id, VPC_ROWS[0].region).await;

    for row in CATALOG {
        let first = get_zone_limit(&router, &zone, row.limit_type).await;
        let first_reported = reported(&first, row.limit_type);
        for attempt in 1..=3 {
            let again = get_zone_limit(&router, &zone, row.limit_type).await;
            let again_reported = reported(&again, row.limit_type);
            assert_eq!(
                again_reported, first_reported,
                "case {:?}: read {attempt} must report what the first read did ({:?}), got {:?}; \
                 body {}",
                row.limit_type, first_reported, again_reported, again.body
            );
        }
    }

    let records = listed_record_count(&router, &zone).await;
    assert_eq!(
        records,
        DEFAULT_RECORD_SETS + 1,
        "reading limits must not create or destroy a record set; ListResourceRecordSets reports \
         {records}"
    );
    let vpcs = read_association_count(&router, &zone).await;
    assert_eq!(
        vpcs, 2,
        "reading limits must not add or remove an association; GetHostedZone reports {vpcs}"
    );
}

#[tokio::test]
async fn a_type_outside_the_per_zone_limit_catalog_is_refused() {
    // `Type` is a modelled enum, so a value the catalog does not hold is a bad
    // request — never a 200 carrying an invented limit, and never the 501 of an
    // unimplemented operation.
    let router = router();
    let zone = create_zone_ok(&router, "enum.example.com", "zone-limit-enum").await;
    for case in [
        "MAX_RRSETS",
        "max_rrsets_by_zone",
        "MAX_RRSETS_BY_ZONE_EXTRA",
        "MAX_HOSTED_ZONES_BY_OWNER",
        "MAX_VPCS_ASSOCIATED",
    ] {
        let reply = get_zone_limit(&router, &zone, case).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for a type outside HostedZoneLimitType, got {} \
             with body {}",
            reply.status, reply.body
        );
        // The boundary's own refusal of a modelled enum constraint, not one of
        // this operation's zone errors: the request names no limit to report.
        assert_eq!(
            reply.error_code(),
            "InvalidInput",
            "case {case:?}: expected InvalidInput, got body {}",
            reply.body
        );
    }
}
