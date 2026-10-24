// SPDX-License-Identifier: CC0-1.0

//! The two documented EFFECTS of the configuration members `CreateFHIRDatastore`
//! takes for encryption and for preloaded data — as opposed to the members being
//! echoed back.
//!
//! The rules under test, each over a SET of inputs rather than one recorded case:
//!
//! 1. a data store is ALWAYS encrypted, so every read of one reports the key that
//!    encrypts it: the customer-managed key its create named, or else the
//!    service-owned key the modelled `CmkType` carries for that case
//!    (`KmsEncryptionConfig` is documented "If a customer-owned key is not
//!    specified, an Amazon Web Services-owned key is used for encryption").
//!    Asserted through BOTH readers of `DatastoreProperties`
//!    (`DescribeFHIRDatastore` and `ListFHIRDatastores`), for every form the member
//!    takes and whatever else the create carried, and for the data store
//!    `RestoreFHIRDatastore` mints as well as the one `CreateFHIRDatastore` does;
//! 2. a create that asks for preloaded data brings the data store UP holding it:
//!    `PreloadDataConfig` is "an optional parameter to preload (import) open source
//!    Synthea FHIR data upon creation of the data store", so a preloaded data store
//!    serves FHIR R4 resources on its endpoint from the moment it is `ACTIVE`, each
//!    one readable through the same `read` interaction a created resource is,
//!    while a create that asks for none comes up empty.
//!
//! Every call goes through
//! [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter) by way
//! of the harness in `fhir_rest_and_validate_source`, on both of this service's wires:
//! the awsJson1_0 control plane and a data store's FHIR REST endpoint.
//!
//! The preloaded resources are LISTED from the shared store the handlers write,
//! because the FHIR `search` interaction that would enumerate them over the wire is
//! still unimplemented (its own gap); each one is then READ over the data store's
//! advertised endpoint, so what the assertions check is what a client is served.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::service::synthea::{
    PRELOADED_RECORD_COUNT, PRELOADED_RESOURCE_COUNT,
};
use service_com_amazonaws_healthlake_healthlake::state::ComAmazonawsHealthlakeHealthlakeState;

use crate::fhir_datastore_lifecycle::{error_code, PAST_TRANSITION};
use crate::fhir_rest_and_validate_source::{endpoint_path, outcome_code, HealthLake};

/// The `SseConfiguration` a data store created without a customer-provided key
/// reports: the service-owned key, and no `KmsKeyId` — a service-owned key is not one
/// a caller names.
fn service_owned_key() -> Value {
    json!({ "KmsEncryptionConfig": { "CmkType": "AWS_OWNED_KMS_KEY" } })
}

/// A data store this suite created, after it has settled to `ACTIVE`.
struct Datastore {
    id: String,
    /// The PATH of the `DatastoreEndpoint` the service advertised for it, which is
    /// where its FHIR interactions go.
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

/// Create a data store from `request`, wait out its `CREATING` -> `ACTIVE`
/// transition, and report what the service says about it.
async fn active_datastore(service: &HealthLake, request: &Value) -> Datastore {
    let created = ok(service, "CreateFHIRDatastore", request).await;
    let id = string_at(&created, "DatastoreId");
    tokio::time::sleep(PAST_TRANSITION).await;
    let properties = ok(service, "DescribeFHIRDatastore", &json!({ "DatastoreId": id })).await
        ["DatastoreProperties"]
        .clone();
    assert_eq!(
        properties["DatastoreStatus"], "ACTIVE",
        "the data store must be ACTIVE before its endpoint is used, got {properties}"
    );
    Datastore {
        path: endpoint_path(&string_at(&properties, "DatastoreEndpoint")),
        id,
        properties,
    }
}

/// The `ListFHIRDatastores` entry for `id`.
async fn listed(service: &HealthLake, id: &str) -> Value {
    let body = ok(service, "ListFHIRDatastores", &json!({})).await;
    body["DatastorePropertiesList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|entry| entry["DatastoreId"] == json!(id))
        .unwrap_or_else(|| panic!("ListFHIRDatastores must report the data store {id}, got {body}"))
}

/// Every FHIR resource the data store `id` HOLDS, keyed by `(resourceType, logical
/// id)`, read out of the shared store the handlers and the data plane write.
fn held_resources(service: &HealthLake, id: &str) -> BTreeMap<(String, String), Value> {
    let state = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let store = state.lock();
    let datastore = store
        .datastores
        .get(id)
        .unwrap_or_else(|| panic!("the store must hold the data store {id}"));
    datastore
        .fhir_resources
        .iter()
        .map(|(key, resource)| (key.clone(), resource.content.clone()))
        .collect()
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

/// A create request asking for the preloaded Synthea data set, with `extra` merged
/// over it so a row can carry whatever else it likes.
fn preload_request(name: &str, extra: Value) -> Value {
    let mut request = json!({
        "DatastoreTypeVersion": "R4",
        "DatastoreName": name,
        "PreloadDataConfig": { "PreloadDataType": "SYNTHEA" },
    });
    merge(&mut request, extra);
    request
}

/// Write every member of `extra` onto `request`.
fn merge(request: &mut Value, extra: Value) {
    if let (Some(target), Some(source)) = (request.as_object_mut(), extra.as_object()) {
        for (member, value) in source {
            target.insert(member.clone(), value.clone());
        }
    }
}

/// The rows of rule 1: a create request, and the `SseConfiguration` every read of
/// the data store it mints must report.
///
/// The rows differ along what the rule generalises over — whether the member is
/// supplied, which `CmkType` it names, and which form of `KmsKeyId` it carries — and
/// along what ELSE the request holds, so no row's answer can be keyed to the shape of
/// the request. The `KmsKeyId` forms are the ones the modelled `EncryptionKeyID`
/// pattern admits (`smithy_query(query="shape", shape="KmsEncryptionConfig")`): a key
/// ARN, a bare key id and an alias.
fn encryption_cases() -> Vec<(&'static str, Value, Value)> {
    vec![
        (
            "no SseConfiguration, only the required member",
            json!({ "DatastoreTypeVersion": "R4" }),
            service_owned_key(),
        ),
        (
            "no SseConfiguration, named and tagged",
            json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": "unencrypted.request",
                "Tags": [{ "Key": "team", "Value": "clinical" }],
            }),
            service_owned_key(),
        ),
        (
            "no SseConfiguration, Synthea preload and an NLP configuration",
            preload_request(
                "preloaded no key named",
                json!({ "NlpConfiguration": { "Status": "DISABLED" } }),
            ),
            service_owned_key(),
        ),
        (
            "the service-owned key named explicitly",
            json!({
                "DatastoreTypeVersion": "R4",
                "SseConfiguration": { "KmsEncryptionConfig": { "CmkType": "AWS_OWNED_KMS_KEY" } },
            }),
            service_owned_key(),
        ),
        (
            "a customer-managed key by ARN",
            json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": "cmk by arn",
                "SseConfiguration": {
                    "KmsEncryptionConfig": {
                        "CmkType": "CUSTOMER_MANAGED_KMS_KEY",
                        "KmsKeyId": "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                    },
                },
            }),
            json!({
                "KmsEncryptionConfig": {
                    "CmkType": "CUSTOMER_MANAGED_KMS_KEY",
                    "KmsKeyId": "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                },
            }),
        ),
        (
            "a customer-managed key by bare key id",
            json!({
                "DatastoreTypeVersion": "R4",
                "SseConfiguration": {
                    "KmsEncryptionConfig": {
                        "CmkType": "CUSTOMER_MANAGED_KMS_KEY",
                        "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
                    },
                },
            }),
            json!({
                "KmsEncryptionConfig": {
                    "CmkType": "CUSTOMER_MANAGED_KMS_KEY",
                    "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
                },
            }),
        ),
        (
            "a customer-managed key by alias",
            json!({
                "DatastoreTypeVersion": "R4",
                "SseConfiguration": {
                    "KmsEncryptionConfig": {
                        "CmkType": "CUSTOMER_MANAGED_KMS_KEY",
                        "KmsKeyId": "alias/healthlake/fhir",
                    },
                },
            }),
            json!({
                "KmsEncryptionConfig": {
                    "CmkType": "CUSTOMER_MANAGED_KMS_KEY",
                    "KmsKeyId": "alias/healthlake/fhir",
                },
            }),
        ),
    ]
}

#[tokio::test]
async fn every_read_reports_the_key_that_encrypts_the_data_store() {
    let service = HealthLake::new();

    for (case, request, expected) in encryption_cases() {
        let datastore = active_datastore(&service, &request).await;
        assert_eq!(
            datastore.properties["SseConfiguration"], expected,
            "case {case:?}: DescribeFHIRDatastore must report the key that encrypts the data \
             store, got {}",
            datastore.properties
        );

        let entry = listed(&service, &datastore.id).await;
        assert_eq!(
            entry["SseConfiguration"], expected,
            "case {case:?}: ListFHIRDatastores must report the same key Describe does, got {entry}"
        );
    }
}

#[tokio::test]
async fn a_restored_data_store_reports_the_key_that_encrypts_it() {
    let service = HealthLake::new();

    // A restore inherits the source's encryption unless the request names a key of
    // its own, so both rows end up with a key to report: the source's inherited
    // service-owned one, and the customer-managed one the restore names.
    let source = active_datastore(
        &service,
        &json!({
            "DatastoreTypeVersion": "R4",
            "DatastoreName": "restore source without a named key",
            "BackupConfiguration": {
                "Status": "ENABLED",
                "BackupType": "CONTINUOUS",
                "RetentionPeriodInDays": 7,
                "BackupTagsEnabled": true,
            },
        }),
    )
    .await;
    assert_eq!(
        source.properties["SseConfiguration"],
        service_owned_key(),
        "the source data store named no key, so it reports the service-owned one, got {}",
        source.properties
    );

    let rows = vec![
        (
            "restore inheriting the source's encryption",
            json!({
                "SourceDatastoreId": source.id,
                "RestoreConfiguration": { "ContinuousBackupRestoreConfiguration": {} },
            }),
            service_owned_key(),
        ),
        (
            "restore naming a customer-managed key of its own",
            json!({
                "SourceDatastoreId": source.id,
                "RestoreConfiguration": { "ContinuousBackupRestoreConfiguration": {} },
                "SseConfiguration": {
                    "KmsEncryptionConfig": {
                        "CmkType": "CUSTOMER_MANAGED_KMS_KEY",
                        "KmsKeyId": "alias/healthlake/restored",
                    },
                },
            }),
            json!({
                "KmsEncryptionConfig": {
                    "CmkType": "CUSTOMER_MANAGED_KMS_KEY",
                    "KmsKeyId": "alias/healthlake/restored",
                },
            }),
        ),
    ];

    for (case, request, expected) in rows {
        let restored = ok(&service, "RestoreFHIRDatastore", &request).await;
        let id = string_at(&restored, "DatastoreId");
        let properties = ok(
            &service,
            "DescribeFHIRDatastore",
            &json!({ "DatastoreId": id }),
        )
        .await["DatastoreProperties"]
            .clone();
        assert_eq!(
            properties["SseConfiguration"], expected,
            "case {case:?}: a restored data store must report the key that encrypts it, got \
             {properties}"
        );
    }
}

#[tokio::test]
async fn a_preload_type_outside_the_modelled_set_is_refused_and_mints_nothing() {
    let service = HealthLake::new();

    // `PreloadDataType` models exactly one value ("Only Synthea preloaded data is
    // supported"), so these rows differ in case, spacing and in naming another of
    // this service's formats — none of them names a data set to preload.
    for case in ["", "synthea", "SYNTHEA ", "Synthea", "CCDA"] {
        let (status, payload) = service
            .rpc(
                "CreateFHIRDatastore",
                &json!({
                    "DatastoreTypeVersion": "R4",
                    "PreloadDataConfig": { "PreloadDataType": case },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a PreloadDataType outside the modelled set is refused, got {status}: \
             {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?}: the refusal is the modelled ValidationException, got {payload}"
        );
    }

    let body = ok(&service, "ListFHIRDatastores", &json!({})).await;
    let minted = body["DatastorePropertiesList"]
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(
        minted, 0,
        "a refused create mints nothing, got {minted} data store(s): {body}"
    );
}

/// The rows of rule 2: a create request, and whether the data store it mints comes
/// up holding the preloaded data set.
///
/// The rows differ along what the rule generalises over — whether the preload was
/// asked for — and, within each answer, along what else the request carries, so no
/// row's answer can be keyed to the shape of the request.
fn preload_cases() -> Vec<(&'static str, Value, bool)> {
    vec![
        (
            "no PreloadDataConfig, only the required member",
            json!({ "DatastoreTypeVersion": "R4" }),
            false,
        ),
        (
            "no PreloadDataConfig, named, tagged and customer-encrypted",
            json!({
                "DatastoreTypeVersion": "R4",
                "DatastoreName": "not preloaded",
                "Tags": [{ "Key": "preloaded", "Value": "no" }],
                "SseConfiguration": {
                    "KmsEncryptionConfig": {
                        "CmkType": "CUSTOMER_MANAGED_KMS_KEY",
                        "KmsKeyId": "alias/healthlake/plain",
                    },
                },
            }),
            false,
        ),
        (
            "SYNTHEA preload, nothing else",
            json!({
                "DatastoreTypeVersion": "R4",
                "PreloadDataConfig": { "PreloadDataType": "SYNTHEA" },
            }),
            true,
        ),
        (
            "SYNTHEA preload, named, tagged and customer-encrypted",
            preload_request(
                "preloaded and configured",
                json!({
                    "Tags": [{ "Key": "preloaded", "Value": "yes" }],
                    "SseConfiguration": {
                        "KmsEncryptionConfig": {
                            "CmkType": "CUSTOMER_MANAGED_KMS_KEY",
                            "KmsKeyId": "alias/healthlake/preloaded",
                        },
                    },
                    "IdentityProviderConfiguration": {
                        "AuthorizationStrategy": "AWS_AUTH",
                        "FineGrainedAuthorizationEnabled": false,
                    },
                }),
            ),
            true,
        ),
    ]
}

#[tokio::test]
async fn a_preloaded_data_store_comes_up_holding_readable_synthea_resources() {
    let service = HealthLake::new();

    for (case, request, preloaded) in preload_cases() {
        let datastore = active_datastore(&service, &request).await;

        // The configuration is reported either way; what differs is whether the
        // data store HOLDS anything.
        match request.get("PreloadDataConfig") {
            Some(supplied) => assert_eq!(
                &datastore.properties["PreloadDataConfig"], supplied,
                "case {case:?}: Describe must report the stored PreloadDataConfig, got {}",
                datastore.properties
            ),
            None => assert!(
                datastore.properties.get("PreloadDataConfig").is_none(),
                "case {case:?}: no preload was asked for, so Describe must omit the member, got {}",
                datastore.properties
            ),
        }

        let held = held_resources(&service, &datastore.id);
        if !preloaded {
            assert!(
                held.is_empty(),
                "case {case:?}: a data store that asked for no preload comes up empty, got {} \
                 resource(s)",
                held.len()
            );
            continue;
        }

        assert_eq!(
            held.len(),
            PRELOADED_RESOURCE_COUNT,
            "case {case:?}: a preloaded data store holds the preloaded data set, got {} \
             resource(s)",
            held.len()
        );

        let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
        for ((resource_type, id), content) in &held {
            *by_type.entry(resource_type.clone()).or_default() += 1;

            // Every preloaded resource is READABLE on the endpoint the data store
            // advertises, answering exactly the bytes it holds — the same read a
            // resource a caller created answers.
            let (status, body) = read_resource(&service, &datastore.path, resource_type, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a preloaded {resource_type}/{id} must be readable, got {status}: \
                 {body}"
            );
            assert_eq!(
                &body, content,
                "case {case:?}: the read of {resource_type}/{id} must answer the stored resource"
            );
            assert_eq!(
                body["resourceType"],
                json!(resource_type),
                "case {case:?}: {resource_type}/{id} must carry its own resourceType, got {body}"
            );
            assert_eq!(
                body["id"],
                json!(id),
                "case {case:?}: {resource_type}/{id} must carry the id it is stored under, got \
                 {body}"
            );
            assert_eq!(
                body["meta"]["versionId"],
                json!("1"),
                "case {case:?}: a preloaded {resource_type} is at version 1, got {body}"
            );
            assert!(
                body["meta"]["lastUpdated"].is_string(),
                "case {case:?}: a preloaded {resource_type} carries meta.lastUpdated, got {body}"
            );
            assert_resource_is_coherent(case, &held, resource_type, &body);
        }

        for resource_type in ["Patient", "Encounter", "Condition"] {
            assert_eq!(
                by_type.get(resource_type).copied().unwrap_or_default(),
                PRELOADED_RECORD_COUNT,
                "case {case:?}: the preload holds one {resource_type} per synthetic record, got \
                 {by_type:?}"
            );
        }
        let observations = by_type.get("Observation").copied().unwrap_or_default();
        assert!(
            observations >= PRELOADED_RECORD_COUNT,
            "case {case:?}: the preload holds the vital signs taken at each record's encounter, \
             got {observations} Observation(s) for {PRELOADED_RECORD_COUNT} record(s)"
        );
    }
}

/// The invariants of ONE preloaded resource: that it says what its type requires,
/// and that every reference it carries names a resource the same data store holds.
///
/// The code systems are the published terminologies FHIR R4 writes these resources
/// in — LOINC (<https://loinc.org>) for a vital-sign observation's code, SNOMED CT
/// for a condition's — which is what the generator's own constants cite.
fn assert_resource_is_coherent(
    case: &str,
    held: &BTreeMap<(String, String), Value>,
    resource_type: &str,
    body: &Value,
) {
    match resource_type {
        "Patient" => {
            for member in ["gender", "birthDate"] {
                assert!(
                    body[member].is_string(),
                    "case {case:?}: a preloaded Patient carries {member}, got {body}"
                );
            }
            assert!(
                body["name"][0]["family"].is_string()
                    && body["name"][0]["given"][0].is_string(),
                "case {case:?}: a preloaded Patient carries a name, got {body}"
            );
            assert!(
                body["identifier"][0]["value"].is_string(),
                "case {case:?}: a preloaded Patient carries an identifier, got {body}"
            );
        }
        "Encounter" => {
            assert_eq!(
                body["status"],
                json!("finished"),
                "case {case:?}: a preloaded Encounter is a finished visit, got {body}"
            );
            assert!(
                body["period"]["start"].is_string() && body["period"]["end"].is_string(),
                "case {case:?}: a preloaded Encounter carries the period it covered, got {body}"
            );
            assert_reference_resolves(case, held, body, "subject", "Patient");
        }
        "Condition" => {
            assert_eq!(
                body["code"]["coding"][0]["system"],
                json!("http://snomed.info/sct"),
                "case {case:?}: a preloaded Condition is coded in SNOMED CT, got {body}"
            );
            assert!(
                body["code"]["coding"][0]["code"].is_string(),
                "case {case:?}: a preloaded Condition carries a code, got {body}"
            );
            assert_reference_resolves(case, held, body, "subject", "Patient");
            assert_reference_resolves(case, held, body, "encounter", "Encounter");
        }
        "Observation" => {
            assert_eq!(
                body["code"]["coding"][0]["system"],
                json!("http://loinc.org"),
                "case {case:?}: a preloaded Observation is coded in LOINC, got {body}"
            );
            assert!(
                body["valueQuantity"]["value"].is_number()
                    || body["component"][0]["valueQuantity"]["value"].is_number(),
                "case {case:?}: a preloaded Observation carries the value measured, got {body}"
            );
            assert!(
                body["effectiveDateTime"].is_string(),
                "case {case:?}: a preloaded Observation carries when it was taken, got {body}"
            );
            assert_reference_resolves(case, held, body, "subject", "Patient");
            assert_reference_resolves(case, held, body, "encounter", "Encounter");
        }
        other => panic!(
            "case {case:?}: the preload holds only the FHIR R4 types a Synthea record is made \
             of, got a {other}: {body}"
        ),
    }
}

/// Assert `body`'s `member` reference names a resource of `expected_type` the SAME
/// data store holds: a preloaded child resource that referenced nothing readable
/// would be a dangling record.
fn assert_reference_resolves(
    case: &str,
    held: &BTreeMap<(String, String), Value>,
    body: &Value,
    member: &str,
    expected_type: &str,
) {
    let reference = body[member]["reference"].as_str().unwrap_or_else(|| {
        panic!("case {case:?}: the resource must reference a {expected_type} at {member}, got {body}")
    });
    let (resource_type, id) = reference.split_once('/').unwrap_or_else(|| {
        panic!("case {case:?}: a FHIR reference is '<Type>/<id>', got {reference:?}")
    });
    assert_eq!(
        resource_type, expected_type,
        "case {case:?}: {member} must reference a {expected_type}, got {reference:?}"
    );
    assert!(
        held.contains_key(&(resource_type.to_string(), id.to_string())),
        "case {case:?}: {member} references {reference:?}, which the data store does not hold"
    );
}

#[tokio::test]
async fn preloaded_resources_belong_to_the_data_store_that_asked_for_them() {
    let service = HealthLake::new();

    let first = active_datastore(&service, &preload_request("first preload", json!({}))).await;
    let second = active_datastore(&service, &preload_request("second preload", json!({}))).await;
    let plain = active_datastore(&service, &json!({ "DatastoreTypeVersion": "R4" })).await;

    let held_first = held_resources(&service, &first.id);
    let held_second = held_resources(&service, &second.id);
    assert_eq!(
        held_first.len(),
        PRELOADED_RESOURCE_COUNT,
        "the first preload holds the data set, got {}",
        held_first.len()
    );
    assert_eq!(
        held_second.len(),
        PRELOADED_RESOURCE_COUNT,
        "the second preload holds the data set, got {}",
        held_second.len()
    );

    // Two preloads are two data sets: the logical ids one data store assigned are
    // not the ids the other assigned, so neither can be read on the other's
    // endpoint.
    for key in held_first.keys() {
        assert!(
            !held_second.contains_key(key),
            "two preloads must assign distinct logical ids, both hold {key:?}"
        );
    }

    let ((resource_type, id), _content) = held_first
        .iter()
        .next()
        .expect("the first preload holds a resource");
    for other in [&second, &plain] {
        let (status, body) = read_resource(&service, &other.path, resource_type, id).await;
        assert_eq!(
            status, 404,
            "another data store's endpoint must not serve {resource_type}/{id}, got {status}: \
             {body}"
        );
        assert_eq!(
            outcome_code(&body),
            "not-found",
            "the refusal is a FHIR not-found OperationOutcome, got {body}"
        );
    }
}

#[tokio::test]
async fn a_preloaded_data_store_still_serves_its_own_writes() {
    let service = HealthLake::new();

    let datastore = active_datastore(&service, &preload_request("preload plus writes", json!({})))
        .await;
    let before = held_resources(&service, &datastore.id);

    // A write to a preloaded data store is stored beside the preloaded resources:
    // the preload is data the data store HOLDS, not a mode it is in.
    let submitted = json!({
        "resourceType": "Patient",
        "name": [{ "use": "official", "family": "Written", "given": ["Olive"] }],
        "gender": "female",
        "birthDate": "1991-04-02",
    });
    let (status, stored, _headers) = service
        .fhir(
            "POST",
            &format!("{}Patient", datastore.path),
            &submitted,
        )
        .await;
    assert_eq!(
        status, 201,
        "a FHIR create on a preloaded data store answers 201, got {status}: {stored}"
    );
    let written_id = string_at(&stored, "id");

    let after = held_resources(&service, &datastore.id);
    assert_eq!(
        after.len(),
        before.len() + 1,
        "the write adds one resource to the preloaded data set, got {} after {}",
        after.len(),
        before.len()
    );
    for key in before.keys() {
        assert!(
            after.contains_key(key),
            "a write must not disturb the preloaded resource {key:?}"
        );
    }

    let (status, body) = read_resource(&service, &datastore.path, "Patient", &written_id).await;
    assert_eq!(
        status, 200,
        "the written Patient must be readable beside the preloaded ones, got {status}: {body}"
    );
    assert_eq!(
        body["name"][0]["family"],
        json!("Written"),
        "the read must answer the submitted content, got {body}"
    );
}
