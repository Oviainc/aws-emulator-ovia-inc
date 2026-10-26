// SPDX-License-Identifier: CC0-1.0

//! `StopCampaign`, end to end over the wire.
//!
//! The rules under test:
//!
//! * stopping ends a run that is under way, so it is legal only from the states a
//!   campaign reaches by having been started (`Running`, `Paused`) and is refused
//!   from a campaign that has never run, from one whose run is already over and
//!   from a terminal state;
//! * a refused stop answers HTTP 409 `InvalidCampaignStateException`, whose `state`
//!   member names the state the campaign is actually in, and writes nothing: every
//!   later read reports the state the refusal named and the campaign still takes
//!   the actions that state allows;
//! * a legal stop leaves the campaign `Stopped` for every projection of the record.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    CampaignAction, CampaignState,
};

// The campaign fixture (onboard, create, read the state, drive the lifecycle) is the
// shared one every lifecycle suite in this crate sets its cases up with.
use super::common::{
    call, campaign_state as state, create_campaign, drive_lifecycle as drive, error_code, router,
};

/// `now + offset_secs` as the RFC 3339 instant a `Schedule` member carries on the
/// wire: `Schedule$startTime` / `$endTime` target `TimeStamp`, which the model marks
/// `@timestampFormat("date-time")`.
fn instant(offset_secs: f64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("a test clock after the Unix epoch")
        .as_secs_f64();
    DateTime::from_secs_f64(now + offset_secs)
        .fmt(Format::DateTime)
        .expect("a formattable instant")
}

/// The transition table for `Stop`, over EVERY modelled state: a run under way ends
/// (`Running`, `Paused` move to `Stopped`) and nothing else moves.
#[test]
fn stop_moves_only_a_campaign_whose_run_is_under_way() {
    // Every variant of the model's `CampaignState` enum, paired with the state
    // `Stop` leaves it in (`None` = the action is refused from there).
    let cases: &[(CampaignState, Option<CampaignState>)] = &[
        (CampaignState::Running, Some(CampaignState::Stopped)),
        (CampaignState::Paused, Some(CampaignState::Stopped)),
        (CampaignState::Initialized, None),
        (CampaignState::Stopped, None),
        (CampaignState::Completed, None),
        (CampaignState::Failed, None),
    ];

    for (from, expected) in cases {
        let actual = from.after(CampaignAction::Stop);
        assert_eq!(
            actual, *expected,
            "case {from:?} under Stop: expected {expected:?}, got {actual:?}"
        );
    }
}

/// Over a TABLE of states reachable through the API — each reached by a different
/// sequence of lifecycle calls, and one reached by the clock closing the campaign's
/// run window — `StopCampaign` succeeds exactly where a run is under way and is
/// refused with 409 `InvalidCampaignStateException` everywhere else, and in both
/// cases `GetCampaignState` afterwards reports the expected state.
#[tokio::test]
async fn stop_succeeds_only_from_a_run_under_way_over_every_reachable_state() {
    let region = "eu-central-1";

    // (case, schedule to create with, the actions that reach the state under test,
    //  the state they reach, expected HTTP status of the stop, expected state after)
    #[allow(clippy::type_complexity)]
    let cases: &[(&str, Option<Value>, &[&str], &str, u16, &str)] = &[
        (
            "stop a freshly created campaign",
            None,
            &[],
            "Initialized",
            409,
            "Initialized",
        ),
        (
            "stop a running campaign",
            None,
            &["start"],
            "Running",
            200,
            "Stopped",
        ),
        (
            "stop a paused campaign",
            None,
            &["start", "pause"],
            "Paused",
            200,
            "Stopped",
        ),
        (
            "stop a campaign resumed after a pause",
            None,
            &["start", "pause", "resume"],
            "Running",
            200,
            "Stopped",
        ),
        (
            "stop an already stopped campaign",
            None,
            &["start", "stop"],
            "Stopped",
            409,
            "Stopped",
        ),
        (
            "stop a campaign restarted after a stop",
            None,
            &["start", "stop", "start"],
            "Running",
            200,
            "Stopped",
        ),
        (
            "stop a campaign whose run window has closed",
            Some(json!({"startTime": instant(-7_200.0), "endTime": instant(-3_600.0)})),
            &["start"],
            "Completed",
            409,
            "Completed",
        ),
    ];

    // One store and one onboarded instance for the whole table — each case gets its
    // own campaign, so the cases are independent without paying for the onboarding
    // wait once per row.
    let router = router();
    let instance = "stop-table-instance";
    for (index, (case, schedule, setup, before, expected_status, expected_state)) in
        cases.iter().enumerate()
    {
        let id = create_campaign(
            &router,
            region,
            instance,
            &format!("stop-{index}"),
            schedule.clone(),
        )
        .await;
        drive(&router, region, &id, setup, case).await;

        let actual_before = state(&router, region, &id, case).await;
        assert_eq!(
            actual_before, *before,
            "case {case:?}: setup should have reached {before:?}, got {actual_before:?}"
        );

        let (status, body) = call(
            &router,
            region,
            "POST",
            &format!("/v2/campaigns/{id}/stop"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if *expected_status == 409 {
            assert!(
                error_code(&body).ends_with("InvalidCampaignStateException"),
                "case {case:?}: expected InvalidCampaignStateException, got {}",
                error_code(&body)
            );
            assert_eq!(
                body["state"].as_str(),
                Some(*before),
                "case {case:?}: expected the refusal to name state {before:?}, got {:?}",
                body["state"]
            );
        }

        let actual = state(&router, region, &id, case).await;
        assert_eq!(
            actual, *expected_state,
            "case {case:?}: expected state {expected_state:?} afterwards, got {actual:?}"
        );

        // The campaign is still the record the store held, whatever the stop answered.
        let (describe_status, described) = call(
            &router,
            region,
            "GET",
            &format!("/v2/campaigns/{id}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            describe_status, 200,
            "case {case:?}: DescribeCampaign expected 200, got {describe_status} {described}"
        );
        assert_eq!(
            described["campaign"]["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: DescribeCampaign should still hold the campaign, got {described}"
        );
    }
}

/// A stop refused from `Initialized` writes NOTHING: the campaign is still a
/// campaign that can be started, and once started it takes outbound requests —
/// which `Stopped` would refuse. Driven over several repeats of the refused stop,
/// so no refusal accumulates any state.
#[tokio::test]
async fn a_refused_stop_leaves_the_campaign_startable() {
    let region = "us-west-2";

    for attempts in [1_usize, 3] {
        let case = format!("{attempts} refused stops before the start");
        let router = router();
        let id = create_campaign(
            &router,
            region,
            "instance-refused",
            &format!("refused-{attempts}"),
            None,
        )
        .await;

        for attempt in 0..attempts {
            let step = format!("{case}, attempt {attempt}");
            let (status, body) = call(
                &router,
                region,
                "POST",
                &format!("/v2/campaigns/{id}/stop"),
                &Value::Null,
            )
            .await;
            assert_eq!(
                status, 409,
                "case {step:?}: expected HTTP 409, got {status} {body}"
            );
            assert_eq!(
                body["state"].as_str(),
                Some("Initialized"),
                "case {step:?}: expected the refusal to name state \"Initialized\", got {:?}",
                body["state"]
            );
            let actual = state(&router, region, &id, &step).await;
            assert_eq!(
                actual, "Initialized",
                "case {step:?}: expected the refusal to write nothing, got {actual:?}"
            );
        }

        drive(&router, region, &id, &["start"], &case).await;
        let running = state(&router, region, &id, &case).await;
        assert_eq!(
            running, "Running",
            "case {case:?}: expected the campaign to start after the refusals, got {running:?}"
        );

        // `PutOutboundRequestBatch` takes requests only while the campaign is
        // Running, so this is the end-to-end evidence that the refused stops left
        // the campaign in its pre-run state rather than in Stopped.
        let (status, body) = call(
            &router,
            region,
            "PUT",
            &format!("/v2/campaigns/{id}/outbound-requests"),
            &json!({
                "outboundRequests": [{
                    "clientToken": format!("token-{attempts}"),
                    "expirationTime": instant(3_600.0),
                    "channelSubtypeParameters": {
                        "telephony": {
                            "destinationPhoneNumber": "+15555550100",
                            "attributes": {}
                        }
                    }
                }]
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutOutboundRequestBatch expected 200, got {status} {body}"
        );
    }
}

/// An id no campaign holds is 404 `ResourceNotFoundException` on stop, over several
/// well-formed ids — the refusal is "no such campaign", not anything about one id's
/// shape — and the store is not altered by the attempt.
#[tokio::test]
async fn stopping_an_unknown_campaign_is_404() {
    let region = "ap-northeast-1";
    let router = router();
    let known = create_campaign(&router, region, "instance-known", "known", None).await;
    drive(&router, region, &known, &["start"], "known campaign").await;

    let unknown_ids: Vec<String> = ["missing", "c0ffee", "A-B:C1", "0"]
        .iter()
        .map(|id| (*id).to_string())
        .chain(std::iter::once(format!("{known}x")))
        .collect();
    for unknown in &unknown_ids {
        let (status, body) = call(
            &router,
            region,
            "POST",
            &format!("/v2/campaigns/{unknown}/stop"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {unknown:?}: expected HTTP 404, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {unknown:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );
    }

    let untouched = state(&router, region, &known, "after the failed stops").await;
    assert_eq!(
        untouched, "Running",
        "case \"after the failed stops\": expected the known campaign still Running, got \
         {untouched:?}"
    );
}
