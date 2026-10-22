// SPDX-License-Identifier: CC0-1.0

//! The fixture every Connect suite in this target drives, and the helpers more than one
//! of them needs.
//!
//! One emulator, one region, with the Connect service mounted, reached over HTTP through
//! the same `MultiServiceRouter` the emulator serves with. The region travels in the
//! SigV4 credential scope, which is also how the handlers learn which region to mint
//! ARNs in.
//!
//! A suite adds its own operation helpers in its own file, as an `impl Connect` block
//! there: this module holds only what two or more suites share, so no suite stands up a
//! second copy of the emulator fixture.

use std::sync::Arc;

use bytes::Bytes;
use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the HTTP status, the `x-amzn-errortype` error code restJson1
/// names a modelled error shape with (empty on a success), and the parsed JSON body.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error_code: String,
    pub(crate) body: Value,
}

/// The mounted emulator a suite sends requests to.
pub(crate) struct Connect {
    router: MultiServiceRouter,
    region: String,
}

impl Default for Connect {
    fn default() -> Self {
        Self::in_region("us-east-1")
    }
}

impl Connect {
    pub(crate) fn in_region(region: &str) -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self {
            router,
            region: region.to_string(),
        }
    }

    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let answer = self.send_answer(method, uri, body).await;
        (answer.status, answer.body)
    }

    /// The same call as [`Connect::send`], keeping the `x-amzn-errortype` header a
    /// suite needs to tell two modelled errors of the same status apart.
    pub(crate) async fn send_answer(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
        });
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "connect.amazonaws.com")
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=TESTKEY/20240101/{}/connect/aws4_request, \
                     SignedHeaders=host, Signature=abc",
                    self.region
                ),
            )
            .body(payload)
            .expect("a well-formed test request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error_code,
            body: parsed,
        }
    }

    /// `CreateInstance`, asserting success and handing back `(Id, Arn)`.
    pub(crate) async fn create_instance(&self, body: Value) -> (String, String) {
        let (status, out) = self.send("PUT", "/instance", Some(body.clone())).await;
        assert_eq!(status, 200, "CreateInstance {body}: got {out}");
        (
            out["Id"].as_str().expect("Id").to_string(),
            out["Arn"].as_str().expect("Arn").to_string(),
        )
    }
}

/// Build the `maxResults`/`nextToken` query string the `List*` models bind, plus any
/// number of repetitions of one list-valued query parameter (`queueTypes`,
/// `contactFlowTypes`). Pass an empty `values` for a `List*` that binds no such member.
pub(crate) fn query(
    max_results: Option<i32>,
    token: Option<&str>,
    repeated: &str,
    values: &[&str],
) -> String {
    let mut params: Vec<String> = Vec::new();
    if let Some(max) = max_results {
        params.push(format!("maxResults={max}"));
    }
    if let Some(token) = token {
        params.push(format!("nextToken={token}"));
    }
    for value in values {
        params.push(format!("{repeated}={value}"));
    }
    if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    }
}

/// A minimal valid `CreateInstance` body.
pub(crate) fn connect_managed(alias: &str) -> Value {
    json!({
        "IdentityManagementType": "CONNECT_MANAGED",
        "InstanceAlias": alias,
        "InboundCallsEnabled": true,
        "OutboundCallsEnabled": true,
    })
}

/// `8-4-4-4-12` lowercase hex: the shape of the v4 UUIDs Connect mints.
pub(crate) fn looks_like_a_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len && group.chars().all(|c| c.is_ascii_hexdigit())
            })
}

/// Pull one member out of every entry of a summary list.
pub(crate) fn ids_of(list: &Value, key: &str) -> Vec<String> {
    list.as_array()
        .expect("a summary list")
        .iter()
        .map(|entry| entry[key].as_str().expect("an id").to_string())
        .collect()
}
