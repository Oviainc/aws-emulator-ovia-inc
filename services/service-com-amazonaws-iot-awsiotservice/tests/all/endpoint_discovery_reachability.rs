// SPDX-License-Identifier: CC0-1.0

//! The address `DescribeEndpoint` publishes is one this emulator SERVES: the discovery
//! call is followed here by the data-plane calls a client makes against the value it was
//! handed.
//!
//! This is the sequence every AWS SDK sample prescribes — call `DescribeEndpoint`, then
//! build an IoT data client with `endpoint_url` set to the `endpointAddress` — so what it
//! pins is that the announced address leads back INTO the emulator and that the plane
//! discovered through it answers and keeps state. The IoT data plane is mounted here
//! beside the control plane on one engine, the shape `core/router/src/lib.rs` mounts them
//! in; the Jobs/Commands data plane, which lives in a third crate, is driven the same way
//! in `core/router/tests/all/iot_jobs_data_endpoint_discovery.rs`.
//!
//! This router resolves a service from the SigV4 credential scope first
//! (`emulator_server::resolve_route_key`), so what the second leg proves is that the
//! discovered address is the emulator's own endpoint — the one address its planes are
//! served at — and not that the host is the dispatch key. That the published value names
//! no host outside the emulator, for every endpoint type and wherever the emulator is
//! served, is pinned in `tests/all/describe_endpoint.rs`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::{EndpointUrl, MultiServiceRouter};

/// The region every call below is signed for.
const REGION: &str = "us-west-1";

/// One address this emulator is served at, as the core publishes it from its data port.
/// The port is the operating system's in a real run; what matters here is that the
/// discovery call reports this address and that the data plane answers at it.
const SERVED_AT: &str = "https://127.0.0.1:41287";

/// The endpoint types a client discovers an IoT data plane with: the whole catalog
/// `DescribeEndpointRequest$endpointType` documents (`iot:Data`, `iot:Data-ATS`,
/// `iot:CredentialProvider`, `iot:Jobs`), plus the request that names none — every one of
/// which has to lead a client back to this emulator.
///
/// Each row carries its own thing, so no row can pass by reading another's shadow.
const DISCOVERY_CASES: &[(Option<&str>, &str)] = &[
    (None, "probe-default"),
    (Some("iot:Data"), "probe-data"),
    (Some("iot:Data-ATS"), "probe-data-ats"),
    (Some("iot:CredentialProvider"), "probe-credentials"),
    (Some("iot:Jobs"), "probe-jobs"),
];

/// The emulator: the IoT control plane and the IoT data plane on ONE engine, served at
/// [`SERVED_AT`].
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.set_url_base(
            EndpointUrl::parse(SERVED_AT).expect("the served base is a usable endpoint"),
        );
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iotdataplane_iotmoonrakerservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router }
    }

    /// One request, signed for `signing_name` and addressed to `url` — with the `Host`
    /// header the client would send, which for a data-plane call is the discovered
    /// address itself.
    async fn call(
        &self,
        method: &str,
        url: &str,
        host: &str,
        signing_name: &str,
        body: &str,
    ) -> (u16, String) {
        let request = http::Request::builder()
            .method(method)
            .uri(url)
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let rendered = String::from_utf8_lossy(response.body()).to_string();
        (status, rendered)
    }

    /// `GET /endpoint?endpointType=…` on the control plane, at the control-plane host a
    /// client reaches the emulator on, returning the address it publishes.
    async fn discover(&self, endpoint_type: Option<&str>) -> String {
        let host = service_common::data_plane_addressing::url_authority(SERVED_AT);
        let query = match endpoint_type {
            Some(value) => format!("?endpointType={}", crate::describe_endpoint::percent_encode(value)),
            None => String::new(),
        };
        let (status, rendered) = self
            .call(
                "GET",
                &format!("https://{host}/endpoint{query}"),
                host,
                "iot",
                "",
            )
            .await;
        assert_eq!(
            status, 200,
            "case {endpoint_type:?}: endpoint discovery must answer 200, got {status} with {rendered}"
        );
        let parsed: Value = serde_json::from_str(&rendered).unwrap_or(Value::Null);
        parsed
            .get("endpointAddress")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {endpoint_type:?}: no endpointAddress in {rendered}"))
            .to_string()
    }
}

/// A client that does exactly what the AWS documentation prescribes — discover the
/// endpoint, then talk to the data plane at the address it was given — reaches the data
/// plane, for every endpoint type.
#[tokio::test]
async fn a_discovered_endpoint_address_serves_the_iot_data_plane() {
    let emulator = Emulator::new();

    for (endpoint_type, thing) in DISCOVERY_CASES {
        let case = endpoint_type.unwrap_or("no endpointType");
        let address = emulator.discover(*endpoint_type).await;

        // The address is one a client of this emulator can dial: not an AWS host, and
        // not a host that names any service's endpoint.
        assert!(
            !address.contains("amazonaws.com")
                && !service_common::data_plane_addressing::names_a_service(&address),
            "case {case}: the discovered address must be one this emulator serves, \
             got {address:?}"
        );

        // The second leg: the IoT data plane, addressed at the discovered host. It must
        // be mounted there (not 404 UnknownService), implemented (not 501
        // NotImplemented), and backed by state — which is what the shadow written here
        // and read back at the same address proves.
        let (status, rendered) = emulator
            .call(
                "POST",
                &format!("https://{address}/things/{thing}/shadow"),
                &address,
                "iotdata",
                r#"{"state":{"desired":{"lamp":"on"}}}"#,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: updating a shadow at the discovered address {address} must \
             answer 200, got {status} with {rendered}"
        );

        let (status, rendered) = emulator
            .call(
                "GET",
                &format!("https://{address}/things/{thing}/shadow"),
                &address,
                "iotdata",
                "",
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: reading the shadow back at the discovered address {address} \
             must answer 200, got {status} with {rendered}"
        );
        let document: Value = serde_json::from_str(&rendered).unwrap_or(Value::Null);
        assert_eq!(
            document
                .pointer("/state/desired/lamp")
                .and_then(Value::as_str),
            Some("on"),
            "case {case}: the shadow written through the discovered address must be the \
             one read back there, got {rendered}"
        );

        // The control plane keeps answering at the same address: one endpoint serves
        // this emulator's whole IoT surface, so a client needs no second address.
        let (status, rendered) = emulator
            .call(
                "GET",
                &format!("https://{address}/endpoint"),
                &address,
                "iot",
                "",
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case}: the control plane must answer at the discovered address \
             {address}, got {status} with {rendered}"
        );
    }
}
