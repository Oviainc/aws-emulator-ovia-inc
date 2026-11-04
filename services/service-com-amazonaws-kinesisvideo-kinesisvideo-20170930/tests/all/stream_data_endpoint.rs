// SPDX-License-Identifier: CC0-1.0

//! `GetDataEndpoint`: which endpoint a stream's media APIs are addressed at.
//!
//! The rule under test is that the reported `DataEndpoint` is resolved from the
//! requested `APIName` and the stream it names — a bare `scheme://host` base URL of the
//! data plane that serves that API — and never the control-plane endpoint the call
//! itself arrived on. Kinesis Video Media's `InvalidEndpointException` ("Caller used
//! wrong endpoint to write data to a stream. On receiving such an exception, the user
//! must call `GetDataEndpoint` with `AccessMode` set to \"READ\" …") is what makes the
//! value per-plane: one endpoint serves the writing of media and another the reading of
//! it, so the two cannot be one address, and neither can be the control plane's.
//!
//! The concrete host and generated id are per-environment and nothing here asserts
//! them. What the cases assert is the shape of the value, its independence from the
//! address the request came in on, and the partition of the published `APIName` set it
//! induces — over a table whose rows differ in the API asked for, the stream named, the
//! form the stream is named by, the region of the credential scope, and the host the
//! request arrives on (an AWS-form control-plane host and a bare loopback listener).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as kvs;
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying only this service, over its own engine, with request
/// authentication off so a case can send an unsigned-but-scoped request.
fn harness() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(kvs::EmulatedService::new(engine));
    router
}

/// One control-plane address a case addresses the emulator at: the authority the
/// request arrives on, and the region of its SigV4 credential scope.
#[derive(Debug, Clone, Copy)]
struct ControlPlane {
    authority: &'static str,
    region: &'static str,
}

/// The control planes the table drives: the service's own regional endpoint, and a bare
/// loopback listener in another region — the two forms a request actually arrives on.
const CONTROL_PLANES: &[ControlPlane] = &[
    ControlPlane {
        authority: "kinesisvideo.us-east-1.amazonaws.com",
        region: "us-east-1",
    },
    ControlPlane {
        authority: "127.0.0.1:35573",
        region: "eu-west-2",
    },
];

/// POST one restJson1 call to `control` and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    control: ControlPlane,
    path: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let ControlPlane { authority, region } = control;
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{authority}{path}"))
        .header("host", authority)
        .header("content-type", "application/json")
        // The SigV4 credential scope is what routes the request to this service and
        // names its region; the signature itself is not checked here.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kinesisvideo/\
                 aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, parsed)
}

/// Create `name` on `control` and return its `StreamARN`.
async fn create_stream(
    router: &MultiServiceRouter,
    control: ControlPlane,
    name: &str,
) -> String {
    let (status, body) = call(
        router,
        control,
        "/createStream",
        serde_json::json!({ "StreamName": name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateStream expected HTTP 200, got {status} with {body:?}"
    );
    body.get("StreamARN")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case {name:?}: CreateStream returned no StreamARN: {body:?}"))
        .to_string()
}

/// `GetDataEndpoint` for one identifier and `APIName`, asserting the 200 and returning
/// the `DataEndpoint` value.
async fn data_endpoint(
    router: &MultiServiceRouter,
    control: ControlPlane,
    case: &str,
    identifier: serde_json::Value,
    api_name: &str,
) -> String {
    let mut request = identifier;
    request
        .as_object_mut()
        .expect("the identifier is a JSON object")
        .insert("APIName".to_string(), serde_json::json!(api_name));
    let (status, body) = call(router, control, "/getDataEndpoint", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with {body:?}"
    );
    body.get("DataEndpoint")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no DataEndpoint in {body:?}"))
        .to_string()
}

/// The host of a `scheme://host` base URL, failing the case when the value carries a
/// path, a query or no scheme at all: "The returned endpoint does not have the API name
/// appended. The client needs to add the API name to the returned endpoint", so
/// anything after the host would be appended to rather than replaced.
fn base_url_host(case: &str, endpoint: &str) -> String {
    let rest = endpoint.strip_prefix("https://").unwrap_or_else(|| {
        panic!("case {case:?}: expected an https base URL, got {endpoint:?}")
    });
    assert!(
        !rest.is_empty() && !rest.contains('/') && !rest.contains('?') && !rest.contains('#'),
        "case {case:?}: expected scheme and host only with no path, got {endpoint:?}"
    );
    rest.to_string()
}

/// Every `APIName` the model publishes resolves to the data endpoint of the plane that
/// serves it: a bare base URL, never the control-plane endpoint the call arrived on,
/// with every API of one access mode on one host and the two modes on different hosts.
#[tokio::test]
async fn each_api_name_resolves_to_the_endpoint_of_the_plane_that_serves_it() {
    // The only API that WRITES media onto a stream; every other published value reads
    // it. From the `APIName` enum (`smithy_query(query="shape", shape="APIName")`).
    const PUT_MEDIA: &str = "PUT_MEDIA";

    assert!(
        kvs::ops::types::A_P_I_NAME_VALUES.contains(&PUT_MEDIA),
        "the model should publish {PUT_MEDIA:?}, got {:?}",
        kvs::ops::types::A_P_I_NAME_VALUES
    );

    for control in CONTROL_PLANES {
        let control = *control;
        for stream in ["probe", "second.stream_2-B"] {
            let router = harness();
            let arn = create_stream(&router, control, stream).await;
            // The two identifier forms of one stream are the same resource, so they
            // name the same endpoint.
            let identifiers: Vec<(&str, serde_json::Value)> = vec![
                ("by name", serde_json::json!({ "StreamName": stream })),
                ("by ARN", serde_json::json!({ "StreamARN": arn })),
            ];

            let mut endpoints: BTreeMap<String, String> = BTreeMap::new();
            let mut markers: BTreeMap<String, String> = BTreeMap::new();
            for api_name in kvs::ops::types::A_P_I_NAME_VALUES {
                for (form, identifier) in &identifiers {
                    let case = format!(
                        "{api_name} {form} on {} in {}",
                        control.authority, control.region
                    );
                    let endpoint =
                        data_endpoint(&router, control, &case, identifier.clone(), api_name).await;
                    let host = base_url_host(&case, &endpoint);

                    assert_ne!(
                        endpoint,
                        format!("https://{}", control.authority),
                        "case {case:?}: the data endpoint must not be the control-plane \
                         endpoint the call was made to, got {endpoint:?}"
                    );
                    assert_ne!(
                        host, control.authority,
                        "case {case:?}: the data host must not be the control-plane host, \
                         got {endpoint:?}"
                    );
                    // One label of the plane's own under this service's regional host:
                    // the plane marker and the generated id are in that label, and the
                    // region is in the suffix.
                    let suffix = format!(".{}", kvs::state::service_host(control.region));
                    let label = host.strip_suffix(&suffix).unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: expected a host under {suffix:?}, got {endpoint:?}"
                        )
                    });
                    assert!(
                        !label.is_empty() && !label.contains('.') && !label.contains(':'),
                        "case {case:?}: expected one host label under {suffix:?}, got \
                         {endpoint:?}"
                    );
                    let (marker, id) = label.split_once('-').unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: expected a <plane>-<id> host label, got {endpoint:?}"
                        )
                    });
                    assert!(
                        !marker.is_empty() && !id.is_empty(),
                        "case {case:?}: expected a <plane>-<id> host label, got {endpoint:?}"
                    );
                    // The API name is never appended to the value.
                    assert!(
                        !endpoint.to_ascii_lowercase().contains(
                            &api_name.to_ascii_lowercase().replace('_', "")
                        ),
                        "case {case:?}: the endpoint must not carry the API name, got \
                         {endpoint:?}"
                    );

                    // Both identifier forms of one stream name one endpoint.
                    let previous = endpoints.insert((*api_name).to_string(), endpoint.clone());
                    if let Some(previous) = previous {
                        assert_eq!(
                            previous, endpoint,
                            "case {case:?}: the two identifier forms of one stream must \
                             resolve to one endpoint, expected {previous:?}, got {endpoint:?}"
                        );
                    }
                    markers.insert((*api_name).to_string(), marker.to_string());
                }
            }

            // The published API set is partitioned by the access mode each API needs:
            // the one that writes media to the stream, and the ones that read it. Two
            // endpoints, and the mode marker of one is not the marker of the other.
            let write_endpoint = endpoints
                .get(PUT_MEDIA)
                .unwrap_or_else(|| panic!("case {stream:?}: no endpoint for {PUT_MEDIA}"));
            let read_endpoints: BTreeSet<&String> = endpoints
                .iter()
                .filter(|(api, _)| api.as_str() != PUT_MEDIA)
                .map(|(_, endpoint)| endpoint)
                .collect();
            assert_eq!(
                read_endpoints.len(),
                1,
                "case {stream:?} on {}: every API that reads the stream shares one data \
                 endpoint, got {endpoints:?}",
                control.authority
            );
            assert!(
                !read_endpoints.contains(write_endpoint),
                "case {stream:?} on {}: the endpoint that writes media must not be the one \
                 that reads it, got {endpoints:?}",
                control.authority
            );
            let write_marker = markers
                .get(PUT_MEDIA)
                .unwrap_or_else(|| panic!("case {stream:?}: no marker for {PUT_MEDIA}"));
            let read_markers: BTreeSet<&String> = markers
                .iter()
                .filter(|(api, _)| api.as_str() != PUT_MEDIA)
                .map(|(_, marker)| marker)
                .collect();
            assert!(
                !read_markers.contains(write_marker),
                "case {stream:?} on {}: the write plane marker must differ from the read \
                 one, got {markers:?}",
                control.authority
            );
        }
    }
}

/// The endpoint is the STREAM's: stable however often it is read, and different for a
/// different stream of the same store, plane and region.
#[tokio::test]
async fn a_data_endpoint_is_the_streams_own_and_a_read_does_not_change_it() {
    let control = CONTROL_PLANES[0];
    let router = harness();
    let streams = ["alpha", "beta", "a.b_c-1"];
    for stream in streams {
        create_stream(&router, control, stream).await;
    }

    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for api_name in ["PUT_MEDIA", "GET_MEDIA", "LIST_FRAGMENTS"] {
        for stream in streams {
            let mut values = Vec::new();
            for round in 1..=3 {
                let case = format!("{stream} {api_name} read {round}");
                values.push(
                    data_endpoint(
                        &router,
                        control,
                        &case,
                        serde_json::json!({ "StreamName": stream }),
                        api_name,
                    )
                    .await,
                );
            }
            let first = values[0].clone();
            assert!(
                values.iter().all(|value| *value == first),
                "case {stream:?} {api_name:?}: reading the endpoint must not change it, \
                 got {values:?}"
            );
            seen.entry(api_name.to_string()).or_default().push(first);
        }
    }

    for (api_name, values) in &seen {
        let distinct: BTreeSet<&String> = values.iter().collect();
        assert_eq!(
            distinct.len(),
            streams.len(),
            "case {api_name:?}: each stream must have its own data endpoint, got {values:?}"
        );
    }
}

/// Every value the model publishes is placed on a plane, and a value it does not
/// publish is placed on none — so no `APIName` is served an endpoint chosen for it, and
/// an unpublished one is refused as a bad argument.
#[tokio::test]
async fn every_published_api_name_is_placed_on_a_plane_and_nothing_else_is() {
    for api_name in kvs::ops::types::A_P_I_NAME_VALUES {
        assert!(
            kvs::state::data_plane_of(api_name).is_some(),
            "case {api_name:?}: every published APIName must resolve to a data plane"
        );
    }

    let control = CONTROL_PLANES[0];
    let router = harness();
    create_stream(&router, control, "placed").await;
    for case in ["NOT_AN_API", "put_media", "GET_MEDIA ", ""] {
        assert!(
            kvs::state::data_plane_of(case).is_none(),
            "case {case:?}: a value the model does not publish must resolve to no plane"
        );
        let (status, body) = call(
            &router,
            control,
            "/getDataEndpoint",
            serde_json::json!({ "StreamName": "placed", "APIName": case }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            body.get("__type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .rsplit('#')
                .next()
                .unwrap_or_default(),
            "InvalidArgumentException",
            "case {case:?}: got {body:?}"
        );
    }
}
