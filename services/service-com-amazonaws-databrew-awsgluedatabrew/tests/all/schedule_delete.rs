// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteSchedule`: the delete drops the stored schedule record
//! and echoes the name it dropped, so every later read of that name resolves nothing,
//! and a name no schedule is stored under is refused with the modelled 404.
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch` with a
//! restJson1 request), so what is asserted is the HTTP status, the error code and the
//! JSON body a client sees.
//!
//! The tables below span the class the implementation generalises over rather than one
//! recorded request: every shape a schedule can have when it is deleted (its cron
//! expression, the number of jobs it names, its tags, the form and length of its name),
//! and every way a `DeleteSchedule` can address a name nothing is stored under.

use serde_json::{json, Value};

use crate::databrew_harness::{label, Emulator, Reply, ROLE_ARN};

/// `CreateSchedule`: `POST /schedules` (model: `smithy_query(query="operation",
/// operation="CreateSchedule")`).
async fn create_schedule(emulator: &Emulator, body: Value) -> Reply {
    emulator.call("POST", "/schedules", Some(body)).await
}

/// `DescribeSchedule`: `GET /schedules/{Name}`.
async fn describe_schedule(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call("GET", &format!("/schedules/{}", label(name)), None)
        .await
}

/// `UpdateSchedule`: `PUT /schedules/{Name}`.
async fn update_schedule(emulator: &Emulator, name: &str, cron_expression: &str) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/schedules/{}", label(name)),
            Some(json!({"CronExpression": cron_expression})),
        )
        .await
}

/// `DeleteSchedule`: `DELETE /schedules/{Name}` (model: `smithy_query(query="operation",
/// operation="DeleteSchedule")`).
async fn delete_schedule(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call("DELETE", &format!("/schedules/{}", label(name)), None)
        .await
}

/// `CreateRecipeJob`: the job a schedule names. `RoleArn` is the only other member the
/// create requires.
async fn create_job(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "POST",
            "/recipeJobs",
            Some(json!({"Name": name, "RoleArn": ROLE_ARN})),
        )
        .await
}

/// `DescribeJob`: `GET /jobs/{Name}`, so a case can check a deleted schedule left the
/// jobs it named alone.
async fn describe_job(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call("GET", &format!("/jobs/{}", label(name)), None)
        .await
}

/// The sentence DataBrew answers a missing schedule with (`not_found_message` in
/// `src/state.rs`, which words this resource kind's refusals in the short form).
fn schedule_not_found_message(name: &str) -> String {
    format!("Schedule {name} not found")
}

/// One row of the delete table: a schedule of some shape, created and then deleted.
struct DeleteCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The schedule name, which is also what the delete echoes and the misses report.
    name: &'static str,
    /// The expression the schedule runs on.
    cron_expression: &'static str,
    /// The jobs it names, created first; `None` omits the member entirely.
    job_names: Option<&'static [&'static str]>,
    /// Its tags, when the row carries any.
    tags: Option<&'static str>,
}

/// A schedule of any shape is deleted, the delete echoes the name it dropped, and every
/// later read of that name — describe, update, a second delete — resolves nothing.
///
/// The rows vary the shape of the record being deleted: the documented cron forms
/// (`cron(minutes hours day-of-month month day-of-week year)`, the forms
/// `CreateScheduleRequest$CronExpression`'s doc links to), the count of jobs it names
/// (none, one, several), whether it carries tags, and the form and length of its name
/// over the documented character set ("Valid characters are alphanumeric (A-Z, a-z,
/// 0-9), hyphen (-), period (.), and space" — `smithy_query(query="doc",
/// shape="CreateScheduleRequest", member="Name")`).
#[tokio::test]
async fn deleting_a_schedule_of_any_shape_drops_it_and_echoes_its_name() {
    let cases = [
        DeleteCase {
            case: "a plain schedule naming no job",
            name: "plain",
            cron_expression: "cron(0 12 * * ? *)",
            job_names: None,
            tags: None,
        },
        DeleteCase {
            case: "an empty job list",
            name: "no-jobs",
            cron_expression: "cron(*/15 * * * ? *)",
            job_names: Some(&[]),
            tags: None,
        },
        DeleteCase {
            case: "one job",
            name: "one.job",
            cron_expression: "cron(15 10 ? * 6L 2019-2022)",
            job_names: Some(&["job-a"]),
            tags: None,
        },
        DeleteCase {
            case: "several jobs and tags",
            name: "many jobs",
            cron_expression: "cron(0,30 6,18 1,15 * ? *)",
            job_names: Some(&["job-a", "job-b", "job-c"]),
            tags: Some("team"),
        },
        DeleteCase {
            case: "a one-character name, the modelled minimum",
            name: "s",
            cron_expression: "cron(59 23 L * ? *)",
            job_names: Some(&["job-b"]),
            tags: Some("owner"),
        },
        DeleteCase {
            case: "a mixed-case name of the documented character set",
            name: "MiXeD123.sched-ule 7",
            cron_expression: "cron(0 0 ? JAN SUN#1 *)",
            job_names: Some(&["job-c"]),
            tags: None,
        },
    ];

    let emulator = Emulator::new();
    for job in ["job-a", "job-b", "job-c"] {
        assert_eq!(
            create_job(&emulator, job).await.status,
            200,
            "case {job:?}: the job a schedule names must be created"
        );
    }
    // One schedule nothing in the table deletes, so each delete can be shown to drop
    // only the record it addressed.
    assert_eq!(
        create_schedule(
            &emulator,
            json!({"Name": "untouched", "CronExpression": "cron(0 6 * * ? *)"}),
        )
        .await
        .status,
        200,
        "the bystander schedule must be created"
    );

    for DeleteCase {
        case,
        name,
        cron_expression,
        job_names,
        tags,
    } in cases
    {
        let mut body = json!({"Name": name, "CronExpression": cron_expression});
        if let Some(job_names) = job_names {
            body["JobNames"] = json!(job_names);
        }
        if let Some(tag_key) = tags {
            body["Tags"] = json!({tag_key: "data"});
        }
        let created = create_schedule(&emulator, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateSchedule to answer 200, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            describe_schedule(&emulator, name).await.status,
            200,
            "case {case:?}: the created schedule must resolve before the delete"
        );

        let deleted = delete_schedule(&emulator, name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected DeleteSchedule to answer 200, got {} {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.text("Name"),
            name,
            "case {case:?}: expected DeleteSchedule to echo the deleted name, got {:?}",
            deleted.body
        );

        // The record is gone, so every later read of that name misses.
        let described = describe_schedule(&emulator, name).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: expected DescribeSchedule after the delete to answer 404, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException after the delete, got {:?}",
            described.error_type
        );
        let updated = update_schedule(&emulator, name, "cron(0 1 * * ? *)").await;
        assert_eq!(
            updated.status, 404,
            "case {case:?}: expected UpdateSchedule after the delete to answer 404, got {} {:?}",
            updated.status, updated.body
        );
        // DeleteSchedule is not idempotent: the second delete addresses a name nothing
        // is stored under, so it misses exactly as the describe does.
        let again = delete_schedule(&emulator, name).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: expected a second DeleteSchedule to answer 404, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException on the second delete, got {:?}",
            again.error_type
        );
        assert_eq!(
            again.error_message(),
            schedule_not_found_message(name),
            "case {case:?}: expected the service's missing-schedule sentence, got {:?}",
            again.body
        );

        // A schedule only NAMES the jobs it runs, so the delete leaves them, and every
        // other schedule, as they were.
        for job in job_names.unwrap_or(&[]) {
            assert_eq!(
                describe_job(&emulator, job).await.status,
                200,
                "case {case:?}: deleting the schedule must leave job {job:?} alone"
            );
        }
        assert_eq!(
            describe_schedule(&emulator, "untouched").await.status,
            200,
            "case {case:?}: the delete must drop only the schedule it addressed"
        );
    }

    // The name the delete frees can be created again, and the new record is the one the
    // reads then serve.
    let recreated = create_schedule(
        &emulator,
        json!({"Name": "plain", "CronExpression": "cron(0 3 * * ? *)"}),
    )
    .await;
    assert_eq!(
        recreated.status, 200,
        "case \"re-create a deleted name\": expected 200, got {} {:?}",
        recreated.status, recreated.body
    );
    assert_eq!(
        describe_schedule(&emulator, "plain")
            .await
            .text("CronExpression"),
        "cron(0 3 * * ? *)",
        "case \"re-create a deleted name\": expected the new record's CronExpression back"
    );
}

/// A `DeleteSchedule` that addresses a name no schedule is stored under is refused with
/// the modelled `ResourceNotFoundException` (HTTP 404), naming the schedule the request
/// asked for, and nothing else in the store is touched.
///
/// The rows span the ways a name can resolve to no schedule: never created, the name of
/// a DIFFERENT resource kind (a job, a dataset), a name differing only in case from a
/// stored one, and a name of the documented character set nobody used.
#[tokio::test]
async fn deleting_a_schedule_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();
    assert_eq!(
        create_job(&emulator, "job-a").await.status,
        200,
        "the job the lookup must not confuse for a schedule has to exist"
    );
    assert_eq!(
        create_schedule(
            &emulator,
            json!({
                "Name": "nightly",
                "CronExpression": "cron(0 12 * * ? *)",
                "JobNames": ["job-a"],
            }),
        )
        .await
        .status,
        200,
        "the stored schedule the misses must not hit has to exist"
    );

    let cases = [
        ("a schedule nobody created", "ghost"),
        ("the name of an existing job", "job-a"),
        ("a stored schedule's name in another case", "NIGHTLY"),
        ("a name with a space", "never made"),
        ("a name with a period", "never.made"),
        ("a one-character name", "z"),
        ("a 255-character name, the modelled maximum", LONGEST_NAME),
    ];

    for (case, name) in cases {
        let refused = delete_schedule(&emulator, name).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            refused.error_message(),
            schedule_not_found_message(name),
            "case {case:?}: expected the service's missing-schedule sentence, got {:?}",
            refused.body
        );
    }

    // The refused deletes changed nothing.
    let described = describe_schedule(&emulator, "nightly").await;
    assert_eq!(
        described.status, 200,
        "the stored schedule must survive every refused delete, got {} {:?}",
        described.status, described.body
    );
    assert_eq!(
        described.member("JobNames"),
        json!(["job-a"]),
        "the stored schedule must be unchanged by the refused deletes, got {:?}",
        described.body["JobNames"]
    );
    assert_eq!(
        describe_job(&emulator, "job-a").await.status,
        200,
        "a DeleteSchedule naming a job must not delete that job"
    );
}

/// A name at `ScheduleName`'s modelled maximum (`len 1..255`, from
/// `smithy_query(query="operation", operation="DeleteSchedule")`), built from the
/// documented character set.
const LONGEST_NAME: &str = concat!(
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "aaaaa",
);
