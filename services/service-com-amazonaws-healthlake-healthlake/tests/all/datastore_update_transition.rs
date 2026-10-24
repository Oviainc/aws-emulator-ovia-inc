// SPDX-License-Identifier: CC0-1.0

//! The LIFECYCLE a configuration change puts a data store on.
//!
//! One rule, asserted over a SET of inputs rather than one request: applying a
//! configuration change to a resource is asynchronous work, so the record
//! reports the modelled transient status (`UPDATING`) from the instant the
//! change is accepted until the transition the clock owes falls due, and the
//! settled status (`ACTIVE`) after it — reported identically by the mutation's
//! own answer and by every reporting operation
//! (`DescribeFHIRDatastore`, `ListFHIRDatastores`, including its
//! `DatastoreStatus` filter), with the written members held across both phases.
//!
//! The rows span what the rule generalises over — WHICH members a request
//! carries (one, several, all, none) and the form of their values (ASCII,
//! unicode, a name at the modelled maximum, each configuration's enum, a list of
//! one and of three, the backup retention at a published bound) — because the
//! status a data store reports cannot depend on the shape of the request that
//! changed it.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the shared harness in `fhir_datastore_lifecycle`, so routing,
//! deserialization and the modelled constraints are exercised too.

use serde_json::{json, Value};

use crate::fhir_datastore_lifecycle::{error_code, error_message, HealthLake, PAST_TRANSITION};

/// The create this suite's data stores start from: settled, named, and carrying
/// the configurations an update replaces, so a row that changes one member is
/// changing something the record already held.
fn baseline_create() -> Value {
    json!({
        "DatastoreTypeVersion": "R4",
        "DatastoreName": "transition.baseline",
        "AnalyticsConfiguration": { "Status": "ENABLED" },
        "NlpConfiguration": { "Status": "DISABLED" },
        "ProfileConfiguration": {
            "DefaultProfiles": ["http://hl7.org/fhir/StructureDefinition/Patient"],
        },
        "IdentityProviderConfiguration": {
            "AuthorizationStrategy": "AWS_AUTH",
            "FineGrainedAuthorizationEnabled": false,
        },
    })
}

/// The update requests the table spans, each with the path
/// `DatastoreProperties` reports a changed member at and the value it must
/// report there.
///
/// Source for the member set: `smithy_query(query="operation",
/// operation="UpdateFHIRDatastore")`; for where `BackupConfiguration` is
/// reported: `smithy_query(query="shape",
/// shape="DatastoreProperties,DatastoreBackupStatus")` — inside
/// `BackupStatusInfo.Configuration`.
#[allow(clippy::type_complexity)]
fn update_cases() -> Vec<(&'static str, Value, Vec<(&'static [&'static str], Value)>)> {
    vec![
        (
            "an ASCII rename",
            json!({ "DatastoreName": "renamed.by.transition" }),
            vec![(
                &["DatastoreName"] as &[&str],
                json!("renamed.by.transition"),
            )],
        ),
        (
            "a unicode rename",
            json!({ "DatastoreName": "клиника записи 7" }),
            vec![(&["DatastoreName"], json!("клиника записи 7"))],
        ),
        (
            "a rename at the modelled 256-character maximum",
            json!({ "DatastoreName": "n".repeat(256) }),
            vec![(&["DatastoreName"], json!("n".repeat(256)))],
        ),
        (
            "analytics flipped to PAUSED",
            json!({ "AnalyticsConfiguration": { "Status": "PAUSED" } }),
            vec![(
                &["AnalyticsConfiguration"],
                json!({ "Status": "PAUSED" }),
            )],
        ),
        (
            "NLP flipped to ENABLED",
            json!({ "NlpConfiguration": { "Status": "ENABLED" } }),
            vec![(&["NlpConfiguration"], json!({ "Status": "ENABLED" }))],
        ),
        (
            "a default profile list of three",
            json!({
                "ProfileConfiguration": {
                    "DefaultProfiles": [
                        "http://hl7.org/fhir/us/core/StructureDefinition/us-core-patient",
                        "http://hl7.org/fhir/us/core/StructureDefinition/us-core-encounter",
                        "http://hl7.org/fhir/StructureDefinition/Observation",
                    ],
                },
            }),
            vec![(
                &["ProfileConfiguration"],
                json!({
                    "DefaultProfiles": [
                        "http://hl7.org/fhir/us/core/StructureDefinition/us-core-patient",
                        "http://hl7.org/fhir/us/core/StructureDefinition/us-core-encounter",
                        "http://hl7.org/fhir/StructureDefinition/Observation",
                    ],
                }),
            )],
        ),
        (
            "the identity provider swapped to SMART on FHIR",
            json!({
                "IdentityProviderConfiguration": {
                    "AuthorizationStrategy": "SMART_ON_FHIR",
                    "FineGrainedAuthorizationEnabled": true,
                },
            }),
            vec![(
                &["IdentityProviderConfiguration"],
                json!({
                    "AuthorizationStrategy": "SMART_ON_FHIR",
                    "FineGrainedAuthorizationEnabled": true,
                }),
            )],
        ),
        (
            "backup enabled at the published minimum retention of 1 day",
            json!({
                "BackupConfiguration": {
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 1,
                    "BackupTagsEnabled": false,
                },
            }),
            vec![(
                &["BackupStatusInfo", "Configuration"],
                json!({
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 1,
                    "BackupTagsEnabled": false,
                }),
            )],
        ),
        (
            "every updatable member at once",
            json!({
                "DatastoreName": "fully.updated",
                "AnalyticsConfiguration": { "Status": "DISABLED" },
                "NlpConfiguration": { "Status": "ENABLED" },
                "ProfileConfiguration": {
                    "DefaultProfiles": ["http://hl7.org/fhir/StructureDefinition/Condition"],
                },
                "IdentityProviderConfiguration": {
                    "AuthorizationStrategy": "SMART_ON_FHIR_V1",
                    "FineGrainedAuthorizationEnabled": true,
                },
                "BackupConfiguration": {
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 30,
                    "BackupTagsEnabled": true,
                },
            }),
            vec![
                (&["DatastoreName"], json!("fully.updated")),
                (&["AnalyticsConfiguration"], json!({ "Status": "DISABLED" })),
                (&["NlpConfiguration"], json!({ "Status": "ENABLED" })),
                (
                    &["BackupStatusInfo", "Configuration", "RetentionPeriodInDays"],
                    json!(30),
                ),
            ],
        ),
        (
            "no updatable member at all",
            json!({}),
            vec![(&["DatastoreName"], json!("transition.baseline"))],
        ),
    ]
}

/// Whether `ListFHIRDatastores`, filtered by `status` alone, reports `id`.
async fn listed_under(service: &HealthLake, status: &str, id: &str) -> bool {
    service
        .list_all(&json!({ "DatastoreStatus": status }))
        .await
        .iter()
        .any(|properties| properties["DatastoreId"] == json!(id))
}

/// Every expected member, at the path `DatastoreProperties` reports it under, as
/// `properties` reports it.
fn assert_members(
    case: &str,
    phase: &str,
    properties: &Value,
    expected: &[(&'static [&'static str], Value)],
) {
    for (path, value) in expected {
        let mut reported = properties;
        for segment in path.iter() {
            reported = &reported[*segment];
        }
        assert_eq!(
            reported,
            value,
            "case {case:?} ({phase}): expected {} to be {value}, got {properties}",
            path.join(".")
        );
    }
}

#[tokio::test]
async fn an_update_reports_updating_until_the_transition_falls_due_then_active() {
    let service = HealthLake::new();

    // One data store per row, all created before the single wait, so the table
    // costs one lifecycle span rather than one per row.
    let mut rows = Vec::new();
    for (case, update, expected_members) in update_cases() {
        let (id, _) = service.create(&baseline_create()).await;
        rows.push((case, update, expected_members, id));
    }
    tokio::time::sleep(PAST_TRANSITION).await;

    for (case, update, expected_members, id) in &rows {
        let before = service.describe(id).await;
        assert_eq!(
            before["DatastoreStatus"], "ACTIVE",
            "case {case:?}: the data store must be ACTIVE before the update, got {before}"
        );

        let mut request = update.clone();
        request["DatastoreId"] = json!(id);
        let answered = service.ok("UpdateFHIRDatastore", &request).await;
        let properties = &answered["DatastoreProperties"];

        // The transient status is what the mutation itself answers, so the span
        // is never zero: a caller acting straight after the call sees it.
        assert_eq!(
            properties["DatastoreStatus"], "UPDATING",
            "case {case:?}: UpdateFHIRDatastore must report the transient UPDATING status, \
             got {properties}"
        );
        assert_members(case, "the update's answer", properties, expected_members);

        // And every reporting operation agrees with it at the same instant.
        let reported = service.describe(id).await;
        assert_eq!(
            reported["DatastoreStatus"], "UPDATING",
            "case {case:?}: DescribeFHIRDatastore must report UPDATING while the change is \
             being applied, got {reported}"
        );
        assert_members(case, "the describe in flight", &reported, expected_members);
        assert!(
            listed_under(&service, "UPDATING", id).await,
            "case {case:?}: ListFHIRDatastores filtered by UPDATING must report the data store \
             while the change is being applied"
        );
        assert!(
            !listed_under(&service, "ACTIVE", id).await,
            "case {case:?}: a data store being updated must not be listed under ACTIVE"
        );
    }

    // The transition the clock owes lands with nothing but the clock: it was due
    // at this instant whether or not anybody polled.
    tokio::time::sleep(PAST_TRANSITION).await;

    for (case, _, expected_members, id) in &rows {
        let settled = service.describe(id).await;
        assert_eq!(
            settled["DatastoreStatus"], "ACTIVE",
            "case {case:?}: the data store must settle back to ACTIVE, got {settled}"
        );
        assert_members(case, "the settled describe", &settled, expected_members);
        assert!(
            listed_under(&service, "ACTIVE", id).await,
            "case {case:?}: a settled data store must be listed under ACTIVE"
        );
        assert!(
            !listed_under(&service, "UPDATING", id).await,
            "case {case:?}: a settled data store must no longer be listed under UPDATING"
        );
    }
}

#[tokio::test]
async fn reading_a_data_store_being_updated_neither_advances_nor_reverses_its_status() {
    let service = HealthLake::new();
    let (id, _) = service.create(&baseline_create()).await;
    tokio::time::sleep(PAST_TRANSITION).await;

    service
        .ok(
            "UpdateFHIRDatastore",
            &json!({ "DatastoreId": &id, "DatastoreName": "polled.during.update" }),
        )
        .await;

    // Repeated reads inside the window report the SAME transient status: a read
    // is not what moves the record on, and it does not undo the transition the
    // update started either.
    for poll in 0..5u32 {
        let reported = service.describe(&id).await;
        assert_eq!(
            reported["DatastoreStatus"], "UPDATING",
            "poll {poll}: a read must not settle the data store early, got {reported}"
        );
        assert_eq!(
            reported["DatastoreName"], "polled.during.update",
            "poll {poll}: the update's write must be reported while it is in flight, \
             got {reported}"
        );
    }

    tokio::time::sleep(PAST_TRANSITION).await;
    let settled = service.describe(&id).await;
    assert_eq!(
        settled["DatastoreStatus"], "ACTIVE",
        "the data store must settle to ACTIVE after the transition falls due, got {settled}"
    );
}

#[tokio::test]
async fn a_data_store_being_updated_still_serves_its_fhir_endpoint() {
    // The data plane needs the harness that drives BOTH wires, so this row uses
    // the one `fhir_rest_and_validate_source` publishes rather than a second
    // copy of it.
    let service = crate::fhir_rest_and_validate_source::HealthLake::new();
    let (id, path) = service.active_datastore("transition.dataplane").await;

    // A resource written before the update is readable through the endpoint
    // while the configuration change is in flight: a configuration change does
    // not touch what the data store holds.
    let created = crate::fhir_rest_and_validate_source::created(
        &service,
        &path,
        "Patient",
        &json!({ "resourceType": "Patient", "active": true }),
    )
    .await;
    let resource_id = created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("a created FHIR resource carries an id, got {created}"))
        .to_string();

    let (status, updated) = service
        .rpc(
            "UpdateFHIRDatastore",
            &json!({
                "DatastoreId": &id,
                "AnalyticsConfiguration": { "Status": "DISABLED" },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "UpdateFHIRDatastore must answer 200, got {status}: {updated}"
    );
    assert_eq!(
        updated["DatastoreProperties"]["DatastoreStatus"], "UPDATING",
        "the update must put the data store in UPDATING, got {updated}"
    );

    let (status, read, _headers) = service
        .fhir("GET", &format!("{path}Patient/{resource_id}"), &Value::Null)
        .await;
    assert_eq!(
        status, 200,
        "a data store being updated must still serve a read of a resource it holds, \
         got {status}: {read}"
    );
    assert_eq!(
        read["id"], resource_id,
        "the served resource must be the one that was created, got {read}"
    );
}

#[tokio::test]
async fn a_second_update_while_the_first_is_in_flight_is_refused_as_a_conflict() {
    let service = HealthLake::new();
    let (id, _) = service.create(&baseline_create()).await;
    tokio::time::sleep(PAST_TRANSITION).await;

    service
        .ok(
            "UpdateFHIRDatastore",
            &json!({ "DatastoreId": &id, "DatastoreName": "first.update" }),
        )
        .await;

    let (status, payload) = service
        .call(
            "UpdateFHIRDatastore",
            &json!({ "DatastoreId": &id, "DatastoreName": "second.update" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a mutation of a data store in transition must be refused, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ConflictException",
        "a data store in a transition state must answer ConflictException, got {payload}"
    );
    let message = error_message(&payload);
    assert!(
        message.contains("UPDATING"),
        "the conflict sentence must state the status the record is in, got {message:?}"
    );

    // The refused second update wrote nothing, and the first one still settles.
    tokio::time::sleep(PAST_TRANSITION).await;
    let settled = service.describe(&id).await;
    assert_eq!(
        settled["DatastoreStatus"], "ACTIVE",
        "the first update must still settle the data store to ACTIVE, got {settled}"
    );
    assert_eq!(
        settled["DatastoreName"], "first.update",
        "the refused update must not have written its name, got {settled}"
    );
}
