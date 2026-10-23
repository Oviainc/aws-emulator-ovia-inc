// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CancelQuery` — cancelling a CloudTrail Lake query.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * `CancelQuery` CANCELS a persisted query that has not reached a terminated
//!   status, for every such status: the response reports the stored `CANCELLED`
//!   status and the cancellation PERSISTS, so a second call is refused and
//!   `DescribeQuery` reports the query cancelled;
//! * a run already in a terminated state — the `QueryStatus` values
//!   `InactiveQueryException` names ("FINISHED, FAILED, TIMED_OUT, or CANCELLED") —
//!   cannot be cancelled, and the refusal leaves the run as it was;
//! * `EventDataStore` is "the ARN (or the ID suffix of the ARN) of an event data
//!   store": an unreadable one is `EventDataStoreARNInvalidException`, a readable
//!   one naming no store in the addressed Region is
//!   `EventDataStoreNotFoundException`, and a store awaiting deletion is
//!   `InactiveEventDataStoreException`;
//! * a query runs IN an event data store, so the store resolves BEFORE the query
//!   id: a Region — or the account `EventDataStoreOwnerAccountId` names — holding
//!   no store refuses the call with `EventDataStoreNotFoundException`, whatever
//!   query id it carries;
//! * a query is cancellable only from the Region it ran in and only through the
//!   event data store it ran on; any other query id is
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
//! assertion — the implemented `DescribeQuery` reads the result back.

use serde_json::{json, Value};
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::{
    EVENT_DATA_STORE_STATUS_VALUES, QUERY_STATUS_VALUES,
};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, EventDataStoreRecord, EventDataStoreState, QueryRecord, QueryState,
};

use super::emulator::{Emulator, HOME_REGION};

/// The id of the event data store most cases run their queries on. An event data
/// store id is the resource suffix of its ARN.
const STORE_ID: &str = "aaaaaaaa-5555-6666-7777-888888888888";

/// The HTTP status this service answers a not-found refusal with: the model
/// annotates `EventDataStoreNotFoundException` / `QueryIdNotFoundException`
/// `@httpError(404)`, and the recorded CloudTrail response is 400.
const NOT_FOUND_STATUS: u16 = 400;

/// A `UUID` is exactly 36 characters of `[a-f0-9-]`, so a query id is built to
/// that shape: the boundary validator refuses anything else before the handler
/// runs.
fn query_id(nth: u8) -> String {
    format!("{nth:02x}bbbbbb-5555-6666-7777-888888888888")
}

/// An instant, as a fixed offset from the epoch so creation times are ordered and
/// comparable.
fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

/// Seed one event data store, the way `CreateEventDataStore` will once it is
/// implemented.
fn seed_store(emulator: &Emulator, id: &str, region: &str, status: EventDataStoreState) {
    seed_store_owned_by(emulator, id, region, status, None);
}

/// Seed one event data store owned by `owner_account_id` (`None` = the emulator's
/// own account), for the cross-account cases.
fn seed_store_owned_by(
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
            // The members only a read of the store itself reports (its selectors,
            // retention, billing mode, federation) are not what these cases vary.
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

/// A query run on [`STORE_ID`] in [`HOME_REGION`], in `status`.
fn query_in(nth: u8, status: QueryState) -> QueryRecord {
    QueryRecord {
        query_id: query_id(nth),
        event_data_store_id: STORE_ID.to_string(),
        region: HOME_REGION.to_string(),
        query_string: format!("SELECT eventName FROM {STORE_ID} LIMIT {nth}"),
        status,
        created_at: Some(at(1_700_000_000 + u64::from(nth))),
        ..QueryRecord::default()
    }
}

/// The status the registry holds for `query_id`, as the wire spells it.
fn stored_status(emulator: &Emulator, query_id: &str) -> String {
    emulator
        .state()
        .lock()
        .queries
        .get(query_id)
        .map(|record| record.status.as_wire().to_string())
        .unwrap_or_default()
}

/// The `QueryStatus` of `query_id` as the implemented `DescribeQuery` reports it.
async fn described_status(emulator: &Emulator, case: &str, query_id: &str) -> String {
    let reply = emulator
        .call("DescribeQuery", &json!({ "QueryId": query_id }))
        .await;
    reply
        .expect_ok(case)
        .get("QueryStatus")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every `QueryStatus` the model publishes, split into the runs that can still be
/// cancelled and the terminated ones that cannot — the catalog is the model's own
/// (`ops::types::QUERY_STATUS_VALUES`), and `QueryState::is_terminal` is the rule
/// `InactiveQueryException` documents ("FINISHED, FAILED, TIMED_OUT, or
/// CANCELLED").
fn every_query_state() -> Vec<QueryState> {
    let states = vec![
        QueryState::Queued,
        QueryState::Running,
        QueryState::Finished,
        QueryState::Failed,
        QueryState::Cancelled,
        QueryState::TimedOut,
    ];
    let mut wire: Vec<&str> = states.iter().map(|state| state.as_wire()).collect();
    wire.sort_unstable();
    let mut published: Vec<&str> = QUERY_STATUS_VALUES.to_vec();
    published.sort_unstable();
    assert_eq!(
        wire, published,
        "the rows must span the model's whole QueryStatus catalog, got {wire:?}"
    );
    states
}

/// Every `EventDataStoreStatus` the model publishes, checked against the model's own
/// catalog (`ops::types::EVENT_DATA_STORE_STATUS_VALUES`) so a status added to the
/// model fails this test instead of going untested.
fn every_event_data_store_state() -> Vec<EventDataStoreState> {
    let states = vec![
        EventDataStoreState::Created,
        EventDataStoreState::Enabled,
        EventDataStoreState::StartingIngestion,
        EventDataStoreState::StoppingIngestion,
        EventDataStoreState::StoppedIngestion,
        EventDataStoreState::PendingDeletion,
    ];
    let mut wire: Vec<&str> = states.iter().map(|state| state.as_wire()).collect();
    wire.sort_unstable();
    let mut published: Vec<&str> = EVENT_DATA_STORE_STATUS_VALUES.to_vec();
    published.sort_unstable();
    assert_eq!(
        wire, published,
        "the rows must span the model's whole EventDataStoreStatus catalog, got {wire:?}"
    );
    states
}

/// RULE: a query that has not terminated is cancelled — the response reports the
/// stored `CANCELLED` status, and the cancellation PERSISTS, so the registry,
/// `DescribeQuery` and a second `CancelQuery` all agree afterwards.
///
/// The rows differ in the status cancelled from, in the form the event data store
/// is named in (full ARN, the `eventdatastore/` resource suffix, the bare id, or
/// not named at all), in whether the owner account is named, and in the Region.
#[tokio::test]
async fn a_query_that_has_not_terminated_is_cancelled_and_stays_cancelled() {
    let emulator = Emulator::new();
    let other_region = "eu-west-1";
    let store_elsewhere = "cccccccc-5555-6666-7777-888888888888";
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    seed_store(
        &emulator,
        store_elsewhere,
        other_region,
        EventDataStoreState::Enabled,
    );
    let owner = engine::emulator_account_id();

    // (case, the store identifier the request carries, whether it names the owner)
    let store_forms: Vec<(&str, Option<String>, bool)> = vec![
        ("the store by id", Some(STORE_ID.to_string()), false),
        (
            "the store by its resource suffix",
            Some(format!("eventdatastore/{STORE_ID}")),
            false,
        ),
        (
            "the store by ARN",
            Some(event_data_store_arn(HOME_REGION, STORE_ID)),
            false,
        ),
        (
            "the store by ARN with its owner account",
            Some(event_data_store_arn(HOME_REGION, STORE_ID)),
            true,
        ),
        ("no store at all", None, false),
        ("no store, only the owner account", None, true),
    ];

    let mut nth = 0_u8;
    for state in every_query_state()
        .into_iter()
        .filter(|state| !state.is_terminal())
    {
        for (form, identifier, names_owner) in &store_forms {
            nth += 1;
            let record = query_in(nth, state);
            seed_query(&emulator, &record);

            let mut request = json!({ "QueryId": record.query_id });
            if let Some(identifier) = identifier {
                request["EventDataStore"] = json!(identifier);
            }
            if *names_owner {
                request["EventDataStoreOwnerAccountId"] = json!(owner);
            }
            let case = format!("a {} query, naming {form}", state.as_wire());

            let reply = emulator.call("CancelQuery", &request).await;
            let body = reply.expect_ok(&case).clone();
            assert_eq!(
                body.get("QueryId").and_then(Value::as_str),
                Some(record.query_id.as_str()),
                "case {case:?}: the response must name the query cancelled, got {body:?}"
            );
            assert_eq!(
                body.get("QueryStatus").and_then(Value::as_str),
                Some(QueryState::Cancelled.as_wire()),
                "case {case:?}: a cancelled query reports CANCELLED, got {body:?}"
            );
            // The store is this account's own, so there is no cross-account owner
            // to report back.
            assert!(
                body.get("EventDataStoreOwnerAccountId").is_none(),
                "case {case:?}: a query on this account's own store reports no owner \
                 account, got {body:?}"
            );

            // The cancellation is PERSISTED, not just reported: the registry, the
            // implemented reader and a second cancel all agree.
            assert_eq!(
                stored_status(&emulator, &record.query_id),
                QueryState::Cancelled.as_wire(),
                "case {case:?}: the registry must hold the cancelled status"
            );
            assert_eq!(
                described_status(&emulator, &case, &record.query_id).await,
                QueryState::Cancelled.as_wire(),
                "case {case:?}: DescribeQuery must report the query cancelled"
            );
            emulator
                .call("CancelQuery", &request)
                .await
                .expect_error(&format!("{case}, cancelled twice"), 400, "InactiveQueryException");
        }
    }

    // A query is cancelled in the Region it ran in, through that Region's store.
    let elsewhere = QueryRecord {
        region: other_region.to_string(),
        event_data_store_id: store_elsewhere.to_string(),
        ..query_in(90, QueryState::Running)
    };
    seed_query(&emulator, &elsewhere);
    let case = "a running query in another Region";
    let reply = emulator
        .call_in(
            "CancelQuery",
            other_region,
            &json!({ "QueryId": elsewhere.query_id, "EventDataStore": store_elsewhere }),
        )
        .await;
    assert_eq!(
        reply.expect_ok(case).get("QueryStatus").and_then(Value::as_str),
        Some(QueryState::Cancelled.as_wire()),
        "case {case:?}: a query cancels in its own Region, got {reply:?}"
    );
}

/// RULE: a run that has reached a terminated status cannot be cancelled, and the
/// refusal leaves it exactly as it was. The rows are the model's whole
/// `QueryStatus` catalog, split by [`QueryState::is_terminal`].
#[tokio::test]
async fn a_terminated_query_cannot_be_cancelled_and_is_left_alone() {
    let emulator = Emulator::new();
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    for (nth, state) in every_query_state().into_iter().enumerate() {
        let record = query_in(nth as u8, state);
        seed_query(&emulator, &record);
        let case = format!("a {} query", state.as_wire());

        let reply = emulator
            .call(
                "CancelQuery",
                &json!({ "QueryId": record.query_id, "EventDataStore": STORE_ID }),
            )
            .await;
        if state.is_terminal() {
            reply.expect_error(&case, 400, "InactiveQueryException");
            assert_eq!(
                stored_status(&emulator, &record.query_id),
                state.as_wire(),
                "case {case:?}: a refused cancellation must leave the stored status alone"
            );
        } else {
            reply.expect_ok(&case);
            assert_eq!(
                stored_status(&emulator, &record.query_id),
                QueryState::Cancelled.as_wire(),
                "case {case:?}: a query that had not terminated must be cancelled"
            );
        }
    }
}

/// RULE: a query is cancellable only from the Region it ran in and only through the
/// event data store it ran on; every other query id is `QueryIdNotFoundException`,
/// and nothing is cancelled.
#[tokio::test]
async fn a_query_id_that_resolves_to_no_query_here_is_not_found() {
    let emulator = Emulator::new();
    let other_region = "eu-west-1";
    let other_store = "dddddddd-5555-6666-7777-888888888888";
    let store_elsewhere = "eeeeeeee-5555-6666-7777-888888888888";
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    seed_store(
        &emulator,
        other_store,
        HOME_REGION,
        EventDataStoreState::Enabled,
    );
    seed_store(
        &emulator,
        store_elsewhere,
        other_region,
        EventDataStoreState::Enabled,
    );

    let here = query_in(1, QueryState::Running);
    let elsewhere = QueryRecord {
        region: other_region.to_string(),
        event_data_store_id: store_elsewhere.to_string(),
        ..query_in(2, QueryState::Running)
    };
    seed_query(&emulator, &here);
    seed_query(&emulator, &elsewhere);

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
            .call_in("CancelQuery", region, &body)
            .await
            .expect_error(case, NOT_FOUND_STATUS, "QueryIdNotFoundException");
    }

    // Neither running query the refusals passed over was touched.
    for (case, record) in [("here", &here), ("elsewhere", &elsewhere)] {
        assert_eq!(
            stored_status(&emulator, &record.query_id),
            QueryState::Running.as_wire(),
            "case {case:?}: a refused cancellation must leave the query running"
        );
    }
}

/// RULE: `EventDataStore` is "the ARN (or the ID suffix of the ARN) of an event
/// data store": an unreadable value is refused as an invalid ARN, and a readable
/// one naming no store in the Region it addresses is not found — before the query
/// id is read, so a request naming a persisted query is refused all the same.
#[tokio::test]
async fn the_event_data_store_resolves_before_the_query_id() {
    let emulator = Emulator::new();
    let record = query_in(1, QueryState::Running);
    seed_query(&emulator, &record);

    // No event data store in the Region at all: nothing could hold the query.
    for (case, body) in [
        (
            "a persisted query id with no store in the Region",
            json!({ "QueryId": record.query_id }),
        ),
        (
            "an unknown query id with no store in the Region",
            json!({ "QueryId": query_id(9) }),
        ),
    ] {
        emulator
            .call("CancelQuery", &body)
            .await
            .expect_error(case, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
    }

    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);

    // Readable, but naming no store in the Region it addresses.
    for identifier in [
        "ffffffff-5555-6666-7777-888888888888".to_string(),
        event_data_store_arn("eu-west-1", STORE_ID),
        event_data_store_arn(HOME_REGION, "ffffffff-5555-6666-7777-888888888888"),
    ] {
        let case = format!("unknown EventDataStore {identifier}");
        emulator
            .call(
                "CancelQuery",
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
        // No account in the ARN.
        format!("arn:aws:cloudtrail:{HOME_REGION}::eventdatastore/{STORE_ID}"),
        // The resource suffix is empty.
        format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:eventdatastore/"),
    ] {
        let case = format!("invalid EventDataStore {identifier}");
        emulator
            .call(
                "CancelQuery",
                &json!({ "QueryId": record.query_id, "EventDataStore": identifier }),
            )
            .await
            .expect_error(&case, 400, "EventDataStoreARNInvalidException");
    }

    // Every refusal above left the running query alone.
    assert_eq!(
        stored_status(&emulator, &record.query_id),
        QueryState::Running.as_wire(),
        "a refused cancellation must leave the query running"
    );

    // A query whose own store the registry no longer holds resolves no store either,
    // even though the Region holds one.
    let orphaned = QueryRecord {
        event_data_store_id: "99999999-5555-6666-7777-888888888888".to_string(),
        ..query_in(5, QueryState::Running)
    };
    seed_query(&emulator, &orphaned);
    let case = "a query whose event data store is no longer in the registry";
    emulator
        .call("CancelQuery", &json!({ "QueryId": orphaned.query_id }))
        .await
        .expect_error(case, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
    assert_eq!(
        stored_status(&emulator, &orphaned.query_id),
        QueryState::Running.as_wire(),
        "case {case:?}: a refused cancellation must leave the query running"
    );
}

/// RULE: `EventDataStoreOwnerAccountId` names the account the store belongs to, so
/// a call naming an account that owns no store holding the query resolves no store
/// at all — and a query on a store shared from another account cancels and reports
/// that owner back.
#[tokio::test]
async fn the_owner_account_named_has_to_own_the_store_the_query_ran_on() {
    let emulator = Emulator::new();
    let shared_store = "11111111-5555-6666-7777-888888888888";
    let other_account = "222233334444";
    seed_store(&emulator, STORE_ID, HOME_REGION, EventDataStoreState::Enabled);
    seed_store_owned_by(
        &emulator,
        shared_store,
        HOME_REGION,
        EventDataStoreState::Enabled,
        Some(other_account),
    );

    let own = query_in(1, QueryState::Running);
    let shared = QueryRecord {
        event_data_store_id: shared_store.to_string(),
        event_data_store_owner_account_id: Some(other_account.to_string()),
        ..query_in(2, QueryState::Running)
    };
    seed_query(&emulator, &own);
    seed_query(&emulator, &shared);

    // An owner account that owns no store holding this query resolves no store.
    for (case, body) in [
        (
            "an account owning no store, with the store named",
            json!({
                "QueryId": own.query_id,
                "EventDataStore": STORE_ID,
                "EventDataStoreOwnerAccountId": "999999999999",
            }),
        ),
        (
            "an account owning no store, with no store named",
            json!({
                "QueryId": own.query_id,
                "EventDataStoreOwnerAccountId": "999999999999",
            }),
        ),
        (
            "an account whose store did not run this query",
            json!({
                "QueryId": own.query_id,
                "EventDataStoreOwnerAccountId": other_account,
            }),
        ),
    ] {
        emulator
            .call("CancelQuery", &body)
            .await
            .expect_error(case, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
    }
    assert_eq!(
        stored_status(&emulator, &own.query_id),
        QueryState::Running.as_wire(),
        "a refused cancellation must leave the query running"
    );

    // The query that DID run on the shared store cancels, and reports the owner
    // the record carries.
    let case = "a query on a store shared from another account";
    let reply = emulator
        .call(
            "CancelQuery",
            &json!({
                "QueryId": shared.query_id,
                "EventDataStore": shared_store,
                "EventDataStoreOwnerAccountId": other_account,
            }),
        )
        .await;
    let body = reply.expect_ok(case).clone();
    assert_eq!(
        (
            body.get("QueryStatus").and_then(Value::as_str),
            body.get("EventDataStoreOwnerAccountId").and_then(Value::as_str),
        ),
        (
            Some(QueryState::Cancelled.as_wire()),
            Some(other_account)
        ),
        "case {case:?}: expected a cancelled cross-account query owned by {other_account}, \
         got {body:?}"
    );
}

/// RULE: a store awaiting deletion serves no query, so a query on it cannot be
/// cancelled — whether the request names the store or leaves the query id to name
/// it. The rows are the model's whole `EventDataStoreStatus` catalog.
#[tokio::test]
async fn a_query_on_a_store_awaiting_deletion_cannot_be_cancelled() {
    let emulator = Emulator::new();

    for (nth, status) in every_event_data_store_state().into_iter().enumerate() {
        seed_store(&emulator, STORE_ID, HOME_REGION, status);
        for (named, identifier) in [("by id", Some(STORE_ID)), ("by the query alone", None)] {
            let record = query_in(10 * nth as u8 + u8::from(identifier.is_none()), QueryState::Queued);
            seed_query(&emulator, &record);
            let case = format!(
                "a queued query on a {} store, named {named}",
                status.as_wire()
            );

            let mut request = json!({ "QueryId": record.query_id });
            if let Some(identifier) = identifier {
                request["EventDataStore"] = json!(identifier);
            }
            let reply = emulator.call("CancelQuery", &request).await;
            if status.is_queryable() {
                reply.expect_ok(&case);
            } else {
                reply.expect_error(&case, 400, "InactiveEventDataStoreException");
                assert_eq!(
                    stored_status(&emulator, &record.query_id),
                    QueryState::Queued.as_wire(),
                    "case {case:?}: a refused cancellation must leave the query queued"
                );
            }
        }
    }
}
