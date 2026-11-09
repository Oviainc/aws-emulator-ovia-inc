// SPDX-License-Identifier: CC0-1.0

//! The account's SERVICE SETTINGS end to end: `GetServiceSetting` reports one,
//! `UpdateServiceSetting` overwrites it with the account's own value, and
//! `ResetServiceSetting` restores the value the Amazon Web Services service team
//! provisioned.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body, dispatched by the
//! multi-service router (`OPERATIONS.md`, "Wire protocol"), through this crate's one test
//! harness (`all/parameter_store.rs`). Nothing reaches into the state struct, so a
//! response that is not computed from persisted state cannot pass: every read follows the
//! write whose work it reports.
//!
//! The suites are tables. Their rows differ along what the rules under test generalise
//! over — WHICH setting of the provisioned catalogue, the spelling of its `SettingId` (the
//! documented path or the setting's ARN), the form of the value it holds (a boolean, a
//! documented word, a free-text name), the letter case a documented value is written in,
//! and the Region the request signs for — rather than the single setting a recorded unit
//! happened to reset.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ssm_amazonssm::state::service_settings::{
    modifying_user, SettingDefinition, MAX_SERVICE_SETTINGS_TOTAL, PROVISIONING_USER, SETTINGS,
    STATUS_CUSTOMIZED, STATUS_DEFAULT, UNSET_VALUE,
};

use super::parameter_store::{
    call, call_in_region, error_code, error_message as message, router, ACCOUNT, REGION,
};

/// The sentence `ServiceSettingNotFound` carries, spelled out here as the error shape's
/// own documentation words it
/// (`smithy_query(query="shape", shape="com.amazonaws.ssm#ServiceSettingNotFound")`), so a
/// change to the wording the handler answers with fails this suite.
const SETTING_NOT_FOUND: &str = "The specified service setting wasn't found. Either the service \
                                name or the setting hasn't been provisioned by the Amazon Web \
                                Services service team.";

/// A second Region, so a suite can prove a setting is held at the endpoint it was set
/// through rather than account-wide across every endpoint.
const OTHER_REGION: &str = "eu-west-1";

/// The values a setting whose value is FREE TEXT is updated to — the API documents no
/// value list for those settings (`UpdateServiceSettingRequest$SettingValue` states one
/// only for the settings that take a fixed word), so the rows differ in the form of the
/// value they persist: a log-group path, an IAM role name, a boolean word.
const FREE_TEXT_VALUES: &[&str] = &[
    "/aws/ssm/automation-customer-scripts",
    "SSMDefaultEC2InstanceManagementRole",
    "true",
];

/// The ARN spelling of `id` at `region`'s endpoint, as
/// `UpdateServiceSettingRequest$SettingId` documents it
/// (`arn:aws:ssm:us-east-1:111122223333:servicesetting/ssm/parameter-store/high-throughput-enabled`).
fn setting_arn(region: &str, id: &str) -> String {
    format!("arn:aws:ssm:{region}:{ACCOUNT}:servicesetting{id}")
}

/// The value `setting` is customized to: the first value it documents that differs from
/// the one its service team provisioned, or — for a setting the API documents no values
/// for — one of [`FREE_TEXT_VALUES`], chosen by `index` so the table spans all three
/// forms.
fn custom_value(index: usize, setting: &SettingDefinition) -> String {
    setting
        .values
        .iter()
        .find(|value| !value.eq_ignore_ascii_case(setting.default_value))
        .map_or_else(
            || FREE_TEXT_VALUES[index % FREE_TEXT_VALUES.len()].to_string(),
            |value| (*value).to_string(),
        )
}

/// `GetServiceSetting` for `id`, returning `(status, body)`.
async fn get(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GetServiceSetting", &json!({ "SettingId": id })).await
}

/// The `ServiceSetting` a successful `GetServiceSetting` reports for `id`.
async fn get_ok(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, body) = get(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetServiceSetting {id:?} must succeed, got {status} {body}"
    );
    body["ServiceSetting"].clone()
}

/// `UpdateServiceSetting`, returning `(status, body)`.
async fn update(router: &MultiServiceRouter, id: &str, value: &str) -> (u16, Value) {
    call(
        router,
        "UpdateServiceSetting",
        &json!({ "SettingId": id, "SettingValue": value }),
    )
    .await
}

/// `ResetServiceSetting`, returning `(status, body)`.
async fn reset(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "ResetServiceSetting", &json!({ "SettingId": id })).await
}

/// Every setting the service teams provision reports its provisioned reading, takes the
/// account's own value, and is restored to the provisioned one by a reset — whichever
/// spelling of its id the calls use.
///
/// The reset is asserted against the value the FIRST read reported, not against a value
/// written into the test, so what it restores has to come from the store rather than from
/// the request.
#[tokio::test]
async fn every_provisioned_setting_takes_a_value_and_is_reset_to_the_provisioned_one() {
    let router = router();
    let modifier = modifying_user();
    for (index, setting) in SETTINGS.iter().enumerate() {
        let case = setting.id;
        // Half the rows address the setting by its documented path and half by its ARN:
        // either spelling names the one setting.
        let addressed = if index % 2 == 0 {
            setting.id.to_string()
        } else {
            setting_arn(REGION, setting.id)
        };

        let provisioned = get_ok(&router, case, &addressed).await;
        assert_eq!(
            provisioned["SettingId"],
            json!(setting.id),
            "case {case:?}: expected the documented setting id, got {provisioned}"
        );
        assert_eq!(
            provisioned["ARN"],
            json!(setting_arn(REGION, setting.id)),
            "case {case:?}: expected the setting's ARN at the signed Region, got {provisioned}"
        );
        assert_eq!(
            provisioned["Status"],
            json!(STATUS_DEFAULT),
            "case {case:?}: an unmodified setting is at its provisioned value, got {provisioned}"
        );
        assert_eq!(
            provisioned["LastModifiedUser"],
            json!(PROVISIONING_USER),
            "case {case:?}: an unmodified setting was last modified by the service team that \
             provisioned it, got {provisioned}"
        );
        let provisioned_value = provisioned["SettingValue"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a setting reports a value, got {provisioned}"))
            .to_string();

        // A read modifies nothing: the second read reports the first one's reading whole,
        // modification instant included.
        let read_again = get_ok(&router, case, &addressed).await;
        assert_eq!(
            read_again, provisioned,
            "case {case:?}: reading a setting must not change it, got {read_again} after \
             {provisioned}"
        );

        let custom = custom_value(index, setting);
        assert_ne!(
            custom, provisioned_value,
            "case {case:?}: the table must customize the setting to a different value"
        );
        // A reset of a setting that is already at its provisioned value restores nothing,
        // so it records nothing and reports the reading the setting already held.
        let (status, body) = reset(&router, &addressed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: resetting an unmodified setting must succeed, got {status} {body}"
        );
        assert_eq!(
            body["ServiceSetting"], provisioned,
            "case {case:?}: a reset that restores nothing must report the provisioned reading \
             unchanged, got {body}"
        );
        let (status, body) = update(&router, &addressed, &custom).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateServiceSetting {custom:?} must succeed, got {status} {body}"
        );

        // Read back by the OTHER spelling of the id than the update used.
        let customized = get_ok(
            &router,
            case,
            &if index % 2 == 0 {
                setting_arn(REGION, setting.id)
            } else {
                setting.id.to_string()
            },
        )
        .await;
        assert_eq!(
            customized["SettingValue"],
            json!(custom),
            "case {case:?}: expected the value the update stored, got {customized}"
        );
        assert_eq!(
            customized["Status"],
            json!(STATUS_CUSTOMIZED),
            "case {case:?}: a setting the account overwrote is customized, got {customized}"
        );
        assert_eq!(
            customized["LastModifiedUser"],
            json!(modifier),
            "case {case:?}: expected the calling principal as the modifier, got {customized}"
        );
        let reported_modifier = customized["LastModifiedUser"].as_str().unwrap_or_default();
        assert!(
            reported_modifier.starts_with(&format!("arn:aws:iam::{ACCOUNT}:user/")),
            "case {case:?}: the modifier must be an IAM principal ARN of this account, got \
             {customized}"
        );

        let (status, body) = reset(&router, &addressed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ResetServiceSetting must succeed, got {status} {body}"
        );
        let answered = body["ServiceSetting"].clone();
        assert_eq!(
            answered["SettingValue"],
            json!(provisioned_value),
            "case {case:?}: expected the provisioned value {provisioned_value:?} restored, got \
             {answered}"
        );
        assert_eq!(
            answered["Status"],
            json!(STATUS_DEFAULT),
            "case {case:?}: a reset setting is back at its provisioned value, got {answered}"
        );
        assert_eq!(
            answered["SettingId"],
            json!(setting.id),
            "case {case:?}: expected the documented setting id, got {answered}"
        );
        assert_eq!(
            answered["ARN"],
            json!(setting_arn(REGION, setting.id)),
            "case {case:?}: expected the setting's ARN at the signed Region, got {answered}"
        );

        let after = get_ok(&router, case, &addressed).await;
        assert_eq!(
            after, answered,
            "case {case:?}: the read must report what the reset answered, got {after} after \
             {answered}"
        );

        // Restoring the provisioned value is idempotent: resetting a setting already at
        // it reports the reading the first reset recorded, whole, rather than recording a
        // second restoration or refusing.
        let (status, body) = reset(&router, &addressed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a second ResetServiceSetting must succeed, got {status} {body}"
        );
        assert_eq!(
            body["ServiceSetting"], answered,
            "case {case:?}: expected the reading the reset recorded, got {body}"
        );
    }
}

/// A setting whose value names a resource reports the unconfigured sentinel until the
/// account names one — the catalogue's own value for a setting of that kind, asserted on
/// every entry that carries it.
#[tokio::test]
async fn a_setting_that_names_nothing_reports_the_unconfigured_value() {
    let router = router();
    let unset: Vec<&SettingDefinition> = SETTINGS
        .iter()
        .filter(|setting| setting.default_value == UNSET_VALUE)
        .collect();
    assert!(
        !unset.is_empty(),
        "the catalogue must hold at least one setting that names a resource"
    );
    for setting in unset {
        let reading = get_ok(&router, setting.id, setting.id).await;
        assert_eq!(
            reading["SettingValue"],
            json!(UNSET_VALUE),
            "case {:?}: expected the unconfigured value, got {reading}",
            setting.id
        );
    }
}

/// A `SettingId` no Amazon Web Services service team provisions is refused 400
/// `ServiceSettingNotFound` by every operation of the slice — "You can't create a new
/// SettingId" — and nothing is stored for it.
#[tokio::test]
async fn a_setting_no_service_team_provisions_is_refused() {
    let router = router();
    // Each row is a different WAY of naming no provisioned setting: an unknown leaf under
    // a provisioned namespace, a prefix of a provisioned id, the same id without its
    // leading separator and in the wrong letter case, a setting ARN whose path names
    // nothing, another resource's ARN of this service, and another service's ARN.
    let unprovisioned = [
        "/ssm/parameter-store/no-such-setting",
        "/ssm/parameter-store",
        "ssm/parameter-store/high-throughput-enabled",
        "/SSM/Parameter-Store/High-Throughput-Enabled",
        &setting_arn(REGION, "/ssm/parameter-store/no-such-setting"),
        &format!("arn:aws:ssm:{REGION}:{ACCOUNT}:parameter/not-a-setting"),
        &format!("arn:aws:s3:{REGION}:{ACCOUNT}:servicesetting/ssm/opsinsights/opscenter"),
    ];
    for id in unprovisioned {
        for (operation, body) in [
            ("GetServiceSetting", json!({ "SettingId": id })),
            (
                "UpdateServiceSetting",
                json!({ "SettingId": id, "SettingValue": "true" }),
            ),
            ("ResetServiceSetting", json!({ "SettingId": id })),
        ] {
            let (status, response) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {id:?} / {operation}: expected the refusal of an unprovisioned setting, got \
                 {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ServiceSettingNotFound",
                "case {id:?} / {operation}: expected ServiceSettingNotFound, got {response}"
            );
            let refusal = message(&response);
            assert_eq!(
                refusal, SETTING_NOT_FOUND,
                "case {id:?} / {operation}: expected the error shape's own sentence, got {response}"
            );
        }
    }
}

/// A setting the API documents values for takes those values — in any letter case, and
/// stored as the account spelled them — and refuses 400 `ValidationException` anything
/// else; a setting it documents none for takes free text.
#[tokio::test]
async fn a_setting_takes_the_values_it_documents() {
    let router = router();
    for setting in SETTINGS {
        let case = setting.id;
        for documented in setting.values {
            for spelling in [
                documented.to_string(),
                documented.to_ascii_uppercase(),
                documented.to_ascii_lowercase(),
            ] {
                let (status, body) = update(&router, setting.id, &spelling).await;
                assert_eq!(
                    status, 200,
                    "case {case:?} / {spelling:?}: a documented value must be accepted, got \
                     {status} {body}"
                );
                let reading = get_ok(&router, case, setting.id).await;
                assert_eq!(
                    reading["SettingValue"],
                    json!(spelling),
                    "case {case:?} / {spelling:?}: expected the value as it was written, got \
                     {reading}"
                );
            }
        }

        // What no setting accepts: a value outside its documented list. A setting that
        // documents no list takes free text, so the same input is stored there.
        let outside = "not-a-value-this-setting-documents";
        let (status, body) = update(&router, setting.id, outside).await;
        if setting.values.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: a setting that documents no values takes free text, got {status} \
                 {body}"
            );
            let reading = get_ok(&router, case, setting.id).await;
            assert_eq!(
                reading["SettingValue"],
                json!(outside),
                "case {case:?}: expected the free-text value stored, got {reading}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a value outside {:?} must be refused, got {status} {body}",
                setting.values
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body}"
            );
            let refusal = message(&body);
            assert!(
                refusal.contains(outside) && refusal.contains(setting.id),
                "case {case:?}: the refusal must name the value and the setting, got {body}"
            );
        }

        // The refusal left the setting as it was.
        let (status, _) = reset(&router, setting.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the setting must still be resettable after a refusal"
        );
    }
}

/// A setting is held at the endpoint it was set through, and each setting holds its own
/// value: a value set in one Region leaves the same setting in another Region, and every
/// other setting, at the provisioned reading.
#[tokio::test]
async fn a_setting_is_held_per_region_and_per_setting() {
    let router = router();
    let subject = SETTINGS
        .iter()
        .find(|setting| !setting.values.is_empty())
        .expect("the catalogue documents values for at least one setting");
    let custom = custom_value(0, subject);

    let (status, body) = update(&router, subject.id, &custom).await;
    assert_eq!(
        status, 200,
        "case {:?}: the update must succeed, got {status} {body}",
        subject.id
    );

    let (status, body) = call_in_region(
        &router,
        OTHER_REGION,
        "GetServiceSetting",
        &json!({ "SettingId": subject.id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {OTHER_REGION}: the read must succeed, got {status} {body}"
    );
    let elsewhere = &body["ServiceSetting"];
    assert_eq!(
        elsewhere["Status"],
        json!(STATUS_DEFAULT),
        "case {OTHER_REGION}: a value set at another endpoint must not be reported here, got \
         {elsewhere}"
    );
    assert_eq!(
        elsewhere["SettingValue"],
        json!(subject.default_value),
        "case {OTHER_REGION}: expected the provisioned value, got {elsewhere}"
    );
    assert_eq!(
        elsewhere["ARN"],
        json!(setting_arn(OTHER_REGION, subject.id)),
        "case {OTHER_REGION}: expected this endpoint's ARN, got {elsewhere}"
    );

    for other in SETTINGS.iter().filter(|setting| setting.id != subject.id) {
        let reading = get_ok(&router, other.id, other.id).await;
        assert_eq!(
            reading["Status"],
            json!(STATUS_DEFAULT),
            "case {:?}: customizing another setting must leave this one provisioned, got {reading}",
            other.id
        );
        assert_eq!(
            reading["SettingValue"],
            json!(other.default_value),
            "case {:?}: expected the provisioned value, got {reading}",
            other.id
        );
    }
}

/// The store a modification writes to is BOUNDED.
///
/// A setting is recorded under the Region of the request's own credential scope, which
/// the caller chooses, so the rows here differ along that axis: a modification for a
/// Region past the emulator's own total is refused rather than growing the store, a
/// setting the store already holds stays modifiable with the store full, and a read of
/// one it does not hold still answers — because a read records nothing.
#[tokio::test]
async fn the_store_a_modification_writes_to_is_bounded() {
    let router = router();
    let subject = &SETTINGS[0];
    let custom = custom_value(0, subject);

    let mut refused = None;
    for index in 0..=MAX_SERVICE_SETTINGS_TOTAL {
        // One Region per row, each recording one setting, so the entry count is the row
        // count and the guard trips on the row that would exceed it.
        let region = format!("ap-emulator-{index}");
        let (status, body) = call_in_region(
            &router,
            &region,
            "UpdateServiceSetting",
            &json!({ "SettingId": subject.id, "SettingValue": custom }),
        )
        .await;
        if status != 200 {
            refused = Some((index, status, body));
            break;
        }
    }

    let (index, status, body) = refused.unwrap_or_else(|| {
        panic!(
            "case the full store: a modification past {MAX_SERVICE_SETTINGS_TOTAL} settings must \
             be refused"
        )
    });
    assert_eq!(
        index, MAX_SERVICE_SETTINGS_TOTAL,
        "case the full store: expected the refusal only on the modification past \
         {MAX_SERVICE_SETTINGS_TOTAL}, got it at {index} with {body}"
    );
    assert_eq!(
        status, 400,
        "case the full store: expected the emulator's own guard to refuse, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case the full store: expected the refusal this crate's other total guards answer with, \
         got {body}"
    );
    let refusal = message(&body);
    assert!(
        refusal.contains(&MAX_SERVICE_SETTINGS_TOTAL.to_string()),
        "case the full store: the refusal must name the total it tripped, got {body}"
    );

    // A setting the store already holds stays modifiable: the guard refuses a NEW key, so
    // it cannot wedge a setting an account has already set.
    let held_region = "ap-emulator-0";
    let (status, body) = call_in_region(
        &router,
        held_region,
        "UpdateServiceSetting",
        &json!({ "SettingId": subject.id, "SettingValue": subject.default_value }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {held_region}: a setting already recorded must stay modifiable, got {status} {body}"
    );
    let (status, body) = call_in_region(
        &router,
        held_region,
        "GetServiceSetting",
        &json!({ "SettingId": subject.id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {held_region}: the read must succeed, got {status} {body}"
    );
    assert_eq!(
        body["ServiceSetting"]["SettingValue"],
        json!(subject.default_value),
        "case {held_region}: expected the value the last update stored, got {body}"
    );
    assert_eq!(
        body["ServiceSetting"]["Status"],
        json!(STATUS_CUSTOMIZED),
        "case {held_region}: a value the account set is customized, got {body}"
    );
    let (status, body) = call_in_region(
        &router,
        held_region,
        "ResetServiceSetting",
        &json!({ "SettingId": subject.id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {held_region}: a setting already recorded must stay resettable, got {status} {body}"
    );

    // And a Region the store holds nothing for still reads: nothing is recorded by a read,
    // so the guard has nothing to refuse.
    let (status, body) = call_in_region(
        &router,
        "ap-emulator-unrecorded",
        "GetServiceSetting",
        &json!({ "SettingId": subject.id }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case an unrecorded Region: the read must succeed with the store full, got {status} {body}"
    );
    assert_eq!(
        body["ServiceSetting"]["Status"],
        json!(STATUS_DEFAULT),
        "case an unrecorded Region: expected the provisioned reading, got {body}"
    );
}
