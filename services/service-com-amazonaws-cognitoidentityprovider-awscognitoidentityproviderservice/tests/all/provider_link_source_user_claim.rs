// SPDX-License-Identifier: CC0-1.0

//! Which refusal `AdminLinkProviderForUser` answers for a `SourceUser` the pool already
//! resolves, driven over the wire exactly as an SDK drives it (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.AdminLinkProviderForUser`).
//!
//! The rule under test: a `SourceUser` must be "an external IdP account for a user who
//! doesn't exist yet in the user pool ... a federated user ..., not another native user"
//! (`smithy_query(query="doc",shape="AdminLinkProviderForUserRequest",member="SourceUser")`),
//! and a stored link makes its identity resolve to the local profile it points at. So a
//! second link of one identity is refused for WHAT that identity resolves to: the link
//! itself when the request names the profile it already points at; an invalid `SourceUser`
//! (`InvalidParameterException`) when it points the identity at another profile and the
//! profile it resolves to signs in with a username and a password, which is what that
//! sentence reports and what the native `Cognito` provider reports outright; and the alias
//! collision `AliasExistsException` models — "an ... alias for a different user profile"
//! (that error in this service's model) — when the profile it resolves to holds no
//! password and so is no such user, which is the state
//! `AdminDisableProviderForUser` leaves a local profile in.
//!
//! Every pool, profile, identity provider, password and link is real: they come from
//! `CreateUserPool`, `AdminCreateUser`, `AdminSetUserPassword`, `CreateIdentityProvider`,
//! `AdminLinkProviderForUser` and `AdminDisableProviderForUser`, and each stored link is
//! read back through `AdminGetUser`.
//!
//! Each table row names the case it asserts and every assertion reports the actual value.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The signature is
/// never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy, so a local profile is one that
/// signs in with a username and a password.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The provider name a pool's OWN users sign in under: "the `ProviderName` should be set
/// to `Cognito` for users in Cognito user pools"
/// (`smithy_query(query="doc",shape="AdminLinkProviderForUserRequest",member="DestinationUser")`).
const NATIVE_PROVIDER: &str = "Cognito";

/// The `ProviderAttributeName` a social or SAML identity is named by: "you must set the
/// `ProviderAttributeName` to `Cognito_Subject`"
/// (`smithy_query(query="doc",shape="AdminLinkProviderForUserRequest",member="SourceUser")`).
const SUBJECT_ATTRIBUTE: &str = "Cognito_Subject";

/// The sentence a `SourceUser` that resolves to a local profile which signs in with a
/// username and a password is refused with.
const INVALID_SOURCE_USER: &str = "Invalid SourceUser: Cognito users with a username/password \
     may not be passed in as a SourceUser, only as a DestinationUser";

/// The sentence a repeat of the link an identity already holds is refused with.
const ALREADY_LINKED: &str = "SourceUser is already linked to DestinationUser";

/// The sentence a link of an identity another local profile already answers for is
/// refused with.
const LINKED_TO_ANOTHER_USER: &str =
    "SourceUser with the specified attribute value is already linked to another user";

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

/// The `message` of an error body.
fn error_message(payload: &Value) -> &str {
    payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
}

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// Create the local profile `username` in `user_pool_id`, with no password of its own.
async fn user(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
    ok(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
}

/// Give `username` a permanent password, making it a profile that signs in with a
/// username and a password.
async fn set_password(router: &MultiServiceRouter, user_pool_id: &str, username: &str) {
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

/// Configure `provider_name` of type `provider_type` on `user_pool_id`, so an identity of
/// that provider is one the pool can link.
async fn provider(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    provider_name: &str,
    provider_type: &str,
) {
    ok(
        router,
        "CreateIdentityProvider",
        &json!({
            "UserPoolId": user_pool_id,
            "ProviderName": provider_name,
            "ProviderType": provider_type,
            "ProviderDetails": { "MetadataURL": "https://idp.example.test/metadata" },
        }),
    )
    .await;
}

/// A `ProviderUserIdentifierType` naming every member.
fn identity(provider_name: &str, attribute_name: &str, attribute_value: &str) -> Value {
    json!({
        "ProviderName": provider_name,
        "ProviderAttributeName": attribute_name,
        "ProviderAttributeValue": attribute_value,
    })
}

/// Link `source` to the local profile `destination` in `user_pool_id`.
async fn link(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    source: &Value,
    destination: &str,
) -> (u16, Value) {
    call(
        router,
        "AdminLinkProviderForUser",
        &json!({
            "UserPoolId": user_pool_id,
            "SourceUser": source,
            "DestinationUser": {
                "ProviderName": NATIVE_PROVIDER,
                "ProviderAttributeValue": destination,
            },
        }),
    )
    .await
}

/// Deactivate `source` in `user_pool_id`.
async fn disable(router: &MultiServiceRouter, user_pool_id: &str, source: &Value) -> (u16, Value) {
    call(
        router,
        "AdminDisableProviderForUser",
        &json!({ "UserPoolId": user_pool_id, "User": source }),
    )
    .await
}

/// The `providerName`/`userId` pairs `username`'s `identities` attribute renders, which is
/// how a STORED link is recognised whatever else it carries.
async fn linked_pairs(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> Vec<String> {
    let payload = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    let rendered = payload["UserAttributes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|attribute| attribute["Name"] == "identities")
        .and_then(|attribute| attribute["Value"].as_str().map(str::to_string));
    let Some(rendered) = rendered else {
        return Vec::new();
    };
    let entries: Vec<Value> = serde_json::from_str(&rendered)
        .unwrap_or_else(|error| panic!("the identities attribute must be JSON ({error})"));
    entries
        .iter()
        .map(|entry| {
            format!(
                "{}/{}",
                entry["providerName"].as_str().unwrap_or("<none>"),
                entry["userId"].as_str().unwrap_or("<none>")
            )
        })
        .collect()
}

/// One row of the identity table: the federated identity a link is made for.
struct IdentityCase {
    case: &'static str,
    provider_name: &'static str,
    provider_type: &'static str,
    attribute_name: &'static str,
    attribute_value: &'static str,
}

/// The rows differ along the IdP the identity federates through — the provider types a
/// pool can hold (`smithy_query(query="shape",shape="IdentityProviderTypeType")` lists
/// `SAML`, `Facebook`, `Google`, `LoginWithAmazon`, `SignInWithApple`, `OIDC`) — along the
/// ATTRIBUTE it is named by (the documented subject, a mapped claim, a custom claim) and
/// along the FORM of its value (an opaque id, an email address, a non-ASCII value, a long
/// one).
const IDENTITY_CASES: &[IdentityCase] = &[
    IdentityCase {
        case: "a SAML identity named by a mapped email claim",
        provider_name: "CorpSAML",
        provider_type: "SAML",
        attribute_name: "email",
        attribute_value: "ada@idp.example.test",
    },
    IdentityCase {
        case: "a Facebook identity named by its subject",
        provider_name: "Facebook",
        provider_type: "Facebook",
        attribute_name: SUBJECT_ATTRIBUTE,
        attribute_value: "10224266394238161",
    },
    IdentityCase {
        case: "a Google identity named by a non-ASCII subject",
        provider_name: "Google",
        provider_type: "Google",
        attribute_name: SUBJECT_ATTRIBUTE,
        attribute_value: "ग्रेस-होपर",
    },
    IdentityCase {
        case: "an OIDC identity named by a custom claim carrying a long opaque id",
        provider_name: "CorpOIDC",
        provider_type: "OIDC",
        attribute_name: "corp_person_id",
        attribute_value: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    },
    IdentityCase {
        case: "a Login with Amazon identity named by its subject",
        provider_name: "LoginWithAmazon",
        provider_type: "LoginWithAmazon",
        attribute_name: SUBJECT_ATTRIBUTE,
        attribute_value: "amzn1.account.AFEDCBA9876543210",
    },
    IdentityCase {
        case: "a Sign in with Apple identity named by its subject",
        provider_name: "SignInWithApple",
        provider_type: "SignInWithApple",
        attribute_name: SUBJECT_ATTRIBUTE,
        attribute_value: "001234.fedcba9876543210.0001",
    },
];

#[tokio::test]
async fn a_second_link_is_refused_for_what_the_source_identity_resolves_to() {
    let router = router();
    let user_pool_id = pool(&router, "source-user-pool").await;
    for case in IDENTITY_CASES {
        provider(
            &router,
            &user_pool_id,
            case.provider_name,
            case.provider_type,
        )
        .await;
    }

    for (index, case) in IDENTITY_CASES.iter().enumerate() {
        let label = case.case;
        // One pair of profiles per row, so each row starts from an identity and two
        // profiles nothing has linked yet, both signing in with a password.
        let held = format!("held-{index}");
        let other = format!("other-{index}");
        for username in [&held, &other] {
            user(&router, &user_pool_id, username).await;
            set_password(&router, &user_pool_id, username).await;
        }
        let source = identity(case.provider_name, case.attribute_name, case.attribute_value);
        let stored_pair = format!("{}/{}", case.provider_name, case.attribute_value);

        let (status, payload) = link(&router, &user_pool_id, &source, &held).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the first link must be served, got {status} with {payload}"
        );
        // ROUND TRIP: the link is STORED, so the profile renders the identity.
        let pairs = linked_pairs(&router, &user_pool_id, &held).await;
        assert_eq!(
            pairs,
            vec![stored_pair.clone()],
            "case {label:?}: the linked profile must render the identity, got {pairs:?}"
        );

        // The link makes the identity resolve to a local profile that signs in with a
        // password, so submitting it as a SourceUser for ANOTHER profile submits no
        // external account at all — the same refusal the native provider gets, which the
        // next assertion pins beside it.
        let (status, payload) = link(&router, &user_pool_id, &source, &other).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidParameterException", INVALID_SOURCE_USER),
            "case {label:?}: a linked identity is a local profile's, got {status} with {payload}"
        );
        let native = identity(NATIVE_PROVIDER, SUBJECT_ATTRIBUTE, &held);
        let (status, payload) = link(&router, &user_pool_id, &native, &other).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidParameterException", INVALID_SOURCE_USER),
            "case {label:?}: a native SourceUser links nothing, got {status} with {payload}"
        );

        // A call that names the profile the identity ALREADY links to asks for the link
        // the pool holds, and is told so.
        let (status, payload) = link(&router, &user_pool_id, &source, &held).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidParameterException", ALREADY_LINKED),
            "case {label:?}: a repeat of the stored link says so, got {status} with {payload}"
        );

        // Deactivating the profile's NATIVE sign-in withdraws its password and leaves the
        // link standing, so the identity now resolves to a profile that is no user pool
        // user with a username and a password: what is left to report is that another
        // profile already answers for the identity.
        let (status, payload) = disable(&router, &user_pool_id, &native).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the native sign-in must be deactivatable, got {status} with {payload}"
        );
        let pairs = linked_pairs(&router, &user_pool_id, &held).await;
        assert_eq!(
            pairs,
            vec![stored_pair.clone()],
            "case {label:?}: the link stands through that deactivation, got {pairs:?}"
        );
        let (status, payload) = link(&router, &user_pool_id, &source, &other).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "AliasExistsException", LINKED_TO_ANOTHER_USER),
            "case {label:?}: the identity is another profile's, got {status} with {payload}"
        );
        let pairs = linked_pairs(&router, &user_pool_id, &other).await;
        assert!(
            pairs.is_empty(),
            "case {label:?}: the refused link stored nothing, got {pairs:?}"
        );

        // The class follows the STATE and nothing else: a password set again makes the
        // linked profile a username/password user again, and the refusal goes back to
        // reporting the invalid source.
        set_password(&router, &user_pool_id, &held).await;
        let (status, payload) = link(&router, &user_pool_id, &source, &other).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidParameterException", INVALID_SOURCE_USER),
            "case {label:?}: the restored password restores the class, got {status} with {payload}"
        );

        // Deactivating the IDENTITY removes the link, so the identity resolves to no
        // local profile and links where the caller points it next.
        let (status, payload) = disable(&router, &user_pool_id, &source).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the stored link must be removable, got {status} with {payload}"
        );
        let (status, payload) = link(&router, &user_pool_id, &source, &other).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a de-linked identity links again, got {status} with {payload}"
        );
        let pairs = linked_pairs(&router, &user_pool_id, &other).await;
        assert_eq!(
            pairs,
            vec![stored_pair],
            "case {label:?}: the new link is stored on the new profile, got {pairs:?}"
        );
        let pairs = linked_pairs(&router, &user_pool_id, &held).await;
        assert!(
            pairs.is_empty(),
            "case {label:?}: the profile it left renders no identity, got {pairs:?}"
        );
    }
}

#[tokio::test]
async fn a_profile_with_no_password_is_not_a_username_password_user() {
    let router = router();
    let user_pool_id = pool(&router, "passwordless-pool").await;
    provider(&router, &user_pool_id, "CorpSAML", "SAML").await;
    let source = identity("CorpSAML", "email", "grace@idp.example.test");

    // The rows differ along WHY the profile the identity resolves to holds no password:
    // one was created without one, the other had the one it was given withdrawn by a
    // deactivation of its native sign-in.
    for (case, held, other, withdraw) in [
        ("a profile created with no password", "held", "other", false),
        (
            "a profile whose password was withdrawn",
            "withdrawn",
            "spare",
            true,
        ),
    ] {
        for username in [held, other] {
            user(&router, &user_pool_id, username).await;
        }
        if withdraw {
            set_password(&router, &user_pool_id, held).await;
        }
        let (status, payload) = link(&router, &user_pool_id, &source, held).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the link must be served, got {status} with {payload}"
        );
        if withdraw {
            let (status, payload) = disable(
                &router,
                &user_pool_id,
                &identity(NATIVE_PROVIDER, SUBJECT_ATTRIBUTE, held),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the native sign-in must be deactivatable, got {status} with \
                 {payload}"
            );
        }

        // No password, so the profile the identity resolves to is not a user pool user
        // with a username and a password: the refusal reports the alias instead.
        let (status, payload) = link(&router, &user_pool_id, &source, other).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "AliasExistsException", LINKED_TO_ANOTHER_USER),
            "case {case:?}: expected the alias collision, got {status} with {payload}"
        );
        // The profile the link points at is still the only holder, and the request that
        // named it is still told it holds the link.
        let (status, payload) = link(&router, &user_pool_id, &source, held).await;
        assert_eq!(
            (status, error_code(&payload), error_message(&payload)),
            (400, "InvalidParameterException", ALREADY_LINKED),
            "case {case:?}: a repeat of the stored link says so, got {status} with {payload}"
        );

        // Leave the pool with no link, so the next row starts from an unlinked identity.
        let (status, payload) = disable(&router, &user_pool_id, &source).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the stored link must be removable, got {status} with {payload}"
        );
    }
}
