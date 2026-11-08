// SPDX-License-Identifier: CC0-1.0

//! `DeletePaymentCredentialProvider`, and the read that reports what it removed, end to
//! end over HTTP.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. Secrets Manager is mounted
//! beside the control plane on the one shared engine — what the assembled emulator is —
//! because a provider's credentials live in that service.
//!
//! The rule under test, read off the Smithy model rather than off any one recorded request
//! (`smithy_query(query="operation",operation="DeletePaymentCredentialProvider,\
//! GetPaymentCredentialProvider")`): the delete removes the provider the `name` addresses
//! and answers its modelled success (HTTP 204) whether or not that name carried one — the
//! name is already in the state the delete establishes, which is what the recorded
//! behaviour of this operation shows — so what reports the removal is the READ:
//! `GetPaymentCredentialProvider` answers the modelled `ResourceNotFoundException` (HTTP
//! 404, "PaymentCredentialProvider not found") for a name that carries no provider,
//! whether it never did or the delete removed it. One provider's delete leaves every other
//! provider of the account untouched.
//!
//! The rows differ along what the rule generalises over: the shape and length of the name,
//! whether the name ever carried a provider, how many times the delete is repeated, and
//! the vendor of the provider being removed.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// The sentence this plane refuses a provider no name of this account resolves to with.
const NOT_FOUND: &str = "PaymentCredentialProvider not found";

/// A credential a row hands the service, which must never appear in a control-plane
/// response.
const API_KEY_SECRET: &str = "cdp-api-key-secret-value";
const WALLET_SECRET: &str = "cdp-wallet-secret-value";
const APP_SECRET: &str = "privy-app-secret-value";
const PRIVATE_KEY: &str = "privy-authorization-private-key-value";

/// One answer from the emulator: its status, its parsed body and the body as it came off
/// the wire.
struct Answer {
    status: u16,
    body: Value,
    raw: String,
}

impl Answer {
    fn of(response: http::Response<Bytes>) -> Self {
        let raw = String::from_utf8_lossy(response.body()).to_string();
        Self {
            status: response.status().as_u16(),
            body: if response.body().is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(response.body()).unwrap_or(Value::Null)
            },
            raw,
        }
    }
}

/// The emulator the rows run against: the control plane and Secrets Manager, on one engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::default();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One `restJson1` request to the control plane. The credential scope names
    /// `bedrock-agentcore`, which is what the router routes on.
    async fn control(&self, uri: &str, body: &Value) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
             aws4_request, SignedHeaders=host, Signature=abc"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("authorization", authorization)
            .header("host", "bedrock-agentcore-control.amazonaws.com")
            .header("content-type", "application/json")
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        Answer::of(self.router.dispatch(request).await)
    }

    async fn create_provider(&self, body: &Value) -> Answer {
        self.control("/identities/CreatePaymentCredentialProvider", body)
            .await
    }

    async fn get_provider(&self, name: &str) -> Answer {
        self.control(
            "/identities/GetPaymentCredentialProvider",
            &json!({"name": name}),
        )
        .await
    }

    async fn delete_provider(&self, name: &str) -> Answer {
        self.control(
            "/identities/DeletePaymentCredentialProvider",
            &json!({"name": name}),
        )
        .await
    }
}

/// The error code `restJson1` puts in the body's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `CreatePaymentCredentialProvider` body for `name`, with every credential a secret the
/// service manages, for the two vendors
/// (`smithy_query(query="shape",shape="PaymentCredentialProviderVendorType")`).
fn create_body(name: &str, vendor: &str) -> Value {
    let configuration = if vendor == "CoinbaseCDP" {
        json!({"coinbaseCdpConfiguration": {
            "apiKeyId": "cdp_api_key_1",
            "apiKeySecret": API_KEY_SECRET,
            "walletSecret": WALLET_SECRET,
        }})
    } else {
        json!({"stripePrivyConfiguration": {
            "appId": "privy_app_1",
            "appSecret": APP_SECRET,
            "authorizationPrivateKey": PRIVATE_KEY,
            "authorizationId": "privy_authorization_1",
        }})
    };
    json!({
        "name": name,
        "credentialProviderVendor": vendor,
        "providerConfigurationInput": configuration,
    })
}

/// The delete of a name that carries no provider is served, not refused: it asks for a
/// state the account is already in. The READ is what reports that nothing is there.
#[tokio::test]
async fn the_delete_of_a_name_that_carries_no_provider_is_served() {
    let emulator = Emulator::new();
    // Every name satisfies `CredentialProviderName`'s pattern `^[a-zA-Z0-9\-_]+$`
    // (1..128), so the answer is the handler's and not the boundary constraint walk's:
    // one character, hyphens, underscores, digits, mixed case, and the longest name the
    // length range admits.
    let names = [
        "a",
        "never-created",
        "never_created_either",
        "0123456789",
        "MixedCase-Provider_9",
        &"n".repeat(128),
    ];
    for name in names {
        let read = emulator.get_provider(name).await;
        assert_eq!(
            (read.status, error_code(&read.body).as_str(), text(&read.body, "message")),
            (404, "ResourceNotFoundException", NOT_FOUND.to_string()),
            "case {name:?}: the read of a name that carries no provider is the modelled \
             404, got {} {} ({})",
            read.status,
            error_code(&read.body),
            read.body
        );

        let removed = emulator.delete_provider(name).await;
        assert_eq!(
            removed.status, 204,
            "case {name:?}: the delete of that same name is served, got {} ({})",
            removed.status, removed.body
        );
        assert!(
            removed.body.get("__type").is_none(),
            "case {name:?}: a served delete answers no error envelope, got {}",
            removed.body
        );
        assert_eq!(
            emulator.get_provider(name).await.status,
            404,
            "case {name:?}: and the read still reports no provider"
        );
    }
}

/// A provider exists between its create and its delete: the delete removes it, the read
/// stops reporting it, and repeating the delete is served just as the first was.
#[tokio::test]
async fn the_delete_removes_the_provider_the_name_carried() {
    for vendor in ["CoinbaseCDP", "StripePrivy"] {
        let emulator = Emulator::new();
        let name = format!("deleted-{}", vendor.to_ascii_lowercase());
        let created = emulator.create_provider(&create_body(&name, vendor)).await;
        assert_eq!(
            created.status, 201,
            "case {vendor:?}: the provider must be created, got {} ({})",
            created.status, created.body
        );
        let read = emulator.get_provider(&name).await;
        assert_eq!(
            (read.status, text(&read.body, "credentialProviderVendor")),
            (200, vendor.to_string()),
            "case {vendor:?}: the provider reads back before the delete, got {} ({})",
            read.status,
            read.body
        );

        for repeat in 1..=3 {
            let removed = emulator.delete_provider(&name).await;
            assert_eq!(
                removed.status, 204,
                "case {vendor:?}: delete {repeat} is served, got {} ({})",
                removed.status, removed.body
            );
            let gone = emulator.get_provider(&name).await;
            assert_eq!(
                (gone.status, text(&gone.body, "message")),
                (404, NOT_FOUND.to_string()),
                "case {vendor:?}: after delete {repeat} the read reports no provider, got {} \
                 ({})",
                gone.status,
                gone.body
            );
            for credential in [API_KEY_SECRET, WALLET_SECRET, APP_SECRET, PRIVATE_KEY] {
                assert!(
                    !removed.raw.contains(credential) && !gone.raw.contains(credential),
                    "case {vendor:?}: no answer carries a credential, found {credential:?}"
                );
            }
        }
    }
}

/// The delete addresses ONE provider: the others of the account are untouched, and the
/// name is free for a create again afterwards.
#[tokio::test]
async fn the_delete_addresses_one_provider_of_the_account() {
    let emulator = Emulator::new();
    for name in ["kept-provider", "removed-provider"] {
        let created = emulator
            .create_provider(&create_body(name, "CoinbaseCDP"))
            .await;
        assert_eq!(
            created.status, 201,
            "setup {name:?}: the provider must be created, got {} ({})",
            created.status, created.body
        );
    }

    let removed = emulator.delete_provider("removed-provider").await;
    assert_eq!(
        removed.status, 204,
        "the delete is served, got {} ({})",
        removed.status, removed.body
    );
    let kept = emulator.get_provider("kept-provider").await;
    assert_eq!(
        kept.status, 200,
        "the other provider of the account is untouched, got {} ({})",
        kept.status, kept.body
    );
    let gone = emulator.get_provider("removed-provider").await;
    assert_eq!(
        (gone.status, text(&gone.body, "message")),
        (404, NOT_FOUND.to_string()),
        "the removed provider is gone, got {} ({})",
        gone.status,
        gone.body
    );

    // The name the delete freed takes a provider again, which is how a client recovers a
    // name it removed.
    let recreated = emulator
        .create_provider(&create_body("removed-provider", "StripePrivy"))
        .await;
    assert_eq!(
        recreated.status, 201,
        "the freed name takes a provider again, got {} ({})",
        recreated.status, recreated.body
    );
    assert_eq!(
        text(
            &emulator.get_provider("removed-provider").await.body,
            "credentialProviderVendor"
        ),
        "StripePrivy",
        "the recreated provider is the one just created, not the one that was removed"
    );
}
