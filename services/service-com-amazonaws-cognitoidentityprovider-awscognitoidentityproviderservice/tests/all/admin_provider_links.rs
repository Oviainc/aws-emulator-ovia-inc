// SPDX-License-Identifier: CC0-1.0

//! `AdminLinkProviderForUser` and `AdminDisableProviderForUser` over the wire,
//! driven exactly as an SDK drives them (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: deactivating a provider for a user acts on a stored
//! LINK between a provider identity — the (`ProviderName`,
//! `ProviderAttributeName`, `ProviderAttributeValue`) triple — and the local
//! profile a link resolves to. A link made for an external IdP identity is
//! removed by a deactivation of exactly that identity; a deactivation of the
//! native `Cognito` provider withdraws the local profile's password sign-in
//! while its links stand; and a request naming an account the pool holds no
//! link for is refused with a 400 `InvalidParameterException` carrying
//! "Account isn't linked to anyone else, verify that the account has been
//! linked" and stores nothing.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// A password satisfying a default pool's password policy, so a local profile
/// can sign in with it before its native sign-in is withdrawn.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// The sentence a deactivation answers for an account the pool holds no link
/// for.
const NOT_LINKED_MESSAGE: &str =
    "Account isn't linked to anyone else, verify that the account has been linked";

/// The `ProviderName` that names a pool's own users rather than a third-party
/// IdP: "to deactivate a local user, set `ProviderName` to `Cognito`"
/// (`smithy_query(query="doc",shape="AdminDisableProviderForUser")`).
const NATIVE_PROVIDER: &str = "Cognito";

/// The `ProviderAttributeName` that names an identity by its subject: "the
/// `ProviderAttributeName` must always be `Cognito_Subject` for social IdPs"
/// (`smithy_query(query="doc",shape="AdminDisableProviderForUser")`).
const SUBJECT_ATTRIBUTE: &str = "Cognito_Subject";

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

/// Configure `provider_name` of type `provider_type` on `user_pool_id`, so an
/// identity of that provider is one the pool can link.
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
            "ProviderDetails": { "client_id": "1example23456789" },
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

/// The native identity of the local profile `username` names.
fn native_identity(username: &str) -> Value {
    identity(NATIVE_PROVIDER, SUBJECT_ATTRIBUTE, username)
}

/// A `DestinationUser` naming the local profile `username` identifies. "The
/// `ProviderAttributeName` of the `DestinationUser` is ignored"
/// (`smithy_query(query="doc",shape="AdminLinkProviderForUserRequest",member="DestinationUser")`),
/// so it is sent and never read.
fn destination(username: &str) -> Value {
    identity(NATIVE_PROVIDER, SUBJECT_ATTRIBUTE, username)
}

/// Link `source` to `destination` in `user_pool_id`.
async fn link(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    source: &Value,
    destination: &Value,
) -> (u16, Value) {
    call(
        router,
        "AdminLinkProviderForUser",
        &json!({
            "UserPoolId": user_pool_id,
            "SourceUser": source,
            "DestinationUser": destination,
        }),
    )
    .await
}

/// Deactivate `identity` in `user_pool_id`.
async fn disable(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    identity: &Value,
) -> (u16, Value) {
    call(
        router,
        "AdminDisableProviderForUser",
        &json!({ "UserPoolId": user_pool_id, "User": identity }),
    )
    .await
}

/// Assert `payload` is the refusal for an account that holds no link.
fn assert_not_linked(case: &str, status: u16, payload: &Value) {
    assert_eq!(
        (status, error_code(payload)),
        (400, "InvalidParameterException"),
        "case {case:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
    );
    assert_eq!(
        error_message(payload),
        NOT_LINKED_MESSAGE,
        "case {case:?}: expected the unlinked-account sentence, got {payload}"
    );
}

/// The entries of `username`'s `identities` attribute, as `AdminGetUser`
/// renders them: an empty list for a profile that reports no such attribute.
async fn identities(router: &MultiServiceRouter, user_pool_id: &str, username: &str) -> Vec<Value> {
    let payload = ok(
        router,
        "AdminGetUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    let attributes = payload["UserAttributes"]
        .as_array()
        .unwrap_or_else(|| panic!("AdminGetUser must report UserAttributes, got {payload}"))
        .clone();
    let Some(attribute) = attributes
        .iter()
        .find(|attribute| attribute["Name"] == "identities")
    else {
        return Vec::new();
    };
    let rendered = attribute["Value"]
        .as_str()
        .unwrap_or_else(|| panic!("the identities attribute carries a value, got {attribute}"));
    serde_json::from_str::<Vec<Value>>(rendered).unwrap_or_else(|error| {
        panic!("the identities attribute is a JSON array ({error}), got {rendered:?}")
    })
}

/// The `providerName`/`userId` pairs of `username`'s rendered identities, which
/// is how a linked identity is recognised whatever else it carries.
async fn linked_pairs(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    username: &str,
) -> Vec<String> {
    identities(router, user_pool_id, username)
        .await
        .iter()
        .map(|entry| {
            format!(
                "{}/{}",
                entry["providerName"].as_str().unwrap_or("<no providerName>"),
                entry["userId"].as_str().unwrap_or("<no userId>")
            )
        })
        .collect()
}

/// One row of the external-identity table: the IdP to configure and the
/// identity triple to link through it.
struct IdentityCase {
    case: &'static str,
    provider_name: String,
    /// One of the modelled `IdentityProviderTypeType` values
    /// (`smithy_query(query="shape",shape="IdentityProviderTypeType")`): SAML,
    /// Facebook, Google, LoginWithAmazon, SignInWithApple, OIDC.
    provider_type: &'static str,
    attribute_name: String,
    attribute_value: String,
}

impl IdentityCase {
    /// This row's identity triple.
    fn identity(&self) -> Value {
        identity(
            &self.provider_name,
            &self.attribute_name,
            &self.attribute_value,
        )
    }
}

/// The table. Rows differ along the provider TYPE (the whole modelled catalog),
/// the provider NAME's length (1 and the modelled maximum 32, from
/// `ProviderNameType` len 1..32 in
/// `smithy_query(query="shape",shape="ProviderUserIdentifierType")`), the
/// ATTRIBUTE the identity is named by (the subject, a mapped SAML claim, a
/// custom OIDC claim) and the FORM of its value (an opaque id, an email, a
/// non-ASCII value, a long one).
fn identity_cases() -> Vec<IdentityCase> {
    vec![
        IdentityCase {
            case: "a SAML identity named by a mapped email claim",
            provider_name: "CorpSAML".to_string(),
            provider_type: "SAML",
            attribute_name: "email".to_string(),
            attribute_value: "dana@example.com".to_string(),
        },
        IdentityCase {
            case: "a Facebook identity named by its subject",
            provider_name: "Facebook".to_string(),
            provider_type: "Facebook",
            attribute_name: SUBJECT_ATTRIBUTE.to_string(),
            attribute_value: "10224266394238161".to_string(),
        },
        IdentityCase {
            case: "a Google identity named by its subject",
            provider_name: "Google".to_string(),
            provider_type: "Google",
            attribute_name: SUBJECT_ATTRIBUTE.to_string(),
            attribute_value: "117550118874624030041".to_string(),
        },
        IdentityCase {
            case: "a Login with Amazon identity named by its subject",
            provider_name: "LoginWithAmazon".to_string(),
            provider_type: "LoginWithAmazon",
            attribute_name: SUBJECT_ATTRIBUTE.to_string(),
            attribute_value: "amzn1.account.AEXAMPLE".to_string(),
        },
        IdentityCase {
            case: "a Sign in with Apple identity named by a non-ASCII value",
            provider_name: "SignInWithApple".to_string(),
            provider_type: "SignInWithApple",
            attribute_name: SUBJECT_ATTRIBUTE.to_string(),
            attribute_value: "001234.ユーザー①.0700".to_string(),
        },
        IdentityCase {
            case: "an OIDC identity named by a custom claim, provider name at the modelled \
                   length maximum 32",
            provider_name: "p".repeat(32),
            provider_type: "OIDC",
            attribute_name: "custom:employee_number".to_string(),
            attribute_value: "e".repeat(200),
        },
        IdentityCase {
            case: "an OIDC identity whose provider name is the shortest a created IdP may carry",
            // `CreateIdentityProvider.ProviderName` carries a stricter pattern
            // than `ProviderUserIdentifierType.ProviderName` (len 1..32):
            // `^[^_\p{Z}][\p{L}\p{M}\p{S}\p{N}\p{P}][^_\p{Z}]+$` admits three
            // characters at the least
            // (`smithy_query(query="operation",operation="CreateIdentityProvider")`).
            provider_name: "Ap1".to_string(),
            provider_type: "OIDC",
            attribute_name: "sub".to_string(),
            attribute_value: "9".to_string(),
        },
    ]
}

#[tokio::test]
async fn a_linked_external_identity_is_de_linked_exactly_once() {
    let router = router();
    let user_pool_id = pool(&router, "link-pool").await;

    for (index, case) in identity_cases().into_iter().enumerate() {
        let label = case.case;
        // One destination profile per row, so each row starts from a profile
        // nothing is linked to yet.
        let username = format!("user-{index}");
        user(&router, &user_pool_id, &username).await;
        provider(
            &router,
            &user_pool_id,
            &case.provider_name,
            case.provider_type,
        )
        .await;

        // Nothing is linked yet, so the deactivation has no link to act on.
        let (status, payload) = disable(&router, &user_pool_id, &case.identity()).await;
        assert_not_linked(label, status, &payload);

        let (status, payload) = link(
            &router,
            &user_pool_id,
            &case.identity(),
            &destination(&username),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the link must succeed, got {status} with {payload}"
        );
        assert_eq!(
            payload,
            Value::Object(Map::new()),
            "case {label:?}: the link answers an empty body, got {payload}"
        );

        // The link is STORED, so a read of the profile renders it: the whole
        // identity, the stored IdP's type beside it.
        let rendered = identities(&router, &user_pool_id, &username).await;
        assert_eq!(
            rendered.len(),
            1,
            "case {label:?}: the profile renders the one link it holds, got {rendered:?}"
        );
        let entry = &rendered[0];
        assert_eq!(
            entry["providerName"].as_str(),
            Some(case.provider_name.as_str()),
            "case {label:?}: the rendered identity names its provider, got {entry}"
        );
        assert_eq!(
            entry["userId"].as_str(),
            Some(case.attribute_value.as_str()),
            "case {label:?}: the rendered identity carries the linked value, got {entry}"
        );
        assert_eq!(
            entry["providerType"].as_str(),
            Some(case.provider_type),
            "case {label:?}: the rendered identity carries the stored IdP's type, got {entry}"
        );
        assert_eq!(
            entry["primary"],
            json!(false),
            "case {label:?}: a linked external identity is not the primary one, got {entry}"
        );
        assert!(
            entry["dateCreated"].as_i64().is_some_and(|at| at > 0),
            "case {label:?}: the rendered identity carries when it was linked, got {entry}"
        );

        // A deactivation of ANOTHER identity of the same provider finds no
        // link: the stored link is keyed by the whole triple, not by provider.
        let other_value = format!("{}-other", case.attribute_value);
        let (status, payload) = disable(
            &router,
            &user_pool_id,
            &identity(&case.provider_name, &case.attribute_name, &other_value),
        )
        .await;
        assert_not_linked(label, status, &payload);
        // So is one that names the same value under another attribute.
        let (status, payload) = disable(
            &router,
            &user_pool_id,
            &identity(
                &case.provider_name,
                "an_unlinked_attribute",
                &case.attribute_value,
            ),
        )
        .await;
        assert_not_linked(label, status, &payload);

        // The linked identity itself is de-linked, ONCE: the second call finds
        // the link gone.
        let (status, payload) = disable(&router, &user_pool_id, &case.identity()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the deactivation must remove the stored link, got {status} with {payload}"
        );
        assert_eq!(
            payload,
            Value::Object(Map::new()),
            "case {label:?}: the deactivation answers an empty body, got {payload}"
        );
        let (status, payload) = disable(&router, &user_pool_id, &case.identity()).await;
        assert_not_linked(label, status, &payload);

        // The removal is stored too: the profile renders no identity at all
        // again.
        let rendered = identities(&router, &user_pool_id, &username).await;
        assert!(
            rendered.is_empty(),
            "case {label:?}: a de-linked profile renders no identity, got {rendered:?}"
        );
    }
}

#[tokio::test]
async fn a_de_link_removes_only_the_identity_it_names() {
    let router = router();
    let user_pool_id = pool(&router, "selective-pool").await;
    user(&router, &user_pool_id, "alice").await;
    for (provider_name, provider_type) in [("CorpSAML", "SAML"), ("Facebook", "Facebook")] {
        provider(&router, &user_pool_id, provider_name, provider_type).await;
    }

    let saml = identity("CorpSAML", "email", "alice@example.com");
    let facebook = identity("Facebook", SUBJECT_ATTRIBUTE, "10224266394238161");
    for source in [&saml, &facebook] {
        let (status, payload) = link(&router, &user_pool_id, source, &destination("alice")).await;
        assert_eq!(
            status, 200,
            "both identities must link to the one profile, got {status} with {payload}"
        );
    }

    // Removing one leaves the other: each link is stored under its own identity.
    let pairs = linked_pairs(&router, &user_pool_id, "alice").await;
    assert_eq!(
        pairs,
        vec![
            "CorpSAML/alice@example.com".to_string(),
            "Facebook/10224266394238161".to_string(),
        ],
        "case \"both links\": the profile renders both identities, oldest first, got {pairs:?}"
    );

    let (status, payload) = disable(&router, &user_pool_id, &saml).await;
    assert_eq!(
        status, 200,
        "case \"the SAML link\": the deactivation must succeed, got {status} with {payload}"
    );
    let pairs = linked_pairs(&router, &user_pool_id, "alice").await;
    assert_eq!(
        pairs,
        vec!["Facebook/10224266394238161".to_string()],
        "case \"after the SAML link was removed\": only the other identity is rendered, \
         got {pairs:?}"
    );
    let (status, payload) = disable(&router, &user_pool_id, &saml).await;
    assert_not_linked("the SAML link, a second time", status, &payload);
    let (status, payload) = disable(&router, &user_pool_id, &facebook).await;
    assert_eq!(
        status, 200,
        "case \"the surviving Facebook link\": the deactivation must succeed, got {status} with {payload}"
    );
}

#[tokio::test]
async fn a_link_lands_on_one_profile_of_one_pool() {
    let router = router();
    let first_pool = pool(&router, "first-pool").await;
    let second_pool = pool(&router, "second-pool").await;
    for user_pool_id in [&first_pool, &second_pool] {
        for username in ["alice", "bob"] {
            user(&router, user_pool_id, username).await;
        }
        provider(&router, user_pool_id, "CorpSAML", "SAML").await;
    }

    let source = identity("CorpSAML", "email", "alice@example.com");
    let (status, payload) = link(&router, &first_pool, &source, &destination("alice")).await;
    assert_eq!(
        status, 200,
        "the link must succeed, got {status} with {payload}"
    );

    // The same identity in the OTHER pool, and the native identity of the other
    // profile of the SAME pool, are linked to nothing.
    for (label, user_pool_id, named) in [
        ("the same identity in another pool", &second_pool, &source),
        (
            "another profile of the pool the link was made in",
            &first_pool,
            &native_identity("bob"),
        ),
        (
            "the linked profile's namesake in another pool",
            &second_pool,
            &native_identity("alice"),
        ),
    ] {
        let (status, payload) = disable(&router, user_pool_id, named).await;
        assert_not_linked(label, status, &payload);
    }

    // The profile the link was made for is linked, through its native identity
    // and through the external identity alike.
    let (status, payload) = disable(&router, &first_pool, &native_identity("alice")).await;
    assert_eq!(
        status, 200,
        "the linked profile's native identity is linked, got {status} with {payload}"
    );
    let (status, payload) = disable(&router, &first_pool, &source).await;
    assert_eq!(
        status, 200,
        "the external identity is still linked, got {status} with {payload}"
    );
}

#[tokio::test]
async fn a_destination_resolves_by_username_or_by_sub() {
    let router = router();
    let user_pool_id = pool(&router, "destination-pool").await;
    let sub = user(&router, &user_pool_id, "carol").await;
    provider(&router, &user_pool_id, "CorpSAML", "SAML").await;

    // "For a native username + password user, the `ProviderAttributeValue` for
    // the `DestinationUser` should be the username in the user pool" — and a
    // user resolves by its username or by its `sub`, so either names the same
    // stored profile.
    for (label, destination_value, source_value) in [
        ("a destination named by username", "carol".to_string(), "carol@example.com"),
        ("a destination named by sub", sub, "carol.second@example.com"),
    ] {
        let source = identity("CorpSAML", "email", source_value);
        let (status, payload) = link(
            &router,
            &user_pool_id,
            &source,
            &destination(&destination_value),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the link must succeed, got {status} with {payload}"
        );
        // The link landed on the one profile: its native identity is linked,
        // and the identity itself de-links.
        let (status, payload) = disable(&router, &user_pool_id, &native_identity("carol")).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the profile must hold the link, got {status} with {payload}"
        );
        let (status, payload) = disable(&router, &user_pool_id, &source).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the identity must de-link, got {status} with {payload}"
        );
    }
}

#[tokio::test]
async fn the_native_provider_deactivation_withdraws_password_sign_in() {
    let router = router();
    let user_pool_id = pool(&router, "native-pool").await;
    let client = ok(
        &router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": "native-pool-client",
            "ExplicitAuthFlows": ["ALLOW_ADMIN_USER_PASSWORD_AUTH"],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    user(&router, &user_pool_id, "erin").await;
    set_password(&router, &user_pool_id, "erin").await;
    provider(&router, &user_pool_id, "CorpSAML", "SAML").await;

    // Before any link there is nothing to deactivate, and the refusal stores
    // nothing: the password still signs in.
    let (status, payload) = disable(&router, &user_pool_id, &native_identity("erin")).await;
    assert_not_linked("the native identity of an unlinked profile", status, &payload);
    let (status, payload) = sign_in(&router, &user_pool_id, &client_id, "erin").await;
    assert_eq!(
        status, 200,
        "case \"after the refusal\": the password must still sign in, got {status} with {payload}"
    );

    let source = identity("CorpSAML", "email", "erin@example.com");
    let (status, payload) = link(&router, &user_pool_id, &source, &destination("erin")).await;
    assert_eq!(
        status, 200,
        "the link must succeed, got {status} with {payload}"
    );

    // Now the profile is linked, so the native provider can be deactivated:
    // "they can't use their password to sign in".
    let (status, payload) = disable(&router, &user_pool_id, &native_identity("erin")).await;
    assert_eq!(
        status, 200,
        "the native deactivation must succeed, got {status} with {payload}"
    );
    let (status, payload) = sign_in(&router, &user_pool_id, &client_id, "erin").await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "NotAuthorizedException"),
        "case \"a password sign-in after the native deactivation\": expected a 400 \
         NotAuthorizedException, got {status} with {payload}"
    );

    // The external link stands: only the native factor was deactivated.
    let (status, payload) = disable(&router, &user_pool_id, &source).await;
    assert_eq!(
        status, 200,
        "the external link must survive the native deactivation, got {status} with {payload}"
    );

    // An administrator setting a password again restores password sign-in.
    set_password(&router, &user_pool_id, "erin").await;
    let (status, payload) = sign_in(&router, &user_pool_id, &client_id, "erin").await;
    assert_eq!(
        status, 200,
        "case \"after a password is set again\": the password must sign in, got {status} with {payload}"
    );
}

/// Set `username`'s permanent password to [`GOOD_PASSWORD`].
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

/// Attempt a password sign-in of `username` and return its status and body.
async fn sign_in(
    router: &MultiServiceRouter,
    user_pool_id: &str,
    client_id: &str,
    username: &str,
) -> (u16, Value) {
    call(
        router,
        "AdminInitiateAuth",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientId": client_id,
            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await
}

#[tokio::test]
async fn a_deactivation_naming_nothing_stored_is_refused() {
    let router = router();
    let user_pool_id = pool(&router, "refusal-pool").await;
    user(&router, &user_pool_id, "dave").await;
    provider(&router, &user_pool_id, "CorpSAML", "SAML").await;
    let linked = identity("CorpSAML", "email", "dave@example.com");
    let (status, payload) = link(&router, &user_pool_id, &linked, &destination("dave")).await;
    assert_eq!(
        status, 200,
        "the link under test must be stored, got {status} with {payload}"
    );

    // Rows differ along WHICH part of the request names nothing: the pool, the
    // local profile, or a member of the identifier the model leaves optional
    // but an identity cannot do without.
    for (label, body, expected) in [
        (
            "an absent user pool",
            json!({ "UserPoolId": "us-east-1_absent", "User": native_identity("dave") }),
            (400, "ResourceNotFoundException"),
        ),
        (
            "a native identity naming a user the pool does not hold",
            json!({ "UserPoolId": user_pool_id, "User": native_identity("absent-user") }),
            (400, "UserNotFoundException"),
        ),
        (
            "an identifier naming no provider",
            json!({
                "UserPoolId": user_pool_id,
                "User": { "ProviderAttributeName": SUBJECT_ATTRIBUTE, "ProviderAttributeValue": "dave" },
            }),
            (400, "InvalidParameterException"),
        ),
        (
            "an identifier naming no attribute",
            json!({
                "UserPoolId": user_pool_id,
                "User": { "ProviderName": NATIVE_PROVIDER, "ProviderAttributeValue": "dave" },
            }),
            (400, "InvalidParameterException"),
        ),
        (
            "an identifier naming no attribute value",
            json!({
                "UserPoolId": user_pool_id,
                "User": { "ProviderName": NATIVE_PROVIDER, "ProviderAttributeName": SUBJECT_ATTRIBUTE },
            }),
            (400, "InvalidParameterException"),
        ),
    ] {
        let (status, payload) = call(&router, "AdminDisableProviderForUser", &body).await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
    }

    // Every refusal above stored nothing: the link made at the start is still
    // there to be removed exactly once.
    let (status, payload) = disable(&router, &user_pool_id, &linked).await;
    assert_eq!(
        status, 200,
        "a refused call stores nothing, so the stored link is still there, got {status} with {payload}"
    );
}

#[tokio::test]
async fn a_missing_identifier_member_is_named_by_the_refusal() {
    let router = router();
    let user_pool_id = pool(&router, "member-pool").await;
    user(&router, &user_pool_id, "dave").await;

    // One row per member of `ProviderUserIdentifierType`
    // (`smithy_query(query="shape",shape="ProviderUserIdentifierType")`): the
    // refusal names the member that is missing, whichever it is.
    for (label, identifier, member) in [
        (
            "no provider",
            json!({ "ProviderAttributeName": SUBJECT_ATTRIBUTE, "ProviderAttributeValue": "dave" }),
            "ProviderName",
        ),
        (
            "no attribute name",
            json!({ "ProviderName": NATIVE_PROVIDER, "ProviderAttributeValue": "dave" }),
            "ProviderAttributeName",
        ),
        (
            "no attribute value",
            json!({ "ProviderName": NATIVE_PROVIDER, "ProviderAttributeName": SUBJECT_ATTRIBUTE }),
            "ProviderAttributeValue",
        ),
        (
            "an empty provider attribute value",
            json!({
                "ProviderName": NATIVE_PROVIDER,
                "ProviderAttributeName": SUBJECT_ATTRIBUTE,
                "ProviderAttributeValue": "",
            }),
            "ProviderAttributeValue",
        ),
    ] {
        let (status, payload) = disable(&router, &user_pool_id, &identifier).await;
        assert_eq!(
            (status, error_code(&payload)),
            (400, "InvalidParameterException"),
            "case {label:?}: expected a 400 InvalidParameterException, got {status} with {payload}"
        );
        assert_eq!(
            error_message(&payload),
            format!("Missing required parameter {member}"),
            "case {label:?}: the refusal must name {member}, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_link_requires_a_configured_provider_and_a_stored_destination() {
    let router = router();
    let user_pool_id = pool(&router, "link-refusal-pool").await;
    user(&router, &user_pool_id, "frank").await;
    provider(&router, &user_pool_id, "CorpSAML", "SAML").await;

    let configured = identity("CorpSAML", "email", "frank@example.com");
    for (label, source, destination_name, expected, message) in [
        (
            "a source provider the pool holds no IdP for",
            identity("Unconfigured", "email", "frank@example.com"),
            "frank".to_string(),
            (400, "ResourceNotFoundException"),
            Some(format!(
                "Identity provider Unconfigured for tenantId {user_pool_id} does not exist."
            )),
        ),
        (
            "a source naming the native provider, which is no IdP of the pool",
            identity(NATIVE_PROVIDER, SUBJECT_ATTRIBUTE, "frank"),
            "frank".to_string(),
            (400, "ResourceNotFoundException"),
            Some(format!(
                "Identity provider Cognito for tenantId {user_pool_id} does not exist."
            )),
        ),
        (
            "a destination the pool holds no user for",
            configured.clone(),
            "absent-user".to_string(),
            (400, "UserNotFoundException"),
            Some("User does not exist.".to_string()),
        ),
        (
            "a destination naming no value",
            configured.clone(),
            String::new(),
            (400, "InvalidParameterException"),
            Some("Missing required parameter ProviderAttributeValue".to_string()),
        ),
    ] {
        let (status, payload) = link(
            &router,
            &user_pool_id,
            &source,
            &destination(&destination_name),
        )
        .await;
        assert_eq!(
            (status, error_code(&payload)),
            expected,
            "case {label:?}: expected {expected:?}, got {status} with {payload}"
        );
        if let Some(message) = message {
            assert_eq!(
                error_message(&payload),
                message,
                "case {label:?}: expected the refusal to read {message:?}, got {payload}"
            );
        }
    }

    // An absent pool is a 400 too, and nothing above was stored: the profile
    // still holds no link.
    let (status, payload) = link(
        &router,
        "us-east-1_absent",
        &configured,
        &destination("frank"),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "ResourceNotFoundException"),
        "case \"an absent user pool\": expected a 400 ResourceNotFoundException, got {status} with {payload}"
    );
    let (status, payload) = disable(&router, &user_pool_id, &native_identity("frank")).await;
    assert_not_linked("a profile no refused link reached", status, &payload);
}

#[tokio::test]
async fn an_identity_links_to_one_profile() {
    let router = router();
    let user_pool_id = pool(&router, "one-profile-pool").await;
    for username in ["gina", "hal"] {
        user(&router, &user_pool_id, username).await;
    }
    provider(&router, &user_pool_id, "CorpSAML", "SAML").await;
    let source = identity("CorpSAML", "email", "gina@example.com");

    let (status, payload) = link(&router, &user_pool_id, &source, &destination("gina")).await;
    assert_eq!(
        status, 200,
        "the first link must succeed, got {status} with {payload}"
    );

    // The same identity cannot be re-pointed at another profile.
    let (status, payload) = link(&router, &user_pool_id, &source, &destination("hal")).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case \"a second destination for one identity\": expected a 400 \
         InvalidParameterException, got {status} with {payload}"
    );
    let (status, payload) = disable(&router, &user_pool_id, &native_identity("hal")).await;
    assert_not_linked("the profile the refused link named", status, &payload);

    // Linking it again to the profile it ALREADY names is refused too: a source
    // identity carries one link, so the identity has to be deactivated before it
    // links again, and the one stored link is still removed exactly once.
    let (status, payload) = link(&router, &user_pool_id, &source, &destination("gina")).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case \"a repeated link to the same profile\": expected a 400 \
         InvalidParameterException, got {status} with {payload}"
    );
    assert_eq!(
        error_message(&payload),
        "SourceUser is already linked to DestinationUser",
        "case \"a repeated link to the same profile\": got {payload}"
    );
    let (status, payload) = disable(&router, &user_pool_id, &source).await;
    assert_eq!(
        status, 200,
        "the stored link must be removed, got {status} with {payload}"
    );
    let (status, payload) = disable(&router, &user_pool_id, &source).await;
    assert_not_linked("the identity, a second time", status, &payload);
}

#[tokio::test]
async fn a_profile_takes_five_linked_identities() {
    let router = router();
    let user_pool_id = pool(&router, "bound-pool").await;
    user(&router, &user_pool_id, "iris").await;

    // "The maximum number of federated identities linked to a user is five."
    // (`smithy_query(query="doc",shape="AdminLinkProviderForUser")`): five link,
    // the sixth is refused, and the bound is on what is STORED, so a link
    // freed by a deactivation can be taken again.
    let sources: Vec<Value> = (0..6)
        .map(|index| {
            let provider_name = format!("CorpSAML{index}");
            identity(&provider_name, "email", &format!("iris+{index}@example.com"))
        })
        .collect();
    for index in 0..6 {
        provider(
            &router,
            &user_pool_id,
            &format!("CorpSAML{index}"),
            "SAML",
        )
        .await;
    }

    for (index, source) in sources.iter().take(5).enumerate() {
        let (status, payload) = link(&router, &user_pool_id, source, &destination("iris")).await;
        assert_eq!(
            status, 200,
            "case \"link {index}\": expected success, got {status} with {payload}"
        );
    }

    let sixth = &sources[5];
    let (status, payload) = link(&router, &user_pool_id, sixth, &destination("iris")).await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "LimitExceededException"),
        "case \"the sixth link\": expected a 400 LimitExceededException, got {status} with {payload}"
    );
    let (status, payload) = disable(&router, &user_pool_id, sixth).await;
    assert_not_linked("the identity the refused link named", status, &payload);

    // Freeing one slot lets the sixth identity link.
    let (status, payload) = disable(&router, &user_pool_id, &sources[0]).await;
    assert_eq!(
        status, 200,
        "the first link must be removed, got {status} with {payload}"
    );
    let (status, payload) = link(&router, &user_pool_id, sixth, &destination("iris")).await;
    assert_eq!(
        status, 200,
        "case \"the sixth link after one was freed\": expected success, got {status} with {payload}"
    );
}

/// One row of the destination-identifier table: the `DestinationUser` members
/// to send — every row naming the SAME stored profile — and the source identity
/// linked through them.
struct DestinationCase {
    case: &'static str,
    /// The destination's `ProviderAttributeName`, which the request must be
    /// read without. Its modelled range is length 0..131072
    /// (`smithy_query(query="shape",shape="ProviderUserIdentifierType")`), and
    /// the rows walk both of its bounds.
    attribute_name: String,
    /// Whether the destination names the profile by its generated `sub` rather
    /// than by its username, the two values a stored profile resolves by.
    by_sub: bool,
    /// This row's external identity value, so every row links an identity of
    /// its own and starts from a profile that holds no link.
    source_value: &'static str,
}

/// The table. Rows differ along the destination's attribute NAME (the subject,
/// the model's own example claim, a custom claim, and each bound of the
/// modelled length range) and along the FORM of the value that names the
/// profile (its username, its `sub`).
fn destination_cases() -> Vec<DestinationCase> {
    vec![
        DestinationCase {
            case: "the subject attribute, the profile named by its username",
            attribute_name: SUBJECT_ATTRIBUTE.to_string(),
            by_sub: false,
            source_value: "jade+subject@example.com",
        },
        DestinationCase {
            // "The name of the provider attribute to link to, such as
            // `NameID`"
            // (`smithy_query(query="shape",shape="ProviderUserIdentifierType")`).
            case: "the claim the model names as an example",
            attribute_name: "NameID".to_string(),
            by_sub: false,
            source_value: "jade+nameid@example.com",
        },
        DestinationCase {
            case: "a custom claim, the profile named by its sub",
            attribute_name: "custom:employee_number".to_string(),
            by_sub: true,
            source_value: "jade+custom@example.com",
        },
        DestinationCase {
            case: "an empty attribute name, the modelled length minimum 0",
            attribute_name: String::new(),
            by_sub: false,
            source_value: "jade+empty@example.com",
        },
        DestinationCase {
            case: "an attribute name at the modelled length maximum 131072",
            attribute_name: "n".repeat(131_072),
            by_sub: true,
            source_value: "jade+max@example.com",
        },
    ]
}

#[tokio::test]
async fn a_destination_is_read_as_the_value_that_names_the_profile() {
    let router = router();
    let user_pool_id = pool(&router, "destination-member-pool").await;
    let sub = user(&router, &user_pool_id, "jade").await;
    provider(&router, &user_pool_id, "CorpSAML", "SAML").await;

    // "The `ProviderAttributeName` of the `DestinationUser` is ignored"
    // (`smithy_query(query="doc",shape="AdminLinkProviderForUserRequest",member="DestinationUser")`),
    // so whatever attribute name a destination carries, the profile it resolves
    // to is the one its VALUE names, and the identity STORED on that profile is
    // the source triple alone.
    for case in destination_cases() {
        let label = case.case;
        let destination_value = if case.by_sub {
            sub.clone()
        } else {
            "jade".to_string()
        };
        let destination_user = json!({
            "ProviderName": NATIVE_PROVIDER,
            "ProviderAttributeName": case.attribute_name,
            "ProviderAttributeValue": destination_value,
        });
        let source = identity("CorpSAML", "email", case.source_value);

        let (status, payload) = link(&router, &user_pool_id, &source, &destination_user).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the link must succeed, got {status} with {payload}"
        );

        // The one profile holds the one link, keyed by the SOURCE identity: the
        // destination's own members are no part of what was stored.
        let pairs = linked_pairs(&router, &user_pool_id, "jade").await;
        assert_eq!(
            pairs,
            vec![format!("CorpSAML/{}", case.source_value)],
            "case {label:?}: the profile must render the source identity alone, got {pairs:?}"
        );

        // Presenting the source triple again removes exactly that link, so the
        // next row starts from a profile holding none.
        let (status, payload) = disable(&router, &user_pool_id, &source).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the stored link must be removed, got {status} with {payload}"
        );
        let pairs = linked_pairs(&router, &user_pool_id, "jade").await;
        assert!(
            pairs.is_empty(),
            "case {label:?}: the de-linked profile renders no identity, got {pairs:?}"
        );
    }

    // A destination that names no provider identifies nothing, and the refusal
    // names the member that is missing — the same rule the source identifier is
    // read under.
    let source = identity("CorpSAML", "email", "jade+noprovider@example.com");
    let (status, payload) = link(
        &router,
        &user_pool_id,
        &source,
        &json!({ "ProviderAttributeName": SUBJECT_ATTRIBUTE, "ProviderAttributeValue": "jade" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case \"a destination naming no provider\": expected a 400 InvalidParameterException, \
         got {status} with {payload}"
    );
    assert_eq!(
        error_message(&payload),
        "Missing required parameter ProviderName",
        "case \"a destination naming no provider\": the refusal must name ProviderName, \
         got {payload}"
    );

    // One character past the modelled length maximum is outside the constraint,
    // so the request is a 400 rather than a link.
    let (status, payload) = link(
        &router,
        &user_pool_id,
        &source,
        &json!({
            "ProviderName": NATIVE_PROVIDER,
            "ProviderAttributeName": "n".repeat(131_073),
            "ProviderAttributeValue": "jade",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&payload)),
        (400, "InvalidParameterException"),
        "case \"an attribute name one past the modelled maximum\": expected a 400 \
         InvalidParameterException, got {status} with {payload}"
    );

    // Neither refusal stored anything: the profile still holds no link.
    let pairs = linked_pairs(&router, &user_pool_id, "jade").await;
    assert!(
        pairs.is_empty(),
        "case \"a profile no refused link reached\": expected no identity, got {pairs:?}"
    );
}
