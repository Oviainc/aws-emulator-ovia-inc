// SPDX-License-Identifier: CC0-1.0

//! The one harness the S3 Tables suites drive the emulator through.
//!
//! Every suite goes over the router (`MultiServiceRouter::dispatch` on a restJson1
//! request whose SigV4 credential scope names the `s3tables` signing name), so each
//! case exercises routing, deserialization, the handler, the persisted state and
//! serialization — the same path an AWS SDK client takes. The request helpers live
//! here rather than in one suite, so two suites cannot disagree about how an
//! operation is addressed.
//!
//! `table_bucket_lifecycle.rs` still carries its own copy of the router bootstrap:
//! it covers the bucket create/read rules and is left to migrate with its next edit
//! rather than be rewritten from under its owner here. (No list of suites is kept
//! here, so this comment cannot rot — `tests/all.rs` is the register.)

// Each suite uses the part of the harness its own rules need; an item no suite has
// reached yet is still part of the contract this file states.
#![allow(dead_code)]

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::Value;
use service_com_amazonaws_s3tables_s3tablebuckets as s3tables;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`engine::emulator_account_id()`),
/// restated so an ARN assertion reads whole.
pub const ACCOUNT: &str = "000000000000";

/// One response, reduced to what a client branches on.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub error_code: Option<String>,
    pub body: Value,
}

impl Reply {
    /// A member of the body, as a string. Panics when it is absent, because a
    /// required member the model declares is never absent from a success.
    pub fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "response has no string `{member}` member: {body}",
                    body = self.body
                )
            })
            .to_string()
    }

    /// A member of the body, as a string, or `None` when the response carries
    /// none.
    pub fn optional_text(&self, member: &str) -> Option<String> {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// A member of the body, as a list.
    pub fn list(&self, member: &str) -> Vec<Value> {
        self.body
            .get(member)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "response has no list `{member}` member: {body}",
                    body = self.body
                )
            })
    }

    /// The `message` member of an error body, when it carries one.
    pub fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
            .to_string()
    }
}

/// The emulator under test: ONE shared engine with the S3 Tables service mounted
/// on it, reached the way the production router reaches it.
pub struct Emulator {
    router: MultiServiceRouter,
    pub engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(s3tables::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call one operation over the wire. `region` is the region the request's
    /// SigV4 credential scope names, which is the region the handlers scope an
    /// ARN to.
    pub async fn call(
        &self,
        method: &str,
        path: &str,
        region: &str,
        body: Option<&Value>,
    ) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTS3TABLES/20260101/{region}/s3tables/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0000"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3tables.{region}.amazonaws.com{path}"))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(|value| {
                value
                    .rsplit('#')
                    .next()
                    .unwrap_or(value)
                    .split(':')
                    .next()
                    .unwrap_or(value)
                    .to_string()
            });
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    pub async fn create_table_bucket(&self, region: &str, request: &Value) -> Reply {
        self.call("PUT", "/buckets", region, Some(request)).await
    }

    pub async fn get_table_bucket(&self, region: &str, arn: &str) -> Reply {
        self.call("GET", &format!("/buckets/{}", encode(arn)), region, None)
            .await
    }

    pub async fn list_table_buckets(&self, region: &str, query: &[(&str, String)]) -> Reply {
        self.call("GET", &format!("/buckets{}", query_string(query)), region, None)
            .await
    }

    pub async fn delete_table_bucket(&self, region: &str, arn: &str) -> Reply {
        self.call("DELETE", &format!("/buckets/{}", encode(arn)), region, None)
            .await
    }

    pub async fn create_namespace(&self, region: &str, arn: &str, request: &Value) -> Reply {
        self.call(
            "PUT",
            &format!("/namespaces/{}", encode(arn)),
            region,
            Some(request),
        )
        .await
    }

    pub async fn get_namespace(&self, region: &str, arn: &str, namespace: &str) -> Reply {
        self.call(
            "GET",
            &format!("/namespaces/{}/{}", encode(arn), encode(namespace)),
            region,
            None,
        )
        .await
    }

    pub async fn delete_namespace(&self, region: &str, arn: &str, namespace: &str) -> Reply {
        self.call(
            "DELETE",
            &format!("/namespaces/{}/{}", encode(arn), encode(namespace)),
            region,
            None,
        )
        .await
    }

    pub async fn create_table(
        &self,
        region: &str,
        arn: &str,
        namespace: &str,
        request: &Value,
    ) -> Reply {
        self.call(
            "PUT",
            &format!("/tables/{}/{}", encode(arn), encode(namespace)),
            region,
            Some(request),
        )
        .await
    }

    /// `GetTable` addressed by the bucket, the namespace and the name.
    pub async fn get_table(
        &self,
        region: &str,
        arn: &str,
        namespace: &str,
        name: &str,
    ) -> Reply {
        self.call(
            "GET",
            &format!(
                "/get-table{}",
                query_string(&[
                    ("tableBucketARN", arn.to_string()),
                    ("namespace", namespace.to_string()),
                    ("name", name.to_string()),
                ])
            ),
            region,
            None,
        )
        .await
    }

    /// `GetTable` addressed by the table's own ARN.
    pub async fn get_table_by_arn(&self, region: &str, table_arn: &str) -> Reply {
        self.call(
            "GET",
            &format!(
                "/get-table{}",
                query_string(&[("tableArn", table_arn.to_string())])
            ),
            region,
            None,
        )
        .await
    }

    /// `GetTable` with whichever members the case names, so a suite can address
    /// it incompletely on purpose.
    pub async fn get_table_with(&self, region: &str, query: &[(&str, String)]) -> Reply {
        self.call("GET", &format!("/get-table{}", query_string(query)), region, None)
            .await
    }

    pub async fn list_tables(
        &self,
        region: &str,
        arn: &str,
        query: &[(&str, String)],
    ) -> Reply {
        self.call(
            "GET",
            &format!("/tables/{}{}", encode(arn), query_string(query)),
            region,
            None,
        )
        .await
    }

    pub async fn delete_table(
        &self,
        region: &str,
        arn: &str,
        namespace: &str,
        name: &str,
        query: &[(&str, String)],
    ) -> Reply {
        self.call(
            "DELETE",
            &format!(
                "/tables/{}/{}/{}{}",
                encode(arn),
                encode(namespace),
                encode(name),
                query_string(query)
            ),
            region,
            None,
        )
        .await
    }
}

impl Default for Emulator {
    fn default() -> Self {
        Self::new()
    }
}

/// An `@httpLabel` is ONE path segment and a query value ends at the next `&`, so
/// an ARN's `:` and `/` have to travel as `%3A` and `%2F` or the request
/// addresses a different route.
pub fn encode(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
        .replace('&', "%26")
        .replace('=', "%3D")
}

/// The query string for `parameters`, or the empty string when there are none.
pub fn query_string(parameters: &[(&str, String)]) -> String {
    if parameters.is_empty() {
        return String::new();
    }
    let pairs: Vec<String> = parameters
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect();
    format!("?{}", pairs.join("&"))
}

/// The ARN `CreateTableBucket` must mint for `name` in `region`.
pub fn expected_bucket_arn(region: &str, name: &str) -> String {
    format!("arn:aws:s3tables:{region}:{ACCOUNT}:bucket/{name}")
}

/// A timestamp member as epoch seconds, accepting either wire form a timestamp
/// can take (the model marks these `date-time`, so a string is what is expected).
pub fn timestamp_seconds(body: &Value, member: &str, case: &str) -> f64 {
    match body.get(member) {
        Some(Value::String(text)) => DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {member} {text:?} is not a date-time: {error}")
            })
            .as_secs_f64(),
        Some(Value::Number(number)) => number.as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: {member} {number} is not representable as seconds")
        }),
        other => panic!("case {case:?}: {member} is missing or not a timestamp: {other:?}"),
    }
}

pub fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}
