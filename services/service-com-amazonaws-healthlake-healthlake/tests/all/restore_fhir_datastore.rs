// SPDX-License-Identifier: CC0-1.0

//! What `RestoreFHIRDatastore` mints, and what it refuses.
//!
//! The rules under test, each over a SET of inputs rather than one case:
//!
//! 1. a restore mints a NEW data store holding the source data store's FHIR
//!    resources AS OF the requested restore point — each resource at the newest
//!    version written at or before it, and no resource this data store holds no
//!    version of from then (one created later, or one whose every retained
//!    version postdates it) — and leaves the source untouched;
//! 2. every input member that names a property of the restored data store
//!    replaces that property, and one the request omits is inherited from the
//!    data store being restored;
//! 3. the restored data store's tags are the source's exactly when the source's
//!    backup configuration includes them, with the request's tags applied over
//!    them key by key;
//! 4. a restore is refused — and mints nothing — when the source does not exist,
//!    holds no enabled backup, is in a transition state, or the restore point
//!    falls outside the window that runs from the instant backup was enabled on
//!    the source up to now;
//! 5. one `ClientToken` restores one data store: a repeat of the same request
//!    resolves to it, and the same token with a different request is refused.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the harness in `fhir_rest_and_validate_source`, so routing,
//! deserialization, the modelled constraints and the error envelope are all
//! exercised on BOTH wires: the awsJson1_0 control plane and the FHIR REST
//! endpoint a restored data store serves.
//!
//! Every modelled CLIENT error is asserted at HTTP 400 whatever status its shape's
//! `@httpError` names, for the reason `tests/all/error_envelopes.rs` pins: awsJson1_0
//! binds nothing to the transport, so this service reports an error's FAULT.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::fhir_datastore_lifecycle::{error_code, PAST_TRANSITION};
use crate::fhir_rest_and_validate_source::{created, endpoint_path, outcome_code, HealthLake};

/// The gap left between the writes a restore point has to separate.
///
/// Long enough that two FHIR writes land on either side of an instant captured
/// between them whatever the clock's resolution, short enough that a suite of
/// restores stays quick.
const WRITE_GAP: Duration = Duration::from_millis(120);

/// Seconds since the epoch, the scale awsJson1_0 renders a modelled timestamp
/// (`HealthLakeTimestamp`) on.
fn epoch_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// A data store this suite restores from, after it has settled to `ACTIVE`.
struct Datastore {
    id: String,
    arn: String,
    /// The PATH of the `DatastoreEndpoint` the service advertised for it, which
    /// is where its FHIR interactions go.
    path: String,
    /// The `DatastoreProperties` `DescribeFHIRDatastore` reports for it.
    properties: Value,
}

/// Dispatch one control-plane call that must succeed, returning its body.
async fn ok(service: &HealthLake, operation: &str, body: &Value) -> Value {
    let (status, payload) = service.rpc(operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must answer 200, got {status}: {payload}"
    );
    payload
}

/// A required string member of a response body.
fn string_at(body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("the response must carry a string {member}, got {body}"))
        .to_string()
}

/// The `DatastoreProperties` `DescribeFHIRDatastore` reports for `id`.
async fn describe(service: &HealthLake, id: &str) -> Value {
    ok(service, "DescribeFHIRDatastore", &json!({ "DatastoreId": id }))
        .await["DatastoreProperties"]
        .clone()
}

/// Create a data store from `request`, wait out its `CREATING` -> `ACTIVE`
/// transition, and report what the service says about it.
async fn active_datastore(service: &HealthLake, request: &Value) -> Datastore {
    let created = ok(service, "CreateFHIRDatastore", request).await;
    let id = string_at(&created, "DatastoreId");
    tokio::time::sleep(PAST_TRANSITION).await;
    let properties = describe(service, &id).await;
    assert_eq!(
        properties["DatastoreStatus"], "ACTIVE",
        "the data store must be ACTIVE before it is restored from, got {properties}"
    );
    Datastore {
        id,
        arn: string_at(&properties, "DatastoreArn"),
        path: endpoint_path(&string_at(&properties, "DatastoreEndpoint")),
        properties,
    }
}

/// A create request for a data store whose continuous backup is ENABLED — the
/// only kind `RestoreFHIRDatastore` has a restore window for.
///
/// Member names and value sets: `smithy_query(query="shape",
/// shape="BackupConfiguration")` (`Status` one of ENABLED/DISABLED,
/// `RetentionPeriodInDays` in 1..30, `BackupTagsEnabled` a boolean).
fn backup_enabled_create(name: &str, retention_days: i64, backup_tags: bool) -> Value {
    json!({
        "DatastoreTypeVersion": "R4",
        "DatastoreName": name,
        "BackupConfiguration": {
            "Status": "ENABLED",
            "BackupType": "CONTINUOUS",
            "RetentionPeriodInDays": retention_days,
            "BackupTagsEnabled": backup_tags,
        },
    })
}

/// A `RestoreConfiguration` naming `point` as the instant to restore to, or
/// naming no point at all when it is `None`.
fn restore_configuration(point: Option<f64>) -> Value {
    match point {
        Some(point) => json!({
            "ContinuousBackupRestoreConfiguration": { "RestorePointTime": point },
        }),
        None => json!({ "ContinuousBackupRestoreConfiguration": {} }),
    }
}

/// A restore request for `source`, restoring to `point`.
fn restore_request(source: &str, point: Option<f64>) -> Value {
    json!({
        "SourceDatastoreId": source,
        "RestoreConfiguration": restore_configuration(point),
    })
}

/// One FHIR `read` of `resource_type`/`id` against the endpoint path `path`.
async fn read_resource(
    service: &HealthLake,
    path: &str,
    resource_type: &str,
    id: &str,
) -> (u16, Value) {
    let (status, body, _headers) = service
        .fhir("GET", &format!("{path}{resource_type}/{id}"), &Value::Null)
        .await;
    (status, body)
}

/// A FHIR resource this suite wrote: how to address it, and the bytes the source
/// data store stored for it.
struct Written {
    resource_type: String,
    id: String,
    stored: Value,
}

/// Write `resource` to `path` as a FHIR create and remember how to read it back.
async fn write_resource(
    service: &HealthLake,
    path: &str,
    resource_type: &str,
    resource: &Value,
) -> Written {
    let stored = created(service, path, resource_type, resource).await;
    Written {
        resource_type: resource_type.to_string(),
        id: stored["id"]
            .as_str()
            .unwrap_or_else(|| panic!("the create assigns an id, got {stored}"))
            .to_string(),
        stored,
    }
}

/// A `DatastoreId` is 32 lowercase hex characters, which is what the modelled
/// data store ARN (`datastore/fhir/.{32}`) and `DatastoreId`'s own 1..32 length
/// bound together require.
fn assert_restored_identifiers(case: &str, restored: &Value, source: &Datastore) {
    let id = string_at(restored, "DatastoreId");
    assert_eq!(
        id.len(),
        32,
        "case {case:?}: a restored DatastoreId is 32 characters, got {id:?}"
    );
    assert!(
        id.bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "case {case:?}: a restored DatastoreId is lowercase hex, got {id:?}"
    );
    assert_ne!(
        id, source.id,
        "case {case:?}: a restore mints a NEW data store, not the source"
    );
    let arn = string_at(restored, "DatastoreArn");
    assert!(
        arn.ends_with(&format!("datastore/fhir/{id}")) && arn.starts_with("arn:aws:healthlake:"),
        "case {case:?}: the restored ARN must name the restored data store, got {arn:?}"
    );
    assert_eq!(
        restored["DatastoreStatus"], "CREATING",
        "case {case:?}: a restored data store is reported in CREATING, got {restored}"
    );
    assert!(
        string_at(restored, "DatastoreEndpoint").contains(&id),
        "case {case:?}: the restored endpoint must address the restored data store, got {restored}"
    );
}

#[tokio::test]
async fn a_restore_reproduces_the_source_as_of_the_restore_point() {
    let service = HealthLake::new();
    let source = active_datastore(&service, &backup_enabled_create("pitr.source", 7, false)).await;

    // Three FHIR writes with an instant captured before, between and after them,
    // so a restore point can fall in each gap.
    let before_any_write = epoch_secs();
    tokio::time::sleep(WRITE_GAP).await;
    let patient = write_resource(
        &service,
        &source.path,
        "Patient",
        &json!({
            "resourceType": "Patient",
            "name": [{ "family": "Lovelace", "given": ["Ada"] }],
        }),
    )
    .await;
    tokio::time::sleep(WRITE_GAP).await;
    let after_first = epoch_secs();
    tokio::time::sleep(WRITE_GAP).await;
    let observation = write_resource(
        &service,
        &source.path,
        "Observation",
        &json!({
            "resourceType": "Observation",
            "status": "final",
            "valueQuantity": { "value": 36.6, "unit": "C" },
        }),
    )
    .await;
    tokio::time::sleep(WRITE_GAP).await;
    let after_second = epoch_secs();
    tokio::time::sleep(WRITE_GAP).await;
    let practitioner = write_resource(
        &service,
        &source.path,
        "Practitioner",
        &json!({ "resourceType": "Practitioner", "active": true }),
    )
    .await;
    tokio::time::sleep(WRITE_GAP).await;
    let after_third = epoch_secs();

    let written = [&patient, &observation, &practitioner];

    // The rule generalises over WHERE the restore point falls among the writes,
    // so the rows walk it across every gap — and over a request that names no
    // point at all, which restores to the latest instant the backup covers.
    let cases: Vec<(&str, Option<f64>, usize)> = vec![
        ("a point before any resource was written", Some(before_any_write), 0),
        ("a point between the first and the second write", Some(after_first), 1),
        ("a point between the second and the third write", Some(after_second), 2),
        ("a point after every write", Some(after_third), 3),
        ("no restore point, i.e. the latest the backup covers", None, 3),
    ];

    for (case, point, expected) in cases {
        let restored = ok(
            &service,
            "RestoreFHIRDatastore",
            &restore_request(&source.id, point),
        )
        .await;
        assert_restored_identifiers(case, &restored, &source);
        let restored_id = string_at(&restored, "DatastoreId");
        let restored_path = endpoint_path(&string_at(&restored, "DatastoreEndpoint"));

        // The restored data store owes the clock its CREATING -> ACTIVE
        // transition, and serves nothing until it has settled.
        tokio::time::sleep(PAST_TRANSITION).await;
        let properties = describe(&service, &restored_id).await;
        assert_eq!(
            properties["DatastoreStatus"], "ACTIVE",
            "case {case:?}: the restored data store settles to ACTIVE, got {properties}"
        );
        assert_eq!(
            string_at(&properties, "DatastoreEndpoint"),
            string_at(&restored, "DatastoreEndpoint"),
            "case {case:?}: the restore must advertise the endpoint Describe reports, \
             got {properties}"
        );

        for (index, resource) in written.iter().enumerate() {
            let (status, body) =
                read_resource(&service, &restored_path, &resource.resource_type, &resource.id)
                    .await;
            if index < expected {
                assert_eq!(
                    status, 200,
                    "case {case:?}: the restored data store must hold the {} written at or \
                     before the restore point, got {status}: {body}",
                    resource.resource_type
                );
                assert_eq!(
                    body, resource.stored,
                    "case {case:?}: a restored resource is the stored resource verbatim, \
                     got {body}"
                );
            } else {
                assert_eq!(
                    status, 404,
                    "case {case:?}: the restored data store must NOT hold the {} written after \
                     the restore point, got {status}: {body}",
                    resource.resource_type
                );
                assert_eq!(
                    outcome_code(&body),
                    "not-found",
                    "case {case:?}: a resource the restore point excludes reads as not-found, \
                     got {body}"
                );
            }
        }

        // The source is REPORTED from, never written to: every resource it held
        // is still readable there after the restore.
        for resource in written {
            let (status, body) =
                read_resource(&service, &source.path, &resource.resource_type, &resource.id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a restore leaves the source's {} in place, got {status}: {body}",
                resource.resource_type
            );
        }
    }
}

/// Every input member of `RestoreFHIRDatastore` that names a property of the
/// restored data store, as `DatastoreProperties` reports it.
///
/// Source: `smithy_query(query="operation", operation="RestoreFHIRDatastore")` —
/// these are its input members other than `SourceDatastoreId`,
/// `RestoreConfiguration`, `ClientToken` and `Tags` (the tags have their own
/// rule below), and `smithy_query(query="shape", shape="DatastoreProperties")`
/// reports each under its own name.
const OVERRIDABLE_PROPERTIES: &[&str] = &[
    "DatastoreName",
    "SseConfiguration",
    "IdentityProviderConfiguration",
    "AnalyticsConfiguration",
    "NlpConfiguration",
    "ProfileConfiguration",
];

#[tokio::test]
async fn a_restore_applies_the_members_it_carries_and_inherits_the_rest() {
    let service = HealthLake::new();
    // A source carrying every property a restore can override, plus two it
    // cannot (`PreloadDataConfig`, `DatastoreTypeVersion`), so an inherited
    // property is observably carried over rather than vacuously absent.
    let source = active_datastore(
        &service,
        &json!({
            "DatastoreTypeVersion": "R4",
            "DatastoreName": "inherited.source",
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
                "RetentionPeriodInDays": 30,
                "BackupTagsEnabled": false,
            },
        }),
    )
    .await;

    // The rule generalises over WHICH members a request carries and over the
    // form of their values, so the rows differ along both: none, one, several,
    // all; each enum flipped to the other value the model declares; a name in
    // unicode and a name at the modelled 256-character maximum; an empty list
    // and a list of two.
    let cases: Vec<(&str, Value)> = vec![
        ("no overrides at all", json!({})),
        (
            "an ASCII name only",
            json!({ "DatastoreName": "restored-store" }),
        ),
        (
            "a unicode name with the punctuation the modelled pattern allows",
            json!({ "DatastoreName": "données.santé_2026+eu" }),
        ),
        (
            "a name at the modelled 256-character maximum",
            json!({ "DatastoreName": "r".repeat(256) }),
        ),
        (
            "analytics flipped to DISABLED",
            json!({ "AnalyticsConfiguration": { "Status": "DISABLED" } }),
        ),
        (
            "NLP flipped to ENABLED and the default profile list emptied",
            json!({
                "NlpConfiguration": { "Status": "ENABLED" },
                "ProfileConfiguration": { "DefaultProfiles": [] },
            }),
        ),
        (
            "a customer-managed key and a SMART on FHIR identity provider",
            json!({
                "SseConfiguration": {
                    "KmsEncryptionConfig": {
                        "CmkType": "CUSTOMER_MANAGED_KMS_KEY",
                        "KmsKeyId":
                            "arn:aws:kms:us-east-1:000000000000:key/00000000-0000-0000-0000-000000000000",
                    },
                },
                "IdentityProviderConfiguration": {
                    "AuthorizationStrategy": "SMART_ON_FHIR",
                    "FineGrainedAuthorizationEnabled": true,
                    "Metadata": "{\"issuer\":\"https://example.test\"}",
                },
            }),
        ),
        (
            "every overridable member at once",
            json!({
                "DatastoreName": "fully-overridden",
                "SseConfiguration": {
                    "KmsEncryptionConfig": { "CmkType": "AWS_OWNED_KMS_KEY" },
                },
                "AnalyticsConfiguration": { "Status": "DISABLED" },
                "NlpConfiguration": { "Status": "ENABLED" },
                "ProfileConfiguration": {
                    "DefaultProfiles": [
                        "http://hl7.org/fhir/us/core/StructureDefinition/us-core-patient",
                        "http://hl7.org/fhir/StructureDefinition/Observation",
                    ],
                },
                "IdentityProviderConfiguration": {
                    "AuthorizationStrategy": "SMART_ON_FHIR",
                    "FineGrainedAuthorizationEnabled": true,
                },
            }),
        ),
    ];

    for (case, overrides) in cases {
        let mut request = restore_request(&source.id, None);
        for (member, value) in overrides
            .as_object()
            .expect("every row's overrides are an object")
        {
            request[member] = value.clone();
        }
        let restored = ok(&service, "RestoreFHIRDatastore", &request).await;
        assert_restored_identifiers(case, &restored, &source);
        let restored_id = string_at(&restored, "DatastoreId");

        tokio::time::sleep(PAST_TRANSITION).await;
        let properties = describe(&service, &restored_id).await;

        for member in OVERRIDABLE_PROPERTIES {
            let expected = overrides
                .get(member)
                .unwrap_or(&source.properties[*member]);
            assert_eq!(
                &properties[*member], expected,
                "case {case:?}: {member} must be the one the request carried, else the source's; \
                 got {}",
                properties[*member]
            );
        }
        // Properties no input of this operation names are the source's: the
        // restored data store holds the source's resources, so it speaks the
        // same release version and carries the same preloaded data.
        for member in ["DatastoreTypeVersion", "PreloadDataConfig"] {
            assert_eq!(
                properties[member], source.properties[member],
                "case {case:?}: {member} is inherited from the source, got {properties}"
            );
        }
        // This operation carries no `BackupConfiguration`, so the restored data
        // store has no backup of its own until one is configured on it.
        assert!(
            properties.get("BackupStatusInfo").is_none(),
            "case {case:?}: a restored data store starts with no backup configuration, \
             got {properties}"
        );
        assert!(
            properties["CreatedAt"].as_f64().unwrap_or_default()
                >= source.properties["CreatedAt"].as_f64().unwrap_or_default(),
            "case {case:?}: the restored data store is created by THIS call, so its CreatedAt \
             is not before the source's, got {properties}"
        );
    }
}

/// One row of the tag rule: whether the source's backup includes its tags, what
/// the restore request asks for, and the tags the restored data store must
/// carry.
struct TagCase {
    case: &'static str,
    backup_tags: bool,
    requested: Value,
    expected: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn restored_tags_follow_the_sources_backup_tags_setting() {
    let service = HealthLake::new();

    // The rule generalises over whether the backup includes the source's tags
    // and over what the request asks for, so the rows span both: tags included
    // or not, and a request carrying none, a disjoint key, or a key the source
    // already has.
    let cases = vec![
        TagCase {
            case: "backup tags included and no tags requested",
            backup_tags: true,
            requested: Value::Null,
            expected: &[("env", "test"), ("team", "clinical")],
        },
        TagCase {
            case: "backup tags included and a disjoint key requested",
            backup_tags: true,
            requested: json!([{ "Key": "purpose", "Value": "restore" }]),
            expected: &[
                ("env", "test"),
                ("purpose", "restore"),
                ("team", "clinical"),
            ],
        },
        TagCase {
            case: "backup tags included and a key the source already carries requested",
            backup_tags: true,
            requested: json!([{ "Key": "env", "Value": "restored" }]),
            expected: &[("env", "restored"), ("team", "clinical")],
        },
        TagCase {
            case: "backup tags excluded and no tags requested",
            backup_tags: false,
            requested: Value::Null,
            expected: &[],
        },
        TagCase {
            case: "backup tags excluded and two keys requested",
            backup_tags: false,
            requested: json!([
                { "Key": "env", "Value": "restored" },
                { "Key": "purpose", "Value": "restore" },
            ]),
            expected: &[("env", "restored"), ("purpose", "restore")],
        },
    ];

    for TagCase {
        case,
        backup_tags,
        requested,
        expected,
    } in cases
    {
        let mut create = backup_enabled_create("tagged.source", 14, backup_tags);
        create["Tags"] = json!([
            { "Key": "env", "Value": "test" },
            { "Key": "team", "Value": "clinical" },
        ]);
        let source = active_datastore(&service, &create).await;

        let mut request = restore_request(&source.id, None);
        if !requested.is_null() {
            request["Tags"] = requested.clone();
        }
        let restored = ok(&service, "RestoreFHIRDatastore", &request).await;
        let restored_arn = string_at(&restored, "DatastoreArn");

        let listed = ok(
            &service,
            "ListTagsForResource",
            &json!({ "ResourceARN": restored_arn }),
        )
        .await;
        let mut actual: Vec<(String, String)> = listed["Tags"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|tag| (string_at(tag, "Key"), string_at(tag, "Value")))
            .collect();
        actual.sort();
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: the restored data store's tags must be the backed-up ones with the \
             request's applied over them, got {actual:?}"
        );

        // The source keeps its own tags whatever the restore asked for.
        let source_tags = ok(
            &service,
            "ListTagsForResource",
            &json!({ "ResourceARN": source.arn }),
        )
        .await;
        assert_eq!(
            source_tags["Tags"].as_array().map(Vec::len),
            Some(2),
            "case {case:?}: a restore leaves the source's tags alone, got {source_tags}"
        );
    }
}

#[tokio::test]
async fn a_restore_outside_the_rules_is_refused_and_mints_nothing() {
    let service = HealthLake::new();
    let before_any_datastore = epoch_secs();
    let source = active_datastore(&service, &backup_enabled_create("refused.source", 7, false))
        .await;
    let without_backup = active_datastore(
        &service,
        &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "no.backup" }),
    )
    .await;
    let backup_disabled = active_datastore(
        &service,
        &json!({
            "DatastoreTypeVersion": "R4",
            "DatastoreName": "backup.disabled",
            "BackupConfiguration": {
                "Status": "DISABLED",
                "BackupType": "CONTINUOUS",
                "RetentionPeriodInDays": 7,
                "BackupTagsEnabled": false,
            },
        }),
    )
    .await;

    let live_datastores = ok(&service, "ListFHIRDatastores", &json!({}))
        .await["DatastorePropertiesList"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();

    // One row per way a restore can be wrong: an id no data store holds, a
    // source with nothing to restore from, and a restore point on either side of
    // the window the service reports.
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a source id no data store holds",
            restore_request(&"0".repeat(32), None),
            "ResourceNotFoundException",
        ),
        (
            "a source that was never configured for backup",
            restore_request(&without_backup.id, None),
            "ValidationException",
        ),
        (
            "a source whose backup configuration is DISABLED",
            restore_request(&backup_disabled.id, None),
            "ValidationException",
        ),
        (
            "a restore point in the future",
            restore_request(&source.id, Some(epoch_secs() + 3600.0)),
            "ValidationException",
        ),
        (
            "a restore point before backup was enabled on the source",
            restore_request(&source.id, Some(before_any_datastore)),
            "ValidationException",
        ),
        (
            "a restore point at the start of the epoch",
            restore_request(&source.id, Some(0.0)),
            "ValidationException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, payload) = service.rpc("RestoreFHIRDatastore", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused restore answers 400, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            expected,
            "case {case:?}: expected {expected}, got {payload}"
        );
    }

    // A source in a transition state is refused with the error the model
    // documents for exactly that ("the data store is in a transition state"),
    // whichever transition it is in.
    let creating = ok(
        &service,
        "CreateFHIRDatastore",
        &backup_enabled_create("still.creating", 7, false),
    )
    .await;
    let (status, payload) = service
        .rpc(
            "RestoreFHIRDatastore",
            &restore_request(&string_at(&creating, "DatastoreId"), None),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"a CREATING source\": a refused restore answers 400, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ConflictException",
        "case \"a CREATING source\": a source still in CREATING is a transition-state \
         conflict, got {payload}"
    );

    let deleting = active_datastore(&service, &backup_enabled_create("to.delete", 7, false)).await;
    ok(
        &service,
        "DeleteFHIRDatastore",
        &json!({ "DatastoreId": deleting.id }),
    )
    .await;
    let (status, payload) = service
        .rpc("RestoreFHIRDatastore", &restore_request(&deleting.id, None))
        .await;
    assert_eq!(
        status, 400,
        "case \"a DELETING source\": a refused restore answers 400, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ConflictException",
        "case \"a DELETING source\": a source on its way out is a transition-state conflict, \
         got {payload}"
    );
    // Once the delete has settled the data store is gone, not in transition.
    tokio::time::sleep(PAST_TRANSITION).await;
    let (status, payload) = service
        .rpc("RestoreFHIRDatastore", &restore_request(&deleting.id, None))
        .await;
    assert_eq!(
        status, 400,
        "case \"a DELETED source\": a refused restore answers 400, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ResourceNotFoundException",
        "case \"a DELETED source\": a deleted data store is gone rather than in transition, \
         got {payload}"
    );

    // Not one of those refusals minted a data store: the registry holds exactly
    // the sources this test created, plus the two it created for the transition
    // rows.
    let listed = ok(&service, "ListFHIRDatastores", &json!({})).await;
    assert_eq!(
        listed["DatastorePropertiesList"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        live_datastores + 2,
        "a refused restore must mint nothing, got {listed}"
    );
}

#[tokio::test]
async fn one_client_token_restores_one_datastore() {
    let service = HealthLake::new();
    let source = active_datastore(&service, &backup_enabled_create("token.source", 7, false)).await;

    let mut request = restore_request(&source.id, None);
    request["ClientToken"] = json!("restore-token-1");
    request["DatastoreName"] = json!("token-restored");

    let first = ok(&service, "RestoreFHIRDatastore", &request).await;
    let repeat = ok(&service, "RestoreFHIRDatastore", &request).await;
    assert_eq!(
        string_at(&repeat, "DatastoreId"),
        string_at(&first, "DatastoreId"),
        "a repeat of the SAME request with the same ClientToken resolves to the data store it \
         already restored, got {repeat}"
    );
    assert_eq!(
        string_at(&repeat, "DatastoreArn"),
        string_at(&first, "DatastoreArn"),
        "the repeat reports the same ARN, got {repeat}"
    );

    // The same token with a DIFFERENT request is a different request.
    let mut different = request.clone();
    different["DatastoreName"] = json!("token-restored-elsewhere");
    let (status, payload) = service.rpc("RestoreFHIRDatastore", &different).await;
    assert_eq!(
        status, 400,
        "a token reused for a different request is refused, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "a token reused for a different request is a validation failure, got {payload}"
    );

    // A fresh token restores a second data store.
    let mut fresh = request.clone();
    fresh["ClientToken"] = json!("restore-token-2");
    let second = ok(&service, "RestoreFHIRDatastore", &fresh).await;
    assert_ne!(
        string_at(&second, "DatastoreId"),
        string_at(&first, "DatastoreId"),
        "a fresh ClientToken restores a NEW data store, got {second}"
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    let listed = ok(&service, "ListFHIRDatastores", &json!({})).await;
    assert_eq!(
        listed["DatastorePropertiesList"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        3,
        "the source and the two restored data stores are all that exist, got {listed}"
    );
}

#[tokio::test]
async fn the_restorable_window_runs_from_when_backup_was_enabled_to_now() {
    let service = HealthLake::new();
    // A retention period cannot bind the window of a data store younger than
    // itself, so the earliest restorable point here is the instant backup became
    // enabled — for every retention the modelled 1..30 range allows, at each
    // published bound and in between.
    for retention_days in [1, 7, 30] {
        let case = format!("{retention_days}-day retention");
        let before_backup = epoch_secs();
        tokio::time::sleep(WRITE_GAP).await;
        let source = active_datastore(
            &service,
            &backup_enabled_create("window.source", retention_days, false),
        )
        .await;
        let after_backup = epoch_secs();

        // The instant the service says backup became enabled is between the two
        // the rows restore at, so one falls before the window and one inside it.
        let enabled_at = source.properties["BackupStatusInfo"]["BackupEnabledAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: backup is enabled, got {}", source.properties));
        assert!(
            enabled_at > before_backup && enabled_at < after_backup,
            "case {case:?}: BackupEnabledAt ({enabled_at}) must fall between the instants this \
             test captured around the create ({before_backup}, {after_backup})"
        );

        // Inside the window: at or after the instant backup was enabled and not
        // in the future.
        let inside = ok(
            &service,
            "RestoreFHIRDatastore",
            &restore_request(&source.id, Some(after_backup)),
        )
        .await;
        assert_restored_identifiers(&case, &inside, &source);

        // Outside it, on either side.
        for (bound, point) in [
            ("before backup was enabled", before_backup),
            ("in the future", epoch_secs() + 3600.0),
        ] {
            let (status, payload) = service
                .rpc(
                    "RestoreFHIRDatastore",
                    &restore_request(&source.id, Some(point)),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a restore point {bound} is refused, got {status}: {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ValidationException",
                "case {case:?}: a restore point {bound} is a validation failure, got {payload}"
            );
            assert!(
                payload
                    .get("Message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .contains("RestorePointTime"),
                "case {case:?}: the refusal of a point {bound} names the member it refused, \
                 got {payload}"
            );
        }
    }
}

#[tokio::test]
async fn a_restored_datastore_serves_its_own_fhir_endpoint() {
    let service = HealthLake::new();
    let source = active_datastore(&service, &backup_enabled_create("writable.source", 7, false))
        .await;
    let patient = write_resource(
        &service,
        &source.path,
        "Patient",
        &json!({ "resourceType": "Patient", "active": true }),
    )
    .await;

    let restored = ok(
        &service,
        "RestoreFHIRDatastore",
        &restore_request(&source.id, None),
    )
    .await;
    let restored_path = endpoint_path(&string_at(&restored, "DatastoreEndpoint"));

    // Until the restore has settled, the restored endpoint serves nothing.
    let (status, body) =
        read_resource(&service, &restored_path, &patient.resource_type, &patient.id).await;
    assert_eq!(
        status, 404,
        "a CREATING data store serves no FHIR interaction, got {status}: {body}"
    );

    tokio::time::sleep(PAST_TRANSITION).await;
    let (status, body) =
        read_resource(&service, &restored_path, &patient.resource_type, &patient.id).await;
    assert_eq!(
        status, 200,
        "the settled restore serves the copied resource, got {status}: {body}"
    );

    // The two data stores are independent from the restore on: a write to the
    // restored one is not visible on the source.
    let fresh = write_resource(
        &service,
        &restored_path,
        "Observation",
        &json!({ "resourceType": "Observation", "status": "preliminary" }),
    )
    .await;
    let (status, body) = read_resource(&service, &source.path, &fresh.resource_type, &fresh.id).await;
    assert_eq!(
        status, 404,
        "a write to the restored data store must not appear on the source, got {status}: {body}"
    );
    let (status, body) =
        read_resource(&service, &restored_path, &fresh.resource_type, &fresh.id).await;
    assert_eq!(
        status, 200,
        "the restored data store serves its own writes, got {status}: {body}"
    );
}

/// Write `resource` to `path` as a FHIR `update` (`PUT [base]/{type}/{id}`) and
/// return the version it stored, after asserting the status and the version the
/// write must land on.
///
/// `expected_status` is 201 for the write that creates the resource at that id
/// (FHIR's `update-as-create`) and 200 for one that supersedes a version held
/// there.
async fn put_version(
    service: &HealthLake,
    path: &str,
    resource_type: &str,
    id: &str,
    resource: &Value,
    expected_status: u16,
    expected_version: u64,
) -> Value {
    let (status, body, _headers) = service
        .fhir("PUT", &format!("{path}{resource_type}/{id}"), resource)
        .await;
    assert_eq!(
        status, expected_status,
        "the update of {resource_type}/{id} must answer {expected_status}, got {status}: {body}"
    );
    assert_eq!(
        body["meta"]["versionId"],
        json!(expected_version.to_string()),
        "the update must land on version {expected_version}, got {body}"
    );
    body
}

/// One FHIR resource this suite wrote more than once: how to address it, and the
/// body each of its versions was stored as, oldest first (index 0 is
/// `meta.versionId` 1).
struct Versioned {
    resource_type: &'static str,
    id: String,
    versions: Vec<Value>,
}

/// One row of the restore-point table: where the restore point falls, and the
/// index of the version each written resource must be restored at — `None` for
/// a resource the restored data store must hold no version of.
struct RestorePointCase {
    case: &'static str,
    point: Option<f64>,
    expected: [Option<usize>; 3],
}

/// Read `resource` back from the data store serving `path` and assert it is the
/// version `expected` names — the index of a version in [`Versioned::versions`],
/// or `None` for a resource that data store must hold no version of.
async fn assert_restored_version(
    case: &str,
    service: &HealthLake,
    path: &str,
    resource: &Versioned,
    expected: Option<usize>,
) {
    let (status, body) = read_resource(service, path, resource.resource_type, &resource.id).await;
    match expected {
        Some(index) => {
            assert_eq!(
                status, 200,
                "case {case:?}: the restored data store must hold {}/{} at version {}, got \
                 {status}: {body}",
                resource.resource_type,
                resource.id,
                index + 1
            );
            assert_eq!(
                body, resource.versions[index],
                "case {case:?}: {}/{} must be restored at the version that was current then \
                 (version {}), got {body}",
                resource.resource_type,
                resource.id,
                index + 1
            );
            assert_eq!(
                body["meta"]["versionId"],
                json!((index + 1).to_string()),
                "case {case:?}: the restored {} must report the version that was current then, \
                 got {body}",
                resource.resource_type
            );
        }
        None => {
            assert_eq!(
                status, 404,
                "case {case:?}: the restored data store must hold NO version of {}/{}, got \
                 {status}: {body}",
                resource.resource_type, resource.id
            );
            assert_eq!(
                outcome_code(&body),
                "not-found",
                "case {case:?}: a resource the restore point reaches no version of reads as \
                 not-found, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_restore_reproduces_each_resource_at_the_version_current_at_the_restore_point() {
    let service = HealthLake::new();
    let source =
        active_datastore(&service, &backup_enabled_create("pitr.history", 7, false)).await;

    // Three resources that differ along what the rule generalises over: how many
    // versions they have (three, one, two), which interaction wrote the first
    // one (a server-assigned id from a POST, a caller-chosen id from a PUT), the
    // resource TYPE, and the form of each version's content. An instant is
    // captured between every pair of writes, so a restore point can fall in each
    // gap.
    let before_any_write = epoch_secs();
    tokio::time::sleep(WRITE_GAP).await;

    let patient = write_resource(
        &service,
        &source.path,
        "Patient",
        &json!({
            "resourceType": "Patient",
            "active": true,
            "name": [{ "family": "Lovelace", "given": ["Ada"] }],
        }),
    )
    .await;
    let mut patient_versions = vec![patient.stored.clone()];
    tokio::time::sleep(WRITE_GAP).await;
    let after_patient_v1 = epoch_secs();
    tokio::time::sleep(WRITE_GAP).await;

    let observation = write_resource(
        &service,
        &source.path,
        "Observation",
        &json!({
            "resourceType": "Observation",
            "status": "final",
            "valueQuantity": { "value": 36.6, "unit": "C" },
        }),
    )
    .await;
    tokio::time::sleep(WRITE_GAP).await;
    let after_observation = epoch_secs();
    tokio::time::sleep(WRITE_GAP).await;

    patient_versions.push(
        put_version(
            &service,
            &source.path,
            "Patient",
            &patient.id,
            &json!({ "resourceType": "Patient", "id": patient.id, "active": false }),
            200,
            2,
        )
        .await,
    );
    tokio::time::sleep(WRITE_GAP).await;
    let after_patient_v2 = epoch_secs();
    tokio::time::sleep(WRITE_GAP).await;

    // A caller-chosen id, written first by the update interaction itself.
    let practitioner_id = "pitr.history-practitioner".to_string();
    let mut practitioner_versions = vec![
        put_version(
            &service,
            &source.path,
            "Practitioner",
            &practitioner_id,
            &json!({
                "resourceType": "Practitioner",
                "id": practitioner_id,
                "active": true,
            }),
            201,
            1,
        )
        .await,
    ];
    tokio::time::sleep(WRITE_GAP).await;
    let after_practitioner_v1 = epoch_secs();
    tokio::time::sleep(WRITE_GAP).await;

    patient_versions.push(
        put_version(
            &service,
            &source.path,
            "Patient",
            &patient.id,
            &json!({
                "resourceType": "Patient",
                "id": patient.id,
                "active": true,
                "telecom": [{ "system": "email", "value": "ada@example.test" }],
            }),
            200,
            3,
        )
        .await,
    );
    tokio::time::sleep(WRITE_GAP).await;
    let after_patient_v3 = epoch_secs();
    tokio::time::sleep(WRITE_GAP).await;

    practitioner_versions.push(
        put_version(
            &service,
            &source.path,
            "Practitioner",
            &practitioner_id,
            &json!({
                "resourceType": "Practitioner",
                "id": practitioner_id,
                "active": false,
                "qualification": [{ "code": { "text": "MD" } }],
            }),
            200,
            2,
        )
        .await,
    );
    tokio::time::sleep(WRITE_GAP).await;
    let after_every_write = epoch_secs();

    let written = [
        Versioned {
            resource_type: "Patient",
            id: patient.id.clone(),
            versions: patient_versions,
        },
        Versioned {
            resource_type: "Observation",
            id: observation.id.clone(),
            versions: vec![observation.stored.clone()],
        },
        Versioned {
            resource_type: "Practitioner",
            id: practitioner_id.clone(),
            versions: practitioner_versions,
        },
    ];

    // The rule generalises over WHERE the restore point falls among the writes,
    // so the rows walk it across every gap — and over a request naming no point
    // at all, which restores to the latest instant the backup covers. Each row
    // names, per resource, the index of the version that was current then.
    let cases = vec![
        RestorePointCase {
            case: "before any resource was written",
            point: Some(before_any_write),
            expected: [None, None, None],
        },
        RestorePointCase {
            case: "after the Patient's first version only",
            point: Some(after_patient_v1),
            expected: [Some(0), None, None],
        },
        RestorePointCase {
            case: "after the Observation, which is never updated",
            point: Some(after_observation),
            expected: [Some(0), Some(0), None],
        },
        RestorePointCase {
            case: "after the Patient's second version",
            point: Some(after_patient_v2),
            expected: [Some(1), Some(0), None],
        },
        RestorePointCase {
            case: "after the Practitioner was created at a caller-chosen id",
            point: Some(after_practitioner_v1),
            expected: [Some(1), Some(0), Some(0)],
        },
        RestorePointCase {
            case: "after the Patient's third version",
            point: Some(after_patient_v3),
            expected: [Some(2), Some(0), Some(0)],
        },
        RestorePointCase {
            case: "after every write",
            point: Some(after_every_write),
            expected: [Some(2), Some(0), Some(1)],
        },
        RestorePointCase {
            case: "no restore point, i.e. the latest the backup covers",
            point: None,
            expected: [Some(2), Some(0), Some(1)],
        },
    ];

    for RestorePointCase {
        case,
        point,
        expected,
    } in cases
    {
        let restored = ok(
            &service,
            "RestoreFHIRDatastore",
            &restore_request(&source.id, point),
        )
        .await;
        assert_restored_identifiers(case, &restored, &source);
        let restored_path = endpoint_path(&string_at(&restored, "DatastoreEndpoint"));
        tokio::time::sleep(PAST_TRANSITION).await;

        for (resource, expected) in written.iter().zip(expected) {
            assert_restored_version(case, &service, &restored_path, resource, expected).await;
        }

        // The source is REPORTED from, never written to: every resource it holds
        // is still readable there at its newest version after the restore.
        for resource in &written {
            let newest = Some(resource.versions.len() - 1);
            assert_restored_version(case, &service, &source.path, resource, newest).await;
        }
    }
}

#[tokio::test]
async fn a_restore_reaches_the_versions_the_data_plane_still_retains() {
    // The retained-version ceiling is this emulator's own
    // (`crate::state::MAX_FHIR_RESOURCE_VERSIONS`): a resource keeps that many
    // versions, and a write past it drops the OLDEST. So a restore point reaches
    // every version still retained, and a resource whose version from that
    // instant has been dropped is not restored — the same answer as a restore
    // point before it existed.
    let retained = service_com_amazonaws_healthlake_healthlake::state::MAX_FHIR_RESOURCE_VERSIONS;
    let service = HealthLake::new();
    let source =
        active_datastore(&service, &backup_enabled_create("pitr.ceiling", 7, false)).await;

    let patient = write_resource(
        &service,
        &source.path,
        "Patient",
        &json!({ "resourceType": "Patient", "active": true }),
    )
    .await;
    let mut versions = vec![patient.stored.clone()];
    // An instant after each version was written, so a restore point can name the
    // span every single version was current for.
    tokio::time::sleep(WRITE_GAP).await;
    let mut after_version = vec![epoch_secs()];

    // One write past the ceiling, so exactly the oldest version is dropped.
    for write in 2..=(retained + 1) {
        tokio::time::sleep(WRITE_GAP).await;
        versions.push(
            put_version(
                &service,
                &source.path,
                "Patient",
                &patient.id,
                &json!({
                    "resourceType": "Patient",
                    "id": patient.id,
                    "active": write % 2 == 0,
                    "multipleBirthInteger": write,
                }),
                200,
                write as u64,
            )
            .await,
        );
        tokio::time::sleep(WRITE_GAP).await;
        after_version.push(epoch_secs());
    }

    let resource = Versioned {
        resource_type: "Patient",
        id: patient.id.clone(),
        versions,
    };

    // The rows walk the ceiling: the version just past it (dropped), the oldest
    // version still retained, one in the middle, and the newest — reached both
    // by naming its instant and by naming no restore point at all.
    let cases: Vec<(&str, Option<f64>, Option<usize>)> = vec![
        (
            "the span of the one version the ceiling dropped",
            Some(after_version[0]),
            None,
        ),
        (
            "the span of the oldest version still retained",
            Some(after_version[1]),
            Some(1),
        ),
        (
            "the span of a version in the middle of what is retained",
            Some(after_version[retained / 2]),
            Some(retained / 2),
        ),
        (
            "the span of the newest version",
            Some(after_version[retained]),
            Some(retained),
        ),
        (
            "no restore point, i.e. the latest the backup covers",
            None,
            Some(retained),
        ),
    ];

    for (case, point, expected) in cases {
        let restored = ok(
            &service,
            "RestoreFHIRDatastore",
            &restore_request(&source.id, point),
        )
        .await;
        let restored_path = endpoint_path(&string_at(&restored, "DatastoreEndpoint"));
        tokio::time::sleep(PAST_TRANSITION).await;
        assert_restored_version(case, &service, &restored_path, &resource, expected).await;
    }
}

#[tokio::test]
async fn a_restore_before_an_update_carries_the_version_that_update_superseded() {
    // The reported case: a resource CREATED before the restore point and UPDATED
    // after it is restored at the version it had at that instant, not dropped.
    let service = HealthLake::new();
    let source =
        active_datastore(&service, &backup_enabled_create("updated.source", 7, false)).await;
    let patient = write_resource(
        &service,
        &source.path,
        "Patient",
        &json!({ "resourceType": "Patient", "active": true }),
    )
    .await;

    tokio::time::sleep(WRITE_GAP).await;
    let between = epoch_secs();
    tokio::time::sleep(WRITE_GAP).await;

    // The FHIR update interaction supersedes the version a read answers, and the
    // superseded version stays retained for an earlier instant to be read at.
    let updated = put_version(
        &service,
        &source.path,
        "Patient",
        &patient.id,
        &json!({ "resourceType": "Patient", "id": patient.id, "active": false }),
        200,
        2,
    )
    .await;

    // Restoring to an instant between the two writes carries the version that
    // was current then — version 1, verbatim, not the content the update wrote.
    let before = ok(
        &service,
        "RestoreFHIRDatastore",
        &restore_request(&source.id, Some(between)),
    )
    .await;
    let before_path = endpoint_path(&string_at(&before, "DatastoreEndpoint"));
    tokio::time::sleep(PAST_TRANSITION).await;
    let (status, body) = read_resource(&service, &before_path, "Patient", &patient.id).await;
    assert_eq!(
        status, 200,
        "a resource the source held at the restore point is restored, got {status}: {body}"
    );
    assert_eq!(
        body, patient.stored,
        "the restored resource is the version that was current at the restore point, verbatim, \
         got {body}"
    );

    // Restoring to an instant AFTER the update carries the version it wrote.
    let after = ok(
        &service,
        "RestoreFHIRDatastore",
        &restore_request(&source.id, None),
    )
    .await;
    let after_path = endpoint_path(&string_at(&after, "DatastoreEndpoint"));
    tokio::time::sleep(PAST_TRANSITION).await;
    let (status, body) = read_resource(&service, &after_path, "Patient", &patient.id).await;
    assert_eq!(
        status, 200,
        "a restore past the update holds the updated resource, got {status}: {body}"
    );
    assert_eq!(
        body, updated,
        "the restored resource is the stored version verbatim, got {body}"
    );
}
