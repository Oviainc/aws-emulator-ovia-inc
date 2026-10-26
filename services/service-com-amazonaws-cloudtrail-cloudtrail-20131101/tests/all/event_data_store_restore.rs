// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RestoreEventDataStore`, read back through
//! `GetEventDataStore`.
//!
//! Every case goes over the wire through the shared [`Emulator`] harness — an
//! `awsJson1_1` `POST /` dispatched by the same `MultiServiceRouter` the production
//! router serves — so what is asserted here is what a client sees, including the HTTP
//! status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model
//! (`smithy_query(query="operation", operation="RestoreEventDataStore")`):
//!
//! * "Restores a deleted event data store specified by EventDataStore": only a store
//!   awaiting deletion has anything to restore, so every other status is refused with
//!   `InvalidEventDataStoreStatusException` and the store is left as it was;
//! * a restored store leaves the wait period `DeleteEventDataStore` started: it reads
//!   the transient provisioning status the model carries, and the CLOCK — not another
//!   call — moves it to ingesting again;
//! * restoring changes no other member: the configuration the store carried comes
//!   back with it, and the next read reports the same values;
//! * "You can only restore a deleted event data store within the seven-day wait
//!   period after deletion": once that wait has elapsed the store is gone, and an
//!   identifier naming no store of this account and Region is
//!   `EventDataStoreNotFoundException` naming the ARN it addressed, while a value
//!   WRITTEN as an ARN that is not an event data store ARN is
//!   `EventDataStoreARNInvalidException`.

use std::time::Duration;

use serde_json::{json, Value};

use crate::emulator::{Emulator, HOME_REGION};

/// The values of the model's `EventDataStoreStatus` enum this suite reads.
const CREATED: &str = "CREATED";
/// See [`CREATED`].
const ENABLED: &str = "ENABLED";
/// See [`CREATED`].
const PENDING_DELETION: &str = "PENDING_DELETION";
/// See [`CREATED`].
const STOPPED_INGESTION: &str = "STOPPED_INGESTION";
/// See [`CREATED`].
const STOPPING_INGESTION: &str = "STOPPING_INGESTION";

/// The HTTP status this service answers a not-found refusal with: the model annotates
/// `EventDataStoreNotFoundException` `@httpError(404)`, and CloudTrail answers 400 for
/// it on the wire, which is what every other store operation in this crate states.
const NOT_FOUND_STATUS: u16 = 400;

/// The members a restore's response and a later `GetEventDataStore` both report, so
/// one can be compared with the other member by member.
///
/// `Status` is left out: it is the one member the CLOCK moves on its own.
const SHARED_MEMBERS: [&str; 9] = [
    "EventDataStoreArn",
    "Name",
    "AdvancedEventSelectors",
    "MultiRegionEnabled",
    "OrganizationEnabled",
    "RetentionPeriod",
    "TerminationProtectionEnabled",
    "KmsKeyId",
    "BillingMode",
];

/// Long enough for the provisioning span the state module owns to come due.
fn past_provisioning() -> Duration {
    service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::EVENT_DATA_STORE_PROVISIONING
        + Duration::from_millis(250)
}

/// Long enough for the ingestion span the state module owns to come due.
fn past_ingestion_settle() -> Duration {
    service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::INGESTION_SETTLE_AFTER
        + Duration::from_millis(250)
}

/// The string member `name` of `body`, or `""`.
fn text(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The ARN's id suffix, which is the other identifier form every store operation
/// accepts.
fn store_id(body: &Value) -> String {
    text(body, "EventDataStoreArn")
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Create a store from `request` and hand back the response body.
async fn create(emulator: &Emulator, request: &Value) -> Value {
    emulator
        .call("CreateEventDataStore", request)
        .await
        .expect_ok(&format!("CreateEventDataStore {request}"))
        .clone()
}

/// Read the store `identifier` names and hand back the response body.
async fn get(emulator: &Emulator, case: &str, identifier: &str) -> Value {
    emulator
        .call("GetEventDataStore", &json!({"EventDataStore": identifier}))
        .await
        .expect_ok(case)
        .clone()
}

/// Assert `read` reports every shared member exactly as `before` did.
fn assert_members_unchanged(case: &str, before: &Value, read: &Value) {
    for member in SHARED_MEMBERS {
        let expected = before.get(member);
        let actual = read.get(member);
        assert_eq!(
            actual, expected,
            "case {case}: expected {member} {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: a store awaiting deletion is restored — it leaves the wait period, reads the
/// transient provisioning status, and the CLOCK moves it to ingesting again — and
/// every other member comes back exactly as the store carried it.
///
/// The rows differ along the configuration the store carries (which optional members
/// it was created with, its billing mode and retention band, whether it had stopped
/// ingesting) and along the identifier form the restore is addressed by.
#[tokio::test]
async fn a_store_awaiting_deletion_is_restored_with_its_configuration() {
    // (case, the creation request)
    let rows: Vec<(&str, Value)> = vec![
        (
            "a store created with nothing but a name",
            json!({"Name": "restore-defaults"}),
        ),
        (
            "a store carrying selectors, a retention period and a key",
            json!({
                "Name": "restore-configured",
                "RetentionPeriod": 90,
                "KmsKeyId": "alias/lake-key",
                "MultiRegionEnabled": true,
                "OrganizationEnabled": true,
                "AdvancedEventSelectors": [{
                    "Name": "data events",
                    "FieldSelectors": [{"Field": "eventCategory", "Equals": ["Data"]}],
                }],
            }),
        ),
        (
            "a store on the other billing mode, at that band's longest retention",
            json!({
                "Name": "restore-fixed",
                "BillingMode": "FIXED_RETENTION_PRICING",
                "RetentionPeriod": 2557,
            }),
        ),
        (
            "a store that was not ingesting when it was deleted",
            json!({"Name": "restore-stopped", "StartIngestion": false}),
        ),
    ];

    for (index, (case, request)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let created = create(&emulator, request).await;
        let id = store_id(&created);
        let arn = text(&created, "EventDataStoreArn");
        // Both identifier forms address the same store.
        let identifier = if index % 2 == 0 { id.clone() } else { arn };
        // Let the store finish provisioning, so the delete below is the only reason
        // its status moves.
        tokio::time::sleep(past_provisioning()).await;

        emulator
            .call("DeleteEventDataStore", &json!({"EventDataStore": &identifier}))
            .await
            .expect_ok(&format!("{case}: DeleteEventDataStore"));
        let deleted = get(&emulator, case, &id).await;
        assert_eq!(
            text(&deleted, "Status"),
            PENDING_DELETION,
            "case {case}: expected the delete to leave {PENDING_DELETION}, got {deleted:?}"
        );

        let restored = emulator
            .call(
                "RestoreEventDataStore",
                &json!({"EventDataStore": &identifier}),
            )
            .await
            .expect_ok(case)
            .clone();
        // Restoring is asynchronous, so the store reads the transient status first and
        // the clock owes it the settled one.
        assert_eq!(
            text(&restored, "Status"),
            CREATED,
            "case {case}: expected the restore to report {CREATED}, got {restored:?}"
        );
        assert_members_unchanged(case, &created, &restored);

        // The response and the next read are the same values: the restore persisted.
        let read = get(&emulator, &format!("{case}, read back"), &id).await;
        assert_eq!(
            text(&read, "Status"),
            CREATED,
            "case {case}: expected the store to still read {CREATED}, got {read:?}"
        );
        assert_members_unchanged(&format!("{case}, read back"), &created, &read);

        // The CLOCK, not another call, takes the restored store back to ingesting.
        tokio::time::sleep(past_provisioning()).await;
        let settled = get(&emulator, &format!("{case}, settled"), &id).await;
        assert_eq!(
            text(&settled, "Status"),
            ENABLED,
            "case {case}: expected the clock to settle the store into {ENABLED}, got {settled:?}"
        );
        assert_members_unchanged(&format!("{case}, settled"), &created, &settled);
    }
}

/// RULE: only a store awaiting deletion can be restored; every other status the model
/// carries is refused with `InvalidEventDataStoreStatusException` and changes nothing.
///
/// The rows span the statuses a store reaches over this API — the transient one a
/// creation leaves, the ingesting one the clock settles it into, the stopped one, the
/// transient one a stop leaves, and the provisioning status a restore itself leaves
/// (so a second restore is refused).
#[tokio::test]
async fn a_store_in_any_other_status_cannot_be_restored() {
    // (case, the creation request, how long to wait before restoring, whether to stop
    //  ingestion first, the status the store must be in)
    let rows: Vec<(&str, Value, bool, bool, &str)> = vec![
        (
            "a store still provisioning",
            json!({"Name": "status-created"}),
            false,
            false,
            CREATED,
        ),
        (
            "a store that is ingesting",
            json!({"Name": "status-enabled"}),
            true,
            false,
            ENABLED,
        ),
        (
            "a store created not ingesting",
            json!({"Name": "status-stopped", "StartIngestion": false}),
            true,
            false,
            STOPPED_INGESTION,
        ),
        (
            "a store whose ingestion is stopping",
            json!({
                "Name": "status-stopping",
                "AdvancedEventSelectors": [{
                    "Name": "management events",
                    "FieldSelectors": [{"Field": "eventCategory", "Equals": ["Management"]}],
                }],
            }),
            true,
            true,
            STOPPING_INGESTION,
        ),
    ];

    for (case, request, settle, stop_ingestion, status) in &rows {
        let emulator = Emulator::new();
        let created = create(&emulator, request).await;
        let id = store_id(&created);
        if *settle {
            tokio::time::sleep(past_provisioning()).await;
        }
        if *stop_ingestion {
            emulator
                .call(
                    "StopEventDataStoreIngestion",
                    &json!({"EventDataStore": &id}),
                )
                .await
                .expect_ok(&format!("{case}: StopEventDataStoreIngestion"));
        }

        let before = get(&emulator, case, &id).await;
        assert_eq!(
            text(&before, "Status"),
            *status,
            "case {case}: expected the store to be in {status}, got {before:?}"
        );
        emulator
            .call("RestoreEventDataStore", &json!({"EventDataStore": &id}))
            .await
            .expect_error(case, 400, "InvalidEventDataStoreStatusException");

        // The refused restore changed nothing: the store is still the store it was,
        // in the status it was in (whatever the clock still owes it).
        let read = get(&emulator, &format!("{case}, after the refusal"), &id).await;
        assert_members_unchanged(&format!("{case}, after the refusal"), &before, &read);
    }

    // A restore is not repeatable: the store it leaves is provisioning, which is not a
    // status that supports the operation either.
    let emulator = Emulator::new();
    let created = create(&emulator, &json!({"Name": "restore-twice"})).await;
    let id = store_id(&created);
    tokio::time::sleep(past_provisioning()).await;
    emulator
        .call("DeleteEventDataStore", &json!({"EventDataStore": &id}))
        .await
        .expect_ok("DeleteEventDataStore");
    emulator
        .call("RestoreEventDataStore", &json!({"EventDataStore": &id}))
        .await
        .expect_ok("the first restore");
    emulator
        .call("RestoreEventDataStore", &json!({"EventDataStore": &id}))
        .await
        .expect_error(
            "the second restore",
            400,
            "InvalidEventDataStoreStatusException",
        );
    // And the store the first restore brought back still settles into ingesting.
    tokio::time::sleep(past_provisioning() + past_ingestion_settle()).await;
    let settled = get(&emulator, "after the second restore", &id).await;
    assert_eq!(
        text(&settled, "Status"),
        ENABLED,
        "case the second restore: expected the store to still settle into {ENABLED}, \
         got {settled:?}"
    );
}

/// RULE: an identifier is an ARN or the ARN's id suffix; one naming no store of this
/// account and Region is `EventDataStoreNotFoundException` naming the ARN it
/// addressed, and a value WRITTEN as an ARN that is not an event data store ARN is
/// `EventDataStoreARNInvalidException` — including a store whose seven-day wait has
/// elapsed, which no call can restore any more.
#[tokio::test]
async fn an_identifier_naming_no_store_is_refused_over_the_arn_it_addressed() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    let absent = "11111111-1111-1111-1111-111111111111";
    // A store that DOES exist, so a Region-mismatch row is about the Region.
    let created = create(&emulator, &json!({"Name": "present"})).await;
    let present_id = store_id(&created);

    // (case, the identifier, the ARN the refusal must name)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an id that names no store",
            absent.to_string(),
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{absent}"),
        ),
        (
            "an ARN that names no store in the caller's Region",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{absent}"),
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{absent}"),
        ),
        (
            "an ARN that names no store in another Region",
            format!("arn:aws:cloudtrail:us-west-2:{account}:eventdatastore/{absent}"),
            format!("arn:aws:cloudtrail:us-west-2:{account}:eventdatastore/{absent}"),
        ),
        (
            "an existing store addressed in another Region",
            format!("arn:aws:cloudtrail:ap-south-1:{account}:eventdatastore/{present_id}"),
            format!("arn:aws:cloudtrail:ap-south-1:{account}:eventdatastore/{present_id}"),
        ),
    ];

    for (case, identifier, addressed) in &rows {
        let reply = emulator
            .call(
                "RestoreEventDataStore",
                &json!({"EventDataStore": identifier}),
            )
            .await;
        reply.expect_error(case, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
        assert_eq!(
            reply.message(),
            format!("The event data store {addressed} was not found."),
            "case {case}: the refusal must name the ARN it addressed, got {:?}",
            reply.message()
        );
    }

    // A value WRITTEN as an ARN has to BE an event data store ARN, and a bare value
    // has to be an id this service could have minted. The rows differ in which part of
    // the form is broken.
    let malformed: Vec<(&str, String)> = vec![
        ("a bare value that is no minted id", "not-an-arn".to_string()),
        (
            "another resource type",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:trail/demo"),
        ),
        (
            "another service",
            format!("arn:aws:s3:{HOME_REGION}:{account}:eventdatastore/{absent}"),
        ),
        (
            "no id suffix",
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/"),
        ),
    ];
    for (case, identifier) in &malformed {
        let reply = emulator
            .call(
                "RestoreEventDataStore",
                &json!({"EventDataStore": identifier}),
            )
            .await;
        reply.expect_error(case, 400, "EventDataStoreARNInvalidException");
        let addressed = if identifier.starts_with("arn:") {
            identifier.clone()
        } else {
            format!("arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/{identifier}")
        };
        assert_eq!(
            reply.message(),
            format!("The event data store ARN {addressed} format is not valid."),
            "case {case}: expected the refusal to name the ARN it addressed, got {:?}",
            reply.message()
        );
    }

    // "You can only restore a deleted event data store within the seven-day wait
    // period after deletion": once the wait has elapsed the store is gone, so the
    // identifier that named it names nothing. Seven days cannot be waited out, so the
    // instant the delete recorded is moved into the past in the registry the handlers
    // read; the removal itself is the service's and is observed over the wire.
    tokio::time::sleep(past_provisioning()).await;
    emulator
        .call(
            "DeleteEventDataStore",
            &json!({"EventDataStore": &present_id}),
        )
        .await
        .expect_ok("DeleteEventDataStore");
    {
        let state = emulator.state();
        let mut store = state.lock();
        let record = store
            .event_data_stores
            .get_mut(&present_id)
            .expect("the created store is in the registry");
        record.deleted_at = Some(
            std::time::SystemTime::now()
                - service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::EVENT_DATA_STORE_DELETION_WAIT,
        );
    }
    let case = "a store whose wait period elapsed";
    let reply = emulator
        .call(
            "RestoreEventDataStore",
            &json!({"EventDataStore": &present_id}),
        )
        .await;
    reply.expect_error(case, NOT_FOUND_STATUS, "EventDataStoreNotFoundException");
    assert_eq!(
        reply.message(),
        format!(
            "The event data store arn:aws:cloudtrail:{HOME_REGION}:{account}:eventdatastore/\
             {present_id} was not found."
        ),
        "case {case}: the refusal must name the ARN it addressed, got {:?}",
        reply.message()
    );
}
