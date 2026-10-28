// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetReusableDelegationSetLimit`.
//!
//! The rule under test: for the `ReusableDelegationSetLimitType` named in the
//! request the operation reports the per-set maximum in force for that kind of
//! entity, beside the number of entities of that kind THE NAMED REUSABLE
//! DELEGATION SET holds right now, counted from the service's state at the
//! moment of the call — and an id that names no reusable delegation set is the
//! modelled `NoSuchDelegationSet` refusal.
//!
//! Every rule is asserted over a SET of inputs rather than one request: the
//! tables below span the whole limit catalog (every value
//! `ReusableDelegationSetLimitType` accepts, taken from the model), several zone
//! populations (a set nothing was created in, a set minted from an existing
//! zone, each of four zones created in a set, and one deleted again), two sets
//! holding DIFFERENT populations beside zones in no set at all, both spellings
//! of a set id, and several forms of an id that names nothing.
//!
//! The harness (`router`, `call`, `Reply`) and the request builders come from
//! the suites that own the zone and delegation-set operations, so these tests
//! drive the service through the same router a socket-served request goes
//! through.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::REUSABLE_DELEGATION_SET_LIMIT_TYPE_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::{
    MAX_HOSTED_ZONES_PER_DELEGATION_SET, REUSABLE_DELEGATION_SET_LIMIT_MAX_ZONES,
};

use crate::account_limits::reported;
use crate::delegation_set_deletion::delete_set;
use crate::hosted_zone_count::delete_zone;
use crate::hosted_zone_records::{call, create_zone_ok, router, Reply};
use crate::refusal_message_rules::message_of;
use crate::reusable_delegation_sets::{
    bare_set_id, create_set, delegation_set_element, get_set, set_count, set_of,
};
use crate::vpc_associations::{bare_zone_id, create_zone};

/// Ask one reusable delegation set for one of its limits, with the
/// `{DelegationSetId}` label written exactly as `set_id` spells it.
async fn get_set_limit(router: &MultiServiceRouter, set_id: &str, limit_type: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/reusabledelegationsetlimit/{set_id}/{limit_type}"),
        "",
    )
    .await
}

/// Create a reusable delegation set and return `(the prefixed Id, the bare id)`.
async fn set_ids(
    router: &MultiServiceRouter,
    caller_reference: &str,
    hosted_zone_id: Option<&str>,
) -> (String, String) {
    let created = create_set(router, caller_reference, hosted_zone_id).await;
    assert_eq!(
        created.status, 201,
        "CreateReusableDelegationSet({caller_reference}) must answer 201, got {} with body {}",
        created.status, created.body
    );
    let id = set_of(&created, caller_reference).id;
    let bare = bare_set_id(&id).to_string();
    (id, bare)
}

/// Create a public hosted zone IN the reusable delegation set `set_id` and
/// return its bare (`Z…`) id.
async fn create_zone_in_set(
    router: &MultiServiceRouter,
    name: &str,
    caller_reference: &str,
    set_id: &str,
) -> String {
    let reply = create_zone(
        router,
        name,
        caller_reference,
        &delegation_set_element(set_id),
    )
    .await;
    assert_eq!(
        reply.status, 201,
        "creating {name} in {set_id}: expected 201 from CreateHostedZone, got {} with body {}",
        reply.status, reply.body
    );
    bare_zone_id(&reply)
}

/// How many hosted zones `ListHostedZones` filtered by the set reports, which
/// is what the count the limit reports has to agree with.
async fn listed_zones_in_set(router: &MultiServiceRouter, bare_set: &str) -> usize {
    let reply = call(
        router,
        "GET",
        &format!("/2013-04-01/hostedzone?delegationsetid={bare_set}"),
        "",
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from ListHostedZones?delegationsetid={bare_set}, got {} with body {}",
        reply.status, reply.body
    );
    reply.all("HostedZone/Id").len()
}

/// One row of the per-set limit catalog: a value of
/// `ReusableDelegationSetLimitType` and the maximum in force for it.
struct LimitRow {
    limit_type: &'static str,
    value: usize,
}

/// The whole per-set limit catalog, each entry paired with the quota in force
/// for it. Checked against the model's own `ReusableDelegationSetLimitType`
/// value list below, so a limit type this table forgets fails the test rather
/// than going unasserted.
const CATALOG: &[LimitRow] = &[LimitRow {
    limit_type: REUSABLE_DELEGATION_SET_LIMIT_MAX_ZONES,
    value: MAX_HOSTED_ZONES_PER_DELEGATION_SET,
}];

#[tokio::test]
async fn the_table_below_covers_every_modelled_reusable_delegation_set_limit_type() {
    // `REUSABLE_DELEGATION_SET_LIMIT_TYPE_VALUES` is the model's own
    // `ReusableDelegationSetLimitType` value list, so this is what keeps the
    // catalog above from drifting from it.
    let mut covered: Vec<&str> = CATALOG.iter().map(|row| row.limit_type).collect();
    covered.sort_unstable();
    let mut modelled: Vec<&str> = REUSABLE_DELEGATION_SET_LIMIT_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        covered, modelled,
        "every ReusableDelegationSetLimitType must be covered: expected {modelled:?}, got \
         {covered:?}"
    );
}

#[tokio::test]
async fn every_limit_type_reports_its_maximum_for_a_set_nothing_was_created_in() {
    let router = router();
    let (_, bare) = set_ids(&router, "set-limit-fresh", None).await;

    for row in CATALOG {
        let reply = get_set_limit(&router, &bare, row.limit_type).await;
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
        assert_eq!(
            count, "0",
            "case {:?}: a set nothing was created in must report Count 0, got {count:?}; body {}",
            row.limit_type, reply.body
        );
    }
}

/// One zone created in the set: its DNS name, and whether the create names the
/// set by its `/delegationset/`-prefixed id rather than its bare one.
struct ZoneRow {
    name: &'static str,
    caller_reference: &'static str,
    prefixed: bool,
}

/// The zones created one at a time, each row differing in name shape and in the
/// form the create names the set in.
const ZONE_ROWS: &[ZoneRow] = &[
    ZoneRow {
        name: "alpha.example.com",
        caller_reference: "set-limit-zone-alpha",
        prefixed: false,
    },
    ZoneRow {
        name: "beta.example.org.",
        caller_reference: "set-limit-zone-beta",
        prefixed: true,
    },
    ZoneRow {
        name: "deep.sub.gamma.example.net",
        caller_reference: "set-limit-zone-gamma",
        prefixed: false,
    },
    ZoneRow {
        name: "delta.example.io",
        caller_reference: "set-limit-zone-delta",
        prefixed: true,
    },
];

#[tokio::test]
async fn the_zone_count_follows_the_zones_created_in_the_set() {
    let router = router();
    let (prefixed_id, bare) = set_ids(&router, "set-limit-population", None).await;

    // Several populations, not one: the count is asserted on the empty set,
    // after each of four creates, and after a delete takes one away again.
    let mut created: Vec<String> = Vec::new();
    for (index, row) in ZONE_ROWS.iter().enumerate() {
        let case = format!("{index} zone(s) created in the set");
        let reply = get_set_limit(&router, &bare, REUSABLE_DELEGATION_SET_LIMIT_MAX_ZONES).await;
        let (_, value, count) = reported(&reply, &case);
        assert_eq!(
            count,
            index.to_string(),
            "case {case:?}: expected Count {index}, got {count:?}; body {}",
            reply.body
        );
        // The reported maximum is the set's quota, so it does not move as the
        // population does.
        assert_eq!(
            value,
            MAX_HOSTED_ZONES_PER_DELEGATION_SET.to_string(),
            "case {case:?}: expected Limit/Value {MAX_HOSTED_ZONES_PER_DELEGATION_SET}, got \
             {value:?}; body {}",
            reply.body
        );
        // And the count is the same population `ListHostedZones` filtered by
        // the set reports.
        let listed = listed_zones_in_set(&router, &bare).await;
        assert_eq!(
            count,
            listed.to_string(),
            "case {case:?}: the count must agree with ListHostedZones?delegationsetid ({listed} \
             zones), got {count:?}"
        );

        let named = if row.prefixed {
            prefixed_id.clone()
        } else {
            bare.clone()
        };
        created.push(create_zone_in_set(&router, row.name, row.caller_reference, &named).await);
    }

    // A zone the set no longer carries is no longer counted.
    let removed = created.pop().expect("four zones were created in the set");
    delete_zone(&router, &removed, "after a delete").await;
    let expected = ZONE_ROWS.len() - 1;
    let reply = get_set_limit(&router, &bare, REUSABLE_DELEGATION_SET_LIMIT_MAX_ZONES).await;
    let (_, _, count) = reported(&reply, "after a delete");
    assert_eq!(
        count,
        expected.to_string(),
        "case \"after a delete\": expected Count {expected}, got {count:?}; body {}",
        reply.body
    );
}

#[tokio::test]
async fn a_set_minted_from_an_existing_zone_counts_that_zone() {
    // A set whose name servers come from an existing public zone is a set that
    // zone was created in from then on, so its population starts at one rather
    // than at zero.
    let router = router();
    let zone = create_zone_ok(&router, "marked.example.com", "set-limit-marked").await;
    let (_, bare) = set_ids(&router, "set-limit-from-zone", Some(&zone)).await;

    let reply = get_set_limit(&router, &bare, REUSABLE_DELEGATION_SET_LIMIT_MAX_ZONES).await;
    let (_, _, count) = reported(&reply, "a set minted from an existing zone");
    assert_eq!(
        count, "1",
        "case \"a set minted from an existing zone\": expected Count 1, got {count:?}; body {}",
        reply.body
    );
    let listed = listed_zones_in_set(&router, &bare).await;
    assert_eq!(
        listed, 1,
        "the listing filtered by the set must report the zone it was minted from, got {listed} \
         zones"
    );
}

#[tokio::test]
async fn each_set_reports_its_own_zone_population() {
    // Two sets holding DIFFERENT populations, beside zones in no set at all, so
    // a count read off the account rather than off the named set fails here.
    let router = router();
    let (_, busy) = set_ids(&router, "set-limit-busy", None).await;
    let (_, quiet) = set_ids(&router, "set-limit-quiet", None).await;

    for row in &ZONE_ROWS[..3] {
        create_zone_in_set(&router, row.name, row.caller_reference, &busy).await;
    }
    create_zone_in_set(
        &router,
        ZONE_ROWS[3].name,
        ZONE_ROWS[3].caller_reference,
        &quiet,
    )
    .await;
    // Two zones created in NO delegation set, which neither set counts.
    create_zone_ok(&router, "loose-one.example.com", "set-limit-loose-one").await;
    create_zone_ok(&router, "loose-two.example.com", "set-limit-loose-two").await;

    for (set, expected) in [(busy.as_str(), 3), (quiet.as_str(), 1)] {
        let case = format!("{REUSABLE_DELEGATION_SET_LIMIT_MAX_ZONES} on {set}");
        let reply = get_set_limit(&router, set, REUSABLE_DELEGATION_SET_LIMIT_MAX_ZONES).await;
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
async fn a_delegation_set_id_is_read_in_either_spelling() {
    // The bare id and the `/delegationset/`-prefixed id a response hands back
    // name the same set, so both report the same thing.
    let router = router();
    let (_, bare) = set_ids(&router, "set-limit-spelling", None).await;
    create_zone_in_set(
        &router,
        ZONE_ROWS[0].name,
        ZONE_ROWS[0].caller_reference,
        &bare,
    )
    .await;

    let from_bare_reply = get_set_limit(&router, &bare, REUSABLE_DELEGATION_SET_LIMIT_MAX_ZONES)
        .await;
    let from_bare = reported(&from_bare_reply, "the bare id");
    let prefixed_reply = get_set_limit(
        &router,
        &format!("%2Fdelegationset%2F{bare}"),
        REUSABLE_DELEGATION_SET_LIMIT_MAX_ZONES,
    )
    .await;
    let from_prefixed = reported(&prefixed_reply, "the /delegationset/-prefixed id");
    assert_eq!(
        from_prefixed, from_bare,
        "case \"the /delegationset/-prefixed id\": expected what the bare id reported \
         ({from_bare:?}), got {from_prefixed:?}; body {}",
        prefixed_reply.body
    );
    assert_eq!(
        from_bare.2, "1",
        "the set carries one zone, got Count {:?}; body {}",
        from_bare.2, from_bare_reply.body
    );
}

#[tokio::test]
async fn an_id_that_names_no_delegation_set_is_refused_for_every_limit_type() {
    let router = router();
    // One set that DOES exist, so the store is not empty and the refusal is
    // about the id that was asked about.
    let (_, existing) = set_ids(&router, "set-limit-exists", None).await;

    // A set that existed and was deleted names nothing afterwards either.
    let (_, deleted) = set_ids(&router, "set-limit-deleted", None).await;
    let removed = delete_set(&router, &deleted).await;
    assert_eq!(
        removed.status, 200,
        "expected 200 from DeleteReusableDelegationSet, got {} with body {}",
        removed.status, removed.body
    );

    // The rows differ along the SPELLING of the id (bare, `/delegationset/`-
    // prefixed, lowercase), its length, and the kind of resource it looks like
    // (a delegation set, a hosted zone).
    let rows = [
        ("N0000000000000000".to_string(), "N0000000000000000".to_string()),
        (
            "%2Fdelegationset%2FN0000000000000000".to_string(),
            "N0000000000000000".to_string(),
        ),
        (deleted.clone(), deleted.clone()),
        ("n1pa6795sample".to_string(), "n1pa6795sample".to_string()),
        ("Z0000000000000000".to_string(), "Z0000000000000000".to_string()),
        ("N".to_string(), "N".to_string()),
    ];

    for (requested, bare) in rows {
        for row in CATALOG {
            let case = format!("{} of {requested}", row.limit_type);
            let reply = get_set_limit(&router, &requested, row.limit_type).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "NoSuchDelegationSet",
                "case {case:?}: expected NoSuchDelegationSet, got body {}",
                reply.body
            );
            // This service reports a delegation set id that resolves to nothing
            // as that id alone, in its bare form, however the request spelled it.
            let got = message_of(&reply, &case);
            assert_eq!(
                got, bare,
                "case {case:?}: expected message {bare:?}, got {got:?}"
            );
            // The refusal is about the id asked about, not the set that does
            // exist.
            assert!(
                !got.contains(&existing),
                "case {case:?}: the message must not name the set {existing}, got {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn reading_a_limit_leaves_the_set_exactly_as_it_was() {
    // The rule is a report, so repeating it answers the same thing and consumes
    // nothing: the set and the account's set count after any number of reads
    // are still the ones the account held.
    let router = router();
    let (_, bare) = set_ids(&router, "set-limit-idempotent", None).await;
    create_zone_in_set(
        &router,
        ZONE_ROWS[0].name,
        ZONE_ROWS[0].caller_reference,
        &bare,
    )
    .await;
    let before = set_of(&get_set(&router, &bare).await, "before the reads");
    let count_before = set_count(&router).await;

    for row in CATALOG {
        let first = get_set_limit(&router, &bare, row.limit_type).await;
        let first_reported = reported(&first, row.limit_type);
        for attempt in 1..=3 {
            let again = get_set_limit(&router, &bare, row.limit_type).await;
            let again_reported = reported(&again, row.limit_type);
            assert_eq!(
                again_reported, first_reported,
                "case {:?}: read {attempt} must report what the first read did ({:?}), got {:?}; \
                 body {}",
                row.limit_type, first_reported, again_reported, again.body
            );
        }
    }

    let after = set_of(&get_set(&router, &bare).await, "after the reads");
    assert_eq!(
        after, before,
        "reading limits must leave the set exactly as it was: expected {before:?}, got {after:?}"
    );
    let count_after = set_count(&router).await;
    assert_eq!(
        count_after, count_before,
        "reading limits must not create or destroy a delegation set: expected \
         {count_before:?}, got {count_after:?}"
    );
}

#[tokio::test]
async fn a_type_outside_the_per_set_limit_catalog_is_refused() {
    // `Type` is a modelled enum, so a value the catalog does not hold is a bad
    // request — never a 200 carrying an invented limit, and never the 501 of an
    // unimplemented operation.
    let router = router();
    let (_, bare) = set_ids(&router, "set-limit-enum", None).await;
    for case in [
        "MAX_ZONES",
        "max_zones_by_reusable_delegation_set",
        "MAX_ZONES_BY_REUSABLE_DELEGATION_SET_EXTRA",
        "MAX_HOSTED_ZONES_BY_OWNER",
        "MAX_RRSETS_BY_ZONE",
    ] {
        let reply = get_set_limit(&router, &bare, case).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for a type outside \
             ReusableDelegationSetLimitType, got {} with body {}",
            reply.status, reply.body
        );
        // The boundary's own refusal of a modelled enum constraint, not this
        // operation's delegation-set error: the request names no limit to report.
        assert_eq!(
            reply.error_code(),
            "InvalidInput",
            "case {case:?}: expected InvalidInput, got body {}",
            reply.body
        );
    }
}
