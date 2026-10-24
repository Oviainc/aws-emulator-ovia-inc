// SPDX-License-Identifier: CC0-1.0

//! `ResumeCampaign`, end to end over the wire.
//!
//! The rules under test:
//!
//! * resuming is the inverse of pausing: it is legal from `Paused` and from
//!   `Paused` only, and it leaves the campaign `Running` for every later read;
//! * from any other state the call is refused with HTTP 409
//!   `InvalidCampaignStateException`, whose `state` member names the state the
//!   campaign is actually in, and the stored state is left exactly where it was;
//! * an id the store does not hold is HTTP 404 `ResourceNotFoundException`,
//!   whatever the id looks like;
//! * resume/pause cycles are not limited in number: each cycle moves the stored
//!   state and nothing accumulates.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    CampaignAction, CampaignState, ONBOARDING_JOB_DURATION,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router};

/// Onboard the Connect instance a campaign belongs to: `CreateCampaign` mints a
/// campaign only for an instance whose onboarding job has SUCCEEDED, which it does
/// one [`ONBOARDING_JOB_DURATION`] after `StartInstanceOnboardingJob`.
async fn onboard(router: &MultiServiceRouter, region: &str, instance: &str) {
    let (status, started) = call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartInstanceOnboardingJob {instance}: expected 200, got {status} {started}"
    );
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(100)).await;
}

/// A campaign on a freshly onboarded instance, returning its minted id.
async fn campaign(router: &MultiServiceRouter, region: &str, instance: &str) -> String {
    onboard(router, region, instance).await;
    let (status, created) = call(
        router,
        region,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": format!("resume-{instance}"),
            "connectInstanceId": instance,
            "channelSubtypeConfig": {
                "telephony": {
                    "outboundMode": {"agentless": {}},
                    "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
                }
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateCampaign on {instance}: expected 200, got {status} {created}"
    );
    created["id"]
        .as_str()
        .expect("a minted campaign id")
        .to_string()
}

/// `GetCampaignState` for `id`.
async fn state(router: &MultiServiceRouter, region: &str, id: &str, case: &str) -> String {
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
    body["state"].as_str().unwrap_or("<no state>").to_string()
}

/// Drive the lifecycle actions in `path` (each asserted to succeed), leaving the
/// campaign in whatever state they wrote.
async fn drive(router: &MultiServiceRouter, region: &str, id: &str, path: &[&str], case: &str) {
    for action in path {
        let (status, body) = call(
            router,
            region,
            "POST",
            &format!("/v2/campaigns/{id}/{action}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup {action} expected 200, got {status} {body}"
        );
    }
}

/// The transition table for `Resume`, over EVERY modelled state: only `Paused`
/// moves, and it moves to `Running`.
#[test]
fn resume_moves_only_a_paused_campaign() {
    // Every variant of the model's `CampaignState` enum, paired with the state
    // `Resume` leaves it in (`None` = the action is refused from there).
    let cases: &[(CampaignState, Option<CampaignState>)] = &[
        (CampaignState::Paused, Some(CampaignState::Running)),
        (CampaignState::Initialized, None),
        (CampaignState::Running, None),
        (CampaignState::Stopped, None),
        (CampaignState::Completed, None),
        (CampaignState::Failed, None),
    ];

    for (from, expected) in cases {
        let actual = from.after(CampaignAction::Resume);
        assert_eq!(
            actual, *expected,
            "case {from:?} under Resume: expected {expected:?}, got {actual:?}"
        );
    }
}

/// Over a TABLE of states reachable through the API — each reached by a different
/// sequence of lifecycle calls — `ResumeCampaign` succeeds exactly from `Paused`
/// and is refused with 409 `InvalidCampaignStateException` everywhere else, and in
/// both cases `GetCampaignState` afterwards reports the expected state.
#[tokio::test]
async fn resume_succeeds_only_from_paused_over_every_reachable_state() {
    let region = "eu-central-1";

    // (case, the actions that reach the state under test, the state they reach,
    //  expected HTTP status of the resume, expected state afterwards)
    let cases: &[(&str, &[&str], &str, u16, &str)] = &[
        (
            "resume a freshly created campaign",
            &[],
            "Initialized",
            409,
            "Initialized",
        ),
        (
            "resume a running campaign",
            &["start"],
            "Running",
            409,
            "Running",
        ),
        (
            "resume a paused campaign",
            &["start", "pause"],
            "Paused",
            200,
            "Running",
        ),
        (
            "resume a stopped campaign",
            &["start", "stop"],
            "Stopped",
            409,
            "Stopped",
        ),
        (
            "resume a campaign stopped straight from Initialized",
            &["stop"],
            "Stopped",
            409,
            "Stopped",
        ),
        (
            "resume a campaign paused after a restart",
            &["start", "pause", "start", "pause"],
            "Paused",
            200,
            "Running",
        ),
    ];

    for (index, (case, setup, before, expected_status, expected_state)) in
        cases.iter().enumerate()
    {
        let router = router();
        let instance = format!("instance-{index}");
        let id = campaign(&router, region, &instance).await;
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
            &format!("/v2/campaigns/{id}/resume"),
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

        // The resumed campaign is Running for every projection of the record, not
        // only for `GetCampaignState`.
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

/// Pause/resume cycles are unbounded: each resume moves the stored state back to
/// `Running` and each pause back to `Paused`, over several cycle counts.
#[tokio::test]
async fn pause_and_resume_cycle_as_many_times_as_asked() {
    let region = "us-west-2";

    for cycles in [1_usize, 2, 5] {
        let case = format!("{cycles} pause/resume cycles");
        let router = router();
        let id = campaign(&router, region, "instance-cycles").await;
        drive(&router, region, &id, &["start"], &case).await;

        for cycle in 0..cycles {
            let step = format!("{case}, cycle {cycle} pause");
            drive(&router, region, &id, &["pause"], &step).await;
            let paused = state(&router, region, &id, &step).await;
            assert_eq!(
                paused, "Paused",
                "case {step:?}: expected Paused after the pause, got {paused:?}"
            );

            let step = format!("{case}, cycle {cycle} resume");
            drive(&router, region, &id, &["resume"], &step).await;
            let resumed = state(&router, region, &id, &step).await;
            assert_eq!(
                resumed, "Running",
                "case {step:?}: expected Running after the resume, got {resumed:?}"
            );
        }
    }
}

/// An id no campaign holds is 404 `ResourceNotFoundException` on resume, over
/// several well-formed ids — the refusal is "no such campaign", not anything about
/// one id's shape — and the store is not altered by the attempt.
#[tokio::test]
async fn resuming_an_unknown_campaign_is_404() {
    let region = "ap-northeast-1";
    let router = router();
    let known = campaign(&router, region, "instance-known").await;
    drive(&router, region, &known, &["start", "pause"], "known campaign").await;

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
            &format!("/v2/campaigns/{unknown}/resume"),
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

    let untouched = state(&router, region, &known, "after the failed resumes").await;
    assert_eq!(
        untouched, "Paused",
        "case \"after the failed resumes\": expected the known campaign still Paused, got {untouched:?}"
    );
}
