// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartEventDataStoreIngestion`.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness —
//! an `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the
//! production router serves — so what is asserted here is what a client sees,
//! including the HTTP status and the error code.
//!
//! The rules under test:
//!
//! * "To start ingestion, the event data store `Status` must be `STOPPED_INGESTION`
//!   and the `eventCategory` must be `Management`, `Data`, `NetworkActivity`, or
//!   `ConfigurationItem`" (model doc for `StartEventDataStoreIngestion`), so a store
//!   in that status and of one of those categories moves off `STOPPED_INGESTION`
//!   into the transient `STARTING_INGESTION`, which the clock settles into `ENABLED`
//!   after
//!   [`service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::INGESTION_SETTLE_AFTER`],
//!   and the call answers with the modelled — empty —
//!   `StartEventDataStoreIngestionResponse`;
//! * a store in any other status is refused with `InvalidEventDataStoreStatusException`
//!   — including one already ingesting, which is what makes the call NOT idempotent —
//!   and a store collecting any other category with
//!   `InvalidEventDataStoreCategoryException`;
//! * the identifier is "The ARN (or ID suffix of the ARN) of the event data store"
//!   (model doc for `StartEventDataStoreIngestionRequest$EventDataStore`), so every
//!   form addresses the same store — the ARN pinning the Region it names, a bare id
//!   the Region the call was made in — an identifier readable as neither form is
//!   `EventDataStoreARNInvalidException`, and one naming no store in the Region it
//!   addresses `EventDataStoreNotFoundException`;
//! * every refusal leaves the registry exactly as it was, and reading a store back
//!   changes nothing a later caller sees.
//!
//! The registry is seeded through the service's public `state` module because
//! `CreateEventDataStore` is still a scaffold stub (it answers 501), so the stores
//! these cases start cannot yet be made over the wire. Every ingestion call, and
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
const OTHER_REGION: &str = "eu-west-1";

/// The HTTP status this service answers a not-found refusal with: the model
/// annotates `EventDataStoreNotFoundException` `@httpError(404)`, and the recorded
/// CloudTrail response for an ingestion call naming an absent store is 400.
const NOT_FOUND_STATUS: u16 = 400;

/// The event categories whose ingestion can be started, as the model doc for
/// `StartEventDataStoreIngestion` names them: "the `eventCategory` must be
/// `Management`, `Data`, `NetworkActivity`, or `ConfigurationItem`". Written out
/// here so the service's own catalogue is asserted against the doc rather than
/// against itself.
const DOCUMENTED_STARTABLE_CATEGORIES: [&str; 4] =
    ["Management", "Data", "NetworkActivity", "ConfigurationItem"];

/// A margin on top of [`INGESTION_SETTLE_AFTER`], so a case that waits for the
/// clock does not race the instant the handler read.
fn past_settle() -> Duration {
    INGESTION_SETTLE_AFTER + Duration::from_millis(250)
}

/// A `STOPPED_INGESTION` store under `id` in `region` collecting `categories` —
/// what a store looks like before anybody starts its ingestion.
///
/// Which categories a store collects is what its advanced event selectors state, on
/// the `eventCategory` field.
fn stopped_store_collecting(id: &str, region: &str, categories: &[&str]) -> EventDataStoreRecord {
    EventDataStoreRecord {
        id: id.to_string(),
        name: format!("lake-{id}"),
        region: region.to_string(),
        status: EventDataStoreState::StoppedIngestion,
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

/// Call `StartEventDataStoreIngestion` from `region`.
async fn start_in(emulator: &Emulator, region: &str, identifier: &str) -> Reply {
    emulator
        .call_in(
            "StartEventDataStoreIngestion",
            region,
            &json!({ "EventDataStore": identifier }),
        )
        .await
}

/// Call `StopEventDataStoreIngestion` from `region` — the other half of the pair,
/// for the case that drives a whole stop/start round trip over the wire.
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

/// One row of the table a successful start spans: the store to start, the
/// identifier form it is addressed by and the Region the call is made in.
struct Row {
    case: &'static str,
    store: EventDataStoreRecord,
    /// Built from the stored store, so the row states the FORM, not a literal.
    identifier: fn(&EventDataStoreRecord) -> String,
    caller_region: &'static str,
}

/// The table of stores a successful start spans: one row per documented startable
/// category, plus a store collecting several of them, differing along everything
/// the rule generalises over — which identifier form names the store, which Region
/// the call is made in, which Region the store lives in, and whether it had been
/// updated before.
fn rows() -> Vec<Row> {
    let management = stopped_store_collecting(
        "11111111-1111-4111-8111-111111111111",
        HOME_REGION,
        &[DOCUMENTED_STARTABLE_CATEGORIES[0]],
    );
    let data = stopped_store_collecting(
        "22222222-2222-4222-8222-222222222222",
        HOME_REGION,
        &[DOCUMENTED_STARTABLE_CATEGORIES[1]],
    );
    let network_activity = stopped_store_collecting(
        "33333333-3333-4333-8333-333333333333",
        HOME_REGION,
        &[DOCUMENTED_STARTABLE_CATEGORIES[2]],
    );
    let mut configuration_item = stopped_store_collecting(
        "44444444-4444-4444-8444-444444444444",
        OTHER_REGION,
        &[DOCUMENTED_STARTABLE_CATEGORIES[3]],
    );
    configuration_item.multi_region_enabled = true;
    let mut several = stopped_store_collecting(
        "55555555-5555-4555-8555-555555555555",
        OTHER_REGION,
        &DOCUMENTED_STARTABLE_CATEGORIES[2..],
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
            // store living outside the home Region is started by id from there.
            case: "a store collecting several categories, by bare id from its own \
                   non-home Region",
            store: several,
            identifier: |store| store.id.clone(),
            caller_region: OTHER_REGION,
        },
    ]
}

#[tokio::test]
async fn the_service_starts_ingestion_on_exactly_the_documented_categories() {
    assert_eq!(
        INGESTION_CONTROLLABLE_EVENT_CATEGORIES, DOCUMENTED_STARTABLE_CATEGORIES,
        "case the documented catalogue: expected {DOCUMENTED_STARTABLE_CATEGORIES:?}, got \
         {INGESTION_CONTROLLABLE_EVENT_CATEGORIES:?}"
    );
}

#[tokio::test]
async fn starting_ingestion_moves_the_store_into_starting_and_settles_it_enabled() {
    let emulator = Emulator::new();
    let rows = rows();
    for row in &rows {
        seed(&emulator, &row.store);
    }

    // Every row is started first, then the clock is given its span ONCE, so the
    // settled status is asserted for every row after a single wait.
    let began = Instant::now();
    let mut started = Vec::new();
    for row in &rows {
        let case = row.case;
        let identifier = (row.identifier)(&row.store);
        let arn = row.store.arn();

        let called_at = SystemTime::now();
        let reply = start_in(&emulator, row.caller_region, &identifier).await;
        // `StartEventDataStoreIngestionResponse` models no members, so the answer
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
            EventDataStoreState::StartingIngestion,
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
        if began.elapsed() < INGESTION_SETTLE_AFTER {
            assert_eq!(
                member(read_body, "Status").as_deref(),
                Some(EventDataStoreState::StartingIngestion.as_wire()),
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
            "case {case:?}: starting ingestion must move the store's UpdatedTimestamp \
             past its CreatedTimestamp, got {read_body:?}"
        );

        started.push((case, row.store.region.clone(), arn));
    }

    // The clock owes `STARTING_INGESTION` -> `ENABLED` at the settle instant,
    // whether or not anybody reads the store.
    sleep(past_settle());

    for (case, region, arn) in started {
        let read = get_in(&emulator, &region, &arn).await;
        let body = read.expect_ok(case).clone();
        assert_eq!(
            member(&body, "Status").as_deref(),
            Some(EventDataStoreState::Enabled.as_wire()),
            "case {case:?}: once the settle span has passed a read must report \
             ENABLED, got {body:?}"
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

        // Ingestion is running, so the store's status no longer supports starting
        // it: the call is not idempotent.
        let twice = start_in(&emulator, &region, &arn).await;
        twice.expect_error(case, 400, "InvalidEventDataStoreStatusException");
        assert_eq!(
            member(get_in(&emulator, &region, &arn).await.expect_ok(case), "Status").as_deref(),
            Some(EventDataStoreState::Enabled.as_wire()),
            "case {case:?}: a refused second start must leave the store enabled"
        );
    }
}

#[tokio::test]
async fn a_store_whose_ingestion_cannot_be_started_is_refused_and_left_alone() {
    let emulator = Emulator::new();
    let future = SystemTime::now() + Duration::from_secs(60);
    let startable = DOCUMENTED_STARTABLE_CATEGORIES[1];

    // One store per status the model's `EventDataStoreStatus` enum carries other
    // than `STOPPED_INGESTION`: none of them supports starting ingestion.
    let mut created = stopped_store_collecting(
        "11111111-1111-4111-8111-111111111111",
        HOME_REGION,
        &[startable],
    );
    created.status = EventDataStoreState::Created;
    let mut enabled = stopped_store_collecting(
        "22222222-2222-4222-8222-222222222222",
        HOME_REGION,
        &[startable],
    );
    enabled.status = EventDataStoreState::Enabled;
    let mut pending_deletion = stopped_store_collecting(
        "33333333-3333-4333-8333-333333333333",
        HOME_REGION,
        &[startable],
    );
    pending_deletion.status = EventDataStoreState::PendingDeletion;
    let mut stopping = stopped_store_collecting(
        "44444444-4444-4444-8444-444444444444",
        HOME_REGION,
        &[startable],
    );
    stopping.status = EventDataStoreState::StoppingIngestion;
    stopping.ingestion_settles_at = Some(future);
    let mut starting = stopped_store_collecting(
        "55555555-5555-4555-8555-555555555555",
        HOME_REGION,
        &[startable],
    );
    starting.status = EventDataStoreState::StartingIngestion;
    starting.ingestion_settles_at = Some(future);

    // Stopped stores whose event category is not one ingestion can be started on.
    let insights = stopped_store_collecting(
        "66666666-6666-4666-8666-666666666666",
        HOME_REGION,
        &[INSIGHT_EVENT_CATEGORY],
    );
    let audit_log = stopped_store_collecting(
        "77777777-7777-4777-8777-777777777777",
        HOME_REGION,
        &["ActivityAuditLog"],
    );
    let mut no_selectors = stopped_store_collecting(
        "88888888-8888-4888-8888-888888888888",
        HOME_REGION,
        &[startable],
    );
    no_selectors.advanced_event_selectors.clear();

    // A stopped, startable store in another Region, for the addressing cases.
    let elsewhere = stopped_store_collecting(
        "99999999-9999-4999-8999-999999999999",
        OTHER_REGION,
        &[startable],
    );
    let here = stopped_store_collecting(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        HOME_REGION,
        &[startable],
    );

    for store in [
        &created,
        &enabled,
        &pending_deletion,
        &stopping,
        &starting,
        &insights,
        &audit_log,
        &no_selectors,
        &elsewhere,
        &here,
    ] {
        seed(&emulator, store);
    }
    let before = registry(&emulator);

    let absent = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
    // Each row names a condition ingestion cannot be started under, and the
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
            event_data_store_arn(OTHER_REGION, &here.id),
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
            "a store already ingesting",
            HOME_REGION,
            enabled.id.clone(),
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
        let reply = start_in(&emulator, region, &identifier).await;
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
        "a refused start must leave every stored store exactly as it was, got {:?}",
        registry(&emulator)
    );
}

#[tokio::test]
async fn ingestion_can_be_stopped_and_started_again_over_the_wire() {
    let emulator = Emulator::new();
    // A store that STARTS enabled, so the whole pause/resume cycle is driven by the
    // two ingestion operations rather than by the seed.
    let mut store = stopped_store_collecting(
        "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
        HOME_REGION,
        &DOCUMENTED_STARTABLE_CATEGORIES[..1],
    );
    store.status = EventDataStoreState::Enabled;
    seed(&emulator, &store);
    let arn = store.arn();
    let case = "a stop/start cycle on one management store";

    stop_in(&emulator, HOME_REGION, &arn).await.expect_ok(case);
    sleep(past_settle());
    assert_eq!(
        member(
            get_in(&emulator, HOME_REGION, &arn).await.expect_ok(case),
            "Status"
        )
        .as_deref(),
        Some(EventDataStoreState::StoppedIngestion.as_wire()),
        "case {case:?}: a settled stop must leave the store STOPPED_INGESTION"
    );

    start_in(&emulator, HOME_REGION, &arn).await.expect_ok(case);
    sleep(past_settle());
    assert_eq!(
        member(
            get_in(&emulator, HOME_REGION, &arn).await.expect_ok(case),
            "Status"
        )
        .as_deref(),
        Some(EventDataStoreState::Enabled.as_wire()),
        "case {case:?}: a settled start must leave the store ENABLED"
    );

    // The cycle can run again: the store is back in a status that supports
    // stopping, which is what makes the pair a cycle rather than a one-way move.
    stop_in(&emulator, HOME_REGION, &arn).await.expect_ok(case);
    sleep(past_settle());
    assert_eq!(
        member(
            get_in(&emulator, HOME_REGION, &arn).await.expect_ok(case),
            "Status"
        )
        .as_deref(),
        Some(EventDataStoreState::StoppedIngestion.as_wire()),
        "case {case:?}: the pair must be repeatable, got a store that cannot be \
         stopped again"
    );
}
