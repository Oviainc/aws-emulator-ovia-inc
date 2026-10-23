// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this target drives the emulator through.
//!
//! One engine, one mounted service, one router: an `http::Request` is dispatched to the
//! mounted service, so the restJson1 routing, the boundary constraint validation, the
//! handler and the response serialization all run exactly as they do in the emulator
//! binary. Nothing here reaches into `crate::state` directly — a write is only believed
//! when a later `Get`/`List` call reports it back.

use std::sync::Arc;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the tests sign for; it is the region every expected ARN carries.
pub const REGION: &str = "us-east-1";

/// The SigV4 access key id the tests sign with unless a suite names another; it is the
/// identity a handler reports or records for the call.
pub const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// One response, reduced to what the assertions read.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    /// The wire error code: restJson1 resolves an error shape from
    /// `x-amzn-errortype`, falling back to the body's `__type`.
    pub error_code: Option<String>,
    pub body: Value,
    /// How many bytes of body the response carried, so a suite can tell a response
    /// with no body at all from one carrying an empty JSON object.
    pub body_bytes: usize,
}

impl Reply {
    pub fn code(&self) -> &str {
        self.error_code.as_deref().unwrap_or("")
    }

    /// A member of the response body, treating JSON `null` as absent the way an
    /// omitted optional member is.
    pub fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    /// Whether the response reported NO content: no body at all, or a body carrying no
    /// members. This is what a 204 answer must satisfy — there is nothing for a client
    /// to parse either way.
    pub fn carries_no_content(&self) -> bool {
        match &self.body {
            Value::Null => self.body_bytes == 0,
            Value::Object(members) => members.is_empty(),
            _ => false,
        }
    }
}

/// The emulator under test: one engine, one mounted service, one router.
pub struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    pub async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        self.send_as(ACCESS_KEY_ID, method, path, body).await
    }

    /// The same call, signed with a named access key id.
    ///
    /// A suite that asserts on what a handler reports or records about the CALLER needs
    /// its own identity, so its records can be told apart from the ones a sibling suite
    /// writes in parallel.
    pub async fn send_as(
        &self,
        access_key_id: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = match body {
            Some(value) => serde_json::to_vec(&value).expect("the request body serializes"),
            None => Vec::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://access-analyzer.{REGION}.amazonaws.com{path}"))
            // The SigV4 credential scope is what routes the request to this service
            // and what names the region its ARNs carry. Signatures are not verified
            // unless the process turns authentication on.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/access-analyzer/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error_code = error_code.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        Reply {
            status,
            error_code,
            body,
            body_bytes: response.body().len(),
        }
    }

    pub async fn create_analyzer(&self, body: Value) -> Reply {
        self.send("PUT", "/analyzer", Some(body)).await
    }

    pub async fn get_analyzer(&self, name: &str) -> Reply {
        self.send("GET", &format!("/analyzer/{name}"), None).await
    }

    pub async fn update_analyzer(&self, name: &str, body: Value) -> Reply {
        self.send("PUT", &format!("/analyzer/{name}"), Some(body))
            .await
    }

    pub async fn delete_analyzer(&self, name: &str) -> Reply {
        self.send("DELETE", &format!("/analyzer/{name}"), None).await
    }

    pub async fn list_analyzers(&self, query: &str) -> Reply {
        self.send("GET", &format!("/analyzer{query}"), None).await
    }

    pub async fn create_archive_rule(&self, analyzer: &str, body: Value) -> Reply {
        self.send(
            "PUT",
            &format!("/analyzer/{analyzer}/archive-rule"),
            Some(body),
        )
        .await
    }

    pub async fn get_archive_rule(&self, analyzer: &str, rule: &str) -> Reply {
        self.send(
            "GET",
            &format!("/analyzer/{analyzer}/archive-rule/{rule}"),
            None,
        )
        .await
    }

    pub async fn update_archive_rule(&self, analyzer: &str, rule: &str, body: Value) -> Reply {
        self.send(
            "PUT",
            &format!("/analyzer/{analyzer}/archive-rule/{rule}"),
            Some(body),
        )
        .await
    }

    pub async fn list_archive_rules(&self, analyzer: &str, query: &str) -> Reply {
        self.send(
            "GET",
            &format!("/analyzer/{analyzer}/archive-rule{query}"),
            None,
        )
        .await
    }

    pub async fn delete_archive_rule(&self, analyzer: &str, rule: &str) -> Reply {
        self.send(
            "DELETE",
            &format!("/analyzer/{analyzer}/archive-rule/{rule}"),
            None,
        )
        .await
    }

    pub async fn list_policy_generations(&self, query: &str) -> Reply {
        self.send("GET", &format!("/policy/generation{query}"), None)
            .await
    }

    /// `POST /resource/scan` with the given request body.
    pub async fn start_resource_scan(&self, body: Value) -> Reply {
        self.send("POST", "/resource/scan", Some(body)).await
    }

    /// `POST /tags/{resourceArn}` with the given tag map.
    pub async fn tag_resource(&self, resource_arn: &str, tags: &Value) -> Reply {
        self.send(
            "POST",
            &format!("/tags/{}", label(resource_arn)),
            Some(json!({ "tags": tags })),
        )
        .await
    }

    /// `GET /tags/{resourceArn}`.
    pub async fn list_tags_for_resource(&self, resource_arn: &str) -> Reply {
        self.send("GET", &format!("/tags/{}", label(resource_arn)), None)
            .await
    }

    /// `DELETE /tags/{resourceArn}?tagKeys=…`, one query entry per key (`tagKeys` is an
    /// `@httpQuery` list).
    pub async fn untag_resource(&self, resource_arn: &str, tag_keys: &[&str]) -> Reply {
        let query = tag_keys
            .iter()
            .map(|key| format!("tagKeys={}", label(key)))
            .collect::<Vec<_>>()
            .join("&");
        self.send(
            "DELETE",
            &format!("/tags/{}?{query}", label(resource_arn)),
            None,
        )
        .await
    }
}

/// A value encoded for an `@httpLabel` or an `@httpQuery` entry the way an SDK encodes
/// one: every character outside the unreserved set is percent-escaped, so an ARN's own
/// `:` and `/` do not read as path structure.
pub fn label(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// Every `ValidationExceptionReason` this service's model declares, as the pair the model
/// itself declares: the lowerCamel discriminator (`value`) and the SCREAMING_SNAKE name
/// (`name`) a response spells it with.
///
/// Copied from the model's own enum — `smithy-model/…accessanalyzer.json`, shape
/// `ValidationExceptionReason`, which declares
/// `{"value":"unknownOperation","name":"UNKNOWN_OPERATION"}`,
/// `{"value":"cannotParse","name":"CANNOT_PARSE"}`,
/// `{"value":"fieldValidationFailed","name":"FIELD_VALIDATION_FAILED"}`,
/// `{"value":"other","name":"OTHER"}` and
/// `{"value":"notSupported","name":"NOT_SUPPORTED"}` (the `values` list
/// `smithy_query(query="shape",shape="ValidationExceptionReason")` reports is the `value`
/// half of those pairs). AWS serializes the `name`: a recorded refusal carries
/// `"reason":"FIELD_VALIDATION_FAILED"`.
pub const VALIDATION_REASONS: &[(&str, &str)] = &[
    ("unknownOperation", "UNKNOWN_OPERATION"),
    ("cannotParse", "CANNOT_PARSE"),
    ("fieldValidationFailed", "FIELD_VALIDATION_FAILED"),
    ("other", "OTHER"),
    ("notSupported", "NOT_SUPPORTED"),
];

/// The spelling a response carries for the modelled `discriminator`, from
/// [`VALIDATION_REASONS`].
///
/// Panics on a discriminator the model does not declare, so a test can never assert
/// against a reason this service has no business raising.
pub fn wire_reason(discriminator: &str) -> &'static str {
    VALIDATION_REASONS
        .iter()
        .find(|(modelled, _)| *modelled == discriminator)
        .map(|(_, wire)| *wire)
        .unwrap_or_else(|| {
            panic!("{discriminator:?} is not a modelled ValidationExceptionReason")
        })
}

/// The ARN an analyzer of `name` must carry:
/// `arn:aws:access-analyzer:<region>:<account>:analyzer/<name>`, with the account
/// segment taken from the core's single source of truth.
pub fn expected_arn(name: &str) -> String {
    format!(
        "arn:aws:access-analyzer:{REGION}:{}:analyzer/{name}",
        engine::emulator_account_id()
    )
}

/// Whether a `Timestamp` member carries an instant from this run's clock.
///
/// The instant itself is generated per call, so only its shape and plausibility are
/// asserted. Both encodings are accepted — so the assertion pins the handler's
/// behaviour rather than the codec's choice — and both apply the same lower bound of
/// 2020-09-13 (epoch 1_600_000_000), which is what rules out a zero/default stand-in:
/// the framework's JSON codec renders a timestamp as epoch seconds, while an RFC3339
/// rendering must carry a four-digit year of 2020 or later.
pub fn is_timestamp(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Number(seconds)) => seconds.as_f64().is_some_and(|s| s > 1_600_000_000.0),
        Some(Value::String(text)) => {
            let year = text
                .get(..4)
                .and_then(|year| year.parse::<u32>().ok())
                .unwrap_or(0);
            year >= 2020 && text.contains('T') && text.ends_with('Z')
        }
        _ => false,
    }
}

/// Whether `later` is at or after `earlier`, for two `Timestamp` members read off the
/// wire.
///
/// Both encodings the codec may choose are ordered: epoch seconds numerically, an
/// RFC3339 rendering by its whole-second text (whose fields run most- to
/// least-significant at a fixed `Z` offset) and then by its fractional second as a
/// NUMBER. Comparing an RFC3339 rendering as plain text is not sound — the codec trims
/// trailing zeros from the fraction, so a whole second (`…:00Z`) sorts after the same
/// second plus a fraction (`…:00.5Z`) although it is earlier. A pair of differently
/// encoded values is not comparable, so it is reported as not ordered and the assertion
/// that reads this fails loudly.
pub fn timestamp_at_least(later: Option<&Value>, earlier: Option<&Value>) -> bool {
    match (later, earlier) {
        (Some(Value::Number(later)), Some(Value::Number(earlier))) => {
            match (later.as_f64(), earlier.as_f64()) {
                (Some(later), Some(earlier)) => later >= earlier,
                _ => false,
            }
        }
        (Some(Value::String(later)), Some(Value::String(earlier))) => {
            match (rfc3339_parts(later), rfc3339_parts(earlier)) {
                (Some((later_second, later_fraction)), Some((earlier_second, earlier_fraction))) => {
                    later_second > earlier_second
                        || (later_second == earlier_second && later_fraction >= earlier_fraction)
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// An RFC3339 instant split into the part that orders lexicographically (everything up to
/// the seconds field) and the fractional second, which does not.
fn rfc3339_parts(text: &str) -> Option<(&str, f64)> {
    let body = text.strip_suffix('Z')?;
    match body.split_once('.') {
        Some((whole, fraction)) => {
            let fraction: f64 = format!("0.{fraction}").parse().ok()?;
            Some((whole, fraction))
        }
        None => Some((body, 0.0)),
    }
}

/// The ordering [`timestamp_at_least`] implements, over both renderings the codec may
/// choose and over the boundary the trimmed fraction creates: a whole second is EARLIER
/// than the same second plus any fraction, though its text sorts after it.
#[test]
fn timestamps_are_ordered_whichever_way_they_are_rendered() {
    let cases: &[(&str, Value, Value, bool)] = &[
        (
            "epoch seconds, later first",
            Value::from(1_700_000_001.0),
            Value::from(1_700_000_000.0),
            true,
        ),
        (
            "epoch seconds, earlier first",
            Value::from(1_700_000_000.0),
            Value::from(1_700_000_001.0),
            false,
        ),
        (
            "RFC3339 whole seconds apart",
            Value::from("2026-01-01T00:00:01Z"),
            Value::from("2026-01-01T00:00:00Z"),
            true,
        ),
        (
            "RFC3339 fraction after a whole second, same second",
            Value::from("2026-01-01T00:00:00.5Z"),
            Value::from("2026-01-01T00:00:00Z"),
            true,
        ),
        (
            "RFC3339 whole second before a fraction of it",
            Value::from("2026-01-01T00:00:00Z"),
            Value::from("2026-01-01T00:00:00.5Z"),
            false,
        ),
        (
            "RFC3339 fractions of unequal length",
            Value::from("2026-01-01T00:00:00.5Z"),
            Value::from("2026-01-01T00:00:00.123456Z"),
            true,
        ),
        (
            "RFC3339 equal instants",
            Value::from("2026-01-01T00:00:00.25Z"),
            Value::from("2026-01-01T00:00:00.25Z"),
            true,
        ),
        (
            "mixed renderings are not comparable",
            Value::from("2026-01-01T00:00:00Z"),
            Value::from(1_700_000_000.0),
            false,
        ),
    ];
    for (case, later, earlier, expected) in cases {
        assert_eq!(
            timestamp_at_least(Some(later), Some(earlier)),
            *expected,
            "case {case:?}: expected {expected} for {later:?} at or after {earlier:?}"
        );
    }
}

/// Every analyzer name a listing reports, in the order it reports them.
pub fn listed_names(reply: &Reply) -> Vec<String> {
    reply
        .member("analyzers")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every rule name a listing reports, in the order it reports them.
pub fn listed_rule_names(reply: &Reply) -> Vec<String> {
    reply
        .member("archiveRules")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("ruleName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}
