// SPDX-License-Identifier: CC0-1.0

//! How a Lambda function's answer BECOMES the route's response, in each payload format
//! version.
//!
//! "The payload format version specifies … how API Gateway interprets the response from
//! Lambda" (`Integration$PayloadFormatVersion`), and the two versions interpret it
//! differently:
//!
//! * `2.0` admits a STRUCTURED response — `statusCode`, `headers`, `cookies`, `body`,
//!   `isBase64Encoded` — and, for a function that returns valid JSON naming no
//!   `statusCode`, INFERS one: "if your function returns valid JSON and doesn't return a
//!   statusCode, API Gateway assumes a 200 status code, a content-type of application/json,
//!   and the function's response as the body";
//! * `1.0` admits the structured response only — the format a REST api's proxy integration
//!   uses, with `multiValueHeaders` beside `headers` — and infers nothing, so a function
//!   that answers anything else produced no response the gateway can map.
//!
//! A response the format does not admit is NOT patched into one: it is reported as the
//! "integration produced no response" class ([`Malformed`]), which the caller is answered
//! the gateway's own status for. Inventing a status or a body for it would hand the caller
//! an answer no function gave.
//!
//! Both directions of the base64 flag are the same rule as the request's: an event is a
//! JSON document, so a payload that is not text travels encoded — "isBase64Encoded: set to
//! true if the body is base64-encoded" — and what the client receives is the decoded bytes,
//! byte for byte what the function produced.

use ::base64::Engine as _;
use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use serde_json::Value;

use super::super::data_plane::JSON_CONTENT_TYPE;
use super::proxy_event::{PAYLOAD_FORMAT_1_0, PAYLOAD_FORMAT_2_0};

/// The status a `2.0` response that names none is given: "API Gateway assumes a 200 status
/// code".
const INFERRED_STATUS: u16 = 200;

/// The response header a function sets a cookie through in a `2.0` response's `cookies`
/// list (RFC 6265 §4.1): one header per cookie, which is why the list exists at all.
const SET_COOKIE_HEADER: &str = "set-cookie";

/// The response header whose value says what the body IS, defaulted when the function named
/// none.
const CONTENT_TYPE_HEADER: &str = "content-type";

/// Why a function's answer is no response this format can map. Carried to the caller's log
/// only: the client is answered the gateway's own status for an integration that produced
/// no response.
#[derive(Debug, Clone)]
pub struct Malformed {
    /// What about the answer could not be mapped.
    pub reason: String,
}

/// The response bytes and metadata one mapped answer is, before it is rendered.
#[derive(Debug, Clone)]
struct Mapped {
    status: u16,
    /// Every response header, in the order they are set; a name may repeat, which is what
    /// carries several cookies.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// The route's response for `payload`, the bytes the function returned, under
/// `payload_format`.
pub fn map(payload: &[u8], payload_format: &str) -> Result<RtResponse, Malformed> {
    let mapped = if payload_format == PAYLOAD_FORMAT_2_0 {
        mapped_2_0(payload)?
    } else {
        mapped_1_0(payload)?
    };
    render(mapped)
}

/// A `2.0` answer: the structured response when the function named a `statusCode`, and the
/// inferred one otherwise.
fn mapped_2_0(payload: &[u8]) -> Result<Mapped, Malformed> {
    let document: Value = serde_json::from_slice(payload).map_err(|error| Malformed {
        reason: format!("the function's response is not valid JSON: {error}"),
    })?;
    let names_status = document
        .as_object()
        .is_some_and(|members| members.contains_key("statusCode"));
    if !names_status {
        // The inferred response: "API Gateway assumes a 200 status code, a content-type of
        // application/json, and the function's response as the body". The body is the
        // function's own document, rendered as the JSON it is.
        return Ok(Mapped {
            status: INFERRED_STATUS,
            headers: vec![(
                CONTENT_TYPE_HEADER.to_string(),
                JSON_CONTENT_TYPE.to_string(),
            )],
            body: serde_json::to_vec(&document).unwrap_or_default(),
        });
    }
    let status = status_of(&document)?;
    let mut headers = headers_of(&document);
    for cookie in list_of(&document, "cookies") {
        headers.push((SET_COOKIE_HEADER.to_string(), cookie));
    }
    Ok(Mapped {
        status,
        headers: with_default_content_type(headers),
        body: body_of(&document)?,
    })
}

/// A `1.0` answer: the structured response only. A function that answered anything else
/// produced no response this format can map — there is no inference in `1.0`.
fn mapped_1_0(payload: &[u8]) -> Result<Mapped, Malformed> {
    let document: Value = serde_json::from_slice(payload).map_err(|error| Malformed {
        reason: format!("the function's response is not valid JSON: {error}"),
    })?;
    if !document
        .as_object()
        .is_some_and(|members| members.contains_key("statusCode"))
    {
        return Err(Malformed {
            reason: "the function's response names no statusCode, which payload format 1.0 \
                     requires of every response"
                .to_string(),
        });
    }
    let status = status_of(&document)?;
    let mut headers = headers_of(&document);
    // `multiValueHeaders` names a header that is sent more than once, which is how this
    // format carries several cookies: each value becomes its own header field.
    if let Some(multi) = document.get("multiValueHeaders").and_then(Value::as_object) {
        for (name, values) in multi {
            for value in values.as_array().cloned().unwrap_or_default() {
                if let Some(rendered) = header_value(&value) {
                    headers.push((name.to_ascii_lowercase(), rendered));
                }
            }
        }
    }
    Ok(Mapped {
        status,
        headers: with_default_content_type(headers),
        body: body_of(&document)?,
    })
}

/// The status the structured response names, refused when it is not a number inside the
/// range a response status can take (RFC 9110 §15: three digits, 100 through 599).
fn status_of(document: &Value) -> Result<u16, Malformed> {
    let named = document.get("statusCode").cloned().unwrap_or(Value::Null);
    let status = match &named {
        Value::Number(number) => number.as_i64(),
        // A function that states the status as a string states one too: `"200"` is the
        // status 200, which is what a handler written in a dynamically typed runtime
        // produces when it formats the value.
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    };
    let status = status.ok_or_else(|| Malformed {
        reason: format!("the function's response names no readable statusCode: {named}"),
    })?;
    u16::try_from(status)
        .ok()
        .filter(|status| (100..=599).contains(status))
        .ok_or_else(|| Malformed {
            reason: format!("the function's response names the status {status}, which is no \
                             HTTP response status"),
        })
}

/// The response headers the structured response names, by lowercased name. A value that is
/// not a string is rendered as the JSON scalar it is, which is what a handler setting a
/// number or a boolean means.
fn headers_of(document: &Value) -> Vec<(String, String)> {
    let Some(headers) = document.get("headers").and_then(Value::as_object) else {
        return Vec::new();
    };
    headers
        .iter()
        .filter_map(|(name, value)| {
            header_value(value).map(|value| (name.to_ascii_lowercase(), value))
        })
        .collect()
}

/// One header value as it is sent: a string as itself, a number or a boolean as its own
/// rendering, and a structured value not at all — a header field is text.
fn header_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// The string list the structured response names under `member`, or an empty list.
fn list_of(document: &Value, member: &str) -> Vec<String> {
    document
        .get(member)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The response body the structured response names: the text as it is, or the bytes it
/// decodes to when the function marked it base64-encoded. A response naming no body has an
/// empty one.
fn body_of(document: &Value) -> Result<Vec<u8>, Malformed> {
    let encoded = document.get("isBase64Encoded").and_then(Value::as_bool) == Some(true);
    let body = match document.get("body") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::String(text)) => text.clone(),
        // A body that is not a string is the function's own document; a handler that
        // returns one means its rendering, which is what AWS sends on.
        Some(other) => other.to_string(),
    };
    if !encoded {
        return Ok(body.into_bytes());
    }
    ::base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .map_err(|error| Malformed {
            reason: format!(
                "the function marked its body base64-encoded and it is not base64: {error}"
            ),
        })
}

/// `headers` with the content type a structured response that named none is given:
/// "content-type … defaults to application/json".
fn with_default_content_type(mut headers: Vec<(String, String)>) -> Vec<(String, String)> {
    if !headers
        .iter()
        .any(|(name, _)| name == CONTENT_TYPE_HEADER)
    {
        headers.push((
            CONTENT_TYPE_HEADER.to_string(),
            JSON_CONTENT_TYPE.to_string(),
        ));
    }
    headers
}

/// The mapped answer as the response the caller receives.
///
/// A header value the HTTP layer refuses is left OFF rather than failing the whole
/// response: the function's status and body are its answer, and a field that cannot be sent
/// is named in the log instead.
fn render(mapped: Mapped) -> Result<RtResponse, Malformed> {
    let status = StatusCode::try_from(mapped.status).map_err(|_| Malformed {
        reason: format!("{} is no HTTP response status", mapped.status),
    })?;
    let length = mapped.body.len().to_string();
    let mut response = RtResponse::new(status, SdkBody::from(mapped.body));
    let headers = response.headers_mut();
    for (name, value) in mapped.headers {
        if headers.try_append(name.clone(), value).is_err() {
            ::tracing::debug!(
                header = %name,
                "the function set a response header that cannot be sent, so it is left off \
                 the answer"
            );
        }
    }
    let _ = headers.try_insert("content-length", length);
    Ok(response)
}

/// Whether `payload_format` is one this mapping reads. An integration carrying any other
/// value names a format this emulator cannot interpret a response in, which the caller
/// states rather than guessing at.
pub fn is_known_format(payload_format: &str) -> bool {
    payload_format == PAYLOAD_FORMAT_1_0 || payload_format == PAYLOAD_FORMAT_2_0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What one mapped answer carries: its status, its body as text, and its headers.
    type Answered = (u16, String, Vec<(String, String)>);

    /// The status, the body and the headers one mapped answer carries.
    fn answered(payload: &str, format: &str) -> Result<Answered, String> {
        match map(payload.as_bytes(), format) {
            Ok(response) => {
                let status = response.status().as_u16();
                let headers = response
                    .headers()
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect();
                let body = response
                    .body()
                    .bytes()
                    .map(|bytes| String::from_utf8_lossy(bytes).to_string())
                    .unwrap_or_default();
                Ok((status, body, headers))
            }
            Err(malformed) => Err(malformed.reason),
        }
    }

    #[test]
    fn a_2_0_response_is_read_structured_and_otherwise_inferred() {
        // Rows differ along the SHAPE of what the function returned: the structured
        // response, a structured one naming only a status, and every valid JSON value a
        // function can return naming no status — all of which are inferred.
        let cases: [(&str, &str, u16, &str); 6] = [
            (
                "the structured response",
                r#"{"statusCode":201,"headers":{"x-made":"here"},"body":"created"}"#,
                201,
                "created",
            ),
            (
                "a structured response naming only its status",
                r#"{"statusCode":204}"#,
                204,
                "",
            ),
            (
                "a status stated as a string",
                r#"{"statusCode":"418","body":"short"}"#,
                418,
                "short",
            ),
            (
                "an object naming no status, which is inferred",
                r#"{"ok":true}"#,
                200,
                r#"{"ok":true}"#,
            ),
            ("a JSON string, which is inferred", r#""hello""#, 200, r#""hello""#),
            ("a JSON number, which is inferred", "42", 200, "42"),
        ];
        for (case, payload, expected_status, expected_body) in cases {
            let actual = answered(payload, PAYLOAD_FORMAT_2_0);
            match actual {
                Ok((status, body, _)) => {
                    assert_eq!(
                        status, expected_status,
                        "case {case:?}: expected status {expected_status}, got {status}"
                    );
                    assert_eq!(
                        body, expected_body,
                        "case {case:?}: expected body {expected_body:?}, got {body:?}"
                    );
                }
                Err(reason) => panic!("case {case:?}: expected a mapped response, got {reason}"),
            }
        }
    }

    #[test]
    fn a_1_0_response_must_be_the_structured_one() {
        // The same payloads under 1.0: only the structured ones map, because that format
        // infers nothing.
        let cases: [(&str, &str, Option<u16>); 4] = [
            (
                "the structured response",
                r#"{"statusCode":200,"body":"ok"}"#,
                Some(200),
            ),
            ("an object naming no status", r#"{"ok":true}"#, None),
            ("a JSON string", r#""hello""#, None),
            ("text that is not JSON at all", "hello", None),
        ];
        for (case, payload, expected) in cases {
            let actual = answered(payload, PAYLOAD_FORMAT_1_0);
            match (actual, expected) {
                (Ok((status, _, _)), Some(expected)) => assert_eq!(
                    status, expected,
                    "case {case:?}: expected status {expected}, got {status}"
                ),
                (Err(_), None) => {}
                (Ok((status, _, _)), None) => panic!(
                    "case {case:?}: expected no mappable response, got status {status}"
                ),
                (Err(reason), Some(expected)) => {
                    panic!("case {case:?}: expected status {expected}, got {reason}")
                }
            }
        }
    }

    #[test]
    fn every_cookie_the_function_set_becomes_its_own_header() {
        let payload = r#"{"statusCode":200,"cookies":["a=1; Path=/","b=2"],"body":""}"#;
        let (_, _, headers) = answered(payload, PAYLOAD_FORMAT_2_0).expect("it maps");
        let cookies: Vec<&String> = headers
            .iter()
            .filter(|(name, _)| name == SET_COOKIE_HEADER)
            .map(|(_, value)| value)
            .collect();
        assert_eq!(
            cookies,
            vec!["a=1; Path=/", "b=2"],
            "case {:?}: expected one set-cookie field per cookie, got {headers:?}",
            "a 2.0 response carrying two cookies"
        );
    }

    #[test]
    fn a_body_the_function_marked_encoded_reaches_the_client_as_its_bytes() {
        // The rule both ways: what the function encoded is what the client receives.
        let bytes: Vec<u8> = vec![0x00, 0x01, 0xfe, 0xff];
        let encoded = ::base64::engine::general_purpose::STANDARD.encode(&bytes);
        let payload = format!(
            r#"{{"statusCode":200,"isBase64Encoded":true,"body":"{encoded}"}}"#
        );
        let response = map(payload.as_bytes(), PAYLOAD_FORMAT_2_0).expect("it maps");
        let served = response
            .body()
            .bytes()
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        assert_eq!(
            served, bytes,
            "case {:?}: expected the decoded bytes, got {served:?}",
            "a base64-encoded response body"
        );
        // And a body marked encoded that is not base64 maps to nothing at all.
        let broken = map(
            br#"{"statusCode":200,"isBase64Encoded":true,"body":"not base64 at all"}"#,
            PAYLOAD_FORMAT_2_0,
        );
        assert!(
            broken.is_err(),
            "case {:?}: expected no mappable response",
            "a body marked encoded that is not base64"
        );
    }

    #[test]
    fn a_status_outside_the_response_range_maps_to_nothing() {
        // Rows differ along the value: each published bound of the range, one past each,
        // and a value that is no number at all.
        let cases: [(&str, &str, bool); 6] = [
            ("the lowest response status", "100", true),
            ("the highest response status", "599", true),
            ("one below the range", "99", false),
            ("one above the range", "600", false),
            ("a negative status", "-1", false),
            ("a status that is not a number", "{}", false),
        ];
        for (case, status, mappable) in cases {
            let payload = format!(r#"{{"statusCode":{status},"body":"x"}}"#);
            let actual = map(payload.as_bytes(), PAYLOAD_FORMAT_2_0).is_ok();
            assert_eq!(
                actual, mappable,
                "case {case:?}: expected mappable {mappable}, got {actual}"
            );
        }
    }
}
