// SPDX-License-Identifier: CC0-1.0

//! A campaign's state is owed to the CLOCK as well as to the lifecycle calls.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handler together. The rules under test:
//!
//! * a campaign that has entered its run — `Running` or `Paused` — is `Completed`
//!   once its schedule's `endTime` has passed, which `GetCampaignState` reports
//!   without anything having had to write it, while a campaign that never entered a
//!   run (`Initialized`), one whose run a caller ended (`Stopped`) and one with no
//!   schedule at all stay where the calls left them;
//! * reporting that state WRITES NOTHING: repeated reads agree and the campaign's
//!   stored schedule is untouched;
//! * every operation gated on the campaign's state judges the clock-derived one, so
//!   a campaign whose run window has closed is past dialling
//!   (`PutOutboundRequestBatch`), past reconfiguration (the `Update*`/`Delete*`
//!   configuration operations) and past every lifecycle action — each HTTP 409
//!   `InvalidCampaignStateException` naming `Completed`.

use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::Schedule;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    CampaignState, StoredCampaign, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router};

/// Now, as seconds since the Unix epoch.
fn unix_secs() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a test clock after the Unix epoch")
        .as_secs_f64()
}

/// `now + offset_secs` as the RFC 3339 instant a `Schedule` member carries on the
/// wire: `Schedule$startTime` / `$endTime` target `TimeStamp`, which the model marks
/// `@timestampFormat("date-time")`.
fn instant(offset_secs: f64) -> String {
    DateTime::from_secs_f64(unix_secs() + offset_secs)
        .fmt(Format::DateTime)
        .expect("a formattable instant")
}

/// A minimal telephony `channelSubtypeConfig`: `outboundMode` and
/// `defaultOutboundConfig` are both required by the model.
fn telephony_config() -> Value {
    json!({
        "telephony": {
            "outboundMode": {"agentless": {}},
            "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
        }
    })
}

/// One outbound request body: the members `OutboundRequest` requires, for telephony.
fn telephony_request(client_token: &str) -> Value {
    json!({
        "clientToken": client_token,
        "expirationTime": "2099-01-01T00:00:00Z",
        "channelSubtypeParameters": {
            "telephony": {"destinationPhoneNumber": "+15550100", "attributes": {}}
        }
    })
}

/// Onboard `instance` and wait out the span the clock owes its onboarding job, so the
/// instance is a campaign target by the time this returns: `CreateCampaign` mints a
/// campaign only for an instance whose job has SUCCEEDED.
async fn onboard(router: &MultiServiceRouter, region: &str, instance: &str) {
    let (status, body) = call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {instance:?}: expected onboarding to start, got {status} {body}"
    );
    tokio::time::sleep(ONBOARDING_JOB_DURATION + Duration::from_millis(150)).await;
}

/// Create a campaign on an onboarded `instance`, carrying the run `window` as its
/// `schedule` (offsets in seconds from now, `None` for a campaign with no schedule
/// at all), and hand back its id.
async fn create_campaign(
    router: &MultiServiceRouter,
    region: &str,
    instance: &str,
    name: &str,
    window: Window,
) -> String {
    let mut body = json!({
        "name": name,
        "connectInstanceId": instance,
        "channelSubtypeConfig": telephony_config(),
    });
    if let Some((start, end)) = window {
        body["schedule"] = json!({"startTime": instant(start), "endTime": instant(end)});
    }
    let (status, response) = call(router, region, "PUT", "/v2/campaigns", &body).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateCampaign expected 200, got {status} {response}"
    );
    response["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: CreateCampaign returned no id: {response}"))
        .to_string()
}

/// The state `GetCampaignState` reports for `id`, asserting the read succeeded.
async fn state_of(router: &MultiServiceRouter, region: &str, id: &str, case: &str) -> String {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/v2/campaigns/{id}/state"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetCampaignState expected 200, got {status} {body}"
    );
    body["state"]
        .as_str()
        .unwrap_or("<no state>")
        .to_string()
}

/// One lifecycle action (`start`, `pause`, `resume`, `stop`) on `id`.
async fn act(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    action: &str,
) -> (u16, Value) {
    call(
        router,
        region,
        "POST",
        &format!("/v2/campaigns/{id}/{action}"),
        &Value::Null,
    )
    .await
}

/// The `campaign` object `DescribeCampaign` projects for `id`, asserting the read
/// succeeded.
async fn describe(router: &MultiServiceRouter, region: &str, id: &str, case: &str) -> Value {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/v2/campaigns/{id}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeCampaign expected 200, got {status} {body}"
    );
    body["campaign"].clone()
}

/// A campaign's run window, as `(startTime, endTime)` offsets in seconds from now,
/// or `None` for a campaign carrying no schedule at all.
type Window = Option<(f64, f64)>;

/// One row of the derivation table: a window and whether it has closed by now.
struct WindowCase {
    case: &'static str,
    window: Window,
    closed: bool,
}

/// One row of the `GetCampaignState` table: a campaign created with `window`, put
/// through `actions`, read `wait` later.
struct StateCase {
    case: &'static str,
    window: Window,
    actions: &'static [&'static str],
    wait: Duration,
    expected: &'static str,
}

/// One state-gated operation: how it is addressed and the body it takes.
struct Gated {
    operation: &'static str,
    method: &'static str,
    suffix: &'static str,
    body: fn() -> Value,
}

/// A stored campaign in `lifecycle_state`, carrying `window` (offsets in seconds
/// from now) as its schedule — the record [`StoredCampaign::state_at`] reads.
fn stored(lifecycle_state: CampaignState, window: Window) -> StoredCampaign {
    StoredCampaign {
        id: "000000000000".to_string(),
        arn: "arn:aws:connect-campaigns:us-east-1:000000000000:campaign/000000000000".to_string(),
        name: "probe".to_string(),
        connect_instance_id: "instance-1".to_string(),
        channel_subtype_config: None,
        campaign_type: None,
        source: None,
        connect_campaign_flow_arn: None,
        schedule: window.map(|(start, end)| Schedule {
            start_time: DateTime::from_secs_f64(unix_secs() + start),
            end_time: DateTime::from_secs_f64(unix_secs() + end),
            refresh_frequency: None,
        }),
        entry_limits_config: None,
        communication_time_config: None,
        communication_limits_override: None,
        tags: std::collections::BTreeMap::new(),
        lifecycle_state,
        created_at: SystemTime::now(),
        outbound_requests: std::collections::BTreeMap::new(),
        profile_outbound_requests: std::collections::BTreeMap::new(),
    }
}

/// The state a campaign is at is derived from the stored lifecycle state AND the
/// stored schedule, over a TABLE spanning the whole `CampaignState` catalog (source:
/// the model's `CampaignState` enum) crossed with the three things a run window can
/// be: absent, still open, already closed.
///
/// Only a campaign that has entered its run is completed by the clock; the derivation
/// is a pure read, so the same record answers the same way twice.
#[test]
fn the_stored_state_and_the_stored_schedule_together_decide_the_state() {
    let catalog = [
        CampaignState::Initialized,
        CampaignState::Running,
        CampaignState::Paused,
        CampaignState::Stopped,
        CampaignState::Failed,
        CampaignState::Completed,
    ];
    let windows: &[WindowCase] = &[
        WindowCase {
            case: "no schedule",
            window: None,
            closed: false,
        },
        WindowCase {
            case: "window open",
            window: Some((-60.0, 3_600.0)),
            closed: false,
        },
        WindowCase {
            case: "window opens later",
            window: Some((3_600.0, 7_200.0)),
            closed: false,
        },
        WindowCase {
            case: "window closed a second ago",
            window: Some((-3_600.0, -1.0)),
            closed: true,
        },
        WindowCase {
            case: "window closed an hour ago",
            window: Some((-7_200.0, -3_600.0)),
            closed: true,
        },
        WindowCase {
            case: "window closed thirty days ago",
            window: Some((-2_595_600.0, -2_592_000.0)),
            closed: true,
        },
    ];

    for stored_state in catalog {
        for WindowCase {
            case: window_case,
            window,
            closed,
        } in windows
        {
            let record = stored(stored_state, *window);
            let now = SystemTime::now();
            let entered_its_run =
                matches!(stored_state, CampaignState::Running | CampaignState::Paused);
            let expected = if *closed && entered_its_run {
                CampaignState::Completed
            } else {
                stored_state
            };
            let actual = record.state_at(now);
            let case = format!("{} with {window_case}", stored_state.as_wire());
            assert_eq!(
                actual, expected,
                "case {case:?}: expected state {expected:?}, got {actual:?}"
            );
            // A pure read: asking again at the same instant answers the same.
            assert_eq!(
                record.state_at(now),
                actual,
                "case {case:?}: expected a second read to agree, got {:?} then {actual:?}",
                record.state_at(now)
            );
            assert_eq!(
                record.run_window_closed_at(now),
                *closed,
                "case {case:?}: expected run_window_closed_at {closed}, got {}",
                record.run_window_closed_at(now)
            );
        }
    }
}

/// `GetCampaignState` reports the state the clock leaves the campaign at, over a
/// TABLE whose rows differ along what the rule generalises over: whether the campaign
/// carries a schedule at all, how far past (or short of) its `endTime` the clock is,
/// which lifecycle calls it took, and whether its window closed before or after those
/// calls.
#[tokio::test]
async fn get_campaign_state_reports_completed_once_the_run_window_has_closed() {
    let region = "us-west-2";
    let router = router();
    let instance = "schedule-completion-instance";
    onboard(&router, region, instance).await;

    // A window that closes shortly after the call that starts the run, so the rows
    // below cross the `endTime` with NOTHING calling in between — the campaign is
    // completed by the clock, not by a call.
    const SHORT_WINDOW_SECS: f64 = 0.4;
    const WAIT_OUT_SHORT_WINDOW: Duration = Duration::from_millis(600);

    let cases: &[StateCase] = &[
        StateCase {
            case: "no schedule, started",
            window: None,
            actions: &["start"],
            wait: Duration::ZERO,
            expected: "Running",
        },
        StateCase {
            case: "window open for an hour, never started",
            window: Some((-60.0, 3_600.0)),
            actions: &[],
            wait: Duration::ZERO,
            expected: "Initialized",
        },
        StateCase {
            case: "window open for an hour, started",
            window: Some((-60.0, 3_600.0)),
            actions: &["start"],
            wait: Duration::ZERO,
            expected: "Running",
        },
        StateCase {
            case: "window open for an hour, started then paused",
            window: Some((-60.0, 3_600.0)),
            actions: &["start", "pause"],
            wait: Duration::ZERO,
            expected: "Paused",
        },
        StateCase {
            case: "window open for an hour, started then stopped",
            window: Some((-60.0, 3_600.0)),
            actions: &["start", "stop"],
            wait: Duration::ZERO,
            expected: "Stopped",
        },
        StateCase {
            case: "window closed a second ago, started",
            window: Some((-3_600.0, -1.0)),
            actions: &["start"],
            wait: Duration::ZERO,
            expected: "Completed",
        },
        StateCase {
            case: "window closed an hour ago, started",
            window: Some((-7_200.0, -3_600.0)),
            actions: &["start"],
            wait: Duration::ZERO,
            expected: "Completed",
        },
        StateCase {
            case: "window closed thirty days ago, started",
            window: Some((-2_595_600.0, -2_592_000.0)),
            actions: &["start"],
            wait: Duration::ZERO,
            expected: "Completed",
        },
        StateCase {
            case: "window closed an hour ago, never started",
            window: Some((-7_200.0, -3_600.0)),
            actions: &[],
            wait: Duration::ZERO,
            expected: "Initialized",
        },
        StateCase {
            case: "window closes while the campaign runs",
            window: Some((-1.0, SHORT_WINDOW_SECS)),
            actions: &["start"],
            wait: WAIT_OUT_SHORT_WINDOW,
            expected: "Completed",
        },
        StateCase {
            case: "window closes while the campaign is paused",
            window: Some((-1.0, SHORT_WINDOW_SECS)),
            actions: &["start", "pause"],
            wait: WAIT_OUT_SHORT_WINDOW,
            expected: "Completed",
        },
        StateCase {
            case: "window closes after the caller stopped the run",
            window: Some((-1.0, SHORT_WINDOW_SECS)),
            actions: &["start", "stop"],
            wait: WAIT_OUT_SHORT_WINDOW,
            expected: "Stopped",
        },
        StateCase {
            case: "window still open after the same wait, started",
            window: Some((-1.0, 3_600.0)),
            actions: &["start"],
            wait: WAIT_OUT_SHORT_WINDOW,
            expected: "Running",
        },
    ];

    for StateCase {
        case,
        window,
        actions,
        wait,
        expected,
    } in cases
    {
        let id = create_campaign(&router, region, instance, case, *window).await;
        let sent_schedule = describe(&router, region, &id, case).await["schedule"].clone();
        for action in *actions {
            let (status, body) = act(&router, region, &id, action).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {action:?} to be accepted, got {status} {body}"
            );
        }
        if !wait.is_zero() {
            tokio::time::sleep(*wait).await;
        }

        let actual = state_of(&router, region, &id, case).await;
        assert_eq!(
            &actual, expected,
            "case {case:?}: expected state {expected:?}, got {actual:?}"
        );
        // Reporting the state changes nothing a later caller sees: the state is the
        // same on the next read, and the stored schedule is untouched.
        let again = state_of(&router, region, &id, case).await;
        assert_eq!(
            again, actual,
            "case {case:?}: expected the second read to report {actual:?}, got {again:?}"
        );
        let campaign = describe(&router, region, &id, case).await;
        assert_eq!(
            campaign["schedule"], sent_schedule,
            "case {case:?}: expected the stored schedule {sent_schedule} to be untouched, got {}",
            campaign["schedule"]
        );
    }
}

/// Every operation gated on a campaign's state judges the clock-derived one: a
/// campaign whose run window has closed refuses each of them with HTTP 409
/// `InvalidCampaignStateException` naming `Completed`, while the same campaign with an
/// open window takes them all.
///
/// The rows are the gated operations themselves — the dialling path, the
/// configuration path and every lifecycle action.
#[tokio::test]
async fn a_campaign_past_its_window_refuses_every_state_gated_operation() {
    let region = "eu-west-1";
    let router = router();
    let instance = "schedule-gate-instance";
    onboard(&router, region, instance).await;

    let gated: &[Gated] = &[
        Gated {
            operation: "PutOutboundRequestBatch",
            method: "PUT",
            suffix: "/outbound-requests",
            body: || json!({"outboundRequests": [telephony_request("token-1")]}),
        },
        Gated {
            operation: "UpdateCampaignCommunicationLimits",
            method: "POST",
            suffix: "/communication-limits",
            body: || {
                json!({"communicationLimitsOverride": {"allChannelSubtypes": {
                    "communicationLimitsList": [
                        {"maxCountPerRecipient": 1, "frequency": 1, "unit": "DAY"}
                    ]
                }}})
            },
        },
        Gated {
            operation: "UpdateCampaignCommunicationTime",
            method: "POST",
            suffix: "/communication-time",
            body: || {
                // `localTimeZoneConfig` is `CommunicationTimeConfig`'s one
                // `@required` member.
                json!({"communicationTimeConfig": {
                    "localTimeZoneConfig": {"defaultTimeZone": "America/New_York"}
                }})
            },
        },
        Gated {
            operation: "DeleteCampaignCommunicationLimits",
            method: "DELETE",
            // `config` is the one value `CommunicationLimitsConfigType` declares
            // (source: `COMMUNICATION_LIMITS_CONFIG_TYPE_VALUES`).
            suffix: "/communication-limits?config=ALL_CHANNEL_SUBTYPES",
            body: || Value::Null,
        },
        Gated {
            operation: "DeleteCampaignCommunicationTime",
            method: "DELETE",
            suffix: "/communication-time?config=TELEPHONY",
            body: || Value::Null,
        },
        Gated {
            operation: "UpdateCampaignFlowAssociation",
            method: "POST",
            suffix: "/flow",
            body: || {
                json!({"connectCampaignFlowArn":
                    "arn:aws:connect:eu-west-1:000000000000:instance/i-1/flow/f-1"})
            },
        },
        Gated {
            operation: "StartCampaign",
            method: "POST",
            suffix: "/start",
            body: || Value::Null,
        },
        Gated {
            operation: "PauseCampaign",
            method: "POST",
            suffix: "/pause",
            body: || Value::Null,
        },
        Gated {
            operation: "ResumeCampaign",
            method: "POST",
            suffix: "/resume",
            body: || Value::Null,
        },
        Gated {
            operation: "StopCampaign",
            method: "POST",
            suffix: "/stop",
            body: || Value::Null,
        },
    ];

    for Gated {
        operation,
        method,
        suffix,
        body,
    } in gated
    {
        // A campaign started while its window was open, whose window has since
        // closed: the run is over, so the operation is refused.
        let case = format!("{operation} past the window");
        let closed = create_campaign(
            &router,
            region,
            instance,
            &case,
            Some((-7_200.0, -3_600.0)),
        )
        .await;
        let (status, started) = act(&router, region, &closed, "start").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected StartCampaign to be accepted, got {status} {started}"
        );
        assert_eq!(
            state_of(&router, region, &closed, &case).await,
            "Completed",
            "case {case:?}: expected the campaign to be Completed before the call"
        );

        let (status, refused) = call(
            &router,
            region,
            method,
            &format!("/v2/campaigns/{closed}{suffix}"),
            &body(),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} {refused}"
        );
        assert!(
            error_code(&refused).ends_with("InvalidCampaignStateException"),
            "case {case:?}: expected InvalidCampaignStateException, got {}",
            error_code(&refused)
        );
        assert_eq!(
            refused["state"].as_str(),
            Some("Completed"),
            "case {case:?}: expected the refusal to name state \"Completed\", got {:?}",
            refused["state"]
        );
        // The refusal wrote nothing: the campaign is still Completed.
        assert_eq!(
            state_of(&router, region, &closed, &case).await,
            "Completed",
            "case {case:?}: expected the campaign to still be Completed after the refusal"
        );

        // The same call on a campaign whose window is still open is served — so the
        // refusal above is the closed window, not the operation.
        let open_case = format!("{operation} inside the window");
        let open = create_campaign(
            &router,
            region,
            instance,
            &open_case,
            Some((-60.0, 3_600.0)),
        )
        .await;
        let (status, started) = act(&router, region, &open, "start").await;
        assert_eq!(
            status, 200,
            "case {open_case:?}: expected StartCampaign to be accepted, got {status} {started}"
        );
        // `start` and `resume` are the two actions that are illegal from `Running`
        // for reasons of their own, so they are not expected to be served here.
        let expected_status = if matches!(*operation, "StartCampaign" | "ResumeCampaign") {
            409
        } else {
            200
        };
        let (status, served) = call(
            &router,
            region,
            method,
            &format!("/v2/campaigns/{open}{suffix}"),
            &body(),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {open_case:?}: expected HTTP {expected_status}, got {status} {served}"
        );
        if expected_status == 409 {
            assert_eq!(
                served["state"].as_str(),
                Some("Running"),
                "case {open_case:?}: expected the refusal to name state \"Running\", got {:?}",
                served["state"]
            );
        }
    }
}
