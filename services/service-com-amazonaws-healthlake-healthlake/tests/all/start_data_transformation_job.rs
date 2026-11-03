// SPDX-License-Identifier: CC0-1.0

//! What `StartDataTransformationJob` mints, what `DescribeDataTransformationJob`
//! reads back, and what the start refuses.
//!
//! The rules under test, each over a SET of inputs rather than one case:
//!
//! 1. a start MINTS one data transformation job record, persisting every member
//!    the request carried, and answers the minted `JobId` with the `SUBMITTED`
//!    status the record holds — the ONE record
//!    `DescribeDataTransformationJob` reports;
//! 2. the profile the job runs against is resolved and STORED at start: its
//!    `ProfileId`, its name, and the version the job runs — the profile's
//!    latest published version, or its DRAFT (0) while nothing is published
//!    from it;
//! 3. the clock then settles the started job to the terminal status the work
//!    this emulator could actually do warrants: the modelled `FAILED` with the
//!    service's own `Message` saying why, `EndTime` stamped once, and no
//!    `JobProgressReport` at all, because no file was scanned;
//! 4. a `ProfileId` no profile record holds is `ResourceNotFoundException` in
//!    this service's fixed not-found sentence, and a refused start mints
//!    nothing;
//! 5. one `ClientToken` starts one job: a call carrying a token already seen
//!    is IGNORED and answered out of the job that token started — whatever
//!    status that job has reached by then, and whatever else the request says.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`](smithy_server_runtime::MultiServiceRouter)
//! by way of the harness in `fhir_datastore_lifecycle`, so routing,
//! deserialization, the modelled constraints and the error envelope are all
//! exercised, not bypassed.
//!
//! Every modelled CLIENT error is asserted at HTTP 400 whatever status its
//! shape's `@httpError` names, for the reason `tests/all/error_envelopes.rs`
//! pins: awsJson1_0 binds nothing to the transport, so this service reports an
//! error's FAULT.

use serde_json::{json, Value};
use service_com_amazonaws_healthlake_healthlake::state::{
    ComAmazonawsHealthlakeHealthlakeState, ProfileVersionRecord, profile_not_found_message,
    TRANSFORMATION_NOT_PERFORMED_MESSAGE,
};

use crate::fhir_datastore_lifecycle::{error_code, error_message, string_at, HealthLake,
    PAST_TRANSITION};

/// The `DataAccessRoleArn` every start below carries, satisfying the modelled
/// `DataTransformationIamRoleArn` pattern
/// (`^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$`).
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthLakeTransform";

/// A well-formed `ProfileId` no profile is held for: 32 lowercase hex
/// characters, exactly what the modelled `ProfileIdString` pattern
/// (`^[a-f0-9]{32}$`) requires, so the refusal is about the profile not
/// existing rather than about the identifier's shape.
const UNHELD_PROFILE_ID: &str = "ffffffffffffffffffffffffffffffff";

/// A second well-formed `ProfileId` no profile is held for.
const OTHER_UNHELD_PROFILE_ID: &str = "00000000000000000000000000000000";

/// The version that names a profile's DRAFT, as `CreateDataTransformationProfile`
/// documents it ("The starting version is always 0, which indicates the profile
/// is in DRAFT state").
const DRAFT_VERSION: i64 = 0;

/// Create one data transformation profile and return `(ProfileId, ProfileName)`.
///
/// `CreateDataTransformationProfile` answers 200 — awsJson1_0 binds nothing to the
/// transport, so the operation's `@http(code: 201)` is inert — so the
/// call is dispatched rather than taken through the harness's 200-only helper.
///
/// `pub(crate)` so the sibling suites of this test target get a profile the same
/// way rather than each growing its own copy:
/// `tests/all/idempotency_replay_precedes_state_gates.rs` starts a job against
/// one and then deletes it.
pub(crate) async fn profile(service: &HealthLake, name: &str) -> (String, String) {
    let (status, created) = service
        .call(
            "CreateDataTransformationProfile",
            &json!({
                "ProfileName": name,
                "SourceFormat": "CCDA",
                "Source": { "ProfileMapping": { "ProfileMapping": {
                    "sections/allergies.map": "AllergyIntolerance <- allergy",
                } } },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "creating the profile {name:?} must answer the protocol's 200, got {status}: {created}"
    );
    (string_at(&created, "ProfileId"), name.to_string())
}

/// Publish `versions` of the profile `profile_id` names, straight into the
/// store the handler reads.
///
/// `PublishDataTransformationProfile` — the operation that would snapshot a
/// DRAFT into a version of its own — is still a scaffold stub with its own
/// issue, so a profile that HAS a published version is seeded here, through the
/// shared engine the harness exposes, exactly as
/// `tests/all/data_transformation_job.rs` seeds job records.
fn publish_versions(service: &HealthLake, profile_id: &str, versions: &[i32]) {
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let mut store = store.lock();
    let profile = store
        .profiles
        .get_mut(profile_id)
        .unwrap_or_else(|| panic!("the created profile {profile_id:?} must be held"));
    for version in versions {
        let snapshot = ProfileVersionRecord {
            version: *version,
            mapping: profile.draft.mapping.clone(),
            change_description: None,
            last_updated_at: profile.created_at,
        };
        profile.published.insert(*version, snapshot);
    }
}

/// How many data transformation job records the store holds.
fn held_jobs(service: &HealthLake) -> usize {
    let store = service
        .engine
        .state::<ComAmazonawsHealthlakeHealthlakeState>();
    let held = store.lock().transformation_jobs.len();
    held
}

/// The `TransformationJobProperties` `DescribeDataTransformationJob` reports for
/// `job_id`.
async fn describe(service: &HealthLake, job_id: &str) -> Value {
    service
        .ok("DescribeDataTransformationJob", &json!({ "JobId": job_id }))
        .await["TransformationJobProperties"]
        .clone()
}

/// One `InputDataConfig` the modelled shape admits: `S3Uri` matches
/// `^s3://[a-z0-9][a-z0-9.\-]{1,61}[a-z0-9](/.+)?$` (len 8..2048) and the
/// optional `SourceFormat` comes from the modelled `SourceFormat` enum.
fn input_data_config(s3_uri: &str, source_format: Option<&str>) -> Value {
    let mut config = json!({ "S3Uri": s3_uri });
    if let Some(format) = source_format {
        config["SourceFormat"] = json!(format);
    }
    config
}

/// One `OutputDataConfig` the modelled shape admits, from the member bounds
/// `smithy_query(query="shape", shape="DataTransformationS3Configuration")`
/// states: an `S3Uri` of the same pattern and a required `KmsKeyId`.
fn output_data_config(s3_uri: &str, kms_key_id: &str) -> Value {
    json!({ "S3Configuration": { "S3Uri": s3_uri, "KmsKeyId": kms_key_id } })
}

/// One row of the round-trip table: what a start carries, beside the case name.
struct Row {
    case: &'static str,
    client_token: &'static str,
    job_name: Option<String>,
    input: Value,
    output: Value,
    drift_detection_enabled: Option<bool>,
    provenance_enabled: Option<bool>,
}

impl Row {
    fn new(case: &'static str, client_token: &'static str) -> Self {
        Self {
            case,
            client_token,
            job_name: None,
            input: input_data_config("s3://transform-in-bucket", None),
            output: output_data_config(
                "s3://transform-out-bucket",
                "1234abcd-12ab-34cd-56ef-1234567890ab",
            ),
            drift_detection_enabled: None,
            provenance_enabled: None,
        }
    }

    fn named(mut self, name: &str) -> Self {
        self.job_name = Some(name.to_string());
        self
    }

    fn input(mut self, s3_uri: &str, source_format: Option<&str>) -> Self {
        self.input = input_data_config(s3_uri, source_format);
        self
    }

    fn output(mut self, s3_uri: &str, kms_key_id: &str) -> Self {
        self.output = output_data_config(s3_uri, kms_key_id);
        self
    }

    fn flags(mut self, drift: Option<bool>, provenance: Option<bool>) -> Self {
        self.drift_detection_enabled = drift;
        self.provenance_enabled = provenance;
        self
    }

    /// This row as a `StartDataTransformationJob` request body for `profile_id`.
    fn request(&self, profile_id: &str) -> Value {
        let mut request = json!({
            "ClientToken": self.client_token,
            "DataAccessRoleArn": DATA_ACCESS_ROLE_ARN,
            "InputDataConfig": self.input,
            "OutputDataConfig": self.output,
            "ProfileId": profile_id,
        });
        if let Some(name) = &self.job_name {
            request["JobName"] = json!(name);
        }
        if let Some(drift) = self.drift_detection_enabled {
            request["DriftDetectionEnabled"] = json!(drift);
        }
        if let Some(provenance) = self.provenance_enabled {
            request["ProvenanceEnabled"] = json!(provenance);
        }
        request
    }
}

/// The rows the round-trip table spans: the rule is that every member a start
/// carries is persisted and reported back, so the rows differ along which
/// optional members are present and along the FORM of the values the modelled
/// shapes admit — both `SourceFormat` values and none, a bucket-root `S3Uri`
/// against a prefixed and a nested one, the three `KmsKeyId` forms the modelled
/// `EncryptionKeyID` pattern spells (bare key id, `alias/...`, full ARN), an
/// ASCII and a unicode `JobName`, a name at the modelled 64-character maximum,
/// and each boolean flag set, cleared and omitted.
fn round_trip_rows() -> Vec<Row> {
    vec![
        Row::new("only the required members", "token-minimal"),
        Row::new("named, CCDA source, prefixed URIs, key ARN", "token-ccda")
            .named("nightly-ccda")
            .input("s3://transform-in-bucket/incoming", Some("CCDA"))
            .output(
                "s3://transform-out-bucket/r4",
                "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
            ),
        Row::new("CSV source, nested URIs, key alias, both flags set", "token-csv")
            .named("hourly csv")
            .input("s3://transform-in.bucket/csv/batch-07", Some("CSV"))
            .output("s3://transform-out.bucket/tenant/42/out", "alias/healthlake/transform")
            .flags(Some(true), Some(true)),
        Row::new("unicode JobName, both flags explicitly disabled", "token-unicode")
            .named("клиника записи 42")
            .flags(Some(false), Some(false)),
        Row::new("JobName at the modelled 64-character maximum", "token-max-name")
            .named(&"n".repeat(64))
            .flags(Some(true), None),
    ]
}

/// `JobId` values are 32 lowercase hex characters, which satisfies the modelled
/// `DataTransformationJobId` length bound (1..32) and its alphabet
/// (`^[a-zA-Z0-9-]+$`).
fn assert_well_formed_job_id(case: &str, job_id: &str) {
    assert_eq!(
        job_id.len(),
        32,
        "case {case:?}: JobId must be 32 characters, got {job_id:?}"
    );
    assert!(
        job_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "case {case:?}: JobId must be lowercase hex, got {job_id:?}"
    );
}

#[tokio::test]
async fn a_start_mints_one_job_and_describe_reports_every_member_back() {
    let service = HealthLake::new();
    let (profile_id, profile_name) = profile(&service, "ccda-to-r4").await;
    let mut minted: Vec<String> = Vec::new();

    for row in round_trip_rows() {
        let case = row.case;
        let request = row.request(&profile_id);
        let started = service.ok("StartDataTransformationJob", &request).await;

        let job_id = string_at(&started, "JobId");
        assert_well_formed_job_id(case, &job_id);
        assert_eq!(
            started["JobStatus"],
            json!("SUBMITTED"),
            "case {case:?}: a started job must be answered SUBMITTED, got {started}"
        );
        assert!(
            !minted.contains(&job_id),
            "case {case:?}: every start must mint its own JobId, {job_id:?} was minted before"
        );
        minted.push(job_id.clone());
        assert_eq!(
            held_jobs(&service),
            minted.len(),
            "case {case:?}: each start must mint exactly one job record"
        );

        // The record is read back through the operation that reports ONE job.
        let described = describe(&service, &job_id).await;
        for (member, expected) in [
            ("JobId", json!(job_id)),
            ("JobStatus", json!("SUBMITTED")),
            ("InputDataConfig", row.input.clone()),
            ("OutputDataConfig", row.output.clone()),
            ("DataAccessRoleArn", json!(DATA_ACCESS_ROLE_ARN)),
            ("ProfileId", json!(profile_id)),
            ("ProfileName", json!(profile_name)),
            ("ProfileVersion", json!(DRAFT_VERSION)),
        ] {
            assert_eq!(
                described[member], expected,
                "case {case:?}: the described job must report {member} as {expected}, got {described}"
            );
        }
        assert_eq!(
            described.get("JobName").cloned(),
            row.job_name.clone().map(Value::from),
            "case {case:?}: the described job must report the JobName the start carried, got {described}"
        );
        assert_eq!(
            described.get("DriftDetectionEnabled").and_then(Value::as_bool),
            row.drift_detection_enabled,
            "case {case:?}: DriftDetectionEnabled must be reported as the start set it, got {described}"
        );
        // `ProvenanceEnabled` is modelled required with a default of true, so a
        // request that names none still runs — and reports — the default.
        assert_eq!(
            described.get("ProvenanceEnabled").and_then(Value::as_bool),
            Some(row.provenance_enabled.unwrap_or(true)),
            "case {case:?}: ProvenanceEnabled must be reported, defaulting to true, got {described}"
        );
        assert!(
            described["SubmitTime"].is_number(),
            "case {case:?}: the described job must report a SubmitTime, got {described}"
        );
        for absent in ["EndTime", "Message", "JobProgressReport"] {
            assert!(
                described.get(absent).is_none(),
                "case {case:?}: a job that has not ended must report no {absent}, got {described}"
            );
        }
    }
}

#[tokio::test]
async fn the_version_a_job_runs_is_the_profile_s_latest_published_one() {
    let service = HealthLake::new();

    // The rows differ along what the profile has published when the job is
    // started: nothing (so the DRAFT, version 0, is the only version it has),
    // one published version, and several (the latest of them). Each row's
    // `ClientToken` has to satisfy the modelled `^[a-zA-Z0-9-]+$` pattern, so
    // it is spelled with `-` rather than taken from the case name.
    for (case, token, published, expected_version) in [
        (
            "nothing published, so the DRAFT",
            "token-draft",
            &[][..],
            0_i64,
        ),
        ("one published version", "token-one-version", &[7][..], 7),
        (
            "several published versions",
            "token-several-versions",
            &[3, 7, 12][..],
            12,
        ),
        (
            "the highest modelled version published",
            "token-highest-version",
            &[99][..],
            99,
        ),
    ] {
        let (profile_id, profile_name) = profile(&service, case).await;
        publish_versions(&service, &profile_id, published);

        let request = Row::new(case, token).request(&profile_id);
        let started = service.ok("StartDataTransformationJob", &request).await;
        let described = describe(&service, &string_at(&started, "JobId")).await;
        assert_eq!(
            described["ProfileVersion"],
            json!(expected_version),
            "case {case:?}: the job must run the profile's version {expected_version}, got {described}"
        );
        assert_eq!(
            described["ProfileName"],
            json!(profile_name),
            "case {case:?}: the job must report the profile's name, got {described}"
        );
    }
}

#[tokio::test]
async fn the_clock_settles_a_started_job_to_the_status_its_work_warrants() {
    let service = HealthLake::new();
    let (profile_id, _name) = profile(&service, "settling").await;

    // The rows differ along the start the job was given: the work this emulator
    // can do is the same whatever the request asked for, so every row settles
    // the same way.
    for row in round_trip_rows() {
        let case = row.case;
        let started = service
            .ok("StartDataTransformationJob", &row.request(&profile_id))
            .await;
        let job_id = string_at(&started, "JobId");

        // Before the span is up the job still reports the status it was started
        // with, so a caller describing it straight away sees the transient one.
        let pending = describe(&service, &job_id).await;
        assert_eq!(
            pending["JobStatus"],
            json!("SUBMITTED"),
            "case {case:?}: before the transition falls due the job must report SUBMITTED, got {pending}"
        );
        assert!(
            pending.get("EndTime").is_none() && pending.get("Message").is_none(),
            "case {case:?}: a running job must report neither EndTime nor Message, got {pending}"
        );

        tokio::time::sleep(PAST_TRANSITION).await;

        let settled = describe(&service, &job_id).await;
        assert_eq!(
            settled["JobStatus"],
            json!("FAILED"),
            "case {case:?}: once due the job must report FAILED, got {settled}"
        );
        assert_eq!(
            settled.get("Message").and_then(Value::as_str),
            Some(TRANSFORMATION_NOT_PERFORMED_MESSAGE),
            "case {case:?}: the ended job must state why it failed, got {settled}"
        );
        assert!(
            settled["EndTime"].is_number(),
            "case {case:?}: a job that reached a terminal status must report an EndTime, got {settled}"
        );
        assert!(
            settled.get("JobProgressReport").is_none(),
            "case {case:?}: a job nothing ran must report no JobProgressReport, got {settled}"
        );

        // `EndTime` and the message are stored when the job ends, not
        // re-derived per read, so a second read reports the same record.
        let again = describe(&service, &job_id).await;
        assert_eq!(
            again, settled,
            "case {case:?}: two reads of one ended job must report the same record"
        );
    }
}

#[tokio::test]
async fn a_profile_id_no_record_holds_is_refused_and_mints_nothing() {
    let service = HealthLake::new();
    let (held_profile, _name) = profile(&service, "held").await;
    // A data store's identifier is 32 lowercase hex characters too; it is not a
    // transformation profile, so it must not resolve as one.
    let (datastore_id, _arn) = service
        .create(&json!({ "DatastoreTypeVersion": "R4", "DatastoreName": "not a profile" }))
        .await;

    for (case, profile_id) in [
        ("a well-formed id no profile was minted under", UNHELD_PROFILE_ID),
        ("another well-formed id nothing holds", OTHER_UNHELD_PROFILE_ID),
        ("the id of a FHIR data store", datastore_id.as_str()),
    ] {
        let request = Row::new(case, "token-refused").request(profile_id);
        let (status, payload) = service.call("StartDataTransformationJob", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the start must be refused, got {status}: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {case:?}: the refusal must be ResourceNotFoundException, got {payload}"
        );
        let expected = profile_not_found_message(profile_id);
        assert_eq!(
            error_message(&payload),
            expected,
            "case {case:?}: the refusal must name the profile it could not find, got {payload}"
        );
        assert!(
            payload.to_string().contains(profile_id),
            "case {case:?}: the refusal must name the profile the request asked for, \
             got {payload}"
        );
        assert_eq!(
            held_jobs(&service),
            0,
            "case {case:?}: a refused start must mint no job"
        );
    }

    // The same request against the profile that IS held starts a job, so the
    // refusals above were about the profile and nothing else.
    let started = service
        .ok(
            "StartDataTransformationJob",
            &Row::new("held profile", "token-held").request(&held_profile),
        )
        .await;
    assert_well_formed_job_id("held profile", &string_at(&started, "JobId"));
}

#[tokio::test]
async fn one_client_token_starts_one_data_transformation_job() {
    let service = HealthLake::new();
    let (profile_id, _name) = profile(&service, "idempotent").await;
    let request = Row::new("idempotent", "token-one")
        .named("hourly")
        .input("s3://transform-in-bucket/hourly", Some("CSV"))
        .flags(Some(true), Some(false))
        .request(&profile_id);

    let first = service.ok("StartDataTransformationJob", &request).await;
    let job_id = string_at(&first, "JobId");

    // The SAME request with the SAME token resolves to the job it started.
    let repeat = service.ok("StartDataTransformationJob", &request).await;
    assert_eq!(
        repeat, first,
        "a repeat of one request under one ClientToken must answer the job it started"
    );
    assert_eq!(
        held_jobs(&service),
        1,
        "one ClientToken must start exactly one job"
    );

    // A token already seen makes the service ignore the call it carries and
    // return no error, as `StartDataTransformationJobRequest$ClientToken`
    // documents, so each row below is answered with the job the token started
    // and starts nothing. The rows differ along which member differs from the
    // request that minted it.
    let (other_profile, _name) = profile(&service, "other").await;
    for (case, member, value) in [
        ("a different JobName", "JobName", json!("nightly")),
        (
            "a different DataAccessRoleArn",
            "DataAccessRoleArn",
            json!("arn:aws:iam::111122223333:role/OtherTransform"),
        ),
        (
            "a different InputDataConfig",
            "InputDataConfig",
            input_data_config("s3://transform-in-bucket/elsewhere", Some("CSV")),
        ),
        (
            "a different InputDataConfig SourceFormat",
            "InputDataConfig",
            input_data_config("s3://transform-in-bucket/hourly", Some("CCDA")),
        ),
        (
            "a different OutputDataConfig",
            "OutputDataConfig",
            output_data_config("s3://transform-out-bucket/elsewhere", "alias/other"),
        ),
        ("a different ProfileId", "ProfileId", json!(other_profile)),
        (
            "a different DriftDetectionEnabled",
            "DriftDetectionEnabled",
            json!(false),
        ),
        (
            "a different ProvenanceEnabled",
            "ProvenanceEnabled",
            json!(true),
        ),
    ] {
        let mut different = request.clone();
        different[member] = value;
        let (status, payload) = service
            .call("StartDataTransformationJob", &different)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a call under a token already seen must not be refused, got {status}: {payload}"
        );
        assert_eq!(
            payload["JobId"],
            json!(job_id),
            "case {case:?}: the call must be answered with the job the token started, got {payload}"
        );
        // What the ignored call carried reached no record: the job still
        // reports the members the first request named.
        let described = describe(&service, &job_id).await;
        assert_eq!(
            described.get("JobName").and_then(Value::as_str),
            Some("hourly"),
            "case {case:?}: an ignored call must change nothing on the job, got {described}"
        );
        assert_eq!(
            described["ProfileId"],
            json!(profile_id),
            "case {case:?}: an ignored call must change nothing on the job, got {described}"
        );
    }
    assert_eq!(
        held_jobs(&service),
        1,
        "every ignored call above must have started no further job"
    );

    // A DIFFERENT token starts a second job of the same request.
    let mut second_token = request.clone();
    second_token["ClientToken"] = json!("token-two");
    let second = service
        .ok("StartDataTransformationJob", &second_token)
        .await;
    assert_ne!(
        string_at(&second, "JobId"),
        job_id,
        "a distinct ClientToken must start its own job, got {second}"
    );

    // Once the clock has settled the first job, its token still resolves to it —
    // answered out of the STORED record, so the status is the one the job has
    // reached rather than the one it started in.
    tokio::time::sleep(PAST_TRANSITION).await;
    let settled_repeat = service.ok("StartDataTransformationJob", &request).await;
    assert_eq!(
        settled_repeat["JobId"],
        json!(job_id),
        "the token must still resolve to the job it started, got {settled_repeat}"
    );
    assert_eq!(
        settled_repeat["JobStatus"],
        json!("FAILED"),
        "a repeat must report the status the stored job has reached, got {settled_repeat}"
    );
    assert_eq!(
        held_jobs(&service),
        2,
        "the repeats above must have started no further job"
    );
}
