// SPDX-License-Identifier: CC0-1.0

//! When the signature a signing job produced expires, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted job and serialization — the same
//! path a client takes. The rule under test:
//!
//! * the signature a job produced is valid for the `signatureValidityPeriod` of
//!   the profile version that job signed with, so `signatureExpiresAt` is that
//!   many calendar units on from the instant the signature exists at — the job's
//!   completion — and a job that has produced no signature yet reports no
//!   expiry;
//! * the period is SNAPSHOTTED when the job starts, so a later write to the same
//!   profile moves no expiry a signature already carries;
//! * the instant one job reports is the instant every reader of it sees: the
//!   summary a listing reports, and the value a listing's expiry window is
//!   judged against.
//!
//! The cases are a table whose rows differ along the UNIT the period is counted
//! in (every `ValidityType` value), the COUNT (including a zero-length period and
//! the documented default a request naming no period takes) and whether the
//! period names a unit at all.

use std::sync::Arc;
use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_signer_wallabyservice::state::JOB_COMPLETION_SPAN;
use service_com_amazonaws_signer_wallabyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every case here calls through.
const REGION: &str = "us-west-2";

/// A platform this service publishes that signs through the S3 code-signing
/// workflow `StartSigningJob` serves.
const PLATFORM_ID: &str = "AWSLambda-SHA384-ECDSA";

/// An ACM certificate ARN for `signingMaterial`, carried by every put below so a
/// profile can be created on a platform that requires the certificate its
/// signatures are issued under.
const CERTIFICATE_ARN: &str = "arn:aws:acm:us-west-2:000000000000:certificate/abc";

/// `ValidityType`'s values, the units a signature validity period can be counted
/// in (`smithy_query(query="shape", shape="ValidityType")`), spelled out here so
/// a wrong unit in the handler fails this suite.
const UNIT_DAYS: &str = "DAYS";
const UNIT_MONTHS: &str = "MONTHS";
const UNIT_YEARS: &str = "YEARS";

/// How many months make a year, for the rule this suite states independently of
/// the handler.
const MONTHS_PER_YEAR: u32 = 12;

/// The validity period a profile created without one takes:
/// `PutSigningProfileRequest$signatureValidityPeriod` is documented "The default
/// validity period override for any signature generated using this signing
/// profile. If unspecified, the default is 135 months."
/// (`smithy_query(query="doc", shape="PutSigningProfileRequest",
/// member="signatureValidityPeriod")`).
const DEFAULT_VALIDITY_MONTHS: u32 = 135;

/// How long after the completion span a case waits before reading a job, so the
/// completion the clock owes it is due however slow the host is.
const SETTLE_SLACK: Duration = Duration::from_millis(250);

/// One emulator: this service and Amazon S3 over one shared engine — a signing
/// job of this workflow reads the object it signs out of S3 and writes the signed
/// artifact back there, so a job that signs needs both front doors.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    super::code_signing_objects::mount_object_storage(&mut router, &engine);
    router
}

/// The source bucket a job here signs out of, the key in it, and the bucket the
/// signed artifact is written to.
const SOURCE_BUCKET: &str = "src";
const SOURCE_KEY: &str = "code.zip";
const DESTINATION_BUCKET: &str = "dst";

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn str_member(&self, member: &str) -> String {
        self.body[member].as_str().unwrap_or_default().to_string()
    }

    /// The epoch-seconds instant this answer reports for `member`, or `None` when
    /// the member is absent.
    fn instant(&self, member: &str) -> Option<f64> {
        self.body[member].as_f64()
    }
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("signer.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/signer/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Create an ACTIVE profile named `name`, carrying `period` as its
/// `signatureValidityPeriod` (`None` for a request that names none).
async fn put_profile(router: &MultiServiceRouter, name: &str, period: Option<Value>) {
    let mut body = json!({
        "platformId": PLATFORM_ID,
        "signingMaterial": { "certificateArn": CERTIFICATE_ARN },
    });
    if let Some(period) = period {
        body["signatureValidityPeriod"] = period;
    }
    let answer = call(
        router,
        "PUT",
        &format!("/signing-profiles/{name}"),
        Some(body),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "seeding profile {name}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
}

/// Start a signing job against `profile_name`, answering its job id.
///
/// The source object is seeded first, so the job signs something and reaches the
/// terminal status whose expiry these cases are about.
async fn start_job(router: &MultiServiceRouter, profile_name: &str, token: &str) -> String {
    let version = super::code_signing_objects::seed_signing_objects(
        router,
        REGION,
        SOURCE_BUCKET,
        SOURCE_KEY,
        DESTINATION_BUCKET,
    )
    .await;
    let answer = call(
        router,
        "POST",
        "/signing-jobs",
        Some(json!({
            "source": { "s3": {
                "bucketName": SOURCE_BUCKET,
                "key": SOURCE_KEY,
                "version": version,
            } },
            "destination": { "s3": { "bucketName": DESTINATION_BUCKET, "prefix": "signed/" } },
            "profileName": profile_name,
            "clientRequestToken": token,
        })),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "starting a job for {profile_name}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.str_member("jobId")
}

async fn describe_job(router: &MultiServiceRouter, job_id: &str) -> Answer {
    call(router, "GET", &format!("/signing-jobs/{job_id}"), None).await
}

/// `ListSigningJobs` with the raw query string `query`.
async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/signing-jobs{query}"), None).await
}

/// The job ids a listing reported, in order.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["jobs"]
        .as_array()
        .unwrap_or_else(|| panic!("ListSigningJobs must report a jobs list: {:?}", answer.body))
        .iter()
        .map(|job| job["jobId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// An instant as a `@httpQuery`-bound timestamp travels: the ISO 8601
/// (`date-time`) rendering such a binding carries by default.
fn instant_param(epoch_secs: i64) -> String {
    DateTime::from_secs(epoch_secs)
        .fmt(Format::DateTime)
        .expect("an epoch-based instant renders as date-time")
}

/// The whole epoch second a reported instant falls in.
///
/// The rule is about which calendar instant the expiry names, and a job's
/// completion carries a sub-second part that every unit below moves along
/// unchanged, so the two sides are compared at whole-second resolution.
fn whole_second(instant: f64) -> i64 {
    instant.floor() as i64
}

/// The instant `count` of `unit` counts to from `signed_at_secs`, as whole epoch
/// seconds.
///
/// The rule written out independently of the handler: a validity period is a
/// count of a CALENDAR unit, so the expiry is that many calendar units on from
/// the instant the signature was made — which is not a fixed span of seconds for
/// a unit whose length varies.
fn counted_to(signed_at_secs: i64, count: u32, unit: &str) -> i64 {
    let signed_at = ::chrono::DateTime::from_timestamp(signed_at_secs, 0)
        .expect("an epoch-seconds instant is a calendar instant");
    let reached = match unit {
        UNIT_DAYS => signed_at.checked_add_days(::chrono::Days::new(count.into())),
        UNIT_MONTHS => signed_at.checked_add_months(::chrono::Months::new(count)),
        UNIT_YEARS => signed_at.checked_add_months(::chrono::Months::new(count * MONTHS_PER_YEAR)),
        other => panic!("{other:?} is not one of ValidityType's values"),
    }
    .expect("a period this suite names reaches a calendar instant");
    reached.timestamp()
}

/// One row of the period table: the profile the job signs with carries `period`
/// on the wire, and the signature it produces expires `count` of `unit` after it
/// was made — or at no instant at all, for a period that names no unit.
struct Row {
    case: &'static str,
    profile: &'static str,
    /// What the `PutSigningProfile` body carries as `signatureValidityPeriod`,
    /// or `None` for a request that names none.
    period: Option<Value>,
    /// The count and unit the expiry must be reached by, or `None` when the
    /// period fixes no expiry.
    expected: Option<(u32, &'static str)>,
}

/// The rows: every `ValidityType` unit, a zero-length period, the documented
/// default a request naming no period takes, and a period naming no unit.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no period at all — the documented 135-month default",
            profile: "Expiry_Default",
            period: None,
            expected: Some((DEFAULT_VALIDITY_MONTHS, UNIT_MONTHS)),
        },
        Row {
            case: "a period counted in days",
            profile: "Expiry_Days",
            period: Some(json!({ "value": 30, "type": UNIT_DAYS })),
            expected: Some((30, UNIT_DAYS)),
        },
        Row {
            case: "a single-month period",
            profile: "Expiry_One_Month",
            period: Some(json!({ "value": 1, "type": UNIT_MONTHS })),
            expected: Some((1, UNIT_MONTHS)),
        },
        Row {
            case: "a period counted in years",
            profile: "Expiry_Years",
            period: Some(json!({ "value": 5, "type": UNIT_YEARS })),
            expected: Some((5, UNIT_YEARS)),
        },
        Row {
            case: "a zero-length period, which expires the signature as it is made",
            profile: "Expiry_Zero",
            period: Some(json!({ "value": 0, "type": UNIT_DAYS })),
            expected: Some((0, UNIT_DAYS)),
        },
        Row {
            case: "a period naming no unit, which counts no span",
            profile: "Expiry_No_Unit",
            period: Some(json!({ "value": 7 })),
            expected: None,
        },
    ]
}

/// A job's `signatureExpiresAt` is its profile's validity period counted off
/// from the instant the signature was made.
#[tokio::test]
async fn a_signature_expires_its_profiles_validity_period_after_it_was_made() {
    let router = emulator();
    let rows = rows();
    let mut started = Vec::with_capacity(rows.len());
    for row in &rows {
        put_profile(&router, row.profile, row.period.clone()).await;
        started.push(start_job(&router, row.profile, &format!("tok-{}", row.profile)).await);
    }

    tokio::time::sleep(JOB_COMPLETION_SPAN + SETTLE_SLACK).await;

    for (row, job_id) in rows.iter().zip(&started) {
        let described = describe_job(&router, job_id).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected the job to be readable, got {} {:?}",
            row.case, described.status, described.body
        );
        let completed_at = described.instant("completedAt").unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the completed job to report completedAt, got {:?}",
                row.case, described.body
            )
        });
        let reported = described.instant("signatureExpiresAt");
        match row.expected {
            Some((count, unit)) => {
                let reported = reported.unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected signatureExpiresAt to be reported, got {:?}",
                        row.case, described.body
                    )
                });
                let want = counted_to(whole_second(completed_at), count, unit);
                assert_eq!(
                    whole_second(reported),
                    want,
                    "case {:?}: expected the signature to expire {count} {unit} after it was \
                     made ({want}), got {reported}",
                    row.case
                );
            }
            None => assert!(
                reported.is_none(),
                "case {:?}: expected no signatureExpiresAt for a period that counts no span, \
                 got {reported:?}",
                row.case
            ),
        }
    }
}

/// A job that has produced no signature yet reports no expiry; the completion
/// that produces the signature is what fixes one, and every later read reports
/// that same instant.
#[tokio::test]
async fn a_running_job_reports_no_expiry_and_a_completed_one_reports_a_stable_instant() {
    let router = emulator();
    put_profile(
        &router,
        "Expiry_Running",
        Some(json!({ "value": 2, "type": UNIT_MONTHS })),
    )
    .await;
    let job_id = start_job(&router, "Expiry_Running", "tok-expiry-running").await;

    let running = describe_job(&router, &job_id).await;
    assert_eq!(
        running.body["status"].as_str(),
        Some("InProgress"),
        "case running: expected the job to still be running, got {:?}",
        running.body
    );
    assert!(
        running.instant("signatureExpiresAt").is_none(),
        "case running: expected no signature expiry before the signature exists, got {:?}",
        running.body
    );

    tokio::time::sleep(JOB_COMPLETION_SPAN + SETTLE_SLACK).await;

    let first = describe_job(&router, &job_id).await;
    let completed_at = first
        .instant("completedAt")
        .unwrap_or_else(|| panic!("case completed: expected completedAt, got {:?}", first.body));
    let expiry = first.instant("signatureExpiresAt").unwrap_or_else(|| {
        panic!(
            "case completed: expected signatureExpiresAt once the signature exists, got {:?}",
            first.body
        )
    });
    assert_eq!(
        whole_second(expiry),
        counted_to(whole_second(completed_at), 2, UNIT_MONTHS),
        "case completed: expected two months on from the completion, got {expiry}"
    );

    // Reading the job again, and reading it through the listing, reports the one
    // instant: nothing recomputes it against the clock of the asking.
    let again = describe_job(&router, &job_id).await;
    assert_eq!(
        again.instant("signatureExpiresAt"),
        Some(expiry),
        "case reread: expected the same expiry on a later read, got {:?}",
        again.body
    );
    let listed = list(&router, "").await;
    let summary = listed.body["jobs"][0].clone();
    assert_eq!(
        summary["signatureExpiresAt"].as_f64(),
        Some(expiry),
        "case summary: expected the listed job to report the expiry its reader reports, got \
         {summary:?}"
    );
}

/// The validity period is snapshotted when the job starts: a job's expiry is
/// counted in the period of the profile version it signed with, so a job started
/// from another profile stating another period carries that period's expiry and
/// neither job's expiry moves the other's.
#[tokio::test]
async fn the_period_a_signature_expires_under_is_the_one_its_job_started_with() {
    let router = emulator();
    put_profile(
        &router,
        "Expiry_Snapshot_Days",
        Some(json!({ "value": 10, "type": UNIT_DAYS })),
    )
    .await;
    let first_job = start_job(&router, "Expiry_Snapshot_Days", "tok-expiry-snapshot-1").await;
    tokio::time::sleep(JOB_COMPLETION_SPAN + SETTLE_SLACK).await;

    let before = describe_job(&router, &first_job).await;
    let first_completed = before
        .instant("completedAt")
        .unwrap_or_else(|| panic!("case first: expected completedAt, got {:?}", before.body));
    let first_expiry = before.instant("signatureExpiresAt").unwrap_or_else(|| {
        panic!(
            "case first: expected signatureExpiresAt, got {:?}",
            before.body
        )
    });
    assert_eq!(
        whole_second(first_expiry),
        counted_to(whole_second(first_completed), 10, UNIT_DAYS),
        "case first: expected ten days on from the completion, got {first_expiry}"
    );

    // Another profile, stating a different period.
    put_profile(
        &router,
        "Expiry_Snapshot_Years",
        Some(json!({ "value": 3, "type": UNIT_YEARS })),
    )
    .await;
    let reread = describe_job(&router, &first_job).await;
    assert_eq!(
        reread.instant("signatureExpiresAt"),
        Some(first_expiry),
        "case another profile: expected the existing signature to keep its expiry, got {:?}",
        reread.body
    );

    let second_job = start_job(&router, "Expiry_Snapshot_Years", "tok-expiry-snapshot-2").await;
    tokio::time::sleep(JOB_COMPLETION_SPAN + SETTLE_SLACK).await;
    let after = describe_job(&router, &second_job).await;
    let second_completed = after
        .instant("completedAt")
        .unwrap_or_else(|| panic!("case second: expected completedAt, got {:?}", after.body));
    let second_expiry = after.instant("signatureExpiresAt").unwrap_or_else(|| {
        panic!(
            "case second: expected signatureExpiresAt, got {:?}",
            after.body
        )
    });
    assert_eq!(
        whole_second(second_expiry),
        counted_to(whole_second(second_completed), 3, UNIT_YEARS),
        "case second: expected three years on from the completion, got {second_expiry}"
    );
    assert_eq!(
        describe_job(&router, &first_job)
            .await
            .instant("signatureExpiresAt"),
        Some(first_expiry),
        "case first after the second job: expected its own expiry to stand"
    );
}

/// The expiry a job reports is the value a listing's expiry window is judged
/// against: the bounds partition at the instant they name.
#[tokio::test]
async fn an_expiry_window_is_judged_against_the_instant_the_job_reports() {
    let router = emulator();
    put_profile(
        &router,
        "Expiry_Window",
        Some(json!({ "value": 1, "type": UNIT_YEARS })),
    )
    .await;
    let job_id = start_job(&router, "Expiry_Window", "tok-expiry-window").await;
    tokio::time::sleep(JOB_COMPLETION_SPAN + SETTLE_SLACK).await;

    let described = describe_job(&router, &job_id).await;
    let expiry = whole_second(described.instant("signatureExpiresAt").unwrap_or_else(|| {
        panic!(
            "expected the completed job to report signatureExpiresAt, got {:?}",
            described.body
        )
    }));

    // (case, the query, whether the job is one the window enumerates)
    let cases: Vec<(&str, String, bool)> = vec![
        (
            "a lower bound at the whole second the expiry falls in",
            format!("?signatureExpiresAfter={}", instant_param(expiry)),
            true,
        ),
        (
            "a lower bound a day past the expiry",
            format!("?signatureExpiresAfter={}", instant_param(expiry + 86_400)),
            false,
        ),
        (
            "an upper bound at the whole second the expiry falls in",
            format!("?signatureExpiresBefore={}", instant_param(expiry)),
            false,
        ),
        (
            "an upper bound a day past the expiry",
            format!("?signatureExpiresBefore={}", instant_param(expiry + 86_400)),
            true,
        ),
        (
            "a window around the expiry",
            format!(
                "?signatureExpiresAfter={}&signatureExpiresBefore={}",
                instant_param(expiry - 86_400),
                instant_param(expiry + 86_400)
            ),
            true,
        ),
    ];

    for (case, query, enumerated) in cases {
        let answer = list(&router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 for {query}, got {} {:?}",
            answer.status, answer.body
        );
        let reported = listed_ids(&answer);
        let want: Vec<String> = if enumerated {
            vec![job_id.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            reported, want,
            "case {case:?}: expected {want:?} for {query}, got {reported:?}"
        );
    }
}
