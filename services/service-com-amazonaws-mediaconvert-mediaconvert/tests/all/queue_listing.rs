// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListQueues`.
//!
//! The rule every case below is one row of: `ListQueues` reports the account's STORED
//! queues — "This will return the queues themselves, not just a list of them" — ordered on
//! the key `listBy` names (`QueueListBy`: "alphabetically by NAME or chronologically by
//! CREATION_DATE. If you don't specify, the service will list them by creation date") in
//! the direction `order` names, in pages of at most `maxResults` with an opaque
//! `nextToken` that resumes immediately after the last queue of the page just returned,
//! beside the Region's concurrent-job pool (`TotalConcurrentJobs`) and the part of it no
//! on-demand queue has allocated (`UnallocatedConcurrentJobs`).
//!
//! Cases that are about the ORDERING seed their records straight into the store, because
//! only a seeded record can carry a creation instant a case chose; the write side's own
//! coverage lives in `tests/all/job_lifecycle.rs` and `tests/all/queue_update.rs`. The
//! round-trip case goes through `CreateQueue` and `GetQueue` over HTTP instead, so the
//! listing is shown to project the same stored record those two do. Nothing here reaches
//! past HTTP except the seeding and the store snapshot.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    QueueRecord, DEFAULT_CONCURRENT_JOBS, DEFAULT_MAXIMUM_CONCURRENT_FEEDS, DEFAULT_QUEUE_NAME,
    PRICING_PLAN_ON_DEMAND, QUEUE_STATUS_ACTIVE, QUEUE_TYPE_CUSTOM, TOTAL_CONCURRENT_JOBS,
};
use service_com_amazonaws_mediaconvert_mediaconvert::ComAmazonawsMediaconvertMediaconvertState;

use super::job_lifecycle::{Emulator, Reply, REGION};

/// `GET /2017-08-29/queues` — the binding
/// `smithy_query(query="operation",operation="ListQueues")` reports, success code 200.
async fn list_queues(emulator: &Emulator, query: &str) -> Reply {
    list_queues_in(emulator, REGION, query).await
}

/// The same call signed for another Region, so a case can show the ARNs follow the
/// request.
async fn list_queues_in(emulator: &Emulator, region: &str, query: &str) -> Reply {
    let uri = if query.is_empty() {
        "/2017-08-29/queues".to_string()
    } else {
        format!("/2017-08-29/queues?{query}")
    };
    emulator.send_in(region, "GET", &uri, None).await
}

/// The `name` of every queue in `queues`, in the order they were returned.
fn names(reply: &Reply) -> Vec<String> {
    reply
        .at("/queues")
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.pointer("/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `nextToken`, or `None` when the response carries none.
fn next_token(reply: &Reply) -> Option<String> {
    reply.at("/nextToken").as_str().map(str::to_string)
}

/// `(totalConcurrentJobs, unallocatedConcurrentJobs)` as the response reports them.
fn pool(reply: &Reply) -> (Option<i64>, Option<i64>) {
    (
        reply.at("/totalConcurrentJobs").as_i64(),
        reply.at("/unallocatedConcurrentJobs").as_i64(),
    )
}

/// An instant `seconds_ago` before now, for seeding a creation order that is deliberately
/// not the alphabetical one.
fn ago(seconds_ago: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(seconds_ago)
}

/// Epoch seconds of an instant, the unit `CreatedAt`/`LastUpdated` are modelled in
/// (`__timestampUnix`, `epoch-seconds`).
fn epoch_secs(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Put one of the account's own ([`QUEUE_TYPE_CUSTOM`]) queues in the store, created at
/// an instant the case chose.
fn seed_queue(emulator: &Emulator, name: &str, created_at: SystemTime) {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .queues
        .insert(
            name.to_string(),
            QueueRecord {
                name: name.to_string(),
                description: Some(format!("{name} description")),
                pricing_plan: PRICING_PLAN_ON_DEMAND.to_string(),
                status: QUEUE_STATUS_ACTIVE.to_string(),
                queue_type: QUEUE_TYPE_CUSTOM.to_string(),
                concurrent_jobs: DEFAULT_CONCURRENT_JOBS,
                maximum_concurrent_feeds: DEFAULT_MAXIMUM_CONCURRENT_FEEDS,
                reservation_plan: None,
                created_at,
                last_updated: created_at,
                tags: BTreeMap::new(),
            },
        );
}

/// `(name, createdAt, lastUpdated, status, concurrentJobs)` for every stored queue, so a
/// case can show a read left the store alone.
fn store_snapshot(emulator: &Emulator) -> Vec<(String, u64, u64, String, i32)> {
    emulator
        .engine
        .state::<ComAmazonawsMediaconvertMediaconvertState>()
        .lock()
        .queues
        .values()
        .map(|record| {
            (
                record.name.clone(),
                epoch_secs(record.created_at),
                epoch_secs(record.last_updated),
                record.status.clone(),
                record.concurrent_jobs,
            )
        })
        .collect()
}

/// Three of the account's own queues whose alphabetical order and creation order are
/// deliberately different, so a case can tell the two keys apart. The built-in
/// [`DEFAULT_QUEUE_NAME`] queue every account carries is the NEWEST of the four, because
/// the store seeds it when the account comes up.
fn seed_three(emulator: &Emulator) {
    seed_queue(emulator, "zeta-queue", ago(300));
    seed_queue(emulator, "alpha-queue", ago(200));
    seed_queue(emulator, "mid-queue", ago(100));
}

/// `listBy` picks the key and `order` picks the direction: the full matrix of the two
/// `QueueListBy` values against the two `Order` values, plus the absent default of each.
#[tokio::test]
async fn list_by_and_order_span_the_orderings_the_model_declares() {
    let emulator = Emulator::new();
    seed_three(&emulator);

    // Byte order on `Name`: the built-in queue's capital letter sorts before the
    // lowercase names.
    let by_name = vec![
        DEFAULT_QUEUE_NAME.to_string(),
        "alpha-queue".to_string(),
        "mid-queue".to_string(),
        "zeta-queue".to_string(),
    ];
    let by_creation = vec![
        "zeta-queue".to_string(),
        "alpha-queue".to_string(),
        "mid-queue".to_string(),
        DEFAULT_QUEUE_NAME.to_string(),
    ];
    let mut name_descending = by_name.clone();
    name_descending.reverse();
    let mut creation_descending = by_creation.clone();
    creation_descending.reverse();

    let cases: Vec<(&str, &Vec<String>)> = vec![
        // No `listBy`: "If you don't specify, the service will list them by creation
        // date", and no `order` runs up that key, oldest first.
        ("", &by_creation),
        ("order=ASCENDING", &by_creation),
        ("order=DESCENDING", &creation_descending),
        ("listBy=CREATION_DATE", &by_creation),
        ("listBy=CREATION_DATE&order=ASCENDING", &by_creation),
        ("listBy=CREATION_DATE&order=DESCENDING", &creation_descending),
        ("listBy=NAME", &by_name),
        ("listBy=NAME&order=ASCENDING", &by_name),
        ("listBy=NAME&order=DESCENDING", &name_descending),
    ];

    for (query, expected) in cases {
        let reply = list_queues(&emulator, query).await;
        assert_eq!(
            reply.status, 200,
            "case {query:?}: expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            &names(&reply),
            expected,
            "case {query:?}: expected {expected:?}, got {:?}",
            names(&reply)
        );
        assert_eq!(
            next_token(&reply),
            None,
            "case {query:?}: one page holds every queue, so no nextToken was due, got {:?}",
            next_token(&reply)
        );
    }
}

/// Paging walks the ordering exactly once: concatenating the pages reproduces the whole
/// listing, no page is longer than asked for, and the last page carries no token.
///
/// The table spans the page sizes across the `maxResults` range (its published bounds, 1
/// and 20, and sizes in between), both keys and both directions, over more queues than one
/// page holds.
#[tokio::test]
async fn paging_walks_every_queue_exactly_once() {
    let emulator = Emulator::new();
    // 25 of the account's own queues, plus the built-in one: more than the 20 a single
    // page holds, so the default page size is exercised too.
    let seeded = 25_usize;
    for index in 0..seeded {
        // Zero-padded so alphabetical order and creation order agree among them, and the
        // built-in queue is the only name whose two positions differ.
        seed_queue(
            &emulator,
            &format!("queue-{index:02}"),
            ago(1_000 - index as u64),
        );
    }
    let seeded_names: Vec<String> = (0..seeded).map(|index| format!("queue-{index:02}")).collect();
    let total = seeded + 1;

    // The built-in queue is the newest record and the first name in byte order.
    let mut by_name = vec![DEFAULT_QUEUE_NAME.to_string()];
    by_name.extend(seeded_names.clone());
    let mut by_creation = seeded_names.clone();
    by_creation.push(DEFAULT_QUEUE_NAME.to_string());

    for (list_by, ascending) in [("NAME", &by_name), ("CREATION_DATE", &by_creation)] {
        let mut descending = ascending.clone();
        descending.reverse();
        for order in ["ASCENDING", "DESCENDING"] {
            let expected = if order == "ASCENDING" {
                ascending
            } else {
                &descending
            };
            for page_size in [1_usize, 2, 7, 20] {
                let case = format!("listBy={list_by}&order={order}&maxResults={page_size}");
                let mut seen: Vec<String> = Vec::new();
                let mut token: Option<String> = None;
                let mut pages = 0_usize;
                loop {
                    let mut query = case.clone();
                    if let Some(token) = &token {
                        query.push_str(&format!("&nextToken={token}"));
                    }
                    let reply = list_queues(&emulator, &query).await;
                    assert_eq!(
                        reply.status, 200,
                        "case {case}: page {pages} expected HTTP 200, got {} with body {}",
                        reply.status, reply.body
                    );
                    let page = names(&reply);
                    assert!(
                        page.len() <= page_size,
                        "case {case}: page {pages} expected at most {page_size} items, got {page:?}"
                    );
                    seen.extend(page);
                    pages += 1;
                    token = next_token(&reply);
                    if token.is_none() {
                        break;
                    }
                    assert!(
                        pages <= total + 1,
                        "case {case}: paging did not terminate, saw {seen:?}"
                    );
                }
                assert_eq!(
                    &seen, expected,
                    "case {case}: expected the whole listing in order, got {seen:?}"
                );
                let expected_pages = total.div_ceil(page_size);
                assert_eq!(
                    pages, expected_pages,
                    "case {case}: expected {expected_pages} pages, got {pages}"
                );
            }
        }
    }

    // No `maxResults`: "up to twenty of your queues", and a token for the rest.
    let first = list_queues(&emulator, "listBy=NAME").await;
    assert_eq!(
        names(&first).len(),
        20,
        "case default page size: expected 20 items, got {:?}",
        names(&first)
    );
    let token = next_token(&first)
        .unwrap_or_else(|| panic!("case default page size: a {total}-queue listing owes a token"));
    let second = list_queues(&emulator, &format!("listBy=NAME&nextToken={token}")).await;
    assert_eq!(
        names(&second),
        by_name[20..],
        "case default page size: expected the remaining queues, got {:?}",
        names(&second)
    );
    assert_eq!(
        next_token(&second),
        None,
        "case default page size: the second page ends the listing, got {:?}",
        next_token(&second)
    );
}

/// `maxResults` carries the model's own range, 1..20: each published bound is accepted and
/// one step past either is refused before the handler runs.
#[tokio::test]
async fn max_results_is_held_to_the_range_the_model_declares() {
    let emulator = Emulator::new();
    seed_three(&emulator);

    for accepted in ["maxResults=1", "maxResults=20"] {
        let reply = list_queues(&emulator, accepted).await;
        assert_eq!(
            reply.status, 200,
            "case {accepted:?}: inside the range, expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
    }
    for refused in ["maxResults=0", "maxResults=21"] {
        let reply = list_queues(&emulator, refused).await;
        assert_eq!(
            reply.status, 400,
            "case {refused:?}: outside the range, expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
    }
}

/// A `nextToken` this service never issued is a bad request, not a panic and not a silent
/// first page. The table spans the ways a token can be wrong.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_a_bad_request() {
    let emulator = Emulator::new();
    seed_three(&emulator);

    let cases = [
        // Not hex at all.
        "not-a-token".to_string(),
        // Hex of a string with no `:` separator.
        ::hex::encode("alpha-queue"),
        // Hex of a separated pair whose first half is not an instant.
        ::hex::encode("soon:alpha-queue"),
        // Hex of an empty cursor.
        ::hex::encode(""),
    ];

    for token in cases {
        let reply = list_queues(&emulator, &format!("nextToken={token}")).await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("BadRequestException"),
            "case {token:?}: expected BadRequestException, got {:?}",
            reply.error_type
        );
    }
}

/// A listed queue is the SAME projection of the stored record `GetQueue` answers with, for
/// every kind of queue a caller can create, in whatever Region the request is signed for.
///
/// This is the round trip: the create persists, the listing reports what was persisted,
/// and the single-resource reader agrees with it item for item.
#[tokio::test]
async fn a_listed_queue_is_the_stored_record_get_queue_reads_back() {
    let emulator = Emulator::new();

    let creates = [
        // The bare create: every concurrency member is service-applied.
        json!({ "name": "plain-queue" }),
        // Members the caller chose, including the published lower bound of
        // `MaximumConcurrentFeeds` (`__integerMin0`).
        json!({
            "name": "described-queue",
            "description": "nightly promos",
            "concurrentJobs": 7,
            "maximumConcurrentFeeds": 0,
            "status": "PAUSED",
            "tags": { "team": "video" }
        }),
        // A reserved queue, whose concurrency comes from its reservation plan.
        json!({
            "name": "reserved-queue",
            "pricingPlan": "RESERVED",
            "reservationPlanSettings": {
                "commitment": "ONE_YEAR",
                "renewalType": "EXPIRE",
                "reservedSlots": 5
            }
        }),
    ];
    let mut created: Vec<String> = Vec::new();
    for body in &creates {
        let reply = emulator.create_queue(body.clone()).await;
        assert_eq!(
            reply.status, 201,
            "case {body}: expected HTTP 201, got {} with body {}",
            reply.status, reply.body
        );
        created.push(reply.string("/queue/name"));
    }

    for region in [REGION, "eu-central-1", "ap-southeast-2"] {
        let listed = list_queues_in(&emulator, region, "listBy=NAME").await;
        assert_eq!(
            listed.status, 200,
            "case region {region:?}: expected HTTP 200, got {} with body {}",
            listed.status, listed.body
        );
        for name in &created {
            let item = listed
                .at("/queues")
                .as_array()
                .and_then(|items| {
                    items
                        .iter()
                        .find(|item| item.pointer("/name").and_then(Value::as_str) == Some(name))
                })
                .cloned()
                .unwrap_or(Value::Null);
            let single = emulator.send_in(region, "GET", &format!("/2017-08-29/queues/{name}"), None).await;
            assert_eq!(
                item,
                *single.at("/queue"),
                "case region {region:?} queue {name:?}: the listing must project the same \
                 stored record GetQueue does, got {item} and {}",
                single.at("/queue")
            );
            // Built the way the emulator's own ARN helper builds it, so the expectation is
            // not a second hand-written format, and it names the REQUEST's Region.
            assert_eq!(
                item.pointer("/arn").and_then(Value::as_str),
                Some(engine::emulator_arn("mediaconvert", region, &format!("queues/{name}")).as_str()),
                "case region {region:?} queue {name:?}: expected this Region's ARN, got {item}"
            );
        }
    }
}

/// The pool members are computed from the stored queues: `TotalConcurrentJobs` is the
/// Region's ceiling, and `UnallocatedConcurrentJobs` is what no ON_DEMAND queue has
/// allocated out of it — "The remaining number of concurrent jobs that are not associated
/// with a queue and are available to allocate to a queue."
///
/// Every row's expectation is arithmetic on the crate's own constants, so no case restates
/// a number the handler computes. The rows span: nothing created, one and several
/// allocations, an allocation the service filled in, a RESERVED queue (paid for by its
/// reservation plan, so outside the on-demand ceiling), and an account that has allocated
/// more than the ceiling.
#[tokio::test]
async fn the_concurrent_job_pool_is_computed_from_the_stored_queues() {
    /// A reserved queue's create body: its slots are bought, not allocated out of the
    /// on-demand pool.
    fn reserved(name: &str, slots: i32) -> Value {
        json!({
            "name": name,
            "pricingPlan": "RESERVED",
            "reservationPlanSettings": {
                "commitment": "ONE_YEAR",
                "renewalType": "EXPIRE",
                "reservedSlots": slots
            }
        })
    }

    // The built-in `Default` queue is on-demand and already holds its own allocation, so
    // every row starts from the ceiling less that.
    let fresh = TOTAL_CONCURRENT_JOBS - DEFAULT_CONCURRENT_JOBS;
    let cases: Vec<(&str, Vec<Value>, i32)> = vec![
        ("no queue beyond the built-in one", vec![], fresh),
        (
            "one on-demand queue allocating 50",
            vec![json!({ "name": "a", "concurrentJobs": 50 })],
            fresh - 50,
        ),
        (
            "two on-demand queues allocating 50 and 125",
            vec![
                json!({ "name": "a", "concurrentJobs": 50 }),
                json!({ "name": "b", "concurrentJobs": 125 }),
            ],
            fresh - 175,
        ),
        (
            "an on-demand queue whose allocation the service filled in",
            vec![json!({ "name": "a" })],
            fresh - DEFAULT_CONCURRENT_JOBS,
        ),
        (
            "a reserved queue allocates nothing from the on-demand pool",
            vec![reserved("r", 5)],
            fresh,
        ),
        (
            "a reserved queue beside an on-demand one: only the on-demand one allocates",
            vec![reserved("r", 5), json!({ "name": "a", "concurrentJobs": 30 })],
            fresh - 30,
        ),
        (
            "allocated past the ceiling: nothing is left to allocate, never a negative",
            vec![
                json!({ "name": "a" }),
                json!({ "name": "b" }),
                json!({ "name": "c" }),
                json!({ "name": "d" }),
                json!({ "name": "e" }),
            ],
            0,
        ),
    ];

    for (label, creates, expected_unallocated) in cases {
        let emulator = Emulator::new();
        for body in &creates {
            let reply = emulator.create_queue(body.clone()).await;
            assert_eq!(
                reply.status, 201,
                "case {label:?}: create {body} expected HTTP 201, got {} with body {}",
                reply.status, reply.body
            );
        }
        let reply = list_queues(&emulator, "").await;
        assert_eq!(
            pool(&reply),
            (
                Some(i64::from(TOTAL_CONCURRENT_JOBS)),
                Some(i64::from(expected_unallocated))
            ),
            "case {label:?}: expected the ceiling and {expected_unallocated} unallocated, \
             got {:?} from body {}",
            pool(&reply),
            reply.body
        );

        // The pool is the account's in this Region, not the page's: every page of a paged
        // listing reports the same two counts.
        let mut token: Option<String> = None;
        let mut page = 0_usize;
        loop {
            let query = match &token {
                Some(token) => format!("maxResults=1&nextToken={token}"),
                None => "maxResults=1".to_string(),
            };
            let paged = list_queues(&emulator, &query).await;
            assert_eq!(
                pool(&paged),
                pool(&reply),
                "case {label:?}: page {page} must report the same pool as the whole \
                 listing, got {:?}",
                pool(&paged)
            );
            page += 1;
            token = next_token(&paged);
            if token.is_none() {
                break;
            }
        }
    }
}

/// Listing is a read: it changes nothing a later caller sees, however often and with
/// whatever filters it is called.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    seed_three(&emulator);
    let before = store_snapshot(&emulator);

    let queries = [
        "",
        "listBy=NAME",
        "listBy=NAME&order=DESCENDING",
        "listBy=CREATION_DATE&order=DESCENDING",
        "maxResults=1",
    ];
    let mut first_bodies = Vec::new();
    for query in queries {
        first_bodies.push(list_queues(&emulator, query).await.body);
    }
    for (query, first) in queries.iter().zip(first_bodies) {
        let again = list_queues(&emulator, query).await.body;
        assert_eq!(
            again, first,
            "case {query:?}: a repeated listing must answer the same, got {again} then {first}"
        );
    }

    let after = store_snapshot(&emulator);
    assert_eq!(
        after, before,
        "the store must be untouched by reads: expected {before:?}, got {after:?}"
    );
}
