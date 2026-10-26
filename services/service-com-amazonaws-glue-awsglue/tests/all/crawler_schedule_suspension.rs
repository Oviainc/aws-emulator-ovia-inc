// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StopCrawlerSchedule`, and the `GetCrawler`/`GetCrawlers` reads
//! that report what it wrote.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: suspending a crawler's schedule sets that crawler's schedule
//! state to `NOT_SCHEDULED` and decides nothing else — the cron expression stays on the
//! crawler, its definition `Version` and its run `State` stay where they were — and a
//! crawler with no schedule running (already suspended, or carrying no expression at all)
//! has no scheduler to stop, so it is refused.
//!
//! The recorded workflow's own call — one crawler, scheduled at create and then stopped —
//! is ONE row of the first table; the rows differ along the cron expression the crawler
//! was scheduled by, how the schedule got onto it, and the run state the crawler is in
//! when the schedule is suspended.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    messages, ComAmazonawsGlueAwsglueState, CRAWLER_STATE_READY, CRAWLER_STATE_RUNNING,
    CRAWLER_STATE_STOPPING, SCHEDULE_STATE_NOT_SCHEDULED, SCHEDULE_STATE_SCHEDULED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok};

/// The role every crawler here carries: suspending a schedule never touches it, so it is
/// one of the members each row reads back unchanged.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/AWSGlueServiceRole-suspend";

/// Cron expressions the service accepts: `cron(..)` around the six fields the
/// documentation spells out — "to run something every day at 12:15 UTC, you would
/// specify: `cron(15 12 * * ? *)`" (`Schedule$ScheduleExpression`, the shape
/// `CreateCrawlerRequest$Schedule` and `UpdateCrawlerScheduleRequest$Schedule` target).
/// The first row is the documented example; the rest vary the fields the issue's own call
/// did not: a step value, names rather than numbers in the day-of-week field, a list of
/// months, and a range in the year field.
const CRON_EXPRESSIONS: &[(&str, &str)] = &[
    ("the documented example", "cron(15 12 * * ? *)"),
    ("every hour on the hour", "cron(0 * * * ? *)"),
    ("a step in the minute field", "cron(0/15 * * * ? *)"),
    ("weekdays by name", "cron(30 2 ? * MON-FRI *)"),
    ("a list of months", "cron(0 6 1 JAN,APR,JUL,OCT ? *)"),
    ("a range of years", "cron(45 23 L * ? 2026-2030)"),
];

/// How the schedule under test got onto the crawler: both writes state a schedule, so
/// both leave it in force and both are something a suspend has to be able to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScheduledBy {
    /// `CreateCrawler` carried the expression.
    Create,
    /// The create carried none and `UpdateCrawlerSchedule` wrote it afterwards.
    ScheduleWrite,
}

/// A `CreateCrawler` body for `name`, with `schedule` when one is given. Every other
/// member is the same across the rows, so what a suspend leaves alone is observable
/// rather than merely absent.
fn crawler_body(name: &str, schedule: Option<&str>) -> Value {
    let mut body = json!({
        "Name": name,
        "Role": ROLE_ARN,
        "DatabaseName": "analytics",
        "Description": "a crawler whose schedule is suspended through StopCrawlerSchedule",
        "Targets": { "S3Targets": [{ "Path": "s3://crawler-bucket/data/" }] },
        "Classifiers": ["csv-classifier"],
        "TablePrefix": "suspended_",
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

/// The same crawler as the paging read (`GetCrawlers`) reports it, which is how a row
/// checks the suspended state is held in the STORE rather than invented by one handler.
async fn get_crawler_through_the_listing(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetCrawlers", &json!({})).await;
    let crawlers = body
        .get("Crawlers")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("GetCrawlers must report a Crawlers list, got {body}"));
    crawlers
        .iter()
        .find(|crawler| crawler.get("Name").and_then(Value::as_str) == Some(name))
        .cloned()
        .unwrap_or_else(|| panic!("GetCrawlers must report {name}, got {body}"))
}

/// Create `name` and leave it scheduled by `expression`, the way `scheduled_by` says, and
/// report what `GetCrawler` reads back.
async fn scheduled_crawler(
    router: &MultiServiceRouter,
    name: &str,
    expression: &str,
    scheduled_by: ScheduledBy,
) -> Value {
    match scheduled_by {
        ScheduledBy::Create => {
            ok(router, "CreateCrawler", &crawler_body(name, Some(expression))).await;
        }
        ScheduledBy::ScheduleWrite => {
            ok(router, "CreateCrawler", &crawler_body(name, None)).await;
            ok(
                router,
                "UpdateCrawlerSchedule",
                &json!({ "CrawlerName": name, "Schedule": expression }),
            )
            .await;
        }
    }
    let crawler = get_crawler(router, name).await;
    assert_eq!(
        crawler.get("Schedule").cloned(),
        Some(json!({
            "ScheduleExpression": expression,
            "State": SCHEDULE_STATE_SCHEDULED,
        })),
        "case {scheduled_by:?} {expression}: a stated schedule is in force before the \
         suspend, got {crawler}"
    );
    crawler
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

/// Assert that `after` is `before` with its schedule suspended: the same cron expression
/// with the state `NOT_SCHEDULED`, a `LastUpdated` within the window the suspend happened
/// in, and every other member — `Version` and the run `State` included — untouched.
fn assert_schedule_suspended(
    case: &str,
    before: &Value,
    after: &Value,
    started: SystemTime,
    finished: SystemTime,
) {
    let expression = before
        .pointer("/Schedule/ScheduleExpression")
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the crawler was scheduled, got {before}"));
    assert_eq!(
        after.get("Schedule").cloned(),
        Some(json!({
            "ScheduleExpression": expression,
            "State": SCHEDULE_STATE_NOT_SCHEDULED,
        })),
        "case {case}: expected the expression kept and the state suspended, got {after}"
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

    // Everything the call says nothing about is exactly what it was: the definition's
    // `Version`, the crawler's run `State`, its `CreationTime` and every member the
    // create wrote.
    let mut expected = before.clone();
    let members = expected.as_object_mut().expect("a Crawler is a structure");
    members.insert(
        "Schedule".to_string(),
        after["Schedule"].clone(),
    );
    members.insert("LastUpdated".to_string(), after["LastUpdated"].clone());
    assert_eq!(
        after, &expected,
        "case {case}: expected {expected}, got {after}"
    );
}

// ---------------------------------------------------------------------------
// The suspend
// ---------------------------------------------------------------------------

/// Suspending a schedule sets the addressed crawler's schedule state to `NOT_SCHEDULED`
/// and leaves everything else about the crawler — its cron expression included — standing.
///
/// The rows differ along the expression the crawler was scheduled by and along which write
/// put that schedule in force.
#[tokio::test]
async fn a_suspend_sets_the_crawlers_schedule_state_to_not_scheduled() {
    let router = emulator();

    let mut cases: Vec<(String, &str, ScheduledBy)> = Vec::new();
    for (form, expression) in CRON_EXPRESSIONS {
        cases.push((
            format!("{form}, scheduled at create"),
            expression,
            ScheduledBy::Create,
        ));
        cases.push((
            format!("{form}, scheduled by a schedule write"),
            expression,
            ScheduledBy::ScheduleWrite,
        ));
    }

    for (index, (case, expression, scheduled_by)) in cases.iter().enumerate() {
        let name = format!("suspend-{index}");
        let before = scheduled_crawler(&router, &name, expression, *scheduled_by).await;

        let started = SystemTime::now();
        let answer = ok(
            &router,
            "StopCrawlerSchedule",
            &json!({ "CrawlerName": &name }),
        )
        .await;
        assert_eq!(
            answer,
            json!({}),
            "case {case}: StopCrawlerSchedule answers an empty structure, got {answer}"
        );

        let after = get_crawler(&router, &name).await;
        assert_schedule_suspended(case, &before, &after, started, SystemTime::now());

        // The suspended state is the store's, not one reader's.
        let listed = get_crawler_through_the_listing(&router, &name).await;
        assert_eq!(
            listed, after,
            "case {case}: the paging read must report the same crawler, expected {after}, \
             got {listed}"
        );
    }
}

/// A crawl in progress does not stand in the way of a suspend, and the suspend does not
/// stop it: the call is served in every `CrawlerState` and leaves that state where it was.
///
/// The crawler is put into each state directly, because the operations that move it there
/// are not implemented yet; the rule under test is that the suspend is served and decides
/// nothing about the run, not how the run state got there.
#[tokio::test]
async fn a_suspend_does_not_stop_a_crawler_that_is_running() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

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

    for (round, run_state) in [
        CRAWLER_STATE_RUNNING,
        CRAWLER_STATE_STOPPING,
        CRAWLER_STATE_READY,
    ]
    .iter()
    .enumerate()
    {
        let name = format!("busy-{round}");
        scheduled_crawler(
            &router,
            &name,
            "cron(15 12 * * ? *)",
            ScheduledBy::Create,
        )
        .await;
        set_state(&engine, &name, run_state);
        let before = get_crawler(&router, &name).await;

        let started = SystemTime::now();
        ok(
            &router,
            "StopCrawlerSchedule",
            &json!({ "CrawlerName": &name }),
        )
        .await;

        let after = get_crawler(&router, &name).await;
        let case = format!("run state {run_state}");
        assert_schedule_suspended(&case, &before, &after, started, SystemTime::now());
        assert_eq!(
            after.get("State").and_then(Value::as_str),
            Some(*run_state),
            "case {case}: a suspend does not move the crawler's run state, got {after}"
        );
    }
}

/// The expression a suspend leaves on the crawler is still the crawler's own `Schedule`
/// member: a schedule write that states an expression puts it back in force, and one that
/// states none takes it off a suspended crawler entirely.
#[tokio::test]
async fn a_schedule_write_after_a_suspend_states_the_schedule_again() {
    let router = emulator();

    for (case, restated, expected_schedule) in [
        (
            "an expression puts a schedule back in force",
            Some("cron(0 3 * * ? *)"),
            Some(json!({
                "ScheduleExpression": "cron(0 3 * * ? *)",
                "State": SCHEDULE_STATE_SCHEDULED,
            })),
        ),
        ("no expression takes the schedule off", None, None),
    ] {
        let name = format!("restated-{}", restated.unwrap_or("none"));
        scheduled_crawler(
            &router,
            &name,
            "cron(15 12 * * ? *)",
            ScheduledBy::Create,
        )
        .await;
        ok(
            &router,
            "StopCrawlerSchedule",
            &json!({ "CrawlerName": &name }),
        )
        .await;

        let mut request = json!({ "CrawlerName": &name });
        if let Some(restated) = restated {
            request["Schedule"] = json!(restated);
        }
        ok(&router, "UpdateCrawlerSchedule", &request).await;

        let after = get_crawler(&router, &name).await;
        assert_eq!(
            after.get("Schedule").cloned(),
            expected_schedule,
            "case {case}: expected {expected_schedule:?}, got {after}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One row of the not-running table: how a crawler gets into a state with no schedule
/// running for it.
struct NotRunningCase {
    /// What this row is, named in every assertion it makes.
    case: &'static str,
    /// The cron expression the create carried, if any.
    created_with: Option<&'static str>,
    /// The schedule a write states after the create, when a write is made at all:
    /// `Some(None)` states NO expression, which is how a write takes a schedule off.
    restated: Option<Option<&'static str>>,
    /// How many suspends are served before the refused one.
    served_suspends: usize,
}

/// A crawler with no schedule running has no scheduler to stop, so the call is the
/// modelled not-running refusal and the crawler is left exactly as it was.
///
/// The rows span the ways a schedule can fail to be running: a crawler created without
/// one, a crawler whose schedule a write took off, and a crawler whose schedule this very
/// operation already suspended — once, and then repeatedly, because a suspend is not a
/// call that can be made twice.
#[tokio::test]
async fn a_suspend_of_a_schedule_that_is_not_running_is_the_modelled_refusal() {
    let router = emulator();

    let cases = &[
        NotRunningCase {
            case: "created without a schedule",
            created_with: None,
            restated: None,
            served_suspends: 0,
        },
        NotRunningCase {
            case: "a schedule write took the schedule off",
            created_with: Some("cron(15 12 * * ? *)"),
            restated: Some(None),
            served_suspends: 0,
        },
        NotRunningCase {
            case: "already suspended once",
            created_with: Some("cron(0 * * * ? *)"),
            restated: None,
            served_suspends: 1,
        },
        NotRunningCase {
            case: "suspended, rescheduled and suspended again",
            created_with: Some("cron(0 * * * ? *)"),
            restated: Some(Some("cron(30 2 ? * MON-FRI *)")),
            served_suspends: 1,
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let NotRunningCase {
            case,
            created_with,
            restated,
            served_suspends,
        } = row;
        let name = format!("not-running-{index}");
        ok(&router, "CreateCrawler", &crawler_body(&name, *created_with)).await;
        if let Some(restated) = restated {
            let mut request = json!({ "CrawlerName": &name });
            if let Some(expression) = restated {
                request["Schedule"] = json!(expression);
            }
            ok(&router, "UpdateCrawlerSchedule", &request).await;
        }
        for _ in 0..*served_suspends {
            ok(
                &router,
                "StopCrawlerSchedule",
                &json!({ "CrawlerName": &name }),
            )
            .await;
        }

        let before = get_crawler(&router, &name).await;
        // Repeated refusals: the second and third call answer exactly as the first.
        for attempt in 0..3 {
            let (status, body) = call(
                &router,
                "StopCrawlerSchedule",
                &json!({ "CrawlerName": &name }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case} attempt {attempt}: expected 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("SchedulerNotRunningException"),
                "case {case} attempt {attempt}: expected SchedulerNotRunningException, got \
                 {body}"
            );
            assert_eq!(
                error_message(&body),
                messages::scheduler_not_running(&name),
                "case {case} attempt {attempt}: expected the not-running refusal for {name}, \
                 got {body}"
            );

            let after = get_crawler(&router, &name).await;
            assert_eq!(
                after, before,
                "case {case} attempt {attempt}: a refused suspend must change nothing, \
                 expected {before}, got {after}"
            );
        }
    }
}

/// Suspending the schedule of a crawler this account does not hold is the modelled
/// not-found refusal, and it creates nothing: the rows span a name never created and names
/// that differ from a stored crawler's only in case, because the model documents no case
/// fold for a crawler name.
#[tokio::test]
async fn a_suspend_against_an_absent_crawler_is_the_modelled_miss() {
    let router = emulator();
    scheduled_crawler(
        &router,
        "present-crawler",
        "cron(15 12 * * ? *)",
        ScheduledBy::Create,
    )
    .await;

    for (case, name) in [
        ("never created", "absent-crawler"),
        ("differs only in case", "PRESENT-CRAWLER"),
        ("differs only in one letter's case", "present-Crawler"),
        ("a name with a space", "present crawler"),
    ] {
        let (status, body) = call(&router, "StopCrawlerSchedule", &json!({ "CrawlerName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::crawler_not_found(name),
            "case {case}: expected the crawler miss for {name}, got {body}"
        );

        // The refused call created nothing under that name, and left the crawler it
        // nearly addressed scheduled.
        let (status, body) = call(&router, "GetCrawler", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: a refused suspend must not create a crawler, got {status} with \
             {body}"
        );
        let present = get_crawler(&router, "present-crawler").await;
        assert_eq!(
            present.pointer("/Schedule/State").and_then(Value::as_str),
            Some(SCHEDULE_STATE_SCHEDULED),
            "case {case}: the stored crawler's schedule must still be in force, got {present}"
        );
    }
}
