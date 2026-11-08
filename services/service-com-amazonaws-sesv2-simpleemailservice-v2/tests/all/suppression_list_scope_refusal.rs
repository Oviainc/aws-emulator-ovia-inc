// SPDX-License-Identifier: CC0-1.0

//! One address, every suppression list of one account: which list each read answers
//! about, and which list a refusal names.
//!
//! The case tables for the suppression read and its refusals live in
//! `suppressed_destinations.rs`, each row against an account of its own. This suite
//! covers what a per-row account cannot show: ONE account in which a single address
//! sits on some of its lists and not others, read through every list it has. The rule
//! it pins is that the list a read answers about — and the list its refusal names: a
//! tenant's suppression list when the request scoped the lookup to a tenant, the
//! account's when it did not — is selected by the REQUEST and not by the address, so
//! the same address is reported on one list and refused on another within one state.
//!
//! The case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, label and query decoding, the handler and the
//! error envelope are all exercised.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    suppression_key, ComAmazonawsSesv2SimpleemailserviceV2State, SuppressedDestinationRecord,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The emulated service behind its router, with the engine its state is reached
/// through so a list can be seeded and then read back over the wire.
struct Ses {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // This suite exercises the suppression surface, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine.clone()));
        Self { router, engine }
    }

    /// The status, the error code and the parsed body of one answered request.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, String, Value) {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.us-east-1.amazonaws.com{path}"))
            .header("host", "email.us-east-1.amazonaws.com")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/us-east-1/ses/aws4_request, SignedHeaders=host, Signature=00",
            );
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request.body(payload).expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let code = raw
            .split('#')
            .next_back()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string();
        (status, code, body)
    }

    /// `GetSuppressedDestination`: the address as a path label, and the tenant whose
    /// list to read as the `TenantName` query parameter when the read names one.
    async fn get_suppressed(
        &self,
        address: &str,
        tenant: Option<&str>,
    ) -> (u16, String, Value) {
        let query = match tenant {
            Some(tenant) => format!("?TenantName={tenant}"),
            None => String::new(),
        };
        self.call(
            "GET",
            &format!("/v2/email/suppression/addresses/{address}{query}"),
            None,
        )
        .await
    }

    /// `CreateTenant`, so a read scoped to a tenant names one this account holds.
    async fn create_tenant(&self, name: &str) {
        let (status, _, body) = self
            .call("POST", "/v2/email/tenants", Some(json!({"TenantName": name})))
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the tenant to be created, got {status} {body:?}"
        );
    }

    /// Put one address on one of this account's suppression lists.
    fn seed(&self, tenant: Option<&str>, address: &str) {
        let record = SuppressedDestinationRecord {
            email_address: address.to_string(),
            reason: "BOUNCE".to_string(),
            last_update_time: SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600),
            message_id: None,
            feedback_id: None,
            tenant_name: tenant.map(str::to_string),
        };
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let mut store = state.lock();
        let stored = store.suppress_destination(suppression_key(tenant, address), record);
        assert!(
            stored,
            "case {tenant:?}: expected the suppression-list entry to be stored"
        );
    }
}

#[tokio::test]
async fn one_address_is_reported_or_refused_per_list_within_one_account() {
    let address = "shared@example.com";
    // Four lists in one account, differing in both spellings a `TenantName` admits,
    // and the address put on half of them.
    let lists: [(Option<&str>, bool); 4] = [
        (None, true),
        (Some("tenant-one"), true),
        (Some("tenant_two"), false),
        (Some("tenant-three"), false),
    ];

    let ses = Ses::new();
    for (list, _) in lists {
        if let Some(tenant) = list {
            ses.create_tenant(tenant).await;
        }
    }
    for (list, suppressed) in lists {
        if suppressed {
            ses.seed(list, address);
        }
    }

    for (list, suppressed) in lists {
        let (status, code, body) = ses.get_suppressed(address, list).await;
        if suppressed {
            assert_eq!(
                status, 200,
                "case {list:?}: expected the address to be reported on this list, got {status} {body:?}"
            );
            // The entry reports the list it is on, so the read answered about the
            // list the request selected and not about another one holding the
            // same address.
            let reported = body
                .get("SuppressedDestination")
                .and_then(|entry| entry.get("TenantName"))
                .and_then(Value::as_str);
            assert_eq!(
                reported, list,
                "case {list:?}: expected the entry to report the list it is on, got {reported:?}"
            );
            continue;
        }
        assert_eq!(
            (status, code.as_str()),
            (404, "NotFoundException"),
            "case {list:?}: expected 404 NotFoundException for a list without the address, got {status} {code} {body:?}"
        );
        let message = body
            .get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let named = match list {
            Some(_) => "tenant suppression list",
            None => "suppression list",
        };
        assert_eq!(
            message,
            format!("Email address {address} does not exist on your {named}."),
            "case {list:?}: expected the refusal to name the {named}, got {message:?}"
        );
    }
}
