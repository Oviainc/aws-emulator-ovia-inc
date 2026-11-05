// SPDX-License-Identifier: CC0-1.0

//! What a `CreateScheduledReport` `clientToken` is worth once a create has used it, end
//! to end over the wire.
//!
//! The rule under test: a create SPENDS the `@idempotencyToken` it is presented with, so
//! once a call under a token has stored its report that token is terminally spent —
//! every later call presenting it is refused with the modelled `ConflictException` (409)
//! naming the report already created under it, creates no second report and is answered
//! no ARN, whatever the rest of the request says and however long after it arrives. The
//! separate "creation is in progress" wording belongs only to a call that overlaps a
//! create which has not stored its report yet; a caller already holding the first call's
//! answer is never in that case.
//!
//! The table spans what the rule generalises over — how the later call differs from the
//! create that spent the token (not at all, each member of the submitted report, the
//! tags, their order and count) and how long after it arrives (straight away, or past a
//! pause far longer than any span an in-process create could still be running for). The
//! recorded case, a reuse carrying a different `scheduledReport.name` straight after the
//! first answer, is one row of it.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::service_state;

use super::create_report_client_token::{
    first_request, serve_first, stored_arns, Served, TOKEN,
};
use super::create_report_conflict_message::{already_exists_message, IN_PROGRESS_MESSAGE};
use super::dashboard_lifecycle::{call, emulator, error_code, ok_call};
use super::scheduled_report_lifecycle::source_dashboard;

/// How long the paused rows wait before presenting the spent token again.
///
/// A spent token's refusal is decided by what the create under it did, never by the
/// clock, so this only has to be long enough that a reuse arriving after it could not be
/// mistaken for one overlapping a create still in progress.
const PAUSE_BEFORE_REUSE: Duration = Duration::from_millis(750);

/// The members of `scheduledReport` of a request under rewrite.
fn report_mut(request: &mut Value) -> &mut serde_json::Map<String, Value> {
    request["scheduledReport"]
        .as_object_mut()
        .expect("the request carries a scheduledReport object")
}

/// One row: how the call reusing the spent token differs from the create that spent it,
/// and whether it arrives straight after that create's answer or past the pause.
struct ReuseCase {
    label: &'static str,
    /// Rewrites the first request into the reuse. The second argument is the ARN of
    /// another dashboard this account stores, so a row can point the reuse elsewhere.
    reuse: fn(&mut Value, &str),
    after_a_pause: bool,
}

fn cases() -> Vec<ReuseCase> {
    vec![
        ReuseCase {
            label: "the identical request an SDK transport retry resends",
            reuse: |_request, _other| {},
            after_a_pause: false,
        },
        ReuseCase {
            label: "the identical request, long after the first answer",
            reuse: |_request, _other| {},
            after_a_pause: true,
        },
        ReuseCase {
            label: "a different report name",
            reuse: |request, _other| {
                request["scheduledReport"]["name"] = json!("Idempotent Rollup 2");
            },
            after_a_pause: false,
        },
        ReuseCase {
            label: "a different report name, long after the first answer",
            reuse: |request, _other| {
                request["scheduledReport"]["name"] = json!("Idempotent Rollup 2");
            },
            after_a_pause: true,
        },
        ReuseCase {
            label: "a different dashboard to report on",
            reuse: |request, other| {
                request["scheduledReport"]["dashboardArn"] = json!(other);
            },
            after_a_pause: false,
        },
        ReuseCase {
            label: "a different execution role",
            reuse: |request, _other| {
                request["scheduledReport"]["scheduledReportExecutionRoleArn"] =
                    json!("arn:aws:iam::123456789012:role/OtherReportRole");
            },
            after_a_pause: false,
        },
        ReuseCase {
            label: "a different schedule expression",
            reuse: |request, _other| {
                request["scheduledReport"]["scheduleConfig"]["scheduleExpression"] =
                    json!("rate(7 days)");
            },
            after_a_pause: true,
        },
        ReuseCase {
            label: "a different schedule time zone",
            reuse: |request, _other| {
                request["scheduledReport"]["scheduleConfig"]["scheduleExpressionTimeZone"] =
                    json!("America/New_York");
            },
            after_a_pause: false,
        },
        ReuseCase {
            label: "a different schedule period",
            reuse: |request, _other| {
                request["scheduledReport"]["scheduleConfig"]["schedulePeriod"]["endTime"] =
                    json!(1_830_297_600);
            },
            after_a_pause: false,
        },
        ReuseCase {
            label: "the schedule state spelled out as the value the first create stored",
            reuse: |request, _other| {
                // `ScheduleState` values, source: the `ScheduleState` enum in the model; a
                // create that omits `state` stores ENABLED, which the first request did.
                request["scheduledReport"]["scheduleConfig"]["state"] = json!("ENABLED");
            },
            after_a_pause: true,
        },
        ReuseCase {
            label: "the other schedule state",
            reuse: |request, _other| {
                request["scheduledReport"]["scheduleConfig"]["state"] = json!("DISABLED");
            },
            after_a_pause: false,
        },
        ReuseCase {
            label: "the description dropped",
            reuse: |request, _other| {
                report_mut(request).remove("description");
            },
            after_a_pause: false,
        },
        ReuseCase {
            label: "the widget ids dropped",
            reuse: |request, _other| {
                report_mut(request).remove("widgetIds");
            },
            after_a_pause: true,
        },
        ReuseCase {
            label: "a different date-range override",
            reuse: |request, _other| {
                request["scheduledReport"]["widgetDateRangeOverride"]["startTime"] =
                    json!({"type": "RELATIVE", "value": "-P3M"});
            },
            after_a_pause: false,
        },
        ReuseCase {
            label: "the same tags in the other order",
            reuse: |request, _other| {
                let tags = request["resourceTags"]
                    .as_array_mut()
                    .expect("resourceTags is a list");
                tags.reverse();
            },
            after_a_pause: true,
        },
        ReuseCase {
            label: "a different value on a tag the first request carried",
            reuse: |request, _other| {
                request["resourceTags"] = json!([
                    {"key": "team", "value": "platform"},
                    {"key": "env", "value": "prod"}
                ]);
            },
            after_a_pause: false,
        },
        ReuseCase {
            label: "one more tag than the first request carried",
            reuse: |request, _other| {
                request["resourceTags"]
                    .as_array_mut()
                    .expect("resourceTags is a list")
                    .push(json!({"key": "cost-center", "value": "4711"}));
            },
            after_a_pause: true,
        },
        ReuseCase {
            label: "the tags dropped",
            reuse: |request, _other| {
                request
                    .as_object_mut()
                    .expect("the request is an object")
                    .remove("resourceTags");
            },
            after_a_pause: false,
        },
    ]
}

/// Present `request` to the create under a token already spent and assert the refusal the
/// rule demands: the modelled conflict, worded in the name of the report the token
/// already created, with nothing created and nothing changed.
async fn assert_refused_as_already_created(label: &str, served: &Served, request: &Value) {
    let (status, body) = call(&served.router, "CreateScheduledReport", request).await;
    assert_eq!(
        status, 409,
        "case {label}: a create presenting a spent clientToken is refused with 409, got {status}: {body}"
    );
    assert!(
        error_code(&body).ends_with("ConflictException"),
        "case {label}: expected ConflictException, got {body}"
    );
    assert_eq!(
        body["arn"],
        Value::Null,
        "case {label}: a refused create answers no arn, got {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    let expected = already_exists_message(&served.arn);
    assert_eq!(
        message, expected,
        "case {label}: expected {expected:?}, got {message:?}"
    );
    assert!(
        !message.contains(TOKEN),
        "case {label}: the presented clientToken must not appear in the refusal, got {message:?}"
    );

    // The refusal created no second report and left the first one exactly as it was.
    let arns = stored_arns(&served.router).await;
    assert_eq!(
        arns,
        vec![served.arn.clone()],
        "case {label}: the token must have created exactly one report, got {arns:?}"
    );
    let reread = ok_call(
        &served.router,
        "GetScheduledReport",
        &json!({"arn": served.arn}),
    )
    .await;
    assert_eq!(
        reread, served.stored,
        "case {label}: the refused reuse must leave the stored record untouched, got {reread}"
    );
}

/// Every call presenting a token a create has already spent is refused in the name of the
/// report that create stored — over the table above, whatever the rest of the request
/// says and whether the reuse follows the first answer straight away or long after.
#[tokio::test]
async fn a_spent_client_token_refuses_every_later_create() {
    let cases = cases();
    let mut served: Vec<Served> = Vec::new();
    for case in &cases {
        served.push(serve_first(case.label).await);
    }

    // The immediate rows first: the caller holds the first call's answer and presents the
    // token again at once, which is the recorded case.
    for (case, served) in cases.iter().zip(&served) {
        if case.after_a_pause {
            continue;
        }
        let mut reuse = served.request.clone();
        (case.reuse)(&mut reuse, &served.other_dashboard);
        assert_refused_as_already_created(case.label, served, &reuse).await;
    }

    // One pause covers every paused row: a spent token does not come back.
    tokio::time::sleep(PAUSE_BEFORE_REUSE).await;
    for (case, served) in cases.iter().zip(&served) {
        if !case.after_a_pause {
            continue;
        }
        let mut reuse = served.request.clone();
        (case.reuse)(&mut reuse, &served.other_dashboard);
        assert_refused_as_already_created(case.label, served, &reuse).await;
    }
}

/// Being spent is not a one-shot: however many times the token comes back it is refused
/// the same way and the store still holds the one report — while a token this service has
/// not seen is a create key of its own and creates its own report.
#[tokio::test]
async fn a_spent_client_token_stays_spent_however_often_it_is_presented() {
    let served = serve_first("repeated reuse").await;

    for attempt in 1..=4 {
        let label = format!("reuse attempt {attempt}");
        assert_refused_as_already_created(&label, &served, &served.request.clone()).await;
    }

    let mut fresh = served.request.clone();
    fresh["clientToken"] = json!("another-sdk-token-91ab");
    let second = ok_call(&served.router, "CreateScheduledReport", &fresh).await;
    let second_arn = second["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case fresh token: the create returns an arn: {second}"))
        .to_string();
    assert_ne!(
        second_arn, served.arn,
        "case fresh token: a token this service has not seen creates its own report: {second}"
    );
    let mut expected = vec![served.arn.clone(), second_arn];
    expected.sort();
    let mut arns = stored_arns(&served.router).await;
    arns.sort();
    assert_eq!(
        arns, expected,
        "case fresh token: the two tokens must have created exactly two reports"
    );
}

/// The other conflict the model documents, "a resource that ... is being created": a call
/// presenting a token whose create has claimed it and not stored its report yet is told
/// the creation is in progress — the transient refusal, which a caller may present the
/// same token for again. The claim is taken in the store, which is where a create holds
/// it between claiming the token and storing its report, and the call that follows once
/// that create has given the claim up succeeds.
#[tokio::test]
async fn a_create_overlapping_an_unsettled_claim_is_told_the_creation_is_in_progress() {
    let (router, engine) = emulator();
    let (dashboard_arn, widget_id) = source_dashboard(&router, "InFlightClaimSource").await;
    let request = first_request(&dashboard_arn, &widget_id);

    // A create has claimed the token and has not stored its report yet.
    {
        let state = service_state(&*engine).expect("the dashboard store");
        let claimed = state
            .lock()
            .reserve_scheduled_report_creation(TOKEN.to_string());
        assert!(
            claimed.is_none(),
            "case in flight: the token must be free before the claim, got {claimed:?}"
        );
    }

    let (status, body) = call(&router, "CreateScheduledReport", &request).await;
    assert_eq!(
        status, 409,
        "case in flight: a create overlapping an unsettled claim is refused with 409, got {status}: {body}"
    );
    assert!(
        error_code(&body).ends_with("ConflictException"),
        "case in flight: expected ConflictException, got {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, IN_PROGRESS_MESSAGE,
        "case in flight: expected the fixed in-progress text {IN_PROGRESS_MESSAGE:?}, got {message:?}"
    );
    let stored = stored_arns(&router).await;
    assert!(
        stored.is_empty(),
        "case in flight: the refused create must store no report, got {stored:?}"
    );

    // The conflict really was transient: once the create holding the claim has given it
    // up — as a create refused before it stores anything does — the same token works.
    {
        let state = service_state(&*engine).expect("the dashboard store");
        state.lock().release_scheduled_report_creation(TOKEN);
    }
    let created = ok_call(&router, "CreateScheduledReport", &request).await;
    let arn = created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case in flight: the create returns an arn: {created}"))
        .to_string();
    assert_eq!(
        stored_arns(&router).await,
        vec![arn.clone()],
        "case in flight: the create that got the token must have stored its report"
    );

    // And that create spent the token: the next call presenting it is refused in the name
    // of the report it created, not told a creation is in progress.
    let (status, body) = call(&router, "CreateScheduledReport", &request).await;
    let expected = already_exists_message(&arn);
    assert_eq!(
        status, 409,
        "case in flight then spent: expected 409, got {status}: {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(expected.as_str()),
        "case in flight then spent: expected {expected:?}, got {body}"
    );

    // One claim per token, and it is the one that create settled.
    let settled = service_state(&*engine)
        .expect("the dashboard store")
        .lock()
        .scheduled_report_client_tokens
        .get(TOKEN)
        .and_then(|claim| claim.created_arn().map(str::to_string));
    assert_eq!(
        settled,
        Some(arn),
        "case in flight then spent: the token's claim must name the report it created, got {settled:?}"
    );
}
