// SPDX-License-Identifier: CC0-1.0

//! The Campaigns onboarding of an Amazon Connect instance, end to end over HTTP.
//!
//! Every case drives a real request through the router the production entrypoint
//! serves, so each assertion exercises the restJson1 wire protocol, the modelled
//! constraints and the handlers together. The rules under test, each over a table whose
//! rows differ along what the rule generalises over rather than along one request:
//!
//! * `StartInstanceOnboardingJob` PERSISTS the `encryptionConfig` it is given as the
//!   named instance's onboarding record and reports that record's job status, which is
//!   in progress until the job is due to have finished and succeeded from then on —
//!   whatever the instance id or the encryption config;
//! * `GetInstanceOnboardingJobStatus` only REPORTS: polling never starts or finishes a
//!   job, repeated reads at the same point of the job agree, and an instance this
//!   service holds no RECORD for is HTTP 404 `ResourceNotFoundException` naming the
//!   account the record would belong to and the instance the request asked about;
//! * onboarding resolves the `connectInstanceId` against Amazon Connect's own instance
//!   registry, so an id the account does not hold is HTTP 404 — in the words that
//!   operation refuses an instance it cannot find — and persists nothing;
//! * an instance that already holds an onboarding job is not onboarded a second time —
//!   HTTP 409 `ConflictException`, with the stored record left exactly as it was;
//! * a `keyArn` is required when encryption is `enabled`; a config that breaks that rule
//!   is HTTP 400 `ValidationException` and persists nothing.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::ops::types::INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES;
use service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::state::{
    connect_instance_not_found_message, onboarding_record_absent_message,
    ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState,
    INSTANCE_ALREADY_ONBOARDED_MESSAGE, ONBOARDING_JOB_DURATION, ONBOARDING_STATUS_IN_PROGRESS,
    ONBOARDING_STATUS_SUCCEEDED,
};

use super::common::{call, router, Answer, Harness};

/// The region every case here signs for; nothing in an onboarding record is
/// region-dependent, so one is enough.
const REGION: &str = "us-east-1";

/// The sentence a request asking to ONBOARD an instance the account does not hold is
/// refused with, as fixed text around the id the request carried. Written out here rather
/// than read back out of the implementation, so this suite pins the wording itself;
/// `{id}` is where the request's instance id goes.
const NOT_FOUND_WORDING: &str = "ConnectInstanceId not found {id}.";

/// The sentence a request asking about an instance's onboarding RECORD is refused with
/// when there is none: the record is account-scoped, so it names the account and the
/// instance, and carries no trailing period. `{account}` and `{id}` are where those go.
const RECORD_ABSENT_WORDING: &str =
    "Instance onboarding record does not exist for aws account id {account} and instance id {id}";

/// The sentence an instance that is already onboarded is refused with — the one the
/// model documents for `ConflictException`. It carries nothing from the request, so
/// there is no `{id}` to fill.
const ALREADY_ONBOARDED_WORDING: &str =
    "The request could not be processed because of conflict in the current state of the resource.";

/// `wording` with the request's instance `id` — and the caller's account id, which the
/// emulator presents as its own canonical one — in the places it names.
fn worded(wording: &str, id: &str) -> String {
    wording
        .replace("{id}", id)
        .replace("{account}", emulator_server::emulator_account_id())
}

/// `StartInstanceOnboardingJob` for `instance` with `encryption_config`, exactly as
/// sent: no fixture step is folded in, so a case decides for itself whether the account
/// holds the instance.
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

/// The `connectInstanceOnboardingJobStatus$status` a success body carries.
fn job_status(answer: &Answer) -> &str {
    answer
        .body
        .pointer("/connectInstanceOnboardingJobStatus/status")
        .and_then(Value::as_str)
        .unwrap_or("<no status>")
}

/// The `enabled` / `keyArn` / `encryptionType` the STORE holds for `instance`, or `None`
/// when it holds no onboarding record at all.
///
/// The encryption config a start call persists is reported by no operation this slice
/// implements, so the store is where a case sees whether the write happened.
fn stored_config(harness: &Harness, instance: &str) -> Option<(bool, Option<String>, Option<String>)> {
    let state = harness
        .engine()
        .state::<ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState>();
    let store = state.lock();
    store.instance_onboarding(instance).map(|record| {
        (
            record.encryption_config.enabled,
            record.encryption_config.key_arn.clone(),
            record.encryption_config.encryption_type.clone(),
        )
    })
}

/// The `enabled` / `keyArn` / `encryptionType` a request body carried, in the shape
/// [`stored_config`] reports — so a case compares what it sent with what was kept
/// instead of with a literal written twice.
fn sent_config(encryption_config: &Value) -> (bool, Option<String>, Option<String>) {
    (
        encryption_config
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        encryption_config
            .get("keyArn")
            .and_then(Value::as_str)
            .map(str::to_string),
        encryption_config
            .get("encryptionType")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

/// Put the clock past the instant `instance`'s job is due to have finished, without
/// waiting for it: the record carries that instant, so moving it is the same as letting
/// the clock reach it. The suite also proves the wall clock alone does it, in
/// [`the_clock_alone_finishes_a_started_job`].
fn reach_the_due_instant(harness: &Harness, instance: &str) {
    let state = harness
        .engine()
        .state::<ComAmazonawsConnectcampaignsAmazonconnectcampaignserviceState>();
    let mut store = state.lock();
    let record = store
        .instance_onboarding
        .get_mut(instance)
        .unwrap_or_else(|| panic!("case {instance:?}: the start call stored no record"));
    record.succeeds_at = SystemTime::now() - Duration::from_secs(1);
}

/// `enabled: false` — the service encrypts with its own key, so no `keyArn`.
fn service_key_config() -> Value {
    json!({"enabled": false})
}

/// A KMS key ARN, the form `EncryptionConfig$keyArn` takes (an `EncryptionKey`, len
/// 0..500, from `smithy_query(query="shape", shape="EncryptionConfig")`).
const KEY_ARN: &str = "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555";

/// Instance ids and encryption configs spanning what the rule generalises over:
/// `InstanceId` (len 0..256, pattern `^[a-zA-Z0-9_\-.]*$`, from
/// `smithy_query(query="shape", shape="InstanceId")`) — the shortest id a path label can
/// carry, the uuid form a Connect instance id takes, every punctuation character the
/// pattern admits, the longest id the bound allows — each paired with a different
/// `EncryptionConfig` form.
fn onboarding_rows() -> Vec<(&'static str, String, Value)> {
    vec![
        (
            "one character, the service's own key",
            "a".to_string(),
            service_key_config(),
        ),
        (
            "uuid form, a customer KMS key",
            "1b4d2c7e-0a55-4f81-9d2c-7e0a554f8100".to_string(),
            json!({"enabled": true, "encryptionType": "KMS", "keyArn": KEY_ARN}),
        ),
        (
            "every admitted punctuation, the supported type with no key",
            "a.b_c-D9".to_string(),
            json!({"enabled": false, "encryptionType": "KMS"}),
        ),
        (
            "256 characters, a key with no type",
            "i".repeat(256),
            json!({"enabled": true, "keyArn": KEY_ARN}),
        ),
    ]
}

#[test]
fn the_reported_statuses_are_the_modelled_wire_values() {
    // The wire contract, pinned against the model's own catalog
    // (`InstanceOnboardingJobStatusCode`: FAILED, IN_PROGRESS, SUCCEEDED) and against
    // the literals a client matches on, so the constants the cases below compare with
    // cannot drift off the model.
    for (case, value, expected) in [
        (
            "a job that has not finished",
            ONBOARDING_STATUS_IN_PROGRESS,
            "IN_PROGRESS",
        ),
        (
            "a finished job",
            ONBOARDING_STATUS_SUCCEEDED,
            "SUCCEEDED",
        ),
    ] {
        assert_eq!(
            value, expected,
            "case {case:?}: expected the modelled wire value {expected:?}, got {value:?}"
        );
        assert!(
            INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES.contains(&value),
            "case {case:?}: expected a value from InstanceOnboardingJobStatusCode's catalog \
             {INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES:?}, got {value:?}"
        );
    }

    // The span the clock owes the transition is a real one, so a caller acting straight
    // after the start call still sees the transient status.
    assert!(
        ONBOARDING_JOB_DURATION > Duration::ZERO,
        "case \"the job's span\": expected a non-zero span, got {ONBOARDING_JOB_DURATION:?}"
    );
}

#[tokio::test]
async fn a_started_job_is_in_progress_and_the_clock_moves_it_to_succeeded() {
    for (case, instance, config) in onboarding_rows() {
        let harness = router();
        harness.register_connect_instance(REGION, &instance);

        let started = start(&harness, &instance, &config).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartInstanceOnboardingJob, got {} {}",
            started.status, started.body
        );
        assert_eq!(
            job_status(&started),
            ONBOARDING_STATUS_IN_PROGRESS,
            "case {case:?}: a job the call has only just started is not finished yet, got {}",
            started.body
        );
        assert_eq!(
            started
                .body
                .pointer("/connectInstanceOnboardingJobStatus/connectInstanceId")
                .and_then(Value::as_str),
            Some(instance.as_str()),
            "case {case:?}: the status names the instance it onboards, got {}",
            started.body
        );
        assert!(
            started
                .body
                .pointer("/connectInstanceOnboardingJobStatus/failureCode")
                .is_none_or(Value::is_null),
            "case {case:?}: a job that has not failed carries no failureCode, got {}",
            started.body
        );

        // The record, not the response, is the source of truth: the config the call
        // carried is what the store holds for this instance.
        assert_eq!(
            stored_config(&harness, &instance),
            Some(sent_config(&config)),
            "case {case:?}: expected the encryption config to be persisted exactly as sent, \
             got {:?}",
            stored_config(&harness, &instance)
        );

        // Reporting the status does not finish the job: the span is never zero, so a
        // caller polling straight after the start call still sees the transient status,
        // and two reads at the same point of the job agree.
        for read in 1..=2 {
            let polled = status(&harness, &instance).await;
            assert_eq!(
                (polled.status, job_status(&polled)),
                (200, ONBOARDING_STATUS_IN_PROGRESS),
                "case {case:?} read {read}: polling must not finish the job, got {} {}",
                polled.status,
                polled.body
            );
        }

        reach_the_due_instant(&harness, &instance);

        // Past the due instant the job has finished, and it stays finished.
        for read in 1..=2 {
            let polled = status(&harness, &instance).await;
            assert_eq!(
                (polled.status, job_status(&polled)),
                (200, ONBOARDING_STATUS_SUCCEEDED),
                "case {case:?} read {read}: expected the job to have finished, got {} {}",
                polled.status,
                polled.body
            );
        }

        // Finishing changed the reported status and nothing else: the stored config is
        // still the one that was sent.
        assert_eq!(
            stored_config(&harness, &instance),
            Some(sent_config(&config)),
            "case {case:?}: the finished job keeps the config the call carried, got {:?}",
            stored_config(&harness, &instance)
        );
    }
}

#[tokio::test]
async fn the_clock_alone_finishes_a_started_job() {
    let harness = router();
    let instance = "clock-owed-instance";
    harness.register_connect_instance(REGION, instance);

    let started = start(&harness, instance, &service_key_config()).await;
    assert_eq!(
        (started.status, job_status(&started)),
        (200, ONBOARDING_STATUS_IN_PROGRESS),
        "case \"start the job\": expected a job that has just started, got {} {}",
        started.status,
        started.body
    );

    // Nothing in the emulator sleeps for the transition: the record stores the instant,
    // so this case only has to let the wall clock reach it.
    tokio::time::sleep(ONBOARDING_JOB_DURATION + Duration::from_millis(250)).await;

    let polled = status(&harness, instance).await;
    assert_eq!(
        (polled.status, job_status(&polled)),
        (200, ONBOARDING_STATUS_SUCCEEDED),
        "case \"past the due instant\": expected the clock alone to have finished the job, \
         got {} {}",
        polled.status,
        polled.body
    );
}

#[tokio::test]
async fn an_instance_with_no_onboarding_job_is_not_found() {
    let harness = router();
    let onboarded = "onboarded-instance";
    harness.register_connect_instance(REGION, onboarded);
    let started = start(&harness, onboarded, &service_key_config()).await;
    assert_eq!(
        started.status, 200,
        "case \"onboard one instance\": expected HTTP 200, got {} {}",
        started.status, started.body
    );

    // One onboarded instance onboards no other: a record is keyed by its own instance
    // id, so an id the account holds without having onboarded it, an id it does not hold
    // at all, and an id that merely resembles the onboarded one are all misses.
    let unonboarded = "held-but-not-onboarded";
    harness.register_connect_instance(REGION, unonboarded);
    let rows: Vec<(&str, String)> = vec![
        ("an instance the account holds but nobody onboarded", unonboarded.to_string()),
        ("an id the account does not hold", "never-registered".to_string()),
        (
            "the onboarded id with a suffix",
            format!("{onboarded}-2"),
        ),
        ("the uuid form", "7c3ad1e9-55b0-4a62-8f31-20d4e6b7c900".to_string()),
        ("256 characters (the length maximum)", "z".repeat(256)),
    ];

    for (case, instance) in rows {
        let refused = status(&harness, &instance).await;
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
            "case {case:?}: expected the record-absent wording around the account and the \
             id the request carried, got {}",
            refused.body
        );
    }

    // The reads refused above left the one stored record alone.
    let polled = status(&harness, onboarded).await;
    assert_eq!(
        polled.status, 200,
        "case \"the onboarded instance afterwards\": expected HTTP 200, got {} {}",
        polled.status, polled.body
    );
}

#[tokio::test]
async fn onboarding_requires_an_instance_the_account_holds() {
    let harness = router();

    // Ids the account's Connect registry does not hold, spanning `InstanceId`: the uuid
    // form, the shortest id a path label can carry, every punctuation character the
    // pattern admits, the longest id the bound allows — plus an id the account HELD and
    // has since given up, because the gate is the registry's present contents and not
    // the id's shape or history.
    let surrendered = "surrendered-instance";
    harness.register_connect_instance(REGION, surrendered);
    assert!(
        harness.deregister_connect_instance(surrendered),
        "case \"an id the account held and gave up\": expected the registered instance to be \
         removable"
    );
    let rows: Vec<(&str, String)> = vec![
        ("the uuid form", "3f9c1a2b-4d5e-6f70-8192-a3b4c5d6e7f8".to_string()),
        ("one character", "q".to_string()),
        ("every admitted punctuation", "x.y_z-W8".to_string()),
        ("256 characters (the length maximum)", "m".repeat(256)),
        ("an id the account held and gave up", surrendered.to_string()),
    ];

    for (case, instance) in rows {
        let refused = start(&harness, &instance, &service_key_config()).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404 for an instance the account does not hold, \
             got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            worded(NOT_FOUND_WORDING, &instance),
            "case {case:?}: expected the not-found wording around the id the request \
             carried, got {}",
            refused.body
        );
        assert_eq!(
            stored_config(&harness, &instance),
            None,
            "case {case:?}: a refused onboarding persists nothing, got {:?}",
            stored_config(&harness, &instance)
        );
    }

    // The positive control: the same call for an instance the account DOES hold is
    // served, so the refusals above are the gate and not the operation being broken.
    let held = "held-instance";
    harness.register_connect_instance(REGION, held);
    let started = start(&harness, held, &service_key_config()).await;
    assert_eq!(
        (started.status, job_status(&started)),
        (200, ONBOARDING_STATUS_IN_PROGRESS),
        "case \"an instance the account holds\": expected the onboarding to be served, \
         got {} {}",
        started.status,
        started.body
    );
}

#[tokio::test]
async fn an_instance_that_already_holds_a_job_is_not_onboarded_again() {
    // One row per way a second call can differ from the first: the same config, a
    // different config, and a call made after the first job is due to have finished —
    // none of them starts a second job, and none of them overwrites the first.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("the same config, job still running", service_key_config(), false),
        (
            "a different config, job still running",
            json!({"enabled": true, "encryptionType": "KMS", "keyArn": KEY_ARN}),
            false,
        ),
        (
            "a different config, job already finished",
            json!({"enabled": true, "keyArn": KEY_ARN}),
            true,
        ),
    ];

    for (case, second_config, finish_first) in rows {
        let harness = router();
        let instance = "twice-onboarded-instance";
        harness.register_connect_instance(REGION, instance);

        let first_config = service_key_config();
        let started = start(&harness, instance, &first_config).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected the first onboarding to be served, got {} {}",
            started.status, started.body
        );
        if finish_first {
            reach_the_due_instant(&harness, instance);
        }

        let refused = start(&harness, instance, &second_config).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected HTTP 409 for an instance that already holds a job, \
             got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ConflictException"),
            "case {case:?}: expected ConflictException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            ALREADY_ONBOARDED_WORDING,
            "case {case:?}: expected the conflict wording, got {}",
            refused.body
        );
        assert!(
            refused.body.get("connectInstanceOnboardingJobStatus").is_none(),
            "case {case:?}: a refused onboarding reports no job, got {}",
            refused.body
        );

        // The refusal left the first job's record exactly as it was — the second call's
        // config was not taken.
        assert_eq!(
            stored_config(&harness, instance),
            Some(sent_config(&first_config)),
            "case {case:?}: expected the first config to be kept, got {:?}",
            stored_config(&harness, instance)
        );
        let polled = status(&harness, instance).await;
        assert_eq!(
            (polled.status, job_status(&polled)),
            (
                200,
                if finish_first {
                    ONBOARDING_STATUS_SUCCEEDED
                } else {
                    ONBOARDING_STATUS_IN_PROGRESS
                }
            ),
            "case {case:?}: expected the first job to be reported unchanged, got {} {}",
            polled.status,
            polled.body
        );
    }
}

#[tokio::test]
async fn a_kms_key_is_required_exactly_when_encryption_is_enabled() {
    // `EncryptionConfig`'s own documentation: "If disabled, service will perform
    // encryption with its own key. If enabled, a KMS key id needs to be provided" (from
    // `smithy_query(query="shape", shape="EncryptionConfig")`). Rows span both sides of
    // that rule, including the combinations it says nothing about.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("disabled, no type, no key", json!({"enabled": false}), true),
        (
            "disabled with the supported type and no key",
            json!({"enabled": false, "encryptionType": "KMS"}),
            true,
        ),
        (
            "disabled with a key (no documented rule forbids it)",
            json!({"enabled": false, "keyArn": KEY_ARN}),
            true,
        ),
        (
            "enabled with the supported type and a key",
            json!({"enabled": true, "encryptionType": "KMS", "keyArn": KEY_ARN}),
            true,
        ),
        (
            "enabled with a key and no type",
            json!({"enabled": true, "keyArn": KEY_ARN}),
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
    ];

    for (case, config, accepted) in rows {
        let harness = router();
        let instance = "encryption-config-instance";
        harness.register_connect_instance(REGION, instance);

        let answer = start(&harness, instance, &config).await;
        if accepted {
            assert_eq!(
                (answer.status, job_status(&answer)),
                (200, ONBOARDING_STATUS_IN_PROGRESS),
                "case {case:?}: expected the config to be accepted, got {} {}",
                answer.status,
                answer.body
            );
            assert_eq!(
                stored_config(&harness, instance),
                Some(sent_config(&config)),
                "case {case:?}: expected the accepted config to be persisted as sent, got {:?}",
                stored_config(&harness, instance)
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code.ends_with("ValidationException"),
                "case {case:?}: expected ValidationException, got {:?}",
                answer.error_code
            );
            assert!(
                answer.message().contains("encryptionConfig.keyArn"),
                "case {case:?}: expected the refusal to name the member at fault, got {}",
                answer.body
            );
            // A refusal persists nothing, so the instance still has no job at all.
            assert_eq!(
                stored_config(&harness, instance),
                None,
                "case {case:?}: a refused start stores nothing, got {:?}",
                stored_config(&harness, instance)
            );
            let polled = status(&harness, instance).await;
            assert_eq!(
                polled.status, 404,
                "case {case:?}: expected the status read to be a 404 afterwards, got {} {}",
                polled.status, polled.body
            );
        }
    }
}

#[test]
fn the_shared_refusal_wordings_are_the_ones_this_suite_pins() {
    // The handlers compose their refusals through the crate's own wording helpers, so
    // the sentences pinned above are checked against those helpers too — a change to
    // either side has to change both.
    assert_eq!(
        connect_instance_not_found_message("an-instance"),
        worded(NOT_FOUND_WORDING, "an-instance"),
        "case \"the not-found wording helper\": expected {:?}",
        worded(NOT_FOUND_WORDING, "an-instance")
    );
    assert_eq!(
        onboarding_record_absent_message("an-instance"),
        worded(RECORD_ABSENT_WORDING, "an-instance"),
        "case \"the record-absent wording helper\": expected {:?}",
        worded(RECORD_ABSENT_WORDING, "an-instance")
    );
    assert!(
        !onboarding_record_absent_message("an-instance").ends_with('.'),
        "case \"the record-absent wording helper\": this class carries no trailing period, \
         got {:?}",
        onboarding_record_absent_message("an-instance")
    );
    assert_eq!(
        INSTANCE_ALREADY_ONBOARDED_MESSAGE, ALREADY_ONBOARDED_WORDING,
        "case \"the conflict wording\": expected {ALREADY_ONBOARDED_WORDING:?}, got \
         {INSTANCE_ALREADY_ONBOARDED_MESSAGE:?}"
    );
}
