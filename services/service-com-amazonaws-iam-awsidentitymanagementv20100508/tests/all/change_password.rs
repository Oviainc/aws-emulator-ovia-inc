// SPDX-License-Identifier: CC0-1.0

//! An IAM user changing its OWN console password, end to end over the awsQuery wire.
//!
//! RULE: a request that carries a secret the service accepted earlier is served only
//! when that secret is the one in force, so the account holds a verifier of every
//! console password a write sets and `ChangePassword` — which "changes the password of
//! the IAM user who is calling this operation" and takes that user's "current password"
//! — replaces the password of the user the signing access key belongs to only when the
//! `OldPassword` it presents verifies against the password in force. The change leaves
//! the create date the first password minted ("the CreateDate reflects the initial
//! password creation date", `GetLoginProfile`) and clears the demand that the user "set
//! a new password on next sign-in", which the user has just met; a request that presents
//! anything else is refused HTTP 403 `AccessDenied` and the password in force survives.
//!
//! Each test is a TABLE whose rows differ along what the rule generalises over: the
//! password's length (including both published `passwordType` bounds) and alphabet, the
//! owning user's name alphabet and case, the reset demand the profile carried, which
//! write set the password in force (the create, the administrative update, or an earlier
//! change), and the way a presented password differs from the one in force.
//!
//! The awsQuery harness (router, call, element) is the one `tests/all/user_lifecycle.rs`
//! owns, so every suite in this crate builds and reads a request the same way.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{
    assert_access_denied_message, call, call_signed_by, element, elements, error_code, router,
};

/// The error code AWS answers a `ChangePassword` whose `OldPassword` is not the password
/// in force with, and the status it carries (HTTP 403): presenting the current password
/// is what entitles a caller to replace it.
const REFUSED_CODE: &str = "AccessDenied";

/// The error code a caller whose credential belongs to no IAM user is refused with —
/// `InvalidUserTypeException`'s awsQuery code, HTTP 400 — because "the Amazon Web
/// Services account root user password is not affected by this operation" and a session
/// credential has no console password of its own.
const WRONG_USER_TYPE_CODE: &str = "InvalidUserType";

/// The code a caller with no console password at all is refused with: a login profile
/// "is created when you create a password for the user" (`GetLoginProfile`), so there is
/// no password to change.
const NOT_FOUND_CODE: &str = "NoSuchEntity";

/// One password the account can hold: a row of the class `passwordType` admits (len
/// 1..128, pattern `^[\u0009\u000A\u000D\u0020-\u00FF]+$` —
/// `smithy_query(query="operation", operation="ChangePassword")`).
struct Case {
    /// The name the user is created with, spanning the `^[\w+=,.@-]+$` alphabet
    /// `userNameType` admits.
    stored_name: &'static str,
    /// Whether the create asked that the user set a new password on next sign-in.
    created_reset: bool,
    /// The password `CreateLoginProfile` set, which the first change must present.
    first_password: String,
    /// The password that change sets, which a second change must present instead.
    second_password: String,
}

impl Case {
    /// A label naming the row in every assertion message. The passwords themselves are
    /// not in it: a secret belongs in no diagnostic.
    fn label(&self) -> String {
        format!(
            "{} (held reset {}, password lengths {} then {})",
            self.stored_name,
            self.created_reset,
            self.first_password.chars().count(),
            self.second_password.chars().count()
        )
    }
}

/// Rows spanning the name alphabet and both cases, both values of the reset demand, and
/// the password class: the published minimum length (1), the published maximum (128),
/// the whitespace the alphabet admits (tab, line feed, carriage return), the characters
/// up to `\u00FF`, and the characters the XML serializer has to escape.
fn cases() -> Vec<Case> {
    vec![
        Case {
            stored_name: "alice",
            created_reset: true,
            first_password: "S3cret-alice".to_string(),
            second_password: "N3w-alice-password".to_string(),
        },
        Case {
            stored_name: "Bob",
            created_reset: false,
            first_password: "S3cret-bob".to_string(),
            second_password: "N3w-bob-password".to_string(),
        },
        Case {
            stored_name: "carol_7",
            created_reset: true,
            // The whitespace the alphabet admits.
            first_password: "S3cret\u{0009}carol\u{000A}x\u{000D}!".to_string(),
            second_password: "N3w\u{0009}carol".to_string(),
        },
        Case {
            stored_name: "dave.x",
            created_reset: false,
            // The top of the admitted alphabet, plus the characters XML escapes.
            first_password: "S3cret-\u{00E9}ve-\u{00FF} \"&<>'".to_string(),
            second_password: "N3w-\u{00FF}-dave".to_string(),
        },
        Case {
            stored_name: "Eve+1",
            created_reset: true,
            // The published minimum length, 1, on both sides of the change.
            first_password: "~".to_string(),
            second_password: "!".to_string(),
        },
        Case {
            stored_name: "frank=2",
            created_reset: false,
            // The published maximum length, 128.
            first_password: "F".repeat(128),
            second_password: "G".repeat(128),
        },
        Case {
            stored_name: "grace@3",
            created_reset: true,
            first_password: "S3cret-grace".to_string(),
            // One character of the maximum, so the two differ in length as well.
            second_password: "H".repeat(127),
        },
        Case {
            stored_name: "heidi-4",
            created_reset: false,
            first_password: "S3cret-heidi".to_string(),
            second_password: "S3cret-heidi-2".to_string(),
        },
    ]
}

/// Create `user`, give it the console password `password` with the reset demand `reset`,
/// and mint an access key for it — answering that key's id, which is what a request
/// `ChangePassword` serves is signed by.
async fn user_with_password(
    router: &MultiServiceRouter,
    case: &str,
    user: &str,
    password: &str,
    reset: bool,
) -> String {
    let (status, body) = call(router, "CreateUser", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser must succeed, got {status}: {body}"
    );
    let (status, created) = call(
        router,
        "CreateLoginProfile",
        &[
            ("UserName", user),
            ("Password", password),
            ("PasswordResetRequired", if reset { "true" } else { "false" }),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateLoginProfile must succeed, got {status}: {created}"
    );
    user_key(router, case, user).await
}

/// Mint an access key for `user` and answer its id.
async fn user_key(router: &MultiServiceRouter, case: &str, user: &str) -> String {
    let (status, minted) = call(router, "CreateAccessKey", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateAccessKey must succeed, got {status}: {minted}"
    );
    element(&minted, "AccessKeyId")
}

/// One `ChangePassword` presented as signed by `akid`.
async fn change(
    router: &MultiServiceRouter,
    akid: Option<&str>,
    old_password: &str,
    new_password: &str,
) -> (u16, String) {
    call_signed_by(
        router,
        akid,
        "ChangePassword",
        &[
            ("OldPassword", old_password),
            ("NewPassword", new_password),
        ],
    )
    .await
}

/// `GetLoginProfile` for `user`.
async fn profile(router: &MultiServiceRouter, user: &str) -> (u16, String) {
    call(router, "GetLoginProfile", &[("UserName", user)]).await
}

/// The action the refusal names, and the reason clause closing it, spelled out here
/// rather than imported from the handler, so a change of either wording fails this
/// suite instead of following it.
const ACTION: &str = "iam:ChangePassword";
const REASON: &str = "because the old password supplied is not the user's current password";

/// The ARN the account holds `user` under, read back through `GetUser` rather than
/// built here, so an assertion about a refusal's message is made against the value the
/// account itself reports.
async fn user_arn(router: &MultiServiceRouter, case: &str, user: &str) -> String {
    let (status, read) = call(router, "GetUser", &[("UserName", user)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture GetUser must succeed, got {status}: {read}"
    );
    element(&read, "Arn")
}

/// Assert that no password in `passwords` appears anywhere in `body`, and that the body
/// carries no `Password` element: `LoginProfile` carries only the user name, the create
/// date and the reset flag (`smithy_query(query="shape", shape="LoginProfile")`), and
/// every password member the model declares is sensitive.
fn assert_no_password(label: &str, body: &str, passwords: &[&str]) {
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

/// RULE: a change that presents the password in force replaces it, reports nothing of
/// its own, keeps the create date the first password minted and clears the reset demand;
/// the password it SET is then the one in force, so the next change must present that
/// one and the replaced password no longer works.
#[tokio::test]
async fn a_change_presenting_the_password_in_force_replaces_it() {
    let router = router();

    for case in cases() {
        let label = case.label();
        let akid = user_with_password(
            &router,
            &label,
            case.stored_name,
            &case.first_password,
            case.created_reset,
        )
        .await;
        let (_, created) = profile(&router, case.stored_name).await;
        let created_date = element(&created, "CreateDate");

        let (status, changed) = change(
            &router,
            Some(&akid),
            &case.first_password,
            &case.second_password,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected 200 from the change, got {status}: {changed}"
        );
        // The output shape is `Unit`, so the change reports no profile of its own.
        assert!(
            elements(&changed, "LoginProfile").is_empty(),
            "case {label:?}: expected the change to report no login profile, got {changed}"
        );
        assert_no_password(
            &label,
            &changed,
            &[&case.first_password, &case.second_password],
        );

        // The change PERSISTED: the profile is readable, under the stored name, with the
        // create date the create minted and no reset demand left standing.
        let (status, read) = profile(&router, case.stored_name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the profile to be readable, got {status}: {read}"
        );
        assert_eq!(
            element(&read, "UserName"),
            case.stored_name,
            "case {label:?}: expected the stored user name, got {:?}",
            element(&read, "UserName")
        );
        assert_eq!(
            element(&read, "PasswordResetRequired"),
            "false",
            "case {label:?}: expected the change to clear the reset demand, got {:?}",
            element(&read, "PasswordResetRequired")
        );
        assert_eq!(
            element(&read, "CreateDate"),
            created_date,
            "case {label:?}: the change moved the create date, got {:?}",
            element(&read, "CreateDate")
        );
        assert_no_password(
            &label,
            &read,
            &[&case.first_password, &case.second_password],
        );

        // The replaced password is no longer in force, so presenting it again is
        // refused, and the password the change set IS in force, so presenting that one
        // succeeds.
        let (status, refused) = change(
            &router,
            Some(&akid),
            &case.first_password,
            &case.second_password,
        )
        .await;
        assert_eq!(
            status, 403,
            "case {label:?}: expected 403 for the replaced password, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            REFUSED_CODE,
            "case {label:?}: expected {REFUSED_CODE}, got {:?}",
            error_code(&refused)
        );
        // The refusal is worded from the account's own report of the caller: its ARN
        // (which carries the account id and the user's path) and the user name in the
        // case the account stores, which differs from the case this row spelled.
        let arn = user_arn(&router, &label, case.stored_name).await;
        assert_access_denied_message(
            &label,
            &element(&refused, "Message"),
            &arn,
            ACTION,
            &format!("user {}", case.stored_name),
            REASON,
        );
        assert_no_password(
            &label,
            &refused,
            &[&case.first_password, &case.second_password],
        );
        let (status, again) = change(
            &router,
            Some(&akid),
            &case.second_password,
            &case.first_password,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the password the change set to be in force, \
             got {status}: {again}"
        );
    }
}

/// RULE: only the password in force is accepted, so a presented value that differs from
/// it in any way — a different password, its case folded, a prefix or an extension of
/// it, the new password, a password another user holds — is refused and the password in
/// force survives, which the change that presents the right one afterwards proves.
#[tokio::test]
async fn a_change_presenting_anything_else_is_refused_and_changes_nothing() {
    let router = router();
    let held = "S3cret-held-\u{00E9}";
    let other_user_password = "S3cret-other";
    let akid = user_with_password(&router, "refusals", "Refused", held, true).await;
    let arn = user_arn(&router, "refusals", "Refused").await;
    // A second user, so a row can present a password that IS in force — for somebody
    // else.
    user_with_password(&router, "refusals", "other", other_user_password, false).await;

    // Rows spanning the ways a presented password can differ from the one in force.
    let wrong: Vec<(&str, String)> = vec![
        ("an unrelated password", "N0t-the-password".to_string()),
        ("the held password in another case", held.to_uppercase()),
        (
            "a prefix of the held password",
            held.chars().take(held.chars().count() - 1).collect(),
        ),
        ("the held password with a suffix", format!("{held}x")),
        (
            "the held password with a trailing space",
            format!("{held} "),
        ),
        ("the password the change would set", "N3w-password".to_string()),
        (
            "a password in force for another user",
            other_user_password.to_string(),
        ),
        ("a single character", "~".to_string()),
        // The published maximum length, so the refusal is not about the size of the
        // value.
        ("the longest password the model admits", "Z".repeat(128)),
    ];

    for (label, presented) in &wrong {
        let (status, refused) = change(&router, Some(&akid), presented, "N3w-password").await;
        assert_eq!(
            status, 403,
            "case {label:?}: expected 403 from the change, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            REFUSED_CODE,
            "case {label:?}: expected {REFUSED_CODE}, got {:?}",
            error_code(&refused)
        );
        assert_access_denied_message(
            label,
            &element(&refused, "Message"),
            &arn,
            ACTION,
            "user Refused",
            REASON,
        );
        assert_no_password(label, &refused, &[held, presented, "N3w-password"]);

        // Nothing moved: the reset demand the create asked for still stands, and the
        // password in force is still the held one.
        let (status, read) = profile(&router, "refused").await;
        assert_eq!(
            (status, element(&read, "PasswordResetRequired")),
            (200, "true".to_string()),
            "case {label:?}: a refused change altered the profile: {read}"
        );
    }

    // The held password is still the one in force after every refusal above.
    let (status, changed) = change(&router, Some(&akid), held, "N3w-password").await;
    assert_eq!(
        status, 200,
        "expected the held password to still be in force after the refusals, \
         got {status}: {changed}"
    );
}

/// RULE: the user whose password changes is the one the signing credential belongs to,
/// so a request presented with a credential no IAM user owns changes nothing and is
/// refused as the wrong type of user; and a caller that holds no console password has
/// none to change, which is the not-found refusal the login-profile reads answer with.
#[tokio::test]
async fn a_caller_without_an_iam_identity_or_a_password_is_refused() {
    let router = router();
    let held = "S3cret-identified";
    let akid = user_with_password(&router, "identity", "identified", held, false).await;

    // Rows spanning the credentials that belong to no IAM user of this account: a key
    // id the account holds no record of, and the shape a session credential's id takes.
    // A request carrying NO credential is not one of them: the emulator verifies every
    // signature, so an unsigned request is refused as an authentication failure before
    // any handler runs, which is a different refusal and not this operation's.
    let foreign: &[(&str, Option<&str>)] = &[
        ("an unknown long-term key", Some("AKIAUNKNOWNKEYID0000")),
        ("a session credential", Some("ASIASESSIONKEYID0000")),
    ];
    for (label, akid) in foreign {
        let (status, refused) = change(&router, *akid, held, "N3w-password").await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 from the change, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            WRONG_USER_TYPE_CODE,
            "case {label:?}: expected {WRONG_USER_TYPE_CODE}, got {:?}",
            error_code(&refused)
        );
        // The user that does hold a password still holds the same one.
        let (status, read) = profile(&router, "identified").await;
        assert_eq!(
            status, 200,
            "case {label:?}: the refused change disturbed another user's profile: {read}"
        );
    }
    let (status, changed) = change(&router, Some(&akid), held, "N3w-password").await;
    assert_eq!(
        status, 200,
        "expected the held password to still be in force, got {status}: {changed}"
    );

    // A user the account holds, with a key of its own, but no console password: there is
    // no password to change, and creating the first one is `CreateLoginProfile`'s job.
    let (status, body) = call(&router, "CreateUser", &[("UserName", "Passwordless")]).await;
    assert_eq!(status, 200, "CreateUser must succeed, got {status}: {body}");
    let passwordless = user_key(&router, "passwordless", "Passwordless").await;
    let (status, refused) = change(
        &router,
        Some(&passwordless),
        "S3cret-none",
        "N3w-password",
    )
    .await;
    assert_eq!(
        status, 404,
        "expected 404 for a caller with no console password, got {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        NOT_FOUND_CODE,
        "expected {NOT_FOUND_CODE}, got {:?}",
        error_code(&refused)
    );
    assert_eq!(
        element(&refused, "Message"),
        "Login Profile for User Passwordless cannot be found.",
        "expected the login-profile not-found sentence naming the stored user name, got {:?}",
        element(&refused, "Message")
    );
    // The refused change created nothing.
    let (status, read) = profile(&router, "Passwordless").await;
    assert_eq!(
        status, 404,
        "a refused change made a profile anyway, got {status}: {read}"
    );
}

/// RULE: the password in force is whatever the LAST write set, whichever operation set
/// it — the create, the administrative `UpdateLoginProfile`, or an earlier change — so a
/// change must present that one and nothing else.
#[tokio::test]
async fn the_password_in_force_is_the_one_the_last_write_set() {
    let router = router();

    // Rows spanning the writes that set a console password, each naming the password it
    // leaves in force.
    let created = "S3cret-created";
    let updated = "S3cret-updated-\u{00FF}";
    let changed = "S3cret-changed";
    let akid = user_with_password(&router, "writes", "Writer", created, true).await;

    // The create's password is in force, so the administrative update is what replaces
    // it next.
    let (status, body) = call(
        &router,
        "UpdateLoginProfile",
        &[("UserName", "writer"), ("Password", updated)],
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateLoginProfile must succeed, got {status}: {body}"
    );
    // The password the update replaced is no longer in force.
    let (status, refused) = change(&router, Some(&akid), created, changed).await;
    assert_eq!(
        status, 403,
        "expected the password the update replaced to be refused, got {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        REFUSED_CODE,
        "expected {REFUSED_CODE}, got {:?}",
        error_code(&refused)
    );
    // The update's password IS, and the change replaces it.
    let (status, body) = change(&router, Some(&akid), updated, changed).await;
    assert_eq!(
        status, 200,
        "expected the password the update set to be in force, got {status}: {body}"
    );
    // The update left the reset demand the create asked for standing (it supplied no
    // `PasswordResetRequired`), and the change has now cleared it.
    let (status, read) = profile(&router, "WRITER").await;
    assert_eq!(
        (status, element(&read, "PasswordResetRequired")),
        (200, "false".to_string()),
        "expected the change to clear the reset demand: {read}"
    );
    // And the change's own password is what a further change must present.
    let (status, refused) = change(&router, Some(&akid), updated, created).await;
    assert_eq!(
        status, 403,
        "expected the password the change replaced to be refused, got {status}: {refused}"
    );
    let (status, body) = change(&router, Some(&akid), changed, created).await;
    assert_eq!(
        status, 200,
        "expected the password the change set to be in force, got {status}: {body}"
    );
    assert_no_password("writes", &body, &[created, updated, changed]);
}
