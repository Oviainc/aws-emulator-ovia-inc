// SPDX-License-Identifier: CC0-1.0

//! Import jobs (`CreateImportJob` and `GetImportJob`), end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, deserialization, the handler and serialization are
//! all exercised, and reaches the engine's own state to read the stored job back and to
//! move the instants the clock-owed transitions of its run fall due at.
//!
//! The rules these cases pin:
//!
//! * a read of an import job is computed from the stored job record: it reports the id
//!   the job was minted with, the destination and the data source the create supplied,
//!   the status the clock has made due, when the job was created and completed, and the
//!   records the job really imported;
//! * a `JobId` the account holds no import job for is refused as the resource it is —
//!   `NotFoundException`, HTTP 404 — whatever form that id takes;
//! * `ImportDestination` names exactly one destination, and a contact-list destination
//!   names a list the account holds, so a create that does neither is refused with the
//!   one input error the operation models and persists nothing;
//! * a run's `CREATED` -> `PROCESSING` -> `COMPLETED` transitions are owed by the clock,
//!   not by being read: a read reports whichever of them were already due, and never
//!   moves a run on — or changes anything else about the job — because somebody looked.
//!
//! The tables below span what the rules generalise over: both `ImportDestinationType`
//! destinations with every import action each models, both `DataFormat` values, S3 URLs
//! of differing depth and form, the form and length of a `JobId` the account does not
//! hold, and every status a run reaches.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::ops::types::{
    CONTACT_LIST_IMPORT_ACTION_VALUES, DATA_FORMAT_VALUES, IMPORT_DESTINATION_TYPE_VALUES,
    JOB_STATUS_VALUES, SUPPRESSION_LIST_IMPORT_ACTION_VALUES,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    contact_list_not_found, import_job_not_found, invalid_import_destination,
    ComAmazonawsSesv2SimpleemailserviceV2State, ImportJobRecord, IMPORT_JOB_COMPLETED,
    IMPORT_JOB_COMPLETION_SETTLE, IMPORT_JOB_CREATED, IMPORT_JOB_PROCESSING,
    IMPORT_JOB_PROCESSING_SETTLE,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the cases execute in: the one their `Authorization` credential scope
/// names. An import job belongs to the account, so one Region is enough.
const HOME: &str = "us-east-1";

/// Long enough that a transition held this far out cannot fall due while a case runs.
const HELD: Duration = Duration::from_secs(3600);

/// Just enough to put a due instant in the past.
const ELAPSED: Duration = Duration::from_secs(1);

/// How far apart the instant a create is answered at and the instant the assertion
/// reads may be, so a case cannot fail on the wall clock moving between the two.
const CLOCK_SLACK: Duration = Duration::from_secs(30);

/// `ImportDestinationType$CONTACT_LIST` — the destination type a contact-list
/// destination selects.
const CONTACT_LIST: &str = "CONTACT_LIST";

/// `ImportDestinationType$SUPPRESSION_LIST` — the destination type a suppression-list
/// destination selects.
const SUPPRESSION_LIST: &str = "SUPPRESSION_LIST";

/// One answered request: the status, the error shape when there was one, and the parsed
/// body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The message the error carried.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A string member of the body, or `""` when absent.
    fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A timestamp member of the body, as the epoch seconds the wire carries.
    fn timestamp(&self, member: &str) -> Option<f64> {
        self.body.get(member).and_then(Value::as_f64)
    }
}

/// One import job the cases create, and what the job it stores must carry.
struct Row {
    case: &'static str,
    /// The `ImportDestination` document the request carries.
    destination: Value,
    /// The `ImportDestinationType` that destination selects.
    destination_type: &'static str,
    /// The contact list the destination names, for a contact-list destination.
    contact_list: Option<&'static str>,
    /// `ImportDataSource$S3Url`.
    s3_url: &'static str,
    /// `ImportDataSource$DataFormat`.
    data_format: &'static str,
}

/// The create requests the cases drive.
///
/// The rows span what the rules generalise over: both members of `ImportDestination`,
/// every value of `SuppressionListImportAction` and of `ContactListImportAction`
/// (`smithy_query(query="shape",shape="SuppressionListImportAction,ContactListImportAction")`),
/// both `DataFormat` values, and S3 URLs of differing depth and form within
/// `ImportDataSource$S3Url`'s modelled pattern
/// (`smithy_query(query="shape",shape="S3Url")`: `^s3:\/\/([^\/]+)\/(.*?([^\/]+)\/?)$`)
/// — an object at the root of a bucket, one under nested prefixes, and a prefix ending
/// in a separator.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "suppression list, PUT, CSV, object at the bucket root",
            destination: json!({
                "SuppressionListDestination": { "SuppressionListImportAction": "PUT" },
            }),
            destination_type: SUPPRESSION_LIST,
            contact_list: None,
            s3_url: "s3://ses-imports/suppression.csv",
            data_format: "CSV",
        },
        Row {
            case: "suppression list, DELETE, JSON, nested prefixes",
            destination: json!({
                "SuppressionListDestination": { "SuppressionListImportAction": "DELETE" },
            }),
            destination_type: SUPPRESSION_LIST,
            contact_list: None,
            s3_url: "s3://ses-imports/2026/10/29/suppression-removals.json",
            data_format: "JSON",
        },
        Row {
            case: "contact list, PUT, CSV, prefix ending in a separator",
            destination: json!({
                "ContactListDestination": {
                    "ContactListName": "newsletter",
                    "ContactListImportAction": "PUT",
                },
            }),
            destination_type: CONTACT_LIST,
            contact_list: Some("newsletter"),
            s3_url: "s3://ses-imports/contacts/",
            data_format: "CSV",
        },
        Row {
            case: "contact list, DELETE, JSON, object under one prefix",
            destination: json!({
                "ContactListDestination": {
                    "ContactListName": "product-announcements",
                    "ContactListImportAction": "DELETE",
                },
            }),
            destination_type: CONTACT_LIST,
            contact_list: Some("product-announcements"),
            s3_url: "s3://ses-imports/contacts/unsubscribed.json",
            data_format: "JSON",
        },
    ]
}

/// The `JobId`s the account holds no import job for.
///
/// The rows span the forms a job id takes: the dashed 8-4-4-4-12 hex shape a create
/// mints, the published minimum of one character (`JobId` is `length: min 1`), a long
/// id, and ids carrying the other characters a path label admits.
const UNHELD_JOB_IDS: [(&str, &str); 5] = [
    ("the minted shape", "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"),
    ("the published minimum length", "z"),
    (
        "a long id",
        "00000000000000000000000000000000000000000000000000000000000000000000000000000000",
    ),
    ("dots and underscores", "import_job.2026-10-29"),
    ("mixed case", "ImportJobThatWasNeverCreated"),
];

/// The emulated service behind its router, plus the shared engine so a case can read the
/// stored job back.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// Send one request to `path`, with `body` as its document when it carries one.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let host = format!("email.{HOME}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{HOME}/ses/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// Send one `CreateImportJob` request.
    async fn create(&self, body: Value) -> Reply {
        self.call("POST", "/v2/email/import-jobs", Some(body)).await
    }

    /// Send one `GetImportJob` request.
    async fn get(&self, job_id: &str) -> Reply {
        self.call("GET", &format!("/v2/email/import-jobs/{job_id}"), None)
            .await
    }

    /// Create the contact list `row` names, when it names one.
    async fn create_destination_list(&self, row: &Row) {
        let Some(name) = row.contact_list else {
            return;
        };
        let reply = self
            .call(
                "POST",
                "/v2/email/contact-lists",
                Some(json!({ "ContactListName": name })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: the destination list must be created, got {} {:?}",
            row.case, reply.status, reply.body
        );
    }

    /// Create the import job `row` describes, and answer the id minted for it.
    async fn create_row(&self, row: &Row) -> String {
        let reply = self
            .create(json!({
                "ImportDestination": row.destination,
                "ImportDataSource": {
                    "S3Url": row.s3_url,
                    "DataFormat": row.data_format,
                },
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: a create of a modelled destination is served, got {} {:?}",
            row.case, reply.status, reply.body
        );
        let job_id = reply.text("JobId");
        assert!(
            !job_id.is_empty(),
            "case {:?}: expected the minted JobId, got {:?}",
            row.case,
            reply.body
        );
        job_id
    }

    /// The stored job `job_id` names, if the store holds one.
    fn stored(&self, job_id: &str) -> Option<ImportJobRecord> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store.import_jobs.get(job_id).cloned()
    }

    /// How many import jobs the account holds.
    fn stored_count(&self) -> usize {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store.import_jobs.len()
    }

    /// Move the instants `job_id`'s transitions fall due at.
    fn set_due(&self, job_id: &str, processing_due_at: SystemTime, completion_due_at: SystemTime) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let record = store
            .import_jobs
            .get_mut(job_id)
            .unwrap_or_else(|| panic!("case {job_id:?}: the job must be in the store"));
        record.processing_due_at = processing_due_at;
        record.completion_due_at = completion_due_at;
    }

    /// Hold both of `job_id`'s transitions off, so an assertion about the status it was
    /// created in cannot race the span.
    fn hold_created(&self, job_id: &str) {
        let now = SystemTime::now();
        self.set_due(job_id, now + HELD, now + HELD + HELD);
    }

    /// Bring the start of `job_id`'s run due, without waiting for it, and hold its
    /// completion off.
    fn bring_processing_due(&self, job_id: &str) {
        let now = SystemTime::now();
        self.set_due(job_id, now - ELAPSED, now + HELD);
    }

    /// Bring the completion of `job_id`'s run due, without waiting for it, at a whole
    /// epoch second so the timestamp it is reported with compares exactly. Answers that
    /// instant.
    fn bring_completion_due(&self, job_id: &str) -> f64 {
        let now = SystemTime::now();
        let completed_at_secs = now
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs()
            - ELAPSED.as_secs();
        let completed_at = SystemTime::UNIX_EPOCH + Duration::from_secs(completed_at_secs);
        self.set_due(job_id, now - ELAPSED - ELAPSED, completed_at);
        completed_at_secs as f64
    }
}

/// How close a timestamp the wire reports must be to the instant the record holds.
///
/// A modelled `Timestamp` is serialised to the second with a fraction, so comparing it
/// with the stored instant is a comparison to within the resolution the wire carries
/// rather than bit for bit.
const TIMESTAMP_TOLERANCE: f64 = 0.001;

/// Epoch seconds at `instant`, as the wire reports a timestamp.
fn epoch_secs(instant: SystemTime) -> f64 {
    instant
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

#[tokio::test]
async fn a_read_reports_the_stored_job_the_create_minted() {
    // The destinations, actions and formats the rows span are the modelled sets, so no
    // value of any of them goes untested.
    assert_eq!(
        IMPORT_DESTINATION_TYPE_VALUES,
        &[CONTACT_LIST, SUPPRESSION_LIST],
        "the destination types these cases span must be the modelled set, got \
         {IMPORT_DESTINATION_TYPE_VALUES:?}"
    );
    assert_eq!(
        (
            SUPPRESSION_LIST_IMPORT_ACTION_VALUES,
            CONTACT_LIST_IMPORT_ACTION_VALUES,
            DATA_FORMAT_VALUES,
        ),
        (
            &["DELETE", "PUT"][..],
            &["DELETE", "PUT"][..],
            &["CSV", "JSON"][..],
        ),
        "the import actions and data formats these cases span must be the modelled sets"
    );

    let ses = Ses::new();
    let mut minted: Vec<String> = Vec::new();

    for row in rows() {
        let case = row.case;
        ses.create_destination_list(&row).await;

        let before = SystemTime::now();
        let job_id = ses.create_row(&row).await;
        let after = SystemTime::now();
        assert!(
            !minted.contains(&job_id),
            "case {case:?}: every create mints a job id of its own, got {job_id:?} again"
        );
        minted.push(job_id.clone());

        // The create stored the job: the id it answered with names it, and the record
        // carries what the request supplied.
        let record = ses.stored(&job_id).unwrap_or_else(|| {
            panic!("case {case:?}: the create must store a job under {job_id:?}")
        });
        assert_eq!(
            record.job_id, job_id,
            "case {case:?}: the stored job carries the id the create answered with, got {:?}",
            record.job_id
        );
        assert_eq!(
            record.destination_type(),
            Some(row.destination_type),
            "case {case:?}: expected the destination type the destination selects, got {:?}",
            record.destination_type()
        );
        assert_eq!(
            (
                record.data_source.s3_url.as_str(),
                record.data_source.data_format.as_str(),
            ),
            (row.s3_url, row.data_format),
            "case {case:?}: expected the data source the request supplied, got {:?}",
            record.data_source
        );
        assert_eq!(
            (record.processed_records_count, record.failed_records_count),
            (0, 0),
            "case {case:?}: a job that has imported nothing counts no record, got {:?}",
            (record.processed_records_count, record.failed_records_count)
        );
        assert!(
            record.created_at >= before - CLOCK_SLACK && record.created_at <= after + CLOCK_SLACK,
            "case {case:?}: expected the job created at the instant of the call, got {:?}",
            record.created_at
        );
        // Both transitions of the run are owed by the clock at the spans the state
        // module states, counted from when the job was created.
        assert_eq!(
            (
                record.processing_due_at.duration_since(record.created_at).ok(),
                record.completion_due_at.duration_since(record.created_at).ok(),
            ),
            (
                Some(IMPORT_JOB_PROCESSING_SETTLE),
                Some(IMPORT_JOB_PROCESSING_SETTLE + IMPORT_JOB_COMPLETION_SETTLE),
            ),
            "case {case:?}: expected both transitions due at the stated spans, got {:?}",
            (record.processing_due_at, record.completion_due_at)
        );

        // A read reports that stored job, member for member.
        ses.hold_created(&job_id);
        let reply = ses.get(&job_id).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a read of a stored job is served, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.text("JobId"),
            job_id,
            "case {case:?}: expected the id the job was minted with, got {:?}",
            reply.body
        );
        assert_eq!(
            reply.body.get("ImportDestination"),
            Some(&row.destination),
            "case {case:?}: expected the destination the create supplied, got {:?}",
            reply.body.get("ImportDestination")
        );
        assert_eq!(
            reply.body.get("ImportDataSource"),
            Some(&json!({ "S3Url": row.s3_url, "DataFormat": row.data_format })),
            "case {case:?}: expected the data source the create supplied, got {:?}",
            reply.body.get("ImportDataSource")
        );
        assert_eq!(
            reply.text("JobStatus"),
            IMPORT_JOB_CREATED,
            "case {case:?}: a job whose run has not started has just been created, got {:?}",
            reply.body
        );
        let created = reply.timestamp("CreatedTimestamp").unwrap_or_default();
        let stored = epoch_secs(record.created_at);
        assert!(
            (created - stored).abs() < TIMESTAMP_TOLERANCE,
            "case {case:?}: expected the instant the job was created ({stored:?}), got \
             {created:?}"
        );
        assert_eq!(
            reply.timestamp("CompletedTimestamp"),
            None,
            "case {case:?}: a run that has not completed has no completion instant, got {:?}",
            reply.timestamp("CompletedTimestamp")
        );
        assert_eq!(
            reply.body.get("FailureInfo"),
            None,
            "case {case:?}: a run nothing diagnosed a failure of carries no failure details, \
             got {:?}",
            reply.body.get("FailureInfo")
        );
        assert_eq!(
            (
                reply.body.get("ProcessedRecordsCount").and_then(Value::as_i64),
                reply.body.get("FailedRecordsCount").and_then(Value::as_i64),
            ),
            (Some(0), Some(0)),
            "case {case:?}: expected the counts the record holds, got {:?}",
            reply.body
        );
        assert!(
            JOB_STATUS_VALUES.contains(&reply.text("JobStatus").as_str()),
            "case {case:?}: expected a modelled JobStatus, got {:?}",
            reply.text("JobStatus")
        );
    }

    assert_eq!(
        ses.stored_count(),
        minted.len(),
        "every create stored exactly one job, got {} for {} creates",
        ses.stored_count(),
        minted.len()
    );

    // Each stored job is read back by its own id: a read never reports another job's
    // record.
    for (index, job_id) in minted.iter().enumerate() {
        let reply = ses.get(job_id).await;
        assert_eq!(
            reply.text("JobId"),
            *job_id,
            "job {index}: a read reports the job its id names, got {:?}",
            reply.body
        );
    }
}

#[tokio::test]
async fn a_read_of_a_job_id_the_account_does_not_hold_is_refused_as_not_found() {
    let ses = Ses::new();

    // An account holding no import job at all, and then one holding a job of its own:
    // neither answers for an id it does not hold.
    for (case, job_id) in UNHELD_JOB_IDS {
        let reply = ses.get(job_id).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException for {job_id:?}, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            import_job_not_found(job_id),
            "case {case:?}: expected the refusal to name the import job it was asked for, got {:?}",
            reply.message()
        );
    }

    let row = &rows()[0];
    ses.create_destination_list(row).await;
    let held = ses.create_row(row).await;
    for (case, job_id) in UNHELD_JOB_IDS {
        let reply = ses.get(job_id).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (404, "NotFoundException"),
            "case {case:?}: a held job does not answer for {job_id:?}, got {} {:?}",
            reply.status,
            reply.body
        );
    }
    let reply = ses.get(&held).await;
    assert_eq!(
        reply.status, 200,
        "the job the account does hold is still served, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        ses.stored_count(),
        1,
        "a refused read stores nothing, got {} jobs",
        ses.stored_count()
    );
}

#[tokio::test]
async fn the_status_a_read_reports_is_the_one_the_clock_has_made_due() {
    let ses = Ses::new();
    let row = &rows()[2];
    ses.create_destination_list(row).await;
    let job_id = ses.create_row(row).await;

    // Before the start of the run falls due.
    ses.hold_created(&job_id);
    let reply = ses.get(&job_id).await;
    assert_eq!(
        (reply.text("JobStatus"), reply.timestamp("CompletedTimestamp")),
        (IMPORT_JOB_CREATED.to_string(), None),
        "case \"held\": a run not yet started has just been created, got {:?}",
        reply.body
    );

    // A read does not move the run on, nor change anything else about the job.
    let before = format!("{:?}", ses.stored(&job_id));
    let again = ses.get(&job_id).await;
    assert_eq!(
        again.text("JobStatus"),
        IMPORT_JOB_CREATED,
        "case \"read twice\": a read must not move the run on, got {:?}",
        again.body
    );
    assert_eq!(
        format!("{:?}", ses.stored(&job_id)),
        before,
        "case \"read twice\": a read must leave the stored job as it was, got {:?}",
        ses.stored(&job_id)
    );

    // Once the start of the run is due.
    ses.bring_processing_due(&job_id);
    let reply = ses.get(&job_id).await;
    assert_eq!(
        (reply.text("JobStatus"), reply.timestamp("CompletedTimestamp")),
        (IMPORT_JOB_PROCESSING.to_string(), None),
        "case \"processing due\": a started run is processing and has not completed, got {:?}",
        reply.body
    );

    // Once its completion is due: the run completed when it fell due, not when somebody
    // first looked.
    let completed_at = ses.bring_completion_due(&job_id);
    let reply = ses.get(&job_id).await;
    assert_eq!(
        (
            reply.text("JobStatus"),
            reply.timestamp("CompletedTimestamp")
        ),
        (IMPORT_JOB_COMPLETED.to_string(), Some(completed_at)),
        "case \"completion due\": expected the completed run reported at the instant it fell \
         due, got {:?}",
        reply.body
    );
    assert_eq!(
        ses.stored(&job_id).map(|record| record.status(SystemTime::now())),
        Some(IMPORT_JOB_COMPLETED),
        "case \"completion due\": the stored job reports the same status, got {:?}",
        ses.stored(&job_id)
    );
}

#[tokio::test]
async fn a_create_that_names_no_one_destination_the_account_holds_is_refused() {
    /// One refused create: the destination document and the sentence it is refused with.
    struct Refusal {
        case: &'static str,
        destination: Value,
        message: String,
    }

    let refusals = vec![
        Refusal {
            case: "neither destination",
            destination: json!({}),
            message: invalid_import_destination(),
        },
        Refusal {
            case: "both destinations",
            destination: json!({
                "SuppressionListDestination": { "SuppressionListImportAction": "PUT" },
                "ContactListDestination": {
                    "ContactListName": "newsletter",
                    "ContactListImportAction": "PUT",
                },
            }),
            message: invalid_import_destination(),
        },
        Refusal {
            case: "a contact list the account does not hold",
            destination: json!({
                "ContactListDestination": {
                    "ContactListName": "never-created",
                    "ContactListImportAction": "PUT",
                },
            }),
            message: contact_list_not_found("never-created"),
        },
        Refusal {
            case: "a contact list the account does not hold, under the other action",
            destination: json!({
                "ContactListDestination": {
                    "ContactListName": "also-never-created",
                    "ContactListImportAction": "DELETE",
                },
            }),
            message: contact_list_not_found("also-never-created"),
        },
    ];

    for refusal in refusals {
        let ses = Ses::new();
        let case = refusal.case;
        let reply = ses
            .create(json!({
                "ImportDestination": refusal.destination,
                "ImportDataSource": {
                    "S3Url": "s3://ses-imports/contacts.csv",
                    "DataFormat": "CSV",
                },
            }))
            .await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "BadRequestException"),
            "case {case:?}: expected 400 BadRequestException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.message(),
            refusal.message,
            "case {case:?}: expected the sentence the state module states, got {:?}",
            reply.message()
        );
        assert_eq!(
            ses.stored_count(),
            0,
            "case {case:?}: a refused create persists nothing, got {} jobs",
            ses.stored_count()
        );
    }
}
