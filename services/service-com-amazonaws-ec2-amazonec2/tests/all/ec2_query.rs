// SPDX-License-Identifier: CC0-1.0

//! The ec2Query test harness every suite in this crate drives the service through.
//!
//! EC2 speaks `ec2Query`: an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//! The envelope is selected by the `ec2` signing name in the request's SigV4 credential
//! scope, and the Region in that scope is the Region the service answers for — so every
//! call here carries one.
//!
//! This module holds only the plumbing: building the form body, dispatching it through
//! the same `MultiServiceRouter` the emulator serves with, and reading elements back out
//! of the XML. What each operation asserts lives in that operation's own suite.

use std::sync::Arc;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

/// The EC2 API version a client sends in an ec2Query body.
pub const API_VERSION: &str = "2016-11-15";

/// The emulator the tests drive: one shared engine with the EC2 service mounted on it,
/// request authentication off (these tests sign nothing — they only carry the
/// `Credential=` scope the envelope and the Region are read from). The engine is handed
/// back too, so a suite can seed the store a handler reads.
pub fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Percent-encode the characters that are significant in an
/// `application/x-www-form-urlencoded` body, leaving everything else as a client's own
/// encoder would (`:` and `/` in a CIDR block).
fn form_value(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '&' => "%26".to_string(),
            '=' => "%3D".to_string(),
            '+' => "%2B".to_string(),
            '%' => "%25".to_string(),
            ' ' => "%20".to_string(),
            '\\' => "%5C".to_string(),
            // Historically accepted as a field separator beside `&`, so a value carrying
            // one is encoded rather than left to the parser's reading of it.
            ';' => "%3B".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// One `action` call signed for `region`, with `params` as extra form fields. Answers the
/// HTTP status and the response body.
pub async fn call(
    router: &MultiServiceRouter,
    region: &str,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_value(value));
    }
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{region}/ec2/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ec2.{region}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body))
        .expect("a well-formed ec2Query request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// `ZoneName.N` / `VpcId.N` / `Filter.N.Name` form fields, as ec2Query flattens a list
/// into indexed keys.
pub fn indexed(prefix: &str, values: &[&str]) -> Vec<(String, String)> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| (format!("{prefix}.{}", index + 1), (*value).to_string()))
        .collect()
}

/// The form fields for one `Filters` entry at position `index` (counting from 1).
fn filter_fields(index: usize, name: &str, values: &[&str]) -> Vec<(String, String)> {
    let mut fields = vec![(format!("Filter.{index}.Name"), name.to_string())];
    fields.extend(indexed(&format!("Filter.{index}.Value"), values));
    fields
}

/// The form fields for a whole `Filters` list.
pub fn filters(entries: &[(&str, &[&str])]) -> Vec<(String, String)> {
    entries
        .iter()
        .enumerate()
        .flat_map(|(index, (name, values))| filter_fields(index + 1, name, values))
        .collect()
}

/// The text of the first `<name>…</name>` element in `xml`.
pub fn element(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].to_string())
}

/// The text of every `<name>…</name>` element in `xml`, in document order.
pub fn elements(xml: &str, name: &str) -> Vec<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        match rest.find(&close) {
            Some(end) => {
                found.push(rest[..end].to_string());
                rest = &rest[end + close.len()..];
            }
            None => break,
        }
    }
    found
}

/// The `<Code>` of a failure body — EC2's error envelope is
/// `<Response><Errors><Error><Code/><Message/></Error></Errors></Response>`.
pub fn error_code(xml: &str) -> String {
    element(xml, "Code").unwrap_or_default()
}

/// XML text as the service wrote it, with the five entities resolved — an error message
/// quoting a filter name reaches the wire as `&apos;…&apos;`.
pub fn unescaped(xml_text: &str) -> String {
    xml_text
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}
