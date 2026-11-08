// SPDX-License-Identifier: CC0-1.0

//! What `StartFHIRImportJob` imports, what it records, and what either refuses.
//!
//! The rules under test, each over a SET of inputs rather than one case:
//!
//! 1. a start IMPORTS: the FHIR resources the input location holds are read out
//!    of the emulator's own Amazon S3 and stored on the data store, where its
//!    FHIR REST endpoint serves each one at its own logical id — for input held
//!    as newline-delimited JSON, as one JSON document, as a JSON array, in one
//!    object or in several under a prefix;
//! 2. the start MINTS one job record, answers the minted `JobId` with the
//!    `SUBMITTED` status, persists every member the request carried, and that
//!    ONE record is what `DescribeFHIRImportJob` and `ListFHIRImportJobs` both
//!    report — with the `JobProgressReport` counting what the run actually did;
//! 3. the clock then settles the job to the status those counts warrant:
//!    everything scanned imported is `COMPLETED`, some of it
//!    `COMPLETED_WITH_ERRORS`, and none of it `FAILED` with the `Message` saying
//!    which;
//! 4. only a settled, usable (`ACTIVE`) data store can be imported into: an id
//!    the registry holds no record for is `ResourceNotFoundException`, a record
//!    in any other status — `CREATING`, `DELETING`, `DELETED` — is
//!    `AccessDeniedException` stating that the data store is not active, and a
//!    refused start mints nothing and imports nothing;
//! 5. one `ClientToken` starts one import job: a repeat of the same request
//!    answers out of the job it started and imports nothing a second time, and
//!    the same token with a different request is refused;
//! 6. an import of a logical id the data store already holds writes a further
//!    VERSION of that resource, as an `update` of it would.
//!
//! Amazon S3 is mounted beside this service on ONE engine, exactly as the
//! assembled router mounts them, and every input below is staged through that
//! service's OWN `PutObject` — so what an import reads is what Amazon S3 really
//! holds, and what it imported is read back through the data store's own FHIR
//! endpoint.
//!
//! Every modelled CLIENT error is asserted at HTTP 400 whatever status its
//! shape's `@httpError` names, for the reason `tests/all/error_envelopes.rs`
//! pins: awsJson1_0 binds nothing to the transport, so this service reports an
//! error's FAULT.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    ComAmazonawsHealthlakeHealthlakeState, DATASTORE_NOT_ACTIVE_MESSAGE,
    DATASTORE_NOT_FOUND_MESSAGE,
};

use crate::fhir_datastore_lifecycle::{error_code, error_message};
use crate::fhir_rest_and_validate_source::HealthLake;

/// The `DataAccessRoleArn` every start below carries, satisfying the modelled
/// `IamRoleArn` pattern (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeImport";

/// The bucket every input below is staged in, and one no bucket is created for.
const BUCKET: &str = "healthlake-import-bucket";
const UNHELD_BUCKET: &str = "healthlake-bucket-that-was-never-created";

/// A well-formed `DatastoreId` no data store is held for: 32 lowercase hex
/// characters, the form this service mints, so the refusal below is about the
/// data store not existing rather than about the identifier's shape.
const UNHELD_DATASTORE_ID: &str = "ffffffffffffffffffffffffffffffff";

/// Long enough that the transition `crate::state::IMPORT_JOB_RUN` owes a started
/// import job (500 ms) has fallen due.
const PAST_JOB_RUN: Duration = Duration::from_millis(900);

/// One `JobOutputDataConfig` the modelled union admits: the member bounds
/// `smithy_query(query="shape", shape="S3Configuration")` states.
fn output_data_config() -> Value {
    json!({
        "S3Configuration": {
            "S3Uri": format!("s3://{BUCKET}/output"),
            "KmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
        }
    })
}

/// A `StartFHIRImportJob` request body importing `s3_uri` into `datastore_id`.
fn start_request(datastore_id: &str, s3_uri: &str) -> Value {
    json!({
        "DatastoreId": datastore_id,
        "DataAccessRoleArn": DATA_ACCESS_ROLE_ARN,
        "InputDataConfig": { "S3Uri": s3_uri },
        "JobOutputDataConfig": output_data_config(),
    })
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

/// The `ImportJobProperties` `DescribeFHIRImportJob` reports for one job.
async fn describe_job(service: &HealthLake, datastore_id: &str, job_id: &str) -> Value {
    ok(
        service,
        "DescribeFHIRImportJob",
        &json!({ "DatastoreId": datastore_id, "JobId": job_id }),
    )
    .await["ImportJobProperties"]
        .clone()
}

/// Every import job `ListFHIRImportJobs` reports for a data store.
async fn listed_jobs(service: &HealthLake, datastore_id: &str) -> Vec<Value> {
    ok(
        service,
        "ListFHIRImportJobs",
        &json!({ "DatastoreId": datastore_id }),
    )
    .await["ImportJobPropertiesList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Every FHIR resource the data store `id` HOLDS, keyed by `(resourceType,
/// logical id)`, read out of the shared store the handlers and the data plane
/// write.
///
/// Read directly because the FHIR `search` interaction that would enumerate them
/// over the wire is its own gap; every resource this suite asserts was imported
/// is then READ BACK over the endpoint, which is the wire a client uses.
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

/// Create the bucket every input is staged in, through Amazon S3's own
/// `CreateBucket`.
async fn create_bucket(service: &HealthLake) {
    let created = service.s3().create_bucket().bucket(BUCKET).send().await;
    assert!(
        created.is_ok(),
        "Amazon S3 must create the input bucket, got {created:?}"
    );
}

/// Stage one input object, through Amazon S3's own `PutObject`.
async fn stage(service: &HealthLake, key: &str, body: &str) {
    let put = service
        .s3()
        .put_object()
        .bucket(BUCKET)
        .key(key)
        .body(aws_sdk_s3::primitives::ByteStream::from(
            body.as_bytes().to_vec(),
        ))
        .send()
        .await;
    assert!(
        put.is_ok(),
        "Amazon S3 must hold the input object {key}, got {put:?}"
    );
}

/// One FHIR resource of `resource_type` at the logical id `id`, with a content
/// member that tells one row's resources apart.
fn resource(resource_type: &str, id: Option<&str>, marker: &str) -> Value {
    let mut content = json!({ "resourceType": resource_type, "language": marker });
    if let Some(id) = id {
        content["id"] = json!(id);
    }
    content
}

/// The resources of one input object as newline-delimited JSON — the bulk form
/// HealthLake documents for import: one resource per line.
fn ndjson(resources: &[Value]) -> String {
    resources
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// One row of the delivery table: the objects to stage, the location the import
/// reads, and the resources it must then hold.
struct Row {
    case: &'static str,
    /// `(key, contents)` for each object staged before the start.
    objects: Vec<(String, String)>,
    /// The `InputDataConfig$S3Uri` the start names.
    s3_uri: String,
    /// `(resourceType, logical id)` of each resource the import must deliver.
    delivered: Vec<(&'static str, &'static str)>,
}

/// The delivery table: the rule is that every FHIR resource the input holds is
/// imported and served at its own logical id, so the rows differ along the FORM
/// the input takes — one resource per file, several, several files under one
/// prefix, one object named outright, one JSON document rather than
/// newline-delimited JSON, and a JSON array of resources.
fn delivery_rows() -> Vec<Row> {
    vec![
        Row {
            case: "one newline-delimited file holding one resource",
            objects: vec![(
                "one/patients.ndjson".to_string(),
                ndjson(&[resource("Patient", Some("pat-one"), "one")]),
            )],
            s3_uri: format!("s3://{BUCKET}/one/"),
            delivered: vec![("Patient", "pat-one")],
        },
        Row {
            case: "one file holding several resources of different types",
            objects: vec![(
                "several/bundle.ndjson".to_string(),
                ndjson(&[
                    resource("Patient", Some("pat-several"), "several"),
                    resource("Observation", Some("obs-several"), "several"),
                    resource("Condition", Some("cond-several"), "several"),
                ]),
            )],
            s3_uri: format!("s3://{BUCKET}/several/"),
            delivered: vec![
                ("Patient", "pat-several"),
                ("Observation", "obs-several"),
                ("Condition", "cond-several"),
            ],
        },
        Row {
            case: "several files under one prefix",
            objects: vec![
                (
                    "many/part-1.ndjson".to_string(),
                    ndjson(&[
                        resource("Patient", Some("pat-many-1"), "many"),
                        resource("Observation", Some("obs-many-1"), "many"),
                    ]),
                ),
                (
                    "many/part-2.ndjson".to_string(),
                    ndjson(&[resource("Patient", Some("pat-many-2"), "many")]),
                ),
                // A folder marker is not a file of FHIR data.
                ("many/".to_string(), String::new()),
            ],
            s3_uri: format!("s3://{BUCKET}/many/"),
            delivered: vec![
                ("Patient", "pat-many-1"),
                ("Patient", "pat-many-2"),
                ("Observation", "obs-many-1"),
            ],
        },
        Row {
            case: "the input location names one object rather than a prefix",
            objects: vec![
                (
                    "single/only.ndjson".to_string(),
                    ndjson(&[resource("Patient", Some("pat-single"), "single")]),
                ),
                (
                    "single/other.ndjson".to_string(),
                    ndjson(&[resource("Patient", Some("pat-other"), "other")]),
                ),
            ],
            s3_uri: format!("s3://{BUCKET}/single/only.ndjson"),
            delivered: vec![("Patient", "pat-single")],
        },
        Row {
            case: "one file holding a single JSON document",
            objects: vec![(
                "document/patient.json".to_string(),
                format!(
                    "{:#}",
                    resource("Patient", Some("pat-document"), "document")
                ),
            )],
            s3_uri: format!("s3://{BUCKET}/document/"),
            delivered: vec![("Patient", "pat-document")],
        },
        Row {
            case: "one file holding a JSON array of resources",
            objects: vec![(
                "array/patients.json".to_string(),
                json!([
                    resource("Patient", Some("pat-array-1"), "array"),
                    resource("Patient", Some("pat-array-2"), "array"),
                ])
                .to_string(),
            )],
            s3_uri: format!("s3://{BUCKET}/array/"),
            delivered: vec![("Patient", "pat-array-1"), ("Patient", "pat-array-2")],
        },
    ]
}

#[tokio::test]
async fn an_import_delivers_every_resource_its_input_holds() {
    for row in delivery_rows() {
        let service = HealthLake::with_object_store();
        create_bucket(&service).await;
        for (key, body) in &row.objects {
            stage(&service, key, body).await;
        }
        let (datastore_id, path) = service.active_datastore("importing store").await;

        let started = ok(
            &service,
            "StartFHIRImportJob",
            &start_request(&datastore_id, &row.s3_uri),
        )
        .await;
        let job_id = string_at(&started, "JobId");
        let case = row.case;
        assert_eq!(
            started["JobStatus"],
            json!("SUBMITTED"),
            "case {case:?}: a started job must be answered SUBMITTED, got {started}"
        );
        assert_eq!(
            started["DatastoreId"],
            json!(datastore_id),
            "case {case:?}: the start must report the data store it imports into, got {started}"
        );
        assert_eq!(
            job_id.len(),
            32,
            "case {case:?}: JobId must be 32 characters, got {job_id:?}"
        );

        // What the data store HOLDS is exactly the resources the input held.
        let held = held_resources(&service, &datastore_id);
        let mut expected: Vec<(String, String)> = row
            .delivered
            .iter()
            .map(|(resource_type, id)| ((*resource_type).to_string(), (*id).to_string()))
            .collect();
        expected.sort();
        let mut holding: Vec<(String, String)> = held.keys().cloned().collect();
        holding.sort();
        assert_eq!(
            holding, expected,
            "case {case:?}: the import must deliver exactly the resources the input held"
        );

        // And each one is served by the data store's own FHIR endpoint, at the
        // logical id the imported resource carried.
        for (resource_type, id) in &row.delivered {
            let (status, body) = read_resource(&service, &path, resource_type, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the imported {resource_type}/{id} must be readable, got \
                 {status}: {body}"
            );
            assert_eq!(
                body["id"],
                json!(id),
                "case {case:?}: the imported resource keeps its own logical id, got {body}"
            );
            assert_eq!(
                body["resourceType"],
                json!(resource_type),
                "case {case:?}: the imported resource keeps its resourceType, got {body}"
            );
            assert_eq!(
                body["meta"]["versionId"],
                json!("1"),
                "case {case:?}: a newly imported resource is at version 1, got {body}"
            );
        }

        // The job settles to COMPLETED, and its progress report counts what the
        // run really did.
        tokio::time::sleep(PAST_JOB_RUN).await;
        let described = describe_job(&service, &datastore_id, &job_id).await;
        assert_eq!(
            described["JobStatus"],
            json!("COMPLETED"),
            "case {case:?}: an import that imported everything it scanned must COMPLETE, got \
             {described}"
        );
        assert!(
            described.get("Message").is_none(),
            "case {case:?}: a COMPLETED import has nothing to explain, got {described}"
        );
        assert!(
            described["EndTime"].is_number(),
            "case {case:?}: a job that has ended reports an EndTime, got {described}"
        );
        let report = &described["JobProgressReport"];
        let scanned_files = row
            .objects
            .iter()
            .filter(|(key, _)| !key.ends_with('/'))
            .filter(|(key, _)| row.s3_uri.ends_with('/') || row.s3_uri.ends_with(key.as_str()))
            .count();
        for (member, expected) in [
            ("TotalNumberOfResourcesScanned", row.delivered.len()),
            ("TotalNumberOfResourcesImported", row.delivered.len()),
            ("TotalNumberOfResourcesWithCustomerError", 0),
            ("TotalNumberOfFilesReadWithCustomerError", 0),
            ("TotalNumberOfScannedNonFhirFiles", 0),
            ("TotalNumberOfScannedFiles", scanned_files),
            ("TotalNumberOfImportedFiles", scanned_files),
        ] {
            assert_eq!(
                report[member],
                json!(expected),
                "case {case:?}: the progress report must count {member} as {expected}, got \
                 {report}"
            );
        }

        // ONE record: what the listing reports is exactly what the describe did.
        let jobs = listed_jobs(&service, &datastore_id).await;
        assert_eq!(
            jobs.len(),
            1,
            "case {case:?}: a start adds exactly one job, got {jobs:?}"
        );
        assert_eq!(
            jobs.first(),
            Some(&described),
            "case {case:?}: the listing and the describe must report the SAME record"
        );
    }
}

#[tokio::test]
async fn a_start_persists_every_member_it_carried() {
    let service = HealthLake::with_object_store();
    create_bucket(&service).await;
    stage(
        &service,
        "members/patients.ndjson",
        &ndjson(&[resource("Patient", Some("pat-members"), "members")]),
    )
    .await;
    let (datastore_id, _path) = service.active_datastore("importing store").await;

    // The rows span the optional members a start may carry, each of which the
    // record must report back: the job name, and every value the modelled
    // `ValidationLevel` enum declares
    // (`smithy_query(query="shape", shape="ValidationLevel")`).
    let rows: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("no JobName and no ValidationLevel", None, None),
        ("a JobName only", Some("nightly-import"), None),
        ("ValidationLevel strict", Some("strict-import"), Some("strict")),
        ("ValidationLevel minimal", None, Some("minimal")),
        (
            "ValidationLevel structure-only",
            Some("структура 42"),
            Some("structure-only"),
        ),
    ];
    for (case, job_name, validation_level) in rows {
        let mut request = start_request(&datastore_id, &format!("s3://{BUCKET}/members/"));
        if let Some(name) = job_name {
            request["JobName"] = json!(name);
        }
        if let Some(level) = validation_level {
            request["ValidationLevel"] = json!(level);
        }
        let started = ok(&service, "StartFHIRImportJob", &request).await;
        let job_id = string_at(&started, "JobId");

        let described = describe_job(&service, &datastore_id, &job_id).await;
        for (member, expected) in [
            ("JobId", json!(job_id)),
            ("DatastoreId", json!(datastore_id)),
            ("DataAccessRoleArn", json!(DATA_ACCESS_ROLE_ARN)),
            (
                "InputDataConfig",
                json!({ "S3Uri": format!("s3://{BUCKET}/members/") }),
            ),
            ("JobOutputDataConfig", output_data_config()),
        ] {
            assert_eq!(
                described[member], expected,
                "case {case:?}: the described job must report {member} as {expected}, got \
                 {described}"
            );
        }
        assert_eq!(
            described.get("JobName").cloned(),
            job_name.map(Value::from),
            "case {case:?}: the described job must report the JobName the start carried, got \
             {described}"
        );
        assert_eq!(
            described.get("ValidationLevel").cloned(),
            validation_level.map(Value::from),
            "case {case:?}: the described job must report the ValidationLevel the start \
             carried, got {described}"
        );
        assert!(
            described["SubmitTime"].is_number(),
            "case {case:?}: the described job must report a SubmitTime, got {described}"
        );
    }
}

#[tokio::test]
async fn an_import_assigns_an_id_to_a_resource_that_carries_none() {
    let service = HealthLake::with_object_store();
    create_bucket(&service).await;
    stage(
        &service,
        "assigned/patients.ndjson",
        &ndjson(&[
            resource("Patient", None, "assigned"),
            resource("Observation", None, "assigned"),
        ]),
    )
    .await;
    let (datastore_id, path) = service.active_datastore("importing store").await;

    let started = ok(
        &service,
        "StartFHIRImportJob",
        &start_request(&datastore_id, &format!("s3://{BUCKET}/assigned/")),
    )
    .await;
    let job_id = string_at(&started, "JobId");

    // FHIR requires the SERVER to assign a logical id when the resource carries
    // none, so each resource is stored under an id this service minted — and is
    // readable at it.
    let held = held_resources(&service, &datastore_id);
    assert_eq!(
        held.len(),
        2,
        "every resource of the input must be delivered, got {:?}",
        held.keys().collect::<Vec<_>>()
    );
    for ((resource_type, id), content) in &held {
        assert!(
            !id.is_empty() && id.len() <= 64,
            "case {resource_type:?}: an assigned logical id must be a FHIR id, got {id:?}"
        );
        assert_eq!(
            content["id"],
            json!(id),
            "case {resource_type:?}: the stored resource carries the id it is held under, got \
             {content}"
        );
        let (status, body) = read_resource(&service, &path, resource_type, id).await;
        assert_eq!(
            status, 200,
            "case {resource_type:?}: the imported resource must be readable at its assigned \
             id, got {status}: {body}"
        );
    }

    tokio::time::sleep(PAST_JOB_RUN).await;
    let described = describe_job(&service, &datastore_id, &job_id).await;
    assert_eq!(
        described["JobStatus"],
        json!("COMPLETED"),
        "an import of resources without ids still completes, got {described}"
    );
}

/// One row of the partial-outcome table: what the input holds, and what the job
/// must then report.
struct Outcome {
    case: &'static str,
    /// `(key, contents)` for each object staged, and whether the bucket is
    /// created at all.
    objects: Vec<(String, String)>,
    /// The `InputDataConfig$S3Uri` the start names.
    s3_uri: String,
    /// The status the clock must owe the job.
    status: &'static str,
    /// The resources the data store must hold afterwards.
    delivered: Vec<(&'static str, &'static str)>,
    /// `(scanned, imported, with customer error)` the progress report must
    /// state.
    counts: (usize, usize, usize),
}

/// The partial-outcome table: the rule is that the status follows the counts the
/// run measured, so the rows differ along what the input makes unreadable — a
/// line that is not JSON, a JSON object that is not a FHIR resource, a resource
/// whose id is not a FHIR logical id, a file that is not text at all, an input
/// location holding nothing, and a bucket Amazon S3 holds nothing under.
fn outcome_rows() -> Vec<Outcome> {
    vec![
        Outcome {
            case: "a line that is not JSON beside a resource that is",
            objects: vec![(
                "mixed/patients.ndjson".to_string(),
                format!(
                    "{}\nthis line is not JSON\n",
                    resource("Patient", Some("pat-mixed"), "mixed")
                ),
            )],
            s3_uri: format!("s3://{BUCKET}/mixed/"),
            status: "COMPLETED_WITH_ERRORS",
            delivered: vec![("Patient", "pat-mixed")],
            counts: (2, 1, 1),
        },
        Outcome {
            case: "a JSON object carrying no resourceType",
            objects: vec![(
                "typeless/thing.ndjson".to_string(),
                json!({ "name": "no resourceType here" }).to_string(),
            )],
            s3_uri: format!("s3://{BUCKET}/typeless/"),
            status: "FAILED",
            delivered: vec![],
            counts: (1, 0, 1),
        },
        Outcome {
            case: "a resource whose id is not a FHIR logical id",
            objects: vec![(
                "badid/patients.ndjson".to_string(),
                ndjson(&[resource("Patient", Some("not a logical id"), "badid")]),
            )],
            s3_uri: format!("s3://{BUCKET}/badid/"),
            status: "FAILED",
            delivered: vec![],
            counts: (1, 0, 1),
        },
        Outcome {
            case: "a file that holds no FHIR resource at all",
            objects: vec![(
                "text/notes.txt".to_string(),
                "plain text that is not FHIR".to_string(),
            )],
            s3_uri: format!("s3://{BUCKET}/text/"),
            status: "FAILED",
            delivered: vec![],
            counts: (1, 0, 1),
        },
        Outcome {
            case: "an input location holding nothing",
            objects: vec![],
            s3_uri: format!("s3://{BUCKET}/empty/"),
            status: "FAILED",
            delivered: vec![],
            counts: (0, 0, 0),
        },
        Outcome {
            case: "a bucket Amazon S3 holds nothing under",
            objects: vec![],
            s3_uri: format!("s3://{UNHELD_BUCKET}/input/"),
            status: "FAILED",
            delivered: vec![],
            counts: (0, 0, 0),
        },
    ]
}

#[tokio::test]
async fn an_import_reports_what_it_could_not_read() {
    for row in outcome_rows() {
        let service = HealthLake::with_object_store();
        create_bucket(&service).await;
        for (key, body) in &row.objects {
            stage(&service, key, body).await;
        }
        let (datastore_id, path) = service.active_datastore("importing store").await;
        let case = row.case;

        // A start whose RUN could not import everything is still a started job:
        // what the run could not do is the job's own status, never a refusal of
        // the request.
        let started = ok(
            &service,
            "StartFHIRImportJob",
            &start_request(&datastore_id, &row.s3_uri),
        )
        .await;
        let job_id = string_at(&started, "JobId");
        assert_eq!(
            started["JobStatus"],
            json!("SUBMITTED"),
            "case {case:?}: a started job is answered SUBMITTED, got {started}"
        );

        let held = held_resources(&service, &datastore_id);
        assert_eq!(
            held.len(),
            row.delivered.len(),
            "case {case:?}: the data store must hold only what was imported, got {:?}",
            held.keys().collect::<Vec<_>>()
        );
        for (resource_type, id) in &row.delivered {
            let (status, body) = read_resource(&service, &path, resource_type, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the resource that WAS imported must be readable, got {status}: \
                 {body}"
            );
        }

        tokio::time::sleep(PAST_JOB_RUN).await;
        let described = describe_job(&service, &datastore_id, &job_id).await;
        assert_eq!(
            described["JobStatus"],
            json!(row.status),
            "case {case:?}: the job must settle to {}, got {described}",
            row.status
        );
        let message = described["Message"].as_str().unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {case:?}: a job that did not import everything must say why, got {described}"
        );
        // The `Message` is one the modelled `Message` pattern
        // (`^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$`) admits, so what a job reports is
        // a value the model would accept.
        assert!(
            message.chars().all(|character| {
                character.is_alphanumeric()
                    || character.is_whitespace()
                    || "_.:/=+-%@".contains(character)
            }),
            "case {case:?}: the Message must stay inside the modelled alphabet, got {message:?}"
        );
        let report = &described["JobProgressReport"];
        let (scanned, imported, refused) = row.counts;
        for (member, expected) in [
            ("TotalNumberOfResourcesScanned", scanned),
            ("TotalNumberOfResourcesImported", imported),
            ("TotalNumberOfResourcesWithCustomerError", refused),
        ] {
            assert_eq!(
                report[member],
                json!(expected),
                "case {case:?}: the progress report must count {member} as {expected}, got \
                 {report}"
            );
        }
    }
}

#[tokio::test]
async fn only_an_active_data_store_can_be_imported_into() {
    let service = HealthLake::with_object_store();
    create_bucket(&service).await;
    stage(
        &service,
        "gated/patients.ndjson",
        &ndjson(&[resource("Patient", Some("pat-gated"), "gated")]),
    )
    .await;

    // A data store still CREATING, one DELETING, one DELETED and an id no record
    // is held for: the four states a start cannot be served in, each with the
    // error shape and the sentence this service answers it with.
    //
    // The ORDER matters, because each state is reached by the clock: the store
    // deleted first is `DELETED` by the time the next one's creation has settled,
    // the one deleted last is still `DELETING`, and the one created last — with
    // nothing waited out after it — is still `CREATING`.
    let (deleted, _path) = service.active_datastore("deleted store").await;
    ok(
        &service,
        "DeleteFHIRDatastore",
        &json!({ "DatastoreId": &deleted }),
    )
    .await;
    let (deleting, _path) = service.active_datastore("deleting store").await;
    ok(
        &service,
        "DeleteFHIRDatastore",
        &json!({ "DatastoreId": &deleting }),
    )
    .await;
    let creating = string_at(
        &ok(
            &service,
            "CreateFHIRDatastore",
            &json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "creating store" }),
        )
        .await,
        "DatastoreId",
    );

    let rows: Vec<(&str, String, &str, String)> = vec![
        (
            "a data store no record is held for",
            UNHELD_DATASTORE_ID.to_string(),
            "ResourceNotFoundException",
            DATASTORE_NOT_FOUND_MESSAGE.to_string(),
        ),
        (
            "a data store that is still CREATING",
            creating.clone(),
            "AccessDeniedException",
            DATASTORE_NOT_ACTIVE_MESSAGE.to_string(),
        ),
        (
            "a data store that is DELETING",
            deleting.clone(),
            "AccessDeniedException",
            DATASTORE_NOT_ACTIVE_MESSAGE.to_string(),
        ),
        (
            // A record the registry still holds is not a not-found, whatever
            // stage of the lifecycle it has reached: the one line this operation
            // draws is ACTIVE or not.
            "a data store that is DELETED",
            deleted.clone(),
            "AccessDeniedException",
            DATASTORE_NOT_ACTIVE_MESSAGE.to_string(),
        ),
    ];
    for (case, datastore_id, code, message) in rows {
        let (status, payload) = service
            .rpc(
                "StartFHIRImportJob",
                &start_request(&datastore_id, &format!("s3://{BUCKET}/gated/")),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused start answers a client fault, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            code,
            "case {case:?}: the refusal must be {code}, got {payload}"
        );
        assert_eq!(
            error_message(&payload),
            message,
            "case {case:?}: the refusal must state the service's own sentence, got {payload}"
        );
    }

    // A refused start mints no job and imports nothing.
    let state = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let minted = state.lock().import_jobs.len();
    assert_eq!(minted, 0, "a refused start must mint no job record");
    assert!(
        held_resources(&service, &creating).is_empty(),
        "a refused start must import nothing"
    );
}

#[tokio::test]
async fn one_client_token_starts_one_import_job() {
    let service = HealthLake::with_object_store();
    create_bucket(&service).await;
    stage(
        &service,
        "token/patients.ndjson",
        &ndjson(&[resource("Patient", Some("pat-token"), "token")]),
    )
    .await;
    let (datastore_id, path) = service.active_datastore("importing store").await;

    let mut request = start_request(&datastore_id, &format!("s3://{BUCKET}/token/"));
    request["ClientToken"] = json!("import-token-1");
    let first = ok(&service, "StartFHIRImportJob", &request).await;
    let job_id = string_at(&first, "JobId");

    // The repeat is answered out of the job the token started, so one token
    // starts one job — and imports once: the resource stays at version 1.
    let repeat = ok(&service, "StartFHIRImportJob", &request).await;
    assert_eq!(
        string_at(&repeat, "JobId"),
        job_id,
        "a repeat under the same token must answer the job that token started, got {repeat}"
    );
    let jobs = listed_jobs(&service, &datastore_id).await;
    assert_eq!(
        jobs.len(),
        1,
        "one ClientToken must start exactly one import job, got {jobs:?}"
    );
    let (status, body) = read_resource(&service, &path, "Patient", "pat-token").await;
    assert_eq!(status, 200, "the imported resource must be readable: {body}");
    assert_eq!(
        body["meta"]["versionId"],
        json!("1"),
        "a replayed start must not import a second time, got {body}"
    );

    // The same token with a DIFFERENT request is a different request, and is
    // refused rather than answered with the earlier job.
    let mut other = request.clone();
    other["JobName"] = json!("a different import");
    let (status, payload) = service.rpc("StartFHIRImportJob", &other).await;
    assert_eq!(
        status, 400,
        "a token reused with a different request is refused, got {status}: {payload}"
    );
    assert_eq!(
        error_code(&payload),
        "ValidationException",
        "a token reused with a different request answers ValidationException, got {payload}"
    );
}

#[tokio::test]
async fn an_import_of_a_held_id_writes_a_further_version() {
    let service = HealthLake::with_object_store();
    create_bucket(&service).await;
    stage(
        &service,
        "versions/first.ndjson",
        &ndjson(&[resource("Patient", Some("pat-versioned"), "first")]),
    )
    .await;
    stage(
        &service,
        "second/second.ndjson",
        &ndjson(&[resource("Patient", Some("pat-versioned"), "second")]),
    )
    .await;
    let (datastore_id, path) = service.active_datastore("importing store").await;

    ok(
        &service,
        "StartFHIRImportJob",
        &start_request(&datastore_id, &format!("s3://{BUCKET}/versions/")),
    )
    .await;
    ok(
        &service,
        "StartFHIRImportJob",
        &start_request(&datastore_id, &format!("s3://{BUCKET}/second/")),
    )
    .await;

    // The logical id identifies the RESOURCE, so the second import is a further
    // version of the one the first imported rather than a second resource.
    let held = held_resources(&service, &datastore_id);
    assert_eq!(
        held.len(),
        1,
        "importing the same logical id twice holds ONE resource, got {:?}",
        held.keys().collect::<Vec<_>>()
    );
    let (status, body) = read_resource(&service, &path, "Patient", "pat-versioned").await;
    assert_eq!(status, 200, "the imported resource must be readable: {body}");
    assert_eq!(
        body["meta"]["versionId"],
        json!("2"),
        "a second import of one logical id writes version 2, got {body}"
    );
    assert_eq!(
        body["language"],
        json!("second"),
        "a read answers the version the LAST import stored, got {body}"
    );
}
