// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal of an access string that is not a rule list the
//! engine can read, and for its WORDING — the thing a client that logs or matches the
//! refusal sees.
//!
//! The rule under test, over a SET of inputs rather than one request: an `AccessString`
//! (or the rules an `AppendAccessString` adds) that is not a rule list of the engine's ACL
//! grammar is refused with HTTP 400 and the modelled `InvalidParameterValue`, worded as
//! the service words it — ONE fixed sentence that names neither the member that carried
//! the list nor the token that is not a rule — and nothing of the request is applied.
//! Its counterpart is checked too: a list the grammar DOES admit is accepted and read
//! back, so the refusal above cannot be passing by refusing everything.
//!
//! The refused rows vary along WHAT is wrong with the list (a word that is no rule, a
//! near-miss keyword, a pattern with no glob, a qualifier outside the published letters, a
//! selector that is never closed, one that is empty, one whose inner rule is no rule) and
//! along the ENTRY POINT the list arrives through (`CreateUser`,
//! `ModifyUser$AccessString`, `ModifyUser$AppendAccessString`), so no single recorded
//! request is what makes them pass.
//!
//! A rule list whose syntax reads as one but which names a command CATEGORY the engine
//! does not publish is the service's OTHER refusal, worded its own way, and belongs to
//! [`crate::access_string_category_refusal`] rather than here.
//!
//! Every assertion goes over the wire through [`crate::harness`], and each accepted row is
//! read back through `DescribeUsers`. The expected sentence is written out here on
//! purpose: a test that built it from the same helper the handlers call would pass
//! whatever that helper said.

use crate::harness::{params, Emulator, Reply};
use crate::modify_user::describe_user;

/// The sentence ElastiCache refuses an access string that is not a rule list with.
const ACCESS_STRING_RULE: &str = "The access-string is invalid.";

/// The rule list the seeded user starts from, and the EXPANDED form the service stores for
/// it — the list with its command baseline stated outright.
///
/// The stored form is `pub(crate)` for the same reason as the helpers below: the suite
/// about the stored-length bound and what a user takes after a refusal starts from this
/// same seeded user.
const SEED_ACCESS_STRING: &str = "on ~app:* +@read";
pub(crate) const STORED_ACCESS_STRING: &str = "on ~app:* -@all +@read";

/// The lists that are NOT rule lists: the case name and the access string.
///
/// Each row breaks the grammar `CreateUser$AccessString` is written in ("Access
/// permissions string used for this user", the engine's own ACL syntax) in a different
/// way; the rule forms they are near-misses of are the ones the engine publishes
/// (keywords, `~`/`%R~`/`&` patterns, `+`/`-` command and `+@`/`-@` category rules, and a
/// parenthesised selector).
const NOT_RULE_LISTS: &[(&str, &str)] = &[
    ("a sentence of words, none of them a rule", "this is not an acl"),
    (
        "rules followed by punctuation and a word that is no rule",
        "on ~* &*&*&* not-a-real-acl-token %%%garbage%%%",
    ),
    ("a keyword that is one letter off", "onn ~* +@all"),
    ("a key pattern with no glob after it", "on ~ +@all"),
    (
        "a read/write qualifier outside the published letters",
        "on %X~app:* +@all",
    ),
    ("a selector that is never closed", "on ~* (+get ~key1"),
    ("a selector with no rule in it", "on ~* () +@read"),
    ("a selector whose inner rule is no rule", "on ~* (+get bogus)"),
];

/// One accepted row: the case name, the list as sent, and the EXPANDED list the service
/// stores and reports for it.
///
/// The rows span the rule forms the grammar admits, and both branches of the baseline
/// rule: a list that states its command baseline is stored as sent, and one that does not
/// is stored with the deny-all term before its first command rule (at the end of a list
/// that names none).
const RULE_LISTS: &[(&str, &str, &str)] = &[
    ("one keyword and no command rule at all", "on", "on -@all"),
    (
        "a list that states its own baseline",
        "on ~* +@all",
        "on ~* +@all",
    ),
    (
        "a key pattern and a category grant",
        "on ~app:* +@read",
        "on ~app:* -@all +@read",
    ),
    (
        "a qualified key pattern and a channel pattern",
        "on %RW~app:* &news:* +@read",
        "on %RW~app:* &news:* -@all +@read",
    ),
    (
        "a selector, which is one rule of the list it sits in",
        "on ~* (+get ~key1) +@read",
        "on ~* (+get ~key1) -@all +@read",
    ),
    (
        "a command rule naming one subcommand",
        "on ~* +config|get",
        "on ~* -@all +config|get",
    ),
    (
        "a keyword baseline alias, which states the baseline too",
        "off nocommands ~*",
        "off nocommands ~*",
    ),
];

/// `CreateUser` for `id`, with `access_string` as the whole rule list.
///
/// `pub(crate)`, like [`seed_user`] and [`assert_refused`] below, so the suite about the
/// stored-length bound and the recovery after a refusal drives the same create, seed and
/// refusal check rather than carrying a second copy of each.
pub(crate) async fn create_user(emulator: &Emulator, id: &str, access_string: &str) -> Reply {
    emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", id),
                ("UserName", id),
                ("Engine", "redis"),
                ("AccessString", access_string),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await
}

/// Create the user the modify rows change, asserting it starts from
/// [`STORED_ACCESS_STRING`].
pub(crate) async fn seed_user(emulator: &Emulator, id: &str) {
    let created = create_user(emulator, id, SEED_ACCESS_STRING).await;
    assert_eq!(
        (created.status, created.text("AccessString")),
        (200, Some(STORED_ACCESS_STRING)),
        "seeding {id}: CreateUser must answer 200 with {STORED_ACCESS_STRING:?}, got {} {}",
        created.status,
        created.body
    );
}

/// Assert `reply` is the modelled access-string refusal, worded exactly.
pub(crate) fn assert_refused(case: &str, entry: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "InvalidParameterValue"),
        "case {case:?} through {entry}: expected 400 InvalidParameterValue, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        ACCESS_STRING_RULE,
        "case {case:?} through {entry}: expected message {ACCESS_STRING_RULE:?}, got {:?}",
        reply.error_message()
    );
}

#[tokio::test]
async fn a_list_that_is_not_an_acl_rule_list_is_refused_with_one_sentence() {
    let emulator = Emulator::new();
    // The user the two modify entry points change: ONE user for every row, so a row that
    // applied part of its refused list would be caught by the next row's assertion too.
    let seeded = "wording-user";
    seed_user(&emulator, seeded).await;

    for (index, (case, access_string)) in NOT_RULE_LISTS.iter().enumerate() {
        // A create names a user of its own, so a refusal cannot be the id being taken.
        let created_id = format!("created-user-{index}");
        let created = create_user(&emulator, &created_id, access_string).await;
        assert_refused(case, "CreateUser", &created);
        // Nothing was applied: the create stored no user at all.
        let read_back = emulator
            .call("DescribeUsers", &params(&[("UserId", created_id.as_str())]))
            .await;
        assert_eq!(
            (read_back.status, read_back.error_code()),
            (404, "UserNotFound"),
            "case {case:?}: a refused create must store no user, got {} {}",
            read_back.status,
            read_back.body
        );

        for (entry, member) in [
            ("ModifyUser$AccessString", "AccessString"),
            ("ModifyUser$AppendAccessString", "AppendAccessString"),
        ] {
            let modified = emulator
                .call(
                    "ModifyUser",
                    &params(&[("UserId", seeded), (member, access_string)]),
                )
                .await;
            assert_refused(case, entry, &modified);
            // No partial application: the stored rule list is byte-identical afterwards,
            // and the user is still the one a further change is accepted on.
            let stored = describe_user(&emulator, case, seeded).await;
            assert_eq!(
                (stored.text("AccessString"), stored.text("Status")),
                (Some(STORED_ACCESS_STRING), Some("active")),
                "case {case:?} through {entry}: the user must be unchanged and active, got {}",
                stored.body
            );
        }
    }
}

#[tokio::test]
async fn a_list_the_grammar_admits_is_accepted_and_read_back() {
    let emulator = Emulator::new();

    for (index, (case, sent, stored)) in RULE_LISTS.iter().enumerate() {
        let created_id = format!("accepted-user-{index}");
        let created = create_user(&emulator, &created_id, sent).await;
        assert_eq!(
            (created.status, created.text("AccessString")),
            (200, Some(*stored)),
            "case {case:?} through CreateUser: expected 200 with {stored:?}, got {} {}",
            created.status,
            created.body
        );
        let read_back = describe_user(&emulator, case, &created_id).await;
        assert_eq!(
            read_back.text("AccessString"),
            Some(*stored),
            "case {case:?}: the created user must report {stored:?}, got {:?}",
            read_back.text("AccessString")
        );

        // A user of its own for the modify leg: a modify leaves a user transiently
        // `modifying`, so a second row's change would be refused for the state rather
        // than judged on its rule list.
        let modified_id = format!("modified-user-{index}");
        seed_user(&emulator, &modified_id).await;
        let modified = emulator
            .call(
                "ModifyUser",
                &params(&[("UserId", modified_id.as_str()), ("AccessString", sent)]),
            )
            .await;
        assert_eq!(
            (modified.status, modified.text("AccessString")),
            (200, Some(*stored)),
            "case {case:?} through ModifyUser: expected 200 with {stored:?}, got {} {}",
            modified.status,
            modified.body
        );
        let read_back = describe_user(&emulator, case, &modified_id).await;
        assert_eq!(
            read_back.text("AccessString"),
            Some(*stored),
            "case {case:?}: the modified user must report {stored:?}, got {:?}",
            read_back.text("AccessString")
        );
    }
}
