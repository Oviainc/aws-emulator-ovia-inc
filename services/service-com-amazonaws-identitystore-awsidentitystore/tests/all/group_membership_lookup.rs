// SPDX-License-Identifier: CC0-1.0

//! `GetGroupMembershipId`, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: AWSIdentityStore.<Operation>` and an `awsJson1_1` body
//! (OPERATIONS.md, "Wire protocol"), dispatched by the production
//! [`MultiServiceRouter`]. Nothing reaches into the state struct, so an answer that is
//! not computed from the memberships `CreateGroupMembership` persisted cannot pass.
//!
//! The rule under test: `GetGroupMembershipId` reports the `MembershipId` and
//! `MembershipArn` of the membership the identity store holds for the (group, member)
//! pair the request names, and refuses with the modelled `ResourceNotFoundException`
//! (HTTP 404) when it holds none. The suites are TABLES whose rows differ along what
//! that rule generalises over: how many memberships the group and the store hold,
//! which member of which group is asked about, the FORM of each identifier (bare id,
//! resource ARN, upper case, identity-store ARN), which identity store, and which part
//! of the pair is missing.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from OPERATIONS.md, "Wire protocol".
const TARGET_PREFIX: &str = "AWSIdentityStore";

/// Two identity stores, so the per-store scope of the lookup is observable. Any value
/// the modelled `IdentityStoreId` pattern admits works.
const STORE: &str = "d-1234567890";
const OTHER_STORE: &str = "d-abcdef0123";

/// A well-formed `ResourceId` of a resource nothing created, so a refusal comes from
/// state rather than from the boundary validator.
const MISSING_ID: &str = "0123abcd-4567-89ab-cdef-0123456789ab";

/// A router with just this service mounted on a fresh engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are not what these tests are about; the request still carries a SigV4
    // credential scope, which is what routes it to this service.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_identitystore_awsidentitystore::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call: the HTTP status, the wire error code and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, String, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("host", "identitystore.us-east-1.amazonaws.com")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDIDENTITYSTORE/20260101/us-east-1\
             /identitystore/aws4_request, SignedHeaders=host;x-amz-target, Signature=00",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    let code = if header_code.is_empty() {
        parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    } else {
        header_code
    };
    (status, code, parsed)
}

/// A call that must answer 200, returning its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, code, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {code} with {parsed}"
    );
    parsed
}

/// A call that must be refused with `status` and an error code naming `error`.
async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    status: u16,
    error: &str,
    case: &str,
) -> Value {
    let (actual_status, code, parsed) = call(router, operation, body).await;
    assert_eq!(
        actual_status, status,
        "case {case:?}: expected {operation} to answer {status}, got {actual_status} {code} \
         with {parsed}"
    );
    assert!(
        code.ends_with(error),
        "case {case:?}: expected the error code {error}, got {code:?} with {parsed}"
    );
    parsed
}

/// A required string member of a response body.
fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

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
    // belongs to, WHICH member of WHICH group is asked about, and which identity store
    // holds them: (case, store, groups, members per group, the group and member index
    // to look up).
    let rows: [(&str, &str, usize, usize, usize, usize); 6] = [
        ("one group, one member", STORE, 1, 1, 0, 0),
        ("one group, three members, the first", STORE, 1, 3, 0, 0),
        ("one group, three members, the last", STORE, 1, 3, 0, 2),
        ("three groups, the middle one", STORE, 3, 2, 1, 1),
        ("three groups, the last one", STORE, 3, 2, 2, 0),
        ("a second identity store", OTHER_STORE, 2, 2, 1, 1),
    ];

    for (case, store, groups, members, group_index, member_index) in rows {
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

    // Rows differ along WHICH part of the pair the store does not hold: the identity
    // store, the group, the member, or the pairing itself.
    let rows = [
        (
            "an identity store nothing was created in",
            json!({
                "IdentityStoreId": OTHER_STORE,
                "GroupId": group.clone(),
                "MemberId": { "UserId": user.clone() },
            }),
        ),
        (
            "a group nothing created",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": MISSING_ID,
                "MemberId": { "UserId": user.clone() },
            }),
        ),
        (
            "a member nothing created",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "MemberId": { "UserId": MISSING_ID },
            }),
        ),
        (
            "a user who is not a member of that group",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": group.clone(),
                "MemberId": { "UserId": stranger.clone() },
            }),
        ),
        (
            "a member of another group, asked of the wrong group",
            json!({
                "IdentityStoreId": STORE,
                "GroupId": other_group.clone(),
                "MemberId": { "UserId": user.clone() },
            }),
        ),
    ];
    for (row_case, request) in rows {
        let refusal = refused(
            &router,
            "GetGroupMembershipId",
            request,
            404,
            "ResourceNotFoundException",
            row_case,
        )
        .await;
        // The refusal names the resource CLASS that was not found and discloses no id
        // the caller did not already hold.
        assert_eq!(
            refusal["ResourceType"].as_str(),
            Some("GROUP_MEMBERSHIP"),
            "case {row_case:?}: expected the refusal to name the membership class, got {refusal}"
        );
        assert!(
            refusal["ResourceId"].is_null(),
            "case {row_case:?}: expected no ResourceId on the refusal, got {refusal}"
        );
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
        404,
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
