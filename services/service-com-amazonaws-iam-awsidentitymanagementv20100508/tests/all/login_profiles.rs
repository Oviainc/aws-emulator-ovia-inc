// SPDX-License-Identifier: CC0-1.0

//! IAM login profiles (console passwords), end to end over the awsQuery wire.
//!
//! `CreateLoginProfile` creates a password for an IAM user and persists the login
//! profile that `GetLoginProfile` reads back; the password itself is never part of
//! any response. The awsQuery harness (router, call, element) is the one
//! `tests/all/user_lifecycle.rs` owns, so every suite in this crate builds and
//! reads a request the same way.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! owning user's name alphabet and case, the case the request spells that name in,
//! the password's length and alphabet (including the modelled bounds), whether the
//! request asks for a reset on next sign-in at all, and which credentials the
//! request is presented as signed by.

use crate::user_lifecycle::{call, call_signed_by, element, elements, error_code, router};

/// One create: the user the account holds, the case the request spells that user's
/// name in, the password the request carries, and what it asks for
/// `PasswordResetRequired`.
///
/// The names differ on most rows because IAM entity names "are not distinguished by
/// case" (`CreateUser$UserName`), so a request may name the user in any case while
/// the response must report the name as the account holds it.
struct Case {
    /// The name the user is created with — the case IAM stores and reports.
    stored_name: &'static str,
    /// The name `CreateLoginProfile` spells; the same entity, differing only in
    /// case.
    requested_name: &'static str,
    /// The console password the create carries. Owned, so a row can span a length
    /// bound rather than only short literals.
    password: String,
    /// `PasswordResetRequired` as the request sends it, or `None` when the request
    /// omits the parameter entirely — the modelled default is `false`
    /// (`smithy_query(query="operation", operation="CreateLoginProfile")`).
    reset_required: Option<bool>,
}

/// Rows spanning the `^[\w+=,.@-]+$` name alphabet, both cases of the stored name,
/// and the `passwordType` class the model admits: length 1..128 and the alphabet
/// `^[\u0009\u000A\u000D\u0020-\u00FF]+$` — tab, line feed, carriage return and
/// every printable character from the space through `\u00FF`
/// (`smithy_query(query="operation", operation="CreateLoginProfile")`). The rows
/// also span a request that asks for a reset, one that refuses it, and one that
/// omits the parameter.
fn cases() -> Vec<Case> {
    vec![
        Case {
            stored_name: "alice",
            requested_name: "alice",
            password: "S3cret-alice".to_string(),
            reset_required: Some(false),
        },
        Case {
            stored_name: "Bob",
            requested_name: "bob",
            password: "S3cret-bob".to_string(),
            reset_required: Some(true),
        },
        Case {
            stored_name: "carol_7",
            requested_name: "CAROL_7",
            password: "S3cret-carol".to_string(),
            // The parameter is omitted: the modelled default is false.
            reset_required: None,
        },
        Case {
            stored_name: "dave.x",
            requested_name: "dave.X",
            // The whitespace the alphabet admits: tab, line feed, carriage return.
            password: "S3cret\u{0009}dave\u{000A}x\u{000D}!".to_string(),
            reset_required: Some(true),
        },
        Case {
            stored_name: "Eve+1",
            requested_name: "eve+1",
            // The top of the admitted alphabet: characters up to \u00FF.
            password: "S3cret-\u{00E9}ve-\u{00FF}\u{00A1}".to_string(),
            reset_required: Some(false),
        },
        Case {
            stored_name: "frank=2",
            requested_name: "frank=2",
            // The published minimum length, 1. A character the name alphabet
            // `^[\w+=,.@-]+$` cannot contain, so finding it in a response could
            // only mean the password leaked into one.
            password: "~".to_string(),
            reset_required: Some(true),
        },
        Case {
            stored_name: "grace@3",
            requested_name: "GRACE@3",
            // The published maximum length, 128.
            password: "G".repeat(128),
            reset_required: Some(false),
        },
        Case {
            stored_name: "heidi-4",
            requested_name: "heidi-4",
            // The characters the model calls out as "special meaning within that
            // tool": quotes, angle brackets and an ampersand, which also exercise
            // the response serializer's XML escaping.
            password: "S3cret \"&<>'\u{0020}heidi".to_string(),
            reset_required: Some(true),
        },
    ]
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

/// What `PasswordResetRequired` must report for a row: what the request asked for,
/// or the modelled default `false` when it asked for nothing.
fn expected_reset(case: &Case) -> &'static str {
    match case.reset_required {
        Some(true) => "true",
        Some(false) | None => "false",
    }
}

/// Assert that a response body is a login profile of the user `stored_name`, with
/// the reset flag the row expects, a timestamp, and NO password in it.
fn assert_login_profile(case_name: &str, body: &str, stored_name: &str, reset: &str, password: &str) {
    assert_eq!(
        element(body, "UserName"),
        stored_name,
        "case {case_name:?}: expected the profile to report the stored user name, got {:?}",
        element(body, "UserName")
    );
    assert_eq!(
        element(body, "PasswordResetRequired"),
        reset,
        "case {case_name:?}: expected PasswordResetRequired {reset}, got {:?}",
        element(body, "PasswordResetRequired")
    );
    let create_date = element(body, "CreateDate");
    assert!(
        aws_smithy_types::DateTime::from_str(
            &create_date,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .is_ok(),
        "case {case_name:?}: expected an ISO-8601 CreateDate, got {create_date:?}"
    );
    // `LoginProfile` carries only the user name, the create date and the reset flag
    // (`smithy_query(query="shape", shape="LoginProfile")`), so the password is in
    // no response — neither as a member nor anywhere in the body.
    assert!(
        elements(body, "Password").is_empty(),
        "case {case_name:?}: expected no Password element in the response, got {body}"
    );
    assert!(
        !body.contains(password),
        "case {case_name:?}: expected the password never to appear in a response, got {body}"
    );
}

/// RULE: `CreateLoginProfile` creates the named user's console password and
/// persists the login profile it answers with — the user's stored name, the instant
/// the password was created, and the reset-on-next-sign-in flag the request asked
/// for (false by default) — so a later `GetLoginProfile` for that user reports the
/// same profile, and the password appears in no response.
#[tokio::test]
async fn create_login_profile_persists_the_profile_the_read_reports() {
    let router = router();

    for case in cases() {
        let case_name = format!("{} requested as {}", case.stored_name, case.requested_name);
        let (status, body) = call(&router, "CreateUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: CreateUser must succeed, got {status}: {body}"
        );

        // Before the create there is no password, so the profile below is this
        // call's doing.
        let (status, missing) = call(
            &router,
            "GetLoginProfile",
            &[("UserName", case.requested_name)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case_name:?}: expected a user with no password to have no profile, got {status}: {missing}"
        );

        let (status, created) = call(
            &router,
            "CreateLoginProfile",
            &create_params(&case, case.requested_name),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: expected 200, got {status}: {created}"
        );
        assert_login_profile(
            &case_name,
            &created,
            case.stored_name,
            expected_reset(&case),
            &case.password,
        );

        // The profile was PERSISTED: the read reports the same values, including the
        // create date the create minted, and reading it twice cannot change what the
        // next caller sees.
        let (status, fetched) = call(
            &router,
            "GetLoginProfile",
            &[("UserName", case.requested_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: expected the created profile to be readable, got {status}: {fetched}"
        );
        assert_login_profile(
            &case_name,
            &fetched,
            case.stored_name,
            expected_reset(&case),
            &case.password,
        );
        assert_eq!(
            element(&fetched, "CreateDate"),
            element(&created, "CreateDate"),
            "case {case_name:?}: expected the read to report the stored create date, got {:?}",
            element(&fetched, "CreateDate")
        );

        let (status, again) = call(
            &router,
            "GetLoginProfile",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            (status, element(&again, "CreateDate")),
            (200, element(&created, "CreateDate")),
            "case {case_name:?}: a second read answered differently: {again}"
        );
    }
}

/// RULE: a user has at most one login profile, so a create for a user that already
/// has a password is refused with `EntityAlreadyExists` and leaves the password and
/// the profile already held untouched — whatever case the second request spells the
/// name in.
#[tokio::test]
async fn creating_a_second_password_for_a_user_is_refused() {
    let router = router();

    for case in cases() {
        let case_name = format!("{} requested as {}", case.stored_name, case.requested_name);
        let (status, body) = call(&router, "CreateUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: CreateUser must succeed, got {status}: {body}"
        );
        let (status, created) = call(
            &router,
            "CreateLoginProfile",
            &create_params(&case, case.stored_name),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: the first create must succeed, got {status}: {created}"
        );

        // A second create, spelling the name in the other case and asking for the
        // opposite reset flag with a different password. The password is a fixed
        // one rather than derived from the row's, so a row already at the 128
        // character bound stays inside `passwordType`.
        let second = Case {
            stored_name: case.stored_name,
            requested_name: case.requested_name,
            password: "Different-S3cret~2".to_string(),
            reset_required: Some(!matches!(case.reset_required, Some(true))),
        };
        let (status, refused) = call(
            &router,
            "CreateLoginProfile",
            &create_params(&second, second.requested_name),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case_name:?}: expected the second create to be refused with 409, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "EntityAlreadyExists",
            "case {case_name:?}: expected EntityAlreadyExists, got {:?}",
            error_code(&refused)
        );

        // The refusal changed nothing: the profile still reports what the first
        // create persisted.
        let (status, fetched) = call(
            &router,
            "GetLoginProfile",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: expected the held profile to survive, got {status}: {fetched}"
        );
        assert_login_profile(
            &case_name,
            &fetched,
            case.stored_name,
            expected_reset(&case),
            &case.password,
        );
        assert!(
            !fetched.contains(&second.password),
            "case {case_name:?}: expected the refused password never to appear in a response, got {fetched}"
        );
        assert_eq!(
            element(&fetched, "CreateDate"),
            element(&created, "CreateDate"),
            "case {case_name:?}: the refused create moved the create date, got {:?}",
            element(&fetched, "CreateDate")
        );
    }
}

/// RULE: a login profile exists only as the password of a user the account holds,
/// so a create naming an absent user is not found and persists nothing, and a read
/// of a user that does not exist — or exists without a password — is not found
/// either.
#[tokio::test]
async fn a_password_belongs_to_a_user_the_account_holds() {
    let router = router();
    let (status, body) = call(&router, "CreateUser", &[("UserName", "present")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");

    // Rows spanning a plain absent name, one differing from a held name by more than
    // case, names drawn from the rest of the accepted alphabet, and a name at the
    // 64-character bound `userNameType` admits.
    let absent: Vec<String> = vec![
        "absent".to_string(),
        "present2".to_string(),
        "no.such@user".to_string(),
        "NOT_HELD+1".to_string(),
        "x".repeat(64),
    ];

    for name in &absent {
        let (status, refused) = call(
            &router,
            "CreateLoginProfile",
            &[("UserName", name), ("Password", "S3cret-absent")],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected 404 from the create, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {:?}",
            error_code(&refused)
        );

        let (status, read) = call(&router, "GetLoginProfile", &[("UserName", name)]).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected 404 from the read, got {status}: {read}"
        );
        assert_eq!(
            error_code(&read),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {:?}",
            error_code(&read)
        );
    }

    // A user that exists but has no password has no login profile either.
    let (status, read) = call(&router, "GetLoginProfile", &[("UserName", "present")]).await;
    assert_eq!(
        status, 404,
        "expected a passwordless user to have no profile, got {status}: {read}"
    );
    assert_eq!(
        error_code(&read),
        "NoSuchEntity",
        "expected NoSuchEntity, got {:?}",
        error_code(&read)
    );
}

/// RULE: a password outside `passwordType`'s published length range is refused
/// before the operation runs, so a password the model forbids never creates a
/// profile; a password AT either bound is accepted.
#[tokio::test]
async fn a_password_outside_the_published_length_range_is_refused() {
    let router = router();
    for (index, length) in [0usize, 129, 200].into_iter().enumerate() {
        // `passwordType` is length 1..128, so 0 and anything past 128 is out of
        // range.
        let user = format!("outofrange{index}");
        let (status, body) = call(&router, "CreateUser", &[("UserName", &user)]).await;
        assert_eq!(
            status, 200,
            "case {length}: CreateUser must succeed, got {status}: {body}"
        );
        let (status, refused) = call(
            &router,
            "CreateLoginProfile",
            &[("UserName", &user), ("Password", &"p".repeat(length))],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {length}: expected 400, got {status}: {refused}"
        );
        let (status, read) = call(&router, "GetLoginProfile", &[("UserName", &user)]).await;
        assert_eq!(
            status, 404,
            "case {length}: a refused create made a profile anyway, got {status}: {read}"
        );
    }

    // Each published bound itself is accepted.
    for (index, length) in [1usize, 128].into_iter().enumerate() {
        let user = format!("inrange{index}");
        let (status, body) = call(&router, "CreateUser", &[("UserName", &user)]).await;
        assert_eq!(
            status, 200,
            "case {length}: CreateUser must succeed, got {status}: {body}"
        );
        let (status, created) = call(
            &router,
            "CreateLoginProfile",
            &[("UserName", &user), ("Password", &"p".repeat(length))],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {length}: expected 200, got {status}: {created}"
        );
        assert_eq!(
            element(&created, "UserName"),
            user,
            "case {length}: expected the profile of {user:?}, got {:?}",
            element(&created, "UserName")
        );
    }
}

/// RULE: `CreateLoginProfile` with no `UserName` creates the password for the IAM
/// user the request was signed by, resolved from the signing access key id, and
/// `GetLoginProfile` with no `UserName` reports that same user's profile.
#[tokio::test]
async fn an_omitted_name_serves_the_signing_user() {
    let router = router();

    for case in cases() {
        let (status, body) = call(&router, "CreateUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateUser must succeed, got {status}: {body}",
            case.stored_name
        );
        let (status, key) = call(
            &router,
            "CreateAccessKey",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the signing key must be minted, got {status}: {key}",
            case.stored_name
        );
        let signing_key = element(&key, "AccessKeyId");
        // Both calls below are IAM calls the caller's policies decide, so the signing user
        // needs those two grants. They are fixture: what this row is about is WHOSE
        // profile a nameless create and read reach.
        for action in ["iam:CreateLoginProfile", "iam:GetLoginProfile"] {
            crate::identity_policy_enforcement::grant_action(
                &router,
                case.stored_name,
                case.stored_name,
                action,
            )
            .await;
        }

        let mut params: Vec<(&str, &str)> = vec![("Password", &case.password)];
        if let Some(reset) = case.reset_required {
            params.push((
                "PasswordResetRequired",
                if reset { "true" } else { "false" },
            ));
        }
        let (status, created) = call_signed_by(
            &router,
            Some(&signing_key),
            "CreateLoginProfile",
            &params,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200 for the signing user's own password, got {status}: {created}",
            case.stored_name
        );
        assert_login_profile(
            case.stored_name,
            &created,
            case.stored_name,
            expected_reset(&case),
            &case.password,
        );

        let (status, fetched) =
            call_signed_by(&router, Some(&signing_key), "GetLoginProfile", &[]).await;
        assert_eq!(
            (status, element(&fetched, "UserName")),
            (200, case.stored_name.to_string()),
            "case {:?}: expected the signing user's profile, got {status}: {fetched}",
            case.stored_name
        );
    }
}

/// RULE: credentials that belong to no IAM user resolve to no user, so a request
/// that names none is refused rather than having an identity invented for it, and
/// nothing is created.
#[tokio::test]
async fn an_omitted_name_refuses_non_user_credentials() {
    let router = router();
    let (status, body) = call(&router, "CreateUser", &[("UserName", "keyholder")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");

    // Rows spanning no signing key at all, the fallback id an unsigned request
    // resolves to (`engine::extract_akid` answers "default"), a key id of the
    // modelled shape this emulator never issued, and an id that names a user rather
    // than a key.
    let signers: &[Option<&str>] = &[
        None,
        Some("default"),
        Some("AKIAIOSFODNN7EXAMPLE"),
        Some("EMULATORAKAAAAAAAAAA"),
        Some("keyholder"),
    ];

    for signer in signers {
        let case = format!("signed by {signer:?}");
        for action in ["CreateLoginProfile", "GetLoginProfile"] {
            let params: &[(&str, &str)] = if action == "CreateLoginProfile" {
                &[("Password", "S3cret-nobody")]
            } else {
                &[]
            };
            let (status, refused) = call_signed_by(&router, *signer, action, params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 from {action}, got {status}: {refused}"
            );
            // The behaviour under test is the REFUSAL: a 400 that creates nothing,
            // under the error code IAM answers a request it cannot accept with,
            // because neither operation models an error for a request that named no
            // entity.
            assert_eq!(
                error_code(&refused),
                "ValidationError",
                "case {case:?}: expected IAM's validation refusal code from {action}, got {:?}",
                error_code(&refused)
            );
        }
    }

    // Nothing was created for the user whose name was never spelled.
    let (status, read) = call(&router, "GetLoginProfile", &[("UserName", "keyholder")]).await;
    assert_eq!(
        status, 404,
        "expected no profile to have been created, got {status}: {read}"
    );
}

/// RULE: a password is what `CreateLoginProfile` creates, so a request that carries
/// none creates no profile at all and is refused.
#[tokio::test]
async fn a_create_without_a_password_creates_nothing() {
    let router = router();

    // Rows spanning the parameter omitted entirely and omitted beside each value of
    // the reset flag, since `PasswordResetRequired` is not what makes a password.
    let rows: &[&[(&str, &str)]] = &[
        &[("UserName", "nopassword")],
        &[
            ("UserName", "nopassword"),
            ("PasswordResetRequired", "true"),
        ],
        &[
            ("UserName", "nopassword"),
            ("PasswordResetRequired", "false"),
        ],
    ];

    let (status, body) = call(&router, "CreateUser", &[("UserName", "nopassword")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");

    for params in rows {
        let case = format!("{params:?}");
        let (status, refused) = call(&router, "CreateLoginProfile", params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationError",
            "case {case:?}: expected the framework's client-input refusal code, got {:?}",
            error_code(&refused)
        );
        let (status, read) = call(
            &router,
            "GetLoginProfile",
            &[("UserName", "nopassword")],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create made a profile anyway, got {status}: {read}"
        );
    }
}

/// RULE: a login profile is a dependent of its user, so IAM refuses to delete a
/// user that still has a password rather than removing the password behind the
/// caller's back — the same way an access key blocks the delete.
#[tokio::test]
async fn a_user_holding_a_login_profile_cannot_be_deleted() {
    let router = router();

    for case in cases() {
        let case_name = format!("{} requested as {}", case.stored_name, case.requested_name);
        let (status, body) = call(&router, "CreateUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: CreateUser must succeed, got {status}: {body}"
        );

        // Before any password exists the user deletes cleanly, so the refusal below
        // is the login profile's doing and not the user's.
        let (status, deleted) = call(&router, "DeleteUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: expected a passwordless user to delete, got {status}: {deleted}"
        );

        let (status, body) = call(&router, "CreateUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: CreateUser must succeed again, got {status}: {body}"
        );
        let (status, created) = call(
            &router,
            "CreateLoginProfile",
            &create_params(&case, case.requested_name),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case_name:?}: the create must succeed, got {status}: {created}"
        );

        let (status, refused) = call(&router, "DeleteUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 409,
            "case {case_name:?}: expected the delete to be refused with 409, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "DeleteConflict",
            "case {case_name:?}: expected DeleteConflict, got {:?}",
            error_code(&refused)
        );

        // The user is still there, and so is its login profile.
        let (status, fetched) = call(
            &router,
            "GetLoginProfile",
            &[("UserName", case.stored_name)],
        )
        .await;
        assert_eq!(
            (status, element(&fetched, "CreateDate")),
            (200, element(&created, "CreateDate")),
            "case {case_name:?}: expected the refused delete to leave the profile, got {status}: {fetched}"
        );
    }
}
