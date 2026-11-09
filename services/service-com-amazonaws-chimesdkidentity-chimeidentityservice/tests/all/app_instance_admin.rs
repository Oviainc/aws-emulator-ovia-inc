// SPDX-License-Identifier: CC0-1.0

//! Promoting an identity to an `AppInstanceAdmin`, end to end over HTTP.
//!
//! The RULE pinned here: `CreateAppInstanceAdmin` promotes an identity OF the named app
//! instance and persists the promotion, so `DescribeAppInstanceAdmin` reads the same
//! record back — the promoted identity, the app instance it administers and the instant
//! the promotion was recorded — while an ARN that is not an identity of that app
//! instance, an `AppInstanceUser` the service does not hold, and an app instance that
//! does not exist are all refused rather than recorded.
//!
//! The table spans what the rule generalises over: the kind of identity promoted (a
//! stored `AppInstanceUser`, an `AppInstanceBot` whose record this service does not keep),
//! each way the request can name something absent, and the number of times the same
//! promotion is requested.

use serde_json::{json, Value};

use crate::app_instance_lifecycle::{uri_encode, Emulator};

/// One row of the promotion table: what is promoted over which app instance, and the
/// status the service answers with.
#[derive(Debug)]
struct PromotionCase {
    label: &'static str,
    /// How the admin ARN is built from the app instance ARN and the stored user's ARN.
    admin_arn: fn(&str, &str) -> String,
    /// How the app instance ARN is built from the stored one.
    app_instance_arn: fn(&str) -> String,
    expected_status: u16,
    /// The error `Code` the refusal carries, for a row that is refused.
    expected_code: Option<&'static str>,
    /// Whether the promoted identity's `Name` is one this service holds.
    named: bool,
}

async fn promote(emulator: &Emulator, app_instance_arn: &str, admin_arn: &str) -> (u16, Value) {
    emulator
        .call(
            "POST",
            &format!("/app-instances/{}/admins", uri_encode(app_instance_arn)),
            Some(json!({ "AppInstanceAdminArn": admin_arn })),
        )
        .await
}

async fn describe_admin(
    emulator: &Emulator,
    app_instance_arn: &str,
    admin_arn: &str,
) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!(
                "/app-instances/{}/admins/{}",
                uri_encode(app_instance_arn),
                uri_encode(admin_arn)
            ),
            None,
        )
        .await
}

#[tokio::test]
async fn an_app_instance_admin_is_promoted_and_read_back() {
    let cases = [
        PromotionCase {
            label: "a stored AppInstanceUser of the app instance",
            admin_arn: |_instance, user| user.to_string(),
            app_instance_arn: |instance| instance.to_string(),
            expected_status: 201,
            expected_code: None,
            named: true,
        },
        PromotionCase {
            label: "an AppInstanceBot of the app instance, whose record this service does not keep",
            admin_arn: |instance, _user| format!("{instance}/bot/helper"),
            app_instance_arn: |instance| instance.to_string(),
            expected_status: 201,
            expected_code: None,
            named: false,
        },
        PromotionCase {
            label: "an AppInstanceUser the service does not hold",
            admin_arn: |instance, _user| format!("{instance}/user/nobody"),
            app_instance_arn: |instance| instance.to_string(),
            expected_status: 400,
            expected_code: Some("NotFound"),
            named: false,
        },
        PromotionCase {
            label: "an identity of another app instance",
            admin_arn: |_instance, _user| {
                "arn:aws:chime:us-west-2:111122223333:app-instance/other/user/alice".to_string()
            },
            app_instance_arn: |instance| instance.to_string(),
            expected_status: 400,
            expected_code: Some("BadRequest"),
            named: false,
        },
        PromotionCase {
            label: "an app instance that does not exist",
            admin_arn: |instance, _user| format!("{instance}/user/alice"),
            app_instance_arn: |_instance| {
                "arn:aws:chime:us-west-2:111122223333:app-instance/absent".to_string()
            },
            expected_status: 400,
            expected_code: Some("NotFound"),
            named: false,
        },
    ];

    let emulator = Emulator::new();
    let instance = emulator.app_instance("admins", "admins-token").await;
    let alice = emulator.user(&instance, "alice", "alice-token").await;

    for case in &cases {
        let app_instance_arn = (case.app_instance_arn)(&instance);
        let admin_arn = (case.admin_arn)(&instance, &alice);
        let (status, body) = promote(&emulator, &app_instance_arn, &admin_arn).await;
        assert_eq!(
            status, case.expected_status,
            "case {:?}: CreateAppInstanceAdmin must answer {}, got {status} {body:?}",
            case.label, case.expected_status
        );
        if let Some(code) = case.expected_code {
            assert_eq!(
                body["Code"].as_str(),
                Some(code),
                "case {:?}: wrong refusal Code, body {body:?}",
                case.label
            );
            // A refused promotion is not recorded, so nothing reads it back: the read of
            // a grant this service holds no record of is the permanent refusal.
            let (described, described_body) =
                describe_admin(&emulator, &app_instance_arn, &admin_arn).await;
            assert_eq!(
                (described, described_body["Code"].as_str()),
                (403, Some("Forbidden")),
                "case {:?}: a refused promotion must not be readable, got {described} {described_body:?}",
                case.label
            );
            continue;
        }

        assert_eq!(
            body["AppInstanceArn"].as_str(),
            Some(app_instance_arn.as_str()),
            "case {:?}: wrong AppInstanceArn in the promotion, body {body:?}",
            case.label
        );
        assert_eq!(
            body["AppInstanceAdmin"]["Arn"].as_str(),
            Some(admin_arn.as_str()),
            "case {:?}: wrong promoted identity, body {body:?}",
            case.label
        );

        let (described, described_body) =
            describe_admin(&emulator, &app_instance_arn, &admin_arn).await;
        assert_eq!(
            described, 200,
            "case {:?}: DescribeAppInstanceAdmin must answer 200, got {described} {described_body:?}",
            case.label
        );
        let admin = &described_body["AppInstanceAdmin"];
        assert_eq!(
            admin["Admin"]["Arn"].as_str(),
            Some(admin_arn.as_str()),
            "case {:?}: wrong Admin.Arn, got {:?}",
            case.label,
            admin["Admin"]
        );
        assert_eq!(
            admin["AppInstanceArn"].as_str(),
            Some(app_instance_arn.as_str()),
            "case {:?}: wrong AppInstanceArn, got {:?}",
            case.label,
            admin["AppInstanceArn"]
        );
        assert!(
            admin["CreatedTimestamp"].is_number(),
            "case {:?}: CreatedTimestamp must be set, got {:?}",
            case.label,
            admin["CreatedTimestamp"]
        );
        let expected_name = case.named.then_some("alice");
        assert_eq!(
            admin["Admin"].get("Name").and_then(Value::as_str),
            expected_name,
            "case {:?}: wrong Admin.Name, got {:?}",
            case.label,
            admin["Admin"].get("Name")
        );

        // Promotion is a SET: repeating the request neither duplicates the record nor
        // moves the instant it reports.
        let first_timestamp = admin["CreatedTimestamp"].clone();
        for repeat in 1..=2 {
            let (again, again_body) = promote(&emulator, &app_instance_arn, &admin_arn).await;
            assert_eq!(
                again, 201,
                "case {:?}: repeat {repeat} of the promotion must answer 201, got {again} {again_body:?}",
                case.label
            );
        }
        let (_, after) = describe_admin(&emulator, &app_instance_arn, &admin_arn).await;
        assert_eq!(
            after["AppInstanceAdmin"]["CreatedTimestamp"], first_timestamp,
            "case {:?}: a repeated promotion must keep the first CreatedTimestamp, got {:?}",
            case.label, after["AppInstanceAdmin"]["CreatedTimestamp"]
        );
    }
}
