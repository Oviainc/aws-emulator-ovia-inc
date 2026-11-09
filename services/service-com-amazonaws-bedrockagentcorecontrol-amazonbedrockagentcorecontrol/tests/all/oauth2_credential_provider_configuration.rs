// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for an OAuth2 credential provider's CONFIGURATION:
//! `CreateOauth2CredentialProvider`, `GetOauth2CredentialProvider` and
//! `ListOauth2CredentialProviders`.
//!
//! Two rules are under test, and they are different refusal classes of one operation:
//!
//! * `credentialProviderVendor` SELECTS which member of `Oauth2ProviderConfigInput`
//!   carries the credentials, so a request whose supplied member is not the one the vendor
//!   selects is refused HTTP 400 `ValidationException`, never reinterpreted as the vendor
//!   the payload names, and no provider is created;
//! * a member outside its modelled `@pattern` is refused HTTP 400 `ValidationException`
//!   in this service's own constraint-violation wording (`src/service/validation.rs`),
//!   naming the member path and the pattern — the latter quoted unanchored, which is that
//!   module's rule 3, and with no value of the caller's, which is its rule 5 for the
//!   `/identities` family.
//!
//! Secrets Manager is mounted beside this service on ONE engine, the shape
//! `core/router/src/lib.rs` mounts them in, because a provider's client secret is stored
//! in a real secret there.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every request in this suite signs for.
const REGION: &str = "eu-west-1";

/// The modelled `@pattern` of `Oauth2Discovery$discoveryUrl`, as the MODEL spells it
/// (`smithy_query(query="shape", shape="Oauth2Discovery")`) — anchors included, because
/// that is what the check runs against.
const MODELLED_DISCOVERY_URL_PATTERN: &str =
    r"^.+/\.well-known/(openid-configuration|oauth-authorization-server)$";

/// The same expression as this service's refusal QUOTES it: without the `^` / `$` pair the
/// model spells it with, which is rule 3 of this crate's constraint-message wording
/// (`src/service/validation.rs`) — the advertised constraint is the character grammar a
/// value must satisfy, not the model's way of making the match total. Written out rather
/// than computed, so the expectation pins the words a caller reads.
const QUOTED_DISCOVERY_URL_PATTERN: &str =
    r".+/\.well-known/(openid-configuration|oauth-authorization-server)";

/// The two documents that pattern admits, which are the published locations of an
/// authorization server's metadata (OpenID Connect Discovery 1.0 and RFC 8414).
const OPENID_CONFIGURATION: &str = "/.well-known/openid-configuration";
const OAUTH_AUTHORIZATION_SERVER: &str = "/.well-known/oauth-authorization-server";

struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router }
    }

    /// POST one `restJson1` operation of this service, signed for [`REGION`].
    async fn post(&self, uri: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("host", "bedrock-agentcore-control.amazonaws.com")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/aws4_request, SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body is serializable"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn create(&self, body: &Value) -> (u16, Value) {
        self.post("/identities/CreateOauth2CredentialProvider", body)
            .await
    }

    async fn get(&self, name: &str) -> (u16, Value) {
        self.post(
            "/identities/GetOauth2CredentialProvider",
            &json!({"name": name}),
        )
        .await
    }

    async fn list(&self) -> (u16, Value) {
        self.post("/identities/ListOauth2CredentialProviders", &json!({}))
            .await
    }
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A configuration carried in `member`, with the three members every built-in vendor's
/// configuration declares.
fn vendor_config(member: &str) -> Value {
    json!({member: {"clientId": "a-client", "clientSecret": "a-secret"}})
}

/// A custom provider's configuration, whose authorization server the caller names.
fn custom_config(discovery_url: &str) -> Value {
    json!({"customOauth2ProviderConfig": {
        "oauthDiscovery": {"discoveryUrl": discovery_url},
        "clientId": "a-client",
        "clientSecret": "a-secret",
    }})
}

/// The names a listing reports, in the order it reported them.
fn listed_names(body: &Value) -> Vec<String> {
    body["credentialProviders"]
        .as_array()
        .map(|providers| {
            providers
                .iter()
                .map(|provider| provider["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A declared vendor and a supplied configuration member that disagree are refused, and
/// the provider never appears in the account.
///
/// The rows differ along which vendor was declared and which member was supplied — a
/// vendor with a configuration member of its own given another vendor's member, a vendor
/// with one given the custom member, the custom vendor given a built-in member, and a
/// vendor the union declares no member of its own for given a named vendor's member — so
/// the rule is about the pair disagreeing, not about any one vendor.
#[tokio::test]
async fn a_vendor_and_a_configuration_that_disagree_are_refused() {
    let emulator = Emulator::new();

    // (case, provider name, declared vendor, the configuration supplied)
    let rows: [(&str, &str, &str, Value); 5] = [
        (
            "GoogleOauth2 given the custom member",
            "google-given-custom",
            "GoogleOauth2",
            custom_config(&format!("https://accounts.example.com{OPENID_CONFIGURATION}")),
        ),
        (
            "GoogleOauth2 given another vendor's member",
            "google-given-github",
            "GoogleOauth2",
            vendor_config("githubOauth2ProviderConfig"),
        ),
        (
            "CustomOauth2 given a built-in vendor's member",
            "custom-given-google",
            "CustomOauth2",
            vendor_config("googleOauth2ProviderConfig"),
        ),
        (
            "OktaOauth2, which the union declares no member of its own for, given one",
            "okta-given-google",
            "OktaOauth2",
            vendor_config("googleOauth2ProviderConfig"),
        ),
        (
            "SlackOauth2 given the included member",
            "slack-given-included",
            "SlackOauth2",
            json!({"includedOauth2ProviderConfig": {
                "clientId": "a-client",
                "issuer": "https://slack.example.com",
            }}),
        ),
    ];

    for (case, provider, vendor, configuration) in rows {
        let (status, refused) = emulator
            .create(&json!({
                "name": provider,
                "credentialProviderVendor": vendor,
                "oauth2ProviderConfigInput": configuration,
            }))
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} ({refused})"
        );
        assert_eq!(
            refused["message"].as_str().unwrap_or_default(),
            format!("Provided configuration does not match selected type: {vendor}"),
            "case {case:?}: unexpected refusal message, got {refused}"
        );

        let (status, fetched) = emulator.get(provider).await;
        assert_eq!(
            (status, error_code(&fetched).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the refused create left no provider behind, got {status} ({fetched})"
        );
        assert!(
            !listed_names(&emulator.list().await.1).contains(&provider.to_string()),
            "case {case:?}: the refused provider appears in the listing"
        );
    }
}

/// A `discoveryUrl` outside its modelled pattern is refused in this service's
/// constraint-violation wording, which names the member path and the pattern; one that
/// satisfies the pattern is served and reported back.
///
/// The refused rows drive each way the pattern can fail — no well-known path at all, the
/// path without the metadata document, a document the pattern does not admit, the
/// metadata document with nothing before it, and a trailing path segment the pattern's
/// anchored ending does not allow. The served rows drive each document the pattern names.
#[tokio::test]
async fn a_discovery_url_outside_its_pattern_is_refused_and_one_inside_it_is_served() {
    let emulator = Emulator::new();
    let member_path =
        "oauth2ProviderConfigInput.customOauth2ProviderConfig.oauthDiscovery.discoveryUrl";
    // The quoted expression is the modelled one with the anchoring pair dropped, and
    // nothing else: rule 3 changes how the pattern READS, never which values it admits.
    assert_eq!(
        format!("^{QUOTED_DISCOVERY_URL_PATTERN}$"),
        MODELLED_DISCOVERY_URL_PATTERN,
        "the quoted expression is the modelled one without its anchors, got \
         {QUOTED_DISCOVERY_URL_PATTERN}"
    );
    // Rule 5: the `/identities` family states the member and the requirement alone, so the
    // clause quotes no value of the caller's.
    let expected_message = format!(
        "1 validation error detected: Value at '{member_path}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {QUOTED_DISCOVERY_URL_PATTERN}"
    );

    // (case, provider name, the URL the request carries)
    let refused: [(&str, &str, String); 5] = [
        (
            "no well-known path at all",
            "custom-no-well-known",
            "https://example.com/not-a-discovery-endpoint".to_string(),
        ),
        (
            "the well-known path without a metadata document",
            "custom-bare-well-known",
            "https://example.com/.well-known/".to_string(),
        ),
        (
            "a well-known document the pattern does not admit",
            "custom-other-document",
            "https://example.com/.well-known/jwks.json".to_string(),
        ),
        (
            "the metadata document with no issuer before it",
            "custom-no-issuer",
            OPENID_CONFIGURATION.to_string(),
        ),
        (
            "the metadata document followed by another path segment",
            "custom-trailing-segment",
            format!("https://example.com{OPENID_CONFIGURATION}/v2"),
        ),
    ];
    for (case, provider, url) in refused {
        let (status, answer) = emulator
            .create(&json!({
                "name": provider,
                "credentialProviderVendor": "CustomOauth2",
                "oauth2ProviderConfigInput": custom_config(&url),
            }))
            .await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} ({answer})"
        );
        assert_eq!(
            answer["message"].as_str().unwrap_or_default(),
            expected_message,
            "case {case:?}: unexpected refusal message, got {answer}"
        );
        let (status, fetched) = emulator.get(provider).await;
        assert_eq!(
            (status, error_code(&fetched).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the refused create left no provider behind, got {status} ({fetched})"
        );
    }

    // (case, provider name, the URL the request carries)
    let served: [(&str, &str, String); 3] = [
        (
            "the OpenID Connect discovery document",
            "custom-openid",
            format!("https://accounts.example.com{OPENID_CONFIGURATION}"),
        ),
        (
            "the OAuth2 authorization-server metadata document",
            "custom-oauth-metadata",
            format!("https://auth.example.com{OAUTH_AUTHORIZATION_SERVER}"),
        ),
        (
            "the metadata document under a path of its own",
            "custom-nested-issuer",
            format!("https://example.com/tenants/t1{OPENID_CONFIGURATION}"),
        ),
    ];

    for (case, provider, url) in served {
        let (status, created) = emulator
            .create(&json!({
                "name": provider,
                "credentialProviderVendor": "CustomOauth2",
                "oauth2ProviderConfigInput": custom_config(&url),
            }))
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from the create, got {status} ({created})"
        );
        assert_eq!(
            created["oauth2ProviderConfigOutput"]["customOauth2ProviderConfig"]["oauthDiscovery"]
                ["discoveryUrl"],
            json!(url),
            "case {case:?}: the create reports the authorization server it was given, got \
             {created}"
        );

        // The provider is really held: the read reports the configuration the create
        // stored, and the listing names it.
        let (status, fetched) = emulator.get(provider).await;
        assert_eq!(
            (
                status,
                fetched["oauth2ProviderConfigOutput"].clone(),
                fetched["credentialProviderVendor"].clone()
            ),
            (
                200,
                created["oauth2ProviderConfigOutput"].clone(),
                json!("CustomOauth2")
            ),
            "case {case:?}: the read reports what the create stored, got {status} ({fetched})"
        );
        assert!(
            listed_names(&emulator.list().await.1).contains(&provider.to_string()),
            "case {case:?}: the created provider is missing from the listing"
        );
    }
}

/// A provider of a vendor whose configuration member carries no authorization server of
/// its own is reported with the document that vendor publishes, and the provider
/// round-trips through the read and the listing.
///
/// The rows differ along the vendor, including one whose server is scoped to the directory
/// the request names and one the union declares no member of its own for, which is
/// configured through the included member and names its own issuer.
#[tokio::test]
async fn a_built_in_vendors_provider_reports_the_document_that_vendor_publishes() {
    let emulator = Emulator::new();

    // (case, provider name, vendor, the configuration, the discovery expected back)
    let rows: [(&str, &str, &str, Value, String); 4] = [
        (
            "GoogleOauth2",
            "google-provider",
            "GoogleOauth2",
            vendor_config("googleOauth2ProviderConfig"),
            format!("https://accounts.google.com{OPENID_CONFIGURATION}"),
        ),
        (
            "SalesforceOauth2",
            "salesforce-provider",
            "SalesforceOauth2",
            vendor_config("salesforceOauth2ProviderConfig"),
            format!("https://login.salesforce.com{OPENID_CONFIGURATION}"),
        ),
        (
            "MicrosoftOauth2 scoped to the directory the request names",
            "microsoft-provider",
            "MicrosoftOauth2",
            json!({"microsoftOauth2ProviderConfig": {
                "clientId": "a-client",
                "clientSecret": "a-secret",
                "tenantId": "00000000-0000-0000-0000-000000000001",
            }}),
            format!(
                "https://login.microsoftonline.com/00000000-0000-0000-0000-000000000001/v2.0{OPENID_CONFIGURATION}"
            ),
        ),
        (
            "OktaOauth2, configured through the included member with its own issuer",
            "okta-provider",
            "OktaOauth2",
            json!({"includedOauth2ProviderConfig": {
                "clientId": "a-client",
                "clientSecret": "a-secret",
                "issuer": "https://dev-1.okta.example.com/",
            }}),
            format!("https://dev-1.okta.example.com{OPENID_CONFIGURATION}"),
        ),
    ];

    for (case, provider, vendor, configuration, expected_discovery) in rows {
        let (status, created) = emulator
            .create(&json!({
                "name": provider,
                "credentialProviderVendor": vendor,
                "oauth2ProviderConfigInput": configuration,
            }))
            .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from the create, got {status} ({created})"
        );
        // The client secret the caller handed over is stored in a secret rather than
        // echoed, so the answer names the secret and not the value.
        assert!(
            !created.to_string().contains("a-secret"),
            "case {case:?}: the create echoed the client secret, in {created}"
        );
        assert!(
            created["clientSecretArn"]["secretArn"]
                .as_str()
                .unwrap_or_default()
                .starts_with("arn:aws:secretsmanager:"),
            "case {case:?}: the create reports no Secrets Manager secret, in {created}"
        );

        let (status, fetched) = emulator.get(provider).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the read, got {status} ({fetched})"
        );
        let reported = fetched["oauth2ProviderConfigOutput"]
            .as_object()
            .and_then(|members| members.values().next().cloned())
            .unwrap_or(Value::Null);
        assert_eq!(
            reported["oauthDiscovery"]["discoveryUrl"],
            json!(expected_discovery),
            "case {case:?}: the read reports the vendor's published metadata document, got \
             {fetched}"
        );
        assert_eq!(
            fetched["credentialProviderVendor"],
            json!(vendor),
            "case {case:?}: the read reports the vendor the create declared, got {fetched}"
        );
    }

    // Every provider created above is in the account's listing, once each.
    let (status, listed) = emulator.list().await;
    let mut names = listed_names(&listed);
    names.sort();
    assert_eq!(
        (status, names),
        (
            200,
            vec![
                "google-provider".to_string(),
                "microsoft-provider".to_string(),
                "okta-provider".to_string(),
                "salesforce-provider".to_string(),
            ]
        ),
        "the listing reports every created provider once, got {listed}"
    );
}

/// An included provider that names no authorization server at all is refused: the
/// configuration cannot be reported without one.
#[tokio::test]
async fn an_included_provider_without_an_issuer_is_refused() {
    let emulator = Emulator::new();
    let (status, refused) = emulator
        .create(&json!({
            "name": "okta-no-issuer",
            "credentialProviderVendor": "OktaOauth2",
            "oauth2ProviderConfigInput": {"includedOauth2ProviderConfig": {
                "clientId": "a-client",
                "clientSecret": "a-secret",
            }},
        }))
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "an included provider naming no authorization server is refused, got {status} ({refused})"
    );
    let (status, fetched) = emulator.get("okta-no-issuer").await;
    assert_eq!(
        (status, error_code(&fetched).as_str()),
        (404, "ResourceNotFoundException"),
        "the refused create left no provider behind, got {status} ({fetched})"
    );
}
