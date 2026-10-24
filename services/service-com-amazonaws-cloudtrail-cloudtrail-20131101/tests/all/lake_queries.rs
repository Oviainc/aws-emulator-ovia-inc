// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeQuery` — CloudTrail Lake query metadata.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `DescribeQuery` REPORTS a persisted query: every member of
//!   `DescribeQueryResponse` is the value the query record carries, for every
//!   value of the model's `QueryStatus` enum, and `DeliveryS3Uri` /
//!   `DeliveryStatus` are reported exactly when the run delivered results to S3;
//! * it writes nothing — two calls in a row report the same thing;
//! * "You must specify either QueryId or QueryAlias": a request with neither,
//!   with both, or with `RefreshId` but no alias is refused with
//!   `InvalidParameterException`;
//! * a `QueryAlias` reports the LAST run of that alias, narrowed to one
//!   dashboard refresh by `RefreshId`;
//! * `EventDataStore` is "the ARN (or the ID suffix of the ARN) of an event data
//!   store": an unreadable one is `EventDataStoreARNInvalidException`, a readable
//!   one naming no store in the addressed Region is
//!   `EventDataStoreNotFoundException`, and a store awaiting deletion is
//!   `InactiveEventDataStoreException`;
//! * a query lives in an event data store, so the store resolves BEFORE the
//!   selector: a Region — or the account `EventDataStoreOwnerAccountId` names —
//!   holding no store refuses every selector with
//!   `EventDataStoreNotFoundException`;
//! * a query is reachable only from the Region it ran in and only through the
//!   event data store it ran on; an unmatched selector is
//!   `QueryIdNotFoundException`, naming the member and the value it was selected
//!   with.
//!
//! Both not-found refusals answer HTTP 400 — the status CloudTrail answers with,
//! which is not the `@httpError(404)` its model annotates.
//!
//! `StartQuery` and `CreateEventDataStore` are still scaffold stubs
//! (`issues/things-that-do-not-work/…-cloudtrail-startquery-stub.md` and
//! `…-cloudtrail-createeventdatastore-stub.md`), so no implemented operation can
//! mint a query yet. These cases therefore seed the SAME registries the handler
//! reads, through the shared engine, and then go over the wire for every
//! assertion.

use serde_json::{json, Value};
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::{
    DELIVERY_STATUS_VALUES, EVENT_DATA_STORE_STATUS_VALUES, QUERY_STATUS_VALUES,
};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, EventDataStoreRecord, EventDataStoreState, QueryDelivery, QueryRecord,
    QueryState, QueryStatisticsRecord,
};

use super::emulator::{Emulator, HOME_REGION};

/// The id of the event data store most cases run their queries on. An event data
/// store id is the resource suffix of its ARN.
const STORE_ID: &str = "aaaaaaaa-1111-2222-3333-444444444444";

/// The HTTP status this service answers a not-found refusal with: the model
/// annotates `EventDataStoreNotFoundException` / `QueryIdNotFoundException`
/// `@httpError(404)`, and the recorded CloudTrail response is 400.
const NOT_FOUND_STATUS: u16 = 400;

/// A `UUID` is exactly 36 characters of `[a-f0-9-]`, so a query id is built to
/// that shape: the boundary validator refuses anything else before the handler
/// runs.
fn query_id(nth: u8) -> String {
    format!("{nth:02x}aaaaaa-1111-2222-3333-444444444444")
}

/// An instant, as a fixed offset from the epoch so creation times are ordered
/// and comparable.
fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// Seed one event data store, the way `CreateEventDataStore` will once it is
/// implemented.
fn seed_store(emulator: &Emulator, id: &str, region: &str, status: EventDataStoreState) {
    seed_store_with(emulator, id, region, status, None);
}

/// Seed one event data store owned by `owner_account_id` (`None` = the emulator's
/// own account), for the cross-account cases.
fn seed_store_owned_by(
    emulator: &Emulator,
    id: &str,
    region: &str,
    owner_account_id: Option<&str>,
) {
    seed_store_with(
        emulator,
        id,
        region,
        EventDataStoreState::Enabled,
        owner_account_id,
    );
}

fn seed_store_with(
    emulator: &Emulator,
    id: &str,
    region: &str,
    status: EventDataStoreState,
    owner_account_id: Option<&str>,
) {
    emulator.state().lock().event_data_stores.insert(
        id.to_string(),
        EventDataStoreRecord {
            id: id.to_string(),
            name: format!("store-{id}"),
            region: region.to_string(),
            status,
            owner_account_id: owner_account_id.map(str::to_string),
            created_at: Some(at(1_700_000_000)),
            // The members only a read of the store itself reports (see
            // `event_data_store_reads.rs`) are not what these cases vary, and
            // `PutResourcePolicy` is what attaches a resource policy.
            ..EventDataStoreRecord::default()
        },
    );
}

/// Seed one query run, the way `StartQuery` will once it is implemented.
fn seed_query(emulator: &Emulator, record: &QueryRecord) {
    emulator
        .state()
        .lock()
        .queries
        .insert(record.query_id.clone(), record.clone());
}

/// A query run on [`STORE_ID`] in [`HOME_REGION`], with the fields every case
/// shares.
fn query_on_store(nth: u8) -> QueryRecord {
    QueryRecord {
        query_id: query_id(nth),
        event_data_store_id: STORE_ID.to_string(),
        region: HOME_REGION.to_string(),
        query_string: format!("SELECT eventName FROM {STORE_ID} LIMIT {nth}"),
        created_at: Some(at(1_700_000_000 + u64::from(nth))),
        ..QueryRecord::default()
    }
}

/// Assert a `DescribeQuery` reply reports exactly what `record` persists — every
/// member of `DescribeQueryResponse`, present or absent.
fn expect_reported(case: &str, body: &Value, record: &QueryRecord) {
    assert_eq!(
        body.get("QueryId").and_then(Value::as_str),
        Some(record.query_id.as_str()),
        "case {case}: expected QueryId {:?}, got {body:?}",
        record.query_id
    );
    assert_eq!(
        body.get("QueryString").and_then(Value::as_str),
        Some(record.query_string.as_str()),
        "case {case}: expected QueryString {:?}, got {body:?}",
        record.query_string
    );
    assert_eq!(
        body.get("QueryStatus").and_then(Value::as_str),
        Some(record.status.as_wire()),
        "case {case}: expected QueryStatus {:?}, got {body:?}",
        record.status.as_wire()
    );
    assert_eq!(
        body.get("ErrorMessage").and_then(Value::as_str),
        record.error_message.as_deref(),
        "case {case}: expected ErrorMessage {:?}, got {body:?}",
        record.error_message
    );
    assert_eq!(
        body.get("Prompt").and_then(Value::as_str),
        record.prompt.as_deref(),
        "case {case}: expected Prompt {:?}, got {body:?}",
        record.prompt
    );
    assert_eq!(
        body.get("EventDataStoreOwnerAccountId").and_then(Value::as_str),
        record.event_data_store_owner_account_id.as_deref(),
        "case {case}: expected EventDataStoreOwnerAccountId {:?}, got {body:?}",
        record.event_data_store_owner_account_id
    );
    // The S3 URI and the delivery status are reported as a pair, or not at all.
    assert_eq!(
        body.get("DeliveryS3Uri").and_then(Value::as_str),
        record.delivery.as_ref().map(|d| d.s3_uri.as_str()),
        "case {case}: expected DeliveryS3Uri {:?}, got {body:?}",
        record.delivery.as_ref().map(|d| &d.s3_uri)
    );
    assert_eq!(
        body.get("DeliveryStatus").and_then(Value::as_str),
        record.delivery.as_ref().map(|d| d.status.as_str()),
        "case {case}: expected DeliveryStatus {:?}, got {body:?}",
        record.delivery.as_ref().map(|d| &d.status)
    );

    let statistics = body
        .get("QueryStatistics")
        .unwrap_or_else(|| panic!("case {case}: expected QueryStatistics, got {body:?}"));
    assert_eq!(
        statistics.get("EventsMatched").and_then(Value::as_i64),
        Some(record.statistics.events_matched),
        "case {case}: expected EventsMatched {}, got {statistics:?}",
        record.statistics.events_matched
    );
    assert_eq!(
        statistics.get("EventsScanned").and_then(Value::as_i64),
        Some(record.statistics.events_scanned),
        "case {case}: expected EventsScanned {}, got {statistics:?}",
        record.statistics.events_scanned
    );
    assert_eq!(
        statistics.get("BytesScanned").and_then(Value::as_i64),
        Some(record.statistics.bytes_scanned),
        "case {case}: expected BytesScanned {}, got {statistics:?}",
        record.statistics.bytes_scanned
    );
    assert_eq!(
        statistics
            .get("ExecutionTimeInMillis")
            .and_then(Value::as_i64),
        record.statistics.execution_time_in_millis.map(i64::from),
        "case {case}: expected ExecutionTimeInMillis {:?}, got {statistics:?}",
        record.statistics.execution_time_in_millis
    );
    let creation_time = record.created_at.map(|at| {
        at.duration_since(SystemTime::UNIX_EPOCH)
            .expect("a creation time after the epoch")
            .as_secs() as f64
    });
    assert_eq!(
        statistics.get("CreationTime").and_then(Value::as_f64),
        creation_time,
        "case {case}: expected CreationTime {creation_time:?}, got {statistics:?}"
    );
}

/// The state enums this crate stores a query and a store in must cover exactly
/// the values the model's `QueryStatus` and `EventDataStoreStatus` enums accept,
/// so a reported status is always a value a client can read.
#[test]
fn the_stored_states_are_the_models_own_enum_values() {
    let mut query_states: Vec<&str> = [
        QueryState::Queued,
        QueryState::Running,
        QueryState::Finished,
        QueryState::Failed,
        QueryState::Cancelled,
        QueryState::TimedOut,
    ]
    .iter()
    .map(|state| state.as_wire())
    .collect();
    query_states.sort_unstable();
    assert_eq!(
        query_states, QUERY_STATUS_VALUES,
        "case QueryStatus: expected the model's values {QUERY_STATUS_VALUES:?}, got {query_states:?}"
    );

    let mut store_states: Vec<&str> = [
        EventDataStoreState::Created,
        EventDataStoreState::Enabled,
        EventDataStoreState::PendingDeletion,
        EventDataStoreState::StartingIngestion,
        EventDataStoreState::StoppingIngestion,
        EventDataStoreState::StoppedIngestion,
    ]
    .iter()
    .map(|state| state.as_wire())
    .collect();
    store_states.sort_unstable();
    assert_eq!(
        store_states, EVENT_DATA_STORE_STATUS_VALUES,
        "case EventDataStoreStatus: expected the model's values \
         {EVENT_DATA_STORE_STATUS_VALUES:?}, got {store_states:?}"
    );
}

/// Every persisted query, whatever state its run reached, is reported member for
/// member — the row set spans the whole `QueryStatus` catalog, delivery present
/// and absent, and a generated query's prompt.
#[tokio::test]
async fn describe_query_reports_every_persisted_query() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    let records = vec![
        // A run that has not started: no run time, no statistics, no delivery.
        QueryRecord {
            status: QueryState::Queued,
            ..query_on_store(1)
        },
        // A run under way, with the statistics it has accumulated so far.
        QueryRecord {
            status: QueryState::Running,
            statistics: QueryStatisticsRecord {
                events_matched: 3,
                events_scanned: 128,
                bytes_scanned: 4_096,
                execution_time_in_millis: None,
            },
            ..query_on_store(2)
        },
        // A finished run whose results went to S3: the URI and the delivery
        // status travel together.
        QueryRecord {
            status: QueryState::Finished,
            statistics: QueryStatisticsRecord {
                events_matched: 12,
                events_scanned: 9_001,
                bytes_scanned: 1_048_576,
                execution_time_in_millis: Some(742),
            },
            delivery: Some(QueryDelivery {
                s3_uri: "s3://lake-results/describe-query/".to_string(),
                // From the model's `DeliveryStatus` enum.
                status: "SUCCESS".to_string(),
            }),
            event_data_store_owner_account_id: Some("210987654321".to_string()),
            ..query_on_store(3)
        },
        // A failed run carries the reason it failed.
        QueryRecord {
            status: QueryState::Failed,
            error_message: Some("The query could not be parsed.".to_string()),
            statistics: QueryStatisticsRecord {
                events_matched: 0,
                events_scanned: 0,
                bytes_scanned: 0,
                execution_time_in_millis: Some(11),
            },
            ..query_on_store(4)
        },
        // A cancelled run, and a delivery that never completed.
        QueryRecord {
            status: QueryState::Cancelled,
            delivery: Some(QueryDelivery {
                s3_uri: "s3://lake-results/cancelled/".to_string(),
                status: "CANCELLED".to_string(),
            }),
            ..query_on_store(5)
        },
        // A run that hit the service's time limit.
        QueryRecord {
            status: QueryState::TimedOut,
            error_message: Some("The query timed out.".to_string()),
            statistics: QueryStatisticsRecord {
                events_matched: 1,
                events_scanned: 7,
                bytes_scanned: 64,
                execution_time_in_millis: Some(600_000),
            },
            ..query_on_store(6)
        },
        // A generated query reports the natural-language prompt it came from.
        QueryRecord {
            status: QueryState::Finished,
            prompt: Some("Which users signed in last week?".to_string()),
            query_alias: Some("GeneratedQuery".to_string()),
            ..query_on_store(7)
        },
    ];

    // Every row's delivery status is one the model accepts.
    for record in &records {
        if let Some(delivery) = &record.delivery {
            assert!(
                DELIVERY_STATUS_VALUES.contains(&delivery.status.as_str()),
                "case {}: delivery status {:?} is not one of {DELIVERY_STATUS_VALUES:?}",
                record.query_id,
                delivery.status
            );
        }
        seed_query(&emulator, record);
    }

    for record in &records {
        let case = format!("QueryStatus {}", record.status.as_wire());
        // By query id alone, the way the deprecation of `EventDataStore` leaves
        // the request.
        let reply = emulator
            .call("DescribeQuery", &json!({ "QueryId": record.query_id }))
            .await;
        expect_reported(&case, reply.expect_ok(&case), record);

        // And with the event data store the query ran on, in both of the
        // identifier forms the model documents.
        for identifier in [
            STORE_ID.to_string(),
            event_data_store_arn(HOME_REGION, STORE_ID),
        ] {
            let case = format!("{case} via {identifier}");
            let reply = emulator
                .call(
                    "DescribeQuery",
                    &json!({ "QueryId": record.query_id, "EventDataStore": identifier }),
                )
                .await;
            expect_reported(&case, reply.expect_ok(&case), record);
        }
    }
}

/// `DescribeQuery` reports; it does not move the query it reports on. Describing
/// a queued run twice leaves it queued, and leaves the registry as it was.
#[tokio::test]
async fn describing_a_query_changes_nothing_for_the_next_caller() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    let record = QueryRecord {
        status: QueryState::Queued,
        ..query_on_store(1)
    };
    seed_query(&emulator, &record);

    for attempt in 1..=3 {
        let case = format!("DescribeQuery attempt {attempt}");
        let reply = emulator
            .call("DescribeQuery", &json!({ "QueryId": record.query_id }))
            .await;
        expect_reported(&case, reply.expect_ok(&case), &record);
    }

    let stored = emulator
        .state()
        .lock()
        .queries
        .get(&record.query_id)
        .map(|query| query.status);
    assert_eq!(
        stored,
        Some(QueryState::Queued),
        "case stored status: a read must not move the query, got {stored:?}"
    );
}

/// A `QueryAlias` names a query template: `DescribeQuery` reports the LAST run of
/// it, and `RefreshId` narrows that to one dashboard refresh.
#[tokio::test]
async fn describe_query_by_alias_reports_the_last_run_for_the_alias() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    let alias = "DailySignIns";
    // Three runs of one alias, seeded oldest LAST so "the last run" cannot come
    // from insertion order.
    let newest = QueryRecord {
        query_alias: Some(alias.to_string()),
        status: QueryState::Finished,
        created_at: Some(at(1_700_000_300)),
        ..query_on_store(3)
    };
    let middle = QueryRecord {
        query_alias: Some(alias.to_string()),
        status: QueryState::Failed,
        error_message: Some("The query could not be parsed.".to_string()),
        created_at: Some(at(1_700_000_200)),
        ..query_on_store(2)
    };
    let oldest = QueryRecord {
        query_alias: Some(alias.to_string()),
        status: QueryState::Cancelled,
        created_at: Some(at(1_700_000_100)),
        ..query_on_store(1)
    };
    // One run of the same alias that belongs to a dashboard refresh.
    let refreshed = QueryRecord {
        query_alias: Some(alias.to_string()),
        refresh_id: Some("1234567890".to_string()),
        status: QueryState::Finished,
        created_at: Some(at(1_700_000_150)),
        ..query_on_store(4)
    };
    for record in [&newest, &middle, &oldest, &refreshed] {
        seed_query(&emulator, record);
    }

    let reply = emulator
        .call("DescribeQuery", &json!({ "QueryAlias": alias }))
        .await;
    expect_reported("alias alone", reply.expect_ok("alias alone"), &newest);

    let reply = emulator
        .call(
            "DescribeQuery",
            &json!({ "QueryAlias": alias, "RefreshId": "1234567890" }),
        )
        .await;
    expect_reported(
        "alias with RefreshId",
        reply.expect_ok("alias with RefreshId"),
        &refreshed,
    );

    // A refresh nothing ran under, and an alias nothing ran under, resolve to no
    // query at all.
    for (case, body) in [
        (
            "alias with an unknown RefreshId",
            json!({ "QueryAlias": alias, "RefreshId": "9999999999" }),
        ),
        ("an unknown alias", json!({ "QueryAlias": "NeverRun" })),
    ] {
        emulator
            .call("DescribeQuery", &body)
            .await
            .expect_error(case, NOT_FOUND_STATUS, "QueryIdNotFoundException");
    }
}

/// The selector rule: "You must specify either QueryId or QueryAlias", and
/// `RefreshId` means nothing without an alias.
#[tokio::test]
async fn describe_query_refuses_a_request_that_selects_no_query() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    let record = QueryRecord {
        query_alias: Some("DailySignIns".to_string()),
        ..query_on_store(1)
    };
    seed_query(&emulator, &record);

    for (case, body) in [
        ("neither QueryId nor QueryAlias", json!({})),
        (
            "neither, with an event data store",
            json!({ "EventDataStore": STORE_ID }),
        ),
        (
            "both QueryId and QueryAlias",
            json!({ "QueryId": record.query_id, "QueryAlias": "DailySignIns" }),
        ),
        (
            "RefreshId without QueryAlias",
            json!({ "QueryId": record.query_id, "RefreshId": "1234567890" }),
        ),
    ] {
        emulator
            .call("DescribeQuery", &body)
            .await
            .expect_error(case, 400, "InvalidParameterException");
    }
}

/// A query is reachable only from the Region it ran in, and only through the
/// event data store it ran on.
#[tokio::test]
async fn describe_query_resolves_a_query_within_its_region_and_store() {
    let emulator = Emulator::new();
    let other_region = "eu-west-1";
    let other_store = "bbbbbbbb-1111-2222-3333-444444444444";
    let store_elsewhere = "dddddddd-1111-2222-3333-444444444444";
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    seed_store(
        &emulator,
        other_store,
        HOME_REGION,
        EventDataStoreState::Enabled,
    );
    // A query resolves inside its own Region's event data stores, so the other
    // Region needs one of its own.
    seed_store(
        &emulator,
        store_elsewhere,
        other_region,
        EventDataStoreState::Enabled,
    );

    let here = query_on_store(1);
    let elsewhere = QueryRecord {
        region: other_region.to_string(),
        ..query_on_store(2)
    };
    seed_query(&emulator, &here);
    seed_query(&emulator, &elsewhere);

    // Each query answers in its own Region...
    for (case, record, region) in [
        ("the home Region", &here, HOME_REGION),
        ("another Region", &elsewhere, other_region),
    ] {
        let reply = emulator
            .call_in("DescribeQuery", region, &json!({ "QueryId": record.query_id }))
            .await;
        expect_reported(case, reply.expect_ok(case), record);
    }

    // ...and nowhere else, and not through an event data store it did not run on.
    for (case, region, body) in [
        (
            "a query from another Region",
            HOME_REGION,
            json!({ "QueryId": elsewhere.query_id }),
        ),
        (
            "this Region's query from another Region",
            other_region,
            json!({ "QueryId": here.query_id }),
        ),
        (
            "a query through another event data store",
            HOME_REGION,
            json!({ "QueryId": here.query_id, "EventDataStore": other_store }),
        ),
        (
            "an unknown query id",
            HOME_REGION,
            json!({ "QueryId": query_id(9) }),
        ),
    ] {
        emulator
            .call_in("DescribeQuery", region, &body)
            .await
            .expect_error(case, NOT_FOUND_STATUS, "QueryIdNotFoundException");
    }
}

/// `EventDataStore` is "the ARN (or the ID suffix of the ARN) of an event data
/// store": every readable form resolves, an unreadable one is refused as an
/// invalid ARN, and one naming no store in the addressed Region is not found.
#[tokio::test]
async fn describe_query_resolves_the_event_data_store_it_is_given() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    let record = query_on_store(1);
    seed_query(&emulator, &record);

    // Readable identifiers for the store the query ran on.
    for identifier in [
        STORE_ID.to_string(),
        format!("eventdatastore/{STORE_ID}"),
        event_data_store_arn(HOME_REGION, STORE_ID),
    ] {
        let case = format!("EventDataStore {identifier}");
        let reply = emulator
            .call(
                "DescribeQuery",
                &json!({ "QueryId": record.query_id, "EventDataStore": identifier }),
            )
            .await;
        expect_reported(&case, reply.expect_ok(&case), &record);
    }

    // Readable, but naming no store in the Region it addresses.
    for identifier in [
        "cccccccc-1111-2222-3333-444444444444".to_string(),
        event_data_store_arn("eu-west-1", STORE_ID),
        event_data_store_arn(HOME_REGION, "cccccccc-1111-2222-3333-444444444444"),
    ] {
        let case = format!("unknown EventDataStore {identifier}");
        emulator
            .call(
                "DescribeQuery",
                &json!({ "QueryId": record.query_id, "EventDataStore": identifier }),
            )
            .await
            .expect_error(&case, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
    }

    // Unreadable: ARN-shaped, but not an event data store ARN.
    for identifier in [
        // A trail ARN, not an event data store ARN.
        format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/some-trail"),
        // Another service's ARN.
        "arn:aws:s3:::some-bucket".to_string(),
        // Too few ARN fields.
        format!("arn:aws:cloudtrail:{HOME_REGION}:eventdatastore/{STORE_ID}"),
        // No Region in the ARN.
        format!("arn:aws:cloudtrail::000000000000:eventdatastore/{STORE_ID}"),
        // The resource suffix is empty.
        format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:eventdatastore/"),
    ] {
        let case = format!("invalid EventDataStore {identifier}");
        emulator
            .call(
                "DescribeQuery",
                &json!({ "QueryId": record.query_id, "EventDataStore": identifier }),
            )
            .await
            .expect_error(&case, 400, "EventDataStoreARNInvalidException");
    }
}

/// A store serves queries in every status except the one that says it is on its
/// way out: `PENDING_DELETION` is what `InactiveEventDataStoreException` reports.
/// The rows are the model's whole `EventDataStoreStatus` catalog.
#[tokio::test]
async fn a_store_awaiting_deletion_does_not_serve_queries() {
    let emulator = Emulator::new();
    let record = query_on_store(1);
    seed_query(&emulator, &record);

    for status in [
        EventDataStoreState::Created,
        EventDataStoreState::Enabled,
        EventDataStoreState::StartingIngestion,
        EventDataStoreState::StoppingIngestion,
        EventDataStoreState::StoppedIngestion,
        EventDataStoreState::PendingDeletion,
    ] {
        seed_store(&emulator, STORE_ID, HOME_REGION, status);
        let case = format!("EventDataStoreStatus {}", status.as_wire());
        let reply = emulator
            .call(
                "DescribeQuery",
                &json!({ "QueryId": record.query_id, "EventDataStore": STORE_ID }),
            )
            .await;
        if status.is_queryable() {
            expect_reported(&case, reply.expect_ok(&case), &record);
        } else {
            reply.expect_error(&case, 400, "InactiveEventDataStoreException");
        }
    }
}

/// A query lives in an event data store, so the store resolves BEFORE the
/// selector: in a Region holding none, every selector — a query id naming a
/// persisted query included — is refused with `EventDataStoreNotFoundException`,
/// and the same calls reach the step that owns them once the Region holds one.
/// `EventDataStoreOwnerAccountId` is the account the store must belong to.
#[tokio::test]
async fn the_event_data_store_resolves_before_the_query_selector() {
    let emulator = Emulator::new();
    let record = QueryRecord {
        query_alias: Some("DailySignIns".to_string()),
        ..query_on_store(1)
    };
    seed_query(&emulator, &record);

    // No event data store in the Region: nothing could hold a query, whatever
    // the request selects.
    for (case, body) in [
        (
            "a persisted query id with no store",
            json!({ "QueryId": record.query_id }),
        ),
        (
            "an unknown query id with no store",
            json!({ "QueryId": query_id(9) }),
        ),
        (
            "a known alias with no store",
            json!({ "QueryAlias": "DailySignIns" }),
        ),
        (
            "an unknown alias with no store",
            json!({ "QueryAlias": "NeverRun" }),
        ),
        ("no selector with no store", json!({})),
    ] {
        emulator
            .call("DescribeQuery", &body)
            .await
            .expect_error(case, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
    }

    // With a store in the Region, each call reaches the step that owns it.
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    for (case, body) in [
        (
            "a persisted query id with a store",
            json!({ "QueryId": record.query_id }),
        ),
        (
            "a known alias with a store",
            json!({ "QueryAlias": "DailySignIns" }),
        ),
    ] {
        let reply = emulator.call("DescribeQuery", &body).await;
        expect_reported(case, reply.expect_ok(case), &record);
    }
    for (case, body) in [
        (
            "an unknown query id with a store",
            json!({ "QueryId": query_id(9) }),
        ),
        (
            "an unknown alias with a store",
            json!({ "QueryAlias": "NeverRun" }),
        ),
    ] {
        emulator
            .call("DescribeQuery", &body)
            .await
            .expect_error(case, NOT_FOUND_STATUS, "QueryIdNotFoundException");
    }
    emulator
        .call("DescribeQuery", &json!({}))
        .await
        .expect_error("no selector with a store", 400, "InvalidParameterException");

    // A cross-account request resolves the store in the account it names: the
    // emulator's own account owns this store, so another account owns none.
    let reply = emulator
        .call(
            "DescribeQuery",
            &json!({
                "QueryId": record.query_id,
                "EventDataStoreOwnerAccountId": engine::emulator_account_id(),
            }),
        )
        .await;
    expect_reported(
        "the owning account",
        reply.expect_ok("the owning account"),
        &record,
    );
    for (case, body) in [
        (
            "another account, store implied",
            json!({ "QueryId": record.query_id, "EventDataStoreOwnerAccountId": "210987654321" }),
        ),
        (
            "another account, store named",
            json!({
                "QueryId": record.query_id,
                "EventDataStore": STORE_ID,
                "EventDataStoreOwnerAccountId": "210987654321",
            }),
        ),
    ] {
        emulator
            .call("DescribeQuery", &body)
            .await
            .expect_error(case, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
    }

    // The store the query RAN on is the one the owner account has to own: a store
    // that account does own, beside a query that ran on another, is not enough.
    seed_store_owned_by(
        &emulator,
        "cccccccc-1111-2222-3333-444444444444",
        HOME_REGION,
        Some("210987654321"),
    );
    emulator
        .call(
            "DescribeQuery",
            &json!({
                "QueryId": record.query_id,
                "EventDataStoreOwnerAccountId": "210987654321",
            }),
        )
        .await
        .expect_error(
            "another account owning a different store",
            NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        );
}
