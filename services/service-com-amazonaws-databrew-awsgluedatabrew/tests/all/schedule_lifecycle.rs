// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for DataBrew schedules: `CreateSchedule` -> `DescribeSchedule`.
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch` with a
//! restJson1 request), so what is asserted is the HTTP status, the error code and the
//! JSON body a client sees.
//!
//! The tables below span the class the implementation generalises over rather than one
//! recorded request: every documented form of a DataBrew cron expression, every one of
//! its six fields at its published bounds and one past them, and every shape of a
//! `JobNames` list (absent, empty, one job, several, one unknown among known ones).

use serde_json::{json, Value};

use crate::databrew_harness::{job_outputs, label, Emulator, Reply, ARN_PREFIX, CALLER_PRINCIPAL};

/// Create a recipe job a schedule can name. `RoleArn` and one selected output are the
/// only other members `CreateRecipeJob` requires.
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

/// `DescribeSchedule`: `GET /schedules/{Name}`.
async fn describe_schedule(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call("GET", &format!("/schedules/{}", label(name)), None)
        .await
}

/// The `Message` of a modelled DataBrew error, whichever casing the protocol put it
/// on the wire with.
fn error_message(reply: &Reply) -> String {
    reply
        .body
        .get("Message")
        .or_else(|| reply.body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The one sentence DataBrew answers every invalid schedule cron expression with
/// (`INVALID_CRON_EXPRESSION_MESSAGE` in `src/state.rs`).
const INVALID_CRON_MESSAGE: &str =
    "Invalid schedule cron expression. Expected format: Cron(CRON_EXPRESSION)";

/// The cron expression forms DataBrew documents, each one accepted and each one stored
/// verbatim.
///
/// Source: the Glue DataBrew Developer Guide's "Cron expressions" page, which the
/// model's `CreateScheduleRequest$CronExpression` doc links to
/// (<https://docs.aws.amazon.com/databrew/latest/dg/jobs.cron.html>):
/// `cron(minutes hours day-of-month month day-of-week year)`, with the `*`, `-`, `,`,
/// `/`, `?`, `L`, `W` and `#` wildcards. The rows below are the guide's own examples
/// plus one row per wildcard it lists, none of which the issue cited.
#[tokio::test]
async fn a_schedule_round_trips_every_documented_cron_form() {
    let cases = [
        ("every day at noon", "cron(0 12 * * ? *)"),
        ("the guide's last-Friday example", "cron(15 10 ? * 6L 2019-2022)"),
        ("a step over a range of hours", "cron(0/5 8-17 ? * MON-FRI *)"),
        ("named days", "cron(0 18 ? * MON-FRI *)"),
        ("a list of values", "cron(0,30 6,18 1,15 * ? *)"),
        ("the last day of the month", "cron(59 23 L * ? *)"),
        ("the nearest weekday to the 15th", "cron(0 8 15W * ? *)"),
        ("the first Sunday of January", "cron(0 0 ? JAN SUN#1 *)"),
        ("the last weekday of the month", "cron(0 0 LW * ? *)"),
        ("a step over every value", "cron(*/15 * * * ? *)"),
    ];

    let emulator = Emulator::new();
    assert_eq!(
        create_job(&emulator, "nightly-job").await.status,
        200,
        "the job a schedule names must be created"
    );

    for (index, (case, cron_expression)) in cases.iter().enumerate() {
        let name = format!("schedule-{index}");
        let created = create_schedule(
            &emulator,
            json!({
                "Name": name,
                "CronExpression": cron_expression,
                "JobNames": ["nightly-job"],
                "Tags": {"team": "data"},
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?} ({cron_expression}): expected CreateSchedule to answer 200, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.text("Name"),
            name,
            "case {case:?}: CreateSchedule must echo the name it stored, got {:?}",
            created.body
        );

        let described = describe_schedule(&emulator, &name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeSchedule to answer 200, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.text("CronExpression"),
            *cron_expression,
            "case {case:?}: expected the stored CronExpression back, got {:?}",
            described.body["CronExpression"]
        );
        assert_eq!(
            described.member("JobNames"),
            json!(["nightly-job"]),
            "case {case:?}: expected the stored JobNames back, got {:?}",
            described.body["JobNames"]
        );
        assert_eq!(
            described.member("Tags"),
            json!({"team": "data"}),
            "case {case:?}: expected the stored Tags back, got {:?}",
            described.body["Tags"]
        );
        assert_eq!(
            described.text("ResourceArn"),
            format!("{ARN_PREFIX}schedule/{name}"),
            "case {case:?}: unexpected ResourceArn, got {:?}",
            described.body["ResourceArn"]
        );
        assert!(
            described.body["CreateDate"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: expected a server-generated CreateDate, got {:?}",
            described.body["CreateDate"]
        );
        assert_eq!(
            described.text("CreatedBy"),
            CALLER_PRINCIPAL,
            "case {case:?}: expected CreatedBy to name the calling principal, got {:?}",
            described.body["CreatedBy"]
        );
        // A schedule that has only been created has no modification to report, and the
        // model makes both members optional.
        assert!(
            described.is_absent("LastModifiedDate"),
            "case {case:?}: an unmodified schedule must report no LastModifiedDate, got {:?}",
            described.body["LastModifiedDate"]
        );
        assert!(
            described.is_absent("LastModifiedBy"),
            "case {case:?}: an unmodified schedule must report no LastModifiedBy, got {:?}",
            described.body["LastModifiedBy"]
        );
    }
}

/// An expression that is not a cron expression of the documented form is refused with
/// the modelled `ValidationException` (HTTP 400 — the only client error
/// `CreateSchedule` models for a bad input), and the refused create stores nothing.
///
/// The rows span the ways an expression can fail the form: no `cron(..)` around the
/// fields, another expression language, free text, the wrong field count, a field
/// outside its published range, a malformed wildcard, and the guide's limit that `?`
/// stands in exactly one of day-of-month and day-of-week.
#[tokio::test]
async fn an_expression_outside_the_documented_cron_form_is_refused() {
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
        ("? in a field that has no ?", "cron(? 12 * * ? *)"),
        ("a reversed range", "cron(30-5 12 * * ? *)"),
        ("a zero step", "cron(0/0 12 * * ? *)"),
        ("an empty list entry", "cron(0,,30 12 * * ? *)"),
        ("a signed value", "cron(+5 12 * * ? *)"),
        ("an unknown month alias", "cron(0 12 * MOO ? *)"),
        ("a nth-day past the fifth", "cron(0 12 ? * 6#6 *)"),
        ("W where the field has no W", "cron(0 12 ? * 3W *)"),
        ("a non-numeric year", "cron(0 12 * * ? MON)"),
    ];

    let emulator = Emulator::new();
    for (index, (case, cron_expression)) in cases.iter().enumerate() {
        let name = format!("refused-{index}");
        let refused = create_schedule(
            &emulator,
            json!({"Name": name, "CronExpression": cron_expression}),
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
            error_message(&refused),
            INVALID_CRON_MESSAGE,
            "case {case:?} ({cron_expression}): expected the service's invalid-cron message, got {:?}",
            refused.body
        );
        assert_eq!(
            describe_schedule(&emulator, &name).await.status,
            404,
            "case {case:?}: the refused create must store nothing"
        );
    }
}

/// Every cron field accepts its published range and nothing outside it.
///
/// Source: the guide's field table (mirrored by `CRON_FIELDS` in `src/state.rs`):
/// minutes 0-59, hours 0-23, day-of-month 1-31, month 1-12, day-of-week 1-7, year
/// 1970-2199. Each field is tried at both bounds and at one value past each.
#[tokio::test]
async fn each_cron_field_accepts_exactly_its_published_range() {
    let fields = [
        (0usize, "minutes", 0i64, 59i64),
        (1, "hours", 0, 23),
        (2, "day-of-month", 1, 31),
        (3, "month", 1, 12),
        (4, "day-of-week", 1, 7),
        (5, "year", 1970, 2199),
    ];

    /// One expression with `value` in field `index`, every other field a value the
    /// guide accepts. The day-of-week row has to leave day-of-month as `?`, since the
    /// guide allows a value in only one of the two.
    fn expression(index: usize, value: i64) -> String {
        let mut parts = ["0", "0", "1", "1", "?", "2024"].map(String::from);
        if index == 4 {
            parts[2] = "?".to_string();
        }
        parts[index] = value.to_string();
        format!("cron({})", parts.join(" "))
    }

    let emulator = Emulator::new();
    for (index, field, min, max) in fields {
        for (bound, value) in [("min", min), ("max", max)] {
            let name = format!("{field}-{bound}");
            let cron_expression = expression(index, value);
            let created = create_schedule(
                &emulator,
                json!({"Name": name, "CronExpression": cron_expression}),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {field:?} at its {bound} ({cron_expression}): expected 200, got {} {:?}",
                created.status, created.body
            );
            assert_eq!(
                describe_schedule(&emulator, &name)
                    .await
                    .text("CronExpression"),
                cron_expression,
                "case {field:?} at its {bound}: expected the stored CronExpression back"
            );
        }

        for (bound, value) in [("below min", min - 1), ("above max", max + 1)] {
            let name = format!("{field}-{bound}");
            let cron_expression = expression(index, value);
            let refused = create_schedule(
                &emulator,
                json!({"Name": name, "CronExpression": cron_expression}),
            )
            .await;
            assert_eq!(
                refused.status, 400,
                "case {field:?} {bound} ({cron_expression}): expected 400, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type.as_deref(),
                Some("ValidationException"),
                "case {field:?} {bound} ({cron_expression}): expected ValidationException, got {:?}",
                refused.error_type
            );
            assert_eq!(
                error_message(&refused),
                INVALID_CRON_MESSAGE,
                "case {field:?} {bound} ({cron_expression}): expected the service's invalid-cron message, got {:?}",
                refused.body
            );
        }
    }
}

/// A schedule runs the jobs it names, so it can only name jobs this account holds: a
/// name that resolves to no job is refused with the modelled `ValidationException`
/// (`CreateSchedule` models no `ResourceNotFoundException`), and the stored list reads
/// back exactly as given — including the empty list and no list at all, which the
/// modelled `JobNameList` `len 0..50` allows.
#[tokio::test]
async fn a_schedule_can_only_name_jobs_that_exist() {
    let emulator = Emulator::new();
    for job in ["job-a", "job-b"] {
        assert_eq!(
            create_job(&emulator, job).await.status,
            200,
            "case {job:?}: the job must be created"
        );
    }

    let accepted: [(&str, Option<Value>, Value); 4] = [
        ("no JobNames member", None, json!([])),
        ("an empty JobNames list", Some(json!([])), json!([])),
        ("one job", Some(json!(["job-a"])), json!(["job-a"])),
        (
            "several jobs, in order",
            Some(json!(["job-b", "job-a"])),
            json!(["job-b", "job-a"]),
        ),
    ];

    for (index, (case, job_names, expected)) in accepted.into_iter().enumerate() {
        let name = format!("named-{index}");
        let mut body = json!({"Name": name, "CronExpression": "cron(0 12 * * ? *)"});
        if let Some(job_names) = job_names {
            body["JobNames"] = job_names;
        }
        let created = create_schedule(&emulator, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            created.status, created.body
        );
        let described = describe_schedule(&emulator, &name).await;
        assert_eq!(
            described.member("JobNames"),
            expected,
            "case {case:?}: expected JobNames {expected:?}, got {:?}",
            described.body["JobNames"]
        );
    }

    let refused_cases = [
        ("one unknown job", json!(["no-such-job"])),
        ("an unknown job among known ones", json!(["job-a", "no-such-job"])),
    ];
    for (index, (case, job_names)) in refused_cases.into_iter().enumerate() {
        let name = format!("unknown-{index}");
        let refused = create_schedule(
            &emulator,
            json!({
                "Name": name,
                "CronExpression": "cron(0 12 * * ? *)",
                "JobNames": job_names,
            }),
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
            describe_schedule(&emulator, &name).await.status,
            404,
            "case {case:?}: the refused create must store nothing"
        );
    }
}

/// A schedule name is unique and comes from the documented character set, and a
/// schedule nobody created resolves nothing.
#[tokio::test]
async fn a_schedule_name_is_unique_and_from_the_documented_character_set() {
    let emulator = Emulator::new();

    assert_eq!(
        describe_schedule(&emulator, "ghost").await.status,
        404,
        "case \"a schedule nobody created\": expected 404"
    );
    assert_eq!(
        describe_schedule(&emulator, "ghost")
            .await
            .error_type
            .as_deref(),
        Some("ResourceNotFoundException"),
        "case \"a schedule nobody created\": expected ResourceNotFoundException"
    );

    // The documented character set: "Valid characters are alphanumeric (A-Z, a-z,
    // 0-9), hyphen (-), period (.), and space"
    // (`smithy_query(query="doc", shape="CreateScheduleRequest", member="Name")`).
    for name in ["plain", "with-hyphen", "with.period", "with space", "MiXeD123"] {
        let created = create_schedule(
            &emulator,
            json!({"Name": name, "CronExpression": "cron(0 12 * * ? *)"}),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: a name in the documented set must be accepted, got {} {:?}",
            created.status, created.body
        );
    }
    for name in ["under_score", "with/slash", "with$dollar", "   "] {
        let refused = create_schedule(
            &emulator,
            json!({"Name": name, "CronExpression": "cron(0 12 * * ? *)"}),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: a name outside the documented set must be refused, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {name:?}: expected ValidationException, got {:?}",
            refused.error_type
        );
    }

    // A second create of the same name conflicts, and leaves the stored schedule as
    // the first create wrote it.
    let duplicate = create_schedule(
        &emulator,
        json!({"Name": "plain", "CronExpression": "cron(0 1 * * ? *)"}),
    )
    .await;
    assert_eq!(
        duplicate.status, 409,
        "case \"duplicate schedule name\": expected 409, got {} {:?}",
        duplicate.status, duplicate.body
    );
    assert_eq!(
        duplicate.error_type.as_deref(),
        Some("ConflictException"),
        "case \"duplicate schedule name\": expected ConflictException, got {:?}",
        duplicate.error_type
    );
    assert_eq!(
        describe_schedule(&emulator, "plain")
            .await
            .text("CronExpression"),
        "cron(0 12 * * ? *)",
        "case \"duplicate schedule name\": the refused create must not overwrite the stored one"
    );
}
