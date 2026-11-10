// SPDX-License-Identifier: CC0-1.0

//! The one wire harness this crate's suites drive the emulator through.
//!
//! Every call goes the whole way round — `MultiServiceRouter::dispatch` with a
//! signed-looking restJson1 request — so the routes, the `@httpLabel` /
//! `@httpQuery` bindings, the boundary constraint checks and the handlers are all
//! exercised the way a client exercises them. Suites share it so a route or an
//! envelope is spelled in one place.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::{
    RESOURCE_TYPE_ACCOUNT, RESOURCE_TYPE_CLUSTER, RESOURCE_TYPE_STREAM, SETTLE_AFTER,
};
use smithy_server_runtime::MultiServiceRouter;

/// One emulator plus the region its requests are signed for.
pub struct Dsql {
    router: MultiServiceRouter,
    region: String,
}

/// One answer from the emulator: the status, the restJson1 error code, and the body.
pub struct Answer {
    pub status: u16,
    pub error: Option<String>,
    pub body: Value,
}

impl Answer {
    /// A string member of the response body, or the empty string when absent.
    pub fn str(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A member of the response body, or `Value::Null` when absent.
    pub fn get(&self, member: &str) -> Value {
        self.body.get(member).cloned().unwrap_or(Value::Null)
    }

    /// The `message` of an error body, whichever casing the envelope used.
    pub fn message(&self) -> String {
        for member in ["message", "Message"] {
            if let Some(message) = self.body.get(member).and_then(Value::as_str) {
                return message.to_string();
            }
        }
        String::new()
    }
}

impl Dsql {
    pub fn in_region(region: &str) -> Self {
        // The one shared engine the router builds in production, handed to the
        // service through its constructor.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        Self::sharing(&engine, region)
    }

    /// Another view of an EXISTING store, signed for `region`: the service is built
    /// from the same engine `Arc`, which is exactly what the production router does —
    /// so two views signed for different regions read one set of clusters.
    pub fn sharing(engine: &Arc<engine::InMemoryEngine>, region: &str) -> Self {
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_dsql_dsql::EmulatedService::new(
            Arc::clone(engine),
        ));
        Self {
            router,
            region: region.to_string(),
        }
    }

    /// An emulator holding Aurora DSQL beside the services a stream's
    /// `targetDefinition` NAMES: Amazon Kinesis, which owns the data stream the
    /// target addresses, and IAM, which holds the role it asks this service to
    /// assume. One engine and one router, the shape `core/router/src/lib.rs`
    /// mounts every service in — so a `CreateStream` resolves its target against
    /// the very streams and roles a client created through those services' own
    /// APIs rather than against a second copy of them.
    pub fn with_delivery_services(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(service_com_amazonaws_dsql_dsql::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self {
            router,
            region: region.to_string(),
        }
    }

    /// One `awsJson1_1` RPC against the service whose signing name is
    /// `signing_name` and whose target prefix is `target_prefix` — how a client
    /// calls Amazon Kinesis (`Kinesis_20131202`). The credential scope is what
    /// routes it, exactly as in [`Self::call`].
    pub async fn rpc(
        &self,
        signing_name: &str,
        target_prefix: &str,
        operation: &str,
        body: Value,
    ) -> Answer {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{}.amazonaws.com", self.region))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef",
                    self.region
                ),
            )
            .body(::bytes::Bytes::from(
                serde_json::to_vec(&body).expect("a JSON body serializes"),
            ))
            .expect("the request builds");
        Self::answer(self.router.dispatch(request).await)
    }

    /// One `awsQuery` call against IAM — the protocol that service speaks — as the
    /// status and the XML document it answered.
    pub async fn iam(&self, action: &str, fields: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in fields {
            body.push_str(&format!("&{name}={}", form_encode(value)));
        }
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDTESTTESTTESTTEST/20260101/us-east-1/iam/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef",
            )
            .body(::bytes::Bytes::from(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (
            status,
            String::from_utf8_lossy(&response.into_body()).to_string(),
        )
    }

    pub fn new() -> Self {
        Self::in_region("us-east-1")
    }

    /// The region this emulator's requests are signed for.
    pub fn region(&self) -> &str {
        &self.region
    }

    pub async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|v| serde_json::to_vec(&v).expect("a JSON body serializes"))
            .unwrap_or_default();
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://dsql.{}.amazonaws.com{path}", self.region))
            // The routing key and the region both come from the SigV4 credential
            // scope, exactly as they do for an SDK request.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{}/dsql/aws4_request, \
                     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef",
                    self.region
                ),
            )
            .header("content-type", "application/json")
            .body(::bytes::Bytes::from(payload))
            .expect("the request builds");

        let response = self.router.dispatch(request).await;
        Self::answer(response)
    }

    /// The status, the restJson1 error code and the body of one dispatched
    /// response — the one decoding every call in this harness answers through.
    fn answer(response: ::http::Response<::bytes::Bytes>) -> Answer {
        let status = response.status().as_u16();
        let error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        let error = error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        Answer {
            status,
            error,
            body,
        }
    }

    pub async fn create(&self, body: Value) -> Answer {
        self.call("POST", "/cluster", Some(body)).await
    }

    pub async fn get(&self, identifier: &str) -> Answer {
        self.call("GET", &format!("/cluster/{identifier}"), None).await
    }

    pub async fn update(&self, identifier: &str, body: Value) -> Answer {
        self.call("POST", &format!("/cluster/{identifier}"), Some(body))
            .await
    }

    pub async fn delete(&self, identifier: &str) -> Answer {
        self.call("DELETE", &format!("/cluster/{identifier}"), None)
            .await
    }

    /// `DeleteCluster` carrying `clientToken`, which the model binds to the
    /// `client-token` query parameter.
    pub async fn delete_with_token(&self, identifier: &str, token: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/cluster/{identifier}?client-token={token}"),
            None,
        )
        .await
    }

    pub async fn list(&self, query: &str) -> Answer {
        self.call("GET", &format!("/cluster{query}"), None).await
    }

    pub async fn get_policy(&self, identifier: &str) -> Answer {
        self.call("GET", &format!("/cluster/{identifier}/policy"), None)
            .await
    }

    pub async fn put_policy(&self, identifier: &str, body: Value) -> Answer {
        self.call(
            "POST",
            &format!("/cluster/{identifier}/policy"),
            Some(body),
        )
        .await
    }

    pub async fn delete_policy(&self, identifier: &str, query: &str) -> Answer {
        self.call(
            "DELETE",
            &format!("/cluster/{identifier}/policy{query}"),
            None,
        )
        .await
    }

    pub async fn create_stream(&self, cluster_identifier: &str, body: Value) -> Answer {
        self.call(
            "POST",
            &format!("/stream/{cluster_identifier}"),
            Some(body),
        )
        .await
    }

    pub async fn get_stream(&self, cluster_identifier: &str, stream_identifier: &str) -> Answer {
        self.call(
            "GET",
            &format!("/stream/{cluster_identifier}/{stream_identifier}"),
            None,
        )
        .await
    }

    /// `DeleteStream`: `DELETE /stream/{clusterIdentifier}/{streamIdentifier}`,
    /// with whatever `@httpQuery` string the case needs (`?client-token=..`).
    pub async fn delete_stream(
        &self,
        cluster_identifier: &str,
        stream_identifier: &str,
        query: &str,
    ) -> Answer {
        self.call(
            "DELETE",
            &format!("/stream/{cluster_identifier}/{stream_identifier}{query}"),
            None,
        )
        .await
    }

    /// `ListStreams`: `GET /stream/{clusterIdentifier}`, with whatever
    /// `@httpQuery` string the case needs (`?max-results=..&next-token=..`).
    pub async fn list_streams(&self, cluster_identifier: &str, query: &str) -> Answer {
        self.call("GET", &format!("/stream/{cluster_identifier}{query}"), None)
            .await
    }

    pub async fn tag_resource(&self, resource_arn: &str, body: Value) -> Answer {
        // `resourceArn` is an `@httpLabel`, so its colons and slashes travel
        // percent-encoded the way an SDK sends them.
        self.call(
            "POST",
            &format!("/tags/{}", percent_encode_label(resource_arn)),
            Some(body),
        )
        .await
    }

    /// `GET /tags/{resourceArn}`: the tags persisted on an ARN-addressed resource.
    pub async fn list_tags_for_resource(&self, resource_arn: &str) -> Answer {
        self.call(
            "GET",
            &format!("/tags/{}", percent_encode_label(resource_arn)),
            None,
        )
        .await
    }

    /// Create a cluster that accepts a mutation straight away: deletion protection
    /// off, and the clock's `CREATING -> ACTIVE` transition already settled.
    pub async fn create_settled(&self) -> String {
        let created = self.create(json!({"deletionProtectionEnabled": false})).await;
        assert_eq!(
            created.status, 200,
            "CreateCluster must succeed, got {} {:?}",
            created.status, created.body
        );
        wait_for_the_clock().await;
        created.str("identifier")
    }

    /// `count` clusters that accept a mutation straight away, all created before the
    /// one wait — so a table-driven suite waits for the clock once rather than per
    /// case.
    pub async fn create_settled_many(&self, count: usize) -> Vec<String> {
        let mut identifiers = Vec::with_capacity(count);
        for _ in 0..count {
            let created = self.create(json!({"deletionProtectionEnabled": false})).await;
            assert_eq!(
                created.status, 200,
                "CreateCluster must succeed, got {} {:?}",
                created.status, created.body
            );
            identifiers.push(created.str("identifier"));
        }
        wait_for_the_clock().await;
        identifiers
    }
}

/// Percent-encode the characters an ARN carries that a URI path segment does not.
pub fn percent_encode_label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// One value as an `application/x-www-form-urlencoded` field, encoded by the
/// `percent-encoding` crate, which is how IAM's query protocol carries a policy
/// document. Everything outside the unreserved set is encoded.
fn form_encode(value: &str) -> String {
    ::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The text of the first `<element>` of an XML document, or `None` when it carries
/// none — how a suite reads one value out of an `awsQuery` answer.
pub fn xml_text(document: &str, element: &str) -> Option<String> {
    document
        .split_once(&format!("<{element}>"))
        .and_then(|(_, tail)| tail.split_once(&format!("</{element}>")))
        .map(|(text, _)| text.to_string())
}

/// Let the clock's pending transition come due. The emulator never sleeps for it —
/// it stores the due instant — so a test that wants the settled state waits past it.
pub async fn wait_for_the_clock() {
    ::tokio::time::sleep(SETTLE_AFTER + Duration::from_millis(150)).await;
}

/// `ClusterId`'s `^[a-z0-9]{26}$`.
pub fn is_cluster_id(value: &str) -> bool {
    value.len() == 26
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// The ARN `ClusterArn`'s pattern requires for `identifier` in `region`.
pub fn expected_arn(region: &str, identifier: &str) -> String {
    format!(
        "arn:aws:dsql:{region}:{}:cluster/{identifier}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The ARN `StreamArn`'s pattern requires for a stream of a cluster: the stream is
/// nested UNDER its cluster's resource path
/// (`cluster/{cluster}/stream/{stream}`, `state::stream_arn`).
pub fn expected_stream_arn(region: &str, cluster: &str, stream: &str) -> String {
    format!(
        "arn:aws:dsql:{region}:{}:cluster/{cluster}/stream/{stream}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The account segment of an ARN this service minted
/// (`arn:{partition}:dsql:{region}:{account}:{resource}`) — how a suite reads the
/// account id a refusal about a per-account collection reports, from an ARN the
/// service itself answered with rather than from a literal.
pub fn account_of(arn: &str) -> String {
    arn.split(':').nth(4).unwrap_or_default().to_string()
}

/// Whether `resource_id` really is an identifier of a `resource_type` this
/// emulator holds, asked the way a client would ask rather than by re-reading a
/// literal: a cluster through `GetCluster`, a stream of `cluster` through
/// `GetStream`, and the account through the account segment of `cluster_arn`, an
/// ARN this service minted for a resource it holds.
///
/// The one place the identity pair of a refusal is resolved, so every suite that
/// asserts the rule — one resource's own type beside that same resource's own
/// identifier — asks it the same way.
pub async fn names_a_resource_of_its_own_type(
    dsql: &Dsql,
    cluster: &str,
    cluster_arn: &str,
    resource_type: &str,
    resource_id: &str,
) -> bool {
    match resource_type {
        RESOURCE_TYPE_CLUSTER => dsql.get(resource_id).await.status == 200,
        RESOURCE_TYPE_STREAM => dsql.get_stream(cluster, resource_id).await.status == 200,
        RESOURCE_TYPE_ACCOUNT => resource_id == account_of(cluster_arn),
        _ => false,
    }
}

/// A well-formed cluster id for a cluster that was never created.
///
/// Well-formed is the form an identifier of this service is ISSUED in: 26
/// characters of the base32 alphabet, inside the value range the mint draws from,
/// which satisfies `ClusterId`'s `^[a-z0-9]{26}$` (`smithy_query(query="shape",
/// shape="ClusterId")`). Every value of that form reaches the store on every
/// route, so one the account does not hold is reported as an absent resource
/// rather than refused as input, which is what
/// `tests/all/resource_identifier_absence.rs` pins; a 26-character
/// lowercase-alphanumeric value using a letter the alphabet leaves out is refused
/// as input, which `tests/all/identifier_format_refusal.rs` pins, and one above
/// the minted range is answered per operation, which
/// `tests/all/identifier_value_range.rs` pins.
pub fn absent_identifier() -> String {
    "0zzzzzzzzzzzzzzzzzzzzzzzzz".to_string()
}

/// The published bounds of this service's tag shapes, shared by every suite that
/// asserts on tags. Source: `smithy_query(query="shape",
/// shape="TagKey,TagValue,TagMap")` — key `@length(1..128)`, value
/// `@length(0..256)`, both `@pattern("^[a-zA-Z0-9_.:/=+\\-@ ]*$")`, and the map
/// itself `@length(0..200)`.
pub const TAG_KEY_MAX: usize = 128;
/// See [`TAG_KEY_MAX`].
pub const TAG_VALUE_MAX: usize = 256;
/// See [`TAG_KEY_MAX`]: the most tags one resource holds.
pub const TAG_MAP_MAX: usize = 200;
