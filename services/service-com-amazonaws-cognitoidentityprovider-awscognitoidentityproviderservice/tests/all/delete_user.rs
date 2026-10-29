// SPDX-License-Identifier: CC0-1.0

//! `DeleteUser` over the wire, driven exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.DeleteUser`).
//!
//! The rule under test: a self-service deletion whose only subject is the
//! `AccessToken` removes the whole STORED profile that token was issued to — its
//! attributes, its group memberships and the challenge state that could sign it
//! in — and nothing else, so a later read of that user answers the error for a
//! user that is gone (400 `UserNotFoundException`), whether it is read
//! administratively or through an access token that still names it, while every
//! other user of the pool reads back unchanged. The deleted profile "can't be
//! restored": creating the same username again mints a DIFFERENT user, which the
//! old token does not resolve to. A token that names no signed-in user is refused
//! with 400 `NotAuthorizedException` and deletes nothing.
//!
//! The pools, app clients, users, groups and access tokens are all real: they
//! come from `CreateUserPool`, `CreateUserPoolClient`, `CreateGroup`,
//! `AdminCreateUser`, `AdminAddUserToGroup`, `AdminSetUserPassword` and
//! `InitiateAuth`, and every deletion is read back through `AdminGetUser`,
//! `GetUser`, `ListUsersInGroup` and `InitiateAuth`.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes an administrative call to
/// this service. The signature is never checked here: the suite turns request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// This service's regional endpoint host, which a signed call is addressed to.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The emulator's own endpoint: an address, so an unsigned request addressed to
/// it names no service in its `Host` header and `X-Amz-Target` is the only
/// routing signal left.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// A password satisfying a default pool's password policy.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// One emulator with just this service mounted.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Dispatch `operation` with `body` and return its status and JSON response
/// body. A `signed` call carries a credential scope, as the SDK sends every
/// administrative operation; an unsigned one is addressed to the emulator's own
/// address, which is how a signed-in user's client calls `DeleteUser` — the
/// model marks the operation `@optionalAuth`.
async fn dispatch(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    signed: bool,
) -> (u16, Value) {
    let host = if signed { ENDPOINT_HOST } else { EMULATOR_HOST };
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("AWSCognitoIdentityProviderService.{operation}"),
        );
    if signed {
        request = request.header("authorization", AUTHORIZATION);
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
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

/// Call the signed `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body, true).await;
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

/// Create a pool and return its minted `UserPoolId`. `case_sensitive` sets
/// `UsernameConfiguration.CaseSensitive`; `None` creates a pool that configures
/// none. Every name in `custom_schema` is added to `CreateUserPool.Schema`,
/// which the pool stores under the `custom:` prefix.
async fn pool(
    router: &MultiServiceRouter,
    name: &str,
    case_sensitive: Option<bool>,
    custom_schema: &[&str],
) -> String {
    let mut body = Map::new();
    body.insert("PoolName".to_string(), json!(name));
    if let Some(case_sensitive) = case_sensitive {
        body.insert(
            "UsernameConfiguration".to_string(),
            json!({ "CaseSensitive": case_sensitive }),
        );
    }
    if !custom_schema.is_empty() {
        body.insert(
            "Schema".to_string(),
            Value::Array(
                custom_schema
                    .iter()
                    .map(|attribute| {
                        json!({
                            "Name": attribute,
                            "AttributeDataType": "String",
                            "Mutable": true,
                        })
                    })
                    .collect(),
            ),
        );
    }
    let created = ok(router, "CreateUserPool", &Value::Object(body)).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create an app client that allows the password sign-in this suite uses, and
/// return its minted `ClientId`.
async fn client(router: &MultiServiceRouter, user_pool_id: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "deletion-client",
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string()
}

/// `attributes` as the modelled `AttributeListType` an SDK sends.
fn attribute_list(attributes: &[(&str, &str)]) -> Value {
    Value::Array(
        attributes
            .iter()
            .map(|(name, value)| json!({ "Name": name, "Value": value }))
            .collect(),
    )
}

/// Create `username` holding `attributes`, with a PERMANENT password so the user
/// is `CONFIRMED` and can sign in.
async fn confirmed_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) {
    ok(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": attribute_list(attributes),
        }),
    )
    .await;
    ok(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
}

/// Sign `username` in through an unsigned `InitiateAuth` and return its status
/// and body.
async fn initiate_auth(
    router: &MultiServiceRouter,
    client_id: &str,
    username: &str,
) -> (u16, Value) {
    dispatch(
        router,
        "InitiateAuth",
        &json!({
            "ClientId": client_id,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
        false,
    )
    .await
}

/// Sign `username` in and return the `AuthenticationResult` it answered.
async fn sign_in(router: &MultiServiceRouter, client_id: &str, username: &str) -> Value {
    let (status, payload) = initiate_auth(router, client_id, username).await;
    assert_eq!(
        status, 200,
        "signing {username} in must succeed, got {status} with {payload}"
    );
    payload["AuthenticationResult"].clone()
}

/// Sign `username` in `count` times and return the access token of each
/// sign-in: a user may hold several at once, and each one names it.
async fn access_tokens(
    router: &MultiServiceRouter,
    client_id: &str,
    username: &str,
    count: usize,
) -> Vec<String> {
    let mut tokens = Vec::with_capacity(count);
    for _ in 0..count {
        tokens.push(
            sign_in(router, client_id, username).await["AccessToken"]
                .as_str()
                .expect("an access token")
                .to_string(),
        );
    }
    tokens
}

/// Delete the user `access_token` was issued to, as that signed-in user's own
/// client does, and return the status and body.
async fn delete(router: &MultiServiceRouter, access_token: &str) -> (u16, Value) {
    dispatch(
        router,
        "DeleteUser",
        &json!({ "AccessToken": access_token }),
        false,
    )
    .await
}

/// Read `username` administratively and return the status and body.
async fn admin_get_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> (u16, Value) {
    dispatch(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
        true,
    )
    .await
}

/// Read the user `access_token` names through that user's own `GetUser`.
async fn get_user(router: &MultiServiceRouter, access_token: &str) -> (u16, Value) {
    dispatch(
        router,
        "GetUser",
        &json!({ "AccessToken": access_token }),
        false,
    )
    .await
}

/// The value of the `sub` attribute in a modelled `AttributeListType`.
fn sub_of(attributes: &Value) -> String {
    attributes
        .as_array()
        .unwrap_or_else(|| panic!("a read reports the stored attributes, got {attributes}"))
        .iter()
        .find(|attribute| attribute["Name"] == json!("sub"))
        .and_then(|attribute| attribute["Value"].as_str())
        .unwrap_or_else(|| panic!("a stored user holds a sub, got {attributes}"))
        .to_string()
}

/// The usernames `ListUsersInGroup` reports for `group_name`.
async fn group_members(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    group_name: &str,
) -> Vec<String> {
    let listed = ok(
        router,
        "ListUsersInGroup",
        &json!({ "UserPoolId": user_pool_id, "GroupName": group_name }),
    )
    .await;
    listed["Users"]
        .as_array()
        .unwrap_or_else(|| panic!("ListUsersInGroup reports its members, got {listed}"))
        .iter()
        .map(|user| {
            user["Username"]
                .as_str()
                .expect("a listed username")
                .to_string()
        })
        .collect()
}

/// One row of the deletion table.
#[derive(Clone, Copy, Debug)]
struct DeleteCase {
    /// What this row asserts.
    case: &'static str,
    /// `UsernameConfiguration.CaseSensitive` of the pool this row deletes in, or
    /// `None` for a pool that configures none.
    case_sensitive: Option<bool>,
    /// Names added to `CreateUserPool.Schema`, stored under the `custom:` prefix.
    custom_schema: &'static [&'static str],
    /// The username of the user this row deletes.
    username: &'static str,
    /// The attributes that user holds.
    attributes: &'static [(&'static str, &'static str)],
    /// The groups that user belongs to.
    groups: &'static [&'static str],
    /// How many times that user signs in before the deletion, so the row covers
    /// a user holding one access token and one holding several.
    sign_ins: usize,
}

/// The rows differ along what the deleted profile HOLDS (no attribute beyond its
/// own id, several standard ones, a custom one, a verified email), how many
/// groups it belongs to (none, one, two), how many access tokens name it at once
/// (one, three), and the form of its username against the pool's
/// case-sensitivity configuration.
const DELETE_CASES: &[DeleteCase] = &[
    DeleteCase {
        case: "a user holding several standard attributes",
        case_sensitive: None,
        custom_schema: &[],
        username: "ada",
        attributes: &[
            ("name", "Ada Lovelace"),
            ("nickname", "Ada"),
            ("zoneinfo", "UTC"),
        ],
        groups: &[],
        sign_ins: 1,
    },
    DeleteCase {
        case: "a user holding no attribute but its own id",
        case_sensitive: None,
        custom_schema: &[],
        username: "grace",
        attributes: &[],
        groups: &[],
        sign_ins: 1,
    },
    DeleteCase {
        case: "a user holding a custom attribute and a verified email",
        case_sensitive: None,
        custom_schema: &["tier"],
        username: "katherine",
        attributes: &[
            ("email", "katherine@johnson.example"),
            ("email_verified", "true"),
            ("custom:tier", "gold"),
        ],
        groups: &[],
        sign_ins: 1,
    },
    DeleteCase {
        case: "a user that belongs to two groups",
        case_sensitive: None,
        custom_schema: &[],
        username: "margaret",
        attributes: &[("nickname", "Margaret")],
        groups: &["admins", "readers"],
        sign_ins: 1,
    },
    DeleteCase {
        case: "a user that several access tokens name at once",
        case_sensitive: None,
        custom_schema: &[],
        username: "radia",
        attributes: &[("nickname", "Radia")],
        groups: &["admins"],
        sign_ins: 3,
    },
    DeleteCase {
        // A case-insensitive pool holds its users under the lowercased name, so
        // this row deletes a user whose stored key is not its username.
        case: "a mixed-case username in a case-insensitive pool",
        case_sensitive: Some(false),
        custom_schema: &[],
        username: "Barbara.Liskov",
        attributes: &[("nickname", "Barbara")],
        groups: &[],
        sign_ins: 1,
    },
    DeleteCase {
        case: "a mixed-case username in a case-sensitive pool",
        case_sensitive: Some(true),
        custom_schema: &[],
        username: "Frances_Allen",
        attributes: &[("nickname", "Frances")],
        groups: &[],
        sign_ins: 2,
    },
];

#[tokio::test]
async fn a_deletion_removes_the_signed_in_users_whole_profile_and_nothing_else() {
    let router = emulator();
    for (index, case) in DELETE_CASES.iter().enumerate() {
        let DeleteCase {
            case,
            case_sensitive,
            custom_schema,
            username,
            attributes,
            groups,
            sign_ins,
        } = case;
        let user_pool_id = pool(
            &router,
            &format!("deletion-pool-{index}"),
            *case_sensitive,
            custom_schema,
        )
        .await;
        let client_id = client(&router, &user_pool_id).await;
        // A neighbour in the same pool, so the row proves the deletion reaches
        // exactly one stored user.
        let neighbour = format!("neighbour-{index}");
        confirmed_user(
            &router,
            &user_pool_id,
            &neighbour,
            &[("nickname", "Neighbour")],
        )
        .await;
        let neighbour_token = access_tokens(&router, &client_id, &neighbour, 1).await;

        confirmed_user(&router, &user_pool_id, username, attributes).await;
        for group in *groups {
            ok(
                &router,
                "CreateGroup",
                &json!({ "UserPoolId": user_pool_id, "GroupName": group }),
            )
            .await;
            ok(
                &router,
                "AdminAddUserToGroup",
                &json!({
                    "UserPoolId": user_pool_id,
                    "Username": username,
                    "GroupName": group,
                }),
            )
            .await;
        }
        let tokens = access_tokens(&router, &client_id, username, *sign_ins).await;

        let (status, payload) = delete(&router, &tokens[0]).await;
        assert_eq!(
            (status, &payload),
            (200, &json!({})),
            "case {case:?}: DeleteUser must succeed with an empty body, got {status} with {payload}"
        );

        // The round trip: the stored profile is gone, administratively...
        let (status, payload) = admin_get_user(&router, &user_pool_id, username).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "UserNotFoundException"),
            "case {case:?}: expected 400 UserNotFoundException reading the deleted user, \
             got {status} with {payload}"
        );
        // ... and through every access token that still names it, the one the
        // deletion was made with included.
        for (position, token) in tokens.iter().enumerate() {
            let (status, payload) = get_user(&router, token).await;
            assert_eq!(
                (status, error_code(&payload)),
                (400, "UserNotFoundException"),
                "case {case:?}: expected 400 UserNotFoundException from GetUser on access token \
                 {position} of the deleted user, got {status} with {payload}"
            );
        }
        // A second deletion with the same token finds no profile to delete.
        let (status, payload) = delete(&router, &tokens[0]).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "UserNotFoundException"),
            "case {case:?}: expected 400 UserNotFoundException deleting the same user twice, \
             got {status} with {payload}"
        );
        // A deleted profile "can no longer be used to sign in".
        let (status, payload) = initiate_auth(&router, &client_id, username).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "UserNotFoundException"),
            "case {case:?}: expected 400 UserNotFoundException signing the deleted user in, \
             got {status} with {payload}"
        );
        // Its group memberships went with it.
        for group in *groups {
            let members = group_members(&router, &user_pool_id, group).await;
            assert!(
                !members.contains(&username.to_string()),
                "case {case:?}: {group} must no longer list the deleted user, got {members:?}"
            );
        }

        // The neighbour is untouched: it reads back, holds its attribute, and
        // its own access token still names it.
        let (status, payload) = admin_get_user(&router, &user_pool_id, &neighbour).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the neighbour must still read back, got {status} with {payload}"
        );
        let (status, own) = get_user(&router, &neighbour_token[0]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the neighbour's own token must still name it, got {status} with {own}"
        );
        assert_eq!(
            own["Username"],
            json!(neighbour),
            "case {case:?}: the neighbour's token must name the neighbour, got {own}"
        );
    }
}

#[tokio::test]
async fn a_deleted_profile_is_not_restored_by_creating_its_username_again() {
    let router = emulator();
    let user_pool_id = pool(&router, "recreate-pool", None, &[]).await;
    let client_id = client(&router, &user_pool_id).await;
    confirmed_user(&router, &user_pool_id, "ada", &[("nickname", "Ada")]).await;
    let token = access_tokens(&router, &client_id, "ada", 1).await.remove(0);

    let before = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "ada" }),
    )
    .await;
    let first_sub = sub_of(&before["UserAttributes"]);

    let (status, payload) = delete(&router, &token).await;
    assert_eq!(
        status, 200,
        "the deletion must succeed, got {status} with {payload}"
    );

    // The same username is free again, and taking it mints a DIFFERENT user.
    confirmed_user(&router, &user_pool_id, "ada", &[("nickname", "Another Ada")]).await;
    let after = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "ada" }),
    )
    .await;
    let second_sub = sub_of(&after["UserAttributes"]);
    assert_ne!(
        second_sub, first_sub,
        "re-creating the username must mint a different user, got sub {second_sub} for both"
    );

    // The deleted profile is gone for good: the token issued to it resolves to
    // no user rather than to its namesake.
    let (status, payload) = get_user(&router, &token).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "UserNotFoundException"),
        "the deleted user's token must not name the user created after it, \
         got {status} with {payload}"
    );
}

#[tokio::test]
async fn an_administrative_deletion_leaves_the_users_token_naming_a_user_that_is_gone() {
    // The same rule, through the other operation that deletes a user profile:
    // `AdminDeleteUser` names its subject administratively, and the access token
    // the deleted user still holds is answered for the user that is gone.
    let router = emulator();
    let user_pool_id = pool(&router, "admin-deletion-pool", None, &[]).await;
    let client_id = client(&router, &user_pool_id).await;
    confirmed_user(&router, &user_pool_id, "hedy", &[("nickname", "Hedy")]).await;
    let token = access_tokens(&router, &client_id, "hedy", 1).await.remove(0);

    let (status, own) = get_user(&router, &token).await;
    assert_eq!(
        (status, &own["Username"]),
        (200, &json!("hedy")),
        "the token must name its user before the deletion, got {status} with {own}"
    );

    let empty = ok(
        &router,
        "AdminDeleteUser",
        &json!({ "UserPoolId": user_pool_id, "Username": "hedy" }),
    )
    .await;
    assert_eq!(
        empty,
        json!({}),
        "AdminDeleteUser answers an empty body, got {empty}"
    );

    for (case, status_and_code) in [
        ("GetUser with the deleted user's own access token", {
            let (status, payload) = get_user(&router, &token).await;
            (status, error_code(&payload).to_string())
        }),
        ("DeleteUser with the deleted user's own access token", {
            let (status, payload) = delete(&router, &token).await;
            (status, error_code(&payload).to_string())
        }),
        ("AdminGetUser for the deleted user", {
            let (status, payload) = admin_get_user(&router, &user_pool_id, "hedy").await;
            (status, error_code(&payload).to_string())
        }),
    ] {
        assert_eq!(
            (status_and_code.0, status_and_code.1.as_str()),
            (400, "UserNotFoundException"),
            "case {case:?}: expected 400 UserNotFoundException, got {status_and_code:?}"
        );
    }
}

#[tokio::test]
async fn a_token_that_names_no_signed_in_user_is_refused_and_deletes_nothing() {
    let router = emulator();
    let user_pool_id = pool(&router, "token-pool", None, &[]).await;
    let client_id = client(&router, &user_pool_id).await;
    confirmed_user(&router, &user_pool_id, "dave", &[("nickname", "Dave")]).await;
    let result = sign_in(&router, &client_id, "dave").await;
    let id_token = result["IdToken"].as_str().expect("an id token").to_string();
    let refresh_token = result["RefreshToken"]
        .as_str()
        .expect("a refresh token")
        .to_string();

    for (case, token) in [
        (
            "a token this emulator never issued",
            "not-an-issued-token".to_string(),
        ),
        (
            "the id token of the same sign-in, which is not an access token",
            id_token,
        ),
        (
            "the refresh token of the same sign-in, which is not an access token",
            refresh_token,
        ),
    ] {
        let (status, payload) = delete(&router, &token).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "NotAuthorizedException"),
            "case {case:?}: expected 400 NotAuthorizedException, got {status} with {payload}"
        );
    }

    // Every refused call left the stored user where it was.
    let (status, payload) = admin_get_user(&router, &user_pool_id, "dave").await;
    assert_eq!(
        status, 200,
        "a refused deletion must write nothing, got {status} with {payload}"
    );
    let (status, signed_in) = get_user(
        &router,
        result["AccessToken"].as_str().expect("an access token"),
    )
    .await;
    assert_eq!(
        (status, &signed_in["Username"]),
        (200, &json!("dave")),
        "the signed-in user must still be readable through its own token, got {signed_in}"
    );
}
