// SPDX-License-Identifier: CC0-1.0

//! `GetGroupMembershipId`, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses, through the shared
//! harness in [`crate::harness`]: nothing reaches into the state struct, so an answer
//! that is not computed from the memberships `CreateGroupMembership` persisted cannot
//! pass.
//!
//! The rule under test: `GetGroupMembershipId` reports the `MembershipId` and
//! `MembershipArn` of the membership the identity store holds for the (group, member)
//! pair the request names, and refuses with the modelled `ResourceNotFoundException`
//! (the modelled not-found) when it holds none. The suites are TABLES whose rows differ along what
//! that rule generalises over: how many memberships the group and the store hold,
//! which member of which group is asked about, the FORM of each identifier (bare id,
//! resource ARN, upper case, identity-store ARN), and which part of the pair is missing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{emulator, ok, refused, string, MISSING_STORE, NOT_FOUND, STORE};

/// A well-formed `ResourceId` of a resource nothing created, so a refusal comes from
/// state rather than from the boundary validator.
const MISSING_ID: &str = "0123abcd-4567-89ab-cdef-0123456789ab";

/// A `UserName` for a row: the modelled `UserName` pattern
/// (`^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`) admits letters, marks, symbols, numbers and
/// punctuation but NOT whitespace, so a case's spaces become hyphens.
fn slug(case: &str) -> String {
    case.replace(' ', "-").replace(',', "")
}

/// Create a user and return its `UserId`.
async fn create_user(router: &MultiServiceRouter, store: &str, user_name: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateUser",
        json!({ "IdentityStoreId": store, "UserName": user_name }),
        case,
    )
    .await;
    string(&created, "UserId", case)
}

/// Create a group and return its `GroupId`.
async fn create_group(router: &MultiServiceRouter, store: &str, display_name: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateGroup",
        json!({ "IdentityStoreId": store, "DisplayName": display_name }),
        case,
    )
    .await;
    string(&created, "GroupId", case)
}

/// Join `user` to `group` and return the `(MembershipId, MembershipArn)` the service
/// minted for the pair.
async fn join(
    router: &MultiServiceRouter,
    store: &str,
    group: &str,
    user: &str,
    case: &str,
) -> (String, String) {
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
    (
        string(&created, "MembershipId", case),
        string(&created, "MembershipArn", case),
    )
}

/// `GetGroupMembershipId` for one pair.
async fn get_membership_id(
    router: &MultiServiceRouter,
    store: &str,
    group: &str,
    user: &str,
    case: &str,
) -> Value {
    ok(
        router,
        "GetGroupMembershipId",
        json!({
            "IdentityStoreId": store,
            "GroupId": group,
            "MemberId": { "UserId": user },
        }),
        case,
    )
    .await
}

/// Assert the answer is exactly the membership `CreateGroupMembership` minted for the
/// pair: the same id, the ARN of that id (the form the modelled `ResourceArn` pattern
/// demands) and the identity store that holds it.
fn assert_resolves_to(answer: &Value, store: &str, membership_id: &str, membership_arn: &str, case: &str) {
    assert_eq!(
        string(answer, "MembershipId", case),
        membership_id,
        "case {case:?}: expected the stored MembershipId, got {answer}"
    );
    assert_eq!(
        string(answer, "MembershipArn", case),
        membership_arn,
        "case {case:?}: expected the stored MembershipArn, got {answer}"
    );
    assert_eq!(
        string(answer, "MembershipArn", case),
        format!("arn:aws:identitystore:::membership/{membership_id}"),
        "case {case:?}: expected the membership ARN of {membership_id}, got {answer}"
    );
    assert_eq!(
        string(answer, "IdentityStoreId", case),
        store,
        "case {case:?}: expected the answer to name store {store}, got {answer}"
    );
}

#[tokio::test]
async fn a_pair_resolves_to_the_membership_that_was_created_for_it() {
    let router = emulator();
    // Rows differ along how many members the group holds, how many groups the member
    // belongs to, and WHICH member of WHICH group is asked about: (case, groups,
    // members per group, the group and member index to look up).
    let rows: [(&str, usize, usize, usize, usize); 6] = [
        ("one group, one member", 1, 1, 0, 0),
        ("one group, three members, the first", 1, 3, 0, 0),
        ("one group, three members, the last", 1, 3, 0, 2),
        ("three groups, the middle one", 3, 2, 1, 1),
        ("three groups, the last one", 3, 2, 2, 0),
        ("four groups, four members, the last pair", 4, 4, 3, 3),
    ];

    for (case, groups, members, group_index, member_index) in rows {
        let store = STORE;
        // Every pair of the row's groups and members is a membership, so the lookup has
        // to pick the right one out of `groups * members` of them.
        let group_ids: Vec<String> = {
            let mut ids = Vec::new();
            for index in 0..groups {
                let display_name = format!("group-{}-{index}", slug(case));
                ids.push(create_group(&router, store, &display_name, case).await);
            }
            ids
        };
        let user_ids: Vec<String> = {
            let mut ids = Vec::new();
            for index in 0..members {
                let user_name = format!("user-{}-{index}", slug(case));
                ids.push(create_user(&router, store, &user_name, case).await);
            }
            ids
        };
        let mut minted: Vec<(usize, usize, String, String)> = Vec::new();
        for (g, group) in group_ids.iter().enumerate() {
            for (m, user) in user_ids.iter().enumerate() {
                let (id, arn) = join(&router, store, group, user, case).await;
                minted.push((g, m, id, arn));
            }
        }

        let (_, _, expected_id, expected_arn) = minted
            .iter()
            .find(|(g, m, _, _)| *g == group_index && *m == member_index)
            .expect("the row's pair was created")
            .clone();
        let answer = get_membership_id(
            &router,
            store,
            &group_ids[group_index],
            &user_ids[member_index],
            case,
        )
        .await;
        assert_resolves_to(&answer, store, &expected_id, &expected_arn, case);

        // Every other pair resolves to its OWN membership, so the answer is looked up
        // rather than guessed from whatever was created last.
        for (g, m, id, arn) in &minted {
            let row_case = format!("{case}: group {g} member {m}");
            let answer =
                get_membership_id(&router, store, &group_ids[*g], &user_ids[*m], &row_case).await;
            assert_resolves_to(&answer, store, id, arn, &row_case);
        }

        // A read changes nothing a later caller sees: the same pair answers the same
        // membership, and the group still lists exactly the memberships it held.
        let repeat = get_membership_id(
            &router,
            store,
            &group_ids[group_index],
            &user_ids[member_index],
            case,
        )
        .await;
        assert_resolves_to(&repeat, store, &expected_id, &expected_arn, case);
        let listed = ok(
            &router,
            "ListGroupMemberships",
            json!({ "IdentityStoreId": store, "GroupId": group_ids[group_index] }),
            case,
        )
        .await;
        assert_eq!(
            listed["GroupMemberships"].as_array().map(Vec::len),
            Some(members),
            "case {case:?}: expected the lookup to have left the group's memberships alone, \
             got {listed}"
        );
    }
}

#[tokio::test]
async fn every_identifier_form_names_the_same_membership() {
    let router = emulator();
    let case = "identifier forms";
    let group = create_group(&router, STORE, "engineering", case).await;
    let user = create_user(&router, STORE, "alice", case).await;
    let (membership_id, membership_arn) = join(&router, STORE, &group, &user, case).await;

    // Rows differ along the FORM of each identifier. The modelled `ResourceId` pattern
    // admits the bare id, the resource's ARN and either letter case
    // (`^(arn:aws[a-z-]*:identitystore:::(user|group|membership)/)?[A-Fa-f0-9]{8}-…$`),
    // and the modelled `IdentityStoreId` pattern admits the store's id or its ARN
    // (`^(arn:aws[a-z-]*:identitystore::\d{12}:identitystore/)?(d-[0-9a-f]{10}|…)$`) —
    // all of them name the same resource.
    let store_forms = [
        ("store id", STORE.to_string()),
        (
            "store ARN",
            format!(
                "arn:aws:identitystore::{}:identitystore/{STORE}",
                engine::emulator_account_id()
            ),
        ),
    ];
    let group_forms = [
        ("bare id", group.clone()),
        ("resource ARN", format!("arn:aws:identitystore:::group/{group}")),
        ("upper case", group.to_ascii_uppercase()),
    ];
    let member_forms = [
        ("bare id", user.clone()),
        ("resource ARN", format!("arn:aws:identitystore:::user/{user}")),
        ("upper case", user.to_ascii_uppercase()),
    ];

    for (store_case, store) in &store_forms {
        for (group_case, group_form) in &group_forms {
            for (member_case, member_form) in &member_forms {
                let row_case =
                    format!("store by {store_case}, group by {group_case}, member by {member_case}");
                let answer =
                    get_membership_id(&router, store, group_form, member_form, &row_case).await;
                // The answer's own `IdentityStoreId` is the store's id whichever form
                // named it, so it is reported from state rather than echoed.
                assert_resolves_to(&answer, STORE, &membership_id, &membership_arn, &row_case);
            }
        }
    }
}

#[tokio::test]
async fn a_pair_with_no_membership_is_reported_as_not_found() {
    let router = emulator();
    let case = "no such membership";
    let group = create_group(&router, STORE, "engineering", case).await;
    let other_group = create_group(&router, STORE, "security", case).await;
    let user = create_user(&router, STORE, "alice", case).await;
    let stranger = create_user(&router, STORE, "bob", case).await;
    let (membership_id, membership_arn) = join(&router, STORE, &group, &user, case).await;

    // Rows differ along WHICH part of the pair is missing: the identity store, the
    // group, the member, or the pairing itself.
    let rows = [
        (
            "an identity store of another account",
            json!({
                "IdentityStoreId": MISSING_STORE,
                "GroupId": group.clone(),
                "MemberId": { "UserId": user.clone() },
            }),
            "IDENTITY_STORE",
        ),
        (
            "a group nothing created",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": MISSING_ID,
                "MemberId": { "UserId": user.clone() },
            }),
            "GROUP_MEMBERSHIP",
        ),
        (
            "a member nothing created",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "MemberId": { "UserId": MISSING_ID },
            }),
            "GROUP_MEMBERSHIP",
        ),
        (
            "a user who is not a member of that group",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "MemberId": { "UserId": stranger.clone() },
            }),
            "GROUP_MEMBERSHIP",
        ),
        (
            "a member of another group, asked of the wrong group",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": other_group.clone(),
                "MemberId": { "UserId": user.clone() },
            }),
            "GROUP_MEMBERSHIP",
        ),
    ];
    for (row_case, request, expected_type) in rows {
        let refusal = refused(
            &router,
            "GetGroupMembershipId",
            request,
            NOT_FOUND,
            "ResourceNotFoundException",
            row_case,
        )
        .await;
        // The refusal names the resource CLASS that was not found and discloses no id
        // the caller did not already hold.
        assert_eq!(
            refusal["ResourceType"].as_str(),
            Some(expected_type),
            "case {row_case:?}: expected the refusal to name {expected_type}, got {refusal}"
        );
        // A membership has no id to report — the pair is what was asked about — while an
        // identifier naming no store this emulator serves is reported as the store.
        if expected_type == "GROUP_MEMBERSHIP" {
            assert!(
                refusal["ResourceId"].is_null(),
                "case {row_case:?}: expected no ResourceId on the refusal, got {refusal}"
            );
        }
    }

    // The pair that does exist still resolves, so none of the refusals disturbed it.
    let answer = get_membership_id(&router, STORE, &group, &user, case).await;
    assert_resolves_to(&answer, STORE, &membership_id, &membership_arn, case);

    // And once the membership is deleted the pair is not found again: the answer tracks
    // state rather than the fact that the pair once existed.
    ok(
        &router,
        "DeleteGroupMembership",
        json!({ "IdentityStoreId": STORE, "MembershipId": membership_id.clone() }),
        case,
    )
    .await;
    refused(
        &router,
        "GetGroupMembershipId",
        json!({
            "IdentityStoreId": STORE,
            "GroupId": group.clone(),
            "MemberId": { "UserId": user.clone() },
        }),
        NOT_FOUND,
        "ResourceNotFoundException",
        "after the membership is deleted",
    )
    .await;

    // Re-joining the same pair mints a new membership, and the lookup reports THAT one.
    let (rejoined_id, rejoined_arn) = join(&router, STORE, &group, &user, case).await;
    assert_ne!(
        rejoined_id, membership_id,
        "case {case:?}: expected a rejoin to mint a fresh MembershipId, got {rejoined_id}"
    );
    let answer = get_membership_id(&router, STORE, &group, &user, "after a rejoin").await;
    assert_resolves_to(&answer, STORE, &rejoined_id, &rejoined_arn, "after a rejoin");
}
