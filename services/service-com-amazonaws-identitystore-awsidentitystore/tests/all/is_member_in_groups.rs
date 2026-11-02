// SPDX-License-Identifier: CC0-1.0

//! `IsMemberInGroups`, driven end to end over HTTP through the production router.
//!
//! The rule under test: for each id in `GroupIds`, in the order it was requested
//! (repeats included), the response reports whether the named identity store holds a
//! membership joining that group and that member — one result per requested id,
//! naming the resolved group and member — and the check writes nothing.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: how
//! many ids are requested, which of them the member belongs to, the order they come
//! in, repeats, ids no group of this store carries, the id form (bare id, ARN, letter
//! case) and the store the membership lives in. The issue's recorded case — a mixed
//! membership check — is one row.
//!
//! The harness (`emulator`, `call`, `ok`, `refused`) is [`crate::harness`]'s and the
//! resource helpers are the lifecycle suite's, so nothing here reaches into the state
//! struct: a response that is not computed from persisted state cannot pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, ok, refused, string, MISSING_STORE, NOT_FOUND, STORE};
use crate::identity_store_lifecycle::{create_group, create_user};

/// The bounds of the modelled `GroupIds` length (1..100, from
/// `smithy_query(query="operation",operation="IsMemberInGroups")`).
const GROUP_IDS_MIN: usize = 1;
const GROUP_IDS_MAX: usize = 100;

/// A well-formed `ResourceId` no resource of any store was created with: the modelled
/// pattern demands a hexadecimal UUID, so an "unknown group" row still has to be one.
const UNKNOWN_GROUP_ID: &str = "fedcba98-7654-3210-fedc-ba9876543210";

/// A well-formed `ResourceId` no user was created with.
const UNKNOWN_USER_ID: &str = "00000000-1111-2222-3333-444444444444";

/// A name for `case`'s row: the modelled `UserName` pattern admits letters, marks,
/// symbols, numbers and punctuation but NOT whitespace, so a case's spaces become
/// hyphens.
fn name(prefix: &str, case: &str, index: usize) -> String {
    format!("{prefix}-{}-{index}", case.replace(' ', "-"))
}

/// One `IsMemberInGroups` call that must answer 200: its `Results` list.
async fn check(
    router: &MultiServiceRouter,
    store: &str,
    member: &str,
    group_ids: &[String],
    case: &str,
) -> Vec<Value> {
    let body = ok(
        router,
        "IsMemberInGroups",
        json!({
            "IdentityStoreId": store,
            "MemberId": { "UserId": member },
            "GroupIds": group_ids,
        }),
        case,
    )
    .await;
    body["Results"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Results list, got {body}"))
        .clone()
}

/// The `MembershipExists` of one result.
///
/// The modelled member carries `@default: false`, so a false may come back as an
/// omitted member; both forms mean "no membership".
fn exists(result: &Value) -> bool {
    result
        .get("MembershipExists")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Assert `results` answers `expected`: one result per requested id, in request order,
/// each naming the group it was asked about and the member it was asked for.
fn assert_results(
    results: &[Value],
    requested: &[String],
    member: &str,
    expected: &[bool],
    case: &str,
) {
    assert_eq!(
        results.len(),
        expected.len(),
        "case {case:?}: expected one result per requested group ({}), got {results:?}",
        expected.len()
    );
    for (index, ((result, group_id), expected)) in
        results.iter().zip(requested).zip(expected).enumerate()
    {
        assert_eq!(
            result.get("GroupId").and_then(Value::as_str),
            Some(group_id.as_str()),
            "case {case:?}: expected result {index} to name group {group_id:?}, got {result}"
        );
        assert_eq!(
            result
                .get("MemberId")
                .and_then(|member_id| member_id.get("UserId"))
                .and_then(Value::as_str),
            Some(member),
            "case {case:?}: expected result {index} to name member {member:?}, got {result}"
        );
        assert_eq!(
            exists(result),
            *expected,
            "case {case:?}: expected MembershipExists {expected} for group {group_id:?} \
             at index {index}, got {result}"
        );
    }
}

/// Create one user and `groups` groups in `store`, joining the user to the groups at
/// `member_of`. Returns the `UserId` and the `GroupId`s in creation order.
async fn store_with_groups(
    router: &MultiServiceRouter,
    store: &str,
    groups: usize,
    member_of: &[usize],
    case: &str,
) -> (String, Vec<String>) {
    let user = create_user(router, store, &name("user", case, 0), case).await;
    let mut group_ids = Vec::with_capacity(groups);
    for index in 0..groups {
        group_ids.push(create_group(router, store, &name("group", case, index), case).await);
    }
    for index in member_of {
        ok(
            router,
            "CreateGroupMembership",
            json!({
                "IdentityStoreId": store,
                "GroupId": group_ids[*index],
                "MemberId": { "UserId": user },
            }),
            case,
        )
        .await;
    }
    (user, group_ids)
}

/// Each requested group gets its own answer, in the order it was asked about.
#[tokio::test]
async fn every_requested_group_gets_its_own_answer_in_request_order() {
    // Four groups, the member joined to the first and the third: every row below is a
    // selection of those four, so the expected booleans are known per row.
    const MEMBER_OF: &[usize] = &[0, 2];
    // (case, requested group indexes, expected MembershipExists per index)
    let rows: &[(&str, &[usize], &[bool])] = &[
        ("one group the member belongs to", &[0], &[true]),
        ("one group the member does not belong to", &[1], &[false]),
        (
            "every group, mixed membership",
            &[0, 1, 2, 3],
            &[true, false, true, false],
        ),
        (
            "the same groups in reverse order",
            &[3, 2, 1, 0],
            &[false, true, false, true],
        ),
        ("only groups the member belongs to", &[0, 2], &[true, true]),
        (
            "only groups the member does not belong to",
            &[1, 3],
            &[false, false],
        ),
        (
            "a repeated group id",
            &[0, 0, 1, 1],
            &[true, true, false, false],
        ),
    ];

    for (case, requested, expected) in rows {
        let router = emulator();
        let (user, groups) = store_with_groups(&router, STORE, 4, MEMBER_OF, case).await;
        let requested_ids: Vec<String> = requested.iter().map(|i| groups[*i].clone()).collect();
        let results = check(&router, STORE, &user, &requested_ids, case).await;
        assert_results(&results, &requested_ids, &user, expected, case);
    }
}

/// Only a membership of the named store, joining the named group and the named
/// member, counts: an id that names no group of that store holds no membership.
#[tokio::test]
async fn an_id_no_group_of_this_store_carries_holds_no_membership() {
    let case = "ids that name no group of this store";
    let router = emulator();
    // A user joined to the store's one group, and a second group it is NOT a member of.
    let (user, groups) = store_with_groups(&router, STORE, 2, &[0], case).await;

    // (row, the id asked about, whether this store holds that membership)
    let rows: &[(&str, &str, bool)] = &[
        ("the group the member joined", groups[0].as_str(), true),
        ("a group it did not join", groups[1].as_str(), false),
        (
            "a well-formed id no resource carries",
            UNKNOWN_GROUP_ID,
            false,
        ),
        // The three resource kinds share one id space, so a user id is a well-formed
        // `GroupIds` element — and names no group.
        ("the member's own user id", user.as_str(), false),
    ];

    for (row, group_id, expected) in rows {
        let row_case = format!("{case}: {row}");
        let requested = vec![(*group_id).to_string()];
        let results = check(&router, STORE, &user, &requested, &row_case).await;
        assert_results(&results, &requested, &user, &[*expected], &row_case);
    }
}

/// Every id member also accepts the resource's ARN, and the id's letter case does not
/// change which resource it names; the result reports the resolved id.
#[tokio::test]
async fn an_id_member_also_accepts_the_resources_arn() {
    let case = "id forms";
    let router = emulator();
    let (user, groups) = store_with_groups(&router, STORE, 2, &[0], case).await;
    let group_arn = |id: &str| format!("arn:aws:identitystore:::group/{id}");
    let user_arn = format!("arn:aws:identitystore:::user/{user}");

    // (row, the group id as written, the member as written)
    let rows: &[(&str, String, String)] = &[
        ("bare ids", groups[0].clone(), user.clone()),
        ("the group's ARN", group_arn(&groups[0]), user.clone()),
        ("the member's ARN", groups[0].clone(), user_arn.clone()),
        ("both ARNs", group_arn(&groups[0]), user_arn.clone()),
        (
            "an upper-case group id",
            groups[0].to_ascii_uppercase(),
            user.clone(),
        ),
        (
            "an upper-case ARN tail",
            group_arn(&groups[0].to_ascii_uppercase()),
            user_arn.clone(),
        ),
    ];

    for (row, group_as_written, member_as_written) in rows {
        let row_case = format!("{case}: {row}");
        let results = check(
            &router,
            STORE,
            member_as_written,
            std::slice::from_ref(group_as_written),
            &row_case,
        )
        .await;
        // Whatever form was sent, the answer names the resolved ids.
        assert_results(&results, &[groups[0].clone()], &user, &[true], &row_case);
    }

    // The same forms report `false` for the group the member did not join, so the
    // ARN form is not what makes a membership "exist".
    let requested = vec![group_arn(&groups[1])];
    let results = check(&router, STORE, &user_arn, &requested, case).await;
    assert_results(
        &results,
        &[groups[1].clone()],
        &user,
        &[false],
        &format!("{case}: the ARN of a group the member did not join"),
    );
}

/// The check reports the memberships the store holds at that moment, and changes
/// nothing: it is a read.
#[tokio::test]
async fn a_check_follows_the_stored_memberships_and_changes_nothing() {
    let case = "a check follows the store";
    let router = emulator();
    let (user, groups) = store_with_groups(&router, STORE, 2, &[], case).await;
    let requested = groups.clone();

    let join = |group: String| {
        let router = &router;
        let user = user.clone();
        async move {
            ok(
                router,
                "CreateGroupMembership",
                json!({
                    "IdentityStoreId": STORE,
                    "GroupId": group,
                    "MemberId": { "UserId": user },
                }),
                case,
            )
            .await
        }
    };

    // Before any membership exists.
    let results = check(&router, STORE, &user, &requested, case).await;
    assert_results(
        &results,
        &requested,
        &user,
        &[false, false],
        &format!("{case}: no memberships yet"),
    );

    // A repeated check answers the same thing, so the first one consumed nothing.
    let repeated = check(&router, STORE, &user, &requested, case).await;
    assert_eq!(
        repeated, results,
        "case {case:?}: expected a repeated check to answer the same Results, \
         got {repeated:?} after {results:?}"
    );

    let first = join(groups[0].clone()).await;
    let first_membership = string(&first, "MembershipId", case);
    let results = check(&router, STORE, &user, &requested, case).await;
    assert_results(
        &results,
        &requested,
        &user,
        &[true, false],
        &format!("{case}: joined to the first group"),
    );

    join(groups[1].clone()).await;
    let results = check(&router, STORE, &user, &requested, case).await;
    assert_results(
        &results,
        &requested,
        &user,
        &[true, true],
        &format!("{case}: joined to both groups"),
    );

    // The check did not consume, move or invent a membership: the group still lists
    // exactly the membership that was created.
    let listed = ok(
        &router,
        "ListGroupMemberships",
        json!({ "IdentityStoreId": STORE, "GroupId": groups[0] }),
        case,
    )
    .await;
    let membership_ids: Vec<&str> = listed["GroupMemberships"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a GroupMemberships list, got {listed}"))
        .iter()
        .filter_map(|membership| membership.get("MembershipId").and_then(Value::as_str))
        .collect();
    assert_eq!(
        membership_ids,
        vec![first_membership.as_str()],
        "case {case:?}: expected the first group to still list exactly its one \
         membership after the checks, got {listed}"
    );

    ok(
        &router,
        "DeleteGroupMembership",
        json!({ "IdentityStoreId": STORE, "MembershipId": first_membership }),
        case,
    )
    .await;
    let results = check(&router, STORE, &user, &requested, case).await;
    assert_results(
        &results,
        &requested,
        &user,
        &[false, true],
        &format!("{case}: after the first membership was deleted"),
    );
}

/// A store or member the check cannot resolve is refused — and the refusal creates
/// nothing, so it repeats unchanged.
#[tokio::test]
async fn a_store_or_member_the_check_cannot_resolve_is_refused() {
    let case = "unresolvable requests";
    let router = emulator();
    let (user, groups) = store_with_groups(&router, STORE, 1, &[0], case).await;

    // An identity store this emulator does not serve holds no membership to report, and
    // naming it does not bring it into being.
    let unknown_store = MISSING_STORE;
    for attempt in ["first", "repeated"] {
        let row_case = format!("{case}: another account's identity store ({attempt})");
        let refusal = refused(
            &router,
            "IsMemberInGroups",
            json!({
                "IdentityStoreId": unknown_store,
                "MemberId": { "UserId": user },
                "GroupIds": [groups[0]],
            }),
            NOT_FOUND,
            "ResourceNotFoundException",
            &row_case,
        )
        .await;
        // The same refusal both times: the first call did not bring that store into
        // being (which would turn the second into a different failure).
        assert_eq!(
            refusal.get("ResourceType").and_then(Value::as_str),
            Some("IDENTITY_STORE"),
            "case {row_case:?}: expected the store to be named as the missing resource, \
             got {refusal}"
        );
    }

    // (row, the member as written) — a member this store does not hold.
    let rows: &[(&str, &str)] = &[("a user id no store carries", UNKNOWN_USER_ID)];
    for (row, member) in rows {
        let row_case = format!("{case}: {row}");
        let refusal = refused(
            &router,
            "IsMemberInGroups",
            json!({
                "IdentityStoreId": STORE,
                "MemberId": { "UserId": member },
                "GroupIds": [groups[0]],
            }),
            NOT_FOUND,
            "ResourceNotFoundException",
            &row_case,
        )
        .await;
        assert_eq!(
            refusal.get("ResourceType").and_then(Value::as_str),
            Some("USER"),
            "case {row_case:?}: expected the member to be named as the missing resource, \
             got {refusal}"
        );
        assert_eq!(
            refusal.get("ResourceId").and_then(Value::as_str),
            Some(*member),
            "case {row_case:?}: expected the refusal to name the member asked about, \
             got {refusal}"
        );
    }
}

/// The number of groups one check asks about is bounded by the modelled `GroupIds`
/// length (1..100): both bounds are answered, and one past either is refused.
#[tokio::test]
async fn the_requested_group_count_is_bounded_by_the_model() {
    let case = "requested group count";
    let router = emulator();
    let (user, groups) = store_with_groups(&router, STORE, 2, &[0], case).await;

    // The published bounds, and one past each. Repeats are what make a long list
    // cheap to build; which ids repeat is already covered above.
    for count in [GROUP_IDS_MIN, GROUP_IDS_MAX] {
        let row_case = format!("{case}: {count} ids");
        let requested: Vec<String> = (0..count).map(|i| groups[i % 2].clone()).collect();
        let expected: Vec<bool> = (0..count).map(|i| i % 2 == 0).collect();
        let results = check(&router, STORE, &user, &requested, &row_case).await;
        assert_results(&results, &requested, &user, &expected, &row_case);
    }

    for count in [GROUP_IDS_MIN - 1, GROUP_IDS_MAX + 1] {
        let row_case = format!("{case}: {count} ids, past the modelled bound");
        let requested: Vec<String> = (0..count).map(|i| groups[i % 2].clone()).collect();
        refused(
            &router,
            "IsMemberInGroups",
            json!({
                "IdentityStoreId": STORE,
                "MemberId": { "UserId": user },
                "GroupIds": requested,
            }),
            400,
            "ValidationException",
            &row_case,
        )
        .await;
    }

    // A request with no `GroupIds` member at all is refused for the same reason: the
    // member is required.
    let (status, code, body) = call(
        &router,
        "IsMemberInGroups",
        json!({ "IdentityStoreId": STORE, "MemberId": { "UserId": user } }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a request without GroupIds to be refused, \
         got {status} {code} with {body}"
    );
}
