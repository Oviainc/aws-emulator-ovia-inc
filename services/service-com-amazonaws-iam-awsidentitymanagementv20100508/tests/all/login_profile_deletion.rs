// SPDX-License-Identifier: CC0-1.0

//! Deleting an IAM login profile (a console password), end to end over the awsQuery
//! wire.
//!
//! `DeleteLoginProfile` removes the password a user holds, so the profile
//! `GetLoginProfile` used to report is gone and a fresh `CreateLoginProfile` is
//! accepted again. The awsQuery harness (router, call, element) is the one
//! `tests/all/user_lifecycle.rs` owns, so every suite in this crate builds and reads
//! a request the same way.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! owning user's name alphabet and case, the case the request spells that name in,
//! the password's length and alphabet (including the modelled bounds), whether the
//! request asks for a reset on next sign-in at all, whether it names a user at all,
//! and which credentials it is presented as signed by.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code, router};

/// One login profile to delete: the user the account holds, the case a request
/// spells that user's name in, the password the profile was created with, and what
/// the create asked for `PasswordResetRequired`.
///
/// The names differ on most rows because IAM entity names "are not distinguished by
/// case" (`CreateUser$UserName`), so a delete may name the user in any case.
struct Case {
    /// The name the user is created with — the case IAM stores and reports.
    stored_name: &'static str,
    /// The name the delete spells; the same entity, differing only in case.
    requested_name: &'static str,
    /// The console password the profile is created with. Owned, so a row can span a
    /// length bound rather than only short literals.
    password: String,
    /// `PasswordResetRequired` as the create sends it, or `None` when it omits the
    /// parameter entirely — the modelled default is `false`
    /// (`smithy_query(query="operation", operation="CreateLoginProfile")`).
    reset_required: Option<bool>,
}

/// Rows spanning the `^[\w+=,.@-]+$` name alphabet `userNameType` admits, both cases
/// of the stored name, and the `passwordType` class the model admits: length 1..128
/// and the alphabet `^[\u0009\u000A\u000D\u0020-\u00FF]+$`
/// (`smithy_query(query="operation", operation="CreateLoginProfile")`). The rows also
/// span a profile created asking for a reset, one refusing it, and one that omitted
/// the parameter, because none of that changes what a delete does.
fn cases() -> Vec<Case> {
    vec![
        Case {
            stored_name: "ivan",
            requested_name: "ivan",
            password: "S3cret-ivan".to_string(),
            reset_required: Some(false),
        },
        Case {
            stored_name: "Judy",
            requested_name: "judy",
            password: "S3cret-judy".to_string(),
            reset_required: Some(true),
        },
        Case {
            stored_name: "karl_7",
            requested_name: "KARL_7",
            password: "S3cret-karl".to_string(),
            // The parameter is omitted: the modelled default is false.
            reset_required: None,
        },
        Case {
            stored_name: "leia.x",
            requested_name: "leia.X",
            // The whitespace the alphabet admits: tab, line feed, carriage return.
            password: "S3cret\u{0009}leia\u{000A}x\u{000D}!".to_string(),
            reset_required: Some(true),
        },
        Case {
            stored_name: "Mallory+1",
            requested_name: "mallory+1",
            // The top of the admitted alphabet: characters up to \u00FF.
            password: "S3cret-\u{00E9}\u{00FF}\u{00A1}".to_string(),
            reset_required: Some(false),
        },
        Case {
            stored_name: "niaj=2",
            requested_name: "niaj=2",
            // The published minimum length, 1.
            password: "~".to_string(),
            reset_required: Some(true),
        },
        Case {
            stored_name: "olivia@3",
            requested_name: "OLIVIA@3",
            // The published maximum length, 128.
            password: "O".repeat(128),
            reset_required: Some(false),
        },
        Case {
            stored_name: "peggy-4",
            requested_name: "peggy-4",
            // Characters that also exercise the response serializer's XML escaping.
            password: "S3cret \"&<>'\u{0020}peggy".to_string(),
            reset_required: Some(true),
        },
    ]
}

/// A readable name for a row's assertion messages.
fn case_name(case: &Case) -> String {
    format!("{} requested as {}", case.stored_name, case.requested_name)
}

/// The awsQuery parameters of one create, built from a row.
fn create_params<'a>(case: &'a Case, name: &'a str) -> Vec<(&'a str, &'a str)> {
    let mut params: Vec<(&str, &str)> = vec![("UserName", name), ("Password", &case.password)];
    if let Some(reset) = case.reset_required {
        params.push((
            "PasswordResetRequired",
            if reset { "true" } else { "false" },
        ));
    }
    params
}

/// Create the row's user and its login profile, answering the create date the
/// profile was minted with.
async fn with_login_profile(router: &MultiServiceRouter, case: &Case) -> String {
    let name = case_name(case);
    let (status, body) = call(router, "CreateUser", &[("UserName", case.stored_name)]).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateUser must succeed, got {status}: {body}"
    );
    let (status, created) = call(router, "CreateLoginProfile", &create_params(case, case.stored_name)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateLoginProfile must succeed, got {status}: {created}"
    );
    element(&created, "CreateDate")
}

/// RULE: `DeleteLoginProfile` removes the console password the named user holds, so
/// the profile a read used to report is gone — the read answers `NoSuchEntity` — and
/// a fresh `CreateLoginProfile` for that user is accepted again, whatever case the
/// delete spelled the name in.
#[tokio::test]
async fn deleting_a_password_removes_the_profile_the_read_reported() {
    let router = router();

    for case in cases() {
        let name = case_name(&case);
        with_login_profile(&router, &case).await;

        let (status, read) = call(
            &router,
            "GetLoginProfile",
            &[("UserName", case.requested_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the profile must be readable before the delete, got {status}: {read}"
        );

        let (status, deleted) = call(
            &router,
            "DeleteLoginProfile",
            &[("UserName", case.requested_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from the delete, got {status}: {deleted}"
        );
        // The operation's output shape is `Unit`, so the response reports no profile
        // members at all — and never the password, which no response carries.
        assert!(
            elements(&deleted, "LoginProfile").is_empty()
                && elements(&deleted, "CreateDate").is_empty(),
            "case {name:?}: expected the delete to report nothing, got {deleted}"
        );
        assert!(
            !deleted.contains(&case.password),
            "case {name:?}: expected the password never to appear in a response, got {deleted}"
        );

        // The password is GONE from the state the next caller reads.
        let (status, gone) = call(
            &router,
            "GetLoginProfile",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected the deleted profile to be unreadable, got {status}: {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {:?}",
            error_code(&gone)
        );

        // A user has at most one login profile, so with the old one deleted a create
        // is accepted again — and the profile it mints is its own, not the one the
        // delete removed.
        let replacement = Case {
            stored_name: case.stored_name,
            requested_name: case.requested_name,
            password: "Replacement-S3cret~9".to_string(),
            reset_required: Some(!matches!(case.reset_required, Some(true))),
        };
        let (status, recreated) = call(
            &router,
            "CreateLoginProfile",
            &create_params(&replacement, replacement.requested_name),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected a create after the delete to succeed, got {status}: {recreated}"
        );
        assert_eq!(
            element(&recreated, "UserName"),
            case.stored_name,
            "case {name:?}: expected the stored user name, got {:?}",
            element(&recreated, "UserName")
        );
        assert_eq!(
            element(&recreated, "PasswordResetRequired"),
            if matches!(replacement.reset_required, Some(true)) {
                "true"
            } else {
                "false"
            },
            "case {name:?}: expected the new profile to carry the new reset flag, got {:?}",
            element(&recreated, "PasswordResetRequired")
        );
        let (status, reread) = call(
            &router,
            "GetLoginProfile",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            (status, element(&reread, "CreateDate")),
            (200, element(&recreated, "CreateDate")),
            "case {name:?}: expected the recreated profile to be the one held, got {status}: {reread}"
        );
        assert!(
            !reread.contains(&case.password) && !reread.contains(&replacement.password),
            "case {name:?}: expected no password in a response, got {reread}"
        );
    }
}

/// RULE: a delete removes a password that is there, so a second delete of the same
/// password — like a delete for a user that never had one — finds nothing to remove
/// and is refused with `NoSuchEntity`.
#[tokio::test]
async fn a_password_that_is_not_there_cannot_be_deleted() {
    let router = router();

    for case in cases() {
        let name = case_name(&case);
        with_login_profile(&router, &case).await;

        let (status, deleted) = call(
            &router,
            "DeleteLoginProfile",
            &[("UserName", case.requested_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the first delete must succeed, got {status}: {deleted}"
        );

        // Rows spanning the second delete spelling the name in each case: the stored
        // one and the one the first delete used.
        for spelled in [case.stored_name, case.requested_name] {
            let (status, refused) =
                call(&router, "DeleteLoginProfile", &[("UserName", spelled)]).await;
            assert_eq!(
                status, 404,
                "case {name:?} spelled {spelled:?}: expected 404 from the second delete, got {status}: {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "NoSuchEntity",
                "case {name:?} spelled {spelled:?}: expected NoSuchEntity, got {:?}",
                error_code(&refused)
            );
        }

        // The user itself survived every one of those refusals.
        let (status, user) = call(&router, "GetUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            (status, element(&user, "UserName")),
            (200, case.stored_name.to_string()),
            "case {name:?}: expected the user to survive, got {status}: {user}"
        );
    }
}

/// RULE: a login profile exists only as the password of a user the account holds, so
/// a delete naming an absent user — or a user the account holds who has no password
/// — is not found and removes nothing from any other user.
#[tokio::test]
async fn deleting_a_password_of_a_user_the_account_does_not_hold_removes_nothing() {
    let router = router();

    // One held user WITH a password, which no refusal below may touch, and one held
    // user without one.
    let held = Case {
        stored_name: "bystander",
        requested_name: "bystander",
        password: "S3cret-bystander".to_string(),
        reset_required: Some(true),
    };
    let held_create_date = with_login_profile(&router, &held).await;
    let (status, body) = call(&router, "CreateUser", &[("UserName", "nopassword")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");

    // Rows spanning a plain absent name, one differing from a held name by more than
    // case, names drawn from the rest of the accepted alphabet, a name at the
    // 64-character bound `userNameType` admits, and a held user with no password.
    let names: Vec<String> = vec![
        "absent".to_string(),
        "bystander2".to_string(),
        "no.such@user".to_string(),
        "NOT_HELD+1".to_string(),
        "x".repeat(64),
        "nopassword".to_string(),
    ];

    for target in &names {
        let (status, refused) =
            call(&router, "DeleteLoginProfile", &[("UserName", target)]).await;
        assert_eq!(
            status, 404,
            "case {target:?}: expected 404, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "NoSuchEntity",
            "case {target:?}: expected NoSuchEntity, got {:?}",
            error_code(&refused)
        );

        // The refusal removed nothing: the one profile the account holds is still
        // exactly the one the create minted.
        let (status, survivor) = call(
            &router,
            "GetLoginProfile",
            &[("UserName", held.stored_name)],
        )
        .await;
        assert_eq!(
            (status, element(&survivor, "CreateDate")),
            (200, held_create_date.clone()),
            "case {target:?}: expected the bystander's profile untouched, got {status}: {survivor}"
        );
    }
}

/// RULE: `DeleteLoginProfile` with no `UserName` deletes the password of the IAM user
/// the request was signed by, resolved from the signing access key id, and leaves
/// every other user's password where it is.
#[tokio::test]
async fn an_omitted_name_deletes_the_signing_users_password() {
    let router = router();

    for case in cases() {
        let name = case_name(&case);
        with_login_profile(&router, &case).await;

        // A second user with a password, so the nameless delete has to pick the
        // signing one rather than any profile at all.
        let other = format!("other-{}", case.stored_name.to_lowercase());
        let other = other.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-");
        let (status, body) = call(&router, "CreateUser", &[("UserName", &other)]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the bystander must be created, got {status}: {body}"
        );
        let (status, created) = call(
            &router,
            "CreateLoginProfile",
            &[("UserName", &other), ("Password", "S3cret-other")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the bystander's password must be created, got {status}: {created}"
        );
        let other_create_date = element(&created, "CreateDate");

        let (status, key) = call(
            &router,
            "CreateAccessKey",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the signing key must be minted, got {status}: {key}"
        );
        let signing_key = element(&key, "AccessKeyId");

        let (status, deleted) =
            call_signed_by(&router, Some(&signing_key), "DeleteLoginProfile", &[]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 for the signing user's own password, got {status}: {deleted}"
        );

        let (status, gone) =
            call_signed_by(&router, Some(&signing_key), "GetLoginProfile", &[]).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected the signing user's profile to be gone, got {status}: {gone}"
        );

        // "Deleting a user's password does not prevent a user from accessing Amazon
        // Web Services through the command line interface or the API": the signing
        // key is still the user's.
        let (status, keys) = call(
            &router,
            "ListAccessKeys",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the key listing to succeed, got {status}: {keys}"
        );
        assert!(
            elements(&keys, "AccessKeyId").contains(&signing_key),
            "case {name:?}: expected the access key to survive the delete, got {keys}"
        );

        // The bystander kept its password.
        let (status, survivor) = call(&router, "GetLoginProfile", &[("UserName", &other)]).await;
        assert_eq!(
            (status, element(&survivor, "CreateDate")),
            (200, other_create_date),
            "case {name:?}: expected the bystander's profile untouched, got {status}: {survivor}"
        );
    }
}

/// RULE: credentials that belong to no IAM user resolve to no user, so a delete that
/// names none is refused rather than having an identity invented for it, and no
/// password is removed.
#[tokio::test]
async fn an_omitted_name_refuses_non_user_credentials() {
    let router = router();
    let held = Case {
        stored_name: "keyless",
        requested_name: "keyless",
        password: "S3cret-keyless".to_string(),
        reset_required: Some(false),
    };
    let held_create_date = with_login_profile(&router, &held).await;

    // Rows spanning no signing key at all, the fallback id an unsigned request
    // resolves to (`engine::extract_akid` answers "default"), a key id of the
    // modelled shape this emulator never issued, and an id that names a user rather
    // than a key.
    let signers: &[Option<&str>] = &[
        None,
        Some("default"),
        Some("AKIAIOSFODNN7EXAMPLE"),
        Some("EMULATORAKAAAAAAAAAA"),
        Some("keyless"),
    ];

    for signer in signers {
        let case = format!("signed by {signer:?}");
        let (status, refused) =
            call_signed_by(&router, *signer, "DeleteLoginProfile", &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {refused}"
        );
        // The behaviour under test is the REFUSAL: a 400 that removes nothing. The
        // code is the framework's single client-input refusal shape, because the
        // operation models no error for a request that named no entity.
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected the framework's client-input refusal code, got {:?}",
            error_code(&refused)
        );

        let (status, survivor) = call(
            &router,
            "GetLoginProfile",
            &[("UserName", held.stored_name)],
        )
        .await;
        assert_eq!(
            (status, element(&survivor, "CreateDate")),
            (200, held_create_date.clone()),
            "case {case:?}: expected the held profile untouched, got {status}: {survivor}"
        );
    }
}

/// RULE: a login profile is a dependent of its user, so once the password is deleted
/// it no longer blocks the user's own delete — the conflict a held password raises
/// is gone with the password.
#[tokio::test]
async fn a_user_deletes_once_its_password_is_gone() {
    let router = router();

    for case in cases() {
        let name = case_name(&case);
        with_login_profile(&router, &case).await;

        let (status, refused) = call(&router, "DeleteUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 409,
            "case {name:?}: expected a held password to block the user delete, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "DeleteConflict",
            "case {name:?}: expected DeleteConflict, got {:?}",
            error_code(&refused)
        );

        let (status, deleted) = call(
            &router,
            "DeleteLoginProfile",
            &[("UserName", case.requested_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the password delete must succeed, got {status}: {deleted}"
        );

        let (status, removed) = call(&router, "DeleteUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the user to delete once its password was gone, got {status}: {removed}"
        );
        let (status, gone) = call(&router, "GetUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected the user to be gone, got {status}: {gone}"
        );
    }
}
