// SPDX-License-Identifier: CC0-1.0

//! The harness every suite in this crate's test binary shares: a router holding
//! this service over a fresh engine, one awsQuery call, and just enough XML
//! reading to assert on a response.
//!
//! Declared in `tests/all.rs` beside the suites, so each suite reaches it as
//! `crate::harness`.

use service_com_amazonaws_ses_simpleemailservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The API version SES's query API carries on every request.
pub const VERSION: &str = "2010-12-01";

/// How the router resolves which service a request is for: the 4th field of the
/// SigV4 credential scope is the signing name, and this service's is `ses`
/// (`resolve_route_key`). Request authentication is off unless the process turns
/// it on, so the signature itself is never checked here.
pub const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/ses/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// One form field of a query request.
pub type Field = (String, String);

/// A form field, from anything stringy.
pub fn field(name: impl Into<String>, value: impl Into<String>) -> Field {
    (name.into(), value.into())
}

/// A router holding only this service, over a fresh engine — the same
/// construction `router/src/lib.rs` performs for every service.
pub fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
}

/// A router holding only this service, over an engine the CALLER holds, so a
/// suite can read or seed this service's store ([`state::service_state`]) beside
/// the calls it makes over the wire.
///
/// [`state::service_state`]: service_com_amazonaws_ses_simpleemailservice::state::service_state
pub fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// One awsQuery call: `(status, body)`.
pub async fn call(router: &MultiServiceRouter, action: &str, params: &[Field]) -> (u16, String) {
    let mut body = format!("Action={action}&Version={VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("the request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).into_owned())
}

/// Percent-encode a form field, leaving only the RFC 3986 unreserved set.
pub fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(*byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Index just past the opening `<tag>` — or `<tag xmlns="…">`, which is how the
/// codec writes the first element of a nested struct.
fn past_open_tag(xml: &str, tag: &str) -> Option<usize> {
    let needle = format!("<{tag}");
    let mut from = 0;
    while let Some(at) = xml[from..].find(&needle) {
        let at = from + at;
        let after = at + needle.len();
        match xml[after..].chars().next() {
            Some('>') => return Some(after + 1),
            Some(c) if c.is_whitespace() => return Some(after + xml[after..].find('>')? + 1),
            // `<Identities…` is not `<Identity…`: keep looking.
            _ => from = after,
        }
    }
    None
}

/// The inner XML of every `<tag>…</tag>` in `xml`, in document order.
pub fn elements(xml: &str, tag: &str) -> Vec<String> {
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = past_open_tag(rest, tag) {
        let after = &rest[start..];
        let Some(end) = after.find(&close) else { break };
        found.push(after[..end].to_string());
        rest = &after[end + close.len()..];
    }
    found
}

/// The inner XML of the first `<tag>…</tag>` in `xml`.
pub fn element(xml: &str, tag: &str) -> Option<String> {
    elements(xml, tag).into_iter().next()
}

/// The `<entry><key>…</key><value>…</value></entry>` pairs of an awsQuery map,
/// as `(key, the value element's inner XML)`.
pub fn map_entries(xml: &str) -> Vec<(String, String)> {
    elements(xml, "entry")
        .into_iter()
        .map(|entry| {
            (
                element(&entry, "key").unwrap_or_default(),
                element(&entry, "value").unwrap_or_default(),
            )
        })
        .collect()
}
