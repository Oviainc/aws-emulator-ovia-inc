// SPDX-License-Identifier: CC0-1.0

//! `DeleteConfigurationProfile` across the account's deletion-protection configuration.
//!
//! RULE: deletion protection refuses a configuration-profile delete only when a
//! configuration read for that profile falls inside the account's protection period; the
//! account-level `DeletionProtection` configuration and the `DeletionProtectionCheck` the
//! caller sends decide only WHETHER that check runs, never what it finds. The model
//! states both halves: protection "prevents a user from deleting a configuration profile
//! or an environment if AppConfig has called either GetLatestConfiguration or [the
//! deprecated configuration read] for the configuration profile ... during the specified
//! interval", and the check "skips configuration profiles and environments that were
//! created in the past hour"
//! (`smithy_query(query="doc",shape="com.amazonaws.appconfig#DeletionProtectionSettings")`;
//! `APPLY` runs the check anyway, `BYPASS` skips it, `ACCOUNT_DEFAULT` follows the
//! account).
//!
//! No configuration read of any profile is on record in this service — the retrieval call
//! the protection period watches is `appconfigdata`'s `GetLatestConfiguration`, and this
//! service's own deprecated `GetConfiguration` is still a scaffold — so for every profile
//! here the check finds no activity to protect, whichever of the three values asks for it.
//! What the delete then answers is decided by the profile's OWN stored state and nothing
//! else: 404 when the store does not hold the profile, 400 while hosted configuration
//! versions remain under it, 204 once neither is true. A delete also writes nothing into
//! the account settings, so the configuration the check is read from reads back unchanged.
//!
//! Every case drives real HTTP requests through the router the emulator's front door
//! uses, so what is asserted is what an AppConfig client sees on the wire.
//!
//! The table spans what the rule generalises over rather than one recorded account: how
//! the account's protection is configured (never written, explicitly off, on at the
//! documented default interval, on at each published bound of the interval, and on then
//! off again), which `DeletionProtectionCheck` the delete asks for (absent, and every
//! value the model's enum declares), and which stored state the addressed profile is in
//! (absent, holding a hosted version, holding none).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::DELETION_PROTECTION_CHECK_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state::DEFAULT_PROTECTION_PERIOD_IN_MINUTES;

use crate::appconfig_workflow::{create_application, create_profile, json_call, router, send, Answer};

/// The published bounds of `ProtectionPeriodInMinutes`, from the member's target shape
/// (`smithy_query(query="shape",shape="com.amazonaws.appconfig#DeletionProtectionDuration")`:
/// `@range` 15..1440).
const MIN_PROTECTION_PERIOD: i32 = 15;
/// See [`MIN_PROTECTION_PERIOD`].
const MAX_PROTECTION_PERIOD: i32 = 1440;

/// Where the configuration profiles of one application live, from this operation's
/// modelled URI
/// (`smithy_query(query="operation",operation="DeleteConfigurationProfile")`:
/// `/applications/{ApplicationId}/configurationprofiles/{ConfigurationProfileId}`).
fn profiles_uri(application: &str) -> String {
    format!("/applications/{application}/configurationprofiles")
}

/// Where one configuration profile is read, deleted and updated.
fn profile_uri(application: &str, profile: &str) -> String {
    format!("{}/{profile}", profiles_uri(application))
}

/// Where the hosted configuration versions of one profile live.
fn hosted_uri(application: &str, profile: &str) -> String {
    format!("{}/hostedconfigurationversions", profile_uri(application, profile))
}

/// One account-settings row: the patches the account has seen before any delete runs, and
/// the `DeletionProtection` document that must read back while they run.
struct Account {
    case: &'static str,
    patches: Vec<Value>,
    protection: Value,
}

/// The account configurations the rule is asserted over.
fn accounts() -> Vec<Account> {
    vec![
        Account {
            case: "an account nothing has ever written",
            patches: Vec::new(),
            protection: json!({
                "Enabled": false,
                "ProtectionPeriodInMinutes": DEFAULT_PROTECTION_PERIOD_IN_MINUTES,
            }),
        },
        Account {
            case: "deletion protection explicitly off",
            patches: vec![json!({ "DeletionProtection": { "Enabled": false } })],
            protection: json!({
                "Enabled": false,
                "ProtectionPeriodInMinutes": DEFAULT_PROTECTION_PERIOD_IN_MINUTES,
            }),
        },
        Account {
            case: "deletion protection on at the documented default interval",
            patches: vec![json!({ "DeletionProtection": { "Enabled": true } })],
            protection: json!({
                "Enabled": true,
                "ProtectionPeriodInMinutes": DEFAULT_PROTECTION_PERIOD_IN_MINUTES,
            }),
        },
        Account {
            case: "deletion protection on at the lowest interval the model allows",
            patches: vec![json!({
                "DeletionProtection": {
                    "Enabled": true,
                    "ProtectionPeriodInMinutes": MIN_PROTECTION_PERIOD,
                }
            })],
            protection: json!({
                "Enabled": true,
                "ProtectionPeriodInMinutes": MIN_PROTECTION_PERIOD,
            }),
        },
        Account {
            case: "deletion protection on at the highest interval the model allows",
            patches: vec![json!({
                "DeletionProtection": {
                    "Enabled": true,
                    "ProtectionPeriodInMinutes": MAX_PROTECTION_PERIOD,
                }
            })],
            protection: json!({
                "Enabled": true,
                "ProtectionPeriodInMinutes": MAX_PROTECTION_PERIOD,
            }),
        },
        Account {
            case: "deletion protection turned on and then off again",
            patches: vec![
                json!({
                    "DeletionProtection": { "Enabled": true, "ProtectionPeriodInMinutes": 720 }
                }),
                json!({ "DeletionProtection": { "Enabled": false } }),
            ],
            protection: json!({ "Enabled": false, "ProtectionPeriodInMinutes": 720 }),
        },
    ]
}

/// Each way a delete can ask for the check: the header absent, and every value the
/// model's `DeletionProtectionCheck` enum declares (`DELETION_PROTECTION_CHECK_VALUES`).
fn checks() -> Vec<Option<&'static str>> {
    let mut asked: Vec<Option<&'static str>> = vec![None];
    asked.extend(DELETION_PROTECTION_CHECK_VALUES.iter().copied().map(Some));
    asked
}

/// How the row names itself in an assertion: the account configuration and the check.
fn label(account: &str, check: Option<&str>) -> String {
    format!("{account} / DeletionProtectionCheck {}", check.unwrap_or("absent"))
}

/// `PATCH /settings`.
async fn patch_settings(router: &MultiServiceRouter, case: &str, body: &Value) {
    let answer = json_call(router, "PATCH", "/settings", body).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: UpdateAccountSettings must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// The `DeletionProtection` group `GetAccountSettings` reports.
async fn reported_protection(router: &MultiServiceRouter, case: &str) -> Value {
    let answer = json_call(router, "GET", "/settings", &Value::Null).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: GetAccountSettings must answer 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("DeletionProtection")
        .cloned()
        .unwrap_or(Value::Null)
}

/// `DELETE` one configuration profile, optionally asking for a deletion-protection check.
async fn delete_profile(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    check: Option<&str>,
) -> Answer {
    let headers: Vec<(&str, String)> = check
        .map(|value| vec![("x-amzn-deletion-protection-check", value.to_string())])
        .unwrap_or_default();
    send(
        router,
        "DELETE",
        &profile_uri(application, profile),
        "application/json",
        Vec::new(),
        &headers,
    )
    .await
}

/// `GET` one configuration profile.
async fn get_profile(router: &MultiServiceRouter, application: &str, profile: &str) -> Answer {
    json_call(router, "GET", &profile_uri(application, profile), &Value::Null).await
}

/// Write one hosted configuration version under `profile` and answer its version number.
async fn write_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    case: &str,
) -> i32 {
    let created = send(
        router,
        "POST",
        &hosted_uri(application, profile),
        "application/json",
        br#"{"enabled":true}"#.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateHostedConfigurationVersion must answer 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .header("Version-Number")
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateHostedConfigurationVersion answered no Version-Number")
        })
        .parse()
        .unwrap_or_else(|error| {
            panic!("case {case:?}: Version-Number must be an integer, got {error}")
        })
}

/// `DELETE` one hosted configuration version, which must succeed.
async fn delete_version(
    router: &MultiServiceRouter,
    application: &str,
    profile: &str,
    case: &str,
    number: i32,
) {
    let answer = send(
        router,
        "DELETE",
        &format!("{}/{number}", hosted_uri(application, profile)),
        "application/json",
        Vec::new(),
        &[],
    )
    .await;
    assert_eq!(
        answer.status,
        204,
        "case {case:?}: DeleteHostedConfigurationVersion {number} must answer 204, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// A delete that must have succeeded: the modelled 204, with no body.
fn assert_deleted(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        204,
        "case {case:?}: DeleteConfigurationProfile must answer the modelled 204, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.body.is_empty(),
        "case {case:?}: the modelled 204 carries no body, got {:?}",
        String::from_utf8_lossy(&answer.body)
    );
}

/// One modelled refusal: the expected status and error code, on the wire where an SDK
/// classifies it from, with a message for the caller.
fn assert_refusal(case: &str, expected_status: u16, expected_code: &str, answer: &Answer) {
    assert_eq!(
        answer.status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert!(
        answer.error_code().contains(expected_code),
        "case {case:?}: expected {expected_code}, got {:?}",
        answer.error_code()
    );
    assert!(
        answer
            .header("x-amzn-errortype")
            .is_some_and(|value| value.contains(expected_code)),
        "case {case:?}: the refusal must set x-amzn-errortype to {expected_code}, got {:?}",
        answer.header("x-amzn-errortype")
    );
    assert!(
        answer
            .string("Message")
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: the refusal must carry a Message, got {}",
        answer.json()
    );
}

/// RULE: the account's deletion-protection configuration and the caller's
/// `DeletionProtectionCheck` select only whether the protection check runs, and the check
/// finds no configuration read of any profile to protect — so the delete answers what the
/// profile's own stored state dictates under every account configuration and every check.
#[tokio::test]
async fn the_account_protection_configuration_does_not_decide_a_profile_delete() {
    for account in accounts() {
        // A fresh account per row, so a row's answers depend only on its own patches.
        let router = router();
        for patch in &account.patches {
            patch_settings(&router, account.case, patch).await;
        }
        let application = create_application(&router, "protected-deletes").await;

        for check in checks() {
            let case = label(account.case, check);
            let case = case.as_str();

            // The account configuration the check would be read from, as the account
            // settings report it before any delete runs.
            assert_eq!(
                reported_protection(&router, case).await,
                account.protection,
                "case {case:?}: GetAccountSettings must report the written protection"
            );

            // A profile the store does not hold: whatever the protection configuration
            // says, an unresolvable profile is a miss and not a protection refusal.
            assert_refusal(
                case,
                404,
                "ResourceNotFoundException",
                &delete_profile(&router, &application, "nosuchprofile", check).await,
            );

            // A profile that still owns a hosted configuration version: the children-first
            // refusal, which protection neither causes nor suppresses — including under
            // BYPASS, which bypasses deletion protection and not this rule.
            let occupied = create_profile(&router, &application, "occupied").await;
            let version = write_version(&router, &application, &occupied, case).await;
            assert_refusal(
                case,
                400,
                "BadRequestException",
                &delete_profile(&router, &application, &occupied, check).await,
            );
            assert_eq!(
                get_profile(&router, &application, &occupied).await.status,
                200,
                "case {case:?}: a refused delete leaves the profile describable"
            );
            delete_version(&router, &application, &occupied, case, version).await;
            assert_deleted(
                case,
                &delete_profile(&router, &application, &occupied, check).await,
            );
            assert_refusal(
                case,
                404,
                "ResourceNotFoundException",
                &get_profile(&router, &application, &occupied).await,
            );

            // A profile no configuration version was ever written into: deletable
            // straight away, because the check has no retrieval activity to protect.
            let bare = create_profile(&router, &application, "bare").await;
            assert_deleted(case, &delete_profile(&router, &application, &bare, check).await);
            assert_refusal(
                case,
                404,
                "ResourceNotFoundException",
                &get_profile(&router, &application, &bare).await,
            );

            // The deletes wrote nothing into the account settings: the configuration the
            // check is read from is only ever written by `UpdateAccountSettings`.
            assert_eq!(
                reported_protection(&router, case).await,
                account.protection,
                "case {case:?}: a delete must not change the account's deletion protection"
            );
        }
    }
}
