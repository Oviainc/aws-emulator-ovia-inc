// SPDX-License-Identifier: CC0-1.0

//! `DeleteUser`, driven end to end over HTTP through the production router.
//!
//! Every suite is a TABLE whose rows differ along the axis the rule under test
//! generalises over: which user of several is deleted, the form the id is given in,
//! the `Revision` the caller expects, and how many group memberships the user holds.
//! Nothing reaches into the state struct, so a response that is not computed from
//! persisted state cannot pass; the delete is observed through `DescribeUser` and
//! `ListGroupMemberships`, the readers a client has.
//!
//! The harness (`emulator`, `ok`, `refused`, …) is the one
//! [`crate::identity_store_lifecycle`] already carries, so there is one copy of it.

use serde_json::{json, Value};

use crate::harness::{call, emulator, ok, refused, string, MISSING_STORE, NOT_FOUND, STORE};
use crate::identity_store_lifecycle::{create_group, create_user, user_name};

/// The upper bound of the modelled `ResourceRevision` length
/// (`smithy_query(query="operation",operation="DeleteUser")`: `Revision`, len 1..64,
/// pattern `^[0-9]+$`). A revision of that many digits is a well-formed value the
/// boundary validator admits, so a request carrying one reaches the handler.
const REVISION_MAX_LEN: usize = 64;

/// A well-formed `ResourceId` nothing ever created, so a refusal naming it comes from
/// state rather than from the boundary validator.
const UNCREATED_ID: &str = "0123abcd-4567-89ab-cdef-0123456789ab";

/// Create a user and report its `UserId` and `Revision` — the revision a conditional
/// delete has to match.
async fn create_user_with_revision(
    router: &smithy_server_runtime::MultiServiceRouter,
    store: &str,
    name: &str,
    case: &str,
) -> (String, String) {
    let created = ok(
        router,
        "CreateUser",
        json!({ "IdentityStoreId": store, "UserName": name }),
        case,
    )
    .await;
    (
        string(&created, "UserId", case),
        string(&created, "Revision", case),
    )
}

/// Whether `DescribeUser` still reports the user `user_id` names in `store`.
async fn user_exists(
    router: &smithy_server_runtime::MultiServiceRouter,
    store: &str,
    user_id: &str,
) -> bool {
    let (status, _, _) = call(
        router,
        "DescribeUser",
        json!({ "IdentityStoreId": store, "UserId": user_id }),
    )
    .await;
    status == 200
}

/// A delete must answer 200 with an empty body: `DeleteUserResponse` models no members
/// (`smithy_query(query="operation",operation="DeleteUser")`: `output: {}`).
fn assert_empty_body(body: &Value, case: &str) {
    assert_eq!(
        body,
        &json!({}),
        "case {case:?}: expected DeleteUser to answer an empty body, got {body}"
    );
}

#[tokio::test]
async fn deleting_a_user_removes_that_user_and_nothing_else() {
    // Rows differ along WHICH of the store's users is deleted, so a delete that
    // removed the wrong record — or everything — cannot pass.
    let rows = [("the first", 0usize), ("a middle one", 1), ("the last", 2)];

    for (case, victim) in rows {
        let router = emulator();
        let mut users: Vec<String> = Vec::new();
        for index in 0..3 {
            users.push(create_user(&router, STORE, &user_name(case, index), case).await);
        }
        // A GROUP of the same store, so the delete's scope is observable: deleting a
        // user removes that user and nothing else the store holds.
        let group = create_group(&router, STORE, &format!("group-{case}"), case).await;

        let deleted = ok(
            &router,
            "DeleteUser",
            json!({ "IdentityStoreId": STORE, "UserId": users[victim].clone() }),
            case,
        )
        .await;
        assert_empty_body(&deleted, case);

        for (index, user) in users.iter().enumerate() {
            let present = user_exists(&router, STORE, user).await;
            assert_eq!(
                present,
                index != victim,
                "case {case:?}: expected user {index} (id {user}) present={}, got present={present}",
                index != victim
            );
        }
        ok(
            &router,
            "DescribeGroup",
            json!({ "IdentityStoreId": STORE, "GroupId": group.clone() }),
            case,
        )
        .await;

        // Deleting it again finds nothing to delete.
        let body = refused(
            &router,
            "DeleteUser",
            json!({ "IdentityStoreId": STORE, "UserId": users[victim].clone() }),
            NOT_FOUND,
            "ResourceNotFoundException",
            case,
        )
        .await;
        assert_eq!(
            body["ResourceType"].as_str().unwrap_or_default(),
            "USER",
            "case {case:?}: expected the modelled USER ResourceType, got {body}"
        );
    }
}

/// How a row builds the `Revision` it sends out of the user's current revision.
#[derive(Debug, Clone, Copy)]
enum Revision {
    /// No `Revision` member at all: "If you don't provide this value, the user is
    /// deleted regardless of its current revision."
    Absent,
    /// The user's current revision, exactly as the create reported it.
    Current,
    /// The current revision padded with leading zeros — the same number, which the
    /// modelled `^[0-9]+$` admits.
    CurrentPadded,
    /// One past the current revision: a revision the user has not reached.
    OnePast,
    /// `0`: a revision no stored resource carries, since a create is revision 1.
    Zero,
    /// The longest value the modelled length admits, which is far past any revision.
    LongestAllowed,
}

impl Revision {
    /// The member this row sends, given the user's current revision.
    fn member(self, current: &str) -> Option<String> {
        let number: u64 = current.parse().expect("the reported revision is decimal");
        match self {
            Self::Absent => None,
            Self::Current => Some(current.to_string()),
            Self::CurrentPadded => Some(format!("{current:0>8}")),
            Self::OnePast => Some((number + 1).to_string()),
            Self::Zero => Some("0".to_string()),
            Self::LongestAllowed => Some("9".repeat(REVISION_MAX_LEN)),
        }
    }

    /// Whether this row's value matches the user's current revision, and so deletes.
    fn matches(self) -> bool {
        matches!(self, Self::Absent | Self::Current | Self::CurrentPadded)
    }
}

#[tokio::test]
async fn a_delete_happens_only_when_the_expected_revision_is_the_current_one() {
    // Rows differ along the `Revision` the caller expects: absent, the current one in
    // two spellings, and three values that are not it.
    let rows = [
        ("no revision", Revision::Absent),
        ("the current revision", Revision::Current),
        ("the current revision, zero-padded", Revision::CurrentPadded),
        ("one revision past the current", Revision::OnePast),
        ("revision zero", Revision::Zero),
        ("the longest revision allowed", Revision::LongestAllowed),
    ];

    for (case, revision) in rows {
        let router = emulator();
        let (user, current) = create_user_with_revision(&router, STORE, &user_name(case, 0), case).await;
        let mut request = json!({ "IdentityStoreId": STORE, "UserId": user.clone() });
        if let Some(member) = revision.member(&current) {
            request["Revision"] = json!(member);
        }

        if revision.matches() {
            let deleted = ok(&router, "DeleteUser", request, case).await;
            assert_empty_body(&deleted, case);
            assert!(
                !user_exists(&router, STORE, &user).await,
                "case {case:?}: expected user {user} to be gone after a matching-revision delete"
            );
        } else {
            refused(
                &router,
                "DeleteUser",
                request,
                409,
                "ConflictException",
                case,
            )
            .await;
            // A refused conditional delete changes nothing: the user is still there,
            // still at the revision it was created with.
            let described = ok(
                &router,
                "DescribeUser",
                json!({ "IdentityStoreId": STORE, "UserId": user.clone() }),
                case,
            )
            .await;
            assert_eq!(
                string(&described, "Revision", case),
                current,
                "case {case:?}: expected the user to stay at revision {current}, got {described}"
            );
        }
    }
}

#[tokio::test]
async fn deleting_a_user_takes_its_memberships_and_frees_its_user_name() {
    // Rows differ along how many groups the user belongs to, including none.
    let rows = [("no groups", 0usize), ("one group", 1), ("three groups", 3)];

    for (case, group_count) in rows {
        let router = emulator();
        let name = user_name(case, 0);
        let user = create_user(&router, STORE, &name, case).await;
        // A second member of every group, so the cascade is observably scoped to the
        // deleted user rather than emptying the group.
        let bystander = create_user(&router, STORE, &user_name(case, 1), case).await;

        let mut groups: Vec<String> = Vec::new();
        for index in 0..group_count {
            let group = create_group(&router, STORE, &format!("group-{case}-{index}"), case).await;
            for member in [&user, &bystander] {
                ok(
                    &router,
                    "CreateGroupMembership",
                    json!({
                        "IdentityStoreId": STORE,
                        "GroupId": group.clone(),
                        "MemberId": { "UserId": member.clone() },
                    }),
                    case,
                )
                .await;
            }
            groups.push(group);
        }

        let deleted = ok(
            &router,
            "DeleteUser",
            json!({ "IdentityStoreId": STORE, "UserId": user.clone() }),
            case,
        )
        .await;
        assert_empty_body(&deleted, case);

        for group in &groups {
            // The group survives, with exactly the memberships that did not name the
            // deleted user.
            let page = ok(
                &router,
                "ListGroupMemberships",
                json!({ "IdentityStoreId": STORE, "GroupId": group.clone() }),
                case,
            )
            .await;
            let members = page["GroupMemberships"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                members.len(),
                1,
                "case {case:?}: expected group {group} to keep only the bystander's \
                 membership, got {page}"
            );
            assert_eq!(
                members[0]["MemberId"]["UserId"].as_str().unwrap_or_default(),
                bystander,
                "case {case:?}: expected the surviving membership to name the bystander, \
                 got {page}"
            );
            let described = ok(
                &router,
                "DescribeGroup",
                json!({ "IdentityStoreId": STORE, "GroupId": group.clone() }),
                case,
            )
            .await;
            assert_eq!(
                string(&described, "GroupId", case),
                *group,
                "case {case:?}: expected group {group} to survive the delete, got {described}"
            );
        }

        // The `UserName` the deleted user claimed is free again, and the user created
        // under it is a new resource rather than the old one returning.
        let recreated = create_user(&router, STORE, &name, case).await;
        assert_ne!(
            recreated, user,
            "case {case:?}: expected a freshly minted UserId for the re-created {name}"
        );
    }
}

#[tokio::test]
async fn a_delete_accepts_every_form_of_the_user_id_and_refuses_one_it_does_not_hold() {
    // Rows differ along the FORM the id is given in: the modelled `ResourceId` admits
    // the bare id and the resource's ARN, and its hexadecimal digits in either case
    // (`^(arn:aws[a-z-]*:identitystore:::(user|group|membership)/)?…[A-Fa-f0-9]{12}$`).
    type IdForm = fn(&str) -> String;
    let forms: [(&str, IdForm); 3] = [
        ("the bare id", |id| id.to_string()),
        ("the user ARN", |id| {
            format!("arn:aws:identitystore:::user/{id}")
        }),
        ("the id in upper case", |id| id.to_ascii_uppercase()),
    ];

    for (case, form) in forms {
        let router = emulator();
        let user = create_user(&router, STORE, &user_name(case, 0), case).await;
        let deleted = ok(
            &router,
            "DeleteUser",
            json!({ "IdentityStoreId": STORE, "UserId": form(&user) }),
            case,
        )
        .await;
        assert_empty_body(&deleted, case);
        assert!(
            !user_exists(&router, STORE, &user).await,
            "case {case:?}: expected user {user} to be gone after a delete by {case}"
        );
    }

    // And the rows a delete cannot satisfy: the id names no user of the store the
    // request names, or the store itself does not exist.
    let router = emulator();
    let known = create_user(&router, STORE, "alice", "seed").await;
    let missing: [(&str, Value, &str); 3] = [
        (
            "an uncreated id in the live store",
            json!({ "IdentityStoreId": STORE, "UserId": UNCREATED_ID }),
            "USER",
        ),
        (
            "a known user, through a store that does not exist",
            json!({ "IdentityStoreId": MISSING_STORE, "UserId": known.clone() }),
            "IDENTITY_STORE",
        ),
        (
            "a store that does not exist",
            json!({ "IdentityStoreId": MISSING_STORE, "UserId": UNCREATED_ID }),
            "IDENTITY_STORE",
        ),
    ];
    for (case, request, expected_type) in missing {
        let body = refused(
            &router,
            "DeleteUser",
            request,
            NOT_FOUND,
            "ResourceNotFoundException",
            case,
        )
        .await;
        assert_eq!(
            body["ResourceType"].as_str().unwrap_or_default(),
            expected_type,
            "case {case:?}: expected the modelled {expected_type} ResourceType, got {body}"
        );
    }
    // A failed delete is not a write: the user is still in the store that holds it, and
    // the store that does not exist still does not.
    assert!(
        user_exists(&router, STORE, &known).await,
        "expected the known user {known} to survive three failed deletes"
    );
    refused(
        &router,
        "DescribeUser",
        json!({ "IdentityStoreId": MISSING_STORE, "UserId": known.clone() }),
        NOT_FOUND,
        "ResourceNotFoundException",
        "the store was not vivified",
    )
    .await;
}
