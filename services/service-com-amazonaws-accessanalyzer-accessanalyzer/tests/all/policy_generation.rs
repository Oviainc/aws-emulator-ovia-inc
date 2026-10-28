// SPDX-License-Identifier: CC0-1.0

//! The policy generation job lifecycle — `StartPolicyGeneration`, `GetGeneratedPolicy`,
//! `CancelPolicyGeneration` and `ListPolicyGenerations` — driven over the wire.
//!
//! The rule under test: a call that starts asynchronous work validates its request
//! SYNCHRONOUSLY and records nothing when it refuses it, and otherwise mints a stored job
//! whose identifier the response carries; every later read, listing and cancel of that
//! identifier is answered from that ONE record against the clock — `IN_PROGRESS` until
//! the work is due, then `SUCCEEDED`, or `CANCELED` when a cancel reached the job while it
//! was still running — while a cancel of a job already in a terminal status, and any call
//! naming an identifier no start minted, are refused with the modelled
//! `ValidationException` (HTTP 400), the only client error these operations declare.
//!
//! Every case goes through the real router (the shared wire harness in
//! [`super::harness`]), so restJson1 routing, boundary constraint validation, the handler
//! and the response serialization all run as they do in the emulator binary. Nothing
//! reaches into `crate::state`: a started job is only believed when a later
//! `GetGeneratedPolicy` or `ListPolicyGenerations` reports it back.
//!
//! The tables span what the rule generalises over — how many trails a request names and
//! which of a trail's optional members it carries, whether the analysed range carries an
//! end, which principal kind and partition the ARNs name, whether a `clientToken` is
//! carried and repeated, which status the job is in when a cancel arrives, how many jobs
//! one principal has, and the page size a listing asks for. The recorded
//! missing-`cloudTrailDetails` refusal and the single-trail generation that follows it are
//! each ONE row.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::state::POLICY_GENERATION_SPAN;

use crate::harness::{
    is_timestamp, member, timestamp_at_least, wire_reason, Emulator, Reply, REGION,
};

/// The status a success that reports content carries.
const OK: u16 = 200;

/// The status a successful cancel carries: its modelled output is a declared structure
/// with no members, so there is no content to report.
const NO_CONTENT: u16 = 204;

/// The status every refusal of these four operations carries — `ValidationException` is
/// the only client error their models declare.
const BAD_REQUEST: u16 = 400;

/// The error code of that refusal.
const VALIDATION: &str = "ValidationException";

/// How long a case waits PAST [`POLICY_GENERATION_SPAN`] before reading a job back, so a
/// loaded machine cannot read a generation a moment before the clock owes it.
const SETTLE_MARGIN: Duration = Duration::from_millis(250);

/// 2026-01-01T00:00:00Z in epoch seconds: the base every analysed range in these tables
/// is expressed from.
///
/// A request carries a timestamp as epoch seconds, which is what the restJson1 codec
/// parses; a response renders the same instant as the RFC3339 string the member's
/// `date-time` format asks for. [`same_instant`] compares the two renderings, so a row
/// can assert that the range a generation reports is the range the request named.
const RANGE_BASE: f64 = 1_767_225_600.0;

/// One day of an analysed range, in the same unit.
const DAY: f64 = 86_400.0;

/// The `reason` every refusal of this service carries, as the wire spells the model's
/// `fieldValidationFailed` discriminator.
fn field_validation_failed() -> &'static str {
    wire_reason("fieldValidationFailed")
}

/// An IAM role ARN in this emulator's account, in the form `PrincipalArn`'s pattern
/// requires (`^arn:[^:]*:iam::[^:]*:(role|user)/.{1,576}$`).
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// An IAM user ARN, the other principal kind that same pattern accepts.
fn user_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:user/{name}", engine::emulator_account_id())
}

/// A CloudTrail trail ARN, in the form `CloudTrailArn`'s pattern requires
/// (`^arn:[^:]*:cloudtrail:[^:]*:[^:]*:trail/.{1,576}$`).
fn trail_arn(name: &str) -> String {
    format!(
        "arn:aws:cloudtrail:{REGION}:{}:trail/{name}",
        engine::emulator_account_id()
    )
}

/// The service role a generation reads the trail with; `RoleArn`'s pattern is the role
/// half of the principal pattern.
fn access_role() -> String {
    role_arn("AccessAnalyzerMonitorServiceRole")
}

/// A `Trail` naming only the ARN — every other member of the shape is optional.
fn trail(name: &str) -> Value {
    json!({ "cloudTrailArn": trail_arn(name) })
}

/// The request body of a start, from its parts. `trails` is the whole list, so a row can
/// carry none, one or several.
fn start_body(
    principal_arn: &str,
    trails: &[Value],
    start_time: f64,
    end_time: Option<f64>,
    client_token: Option<&str>,
) -> Value {
    let mut details = json!({
        "trails": trails,
        "accessRole": access_role(),
        "startTime": start_time,
    });
    if let Some(end_time) = end_time {
        details["endTime"] = json!(end_time);
    }
    let mut body = json!({
        "policyGenerationDetails": { "principalArn": principal_arn },
        "cloudTrailDetails": details,
    });
    if let Some(token) = client_token {
        body["clientToken"] = json!(token);
    }
    body
}

/// Start a generation that must be accepted, and answer the `jobId` it minted.
async fn start_ok(emulator: &Emulator, case: &str, body: Value) -> String {
    let reply = emulator.start_policy_generation(body).await;
    assert_eq!(
        (reply.status, reply.code()),
        (OK, ""),
        "case {case:?}: expected StartPolicyGeneration to answer {OK}, got {reply:?}"
    );
    let job_id = reply
        .member("jobId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !job_id.is_empty(),
        "case {case:?}: expected a jobId in the response, got {reply:?}"
    );
    job_id
}

/// The `jobDetails` of a `GetGeneratedPolicy` reply.
fn job_details(case: &str, reply: &Reply) -> Value {
    reply
        .member("jobDetails")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected jobDetails in {reply:?}"))
}

/// The `generatedPolicyResult` of a `GetGeneratedPolicy` reply.
fn policy_result(case: &str, reply: &Reply) -> Value {
    reply
        .member("generatedPolicyResult")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected generatedPolicyResult in {reply:?}"))
}

/// The `status` a `jobDetails` or a listing entry reports.
fn status_of(value: &Value) -> &str {
    member(value, "status").and_then(Value::as_str).unwrap_or("")
}

/// Whether two timestamp members name the SAME instant, whichever of the two renderings
/// each is in: the epoch seconds a request carries, or the RFC3339 string a response
/// reports for a `date-time` member.
fn same_instant(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (instant_secs(left), instant_secs(right)) {
        (Some(left), Some(right)) => (left - right).abs() < 1e-6,
        _ => false,
    }
}

/// One timestamp member as epoch seconds, from either rendering; `None` for a member that
/// is absent or is no instant at all.
fn instant_secs(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(seconds) => seconds.as_f64(),
        Value::String(text) => aws_smithy_types::DateTime::from_str(
            text,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .ok()
        .map(|instant| instant.as_secs_f64()),
        _ => None,
    }
}

/// The entry a listing reports for `job_id`, or `None` when it reports none.
fn listed_job(reply: &Reply, job_id: &str) -> Option<Value> {
    reply
        .member("policyGenerations")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|item| item.get("jobId").and_then(Value::as_str) == Some(job_id))
        })
        .cloned()
}

/// Every `jobId` a listing reports, in the order it reports them.
fn listed_ids(reply: &Reply) -> Vec<String> {
    reply
        .member("policyGenerations")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("jobId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the start table.
struct StartCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    principal_arn: String,
    /// The `trails` list the request carries.
    trails: Vec<Value>,
    start_time: f64,
    /// The `endTime` the request carries, `None` for the documented default ("the current
    /// time").
    end_time: Option<f64>,
}

/// The requests a start must accept: the rows differ along how many trails are named and
/// which optional members each carries, whether the range carries an end, and the
/// principal kind and partition the ARNs name.
fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase {
            case: "one trail, naming only its ARN (the recorded case)",
            principal_arn: role_arn("pg_recorded"),
            trails: vec![trail("pg-recorded-trail")],
            start_time: RANGE_BASE,
            end_time: None,
        },
        StartCase {
            case: "a trail naming the regions to analyze",
            principal_arn: role_arn("pg_regions"),
            trails: vec![json!({
                "cloudTrailArn": trail_arn("pg-regions-trail"),
                "regions": ["us-east-1", "eu-west-1"],
            })],
            start_time: RANGE_BASE + DAY,
            end_time: None,
        },
        StartCase {
            case: "a trail analyzing all regions",
            principal_arn: role_arn("pg_all_regions"),
            trails: vec![json!({
                "cloudTrailArn": trail_arn("pg-all-regions-trail"),
                "allRegions": true,
            })],
            start_time: RANGE_BASE + 2.0 * DAY,
            end_time: None,
        },
        StartCase {
            case: "three trails in one request",
            principal_arn: role_arn("pg_many_trails"),
            trails: vec![
                trail("pg-trail-a"),
                json!({
                    "cloudTrailArn": trail_arn("pg-trail-b"),
                    "regions": ["us-west-2"],
                }),
                json!({
                    "cloudTrailArn": trail_arn("pg-trail-c"),
                    "allRegions": false,
                }),
            ],
            start_time: RANGE_BASE + 3.0 * DAY,
            end_time: None,
        },
        StartCase {
            case: "an explicit end of the analysed range",
            principal_arn: role_arn("pg_end_time"),
            trails: vec![trail("pg-end-time-trail")],
            start_time: RANGE_BASE + 4.0 * DAY,
            end_time: Some(RANGE_BASE + 5.0 * DAY),
        },
        StartCase {
            case: "a zero-length range: the end is the start",
            principal_arn: role_arn("pg_same_instant"),
            trails: vec![trail("pg-same-instant-trail")],
            start_time: RANGE_BASE + 6.0 * DAY,
            end_time: Some(RANGE_BASE + 6.0 * DAY),
        },
        StartCase {
            case: "an IAM user rather than a role",
            principal_arn: user_arn("pg_user"),
            trails: vec![trail("pg-user-trail")],
            start_time: RANGE_BASE + 7.0 * DAY,
            end_time: None,
        },
        StartCase {
            case: "another partition's ARNs",
            principal_arn: format!(
                "arn:aws-us-gov:iam::{}:role/pg_partition",
                engine::emulator_account_id()
            ),
            trails: vec![json!({
                "cloudTrailArn": format!(
                    "arn:aws-us-gov:cloudtrail:us-gov-west-1:{}:trail/pg-partition-trail",
                    engine::emulator_account_id()
                ),
            })],
            start_time: RANGE_BASE + 8.0 * DAY,
            end_time: None,
        },
    ]
}

/// The `trailProperties` a generation must report back for the `trails` a row named: the
/// same ARN, and the same optional members, with none invented for a trail that carried
/// none.
fn assert_trail_properties(case: &str, trails: &[Value], reported: Option<&Value>) {
    let reported = reported
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected trailProperties, got {reported:?}"));
    assert_eq!(
        reported.len(),
        trails.len(),
        "case {case:?}: expected {} trailProperties, got {reported:?}",
        trails.len()
    );
    for (requested, properties) in trails.iter().zip(reported) {
        for name in ["cloudTrailArn", "regions", "allRegions"] {
            assert_eq!(
                member(properties, name),
                member(requested, name),
                "case {case:?}: expected the generation to report the {name} the request named, \
                 got {properties:?}"
            );
        }
    }
}

/// RULE: an accepted start mints a job that is `IN_PROGRESS` until the generation is due
/// and `SUCCEEDED` after it, reporting the principal and the trails the request named —
/// for every request form the operation accepts.
#[tokio::test]
async fn a_started_generation_runs_from_in_progress_to_succeeded() {
    let emulator = Emulator::new();
    let cases = start_cases();
    let mut started: Vec<(&StartCase, String, Value)> = Vec::new();

    for row in &cases {
        let job_id = start_ok(
            &emulator,
            row.case,
            start_body(
                &row.principal_arn,
                &row.trails,
                row.start_time,
                row.end_time,
                None,
            ),
        )
        .await;

        // Read straight after the start: the generation is not instantaneous, so the job
        // is still running and has produced no policy yet.
        let reply = emulator.get_generated_policy(&job_id, "").await;
        assert_eq!(
            (reply.status, reply.code()),
            (OK, ""),
            "case {:?}: expected GetGeneratedPolicy to answer {OK}, got {reply:?}",
            row.case
        );
        let details = job_details(row.case, &reply);
        assert_eq!(
            member(&details, "jobId").and_then(Value::as_str),
            Some(job_id.as_str()),
            "case {:?}: expected the job to report the jobId the start returned, got {details:?}",
            row.case
        );
        assert_eq!(
            status_of(&details),
            "IN_PROGRESS",
            "case {:?}: expected a generation read straight after the start to be IN_PROGRESS, \
             got {details:?}",
            row.case
        );
        assert!(
            is_timestamp(member(&details, "startedOn")),
            "case {:?}: expected startedOn to be the instant the job started, got {details:?}",
            row.case
        );
        assert_eq!(
            member(&details, "completedOn"),
            None,
            "case {:?}: a running generation has completed at no instant, got {details:?}",
            row.case
        );
        assert_eq!(
            member(&details, "jobError"),
            None,
            "case {:?}: nothing failed, so the job reports no jobError, got {details:?}",
            row.case
        );

        let result = policy_result(row.case, &reply);
        let properties = member(&result, "properties")
            .cloned()
            .unwrap_or_else(|| panic!("case {:?}: expected properties in {result:?}", row.case));
        assert_eq!(
            member(&properties, "principalArn").and_then(Value::as_str),
            Some(row.principal_arn.as_str()),
            "case {:?}: expected the generation to report the principal the request named, \
             got {properties:?}",
            row.case
        );
        let cloud_trail = member(&properties, "cloudTrailProperties")
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected cloudTrailProperties in {properties:?}",
                    row.case
                )
            });
        assert_trail_properties(row.case, &row.trails, member(&cloud_trail, "trailProperties"));
        assert!(
            same_instant(
                member(&cloud_trail, "startTime"),
                Some(&json!(row.start_time)),
            ),
            "case {:?}: expected the analysed range to start where the request said, \
             got {cloud_trail:?}",
            row.case
        );
        match row.end_time {
            // The end the request named is the end the generation reports.
            Some(end_time) => assert!(
                same_instant(member(&cloud_trail, "endTime"), Some(&json!(end_time))),
                "case {:?}: expected the analysed range to end where the request said, \
                 got {cloud_trail:?}",
                row.case
            ),
            // `endTime` omitted: "the default value is the current time", which is the
            // instant the generation was started.
            None => assert!(
                same_instant(
                    member(&cloud_trail, "endTime"),
                    member(&details, "startedOn"),
                ),
                "case {:?}: expected an omitted endTime to default to the instant the job \
                 started ({:?}), got {cloud_trail:?}",
                row.case,
                member(&details, "startedOn")
            ),
        }
        assert_eq!(
            member(&result, "generatedPolicies"),
            None,
            "case {:?}: a generation that has not completed produced no policy, got {result:?}",
            row.case
        );

        started.push((row, job_id, details));
    }

    // Two starts are two jobs: no id is handed out twice.
    let unique: BTreeSet<&str> = started.iter().map(|(_, id, _)| id.as_str()).collect();
    assert_eq!(
        unique.len(),
        started.len(),
        "expected one distinct jobId per start, got {:?}",
        started.iter().map(|(_, id, _)| id).collect::<Vec<_>>()
    );

    // ONE wait for the whole table: the jobs were all started before it.
    tokio::time::sleep(POLICY_GENERATION_SPAN + SETTLE_MARGIN).await;

    for (row, job_id, running) in &started {
        let reply = emulator.get_generated_policy(job_id, "").await;
        let details = job_details(row.case, &reply);
        assert_eq!(
            status_of(&details),
            "SUCCEEDED",
            "case {:?}: expected the generation to have SUCCEEDED once its span elapsed, \
             got {details:?}",
            row.case
        );
        assert!(
            same_instant(member(&details, "startedOn"), member(running, "startedOn")),
            "case {:?}: expected startedOn to be the instant the job started, got {details:?}",
            row.case
        );
        assert!(
            timestamp_at_least(member(&details, "completedOn"), member(&details, "startedOn")),
            "case {:?}: expected completedOn at or after startedOn, got {details:?}",
            row.case
        );

        // A completed generation reports the policy it produced, and it is a policy
        // document: valid JSON carrying the policy language's own elements.
        let result = policy_result(row.case, &reply);
        let policies = member(&result, "generatedPolicies")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected generatedPolicies on a SUCCEEDED job, got {result:?}",
                    row.case
                )
            });
        assert!(
            !policies.is_empty(),
            "case {:?}: expected at least one generated policy, got {result:?}",
            row.case
        );
        for policy in &policies {
            let text = member(policy, "policy")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let document: Value = serde_json::from_str(text).unwrap_or_else(|error| {
                panic!(
                    "case {:?}: expected the generated policy to be a JSON document, got {text:?} \
                     ({error})",
                    row.case
                )
            });
            assert!(
                document.get("Version").and_then(Value::as_str).is_some(),
                "case {:?}: expected the generated policy to carry a Version, got {document:?}",
                row.case
            );
            assert!(
                document.get("Statement").and_then(Value::as_array).is_some(),
                "case {:?}: expected the generated policy to carry a Statement list, \
                 got {document:?}",
                row.case
            );
        }

        // The listing projects the SAME record: one job, one status, one pair of instants.
        let listed = emulator.list_policy_generations("").await;
        assert_eq!(
            (listed.status, listed.code()),
            (OK, ""),
            "case {:?}: expected ListPolicyGenerations to answer {OK}, got {listed:?}",
            row.case
        );
        let entry = listed_job(&listed, job_id).unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the listing to report {job_id}, got {listed:?}",
                row.case
            )
        });
        assert_eq!(
            member(&entry, "principalArn").and_then(Value::as_str),
            Some(row.principal_arn.as_str()),
            "case {:?}: expected the listing to report the principal the start named, \
             got {entry:?}",
            row.case
        );
        assert_eq!(
            status_of(&entry),
            status_of(&details),
            "case {:?}: expected the listing to report the status the read reports, got {entry:?}",
            row.case
        );
        assert!(
            same_instant(member(&entry, "startedOn"), member(&details, "startedOn"))
                && same_instant(member(&entry, "completedOn"), member(&details, "completedOn")),
            "case {:?}: expected the listing to report the read's instants, got {entry:?} \
             against {details:?}",
            row.case
        );
    }
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    /// The principal the refused request names, so the listing can be asked whether that
    /// request left a job behind.
    principal_arn: String,
    /// What is wrong with the request, as a body built from the row.
    body: Value,
    /// The member the refusal must blame.
    member: &'static str,
    /// The exact `message` the refusal must carry, when AWS's wording for it is known.
    message: Option<&'static str>,
}

/// RULE: a start that does not describe CloudTrail activity to analyze is refused
/// synchronously with the modelled `ValidationException`, naming the member at fault, and
/// records no job.
#[tokio::test]
async fn a_start_that_describes_no_activity_to_analyze_is_refused_and_records_nothing() {
    let cases = vec![
        RefusalCase {
            case: "no cloudTrailDetails at all (the recorded case)",
            principal_arn: role_arn("pg_refuse_missing"),
            body: json!({
                "policyGenerationDetails": { "principalArn": role_arn("pg_refuse_missing") },
            }),
            member: "cloudTrailDetails",
            message: Some("Missing cloudTrailDetails"),
        },
        RefusalCase {
            case: "cloudTrailDetails naming no trail",
            principal_arn: role_arn("pg_refuse_no_trail"),
            body: start_body(
                &role_arn("pg_refuse_no_trail"),
                &[],
                RANGE_BASE + 31.0 * DAY,
                None,
                None,
            ),
            member: "cloudTrailDetails.trails",
            message: None,
        },
        RefusalCase {
            case: "a range that ends before it starts",
            principal_arn: role_arn("pg_refuse_range"),
            body: start_body(
                &role_arn("pg_refuse_range"),
                &[trail("pg-refuse-range-trail")],
                RANGE_BASE + 32.0 * DAY,
                Some(RANGE_BASE + 31.0 * DAY),
                None,
            ),
            member: "cloudTrailDetails.endTime",
            message: None,
        },
        RefusalCase {
            case: "a range that ends one second before it starts",
            principal_arn: role_arn("pg_refuse_range_close"),
            body: start_body(
                &role_arn("pg_refuse_range_close"),
                &[trail("pg-refuse-range-close-trail")],
                RANGE_BASE + 32.0 * DAY + 1.0,
                Some(RANGE_BASE + 32.0 * DAY),
                None,
            ),
            member: "cloudTrailDetails.endTime",
            message: None,
        },
    ];

    let emulator = Emulator::new();
    for RefusalCase {
        case,
        principal_arn,
        body,
        member: blamed,
        message,
    } in cases
    {
        let reply = emulator.start_policy_generation(body).await;
        assert_eq!(
            (reply.status, reply.code()),
            (BAD_REQUEST, VALIDATION),
            "case {case:?}: expected {BAD_REQUEST} {VALIDATION}, got {reply:?}"
        );
        assert_eq!(
            reply.member("reason").and_then(Value::as_str),
            Some(field_validation_failed()),
            "case {case:?}: expected the modelled reason, got {reply:?}"
        );
        let fields = reply
            .member("fieldList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            fields
                .iter()
                .filter_map(|field| field.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>(),
            vec![blamed],
            "case {case:?}: expected the refusal to blame {blamed}, got {reply:?}"
        );
        if let Some(message) = message {
            assert_eq!(
                reply.member("message").and_then(Value::as_str),
                Some(message),
                "case {case:?}: expected the refusal to be worded {message:?}, got {reply:?}"
            );
        }

        // Refused synchronously means no job exists to track: the listing scoped to the
        // principal the request named reports nothing at all.
        let listed = emulator
            .list_policy_generations(&format!("?principalArn={principal_arn}"))
            .await;
        assert_eq!(
            (listed.status, listed.code()),
            (OK, ""),
            "case {case:?}: expected ListPolicyGenerations to answer {OK}, got {listed:?}"
        );
        assert_eq!(
            listed_ids(&listed),
            Vec::<String>::new(),
            "case {case:?}: a refused start records no job, got {listed:?}"
        );
    }
}

/// RULE: a cancel stops a generation that is still running and is refused for one that
/// has already reached a terminal status; either way the job keeps reporting one status.
#[tokio::test]
async fn a_cancel_stops_a_running_generation_and_is_refused_for_a_terminal_one() {
    let emulator = Emulator::new();

    // A job cancelled while it is still running.
    let running_case = "a cancel while the generation is still running";
    let running = start_ok(
        &emulator,
        running_case,
        start_body(
            &role_arn("pg_cancel_running"),
            &[trail("pg-cancel-running-trail")],
            RANGE_BASE + 60.0 * DAY,
            None,
            None,
        ),
    )
    .await;
    let canceled = emulator.cancel_policy_generation(&running).await;
    assert_eq!(
        (canceled.status, canceled.code()),
        (NO_CONTENT, ""),
        "case {running_case:?}: expected CancelPolicyGeneration to answer {NO_CONTENT}, \
         got {canceled:?}"
    );
    assert!(
        canceled.carries_no_content(),
        "case {running_case:?}: expected the cancel to report no content, got {canceled:?}"
    );

    let reply = emulator.get_generated_policy(&running, "").await;
    let details = job_details(running_case, &reply);
    assert_eq!(
        status_of(&details),
        "CANCELED",
        "case {running_case:?}: expected the cancelled generation to report CANCELED, \
         got {details:?}"
    );
    assert!(
        is_timestamp(member(&details, "completedOn")),
        "case {running_case:?}: expected completedOn to be the instant the cancel stopped it, \
         got {details:?}"
    );
    assert_eq!(
        member(&policy_result(running_case, &reply), "generatedPolicies"),
        None,
        "case {running_case:?}: a cancelled generation produced no policy, got {reply:?}"
    );

    // The same cancel retried: `@idempotent`, so it answers the same success and moves
    // nothing — above all not the instant the generation stopped.
    let again = emulator.cancel_policy_generation(&running).await;
    assert_eq!(
        (again.status, again.code()),
        (NO_CONTENT, ""),
        "case {running_case:?}: expected a repeated cancel to answer {NO_CONTENT}, got {again:?}"
    );
    let reread = job_details(running_case, &emulator.get_generated_policy(&running, "").await);
    assert_eq!(
        status_of(&reread),
        "CANCELED",
        "case {running_case:?}: expected the job to stay CANCELED, got {reread:?}"
    );
    assert!(
        same_instant(
            member(&reread, "completedOn"),
            member(&details, "completedOn")
        ),
        "case {running_case:?}: expected a repeated cancel to leave completedOn alone, \
         got {reread:?} against {details:?}"
    );

    // Even after its span would have elapsed, a cancelled generation does not succeed:
    // the cancel is terminal.
    let completing_case = "a cancel of a generation that has already completed";
    let completed = start_ok(
        &emulator,
        completing_case,
        start_body(
            &role_arn("pg_cancel_completed"),
            &[trail("pg-cancel-completed-trail")],
            RANGE_BASE + 61.0 * DAY,
            None,
            None,
        ),
    )
    .await;
    tokio::time::sleep(POLICY_GENERATION_SPAN + SETTLE_MARGIN).await;
    let still_canceled = job_details(
        running_case,
        &emulator.get_generated_policy(&running, "").await,
    );
    assert_eq!(
        status_of(&still_canceled),
        "CANCELED",
        "case {running_case:?}: a cancelled generation never reaches SUCCEEDED, \
         got {still_canceled:?}"
    );

    let before = job_details(
        completing_case,
        &emulator.get_generated_policy(&completed, "").await,
    );
    assert_eq!(
        status_of(&before),
        "SUCCEEDED",
        "case {completing_case:?}: expected the generation to have SUCCEEDED, got {before:?}"
    );
    let refused = emulator.cancel_policy_generation(&completed).await;
    assert_eq!(
        (refused.status, refused.code()),
        (BAD_REQUEST, VALIDATION),
        "case {completing_case:?}: expected {BAD_REQUEST} {VALIDATION}, got {refused:?}"
    );
    assert_eq!(
        refused.member("reason").and_then(Value::as_str),
        Some(field_validation_failed()),
        "case {completing_case:?}: expected the modelled reason, got {refused:?}"
    );
    let message = refused
        .member("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains(&completed) && message.contains(status_of(&before)),
        "case {completing_case:?}: expected the refusal to name the job and the status it is in, \
         got {refused:?}"
    );
    // A refused cancel changes nothing about the job.
    let after = job_details(
        completing_case,
        &emulator.get_generated_policy(&completed, "").await,
    );
    assert_eq!(
        (status_of(&after), member(&after, "completedOn")),
        (status_of(&before), member(&before, "completedOn")),
        "case {completing_case:?}: expected the refused cancel to leave the job alone, \
         got {after:?} against {before:?}"
    );
}

/// RULE: a `jobId` no start minted names no job, so every operation that takes one refuses
/// it with the modelled `ValidationException` rather than reporting an empty job.
#[tokio::test]
async fn an_unknown_job_id_is_refused_by_every_operation_that_takes_one() {
    let emulator = Emulator::new();
    // A real job, so the refusals below are about the id and not about an empty store.
    let known = start_ok(
        &emulator,
        "a job to tell the unknown ids from",
        start_body(
            &role_arn("pg_unknown_baseline"),
            &[trail("pg-unknown-baseline-trail")],
            RANGE_BASE + 90.0 * DAY,
            None,
            None,
        ),
    )
    .await;

    // The forms an id a caller can name takes: a well-formed UUID nobody minted, a bare
    // word, a very long value, and one that is the known id with a single character
    // changed.
    let mutated: String = {
        let mut id: Vec<char> = known.chars().collect();
        let last = id.len() - 1;
        id[last] = if id[last] == 'a' { 'b' } else { 'a' };
        id.into_iter().collect()
    };
    let cases: Vec<(&str, String)> = vec![
        (
            "a well-formed UUID no start minted",
            "11111111-2222-4333-8444-555555555555".to_string(),
        ),
        ("a bare word", "not-a-job".to_string()),
        ("a 512-character value", "j".repeat(512)),
        ("the known id with one character changed", mutated),
    ];

    for (case, job_id) in cases {
        let read = emulator.get_generated_policy(&job_id, "").await;
        assert_eq!(
            (read.status, read.code()),
            (BAD_REQUEST, VALIDATION),
            "case {case:?}: expected GetGeneratedPolicy to answer {BAD_REQUEST} {VALIDATION}, \
             got {read:?}"
        );
        assert_eq!(
            read.member("reason").and_then(Value::as_str),
            Some(field_validation_failed()),
            "case {case:?}: expected the modelled reason, got {read:?}"
        );

        let canceled = emulator.cancel_policy_generation(&job_id).await;
        assert_eq!(
            (canceled.status, canceled.code()),
            (BAD_REQUEST, VALIDATION),
            "case {case:?}: expected CancelPolicyGeneration to answer {BAD_REQUEST} \
             {VALIDATION}, got {canceled:?}"
        );
    }

    // The known job is untouched by all of that.
    let details = job_details(
        "a job to tell the unknown ids from",
        &emulator.get_generated_policy(&known, "").await,
    );
    assert_eq!(
        status_of(&details),
        "IN_PROGRESS",
        "expected the known job to still be running, got {details:?}"
    );
}

/// RULE: reading a job reports it, it does not run it — polling neither advances the
/// status nor restates when the generation started, whichever detail level is asked for.
#[tokio::test]
async fn polling_a_generation_never_changes_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let case = "a job polled while it runs and after it completes";
    let job_id = start_ok(
        &emulator,
        case,
        start_body(
            &role_arn("pg_polling"),
            &[trail("pg-polling-trail")],
            RANGE_BASE + 120.0 * DAY,
            None,
            None,
        ),
    )
    .await;

    // The two detail flags are the other thing a read can vary; neither selects a
    // different job.
    let queries = [
        "",
        "?includeResourcePlaceholders=true",
        "?includeServiceLevelTemplate=true",
        "?includeResourcePlaceholders=false&includeServiceLevelTemplate=false",
    ];

    let first = job_details(case, &emulator.get_generated_policy(&job_id, "").await);
    for query in queries {
        let polled = job_details(
            case,
            &emulator.get_generated_policy(&job_id, query).await,
        );
        assert_eq!(
            status_of(&polled),
            "IN_PROGRESS",
            "case {case:?} ({query:?}): polling must not advance the generation, got {polled:?}"
        );
        assert!(
            same_instant(member(&polled, "startedOn"), member(&first, "startedOn")),
            "case {case:?} ({query:?}): polling must not restate startedOn, got {polled:?}"
        );
        assert_eq!(
            member(&polled, "completedOn"),
            None,
            "case {case:?} ({query:?}): a running generation has no completedOn, got {polled:?}"
        );
    }

    tokio::time::sleep(POLICY_GENERATION_SPAN + SETTLE_MARGIN).await;

    let completed = job_details(case, &emulator.get_generated_policy(&job_id, "").await);
    assert_eq!(
        status_of(&completed),
        "SUCCEEDED",
        "case {case:?}: expected the generation to have SUCCEEDED, got {completed:?}"
    );
    for query in queries {
        let polled = job_details(
            case,
            &emulator.get_generated_policy(&job_id, query).await,
        );
        assert_eq!(
            status_of(&polled),
            "SUCCEEDED",
            "case {case:?} ({query:?}): a completed generation stays SUCCEEDED, got {polled:?}"
        );
        assert!(
            same_instant(
                member(&polled, "completedOn"),
                member(&completed, "completedOn")
            ),
            "case {case:?} ({query:?}): the instant it completed must not move, got {polled:?}"
        );
    }
}

/// RULE: a `clientToken` makes a start idempotent — the same token answers with the job
/// the first call minted and generates no second policy, while a request carrying another
/// token (or none) is another generation.
#[tokio::test]
async fn a_repeated_client_token_answers_with_the_first_job() {
    let emulator = Emulator::new();
    let principal = role_arn("pg_tokens");
    let body = |token: Option<&str>| {
        start_body(
            &principal,
            &[trail("pg-tokens-trail")],
            RANGE_BASE + 150.0 * DAY,
            None,
            token,
        )
    };

    // (case, the token the call carries, whether it must answer with the first job)
    let cases: Vec<(&str, Option<&str>, bool)> = vec![
        ("the first call, carrying a token", Some("pg-token-a"), false),
        ("the same token repeated", Some("pg-token-a"), true),
        (
            "the same token repeated once more",
            Some("pg-token-a"),
            true,
        ),
        ("another token", Some("pg-token-b"), false),
        ("no token at all", None, false),
    ];

    let mut first: Option<String> = None;
    let mut distinct: BTreeSet<String> = BTreeSet::new();
    for (case, token, replays) in cases {
        let job_id = start_ok(&emulator, case, body(token)).await;
        match (replays, &first) {
            (true, Some(first)) => assert_eq!(
                &job_id, first,
                "case {case:?}: expected the recorded token to answer with {first}, got {job_id}"
            ),
            (true, None) => panic!("case {case:?}: no first job to replay"),
            (false, _) => assert!(
                distinct.insert(job_id.clone()),
                "case {case:?}: expected a new jobId, got the already-minted {job_id}"
            ),
        }
        if first.is_none() {
            first = Some(job_id);
        }
    }

    // Three of the five calls were new generations, so that is what the principal holds —
    // a replayed token generated nothing further.
    let listed = emulator
        .list_policy_generations(&format!("?principalArn={principal}"))
        .await;
    assert_eq!(
        listed_ids(&listed).len(),
        distinct.len(),
        "expected {} generations for {principal}, got {listed:?}",
        distinct.len()
    );
}

/// RULE: the listing reports the jobs the store holds, narrowed to a `principalArn` when
/// one is named, and pages over them so a caller following `nextToken` sees each exactly
/// once.
#[tokio::test]
async fn the_listing_filters_by_principal_and_pages_over_the_stored_jobs() {
    let emulator = Emulator::new();
    let first_principal = role_arn("pg_list_one");
    let second_principal = user_arn("pg_list_two");

    let mut expected: Vec<(String, String)> = Vec::new();
    for (index, principal) in [
        &first_principal,
        &first_principal,
        &first_principal,
        &second_principal,
        &second_principal,
    ]
    .into_iter()
    .enumerate()
    {
        let case = "the jobs the listing must report";
        let job_id = start_ok(
            &emulator,
            case,
            start_body(
                principal,
                &[trail(&format!("pg-list-trail-{index}"))],
                RANGE_BASE + 180.0 * DAY,
                None,
                None,
            ),
        )
        .await;
        expected.push((principal.clone(), job_id));
    }

    // (case, the principalArn filter, the jobs it must report)
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "no filter: every job the store holds",
            String::new(),
            expected.iter().map(|(_, id)| id.clone()).collect(),
        ),
        (
            "the principal with three jobs",
            format!("?principalArn={first_principal}"),
            expected
                .iter()
                .filter(|(principal, _)| principal == &first_principal)
                .map(|(_, id)| id.clone())
                .collect(),
        ),
        (
            "the principal with two",
            format!("?principalArn={second_principal}"),
            expected
                .iter()
                .filter(|(principal, _)| principal == &second_principal)
                .map(|(_, id)| id.clone())
                .collect(),
        ),
        (
            "a principal with none",
            format!("?principalArn={}", role_arn("pg_list_absent")),
            Vec::new(),
        ),
    ];

    for (case, query, want) in cases {
        let listed = emulator.list_policy_generations(&query).await;
        assert_eq!(
            (listed.status, listed.code()),
            (OK, ""),
            "case {case:?}: expected ListPolicyGenerations to answer {OK}, got {listed:?}"
        );
        let got: BTreeSet<String> = listed_ids(&listed).into_iter().collect();
        assert_eq!(
            got,
            want.iter().cloned().collect::<BTreeSet<String>>(),
            "case {case:?}: expected exactly {want:?}, got {listed:?}"
        );
    }

    // Paging: every page size from one job to more than the store holds enumerates each
    // job exactly once, and only the last page carries no token.
    for page_size in [1_usize, 2, 5, 50] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        loop {
            let page = emulator.list_policy_generations(&query).await;
            assert_eq!(
                (page.status, page.code()),
                (OK, ""),
                "case {case:?}: expected a page, got {page:?}"
            );
            let ids = listed_ids(&page);
            assert!(
                ids.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} jobs, got {page:?}"
            );
            seen.extend(ids);
            match page.member("nextToken").and_then(Value::as_str) {
                Some(token) => query = format!("?maxResults={page_size}&nextToken={token}"),
                None => break,
            }
        }
        let unique: BTreeSet<&str> = seen.iter().map(String::as_str).collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case:?}: expected every job once, got {seen:?}"
        );
        assert_eq!(
            unique.len(),
            expected.len(),
            "case {case:?}: expected all {} jobs across the pages, got {seen:?}",
            expected.len()
        );
    }

    // The paging arguments are modelled, so a page size below the model's minimum is
    // refused rather than clamped.
    let refused = emulator.list_policy_generations("?maxResults=0").await;
    assert_eq!(
        (refused.status, refused.code()),
        (BAD_REQUEST, VALIDATION),
        "a maxResults below the modelled minimum: expected {BAD_REQUEST} {VALIDATION}, \
         got {refused:?}"
    );
    let bad_token = emulator
        .list_policy_generations("?nextToken=not-a-token")
        .await;
    assert_eq!(
        (bad_token.status, bad_token.code()),
        (BAD_REQUEST, VALIDATION),
        "a nextToken this service never issued: expected {BAD_REQUEST} {VALIDATION}, \
         got {bad_token:?}"
    );
}
