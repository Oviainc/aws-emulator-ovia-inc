// SPDX-License-Identifier: CC0-1.0

//! `DeleteUser` against the user the SERVICE owns, over the wire.
//!
//! The rule under test: a `UserId` resolves to one stored user — ids are stored
//! lowercased — and the built-in user ElastiCache gives every account is not a user any
//! call deletes, so a `DeleteUser` that resolves to it is refused with the modelled
//! fault for the state the account is in, persists nothing (no status change, no event),
//! and leaves the user reported exactly as it was stocked. The refusal is a property of
//! the user the id resolves to, so it does not depend on how the request spelled the id,
//! on what else the account holds, or on how often it is asked.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, so a reply that is not computed from persisted state cannot pass. The request
//! builder and the XML reader are the shared ones in `tests/all/harness.rs`.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply};

/// The wording a refusal about the built-in user carries: the user it is about, and
/// nothing the request chose. Built from the same published constant the handler reads,
/// so a test row cannot pin a spelling of the id the service does not hold.
fn immutable_user_message() -> String {
    format!("{} user can't be deleted.", state::DEFAULT_USER_ID)
}

/// `DeleteUser` for `id`.
async fn delete_user(emulator: &Emulator, id: &str) -> Reply {
    emulator.call("DeleteUser", &params(&[("UserId", id)])).await
}

/// Read one user back through the implemented reader.
async fn describe_user(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .call("DescribeUsers", &params(&[("UserId", id)]))
        .await
}

/// Create one RBAC user, asserting the create itself succeeded.
async fn create_user(emulator: &Emulator, case: &str, id: &str) {
    let created = emulator
        .call(
            "CreateUser",
            &params(&[
                ("UserId", id),
                ("UserName", id),
                ("Engine", "valkey"),
                ("AccessString", "on ~* +@all"),
                ("NoPasswordRequired", "true"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case}: CreateUser {id} must answer 200, got {} {}",
        created.status, created.body
    );
}

/// Assert the built-in user is reported exactly as `stocked` reported it, and that the
/// unfiltered listing still carries it.
async fn assert_untouched(emulator: &Emulator, case: &str, stocked: &Reply) {
    let now = describe_user(emulator, state::DEFAULT_USER_ID).await;
    assert_eq!(
        now.status, 200,
        "case {case}: the built-in user must still be readable, got {} {}",
        now.status, now.body
    );
    for member in ["UserId", "UserName", "Engine", "AccessString", "ARN", "Status"] {
        assert_eq!(
            now.text(member),
            stocked.text(member),
            "case {case}: a refused delete must leave {member} as it was ({:?}), got {:?}",
            stocked.text(member),
            now.text(member)
        );
    }
    assert_eq!(
        now.text("Status"),
        Some(state::USER_STATUS_ACTIVE),
        "case {case}: the built-in user must stay {:?}, got {:?}",
        state::USER_STATUS_ACTIVE,
        now.text("Status")
    );
    let listed = emulator.call("DescribeUsers", &params(&[])).await;
    assert!(
        listed.all("UserId").contains(&state::DEFAULT_USER_ID),
        "case {case}: the built-in user must still be listed, got {}",
        listed.body
    );
}

/// The spellings axis: `UserId` "is stored as a lowercase string", so each of these
/// resolves to the one built-in user and must be answered identically — including the
/// wording, which therefore names no spelling the request chose.
const SPELLINGS: &[&str] = &["default", "DEFAULT", "Default", "dEfAuLt", "defaulT"];

#[tokio::test]
async fn the_built_in_user_is_refused_however_the_request_spells_its_id() {
    let emulator = Emulator::new();
    let stocked = describe_user(&emulator, state::DEFAULT_USER_ID).await;
    assert_eq!(
        stocked.status, 200,
        "the account must be stocked with the built-in user, got {} {}",
        stocked.status, stocked.body
    );

    for spelling in SPELLINGS {
        // Asked twice, so a refusal cannot be a one-off that a second call gets past.
        for attempt in 1..=2 {
            let case = format!("{spelling:?}, attempt {attempt}");
            let refused = delete_user(&emulator, spelling).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (400, "InvalidParameterValue"),
                "case {case}: expected 400 InvalidParameterValue, got {} {}",
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.error_message(),
                immutable_user_message(),
                "case {case}: expected the message {:?}, got {:?}",
                immutable_user_message(),
                refused.error_message()
            );
            assert_untouched(&emulator, &case, &stocked).await;
        }
    }

    // Nothing happened to the account either: a refused delete writes no event, so the
    // log about the built-in user is still empty.
    let events = emulator
        .call(
            "DescribeEvents",
            &params(&[("SourceIdentifier", state::DEFAULT_USER_ID)]),
        )
        .await;
    assert_eq!(
        events.status, 200,
        "DescribeEvents must answer 200, got {} {}",
        events.status, events.body
    );
    assert!(
        events.all("Event/SourceIdentifier").is_empty(),
        "a refused delete must record no event, got {}",
        events.body
    );
}

/// One account-state row: the case, the users to create first, the users to delete
/// first, whether a user group holding the built-in user exists, and the status plus
/// `awsQuery` error code the delete of the built-in user must be answered with.
type AccountCase = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
    bool,
    u16,
    &'static str,
);

/// The account-state axis: what else the account holds when the built-in user is asked
/// for. A group that lists it is the one row with a different answer — the modelled
/// `DefaultUserAssociatedToUserGroupFault`, "the default user assigned to the user
/// group" — because that is the state the account is in, not a different rule about the
/// user.
const ACCOUNT_CASES: &[AccountCase] = &[
    ("a freshly stocked account", &[], &[], false, 400, "InvalidParameterValue"),
    (
        "an account holding other users",
        &["other-one", "other-two"],
        &[],
        false,
        400,
        "InvalidParameterValue",
    ),
    (
        "an account where another user has already been deleted",
        &["other-one"],
        &["other-one"],
        false,
        400,
        "InvalidParameterValue",
    ),
    (
        "an account whose user group lists the built-in user",
        &["other-one"],
        &[],
        true,
        400,
        "DefaultUserAssociatedToUserGroup",
    ),
];

#[tokio::test]
async fn the_built_in_user_survives_whatever_else_the_account_holds() {
    for (case, create, delete, grouped, status, code) in ACCOUNT_CASES {
        // One emulator per row, so each row's account is exactly what it describes.
        let emulator = Emulator::new();
        let stocked = describe_user(&emulator, state::DEFAULT_USER_ID).await;
        for id in *create {
            create_user(&emulator, case, id).await;
        }
        if *grouped {
            let group = emulator
                .call(
                    "CreateUserGroup",
                    &params(&[
                        ("UserGroupId", "holding-group"),
                        ("Engine", "valkey"),
                        ("UserIds.member.1", state::DEFAULT_USER_ID),
                        ("UserIds.member.2", "other-one"),
                    ]),
                )
                .await;
            assert_eq!(
                group.status, 200,
                "case {case}: CreateUserGroup must answer 200, got {} {}",
                group.status, group.body
            );
        }
        for id in *delete {
            let deleted = delete_user(&emulator, id).await;
            assert_eq!(
                deleted.status, 200,
                "case {case}: deleting {id} must answer 200, got {} {}",
                deleted.status, deleted.body
            );
        }

        let refused = delete_user(&emulator, state::DEFAULT_USER_ID).await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (*status, *code),
            "case {case}: expected {status} {code}, got {} {}",
            refused.status,
            refused.body
        );
        assert_untouched(&emulator, case, &stocked).await;
        // The users the row created and did not delete are untouched too: the refusal is
        // about the one user it named.
        for id in create.iter().filter(|id| !delete.contains(id)) {
            let other = describe_user(&emulator, id).await;
            assert_eq!(
                (other.status, other.text("Status")),
                (200, Some(state::USER_STATUS_ACTIVE)),
                "case {case}: user {id} must still be active, got {} {}",
                other.status,
                other.body
            );
        }
    }
}
