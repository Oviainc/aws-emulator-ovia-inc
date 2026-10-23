// SPDX-License-Identifier: CC0-1.0

//! `DeleteInstanceOnboardingJob`, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints
//! and the handlers together. The rule under test: a Connect instance's onboarding
//! job may be deleted ONLY while it is `FAILED`.
//!
//! * an instance with no onboarding record at all is HTTP 404
//!   `ResourceNotFoundException`;
//! * a job in any status other than `FAILED` — `IN_PROGRESS` while the clock has not
//!   finished it, `SUCCEEDED` after — is refused with HTTP 409
//!   `InvalidStateException`, whatever the instance id;
//! * a refused delete WRITES NOTHING: `GetInstanceOnboardingJobStatus` reports the
//!   same status afterwards, `GetConnectInstanceConfig` still answers, and repeating
//!   the delete is refused the same way.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES;
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    ONBOARDING_JOB_DURATION, ONBOARDING_JOB_NOT_FAILED_MESSAGE, ONBOARDING_STATUS_FAILED,
    ONBOARDING_STATUS_IN_PROGRESS, ONBOARDING_STATUS_SUCCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

use super::common::{call, error_code, router};

/// The region every case here signs for; nothing in the onboarding record is
/// region-dependent, so one is enough.
const REGION: &str = "us-east-1";

/// `StartInstanceOnboardingJob` for `instance_id`, with the service's own key.
async fn start(router: &MultiServiceRouter, instance_id: &str) -> (u16, Value) {
    call(
        router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{instance_id}/onboarding"),
        &json!({"encryptionConfig": {"enabled": false}}),
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

/// `GetConnectInstanceConfig` for `instance_id` — the read a refused delete must
/// leave working.
async fn get_config(router: &MultiServiceRouter, instance_id: &str) -> (u16, Value) {
    call(
        router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{instance_id}/config"),
        &Value::Null,
    )
    .await
}

/// `DeleteInstanceOnboardingJob` for `instance_id`.
async fn delete(router: &MultiServiceRouter, instance_id: &str) -> (u16, Value) {
    call(
        router,
        REGION,
        "DELETE",
        &format!("/v2/connect-instance/{instance_id}/onboarding"),
        &Value::Null,
    )
    .await
}

/// The `connectInstanceOnboardingJobStatus$status` a success body carries.
fn job_status(response: &Value) -> &str {
    response
        .pointer("/connectInstanceOnboardingJobStatus/status")
        .and_then(Value::as_str)
        .unwrap_or("<no status>")
}

/// Instance ids spanning `InstanceId` (len 1..256, pattern `^[-_.a-zA-Z0-9]+$`, from
/// `smithy_query(query="operation", operation="DeleteInstanceOnboardingJob")`): the
/// shortest id, the uuid form a Connect instance id takes, every punctuation
/// character the pattern admits, and the longest id the length bound allows.
fn instance_ids() -> Vec<(&'static str, String)> {
    vec![
        ("one character (length minimum)", "a".to_string()),
        (
            "uuid form",
            "77777777-8888-9999-aaaa-bbbbbbbbbbbb".to_string(),
        ),
        ("every admitted punctuation", "A-z_0.9".to_string()),
        ("256 characters (length maximum)", "d".repeat(256)),
    ]
}

#[test]
fn the_only_deletable_status_is_the_modelled_failed_value() {
    // The wire contract, pinned against the model's own catalog
    // (`InstanceOnboardingJobStatusCode`: IN_PROGRESS, SUCCEEDED, FAILED), so the
    // status the delete edge keys on cannot drift off the model.
    assert_eq!(
        ONBOARDING_STATUS_FAILED, "FAILED",
        "case \"failed job\": expected the modelled wire value, got {ONBOARDING_STATUS_FAILED:?}"
    );
    assert!(
        INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES.contains(&ONBOARDING_STATUS_FAILED),
        "case \"failed job\": expected a value from InstanceOnboardingJobStatusCode's catalog \
         {INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES:?}"
    );
}

#[tokio::test]
async fn an_instance_with_no_onboarding_job_cannot_have_one_deleted() {
    let router = router();

    // One onboarded instance does not give any other a job to delete: each record
    // is keyed by its own instance id.
    let (status, body) = start(&router, "onboarded-instance").await;
    assert_eq!(
        status, 200,
        "case \"onboard one instance\": expected HTTP 200, got {status} {body}"
    );

    for case in [
        "never-onboarded",
        "onboarded-instance-with-a-suffix",
        "x",
        "A-z_0.9",
    ] {
        let (status, body) = delete(&router, case).await;
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

    // The delete attempts left the one real record alone.
    let (status, body) = get_status(&router, "onboarded-instance").await;
    assert_eq!(
        status, 200,
        "case \"the onboarded instance afterwards\": expected HTTP 200, got {status} {body}"
    );
}

#[tokio::test]
async fn a_job_that_has_not_failed_is_refused_and_left_untouched() {
    for (case, instance_id) in instance_ids() {
        let router = router();
        let (status, body) = start(&router, &instance_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from StartInstanceOnboardingJob, got {status} {body}"
        );

        // Two points of the job's life, both of them statuses other than FAILED: the
        // transient one the clock has not finished yet, and the settled one after it.
        for (phase, expected_status) in [
            ("in progress", ONBOARDING_STATUS_IN_PROGRESS),
            ("succeeded", ONBOARDING_STATUS_SUCCEEDED),
        ] {
            if phase == "succeeded" {
                tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150))
                    .await;
            }
            let (status, body) = get_status(&router, &instance_id).await;
            assert_eq!(
                (status, job_status(&body)),
                (200, expected_status),
                "case {case:?} ({phase}): expected the job to be at {expected_status}, \
                 got {status} {body}"
            );

            // Repeating the refused delete is refused the same way: a refusal is not
            // a state change, so the second answer matches the first.
            for attempt in 1..=2 {
                let (status, body) = delete(&router, &instance_id).await;
                assert_eq!(
                    status, 409,
                    "case {case:?} ({phase}) attempt {attempt}: expected HTTP 409 deleting a job \
                     that has not failed, got {status} {body}"
                );
                assert!(
                    error_code(&body).ends_with("InvalidStateException"),
                    "case {case:?} ({phase}) attempt {attempt}: expected InvalidStateException, \
                     got {}",
                    error_code(&body)
                );
                assert_eq!(
                    body.get("message").and_then(Value::as_str),
                    Some(ONBOARDING_JOB_NOT_FAILED_MESSAGE),
                    "case {case:?} ({phase}) attempt {attempt}: expected the service's fixed \
                     refusal text — the same one whatever the instance id — got {body}"
                );
            }

            // The refused delete wrote nothing: the job is still at the same status
            // and the instance's config still reads.
            let (status, body) = get_status(&router, &instance_id).await;
            assert_eq!(
                (status, job_status(&body)),
                (200, expected_status),
                "case {case:?} ({phase}): a refused delete must leave the job at \
                 {expected_status}, got {status} {body}"
            );
        }

        // The unaffected read on the same instance still succeeds, and reports the
        // instance the job onboarded.
        let (status, body) = get_config(&router, &instance_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetConnectInstanceConfig to still answer, got {status} {body}"
        );
        assert_eq!(
            body.pointer("/connectInstanceConfig/connectInstanceId")
                .and_then(Value::as_str),
            Some(instance_id.as_str()),
            "case {case:?}: expected the config of the instance that was onboarded, got {body}"
        );
    }
}
