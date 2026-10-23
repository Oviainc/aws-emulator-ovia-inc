// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetEventDataStore`.
//!
//! Every case goes over the wire through the shared [`super::emulator`] harness —
//! an `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the
//! production router serves — so what is asserted here is what a client sees,
//! including the HTTP status and the error code.
//!
//! The rules under test:
//!
//! * a read reports the STORED event data store: every member of the response is
//!   the persisted value, whatever its status, billing mode, retention period,
//!   flags, keys or selectors, and a member the store does not carry stays absent
//!   (model: `GetEventDataStoreResponse`);
//! * the identifier is "The ARN (or ID suffix of the ARN) of the event data store"
//!   (model doc for `GetEventDataStoreRequest$EventDataStore`), so every form
//!   addresses the same store and answers identically, the ARN pinning the Region
//!   it names and a bare id the Region the call was made in;
//! * a readable identifier naming no store in the Region it addresses is refused
//!   with `EventDataStoreNotFoundException`, whose message names the ARN
//!   addressed;
//! * an identifier that cannot be read as either form is refused with
//!   `EventDataStoreARNInvalidException`;
//! * reading changes nothing: a second read, and any later caller, sees exactly
//!   what the first one did.
//!
//! The registry is seeded through the service's public `state` module because
//! `CreateEventDataStore` is still a scaffold stub (it answers 501), so the
//! persisted records a read computes from cannot yet be made over the wire. The
//! read itself is exercised only through the router.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_cloudtrail_cloudtrail_20131101::ops::types::{
    AdvancedEventSelector, AdvancedFieldSelector, PartitionKey,
};
use service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::{
    event_data_store_arn, EventDataStoreBillingMode, EventDataStoreRecord, EventDataStoreState,
    FederationState,
};

use super::emulator::{Emulator, Reply, HOME_REGION};

/// A Region other than [`HOME_REGION`], for the cases about which Region an
/// identifier addresses a store in.
const OTHER_REGION: &str = "us-west-2";

/// The HTTP status this service answers a not-found refusal with: the model
/// annotates `EventDataStoreNotFoundException` `@httpError(404)`, and the recorded
/// CloudTrail response for this operation is 400.
const NOT_FOUND_STATUS: u16 = 400;

/// `RetentionPeriod` is bounded `7..=3653` by the model, so the table carries both
/// published bounds; a value outside them is refused by the boundary validator
/// before a store can hold it.
const RETENTION_MIN: i32 = 7;
/// See [`RETENTION_MIN`].
const RETENTION_MAX: i32 = 3653;

/// An instant, as a fixed offset from the epoch so stored times are comparable.
fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// The epoch seconds a stored instant renders as on the wire.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("a stored timestamp is at or after the epoch")
        .as_secs_f64()
}

/// A selector named `name` over one field, so one store's stored selector list is
/// distinguishable from another's.
fn selector(name: &str, field: &str) -> AdvancedEventSelector {
    AdvancedEventSelector {
        name: Some(name.to_string()),
        field_selectors: vec![AdvancedFieldSelector {
            field: field.to_string(),
            equals: Some(vec!["Management".to_string()]),
            ..AdvancedFieldSelector::default()
        }],
    }
}

/// A store record under `id` in `region`, carrying the values each row then
/// varies.
fn record(id: &str, name: &str, region: &str) -> EventDataStoreRecord {
    EventDataStoreRecord {
        id: id.to_string(),
        name: name.to_string(),
        region: region.to_string(),
        status: EventDataStoreState::Enabled,
        created_at: Some(at(1_700_000_000)),
        advanced_event_selectors: vec![selector("management-events", "eventCategory")],
        retention_period: Some(RETENTION_MIN),
        billing_mode: EventDataStoreBillingMode::ExtendableRetentionPricing,
        federation_status: FederationState::Disabled,
        ..EventDataStoreRecord::default()
    }
}

/// The table of stores a read must report back exactly: the rows differ along the
/// members the response carries — status, billing mode, both published retention
/// bounds, each flag, an absent and a present KMS key, federation off and on,
/// absent and present partition keys, one and two selectors, an absent and a
/// present update time, and the Region the store lives in.
fn stored_stores() -> Vec<(&'static str, EventDataStoreRecord)> {
    let minimal = record(
        "11111111-1111-4111-8111-111111111111",
        "lake-minimal",
        HOME_REGION,
    );

    let mut maximal = record(
        "22222222-2222-4222-8222-222222222222",
        "lake_maximal.store-2",
        HOME_REGION,
    );
    maximal.status = EventDataStoreState::StoppedIngestion;
    maximal.billing_mode = EventDataStoreBillingMode::FixedRetentionPricing;
    maximal.retention_period = Some(RETENTION_MAX);
    maximal.multi_region_enabled = true;
    maximal.organization_enabled = true;
    maximal.termination_protection_enabled = true;
    maximal.kms_key_id = Some(
        "arn:aws:kms:us-east-1:000000000000:key/12345678-1234-1234-1234-123456789012".to_string(),
    );
    maximal.federation_status = FederationState::Enabled;
    maximal.federation_role_arn =
        Some("arn:aws:iam::000000000000:role/lake-federation".to_string());
    // `PartitionKeyList` is bounded 0..=2 by the model, so two is the top of that
    // range.
    maximal.partition_keys = vec![
        PartitionKey {
            name: "eventTime".to_string(),
            r#type: "bigint".to_string(),
        },
        PartitionKey {
            name: "eventName".to_string(),
            r#type: "string".to_string(),
        },
    ];
    maximal.advanced_event_selectors = vec![
        selector("management-events", "eventCategory"),
        selector("data-events", "resources.type"),
    ];
    maximal.updated_at = Some(at(1_700_086_400));

    let mut elsewhere = record(
        "33333333-3333-4333-8333-333333333333",
        "lake-other-region",
        OTHER_REGION,
    );
    elsewhere.status = EventDataStoreState::Created;
    elsewhere.retention_period = Some(90);
    elsewhere.advanced_event_selectors = Vec::new();

    vec![
        ("minimal store", minimal),
        ("every member set", maximal),
        ("in another Region", elsewhere),
    ]
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

/// Every stored store, rendered — for the cases that assert a read left the
/// registry alone.
fn registry(emulator: &Emulator) -> BTreeMap<String, String> {
    emulator
        .state()
        .lock()
        .event_data_stores
        .iter()
        .map(|(id, record)| (id.clone(), format!("{record:?}")))
        .collect()
}

/// Call `GetEventDataStore` from `region` for `identifier`.
async fn get_in(emulator: &Emulator, region: &str, identifier: &str) -> Reply {
    emulator
        .call_in(
            "GetEventDataStore",
            region,
            &json!({ "EventDataStore": identifier }),
        )
        .await
}

/// The error `Message` a reply carries, or `""`.
fn message(reply: &Reply) -> &str {
    reply.message()
}

/// Assert `body` reports exactly what `record` persists.
fn assert_reports(case: &str, body: &Value, record: &EventDataStoreRecord) {
    let string = |member: &str| body.get(member).and_then(Value::as_str).map(str::to_string);
    let flag = |member: &str| body.get(member).and_then(Value::as_bool);
    let number = |member: &str| body.get(member).and_then(Value::as_f64);

    let arn = event_data_store_arn(&record.region, &record.id);
    assert_eq!(
        string("EventDataStoreArn").as_deref(),
        Some(arn.as_str()),
        "case {case:?}: expected the stored store's ARN {arn:?}, got {body:?}"
    );
    assert_eq!(
        string("Name").as_deref(),
        Some(record.name.as_str()),
        "case {case:?}: expected the stored name {:?}, got {body:?}",
        record.name
    );
    assert_eq!(
        string("Status").as_deref(),
        Some(record.status.as_wire()),
        "case {case:?}: expected the stored status {:?}, got {body:?}",
        record.status
    );
    assert_eq!(
        string("BillingMode").as_deref(),
        Some(record.billing_mode.as_wire()),
        "case {case:?}: expected the stored billing mode {:?}, got {body:?}",
        record.billing_mode
    );
    assert_eq!(
        string("FederationStatus").as_deref(),
        Some(record.federation_status.as_wire()),
        "case {case:?}: expected the stored federation status {:?}, got {body:?}",
        record.federation_status
    );
    assert_eq!(
        string("FederationRoleArn"),
        record.federation_role_arn,
        "case {case:?}: expected the stored federation role {:?}, got {body:?}",
        record.federation_role_arn
    );
    assert_eq!(
        string("KmsKeyId"),
        record.kms_key_id,
        "case {case:?}: expected the stored KMS key {:?}, got {body:?}",
        record.kms_key_id
    );
    assert_eq!(
        flag("MultiRegionEnabled"),
        Some(record.multi_region_enabled),
        "case {case:?}: expected MultiRegionEnabled {}, got {body:?}",
        record.multi_region_enabled
    );
    assert_eq!(
        flag("OrganizationEnabled"),
        Some(record.organization_enabled),
        "case {case:?}: expected OrganizationEnabled {}, got {body:?}",
        record.organization_enabled
    );
    assert_eq!(
        flag("TerminationProtectionEnabled"),
        Some(record.termination_protection_enabled),
        "case {case:?}: expected TerminationProtectionEnabled {}, got {body:?}",
        record.termination_protection_enabled
    );
    assert_eq!(
        number("RetentionPeriod"),
        record.retention_period.map(f64::from),
        "case {case:?}: expected the stored retention {:?}, got {body:?}",
        record.retention_period
    );
    assert_eq!(
        number("CreatedTimestamp"),
        record.created_at.map(epoch_seconds),
        "case {case:?}: expected the stored creation time, got {body:?}"
    );
    assert_eq!(
        number("UpdatedTimestamp"),
        record.updated_at.or(record.created_at).map(epoch_seconds),
        "case {case:?}: expected the stored update time, got {body:?}"
    );
    // "UpdatedTimestamp is always either the same or newer than the time shown in
    // CreatedTimestamp" — model doc for `GetEventDataStoreResponse$UpdatedTimestamp`.
    assert!(
        number("UpdatedTimestamp") >= number("CreatedTimestamp"),
        "case {case:?}: UpdatedTimestamp must not precede CreatedTimestamp, got {body:?}"
    );

    let selector_names: Vec<String> = body
        .get("AdvancedEventSelectors")
        .and_then(Value::as_array)
        .map(|selectors| {
            selectors
                .iter()
                .map(|selector| {
                    selector
                        .get("Name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    let stored_selector_names: Vec<String> = record
        .advanced_event_selectors
        .iter()
        .map(|selector| selector.name.clone().unwrap_or_default())
        .collect();
    assert_eq!(
        selector_names, stored_selector_names,
        "case {case:?}: expected the stored selectors {stored_selector_names:?}, got {body:?}"
    );

    let partition_keys: Vec<(String, String)> = body
        .get("PartitionKeys")
        .and_then(Value::as_array)
        .map(|keys| {
            keys.iter()
                .map(|key| {
                    (
                        key.get("Name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        key.get("Type")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let stored_partition_keys: Vec<(String, String)> = record
        .partition_keys
        .iter()
        .map(|key| (key.name.clone(), key.r#type.clone()))
        .collect();
    assert_eq!(
        partition_keys, stored_partition_keys,
        "case {case:?}: expected the stored partition keys {stored_partition_keys:?}, \
         got {body:?}"
    );
}

#[tokio::test]
async fn a_read_reports_the_stored_event_data_store_under_every_identifier_form() {
    let emulator = Emulator::new();
    let stores = stored_stores();
    for (_, record) in &stores {
        seed(&emulator, record);
    }

    for (case, record) in &stores {
        let arn = event_data_store_arn(&record.region, &record.id);
        // The ARN pins the Region the store lives in, so it addresses the store
        // from any Region a caller signs for; the bare id — with or without the
        // resource prefix the ARN carries — names it in the store's own Region.
        let forms = [
            ("by ARN, from its own Region", record.region.clone(), arn.clone()),
            ("by ARN, from another Region", HOME_REGION.to_string(), arn.clone()),
            ("by id", record.region.clone(), record.id.clone()),
            (
                "by prefixed id",
                record.region.clone(),
                format!("eventdatastore/{}", record.id),
            ),
        ];

        let mut answers: Vec<(String, Value)> = Vec::new();
        for (form, caller_region, identifier) in forms {
            let case = format!("{case}, {form}");
            let reply = get_in(&emulator, &caller_region, &identifier).await;
            assert_reports(&case, reply.expect_ok(&case), record);
            answers.push((case, reply.body));
        }
        let (first_case, first) = &answers[0];
        for (case, body) in &answers[1..] {
            assert_eq!(
                body, first,
                "case {case:?}: every identifier form addresses one store, so the answer \
                 must equal the one {first_case:?} got, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_readable_identifier_naming_no_store_is_not_found() {
    let emulator = Emulator::new();
    let stores = stored_stores();
    for (_, record) in &stores {
        seed(&emulator, record);
    }
    let absent_id = "44444444-4444-4444-8444-444444444444";
    let elsewhere = &stores[2].1;

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
            event_data_store_arn(OTHER_REGION, &stores[0].1.id),
        ),
    ];

    for (case, region, identifier) in cases {
        let reply = get_in(&emulator, region, &identifier).await;
        reply.expect_error(case, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
        // The message names the ARN the request addressed: the ARN as given, or the
        // one a bare id completes to in the calling Region.
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
}

#[tokio::test]
async fn an_unreadable_identifier_is_refused() {
    let emulator = Emulator::new();
    for (_, record) in &stored_stores() {
        seed(&emulator, record);
    }
    let id = "11111111-1111-4111-8111-111111111111";

    // Each row is a value the modelled `EventDataStoreArn` pattern and length
    // bounds admit, so the boundary validator hands it to the handler, but which
    // can be read as neither an event data store ARN nor a store id.
    let cases = [
        (
            "an ARN of another resource type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/lake-minimal"),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:logs:{HOME_REGION}:000000000000:eventdatastore/{id}"),
        ),
        (
            "an ARN with no Region",
            format!("arn:aws:cloudtrail::000000000000:eventdatastore/{id}"),
        ),
        (
            "an ARN with no account",
            format!("arn:aws:cloudtrail:{HOME_REGION}::eventdatastore/{id}"),
        ),
        (
            "an ARN with no resource id",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:eventdatastore/"),
        ),
        (
            "an ARN with too many fields",
            format!("arn:aws:cloudtrail:{HOME_REGION}:000000000000:eventdatastore/{id}:1"),
        ),
        (
            "an ARN with too few fields",
            format!("arn:aws:cloudtrail:{HOME_REGION}:eventdatastore/{id}"),
        ),
        ("the resource prefix alone", "eventdatastore/".to_string()),
    ];

    for (case, identifier) in cases {
        let reply = get_in(&emulator, HOME_REGION, &identifier).await;
        reply.expect_error(case, 400, "EventDataStoreARNInvalidException");
        assert!(
            message(&reply).contains(&identifier),
            "case {case:?}: the refusal must name the value it refused, got {:?}",
            message(&reply)
        );
    }
}

#[tokio::test]
async fn reading_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let stores = stored_stores();
    for (_, record) in &stores {
        seed(&emulator, record);
    }
    let before = registry(&emulator);

    for (case, record) in &stores {
        let arn = event_data_store_arn(&record.region, &record.id);
        let first = get_in(&emulator, &record.region, &arn).await;
        let first = first.expect_ok(case).clone();
        for repeat in 1..=3 {
            let case = format!("{case}, read {repeat}");
            let again = get_in(&emulator, &record.region, &arn).await;
            assert_eq!(
                again.expect_ok(&case),
                &first,
                "case {case:?}: a read must answer what the first read answered, got {:?}",
                again.body
            );
        }
    }
    // A refused read must not write either.
    get_in(&emulator, HOME_REGION, "eventdatastore/").await;
    get_in(
        &emulator,
        HOME_REGION,
        &event_data_store_arn(HOME_REGION, "44444444-4444-4444-8444-444444444444"),
    )
    .await;

    assert_eq!(
        registry(&emulator),
        before,
        "the registry must be untouched by reads, got {:?}",
        registry(&emulator)
    );
}
