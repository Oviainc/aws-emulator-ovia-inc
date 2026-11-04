// SPDX-License-Identifier: CC0-1.0

//! The error contract of every `RotateTunnelAccessToken` refusal, asserted as one
//! table rather than per request.
//!
//! RULE: each refusal answers the error AWS answers for ITS class, and the two
//! classes are not interchangeable:
//!
//! * a tunnel the addressed Region's endpoint does not hold — never created,
//!   deleted, or held by another Region — is the operation's MODELLED error,
//!   `ResourceNotFoundException` (the one error
//!   `smithy_query(query="operation", operation="RotateTunnelAccessToken")`
//!   declares), carrying the service's single not-found sentence;
//! * a refusal of a request the service accepts the SHAPE of — the resolved
//!   tunnel is no longer `OPEN`, a `destinationConfig` the requested
//!   `clientMode` does not cover, a `services` list longer than a tunnel is
//!   multiplexed over — is the request-time refusal AWS answers for this
//!   operation: the wire error code `ValidationException`, a code this service's
//!   model declares nowhere, because AWS's refusal here is not one of the errors
//!   the operation lists either.
//!
//! Both classes arrive with this service's client-error status, 400: it speaks
//! `awsJson1_1`, where the model's `@httpError` is inert, so the class a client
//! branches on travels in the body's `__type` (and `x-amzn-errortype`), never in
//! the status line. What must stay distinguishable is therefore the wire code: an
//! SDK resolves `ResourceNotFoundException` into the generated error type the
//! operation declares, and a code that is not that one keeps a retry or error
//! classifier from treating a live-but-closed tunnel as a missing one. So the
//! table below pins the status, the wire code and the message of every refusal
//! this operation can produce, and asserts that no two classes collapse onto the
//! same answer.
//!
//! Driven over the wire through the `MultiServiceRouter` exactly as a client
//! reaches the service (`awsJson1_1`, `POST /` with `X-Amz-Target`).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::state::MAX_SERVICES_PER_TUNNEL;
use service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::{
    ComAmazonawsIotsecuretunnelingIotsecuredtunnelingState as TunnelState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test signs for.
const REGION: &str = "eu-west-1";

/// `ClientMode`'s modelled values, from
/// `smithy_query(query="shape", shape="com.amazonaws.iotsecuretunneling#ClientMode")`:
/// `["SOURCE", "DESTINATION", "ALL"]`.
const CLIENT_MODES: [&str; 3] = ["SOURCE", "DESTINATION", "ALL"];

/// A router carrying only this service, over the engine the test also inspects.
fn mount(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Call one operation the way an SDK does and return `(status, body)`.
async fn call(router: &MultiServiceRouter, operation: &str, input: &Value) -> (u16, Value) {
    let body = serde_json::to_vec(input).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("IoTSecuredTunneling.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/IoTSecuredTunneling/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code (`__type` on awsJson) of a refusal body.
fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or_default()
}

/// The `message` member of a refusal body.
fn message(body: &Value) -> &str {
    body["message"].as_str().unwrap_or_default()
}

/// How a refusal must arrive on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Refusal {
    status: u16,
    code: &'static str,
}

/// The tunnel is not a resource this endpoint holds: the operation's own
/// modelled error, at this service's client-error status.
const MODELLED_NOT_FOUND: Refusal = Refusal {
    status: 400,
    code: "ResourceNotFoundException",
};

/// The request names a tunnel this endpoint holds but asks for something the
/// service refuses at request time: the refusal code AWS answers for this
/// operation, which its model declares nowhere.
const REQUEST_TIME_REFUSAL: Refusal = Refusal {
    status: 400,
    code: "ValidationException",
};

/// What the refusal's `message` must say.
#[derive(Debug, Clone)]
enum ExpectedMessage {
    /// The whole sentence, for a refusal whose wording is fixed text plus values
    /// taken from the request.
    Exact(String),
    /// Every fragment the text must carry, for a refusal whose wording also
    /// reports a count.
    Fragments(Vec<String>),
}

impl ExpectedMessage {
    /// Whether `actual` satisfies this expectation.
    fn is_satisfied_by(&self, actual: &str) -> bool {
        match self {
            Self::Exact(expected) => actual == expected,
            Self::Fragments(fragments) => {
                fragments.iter().all(|fragment| actual.contains(fragment))
            }
        }
    }
}

/// One row: a `RotateTunnelAccessToken` request and the refusal it must answer.
#[derive(Debug)]
struct Row {
    case: String,
    /// The tunnel the request names, when the store holds one — so the row can
    /// also assert the refusal minted and stored nothing.
    tunnel_id: Option<String>,
    request: Value,
    expected: Refusal,
    message: ExpectedMessage,
}

/// Open one tunnel and return its `tunnelId`. `minutes` is the
/// `maxLifetimeTimeoutMinutes` to configure, or `None` for the modelled default.
async fn open_tunnel(router: &MultiServiceRouter, case: &str, minutes: Option<i64>) -> String {
    let mut input = json!({
        "description": case,
        "destinationConfig": {"thingName": "thing-contract", "services": ["SSH"]},
    });
    if let Some(minutes) = minutes {
        input["timeoutConfig"] = json!({"maxLifetimeTimeoutMinutes": minutes});
    }
    let (status, opened) = call(router, "OpenTunnel", &input).await;
    assert_eq!(status, 200, "case {case:?}: OpenTunnel answered {opened:?}");
    opened["tunnelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no tunnelId in {opened:?}"))
        .to_string()
}

/// The stored access-token pair of `tunnel_id`.
fn stored_tokens(engine: &Arc<engine::InMemoryEngine>, tunnel_id: &str) -> (String, String) {
    engine
        .state::<TunnelState>()
        .lock()
        .tunnel(REGION, tunnel_id)
        .map(|tunnel| {
            (
                tunnel.source_access_token.clone(),
                tunnel.destination_access_token.clone(),
            )
        })
        .unwrap_or_else(|| panic!("tunnel {tunnel_id} is not in the store"))
}

/// How a tunnel stops being `OPEN` while its record stays readable.
#[derive(Debug, Clone, Copy)]
enum HowItClosed {
    /// `CloseTunnel` with the `delete` member absent.
    CloseDeleteOmitted,
    /// `CloseTunnel` with `delete=false`.
    CloseDeleteFalse,
    /// The expiry the clock owes once a lifetime of `minutes` has run out.
    LifetimeElapsed { minutes: i64 },
}

/// Bring `tunnel_id` to `CLOSED` the way `how` says, and assert it got there.
async fn close_tunnel(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    tunnel_id: &str,
    how: HowItClosed,
    case: &str,
) {
    match how {
        HowItClosed::CloseDeleteOmitted | HowItClosed::CloseDeleteFalse => {
            let mut input = json!({"tunnelId": tunnel_id});
            if matches!(how, HowItClosed::CloseDeleteFalse) {
                input["delete"] = Value::Bool(false);
            }
            let (status, closed) = call(router, "CloseTunnel", &input).await;
            assert_eq!(status, 200, "case {case:?}: CloseTunnel answered {closed:?}");
        }
        HowItClosed::LifetimeElapsed { minutes } => {
            // Move the whole timeline of the record into the past, so its
            // lifetime ran out a second ago exactly as the clock would have it.
            let shift = Duration::from_secs(u64::try_from(minutes).unwrap_or(1) * 60 + 1);
            let state = engine.state::<TunnelState>();
            let mut store = state.lock();
            let tunnel = store
                .tunnel_mut(REGION, tunnel_id)
                .unwrap_or_else(|| panic!("case {case:?}: the opened tunnel was not persisted"));
            tunnel.created_at -= shift;
            tunnel.last_updated_at -= shift;
            tunnel.expires_at -= shift;
            let expires_at = tunnel.expires_at;
            store.note_open_expiry(expires_at);
        }
    }

    let (status, described) = call(router, "DescribeTunnel", &json!({"tunnelId": tunnel_id})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the record must stay readable, DescribeTunnel answered {described:?}"
    );
    assert_eq!(
        described["tunnel"]["status"].as_str(),
        Some("CLOSED"),
        "case {case:?}: status before the rotate was {:?}",
        described["tunnel"]["status"]
    );
}

/// Every refusal class of the operation, as one table.
async fn refusal_rows(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // CLASS 1 — the endpoint holds no such tunnel. Ids satisfying `TunnelId`'s
    // modelled pattern `^[a-zA-Z0-9_\-+=:]{1,128}$`: the UUID form AWS mints, a
    // name-like id, one using every character class the pattern admits, the
    // shortest it allows and the longest.
    let mut unknown_ids: Vec<String> = vec![
        "7b2d41e0-9a3c-4f51-8c62-0d4e5f6a7b8c".to_string(),
        "rotate-no-such-tunnel-5f1c".to_string(),
        "weird_id+with=every:char-class".to_string(),
        "a".to_string(),
        "z".repeat(128),
    ];
    // A tunnel that existed and whose data `CloseTunnel(delete)` removed is in
    // the same class as one that never existed.
    let deleted_id = open_tunnel(router, "the deleted tunnel", None).await;
    let (status, deleted) = call(
        router,
        "CloseTunnel",
        &json!({"tunnelId": deleted_id, "delete": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "CloseTunnel(delete) answered {status} {deleted:?}"
    );
    unknown_ids.push(deleted_id);

    for tunnel_id in unknown_ids {
        for mode in CLIENT_MODES {
            rows.push(Row {
                case: format!("no such tunnel {tunnel_id:?} + clientMode {mode}"),
                tunnel_id: None,
                request: json!({"tunnelId": tunnel_id, "clientMode": mode}),
                expected: MODELLED_NOT_FOUND,
                message: ExpectedMessage::Exact(format!("Tunnel {tunnel_id} does not exist.")),
            });
        }
    }

    // CLASS 2 — the tunnel is held but is no longer OPEN, however it got there.
    // The lifetimes cover each published bound of `maxLifetimeTimeoutMinutes`
    // (`range(min: 1, max: 720)`) and an interior value.
    let closings = [
        HowItClosed::CloseDeleteOmitted,
        HowItClosed::CloseDeleteFalse,
        HowItClosed::LifetimeElapsed { minutes: 1 },
        HowItClosed::LifetimeElapsed { minutes: 60 },
        HowItClosed::LifetimeElapsed { minutes: 720 },
    ];
    for how in closings {
        for mode in CLIENT_MODES {
            let case = format!("{how:?} + clientMode {mode}");
            let lifetime = match how {
                HowItClosed::LifetimeElapsed { minutes } => Some(minutes),
                _ => None,
            };
            let tunnel_id = open_tunnel(router, &case, lifetime).await;
            close_tunnel(router, engine, &tunnel_id, how, &case).await;
            rows.push(Row {
                case,
                tunnel_id: Some(tunnel_id.clone()),
                request: json!({"tunnelId": tunnel_id, "clientMode": mode}),
                expected: REQUEST_TIME_REFUSAL,
                message: ExpectedMessage::Exact(format!("Tunnel {tunnel_id} is close.")),
            });
        }
    }

    // CLASS 3 — a `destinationConfig` the requested `clientMode` does not
    // cover. The rows differ in the optional `thingName`'s presence and in the
    // number of services, because the rule is about the MEMBER's presence
    // against the mode and about nothing inside it.
    let configs = [
        ("services only", json!({"services": ["SSH"]})),
        (
            "thingName and one service",
            json!({"thingName": "thing-contract", "services": ["SSH"]}),
        ),
        (
            "several services",
            json!({"thingName": "thing-contract", "services": ["SSH", "HTTP", "DB"]}),
        ),
    ];
    for (label, config) in configs {
        let case = format!("destinationConfig ({label}) + clientMode SOURCE");
        let tunnel_id = open_tunnel(router, &case, None).await;
        rows.push(Row {
            case,
            tunnel_id: Some(tunnel_id.clone()),
            request: json!({
                "tunnelId": tunnel_id,
                "clientMode": "SOURCE",
                "destinationConfig": config,
            }),
            expected: REQUEST_TIME_REFUSAL,
            message: ExpectedMessage::Exact(
                "DestinationConfig is not allowed with clientMode SOURCE".to_string(),
            ),
        });
    }

    // CLASS 4 — more streams than a tunnel is multiplexed over: one past the
    // bound and well past it, on each mode that rotates the destination end.
    for extra in [1_usize, 4] {
        let count = MAX_SERVICES_PER_TUNNEL + extra;
        for mode in ["DESTINATION", "ALL"] {
            let case = format!("{count} destination services + clientMode {mode}");
            let tunnel_id = open_tunnel(router, &case, None).await;
            let services: Vec<String> = (0..count).map(|index| format!("svc-{index}")).collect();
            rows.push(Row {
                case,
                tunnel_id: Some(tunnel_id.clone()),
                request: json!({
                    "tunnelId": tunnel_id,
                    "clientMode": mode,
                    "destinationConfig": {"services": services},
                }),
                expected: REQUEST_TIME_REFUSAL,
                message: ExpectedMessage::Fragments(vec![
                    "destinationConfig.services".to_string(),
                    count.to_string(),
                ]),
            });
        }
    }

    rows
}

#[tokio::test]
async fn every_rotate_refusal_answers_the_error_of_its_class() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let rows = refusal_rows(&router, &engine).await;
    assert!(
        rows.len() > CLIENT_MODES.len(),
        "the table must span the operation's refusal classes, got {} rows",
        rows.len()
    );

    for row in &rows {
        let case = &row.case;
        let before = row
            .tunnel_id
            .as_deref()
            .map(|tunnel_id| stored_tokens(&engine, tunnel_id));

        let (status, body) = call(&router, "RotateTunnelAccessToken", &row.request).await;

        assert_eq!(
            status, row.expected.status,
            "case {case:?}: expected HTTP {}, answered {status} {body:?}",
            row.expected.status
        );
        assert_eq!(
            error_code(&body),
            row.expected.code,
            "case {case:?}: expected the wire code {:?}, answered {body:?}",
            row.expected.code
        );
        assert!(
            row.message.is_satisfied_by(message(&body)),
            "case {case:?}: expected the message {:?}, got {:?}",
            row.message,
            message(&body)
        );
        assert!(
            body.get("tunnelArn").is_none()
                && body.get("sourceAccessToken").is_none()
                && body.get("destinationAccessToken").is_none(),
            "case {case:?}: a refusal carries no ARN and no access token, got {body:?}"
        );
        if let (Some(tunnel_id), Some(before)) = (row.tunnel_id.as_deref(), before) {
            assert_eq!(
                stored_tokens(&engine, tunnel_id),
                before,
                "case {case:?}: a refused rotate must mint and store nothing"
            );
        }
    }
}

#[tokio::test]
async fn the_not_found_refusal_is_the_only_one_a_client_resolves_to_the_modelled_error() {
    // The two classes must stay distinguishable by the fact a client branches
    // on — the wire error code, since both classes share one status: a client
    // that resolves the operation's modelled error must not see a held tunnel's
    // request-time refusal as a missing resource, and must not see a missing
    // resource as a request it can fix.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    for row in refusal_rows(&router, &engine).await {
        let case = &row.case;
        let (status, body) = call(&router, "RotateTunnelAccessToken", &row.request).await;
        // The two classes name two codes, so one code resolves one class.
        assert_ne!(
            MODELLED_NOT_FOUND.code, REQUEST_TIME_REFUSAL.code,
            "the two refusal classes must not share one wire code"
        );
        let expected_code = if row.tunnel_id.is_none() {
            MODELLED_NOT_FOUND.code
        } else {
            REQUEST_TIME_REFUSAL.code
        };
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: only a tunnel this endpoint does not hold answers {}, \
             answered {status} {body:?}",
            MODELLED_NOT_FOUND.code
        );
        // Whatever the class, a refusal is a client error and never the
        // scaffold's 501 or a server fault.
        assert!(
            (400..500).contains(&status),
            "case {case:?}: a refusal must be a 4xx client error, answered {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn a_rotation_the_rules_admit_still_succeeds() {
    // The control row for the table above: an OPEN tunnel, a mode that covers
    // the destination end and a config inside the bound is not refused at all,
    // so each refusal above is about its own class and not about the operation.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount(&engine);

    let tunnel_id = open_tunnel(&router, "the admitted rotation", None).await;
    let before = stored_tokens(&engine, &tunnel_id);
    let (status, rotated) = call(
        &router,
        "RotateTunnelAccessToken",
        &json!({
            "tunnelId": tunnel_id,
            "clientMode": "ALL",
            "destinationConfig": {"services": ["SSH", "HTTP"]},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "an admitted rotation answered {status} {rotated:?}"
    );
    let after = stored_tokens(&engine, &tunnel_id);
    assert_ne!(
        after, before,
        "an admitted rotation must replace the stored tokens, still {before:?}"
    );
    assert_eq!(
        rotated["sourceAccessToken"].as_str(),
        Some(after.0.as_str()),
        "the response must carry the stored source token, got {:?}",
        rotated.get("sourceAccessToken")
    );
    assert_eq!(
        rotated["destinationAccessToken"].as_str(),
        Some(after.1.as_str()),
        "the response must carry the stored destination token, got {:?}",
        rotated.get("destinationAccessToken")
    );
}
