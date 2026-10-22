// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every suite in this crate drives the service through.
//!
//! One shared engine with only this service mounted on it, called through
//! `MultiServiceRouter::dispatch` — the same entry the served emulator uses — so
//! a route, a label, a status code and a serialized body are all exercised, not
//! just a handler body.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here signs for; it must show up in every minted ARN.
pub(crate) const REGION: &str = "us-west-2";
/// The account segment `engine::emulator_arn` stamps into every ARN.
pub(crate) const ACCOUNT: &str = "000000000000";

/// One dispatched response: status, the restJson1 error code (which this
/// protocol carries in `x-amzn-errortype`), and the parsed JSON body.
#[derive(Debug)]
pub(crate) struct Resp {
    pub(crate) status: u16,
    pub(crate) error_type: Option<String>,
    pub(crate) body: Value,
}

impl Resp {
    pub(crate) fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }
}

/// One emulator: a shared engine with only this service mounted on it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests send unsigned-but-scoped requests: the `Authorization`
        // header is what routes them and names the region, not what authorises
        // them.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_iottwinmaker_awsiottwinmaker::EmulatedService::new(engine),
        );
        Self { router }
    }

    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Resp {
        let payload = match &body {
            Some(value) => {
                bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
            }
            None => bytes::Bytes::new(),
        };
        let host = format!("api.iottwinmaker.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/iottwinmaker/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(payload)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Resp {
            status,
            error_type,
            body,
        }
    }

    pub(crate) async fn create_workspace(&self, workspace_id: &str) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}"),
            Some(json!({})),
        )
        .await
    }

    pub(crate) async fn get_workspace(&self, id_or_arn: &str) -> Resp {
        self.call("GET", &format!("/workspaces/{id_or_arn}"), None).await
    }

    pub(crate) async fn create_entity(&self, workspace_id: &str, body: Value) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/entities"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn get_entity(&self, workspace_id: &str, entity_id: &str) -> Resp {
        self.call(
            "GET",
            &format!("/workspaces/{workspace_id}/entities/{entity_id}"),
            None,
        )
        .await
    }

    pub(crate) async fn list_entities(&self, workspace_id: &str, body: Value) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/entities-list"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn delete_entity(
        &self,
        workspace_id: &str,
        entity_id: &str,
        recursive: bool,
    ) -> Resp {
        let query = if recursive { "?isRecursive=true" } else { "" };
        self.call(
            "DELETE",
            &format!("/workspaces/{workspace_id}/entities/{entity_id}{query}"),
            None,
        )
        .await
    }

    pub(crate) async fn delete_workspace(&self, workspace_id: &str) -> Resp {
        self.call("DELETE", &format!("/workspaces/{workspace_id}"), None)
            .await
    }

    pub(crate) async fn create_component_type(
        &self,
        workspace_id: &str,
        component_type_id: &str,
        body: Value,
    ) -> Resp {
        self.call(
            "POST",
            &format!("/workspaces/{workspace_id}/component-types/{component_type_id}"),
            Some(body),
        )
        .await
    }

    pub(crate) async fn get_component_type(
        &self,
        workspace_id: &str,
        component_type_id: &str,
    ) -> Resp {
        self.call(
            "GET",
            &format!("/workspaces/{workspace_id}/component-types/{component_type_id}"),
            None,
        )
        .await
    }

    /// `TagResource`: POST /tags with the ARN and the tag map in the body.
    pub(crate) async fn tag_resource(&self, resource_arn: &str, tags: Value) -> Resp {
        self.call(
            "POST",
            "/tags",
            Some(json!({"resourceARN": resource_arn, "tags": tags})),
        )
        .await
    }

    /// `ListTagsForResource`: POST /tags-list, with any extra page members
    /// (`maxResults`, `nextToken`) merged into the body.
    pub(crate) async fn list_tags_for_resource(&self, resource_arn: &str, request: Value) -> Resp {
        let mut body = request;
        body["resourceARN"] = json!(resource_arn);
        self.call("POST", "/tags-list", Some(body)).await
    }

    /// `UntagResource`: DELETE /tags, with the ARN and every key as repeated
    /// `@httpQuery` parameters — the binding the model declares for them.
    pub(crate) async fn untag_resource(&self, resource_arn: &str, tag_keys: &[&str]) -> Resp {
        let mut query = format!("?resourceARN={}", encode_query(resource_arn));
        for key in tag_keys {
            query.push_str("&tagKeys=");
            query.push_str(&encode_query(key));
        }
        self.call("DELETE", &format!("/tags{query}"), None).await
    }
}

pub(crate) fn workspace_arn(workspace_id: &str) -> String {
    format!("arn:aws:iottwinmaker:{REGION}:{ACCOUNT}:workspace/{workspace_id}")
}

pub(crate) fn entity_arn(workspace_id: &str, entity_id: &str) -> String {
    format!("arn:aws:iottwinmaker:{REGION}:{ACCOUNT}:workspace/{workspace_id}/entity/{entity_id}")
}

pub(crate) fn component_type_arn(workspace_id: &str, component_type_id: &str) -> String {
    format!(
        "arn:aws:iottwinmaker:{REGION}:{ACCOUNT}:workspace/{workspace_id}/componentType/\
         {component_type_id}"
    )
}

/// Percent-encode a value for use as a query-parameter value, the way an SDK
/// does: everything outside RFC 3986's unreserved set is escaped, so a tag key
/// carrying `:`, `/`, `=`, `+` or a space survives the round trip.
pub(crate) fn encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// Percent-encode a value for use as a non-greedy `@httpLabel`, the way an SDK
/// does: `:` and `/` are reserved in a path segment, so an ARN passed as the
/// `IdOrArn` label arrives encoded.
pub(crate) fn encode_label(value: &str) -> String {
    value.replace('%', "%25").replace(':', "%3A").replace('/', "%2F")
}
