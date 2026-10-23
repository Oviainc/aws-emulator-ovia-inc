// SPDX-License-Identifier: CC0-1.0

//! The ONE wire harness this crate's suites drive the emulator through.
//!
//! Every suite in `tests/all/` dispatches real, signed restJson1 requests at a
//! `MultiServiceRouter` with the RUM service mounted, so routing, deserialization, the
//! modelled constraints, the handler and the response envelope are all exercised rather
//! than just a handler body. The pieces that are the same for every suite — the SigV4
//! credential scope, the emulator, the raw `Reply`, the dispatch helpers and the two
//! calls (`CreateAppMonitor`, `GetAppMonitor`) a suite needs to set up or read back a
//! resource — live here, so a change to the wire path is made once instead of per suite.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The region every case signs for unless it is testing the region itself.
pub(crate) const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`rum`, per `OPERATIONS.md`'s `## Wire protocol`), which is what the router's
/// `can_handle` routes on, and the region the ARN is derived from. Request
/// authentication is switched off below, so only the scope's shape matters.
pub(crate) fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/rum/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One emulator: a shared engine, the RUM service, and the router that dispatches to
/// it. Authentication is off so a test needs no signing key. The engine is handed back
/// as well, so a case can read the persisted record for the members no implemented
/// operation reports yet (the event buffer `PutRumEvents` appends to).
///
/// The CloudWatch Logs service is mounted beside RUM because a request that turns
/// `CwLogEnabled` on provisions the vended log group by calling it in process, exactly as
/// the assembled emulator does (`core/router`'s `build_router` registers every service).
pub(crate) fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_rum_rum::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// One HTTP answer, kept as raw bytes so a case can assert on an EMPTY body as well as
/// on JSON, and with the response headers so a case can assert on the envelope (the
/// media type the service announces) rather than only on the payload.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: Bytes,
    pub(crate) headers: http::HeaderMap,
}

impl Reply {
    /// The media type this answer announces, as the client reads it.
    pub(crate) fn content_type(&self) -> Option<&str> {
        self.headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
    }

    pub(crate) fn json(&self, case: &str) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "case {case:?}: expected a JSON body, got {:?} ({e})",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code: restJson1 carries it as the body's `__type`.
    pub(crate) fn error_code(&self, case: &str) -> String {
        self.json(case)["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: error body has no __type: {:?}", self.body))
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let body_bytes = match body {
        Some(value) => Bytes::from(serde_json::to_vec(&value).expect("serializable request body")),
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://rum.{region}.amazonaws.com{path_and_query}"))
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(body_bytes)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    Reply {
        status,
        body: response.into_body(),
        headers,
    }
}

pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    call_in(router, REGION, method, path_and_query, body).await
}

/// `AppMonitorId`'s modelled pattern: 36 characters,
/// `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`.
/// Source: `smithy_query(query="shape", shape="AppMonitor")`, member `Id`.
///
/// Also checks the two nibbles a version-4, RFC 4122 UUID fixes — the version digit and
/// the variant digit — since that is the form the service mints.
pub(crate) fn is_modelled_app_monitor_id(id: &str) -> bool {
    let shaped = id.len() == 36
        && id.split('-').map(str::len).eq([8, 4, 4, 4, 12])
        && id.chars().all(|c| c == '-' || c.is_ascii_hexdigit());
    let version = id.chars().nth(14) == Some('4');
    let variant = matches!(id.chars().nth(19), Some('8' | '9' | 'a' | 'b'));
    shaped && version && variant
}

/// The `message` of an error body, under either spelling the member may arrive as.
///
/// ONE definition for every suite: a restJson1 error body carries the modelled `message`
/// member, but an SDK and some AWS frontends spell it `Message`, so a suite that pinned
/// only one spelling would pass or fail on the casing rather than on the text. Takes the
/// parsed body rather than a `Reply`, so the suites that still own their own reply type
/// can use it too.
pub(crate) fn message(document: &Value) -> Option<&str> {
    document["message"]
        .as_str()
        .or_else(|| document["Message"].as_str())
}

/// A `PutRumEvents` body carrying two events of different types, for the monitor whose
/// id is `id`.
///
/// Shared, because more than one suite needs a well-formed batch: the lifecycle suite to
/// show the events land on the monitor the id names, and the error-body suite to reach
/// the not-found refusal for an id that names none.
pub(crate) fn rum_events_body(id: &str) -> Value {
    json!({
        "BatchId": "11111111-2222-4333-8444-555555555555",
        "AppMonitorDetails": {"name": "any", "id": id, "version": "1.0.0"},
        "UserDetails": {
            "userId": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            "sessionId": "ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb",
        },
        "RumEvents": [
            {
                "id": "12345678-1234-4234-8234-123456789012",
                "timestamp": 1_704_067_200,
                "type": "com.amazon.rum.page_view_event",
                "metadata": "{\"version\":\"1.0.0\"}",
                "details": "{\"pageId\":\"/home\"}",
            },
            {
                "id": "22345678-1234-4234-8234-123456789012",
                "timestamp": 1_704_067_260,
                "type": "com.amazon.rum.js_error_event",
                "details": "{\"message\":\"boom\"}",
            },
        ],
    })
}

/// The `Tags` of a reported shape, with an omitted member read as the empty map it
/// stands for: restJson1 draws no distinction between an absent map member and an empty
/// one.
pub(crate) fn tag_map(reported: &Value) -> Value {
    match &reported["Tags"] {
        Value::Null => json!({}),
        tags => tags.clone(),
    }
}

/// Percent-encode one URI path segment or query value: every byte outside RFC 3986's
/// unreserved set is escaped.
///
/// A `ResourceArn` is a non-greedy `@httpLabel`, so an SDK encodes it into ONE path
/// segment — its `:` and `/` included — and a tag key travels the same way in a
/// `@httpQuery` value (so a `+` in a key is not read as a space). An Evidently experiment
/// ARN in `?destinationArn=` and an `application/x-www-form-urlencoded` component (a
/// policy document's braces and quotes) are escaped the same way. This is the one copy
/// every suite in this crate uses, including the ones that bring their own router and
/// `call`.
pub(crate) fn percent_encoded(raw: &str) -> String {
    raw.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Create one app monitor, returning the `Id` the call minted.
pub(crate) async fn create(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let reply = call(router, "POST", "/appmonitor", Some(body)).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected CreateAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let id = reply.json(case)["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateAppMonitor returned no Id"))
        .to_string();
    assert!(
        is_modelled_app_monitor_id(&id),
        "case {case:?}: expected Id to match AppMonitorId's 36-character UUID pattern, got {id:?}"
    );
    id
}

/// The `AppMonitor` structure `GetAppMonitor` reports for `name`.
pub(crate) async fn get_app_monitor(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let reply = call(router, "GET", &format!("/appmonitor/{name}"), None).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected GetAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json(case)["AppMonitor"].clone()
}
