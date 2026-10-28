// SPDX-License-Identifier: CC0-1.0

//! Onboarding a Connect instance to Campaigns needs that instance to exist, end to end
//! over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves, so
//! each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handlers together. The rule under test:
//!
//! * `StartInstanceOnboardingJob` configures an EXISTING Amazon Connect instance, so a
//!   `connectInstanceId` the caller's Amazon Connect account holds no instance for is
//!   refused with HTTP 404 `ResourceNotFoundException` and persists nothing — no
//!   onboarding job to report, no instance configuration to read and no campaign
//!   target for `CreateCampaign`;
//! * the same call for an id that account DOES hold starts the job, whatever form the
//!   id takes and whichever region's endpoint it is signed for;
//! * the registry is matched on the id as a whole: an id that merely resembles a
//!   registered one, in an account holding several instances, is still refused.
//!
//! The rows differ along what the rule generalises over: the form of the instance id
//! (its length bounds, the punctuation its pattern admits, the uuid shape a Connect
//! instance id has) and whether the account holds an instance under it.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State, INSTANCE_NOT_FOUND_MESSAGE,
    ONBOARDING_JOB_DURATION, ONBOARDING_STATUS_IN_PROGRESS,
};

use super::common::{call, error_code, router_with_engine, Harness};

/// The region the cases sign for unless they are about more than one.
const REGION: &str = "us-east-1";

/// A second region, for the case that onboards one registered instance through two
/// endpoints: the Connect instance registry is the ACCOUNT's, so it answers the same
/// question whichever regional endpoint asks it.
const OTHER_REGION: &str = "eu-west-1";

/// Instance ids spanning `InstanceId` (len 1..256, pattern `^[-_.a-zA-Z0-9]+$`, from
/// `smithy_query(query="operation", operation="StartInstanceOnboardingJob")`): the
/// shortest id, the uuid form a Connect instance id takes, every punctuation character
/// the pattern admits, and the longest id the length bound allows.
fn instance_ids() -> Vec<(&'static str, String)> {
    vec![
        ("one character (length minimum)", "a".to_string()),
        (
            "uuid form",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        ("every admitted punctuation", "A-z_0.9".to_string()),
        ("256 characters (length maximum)", "i".repeat(256)),
    ]
}

/// `StartInstanceOnboardingJob` for `instance`, with a config no rule refuses, so the
/// only thing a refusal can be about is the instance.
async fn start(router: &Harness, region: &str, instance: &str) -> (u16, Value) {
    call(
        router,
        region,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await
}

/// `GetInstanceOnboardingJobStatus` for `instance`.
async fn job_status(router: &Harness, region: &str, instance: &str) -> (u16, Value) {
    call(
        router,
        region,
        "GET",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &Value::Null,
    )
    .await
}

/// `GetConnectInstanceConfig` for `instance`.
async fn instance_config(router: &Harness, region: &str, instance: &str) -> (u16, Value) {
    call(
        router,
        region,
        "GET",
        &format!("/v2/connect-instance/{instance}/config"),
        &Value::Null,
    )
    .await
}

/// `CreateCampaign` on `instance` under `name` — the call the onboarding record is the
/// gate for.
async fn create_campaign(
    router: &Harness,
    region: &str,
    instance: &str,
    name: &str,
) -> (u16, Value) {
    call(
        router,
        region,
        "PUT",
        "/v2/campaigns",
        &json!({
            "name": name,
            "connectInstanceId": instance,
            // `outboundMode` and `defaultOutboundConfig` are both required by the model.
            "channelSubtypeConfig": {"telephony": {
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"},
            }},
        }),
    )
    .await
}

/// Assert `(status, body)` is the modelled 404 for an instance the account does not
/// hold: the service's FIXED text for an instance that is not usable by Campaigns,
/// which carries nothing from the request, so it reads the same whatever id was asked
/// about.
fn assert_instance_not_found(case: &str, what: &str, status: u16, body: &Value, instance: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404 from {what} for {instance:?}, got {status} {body}"
    );
    assert!(
        error_code(body).ends_with("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException from {what}, got {:?}",
        error_code(body)
    );
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(INSTANCE_NOT_FOUND_MESSAGE),
        "case {case:?}: expected the service's fixed text for an instance not usable by \
         Campaigns from {what}, got {body}"
    );
}

#[tokio::test]
async fn onboarding_an_instance_the_account_does_not_hold_is_refused_and_stores_nothing() {
    for (case, instance) in instance_ids() {
        let (router, engine) = router_with_engine();

        // The account holds instances — just not this one — so the refusal is the
        // registry lookup failing rather than an empty emulator refusing everything.
        router.register_connect_instance(REGION, "a-registered-instance");
        router.register_connect_instance(REGION, &format!("{instance}-registered"));

        let (status, body) = start(&router, REGION, &instance).await;
        assert_instance_not_found(case, "StartInstanceOnboardingJob", status, &body, &instance);

        // Nothing was persisted: there is no job to report, no configuration to read
        // and no campaign target, and the store holds no record at all.
        let (status, body) = job_status(&router, REGION, &instance).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused onboarding leaves no job to report, got {status} {body}"
        );
        let (status, body) = instance_config(&router, REGION, &instance).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused onboarding leaves no configuration, got {status} {body}"
        );
        let (status, body) = create_campaign(&router, REGION, &instance, "refused-target").await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused onboarding leaves no campaign target, got {status} {body}"
        );

        let state =
            engine.state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>();
        let stored = state.lock().instance_onboarding(REGION, &instance).is_some();
        assert!(
            !stored,
            "case {case:?}: a refused onboarding stores no record, yet the store holds one"
        );
    }
}

#[tokio::test]
async fn onboarding_an_instance_the_account_holds_starts_its_job() {
    for (case, instance) in instance_ids() {
        let (router, _engine) = router_with_engine();
        router.register_connect_instance(REGION, &instance);

        let (status, body) = start(&router, REGION, &instance).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for an instance the account holds, \
             got {status} {body}"
        );
        assert_eq!(
            body.pointer("/connectInstanceOnboardingJobStatus/status")
                .and_then(Value::as_str),
            Some(ONBOARDING_STATUS_IN_PROGRESS),
            "case {case:?}: expected a job that has only just started, got {body}"
        );
        assert_eq!(
            body.pointer("/connectInstanceOnboardingJobStatus/connectInstanceId")
                .and_then(Value::as_str),
            Some(instance.as_str()),
            "case {case:?}: expected the status to name the instance it onboards, got {body}"
        );

        // And the record it wrote is what the reads report, once the clock has reached
        // the instant the job is due at.
        tokio::time::sleep(ONBOARDING_JOB_DURATION + Duration::from_millis(100)).await;
        let (status, body) = job_status(&router, REGION, &instance).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the started job to be reported, got {status} {body}"
        );
        let (status, body) = instance_config(&router, REGION, &instance).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the onboarded instance to have a configuration, \
             got {status} {body}"
        );
        let (status, body) = create_campaign(&router, REGION, &instance, "onboarded-target").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the onboarded instance to be a campaign target, \
             got {status} {body}"
        );
    }
}

#[tokio::test]
async fn an_id_that_only_resembles_a_registered_instance_is_refused() {
    // One registered instance per row, and the id the request names instead: a prefix,
    // a suffix, the same characters in another case, and a punctuation character
    // swapped for another the pattern admits.
    let cases: Vec<(&str, &str, String)> = vec![
        ("a suffix on the registered id", "near-miss", "near-miss-copy".to_string()),
        ("a prefix on the registered id", "near-miss", "x-near-miss".to_string()),
        ("the registered id truncated", "near-miss", "near-mis".to_string()),
        ("another case of the registered id", "near-miss", "NEAR-MISS".to_string()),
        ("another admitted separator", "near.miss", "near_miss".to_string()),
    ];

    for (case, registered, requested) in cases {
        let (router, _engine) = router_with_engine();
        router.register_connect_instance(REGION, registered);

        let (status, body) = start(&router, REGION, &requested).await;
        assert_instance_not_found(case, "StartInstanceOnboardingJob", status, &body, &requested);

        // The registered instance itself is unaffected: it is still onboardable.
        let (status, body) = start(&router, REGION, registered).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the registered instance to onboard, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn an_instance_the_account_holds_is_onboardable_through_every_region() {
    let (router, _engine) = router_with_engine();
    let instance = "account-wide-instance";
    router.register_connect_instance(REGION, instance);

    // The instance registry is the account's, not one region's, so both endpoints
    // resolve the id — each writing its own region's onboarding record, which is why
    // both answer the transient status.
    for region in [REGION, OTHER_REGION] {
        let (status, body) = start(&router, region, instance).await;
        assert_eq!(
            (status, body.pointer("/connectInstanceOnboardingJobStatus/status").and_then(Value::as_str)),
            (200, Some(ONBOARDING_STATUS_IN_PROGRESS)),
            "case {region:?}: expected the account's instance to onboard, got {status} {body}"
        );
    }

    // An id the account does not hold is refused at both endpoints alike.
    let unknown = "no-such-instance";
    for region in [REGION, OTHER_REGION] {
        let (status, body) = start(&router, region, unknown).await;
        assert_instance_not_found(region, "StartInstanceOnboardingJob", status, &body, unknown);
    }
}
