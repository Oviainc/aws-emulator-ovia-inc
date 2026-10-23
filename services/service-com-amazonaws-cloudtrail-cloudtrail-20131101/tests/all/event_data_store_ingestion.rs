// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StopEventDataStoreIngestion`.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness —
//! an `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the
//! production router serves — so what is asserted here is what a client sees,
//! including the HTTP status and the error code.
//!
//! The rules under test:
//!
//! * stopping ingestion on an event data store that is in a state to stop it moves
//!   the store off `ENABLED` into the transient `STOPPING_INGESTION`, which the
//!   clock settles into `STOPPED_INGESTION` after
//!   [`service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::INGESTION_SETTLE_AFTER`],
//!   and answers with the modelled — empty — `StopEventDataStoreIngestionResponse`;
//! * the identifier is "The ARN (or ID suffix of the ARN) of the event data store"
//!   (model doc for `StopEventDataStoreIngestionRequest$EventDataStore`), so every
//!   form addresses the same store — the ARN pinning the Region it names, a bare id
//!   the Region the call was made in;
//! * "To stop ingestion, the event data store `Status` must be `ENABLED` and the
//!   `eventCategory` must be `Management`, `Data`, `NetworkActivity`, or
//!   `ConfigurationItem`" (model doc for `StopEventDataStoreIngestion`), so a store
//!   collecting any other category is refused with
//!   `InvalidEventDataStoreCategoryException` and a store in any other status with
//!   `InvalidEventDataStoreStatusException` — including a store whose ingestion has
//!   already stopped, which is what makes the call NOT idempotent;
//! * an identifier readable as neither form is `EventDataStoreARNInvalidException`
//!   and one naming no store in the Region it addresses
//!   `EventDataStoreNotFoundException`;
//! * every refusal leaves the registry exactly as it was, and reading a store back
//!   changes nothing a later caller sees.
//!
//! The registry is seeded through the service's public `state` module because
//! `CreateEventDataStore` is still a scaffold stub (it answers 501), so the stores
//! these cases stop cannot yet be made over the wire. Every ingestion call, and
//! every read that proves what it persisted, goes through the router.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::{
    AdvancedEventSelector, AdvancedFieldSelector,
};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, EventDataStoreBillingMode, EventDataStoreRecord, EventDataStoreState,
    EVENT_CATEGORY_FIELD, INGESTION_CONTROLLABLE_EVENT_CATEGORIES, INGESTION_SETTLE_AFTER,
    INSIGHT_EVENT_CATEGORY,
};

use super::emulator::{Emulator, Reply, HOME_REGION};

/// A Region other than [`HOME_REGION`], for the cases about which Region an
/// identifier addresses a store in.
const OTHER_REGION: &str = "us-west-2";

/// The HTTP status this service answers a not-found refusal with: the model
/// annotates `EventDataStoreNotFoundException` `@httpError(404)`, and the recorded
/// CloudTrail response for an ingestion call naming an absent store is 400.
const NOT_FOUND_STATUS: u16 = 400;

/// The event categories whose ingestion can be stopped, as the model doc for
/// `StopEventDataStoreIngestion` names them: "the `eventCategory` must be
/// `Management`, `Data`, `NetworkActivity`, or `ConfigurationItem`". Written out
/// here so the service's own catalogue is asserted against the doc rather than
/// against itself.
const DOCUMENTED_STOPPABLE_CATEGORIES: [&str; 4] =
    ["Management", "Data", "NetworkActivity", "ConfigurationItem"];

/// A margin on top of [`INGESTION_SETTLE_AFTER`], so a case that waits for the
/// clock does not race the instant the handler read.
fn past_settle() -> Duration {
    INGESTION_SETTLE_AFTER + Duration::from_millis(250)
}

/// An `ENABLED` store under `id` in `region` collecting `categories` — what a store
/// looks like before anybody stops its ingestion.
///
/// Which categories a store collects is what its advanced event selectors state, on
/// the `eventCategory` field.
fn store_collecting(id: &str, region: &str, categories: &[&str]) -> EventDataStoreRecord {
    EventDataStoreRecord {
        id: id.to_string(),
        name: format!("lake-{id}"),
        region: region.to_string(),
        status: EventDataStoreState::Enabled,
        created_at: Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
        retention_period: Some(7),
        billing_mode: EventDataStoreBillingMode::ExtendableRetentionPricing,
        advanced_event_selectors: vec![AdvancedEventSelector {
            name: Some("categories".to_string()),
            field_selectors: vec![AdvancedFieldSelector {
                field: EVENT_CATEGORY_FIELD.to_string(),
                equals: Some(categories.iter().map(|c| (*c).to_string()).collect()),
                ..AdvancedFieldSelector::default()
            }],
        }],
        ..EventDataStoreRecord::default()
    }
}

/// Persist `record` in the registry every event data store operation reads, as
/// `CreateEventDataStore` will once it is implemented.
fn seed(emulator: &Emulator, record: &EventDataStoreRecord) {
    emulator
        .state()
        .lock()
        .event_data_stores
        .insert(record.id.clone(), record.clone());
}

/// Every stored store, rendered — for the cases that assert a refusal wrote
/// nothing.
fn registry(emulator: &Emulator) -> BTreeMap<String, String> {
    emulator
        .state()
        .lock()
        .event_data_stores
        .iter()
        .map(|(id, record)| (id.clone(), format!("{record:?}")))
        .collect()
}

/// The stored record under `id` — for the assertions about what a call persisted
/// that the wire does not carry (the instant the clock owes the transition at).
fn stored_store(emulator: &Emulator, id: &str) -> EventDataStoreRecord {
    emulator
        .state()
        .lock()
        .event_data_stores
        .get(id)
        .expect("the seeded store is in the registry")
        .clone()
}

/// Call `StopEventDataStoreIngestion` from `region`.
async fn stop_in(emulator: &Emulator, region: &str, identifier: &str) -> Reply {
    emulator
        .call_in(
            "StopEventDataStoreIngestion",
            region,
            &json!({ "EventDataStore": identifier }),
        )
        .await
}

/// Call `GetEventDataStore` from `region` — the read that proves what an ingestion
/// call persisted.
async fn get_in(emulator: &Emulator, region: &str, identifier: &str) -> Reply {
    emulator
        .call_in(
            "GetEventDataStore",
            region,
            &json!({ "EventDataStore": identifier }),
        )
        .await
}

/// A string member of a reply body.
fn member(body: &Value, name: &str) -> Option<String> {
    body.get(name).and_then(Value::as_str).map(str::to_string)
}

/// One row of the table a successful stop spans: the store to stop, the identifier
/// form it is addressed by and the Region the call is made in.
struct Row {
    case: &'static str,
    store: EventDataStoreRecord,
    /// Built from the stored store, so the row states the FORM, not a literal.
    identifier: fn(&EventDataStoreRecord) -> String,
    caller_region: &'static str,
}

/// The table of stores a successful stop spans: one row per documented stoppable
/// category, plus a store collecting several of them, differing along everything
/// the rule generalises over — which identifier form names the store, which Region
/// the call is made in, which Region the store lives in, and whether it had been
/// updated before.
fn rows() -> Vec<Row> {
    let management = store_collecting(
        "11111111-1111-4111-8111-111111111111",
        HOME_REGION,
        &[DOCUMENTED_STOPPABLE_CATEGORIES[0]],
    );
    let data = store_collecting(
        "22222222-2222-4222-8222-222222222222",
        HOME_REGION,
        &[DOCUMENTED_STOPPABLE_CATEGORIES[1]],
    );
    let network_activity = store_collecting(
        "33333333-3333-4333-8333-333333333333",
        HOME_REGION,
        &[DOCUMENTED_STOPPABLE_CATEGORIES[2]],
    );
    let mut configuration_item = store_collecting(
        "44444444-4444-4444-8444-444444444444",
        OTHER_REGION,
        &[DOCUMENTED_STOPPABLE_CATEGORIES[3]],
    );
    configuration_item.multi_region_enabled = true;
    let mut several = store_collecting(
        "55555555-5555-4555-8555-555555555555",
        OTHER_REGION,
        &DOCUMENTED_STOPPABLE_CATEGORIES[..2],
    );
    several.updated_at = Some(UNIX_EPOCH + Duration::from_secs(1_700_086_400));

    vec![
        Row {
            case: "a management store, by bare id, from its own Region",
            store: management,
            identifier: |store| store.id.clone(),
            caller_region: HOME_REGION,
        },
        Row {
            case: "a data store, by prefixed id",
            store: data,
            identifier: |store| format!("eventdatastore/{}", store.id),
            caller_region: HOME_REGION,
        },
        Row {
            case: "a network activity store, by ARN",
            store: network_activity,
            identifier: EventDataStoreRecord::arn,
            caller_region: HOME_REGION,
        },
        Row {
            case: "a configuration item store, by the ARN of another Region's store",
            store: configuration_item,
            identifier: EventDataStoreRecord::arn,
            caller_region: HOME_REGION,
        },
        Row {
            // A bare id names the store in the Region the CALL was made in, so a
            // store living outside the home Region is stopped by id from there.
            case: "a store collecting several categories, by bare id from its own \
                   non-home Region",
            store: several,
            identifier: |store| store.id.clone(),
            caller_region: OTHER_REGION,
        },
    ]
}

#[tokio::test]
async fn the_service_stops_ingestion_on_exactly_the_documented_categories() {
    assert_eq!(
        INGESTION_CONTROLLABLE_EVENT_CATEGORIES, DOCUMENTED_STOPPABLE_CATEGORIES,
        "case the documented catalogue: expected {DOCUMENTED_STOPPABLE_CATEGORIES:?}, got \
         {INGESTION_CONTROLLABLE_EVENT_CATEGORIES:?}"
    );
}

#[tokio::test]
async fn stopping_ingestion_moves_the_store_into_stopping_and_settles_it_stopped() {
    let emulator = Emulator::new();
    let rows = rows();
    for row in &rows {
        seed(&emulator, &row.store);
    }

    // Every row is stopped first, then the clock is given its span ONCE, so the
    // settled status is asserted for every row after a single wait.
    let started = Instant::now();
    let mut stopped = Vec::new();
    for row in &rows {
        let case = row.case;
        let identifier = (row.identifier)(&row.store);
        let arn = row.store.arn();

        let called_at = SystemTime::now();
        let reply = stop_in(&emulator, row.caller_region, &identifier).await;
        // `StopEventDataStoreIngestionResponse` models no members, so the answer
        // carries none: what the call did is read back below.
        assert_eq!(
            reply.expect_ok(case),
            &json!({}),
            "case {case:?}: expected the modelled empty response, got {:?}",
            reply.body
        );

        // The transition is owed to the CLOCK, at an instant strictly after the
        // call: that is what makes the transient status real for every caller,
        // however fast the machine. Asserted on the stored record, so it does not
        // depend on how long this test took.
        let stored = stored_store(&emulator, &row.store.id);
        assert_eq!(
            stored.status,
            EventDataStoreState::StoppingIngestion,
            "case {case:?}: the stored status must be the transient one, got {:?}",
            stored.status
        );
        let due = stored
            .ingestion_settles_at
            .unwrap_or_else(|| panic!("case {case:?}: a transient status owes a settle instant"));
        assert!(
            due > called_at,
            "case {case:?}: the settle instant must fall after the call that started \
             the transition, got {due:?} for a call at {called_at:?}"
        );

        // Read the store back through `GetEventDataStore`: what the call persisted
        // is what a later caller sees.
        let read = get_in(&emulator, &row.store.region, &arn).await;
        let read_body = read.expect_ok(case);
        if started.elapsed() < INGESTION_SETTLE_AFTER {
            assert_eq!(
                member(read_body, "Status").as_deref(),
                Some(EventDataStoreState::StoppingIngestion.as_wire()),
                "case {case:?}: within the settle span a read must still report the \
                 transient status, got {read_body:?}"
            );
        }
        // The store changed, so its update time moved past its creation time — and
        // past the time it last carried.
        let updated = read_body.get("UpdatedTimestamp").and_then(Value::as_f64);
        let created = read_body.get("CreatedTimestamp").and_then(Value::as_f64);
        assert!(
            updated > created,
            "case {case:?}: stopping ingestion must move the store's UpdatedTimestamp \
             past its CreatedTimestamp, got {read_body:?}"
        );

        stopped.push((case, row.store.region.clone(), arn));
    }

    // The clock owes `STOPPING_INGESTION` -> `STOPPED_INGESTION` at the settle
    // instant, whether or not anybody reads the store.
    sleep(past_settle());

    for (case, region, arn) in stopped {
        let read = get_in(&emulator, &region, &arn).await;
        let body = read.expect_ok(case).clone();
        assert_eq!(
            member(&body, "Status").as_deref(),
            Some(EventDataStoreState::StoppedIngestion.as_wire()),
            "case {case:?}: once the settle span has passed a read must report \
             STOPPED_INGESTION, got {body:?}"
        );
        // A settled status stays settled, and a read changes nothing a later caller
        // sees: the transition is materialised once.
        let again = get_in(&emulator, &region, &arn).await;
        assert_eq!(
            again.expect_ok(case),
            &body,
            "case {case:?}: a later read must answer what the settled read answered, \
             got {:?}",
            again.body
        );

        // Ingestion is stopped, so the store's status no longer supports stopping
        // it: the call is not idempotent.
        let twice = stop_in(&emulator, &region, &arn).await;
        twice.expect_error(case, 400, "InvalidEventDataStoreStatusException");
        assert_eq!(
            member(get_in(&emulator, &region, &arn).await.expect_ok(case), "Status").as_deref(),
            Some(EventDataStoreState::StoppedIngestion.as_wire()),
            "case {case:?}: a refused second stop must leave the store stopped"
        );
    }
}

#[tokio::test]
async fn a_store_whose_ingestion_cannot_be_stopped_is_refused_and_left_alone() {
    let emulator = Emulator::new();
    let future = SystemTime::now() + Duration::from_secs(60);
    let stoppable = DOCUMENTED_STOPPABLE_CATEGORIES[0];

    // One store per status the model's `EventDataStoreStatus` enum carries other
    // than `ENABLED`: none of them supports stopping ingestion.
    let mut created = store_collecting(
        "11111111-1111-4111-8111-111111111111",
        HOME_REGION,
        &[stoppable],
    );
    created.status = EventDataStoreState::Created;
    let mut pending_deletion = store_collecting(
        "22222222-2222-4222-8222-222222222222",
        HOME_REGION,
        &[stoppable],
    );
    pending_deletion.status = EventDataStoreState::PendingDeletion;
    let mut already_stopped = store_collecting(
        "33333333-3333-4333-8333-333333333333",
        HOME_REGION,
        &[stoppable],
    );
    already_stopped.status = EventDataStoreState::StoppedIngestion;
    let mut stopping = store_collecting(
        "44444444-4444-4444-8444-444444444444",
        HOME_REGION,
        &[stoppable],
    );
    stopping.status = EventDataStoreState::StoppingIngestion;
    stopping.ingestion_settles_at = Some(future);
    let mut starting = store_collecting(
        "55555555-5555-4555-8555-555555555555",
        HOME_REGION,
        &[stoppable],
    );
    starting.status = EventDataStoreState::StartingIngestion;
    starting.ingestion_settles_at = Some(future);

    // Stores whose event category is not one ingestion can be stopped on.
    let insights = store_collecting(
        "66666666-6666-4666-8666-666666666666",
        HOME_REGION,
        &[INSIGHT_EVENT_CATEGORY],
    );
    let audit_log = store_collecting(
        "77777777-7777-4777-8777-777777777777",
        HOME_REGION,
        &["ActivityAuditLog"],
    );
    let mut no_selectors = store_collecting(
        "88888888-8888-4888-8888-888888888888",
        HOME_REGION,
        &[stoppable],
    );
    no_selectors.advanced_event_selectors.clear();

    // A live, stoppable store in another Region, for the addressing cases.
    let elsewhere = store_collecting(
        "99999999-9999-4999-8999-999999999999",
        OTHER_REGION,
        &[stoppable],
    );
    let live = store_collecting(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        HOME_REGION,
        &[stoppable],
    );

    for store in [
        &created,
        &pending_deletion,
        &already_stopped,
        &stopping,
        &starting,
        &insights,
        &audit_log,
        &no_selectors,
        &elsewhere,
        &live,
    ] {
        seed(&emulator, store);
    }
    let before = registry(&emulator);

    let absent = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    // Each row names a condition ingestion cannot be stopped under, and the
    // modelled error it is refused with. The unreadable identifiers are values the
    // modelled `EventDataStoreArn` pattern and length bounds admit, so the boundary
    // validator hands them to the handler.
    let cases: Vec<(&str, &str, String, u16, &str)> = vec![
        (
            "an ARN of another resource type",
            HOME_REGION,
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/lake"),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "an ARN of another service",
            HOME_REGION,
            format!("arn:aws:logs:{HOME_REGION}:000000000000:eventdatastore/{absent}"),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "an ARN with no Region",
            HOME_REGION,
            format!("arn:aws:cloudtrail::000000000000:eventdatastore/{absent}"),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "the resource prefix alone",
            HOME_REGION,
            "eventdatastore/".to_string(),
            400,
            "EventDataStoreARNInvalidException",
        ),
        (
            "an absent id",
            HOME_REGION,
            absent.to_string(),
            NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "an absent ARN",
            HOME_REGION,
            event_data_store_arn(HOME_REGION, absent),
            NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "the bare id of a store living elsewhere",
            HOME_REGION,
            elsewhere.id.clone(),
            NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "the ARN of this Region's store, in another Region",
            OTHER_REGION,
            event_data_store_arn(OTHER_REGION, &live.id),
            NOT_FOUND_STATUS,
            "EventDataStoreNotFoundException",
        ),
        (
            "a store that has not been enabled yet",
            HOME_REGION,
            created.id.clone(),
            400,
            "InvalidEventDataStoreStatusException",
        ),
        (
            "a store awaiting deletion",
            HOME_REGION,
            pending_deletion.id.clone(),
            400,
            "InvalidEventDataStoreStatusException",
        ),
        (
            "a store whose ingestion has already stopped",
            HOME_REGION,
            already_stopped.id.clone(),
            400,
            "InvalidEventDataStoreStatusException",
        ),
        (
            "a store still stopping ingestion",
            HOME_REGION,
            stopping.id.clone(),
            400,
            "InvalidEventDataStoreStatusException",
        ),
        (
            "a store still starting ingestion",
            HOME_REGION,
            starting.id.clone(),
            400,
            "InvalidEventDataStoreStatusException",
        ),
        (
            "a store collecting Insights events",
            HOME_REGION,
            insights.id.clone(),
            400,
            "InvalidEventDataStoreCategoryException",
        ),
        (
            "a store collecting events from outside Amazon Web Services",
            HOME_REGION,
            audit_log.id.clone(),
            400,
            "InvalidEventDataStoreCategoryException",
        ),
        (
            "a store stating no event category at all",
            HOME_REGION,
            no_selectors.id.clone(),
            400,
            "InvalidEventDataStoreCategoryException",
        ),
    ];

    for (case, region, identifier, status, code) in cases {
        let reply = stop_in(&emulator, region, &identifier).await;
        reply.expect_error(case, status, code);
        // The refusal says which store it was about: the value itself when it names
        // no store at all, the ARN addressed otherwise.
        let named = if code == "EventDataStoreARNInvalidException" || identifier.contains(':') {
            identifier.clone()
        } else {
            event_data_store_arn(region, &identifier)
        };
        assert!(
            reply.message().contains(&named),
            "case {case:?}: the refusal must name {named:?}, got {:?}",
            reply.message()
        );
    }

    assert_eq!(
        registry(&emulator),
        before,
        "a refused stop must leave every stored store exactly as it was, got {:?}",
        registry(&emulator)
    );
}
