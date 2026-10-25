// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal a request earns by saying how a user authenticates
//! TWICE: an `AuthenticationMode`, which states the whole of it on its own, beside one of
//! the older members that state it too (`NoPasswordRequired`, `Passwords`).
//!
//! The rule under test: such a request is refused HTTP 400 `InvalidParameterCombination`
//! with a message built from the two directives the CALLER submitted — the submitted value
//! of the older `NoPasswordRequired` flag, or the fact that passwords were supplied,
//! together with the `AuthenticationMode$Type` the same request asked for — so the answer
//! says which pair collided instead of restating the rule.
//!
//! The table spans what that rule generalises over: every token of the modelled
//! `InputAuthenticationType` catalog (`smithy_query(query="shape",
//! shape="InputAuthenticationType")`: `password | no-password-required | iam`), both values
//! of the boolean flag, the pair whose two halves contradict each other and the pair whose
//! halves agree, the other older member (`Passwords`), a request carrying both older
//! members at once, and both entry points that take these members (`CreateUser` and
//! `ModifyUser`). No single row is what makes the suite pass.
//!
//! Every assertion goes over the wire through [`crate::harness`], and the expected
//! sentences are written out here on purpose: a test that built them from the same helper
//! the handlers call would pass whatever that helper said. Each refused row is also read
//! back, so a refusal that touched the stored user cannot pass, and the suite ends on the
//! positive control — an unopposed modify of the same user still succeeding and settling.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, PAST_TRANSITION};
use crate::modify_user::describe_user;

/// A password inside the published length rule (`state::PASSWORD_MIN_LENGTH` to
/// `state::PASSWORD_MAX_LENGTH` characters), for the user every row starts from.
const PASSWORD: &str = "aVeryLongPassword1";
/// A second one, for the request that tries to change the authentication.
const PASSWORD_TWO: &str = "aVeryLongPassword2";

/// The access string the seeded user is created with, and the expanded form the service
/// stores for it — what a refused request must leave untouched.
const CREATED_ACCESS_STRING: &str = "on ~* +@all";

/// One refused row: the case name, the authentication members the request carries beside
/// the shared ones, and the exact message the refusal must carry.
type ConflictCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    &'static str,
);

/// The conflict table. Each row names an `AuthenticationMode` AND an older member, and the
/// sentence states the submitted value of that older member with the submitted type.
const CONFLICT_CASES: &[ConflictCase] = &[
    (
        "the flag is set and the mode asks for a password",
        &[
            ("NoPasswordRequired", "true"),
            ("AuthenticationMode.Type", "password"),
            ("AuthenticationMode.Passwords.member.1", PASSWORD_TWO),
        ],
        "No password required flag is true but provided authentication type is password",
    ),
    (
        "the flag is set and the mode asks for IAM",
        &[
            ("NoPasswordRequired", "true"),
            ("AuthenticationMode.Type", "iam"),
        ],
        "No password required flag is true but provided authentication type is iam",
    ),
    (
        "the flag is set and the mode asks for the same thing",
        &[
            ("NoPasswordRequired", "true"),
            ("AuthenticationMode.Type", "no-password-required"),
        ],
        "No password required flag is true but provided authentication type is \
         no-password-required",
    ),
    (
        "the flag is cleared and the mode asks for no password",
        &[
            ("NoPasswordRequired", "false"),
            ("AuthenticationMode.Type", "no-password-required"),
        ],
        "No password required flag is false but provided authentication type is \
         no-password-required",
    ),
    (
        "the flag is cleared and the mode asks for a password",
        &[
            ("NoPasswordRequired", "false"),
            ("AuthenticationMode.Type", "password"),
            ("AuthenticationMode.Passwords.member.1", PASSWORD_TWO),
        ],
        "No password required flag is false but provided authentication type is password",
    ),
    (
        "passwords beside a mode that asks for no password",
        &[
            ("Passwords.member.1", PASSWORD_TWO),
            ("AuthenticationMode.Type", "no-password-required"),
        ],
        "Passwords are provided but provided authentication type is no-password-required",
    ),
    (
        "passwords beside a mode carrying passwords of its own",
        &[
            ("Passwords.member.1", PASSWORD_TWO),
            ("AuthenticationMode.Type", "password"),
            ("AuthenticationMode.Passwords.member.1", PASSWORD),
        ],
        "Passwords are provided but provided authentication type is password",
    ),
    (
        "passwords beside a mode that asks for IAM",
        &[
            ("Passwords.member.1", PASSWORD_TWO),
            ("AuthenticationMode.Type", "iam"),
        ],
        "Passwords are provided but provided authentication type is iam",
    ),
    (
        "both older members beside a mode",
        &[
            ("NoPasswordRequired", "true"),
            ("Passwords.member.1", PASSWORD_TWO),
            ("AuthenticationMode.Type", "iam"),
        ],
        "No password required flag is true but provided authentication type is iam",
    ),
];

/// The id of the user the modify rows run against.
const SEEDED_USER: &str = "auth-conflict-user";

/// Create the password user every modify row starts from.
async fn seed_user(emulator: &Emulator, id: &str) {
    let created = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", id),
                ("UserName", id),
                ("Engine", "redis"),
                ("AccessString", CREATED_ACCESS_STRING),
                ("Passwords.member.1", PASSWORD),
            ]),
        )
        .await;
    assert_eq!(
        (
            created.status,
            created.text("Authentication/Type"),
            created.text("Authentication/PasswordCount")
        ),
        (200, Some("password"), Some("1")),
        "the seeded user must be a password user with one password, got {} {}",
        created.status,
        created.body
    );
}

/// Assert `reply` is the modelled combination refusal carrying exactly `message`.
fn assert_conflict_refusal(case: &str, reply: &crate::harness::Reply, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "InvalidParameterCombination"),
        "case {case:?}: expected 400 InvalidParameterCombination, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case:?}: expected message {message:?}, got {:?}",
        reply.error_message()
    );
}

#[tokio::test]
async fn a_modify_naming_two_authentication_directives_reports_both_of_them() {
    let emulator = Emulator::new();
    seed_user(&emulator, SEEDED_USER).await;
    let stored_access_string = describe_user(&emulator, "seed", SEEDED_USER)
        .await
        .text("AccessString")
        .expect("the seeded user reports an access string")
        .to_string();

    for (case, members, message) in CONFLICT_CASES {
        let mut request = params(&[("UserId", SEEDED_USER)]);
        request.extend(params(members));
        let refused = emulator.call("ModifyUser", &request).await;
        assert_conflict_refusal(case, &refused, message);

        // The refusal is effect-free: the user still authenticates the way the create left
        // it, still has the rule list it was created with, and is still settled — so the
        // next row starts from the same user.
        let unchanged = describe_user(&emulator, case, SEEDED_USER).await;
        assert_eq!(
            (
                unchanged.text("Authentication/Type"),
                unchanged.text("Authentication/PasswordCount"),
                unchanged.text("AccessString"),
                unchanged.text("Status"),
            ),
            (
                Some("password"),
                Some("1"),
                Some(stored_access_string.as_str()),
                Some(state::USER_STATUS_ACTIVE),
            ),
            "case {case:?}: a refused modify must leave the user exactly as it was, got {}",
            unchanged.body
        );
    }

    // The positive control: a modify that states ONE thing about the same user is still
    // served, and the change the clock owes settles.
    let modified = emulator
        .call(
            "ModifyUser",
            &params(&[
                ("UserId", SEEDED_USER),
                ("AccessString", "on ~* -@all +@read"),
            ]),
        )
        .await;
    assert_eq!(
        (modified.status, modified.text("AccessString")),
        (200, Some("on ~* -@all +@read")),
        "an unopposed modify must still be served, got {} {}",
        modified.status,
        modified.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let settled = describe_user(&emulator, "positive control", SEEDED_USER).await;
    assert_eq!(
        (
            settled.text("Status"),
            settled.text("AccessString"),
            settled.text("Authentication/Type")
        ),
        (
            Some(state::USER_STATUS_ACTIVE),
            Some("on ~* -@all +@read"),
            Some("password")
        ),
        "the modified user must settle active carrying the change, got {}",
        settled.body
    );
}

#[tokio::test]
async fn a_create_naming_two_authentication_directives_reports_both_of_them() {
    let emulator = Emulator::new();
    // `CreateUser` and `ModifyUser` take the same authentication members, so the same
    // sentences are what a create is refused with — and the id it named stays free.
    for (index, (case, members, message)) in CONFLICT_CASES.iter().enumerate() {
        let id = format!("created-conflict-{index}");
        let mut request = params(&[
            ("UserId", id.as_str()),
            ("UserName", id.as_str()),
            ("Engine", "redis"),
            ("AccessString", CREATED_ACCESS_STRING),
        ]);
        request.extend(params(members));
        let refused = emulator.call("CreateUser", &request).await;
        assert_conflict_refusal(case, &refused, message);

        let described = emulator
            .call("DescribeUsers", &params(&[("UserId", id.as_str())]))
            .await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "UserNotFound"),
            "case {case:?}: a refused create must leave the id unused, got {} {}",
            described.status,
            described.body
        );
    }
}
