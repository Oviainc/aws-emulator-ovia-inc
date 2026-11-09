// SPDX-License-Identifier: CC0-1.0

//! Which NAMES a user pool resolves one of its users by, when the pool compares
//! usernames without case.
//!
//! One rule is under test: `UsernameConfiguration.CaseSensitive` set to `false`
//! "also enables both `preferred_username` and `email` alias to be case
//! insensitive, in addition to the `username` attribute" (this service's model),
//! so in such a pool a lookup resolves the user that holds the value in ANY
//! capitalization, and that one value names ONE user however it is capitalized —
//! while a pool that compares usernames case-sensitively resolves only the
//! spelling it stored. The comparison alone folds case: a read still reports the
//! value the write submitted.
//!
//! Each table row names the case it asserts.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target`
/// prefix a client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// The attribute that records whether a stored `email` was confirmed.
const EMAIL_FLAG: &str = "email_verified";

/// The attribute that records whether a stored `phone_number` was confirmed.
const PHONE_FLAG: &str = "phone_number_verified";

/// The alias attributes the pools in this suite configure: the two the pool's
/// case setting reaches, plus one whose values carry no case at all.
const ALIAS_ATTRIBUTES: [&str; 3] = ["email", "preferred_username", "phone_number"];

/// One emulator with just this service mounted, serving unauthenticated requests
/// so a test signs nothing.
fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(engine));
    router.set_request_authentication(false);
    router
}

/// Dispatch `operation` with `body` and return the status and JSON response body.
async fn dispatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/us-east-1/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, payload)
}

/// Call `operation`, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, operation, body).await;
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

/// Create a pool aliasing [`ALIAS_ATTRIBUTES`] whose `UsernameConfiguration`
/// compares usernames as `case_sensitive` says, and return its id.
async fn pool(router: &MultiServiceRouter, pool_name: &str, case_sensitive: bool) -> String {
    let created = ok(
        router,
        "CreateUserPool",
        &json!({
            "PoolName": pool_name,
            "AliasAttributes": ALIAS_ATTRIBUTES,
            "UsernameConfiguration": { "CaseSensitive": case_sensitive },
        }),
    )
    .await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// `AdminCreateUser` for `username` with `attributes`, returning the status and
/// body so a refusal is a row's outcome rather than a panic.
async fn create_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
    attributes: &[(&str, &str)],
) -> (u16, Value) {
    let list: Vec<Value> = attributes
        .iter()
        .map(|(name, value)| json!({ "Name": name, "Value": value }))
        .collect();
    dispatch(
        router,
        "AdminCreateUser",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "UserAttributes": list,
        }),
    )
    .await
}

/// A user's attribute list, as a name-to-value map: `AdminGetUser` carries it
/// as `UserAttributes` and a `UserType` as `Attributes`, so the caller passes
/// whichever member it holds.
fn attribute_map(list: &Value) -> BTreeMap<String, String> {
    let mut stored = BTreeMap::new();
    for attribute in list.as_array().into_iter().flatten() {
        let name = attribute["Name"].as_str().unwrap_or_default().to_string();
        let value = attribute["Value"].as_str().unwrap_or_default().to_string();
        stored.insert(name, value);
    }
    stored
}

/// `AdminGetUser` for `username`: the status and body, so a refusal is a row's
/// outcome rather than a panic.
async fn read_user(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> (u16, Value) {
    dispatch(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await
}

/// What a row expects of the lookup it makes.
#[derive(Clone, Copy, Debug)]
enum Expected {
    /// The lookup resolves the one user the pool holds.
    Resolves,
    /// The lookup names no user of the pool at all.
    NoUser,
}

/// The mixed-case spellings one user is created with. The stored user is read
/// back by each row's own capitalization of them.
const USERNAME: &str = "Grace-Hopper";
const EMAIL: &str = "Grace.Hopper@Example.com";
const PREFERRED_USERNAME: &str = "Amazing-Grace";
const PHONE_NUMBER: &str = "+12065550100";
const FAMILY_NAME: &str = "Hopper";

/// RULE: a pool that compares usernames without case resolves a user by any
/// capitalization of its username and of its `email` or `preferred_username`
/// alias value, while a case-sensitive pool resolves only the stored spelling —
/// over a table whose rows differ along the pool's case setting, WHICH name the
/// lookup uses, and the capitalization it uses it in.
#[tokio::test]
async fn a_pool_resolves_its_aliases_the_way_its_case_setting_says() {
    let router = router();
    let insensitive = pool(&router, "case-insensitive-aliases", false).await;
    let sensitive = pool(&router, "case-sensitive-aliases", true).await;

    // The same user, with the same mixed-case values, in each pool: an active
    // alias needs its verified flag, so both contact values are vouched for.
    let attributes = [
        ("email", EMAIL),
        (EMAIL_FLAG, "true"),
        ("preferred_username", PREFERRED_USERNAME),
        ("phone_number", PHONE_NUMBER),
        (PHONE_FLAG, "true"),
        ("family_name", FAMILY_NAME),
    ];
    let mut stored_username = BTreeMap::new();
    let mut stored_sub = BTreeMap::new();
    for user_pool_id in [&insensitive, &sensitive] {
        let (status, created) = create_user(&router, user_pool_id, USERNAME, &attributes).await;
        assert_eq!(
            status, 200,
            "the pool must take the user, got {status} with {created}"
        );
        let username = created["User"]["Username"]
            .as_str()
            .expect("a created user carries a username")
            .to_string();
        let sub = attribute_map(&created["User"]["Attributes"])
            .get("sub")
            .expect("a created user carries its sub")
            .clone();
        stored_username.insert(user_pool_id.clone(), username);
        stored_sub.insert(user_pool_id.clone(), sub);
    }
    // A pool that compares usernames without case stores the folded spelling,
    // and one that compares them with case stores the spelling as sent.
    assert_eq!(
        stored_username.get(&insensitive).map(String::as_str),
        Some(USERNAME.to_lowercase().as_str()),
        "case \"the stored username of a case-insensitive pool\": expected the folded \
         spelling, got {stored_username:?}"
    );
    assert_eq!(
        stored_username.get(&sensitive).map(String::as_str),
        Some(USERNAME),
        "case \"the stored username of a case-sensitive pool\": expected the submitted \
         spelling, got {stored_username:?}"
    );

    // (case, pool, the name the lookup uses, what it must resolve to)
    let rows: Vec<(&str, &String, String, Expected)> = vec![
        (
            "a case-insensitive pool, the username as submitted",
            &insensitive,
            USERNAME.to_string(),
            Expected::Resolves,
        ),
        (
            "a case-insensitive pool, the username upper-cased",
            &insensitive,
            USERNAME.to_uppercase(),
            Expected::Resolves,
        ),
        (
            "a case-insensitive pool, the email alias as submitted",
            &insensitive,
            EMAIL.to_string(),
            Expected::Resolves,
        ),
        (
            "a case-insensitive pool, the email alias lower-cased",
            &insensitive,
            EMAIL.to_lowercase(),
            Expected::Resolves,
        ),
        (
            "a case-insensitive pool, the email alias upper-cased",
            &insensitive,
            EMAIL.to_uppercase(),
            Expected::Resolves,
        ),
        (
            "a case-insensitive pool, the preferred_username alias as submitted",
            &insensitive,
            PREFERRED_USERNAME.to_string(),
            Expected::Resolves,
        ),
        (
            "a case-insensitive pool, the preferred_username alias lower-cased",
            &insensitive,
            PREFERRED_USERNAME.to_lowercase(),
            Expected::Resolves,
        ),
        (
            // A number carries no case to fold, so this alias resolves the same
            // way in either pool.
            "a case-insensitive pool, the phone_number alias",
            &insensitive,
            PHONE_NUMBER.to_string(),
            Expected::Resolves,
        ),
        (
            // The setting reaches the username and the two aliases the model
            // names, and nothing else: a value of an attribute that is no
            // sign-in name of the pool names no user whatever its case.
            "a case-insensitive pool, a value of an attribute it does not alias",
            &insensitive,
            FAMILY_NAME.to_lowercase(),
            Expected::NoUser,
        ),
        (
            "a case-insensitive pool, an address no user holds",
            &insensitive,
            "ada@example.com".to_string(),
            Expected::NoUser,
        ),
        (
            "a case-sensitive pool, the username as submitted",
            &sensitive,
            USERNAME.to_string(),
            Expected::Resolves,
        ),
        (
            "a case-sensitive pool, the username upper-cased",
            &sensitive,
            USERNAME.to_uppercase(),
            Expected::NoUser,
        ),
        (
            "a case-sensitive pool, the email alias as submitted",
            &sensitive,
            EMAIL.to_string(),
            Expected::Resolves,
        ),
        (
            "a case-sensitive pool, the email alias lower-cased",
            &sensitive,
            EMAIL.to_lowercase(),
            Expected::NoUser,
        ),
        (
            "a case-sensitive pool, the preferred_username alias as submitted",
            &sensitive,
            PREFERRED_USERNAME.to_string(),
            Expected::Resolves,
        ),
        (
            "a case-sensitive pool, the preferred_username alias lower-cased",
            &sensitive,
            PREFERRED_USERNAME.to_lowercase(),
            Expected::NoUser,
        ),
        (
            "a case-sensitive pool, the phone_number alias",
            &sensitive,
            PHONE_NUMBER.to_string(),
            Expected::Resolves,
        ),
    ];

    for (case, user_pool_id, addressed_by, expected) in &rows {
        let (status, payload) = read_user(&router, user_pool_id, addressed_by).await;
        match expected {
            Expected::Resolves => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the stored user, got {status} with {payload}"
                );
                // However the lookup spelled the name, the read reports the
                // STORED username — so a client keying a cache on it gets one
                // value per user.
                assert_eq!(
                    payload["Username"].as_str(),
                    stored_username.get(*user_pool_id).map(String::as_str),
                    "case {case:?}: expected the stored username, got {payload}"
                );
                // The comparison folds case; the stored value does not.
                let stored = attribute_map(&payload["UserAttributes"]);
                for (name, submitted) in [
                    ("email", EMAIL),
                    ("preferred_username", PREFERRED_USERNAME),
                    ("phone_number", PHONE_NUMBER),
                ] {
                    assert_eq!(
                        stored.get(name).map(String::as_str),
                        Some(submitted),
                        "case {case:?}: expected {name} to keep the submitted spelling, \
                         got {stored:?}"
                    );
                }
            }
            Expected::NoUser => {
                assert!(
                    (400..500).contains(&status),
                    "case {case:?}: expected a client-error refusal, got {status} with {payload}"
                );
                assert_eq!(
                    error_code(&payload),
                    "UserNotFoundException",
                    "case {case:?}: expected UserNotFoundException, got {payload}"
                );
            }
        }
    }

    // A user's own `sub` names it in either pool, whatever the pool does with
    // case: the id is the service's, not a spelling the caller chose.
    for user_pool_id in [&insensitive, &sensitive] {
        let sub = stored_sub.get(user_pool_id).expect("a stored sub").clone();
        let (status, payload) = read_user(&router, user_pool_id, &sub).await;
        assert_eq!(
            status, 200,
            "case \"a lookup by the user's own sub\": expected the stored user, got {status} \
             with {payload}"
        );
        assert_eq!(
            payload["Username"].as_str(),
            stored_username.get(user_pool_id).map(String::as_str),
            "case \"a lookup by the user's own sub\": expected the stored username, got {payload}"
        );
    }
}

/// RULE: an alias value names ONE user of the pool in every capitalization the
/// pool compares without case, so a second user cannot be given a case variation
/// of a value another user already answers to, and an update that moves the
/// value takes the alias from the user that held it — over rows that differ along
/// the pool's case setting and the capitalization the second user submits.
#[tokio::test]
async fn an_alias_value_names_one_user_in_every_capitalization_the_pool_folds() {
    let router = router();
    let insensitive = pool(&router, "case-insensitive-alias-claims", false).await;
    let sensitive = pool(&router, "case-sensitive-alias-claims", true).await;

    let held = "Ada.Lovelace@Example.com";
    for user_pool_id in [&insensitive, &sensitive] {
        let (status, created) = create_user(
            &router,
            user_pool_id,
            "holder",
            &[("email", held), (EMAIL_FLAG, "true")],
        )
        .await;
        assert_eq!(
            status, 200,
            "the pool must take the alias holder, got {status} with {created}"
        );
    }

    // (case, pool, the address the second create submits, whether it is refused)
    let rows: Vec<(&str, &String, String, bool)> = vec![
        (
            "a case-insensitive pool and the address as the holder submitted it",
            &insensitive,
            held.to_string(),
            true,
        ),
        (
            "a case-insensitive pool and the same address lower-cased",
            &insensitive,
            held.to_lowercase(),
            true,
        ),
        (
            "a case-insensitive pool and the same address upper-cased",
            &insensitive,
            held.to_uppercase(),
            true,
        ),
        (
            "a case-sensitive pool and the address as the holder submitted it",
            &sensitive,
            held.to_string(),
            true,
        ),
        (
            // A pool that compares with case holds a different value here, so
            // the create is no duplicate at all.
            "a case-sensitive pool and the same address lower-cased",
            &sensitive,
            held.to_lowercase(),
            false,
        ),
    ];

    for (index, (case, user_pool_id, address, refused)) in rows.iter().enumerate() {
        let (status, payload) = create_user(
            &router,
            user_pool_id,
            &format!("claimant-{index}"),
            &[("email", address), (EMAIL_FLAG, "true")],
        )
        .await;
        if *refused {
            assert!(
                (400..500).contains(&status),
                "case {case:?}: expected a client-error refusal, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "UsernameExistsException",
                "case {case:?}: expected UsernameExistsException, got {payload}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to be taken, got {status} with {payload}"
            );
        }
    }

    // An UPDATE that vouches for a case variation of the held address moves the
    // alias: the new holder answers to every capitalization of it, and the user
    // it was taken from keeps its own account, with its flag no longer vouching
    // for a value that is now somebody else's.
    let (status, created) = create_user(&router, &insensitive, "successor", &[]).await;
    assert_eq!(
        status, 200,
        "the pool must take the successor, got {status} with {created}"
    );
    ok(
        &router,
        "AdminUpdateUserAttributes",
        &json!({
            "UserPoolId": insensitive,
            "Username": "successor",
            "UserAttributes": [
                { "Name": "email", "Value": held.to_uppercase() },
                { "Name": EMAIL_FLAG, "Value": "true" },
            ],
        }),
    )
    .await;
    for (case, addressed_by) in [
        ("the address as the first holder submitted it", held.to_string()),
        ("the address lower-cased", held.to_lowercase()),
        ("the address upper-cased", held.to_uppercase()),
    ] {
        let (status, payload) = read_user(&router, &insensitive, &addressed_by).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the new holder, got {status} with {payload}"
        );
        assert_eq!(
            payload["Username"].as_str(),
            Some("successor"),
            "case {case:?}: expected the alias to name the new holder, got {payload}"
        );
    }
    let demoted = ok(
        &router,
        "AdminGetUser",
        &json!({ "UserPoolId": insensitive, "Username": "holder" }),
    )
    .await;
    let stored = attribute_map(&demoted["UserAttributes"]);
    assert_eq!(
        stored.get("email").map(String::as_str),
        Some(held),
        "case \"the user the alias moved away from\": expected its own address to be kept, \
         got {stored:?}"
    );
    assert_eq!(
        stored.get(EMAIL_FLAG).map(String::as_str),
        Some("false"),
        "case \"the user the alias moved away from\": expected {EMAIL_FLAG} to stop vouching \
         for the address, got {stored:?}"
    );
}
