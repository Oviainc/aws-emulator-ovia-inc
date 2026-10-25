// SPDX-License-Identifier: CC0-1.0

//! What a `CreateScheduledReport` `clientToken` does, end to end over the wire.
//!
//! The rule under test: an `@idempotencyToken` makes a create idempotent, so a request
//! presenting a token an earlier create already recorded is answered with that create's
//! outcome — HTTP 200 with the ARN it minted — and creates no second resource. The
//! modelled `ConflictException` is kept for the conflict the model documents, "a
//! resource that already exists or is being created": a duplicate that races the create
//! the token started, and a token presented with a materially different request.
//!
//! Every assertion spans the class rather than one recorded request: the table below
//! walks the retry over each member a create request carries — the name, the dashboard
//! it reports on, the execution role, each part of the schedule, the description, the
//! widget ids, the date-range override and the tags — asserting the conflict where the
//! retry would have created a different report and the replay where it would have
//! created the same one, whatever order the tags arrived in and whether the request
//! spelled out the schedule state the service defaults to.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::{
    service_state, SCHEDULED_REPORT_CREATION_WINDOW,
};
use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::EmulatedService;

use super::dashboard_lifecycle::{call, error_code, ok_call, router};
use super::scheduled_report_lifecycle::{source_dashboard, EXECUTION_ROLE_ARN};

/// The token the first create presents and every retry below presents again. Every
/// character is in the printable range `ClientToken`'s pattern `^[\u{21}-\u{7e}]+$`
/// admits (length 1..128, `smithy_query(query="shape",shape="ClientToken")`).
const TOKEN: &str = "sdk-generated-token-4f2c";

/// Wait until the creation a token started is no longer in flight, so what comes next is
/// a retry rather than a racing duplicate. A little past the window, because the clock
/// the handler reads is the wall clock.
pub(crate) async fn wait_out_the_creation_window() {
    tokio::time::sleep(SCHEDULED_REPORT_CREATION_WINDOW + Duration::from_millis(150)).await;
}

/// The first request: every optional member of the create populated, `scheduleConfig.state`
/// deliberately left out so the rows below can both spell the default out (a replay) and
/// choose the other value (a conflict).
fn first_request(dashboard_arn: &str, widget_id: &str) -> Value {
    json!({
        "clientToken": TOKEN,
        "scheduledReport": {
            "name": "Idempotent Rollup",
            "dashboardArn": dashboard_arn,
            "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
            "scheduleConfig": {
                "scheduleExpression": "cron(0 8 1 * ? *)",
                "scheduleExpressionTimeZone": "Europe/Berlin",
                "schedulePeriod": {"startTime": 1_767_225_600, "endTime": 1_798_761_600}
            },
            "description": "Monthly spend rollup.",
            "widgetIds": [widget_id],
            "widgetDateRangeOverride": {
                "startTime": {"type": "RELATIVE", "value": "-P1M"},
                "endTime": {"type": "RELATIVE", "value": "P0D"}
            }
        },
        "resourceTags": [
            {"key": "team", "value": "finops"},
            {"key": "env", "value": "prod"}
        ]
    })
}

/// What the retry under the same token must be answered with.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Answer {
    /// The first call's outcome: HTTP 200 carrying the ARN that call minted.
    Replay,
    /// The modelled `ConflictException` (HTTP 409).
    Conflict,
}

/// One row: how the retry differs from the first request, and what that entitles it to.
struct RetryCase {
    label: &'static str,
    answer: Answer,
    /// Rewrites the first request into the retry. The second argument is the ARN of
    /// another dashboard this account stores, so a row can point the retry elsewhere.
    retry: fn(&mut Value, &str),
}

/// The members of `scheduledReport` of a request under rewrite.
fn report_mut(request: &mut Value) -> &mut serde_json::Map<String, Value> {
    request["scheduledReport"]
        .as_object_mut()
        .expect("the request carries a scheduledReport object")
}

fn cases() -> Vec<RetryCase> {
    vec![
        RetryCase {
            label: "the identical request an SDK transport retry resends",
            answer: Answer::Replay,
            retry: |_request, _other| {},
        },
        RetryCase {
            label: "the same tags in the other order",
            answer: Answer::Replay,
            retry: |request, _other| {
                let tags = request["resourceTags"]
                    .as_array_mut()
                    .expect("resourceTags is a list");
                tags.reverse();
            },
        },
        RetryCase {
            label: "one tag key repeated with the value it settles on",
            answer: Answer::Replay,
            retry: |request, _other| {
                request["resourceTags"] = json!([
                    {"key": "team", "value": "platform"},
                    {"key": "team", "value": "finops"},
                    {"key": "env", "value": "prod"}
                ]);
            },
        },
        RetryCase {
            label: "the default schedule state spelled out instead of omitted",
            answer: Answer::Replay,
            retry: |request, _other| {
                // `ScheduleState` values, source: the `ScheduleState` enum in the model;
                // a create that omits `state` stores ENABLED.
                request["scheduledReport"]["scheduleConfig"]["state"] = json!("ENABLED");
            },
        },
        RetryCase {
            label: "a different report name",
            answer: Answer::Conflict,
            retry: |request, _other| {
                request["scheduledReport"]["name"] = json!("Idempotent Rollup 2");
            },
        },
        RetryCase {
            label: "a different dashboard to report on",
            answer: Answer::Conflict,
            retry: |request, other| {
                request["scheduledReport"]["dashboardArn"] = json!(other);
            },
        },
        RetryCase {
            label: "a different execution role",
            answer: Answer::Conflict,
            retry: |request, _other| {
                request["scheduledReport"]["scheduledReportExecutionRoleArn"] =
                    json!("arn:aws:iam::123456789012:role/OtherReportRole");
            },
        },
        RetryCase {
            label: "a different schedule expression",
            answer: Answer::Conflict,
            retry: |request, _other| {
                request["scheduledReport"]["scheduleConfig"]["scheduleExpression"] =
                    json!("rate(7 days)");
            },
        },
        RetryCase {
            label: "a different schedule time zone",
            answer: Answer::Conflict,
            retry: |request, _other| {
                request["scheduledReport"]["scheduleConfig"]["scheduleExpressionTimeZone"] =
                    json!("America/New_York");
            },
        },
        RetryCase {
            label: "a different schedule period",
            answer: Answer::Conflict,
            retry: |request, _other| {
                request["scheduledReport"]["scheduleConfig"]["schedulePeriod"]["endTime"] =
                    json!(1_830_297_600);
            },
        },
        RetryCase {
            label: "the other schedule state",
            answer: Answer::Conflict,
            retry: |request, _other| {
                request["scheduledReport"]["scheduleConfig"]["state"] = json!("DISABLED");
            },
        },
        RetryCase {
            label: "the description dropped",
            answer: Answer::Conflict,
            retry: |request, _other| {
                report_mut(request).remove("description");
            },
        },
        RetryCase {
            label: "the widget ids dropped",
            answer: Answer::Conflict,
            retry: |request, _other| {
                report_mut(request).remove("widgetIds");
            },
        },
        RetryCase {
            label: "a different date-range override",
            answer: Answer::Conflict,
            retry: |request, _other| {
                request["scheduledReport"]["widgetDateRangeOverride"]["startTime"] =
                    json!({"type": "RELATIVE", "value": "-P3M"});
            },
        },
        RetryCase {
            label: "a different value on a tag the first request carried",
            answer: Answer::Conflict,
            retry: |request, _other| {
                request["resourceTags"] = json!([
                    {"key": "team", "value": "platform"},
                    {"key": "env", "value": "prod"}
                ]);
            },
        },
        RetryCase {
            label: "one more tag than the first request carried",
            answer: Answer::Conflict,
            retry: |request, _other| {
                request["resourceTags"]
                    .as_array_mut()
                    .expect("resourceTags is a list")
                    .push(json!({"key": "cost-center", "value": "4711"}));
            },
        },
    ]
}

/// The ARNs of the reports this account stores.
async fn stored_arns(router: &MultiServiceRouter) -> Vec<String> {
    let listed = ok_call(router, "ListScheduledReports", &json!({})).await;
    listed["scheduledReports"]
        .as_array()
        .unwrap_or_else(|| panic!("ListScheduledReports answers a list: {listed}"))
        .iter()
        .map(|summary| {
            summary["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("every summary carries an arn: {summary}"))
                .to_string()
        })
        .collect()
}

/// A row's first create, on its own store: the router, the request, the ARN it minted,
/// the record it stored and a second dashboard for the row that repoints the retry.
struct Served {
    router: MultiServiceRouter,
    request: Value,
    arn: String,
    stored: Value,
    other_dashboard: String,
}

/// Serve the first create of `case` on a fresh store.
async fn serve_first(label: &str) -> Served {
    let router = router();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "TokenReplaySource").await;
    let (other_dashboard, _) = source_dashboard(&router, "TokenReplayOther").await;

    let request = first_request(&dashboard_arn, &widget_id);
    let created = ok_call(&router, "CreateScheduledReport", &request).await;
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: the first create returns an arn: {created}"))
        .to_string();
    let stored = ok_call(&router, "GetScheduledReport", &json!({"arn": arn})).await;
    Served {
        router,
        request,
        arn,
        stored,
        other_dashboard,
    }
}

/// A retry presenting a token this service already created a report under replays that
/// report's ARN, and only a token presented with a materially different request is
/// refused with ConflictException — over the table above. Either way the store still
/// holds exactly the one report the first call created, unchanged.
#[tokio::test]
async fn a_retried_client_token_replays_the_create_it_already_served() {
    // Every row's first create runs first, so the one wait below covers all of them and
    // each retry is a retry rather than a duplicate racing its create.
    let cases = cases();
    let mut served: Vec<Served> = Vec::new();
    for case in &cases {
        served.push(serve_first(case.label).await);
    }
    wait_out_the_creation_window().await;

    for (case, served) in cases.iter().zip(&served) {
        let label = case.label;
        let mut retry = served.request.clone();
        (case.retry)(&mut retry, &served.other_dashboard);
        if case.answer == Answer::Conflict {
            assert_ne!(
                retry, served.request,
                "case {label}: a conflicting row must really change the request"
            );
        }

        let (status, body) = call(&served.router, "CreateScheduledReport", &retry).await;
        match case.answer {
            Answer::Replay => {
                assert_eq!(
                    status, 200,
                    "case {label}: a retry of the request the token already served answers 200, got {status}: {body}"
                );
                assert_eq!(
                    body["arn"].as_str(),
                    Some(served.arn.as_str()),
                    "case {label}: expected the first call's arn {:?}, got {body}",
                    served.arn
                );
            }
            Answer::Conflict => {
                assert_eq!(
                    status, 409,
                    "case {label}: a token reused for a different request answers 409, got {status}: {body}"
                );
                assert!(
                    error_code(&body).ends_with("ConflictException"),
                    "case {label}: expected ConflictException, got {body}"
                );
                let message = body["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(TOKEN) && message.contains(&served.arn),
                    "case {label}: the refusal names the token and the report it already created, got {body}"
                );
            }
        }

        // Neither answer created a second report, and neither touched the first one.
        let arns = stored_arns(&served.router).await;
        assert_eq!(
            arns,
            vec![served.arn.clone()],
            "case {label}: the token must have created exactly one report"
        );
        assert_eq!(
            ok_call(
                &served.router,
                "GetScheduledReport",
                &json!({"arn": served.arn})
            )
            .await,
            served.stored,
            "case {label}: the retry must leave the stored record untouched"
        );
    }
}

/// A duplicate that arrives while the create its token started is still in flight is the
/// conflict the model documents — the report "is being created" — so one of several
/// callers sharing a token gets the report and the others are told a creation is in
/// progress. Once that creation has settled the same request is a retry and is replayed.
#[tokio::test]
async fn a_duplicate_racing_the_create_is_refused_and_the_later_retry_is_replayed() {
    let served = serve_first("racing duplicate").await;

    let (status, body) = call(&served.router, "CreateScheduledReport", &served.request).await;
    assert_eq!(
        status, 409,
        "case racing duplicate: a duplicate token presented while the create is in flight answers 409, got {status}: {body}"
    );
    assert!(
        error_code(&body).ends_with("ConflictException"),
        "case racing duplicate: expected ConflictException, got {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(TOKEN) && message.contains("in progress"),
        "case racing duplicate: the refusal says a creation is already in progress for the token, got {body}"
    );
    assert_eq!(
        stored_arns(&served.router).await,
        vec![served.arn.clone()],
        "case racing duplicate: the race must leave exactly one report"
    );

    wait_out_the_creation_window().await;
    let replayed = ok_call(&served.router, "CreateScheduledReport", &served.request).await;
    assert_eq!(
        replayed["arn"].as_str(),
        Some(served.arn.as_str()),
        "case settled retry: the same request after the creation settled replays the arn, got {replayed}"
    );
    assert_eq!(
        stored_arns(&served.router).await,
        vec![served.arn.clone()],
        "case settled retry: the replay must create no second report"
    );
}

/// A token whose report this service no longer stores has no outcome left to replay, so
/// the same request under it creates a report again and records its own ARN — a retry is
/// never answered with an ARN that resolves to nothing, and the token records do not
/// outlive the reports they describe. `DeleteScheduledReport` is still a 501 stub, so the
/// removal is made in the store the router serves, which is exactly what that operation
/// will do to it.
#[tokio::test]
async fn a_token_whose_report_is_gone_creates_again_instead_of_replaying_it() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    let (dashboard_arn, widget_id) = source_dashboard(&router, "GoneReportSource").await;
    let request = first_request(&dashboard_arn, &widget_id);
    let created = ok_call(&router, "CreateScheduledReport", &request).await;
    let arn = created["arn"].as_str().expect("an arn").to_string();
    wait_out_the_creation_window().await;

    // The report is removed from the store, as a delete would remove it.
    {
        let state = service_state(&*engine).expect("the dashboard store");
        let mut state = state.lock();
        state.scheduled_reports.remove(&arn);
    }

    let again = ok_call(&router, "CreateScheduledReport", &request).await;
    let fresh_arn = again["arn"].as_str().expect("an arn").to_string();
    assert_ne!(
        fresh_arn, arn,
        "the create must mint a new report rather than replay the removed one: {again}"
    );
    assert_eq!(
        stored_arns(&router).await,
        vec![fresh_arn.clone()],
        "the store holds the report this create made"
    );
    assert_eq!(
        ok_call(&router, "GetScheduledReport", &json!({"arn": fresh_arn})).await
            ["scheduledReport"]["arn"]
            .as_str(),
        Some(fresh_arn.as_str()),
        "the report the retry created reads back under its own arn"
    );

    // And the record this create left is the one its own retry replays.
    wait_out_the_creation_window().await;
    let replayed = ok_call(&router, "CreateScheduledReport", &request).await;
    assert_eq!(
        replayed["arn"].as_str(),
        Some(fresh_arn.as_str()),
        "the new create's own retry replays its arn: {replayed}"
    );
    assert_eq!(
        service_state(&*engine)
            .expect("the dashboard store")
            .lock()
            .scheduled_report_client_tokens
            .len(),
        1,
        "one token record per stored report, never one per token ever presented"
    );
}

/// Idempotence is not a one-shot: however many times the same request comes back under
/// its token, it is answered with the one ARN and the store still holds one report —
/// while a fresh token is a fresh request and creates its own.
#[tokio::test]
async fn repeated_retries_all_replay_the_one_arn() {
    let served = serve_first("repeated retries").await;
    wait_out_the_creation_window().await;

    for attempt in 1..=4 {
        let answered = ok_call(&served.router, "CreateScheduledReport", &served.request).await;
        assert_eq!(
            answered["arn"].as_str(),
            Some(served.arn.as_str()),
            "attempt {attempt}: every retry replays the one arn, got {answered}"
        );
        assert_eq!(
            stored_arns(&served.router).await,
            vec![served.arn.clone()],
            "attempt {attempt}: no retry may create a second report"
        );
    }

    // The same request under a token this service has not seen is a new create.
    let mut fresh = served.request.clone();
    fresh["clientToken"] = json!("another-sdk-token-91ab");
    let second = ok_call(&served.router, "CreateScheduledReport", &fresh).await;
    let second_arn = second["arn"].as_str().expect("an arn").to_string();
    assert_ne!(
        second_arn, served.arn,
        "a token this service has not seen creates its own report: {second}"
    );
    let mut expected = vec![served.arn.clone(), second_arn];
    expected.sort();
    let mut arns = stored_arns(&served.router).await;
    arns.sort();
    assert_eq!(
        arns, expected,
        "the two tokens must have created exactly two reports"
    );
}
