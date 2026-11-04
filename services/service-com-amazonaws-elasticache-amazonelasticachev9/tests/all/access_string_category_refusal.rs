// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH of its two refusals an `AccessString` gets, and for the
//! wording of each — on the one authorization surface ElastiCache has, the message is the
//! whole diagnosis a client is given.
//!
//! The rule under test, over a SET of inputs rather than the one recorded request: a rule
//! list that names a command CATEGORY the engine does not publish (`+@<name>` /
//! `-@<name>`) is refused with HTTP 400, the modelled `InvalidParameterValue`, and the
//! sentence that says so and sends the caller to the published catalog; an input that is
//! not rule-list syntax at all keeps the generic sentence
//! ([`crate::access_string_refusal_wording`] owns that class). Both are asserted here
//! together, because the whole point of the rule is that the two are told apart.
//!
//! The refused rows vary along what a category rule can be wrong about (a name of letters
//! the catalog does not carry, a name carrying digits, one carrying an underscore, a name
//! that is a near-miss of a published category, a published name with a typo, an empty
//! name), along the SIGN of the rule (`+@` grants, `-@` revokes), along WHERE in the list
//! the rule sits (first, last, inside a selector) and along the ENTRY POINT the list
//! arrives through (`CreateUser`, `ModifyUser$AccessString`,
//! `ModifyUser$AppendAccessString`).
//!
//! The accepted rows are the counterpart, so the refusal above cannot be passing by
//! refusing every category rule: each names a category the engine DOES publish —
//! including ones no issue cited (`@keyspace`, `@hyperloglog`, `@scripting`) and one
//! spelled in another case — and is read back through `DescribeUsers`.
//!
//! Every assertion goes over the wire through [`crate::harness`]. Both sentences are
//! written out here on purpose: a test that built them from the same module the handlers
//! call would pass whatever that module said.

use crate::access_string_refusal_wording::{create_user, seed_user};
use crate::harness::{params, Emulator, Reply};
use crate::modify_user::describe_user;

/// The sentence ElastiCache refuses a rule list naming a category it does not publish
/// with.
const UNKNOWN_CATEGORY: &str = "Access string contains invalid category(s). Please verify the \
                                list of available category in user-api.";

/// The sentence it refuses an input that is not a rule list at all with.
const NOT_A_RULE_LIST: &str = "The access-string is invalid.";

/// The rule list the seeded user holds, in the expanded form the service stores.
const STORED_ACCESS_STRING: &str = "on ~app:* -@all +@read";

/// Assert `reply` is the modelled parameter-value refusal worded exactly `message`.
fn assert_refused(case: &str, entry: &str, reply: &Reply, message: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "InvalidParameterValue"),
        "case {case:?} through {entry}: expected 400 InvalidParameterValue, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        message,
        "case {case:?} through {entry}: expected message {message:?}, got {:?}",
        reply.error_message()
    );
}

/// One refused row: the case name, the access string, and the sentence it must be refused
/// with.
type RefusedRow = (&'static str, &'static str, &'static str);

/// The rows whose refusal is the CATEGORY one, and the rows that keep the generic sentence
/// beside them.
const REFUSED: &[RefusedRow] = &[
    (
        "a category of letters the catalog does not carry",
        "on ~app:* +@notacategory",
        UNKNOWN_CATEGORY,
    ),
    (
        "a category name carrying digits",
        "on ~app:* +@read2write",
        UNKNOWN_CATEGORY,
    ),
    (
        "a category name carrying an underscore",
        "on ~* +@read_write",
        UNKNOWN_CATEGORY,
    ),
    (
        "a published category with a typo",
        "on ~* +@sortedsets",
        UNKNOWN_CATEGORY,
    ),
    (
        "an unpublished category REVOKED rather than granted",
        "on ~* -@nosuchcategory +@read",
        UNKNOWN_CATEGORY,
    ),
    (
        "an unpublished category as the last rule of the list",
        "on ~* +@read &news:* +@nosuchcategory",
        UNKNOWN_CATEGORY,
    ),
    (
        "an unpublished category inside a selector",
        "on ~* (+@nosuchcategory ~key1) +@read",
        UNKNOWN_CATEGORY,
    ),
    (
        "a category rule naming no category at all",
        "on ~* +@",
        UNKNOWN_CATEGORY,
    ),
    (
        "free text that is no rule list at all",
        "THIS IS NOT AN ACCESS STRING",
        NOT_A_RULE_LIST,
    ),
    (
        "rules the grammar admits followed by a word that is no rule",
        "on ~* +@read not-a-real-acl-token",
        NOT_A_RULE_LIST,
    ),
    (
        "a key pattern with no glob, beside a published category",
        "on ~ +@read",
        NOT_A_RULE_LIST,
    ),
];

/// One accepted row: the case name, the list as sent, and the EXPANDED list the service
/// stores and reports for it.
const ACCEPTED: &[(&str, &str, &str)] = &[
    (
        "a published category no issue cited",
        "on ~* +@keyspace",
        "on ~* -@all +@keyspace",
    ),
    (
        "two more published categories, one revoked",
        "on ~* +@hyperloglog -@scripting",
        "on ~* -@all +@hyperloglog -@scripting",
    ),
    (
        "a published category spelled in another case",
        "on ~* +@READ",
        "on ~* -@all +@READ",
    ),
    (
        "a published category inside a selector",
        "on ~* (+@geo ~key1) +@read",
        "on ~* (+@geo ~key1) -@all +@read",
    ),
    (
        "the baseline category, which states the baseline itself",
        "on ~* +@all",
        "on ~* +@all",
    ),
];

#[tokio::test]
async fn an_unpublished_category_is_diagnosed_as_one_and_nothing_else_is() {
    let emulator = Emulator::new();
    // ONE user for every row, so a row that applied part of its refused list would be
    // caught by the next row's assertion too.
    let seeded = "category-user";
    seed_user(&emulator, seeded).await;

    for (index, (case, access_string, message)) in REFUSED.iter().enumerate() {
        // A create names a user of its own, so a refusal cannot be the id being taken.
        let created_id = format!("category-created-{index}");
        let created = create_user(&emulator, &created_id, access_string).await;
        assert_refused(case, "CreateUser", &created, message);
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
            assert_refused(case, entry, &modified, message);
            // Nothing of the refused list was applied: the stored rule list is
            // byte-identical afterwards and the user is still the one a further change
            // is accepted on.
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
async fn a_published_category_is_accepted_and_read_back() {
    let emulator = Emulator::new();

    for (index, (case, sent, stored)) in ACCEPTED.iter().enumerate() {
        let created_id = format!("category-accepted-{index}");
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
    }
}
