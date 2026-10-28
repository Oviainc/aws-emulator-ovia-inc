// SPDX-License-Identifier: CC0-1.0

//! Which `NoSuchEntity` a login-profile call answers: the user, or the password.
//!
//! The rule under test: a login profile exists only as the password of a user the account
//! holds, so a login-profile call has TWO ways of finding nothing — and AWS tells them
//! apart by the message, at the same HTTP 404 and the same error code `NoSuchEntity`. A
//! name the account holds no user under is reported as that user not being found (`The
//! user with name <UserName> cannot be found.`); a user the account DOES hold, carrying no
//! password, is reported as that user having no login profile (`Login Profile for User
//! <UserName> cannot be found.`). The fixed text is each sentence template; the variable
//! part is the `UserName` the REQUEST carried, which is echoed as the request spelled it.
//! So a caller can tell "create the user" from "create only its password".
//!
//! The suites are TABLES whose rows differ along what the rule generalises over: which
//! login-profile operation is called (the read, the update, the delete and the create —
//! every one of them has to look the user up before the password), which of the two
//! conditions holds, the alphabet and case the name is stored in, the case the request
//! spells it in, and the path the user lives under. Beside them, the lifecycle: the two
//! conditions stay distinct as a password is created and deleted under one unchanged user.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, so a request is
//! built and read exactly one way across this crate.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, error_code, router};

/// The password every row that needs one sends. `passwordType` is length 1..128, so this
/// is inside the published range; no row is about the password's value.
const PASSWORD: &str = "Correct-horse-battery-staple-1";

/// The operations that have to answer the USER-not-found message for a name the account
/// holds no user under: every login-profile operation, the create included — each of them
/// addresses the profile of a user, so the user is what is missing first.
const OPERATIONS_FOR_AN_ABSENT_USER: &[&str] = &[
    "GetLoginProfile",
    "CreateLoginProfile",
    "UpdateLoginProfile",
    "DeleteLoginProfile",
];

/// The operations that have to answer the PROFILE-not-found message for a held user with
/// no password. `CreateLoginProfile` is not one of them: creating the first password is
/// what it does, so for this condition it succeeds rather than refusing.
const OPERATIONS_FOR_A_USER_WITHOUT_A_PASSWORD: &[&str] =
    &["GetLoginProfile", "UpdateLoginProfile", "DeleteLoginProfile"];

/// One row: the user the account holds (if any), the path it lives under, and the name the
/// request spells.
struct Case {
    case: &'static str,
    /// The name the user is created with, or `None` for a name the account holds no user
    /// under.
    stored: Option<&'static str>,
    /// The path the user is created at. IAM entity names are unique account-wide whatever
    /// path they live under, so the path must not change either wording.
    path: &'static str,
    /// The name the request carries — the same entity as `stored` where there is one,
    /// differing only in case, because IAM entity names "are not distinguished by case".
    requested: &'static str,
}

/// Rows spanning both conditions, the `^[\w+=,.@-]+$` alphabet the name constraint
/// admits, requests that fold the stored name up, down and not at all, and a user under a
/// path.
const CASES: &[Case] = &[
    Case {
        case: "a name the account holds no user under",
        stored: None,
        path: "/",
        requested: "nosuchuser",
    },
    Case {
        case: "an absent name with punctuation and mixed case",
        stored: None,
        path: "/",
        requested: "No.Such+User=2",
    },
    Case {
        case: "an absent name of one character",
        stored: None,
        path: "/",
        requested: "z",
    },
    Case {
        case: "a held user with no password",
        stored: Some("present"),
        path: "/",
        requested: "present",
    },
    Case {
        case: "a held user named in another case",
        stored: Some("Present2"),
        path: "/",
        requested: "pRESENT2",
    },
    Case {
        case: "a held user under a path",
        stored: Some("pathed_user@4"),
        path: "/eng/tools/",
        requested: "PATHED_USER@4",
    },
];

/// RULE: both conditions are HTTP 404 `NoSuchEntity`, and the message says WHICH of them
/// it is, naming the user as the request spelled it — on every login-profile operation.
#[tokio::test]
async fn a_login_profile_refusal_says_whether_the_user_or_the_password_is_missing() {
    for case in CASES {
        // One router per row, so the only user the account holds is the row's own.
        let router = router();
        if let Some(stored) = case.stored {
            let (status, body) = call(
                &router,
                "CreateUser",
                &[("UserName", stored), ("Path", case.path)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: the fixture CreateUser must succeed, got {status}: {body}",
                case.case
            );
        }

        let (operations, expected) = match case.stored {
            None => (
                OPERATIONS_FOR_AN_ABSENT_USER,
                format!("The user with name {} cannot be found.", case.requested),
            ),
            Some(_) => (
                OPERATIONS_FOR_A_USER_WITHOUT_A_PASSWORD,
                format!(
                    "Login Profile for User {} cannot be found.",
                    case.requested
                ),
            ),
        };

        for operation in operations {
            let (status, body) = login_profile_call(&router, operation, case.requested).await;
            assert_eq!(
                status, 404,
                "case {:?}/{operation}: expected HTTP 404, got {status}: {body}",
                case.case
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {:?}/{operation}: expected NoSuchEntity, got {:?}",
                case.case,
                error_code(&body)
            );
            assert_eq!(
                element(&body, "Message"),
                expected,
                "case {:?}/{operation}: expected the message {expected:?}, got {:?}",
                case.case,
                element(&body, "Message")
            );
        }
    }
}

/// RULE over a lifecycle: the two conditions are facts of the store at the instant of the
/// call, so one unchanged user moves between them as its password is created and deleted —
/// and a name that never existed is never reported as a missing password.
#[tokio::test]
async fn the_two_conditions_follow_the_store_as_the_password_comes_and_goes() {
    let router = router();
    let user_name = "lifecycle";
    let (status, body) = call(&router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "the fixture CreateUser must succeed, got {status}: {body}"
    );

    // Before any password: the user is held, so the refusal is about the PROFILE.
    let (status, body) = login_profile_call(&router, "GetLoginProfile", user_name).await;
    assert_eq!(
        status, 404,
        "case 'before the password': expected HTTP 404, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "Message"),
        format!("Login Profile for User {user_name} cannot be found."),
        "case 'before the password': expected the profile wording, got {body}"
    );

    // With a password: the read is served, so neither refusal is in play.
    let (status, body) = call(
        &router,
        "CreateLoginProfile",
        &[("UserName", user_name), ("Password", PASSWORD)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case 'the create': CreateLoginProfile must succeed, got {status}: {body}"
    );
    let (status, body) = login_profile_call(&router, "GetLoginProfile", user_name).await;
    assert_eq!(
        status, 200,
        "case 'with the password': expected the read to be served, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "UserName"),
        user_name,
        "case 'with the password': expected the profile of {user_name:?}, got {body}"
    );

    // The password deleted, the user untouched: the refusal is about the PROFILE again,
    // never about the user.
    let (status, body) = login_profile_call(&router, "DeleteLoginProfile", user_name).await;
    assert_eq!(
        status, 200,
        "case 'the delete': DeleteLoginProfile must succeed, got {status}: {body}"
    );
    let (status, body) = login_profile_call(&router, "GetLoginProfile", user_name).await;
    assert_eq!(
        status, 404,
        "case 'after the delete': expected HTTP 404, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "Message"),
        format!("Login Profile for User {user_name} cannot be found."),
        "case 'after the delete': expected the profile wording, got {body}"
    );

    // And the user itself deleted: now the same request is about the USER.
    let (status, body) = call(&router, "DeleteUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case 'the user delete': DeleteUser must succeed, got {status}: {body}"
    );
    let (status, body) = login_profile_call(&router, "GetLoginProfile", user_name).await;
    assert_eq!(
        status, 404,
        "case 'after the user delete': expected HTTP 404, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "Message"),
        format!("The user with name {user_name} cannot be found."),
        "case 'after the user delete': expected the user wording, got {body}"
    );
}

/// One login-profile call naming `user_name`, with the parameters that operation needs:
/// `CreateLoginProfile` carries the password it would store, the others name the user
/// alone.
async fn login_profile_call(
    router: &MultiServiceRouter,
    operation: &str,
    user_name: &str,
) -> (u16, String) {
    let mut params: Vec<(&str, &str)> = vec![("UserName", user_name)];
    if operation == "CreateLoginProfile" {
        params.push(("Password", PASSWORD));
    }
    call(router, operation, &params).await
}
