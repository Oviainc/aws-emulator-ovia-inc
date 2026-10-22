// SPDX-License-Identifier: CC0-1.0

//! The shared wire harness for this crate's suites: one emulator, one request builder,
//! and one reply assertion vocabulary, so no suite grows its own.
//!
//! Every call goes through the router's `dispatch`, so a request is deserialized,
//! constraint-checked, handled and serialized exactly as a client's would be — which is
//! the only way a suite can observe a refusal the framework states BEFORE a handler runs
//! (this service rewords those in `src/service/validation.rs`).
//!
//! `allow(dead_code)`: this module is the vocabulary for every suite, so an individual
//! suite uses only the part of it that its rules need.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::{
    SubscriptionProviderRecord, PROVIDER_STATUS_ACTIVE,
};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::{
    ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState,
    EmulatedService,
};

/// This service's sigv4 signing name (`OPERATIONS.md`, "Wire protocol").
pub const SIGNING_NAME: &str = "license-manager-linux-subscriptions";
/// The region the test requests are signed for.
pub const REGION: &str = "us-west-2";
/// The canonical account id every emulated ARN carries.
pub const ACCOUNT: &str = "000000000000";
/// The only `SubscriptionProviderSource` the model declares (`one of: RedHat`).
pub const RED_HAT: &str = "RedHat";
/// `SubscriptionProviderStatus` of a provider whose token the service holds and
/// accepted, named from the crate so a suite cannot hold a second copy of the value.
pub const ACTIVE: &str = PROVIDER_STATUS_ACTIVE;

/// One emulator: the router that dispatches, plus the shared engine its service was
/// built from (so a test can read or pre-fill the store the handlers use).
pub struct Harness {
    pub router: MultiServiceRouter,
    pub engine: Arc<engine::InMemoryEngine>,
}

pub fn harness() -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests sign nothing; authentication is the framework's concern, not this
    // service's.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    Harness { router, engine }
}

/// What a call answered: the HTTP status, the wire error code (if any) and the body.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub error_type: Option<String>,
    pub body: Value,
}

impl Reply {
    pub fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            self.status, self.body
        );
        &self.body
    }

    /// Assert this reply is the client error `code` with HTTP `status`.
    pub fn error(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            self.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            self.status, self.body
        );
        let actual = self.error_type.as_deref().unwrap_or("<none>");
        assert!(
            actual.ends_with(code),
            "case {case:?}: expected error code {code}, got {actual:?} with body {:?}",
            self.body
        );
    }

    /// The `message` member of an error body, or `""` when there is none.
    pub fn message(&self) -> &str {
        self.body.get("message").and_then(Value::as_str).unwrap_or("")
    }
}

/// Dispatch one request: `method` and `path` (which may carry a query string) exactly as
/// a client would send them, signed for `region`, with `body` as the JSON payload.
pub async fn send(
    h: &Harness,
    method: &str,
    region: &str,
    path: &str,
    body: Option<&Value>,
) -> Reply {
    let payload = match body {
        Some(body) => serde_json::to_vec(body).expect("the request body serializes"),
        None => Vec::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{SIGNING_NAME}.{region}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTEXAMPLE/20260101/{region}/{SIGNING_NAME}\
                 /aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = h.router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_error = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let error_type = header_error.or_else(|| {
        body.get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    Reply {
        status,
        error_type,
        body,
    }
}

pub async fn call_in_region(h: &Harness, region: &str, path: &str, body: &Value) -> Reply {
    send(h, "POST", region, path, Some(body)).await
}

pub async fn call(h: &Harness, path: &str, body: &Value) -> Reply {
    call_in_region(h, REGION, path, body).await
}

pub async fn register(h: &Harness, body: &Value) -> Reply {
    call(h, "/subscription/RegisterSubscriptionProvider", body).await
}

pub async fn get_provider(h: &Harness, arn: &str) -> Reply {
    call(
        h,
        "/subscription/GetRegisteredSubscriptionProvider",
        &json!({ "SubscriptionProviderArn": arn }),
    )
    .await
}

pub async fn list_providers(h: &Harness, body: &Value) -> Reply {
    call(h, "/subscription/ListRegisteredSubscriptionProviders", body).await
}

pub async fn deregister(h: &Harness, arn: &str) -> Reply {
    call(
        h,
        "/subscription/DeregisterSubscriptionProvider",
        &json!({ "SubscriptionProviderArn": arn }),
    )
    .await
}

pub async fn get_settings(h: &Harness) -> Reply {
    call(h, "/subscription/GetServiceSettings", &json!({})).await
}

pub async fn update_settings(h: &Harness, body: &Value) -> Reply {
    call(h, "/subscription/UpdateServiceSettings", body).await
}

/// `ListTagsForResource`: `GET /tags/{resourceArn}`.
pub async fn list_tags(h: &Harness, resource_arn: &str) -> Reply {
    send(
        h,
        "GET",
        REGION,
        &format!("/tags/{}", percent_encode(resource_arn)),
        None,
    )
    .await
}

/// `TagResource`: `PUT /tags/{resourceArn}`, whose body carries the `tags` map.
pub async fn tag(h: &Harness, resource_arn: &str, tags: &Value) -> Reply {
    send(
        h,
        "PUT",
        REGION,
        &format!("/tags/{}", percent_encode(resource_arn)),
        Some(&json!({ "tags": tags })),
    )
    .await
}

/// `UntagResource`: `DELETE /tags/{resourceArn}?tagKeys=…`.
pub async fn untag(h: &Harness, resource_arn: &str, tag_keys: &[&str]) -> Reply {
    let query: Vec<String> = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encode(key)))
        .collect();
    let path = format!(
        "/tags/{}?{}",
        percent_encode(resource_arn),
        query.join("&")
    );
    send(h, "DELETE", REGION, &path, None).await
}

/// Percent-encode everything outside the unreserved set (RFC 3986 §2.3:
/// `ALPHA / DIGIT / "-" / "." / "_" / "~"`), so a label or a query value carrying `:`,
/// `/` or a space reaches the server as the one value it is.
///
/// Written out here rather than pulled from an encoder crate ON PURPOSE: this is a
/// three-line client-side helper used only by this crate's tests, and adding a
/// `[dev-dependencies]` entry for it changes `Cargo.lock` — the one file every other
/// change in this repository also touches, which makes a fix that needs none of it
/// conflict on landing. The server side's decoding is what the tests actually exercise.
pub fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// An ARN of this service's one resource kind, for `id`.
pub fn provider_arn_for(id: &str) -> String {
    format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:subscription-provider/{id}")
}

/// Seed one registration DIRECTLY into the store, and answer its ARN.
///
/// `RegisterSubscriptionProvider` cannot create one here: the real service registers a
/// provider only once it has validated the access token in the referenced secret against
/// that provider, which this emulator cannot do (see
/// `src/ops/register_subscription_provider.rs`), so it refuses every registration. The
/// read, list, deregister and tag operations still have to answer from the store, so a
/// suite that exercises them puts the records there itself — the same seam the handlers
/// use (`engine.state::<…>()`), not a second copy of the store.
pub fn seed_provider(
    h: &Harness,
    id: &str,
    secret_arn: &str,
    tags: &[(&str, &str)],
) -> String {
    let arn = provider_arn_for(id);
    let state = h
        .engine
        .state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>();
    let mut state = state.lock();
    state.providers.insert(
        arn.clone(),
        SubscriptionProviderRecord {
            arn: arn.clone(),
            source: RED_HAT.to_string(),
            secret_arn: secret_arn.to_string(),
            status: ACTIVE.to_string(),
            status_message: None,
            last_successful_data_retrieval_time: None,
            tags: tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect::<HashMap<String, String>>(),
        },
    );
    state.provider_order.push(arn.clone());
    arn
}

/// How many registrations the store holds.
pub fn stored_provider_count(h: &Harness) -> usize {
    h.engine
        .state::<ComAmazonawsLicensemanagerlinuxsubscriptionsLicensemanagerlinuxsubscriptionsState>()
        .lock()
        .providers
        .len()
}

/// The `SubscriptionProviderArn` member of a 200 body.
pub fn provider_arn(reply: &Reply, case: &str) -> String {
    reply
        .ok(case)
        .get("SubscriptionProviderArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no SubscriptionProviderArn in {:?}", reply.body))
        .to_string()
}

/// The providers a list body reports, as `(arn, source, secret arn, status)` rows.
pub fn listed(reply: &Reply, case: &str) -> Vec<(String, String, String, String)> {
    reply
        .ok(case)
        .get("RegisteredSubscriptionProviders")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: no RegisteredSubscriptionProviders in {:?}",
                reply.body
            )
        })
        .iter()
        .map(|entry| {
            let member = |name: &str| {
                entry
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case:?}: no {name} in {entry:?}"))
                    .to_string()
            };
            (
                member("SubscriptionProviderArn"),
                member("SubscriptionProviderSource"),
                member("SecretArn"),
                member("SubscriptionProviderStatus"),
            )
        })
        .collect()
}
