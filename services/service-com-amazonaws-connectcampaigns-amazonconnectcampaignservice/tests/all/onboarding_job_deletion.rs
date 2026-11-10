// SPDX-License-Identifier: CC0-1.0

//! Deleting the Campaigns onboarding job of an Amazon Connect instance, end to end over
//! HTTP.
//!
//! Every case drives a real request through the router the production entrypoint serves,
//! so each assertion exercises the restJson1 wire protocol, the modelled constraints and
//! the handlers together. The one rule under test, over tables whose rows differ along
//! what it generalises over rather than along one request:
//!
//! * an onboarding job may be DELETED only while it is in the failed status — the record
//!   is then gone, so the instance holds no job any more;
//! * a job in any other status KEEPS its record: HTTP 409 `InvalidStateException` stating
//!   that only a failed job can be deleted, with the status a later read reports and the
//!   stored encryption config untouched;
//! * an instance this service holds no onboarding record for has no job to delete: HTTP
//!   404 `ResourceNotFoundException`, in the same words a read of that absent record is
//!   refused with.


use serde_json::{json, Value};
use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::ops::types::{
    INSTANCE_ONBOARDING_JOB_FAILURE_CODE_VALUES, INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES,
};
use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::state::{
    onboarding_record_absent_message, ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState,
    ONBOARDING_STATUS_FAILED, ONBOARDING_STATUS_IN_PROGRESS, ONBOARDING_STATUS_SUCCEEDED,
};

use super::common::{call, job_status, reach_the_due_instant, router, Answer, Harness, KEY_ARN};

/// The region every case here signs for; nothing in an onboarding record is
/// region-dependent, so one is enough.
const REGION: &str = "us-east-1";

/// The sentence a request about an instance's onboarding RECORD is refused with when
/// there is none. Written out here rather than read back out of the implementation, so
/// this suite pins the wording itself; `{account}` and `{id}` are where the account the
/// record would belong to and the request's instance id go.
const RECORD_ABSENT_WORDING: &str =
    "Instance onboarding record does not exist for aws account id {account} and instance id {id}";

/// The sentence a delete of a job that has not failed is refused with. It carries nothing
/// from the request, so there is no `{id}` to fill.
const NOT_FAILED_WORDING: &str =
    "Instance onboarding job is not in failed status. Only failed job can be deleted.";

/// `wording` with the request's instance `id` — and the caller's account id, which the
/// emulator presents as its own canonical one — in the places it names.
fn worded(wording: &str, id: &str) -> String {
    wording
        .replace("{id}", id)
        .replace("{account}", emulator_server::emulator_account_id())
}

/// `StartInstanceOnboardingJob` for `instance` with `encryption_config`.
async fn start(harness: &Harness, instance: &str, encryption_config: &Value) -> Answer {
    call(
        harness,
        REGION,
        "PUT",
        &format!("/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": encryption_config}),
    )
    .await
}

/// `GetInstanceOnboardingJobStatus` for `instance`.
async fn status(harness: &Harness, instance: &str) -> Answer {
    call(
        harness,
        REGION,
        "GET",
        &format!("/connect-instance/{instance}/onboarding"),
        &Value::Null,
    )
    .await
}

/// `DeleteInstanceOnboardingJob` for `instance`.
async fn delete(harness: &Harness, instance: &str) -> Answer {
    call(
        harness,
        REGION,
        "DELETE",
        &format!("/connect-instance/{instance}/onboarding"),
        &Value::Null,
    )
    .await
}

/// Give the account `instance` and onboard it with `encryption_config`, the fixture every
/// case that needs a stored onboarding record starts from.
async fn onboard(harness: &Harness, case: &str, instance: &str, encryption_config: &Value) {
    harness.register_connect_instance(REGION, instance);
    let started = start(harness, instance, encryption_config).await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: expected the onboarding fixture to be accepted, got {} {}",
        started.status, started.body
    );
}

/// Whether the store holds an onboarding record for `instance`.
fn record_held(harness: &Harness, instance: &str) -> bool {
    let state = harness
        .engine()
        .state::<ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState>();
    let store = state.lock();
    store.instance_onboarding(instance).is_some()
}

/// The `keyArn` the store holds for `instance`'s onboarding, or `None` when it holds no
/// record — what a case reads to see that a refused delete wrote nothing.
fn stored_key_arn(harness: &Harness, instance: &str) -> Option<Option<String>> {
    let state = harness
        .engine()
        .state::<ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState>();
    let store = state.lock();
    store
        .instance_onboarding(instance)
        .map(|record| record.encryption_config.key_arn.clone())
}

/// End `instance`'s onboarding job on `failure_code`, the status its record reports as
/// FAILED.
///
/// Written into the record the start call stored, because no operation of this crate
/// fails a job: every `InstanceOnboardingJobFailureCode` names a dependency of the
/// onboarding outside this service, and the onboarding this emulator performs is writing
/// the record. It is the same seam [`super::common::reach_the_due_instant`] uses to reach
/// a point of the job the wall clock would otherwise have to be waited out for.
fn fail_the_job(harness: &Harness, instance: &str, failure_code: &str) {
    let state = harness
        .engine()
        .state::<ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState>();
    let mut store = state.lock();
    let record = store
        .instance_onboarding
        .get_mut(instance)
        .unwrap_or_else(|| panic!("case {instance:?}: the start call stored no record"));
    record.failure_code = Some(failure_code.to_string());
}

/// Instance ids and encryption configs spanning what the rule generalises over:
/// `InstanceId` (len 0..256, pattern `^[a-zA-Z0-9_\-.]*$`, from
/// `smithy_query(query="shape", shape="InstanceId")`) — the shortest id a path label can
/// carry, the uuid form a Connect instance id takes, every punctuation character the
/// pattern admits, the longest id the bound allows — each paired with a different
/// `EncryptionConfig` form.
fn onboarded_rows() -> Vec<(&'static str, String, Value)> {
    vec![
        (
            "one character, the service's own key",
            "d".to_string(),
            json!({"enabled": false}),
        ),
        (
            "uuid form, a customer KMS key",
            "6f2b9a04-71c3-4ed8-9b55-30ac1e7f2b40".to_string(),
            json!({"enabled": true, "encryptionType": "KMS", "keyArn": KEY_ARN}),
        ),
        (
            "every admitted punctuation, the supported type with no key",
            "a.b_c-D9".to_string(),
            json!({"enabled": false, "encryptionType": "KMS"}),
        ),
        (
            "256 characters, a key with no type",
            "k".repeat(256),
            json!({"enabled": true, "keyArn": KEY_ARN}),
        ),
    ]
}

#[test]
fn the_failed_status_is_a_modelled_wire_value() {
    // The status the delete admits, and the codes a failed job reports, are the model's
    // own catalogs (`InstanceOnboardingJobStatusCode`: FAILED, IN_PROGRESS, SUCCEEDED;
    // `InstanceOnboardingJobFailureCode`), so the status the cases below drive a job into
    // cannot drift off the model.
    assert!(
        INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES.contains(&ONBOARDING_STATUS_FAILED),
        "case \"the failed status\": expected a value from InstanceOnboardingJobStatusCode's \
         catalog {INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES:?}, got {ONBOARDING_STATUS_FAILED:?}"
    );
    assert!(
        !INSTANCE_ONBOARDING_JOB_FAILURE_CODE_VALUES.is_empty(),
        "case \"the failure-code catalog\": expected the model's codes, got none"
    );
}

#[tokio::test]
async fn a_failed_job_is_deleted_and_the_instance_then_holds_none() {
    // One row per `InstanceOnboardingJobFailureCode` the model declares (from
    // `smithy_query(query="shape", shape="InstanceOnboardingJobFailureCode")`, re-exported
    // as INSTANCE_ONBOARDING_JOB_FAILURE_CODE_VALUES), each on its own instance and on a
    // different encryption-config form: whatever ended the job, a failed job is deletable.
    let configs = onboarded_rows();
    for (index, failure_code) in INSTANCE_ONBOARDING_JOB_FAILURE_CODE_VALUES
        .iter()
        .enumerate()
    {
        let (form, _, config) = &configs[index % configs.len()];
        let case = format!("{failure_code} on {form}");
        let instance = format!("failed-{}", failure_code.to_lowercase().replace('_', "-"));

        let harness = router();
        onboard(&harness, &case, &instance, config).await;
        fail_the_job(&harness, &instance, failure_code);

        // The job reports the failure before the delete, so the case deletes a job the
        // API itself shows as failed rather than one only the store knows about.
        let reported = status(&harness, &instance).await;
        assert_eq!(
            (reported.status, job_status(&reported)),
            (200, ONBOARDING_STATUS_FAILED),
            "case {case:?}: expected the job to report the failed status, got {} {}",
            reported.status,
            reported.body
        );
        assert_eq!(
            reported
                .body
                .pointer("/connectInstanceOnboardingJobStatus/failureCode")
                .and_then(Value::as_str),
            Some(*failure_code),
            "case {case:?}: expected the failure code the job ended on, got {}",
            reported.body
        );

        let deleted = delete(&harness, &instance).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected HTTP 200 from DeleteInstanceOnboardingJob, got {} {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.body.is_null() || deleted.body == json!({}),
            "case {case:?}: expected an empty response body, got {}",
            deleted.body
        );

        // The record, not the response, is the source of truth: the delete dropped it,
        // so the reader reports no job for the instance any more.
        assert!(
            !record_held(&harness, &instance),
            "case {case:?}: expected the onboarding record to be gone from the store"
        );
        let after = status(&harness, &instance).await;
        assert_eq!(
            after.status, 404,
            "case {case:?}: expected the deleted job to be read back as absent, got {} {}",
            after.status, after.body
        );
        assert_eq!(
            after.message(),
            worded(RECORD_ABSENT_WORDING, &instance),
            "case {case:?}: expected the record-absent wording, got {}",
            after.body
        );

        // Nothing is left to delete, so the delete that follows is the not-found refusal
        // and not a second success.
        let again = delete(&harness, &instance).await;
        assert_eq!(
            (again.status, again.error_code.ends_with("ResourceNotFoundException")),
            (404, true),
            "case {case:?}: expected a second delete to find no job, got {} {:?} {}",
            again.status,
            again.error_code,
            again.body
        );
    }
}

#[tokio::test]
async fn a_job_that_has_not_failed_is_refused_and_left_untouched() {
    // The two statuses this emulator's onboarding reaches, each over every instance-id
    // and encryption-config form: the status is what decides the refusal, not the id.
    for (case, instance, config) in onboarded_rows() {
        for (point, finish) in [
            ("just started", false),
            ("past the due instant", true),
        ] {
            let case = format!("{case}, {point}");
            let harness = router();
            onboard(&harness, &case, &instance, &config).await;
            if finish {
                reach_the_due_instant(&harness, &instance);
            }
            let expected_status = if finish {
                ONBOARDING_STATUS_SUCCEEDED
            } else {
                ONBOARDING_STATUS_IN_PROGRESS
            };

            let refused = delete(&harness, &instance).await;
            assert_eq!(
                refused.status, 409,
                "case {case:?}: expected HTTP 409 for a job that has not failed, got {} {}",
                refused.status, refused.body
            );
            assert!(
                refused.error_code.ends_with("InvalidStateException"),
                "case {case:?}: expected InvalidStateException, got {:?}",
                refused.error_code
            );
            assert_eq!(
                refused.message(),
                NOT_FAILED_WORDING,
                "case {case:?}: expected the only-a-failed-job wording, got {}",
                refused.body
            );

            // The refusal wrote nothing: the record is still there, still reports the
            // status it did, and still holds the config the start call carried.
            assert_eq!(
                stored_key_arn(&harness, &instance),
                Some(
                    config
                        .get("keyArn")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                ),
                "case {case:?}: expected the stored encryption config to be untouched, got {:?}",
                stored_key_arn(&harness, &instance)
            );
            let reported = status(&harness, &instance).await;
            assert_eq!(
                (reported.status, job_status(&reported)),
                (200, expected_status),
                "case {case:?}: expected the refused job to still report {expected_status}, got {} {}",
                reported.status,
                reported.body
            );
        }
    }
}

#[tokio::test]
async fn an_instance_with_no_onboarding_job_cannot_have_one_deleted() {
    let harness = router();
    let onboarded = "onboarded-instance";
    onboard(
        &harness,
        "onboard one instance",
        onboarded,
        &json!({"enabled": false}),
    )
    .await;

    // One onboarded instance onboards no other: a record is keyed by its own instance
    // id, so an id the account holds without having onboarded it, an id it does not hold
    // at all, and an id that merely resembles the onboarded one are all misses — and so
    // is every id form the shape admits.
    let held_but_not_onboarded = "held-but-not-onboarded";
    harness.register_connect_instance(REGION, held_but_not_onboarded);
    let rows: Vec<(&str, String)> = vec![
        (
            "an instance the account holds but nobody onboarded",
            held_but_not_onboarded.to_string(),
        ),
        ("an id the account does not hold", "never-registered".to_string()),
        ("the onboarded id with a suffix", format!("{onboarded}-2")),
        ("one character", "x".to_string()),
        (
            "the uuid form",
            "3d81c4a7-6b20-4f59-8e13-70c5a9d4e2b1".to_string(),
        ),
        ("every admitted punctuation", "q.r_s-T0".to_string()),
        ("256 characters (the length maximum)", "y".repeat(256)),
    ];

    for (case, instance) in rows {
        let refused = delete(&harness, &instance).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404 for an instance with no onboarding job, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            worded(RECORD_ABSENT_WORDING, &instance),
            "case {case:?}: expected the record-absent wording around the account and the id \
             the request carried, got {}",
            refused.body
        );
        assert_eq!(
            refused.message(),
            onboarding_record_absent_message(&instance),
            "case {case:?}: expected the same sentence a read of that absent record is refused \
             with, got {}",
            refused.body
        );
    }

    // The refused deletes left the one stored record alone.
    assert!(
        record_held(&harness, onboarded),
        "case \"the onboarded instance\": expected its record to survive every refused delete"
    );
}
