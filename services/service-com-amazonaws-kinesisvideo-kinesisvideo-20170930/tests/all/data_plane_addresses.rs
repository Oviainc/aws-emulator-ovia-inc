// SPDX-License-Identifier: CC0-1.0

//! Which PLANE an address belongs to: the reading a plane's own routes are scoped by.
//!
//! The rule under test: an announced data endpoint is the address of ONE access mode's
//! plane — Kinesis Video Media's `InvalidEndpointException` is "Caller used wrong endpoint
//! to write data to a stream. On receiving such an exception, the user must call
//! `GetDataEndpoint` with `AccessMode` set to \"READ\"" — so the address announced for an
//! `APIName` maps back to the plane that serves that API, and every other address maps to
//! no plane at all: the control-plane front door the discovery call arrived on, the
//! service's published regional host, and any authority this store announced nothing at.
//! That is what lets each plane's routes be bound on the address announced for them and
//! nowhere else, which is the half of the contract the data planes' own crates apply.
//!
//! The table spans what the rule generalises over: every `APIName` the model publishes
//! (whose access modes partition them), both identifier forms of a stream, and the shapes
//! of address that are not a plane's — this emulator's own front door, the published
//! service host, an announced authority's host on another port, and a value no host is
//! shaped like.

use std::sync::Arc;

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as kvs;
use smithy_server_runtime::MultiServiceRouter;

/// The authority the control calls in this suite arrive on: a client on the machine this
/// emulator runs on reaches it at a loopback address.
const CONTROL_AUTHORITY: &str = "127.0.0.1:9297";

/// The region of the credential scope every request carries.
const REGION: &str = "us-east-1";

/// A router carrying only this service, over an engine this suite keeps a handle on — the
/// store the announced addresses are recorded in.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(kvs::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// POST one restJson1 call at the control-plane authority.
    async fn call(&self, path: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let response = self.call_at(CONTROL_AUTHORITY, path, body).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null);
        (status, parsed)
    }

    /// POST one restJson1 call at `authority` — the address the request arrives on — and
    /// answer the whole response, headers included.
    async fn call_at(
        &self,
        authority: &str,
        path: &str,
        body: serde_json::Value,
    ) -> http::Response<bytes::Bytes> {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{authority}{path}"))
            .header("host", authority)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261107/{REGION}/kinesisvideo/\
                     aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.router.dispatch(request).await
    }

    /// Create `name` and answer its `StreamARN`.
    async fn create_stream(&self, name: &str) -> String {
        let (status, body) = self
            .call(
                "/createStream",
                serde_json::json!({ "StreamName": name, "DataRetentionInHours": 1 }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from CreateStream, got {status} with {body:?}"
        );
        body.get("StreamARN")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("case {name:?}: no StreamARN in {body:?}"))
            .to_string()
    }

    /// The authority announced for `api_name`, with the announced scheme stripped off.
    async fn announced_authority(
        &self,
        identifier: serde_json::Value,
        api_name: &str,
    ) -> String {
        let mut body = identifier;
        body.as_object_mut()
            .expect("the identifier is a JSON object")
            .insert("APIName".to_string(), serde_json::json!(api_name));
        let (status, answer) = self.call("/getDataEndpoint", body).await;
        assert_eq!(
            status, 200,
            "case {api_name:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        let endpoint = answer
            .get("DataEndpoint")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let scheme = format!("{}://", kvs::state::DATA_PLANE_SCHEME);
        endpoint
            .strip_prefix(&scheme)
            .unwrap_or_else(|| {
                panic!("case {api_name:?}: expected a {scheme} origin, got {endpoint:?}")
            })
            .to_string()
    }
}

/// Every announced address maps back to the plane that serves the API it was announced
/// for, whichever identifier form asked for it.
#[tokio::test]
async fn an_announced_address_maps_back_to_the_plane_that_serves_its_api() {
    let harness = Harness::new();
    let arn = harness.create_stream("addressed").await;
    let identifiers: Vec<(&str, serde_json::Value)> = vec![
        ("by name", serde_json::json!({ "StreamName": "addressed" })),
        ("by ARN", serde_json::json!({ "StreamARN": arn })),
    ];

    for api_name in kvs::ops::types::A_P_I_NAME_VALUES {
        let expected = kvs::state::data_plane_of(api_name)
            .unwrap_or_else(|| panic!("case {api_name:?}: the model's APIName must have a plane"));
        for (form, identifier) in &identifiers {
            let case = format!("{api_name} {form}");
            let authority = harness
                .announced_authority(identifier.clone(), api_name)
                .await;
            let placed = kvs::state::published_plane(&*harness.engine, &authority);
            assert_eq!(
                placed,
                Some(expected),
                "case {case:?}: the address announced for it must belong to its own plane, \
                 got {placed:?} for {authority:?}"
            );
        }
    }
}

/// RULE: this service's own routes are served at the endpoint a control call arrives on
/// and not at an address it announced for a data plane — a control call that reached a
/// plane's address is answered `404` naming the operation it could not route, with no
/// message and nothing in the store changed, while the same call at the control-plane
/// address is served.
///
/// The rows differ in the control operation asked for (a read, a list and the discovery
/// call itself) and in the plane whose address it is asked at (write and read), so the
/// scoping is a property of the address rather than of one operation.
#[tokio::test]
async fn a_control_call_is_not_served_at_an_address_announced_for_a_data_plane() {
    let harness = Harness::new();
    harness.create_stream("scoped").await;
    let write = harness
        .announced_authority(serde_json::json!({ "StreamName": "scoped" }), "PUT_MEDIA")
        .await;
    let read = harness
        .announced_authority(serde_json::json!({ "StreamName": "scoped" }), "GET_MEDIA")
        .await;

    // (the control path, the body it carries).
    let calls: Vec<(&str, serde_json::Value)> = vec![
        ("/describeStream", serde_json::json!({ "StreamName": "scoped" })),
        ("/listStreams", serde_json::json!({})),
        (
            "/getDataEndpoint",
            serde_json::json!({ "StreamName": "scoped", "APIName": "PUT_MEDIA" }),
        ),
    ];
    for plane in [&write, &read] {
        for (path, body) in &calls {
            let case = format!("{path} on {plane}");
            let response = harness.call_at(plane, path, body.clone()).await;
            assert_eq!(
                response.status().as_u16(),
                404,
                "case {case:?}: a data plane's address routes no control operation, got {} {:?}",
                response.status(),
                String::from_utf8_lossy(response.body())
            );
            // The refusal is the framework's own answer for an unrouted path: the bare
            // element naming the condition, with no error-type header, because no
            // operation was identified to name one for.
            assert!(
                response.headers().get("x-amzn-errortype").is_none(),
                "case {case:?}: no operation was identified, so none is named in the \
                 error-type header, got {:?}",
                response.headers().get("x-amzn-errortype")
            );
            assert_eq!(
                String::from_utf8_lossy(response.body()),
                smithy_server_runtime::UNRECOGNISED_PROTOCOL_BODY,
                "case {case:?}: expected the one wording this emulator states an unrouted \
                 path in, got {:?}",
                String::from_utf8_lossy(response.body())
            );
        }
    }

    // And the same calls at the control-plane address are served, so the refusals above
    // are about the address and not about the request.
    for (path, body) in &calls {
        let (status, answered) = harness.call(path, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {path:?}: the control-plane address serves its own routes, \
             got {status} with {answered:?}"
        );
    }
}

/// An address that is not a plane's belongs to no plane: the control-plane front door the
/// discovery call arrived on, the published service host, an announced host on another
/// port, and a value no host is shaped like.
#[tokio::test]
async fn an_address_no_plane_was_announced_at_belongs_to_no_plane() {
    let harness = Harness::new();
    harness.create_stream("unplaced").await;
    let write = harness
        .announced_authority(serde_json::json!({ "StreamName": "unplaced" }), "PUT_MEDIA")
        .await;
    let (host, port) = write
        .rsplit_once(':')
        .unwrap_or_else(|| panic!("expected the announced origin to carry its port, got {write:?}"));
    let other_port = port
        .parse::<u16>()
        .map(|port| format!("{host}:{}", port.wrapping_add(1)))
        .unwrap_or_else(|_| panic!("expected a port, got {write:?}"));

    let cases: Vec<(&str, String)> = vec![
        ("the control-plane front door", CONTROL_AUTHORITY.to_string()),
        (
            "the published service host",
            kvs::state::service_host(REGION),
        ),
        ("the announced host on another port", other_port),
        ("the announced host with no port at all", host.to_string()),
        ("a value no host is shaped like", "not a host".to_string()),
        ("nothing at all", String::new()),
    ];
    for (case, authority) in &cases {
        let placed = kvs::state::published_plane(&*harness.engine, authority);
        assert_eq!(
            placed, None,
            "case {case:?}: {authority:?} is no plane's announced address, got {placed:?}"
        );
    }

    // And the one address that IS a plane's still is, so the cases above are not passing
    // because nothing was announced at all.
    assert_eq!(
        kvs::state::published_plane(&*harness.engine, &write),
        Some(kvs::state::DataPlane::Write),
        "the announced write address must belong to the write plane, got none for {write:?}"
    );
}
