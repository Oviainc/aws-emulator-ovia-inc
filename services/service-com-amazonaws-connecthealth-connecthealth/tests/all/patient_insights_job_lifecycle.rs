// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the patient insights job pair,
//! `StartPatientInsightsJob` -> `GetPatientInsightsJob`.
//!
//! Every case drives the service the way a client does — a signed restJson1 request
//! through the router's own dispatch — and asserts the RULE the two operations follow
//! rather than one recorded response:
//!
//! 1. A start PERSISTS the job and the reader PROJECTS the stored record: every member
//!    the get answers with is read back out of what the start wrote, for every shape of
//!    request the model allows, so the table's rows differ along what the rule
//!    generalises over (which input source is configured, how many sources, which
//!    optional members are supplied, the size of a bounded string, the enum value
//!    chosen).
//! 2. A job is SCOPED to its domain: only the (`domainId`, `jobId`) pair it was created
//!    under resolves it, and every other pair is the modelled
//!    `ResourceNotFoundException`.
//! 3. The STATUS a get reports is the one the clock has moved the job to — never zero
//!    milliseconds after the start, and terminal once it is terminal.
//! 4. `clientToken` is an `@idempotencyToken`: a retry resolves to the job the first
//!    call created, and the same token with different parameters is the modelled
//!    `ConflictException`.
//! 5. Every value of the modelled `JobStatus` enum is projected out of state, including
//!    the ones this emulator's own lifecycle never reaches.
//!
//! The constraints asserted here are the model's own
//! (`smithy_query(query="closure", operation="GetPatientInsightsJob")`), quoted beside
//! each constant.

use std::collections::BTreeSet;
use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::ops::types::{
    InputDataConfig, InsightsContext, InsightsOutput, OutputDataConfig,
    PatientInsightsEncounterContext, PatientInsightsPatientContext, UserContext,
    JOB_STATUS_VALUES, PRONOUNS_VALUES,
};
use service_com_amazonaws_connecthealth_connecthealth::state::{
    self, JobStatus, PatientInsightsJob, PatientInsightsJobSpec,
    MAX_PATIENT_INSIGHTS_JOBS_PER_DOMAIN, PATIENT_INSIGHTS_INFERENCE_UNAVAILABLE,
    PATIENT_INSIGHTS_JOB_STEP_SPAN,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};

/// `JobId`: `@length 1..36`, with no pattern.
const JOB_ID_LEN: (usize, usize) = (1, 36);
/// `PatientInsightsEncounterContext$encounterReason`: `@length 1..256`, pattern
/// `^[a-zA-Z0-9 .,-]+$`.
const ENCOUNTER_REASON_LEN: (usize, usize) = (1, 256);
/// `S3Sources`: `@length max 10`.
const MAX_S3_SOURCES: usize = 10;

/// `POST /domain/{domainId}/patient-insights-job` — `StartPatientInsightsJob`'s route.
fn start_path(domain_id: &str) -> String {
    format!("/domain/{domain_id}/patient-insights-job")
}

/// `GET /domain/{domainId}/patient-insights-job/{jobId}` — `GetPatientInsightsJob`'s
/// route.
fn job_path(domain_id: &str, job_id: &str) -> String {
    format!("/domain/{domain_id}/patient-insights-job/{job_id}")
}

/// One member of a response body, which must be present.
fn member(case: &str, body: &Value, name: &str) -> Value {
    body.get(name)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: member {name} must be present in {body}"))
}

/// Whether `arn` is the `JobArn` of `job_id` under `domain_id`, as the modelled pattern
/// declares it:
/// `^arn:aws[-a-z]*:health-agent:[-a-z0-9]+:[0-9]{12}:domain/[-a-zA-Z0-9-]+/patient-insights-job/[-a-zA-Z0-9_/.]+$`
/// — matched by hand, field by field, rather than with a regex engine (the sibling
/// suites in this workspace do the same).
fn is_job_arn(arn: &str, region: &str, domain_id: &str, job_id: &str) -> bool {
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some("aws"),
        Some("health-agent"),
        Some(arn_region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return false;
    };
    let account_ok = account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit());
    arn_region == region
        && account_ok
        && resource == format!("domain/{domain_id}/patient-insights-job/{job_id}")
        && (JOB_ID_LEN.0..=JOB_ID_LEN.1).contains(&job_id.len())
}

/// A `StartPatientInsightsJob` body carrying only the required members, with the
/// parameters a row wants varied folded in by the caller.
fn minimal_body() -> Value {
    json!({
        "patientContext": { "patientId": "patient-0001" },
        "insightsContext": { "insightsType": "PRE_VISIT" },
        "encounterContext": { "encounterReason": "Annual checkup" },
        "userContext": { "role": "CLINICIAN", "userId": "clinician-0001" },
        "inputDataConfig": { "s3Sources": [ { "uri": "s3://clinic-input/patient-0001/" } ] },
        "outputDataConfig": { "s3OutputPath": "s3://clinic-output/insights/" },
    })
}

/// `n` distinct S3 sources, for the row that fills the modelled `S3Sources` list.
fn s3_sources(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|index| json!({ "uri": format!("s3://clinic-input/chart-{index}.json") }))
            .collect(),
    )
}

/// `StartPatientInsightsJob`, asserting the modelled success and returning its body.
async fn start_job(emulator: &Emulator, case: &str, domain_id: &str, body: &Value) -> Value {
    let (status, started) = emulator
        .send("POST", &start_path(domain_id), Some(body))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartPatientInsightsJob must answer 200, got {status} with {started}"
    );
    started
}

/// `GetPatientInsightsJob`, asserting the modelled success and returning its body.
async fn get_job(emulator: &Emulator, case: &str, domain_id: &str, job_id: &str) -> Value {
    let (status, fetched) = emulator
        .send("GET", &job_path(domain_id, job_id), None)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetPatientInsightsJob must answer 200, got {status} with {fetched}"
    );
    fetched
}

/// A domain to start jobs in.
async fn domain_of(emulator: &Emulator, name: &str) -> String {
    string_at(
        &emulator.create_domain(&json!({ "name": name })).await,
        "domainId",
    )
}

/// The rows a start is driven with: one per shape of request the model allows, so the
/// round-trip is asserted over the CLASS of job parameters rather than one recorded
/// body. The first row is the minimal required set; the rest vary which input source is
/// configured, how many sources it carries, which optional members are supplied, the
/// size of a bounded string, and which enum value is chosen.
fn parameter_rows() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![("required members only".to_string(), minimal_body())];

    let mut fhir = minimal_body();
    fhir["inputDataConfig"] = json!({
        "fhirServer": { "fhirEndpoint": "https://fhir.example.org/r4" },
    });
    rows.push(("FHIR server as the only input source".to_string(), fhir));

    let mut fhir_token = minimal_body();
    fhir_token["inputDataConfig"] = json!({
        "fhirServer": {
            "fhirEndpoint": "http://fhir.internal.example/r4",
            "oauthToken": "token-value",
        },
    });
    rows.push((
        "FHIR server with an OAuth token".to_string(),
        fhir_token,
    ));

    for count in [1, 2, MAX_S3_SOURCES] {
        let mut body = minimal_body();
        body["inputDataConfig"] = json!({ "s3Sources": s3_sources(count) });
        rows.push((format!("{count} S3 source(s)"), body));
    }

    let mut both = minimal_body();
    both["inputDataConfig"] = json!({
        "fhirServer": { "fhirEndpoint": "https://fhir.example.org/r4" },
        "s3Sources": s3_sources(3),
    });
    rows.push(("both input sources at once".to_string(), both));

    let mut neither = minimal_body();
    neither["inputDataConfig"] = json!({});
    rows.push((
        "an input config with neither source: nothing is invented for it".to_string(),
        neither,
    ));

    // `Pronouns`' whole catalog, from the modelled enum (`PRONOUNS_VALUES`), so no row
    // is keyed to one value — including `THEY_THEM`, which no example in the issue or
    // the model's docs mentions.
    for pronouns in PRONOUNS_VALUES {
        let mut body = minimal_body();
        body["patientContext"] = json!({
            "patientId": "patient-dob",
            "dateOfBirth": "1984-02-29",
            "pronouns": pronouns,
        });
        rows.push((format!("patient context with pronouns {pronouns}"), body));
    }

    let mut specialty = minimal_body();
    specialty["userContext"] = json!({
        "role": "CLINICIAN",
        "userId": "clinician-specialty",
        "specialty": "PRIMARY_CARE",
    });
    rows.push(("user context with a specialty".to_string(), specialty));

    // Both published ends of `encounterReason`'s `@length 1..256`.
    for len in [ENCOUNTER_REASON_LEN.0, ENCOUNTER_REASON_LEN.1] {
        let mut body = minimal_body();
        body["encounterContext"] = json!({ "encounterReason": "a".repeat(len) });
        rows.push((format!("encounterReason of {len} characters"), body));
    }

    let mut nested_output = minimal_body();
    nested_output["outputDataConfig"] =
        json!({ "s3OutputPath": "s3://clinic-output/2026/01/insights/run.json" });
    rows.push((
        "a nested output prefix".to_string(),
        nested_output,
    ));

    rows
}

#[tokio::test]
async fn a_started_job_is_persisted_and_read_back_member_for_member() {
    let region = "us-west-2";
    let emulator = Emulator::new(region);
    let domain_id = domain_of(&emulator, "clinic-insights").await;

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, body) in parameter_rows() {
        let case = case.as_str();
        let started = start_job(&emulator, case, &domain_id, &body).await;

        let job_id = string_at(&started, "jobId");
        assert!(
            (JOB_ID_LEN.0..=JOB_ID_LEN.1).contains(&job_id.len()),
            "case {case:?}: jobId must be {}..{} characters, got {job_id:?} ({} characters)",
            JOB_ID_LEN.0,
            JOB_ID_LEN.1,
            job_id.len()
        );
        assert!(
            minted.insert(job_id.clone()),
            "case {case:?}: every start must mint a fresh jobId, got the reused {job_id:?}"
        );
        let job_arn = string_at(&started, "jobArn");
        assert!(
            is_job_arn(&job_arn, region, &domain_id, &job_id),
            "case {case:?}: expected the JobArn of {job_id} under {domain_id}, got {job_arn:?}"
        );
        let creation_time = member(case, &started, "creationTime");

        let fetched = get_job(&emulator, case, &domain_id, &job_id).await;
        assert_eq!(
            string_at(&fetched, "jobId"),
            job_id,
            "case {case:?}: the reader must report the stored jobId, got {fetched}"
        );
        assert_eq!(
            string_at(&fetched, "jobArn"),
            job_arn,
            "case {case:?}: the reader must report the stored jobArn, got {fetched}"
        );
        assert_eq!(
            member(case, &fetched, "creationTime"),
            creation_time,
            "case {case:?}: the reader must report the instant the start recorded, got {fetched}"
        );
        // Nothing has transitioned yet: the span the clock owes is never zero, so a
        // caller reading straight after the start still sees the submitted job, whose
        // last change IS its creation.
        assert_eq!(
            string_at(&fetched, "jobStatus"),
            "SUBMITTED",
            "case {case:?}: a job read straight after the start must still be SUBMITTED, \
             got {fetched}"
        );
        assert_eq!(
            member(case, &fetched, "updatedTime"),
            creation_time,
            "case {case:?}: a job that has not transitioned was last updated when it was \
             created, got {fetched}"
        );
        // No run produced insights, so there is no output to point at and nothing to
        // say about the status yet.
        assert!(
            fetched.get("insightsOutput").is_none(),
            "case {case:?}: a job that generated no insights must carry no insightsOutput, \
             got {fetched}"
        );
        assert!(
            fetched.get("statusDetails").is_none(),
            "case {case:?}: a freshly submitted job has no status detail, got {fetched}"
        );

        // The round trip: each parameter member the reader reports is byte-for-byte the
        // one the start was given — an absent optional member stays absent.
        for name in [
            "patientContext",
            "insightsContext",
            "encounterContext",
            "userContext",
            "inputDataConfig",
            "outputDataConfig",
        ] {
            assert_eq!(
                member(case, &fetched, name),
                body[name],
                "case {case:?}: {name} must read back as it was persisted, got {fetched}"
            );
        }
    }
}

#[tokio::test]
async fn the_clock_moves_a_submitted_job_to_the_modelled_failure_status() {
    let emulator = Emulator::new("eu-west-1");
    let domain_id = domain_of(&emulator, "clinic-lifecycle").await;
    let started = start_job(&emulator, "lifecycle", &domain_id, &minimal_body()).await;
    let job_id = string_at(&started, "jobId");
    let creation_time = member("lifecycle", &started, "creationTime");

    // Zero milliseconds after the start: still SUBMITTED.
    let fetched = get_job(&emulator, "straight after the start", &domain_id, &job_id).await;
    assert_eq!(
        string_at(&fetched, "jobStatus"),
        "SUBMITTED",
        "a job read straight after the start must be SUBMITTED, got {fetched}"
    );

    // Real elapsed time is required (the handlers read `SystemTime::now()`), and the
    // workspace `tokio` dependency does not enable the `time` feature, so these are
    // plain blocking sleeps rather than `tokio::time::sleep(..).await`. A little past
    // each span, so the transition the clock owes has fallen due.
    let past_a_span = PATIENT_INSIGHTS_JOB_STEP_SPAN + PATIENT_INSIGHTS_JOB_STEP_SPAN / 4;
    std::thread::sleep(past_a_span);
    let fetched = get_job(&emulator, "one span in", &domain_id, &job_id).await;
    assert_eq!(
        string_at(&fetched, "jobStatus"),
        "IN_PROGRESS",
        "one span after the start the job must have been picked up, got {fetched}"
    );
    assert!(
        fetched.get("statusDetails").is_none(),
        "a job still running has nothing to report about its status, got {fetched}"
    );

    std::thread::sleep(past_a_span);
    let failed = get_job(&emulator, "two spans in", &domain_id, &job_id).await;
    assert_eq!(
        string_at(&failed, "jobStatus"),
        "FAILED",
        "a run this emulator cannot perform must end in the modelled failure status, \
         got {failed}"
    );
    assert_eq!(
        string_at(&failed, "statusDetails"),
        PATIENT_INSIGHTS_INFERENCE_UNAVAILABLE,
        "the failure must say why nothing ran, got {failed}"
    );
    assert!(
        failed.get("insightsOutput").is_none(),
        "a job that generated nothing must carry no insightsOutput, got {failed}"
    );
    assert_ne!(
        member("two spans in", &failed, "updatedTime"),
        creation_time,
        "a transitioned job must report when it last changed, got {failed}"
    );
    assert_eq!(
        member("two spans in", &failed, "creationTime"),
        creation_time,
        "the creation instant must not move, got {failed}"
    );

    // Terminal means terminal: two further reads report exactly the same record, so a
    // read never advances what a later caller sees.
    let again = get_job(&emulator, "after the failure", &domain_id, &job_id).await;
    let and_again = get_job(&emulator, "after the failure, twice", &domain_id, &job_id).await;
    assert_eq!(
        again, failed,
        "a terminal job must read the same every time, got {again}"
    );
    assert_eq!(
        and_again, failed,
        "a terminal job must read the same every time, got {and_again}"
    );
}

#[tokio::test]
async fn a_job_is_readable_only_under_its_own_domain() {
    let emulator = Emulator::new("us-east-1");
    let home = domain_of(&emulator, "clinic-home").await;
    let other = domain_of(&emulator, "clinic-other").await;
    let started = start_job(&emulator, "scoping", &home, &minimal_body()).await;
    let job_id = string_at(&started, "jobId");

    // The job is readable under the domain it was started in.
    let fetched = get_job(&emulator, "its own domain", &home, &job_id).await;
    assert_eq!(
        string_at(&fetched, "jobId"),
        job_id,
        "the job must be readable under its own domain, got {fetched}"
    );

    // Every other (domainId, jobId) pair is the modelled 404: a sibling domain, a
    // jobId nothing minted, and a domainId no domain holds.
    let unknown_job = "0".repeat(JOB_ID_LEN.1);
    let unknown_domain = "dom-0123456789abcdef0";
    for (case, domain, job, expected_in_message) in [
        ("a sibling domain", other.as_str(), job_id.as_str(), job_id.as_str()),
        ("a jobId nothing minted", home.as_str(), unknown_job.as_str(), unknown_job.as_str()),
        ("a jobId one character long", home.as_str(), "x", "x"),
        (
            "a domainId no domain holds",
            unknown_domain,
            job_id.as_str(),
            unknown_domain,
        ),
    ] {
        let (status, body) = emulator.send("GET", &job_path(domain, job), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the modelled 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: got {body}"
        );
        let detail = string_at(&body, "Message");
        assert!(
            detail.contains(expected_in_message),
            "case {case:?}: the detail must name the identifier that did not resolve \
             ({expected_in_message}), got {detail:?}"
        );
    }

    // And a domain on its way out holds nothing readable either.
    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{home}"), None)
        .await;
    assert_eq!(status, 200, "DeleteDomain got {status} with {deleted}");
    let (status, body) = emulator.send("GET", &job_path(&home, &job_id), None).await;
    assert_eq!(
        status, 404,
        "a job under a deleted domain must answer the modelled 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "a job under a deleted domain got {body}"
    );
}

#[tokio::test]
async fn a_client_token_makes_a_start_idempotent_and_a_changed_request_a_conflict() {
    let emulator = Emulator::new("us-east-1");
    let domain_id = domain_of(&emulator, "clinic-idempotent").await;

    // One token, one job: the retry resolves to the job the first call created.
    let token = "token-retry";
    let mut body = minimal_body();
    body["clientToken"] = json!(token);
    let first = start_job(&emulator, "first call", &domain_id, &body).await;
    for attempt in ["second call", "third call"] {
        let retried = start_job(&emulator, attempt, &domain_id, &body).await;
        assert_eq!(
            retried, first,
            "case {attempt:?}: a retry of the same token must answer the first job, got {retried}"
        );
    }

    // The same token with DIFFERENT parameters is a conflict — one row per member a
    // request can differ in.
    let changes: Vec<(&str, Value)> = vec![
        ("a different patientId", json!({ "patientId": "patient-9999" })),
        ("an added dateOfBirth", json!({ "patientId": "patient-0001", "dateOfBirth": "2000-01-31" })),
    ];
    for (case, patient_context) in changes {
        let mut changed = body.clone();
        changed["patientContext"] = patient_context;
        let (status, refused) = emulator
            .send("POST", &start_path(&domain_id), Some(&changed))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a reused token with changed parameters must answer 409, \
             got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: got {refused}"
        );
    }
    for (case, member_name, value) in [
        (
            "a different encounterReason",
            "encounterContext",
            json!({ "encounterReason": "Follow up visit" }),
        ),
        (
            "a different userId",
            "userContext",
            json!({ "role": "CLINICIAN", "userId": "clinician-9999" }),
        ),
        (
            "an added specialty",
            "userContext",
            json!({ "role": "CLINICIAN", "userId": "clinician-0001", "specialty": "PRIMARY_CARE" }),
        ),
        (
            "a different input source",
            "inputDataConfig",
            json!({ "fhirServer": { "fhirEndpoint": "https://fhir.example.org/r4" } }),
        ),
        (
            "an extra input source",
            "inputDataConfig",
            json!({ "s3Sources": s3_sources(2) }),
        ),
        (
            "a different output path",
            "outputDataConfig",
            json!({ "s3OutputPath": "s3://clinic-output/elsewhere/" }),
        ),
    ] {
        let mut changed = body.clone();
        changed[member_name] = value;
        let (status, refused) = emulator
            .send("POST", &start_path(&domain_id), Some(&changed))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: a reused token with changed parameters must answer 409, \
             got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: got {refused}"
        );
    }

    // A different token, and no token at all, each start their own job.
    let mut fresh_token = minimal_body();
    fresh_token["clientToken"] = json!("token-another");
    let mut ids: BTreeSet<String> = BTreeSet::new();
    ids.insert(string_at(&first, "jobId"));
    for (case, request) in [
        ("another token", fresh_token),
        ("no token at all", minimal_body()),
        ("no token at all, again", minimal_body()),
    ] {
        let started = start_job(&emulator, case, &domain_id, &request).await;
        assert!(
            ids.insert(string_at(&started, "jobId")),
            "case {case:?}: a start that is not an idempotent retry must create a new job, \
             got {started}"
        );
    }

    // The token is scoped to its domain: the same token in another domain starts a job
    // there rather than resolving the first one.
    let elsewhere = domain_of(&emulator, "clinic-elsewhere").await;
    let started = start_job(&emulator, "the same token elsewhere", &elsewhere, &body).await;
    assert!(
        ids.insert(string_at(&started, "jobId")),
        "a client token must be scoped to its own domain, got {started}"
    );
}

/// Seed one job directly into this service's store, in a status the emulator's own
/// lifecycle may not reach.
///
/// The way to pin a reader against a record no operation writes: the job is written
/// through the SHARED engine the service was mounted on, exactly as a handler would.
fn seed_job(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    job_id: &str,
    status: JobStatus,
    status_details: Option<&str>,
    insights_output: Option<&str>,
) {
    let now = SystemTime::now();
    let store = state::service_state(&**emulator.engine()).expect("this service's own store");
    let mut store = store.lock();
    let domain = store
        .live_domain_mut(domain_id)
        .unwrap_or_else(|| panic!("case {case:?}: a live domain to seed"));
    let job = PatientInsightsJob {
        job_id: job_id.to_string(),
        domain_id: domain_id.to_string(),
        arn: state::patient_insights_job_arn(&domain.arn, job_id),
        status,
        status_details: status_details.map(str::to_string),
        insights_output: insights_output.map(|uri| InsightsOutput {
            uri: uri.to_string(),
        }),
        created_at: now,
        updated_at: now,
        // Seeded in the status under test, so no transition is owed.
        next_transition_at: None,
        spec: PatientInsightsJobSpec {
            patient_context: PatientInsightsPatientContext {
                patient_id: "patient-seeded".to_string(),
                date_of_birth: None,
                pronouns: None,
            },
            insights_context: InsightsContext {
                insights_type: "PRE_VISIT".to_string(),
            },
            encounter_context: PatientInsightsEncounterContext {
                encounter_reason: "Seeded encounter".to_string(),
            },
            user_context: UserContext {
                role: "CLINICIAN".to_string(),
                user_id: "clinician-seeded".to_string(),
                specialty: None,
            },
            input_data_config: InputDataConfig {
                fhir_server: None,
                s3_sources: None,
            },
            output_data_config: OutputDataConfig {
                s3_output_path: "s3://clinic-output/seeded/".to_string(),
            },
        },
    };
    domain.patient_insights_jobs.insert(job_id.to_string(), job);
}

#[tokio::test]
async fn the_reader_projects_every_modelled_job_status_out_of_state() {
    let emulator = Emulator::new("us-east-1");
    let domain_id = domain_of(&emulator, "clinic-statuses").await;

    // The whole `JobStatus` catalog, from the modelled enum (`JOB_STATUS_VALUES`), so
    // no status is special-cased — including `SUCCEEDED`, which carries the output of a
    // run, and which this emulator's own lifecycle never reaches.
    for (index, wire_status) in JOB_STATUS_VALUES.iter().enumerate() {
        let case = format!("jobStatus {wire_status}");
        let status = JobStatus::from_wire(wire_status)
            .unwrap_or_else(|| panic!("case {case:?}: the state enum must know this wire value"));
        let job_id = format!("seeded-job-{index}");
        let (details, output) = match status {
            JobStatus::Succeeded => (
                Some("Insights generated."),
                Some("s3://clinic-output/seeded/insights.json"),
            ),
            JobStatus::Failed => (Some(PATIENT_INSIGHTS_INFERENCE_UNAVAILABLE), None),
            _ => (None, None),
        };
        seed_job(
            &emulator,
            &case,
            &domain_id,
            &job_id,
            status,
            details,
            output,
        );

        let fetched = get_job(&emulator, &case, &domain_id, &job_id).await;
        assert_eq!(
            string_at(&fetched, "jobStatus"),
            *wire_status,
            "case {case:?}: the reader must report the stored status, got {fetched}"
        );
        match details {
            Some(text) => assert_eq!(
                string_at(&fetched, "statusDetails"),
                text,
                "case {case:?}: the reader must report the stored status detail, got {fetched}"
            ),
            None => assert!(
                fetched.get("statusDetails").is_none(),
                "case {case:?}: a job with no status detail must carry none, got {fetched}"
            ),
        }
        match output {
            Some(uri) => assert_eq!(
                member(&case, &fetched, "insightsOutput"),
                json!({ "uri": uri }),
                "case {case:?}: the reader must report the stored output, got {fetched}"
            ),
            None => assert!(
                fetched.get("insightsOutput").is_none(),
                "case {case:?}: a job with no output must carry none, got {fetched}"
            ),
        }
    }
}

#[tokio::test]
async fn a_domain_at_the_job_ceiling_refuses_a_further_start() {
    let emulator = Emulator::new("us-east-1");
    let domain_id = domain_of(&emulator, "clinic-ceiling").await;

    // One short of the ceiling: the next start is still accepted.
    for index in 0..(MAX_PATIENT_INSIGHTS_JOBS_PER_DOMAIN - 1) {
        seed_job(
            &emulator,
            "below the ceiling",
            &domain_id,
            &format!("seeded-job-{index}"),
            JobStatus::Succeeded,
            None,
            None,
        );
    }
    let started = start_job(&emulator, "the last free slot", &domain_id, &minimal_body()).await;
    assert!(
        !string_at(&started, "jobId").is_empty(),
        "the start one short of the ceiling must be accepted, got {started}"
    );

    // At the ceiling the next start is refused, and the refusal names the count.
    let (status, refused) = emulator
        .send("POST", &start_path(&domain_id), Some(&minimal_body()))
        .await;
    assert_eq!(
        status, 400,
        "a start past the ceiling must be refused, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "a start past the ceiling got {refused}"
    );
    let detail = string_at(&refused, "Message");
    assert!(
        detail.contains(&MAX_PATIENT_INSIGHTS_JOBS_PER_DOMAIN.to_string()),
        "the refusal must name the ceiling it enforced, got {detail:?}"
    );
}
