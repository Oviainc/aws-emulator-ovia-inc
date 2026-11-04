// SPDX-License-Identifier: CC0-1.0

//! The configuration Amazon Cognito resolves for an identity provider from its
//! `ProviderType`, driven through the router exactly as an SDK drives it
//! (`POST /` with `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test: a stored IdP carries, beside the `ProviderDetails` and
//! `AttributeMapping` entries the request states, every entry the service
//! resolves for that `ProviderType` itself — the endpoints of a named social IdP,
//! the `attributes_url_add_attributes` flag, and the `username` mapping of the
//! IdP's own subject claim — and a stated entry is never overwritten. The model
//! documents the resolved entries per type as the difference between its
//! `ProviderDetails` "create or update request" example and the "describe
//! response" example beside it
//! (`smithy_query(query="doc",shape="CreateIdentityProviderRequest",member="ProviderDetails")`).
//! Every read of the provider renders the stored configuration, and an update
//! that replaces the submitted members does not drop it. Each table row names the
//! case it asserts.

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// A request's SigV4 credential scope is what routes it to this service. The
/// signature is never checked here: the suite turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/cognito-idp/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// One emulator with just this service mounted.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(
        service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine),
    );
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

/// Create a pool and return its minted `UserPoolId`.
async fn pool(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateUserPool", &json!({ "PoolName": name })).await;
    created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string()
}

/// A JSON object from key/value pairs.
fn object(entries: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert((*key).to_string(), json!(value));
    }
    Value::Object(map)
}

/// The two maps of `object` merged, with `resolved` losing to `stated`: what a
/// stored provider renders once the service has completed what the request
/// stated.
fn merged(stated: &[(&str, &str)], resolved: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (key, value) in resolved.iter().chain(stated.iter()) {
        map.insert((*key).to_string(), json!(value));
    }
    Value::Object(map)
}

/// One row of the table: a provider to create, and the WHOLE `ProviderDetails`
/// and `AttributeMapping` the stored provider must then render, written out
/// rather than recomputed.
struct Case {
    case: &'static str,
    provider_name: &'static str,
    provider_type: &'static str,
    /// The entries the request states, or `None` to leave the member out (which
    /// `ProviderDetails` cannot be: the model makes it required).
    details: &'static [(&'static str, &'static str)],
    attribute_mapping: Option<&'static [(&'static str, &'static str)]>,
    /// The entries the service must resolve on top of `details`.
    resolved_details: &'static [(&'static str, &'static str)],
    /// The whole `AttributeMapping` the stored provider must render.
    expected_mapping: &'static [(&'static str, &'static str)],
}

/// The table. `ProviderType` spans the whole modelled catalog —
/// `smithy_query(query="shape",shape="IdentityProviderTypeType")`: Facebook,
/// Google, LoginWithAmazon, OIDC, SAML, SignInWithApple — and the rows differ
/// along what the request states: nothing but the client credentials, an
/// `api_version` the Facebook locators are built from, an endpoint the request
/// states for itself, a `username` the request maps for itself, and no attribute
/// mapping at all.
const CASES: &[Case] = &[
    Case {
        case: "a Facebook IdP stating no api_version, whose locators the service resolves",
        provider_name: "Facebook",
        provider_type: "Facebook",
        details: &[
            ("authorize_scopes", "public_profile, email"),
            ("client_id", "1example23456789"),
            ("client_secret", "provider-app-client-secret"),
        ],
        attribute_mapping: None,
        resolved_details: &[
            ("attributes_url", "https://graph.facebook.com/v21.0/me?fields="),
            ("attributes_url_add_attributes", "true"),
            ("authorize_url", "https://www.facebook.com/v21.0/dialog/oauth"),
            ("token_request_method", "GET"),
            (
                "token_url",
                "https://graph.facebook.com/v21.0/oauth/access_token",
            ),
        ],
        expected_mapping: &[("username", "id")],
    },
    Case {
        case: "a Facebook IdP stating an api_version of its own, which every locator carries",
        provider_name: "FacebookPinned",
        provider_type: "Facebook",
        details: &[
            ("api_version", "v12.0"),
            ("authorize_scopes", "public_profile"),
            ("client_id", "2example23456789"),
            ("client_secret", "provider-app-client-secret"),
        ],
        attribute_mapping: Some(&[("email", "email")]),
        resolved_details: &[
            ("attributes_url", "https://graph.facebook.com/v12.0/me?fields="),
            ("attributes_url_add_attributes", "true"),
            ("authorize_url", "https://www.facebook.com/v12.0/dialog/oauth"),
            ("token_request_method", "GET"),
            (
                "token_url",
                "https://graph.facebook.com/v12.0/oauth/access_token",
            ),
        ],
        expected_mapping: &[("email", "email"), ("username", "id")],
    },
    Case {
        case: "a Facebook IdP stating locators and a username mapping of its own, which win",
        provider_name: "FacebookProxied",
        provider_type: "Facebook",
        details: &[
            ("api_version", "v21.0"),
            ("attributes_url", "https://proxy.example.com/me?fields="),
            ("attributes_url_add_attributes", "false"),
            ("authorize_scopes", "email"),
            ("authorize_url", "https://proxy.example.com/dialog/oauth"),
            ("client_id", "3example23456789"),
            ("client_secret", "provider-app-client-secret"),
            ("token_request_method", "POST"),
            ("token_url", "https://proxy.example.com/access_token"),
        ],
        attribute_mapping: Some(&[("username", "email")]),
        resolved_details: &[],
        expected_mapping: &[("username", "email")],
    },
    Case {
        case: "a Google IdP, whose locators and OIDC issuer the service resolves",
        provider_name: "Google",
        provider_type: "Google",
        details: &[
            ("authorize_scopes", "email profile openid"),
            ("client_id", "1example23456789.apps.googleusercontent.com"),
            ("client_secret", "provider-app-client-secret"),
        ],
        attribute_mapping: Some(&[("email", "email")]),
        resolved_details: &[
            (
                "attributes_url",
                "https://people.googleapis.com/v1/people/me?personFields=",
            ),
            ("attributes_url_add_attributes", "true"),
            ("authorize_url", "https://accounts.google.com/o/oauth2/v2/auth"),
            ("oidc_issuer", "https://accounts.google.com"),
            ("token_request_method", "POST"),
            ("token_url", "https://www.googleapis.com/oauth2/v4/token"),
        ],
        expected_mapping: &[("email", "email"), ("username", "sub")],
    },
    Case {
        case: "a LoginWithAmazon IdP, whose profile endpoint the service resolves",
        provider_name: "LoginWithAmazon",
        provider_type: "LoginWithAmazon",
        details: &[
            ("authorize_scopes", "profile postal_code"),
            (
                "client_id",
                "amzn1.application-oa2-client.1example23456789",
            ),
            ("client_secret", "provider-app-client-secret"),
        ],
        attribute_mapping: None,
        resolved_details: &[
            ("attributes_url", "https://api.amazon.com/user/profile"),
            ("attributes_url_add_attributes", "false"),
            ("authorize_url", "https://www.amazon.com/ap/oa"),
            ("token_request_method", "POST"),
            ("token_url", "https://api.amazon.com/auth/o2/token"),
        ],
        expected_mapping: &[("username", "user_id")],
    },
    Case {
        case: "a SignInWithApple IdP, whose authorize and token endpoints the service resolves",
        provider_name: "SignInWithApple",
        provider_type: "SignInWithApple",
        details: &[
            ("authorize_scopes", "email name"),
            ("client_id", "com.example.cognito"),
            ("key_id", "1EXAMPLE"),
            ("private_key", "2EXAMPLE"),
            ("team_id", "3EXAMPLE"),
        ],
        attribute_mapping: Some(&[("email", "email")]),
        resolved_details: &[
            ("attributes_url_add_attributes", "false"),
            ("authorize_url", "https://appleid.apple.com/auth/authorize"),
            ("oidc_issuer", "https://appleid.apple.com"),
            ("token_request_method", "POST"),
            ("token_url", "https://appleid.apple.com/auth/token"),
        ],
        expected_mapping: &[("email", "email"), ("username", "sub")],
    },
    Case {
        case: "an OIDC IdP stating all four endpoints, which only the add-attributes flag completes",
        provider_name: "AuthExampleOIDC",
        provider_type: "OIDC",
        details: &[
            ("attributes_request_method", "GET"),
            ("attributes_url", "https://auth.example.com/userInfo"),
            ("authorize_scopes", "openid profile email"),
            ("authorize_url", "https://auth.example.com/authorize"),
            ("client_id", "1example23456789"),
            ("client_secret", "provider-app-client-secret"),
            (
                "jwks_uri",
                "https://auth.example.com/.well-known/jwks.json",
            ),
            ("oidc_issuer", "https://auth.example.com"),
            ("token_url", "https://auth.example.com/token"),
        ],
        attribute_mapping: Some(&[("email", "email")]),
        resolved_details: &[("attributes_url_add_attributes", "false")],
        expected_mapping: &[("email", "email"), ("username", "sub")],
    },
    Case {
        case: "an OIDC IdP that maps username itself, which the service does not overwrite",
        provider_name: "MappedOIDC",
        provider_type: "OIDC",
        details: &[
            ("attributes_request_method", "GET"),
            ("attributes_url", "https://mapped.example.com/userInfo"),
            ("attributes_url_add_attributes", "true"),
            ("authorize_scopes", "openid"),
            ("authorize_url", "https://mapped.example.com/authorize"),
            ("client_id", "4example23456789"),
            (
                "jwks_uri",
                "https://mapped.example.com/.well-known/jwks.json",
            ),
            ("oidc_issuer", "https://mapped.example.com"),
            ("token_url", "https://mapped.example.com/token"),
        ],
        attribute_mapping: Some(&[("username", "preferred_username")]),
        resolved_details: &[],
        expected_mapping: &[("username", "preferred_username")],
    },
    Case {
        case: "a SAML IdP, whose own metadata document is the only source of its endpoints",
        provider_name: "CorpSAML",
        provider_type: "SAML",
        details: &[
            ("IDPSignout", "true"),
            ("MetadataURL", "https://auth.example.com/sso/saml/metadata"),
            ("RequestSigningAlgorithm", "rsa-sha256"),
        ],
        attribute_mapping: Some(&[("email", "emailAddress")]),
        // Nothing: a SAML IdP's binding URIs and encryption certificate are read
        // from the metadata document the IdP serves, not resolved from its type.
        resolved_details: &[],
        expected_mapping: &[("email", "emailAddress")],
    },
];

impl Case {
    /// This row's `CreateIdentityProvider` request for `user_pool_id`.
    fn request(&self, user_pool_id: &str) -> Value {
        let mut body = Map::new();
        body.insert("UserPoolId".to_string(), json!(user_pool_id));
        body.insert("ProviderName".to_string(), json!(self.provider_name));
        body.insert("ProviderType".to_string(), json!(self.provider_type));
        body.insert("ProviderDetails".to_string(), object(self.details));
        if let Some(mapping) = self.attribute_mapping {
            body.insert("AttributeMapping".to_string(), object(mapping));
        }
        Value::Object(body)
    }

    /// The whole `ProviderDetails` the stored provider must render.
    fn expected_details(&self) -> Value {
        merged(self.details, self.resolved_details)
    }

    /// The `AttributeMapping` a provider of this row's type renders when the
    /// request maps nothing: the mapping of the IdP's own subject claim, whatever
    /// the create stated.
    fn default_mapping(&self) -> &'static [(&'static str, &'static str)] {
        subject_claim_mapping(self.provider_type)
    }
}

#[tokio::test]
async fn a_created_provider_carries_the_configuration_its_type_resolves() {
    let router = router();
    let user_pool_id = pool(&router, "idp-type-defaults-pool").await;

    for row in CASES {
        let case = row.case;
        let created = ok(&router, "CreateIdentityProvider", &row.request(&user_pool_id)).await;
        let provider = &created["IdentityProvider"];

        assert_eq!(
            provider["ProviderDetails"],
            row.expected_details(),
            "case {case:?}: expected the stated details completed by the ones the type resolves, got {provider}"
        );
        assert_eq!(
            provider["AttributeMapping"],
            object(row.expected_mapping),
            "case {case:?}: expected the stated mapping completed by the IdP's subject claim, got {provider}"
        );

        // The round trip: both readers render the STORED provider, and reading it
        // changes nothing a later caller sees.
        let read = ok(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": row.provider_name }),
        )
        .await;
        assert_eq!(
            read["IdentityProvider"], *provider,
            "case {case:?}: DescribeIdentityProvider must render what the create stored, got {read}"
        );
        let again = ok(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": row.provider_name }),
        )
        .await;
        assert_eq!(
            again["IdentityProvider"], read["IdentityProvider"],
            "case {case:?}: a read must not change what the next read sees, got {again}"
        );
    }
}

#[tokio::test]
async fn an_update_that_replaces_the_detail_map_keeps_the_configuration_its_type_resolves() {
    let router = router();
    let user_pool_id = pool(&router, "idp-type-defaults-update-pool").await;

    for row in CASES {
        let case = row.case;
        ok(&router, "CreateIdentityProvider", &row.request(&user_pool_id)).await;

        // The update submits the detail map the create submitted and an EMPTY
        // attribute mapping: both writable members are replaced wholesale, so
        // the configuration the type resolves has to be there again.
        let updated = ok(
            &router,
            "UpdateIdentityProvider",
            &json!({
                "UserPoolId": user_pool_id,
                "ProviderName": row.provider_name,
                "ProviderDetails": object(row.details),
                "AttributeMapping": {},
            }),
        )
        .await;
        let provider = &updated["IdentityProvider"];

        assert_eq!(
            provider["ProviderDetails"],
            row.expected_details(),
            "case {case:?}: an update must not leave the provider without the details its type resolves, got {provider}"
        );
        assert_eq!(
            provider["AttributeMapping"],
            object(row.default_mapping()),
            "case {case:?}: an emptied mapping is completed by the IdP's subject claim again, got {provider}"
        );

        let read = ok(
            &router,
            "DescribeIdentityProvider",
            &json!({ "UserPoolId": user_pool_id, "ProviderName": row.provider_name }),
        )
        .await;
        assert_eq!(
            read["IdentityProvider"], *provider,
            "case {case:?}: DescribeIdentityProvider must render what the update stored, got {read}"
        );
    }
}

#[tokio::test]
async fn an_identifier_lookup_renders_the_same_resolved_configuration() {
    let router = router();
    let user_pool_id = pool(&router, "idp-type-defaults-identifier-pool").await;

    for row in CASES {
        let case = row.case;
        let mut request = row.request(&user_pool_id);
        let identifier = format!("{}.example.com", row.provider_name);
        request
            .as_object_mut()
            .expect("the request is an object")
            .insert("IdpIdentifiers".to_string(), json!([identifier]));
        let created = ok(&router, "CreateIdentityProvider", &request).await;

        let found = ok(
            &router,
            "GetIdentityProviderByIdentifier",
            &json!({ "UserPoolId": user_pool_id, "IdpIdentifier": identifier }),
        )
        .await;
        assert_eq!(
            found["IdentityProvider"], created["IdentityProvider"],
            "case {case:?}: the by-identifier read must render the stored provider, got {found}"
        );
        assert_eq!(
            found["IdentityProvider"]["ProviderDetails"],
            row.expected_details(),
            "case {case:?}: the resolved provider carries the details its type resolves, got {found}"
        );
    }
}

/// The whole `AttributeMapping` a provider of `provider_type` renders when the
/// request maps nothing: the IdP's own subject claim, and nothing at all for a
/// SAML IdP, whose subject arrives in whatever claim its metadata names.
fn subject_claim_mapping(provider_type: &str) -> &'static [(&'static str, &'static str)] {
    match provider_type {
        "Facebook" => &[("username", "id")],
        "LoginWithAmazon" => &[("username", "user_id")],
        "Google" | "SignInWithApple" | "OIDC" => &[("username", "sub")],
        _ => &[],
    }
}

#[tokio::test]
async fn the_shared_expectation_helper_agrees_with_the_tables_written_out_here() {
    // The helper this module exports for the other identity-provider suites has
    // to answer exactly what the rows above write out literally, so a suite that
    // computes its expectation through it is still pinned to a literal table.
    for row in CASES {
        let case = row.case;
        assert_eq!(
            resolved_details(row.provider_type, &object(row.details)),
            row.expected_details(),
            "case {case:?}: the shared helper must answer the details this row writes out"
        );
        let stated = match row.attribute_mapping {
            Some(mapping) => object(mapping),
            None => json!({}),
        };
        assert_eq!(
            resolved_attribute_mapping(row.provider_type, &stated),
            object(row.expected_mapping),
            "case {case:?}: the shared helper must answer the mapping this row writes out"
        );
    }
}

/// What a stored provider of `provider_type` renders for `ProviderDetails` when
/// the create or update request stated `stated`: the stated entries, completed by
/// the ones the service resolves for the type. Shared with the other suites that
/// read a provider back, so the resolved configuration is stated in ONE place.
pub fn resolved_details(provider_type: &str, stated: &Value) -> Value {
    let mut map = stated_entries(stated);
    for (key, value) in type_defaults(provider_type, &map) {
        map.entry(key.to_string()).or_insert(json!(value));
    }
    Value::Object(map)
}

/// Likewise `AttributeMapping`: the stated entries, completed by the mapping of
/// the IdP's own subject claim.
pub fn resolved_attribute_mapping(provider_type: &str, stated: &Value) -> Value {
    let mut map = stated_entries(stated);
    for (key, value) in subject_claim_mapping(provider_type) {
        map.entry((*key).to_string()).or_insert(json!(value));
    }
    Value::Object(map)
}

/// The entries of a submitted map, or none when the request stated no map.
fn stated_entries(stated: &Value) -> Map<String, Value> {
    stated.as_object().cloned().unwrap_or_default()
}

/// The `ProviderDetails` entries the service resolves for `provider_type`, given
/// the entries the request `stated` — the same per-type tables the rows of `CASES`
/// write out one by one.
fn type_defaults(provider_type: &str, stated: &Map<String, Value>) -> Vec<(&'static str, String)> {
    let version = stated
        .get("api_version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .unwrap_or("v21.0")
        .to_string();
    match provider_type {
        "Facebook" => vec![
            (
                "attributes_url",
                format!("https://graph.facebook.com/{version}/me?fields="),
            ),
            ("attributes_url_add_attributes", "true".to_string()),
            (
                "authorize_url",
                format!("https://www.facebook.com/{version}/dialog/oauth"),
            ),
            ("token_request_method", "GET".to_string()),
            (
                "token_url",
                format!("https://graph.facebook.com/{version}/oauth/access_token"),
            ),
        ],
        "Google" => vec![
            (
                "attributes_url",
                "https://people.googleapis.com/v1/people/me?personFields=".to_string(),
            ),
            ("attributes_url_add_attributes", "true".to_string()),
            (
                "authorize_url",
                "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
            ),
            ("oidc_issuer", "https://accounts.google.com".to_string()),
            ("token_request_method", "POST".to_string()),
            (
                "token_url",
                "https://www.googleapis.com/oauth2/v4/token".to_string(),
            ),
        ],
        "LoginWithAmazon" => vec![
            (
                "attributes_url",
                "https://api.amazon.com/user/profile".to_string(),
            ),
            ("attributes_url_add_attributes", "false".to_string()),
            ("authorize_url", "https://www.amazon.com/ap/oa".to_string()),
            ("token_request_method", "POST".to_string()),
            (
                "token_url",
                "https://api.amazon.com/auth/o2/token".to_string(),
            ),
        ],
        "SignInWithApple" => vec![
            ("attributes_url_add_attributes", "false".to_string()),
            (
                "authorize_url",
                "https://appleid.apple.com/auth/authorize".to_string(),
            ),
            ("oidc_issuer", "https://appleid.apple.com".to_string()),
            ("token_request_method", "POST".to_string()),
            (
                "token_url",
                "https://appleid.apple.com/auth/token".to_string(),
            ),
        ],
        "OIDC" => vec![("attributes_url_add_attributes", "false".to_string())],
        _ => Vec::new(),
    }
}
