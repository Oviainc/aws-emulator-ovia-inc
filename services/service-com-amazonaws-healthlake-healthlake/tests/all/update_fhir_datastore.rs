// SPDX-License-Identifier: CC0-1.0

//! What `UpdateFHIRDatastore` writes, member by member, and what it refuses.
//!
//! The sibling suite `control_plane_error_semantics` pins the refusal texts and
//! the ACTIVE-only gate. This one pins the write itself, over a SET of inputs:
//!
//! 1. an update replaces EXACTLY the members the request carries, wherever
//!    `DatastoreProperties` reports each of them, and leaves every other member
//!    of the stored record — including the ones only `CreateFHIRDatastore` can
//!    set — as it was;
//! 2. the modelled `DatastoreBackupStatus.BackupEnabledAt` is the instant backup
//!    became enabled: stamped when the configuration turns enabled, kept while
//!    it stays enabled, cleared when it does not;
//! 3. a request outside a modelled constraint is refused and writes nothing.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the shared harness in `fhir_datastore_lifecycle`, so routing,
//! deserialization, the modelled constraints and the error envelope are all
//! exercised.

use serde_json::{json, Value};

use crate::fhir_datastore_lifecycle::{error_code, HealthLake, PAST_TRANSITION};

/// A create request that sets every optional member `UpdateFHIRDatastore` can
/// replace, plus two it cannot (`SseConfiguration`, `PreloadDataConfig`), so a
/// member an update leaves out is observably retained rather than vacuously
/// absent.
fn baseline_create() -> Value {
    json!({
        "DatastoreTypeVersion": "R4",
        "DatastoreName": "baseline.store",
        "SseConfiguration": {
            "KmsEncryptionConfig": { "CmkType": "AWS_OWNED_KMS_KEY" },
        },
        "PreloadDataConfig": { "PreloadDataType": "SYNTHEA" },
        "AnalyticsConfiguration": { "Status": "ENABLED" },
        "NlpConfiguration": { "Status": "DISABLED" },
        "ProfileConfiguration": {
            "DefaultProfiles": ["http://hl7.org/fhir/StructureDefinition/Patient"],
        },
        "IdentityProviderConfiguration": {
            "AuthorizationStrategy": "AWS_AUTH",
            "FineGrainedAuthorizationEnabled": false,
        },
        "BackupConfiguration": {
            "Status": "ENABLED",
            "BackupType": "CONTINUOUS",
            "RetentionPeriodInDays": 7,
            "BackupTagsEnabled": false,
        },
    })
}

/// Every member `UpdateFHIRDatastore` accepts besides the `DatastoreId` that
/// selects the data store, paired with the path `DatastoreProperties` reports it
/// at.
///
/// Source: `smithy_query(query="operation", operation="UpdateFHIRDatastore")`
/// for the input members, and `smithy_query(query="shape",
/// shape="DatastoreProperties,DatastoreBackupStatus")` for where each is
/// reported — `BackupConfiguration` is reported inside `BackupStatusInfo`, the
/// rest under their own name.
const UPDATABLE_MEMBERS: &[(&str, &[&str])] = &[
    ("DatastoreName", &["DatastoreName"]),
    ("AnalyticsConfiguration", &["AnalyticsConfiguration"]),
    ("NlpConfiguration", &["NlpConfiguration"]),
    ("ProfileConfiguration", &["ProfileConfiguration"]),
    (
        "IdentityProviderConfiguration",
        &["IdentityProviderConfiguration"],
    ),
    ("BackupConfiguration", &["BackupStatusInfo", "Configuration"]),
];

/// The update requests the table spans. The rule generalises over WHICH members
/// a request carries and over the form of their values, so the rows differ along
/// both: one member, every member, or none at all; ASCII / unicode / a name at
/// the modelled 256-character maximum; each configuration's enum flipped to a
/// value the recorded case never carried; a list emptied and a list of three;
/// and the backup retention at each published bound of its 1..30 range.
fn update_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "a new ASCII name only",
            json!({ "DatastoreName": "renamed-store" }),
        ),
        (
            "a unicode name with the punctuation the modelled pattern allows",
            json!({ "DatastoreName": "données.santé_2026+eu" }),
        ),
        (
            "a name at the modelled 256-character maximum",
            json!({ "DatastoreName": "n".repeat(256) }),
        ),
        (
            "analytics flipped to DISABLED",
            json!({ "AnalyticsConfiguration": { "Status": "DISABLED" } }),
        ),
        (
            "NLP flipped to ENABLED",
            json!({ "NlpConfiguration": { "Status": "ENABLED" } }),
        ),
        (
            "the default profile list emptied",
            json!({ "ProfileConfiguration": { "DefaultProfiles": [] } }),
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
        ),
        (
            "the identity provider swapped to SMART on FHIR with metadata and a Lambda",
            json!({
                "IdentityProviderConfiguration": {
                    "AuthorizationStrategy": "SMART_ON_FHIR",
                    "FineGrainedAuthorizationEnabled": true,
                    "Metadata": "{\"issuer\":\"https://example.test\"}",
                    "IdpLambdaArn":
                        "arn:aws:lambda:us-east-1:000000000000:function:healthlake-idp-decoder",
                },
            }),
        ),
        (
            "backup retention at the published minimum of 1 day",
            json!({
                "BackupConfiguration": {
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 1,
                    "BackupTagsEnabled": true,
                },
            }),
        ),
        (
            "backup retention at the published maximum of 30 days",
            json!({
                "BackupConfiguration": {
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 30,
                    "BackupTagsEnabled": false,
                },
            }),
        ),
        (
            "every updatable member at once",
            json!({
                "DatastoreName": "fully updated store",
                "AnalyticsConfiguration": { "Status": "PAUSED" },
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
                    "RetentionPeriodInDays": 14,
                    "BackupTagsEnabled": true,
                },
            }),
        ),
        ("no updatable member at all", json!({})),
    ]
}

/// `baseline` with every member `request` carries replaced where
/// `DatastoreProperties` reports it: the whole record the update must answer
/// with, and the whole record a later read must still report.
fn expected_properties(baseline: &Value, request: &Value) -> Value {
    let mut expected = baseline.clone();
    for (member, path) in UPDATABLE_MEMBERS {
        let Some(value) = request.get(member) else {
            continue;
        };
        let (reported_as, parents) = path.split_last().expect("every path names a member");
        let mut cursor = &mut expected;
        for parent in parents {
            cursor = &mut cursor[*parent];
        }
        cursor[*reported_as] = value.clone();
    }
    expected
}

/// The `DatastoreProperties` `DescribeFHIRDatastore` reports for `id`.
async fn described_properties(service: &HealthLake, id: &str) -> Value {
    service
        .ok("DescribeFHIRDatastore", &json!({ "DatastoreId": id }))
        .await["DatastoreProperties"]
        .clone()
}

/// `UpdateFHIRDatastore`'s request for `id`, carrying `update`'s members.
fn update_request(id: &str, update: &Value) -> Value {
    let mut request = update.clone();
    request["DatastoreId"] = json!(id);
    request
}

#[tokio::test]
async fn update_replaces_the_members_the_request_carries_and_leaves_the_rest() {
    let service = HealthLake::new();

    // One data store per row, all created before the single wait, so the table
    // costs one lifecycle span rather than one per row.
    let mut rows = Vec::new();
    for (case, update) in update_cases() {
        let (id, _) = service.create(&baseline_create()).await;
        rows.push((case, update, id));
    }
    tokio::time::sleep(PAST_TRANSITION).await;

    for (case, update, id) in rows {
        let baseline = described_properties(&service, &id).await;
        assert_eq!(
            baseline["DatastoreStatus"], "ACTIVE",
            "case {case:?}: the data store must settle to ACTIVE before the update, got {baseline}"
        );
        let expected = expected_properties(&baseline, &update);

        let answered = service
            .ok("UpdateFHIRDatastore", &update_request(&id, &update))
            .await;
        let properties = &answered["DatastoreProperties"];
        assert_eq!(
            properties, &expected,
            "case {case:?}: UpdateFHIRDatastore must report the requested members replaced and \
             everything else as it was, got {properties}"
        );

        // The write is in the STORE, not just in the answer.
        let described = described_properties(&service, &id).await;
        assert_eq!(
            described, expected,
            "case {case:?}: DescribeFHIRDatastore must report what the update wrote, \
             got {described}"
        );
    }
}

#[tokio::test]
async fn backup_enabled_at_tracks_when_backup_became_enabled() {
    let service = HealthLake::new();
    let mut create = baseline_create();
    create["BackupConfiguration"] = json!({
        "Status": "DISABLED",
        "BackupType": "CONTINUOUS",
        "RetentionPeriodInDays": 7,
        "BackupTagsEnabled": false,
    });
    let (id, _) = service.create(&create).await;
    tokio::time::sleep(PAST_TRANSITION).await;

    // Backup has never been enabled, so there is no instant to report.
    let properties = described_properties(&service, &id).await;
    assert!(
        properties["BackupStatusInfo"]
            .get("BackupEnabledAt")
            .is_none(),
        "backup was never enabled, so BackupEnabledAt must be absent, got {properties}"
    );
    let created_at = properties["CreatedAt"]
        .as_f64()
        .expect("the data store reports a CreatedAt");

    // Enabling it stamps the instant it became enabled.
    let enabled = service
        .ok(
            "UpdateFHIRDatastore",
            &json!({
                "DatastoreId": &id,
                "BackupConfiguration": {
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 7,
                    "BackupTagsEnabled": false,
                },
            }),
        )
        .await;
    let enabled_at = enabled["DatastoreProperties"]["BackupStatusInfo"]["BackupEnabledAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("enabling backup must report BackupEnabledAt, got {enabled}"));
    assert!(
        enabled_at >= created_at,
        "BackupEnabledAt ({enabled_at}) must be at or after the data store's CreatedAt \
         ({created_at})"
    );

    // A later change that leaves backup enabled does not re-enable anything.
    let changed = service
        .ok(
            "UpdateFHIRDatastore",
            &json!({
                "DatastoreId": &id,
                "BackupConfiguration": {
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 21,
                    "BackupTagsEnabled": true,
                },
            }),
        )
        .await;
    let backup_status = &changed["DatastoreProperties"]["BackupStatusInfo"];
    assert_eq!(
        backup_status["BackupEnabledAt"].as_f64(),
        Some(enabled_at),
        "a configuration change that leaves backup enabled must keep BackupEnabledAt, \
         got {backup_status}"
    );
    assert_eq!(
        backup_status["Configuration"]["RetentionPeriodInDays"], 21,
        "the new retention period must be stored, got {backup_status}"
    );

    // Disabling it clears the instant: nothing is enabled any more.
    let disabled = service
        .ok(
            "UpdateFHIRDatastore",
            &json!({
                "DatastoreId": &id,
                "BackupConfiguration": {
                    "Status": "DISABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 21,
                    "BackupTagsEnabled": true,
                },
            }),
        )
        .await;
    let backup_status = &disabled["DatastoreProperties"]["BackupStatusInfo"];
    assert!(
        backup_status.get("BackupEnabledAt").is_none(),
        "disabling backup must clear BackupEnabledAt, got {backup_status}"
    );

    // And re-enabling it after that stamps a fresh instant rather than the old
    // one.
    let re_enabled = service
        .ok(
            "UpdateFHIRDatastore",
            &json!({
                "DatastoreId": &id,
                "BackupConfiguration": {
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 21,
                    "BackupTagsEnabled": true,
                },
            }),
        )
        .await;
    let re_enabled_at = re_enabled["DatastoreProperties"]["BackupStatusInfo"]["BackupEnabledAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("re-enabling backup must report BackupEnabledAt, got {re_enabled}"));
    assert!(
        re_enabled_at >= enabled_at,
        "re-enabling backup must stamp a fresh BackupEnabledAt ({re_enabled_at}), not the \
         earlier one ({enabled_at})"
    );
}

#[tokio::test]
async fn an_update_outside_a_modelled_constraint_is_refused_and_writes_nothing() {
    let service = HealthLake::new();
    let (id, _) = service.create(&baseline_create()).await;
    tokio::time::sleep(PAST_TRANSITION).await;
    let before = described_properties(&service, &id).await;

    // One case per published bound of a modelled constraint on this operation's
    // input, taken one step past it: `DatastoreName`'s 1..256 length
    // (smithy_query(query="operation", operation="UpdateFHIRDatastore")),
    // `BackupRetentionPeriodInDays`' 1..30 range and the `AnalyticsStatus` enum's
    // value set (smithy_query(query="shape", shape="BackupConfiguration")).
    let cases = [
        (
            "a name one character past the 256 maximum",
            json!({ "DatastoreName": "n".repeat(257) }),
        ),
        (
            "an empty name, one below the length minimum of 1",
            json!({ "DatastoreName": "" }),
        ),
        (
            "backup retention one day below the minimum of 1",
            json!({
                "BackupConfiguration": {
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 0,
                    "BackupTagsEnabled": false,
                },
            }),
        ),
        (
            "backup retention one day past the maximum of 30",
            json!({
                "BackupConfiguration": {
                    "Status": "ENABLED",
                    "BackupType": "CONTINUOUS",
                    "RetentionPeriodInDays": 31,
                    "BackupTagsEnabled": false,
                },
            }),
        ),
        (
            "an analytics status outside the modelled value set",
            json!({ "AnalyticsConfiguration": { "Status": "ENABLED_SOMETIMES" } }),
        ),
    ];
    for (case, update) in cases {
        let (status, payload) = service
            .call("UpdateFHIRDatastore", &update_request(&id, &update))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside a modelled constraint must answer 400, \
             got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: a request outside a modelled constraint must answer \
             ValidationException, got {payload}"
        );
        let after = described_properties(&service, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must change nothing, got {after}"
        );
    }
}
