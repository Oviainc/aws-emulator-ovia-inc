// SPDX-License-Identifier: CC0-1.0

//! Helpers shared by this crate's test suites.
//!
//! One copy of the pieces every suite here needs: the engine and router every
//! test drives the service through, the awsQuery POST, and the two readers over
//! an answered XML body. A suite keeps only what is its own.

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;

/// The region every suite signs for, so the ARNs the service stamps are checked
/// against the region the request names rather than a default.
pub const REGION: &str = "us-west-2";

/// Percent-encode one form value: RFC 3986's unreserved set (the alphanumerics
/// plus `-`, `.`, `_` and `~`) is all that may stay literal in an
/// `application/x-www-form-urlencoded` body.
///
/// THE ONE COPY in this crate's tests: every suite posts through [`query`] below,
/// so the encoder is defined here and nowhere else. Written out rather than taken
/// from `percent_encoding` for one reason — a dev-dependency rewrites
/// `Cargo.lock`, the file nearly every other change in this repository also
/// rewrites, and that lock conflict is what repeatedly blocked this fix from
/// being merged. It is RFC 3986's unreserved set, exercised by every request
/// these suites make.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub fn new_engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

pub fn router_with(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(autoscaling::EmulatedService::new(engine));
    router
}

/// POST one awsQuery action and return its status and body.
pub async fn query(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2011-01-01");
    for (key, value) in params {
        body.push('&');
        body.push_str(key);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://autoscaling.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/autoscaling/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// The text of every `<tag>…</tag>` in document order.
pub fn tag_values(xml: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        out.push(after[..end].to_string());
        rest = &after[end + close.len()..];
    }
    out
}

/// The text of the first `<tag>…</tag>`, or `""` when the member is absent.
pub fn tag_value(xml: &str, tag: &str) -> String {
    tag_values(xml, tag).first().cloned().unwrap_or_default()
}
