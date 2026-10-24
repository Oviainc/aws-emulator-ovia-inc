// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetAccountLimit`.
//!
//! The rule under test: for the `AccountLimitType` named in the request the
//! operation reports the maximum currently in force for that kind of entity,
//! beside the number of entities of that kind the account holds right now,
//! counted from the service's state at the moment of the call. So the tables
//! below span the whole limit catalog (every value `AccountLimitType` accepts,
//! taken from the model) and, for the one entity kind this emulator can bring
//! into being today, the count is driven over several zone populations rather
//! than one: zero zones, one, several, and several minus a deleted one.
//!
//! Every test drives the service through the real router, using the same
//! harness the hosted-zone suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::ACCOUNT_LIMIT_TYPE_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::{
    ACCOUNT_LIMIT_MAX_HEALTH_CHECKS, ACCOUNT_LIMIT_MAX_HOSTED_ZONES,
    ACCOUNT_LIMIT_MAX_REUSABLE_DELEGATION_SETS, ACCOUNT_LIMIT_MAX_TRAFFIC_POLICIES,
    ACCOUNT_LIMIT_MAX_TRAFFIC_POLICY_INSTANCES, MAX_HEALTH_CHECKS, MAX_HOSTED_ZONES,
    MAX_REUSABLE_DELEGATION_SETS, MAX_TRAFFIC_POLICIES, MAX_TRAFFIC_POLICY_INSTANCES,
};

use crate::hosted_zone_records::{call, router, Reply};

/// Ask for one account limit.
async fn get_account_limit(router: &MultiServiceRouter, limit_type: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/2013-04-01/accountlimit/{limit_type}"),
        "",
    )
    .await
}

/// `(Limit/Type, Limit/Value, Count)` as a successful reply reports them.
fn reported(reply: &Reply, case: &str) -> (String, String, String) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetAccountLimit, got {} with body {}",
        reply.status, reply.body
    );
    let pick = |path: &str| {
        reply
            .first(path)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: the reply must carry {path}; body {}",
                    reply.body
                )
            })
            .to_string()
    };
    (pick("Limit/Type"), pick("Limit/Value"), pick("Count"))
}

/// Create a public hosted zone and return the reply.
async fn create_zone(router: &MultiServiceRouter, name: &str, caller_reference: &str) -> Reply {
    call(
        router,
        "POST",
        "/2013-04-01/hostedzone",
        &format!(
            "<CreateHostedZoneRequest xmlns=\"https://route53.amazonaws.com/doc/2013-04-01/\">\
             <Name>{name}</Name><CallerReference>{caller_reference}</CallerReference>\
             </CreateHostedZoneRequest>"
        ),
    )
    .await
}

/// The bare (`Z…`) id of a reply carrying a `HostedZone`.
fn bare_zone_id(reply: &Reply) -> String {
    reply
        .first("HostedZone/Id")
        .unwrap_or_else(|| panic!("the reply must carry HostedZone/Id; body {}", reply.body))
        .trim_start_matches("/hostedzone/")
        .to_string()
}

/// How many zones `ListHostedZones` reports, which is what the hosted-zone
/// count has to agree with.
async fn listed_zone_count(router: &MultiServiceRouter) -> usize {
    let listed = call(router, "GET", "/2013-04-01/hostedzone", "").await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListHostedZones, got {} with body {}",
        listed.status, listed.body
    );
    listed.all("HostedZone/Id").len()
}

/// One row of the limit catalog: a value of `AccountLimitType` and the maximum
/// in force for it.
struct LimitRow {
    limit_type: &'static str,
    value: usize,
}

/// The whole limit catalog, each entry paired with the quota in force for it.
/// Checked against the model's own `AccountLimitType` value list below, so a
/// limit type this table forgets fails the test rather than going unasserted.
const CATALOG: &[LimitRow] = &[
    LimitRow {
        limit_type: ACCOUNT_LIMIT_MAX_HOSTED_ZONES,
        value: MAX_HOSTED_ZONES,
    },
    LimitRow {
        limit_type: ACCOUNT_LIMIT_MAX_HEALTH_CHECKS,
        value: MAX_HEALTH_CHECKS,
    },
    LimitRow {
        limit_type: ACCOUNT_LIMIT_MAX_REUSABLE_DELEGATION_SETS,
        value: MAX_REUSABLE_DELEGATION_SETS,
    },
    LimitRow {
        limit_type: ACCOUNT_LIMIT_MAX_TRAFFIC_POLICIES,
        value: MAX_TRAFFIC_POLICIES,
    },
    LimitRow {
        limit_type: ACCOUNT_LIMIT_MAX_TRAFFIC_POLICY_INSTANCES,
        value: MAX_TRAFFIC_POLICY_INSTANCES,
    },
];

#[tokio::test]
async fn the_table_below_covers_every_modelled_account_limit_type() {
    // `ACCOUNT_LIMIT_TYPE_VALUES` is the model's own `AccountLimitType` value
    // list, so this is what keeps the catalog above from drifting from it.
    let mut covered: Vec<&str> = CATALOG.iter().map(|row| row.limit_type).collect();
    covered.sort_unstable();
    let mut modelled: Vec<&str> = ACCOUNT_LIMIT_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        covered, modelled,
        "every AccountLimitType must be covered: expected {modelled:?}, got {covered:?}"
    );
}

#[tokio::test]
async fn every_account_limit_type_reports_its_maximum_and_a_count() {
    let router = router();
    for row in CATALOG {
        let reply = get_account_limit(&router, row.limit_type).await;
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
        // Nothing has been created on this router, so the account holds no
        // entity of any kind.
        assert_eq!(
            count, "0",
            "case {:?}: an account holding nothing must report Count 0, got {count:?}; body {}",
            row.limit_type, reply.body
        );
    }
}

#[tokio::test]
async fn the_hosted_zone_count_follows_the_zones_the_account_holds() {
    let router = router();
    let mut zone_ids: Vec<String> = Vec::new();

    // Several populations, not one: the count is asserted with no zones, after
    // each of four creates, and after a delete takes one away again.
    for index in 0..4u32 {
        let expected = usize::try_from(index).expect("the index fits a usize");
        let case = format!("{expected} zone(s) created");
        let reply = get_account_limit(&router, ACCOUNT_LIMIT_MAX_HOSTED_ZONES).await;
        let (_, value, count) = reported(&reply, &case);
        assert_eq!(
            count,
            expected.to_string(),
            "case {case:?}: expected Count {expected}, got {count:?}; body {}",
            reply.body
        );
        // The reported maximum is the account's quota, so it does not move as
        // the population does.
        assert_eq!(
            value,
            MAX_HOSTED_ZONES.to_string(),
            "case {case:?}: expected Limit/Value {MAX_HOSTED_ZONES}, got {value:?}; body {}",
            reply.body
        );
        // And the count is the same population `ListHostedZones` reports.
        let listed = listed_zone_count(&router).await;
        assert_eq!(
            count,
            listed.to_string(),
            "case {case:?}: the count must agree with ListHostedZones ({listed} zones), \
             got {count:?}"
        );

        let created = create_zone(
            &router,
            &format!("zone-{index}.example.com"),
            &format!("account-limit-ref-{index}"),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201 from CreateHostedZone, got {} with body {}",
            created.status, created.body
        );
        zone_ids.push(bare_zone_id(&created));
    }

    // A zone the account no longer holds is no longer counted.
    let removed = zone_ids.pop().expect("four zones were created");
    let deleted = call(
        &router,
        "DELETE",
        &format!("/2013-04-01/hostedzone/{removed}"),
        "",
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "expected 200 from DeleteHostedZone, got {} with body {}",
        deleted.status, deleted.body
    );
    let reply = get_account_limit(&router, ACCOUNT_LIMIT_MAX_HOSTED_ZONES).await;
    let (_, _, count) = reported(&reply, "after a delete");
    assert_eq!(
        count,
        zone_ids.len().to_string(),
        "case {:?}: expected Count {}, got {count:?}; body {}",
        "after a delete",
        zone_ids.len(),
        reply.body
    );

    // A hosted zone is not a health check, a delegation set, a traffic policy
    // or a traffic policy instance, so no other limit's count moved with it.
    for row in CATALOG
        .iter()
        .filter(|row| row.limit_type != ACCOUNT_LIMIT_MAX_HOSTED_ZONES)
    {
        let other = get_account_limit(&router, row.limit_type).await;
        let (_, _, other_count) = reported(&other, row.limit_type);
        assert_eq!(
            other_count, "0",
            "case {:?}: creating hosted zones must not move this count, got {other_count:?}; \
             body {}",
            row.limit_type, other.body
        );
    }
}

#[tokio::test]
async fn reading_a_limit_leaves_the_account_exactly_as_it_was() {
    // The rule is a report, so repeating it answers the same thing and consumes
    // nothing: the count after any number of reads is still the population.
    let router = router();
    let created = create_zone(&router, "idempotent.example.org", "account-limit-idempotent").await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateHostedZone, got {} with body {}",
        created.status, created.body
    );

    for row in CATALOG {
        let first = get_account_limit(&router, row.limit_type).await;
        let first_reported = reported(&first, row.limit_type);
        for attempt in 1..=3 {
            let again = get_account_limit(&router, row.limit_type).await;
            let again_reported = reported(&again, row.limit_type);
            assert_eq!(
                again_reported, first_reported,
                "case {:?}: read {attempt} must report what the first read did ({:?}), \
                 got {:?}; body {}",
                row.limit_type, first_reported, again_reported, again.body
            );
        }
    }

    let zones = listed_zone_count(&router).await;
    assert_eq!(
        zones, 1,
        "reading limits must not create or destroy a zone; ListHostedZones reports {zones}"
    );
}

#[tokio::test]
async fn a_type_outside_the_account_limit_catalog_is_refused() {
    // `Type` is a modelled enum, so a value the catalog does not hold is a bad
    // request — never a 200 carrying an invented limit, and never the 501 of an
    // unimplemented operation.
    let router = router();
    for case in [
        "MAX_HOSTED_ZONES",
        "max_hosted_zones_by_owner",
        "MAX_HOSTED_ZONES_BY_OWNER_EXTRA",
        "MAX_CIDR_COLLECTIONS_BY_OWNER",
    ] {
        let reply = get_account_limit(&router, case).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for a type outside AccountLimitType, got {} \
             with body {}",
            reply.status, reply.body
        );
    }
}
