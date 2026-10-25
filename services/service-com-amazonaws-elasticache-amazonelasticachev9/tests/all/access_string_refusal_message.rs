// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the access-string refusal MESSAGE, where
//! [`crate::access_string_refusal_wording`] stops: the stored-length bound a rule list is
//! refused at, what the refusal does NOT carry, and what the user takes after one.
//!
//! The rule under test is the same one: a rule list this service cannot accept is refused
//! with HTTP 400, the modelled `InvalidParameterValue`, and ONE fixed sentence that is a
//! constant of the condition. Here it is asserted along the axes the wording suite does
//! not walk:
//!
//! * the LENGTH axis — the stored rule list is the one thing a caller can grow from call
//!   to call, so a result at the bound the service keeps is accepted and one past it is
//!   refused in exactly those words, through both the member that states a whole list and
//!   the member that adds to one;
//! * what the sentence does NOT carry — for a refused list, the message holds no part of
//!   the request: not the rule that offended, not the member it arrived on, not the user
//!   it was sent for;
//! * what follows a refusal — the stored list is still the pre-call one, the next list the
//!   grammar admits is accepted and converges past the span, and once the user is deleted
//!   both the read and the write answer the modelled not-found for it.
//!
//! Every assertion goes over the wire through [`crate::harness`]. The expected sentence is
//! written out here on purpose: a test that read it from the constant the handlers render
//! would pass whatever that constant said.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::access_string_refusal_wording::{
    assert_refused, create_user, seed_user, STORED_ACCESS_STRING,
};
use crate::harness::{params, Emulator, PAST_TRANSITION};
use crate::modify_user::describe_user;

/// The sentence ElastiCache refuses an access string that is not a rule list with.
const ACCESS_STRING_RULE: &str = "The access-string is invalid.";

/// A rule list the grammar admits, for the leg that asserts a refusal is followed by an
/// accepted change.
const NEXT_ACCESS_STRING: &str = "on ~* +@all";

/// One key pattern whose glob carries `len` characters in total, so a row's axis is the
/// LENGTH of the rule list and not the rules it is made of.
fn pattern_of_length(len: usize) -> String {
    format!("~{}", "k".repeat(len - 1))
}

#[tokio::test]
async fn the_stored_length_bound_is_refused_in_the_services_own_words() {
    let emulator = Emulator::new();
    // The room an append has on the seeded user: the bound, less the list the user already
    // holds and the space that joins the two.
    let room = state::MAX_ACCESS_STRING_LEN - STORED_ACCESS_STRING.chars().count() - 1;
    // `(case, the length of the appended pattern, the status, the error code)`: the row at
    // the bound is accepted, and the rows past it — by one character and by a long way —
    // are refused.
    for (index, (case, appended_len, status, code)) in [
        ("a result exactly at the bound", room, 200, ""),
        (
            "a result one character past the bound",
            room + 1,
            400,
            "InvalidParameterValue",
        ),
        (
            "a result far past the bound",
            room + state::MAX_ACCESS_STRING_LEN,
            400,
            "InvalidParameterValue",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        // One user per row: an accepted append leaves the user transiently `modifying`,
        // so a later row would otherwise be judged on the state rather than on its list.
        let id = format!("bounded-user-{index}");
        seed_user(&emulator, &id).await;
        let appended = pattern_of_length(appended_len);
        let reply = emulator
            .call(
                "ModifyUser",
                &params(&[("UserId", id.as_str()), ("AppendAccessString", &appended)]),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (status, code),
            "case {case:?}: expected {status} {code}, got {} {}",
            reply.status,
            reply.body
        );
        let expected = if status == 200 {
            format!("{STORED_ACCESS_STRING} {appended}")
        } else {
            // A refused append applies nothing, so the stored list is the pre-call one.
            assert_refused(case, "ModifyUser$AppendAccessString", &reply);
            STORED_ACCESS_STRING.to_string()
        };
        let stored = describe_user(&emulator, case, &id).await;
        assert_eq!(
            stored.text("AccessString"),
            Some(expected.as_str()),
            "case {case:?}: the stored rule list must be {} characters, got {:?}",
            expected.chars().count(),
            stored.text("AccessString").map(|list| list.chars().count())
        );
    }

    // The same bound on the member that states a WHOLE list, through both writers: a list
    // one character past the bound is refused in the same words, and nothing is stored.
    let past_bound = pattern_of_length(state::MAX_ACCESS_STRING_LEN + 1);
    let created = create_user(&emulator, "bounded-create-user", &past_bound).await;
    assert_refused("a created list past the bound", "CreateUser", &created);
    let absent = emulator
        .call("DescribeUsers", &params(&[("UserId", "bounded-create-user")]))
        .await;
    assert_eq!(
        (absent.status, absent.error_code()),
        (404, "UserNotFound"),
        "a refused create must store no user, got {} {}",
        absent.status,
        absent.body
    );
    seed_user(&emulator, "bounded-modify-user").await;
    let modified = emulator
        .call(
            "ModifyUser",
            &params(&[
                ("UserId", "bounded-modify-user"),
                ("AccessString", &past_bound),
            ]),
        )
        .await;
    assert_refused(
        "a stated list past the bound",
        "ModifyUser$AccessString",
        &modified,
    );
}

#[tokio::test]
async fn the_refusal_reports_no_part_of_the_request() {
    let emulator = Emulator::new();
    let user = "nothing-echoed-user";
    seed_user(&emulator, user).await;

    // `(case, the list sent, the part of it a paraphrase would quote)`. The rows differ in
    // WHERE the offending rule sits in the list and in what kind of rule it is, so the
    // check is not tied to one token.
    for (case, access_string, offending) in [
        ("an unknown rule at the front", "bogus-rule on ~*", "bogus-rule"),
        (
            "an unknown rule after rules the grammar admits",
            "on ~* &news:* not-a-real-acl-token",
            "not-a-real-acl-token",
        ),
        ("pure punctuation", "on %%%garbage%%%", "%%%garbage%%%"),
        ("a selector never closed", "on ~* (+get ~key1", "(+get ~key1"),
    ] {
        for (entry, member) in [
            ("ModifyUser$AccessString", "AccessString"),
            ("ModifyUser$AppendAccessString", "AppendAccessString"),
        ] {
            let refused = emulator
                .call(
                    "ModifyUser",
                    &params(&[("UserId", user), (member, access_string)]),
                )
                .await;
            assert_refused(case, entry, &refused);
            // The sentence is a constant of the condition: a client reading it learns
            // only that the access string was not accepted.
            for part in [offending, member, user] {
                assert!(
                    !refused.error_message().contains(part),
                    "case {case:?} through {entry}: the refusal must report no part of the \
                     request, but {part:?} is in {:?}",
                    refused.error_message()
                );
            }
            assert_eq!(
                refused.error_message(),
                ACCESS_STRING_RULE,
                "case {case:?} through {entry}: expected {ACCESS_STRING_RULE:?}, got {:?}",
                refused.error_message()
            );
        }
    }
}

#[tokio::test]
async fn a_user_whose_access_string_was_refused_takes_the_next_one() {
    let emulator = Emulator::new();
    let user = "recovering-user";
    seed_user(&emulator, user).await;

    let refused = emulator
        .call(
            "ModifyUser",
            &params(&[("UserId", user), ("AccessString", "not-a-real-acl-token")]),
        )
        .await;
    assert_refused("a list the grammar admits none of", "ModifyUser", &refused);
    // The refusal applied nothing and moved no status, so the next change is judged on
    // its own rule list rather than on the user's state.
    let unchanged = describe_user(&emulator, "after the refusal", user).await;
    assert_eq!(
        (unchanged.text("AccessString"), unchanged.text("Status")),
        (
            Some(STORED_ACCESS_STRING),
            Some(state::USER_STATUS_ACTIVE)
        ),
        "a refused modify must leave the user as it was, got {}",
        unchanged.body
    );

    let accepted = emulator
        .call(
            "ModifyUser",
            &params(&[("UserId", user), ("AccessString", NEXT_ACCESS_STRING)]),
        )
        .await;
    assert_eq!(
        (accepted.status, accepted.text("AccessString")),
        (200, Some(NEXT_ACCESS_STRING)),
        "the next list must be accepted, got {} {}",
        accepted.status,
        accepted.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let settled = describe_user(&emulator, "converged", user).await;
    assert_eq!(
        (settled.text("AccessString"), settled.text("Status")),
        (Some(NEXT_ACCESS_STRING), Some(state::USER_STATUS_ACTIVE)),
        "the accepted change must converge at {:?}, got {}",
        state::USER_STATUS_ACTIVE,
        settled.body
    );

    // Once the user is gone, the read and the write both answer the modelled not-found
    // for the id the request named.
    let deleted = emulator
        .call("DeleteUser", &params(&[("UserId", user)]))
        .await;
    assert_eq!(
        deleted.status, 200,
        "the settled user must be deletable, got {} {}",
        deleted.status, deleted.body
    );
    std::thread::sleep(PAST_TRANSITION);
    let gone_message = format!("User {user} not found.");
    for (action, request) in [
        ("DescribeUsers", params(&[("UserId", user)])),
        (
            "ModifyUser",
            params(&[("UserId", user), ("AccessString", NEXT_ACCESS_STRING)]),
        ),
    ] {
        let gone = emulator.call(action, &request).await;
        assert_eq!(
            (gone.status, gone.error_code(), gone.error_message()),
            (404, "UserNotFound", gone_message.as_str()),
            "case {action:?}: a deleted user must answer the modelled not-found, got {} {}",
            gone.status,
            gone.body
        );
    }
}
