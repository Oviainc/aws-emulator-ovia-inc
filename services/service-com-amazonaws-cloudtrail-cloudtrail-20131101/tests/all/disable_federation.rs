// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DisableFederation`.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness —
//! an `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the
//! production router serves — so what is asserted here is what a client sees,
//! including the HTTP status and the error code.
//!
//! The rules under test:
//!
//! * `DisableFederation` computes its answer from the event data store the
//!   identifier addresses: the store's own ARN and the federation status the store
//!   carries after the call (model: `DisableFederationResponse`);
//! * a store whose federation is on (`ENABLED`, or an `ENABLING` that has not
//!   settled) moves to `DISABLING` and the clock settles it into `DISABLED`,
//!   releasing the federation role, which is held only while federation is on —
//!   `FederationStatus` carries both in-flight values precisely because the call
//!   is asynchronous;
//! * a store whose federation is already off, or already winding down, is left
//!   exactly as it is: the model declares no "federation not enabled" error for
//!   this operation, so the call is idempotent;
//! * the identifier is "The ARN (or ID suffix of the ARN) of the event data store"
//!   (model doc for `DisableFederationRequest$EventDataStore`), so every form
//!   addresses the same store and answers identically, the ARN pinning the Region
//!   it names and a bare id the Region the call was made in;
//! * a readable identifier naming no store in the Region it addresses is refused
//!   with `EventDataStoreNotFoundException`, whose message names the ARN
//!   addressed; one that cannot be read as either form with
//!   `EventDataStoreARNInvalidException`, whose message is "The event data store
//!   ARN <arn> format is not valid." naming the ARN the request addressed — the
//!   value as given when it is ARN-shaped, and the ARN a bare id completes to in
//!   the calling Region; and a store that is being torn down with
//!   `InactiveEventDataStoreException`.
//!
//! The registry is seeded through the service's public `state` module because
//! `CreateEventDataStore` and `EnableFederation` are still scaffold stubs (they
//! answer 501), so a federated store cannot yet be made over the wire. The disable
//! call and the read-back are exercised only through the router.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, EventDataStoreRecord, EventDataStoreState, FederationState,
    FEDERATION_SETTLE_AFTER,
};

use super::emulator::{Emulator, Reply, HOME_REGION};

/// A Region other than [`HOME_REGION`], for the cases about which Region an
/// identifier addresses a store in.
const OTHER_REGION: &str = "us-west-2";

/// The HTTP status this service answers a not-found refusal with: the model
/// annotates `EventDataStoreNotFoundException` `@httpError(404)`, and the recorded
/// CloudTrail response is 400.
const NOT_FOUND_STATUS: u16 = 400;

/// A role ARN a federated store runs under, for the rows that carry one.
const FEDERATION_ROLE: &str = "arn:aws:iam::000000000000:role/lake-federation";

/// An instant, as a fixed offset from the epoch so stored times are comparable.
fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// A store record under `id` in `region` whose federation is in `federation`.
fn record(
    id: &str,
    region: &str,
    status: EventDataStoreState,
    federation: FederationState,
) -> EventDataStoreRecord {
    EventDataStoreRecord {
        id: id.to_string(),
        name: format!("lake-{id}"),
        region: region.to_string(),
        status,
        created_at: Some(at(1_700_000_000)),
        federation_status: federation,
        federation_role_arn: federation.is_on().then(|| FEDERATION_ROLE.to_string()),
        // An in-flight federation state carries the instant it is due to settle;
        // a row that is already in flight is given one far in the future so the
        // call under test cannot be mistaken for the clock.
        federation_settles_at: (federation.settled().is_some())
            .then(|| SystemTime::now() + Duration::from_secs(3_600)),
        ..EventDataStoreRecord::default()
    }
}

/// One row of the table: a stored store, and what `DisableFederation` must answer
/// for it.
struct Row {
    case: &'static str,
    record: EventDataStoreRecord,
    /// The `FederationStatus` the call answers with, immediately.
    answered: FederationState,
    /// Whether the call starts a wind-down the clock then settles.
    winds_down: bool,
}

/// The table: the rows differ along the federation state the store is in (each of
/// the four `FederationStatus` values), the store's own lifecycle status, the
/// Region it lives in, and whether it holds a federation role.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "federation ENABLED",
            record: record(
                "11111111-1111-4111-8111-111111111111",
                HOME_REGION,
                EventDataStoreState::Enabled,
                FederationState::Enabled,
            ),
            answered: FederationState::Disabling,
            winds_down: true,
        },
        Row {
            case: "federation ENABLING, store still CREATED",
            record: record(
                "22222222-2222-4222-8222-222222222222",
                HOME_REGION,
                EventDataStoreState::Created,
                FederationState::Enabling,
            ),
            answered: FederationState::Disabling,
            winds_down: true,
        },
        Row {
            case: "federation ENABLED, store STOPPED_INGESTION in another Region",
            record: record(
                "33333333-3333-4333-8333-333333333333",
                OTHER_REGION,
                EventDataStoreState::StoppedIngestion,
                FederationState::Enabled,
            ),
            answered: FederationState::Disabling,
            winds_down: true,
        },
        Row {
            case: "federation already DISABLED",
            record: record(
                "44444444-4444-4444-8444-444444444444",
                HOME_REGION,
                EventDataStoreState::Enabled,
                FederationState::Disabled,
            ),
            answered: FederationState::Disabled,
            winds_down: false,
        },
        Row {
            case: "federation already DISABLING",
            record: record(
                "55555555-5555-4555-8555-555555555555",
                HOME_REGION,
                EventDataStoreState::StartingIngestion,
                FederationState::Disabling,
            ),
            answered: FederationState::Disabling,
            winds_down: false,
        },
    ]
}

/// Persist `record` in the registry every event data store operation reads, as
/// `CreateEventDataStore` and `EnableFederation` will once they are implemented.
fn seed(emulator: &Emulator, record: &EventDataStoreRecord) {
    emulator
        .state()
        .lock()
        .event_data_stores
        .insert(record.id.clone(), record.clone());
}

/// The stored record under `id`.
fn stored(emulator: &Emulator, id: &str) -> EventDataStoreRecord {
    emulator
        .state()
        .lock()
        .event_data_stores
        .get(id)
        .cloned()
        .expect("the seeded store is in the registry")
}

/// Every stored store, rendered — for the cases that assert a refused call wrote
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

/// Call `DisableFederation` from `region` for `identifier`.
async fn disable_in(emulator: &Emulator, region: &str, identifier: &str) -> Reply {
    emulator
        .call_in(
            "DisableFederation",
            region,
            &json!({ "EventDataStore": identifier }),
        )
        .await
}

/// Read the store back through `GetEventDataStore` — the round trip that proves
/// the call persisted what it answered.
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
fn member(reply: &Reply, name: &str) -> Option<String> {
    reply
        .body
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The error `Message` a reply carries, or `""`.
fn message(reply: &Reply) -> String {
    reply
        .body
        .get("Message")
        .or_else(|| reply.body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn disabling_federation_answers_the_stored_store_and_its_federation_status() {
    for row in rows() {
        let emulator = Emulator::new();
        seed(&emulator, &row.record);
        let arn = event_data_store_arn(&row.record.region, &row.record.id);
        let case = row.case;

        let reply = disable_in(&emulator, &row.record.region, &arn).await;
        let body = reply.expect_ok(case);
        assert_eq!(
            member(&reply, "EventDataStoreArn").as_deref(),
            Some(arn.as_str()),
            "case {case:?}: expected the addressed store's ARN {arn:?}, got {body:?}"
        );
        assert_eq!(
            member(&reply, "FederationStatus").as_deref(),
            Some(row.answered.as_wire()),
            "case {case:?}: expected FederationStatus {:?}, got {body:?}",
            row.answered.as_wire()
        );

        // The round trip: the status the call answered with is the one the store
        // now persists, as an implemented reader reports it.
        let read = get_in(&emulator, &row.record.region, &arn).await;
        let read_body = read.expect_ok(case);
        assert_eq!(
            member(&read, "FederationStatus").as_deref(),
            Some(row.answered.as_wire()),
            "case {case:?}: a read must report the status the call persisted ({:?}), \
             got {read_body:?}",
            row.answered.as_wire()
        );

        let after = stored(&emulator, &row.record.id);
        if row.winds_down {
            // A wind-down this call started is due to settle at the span this
            // service publishes, not sooner and not at once.
            let due = after
                .federation_settles_at
                .expect("a store winding down carries the instant it settles");
            assert!(
                due > SystemTime::now(),
                "case {case:?}: the DISABLING state must still be ahead of its settle \
                 instant, got {after:?}"
            );
            assert!(
                due <= SystemTime::now() + FEDERATION_SETTLE_AFTER,
                "case {case:?}: the wind-down must be due within {FEDERATION_SETTLE_AFTER:?}, \
                 got {after:?}"
            );
            assert!(
                after.updated_at.is_some(),
                "case {case:?}: a call that changed the store must stamp UpdatedTimestamp, \
                 got {after:?}"
            );
        } else {
            // Nothing to turn off: the call wrote nothing at all.
            assert_eq!(
                format!("{after:?}"),
                format!("{:?}", row.record),
                "case {case:?}: a store whose federation is already off must be left as it \
                 was, got {after:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_wind_down_settles_into_disabled_and_releases_the_federation_role() {
    for row in rows().into_iter().filter(|row| row.winds_down) {
        let emulator = Emulator::new();
        seed(&emulator, &row.record);
        let arn = event_data_store_arn(&row.record.region, &row.record.id);
        let case = row.case;

        let reply = disable_in(&emulator, &row.record.region, &arn).await;
        reply.expect_ok(case);

        // Before the span is up the caller still sees the transient state, and the
        // role federation ran under is still held.
        let during = get_in(&emulator, &row.record.region, &arn).await;
        let during_body = during.expect_ok(case);
        assert_eq!(
            member(&during, "FederationStatus").as_deref(),
            Some(FederationState::Disabling.as_wire()),
            "case {case:?}: a read taken straight after the call must see DISABLING, \
             got {during_body:?}"
        );
        assert_eq!(
            member(&during, "FederationRoleArn").as_deref(),
            Some(FEDERATION_ROLE),
            "case {case:?}: the federation role is still held while the store winds down, \
             got {during_body:?}"
        );

        // Past the span, the transition the clock owed is what every later caller
        // sees: DISABLED, with no role left.
        std::thread::sleep(FEDERATION_SETTLE_AFTER + Duration::from_millis(100));
        let settled = get_in(&emulator, &row.record.region, &arn).await;
        let settled_body = settled.expect_ok(case);
        assert_eq!(
            member(&settled, "FederationStatus").as_deref(),
            Some(FederationState::Disabled.as_wire()),
            "case {case:?}: past the settle span a read must see DISABLED, \
             got {settled_body:?}"
        );
        assert_eq!(
            member(&settled, "FederationRoleArn"),
            None,
            "case {case:?}: a store whose federation is off holds no federation role, \
             got {settled_body:?}"
        );

        // And the call is idempotent once it has settled: the same answer, and
        // nothing further to wind down.
        let again = disable_in(&emulator, &row.record.region, &arn).await;
        let again_body = again.expect_ok(case);
        assert_eq!(
            member(&again, "FederationStatus").as_deref(),
            Some(FederationState::Disabled.as_wire()),
            "case {case:?}: disabling an already-disabled store answers DISABLED, \
             got {again_body:?}"
        );
        let after = stored(&emulator, &row.record.id);
        assert_eq!(
            after.federation_settles_at, None,
            "case {case:?}: a settled store owes the clock nothing, got {after:?}"
        );
    }
}

#[tokio::test]
async fn every_identifier_form_addresses_the_same_store() {
    let seeded = record(
        "66666666-6666-4666-8666-666666666666",
        OTHER_REGION,
        EventDataStoreState::Enabled,
        FederationState::Enabled,
    );
    let arn = event_data_store_arn(&seeded.region, &seeded.id);

    // The ARN pins the Region the store lives in, so it addresses the store from
    // any Region a caller signs for; the bare id — with or without the resource
    // prefix the ARN carries — names it in the store's own Region.
    let forms = [
        ("by ARN, from its own Region", seeded.region.clone(), arn.clone()),
        ("by ARN, from another Region", HOME_REGION.to_string(), arn.clone()),
        ("by id", seeded.region.clone(), seeded.id.clone()),
        (
            "by prefixed id",
            seeded.region.clone(),
            format!("eventdatastore/{}", seeded.id),
        ),
    ];

    let mut answers: Vec<(String, Value)> = Vec::new();
    for (form, caller_region, identifier) in forms {
        // A fresh emulator per form: the first call is the one that winds the
        // store down, so each form must be the first to see it.
        let emulator = Emulator::new();
        seed(&emulator, &seeded);
        let reply = disable_in(&emulator, &caller_region, &identifier).await;
        let body = reply.expect_ok(form);
        assert_eq!(
            member(&reply, "EventDataStoreArn").as_deref(),
            Some(arn.as_str()),
            "case {form:?}: expected the addressed store's ARN {arn:?}, got {body:?}"
        );
        let after = stored(&emulator, &seeded.id);
        assert_eq!(
            after.federation_status,
            FederationState::Disabling,
            "case {form:?}: every identifier form addresses the same store, so each must \
             wind its federation down, got {after:?}"
        );
        answers.push((form.to_string(), reply.body));
    }

    let (first_case, first) = &answers[0];
    for (case, body) in &answers[1..] {
        assert_eq!(
            body, first,
            "case {case:?}: every identifier form addresses one store, so the answer must \
             equal the one {first_case:?} got, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_readable_identifier_naming_no_store_is_not_found() {
    let emulator = Emulator::new();
    let rows = rows();
    for row in &rows {
        seed(&emulator, &row.record);
    }
    let before = registry(&emulator);
    let absent_id = "77777777-7777-4777-8777-777777777777";
    let elsewhere = &rows[2].record;
    let home = &rows[0].record;

    // An id and an ARN no store carries, and a store addressed from a Region it
    // does not live in: none of them names a store the registry holds.
    let cases = [
        ("absent id", HOME_REGION, absent_id.to_string()),
        (
            "absent ARN",
            HOME_REGION,
            event_data_store_arn(HOME_REGION, absent_id),
        ),
        (
            "ARN of another Region's store id, in this Region",
            HOME_REGION,
            event_data_store_arn(HOME_REGION, &elsewhere.id),
        ),
        (
            "bare id of a store living elsewhere",
            HOME_REGION,
            elsewhere.id.clone(),
        ),
        (
            "ARN of this Region's store, in another Region",
            OTHER_REGION,
            event_data_store_arn(OTHER_REGION, &home.id),
        ),
    ];

    for (case, region, identifier) in cases {
        let reply = disable_in(&emulator, region, &identifier).await;
        reply.expect_error(case, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
        // The message names the ARN the request addressed: the ARN as given, or
        // the one a bare id completes to in the calling Region.
        let addressed = if identifier.contains(':') {
            identifier.clone()
        } else {
            event_data_store_arn(region, &identifier)
        };
        assert!(
            message(&reply).contains(&addressed),
            "case {case:?}: the refusal must name the store it addressed ({addressed:?}), \
             got {:?}",
            message(&reply)
        );
    }

    assert_eq!(
        registry(&emulator),
        before,
        "a refused call must write nothing, got {:?}",
        registry(&emulator)
    );
}

#[tokio::test]
async fn an_unreadable_identifier_is_refused() {
    let emulator = Emulator::new();
    for row in &rows() {
        seed(&emulator, &row.record);
    }
    let before = registry(&emulator);
    let id = "11111111-1111-4111-8111-111111111111";

    // Each row is a value the modelled `EventDataStoreArn` pattern and length
    // bounds admit, so the boundary validator hands it to the handler, but which
    // can be read as neither an event data store ARN nor a store id. The Region
    // each is called in matters for a value that is not ARN-shaped: that one is
    // completed into an ARN in the calling Region.
    let cases = [
        (
            "an ARN of another resource type",
            HOME_REGION,
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/lake-minimal"),
        ),
        (
            "an ARN of another service",
            HOME_REGION,
            format!("arn:aws:logs:{HOME_REGION}:000000000000:eventdatastore/{id}"),
        ),
        (
            "an ARN of a service that is not CloudTrail at all",
            OTHER_REGION,
            "arn:aws:s3:::not-a-cloudtrail-resource".to_string(),
        ),
        (
            "an ARN with no Region",
            HOME_REGION,
            format!("arn:aws:cloudtrail::000000000000:eventdatastore/{id}"),
        ),
        (
            "an ARN with no account",
            HOME_REGION,
            format!("arn:aws:cloudtrail:{HOME_REGION}::eventdatastore/{id}"),
        ),
        (
            "an ARN with no resource id",
            HOME_REGION,
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:eventdatastore/"),
        ),
        (
            "an ARN with too many fields",
            HOME_REGION,
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:eventdatastore/{id}:1"),
        ),
        (
            "an ARN with too few fields",
            HOME_REGION,
            format!("arn:aws:cloudtrail:{HOME_REGION}:eventdatastore/{id}"),
        ),
        (
            "the resource prefix alone",
            HOME_REGION,
            "eventdatastore/".to_string(),
        ),
        (
            "the resource prefix alone, from another Region",
            OTHER_REGION,
            "eventdatastore/".to_string(),
        ),
    ];

    for (case, region, identifier) in cases {
        let reply = disable_in(&emulator, region, &identifier).await;
        reply.expect_error(case, 400, "EventDataStoreARNInvalidException");
        // "The event data store ARN <arn> format is not valid." — the wording the
        // recorded CloudTrail response carries, naming the ARN the request
        // addressed: the value as given when it is ARN-shaped, and the ARN a value
        // that is not completes to in the calling Region.
        let addressed = if identifier.contains(':') {
            identifier.clone()
        } else {
            event_data_store_arn(
                region,
                identifier
                    .strip_prefix("eventdatastore/")
                    .unwrap_or(&identifier),
            )
        };
        let expected = format!("The event data store ARN {addressed} format is not valid.");
        assert_eq!(
            message(&reply),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&reply)
        );
    }

    assert_eq!(
        registry(&emulator),
        before,
        "a refused call must write nothing, got {:?}",
        registry(&emulator)
    );
}

#[tokio::test]
async fn a_store_being_torn_down_is_inactive() {
    let emulator = Emulator::new();
    let torn_down = record(
        "88888888-8888-4888-8888-888888888888",
        HOME_REGION,
        EventDataStoreState::PendingDeletion,
        FederationState::Enabled,
    );
    seed(&emulator, &torn_down);
    let before = registry(&emulator);

    for (case, identifier) in [
        (
            "by ARN",
            event_data_store_arn(&torn_down.region, &torn_down.id),
        ),
        ("by id", torn_down.id.clone()),
    ] {
        let reply = disable_in(&emulator, &torn_down.region, &identifier).await;
        reply.expect_error(case, 400, "InactiveEventDataStoreException");
        assert!(
            message(&reply).contains(torn_down.status.as_wire()),
            "case {case:?}: the refusal must say which state the store is in, got {:?}",
            message(&reply)
        );
    }

    assert_eq!(
        registry(&emulator),
        before,
        "a refused call must leave the store's federation alone, got {:?}",
        registry(&emulator)
    );
}
