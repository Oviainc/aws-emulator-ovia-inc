// SPDX-License-Identifier: CC0-1.0

//! The HTTP harness every suite in this crate drives the service through: the
//! production multi-service router with this one service mounted on an engine the
//! test also holds.
//!
//! It lives here rather than in one suite so a new suite reuses it instead of
//! copying it. Operation-specific helpers stay in the suite that needs them,
//! added to [`Emulator`] through an inherent `impl` block there.

use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_notifications_notifications::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service, which is what the router's
/// routing key is resolved from. Request authentication is off in these tests,
/// so no real signature is needed — only the scope that names `notifications`.
const CREDENTIAL_SCOPE: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/notifications/aws4_request, SignedHeaders=host, Signature=0";

/// The emulator under test: the production router with this one service mounted
/// on an engine the test also holds, so a test can read the store directly for
/// state no operation reports yet.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Send one request and return its status and parsed JSON body (`Value::Null`
    /// for an empty body).
    pub(crate) async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let payload = match &body {
            Some(value) => {
                bytes::Bytes::from(serde_json::to_vec(value).expect("a serializable body"))
            }
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "notifications.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .header("authorization", CREDENTIAL_SCOPE)
            .body(payload)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.body().clone();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|error| {
                panic!(
                    "{method} {uri} answered a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        (status, parsed)
    }
}

/// Percent-encode one URI path segment, the way an SDK encodes a non-greedy
/// `@httpLabel`: an arn's `:` and `/` must not be read as segment structure.
pub(crate) fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// The error code of an error response, from the restJson1 `__type` member.
pub(crate) fn error_code(response: &Value) -> String {
    response["__type"].as_str().unwrap_or_default().to_string()
}

/// Whether `value` matches the `NotificationConfigurationArn` pattern the model
/// declares,
/// `^arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}$`,
/// checked by hand so these suites need no regex dependency.
pub(crate) fn is_configuration_arn(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("arn:") else {
        return false;
    };
    let Some((partition, rest)) = rest.split_once(':') else {
        return false;
    };
    if !(3..=10).contains(&partition.len())
        || !partition
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'-')
    {
        return false;
    }
    let Some(rest) = rest.strip_prefix("notifications::") else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    if account.len() != 12 || !account.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some(id) = resource.strip_prefix("configuration/") else {
        return false;
    };
    is_arn_id(id)
}

/// The `category`/`sub-category` segments of a
/// `ManagedNotificationConfigurationOsArn`, from the pattern the model declares:
/// `^arn:[a-z-]{3,10}:notifications::[0-9]{12}:managed-notification-configuration/category/[a-zA-Z0-9\-]{3,64}/sub-category/[a-zA-Z0-9\-]{3,64}$`.
/// Checked by hand, like [`is_configuration_arn`] beside it, so these suites
/// need no regex dependency; `None` when `value` does not match the pattern.
pub(crate) fn managed_configuration_arn_segments(value: &str) -> Option<(String, String)> {
    let rest = value.strip_prefix("arn:")?;
    let (partition, rest) = rest.split_once(':')?;
    if !(3..=10).contains(&partition.len())
        || !partition
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'-')
    {
        return None;
    }
    let rest = rest.strip_prefix("notifications::")?;
    let (account, resource) = rest.split_once(':')?;
    if account.len() != 12 || !account.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let path = resource.strip_prefix("managed-notification-configuration/category/")?;
    let (category, sub_category) = path.split_once("/sub-category/")?;
    let segment_ok = |segment: &str| {
        (3..=64).contains(&segment.len())
            && segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if !segment_ok(category) || !segment_ok(sub_category) {
        return None;
    }
    Some((category.to_string(), sub_category.to_string()))
}

/// Whether `value` matches the `EventRuleArn` pattern the model declares,
/// `^arn:[a-z-]{3,10}:notifications::[0-9]{12}:configuration/[a-z0-9]{27}/rule/[a-z0-9]{27}$`
/// — a configuration arn with a `/rule/<id>` segment appended.
pub(crate) fn is_event_rule_arn(value: &str) -> bool {
    let Some((configuration, id)) = value.rsplit_once("/rule/") else {
        return false;
    };
    is_configuration_arn(configuration) && is_arn_id(id)
}

/// Whether `id` is an arn id segment as this service's arn patterns declare it:
/// `[a-z0-9]{27}`.
fn is_arn_id(id: &str) -> bool {
    id.len() == 27 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// Whether a rendered timestamp is a real instant: epoch seconds past 2020, or
/// an RFC 3339 string in this century. This framework renders a timestamp member
/// in either encoding depending on the schema, so both are accepted — but a zero
/// or a placeholder is not.
pub(crate) fn is_an_instant(value: &Value) -> bool {
    match value {
        Value::Number(_) => value.as_f64().is_some_and(|secs| secs > 1_600_000_000.0),
        Value::String(rendered) => {
            rendered.len() >= 20
                && rendered.starts_with("20")
                && rendered[..4].bytes().all(|b| b.is_ascii_digit())
                && rendered.contains('T')
        }
        _ => false,
    }
}
