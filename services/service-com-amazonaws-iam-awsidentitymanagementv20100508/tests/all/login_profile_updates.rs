// SPDX-License-Identifier: CC0-1.0

//! Changing an IAM user's console password, end to end over the awsQuery wire.
//!
//! `UpdateLoginProfile` changes the password of a login profile the account already
//! holds: it applies the members the request supplies, leaves every member it omits
//! at the value the account holds, keeps the create date the first password minted,
//! and reports nothing — so `GetLoginProfile` is what reads the result back. The
//! awsQuery harness (router, call, element) is the one `tests/all/user_lifecycle.rs`
//! owns, so every suite in this crate builds and reads a request the same way.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! owning user's name alphabet and case, the case the request spells that name in,
//! the new password's length and alphabet (including the modelled bounds), which of
//! the two optional members the update supplies at all, and the value the profile
//! held before it.

use crate::user_lifecycle::{call, element, elements, error_code, router};

/// One update: the user the account holds, the case the request spells that user's
/// name in, the profile the create left behind, and what the update supplies.
struct Case {
    /// The name the user is created with — the case IAM stores and reports.
    stored_name: &'static str,
    /// The name `UpdateLoginProfile` spells; the same entity, differing only in
    /// case, because IAM entity names "are not distinguished by case"
    /// (`CreateUser$UserName`).
    requested_name: &'static str,
    /// The password `CreateLoginProfile` carried.
    created_password: String,
    /// `PasswordResetRequired` as the create asked for it.
    created_reset: bool,
    /// The new password the update carries, or `None` when the update omits
    /// `Password` entirely.
    new_password: Option<String>,
    /// `PasswordResetRequired` as the update sends it, or `None` when the update
    /// omits the member entirely. The member targets the BOXED `booleanObjectType`
    /// and carries no modelled default
    /// (`smithy_query(query="operation", operation="UpdateLoginProfile")`), so an
    /// omitted member leaves the held value alone.
    new_reset: Option<bool>,
}

impl Case {
    /// What `PasswordResetRequired` must report after the update: what the update
    /// supplied, or — when it supplied nothing — what the profile already held.
    fn expected_reset(&self) -> bool {
        self.new_reset.unwrap_or(self.created_reset)
    }

    /// A label naming the row in every assertion message.
    fn label(&self) -> String {
        format!(
            "{} requested as {} (held reset {}, update password {:?}, update reset {:?})",
            self.stored_name,
            self.requested_name,
            self.created_reset,
            self.new_password.as_ref().map(|password| password.len()),
            self.new_reset
        )
    }
}

/// Rows spanning the `^[\w+=,.@-]+$` name alphabet, both cases of the stored name,
/// the `passwordType` class the model admits — length 1..128 and the alphabet
/// `^[\u0009\u000A\u000D\u0020-\u00FF]+$`, so tab, line feed, carriage return and
/// every printable character from the space through `\u00FF` — and all four
/// combinations of the two optional members being supplied or omitted.
fn cases() -> Vec<Case> {
    vec![
        // Both members supplied, flipping the held flag.
        Case {
            stored_name: "alice",
            requested_name: "alice",
            created_password: "S3cret-alice".to_string(),
            created_reset: true,
            new_password: Some("N3w-alice-password".to_string()),
            new_reset: Some(false),
        },
        Case {
            stored_name: "Bob",
            requested_name: "bob",
            created_password: "S3cret-bob".to_string(),
            created_reset: false,
            new_password: Some("N3w-bob-password".to_string()),
            new_reset: Some(true),
        },
        // A password-only update: the held flag survives it.
        Case {
            stored_name: "carol_7",
            requested_name: "CAROL_7",
            created_password: "S3cret-carol".to_string(),
            created_reset: true,
            new_password: Some("N3w-carol-password".to_string()),
            new_reset: None,
        },
        Case {
            stored_name: "dave.x",
            requested_name: "dave.X",
            created_password: "S3cret-dave".to_string(),
            created_reset: false,
            // The whitespace the alphabet admits: tab, line feed, carriage return.
            new_password: Some("N3w\u{0009}dave\u{000A}x\u{000D}!".to_string()),
            new_reset: None,
        },
        // A flag-only update: the password the user already has stays in place.
        Case {
            stored_name: "Eve+1",
            requested_name: "eve+1",
            created_password: "S3cret-eve".to_string(),
            created_reset: true,
            new_password: None,
            new_reset: Some(false),
        },
        // The published minimum password length, 1. A character the name alphabet
        // `^[\w+=,.@-]+$` cannot contain, so finding it in a response could only mean
        // the password leaked into one.
        Case {
            stored_name: "frank=2",
            requested_name: "frank=2",
            created_password: "S3cret-frank".to_string(),
            created_reset: false,
            new_password: Some("~".to_string()),
            new_reset: Some(false),
        },
        // The published maximum password length, 128.
        Case {
            stored_name: "grace@3",
            requested_name: "GRACE@3",
            created_password: "S3cret-grace".to_string(),
            created_reset: false,
            new_password: Some("G".repeat(128)),
            new_reset: Some(true),
        },
        // Neither member supplied: nothing to apply, so nothing changes.
        Case {
            stored_name: "heidi-4",
            requested_name: "heidi-4",
            // The top of the admitted alphabet, characters up to \u00FF, plus the
            // characters the XML serializer has to escape.
            created_password: "S3cret-\u{00E9}ve-\u{00FF} \"&<>'".to_string(),
            created_reset: true,
            new_password: None,
            new_reset: None,
        },
    ]
}

/// The awsQuery parameters of the create a row starts from.
fn create_params<'a>(case: &'a Case, name: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("UserName", name),
        ("Password", &case.created_password),
        (
            "PasswordResetRequired",
            if case.created_reset { "true" } else { "false" },
        ),
    ]
}

/// The awsQuery parameters of the update a row drives, carrying only the members
/// that row supplies.
fn update_params<'a>(case: &'a Case, name: &'a str) -> Vec<(&'a str, &'a str)> {
    let mut params: Vec<(&str, &str)> = vec![("UserName", name)];
    if let Some(password) = &case.new_password {
        params.push(("Password", password));
    }
    if let Some(reset) = case.new_reset {
        params.push(("PasswordResetRequired", if reset { "true" } else { "false" }));
    }
    params
}

/// Assert that `body` is the login profile of `stored_name` with the reset flag
/// `reset`, that its create date is an ISO-8601 instant, that it carries no
/// password, and that none of `passwords` appears anywhere in it.
fn assert_profile(label: &str, body: &str, stored_name: &str, reset: bool, passwords: &[&str]) {
    assert_eq!(
        element(body, "UserName"),
        stored_name,
        "case {label:?}: expected the profile to report the stored user name, got {:?}",
        element(body, "UserName")
    );
    assert_eq!(
        element(body, "PasswordResetRequired"),
        if reset { "true" } else { "false" },
        "case {label:?}: expected PasswordResetRequired {reset}, got {:?}",
        element(body, "PasswordResetRequired")
    );
    let create_date = element(body, "CreateDate");
    assert!(
        aws_smithy_types::DateTime::from_str(
            &create_date,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .is_ok(),
        "case {label:?}: expected an ISO-8601 CreateDate, got {create_date:?}"
    );
    // `LoginProfile` carries only the user name, the create date and the reset flag
    // (`smithy_query(query="shape", shape="LoginProfile")`), so no password is in any
    // response — neither as a member nor anywhere in the body.
    assert!(
        elements(body, "Password").is_empty(),
        "case {label:?}: expected no Password element in the response, got {body}"
    );
    for password in passwords {
        assert!(
            !body.contains(password),
            "case {label:?}: expected no password ever to appear in a response, got {body}"
        );
    }
}

/// RULE: `UpdateLoginProfile` changes the password of the login profile the named
/// user already holds, applying only the members the request supplies — a member it
/// omits keeps the value the account holds — while the create date stays the instant
/// the first password was created, and the operation reports nothing of its own, so
/// `GetLoginProfile` answers with the updated profile afterwards.
#[tokio::test]
async fn an_update_applies_the_members_it_supplies_and_leaves_the_rest() {
    let router = router();

    for case in cases() {
        let label = case.label();
        let (status, body) = call(&router, "CreateUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateUser must succeed, got {status}: {body}"
        );
        let (status, created) = call(
            &router,
            "CreateLoginProfile",
            &create_params(&case, case.stored_name),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the create must succeed, got {status}: {created}"
        );
        let created_date = element(&created, "CreateDate");

        let (status, updated) = call(
            &router,
            "UpdateLoginProfile",
            &update_params(&case, case.requested_name),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from the update, got {status}: {updated}"
        );
        // The output shape is `Unit`, so the update reports no profile of its own —
        // and certainly no password.
        assert!(
            elements(&updated, "LoginProfile").is_empty(),
            "case {label:?}: expected the update to report no login profile, got {updated}"
        );
        let mut passwords = vec![case.created_password.as_str()];
        if let Some(password) = &case.new_password {
            passwords.push(password.as_str());
        }
        for password in &passwords {
            assert!(
                !updated.contains(*password),
                "case {label:?}: expected no password in the update response, got {updated}"
            );
        }

        // The update PERSISTED: the read reports the flag the update left behind, the
        // user's stored name, and the create date the create minted.
        let (status, fetched) = call(
            &router,
            "GetLoginProfile",
            &[("UserName", case.requested_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the updated profile to be readable, got {status}: {fetched}"
        );
        assert_profile(
            &label,
            &fetched,
            case.stored_name,
            case.expected_reset(),
            &passwords,
        );
        assert_eq!(
            element(&fetched, "CreateDate"),
            created_date,
            "case {label:?}: the update moved the create date, got {:?}",
            element(&fetched, "CreateDate")
        );

        // Reading it again answers the same, so neither the update nor the read
        // changed what a later caller sees.
        let (status, again) = call(&router, "GetLoginProfile", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            (status, element(&again, "PasswordResetRequired"), element(&again, "CreateDate")),
            (
                200,
                if case.expected_reset() { "true" } else { "false" }.to_string(),
                created_date.clone()
            ),
            "case {label:?}: a second read answered differently: {again}"
        );
    }
}

/// RULE: repeating an update leaves the same profile behind — the members it
/// supplies are set to the values it names however many times it is applied, and a
/// later update that supplies neither member changes nothing at all.
#[tokio::test]
async fn repeating_an_update_changes_nothing_further() {
    let router = router();

    for case in cases() {
        let label = case.label();
        let (status, body) = call(&router, "CreateUser", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateUser must succeed, got {status}: {body}"
        );
        let (status, created) = call(
            &router,
            "CreateLoginProfile",
            &create_params(&case, case.stored_name),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the create must succeed, got {status}: {created}"
        );
        let created_date = element(&created, "CreateDate");

        for attempt in 0..3 {
            let (status, updated) = call(
                &router,
                "UpdateLoginProfile",
                &update_params(&case, case.requested_name),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?} attempt {attempt}: expected 200, got {status}: {updated}"
            );
        }
        // An update supplying neither value member: there is nothing to apply, so the
        // profile is exactly what the updates above left.
        let (status, empty) = call(
            &router,
            "UpdateLoginProfile",
            &[("UserName", case.requested_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 for an update supplying no value member, got {status}: {empty}"
        );

        let (status, fetched) = call(&router, "GetLoginProfile", &[("UserName", case.stored_name)]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the profile to be readable, got {status}: {fetched}"
        );
        assert_profile(
            &label,
            &fetched,
            case.stored_name,
            case.expected_reset(),
            &[case.created_password.as_str()],
        );
        assert_eq!(
            element(&fetched, "CreateDate"),
            created_date,
            "case {label:?}: a repeated update moved the create date, got {:?}",
            element(&fetched, "CreateDate")
        );
    }
}

/// RULE: this operation changes a password that exists, so an update naming a user
/// the account does not hold — or one that holds no password — is not found and
/// creates nothing, since the first password is `CreateLoginProfile`'s to create.
#[tokio::test]
async fn an_update_without_a_profile_to_change_is_not_found() {
    let router = router();

    // Rows spanning a plain absent name, a name differing from a held one by more
    // than case, names drawn from the rest of the accepted alphabet, and a name at
    // the 64-character bound `userNameType` admits.
    let absent: Vec<String> = vec![
        "absent".to_string(),
        "present2".to_string(),
        "no.such@user".to_string(),
        "NOT_HELD+1".to_string(),
        "x".repeat(64),
    ];
    // Rows spanning both value members, each on its own and together, so the refusal
    // does not depend on what the update carried.
    let supplied: &[&[(&str, &str)]] = &[
        &[],
        &[("Password", "N3w-absent-password")],
        &[("PasswordResetRequired", "true")],
        &[
            ("Password", "N3w-absent-password"),
            ("PasswordResetRequired", "false"),
        ],
    ];

    let (status, body) = call(&router, "CreateUser", &[("UserName", "present")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");

    // "present" exists but has no password, so it has no login profile either: an
    // update for it is refused exactly as one for a name the account never held.
    for name in absent.iter().map(String::as_str).chain(["present"]) {
        for extra in supplied {
            let label = format!("{name} with {extra:?}");
            let mut params: Vec<(&str, &str)> = vec![("UserName", name)];
            params.extend_from_slice(extra);
            let (status, refused) = call(&router, "UpdateLoginProfile", &params).await;
            assert_eq!(
                status, 404,
                "case {label:?}: expected 404 from the update, got {status}: {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "NoSuchEntity",
                "case {label:?}: expected NoSuchEntity, got {:?}",
                error_code(&refused)
            );

            // The refused update created nothing.
            let (status, read) = call(&router, "GetLoginProfile", &[("UserName", name)]).await;
            assert_eq!(
                status, 404,
                "case {label:?}: a refused update made a profile anyway, got {status}: {read}"
            );
        }
    }
}

/// RULE: a password outside `passwordType`'s published length range is refused
/// before the operation runs, so it changes nothing about the profile the account
/// holds; a password AT either published bound is accepted.
#[tokio::test]
async fn an_update_password_outside_the_published_length_range_is_refused() {
    let router = router();
    let (status, body) = call(&router, "CreateUser", &[("UserName", "bounded")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");
    let (status, created) = call(
        &router,
        "CreateLoginProfile",
        &[
            ("UserName", "bounded"),
            ("Password", "S3cret-bounded"),
            ("PasswordResetRequired", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the create must succeed, got {status}: {created}"
    );
    let created_date = element(&created, "CreateDate");

    // `passwordType` is length 1..128, so 0 and anything past 128 is out of range.
    for length in [0usize, 129, 200] {
        let (status, refused) = call(
            &router,
            "UpdateLoginProfile",
            &[
                ("UserName", "bounded"),
                ("Password", &"p".repeat(length)),
                ("PasswordResetRequired", "false"),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {length}: expected 400, got {status}: {refused}"
        );
        // The refusal applied NOTHING: the flag the create asked for is still there,
        // and so is the create date.
        let (status, read) = call(&router, "GetLoginProfile", &[("UserName", "bounded")]).await;
        assert_eq!(
            status, 200,
            "case {length}: expected the held profile to survive, got {status}: {read}"
        );
        assert_profile(
            &format!("password of length {length}"),
            &read,
            "bounded",
            true,
            &["S3cret-bounded"],
        );
        assert_eq!(
            element(&read, "CreateDate"),
            created_date,
            "case {length}: a refused update moved the create date, got {:?}",
            element(&read, "CreateDate")
        );
    }

    // Each published bound itself is accepted, and the update beside it applies. The
    // password is built from `~`, a character the response XML and the name alphabet
    // `^[\w+=,.@-]+$` cannot contain, so a body holding it could only mean the
    // password leaked into a response — even at the one-character bound.
    for (index, length) in [1usize, 128].into_iter().enumerate() {
        let reset = index % 2 == 0;
        let (status, updated) = call(
            &router,
            "UpdateLoginProfile",
            &[
                ("UserName", "bounded"),
                ("Password", &"~".repeat(length)),
                ("PasswordResetRequired", if reset { "true" } else { "false" }),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {length}: expected 200, got {status}: {updated}"
        );
        let (status, read) = call(&router, "GetLoginProfile", &[("UserName", "bounded")]).await;
        assert_eq!(
            status, 200,
            "case {length}: expected the profile to be readable, got {status}: {read}"
        );
        assert_profile(
            &format!("password of length {length}"),
            &read,
            "bounded",
            reset,
            &["S3cret-bounded", &"~".repeat(length)],
        );
    }
}
