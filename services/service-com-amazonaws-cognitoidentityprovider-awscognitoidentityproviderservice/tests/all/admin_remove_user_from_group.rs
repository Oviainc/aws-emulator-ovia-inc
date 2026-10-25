// SPDX-License-Identifier: CC0-1.0

//! `AdminRemoveUserFromGroup` over the wire, driven exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: `AdminRemoveUserFromGroup` DROPS, from the stored user
//! named by `Username` in the pool named by `UserPoolId`, membership of the
//! stored group named by `GroupName`, leaving that user's other memberships, the
//! group itself and every other user's membership of it untouched; membership is
//! a SET, so removing one the user does not hold (a repeat, or one never added)
//! drops nothing and still succeeds. The drop is PERSISTED: a later
//! `AdminListGroupsForUser` no longer lists the group and a later sign-in's
//! tokens no longer carry it in their `cognito:groups` claim. An absent pool or
//! group is a 404 `ResourceNotFoundException` and an absent user a 404
//! `UserNotFoundException`, and a refused removal persists nothing.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy, so a user can sign in
/// and the issued tokens can be read for their `cognito:groups` claim.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Call `operation` with `body` and return its status and JSON response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "cognito-idp.us-east-1.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        )
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from(body.to_string()))
        .expect("the request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// The `__type` of an error body, which is the wire error code.
fn error_code(payload: &Value) -> &str {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create `username` in `user_pool_id` and return its generated `sub`.
async fn user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> String {
    let created = ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    created["User"]["Attributes"]
        .as_array()
        .expect("the stored attributes")
        .iter()
        .find(|attribute| attribute["Name"] == "sub")
        .and_then(|attribute| attribute["Value"].as_str())
        .expect("a generated sub")
        .to_string()
}

/// Create the group `group_name` in `user_pool_id`.
async fn group(router: &MultiServiceRouter, user_pool_id: &str, group_name: &str) {
    ok(
        router,
        "CreateGroup",
        &json!({
            "UserPoolId": user_pool_id,
            "GroupName": group_name,
            "Description": format!("the {group_name} group"),
        }),
    )
    .await;
}

/// Add `username` to `group_name`, requiring success.
async fn add(router: &MultiServiceRouter, user_pool_id: &str, username: &str, group_name: &str) {
    ok(
        router,
        "AdminAddUserToGroup",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "GroupName": group_name,
        }),
    )
    .await;
}

/// Remove `username` from `group_name` and return the status and body.
async fn remove(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    group_name: &str,
) -> (u16, Value) {
    call(
        router,
        "AdminRemoveUserFromGroup",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "GroupName": group_name,
        }),
    )
    .await
}

/// The group names `AdminListGroupsForUser` reports for `username`, in the order
/// it served them.
async fn memberships(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> Vec<String> {
    let payload = ok(
        router,
        "AdminListGroupsForUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    payload["Groups"]
        .as_array()
        .unwrap_or_else(|| panic!("Groups must be a list, got {payload}"))
        .iter()
        .map(|entry| {
            entry["GroupName"]
                .as_str()
                .unwrap_or_else(|| panic!("an entry must name a group, got {entry}"))
                .to_string()
        })
        .collect()
}

/// One row of the removal table: the memberships the user starts with, the
/// removals applied in request order and the memberships that must remain (in
/// the ascending name order the listing serves).
struct RemovalCase {
    case: &'static str,
    adds: Vec<String>,
    removes: Vec<String>,
    remaining: Vec<String>,
}

/// Every group name here satisfies `GroupNameType` (length 1..128, pattern
/// `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`) from
/// `smithy_query(query="operation",operation="AdminRemoveUserFromGroup")`. The
/// rows differ along the COUNT of memberships held and removed (one of one, one
/// of several, all of several, none), the FORM of the names (ASCII,
/// punctuation, non-ASCII letters, the length maximum) and whether the same
/// removal is REPEATED or names a membership the user never held.
fn removal_cases() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            case: "the only membership removed",
            adds: vec!["admins".to_string()],
            removes: vec!["admins".to_string()],
            remaining: vec![],
        },
        RemovalCase {
            case: "one of three memberships removed, the others kept",
            adds: vec![
                "admins".to_string(),
                "ops.team-1_a".to_string(),
                "readers".to_string(),
            ],
            removes: vec!["ops.team-1_a".to_string()],
            remaining: vec!["admins".to_string(), "readers".to_string()],
        },
        RemovalCase {
            case: "every membership removed one at a time",
            adds: vec!["admins".to_string(), "readers".to_string()],
            removes: vec!["readers".to_string(), "admins".to_string()],
            remaining: vec![],
        },
        RemovalCase {
            case: "a non-ASCII group name removed, an ASCII one kept",
            adds: vec!["グループ①".to_string(), "admins".to_string()],
            removes: vec!["グループ①".to_string()],
            remaining: vec!["admins".to_string()],
        },
        RemovalCase {
            case: "a group name at the modelled length maximum 128 removed",
            adds: vec!["g".repeat(128), "readers".to_string()],
            removes: vec!["g".repeat(128)],
            remaining: vec!["readers".to_string()],
        },
        RemovalCase {
            case: "the same removal three times: membership is a set",
            adds: vec!["admins".to_string(), "readers".to_string()],
            removes: vec![
                "admins".to_string(),
                "admins".to_string(),
                "admins".to_string(),
            ],
            remaining: vec!["readers".to_string()],
        },
        RemovalCase {
            case: "a membership the user never held: nothing is dropped",
            adds: vec!["admins".to_string()],
            removes: vec!["readers".to_string()],
            remaining: vec!["admins".to_string()],
        },
        RemovalCase {
            case: "no removal at all: the memberships stand",
            adds: vec!["admins".to_string(), "readers".to_string()],
            removes: vec![],
            remaining: vec!["admins".to_string(), "readers".to_string()],
        },
    ]
}

/// Every group name any row of the table names, so each case's pool holds them
/// all.
fn all_group_names() -> Vec<String> {
    let mut names: Vec<String> = removal_cases()
        .iter()
        .flat_map(|case| case.adds.iter().chain(case.removes.iter()).cloned())
        .collect();
    names.sort();
    names.dedup();
    names
}

#[tokio::test]
async fn a_removal_drops_exactly_that_membership_and_persists_it() {
    let router = router();
    let user_pool_id = pool(&router, "removal-pool").await;
    for group_name in all_group_names() {
        group(&router, &user_pool_id, &group_name).await;
    }

    for (index, case) in removal_cases().into_iter().enumerate() {
        let label = case.case;
        // One user per row, so each row starts from the memberships it names.
        let username = format!("user-{index}");
        user(&router, &user_pool_id, &username).await;
        for group_name in &case.adds {
            add(&router, &user_pool_id, &username, group_name).await;
        }

        for group_name in &case.removes {
            let (status, payload) = remove(&router, &user_pool_id, &username, group_name).await;
            assert_eq!(
                status, 200,
                "case {label:?}: removing {group_name:?} must succeed, got {status} with {payload}"
            );
        }

        let actual = memberships(&router, &user_pool_id, &username).await;
        assert_eq!(
            actual, case.remaining,
            "case {label:?}: expected the memberships {:?}, got {actual:?}",
            case.remaining
        );

        // The drop is STORED, not an artefact of the first read: a second read
        // answers the same.
        let again = memberships(&router, &user_pool_id, &username).await;
        assert_eq!(
            again, actual,
            "case {label:?}: a second read answers the same stored membership, got {again:?}"
        );

        // The group itself survives a removal of one of its members.
        for group_name in &case.removes {
            let fetched = ok(
                &router,
                "GetGroup",
                &json!({ "UserPoolId": user_pool_id, "GroupName": group_name }),
            )
            .await;
            assert_eq!(
                fetched["Group"]["GroupName"].as_str(),
                Some(group_name.as_str()),
                "case {label:?}: the group {group_name:?} outlives the removal, got {fetched}"
            );
        }
    }
}

#[tokio::test]
async fn a_removal_touches_one_user_of_one_pool() {
    let router = router();
    let first_pool = pool(&router, "first-pool").await;
    let second_pool = pool(&router, "second-pool").await;

    // The same group name and the same usernames exist in both pools, and every
    // user belongs to the group; only the first pool's `alice` is removed.
    for user_pool_id in [&first_pool, &second_pool] {
        group(&router, user_pool_id, "admins").await;
        for username in ["alice", "bob"] {
            user(&router, user_pool_id, username).await;
            add(&router, user_pool_id, username, "admins").await;
        }
    }
    let (status, payload) = remove(&router, &first_pool, "alice", "admins").await;
    assert_eq!(
        status, 200,
        "the removal must succeed, got {status} with {payload}"
    );

    for (label, user_pool_id, username, expected) in [
        (
            "the removed user of the pool it was removed in",
            &first_pool,
            "alice",
            Vec::<String>::new(),
        ),
        (
            "another user of the same pool",
            &first_pool,
            "bob",
            vec!["admins".to_string()],
        ),
        (
            "the same username in another pool",
            &second_pool,
            "alice",
            vec!["admins".to_string()],
        ),
    ] {
        let actual = memberships(&router, user_pool_id, username).await;
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_user_is_resolved_by_username_or_by_sub() {
    let router = router();
    let user_pool_id = pool(&router, "resolve-pool").await;
    for group_name in ["by-sub", "by-username"] {
        group(&router, &user_pool_id, group_name).await;
    }
    let sub = user(&router, &user_pool_id, "carol").await;
    for group_name in ["by-sub", "by-username"] {
        add(&router, &user_pool_id, "carol", group_name).await;
    }

    // `Username` "must be the sub of a local user or the username" — both name
    // the same stored user, so a removal under either identifier lands on it.
    for (label, identifier, group_name, expected) in [
        (
            "by username",
            "carol".to_string(),
            "by-username",
            vec!["by-sub".to_string()],
        ),
        ("by sub", sub, "by-sub", Vec::<String>::new()),
    ] {
        let (status, payload) = remove(&router, &user_pool_id, &identifier, group_name).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the user must resolve, got {status} with {payload}"
        );
        let actual = memberships(&router, &user_pool_id, "carol").await;
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_absent_pool_group_or_user_is_refused_and_persists_nothing() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-pool").await;
    group(&router, &user_pool_id, "admins").await;
    user(&router, &user_pool_id, "dave").await;
    add(&router, &user_pool_id, "dave", "admins").await;

    for (label, body, expected) in [
        (
            "an absent user pool",
            json!({
                "UserPoolId": "us-east-1_absent",
                "Username": "dave",
                "GroupName": "admins",
            }),
            (404, "ResourceNotFoundException"),
        ),
        (
            "a group the pool does not hold",
            json!({
                "UserPoolId": user_pool_id,
                "Username": "dave",
                "GroupName": "absent-group",
            }),
            (404, "ResourceNotFoundException"),
        ),
        (
            "a group name differing only in case, which is a different group",
            json!({
                "UserPoolId": user_pool_id,
                "Username": "dave",
                "GroupName": "Admins",
            }),
            (404, "ResourceNotFoundException"),
        ),
        (
            "a user the pool does not hold",
            json!({
                "UserPoolId": user_pool_id,
                "Username": "absent-user",
                "GroupName": "admins",
            }),
            (404, "UserNotFoundException"),
        ),
    ] {
        let (status, payload) = call(&router, "AdminRemoveUserFromGroup", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // Nothing above was persisted: the stored membership still stands.
    let actual = memberships(&router, &user_pool_id, "dave").await;
    assert_eq!(
        actual,
        vec!["admins".to_string()],
        "a refused removal persists nothing, got {actual:?}"
    );
}

#[tokio::test]
async fn a_request_outside_a_modelled_constraint_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "constraint-pool").await;
    group(&router, &user_pool_id, "admins").await;
    user(&router, &user_pool_id, "dave").await;

    // One value past each published bound of `AdminRemoveUserFromGroup`'s input
    // (`smithy_query(query="operation",operation="AdminRemoveUserFromGroup")`):
    // `GroupName` and `Username` length 1..128 and pattern
    // `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`, and `UserPoolId` length 1..55 and
    // pattern `^[\w-]+_[0-9a-zA-Z]+$`.
    for (label, body) in [
        (
            "an empty group name, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "Username": "dave", "GroupName": "" }),
        ),
        (
            "a group name of 129 characters, past the length maximum 128",
            json!({
                "UserPoolId": user_pool_id,
                "Username": "dave",
                "GroupName": "g".repeat(129),
            }),
        ),
        (
            "a group name holding a space, which the pattern excludes",
            json!({ "UserPoolId": user_pool_id, "Username": "dave", "GroupName": "two words" }),
        ),
        (
            "an empty username, below the length minimum 1",
            json!({ "UserPoolId": user_pool_id, "Username": "", "GroupName": "admins" }),
        ),
        (
            "a username of 129 characters, past the length maximum 128",
            json!({
                "UserPoolId": user_pool_id,
                "Username": "u".repeat(129),
                "GroupName": "admins",
            }),
        ),
        (
            "a user pool id the pattern excludes",
            json!({ "UserPoolId": "no-underscore", "Username": "dave", "GroupName": "admins" }),
        ),
        (
            "a user pool id of 56 characters, past the length maximum 55",
            json!({
                "UserPoolId": format!("us-east-1_{}", "a".repeat(46)),
                "Username": "dave",
                "GroupName": "admins",
            }),
        ),
        (
            "a request missing the required GroupName",
            json!({ "UserPoolId": user_pool_id, "Username": "dave" }),
        ),
    ] {
        let (status, payload) = call(&router, "AdminRemoveUserFromGroup", &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: a request outside a modelled constraint is a 400, got {status} with {payload}"
        );
        assert_ne!(
            error_code(&payload),
            "NotImplemented",
            "case {label:?}: a bad request is never answered with the scaffold's 501, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_removed_membership_is_gone_from_the_tokens_of_a_later_sign_in() {
    let router = router();
    let user_pool_id = pool(&router, "token-pool").await;
    let client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "token-pool-client",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    for group_name in ["admins", "readers"] {
        group(&router, &user_pool_id, group_name).await;
    }
    user(&router, &user_pool_id, "erin").await;
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": "erin",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    for group_name in ["admins", "readers"] {
        add(&router, &user_pool_id, "erin", group_name).await;
    }

    let before = groups_claim_of_sign_in(&router, &user_pool_id, &client_id, "erin").await;
    assert_eq!(
        before,
        Some(vec!["admins".to_string(), "readers".to_string()]),
        "before the removal the tokens carry both memberships, got {before:?}"
    );

    let (status, payload) = remove(&router, &user_pool_id, "erin", "admins").await;
    assert_eq!(
        status, 200,
        "the removal must succeed, got {status} with {payload}"
    );

    let after = groups_claim_of_sign_in(&router, &user_pool_id, &client_id, "erin").await;
    assert_eq!(
        after,
        Some(vec!["readers".to_string()]),
        "after the removal the tokens carry only the kept membership, got {after:?}"
    );
}

/// Sign `username` in and return the `cognito:groups` claim of the identity
/// token it issues, or `None` when the token carries none.
async fn groups_claim_of_sign_in(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> Option<Vec<String>> {
    let result = ok(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let token = result["AuthenticationResult"]["IdToken"]
        .as_str()
        .unwrap_or_else(|| panic!("an id token must be issued, got {result}"))
        .to_string();
    let claims = claims_of(&token);
    let claim = claims.get("cognito:groups")?.clone();
    let names = claim
        .as_array()
        .unwrap_or_else(|| panic!("cognito:groups must be a list, got {claim}"))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("a group name must be a string, got {name}"))
                .to_string()
        })
        .collect();
    Some(names)
}

/// The claim set of a compact-serialized JWS.
fn claims_of(token: &str) -> Value {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a JWS compact serialization has three segments, got {token}"
    );
    serde_json::from_slice(&decode_base64url(segments[1]))
        .unwrap_or_else(|error| panic!("the claims must be JSON ({error}): {token}"))
}

/// A base64url JWS segment's bytes (RFC 7515 §2): the alphabet substitutions
/// and the stripped padding are restored before decoding.
fn decode_base64url(segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("{segment:?} must be base64url ({error})"))
}
