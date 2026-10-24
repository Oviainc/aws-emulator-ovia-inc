// SPDX-License-Identifier: CC0-1.0

//! What `UpdateFHIRDatastore` answers when the data store it names exists but is
//! not `ACTIVE`.
//!
//! ONE rule, asserted over a SET of inputs rather than one recorded request:
//! `UpdateFHIRDatastore` applies its members only to a record whose
//! `DatastoreStatus` is `ACTIVE`, and a record in ANY other status is refused
//! with the modelled `AccessDeniedException` carrying the service's fixed
//! "not active" sentence — whichever non-`ACTIVE` status that is, and whichever
//! members the request carries. `ResourceNotFoundException` is reserved for a
//! `DatastoreId` the service holds no record of at all, so it never answers an
//! id whose record `DescribeFHIRDatastore` still reports.
//!
//! The rows span the two axes the rule generalises over: the status the record
//! is in (every state this slice can reach, taken from the one table in
//! `datastore_state_conflict`, with `ACTIVE` as the positive control) and the
//! shape of the update (each subset of the optional members the operation
//! accepts, including none at all), so the gate cannot be keyed to one
//! request's shape. There is one test per status, so the statuses run
//! concurrently rather than serialising every row's lifecycle wait. Every
//! status is reached through the real lifecycle rather than by poking the
//! state, and every call goes through
//! [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter) by
//! way of the shared harness in `fhir_datastore_lifecycle`, so routing,
//! deserialization and the error envelope are exercised too.
//!
//! Every modelled CLIENT error is asserted at HTTP 400 whatever 4xx its shape's
//! `@httpError` names: awsJson1_0 binds nothing to the transport, so this
//! service reports an error's FAULT. That rule is pinned, over the whole error
//! catalog, in `tests/all/error_envelopes.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    DatastoreStatus, DATASTORE_NOT_ACTIVE_MESSAGE, DATASTORE_NOT_FOUND_MESSAGE,
};

use crate::datastore_state_conflict::{datastore_in, states};
use crate::fhir_datastore_lifecycle::{error_code, error_message, HealthLake, PAST_TRANSITION};

/// The name every data store below is created with, so a refusal that wrote
/// something anyway is visible as a changed name.
const CREATED_NAME: &str = "not-active.gate";

/// The update requests the table spans, as `(case, the members the request
/// carries besides the DatastoreId)`.
///
/// Source: `smithy_query(query="operation", operation="UpdateFHIRDatastore")` —
/// every optional member of the input. The rows differ along WHICH of them the
/// request carries and along the FORM of the values (ASCII, unicode, a name at
/// the modelled 256-character maximum, a nested configuration, nothing at all),
/// because the rule generalises over the request.
fn updates() -> Vec<(&'static str, Value)> {
    vec![
        ("a rename", json!({ "DatastoreName": "renamed-while-gated" })),
        (
            "a unicode rename with spaces",
            json!({ "DatastoreName": "клиника записи 7" }),
        ),
        (
            "a rename at the modelled 256-character maximum",
            json!({ "DatastoreName": "n".repeat(256) }),
        ),
        (
            "analytics and NLP configuration, no rename",
            json!({
                "AnalyticsConfiguration": { "Status": "ENABLED" },
                "NlpConfiguration": { "Status": "DISABLED" },
            }),
        ),
        (
            "a backup configuration",
            json!({
                "BackupConfiguration": {
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 7,
                    "BackupTagsEnabled": false,
                },
            }),
        ),
        (
            "an identity provider configuration and a rename together",
            json!({
                "DatastoreName": "smart.on:fhir/store",
                "IdentityProviderConfiguration": {
                    "AuthorizationStrategy": "SMART_ON_FHIR",
                    "FineGrainedAuthorizationEnabled": true,
                    "Metadata": "{\"issuer\":\"https://example.test\"}",
                },
            }),
        ),
        (
            "a profile configuration",
            json!({
                "ProfileConfiguration": {
                    "DefaultProfiles": [
                        "http://hl7.org/fhir/us/core/StructureDefinition/us-core-patient",
                    ],
                },
            }),
        ),
        ("no optional member at all", json!({})),
    ]
}

/// The status this suite's settled-status expectation comes from: the one table
/// in `datastore_state_conflict`, so the two suites cannot drift about which
/// states the slice reaches or what each settles to.
fn settles_to(status: DatastoreStatus) -> &'static str {
    states()
        .into_iter()
        .find(|(candidate, _)| *candidate == status)
        .map(|(_, settled)| settled)
        .unwrap_or_else(|| {
            panic!(
                "the shared status table must carry {}",
                status.as_wire()
            )
        })
}

/// Every row of [`updates`] against a data store driven to `status`, which is
/// NOT `ACTIVE`: the refusal, the sentence it carries, that the record is still
/// there, and that nothing was written.
///
/// One test per status calls this, so `cargo test` runs the statuses
/// concurrently rather than serialising every row's lifecycle wait behind the
/// last.
async fn assert_every_update_is_refused_in(status: DatastoreStatus) {
    assert_ne!(
        status,
        DatastoreStatus::Active,
        "this helper is for the statuses the gate REFUSES; ACTIVE is the positive control"
    );
    let settled = settles_to(status);
    for (update_case, members) in updates() {
        let case = format!("{update_case} on a {} data store", status.as_wire());
        // A service per row, so neither the lifecycle waits nor an accepted
        // mutation of one row can be observed by another.
        let service = HealthLake::new();
        let id = datastore_in(&service, status, CREATED_NAME).await;

        let mut request = members.clone();
        request["DatastoreId"] = json!(&id);
        let (http_status, payload) = service.call("UpdateFHIRDatastore", &request).await;

        assert_eq!(
            http_status, 400,
            "case {case:?}: a refused update must answer 400, got {http_status}: {payload}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "AccessDeniedException",
            "case {case:?}: expected __type AccessDeniedException, got {payload}"
        );

        // The sentence is the service's one fixed text, read from the crate
        // rather than retyped so the test and the service cannot drift. It
        // states neither the status the record is in nor the identifier the
        // caller sent.
        let message = error_message(&payload);
        assert_eq!(
            message, DATASTORE_NOT_ACTIVE_MESSAGE,
            "case {case:?}: expected {DATASTORE_NOT_ACTIVE_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(&id),
            "case {case:?}: a refusal must quote no requested identifier, got {message:?}"
        );
        assert!(
            !message.contains(status.as_wire()),
            "case {case:?}: the fixed sentence must not state the record's status, \
             got {message:?}"
        );

        // The record the refusal was about is STILL THERE, and the reporting
        // operation agrees it is: a refused update is never a statement that
        // the id does not exist, not even at the terminal status.
        let still_there = service.describe(&id).await;
        assert_eq!(
            still_there["DatastoreStatus"], status.as_wire(),
            "case {case:?}: the record must still be reported in the status it was in, \
             got {still_there}"
        );

        // The refusal wrote nothing: the name is still the one the create set,
        // and the lifecycle the record was already on is undisturbed — the
        // transition the clock owed still lands, and a terminal record stays
        // terminal.
        tokio::time::sleep(PAST_TRANSITION).await;
        let after = service.describe(&id).await;
        assert_eq!(
            after["DatastoreName"], CREATED_NAME,
            "case {case:?}: a refused update must not rename the data store, got {after}"
        );
        assert_eq!(
            after["DatastoreStatus"], settled,
            "case {case:?}: a refused update must leave the lifecycle alone, expected \
             {settled}, got {after}"
        );
    }
}

#[tokio::test]
async fn an_update_while_the_data_store_is_still_being_created_is_refused() {
    assert_every_update_is_refused_in(DatastoreStatus::Creating).await;
}

#[tokio::test]
async fn an_update_while_an_earlier_change_is_being_applied_is_refused() {
    assert_every_update_is_refused_in(DatastoreStatus::Updating).await;
}

#[tokio::test]
async fn an_update_while_the_data_store_is_being_deleted_is_refused() {
    assert_every_update_is_refused_in(DatastoreStatus::Deleting).await;
}

#[tokio::test]
async fn an_update_of_a_data_store_at_the_terminal_deleted_status_is_refused() {
    assert_every_update_is_refused_in(DatastoreStatus::Deleted).await;
}

/// The positive control for the four tests above: an `ACTIVE` record takes every
/// row, so what they assert is about the STATE and not about the request.
#[tokio::test]
async fn an_update_of_an_active_data_store_is_applied() {
    for (update_case, members) in updates() {
        let case = format!("{update_case} on an ACTIVE data store");
        let service = HealthLake::new();
        let id = datastore_in(&service, DatastoreStatus::Active, CREATED_NAME).await;

        let mut request = members.clone();
        request["DatastoreId"] = json!(&id);
        let (http_status, payload) = service.call("UpdateFHIRDatastore", &request).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: an ACTIVE data store must accept the update, \
             got {http_status}: {payload}"
        );
    }
}

#[tokio::test]
async fn resource_not_found_stays_for_an_id_the_service_holds_no_record_of() {
    let service = HealthLake::new();
    // A live record exists, so every refusal below is about the identifier and
    // not about an empty registry.
    let live = datastore_in(&service, DatastoreStatus::Active, CREATED_NAME).await;

    // The rows span the forms an id with no record can take inside the member's
    // modelled 1..32 length bound and `^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$` pattern
    // (`smithy_query(query="operation", operation="UpdateFHIRDatastore")`): the
    // shortest admissible id, a hex id of the minted width, a unicode id, a
    // punctuation id, the live id truncated, and the live id with one character
    // provably changed.
    let one_off = {
        let mut id = live.clone();
        let flipped = if id.ends_with('0') { '1' } else { '0' };
        id.pop();
        id.push(flipped);
        id
    };
    assert_ne!(
        one_off, live,
        "the one-character-off row must differ from the minted id"
    );
    let rows: Vec<String> = vec![
        "a".to_string(),
        "00000000000000000000000000000000".to_string(),
        "deadbeefdeadbeefdeadbeefdeadbeef".to_string(),
        "хранилище.данных".to_string(),
        "store:one/two=three+four%five@x".to_string(),
        live[..live.len() - 1].to_string(),
        one_off,
    ];
    for id in &rows {
        let (http_status, payload) = service
            .call(
                "UpdateFHIRDatastore",
                &json!({ "DatastoreId": id, "DatastoreName": "renamed-nothing" }),
            )
            .await;
        assert_eq!(
            http_status, 400,
            "case {id:?}: an id with no record must answer 400, got {http_status}: {payload}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {id:?}: an id with no record must answer ResourceNotFoundException, \
             got {payload}"
        );
        let message = error_message(&payload);
        assert_eq!(
            message, DATASTORE_NOT_FOUND_MESSAGE,
            "case {id:?}: expected {DATASTORE_NOT_FOUND_MESSAGE:?}, got {message:?}"
        );
    }

    // The live record still takes the update, so the refusals above are about
    // identity and not about a broken lookup.
    let updated = service
        .ok(
            "UpdateFHIRDatastore",
            &json!({ "DatastoreId": &live, "DatastoreName": "renamed-for-real" }),
        )
        .await;
    assert_eq!(
        updated["DatastoreProperties"]["DatastoreName"], "renamed-for-real",
        "the live data store must take the update, got {updated}"
    );
}
