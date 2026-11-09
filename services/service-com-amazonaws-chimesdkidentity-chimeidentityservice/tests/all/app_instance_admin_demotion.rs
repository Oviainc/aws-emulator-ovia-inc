// SPDX-License-Identifier: CC0-1.0

//! Demoting an `AppInstanceAdmin` with `DeleteAppInstanceAdmin`, end to end over HTTP.
//!
//! The RULE pinned here: a request scoped to an app instance this service holds drops any
//! recorded promotion of the named identity and answers the modelled 204 — whether the
//! directory held a promotion or not, and however many times the request is repeated —
//! leaving the identity itself in place, while a request scoped to an app instance this
//! service does not hold is refused: an app instance that never existed as absent, and one
//! that was DELETED as a request the caller may no longer make.
//!
//! The table spans what the rule generalises over: whether a promotion exists at all, the
//! kind of identity named (a stored `AppInstanceUser`, an `AppInstanceBot` whose record
//! this service does not keep, an ARN that is no identity of the app instance), the number
//! of times the demotion is requested, and an app instance that does not exist; the tail of
//! the test covers the deleted app instance, which only a delete can reach.

use serde_json::{json, Value};

use crate::app_instance_lifecycle::{uri_encode, Emulator};

/// The one sentence this service's `ForbiddenException` carries, as AWS words it for a
/// request the caller may not make.
const FORBIDDEN_MESSAGE: &str = "You are not allowed to make this request.";

/// One row of the demotion table.
#[derive(Debug)]
struct DemotionCase {
    label: &'static str,
    /// How the admin ARN is built from the app instance ARN and the stored user's ARN.
    admin_arn: fn(&str, &str) -> String,
    /// How the app instance ARN the request is scoped to is built from the stored one.
    app_instance_arn: fn(&str) -> String,
    /// Whether the identity is promoted before it is demoted.
    promote_first: bool,
    expected_status: u16,
    /// The error `Code` the refusal carries, for a row that is refused.
    expected_code: Option<&'static str>,
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

async fn demote(emulator: &Emulator, app_instance_arn: &str, admin_arn: &str) -> (u16, Value) {
    emulator
        .call(
            "DELETE",
            &format!(
                "/app-instances/{}/admins/{}",
                uri_encode(app_instance_arn),
                uri_encode(admin_arn)
            ),
            None,
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

/// Every administrator `ListAppInstanceAdmins` reports for one app instance, by ARN.
async fn listed_admins(emulator: &Emulator, app_instance_arn: &str) -> Vec<String> {
    let (status, body) = emulator
        .call(
            "GET",
            &format!("/app-instances/{}/admins", uri_encode(app_instance_arn)),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "ListAppInstanceAdmins({app_instance_arn}) answered {status} {body:?}"
    );
    body["AppInstanceAdmins"]
        .as_array()
        .unwrap_or_else(|| panic!("ListAppInstanceAdmins returned no list: {body:?}"))
        .iter()
        .map(|summary| {
            summary["Admin"]["Arn"]
                .as_str()
                .unwrap_or_else(|| panic!("a listed administrator with no Arn: {summary:?}"))
                .to_string()
        })
        .collect()
}

async fn describe_user(emulator: &Emulator, user_arn: &str) -> (u16, Value) {
    emulator
        .call(
            "GET",
            &format!("/app-instance-users/{}", uri_encode(user_arn)),
            None,
        )
        .await
}

#[tokio::test]
async fn demoting_an_app_instance_admin_drops_only_its_promotion() {
    let cases = [
        DemotionCase {
            label: "a promoted AppInstanceUser of the app instance",
            admin_arn: |_instance, user| user.to_string(),
            app_instance_arn: |instance| instance.to_string(),
            promote_first: true,
            expected_status: 204,
            expected_code: None,
        },
        DemotionCase {
            label: "a promoted AppInstanceBot, whose record this service does not keep",
            admin_arn: |instance, _user| format!("{instance}/bot/helper"),
            app_instance_arn: |instance| instance.to_string(),
            promote_first: true,
            expected_status: 204,
            expected_code: None,
        },
        DemotionCase {
            label: "an AppInstanceUser of the app instance that was never promoted",
            admin_arn: |_instance, user| user.to_string(),
            app_instance_arn: |instance| instance.to_string(),
            promote_first: false,
            expected_status: 204,
            expected_code: None,
        },
        DemotionCase {
            label: "an ARN that is no identity of the app instance, and was never promoted",
            admin_arn: |_instance, _user| {
                "arn:aws:chime:us-west-2:111122223333:app-instance/other/user/alice".to_string()
            },
            app_instance_arn: |instance| instance.to_string(),
            promote_first: false,
            expected_status: 204,
            expected_code: None,
        },
        DemotionCase {
            label: "an app instance that does not exist",
            admin_arn: |instance, _user| format!("{instance}/user/alice"),
            app_instance_arn: |_instance| {
                "arn:aws:chime:us-west-2:111122223333:app-instance/absent".to_string()
            },
            promote_first: false,
            expected_status: 400,
            expected_code: Some("NotFound"),
        },
    ];

    let emulator = Emulator::new();
    let instance = emulator.app_instance("demotions", "demotions-token").await;
    let alice = emulator.user(&instance, "alice", "alice-token").await;
    // A SECOND administrator nothing in the table touches: demoting one promotion must
    // leave every other promotion of the same app instance standing.
    let bob = emulator.user(&instance, "bob", "bob-token").await;
    let (promoted_bob, bob_body) = promote(&emulator, &instance, &bob).await;
    assert_eq!(
        promoted_bob, 201,
        "promoting the bystander must answer 201, got {promoted_bob} {bob_body:?}"
    );

    for case in &cases {
        let app_instance_arn = (case.app_instance_arn)(&instance);
        let admin_arn = (case.admin_arn)(&instance, &alice);

        if case.promote_first {
            let (status, body) = promote(&emulator, &app_instance_arn, &admin_arn).await;
            assert_eq!(
                status, 201,
                "case {:?}: the setup promotion must answer 201, got {status} {body:?}",
                case.label
            );
            let (described, described_body) =
                describe_admin(&emulator, &app_instance_arn, &admin_arn).await;
            assert_eq!(
                described, 200,
                "case {:?}: the setup promotion must be readable, got {described} {described_body:?}",
                case.label
            );
        }

        // Repeating the demotion must answer the same thing every time: the first call
        // removes any record, the later ones have nothing left to remove.
        for attempt in 1..=3 {
            let (status, body) = demote(&emulator, &app_instance_arn, &admin_arn).await;
            assert_eq!(
                status, case.expected_status,
                "case {:?}: attempt {attempt} of DeleteAppInstanceAdmin must answer {}, got {status} {body:?}",
                case.label, case.expected_status
            );
            match case.expected_code {
                Some(code) => {
                    assert_eq!(
                        body["Code"].as_str(),
                        Some(code),
                        "case {:?}: attempt {attempt} carried the wrong refusal Code, body {body:?}",
                        case.label
                    );
                    // The refusal names what the REQUEST asked for, so the message is
                    // derived from the input rather than carried as a literal.
                    let message = body["Message"].as_str().unwrap_or_default();
                    assert!(
                        message.contains(&app_instance_arn),
                        "case {:?}: attempt {attempt} must name the requested AppInstance ARN {app_instance_arn}, got {message:?}",
                        case.label
                    );
                }
                None => assert_eq!(
                    body,
                    Value::Null,
                    "case {:?}: attempt {attempt} must answer an empty body, got {body:?}",
                    case.label
                ),
            }
        }

        if case.expected_code.is_none() {
            // The promotion is gone, so the reader no longer finds one: a grant this
            // service holds no record of is refused as a request the caller may not make.
            let (described, described_body) =
                describe_admin(&emulator, &app_instance_arn, &admin_arn).await;
            assert_eq!(
                described, 403,
                "case {:?}: a demoted identity must not be readable as an admin, got {described} {described_body:?}",
                case.label
            );
            assert_eq!(
                (
                    described_body["Code"].as_str(),
                    described_body["Message"].as_str()
                ),
                (Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
                "case {:?}: wrong refusal reading a demoted admin, body {described_body:?}",
                case.label
            );
        }

        // Demotion does not delete the identity: the stored user is still there.
        let (user_status, user_body) = describe_user(&emulator, &alice).await;
        assert_eq!(
            user_status, 200,
            "case {:?}: the demoted identity must survive, got {user_status} {user_body:?}",
            case.label
        );
        assert_eq!(
            user_body["AppInstanceUser"]["AppInstanceUserArn"].as_str(),
            Some(alice.as_str()),
            "case {:?}: wrong user read back after the demotion, body {user_body:?}",
            case.label
        );

        // And it drops ONLY the promotion it names: the bystander still administers.
        let (bystander, bystander_body) = describe_admin(&emulator, &instance, &bob).await;
        assert_eq!(
            bystander, 200,
            "case {:?}: the bystander admin must still be readable, got {bystander} {bystander_body:?}",
            case.label
        );

        // The listing of the app instance's administrators agrees with both reads: the
        // bystander is in it, the demoted identity is not.
        let listed = listed_admins(&emulator, &instance).await;
        assert!(
            listed.contains(&bob),
            "case {:?}: the bystander admin must still be listed, got {listed:?}",
            case.label
        );
        if case.expected_code.is_none() {
            assert!(
                !listed.contains(&admin_arn),
                "case {:?}: a demoted identity must not be listed, got {listed:?}",
                case.label
            );
        }
    }

    // A demoted identity can be promoted again, which is what makes the demotion a
    // removal from a set rather than a tombstone.
    let (again, again_body) = promote(&emulator, &instance, &alice).await;
    assert_eq!(
        again, 201,
        "re-promoting a demoted identity must answer 201, got {again} {again_body:?}"
    );
    let (described, described_body) = describe_admin(&emulator, &instance, &alice).await;
    assert_eq!(
        described, 200,
        "a re-promoted identity must be readable again, got {described} {described_body:?}"
    );
    let listed = listed_admins(&emulator, &instance).await;
    assert!(
        listed.contains(&alice),
        "a re-promoted identity must be listed again, got {listed:?}"
    );

    // An ARN whose app instance has been DELETED is a different absence from one that
    // never named an app instance: the instance that scoped the promotion is gone, so the
    // request is one this caller may no longer make, as every sibling operation answers.
    let (deleted, delete_body) = emulator
        .call(
            "DELETE",
            &format!("/app-instances/{}", uri_encode(&instance)),
            None,
        )
        .await;
    assert_eq!(
        deleted, 204,
        "DeleteAppInstance must answer 204, got {deleted} {delete_body:?}"
    );
    let (refused, refused_body) = demote(&emulator, &instance, &alice).await;
    assert_eq!(
        refused, 403,
        "demoting under a deleted app instance must answer 403, got {refused} {refused_body:?}"
    );
    assert_eq!(
        refused_body["Code"].as_str(),
        Some("Forbidden"),
        "wrong Code demoting under a deleted app instance, body {refused_body:?}"
    );
    assert_eq!(
        refused_body["Message"].as_str(),
        Some(FORBIDDEN_MESSAGE),
        "wrong Message demoting under a deleted app instance, body {refused_body:?}"
    );
}
