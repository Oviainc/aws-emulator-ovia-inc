// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetHostedZoneCount`.
//!
//! The rule under test: the operation reports the total number of hosted zones
//! the account holds at the moment of the call — public and private alike —
//! counted from the service's state, and reports it without changing anything.
//!
//! So the table below is not one recorded request: it walks a POPULATION over
//! several shapes — no zones at all, a public zone, several public zones, a
//! private zone (which the total includes just as much), and the population
//! shrinking again as zones are deleted — and after every step asserts the
//! count against the zones that were actually created and against what
//! `ListHostedZones` and `GetAccountLimit` report about the same population.
//!
//! Every test drives the service through the real router, using the SAME
//! harness the other hosted-zone suites use, so what is asserted is the
//! HTTP/restXml behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::state::ACCOUNT_LIMIT_MAX_HOSTED_ZONES;

use crate::hosted_zone_records::{call, router, Reply};
use crate::vpc_associations::{bare_zone_id, create_private_zone, create_zone};

/// Ask for the hosted-zone count.
async fn get_hosted_zone_count(router: &MultiServiceRouter) -> Reply {
    call(router, "GET", "/2013-04-01/hostedzonecount", "").await
}

/// The `HostedZoneCount` a successful reply reports.
async fn reported_count(router: &MultiServiceRouter, case: &str) -> i64 {
    let reply = get_hosted_zone_count(router).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetHostedZoneCount, got {} with body {}",
        reply.status, reply.body
    );
    let text = reply
        .first("HostedZoneCount")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the reply must carry HostedZoneCount; body {}",
                reply.body
            )
        })
        .to_string();
    text.parse::<i64>().unwrap_or_else(|_| {
        panic!("case {case:?}: HostedZoneCount must be a number, got {text:?}")
    })
}

/// How many zones `ListHostedZones` reports, which the count has to agree with.
async fn listed_zone_count(router: &MultiServiceRouter, case: &str) -> usize {
    let listed = call(router, "GET", "/2013-04-01/hostedzone", "").await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from ListHostedZones, got {} with body {}",
        listed.status, listed.body
    );
    listed.all("HostedZone/Id").len()
}

/// The `Count` `GetAccountLimit` reports for the hosted-zone quota, which is the
/// same population seen through the other reporting operation.
async fn account_limit_count(router: &MultiServiceRouter, case: &str) -> String {
    let reply = call(
        router,
        "GET",
        &format!("/2013-04-01/accountlimit/{ACCOUNT_LIMIT_MAX_HOSTED_ZONES}"),
        "",
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetAccountLimit, got {} with body {}",
        reply.status, reply.body
    );
    reply
        .first("Count")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the reply must carry Count; body {}",
                reply.body
            )
        })
        .to_string()
}

/// Delete one zone by its bare (`Z…`) id.
async fn delete_zone(router: &MultiServiceRouter, zone_id: &str, case: &str) {
    let reply = call(
        router,
        "DELETE",
        &format!("/2013-04-01/hostedzone/{zone_id}"),
        "",
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from DeleteHostedZone, got {} with body {}",
        reply.status, reply.body
    );
}

/// What one row of the population walk does to the account.
enum Step {
    /// Create a public hosted zone with this name.
    CreatePublic(&'static str),
    /// Create a private hosted zone with this name, associated with this VPC.
    CreatePrivate(&'static str, &'static str),
    /// Delete the zone created earliest of those still held.
    DeleteOldest,
}

/// One row: a step, and the total the account holds once it has been taken.
struct Row {
    case: &'static str,
    step: Step,
    expected: i64,
}

#[tokio::test]
async fn the_hosted_zone_count_is_the_population_of_zones_the_account_holds() {
    // The rows differ along the population's SIZE and its COMPOSITION (public,
    // private, mixed) and along the direction it moves (growing, then
    // shrinking) — not merely along a zone's name.
    let rows = [
        Row {
            case: "the first public zone",
            step: Step::CreatePublic("first.example.com"),
            expected: 1,
        },
        Row {
            case: "a second public zone, in another TLD",
            step: Step::CreatePublic("second.example.net"),
            expected: 2,
        },
        Row {
            case: "a private zone, which the total counts too",
            step: Step::CreatePrivate("private.example.org", "vpc-0a1b2c3d4e5f60718"),
            expected: 3,
        },
        Row {
            case: "a second private zone, on the short VPC id form",
            step: Step::CreatePrivate("other-private.example.test", "vpc-1a2b3c4d"),
            expected: 4,
        },
        Row {
            case: "a fifth zone, public again",
            step: Step::CreatePublic("fifth.example.com"),
            expected: 5,
        },
        Row {
            case: "one zone deleted",
            step: Step::DeleteOldest,
            expected: 4,
        },
        Row {
            case: "a second zone deleted",
            step: Step::DeleteOldest,
            expected: 3,
        },
        Row {
            case: "a zone created again after the deletes",
            step: Step::CreatePublic("back-again.example.com"),
            expected: 4,
        },
    ];

    let router = router();

    // An account that holds no zone counts none.
    let empty = reported_count(&router, "no zones at all").await;
    assert_eq!(
        empty, 0,
        "case {:?}: expected HostedZoneCount 0, got {empty}",
        "no zones at all"
    );

    let mut held: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        match row.step {
            Step::CreatePublic(name) => {
                let reply = create_zone(&router, name, &format!("count-ref-{index}"), "").await;
                assert_eq!(
                    reply.status, 201,
                    "case {:?}: expected 201 from CreateHostedZone, got {} with body {}",
                    row.case, reply.status, reply.body
                );
                held.push(bare_zone_id(&reply));
            }
            Step::CreatePrivate(name, vpc_id) => {
                held.push(
                    create_private_zone(&router, name, &format!("count-ref-{index}"), vpc_id).await,
                );
            }
            Step::DeleteOldest => {
                let oldest = held.remove(0);
                delete_zone(&router, &oldest, row.case).await;
            }
        }

        let count = reported_count(&router, row.case).await;
        assert_eq!(
            count, row.expected,
            "case {:?}: expected HostedZoneCount {}, got {count}",
            row.case, row.expected
        );
        // The row's own bookkeeping, the listing and the account limit all
        // describe the same population, so the count has to agree with each.
        let held_now = i64::try_from(held.len()).expect("the population fits an i64");
        assert_eq!(
            count, held_now,
            "case {:?}: expected the count to be the {held_now} zone(s) still held, got {count}",
            row.case
        );
        let listed = listed_zone_count(&router, row.case).await;
        assert_eq!(
            count.to_string(),
            listed.to_string(),
            "case {:?}: the count must agree with ListHostedZones ({listed} zones), got {count}",
            row.case
        );
        let limit_count = account_limit_count(&router, row.case).await;
        assert_eq!(
            count.to_string(),
            limit_count,
            "case {:?}: the count must agree with GetAccountLimit ({limit_count}), got {count}",
            row.case
        );
    }
}

#[tokio::test]
async fn reading_the_hosted_zone_count_leaves_the_account_exactly_as_it_was() {
    // The rule is a report, so repeating it answers the same thing and creates,
    // destroys and consumes nothing.
    let router = router();
    for (index, name) in ["idempotent-a.example.com", "idempotent-b.example.net"]
        .iter()
        .enumerate()
    {
        let reply = create_zone(&router, name, &format!("count-idempotent-{index}"), "").await;
        assert_eq!(
            reply.status, 201,
            "case {name:?}: expected 201 from CreateHostedZone, got {} with body {}",
            reply.status, reply.body
        );
    }

    let first = reported_count(&router, "the first read").await;
    assert_eq!(
        first, 2,
        "case {:?}: expected HostedZoneCount 2, got {first}",
        "the first read"
    );
    for attempt in 1..=4 {
        let case = format!("read {attempt}");
        let again = reported_count(&router, &case).await;
        assert_eq!(
            again, first,
            "case {case:?}: a repeated read must report what the first read did ({first}), \
             got {again}"
        );
    }

    let listed = listed_zone_count(&router, "after the repeated reads").await;
    assert_eq!(
        listed, 2,
        "reading the count must not create or destroy a zone; ListHostedZones reports {listed}"
    );
}

#[tokio::test]
async fn the_hosted_zone_count_is_not_the_unimplemented_refusal() {
    // A scaffolded operation answers 501, so this pins the operation as served:
    // a 200 carrying a count, whatever the population.
    let router = router();
    let reply = get_hosted_zone_count(&router).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: GetHostedZoneCount must be served, got {} with body {}",
        "an untouched account", reply.status, reply.body
    );
    assert!(
        reply.first("HostedZoneCount").is_some(),
        "case {:?}: expected a HostedZoneCount element, got body {}",
        "an untouched account",
        reply.body
    );
}
