// SPDX-License-Identifier: CC0-1.0

//! The Connect-instance onboarding job, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handlers together. The rules under test:
//!
//! * `StartInstanceOnboardingJob` PERSISTS the `encryptionConfig` it is given as the
//!   named instance's onboarding record and reports that record's job status, which
//!   is `IN_PROGRESS` until the job is due to have settled — whatever the instance id
//!   or the encryption config — and `SUCCEEDED` from then on for a job whose encryption
//!   key the service can use (what a job whose key it cannot use settles on is
//!   `onboarding_job_failure`'s rule);
//! * `GetInstanceOnboardingJobStatus` only REPORTS: polling it never starts or moves
//!   a job, repeated reads at the same point of the job agree, and an instance with
//!   no record is HTTP 404 `ResourceNotFoundException`;
//! * repeating the start call does not restart the job (the operation is
//!   `@idempotent`) and takes the request's encryption config;
//! * a `keyArn` is required when encryption is `enabled`, and the only encryption
//!   type is the one `EncryptionType`'s catalog declares; a config that breaks
//!   either rule is HTTP 400 `ValidationException` and persists nothing.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State, ONBOARDING_JOB_DURATION,
    ONBOARDING_STATUS_IN_PROGRESS, ONBOARDING_STATUS_SUCCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router_with_engine, Harness};

/// The region every case here signs for; nothing in the onboarding record is
/// region-dependent, so one is enough.
const REGION: &str = "us-east-1";

/// `StartInstanceOnboardingJob` for `instance_id` with `encryption_config`, on an
/// instance registered in the account's Connect registry first — onboarding resolves
/// the `connectInstanceId` against it, and the suite covering THAT rule is
/// `instance_onboarding_requires_a_connect_instance`.
async fn start(
    router: &Harness,
    instance_id: &str,
    encryption_config: &Value,
) -> (u16, Value) {
    router.register_connect_instance(REGION, instance_id);
    call(
        router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{instance_id}/onboarding"),
        &json!({"encryptionConfig": encryption_config}),
    )
    .await
}

/// `GetInstanceOnboardingJobStatus` for `instance_id`.
async fn get_status(router: &MultiServiceRouter, instance_id: &str) -> (u16, Value) {
    call(
        router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{instance_id}/onboarding"),
        &Value::Null,
    )
    .await
}

/// The `connectInstanceOnboardingJobStatus$status` a success body carries.
fn job_status(response: &Value) -> &str {
    response
        .get("connectInstanceOnboardingJobStatus")
        .and_then(|status| status.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("<no status>")
}

/// Block until the started job is past its due instant. Nothing in the emulator
/// sleeps for the transition: the record stores the instant, so a case only has to
/// let the clock reach it.
fn wait_past_the_due_instant() {
    std::thread::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150));
}

/// `enabled: false` — the service encrypts with its own key, so no `keyArn`.
fn service_key_config() -> Value {
    json!({"enabled": false})
}

/// Instance ids spanning `InstanceId` (len 1..256, pattern `^[-_.a-zA-Z0-9]+$`, from
/// `smithy_query(query="operation", operation="StartInstanceOnboardingJob")`): the
/// shortest id, the uuid form a Connect instance id takes, every punctuation
/// character the pattern admits, and the longest id the length bound allows.
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

#[test]
fn the_reported_statuses_are_the_modelled_wire_values() {
    // The wire contract, pinned against the model's own catalog
    // (`InstanceOnboardingJobStatusCode`: IN_PROGRESS, SUCCEEDED, FAILED) and against
    // the literals a client matches on, so the constants the cases below compare with
    // cannot drift off the model.
    assert_eq!(
        ONBOARDING_STATUS_IN_PROGRESS, "IN_PROGRESS",
        "case \"unfinished job\": expected the modelled wire value, got {ONBOARDING_STATUS_IN_PROGRESS:?}"
    );
    assert_eq!(
        ONBOARDING_STATUS_SUCCEEDED, "SUCCEEDED",
        "case \"finished job\": expected the modelled wire value, got {ONBOARDING_STATUS_SUCCEEDED:?}"
    );
    for status in [ONBOARDING_STATUS_IN_PROGRESS, ONBOARDING_STATUS_SUCCEEDED] {
        assert!(
            INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES.contains(&status),
            "case {status:?}: expected a value from InstanceOnboardingJobStatusCode's catalog \
             {INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES:?}"
        );
    }
}

#[tokio::test]
async fn a_started_job_is_in_progress_and_the_clock_moves_it_to_succeeded() {
    for (case, instance_id) in instance_ids() {
        let (router, engine) = router_with_engine();

        let (status, body) = start(&router, &instance_id, &service_key_config()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StartInstanceOnboardingJob, got {status} {body}"
        );
        assert_eq!(
            job_status(&body),
            ONBOARDING_STATUS_IN_PROGRESS,
            "case {case:?}: a job the call has only just started is not finished yet, got {body}"
        );
        assert_eq!(
            body["connectInstanceOnboardingJobStatus"]["connectInstanceId"].as_str(),
            Some(instance_id.as_str()),
            "case {case:?}: the status names the instance it onboards, got {body}"
        );
        assert!(
            body["connectInstanceOnboardingJobStatus"]
                .get("failureCode")
                .is_none_or(Value::is_null),
            "case {case:?}: a job that has not failed carries no failureCode, got {body}"
        );

        // The record, not the response, is the source of truth: the config the call
        // carried is what the store holds for this instance.
        {
            let state = engine
                .state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>();
            let store = state.lock();
            let stored = store
                .instance_onboarding(REGION, &instance_id)
                .unwrap_or_else(|| panic!("case {case:?}: the start call stored no record"));
            assert!(
                !stored.encryption_config.enabled,
                "case {case:?}: expected the stored config to be the one sent (enabled=false), \
                 got {:?}",
                stored.encryption_config
            );
            assert_eq!(
                stored.encryption_config.key_arn, None,
                "case {case:?}: expected no stored keyArn, got {:?}",
                stored.encryption_config
            );
        }

        // Reporting the status does not finish the job: the span is never zero, so a
        // caller polling straight after the start call still sees the transient state.
        let (status, body) = get_status(&router, &instance_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetInstanceOnboardingJobStatus, \
             got {status} {body}"
        );
        assert_eq!(
            job_status(&body),
            ONBOARDING_STATUS_IN_PROGRESS,
            "case {case:?}: polling must not finish the job, got {body}"
        );

        wait_past_the_due_instant();

        // Past the due instant the job has finished, and it stays finished: a read
        // is stable, so a second read of the same record agrees with the first.
        for read in 1..=2 {
            let (status, body) = get_status(&router, &instance_id).await;
            assert_eq!(
                status, 200,
                "case {case:?} read {read}: expected HTTP 200, got {status} {body}"
            );
            assert_eq!(
                job_status(&body),
                ONBOARDING_STATUS_SUCCEEDED,
                "case {case:?} read {read}: expected the job to have finished, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn an_instance_with_no_onboarding_job_is_not_found() {
    let (router, _engine) = router_with_engine();

    // One onboarded instance does not onboard any other: each record is keyed by its
    // own instance id.
    let (status, body) = start(&router, "onboarded-instance", &service_key_config()).await;
    assert_eq!(
        status, 200,
        "case \"onboard one instance\": expected HTTP 200, got {status} {body}"
    );

    for case in ["never-onboarded", "onboarded-instance-with-a-suffix", "x"] {
        let (status, body) = get_status(&router, case).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for an instance with no onboarding job, \
             got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {}",
            error_code(&body)
        );
    }
}

#[tokio::test]
async fn repeating_the_start_call_keeps_the_job_and_takes_the_new_config() {
    let (router, engine) = router_with_engine();
    let instance_id = "idempotent-instance";
    let key_arn = "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555";

    let (status, body) = start(&router, instance_id, &service_key_config()).await;
    assert_eq!(
        status, 200,
        "case \"first start\": expected HTTP 200, got {status} {body}"
    );
    wait_past_the_due_instant();

    // The second call reports the job the first one started rather than a fresh one:
    // an instance already onboarded is not pushed back into IN_PROGRESS.
    let config = json!({"enabled": true, "encryptionType": "KMS", "keyArn": key_arn});
    let (status, body) = start(&router, instance_id, &config).await;
    assert_eq!(
        status, 200,
        "case \"repeated start\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        job_status(&body),
        ONBOARDING_STATUS_SUCCEEDED,
        "case \"repeated start\": repeating the call must not restart the job, got {body}"
    );

    // What it does take is the config it carried: the call is the instance's config.
    {
        let state =
            engine.state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>();
        let store = state.lock();
        let stored = store
            .instance_onboarding(REGION, instance_id)
            .expect("case \"repeated start\": the record is still stored");
        assert!(
            stored.encryption_config.enabled,
            "case \"repeated start\": expected the stored config to be the second one, got {:?}",
            stored.encryption_config
        );
        assert_eq!(
            stored.encryption_config.key_arn.as_deref(),
            Some(key_arn),
            "case \"repeated start\": expected the stored keyArn to be the second one, got {:?}",
            stored.encryption_config
        );
    }

    // And the job is still reported as finished afterwards.
    let (status, body) = get_status(&router, instance_id).await;
    assert_eq!(
        status, 200,
        "case \"status after the repeated start\": expected HTTP 200, got {status} {body}"
    );
    assert_eq!(
        job_status(&body),
        ONBOARDING_STATUS_SUCCEEDED,
        "case \"status after the repeated start\": expected the job to still be finished, \
         got {body}"
    );
}

#[tokio::test]
async fn a_kms_key_is_required_exactly_when_encryption_is_enabled() {
    // `keyArn` values are KMS key ARNs; `encryptionType` values are from
    // `EncryptionType`'s catalog (`ENCRYPTION_TYPE_VALUES`, "KMS" alone), plus two
    // values outside it.
    let key_arn = "arn:aws:kms:us-east-1:000000000000:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let cases: Vec<(&str, Value, bool)> = vec![
        ("disabled, no type, no key", json!({"enabled": false}), true),
        (
            "disabled with the supported type and no key",
            json!({"enabled": false, "encryptionType": "KMS"}),
            true,
        ),
        (
            "enabled with the supported type and a key",
            json!({"enabled": true, "encryptionType": "KMS", "keyArn": key_arn}),
            true,
        ),
        (
            "enabled with a key and no type",
            json!({"enabled": true, "keyArn": key_arn}),
            true,
        ),
        (
            "enabled with no key",
            json!({"enabled": true, "encryptionType": "KMS"}),
            false,
        ),
        (
            "enabled with an empty key",
            json!({"enabled": true, "keyArn": ""}),
            false,
        ),
        (
            "disabled with a key (no documented rule forbids it)",
            json!({"enabled": false, "keyArn": key_arn}),
            true,
        ),
        (
            "a type outside the catalog",
            json!({"enabled": true, "encryptionType": "AES256", "keyArn": key_arn}),
            false,
        ),
        (
            "a lowercased catalog value",
            json!({"enabled": true, "encryptionType": "kms", "keyArn": key_arn}),
            false,
        ),
    ];

    for (case, config, accepted) in cases {
        let (router, engine) = router_with_engine();
        let instance_id = "encryption-config-instance";
        let (status, body) = start(&router, instance_id, &config).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} {body}"
            );
            assert_eq!(
                job_status(&body),
                ONBOARDING_STATUS_IN_PROGRESS,
                "case {case:?}: expected a job that has just started, got {body}"
            );
            let state = engine
                .state::<ComAmazonawsConnectcampaignsv2Amazonconnectcampaignservicev2State>();
            let stored = state
                .lock()
                .instance_onboarding(REGION, instance_id)
                .map(|record| {
                    (
                        record.encryption_config.enabled,
                        record.encryption_config.key_arn.clone(),
                        record.encryption_config.encryption_type.clone(),
                    )
                });
            let sent = (
                config.get("enabled").and_then(Value::as_bool).unwrap_or(false),
                config
                    .get("keyArn")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                config
                    .get("encryptionType")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            );
            assert_eq!(
                stored,
                Some(sent.clone()),
                "case {case:?}: expected the accepted config to be persisted exactly as sent \
                 ({sent:?}), got {stored:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {}",
                error_code(&body)
            );
            // A refusal persists nothing, so the instance still has no job at all.
            let (status, body) = get_status(&router, instance_id).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused start stores nothing, so the status read is a 404, \
                 got {status} {body}"
            );
        }
    }
}
