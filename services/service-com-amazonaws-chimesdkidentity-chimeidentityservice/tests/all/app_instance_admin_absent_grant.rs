// SPDX-License-Identifier: CC0-1.0

//! Reading an app-instance administrator grant this service holds no record of, end to
//! end over HTTP.
//!
//! The RULE pinned here: `DescribeAppInstanceAdmin` resolves the PROMOTION the request's
//! pair of ARNs names, and that resolution is an access decision — a pair this service
//! holds no promotion for is refused with the modelled `ForbiddenException` (HTTP 403,
//! `Code` `Forbidden`) carrying the service's one fixed forbidden sentence, with nothing
//! interpolated from the request, rather than with the malformed-input refusal.
//!
//! The table spans what the rule generalises over — every way the grant can be missing:
//! an identity never promoted, one promoted and then demoted, one demoted after a
//! repeated demotion, an identity of another app instance, an app instance that never
//! existed, an app instance that was deleted after the grant was recorded, and the
//! app-instance ARN named as its own administrator. A promoted pair is the control: it
//! still reads 200, so the refusal is a resolution and not a blanket answer.

use serde_json::{json, Value};

use crate::app_instance_lifecycle::{uri_encode, Emulator};

/// The one sentence this service's `ForbiddenException` carries, as AWS words it for a
/// request the caller may not make.
const FORBIDDEN_MESSAGE: &str = "You are not allowed to make this request.";

/// What a row does to the app instance and the identity before the read, and how the two
/// ARNs the read names are built.
#[derive(Debug, Clone, Copy)]
enum Setup {
    /// Nothing: the identity was never promoted.
    NeverPromoted,
    /// Promote, then demote the identity `times` times over.
    DemotedTimes(u32),
    /// Promote the identity, then DELETE the app instance it administers.
    PromotedThenInstanceDeleted,
}

/// One row of the absent-grant table.
#[derive(Debug)]
struct AbsentGrantCase {
    label: &'static str,
    setup: Setup,
    /// How the admin ARN the read names is built from the app instance ARN and the
    /// stored user's ARN.
    admin_arn: fn(&str, &str) -> String,
    /// How the app instance ARN the read names is built from the stored one.
    app_instance_arn: fn(&str) -> String,
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

#[tokio::test]
async fn reading_an_admin_grant_this_service_holds_no_record_of_is_forbidden() {
    let cases = [
        AbsentGrantCase {
            label: "an AppInstanceUser of the app instance that was never promoted",
            setup: Setup::NeverPromoted,
            admin_arn: |_instance, user| user.to_string(),
            app_instance_arn: |instance| instance.to_string(),
        },
        AbsentGrantCase {
            label: "an AppInstanceUser promoted and then demoted",
            setup: Setup::DemotedTimes(1),
            admin_arn: |_instance, user| user.to_string(),
            app_instance_arn: |instance| instance.to_string(),
        },
        AbsentGrantCase {
            label: "an AppInstanceUser demoted three times over",
            setup: Setup::DemotedTimes(3),
            admin_arn: |_instance, user| user.to_string(),
            app_instance_arn: |instance| instance.to_string(),
        },
        AbsentGrantCase {
            label: "an AppInstanceBot of the app instance, promoted and then demoted",
            setup: Setup::DemotedTimes(1),
            admin_arn: |instance, _user| format!("{instance}/bot/helper"),
            app_instance_arn: |instance| instance.to_string(),
        },
        AbsentGrantCase {
            label: "an AppInstanceUser the service does not hold, never promoted",
            setup: Setup::NeverPromoted,
            admin_arn: |instance, _user| format!("{instance}/user/nobody"),
            app_instance_arn: |instance| instance.to_string(),
        },
        AbsentGrantCase {
            label: "an identity of another app instance",
            setup: Setup::NeverPromoted,
            admin_arn: |_instance, _user| {
                "arn:aws:chime:us-west-2:111122223333:app-instance/other/user/alice".to_string()
            },
            app_instance_arn: |instance| instance.to_string(),
        },
        AbsentGrantCase {
            label: "an app instance that never existed",
            setup: Setup::NeverPromoted,
            admin_arn: |_instance, _user| {
                "arn:aws:chime:us-west-2:111122223333:app-instance/absent/user/alice".to_string()
            },
            app_instance_arn: |_instance| {
                "arn:aws:chime:us-west-2:111122223333:app-instance/absent".to_string()
            },
        },
        AbsentGrantCase {
            label: "the app instance ARN named as its own administrator",
            setup: Setup::NeverPromoted,
            admin_arn: |instance, _user| instance.to_string(),
            app_instance_arn: |instance| instance.to_string(),
        },
        AbsentGrantCase {
            label: "a promoted identity whose app instance was then deleted",
            setup: Setup::PromotedThenInstanceDeleted,
            admin_arn: |_instance, user| user.to_string(),
            app_instance_arn: |instance| instance.to_string(),
        },
    ];

    for (row, case) in cases.iter().enumerate() {
        // One app instance per row, so a row that deletes its instance cannot change what
        // the next row reads.
        let emulator = Emulator::new();
        let instance = emulator
            .app_instance(&format!("absent-grant-{row}"), &format!("tok-instance-{row}"))
            .await;
        let alice = emulator
            .user(&instance, &format!("alice-{row}"), &format!("tok-user-{row}"))
            .await;

        let app_instance_arn = (case.app_instance_arn)(&instance);
        let admin_arn = (case.admin_arn)(&instance, &alice);

        match case.setup {
            Setup::NeverPromoted => {}
            Setup::DemotedTimes(times) => {
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
                for attempt in 1..=times {
                    let (status, body) = demote(&emulator, &app_instance_arn, &admin_arn).await;
                    assert_eq!(
                        status, 204,
                        "case {:?}: demotion {attempt} must answer 204, got {status} {body:?}",
                        case.label
                    );
                }
            }
            Setup::PromotedThenInstanceDeleted => {
                let (status, body) = promote(&emulator, &app_instance_arn, &admin_arn).await;
                assert_eq!(
                    status, 201,
                    "case {:?}: the setup promotion must answer 201, got {status} {body:?}",
                    case.label
                );
                let (status, body) = emulator
                    .call(
                        "DELETE",
                        &format!("/app-instances/{}", uri_encode(&app_instance_arn)),
                        None,
                    )
                    .await;
                assert_eq!(
                    status, 204,
                    "case {:?}: DeleteAppInstance must answer 204, got {status} {body:?}",
                    case.label
                );
            }
        }

        let (status, body) = describe_admin(&emulator, &app_instance_arn, &admin_arn).await;
        assert_eq!(
            (status, body["Code"].as_str(), body["Message"].as_str()),
            (403, Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
            "case {:?}: expected the modelled forbidden refusal of an absent grant, got \
             {status} {body:?}",
            case.label
        );
        // The refusal carries the fixed sentence ALONE: neither ARN the request named is
        // interpolated into it.
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            !message.contains(&admin_arn) && !message.contains(&app_instance_arn),
            "case {:?}: the refusal must interpolate neither ARN, got {message:?}",
            case.label
        );

        // Repeating the read answers exactly the same thing: the refusal reports state,
        // it does not change it.
        let (again, again_body) = describe_admin(&emulator, &app_instance_arn, &admin_arn).await;
        assert_eq!(
            (again, again_body["Code"].as_str()),
            (403, Some("Forbidden")),
            "case {:?}: the repeated read must answer the same refusal, got {again} \
             {again_body:?}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_held_grant_still_reads_while_its_neighbours_are_refused() {
    let emulator = Emulator::new();
    let instance = emulator.app_instance("held-grant", "tok-held-instance").await;
    let alice = emulator.user(&instance, "alice", "tok-held-alice").await;
    let bob = emulator.user(&instance, "bob", "tok-held-bob").await;

    let (status, body) = promote(&emulator, &instance, &alice).await;
    assert_eq!(
        status, 201,
        "promoting the held administrator must answer 201, got {status} {body:?}"
    );

    // The control: the pair the directory holds is served, so the refusal below is the
    // resolution of a grant and not a blanket answer from this operation.
    let (status, held) = describe_admin(&emulator, &instance, &alice).await;
    assert_eq!(
        status, 200,
        "the held grant must read 200, got {status} {held:?}"
    );
    assert_eq!(
        held["AppInstanceAdmin"]["Admin"]["Arn"].as_str(),
        Some(alice.as_str()),
        "the held grant must report the promoted identity, got {held:?}"
    );

    // A SECOND identity of the same app instance, never promoted: same app instance, same
    // ARN shape, no grant — refused.
    let (status, body) = describe_admin(&emulator, &instance, &bob).await;
    assert_eq!(
        (status, body["Code"].as_str(), body["Message"].as_str()),
        (403, Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
        "an unpromoted identity of the same app instance must be refused, got {status} \
         {body:?}"
    );

    // And the promoted identity read under a DIFFERENT app instance is a different grant,
    // which this service holds no record of either.
    let other = emulator
        .app_instance("other-held-grant", "tok-held-other")
        .await;
    let (status, body) = describe_admin(&emulator, &other, &alice).await;
    assert_eq!(
        (status, body["Code"].as_str(), body["Message"].as_str()),
        (403, Some("Forbidden"), Some(FORBIDDEN_MESSAGE)),
        "the same identity under another app instance must be refused, got {status} \
         {body:?}"
    );

    // The refusals left the held grant exactly as it was.
    let (status, after) = describe_admin(&emulator, &instance, &alice).await;
    assert_eq!(
        status, 200,
        "the held grant must survive the refused reads, got {status} {after:?}"
    );
    assert_eq!(
        after["AppInstanceAdmin"], held["AppInstanceAdmin"],
        "the held grant must be unchanged by the refused reads, got {after:?}"
    );
}
