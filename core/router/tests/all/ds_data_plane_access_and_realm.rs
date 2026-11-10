// SPDX-License-Identifier: CC0-1.0

//! What the DATA plane of a directory answers FROM: the realm its objects live in, and
//! whether an account of it holds a password — two facts only the Directory Service
//! CONTROL plane holds.
//!
//! `ds:CreateMicrosoftAD` names the directory's domain and `ds:ResetUserPassword` gives one
//! of its accounts a password; the Directory Service DATA plane (`ds-data`) creates the
//! objects inside that directory and reports both. Neither crate can see the other's store,
//! so only this configuration — both services mounted on ONE engine, which is what
//! `core/router` ships — makes the join observable.
//!
//! The rules these rows pin:
//!
//! * the realm every object of a directory is reported in, and the realm inside its
//!   `UserPrincipalName` and `DistinguishedName`, is the DIRECTORY's own fully qualified
//!   domain name — the `Name` the control plane created it with — so two directories
//!   report two realms; and the organizational unit the object is named in is the one the
//!   directory delegates to the account, named after the NetBIOS `ShortName` of that same
//!   create;
//! * `CreateUser` carries no password, so the account it creates has no credential and is
//!   reported INACTIVE until the control plane's `ResetUserPassword` gives it one, which is
//!   what flips `Enabled` to true; an account neither plane holds, and a password the
//!   directory's policy refuses, are refused by that call and change nothing;
//! * and the data plane answers for the directory only while the control plane's DS Data
//!   feature is on for it (the sibling suites `directory_data_plane_authorization` and
//!   `directory_data_access_cycle` own that gate; these rows depend on it, so they turn it
//!   on and read the refusal once on either side of the window).
//!
//! The harness is the one `directory_data_plane_authorization` exports — one emulator, one
//! control-plane call, one data-plane call — so both suites address the two planes the same
//! way.

use serde_json::json;

use crate::directory_data_plane_authorization::{
    create_microsoft_ad, Answer, Emulator, PAST_ONE_SPAN,
};

/// A directory administrator password the directory's own policy accepts: between 8 and 64
/// characters, drawn from at least three of Active Directory's four character classes.
const PASSWORD: &str = "Passw0rd!2345";

/// A password that same policy refuses: inside the modelled `UserPassword` length bound,
/// but drawn from one character class.
const WEAK_PASSWORD: &str = "aaaaaaaaaa";

/// The `AccessDeniedReason` the data plane refuses a directory whose DS Data feature is off
/// with, and the sentence that refusal states.
const DATA_DISABLED: &str = "DATA_DISABLED";
const DATA_ACCESS_NOT_ENABLED: &str =
    "DS Data feature is not enabled on this directory. This feature must first be enabled \
     by calling the EnableDirectoryDataAccess API.";

/// A Managed Microsoft AD directory of domain `name`, in service, with its DS Data feature
/// turned on — the two control-plane calls every row below depends on. `short_name` is the
/// NetBIOS name the create names, or `None` for a create that names none.
async fn directory_serving_its_data_plane(
    emulator: &Emulator,
    name: &str,
    short_name: Option<&str>,
) -> String {
    let mut body = create_microsoft_ad(name);
    if let Some(short_name) = short_name {
        body["ShortName"] = json!(short_name);
    }
    let created = emulator.control_plane("CreateMicrosoftAD", body).await;
    assert_eq!(
        created.status, 200,
        "CreateMicrosoftAD {name:?} must be accepted, got {} with {:?}",
        created.status, created.body
    );
    let directory_id = created.text("DirectoryId");

    // The switch needs a directory that is in service, and it is accepted rather than
    // completed inside the call, so each transition is waited out in turn.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let enabled = emulator
        .control_plane(
            "EnableDirectoryDataAccess",
            json!({ "DirectoryId": &directory_id }),
        )
        .await;
    assert_eq!(
        enabled.status, 200,
        "EnableDirectoryDataAccess on {name:?} must be accepted, got {} with {:?}",
        enabled.status, enabled.body
    );
    tokio::time::sleep(PAST_ONE_SPAN).await;
    directory_id
}

/// The `DC=` form of a domain name: `corp.example.com` -> `DC=corp,DC=example,DC=com`.
pub(super) fn domain_components(realm: &str) -> String {
    realm
        .trim_end_matches('.')
        .split('.')
        .map(|label| format!("DC={label}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Assert that `answer` is the refusal the data plane owes a directory whose DS Data
/// feature is off.
fn assert_data_disabled(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 403,
        "case {case:?}: expected 403, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().contains("AccessDeniedException"),
        "case {case:?}: expected AccessDeniedException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.text("Reason"),
        DATA_DISABLED,
        "case {case:?}: expected the DATA_DISABLED reason, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.text("Message"),
        DATA_ACCESS_NOT_ENABLED,
        "case {case:?}: expected the data-access refusal's own sentence, got {:?}",
        answer.body
    );
}

/// RULE: the data plane reports the directory's OWN domain as the realm of every object it
/// holds, and an account it created is inactive until the control plane's password reset
/// gives it a password.
#[tokio::test]
async fn the_data_plane_answers_from_the_control_planes_directory_record() {
    let emulator = Emulator::new();

    // Two directories, so every realm-valued answer is pinned to the directory it was
    // asked about rather than to one domain the emulator happens to hold: the rows differ
    // along the domain name the directory was created with (its label count and its
    // top-level domain), whether the create named a NetBIOS short name of its own, and the
    // account name created in it.
    let rows: [(&str, &str, Option<&str>, &str); 2] = [
        (
            "a three-label domain and a NetBIOS name of its own",
            "sim2real-one.example.com",
            Some("SIM2REAL"),
            "ada",
        ),
        (
            "a two-label domain whose create named no NetBIOS name",
            "sim2real-two.test",
            None,
            "grace.hopper",
        ),
    ];

    for (case, realm, short_name, user_name) in rows {
        let directory_id = directory_serving_its_data_plane(&emulator, realm, short_name).await;

        let created = emulator
            .data_plane(
                "/Users/CreateUser",
                &directory_id,
                json!({ "SAMAccountName": user_name }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be served, got {} with {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.text("DirectoryId"),
            directory_id,
            "case {case:?}: expected the create to name the directory it wrote to, got {:?}",
            created.body
        );

        // Every realm-valued member answers from the DIRECTORY's own domain, the object is
        // named inside the organizational unit that directory delegates to the account, and
        // the account has no password yet, so Active Directory leaves it inactive.
        let described = emulator
            .data_plane(
                "/Users/DescribeUser",
                &directory_id,
                json!({ "SAMAccountName": user_name }),
            )
            .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected the describe to be served, got {} with {:?}",
            described.status, described.body
        );
        // The unit is named after the directory's NetBIOS name: the one the create named,
        // or the default a create that names none gets — the first label of the domain
        // name, in upper case (`CreateMicrosoftADRequest$ShortName`).
        let delegated_ou = short_name.map_or_else(
            || {
                realm
                    .split('.')
                    .next()
                    .unwrap_or_default()
                    .to_uppercase()
            },
            str::to_string,
        );
        for (member, expected) in [
            ("Realm", json!(realm)),
            (
                "UserPrincipalName",
                json!(format!("{user_name}@{}", realm.to_uppercase())),
            ),
            (
                "DistinguishedName",
                json!(format!(
                    "CN={user_name},OU=Users,OU={delegated_ou},{}",
                    domain_components(realm)
                )),
            ),
            ("Enabled", json!(false)),
        ] {
            assert_eq!(
                described.body[member], expected,
                "case {case:?}: expected {member} to be {expected}, got {:?}",
                described.body
            );
        }

        let listed = emulator
            .data_plane("/Users/ListUsers", &directory_id, json!({}))
            .await;
        assert_eq!(
            listed.body["Realm"],
            json!(realm),
            "case {case:?}: expected every page to echo the directory's own realm, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.body["Users"][0]["Enabled"],
            json!(false),
            "case {case:?}: expected the listing to agree with the describe, got {:?}",
            listed.body
        );

        // A password the directory's policy refuses leaves the account exactly as it was:
        // the reset is the only thing that can activate it, so a refused reset cannot.
        let weak = emulator
            .control_plane(
                "ResetUserPassword",
                json!({
                    "DirectoryId": &directory_id,
                    "UserName": user_name,
                    "NewPassword": WEAK_PASSWORD
                }),
            )
            .await;
        assert!(
            weak.status == 400 && weak.error_code().contains("InvalidPasswordException"),
            "case {case:?}: expected a weak password to be refused, got {} with {:?}",
            weak.status,
            weak.body
        );

        // An account neither plane holds is not one a password can be set on.
        let absent = emulator
            .control_plane(
                "ResetUserPassword",
                json!({
                    "DirectoryId": &directory_id,
                    "UserName": "sim2real-no-such-user",
                    "NewPassword": PASSWORD
                }),
            )
            .await;
        assert!(
            absent.status == 400 && absent.error_code().contains("UserDoesNotExistException"),
            "case {case:?}: expected UserDoesNotExistException for an account nothing \
             created, got {} with {:?}",
            absent.status,
            absent.body
        );

        // The reset gives the account its password, which is what makes it active.
        let reset = emulator
            .control_plane(
                "ResetUserPassword",
                json!({
                    "DirectoryId": &directory_id,
                    "UserName": user_name,
                    "NewPassword": PASSWORD
                }),
            )
            .await;
        assert_eq!(
            reset.status, 200,
            "case {case:?}: expected the reset to be served, got {} with {:?}",
            reset.status, reset.body
        );
        let enabled = emulator
            .data_plane(
                "/Users/DescribeUser",
                &directory_id,
                json!({ "SAMAccountName": user_name }),
            )
            .await;
        assert_eq!(
            enabled.body["Enabled"],
            json!(true),
            "case {case:?}: expected the reset to activate the account, got {:?}",
            enabled.body
        );
        let relisted = emulator
            .data_plane("/Users/ListUsers", &directory_id, json!({}))
            .await;
        assert_eq!(
            relisted.body["Users"][0]["Enabled"],
            json!(true),
            "case {case:?}: expected the listing to agree with the describe, got {:?}",
            relisted.body
        );
    }
}

/// RULE: the realm and the account state above are answered only while the control plane's
/// DS Data feature is on for the directory — before the enable takes effect, and once a
/// withdrawal has, the same calls are refused with the feature's own refusal.
#[tokio::test]
async fn the_feature_the_realm_is_answered_under_is_the_control_planes_to_switch() {
    let emulator = Emulator::new();
    let realm = "sim2real-window.example.com";

    let created = emulator
        .control_plane("CreateMicrosoftAD", create_microsoft_ad(realm))
        .await;
    let directory_id = created.text("DirectoryId");
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // Nothing has enabled the feature for this directory, so every call against it is
    // refused — the writes as well as the reads, which is what makes the switch an
    // access-control boundary rather than a reporting quirk.
    for (route, body) in [
        ("/Users/CreateUser", json!({ "SAMAccountName": "ada" })),
        ("/Users/DescribeUser", json!({ "SAMAccountName": "ada" })),
        ("/Users/ListUsers", json!({})),
        ("/Groups/ListGroups", json!({})),
    ] {
        let answer = emulator.data_plane(route, &directory_id, body).await;
        assert_data_disabled(&format!("{route} before the feature is enabled"), &answer);
    }

    // The enable answers before the feature is in service, so the data plane is still
    // closed until the transition it owes falls due.
    let enabled = emulator
        .control_plane(
            "EnableDirectoryDataAccess",
            json!({ "DirectoryId": &directory_id }),
        )
        .await;
    assert_eq!(
        enabled.status, 200,
        "EnableDirectoryDataAccess must be accepted, got {} with {:?}",
        enabled.status, enabled.body
    );
    let while_enabling = emulator
        .data_plane("/Users/ListUsers", &directory_id, json!({}))
        .await;
    assert_data_disabled("while the enable is under way", &while_enabling);
    tokio::time::sleep(PAST_ONE_SPAN).await;

    let served = emulator
        .data_plane(
            "/Users/CreateUser",
            &directory_id,
            json!({ "SAMAccountName": "ada" }),
        )
        .await;
    assert_eq!(
        served.status, 200,
        "expected the create to be served once the feature is in service, got {} with {:?}",
        served.status, served.body
    );

    // And the control plane's switch closes the data plane again, on its own transition.
    let withdrawn = emulator
        .control_plane(
            "DisableDirectoryDataAccess",
            json!({ "DirectoryId": &directory_id }),
        )
        .await;
    assert_eq!(
        withdrawn.status, 200,
        "DisableDirectoryDataAccess must be accepted, got {} with {:?}",
        withdrawn.status, withdrawn.body
    );
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let after = emulator
        .data_plane(
            "/Users/DescribeUser",
            &directory_id,
            json!({ "SAMAccountName": "ada" }),
        )
        .await;
    assert_data_disabled("once the feature is withdrawn", &after);
}
