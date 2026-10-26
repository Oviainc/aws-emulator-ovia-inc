// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateCrawlerSchedule`, and the `GetCrawler` that reads back
//! what it wrote.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: the request STATES the addressed crawler's schedule — the cron
//! expression it carries becomes the crawler's schedule, and a request carrying none
//! leaves the crawler with no schedule at all — the write moves the crawler's `Version`
//! and `LastUpdated` and touches nothing else about it, and an expression that is not the
//! documented `cron(..)` form is refused with the stored schedule left standing.
//!
//! The recorded workflow's own call — one crawler, rescheduled by a new cron expression —
//! is ONE row of the first table; the rows differ along whether the crawler already had a
//! schedule, the form of the expression (and whether one is sent at all), and how many
//! writes in a row reach the same crawler.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    messages, ComAmazonawsGlueAwsglueState, CRAWLER_STATE_READY, CRAWLER_STATE_RUNNING,
    CRAWLER_STATE_STOPPING, FIRST_CRAWLER_VERSION, SCHEDULE_STATE_SCHEDULED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok};

/// The role every crawler here carries: a schedule write never touches it, so it is one
/// of the members each row reads back unchanged.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-schedules";

/// Cron expressions the service accepts: `cron(..)` around the six fields the
/// documentation spells out — "to run something every day at 12:15 UTC, you would
/// specify: `cron(15 12 * * ? *)`" (`Schedule$ScheduleExpression`, the shape
/// `UpdateCrawlerScheduleRequest$Schedule` targets). The first row is the documented
/// example; the rest vary the fields the issue's own call did not: names rather than
/// numbers in the day-of-week field, a step value, a list, and a range in the year field.
const CRON_EXPRESSIONS: &[(&str, &str)] = &[
    ("the documented example", "cron(15 12 * * ? *)"),
    ("every hour on the hour", "cron(0 * * * ? *)"),
    ("weekdays by name", "cron(30 2 ? * MON-FRI *)"),
    ("a step in the minute field", "cron(0/15 * * * ? *)"),
    ("a list of months", "cron(0 6 1 JAN,APR,JUL,OCT ? *)"),
    ("a range of years", "cron(45 23 L * ? 2026-2030)"),
];

/// A `CreateCrawler` body for `name`, with `schedule` when one is given. Every other
/// member is the same across the rows, so what a schedule write leaves alone is
/// observable rather than merely absent.
fn crawler_body(name: &str, schedule: Option<&str>) -> Value {
    let mut body = json!({
        "Name": name,
        "Role": ROLE_ARN,
        "DatabaseName": "analytics",
        "Description": "a crawler whose schedule is written through UpdateCrawlerSchedule",
        "Targets": { "S3Targets": [{ "Path": "s3://crawler-bucket/data/" }] },
        "Classifiers": ["csv-classifier"],
        "TablePrefix": "scheduled_",
    });
    if let Some(schedule) = schedule {
        body["Schedule"] = json!(schedule);
    }
    body
}

/// The `Crawler` structure `GetCrawler` reports for `name`.
async fn get_crawler(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetCrawler", &json!({ "Name": name })).await;
    body.get("Crawler")
        .cloned()
        .unwrap_or_else(|| panic!("GetCrawler must report a Crawler, got {body}"))
}

/// Create `name` (with `schedule`, if any) and report what `GetCrawler` reads back.
async fn create_crawler(
    router: &MultiServiceRouter,
    name: &str,
    schedule: Option<&str>,
) -> Value {
    ok(router, "CreateCrawler", &crawler_body(name, schedule)).await;
    get_crawler(router, name).await
}

/// One `UpdateCrawlerSchedule` call: the body carries `Schedule` only when one is given,
/// because an ABSENT `Schedule` is what states "this crawler has no schedule".
fn schedule_request(name: &str, schedule: Option<&str>) -> Value {
    let mut request = json!({ "CrawlerName": name });
    if let Some(schedule) = schedule {
        request["Schedule"] = json!(schedule);
    }
    request
}

/// The `Schedule` a crawler scheduled by `expression` reports, or no member at all when
/// the crawler has no schedule.
fn expected_schedule(expression: Option<&str>) -> Option<Value> {
    expression.map(|expression| {
        json!({
            "ScheduleExpression": expression,
            "State": SCHEDULE_STATE_SCHEDULED,
        })
    })
}

/// The `LastUpdated` of a reported crawler, as epoch seconds.
fn last_updated(case: &str, crawler: &Value) -> f64 {
    crawler
        .get("LastUpdated")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case}: expected a LastUpdated, got {crawler}"))
}

/// Epoch seconds of a wall-clock instant, for bounding a server-set timestamp.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs_f64()
}

/// Assert that `after` is `before` with the schedule `expression` states, one more
/// version, and a `LastUpdated` within the window the write happened in — and that
/// nothing else about the crawler moved.
fn assert_schedule_written(
    case: &str,
    before: &Value,
    after: &Value,
    expression: Option<&str>,
    started: SystemTime,
    finished: SystemTime,
) {
    assert_eq!(
        after.get("Schedule").cloned(),
        expected_schedule(expression),
        "case {case}: expected the schedule {expression:?}, got {after}"
    );

    let expected_version = before
        .get("Version")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case}: expected a Version, got {before}"))
        + 1;
    assert_eq!(
        after.get("Version").and_then(Value::as_i64),
        Some(expected_version),
        "case {case}: expected Version {expected_version}, got {after}"
    );

    let moved = last_updated(case, after);
    let floor = epoch_seconds(started).floor();
    let ceiling = epoch_seconds(finished).ceil();
    assert!(
        (floor..=ceiling).contains(&moved),
        "case {case}: expected a LastUpdated within [{floor}, {ceiling}], got {moved}"
    );
    assert!(
        moved >= last_updated(case, before),
        "case {case}: LastUpdated must not go backwards, got {moved} after {}",
        last_updated(case, before)
    );

    // Everything the request says nothing about is exactly what it was, `CreationTime`
    // and the crawler's run state included.
    let mut expected = before.clone();
    let members = expected.as_object_mut().expect("a Crawler is a structure");
    members.remove("Schedule");
    if let Some(schedule) = expected_schedule(expression) {
        members.insert("Schedule".to_string(), schedule);
    }
    members.insert("Version".to_string(), json!(expected_version));
    members.insert("LastUpdated".to_string(), after["LastUpdated"].clone());
    assert_eq!(
        after, &expected,
        "case {case}: expected {expected}, got {after}"
    );
}

// ---------------------------------------------------------------------------
// The write
// ---------------------------------------------------------------------------

/// A schedule write states the crawler's schedule: the expression it carries becomes the
/// schedule whatever the crawler was scheduled by before, and a request carrying none
/// takes the schedule off the crawler.
///
/// The rows differ along whether the crawler already had a schedule and along the
/// expression the request states (every accepted form, and no expression at all).
#[tokio::test]
async fn a_schedule_write_states_the_crawlers_schedule() {
    let router = emulator();

    // Each accepted expression, written onto a crawler created WITH a schedule (the
    // recorded workflow's own shape) and onto one created without.
    let mut cases: Vec<(String, Option<&str>, Option<&str>)> = Vec::new();
    for (form, expression) in CRON_EXPRESSIONS {
        cases.push((
            format!("{form}, replacing a schedule"),
            Some("cron(15 12 * * ? *)"),
            Some(*expression),
        ));
        cases.push((
            format!("{form}, on a crawler with no schedule"),
            None,
            Some(*expression),
        ));
    }
    // No expression at all: the crawler is left with no schedule, whether it had one or
    // not.
    cases.push((
        "no expression, taking a schedule off".to_string(),
        Some("cron(0 1 * * ? *)"),
        None,
    ));
    cases.push((
        "no expression, on a crawler with no schedule".to_string(),
        None,
        None,
    ));

    for (index, (case, before_schedule, written)) in cases.iter().enumerate() {
        let name = format!("schedule-{index}");
        let before = create_crawler(&router, &name, *before_schedule).await;
        assert_eq!(
            before.get("Schedule").cloned(),
            expected_schedule(*before_schedule),
            "case {case}: the create must store the schedule it was given, got {before}"
        );

        let started = SystemTime::now();
        let answer = ok(
            &router,
            "UpdateCrawlerSchedule",
            &schedule_request(&name, *written),
        )
        .await;
        assert_eq!(
            answer,
            json!({}),
            "case {case}: UpdateCrawlerSchedule answers an empty structure, got {answer}"
        );

        let after = get_crawler(&router, &name).await;
        assert_schedule_written(
            case,
            &before,
            &after,
            *written,
            started,
            SystemTime::now(),
        );
    }
}

/// Each write lands on top of the last one and advances the version by one: the schedule
/// a read reports is the one the most recent write stated, across a run of writes that
/// set, replace, remove and set it again.
#[tokio::test]
async fn successive_writes_each_land_on_the_last() {
    let router = emulator();
    let before = create_crawler(&router, "resequenced", None).await;
    assert_eq!(
        before.get("Version").and_then(Value::as_i64),
        Some(FIRST_CRAWLER_VERSION),
        "case fresh crawler: expected Version {FIRST_CRAWLER_VERSION}, got {before}"
    );

    let sequence: &[Option<&str>] = &[
        Some("cron(15 12 * * ? *)"),
        Some("cron(30 2 ? * MON-FRI *)"),
        None,
        None,
        Some("cron(0 * * * ? *)"),
    ];

    let mut previous = before;
    for (round, written) in sequence.iter().enumerate() {
        let case = format!("round {round} writing {written:?}");
        let started = SystemTime::now();
        ok(
            &router,
            "UpdateCrawlerSchedule",
            &schedule_request("resequenced", *written),
        )
        .await;
        let after = get_crawler(&router, "resequenced").await;
        assert_schedule_written(
            &case,
            &previous,
            &after,
            *written,
            started,
            SystemTime::now(),
        );
        previous = after;
    }

    let expected_version = FIRST_CRAWLER_VERSION + sequence.len() as i64;
    assert_eq!(
        previous.get("Version").and_then(Value::as_i64),
        Some(expected_version),
        "case the whole run: expected Version {expected_version}, got {previous}"
    );
}

/// The schedule a write stores is the same member `UpdateCrawler` writes: a crawler
/// rescheduled through either operation reads back the same way, and a later
/// `UpdateCrawler` that names no schedule leaves what this one wrote standing.
#[tokio::test]
async fn the_schedule_written_here_is_the_crawlers_own_schedule_member() {
    let router = emulator();
    create_crawler(&router, "shared-member", Some("cron(15 12 * * ? *)")).await;

    ok(
        &router,
        "UpdateCrawlerSchedule",
        &schedule_request("shared-member", Some("cron(0 3 * * ? *)")),
    )
    .await;
    let after_schedule_write = get_crawler(&router, "shared-member").await;
    assert_eq!(
        after_schedule_write.get("Schedule").cloned(),
        expected_schedule(Some("cron(0 3 * * ? *)")),
        "case written here: expected the schedule this call stated, got {after_schedule_write}"
    );

    // An update that names other members does not disturb it.
    ok(
        &router,
        "UpdateCrawler",
        &json!({ "Name": "shared-member", "TablePrefix": "after_" }),
    )
    .await;
    let after_update = get_crawler(&router, "shared-member").await;
    assert_eq!(
        after_update.get("Schedule").cloned(),
        expected_schedule(Some("cron(0 3 * * ? *)")),
        "case preserved by UpdateCrawler: expected the schedule to stand, got {after_update}"
    );

    // And a schedule `UpdateCrawler` writes is read back exactly as one written here.
    ok(
        &router,
        "UpdateCrawler",
        &json!({ "Name": "shared-member", "Schedule": "cron(0 4 * * ? *)" }),
    )
    .await;
    let after_merge = get_crawler(&router, "shared-member").await;
    assert_eq!(
        after_merge.get("Schedule").cloned(),
        expected_schedule(Some("cron(0 4 * * ? *)")),
        "case written through UpdateCrawler: expected the same reported shape, got {after_merge}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Writing the schedule of a crawler this account does not hold is the modelled
/// not-found refusal, and it creates nothing: the rows span a name never created and
/// names that differ from a stored crawler's only in case, because the model documents no
/// case fold for a crawler name.
#[tokio::test]
async fn a_write_against_an_absent_crawler_is_the_modelled_miss() {
    let router = emulator();
    create_crawler(&router, "present-crawler", Some("cron(15 12 * * ? *)")).await;

    for (case, name) in [
        ("never created", "absent-crawler"),
        ("differs only in case", "PRESENT-CRAWLER"),
        ("differs only in one letter's case", "present-Crawler"),
    ] {
        for (shape, written) in [
            ("with an expression", Some("cron(0 1 * * ? *)")),
            ("with none", None),
        ] {
            let (status, body) = call(
                &router,
                "UpdateCrawlerSchedule",
                &schedule_request(name, written),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case} {shape}: expected 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("EntityNotFoundException"),
                "case {case} {shape}: expected EntityNotFoundException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                messages::crawler_not_found(name),
                "case {case} {shape}: expected the crawler miss for {name}, got {body}"
            );

            // The refused write created nothing under that name.
            let (status, body) = call(&router, "GetCrawler", &json!({ "Name": name })).await;
            assert_eq!(
                status, 400,
                "case {case} {shape}: a refused write must not create a crawler, got {status} \
                 with {body}"
            );
        }
    }
}

/// An expression that is not a `cron(..)` of six fields is not a schedule this service
/// could run, so it is refused with the modelled input error and the stored crawler — its
/// schedule and its `Version` included — is untouched.
///
/// The rows span the ways an expression can miss that form: the fields with no `cron(..)`
/// around them, another expression syntax, too few and too many fields, an unclosed
/// expression, and the empty and blank strings (which do NOT mean "no schedule": the
/// absence of the member is what states that, and it is tested above).
#[tokio::test]
async fn an_expression_that_is_not_a_cron_expression_is_refused() {
    let router = emulator();
    let stored = create_crawler(&router, "unchanged", Some("cron(15 12 * * ? *)")).await;

    for (case, expression) in [
        ("bare cron body", "15 12 * * ? *"),
        ("a rate expression", "rate(5 minutes)"),
        ("too few cron fields", "cron(15 12 * * ?)"),
        ("too many cron fields", "cron(15 12 * * ? * *)"),
        ("unclosed cron", "cron(15 12 * * ? *"),
        ("no fields at all", "cron()"),
        ("the empty expression", ""),
        ("a blank expression", "   "),
    ] {
        let (status, body) = call(
            &router,
            "UpdateCrawlerSchedule",
            &schedule_request("unchanged", Some(expression)),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::schedule_not_a_cron_expression(expression),
            "case {case}: expected the non-cron refusal for {expression}, got {body}"
        );

        let after = get_crawler(&router, "unchanged").await;
        assert_eq!(
            after, stored,
            "case {case}: the stored crawler must be unchanged, expected {stored}, got {after}"
        );
    }
}

/// A crawl in progress does not stand in the way of a schedule write: this operation
/// models no `CrawlerRunningException` (unlike `UpdateCrawler`, which does and refuses),
/// so the write lands in every `CrawlerState` and leaves that state where it was.
///
/// The crawler is put into each state directly, because the operations that move it there
/// are not implemented yet; the rule under test is that the write is served, not how the
/// state got there.
#[tokio::test]
async fn a_crawl_in_progress_does_not_refuse_a_schedule_write() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    create_crawler(&router, "busy", Some("cron(15 12 * * ? *)")).await;

    /// Put the stored crawler into `state`.
    fn set_state(engine: &Arc<engine::InMemoryEngine>, name: &str, state: &str) {
        let store = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = store.lock();
        let entry = store
            .crawlers
            .get_mut(name)
            .unwrap_or_else(|| panic!("the suite created {name}"));
        entry.state = state.to_string();
    }

    for (round, case) in [
        CRAWLER_STATE_RUNNING,
        CRAWLER_STATE_STOPPING,
        CRAWLER_STATE_READY,
    ]
    .iter()
    .enumerate()
    {
        set_state(&engine, "busy", case);
        let before = get_crawler(&router, "busy").await;
        let written = format!("cron(0 {round} * * ? *)");

        let started = SystemTime::now();
        ok(
            &router,
            "UpdateCrawlerSchedule",
            &schedule_request("busy", Some(&written)),
        )
        .await;
        let after = get_crawler(&router, "busy").await;
        assert_schedule_written(
            &format!("state {case}"),
            &before,
            &after,
            Some(&written),
            started,
            SystemTime::now(),
        );
        assert_eq!(
            after.get("State").and_then(Value::as_str),
            Some(*case),
            "case state {case}: a schedule write does not move the crawler's run state, got \
             {after}"
        );
    }
}
