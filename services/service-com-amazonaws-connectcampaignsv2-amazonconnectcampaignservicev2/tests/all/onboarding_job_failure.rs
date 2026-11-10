// SPDX-License-Identifier: CC0-1.0

//! The onboarding job's FAILED outcome, end to end over the wire.
//!
//! Every case drives real HTTP through the router the production entrypoint serves —
//! with AWS KMS mounted beside this service on the one engine, as the assembled
//! emulator mounts it — so each assertion exercises the restJson1 wire protocol, the
//! modelled constraints and the handlers together. The rule under test: an onboarding
//! job settles on the outcome its dependencies decided, which is `SUCCEEDED` only for a
//! job whose encryption key the service can use, and otherwise `FAILED` carrying the
//! `InstanceOnboardingJobFailureCode` of the dependency that stopped it:
//!
//! * the key the config names resolves to no key the account holds in the request's
//!   region — `KMS_KEY_NOT_FOUND`;
//! * it resolves to a key the service cannot encrypt with — `KMS_ACCESS_DENIED`;
//! * the job is `IN_PROGRESS` until it settles either way, so a caller that polls
//!   straight after the start call still sees the transient status;
//! * a `FAILED` job publishes no instance configuration and makes the instance no
//!   campaign target, and it is the one status `DeleteInstanceOnboardingJob` clears:
//!   the delete answers an empty HTTP 200, the record is gone, and the instance can be
//!   onboarded afresh.
//!
//! The keys the rows are put on are minted through KMS's OWN `CreateKey` (and moved by
//! its own `DisableKey` / `ScheduleKeyDeletion`), so every outcome here is decided by
//! what the service that owns those keys really holds.

use serde_json::{json, Value};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::ops::types::{
    INSTANCE_ONBOARDING_JOB_FAILURE_CODE_VALUES, INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES,
};
use service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::state::{
    ONBOARDING_FAILURE_KMS_ACCESS_DENIED, ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND,
    ONBOARDING_JOB_DURATION, ONBOARDING_JOB_NOT_FAILED_MESSAGE, ONBOARDING_STATUS_FAILED,
    ONBOARDING_STATUS_IN_PROGRESS, ONBOARDING_STATUS_SUCCEEDED,
};

use super::common::{call, error_code, router_with_kms, Harness};

/// The region the Campaigns requests here are signed for. A KMS key is a regional
/// resource, so one row deliberately puts its job on a key of ANOTHER region.
const REGION: &str = "us-east-1";

/// The other region, for the row whose key the request's own endpoint does not serve.
const OTHER_REGION: &str = "us-west-2";

/// `StartInstanceOnboardingJob` for `instance` with `encryption_config` — on an
/// instance registered in the account's Connect registry first, since onboarding
/// resolves the `connectInstanceId` against it.
async fn start(router: &Harness, instance: &str, encryption_config: &Value) -> (u16, Value) {
    router.register_connect_instance(REGION, instance);
    call(
        router,
        REGION,
        "PUT",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &json!({"encryptionConfig": encryption_config}),
    )
    .await
}

/// `GetInstanceOnboardingJobStatus` for `instance`.
async fn get_status(router: &Harness, instance: &str) -> (u16, Value) {
    call(
        router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{instance}/onboarding"),
        &Value::Null,
    )
    .await
}

/// `GetConnectInstanceConfig` for `instance` — the read only a SUCCEEDED job publishes.
async fn get_config(router: &Harness, instance: &str) -> (u16, Value) {
    call(
        router,
        REGION,
        "GET",
        &format!("/v2/connect-instance/{instance}/config"),
        &Value::Null,
    )
    .await
}

/// `DeleteInstanceOnboardingJob` for `instance`.
async fn delete(router: &Harness, instance: &str) -> (u16, Value) {
    call(
        router,
        REGION,
        "DELETE",
        &format!("/v2/connect-instance/{instance}/onboarding"),
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

/// The `connectInstanceOnboardingJobStatus$failureCode` a success body carries, or
/// `None` for a job that reports none.
fn failure_code(response: &Value) -> Option<&str> {
    response
        .pointer("/connectInstanceOnboardingJobStatus/failureCode")
        .and_then(Value::as_str)
}

/// Let the clock reach the instant the started job settles at. Nothing in the emulator
/// sleeps for it — the record carries the instant, so only a caller that wants the
/// settled status waits.
async fn settle() {
    tokio::time::sleep(ONBOARDING_JOB_DURATION + std::time::Duration::from_millis(150)).await;
}

/// One KMS key minted through KMS's own `CreateKey` in `region`, with `request` as the
/// create body, answering `(key id, key ARN)`.
async fn create_key(router: &Harness, region: &str, request: &Value) -> (String, String) {
    let (status, created) = router.kms(region, "CreateKey", request).await;
    assert_eq!(
        status, 200,
        "the fixture's CreateKey {request} must succeed, got {status} {created}"
    );
    let metadata = &created["KeyMetadata"];
    (
        metadata["KeyId"]
            .as_str()
            .expect("CreateKey reports the new key's id")
            .to_string(),
        metadata["Arn"]
            .as_str()
            .expect("CreateKey reports the new key's ARN")
            .to_string(),
    )
}

/// A symmetric encryption key of `REGION` the service can encrypt with: the key
/// `CreateKey` mints when the request names no particular shape.
async fn usable_key(router: &Harness) -> (String, String) {
    create_key(router, REGION, &json!({})).await
}

/// Drive one more KMS operation on `key_arn`, asserting KMS served it — how a row moves
/// its key into a state the service cannot use it in.
async fn kms_on_key(router: &Harness, target: &str, body: &Value) {
    let (status, answered) = router.kms(REGION, target, body).await;
    assert_eq!(
        status, 200,
        "the fixture's {target} must succeed, got {status} {answered}"
    );
}

/// An `encryptionConfig` that puts the instance's data on `key`, in the form the model
/// declares: `enabled` with the only `EncryptionType` its catalog holds.
fn on_key(key: &str) -> Value {
    json!({"enabled": true, "encryptionType": "KMS", "keyArn": key})
}

/// Every way an onboarding job's key can stop it, each with the
/// `InstanceOnboardingJobFailureCode` the job settles on.
///
/// The rows differ along what the rule generalises over: which form the identifier
/// takes (a key ARN, a bare key id, an alias, an ARN of another service), whose and
/// where the key is (another region of the same account, another account, an account
/// segment that is no account at all), and — for a key the account DOES hold — what
/// about the key stops it (disabled, awaiting deletion, awaiting imported material, a
/// usage that is not encryption). Each row builds its own router, so one row's key
/// cannot decide the next.
async fn failing_key_cases() -> Vec<(&'static str, Harness, String, &'static str)> {
    let mut cases: Vec<(&'static str, Harness, String, &'static str)> = Vec::new();

    // ---- keys that resolve to nothing: KMS_KEY_NOT_FOUND ----
    let router = router_with_kms();
    cases.push((
        "a key ARN naming no key at all",
        router,
        "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555".to_string(),
        ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND,
    ));

    let router = router_with_kms();
    cases.push((
        "a bare key id naming no key",
        router,
        "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
        ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND,
    ));

    let router = router_with_kms();
    cases.push((
        "an alias naming no key",
        router,
        "alias/campaigns-none".to_string(),
        ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND,
    ));

    let router = router_with_kms();
    cases.push((
        "a value that is no KMS identifier",
        router,
        "not-a-kms-key".to_string(),
        ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND,
    ));

    let router = router_with_kms();
    cases.push((
        "an ARN of another service entirely",
        router,
        "arn:aws:s3:::campaigns-bucket".to_string(),
        ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND,
    ));

    // A key the account really holds, in a region this request's endpoint does not
    // serve: a KMS key is regional, so it is no key here.
    let router = router_with_kms();
    let (_, elsewhere) = create_key(&router, OTHER_REGION, &json!({})).await;
    cases.push((
        "a key of another region",
        router,
        elsewhere,
        ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND,
    ));

    // The same key id under another account: the ARN pins the lookup to that account,
    // which holds no such key here.
    let router = router_with_kms();
    let (key_id, _) = usable_key(&router).await;
    cases.push((
        "a key ARN of another account",
        router,
        format!("arn:aws:kms:{REGION}:999999999999:key/{key_id}"),
        ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND,
    ));

    // ---- keys the service cannot encrypt with: KMS_ACCESS_DENIED ----
    let router = router_with_kms();
    let (_, arn) = usable_key(&router).await;
    kms_on_key(&router, "DisableKey", &json!({"KeyId": arn})).await;
    cases.push((
        "a disabled key",
        router,
        arn,
        ONBOARDING_FAILURE_KMS_ACCESS_DENIED,
    ));

    let router = router_with_kms();
    let (_, arn) = usable_key(&router).await;
    kms_on_key(
        &router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": arn, "PendingWindowInDays": 7}),
    )
    .await;
    cases.push((
        "a key awaiting deletion",
        router,
        arn,
        ONBOARDING_FAILURE_KMS_ACCESS_DENIED,
    ));

    // `KeyUsage` / `KeySpec` values from KMS's own catalogs: a signing key cannot
    // encrypt the instance's data, and a key with no material yet cannot be used at all.
    let router = router_with_kms();
    let (_, arn) = create_key(
        &router,
        REGION,
        &json!({"KeyUsage": "SIGN_VERIFY", "KeySpec": "RSA_2048"}),
    )
    .await;
    cases.push((
        "a sign-and-verify key",
        router,
        arn,
        ONBOARDING_FAILURE_KMS_ACCESS_DENIED,
    ));

    let router = router_with_kms();
    let (_, arn) = create_key(&router, REGION, &json!({"Origin": "EXTERNAL"})).await;
    cases.push((
        "a key awaiting imported material",
        router,
        arn,
        ONBOARDING_FAILURE_KMS_ACCESS_DENIED,
    ));

    // An ARN whose account segment is no account id: KMS refuses such an identifier for
    // want of permission before it looks anything up.
    let router = router_with_kms();
    cases.push((
        "an ARN whose account segment is no account",
        router,
        format!("arn:aws:kms:{REGION}:not-an-account:key/11111111-2222-3333-4444-555555555555"),
        ONBOARDING_FAILURE_KMS_ACCESS_DENIED,
    ));

    cases
}

#[test]
fn the_failure_codes_are_the_modelled_wire_values() {
    // The wire contract, pinned against the model's own catalogs, so the constants the
    // cases below compare with cannot drift off the model.
    for code in [
        ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND,
        ONBOARDING_FAILURE_KMS_ACCESS_DENIED,
    ] {
        assert!(
            INSTANCE_ONBOARDING_JOB_FAILURE_CODE_VALUES.contains(&code),
            "case {code:?}: expected a value from InstanceOnboardingJobFailureCode's catalog \
             {INSTANCE_ONBOARDING_JOB_FAILURE_CODE_VALUES:?}"
        );
    }
    assert!(
        INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES.contains(&ONBOARDING_STATUS_FAILED),
        "case \"failed job\": expected a value from InstanceOnboardingJobStatusCode's catalog \
         {INSTANCE_ONBOARDING_JOB_STATUS_CODE_VALUES:?}"
    );
}

#[tokio::test]
async fn a_job_whose_key_the_service_cannot_use_settles_failed_with_that_codes_reason() {
    let instance = "failing-onboarding";
    for (case, router, key, expected) in failing_key_cases().await {
        let (status, started) = start(&router, instance, &on_key(&key)).await;
        assert_eq!(
            (status, job_status(&started)),
            (200, ONBOARDING_STATUS_IN_PROGRESS),
            "case {case:?}: the start call takes the job and the span is never zero, \
             got {status} {started}"
        );
        assert_eq!(
            failure_code(&started),
            None,
            "case {case:?}: a job that has not settled yet has not failed, got {started}"
        );
        // While it is in progress there is nothing to delete either: the job has not
        // failed yet.
        let (status, body) = delete(&router, instance).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a job still in progress is no failed job, got {status} {body}"
        );

        settle().await;

        // The settled job reports the failure and the code it earned, and it keeps
        // reporting them: a read is stable.
        for read in 1..=2 {
            let (status, body) = get_status(&router, instance).await;
            assert_eq!(
                (status, job_status(&body), failure_code(&body)),
                (200, ONBOARDING_STATUS_FAILED, Some(expected)),
                "case {case:?} read {read}: expected the job to have failed with {expected}, \
                 got {status} {body}"
            );
        }

        // A failed job onboarded nothing: there is no instance configuration to read.
        let (status, body) = get_config(&router, instance).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a failed job publishes no configuration, got {status} {body}"
        );

        // And the failed job is the one status the delete clears: an empty HTTP 200,
        // with the record gone.
        let (status, body) = delete(&router, instance).await;
        assert_eq!(
            (status, &body),
            (200, &Value::Null),
            "case {case:?}: expected an empty HTTP 200 deleting a failed job, \
             got {status} {body}"
        );
        let (status, body) = get_status(&router, instance).await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the deleted job is gone, got {status} {body}"
        );
        // Repeating the delete now has no record to find.
        let (status, body) = delete(&router, instance).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a job already deleted cannot be deleted again, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_job_on_a_key_the_service_can_use_settles_succeeded() {
    let router = router_with_kms();
    let (key_id, key_arn) = usable_key(&router).await;
    // Every identifier form KMS resolves a key by, so the outcome follows the KEY and
    // not the spelling: the key ARN, the bare key id, an alias of it and that alias's
    // own ARN. The alias is minted through KMS's own `CreateAlias`.
    let alias = "alias/campaigns-data";
    kms_on_key(
        &router,
        "CreateAlias",
        &json!({"AliasName": alias, "TargetKeyId": key_id}),
    )
    .await;
    let alias_arn = format!("arn:aws:kms:{REGION}:000000000000:{alias}");

    let cases: Vec<(&str, String, Value)> = vec![
        ("the key ARN", "by-key-arn".to_string(), on_key(&key_arn)),
        ("the bare key id", "by-key-id".to_string(), on_key(&key_id)),
        ("an alias of the key", "by-alias".to_string(), on_key(alias)),
        (
            "the alias's own ARN",
            "by-alias-arn".to_string(),
            on_key(&alias_arn),
        ),
        (
            "the service's own key",
            "by-service-key".to_string(),
            json!({"enabled": false}),
        ),
    ];

    for (case, instance, config) in &cases {
        let (status, started) = start(&router, instance, config).await;
        assert_eq!(
            (status, job_status(&started)),
            (200, ONBOARDING_STATUS_IN_PROGRESS),
            "case {case:?}: expected a job that has just started, got {status} {started}"
        );
    }

    settle().await;

    for (case, instance, config) in &cases {
        let (status, body) = get_status(&router, instance).await;
        assert_eq!(
            (status, job_status(&body), failure_code(&body)),
            (200, ONBOARDING_STATUS_SUCCEEDED, None),
            "case {case:?}: expected a job that succeeded carrying no failure code, \
             got {status} {body}"
        );
        // A job that succeeded published the configuration it was given.
        let (status, body) = get_config(&router, instance).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the configuration of an onboarded instance, \
             got {status} {body}"
        );
        assert_eq!(
            body.pointer("/connectInstanceConfig/encryptionConfig"),
            Some(config),
            "case {case:?}: the configuration carries the encryption config onboarding was \
             given, got {body}"
        );
        // And a job that succeeded is no failed job, so the delete refuses it with the
        // service's fixed wording.
        let (status, body) = delete(&router, instance).await;
        assert_eq!(
            (status, body.get("message").and_then(Value::as_str)),
            (409, Some(ONBOARDING_JOB_NOT_FAILED_MESSAGE)),
            "case {case:?}: a succeeded job cannot be deleted, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn a_failed_job_is_no_campaign_target_until_it_is_cleared_and_redone() {
    let router = router_with_kms();
    let instance = "re-onboarded-instance";
    let (_, key_arn) = usable_key(&router).await;

    // A job put on a key KMS does not hold.
    let (status, body) = start(
        &router,
        instance,
        &on_key("arn:aws:kms:us-east-1:000000000000:key/99999999-8888-7777-6666-555555555555"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the failing job\": expected the start to be taken, got {status} {body}"
    );
    settle().await;
    let (status, body) = get_status(&router, instance).await;
    assert_eq!(
        (status, job_status(&body), failure_code(&body)),
        (
            200,
            ONBOARDING_STATUS_FAILED,
            Some(ONBOARDING_FAILURE_KMS_KEY_NOT_FOUND)
        ),
        "case \"the failing job\": expected the failure its key earned, got {status} {body}"
    );

    // A failed job onboarded nothing, so the instance is no campaign target.
    let campaign = json!({
        "name": "campaign-on-a-failed-onboarding",
        "connectInstanceId": instance,
        "channelSubtypeConfig": {
            "telephony": {
                "outboundMode": {"agentless": {}},
                "defaultOutboundConfig": {"connectContactFlowId": "flow-0001"}
            }
        },
    });
    let (status, body) = call(&router, REGION, "PUT", "/v2/campaigns", &campaign).await;
    assert_eq!(
        (status, error_code(&body)),
        (404, "ResourceNotFoundException"),
        "case \"a campaign on a failed onboarding\": expected the instance to be no campaign \
         target, got {status} {body}"
    );

    // Clearing the failed job is what lets the instance be onboarded afresh, this time
    // on a key the service can use.
    let (status, body) = delete(&router, instance).await;
    assert_eq!(
        (status, &body),
        (200, &Value::Null),
        "case \"clearing the failed job\": expected an empty HTTP 200, got {status} {body}"
    );
    let (status, body) = start(&router, instance, &on_key(&key_arn)).await;
    assert_eq!(
        (status, job_status(&body)),
        (200, ONBOARDING_STATUS_IN_PROGRESS),
        "case \"the second job\": expected a fresh job, got {status} {body}"
    );
    settle().await;
    let (status, body) = get_status(&router, instance).await;
    assert_eq!(
        (status, job_status(&body), failure_code(&body)),
        (200, ONBOARDING_STATUS_SUCCEEDED, None),
        "case \"the second job\": expected the job on a usable key to succeed, \
         got {status} {body}"
    );
    let (status, body) = call(&router, REGION, "PUT", "/v2/campaigns", &campaign).await;
    assert_eq!(
        status, 200,
        "case \"a campaign after re-onboarding\": expected the instance to be a campaign \
         target, got {status} {body}"
    );
}
