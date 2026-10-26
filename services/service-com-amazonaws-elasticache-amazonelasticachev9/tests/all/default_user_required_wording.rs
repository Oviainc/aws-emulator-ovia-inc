// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the `DefaultUserRequired` refusal, over the wire, at every entry point
//! that raises it.
//!
//! The rule under test: a call that would leave a user group without a member whose user
//! name is the built-in one is refused with `DefaultUserRequired` (HTTP 400, awsQuery code
//! `DefaultUserRequired`) carrying the service's ONE sentence for that error class — fixed
//! text, with no part of the request and no part of the store spliced into it — and it
//! changes nothing. The same sentence answers both writers of a membership: a
//! `CreateUserGroup` whose `UserIds` does not name that user, and a `ModifyUserGroup` whose
//! `UserIdsToRemove` does.
//!
//! The rows differ along what the rule generalises over — which writer is called, how many
//! members the group has, how many ids the call names, where the built-in user sits in the
//! list, the case it is written in, whether an addition rides along, and the engine the
//! group is listed for — and the recorded `UserIdsToRemove=['default']` case is a single
//! row.
//!
//! The expected sentence is spelled out here on purpose: a test that read it from the
//! constant the handlers share would pass whatever that constant happened to say.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply};
use crate::user_group_membership::{
    create_user, describe_user_group, modify_user_group, wait_out_the_transition,
};

/// The sentence ElastiCache refuses a user group without the built-in user with. Fixed
/// text for the whole error class: no group id, no user id, no timestamp.
const DEFAULT_USER_REQUIRED: &str =
    "Redis user group needs to contain a user with the user name default.";

/// The engine the users these rows create are created for, and the default for a group.
const START_ENGINE: &str = "redis";

/// `CreateUserGroup` for `id` out of `members`, whatever it answers.
async fn create_user_group(
    emulator: &Emulator,
    id: &str,
    engine: &str,
    members: &[&str],
) -> Reply {
    let mut request = params(&[("UserGroupId", id), ("Engine", engine)]);
    for (index, member) in members.iter().enumerate() {
        request.push((format!("UserIds.member.{}", index + 1), (*member).to_string()));
    }
    emulator.call("CreateUserGroup", &request).await
}

/// Assert `reply` is the `DefaultUserRequired` refusal carrying exactly the service's
/// sentence, and that the sentence says nothing about `request_values`.
fn assert_default_user_required(case: &str, reply: &Reply, request_values: &[&str]) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "DefaultUserRequired"),
        "case {case:?}: expected 400 DefaultUserRequired, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        DEFAULT_USER_REQUIRED,
        "case {case:?}: expected message {DEFAULT_USER_REQUIRED:?}, got {:?}",
        reply.error_message()
    );
    for value in request_values {
        assert!(
            !reply.error_message().contains(value),
            "case {case:?}: the sentence is fixed text, so it must not carry the request's \
             {value:?}, got {:?}",
            reply.error_message()
        );
    }
}

/// One `ModifyUserGroup` row: the members the group starts with, and the change that would
/// leave it without the built-in user.
struct RemovalCase {
    case: &'static str,
    /// The engine the group is listed for, from the published set (`state::USER_ENGINES`).
    engine: &'static str,
    start: &'static [&'static str],
    add: &'static [&'static str],
    remove: &'static [&'static str],
}

/// The axis: the size of the membership, the number of ids removed, the position of the
/// built-in user in that list, the case it is written in, an addition riding along, and
/// the engine.
const REMOVAL_CASES: &[RemovalCase] = &[
    RemovalCase {
        case: "the built-in user alone",
        engine: START_ENGINE,
        start: &["default", "member-one"],
        add: &[],
        remove: &["default"],
    },
    RemovalCase {
        case: "the only member, the built-in user",
        engine: START_ENGINE,
        start: &["default"],
        add: &[],
        remove: &["default"],
    },
    RemovalCase {
        case: "the built-in user after another member",
        engine: START_ENGINE,
        start: &["default", "member-one", "member-two"],
        add: &[],
        remove: &["member-one", "default"],
    },
    RemovalCase {
        case: "the built-in user before another member",
        engine: "valkey",
        start: &["default", "member-one", "member-two"],
        add: &[],
        remove: &["default", "member-two"],
    },
    RemovalCase {
        case: "the built-in user in mixed case",
        // "UserId is stored as a lowercase string", so every spelling names it.
        engine: START_ENGINE,
        start: &["default", "member-one"],
        add: &[],
        remove: &["Default"],
    },
    RemovalCase {
        case: "the built-in user in upper case",
        engine: START_ENGINE,
        start: &["default", "member-one"],
        add: &[],
        remove: &["DEFAULT"],
    },
    RemovalCase {
        case: "the built-in user removed while another is added",
        engine: START_ENGINE,
        start: &["default", "member-one"],
        add: &["member-two"],
        remove: &["default"],
    },
];

#[tokio::test]
async fn removing_the_built_in_user_is_refused_with_the_services_sentence() {
    for row in REMOVAL_CASES {
        // One emulator per row, so each row's users and group are its own.
        let emulator = Emulator::new();
        for id in row.start.iter().chain(row.add) {
            let key = id.to_ascii_lowercase();
            if key == state::DEFAULT_USER_ID {
                // The user the service itself owns; nothing creates it.
                continue;
            }
            create_user(&emulator, row.case, &key).await;
        }
        let created = create_user_group(&emulator, "wording-group", row.engine, row.start).await;
        assert_eq!(
            created.status, 200,
            "case {}: the setup create must answer 200, got {} {}",
            row.case, created.status, created.body
        );
        // A group the clock has not finished creating takes no change at all, so the
        // refusal under test is reached only once it is active.
        wait_out_the_transition();

        let refused =
            modify_user_group(&emulator, "wording-group", row.add, row.remove, None).await;
        assert_default_user_required(row.case, &refused, &["wording-group", row.engine]);

        // The refusal changes nothing: the membership is the one the group was created
        // with, no change is pending, and the group is still active.
        let described = describe_user_group(&emulator, row.case, "wording-group").await;
        assert_eq!(
            described.all("UserIds/member"),
            row.start.to_vec(),
            "case {}: the membership must be untouched, got {}",
            row.case,
            described.body
        );
        assert!(
            described.all("UserIdsToAdd/member").is_empty()
                && described.all("UserIdsToRemove/member").is_empty(),
            "case {}: the refusal must leave no pending change, got {}",
            row.case,
            described.body
        );
        assert_eq!(
            described.text("Status"),
            Some(state::USER_STATUS_ACTIVE),
            "case {}: the group must still be {}, got {}",
            row.case,
            state::USER_STATUS_ACTIVE,
            described.body
        );

        // The positive control: the same call for a member the group may lose is accepted.
        if let Some(removable) = row
            .start
            .iter()
            .find(|id| id.to_ascii_lowercase() != state::DEFAULT_USER_ID)
        {
            let accepted =
                modify_user_group(&emulator, "wording-group", &[], &[removable], None).await;
            assert_eq!(
                accepted.status, 200,
                "case {}: removing {removable} must answer 200, got {} {}",
                row.case, accepted.status, accepted.body
            );
        }
    }
}

/// One `CreateUserGroup` row: the id, the engine, and a membership that does not name the
/// built-in user.
type CreateCase = (&'static str, &'static str, &'static str, &'static [&'static str]);

/// The axis: how many members are named, the case they are written in, a request that
/// names none at all, the spelling of the group id, and the engine.
const CREATE_CASES: &[CreateCase] = &[
    ("one non-default member", "create-one", START_ENGINE, &["member-one"]),
    (
        "two non-default members",
        "CreateTwo",
        "valkey",
        &["member-one", "member-two"],
    ),
    (
        "a member written in another case",
        "create-mixed-case",
        START_ENGINE,
        &["Member-One"],
    ),
    ("no members named at all", "create-none", START_ENGINE, &[]),
];

#[tokio::test]
async fn creating_a_group_without_the_built_in_user_is_refused_with_the_same_sentence() {
    for (case, id, engine, members) in CREATE_CASES {
        let emulator = Emulator::new();
        for member in *members {
            create_user(&emulator, case, &member.to_ascii_lowercase()).await;
        }

        let refused = create_user_group(&emulator, id, engine, members).await;
        assert_default_user_required(case, &refused, &[id, engine]);

        // Nothing was stored: the id is still free, and the reader does not know it.
        let described = emulator
            .call("DescribeUserGroups", &params(&[("UserGroupId", id)]))
            .await;
        assert_eq!(
            (described.status, described.error_code()),
            (404, "UserGroupNotFound"),
            "case {case:?}: the refused create must store nothing, got {} {}",
            described.status,
            described.body
        );

        // The positive control: the same request with the built-in user named is accepted.
        let with_default: Vec<&str> = members
            .iter()
            .copied()
            .chain([state::DEFAULT_USER_ID])
            .collect();
        let created = create_user_group(&emulator, id, engine, &with_default).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create naming the built-in user must answer 200, got {} {}",
            created.status, created.body
        );
    }
}

/// Both writers of a membership answer the SAME sentence, so a client that branches on the
/// refusal text sees one wording whichever call it made.
#[tokio::test]
async fn both_writers_of_a_membership_answer_one_sentence() {
    let emulator = Emulator::new();
    create_user(&emulator, "shared wording", "member-one").await;

    let refused_create =
        create_user_group(&emulator, "shared-wording", START_ENGINE, &["member-one"]).await;
    assert_default_user_required("CreateUserGroup", &refused_create, &["shared-wording"]);

    let created = create_user_group(
        &emulator,
        "shared-wording",
        START_ENGINE,
        &[state::DEFAULT_USER_ID, "member-one"],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the setup create must answer 200, got {} {}",
        created.status, created.body
    );
    wait_out_the_transition();

    let refused_modify =
        modify_user_group(&emulator, "shared-wording", &[], &["default"], None).await;
    assert_default_user_required("ModifyUserGroup", &refused_modify, &["shared-wording"]);

    assert_eq!(
        refused_create.error_message(),
        refused_modify.error_message(),
        "both writers must answer one sentence, got {:?} and {:?}",
        refused_create.error_message(),
        refused_modify.error_message()
    );
}
