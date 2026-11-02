// SPDX-License-Identifier: CC0-1.0

//! `DeleteGroup`, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses (`POST /` with
//! `X-Amz-Target: AWSIdentityStore.DeleteGroup`), dispatched by the production router,
//! and every post-condition is read back through an implemented operation —
//! `DescribeGroup`, `ListGroupMemberships`, `CreateGroup`, `DeleteGroupMembership` —
//! so a delete that answered 200 without touching state cannot pass.
//!
//! The rules under test:
//!
//! * a delete removes exactly the group the request names from exactly the identity
//!   store it names, together with the memberships that pointed at that group, and
//!   releases the `DisplayName` uniqueness claim the group held;
//! * the operation is `@idempotent`
//!   (`smithy_query(query="operation",operation="DeleteGroup")`), so a call that finds
//!   no such group left — one already deleted, one never created, one in a store
//!   nothing created — still answers an empty 200 and removes nothing else;
//! * `Revision`, when sent, is the caller's expected current revision: the delete
//!   happens only on a match, and a mismatch is refused with `ConflictException`
//!   (HTTP 409) leaving the group exactly as it was.
//!
//! The suites are TABLES: rows differ along the axis each rule generalises over
//! (how many memberships the group holds, the form of the id — bare, ARN, prefixed,
//! upper-case —, how often the delete is repeated, and the revision sent, including
//! both ends of the modelled `Revision` length).

use serde_json::{json, Value};

use crate::harness::{emulator, ok, refused, string, MISSING_STORE, NOT_FOUND, STORE};
use crate::identity_store_lifecycle::{
    assert_resource_arn, create_group, create_user, user_name, walk_memberships, MAX_RESULTS_MIN,
};
use smithy_server_runtime::MultiServiceRouter;

/// A well-formed `ResourceId` nothing in any store was ever minted with: the modelled
/// pattern admits it (`smithy_query(query="shape",shape="ResourceId")`), so a refusal
/// or a success here comes from state rather than from the boundary validator.
const UNMINTED_ID: &str = "0123abcd-4567-89ab-cdef-0123456789ab";

/// The bounds of the modelled `Revision` length (`ResourceRevision`, len 1..64,
/// pattern `^[0-9]+$`, from `smithy_query(query="shape",shape="ResourceRevision")`).
const REVISION_MAX_LEN: usize = 64;

/// The revision `CreateGroup` mints for a fresh group: a create is revision 1 and no
/// implemented operation bumps it.
const CREATED_REVISION: &str = "1";

/// Create a group and return its `GroupId` and `GroupArn`, asserting the ARN is the
/// one the modelled `ResourceArn` pattern demands for that id.
async fn create_group_with_arn(
    router: &MultiServiceRouter,
    store: &str,
    display_name: &str,
    case: &str,
) -> (String, String) {
    let created = ok(
        router,
        "CreateGroup",
        json!({ "IdentityStoreId": store, "DisplayName": display_name }),
        case,
    )
    .await;
    let group_id = string(&created, "GroupId", case);
    let group_arn = string(&created, "GroupArn", case);
    assert_resource_arn(&group_arn, "group", &group_id, case);
    (group_id, group_arn)
}

/// Make `user` a member of `group` and return the `MembershipId`.
async fn add_member(
    router: &MultiServiceRouter,
    store: &str,
    group: &str,
    user: &str,
    case: &str,
) -> String {
    let created = ok(
        router,
        "CreateGroupMembership",
        json!({
            "IdentityStoreId": store,
            "GroupId": group,
            "MemberId": { "UserId": user },
        }),
        case,
    )
    .await;
    string(&created, "MembershipId", case)
}

/// Assert `DescribeGroup` reports `group` with `display_name` and `revision`.
async fn assert_described(
    router: &MultiServiceRouter,
    store: &str,
    group: &str,
    display_name: &str,
    revision: &str,
    case: &str,
) {
    let described = ok(
        router,
        "DescribeGroup",
        json!({ "IdentityStoreId": store, "GroupId": group }),
        case,
    )
    .await;
    assert_eq!(
        string(&described, "DisplayName", case),
        display_name,
        "case {case:?}: expected the stored DisplayName, got {described}"
    );
    assert_eq!(
        string(&described, "Revision", case),
        revision,
        "case {case:?}: expected revision {revision}, got {described}"
    );
}

/// Assert `group` is gone from `store`: neither describable nor listable.
async fn assert_gone(router: &MultiServiceRouter, store: &str, group: &str, case: &str) {
    refused(
        router,
        "DescribeGroup",
        json!({ "IdentityStoreId": store, "GroupId": group }),
        NOT_FOUND,
        "ResourceNotFoundException",
        case,
    )
    .await;
    refused(
        router,
        "ListGroupMemberships",
        json!({ "IdentityStoreId": store, "GroupId": group }),
        NOT_FOUND,
        "ResourceNotFoundException",
        case,
    )
    .await;
}

/// One `DeleteGroup` that must answer an empty 200.
async fn delete_group(router: &MultiServiceRouter, request: Value, case: &str) {
    let deleted = ok(router, "DeleteGroup", request, case).await;
    assert_eq!(
        deleted,
        json!({}),
        "case {case:?}: expected DeleteGroup to answer an empty body, got {deleted}"
    );
}

#[tokio::test]
async fn deleting_a_group_removes_it_with_its_memberships_and_frees_its_display_name() {
    // Rows differ along how many memberships the deleted group holds and the form of
    // the id the delete names it by.
    let rows: [(&str, usize, bool); 5] = [
        ("a group with no members, by id", 0, false),
        ("a group with one member, by id", 1, false),
        ("a group with three members, by id", 3, false),
        ("a group with two members, by ARN", 2, true),
        ("a group with five members, by ARN", 5, true),
    ];

    for (case, members, by_arn) in rows {
        let router = emulator();
        let display_name = format!("group-{}", case.replace(' ', "-"));
        let (group, group_arn) = create_group_with_arn(&router, STORE, &display_name, case).await;
        // A second group in the same store: not this delete's to touch.
        let bystander = create_group(&router, STORE, "bystanders", case).await;

        let mut victim_memberships: Vec<String> = Vec::new();
        let mut bystander_memberships: Vec<String> = Vec::new();
        for index in 0..members {
            let user = create_user(&router, STORE, &user_name(case, index), case).await;
            victim_memberships.push(add_member(&router, STORE, &group, &user, case).await);
            // The same user also belongs to the bystander group, so a cascade that
            // removed too much would show up as a missing bystander membership.
            bystander_memberships.push(add_member(&router, STORE, &bystander, &user, case).await);
        }
        let listed = walk_memberships(&router, STORE, &group, Some(MAX_RESULTS_MIN), case).await;
        assert_eq!(
            listed, victim_memberships,
            "case {case:?}: expected the created memberships before the delete, got {listed:?}"
        );

        let named_by = if by_arn { &group_arn } else { &group };
        delete_group(
            &router,
            json!({ "IdentityStoreId": STORE, "GroupId": named_by }),
            case,
        )
        .await;

        // The group itself is gone.
        assert_gone(&router, STORE, &group, case).await;
        // So is every membership that pointed at it: the record is no longer in the
        // store, so even a delete naming its id finds nothing.
        for membership in &victim_memberships {
            refused(
                &router,
                "DeleteGroupMembership",
                json!({ "IdentityStoreId": STORE, "MembershipId": membership }),
                NOT_FOUND,
                "ResourceNotFoundException",
                case,
            )
            .await;
        }
        // And nothing else moved: the bystander group keeps every membership.
        let surviving =
            walk_memberships(&router, STORE, &bystander, Some(MAX_RESULTS_MIN), case).await;
        assert_eq!(
            surviving, bystander_memberships,
            "case {case:?}: expected the bystander group's memberships to survive, \
             got {surviving:?}"
        );

        // The `DisplayName` claim the group held is released, so the name is free: the
        // recreated group is a NEW group, with its own id and no memberships.
        let (recreated, _) = create_group_with_arn(&router, STORE, &display_name, case).await;
        assert_ne!(
            recreated, group,
            "case {case:?}: expected a fresh GroupId for the recreated group"
        );
        assert_described(
            &router,
            STORE,
            &recreated,
            &display_name,
            CREATED_REVISION,
            case,
        )
        .await;
        let fresh = walk_memberships(&router, STORE, &recreated, Some(MAX_RESULTS_MIN), case).await;
        assert!(
            fresh.is_empty(),
            "case {case:?}: expected the recreated group to hold no memberships, got {fresh:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_group_is_idempotent() {
    // Rows differ along how many times the delete is repeated and whether the repeats
    // name the group by id or by ARN.
    let rows: [(&str, usize, bool); 4] = [
        ("deleted twice by id", 2, false),
        ("deleted three times by id", 3, false),
        ("deleted twice, the repeat by ARN", 2, true),
        ("deleted four times, the repeats by ARN", 4, true),
    ];

    for (case, attempts, repeat_by_arn) in rows {
        let router = emulator();
        let (group, group_arn) = create_group_with_arn(&router, STORE, "engineering", case).await;
        let user = create_user(&router, STORE, &user_name(case, 0), case).await;
        add_member(&router, STORE, &group, &user, case).await;

        for attempt in 0..attempts {
            let named_by = if attempt > 0 && repeat_by_arn {
                &group_arn
            } else {
                &group
            };
            delete_group(
                &router,
                json!({ "IdentityStoreId": STORE, "GroupId": named_by }),
                case,
            )
            .await;
            assert_gone(&router, STORE, &group, case).await;
        }
    }
}

#[tokio::test]
async fn deleting_a_group_that_was_never_created_is_a_success_that_removes_nothing() {
    // Rows differ along the store named and the FORM of the unminted id: the modelled
    // `ResourceId` pattern admits the bare UUID, the resource ARN, an optional
    // ten-hex-digit prefix and either letter case, and none of them names a group this
    // emulator minted.
    let rows: [(&str, &str, String); 4] = [
        ("a store that holds another group", STORE, UNMINTED_ID.to_string()),
        (
            "the ARN of a group nothing created",
            STORE,
            format!("arn:aws:identitystore:::group/{UNMINTED_ID}"),
        ),
        (
            "an id carrying the optional store prefix",
            STORE,
            format!("0123456789-{UNMINTED_ID}"),
        ),
        (
            "an upper-case id",
            STORE,
            UNMINTED_ID.to_ascii_uppercase(),
        ),
    ];

    for (case, store, group_id) in rows {
        let router = emulator();
        // The one group that DOES exist: it must still be there afterwards.
        let kept = create_group(&router, STORE, "engineering", case).await;

        delete_group(
            &router,
            json!({ "IdentityStoreId": store, "GroupId": group_id }),
            case,
        )
        .await;

        assert_described(&router, STORE, &kept, "engineering", CREATED_REVISION, case).await;
    }
}

/// Idempotence covers the GROUP, not the identifier the call is addressed to: a delete
/// addressed to a store of ANOTHER account is refused with the modelled not-found — the
/// same refusal every other operation of this service answers one with — while a store of
/// this account that holds no such group is the empty success idempotence promises.
#[tokio::test]
async fn deleting_a_group_in_a_store_this_emulator_does_not_serve_is_refused() {
    // Rows differ along the form the other account's store id takes and whether the group
    // id it names was ever minted, so the refusal is the STORE's and not the group's.
    let router = emulator();
    let kept = create_group(&router, STORE, "engineering", "seed").await;
    let rows: [(&str, String, String); 3] = [
        (
            "another account's store and a group nothing created",
            MISSING_STORE.to_string(),
            UNMINTED_ID.to_string(),
        ),
        (
            "another account's store and a group that exists in ours",
            MISSING_STORE.to_string(),
            kept.clone(),
        ),
        (
            "another account's store in UUID form",
            "arn:aws:identitystore::210987654321:identitystore/\
             a1b2c3d4-5678-90ab-cdef-111122223333"
                .to_string(),
            kept.clone(),
        ),
    ];

    for (case, store, group_id) in rows {
        let body = refused(
            &router,
            "DeleteGroup",
            json!({ "IdentityStoreId": store, "GroupId": group_id }),
            NOT_FOUND,
            "ResourceNotFoundException",
            case,
        )
        .await;
        assert_eq!(
            body["ResourceType"].as_str().unwrap_or_default(),
            "IDENTITY_STORE",
            "case {case:?}: expected the identity store to be the missing resource, got {body}"
        );
        // The refused delete removed nothing and created no store.
        assert_described(&router, STORE, &kept, "engineering", CREATED_REVISION, case).await;
        refused(
            &router,
            "DescribeGroup",
            json!({ "IdentityStoreId": MISSING_STORE, "GroupId": kept.clone() }),
            NOT_FOUND,
            "ResourceNotFoundException",
            case,
        )
        .await;
    }

    // A store of THIS account that holds no such group is served, and `@idempotent`
    // makes that delete the empty success: the post-condition already holds. It writes no
    // group either — the store still lists none.
    for (case, store) in [
        ("a store nothing has written to", "d-0e0f000102".to_string()),
        (
            "that store by ARN",
            format!(
                "arn:aws:identitystore::{}:identitystore/d-0e0f000102",
                engine::emulator_account_id()
            ),
        ),
    ] {
        delete_group(
            &router,
            json!({ "IdentityStoreId": store.clone(), "GroupId": kept.clone() }),
            case,
        )
        .await;
        let listed = ok(
            &router,
            "ListGroups",
            json!({ "IdentityStoreId": store }),
            case,
        )
        .await;
        assert_eq!(
            listed["Groups"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected the delete to leave that store empty, got {listed}"
        );
        // And the group it named is still in the store that does hold it.
        assert_described(&router, STORE, &kept, "engineering", CREATED_REVISION, case).await;
    }
}

#[tokio::test]
async fn a_revision_is_an_expected_current_revision() {
    // Rows differ along the `Revision` sent: the group's current one (and the same
    // value written with a leading zero — the member is a decimal string, so it
    // denotes a number), values on either side of it, the longest the modelled length
    // admits, and one character past that bound (refused by the modelled constraint,
    // not by state).
    let rows: [(&str, String, u16, &str); 7] = [
        (
            "the group's current revision",
            CREATED_REVISION.to_string(),
            200,
            "",
        ),
        (
            "the current revision with a leading zero",
            format!("0{CREATED_REVISION}"),
            200,
            "",
        ),
        ("one revision ahead", "2".to_string(), 409, "ConflictException"),
        (
            "a revision before any create",
            "0".to_string(),
            409,
            "ConflictException",
        ),
        (
            "a far later revision",
            "99".to_string(),
            409,
            "ConflictException",
        ),
        (
            "the longest revision the model admits",
            "9".repeat(REVISION_MAX_LEN),
            409,
            "ConflictException",
        ),
        (
            "one character past the modelled length",
            "9".repeat(REVISION_MAX_LEN + 1),
            400,
            "ValidationException",
        ),
    ];

    for (case, revision, status, error) in rows {
        let router = emulator();
        let (group, _) = create_group_with_arn(&router, STORE, "engineering", case).await;
        let user = create_user(&router, STORE, &user_name(case, 0), case).await;
        let membership = add_member(&router, STORE, &group, &user, case).await;
        let request = json!({
            "IdentityStoreId": STORE,
            "GroupId": group.clone(),
            "Revision": revision,
        });

        if error.is_empty() {
            delete_group(&router, request, case).await;
            assert_gone(&router, STORE, &group, case).await;
        } else {
            refused(&router, "DeleteGroup", request, status, error, case).await;
            // The refusal changed nothing: the group and its membership are both still
            // there, at the revision they were at.
            assert_described(&router, STORE, &group, "engineering", CREATED_REVISION, case).await;
            let remaining =
                walk_memberships(&router, STORE, &group, Some(MAX_RESULTS_MIN), case).await;
            assert_eq!(
                remaining,
                vec![membership],
                "case {case:?}: expected the group's membership to survive a refused delete, \
                 got {remaining:?}"
            );
        }
    }
}
