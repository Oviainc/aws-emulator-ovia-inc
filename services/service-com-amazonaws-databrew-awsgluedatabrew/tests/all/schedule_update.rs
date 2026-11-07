// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateSchedule`: `PUT /schedules/{Name}` followed by the
//! `DescribeSchedule` read, so what the update wrote is what a later caller sees.
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch` with a
//! restJson1 request), so what is asserted is the HTTP status, the error code and the
//! JSON body a client sees.
//!
//! The tables span the class the handler generalises over rather than one recorded
//! request: every documented form of a DataBrew cron expression, every shape of a
//! `JobNames` list (absent, empty, one, several, reordered), and every way an
//! expression can fail the documented form.

use serde_json::{json, Value};

use crate::databrew_harness::{job_outputs, label, Emulator, Reply, CALLER_PRINCIPAL};

/// The one sentence DataBrew answers every invalid schedule cron expression with
/// (`INVALID_CRON_EXPRESSION_MESSAGE` in `src/state.rs`).
const INVALID_CRON_MESSAGE: &str =
    "Invalid schedule cron expression. Expected format: Cron(CRON_EXPRESSION)";

/// The cron expression a schedule is created with, so every case can tell the stored
/// expression from the one its update wrote.
const CREATED_CRON: &str = "cron(0 12 * * ? *)";

/// `CreateRecipeJob`: the job a schedule can name. `RoleArn` and one selected output are
/// the only other members it requires.
async fn create_job(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "POST",
            "/recipeJobs",
            Some(json!({
                "Name": name,
                "RoleArn": "arn:aws:iam::000000000000:role/brew",
                "Outputs": job_outputs("out/"),
            })),
        )
        .await
}

/// `CreateSchedule`: `POST /schedules` (model: `smithy_query(query="operation",
/// operation="CreateSchedule")`).
async fn create_schedule(emulator: &Emulator, body: Value) -> Reply {
    emulator.call("POST", "/schedules", Some(body)).await
}

/// `UpdateSchedule`: `PUT /schedules/{Name}`, the schedule's name on the URI label and
/// the rest of the definition in the body (model:
/// `smithy_query(query="operation", operation="UpdateSchedule")`).
async fn update_schedule(emulator: &Emulator, name: &str, body: Value) -> Reply {
    emulator
        .call("PUT", &format!("/schedules/{}", label(name)), Some(body))
        .await
}

/// `DescribeSchedule`: `GET /schedules/{Name}`.
async fn describe_schedule(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call("GET", &format!("/schedules/{}", label(name)), None)
        .await
}

/// An emulator holding `jobs` and one schedule named `schedule` on [`CREATED_CRON`],
/// carrying a tag and naming the first job.
async fn seeded(schedule: &str, jobs: &[&str]) -> Emulator {
    let emulator = Emulator::new();
    for job in jobs {
        assert_eq!(
            create_job(&emulator, job).await.status,
            200,
            "case {job:?}: the job a schedule names must be created"
        );
    }
    let created = create_schedule(
        &emulator,
        json!({
            "Name": schedule,
            "CronExpression": CREATED_CRON,
            "JobNames": jobs.first().map(|job| vec![*job]).unwrap_or_default(),
            "Tags": {"team": "data"},
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {schedule:?}: the schedule to update must be created, got {} {:?}",
        created.status, created.body
    );
    emulator
}

/// An update REPLACES the schedule's definition — the cron expression it runs on and
/// the jobs it runs — and stamps the audit's last-modified pair, leaving the members the
/// operation carries no value for (the name, the ARN, the tags, the create pair) as the
/// create wrote them.
///
/// The rows differ along what the rule generalises over: the documented cron forms
/// (source: the Glue DataBrew Developer Guide's "Cron expressions" page, which
/// `UpdateScheduleRequest$CronExpression`'s doc links to — `cron(minutes hours
/// day-of-month month day-of-week year)` with the `*`, `-`, `,`, `/`, `?`, `L`, `W` and
/// `#` wildcards) and the shapes of a `JobNames` list, including the absent and empty
/// list the modelled `len 0..50` allows.
#[tokio::test]
async fn an_update_replaces_the_definition_and_stamps_the_audit() {
    let cases: [(&str, &str, Option<Value>, Value); 8] = [
        (
            "a different time of day",
            "cron(30 7 * * ? *)",
            Some(json!(["job-a"])),
            json!(["job-a"]),
        ),
        (
            "the guide's last-Friday example",
            "cron(15 10 ? * 6L 2019-2022)",
            Some(json!(["job-b"])),
            json!(["job-b"]),
        ),
        (
            "a step over a range of hours, several jobs",
            "cron(0/5 8-17 ? * MON-FRI *)",
            Some(json!(["job-a", "job-b", "job-c"])),
            json!(["job-a", "job-b", "job-c"]),
        ),
        (
            "the same jobs in another order",
            "cron(0,30 6,18 1,15 * ? *)",
            Some(json!(["job-c", "job-a"])),
            json!(["job-c", "job-a"]),
        ),
        (
            "the last weekday of the month, an empty job list",
            "cron(0 0 LW * ? *)",
            Some(json!([])),
            json!([]),
        ),
        (
            "no JobNames member at all",
            "cron(0 8 15W * ? *)",
            None,
            json!([]),
        ),
        (
            "the first Sunday of January",
            "cron(0 0 ? JAN SUN#1 *)",
            Some(json!(["job-b", "job-c"])),
            json!(["job-b", "job-c"]),
        ),
        (
            "a step over every value",
            "cron(*/15 * * * ? *)",
            Some(json!(["job-a"])),
            json!(["job-a"]),
        ),
    ];

    for (case, cron_expression, job_names, expected_jobs) in cases {
        let emulator = seeded("nightly", &["job-a", "job-b", "job-c"]).await;
        let before = describe_schedule(&emulator, "nightly").await;

        let mut body = json!({"CronExpression": cron_expression});
        if let Some(job_names) = job_names {
            body["JobNames"] = job_names;
        }
        let updated = update_schedule(&emulator, "nightly", body).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?} ({cron_expression}): expected UpdateSchedule to answer 200, got {} {:?}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.text("Name"),
            "nightly",
            "case {case:?}: UpdateSchedule must echo the name it updated, got {:?}",
            updated.body
        );

        let after = describe_schedule(&emulator, "nightly").await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: expected DescribeSchedule to answer 200, got {} {:?}",
            after.status, after.body
        );
        assert_eq!(
            after.text("CronExpression"),
            cron_expression,
            "case {case:?}: expected the updated CronExpression back, got {:?}",
            after.body["CronExpression"]
        );
        assert_eq!(
            after.member("JobNames"),
            expected_jobs,
            "case {case:?}: expected JobNames {expected_jobs:?}, got {:?}",
            after.body["JobNames"]
        );
        // `UpdateSchedule` carries no `Tags`, `Name` or ARN member, so none of them is
        // this operation's to change.
        assert_eq!(
            after.member("Tags"),
            json!({"team": "data"}),
            "case {case:?}: an update must leave the tags the create wrote, got {:?}",
            after.body["Tags"]
        );
        assert_eq!(
            after.member("ResourceArn"),
            before.member("ResourceArn"),
            "case {case:?}: an update must leave the ARN unchanged, got {:?}",
            after.body["ResourceArn"]
        );
        assert_eq!(
            after.member("CreateDate"),
            before.member("CreateDate"),
            "case {case:?}: an update must leave the CreateDate unchanged, got {:?}",
            after.body["CreateDate"]
        );
        assert_eq!(
            after.text("CreatedBy"),
            CALLER_PRINCIPAL,
            "case {case:?}: an update must leave CreatedBy naming the creator, got {:?}",
            after.body["CreatedBy"]
        );
        // The create left the last-modified pair unset; the update is the modification
        // that fills it.
        assert!(
            before.is_absent("LastModifiedDate") && before.is_absent("LastModifiedBy"),
            "case {case:?}: an unmodified schedule reports no last-modified pair, got {:?}",
            before.body
        );
        assert!(
            after.body["LastModifiedDate"].as_f64().unwrap_or_default()
                >= before.body["CreateDate"].as_f64().unwrap_or_default(),
            "case {case:?}: expected a LastModifiedDate at or after the CreateDate, got {:?}",
            after.body["LastModifiedDate"]
        );
        assert_eq!(
            after.text("LastModifiedBy"),
            CALLER_PRINCIPAL,
            "case {case:?}: expected LastModifiedBy to name the calling principal, got {:?}",
            after.body["LastModifiedBy"]
        );
    }
}

/// An expression that is not a cron expression of the documented form is refused with
/// the modelled `ValidationException` (HTTP 400) carrying the service's one invalid-cron
/// sentence, and the refused update leaves the stored schedule exactly as it was.
///
/// The rows span the ways an expression can fail the form: no `cron(..)` around the
/// fields, another expression language, free text, the wrong field count, a field
/// outside its published range, a malformed wildcard, and the guide's limit that `?`
/// stands in exactly one of day-of-month and day-of-week.
#[tokio::test]
async fn an_update_outside_the_documented_cron_form_is_refused() {
    let cases = [
        ("a bare cron body", "0 12 * * ? *"),
        ("a rate expression", "rate(1 hour)"),
        ("free text", "every day at noon"),
        ("a cron( with no closing paren", "cron(0 12 * * ? *"),
        ("five fields", "cron(0 12 * * ?)"),
        ("seven fields", "cron(0 12 * * ? * *)"),
        ("no fields", "cron()"),
        ("both day fields specified", "cron(0 12 1 * MON *)"),
        ("neither day field specified", "cron(0 12 ? * ? *)"),
        ("minutes past 59", "cron(60 12 * * ? *)"),
        ("hours past 23", "cron(0 24 * * ? *)"),
        ("a year before 1970", "cron(0 12 * * ? 1969)"),
        ("a reversed range", "cron(30-5 12 * * ? *)"),
        ("a zero step", "cron(0/0 12 * * ? *)"),
        ("an unknown month alias", "cron(0 12 * MOO ? *)"),
        ("a nth-day past the fifth", "cron(0 12 ? * 6#6 *)"),
    ];

    let emulator = seeded("nightly", &["job-a"]).await;
    for (case, cron_expression) in cases {
        let refused = update_schedule(
            &emulator,
            "nightly",
            json!({"CronExpression": cron_expression, "JobNames": ["job-a"]}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} ({cron_expression}): expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?} ({cron_expression}): expected ValidationException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.error_message(),
            INVALID_CRON_MESSAGE,
            "case {case:?} ({cron_expression}): expected the service's invalid-cron message, got {:?}",
            refused.body
        );

        let described = describe_schedule(&emulator, "nightly").await;
        assert_eq!(
            described.text("CronExpression"),
            CREATED_CRON,
            "case {case:?}: the refused update must leave the stored expression, got {:?}",
            described.body["CronExpression"]
        );
        assert!(
            described.is_absent("LastModifiedDate"),
            "case {case:?}: a refused update is no modification, got {:?}",
            described.body["LastModifiedDate"]
        );
    }
}

/// A schedule runs the jobs it names, so a schedule WRITE can only name jobs this
/// account holds: a name that resolves to no job is refused with the modelled
/// `ValidationException`, whose message names the missing job, and the stored schedule
/// is left as it was. The same rule holds on the create path, so the last rows drive it
/// through `CreateSchedule` too.
#[tokio::test]
async fn a_schedule_write_can_only_name_jobs_that_exist() {
    let cases = [
        ("one unknown job", json!(["no-such-job"]), "no-such-job"),
        (
            "an unknown job after a known one",
            json!(["job-a", "no-such-job"]),
            "no-such-job",
        ),
        (
            "an unknown job before a known one",
            json!(["missing-job", "job-a"]),
            "missing-job",
        ),
        ("a job name of a different case", json!(["JOB-A"]), "JOB-A"),
    ];

    let emulator = seeded("nightly", &["job-a", "job-b"]).await;
    for (case, job_names, missing) in cases {
        let job_names_again = job_names.clone();
        let refused = update_schedule(
            &emulator,
            "nightly",
            json!({"CronExpression": "cron(0 6 * * ? *)", "JobNames": job_names}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.error_message(),
            format!("Job {missing} not found"),
            "case {case:?}: the refusal must name the missing job, got {:?}",
            refused.body
        );

        let described = describe_schedule(&emulator, "nightly").await;
        assert_eq!(
            described.text("CronExpression"),
            CREATED_CRON,
            "case {case:?}: the refused update must store nothing, got {:?}",
            described.body["CronExpression"]
        );
        assert_eq!(
            described.member("JobNames"),
            json!(["job-a"]),
            "case {case:?}: the refused update must leave the stored JobNames, got {:?}",
            described.body["JobNames"]
        );

        // The rule is the schedule record's, not one operation's: the create path
        // refuses the same list with the same message.
        let refused_create = create_schedule(
            &emulator,
            json!({
                "Name": format!("fresh-{case}"),
                "CronExpression": "cron(0 6 * * ? *)",
                "JobNames": job_names_again,
            }),
        )
        .await;
        assert_eq!(
            refused_create.status, 400,
            "case {case:?} on CreateSchedule: expected 400, got {} {:?}",
            refused_create.status, refused_create.body
        );
        assert_eq!(
            refused_create.error_message(),
            format!("Job {missing} not found"),
            "case {case:?} on CreateSchedule: the refusal must name the missing job, got {:?}",
            refused_create.body
        );
    }
}

/// `Name` addresses the schedule to update rather than a name this operation mints, so
/// a name no schedule holds resolves nothing: the modelled `ResourceNotFoundException`
/// (HTTP 404), naming the kind of resource and the identifier the request carried.
#[tokio::test]
async fn an_update_of_a_schedule_nobody_created_is_refused() {
    let emulator = seeded("nightly", &["job-a"]).await;
    // A name no schedule holds, including names the create path would have accepted and
    // one that only differs from the stored schedule's by case.
    for name in ["ghost", "NIGHTLY", "nightly.2", "with space"] {
        let refused = update_schedule(
            &emulator,
            name,
            json!({"CronExpression": "cron(0 6 * * ? *)"}),
        )
        .await;
        assert_eq!(
            refused.status, 404,
            "case {name:?}: expected 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {name:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.error_message(),
            // A schedule miss is stated in DataBrew's short form, with no terminating
            // period; `tests/all/schedule_refusal_wording.rs` spans that rule.
            format!("Schedule {name} not found"),
            "case {name:?}: unexpected not-found message, got {:?}",
            refused.body
        );
        assert_eq!(
            describe_schedule(&emulator, name).await.status,
            404,
            "case {name:?}: the refused update must create nothing"
        );
    }
}

/// An update writes the definition it carries, so applying the same definition twice
/// leaves the same definition stored, and a later update is what the schedule serves.
#[tokio::test]
async fn repeated_updates_leave_the_last_definition_stored() {
    let emulator = seeded("nightly", &["job-a", "job-b"]).await;
    let definitions = [
        ("cron(0 6 * * ? *)", json!(["job-a"])),
        ("cron(0 6 * * ? *)", json!(["job-a"])),
        ("cron(45 23 L * ? *)", json!(["job-b", "job-a"])),
        ("cron(45 23 L * ? *)", json!(["job-b", "job-a"])),
    ];

    for (round, (cron_expression, job_names)) in definitions.into_iter().enumerate() {
        let updated = update_schedule(
            &emulator,
            "nightly",
            json!({"CronExpression": cron_expression, "JobNames": job_names.clone()}),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case round {round} ({cron_expression}): expected 200, got {} {:?}",
            updated.status, updated.body
        );

        let described = describe_schedule(&emulator, "nightly").await;
        assert_eq!(
            described.text("CronExpression"),
            cron_expression,
            "case round {round}: expected the definition just written, got {:?}",
            described.body["CronExpression"]
        );
        assert_eq!(
            described.member("JobNames"),
            job_names,
            "case round {round}: expected the JobNames just written, got {:?}",
            described.body["JobNames"]
        );
    }
}
