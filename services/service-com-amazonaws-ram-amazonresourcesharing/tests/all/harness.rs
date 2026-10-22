// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives its cases through.
//!
//! One place for "how a request reaches this service": the production
//! `MultiServiceRouter`, restJson1 serialization, the modelled-constraint walk and the
//! handlers. A suite adds CASES, never another way of calling.
//!
//! Extracted from `resource_shares.rs` so a second suite reuses it instead of growing
//! a second copy.
#![allow(dead_code)]

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for; the ARNs must carry exactly this one.
pub const REGION: &str = "us-west-2";
/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries,
/// and the account that owns every share these tests create.
pub const ACCOUNT: &str = "000000000000";

/// A resource ARN in the account that owns the shares: an internal entity.
pub const LOCAL_RESOURCE: &str = "arn:aws:ec2:us-west-2:000000000000:subnet/subnet-1111";
/// A second one, so a row can carry more than one resource.
pub const LOCAL_RESOURCE_TWO: &str = "arn:aws:ec2:us-west-2:000000000000:subnet/subnet-2222";
/// A resource ARN in another account: an external entity.
pub const FOREIGN_RESOURCE: &str = "arn:aws:ec2:us-west-2:111122223333:subnet/subnet-3333";

/// One emulator with an empty store, mounted exactly as the router mounts it.
pub fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ram_amazonresourcesharing::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ram` in [`REGION`], the signal the router routes on and the
/// handlers read the region from.
pub fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ram.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDRAMTEST/20260101/{REGION}/ram/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
pub async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ram#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` a modelled error reported.
pub fn error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Percent-encode a value for use in a query string, which is what an SDK does with
/// an `@httpQuery` member.
pub fn query_escape(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

pub async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/createresourceshare", Some(body)).await
}

pub async fn get_shares(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/getresourceshares", Some(body)).await
}

pub async fn associate(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/associateresourceshare", Some(body)).await
}

pub async fn disassociate(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/disassociateresourceshare", Some(body)).await
}

pub async fn get_associations(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/getresourceshareassociations", Some(body)).await
}

pub async fn update(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/updateresourceshare", Some(body)).await
}

pub async fn tag_resource(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/tagresource", Some(body)).await
}

pub async fn untag_resource(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/untagresource", Some(body)).await
}

pub async fn delete(
    router: &MultiServiceRouter,
    resource_share_arn: &str,
    client_token: Option<&str>,
) -> (u16, Value) {
    let mut uri = format!(
        "/deleteresourceshare?resourceShareArn={}",
        query_escape(resource_share_arn)
    );
    if let Some(token) = client_token {
        uri.push_str(&format!("&clientToken={}", query_escape(token)));
    }
    call(router, "DELETE", &uri, None).await
}

/// Create a share that must succeed, and answer the `resourceShare` it reported.
pub async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, created) = create(router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: create expected 200, got {status} with {created:?}"
    );
    created
        .get("resourceShare")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: create reported no resourceShare, got {created:?}"))
}

/// The ARN of a reported share.
pub fn arn_of(case: &str, share: &Value) -> String {
    share
        .get("resourceShareArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: share carried no resourceShareArn, got {share:?}"))
        .to_string()
}

/// Assert `arn` is a resource-share ARN of this service in this region and account,
/// and answer its id segment.
pub fn assert_share_arn(case: &str, arn: &str) -> String {
    let prefix = format!("arn:aws:ram:{REGION}:{ACCOUNT}:resource-share/");
    let id = arn
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("case {case:?}: expected an ARN starting {prefix}, got {arn:?}"));
    // The id segment RAM uses is a UUID: 8-4-4-4-12 lowercase hex.
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8, 4, 4, 4, 12],
        "case {case:?}: expected a UUID id segment, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
        "case {case:?}: expected a lowercase-hex UUID id segment, got {id:?}"
    );
    id.to_string()
}

/// The shares a `GetResourceShares` response reported.
pub fn shares_of(case: &str, body: &Value) -> Vec<Value> {
    body.get("resourceShares")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: response carried no resourceShares, got {body:?}"))
}

/// The associations a response reported (either operation reports them the same way).
pub fn associations_of(case: &str, body: &Value) -> Vec<Value> {
    body.get("resourceShareAssociations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: response carried no resourceShareAssociations, got {body:?}")
        })
}

/// A timestamp member, as restJson1 renders it: epoch seconds.
pub fn timestamp(case: &str, value: &Value, member: &str) -> f64 {
    value
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} timestamp, got {value:?}"))
}

/// The names of the shares a response reported, sorted.
pub fn names_of(case: &str, body: &Value) -> Vec<String> {
    let mut names: Vec<String> = shares_of(case, body)
        .iter()
        .map(|share| {
            share
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("<no name>")
                .to_string()
        })
        .collect();
    names.sort();
    names
}

/// The one share a `GetResourceShares` response must have reported.
pub fn only_share(case: &str, body: &Value) -> Value {
    let shares = shares_of(case, body);
    assert_eq!(
        shares.len(),
        1,
        "case {case:?}: expected exactly one share, got {body:?}"
    );
    shares[0].clone()
}

/// The `(key, value)` pairs a reported share carries, in the order the response
/// listed them. A share with no tags must report an empty list, not an absent member.
pub fn tags_of(case: &str, share: &Value) -> Vec<(String, String)> {
    share
        .get("tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: share reported no tags member, got {share:?}"))
        .iter()
        .map(|tag| {
            (
                tag.get("key")
                    .and_then(Value::as_str)
                    .unwrap_or("<no key>")
                    .to_string(),
                tag.get("value")
                    .and_then(Value::as_str)
                    .unwrap_or("<no value>")
                    .to_string(),
            )
        })
        .collect()
}
