// SPDX-License-Identifier: CC0-1.0

//! Resetting an account's password — `ResetUserPassword` — driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.ResetUserPassword` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the directory-lifecycle suite's,
//! reached as `crate::directory_lifecycle::<item>`). Nothing a RESPONSE claims is read from
//! the state struct, so an answer that is not computed from persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * a reset resolves the account in the directory the request names, so the administrator
//!   account the create made can be reset — under every spelling an Active Directory
//!   account name is compared under — while a name the directory holds no account under is
//!   refused with `UserDoesNotExistException`;
//! * the administrator account is the one its create's `Password` was the password of, and
//!   each directory type names it the way its own create documents, so one type's name is
//!   not an account of the other;
//! * a password is set in the directory's own domain, so the reset needs a directory that
//!   is in service: one still being built by the transition its create owes is refused with
//!   `DirectoryUnavailableException`, and the same call succeeds once that transition has
//!   fallen due;
//! * the new password is checked against the directory's own password policy — between 8
//!   and 64 characters, drawn from at least three of Active Directory's four character
//!   classes — and one that does not meet it is refused with `InvalidPasswordException`,
//!   decided before the account is resolved, so a refused password reports nothing about
//!   which accounts the directory holds;
//! * an id no directory is stored under is refused with `EntityDoesNotExistException`, the
//!   only not-found class this operation models for the directory itself.

use serde_json::{json, Value};

use crate::directory_lifecycle::{
    call, create_directory, minimal_create, ok, refused, service, PAST_ONE_SPAN,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The administrator account a Simple AD directory is created with, from
/// `CreateDirectoryRequest$Password`'s own documentation: "The directory creation process
/// creates a directory administrator account with the user name `Administrator`".
const SIMPLE_AD_ADMINISTRATOR: &str = "Administrator";

/// The administrator account a Managed Microsoft AD directory is created with, from
/// `CreateMicrosoftADRequest$Password`'s own documentation: "The password for the default
/// administrative user named `Admin`".
const MICROSOFT_AD_ADMINISTRATOR: &str = "Admin";

/// A new password that meets the directory's policy: more than eight characters drawn from
/// all four of Active Directory's character classes.
const GOOD_PASSWORD: &str = "Sim2Real-Reset-Pw1!";

/// Well-formed `DirectoryId`s no directory is ever created under, as `(id, class)`: the
/// shape `^d-[0-9a-f]{10}$` the model declares, with different digits.
const ABSENT_DIRECTORY_IDS: [(&str, &str); 3] = [
    ("d-0000000000", "all zeros"),
    ("d-ffffffffff", "all f"),
    ("d-abcdef0123", "every hex digit once"),
];

/// A `ResetUserPassword` body.
fn reset(directory_id: &str, user_name: &str, new_password: &str) -> Value {
    json!({
        "DirectoryId": directory_id,
        "UserName": user_name,
        "NewPassword": new_password,
    })
}

/// RULE: the account a reset names is resolved in the directory the request names — the
/// administrator account its create made, under any spelling — and a name the directory
/// holds no account under is refused.
#[tokio::test]
async fn a_reset_resolves_the_account_in_the_directory_it_names() {
    let router = service();

    let simple = create_directory(
        &router,
        minimal_create("simple.example.com"),
        "the Simple AD directory",
    )
    .await;
    let managed = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("managed.example.com"),
        "the Managed Microsoft AD directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // (the directory, the account its create made, the account name the OTHER type uses).
    let rows = [
        (&simple, SIMPLE_AD_ADMINISTRATOR, MICROSOFT_AD_ADMINISTRATOR),
        (&managed, MICROSOFT_AD_ADMINISTRATOR, SIMPLE_AD_ADMINISTRATOR),
    ];

    for (directory, administrator, other_types_name) in rows {
        // An Active Directory account name is compared case-insensitively, so every
        // spelling of the administrator's name resolves the same account — and a reset is
        // repeatable, because the account is not consumed by being reset.
        for spelling in [
            administrator.to_string(),
            administrator.to_lowercase(),
            administrator.to_uppercase(),
        ] {
            let case = format!("{directory}: reset {spelling:?}");
            let answer = ok(
                &router,
                "ResetUserPassword",
                reset(directory, &spelling, GOOD_PASSWORD),
                &case,
            )
            .await;
            assert_eq!(
                answer,
                json!({}),
                "case {case:?}: expected the modelled empty answer, got {answer}"
            );
        }

        // Names the directory holds no account under: the other type's administrator
        // name, and names the modelled `CustomerUserName` pattern admits.
        for (unknown, class) in [
            (other_types_name, "the other directory type's administrator"),
            ("someone", "a short name"),
            ("admin2", "a name one character off the administrator's"),
            (
                "a-very-long-service-account-name",
                "a long name with separators",
            ),
        ] {
            let case = format!("{directory}: reset {unknown:?} ({class})");
            refused(
                &router,
                "ResetUserPassword",
                reset(directory, unknown, GOOD_PASSWORD),
                "UserDoesNotExistException",
                &case,
            )
            .await;
        }
    }
}

/// RULE: a reset needs a directory that is in service, and the new password has to meet
/// the directory's own policy.
#[tokio::test]
async fn a_reset_needs_a_serving_directory_and_a_password_that_meets_the_policy() {
    let router = service();

    // Created and reset IMMEDIATELY: the directory is still being built, so the reset is
    // refused — and the same call succeeds once the transition the create owes falls due,
    // with nobody having written anything in between.
    let managed = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("building.example.com"),
        "the directory being built",
    )
    .await;
    refused(
        &router,
        "ResetUserPassword",
        reset(&managed, MICROSOFT_AD_ADMINISTRATOR, GOOD_PASSWORD),
        "DirectoryUnavailableException",
        "a reset while the directory is still being built",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "ResetUserPassword",
        reset(&managed, MICROSOFT_AD_ADMINISTRATOR, GOOD_PASSWORD),
        "a reset once the directory is in service",
    )
    .await;

    // The password policy, over a table spanning the rule: both ends of the length bound
    // and each combination of character classes. Every row is a value the modelled
    // `UserPassword` (length 1..127, no pattern) admits, so the handler is what decides
    // it — including the rows between the directory's own maximum and that modelled one.
    let sixty_four = format!("Sim2Real1!{:a<54}", "");
    let sixty_five = format!("Sim2Real1!{:a<55}", "");
    // The longest value the modelled `UserPassword` (length 1..127) admits, so the
    // boundary validator passes it and the directory's own policy is what refuses it.
    let modelled_maximum = format!("Sim2Real1!{:a<117}", "");
    let rows: [(&str, &str, bool); 12] = [
        ("one character", "a", false),
        ("seven characters of three classes", "aB3!xY7", false),
        ("eight characters of one class", "aaaaaaaa", false),
        ("eight characters of two classes", "aaaaaaa1", false),
        ("eight characters of three classes", "aaaaaA1a", true),
        ("digits, upper and special", "AAAA1111!", true),
        ("lower, special and digits", "aaaa1111!", true),
        ("all four classes", GOOD_PASSWORD, true),
        (
            "a passphrase of four classes",
            "correct-horse-battery-Staple1",
            true,
        ),
        ("exactly the documented maximum", sixty_four.as_str(), true),
        ("one character past it", sixty_five.as_str(), false),
        (
            "the longest the model admits",
            modelled_maximum.as_str(),
            false,
        ),
    ];
    for (class, password, acceptable) in rows {
        let case = format!("a new password: {class}");
        if acceptable {
            ok(
                &router,
                "ResetUserPassword",
                reset(&managed, MICROSOFT_AD_ADMINISTRATOR, password),
                &case,
            )
            .await;
        } else {
            refused(
                &router,
                "ResetUserPassword",
                reset(&managed, MICROSOFT_AD_ADMINISTRATOR, password),
                "InvalidPasswordException",
                &case,
            )
            .await;
            // The policy is decided BEFORE the account is resolved, so a password that
            // does not meet it reports the password rather than whether the account
            // exists.
            let case = format!("{case}, for an account that does not exist");
            refused(
                &router,
                "ResetUserPassword",
                reset(&managed, "someone-who-does-not-exist", password),
                "InvalidPasswordException",
                &case,
            )
            .await;
        }
    }
}

/// RULE: an id no directory is stored under is refused with the one not-found class this
/// operation models for the directory itself.
#[tokio::test]
async fn an_id_naming_no_directory_is_refused() {
    let router = service();

    // One stored directory in the same store, so a refusal cannot come from an empty one.
    let managed = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("stored.example.com"),
        "the stored directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (absent, class) in ABSENT_DIRECTORY_IDS {
        let case = format!("a reset in {absent} ({class})");
        let (status, parsed) = call(
            &router,
            "ResetUserPassword",
            reset(absent, MICROSOFT_AD_ADMINISTRATOR, GOOD_PASSWORD),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("EntityDoesNotExistException"),
            "case {case:?}: expected EntityDoesNotExistException, got {parsed}"
        );
        assert!(
            parsed["Message"]
                .as_str()
                .unwrap_or_default()
                .contains(absent),
            "case {case:?}: expected the refusal to name the requested id, got {parsed}"
        );
    }

    // The refusals changed nothing: the stored directory's own account still resets.
    ok(
        &router,
        "ResetUserPassword",
        reset(&managed, MICROSOFT_AD_ADMINISTRATOR, GOOD_PASSWORD),
        "after the refusals",
    )
    .await;
}
