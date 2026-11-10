// SPDX-License-Identifier: CC0-1.0

//! `DescribeEndpoint`, end to end over HTTP.
//!
//! Every test drives real requests through the `MultiServiceRouter` this crate's
//! service is mounted in, so routing, the restJson1 `GET /endpoint` binding with its
//! `endpointType` query parameter, the handler and the response serialization are all
//! exercised.
//!
//! The rules under test, none of them specific to one endpoint type, one region or one
//! address this emulator is served at:
//!
//! * the `endpointAddress` published is the address THIS emulator answers on — the
//!   `host[:port]` of the endpoint it serves its whole surface at — because a client
//!   DIALS that value to reach the data plane, so it names no host outside the emulator
//!   and no AWS service endpoint;
//! * it does not come from the request: the signed region, the host addressed and the
//!   recorded endpoint identifier change nothing about it, and it changes when the
//!   address the emulator is served at changes;
//! * an account still has ONE endpoint per region — created by the first call there,
//!   recorded in this service's state, and reused by every later call — so repeating the
//!   call returns the same endpoint;
//! * an `endpointType` outside the catalog the model documents is refused with the
//!   modelled `InvalidRequestException` (400) and creates no endpoint.
//!
//! What this suite does NOT prove is that the published host is the router's dispatch
//! key: this router resolves a service from the SigV4 credential scope first
//! (`emulator_server::resolve_route_key`), so a request is served whatever host it
//! addressed. That the discovered address serves the IoT DATA plane is driven in
//! `tests/all/endpoint_discovery_reachability.rs`, and across the whole router in
//! `core/router/tests/all/iot_jobs_data_endpoint_discovery.rs`.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use service_com_amazonaws_iot_awsiotservice::state::{
    ComAmazonawsIotAwsiotserviceState, ENDPOINT_IDENTIFIER_LENGTH, MAX_ENDPOINT_REGIONS,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::{EndpointUrl, MultiServiceRouter};

/// The region used wherever a test is not varying the region. Deliberately NOT the
/// default region, so an assertion about the recorded region cannot pass by accident
/// through the fallback.
const REGION: &str = "eu-west-2";

/// The region `DescribeEndpoint` falls back to when the request carries no readable
/// SigV4 credential scope (`state::DEFAULT_REGION`).
const DEFAULT_REGION: &str = "us-east-1";

/// The endpoint host region of a request whose region cannot be read from a
/// credential scope. It is neither [`REGION`] nor [`DEFAULT_REGION`], so a fallback
/// assertion also proves nothing is taken from the request host.
const FALLBACK_HOST_REGION: &str = "ap-northeast-1";

/// The address this emulator is served at wherever a test is not varying it: a loopback
/// bind with an OS-assigned port, which is what the emulator's own data port looks like.
const SERVED_AT: &str = "https://127.0.0.1:39355";

/// One address this emulator can be served at (`MultiServiceRouter::set_url_base`, which
/// is what the core sets from its data port), and the `host[:port]` a client reaches it
/// by there.
///
/// The rows span the bases the core can publish: a loopback bind with a port, a second
/// port on the same host (so no single value satisfies two rows), an IPv6 bind, a named
/// host with a port, and the base of an emulator that is not served on a port at all
/// (`smithy_server_runtime::UNSERVED_URL_BASE`, reached in process).
const SERVING_CASES: &[(&str, &str)] = &[
    ("https://127.0.0.1:39355", "127.0.0.1:39355"),
    ("https://127.0.0.1:9297", "127.0.0.1:9297"),
    ("https://[::1]:8443", "[::1]:8443"),
    ("https://emulator.test:443", "emulator.test:443"),
    (
        smithy_server_runtime::UNSERVED_URL_BASE,
        "emulator.invalid",
    ),
];

/// One endpoint-type case: what the request asks for.
///
/// There is no per-type address form here on purpose: this emulator serves every one of
/// those endpoints at the single endpoint it answers on, so each type's address is that
/// one address.
struct TypeCase {
    name: &'static str,
    endpoint_type: Option<&'static str>,
}

/// Every endpoint type the `endpointType` member documents (read with
/// `smithy_query(query="doc",shape="DescribeEndpointRequest",member="endpointType",
/// model_path="services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json")`),
/// plus the request that names none.
const TYPE_CASES: &[TypeCase] = &[
    TypeCase {
        name: "no endpointType at all",
        endpoint_type: None,
    },
    TypeCase {
        name: "iot:Data",
        endpoint_type: Some("iot:Data"),
    },
    TypeCase {
        name: "iot:Data-ATS",
        endpoint_type: Some("iot:Data-ATS"),
    },
    TypeCase {
        name: "iot:CredentialProvider",
        endpoint_type: Some("iot:CredentialProvider"),
    },
    TypeCase {
        name: "iot:Jobs",
        endpoint_type: Some("iot:Jobs"),
    },
];

/// How a request is signed, which is where the region of the endpoint record comes from.
#[derive(Debug, Clone, Copy)]
enum Signing<'a> {
    /// A SigV4 `Authorization` header whose credential scope names this region.
    SignedFor(&'a str),
    /// A SigV4 `Authorization` header whose credential scope names this region, sent
    /// to an endpoint host in another region.
    SignedForOnForeignHost(&'a str),
    /// An `Authorization` header that is not a SigV4 scope at all.
    Unparseable,
    /// No `Authorization` header: the request routes by its endpoint host.
    Anonymous,
}

/// One emulator: the shared engine plus a router with this service mounted, served at
/// one address.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// An emulator served at `base` — the published URL the core hands its router when
    /// its data port starts, and the one every address a handler publishes names.
    fn served_at(base: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.set_url_base(
            EndpointUrl::parse(base).unwrap_or_else(|| panic!("{base} is a usable endpoint base")),
        );
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    fn new() -> Self {
        Self::served_at(SERVED_AT)
    }

    /// This service's store, as the handlers reach it.
    fn state(&self) -> engine::ServiceState<ComAmazonawsIotAwsiotserviceState> {
        self.engine.state::<ComAmazonawsIotAwsiotserviceState>()
    }

    /// The endpoint identifier recorded for `region`, if an endpoint was created.
    fn recorded_identifier(&self, region: &str) -> Option<String> {
        self.state()
            .lock()
            .endpoint_identifier(region)
            .map(str::to_string)
    }

    async fn describe_endpoint(&self, signing: Signing<'_>, endpoint_type: Option<&str>) -> Reply {
        let host_region = match signing {
            Signing::SignedFor(region) => region,
            // The host names a region the credential scope does not, so nothing in the
            // answer can have come from the scope by way of the host.
            Signing::SignedForOnForeignHost(_) | Signing::Unparseable | Signing::Anonymous => {
                FALLBACK_HOST_REGION
            }
        };
        let host = format!("iot.{host_region}.amazonaws.com");
        let query = match endpoint_type {
            Some(value) => format!("?endpointType={}", percent_encode(value)),
            None => String::new(),
        };
        let mut request = http::Request::builder()
            .method("GET")
            .uri(format!("https://{host}/endpoint{query}"))
            .header("host", host.clone());
        match signing {
            // The credential scope names the `iot` signing name the model's
            // `aws.auth#sigv4` trait declares, which is also what routes the request.
            Signing::SignedFor(region) | Signing::SignedForOnForeignHost(region) => {
                request = request.header(
                    "authorization",
                    format!(
                        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/iot/aws4_request, \
                         SignedHeaders=host, Signature=0123456789abcdef"
                    ),
                );
            }
            Signing::Unparseable => {
                request = request.header("authorization", "Bearer not-a-sigv4-header");
            }
            Signing::Anonymous => {}
        }
        let request = request
            .body(Bytes::new())
            .expect("the DescribeEndpoint request is well formed");
        Reply::of(self.router.dispatch(request).await)
    }
}

/// A dispatched response, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Bytes,
}

impl Reply {
    fn of(response: http::Response<Bytes>) -> Self {
        Self {
            status: response.status().as_u16(),
            body: response.into_body(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("response body is not JSON ({e}): {}", self.text()))
    }

    /// The `endpointAddress` the response carries.
    fn endpoint_address(&self) -> String {
        self.json()["endpointAddress"]
            .as_str()
            .unwrap_or_else(|| panic!("no endpointAddress in {}", self.text()))
            .to_string()
    }

    /// The error code a restJson1 error response carries.
    fn error_code(&self) -> String {
        self.json()["__type"]
            .as_str()
            .unwrap_or_else(|| panic!("no __type in {}", self.text()))
            .to_string()
    }
}

/// Percent-encode a query value, leaving the unreserved set and the `:` the
/// endpoint-type tokens contain (RFC 3986 allows `:` in a query).
///
/// Hand-written, as the equivalent helpers in sibling crates' test modules are: it
/// is three lines of test-only encoding, and a test helper is not worth a new
/// dependency entry in the workspace lock file.
pub(crate) fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b':' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Whether `identifier` is one lowercase-alphanumeric hostname label starting with a
/// letter, which is the form of the account-specific prefix AWS identifies an account's
/// IoT endpoint by.
fn is_well_formed_identifier(identifier: &str) -> bool {
    identifier.len() == ENDPOINT_IDENTIFIER_LENGTH
        && identifier.starts_with(|c: char| c.is_ascii_lowercase())
        && identifier
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Assert that `address` is an address a client of THIS emulator can dial: a host it can
/// address, and not an AWS public host — which resolves off this machine, where the
/// emulator is the only thing serving the IoT API.
fn assert_addresses_the_emulator(case: &str, address: &str) {
    assert!(
        !address.contains("amazonaws.com"),
        "case {case}: the published address must not name an AWS host, got {address:?}"
    );
    assert!(
        service_common::data_plane_addressing::is_host_shaped(address),
        "case {case}: the published address must be a host a client can dial, got {address:?}"
    );
}

/// Every documented endpoint type is served, and each one is published at the address
/// this emulator answers on — whatever address that is.
#[tokio::test]
async fn every_documented_endpoint_type_is_published_at_the_served_address() {
    // A region the issue did not cite, to keep the rule region-independent.
    let signing = Signing::SignedFor("eu-west-1");
    let region = "eu-west-1";

    for (base, expected) in SERVING_CASES {
        let emulator = Emulator::served_at(base);
        for type_case in TYPE_CASES {
            let case = format!("{} served at {base}", type_case.name);
            let reply = emulator
                .describe_endpoint(signing, type_case.endpoint_type)
                .await;
            assert_eq!(
                reply.status,
                200,
                "case {case}: expected 200, got {} body {}",
                reply.status,
                reply.text()
            );
            let address = reply.endpoint_address();
            assert_eq!(
                &address, expected,
                "case {case}: expected the served address {expected:?}, got {address:?}"
            );
            assert_addresses_the_emulator(&case, &address);

            // The endpoint itself is still created and recorded for the region.
            let identifier = emulator.recorded_identifier(region).unwrap_or_else(|| {
                panic!("case {case}: the call recorded no endpoint for {region}")
            });
            assert!(
                is_well_formed_identifier(&identifier),
                "case {case}: expected a 14-character lowercase-alphanumeric label, \
                 got {identifier:?}"
            );
        }
    }
}

/// The endpoint is created once: the first call records it and every later call — of any
/// type — returns the same endpoint.
#[tokio::test]
async fn the_endpoint_is_created_once_and_every_later_call_returns_it() {
    let emulator = Emulator::new();
    let signing = Signing::SignedFor(REGION);
    let served = service_common::data_plane_addressing::url_authority(SERVED_AT);

    assert_eq!(
        emulator.recorded_identifier(REGION),
        None,
        "no endpoint may exist before the first DescribeEndpoint call"
    );

    let first = emulator.describe_endpoint(signing, None).await;
    assert_eq!(
        first.status,
        200,
        "the first call must succeed, got {} body {}",
        first.status,
        first.text()
    );
    let created = emulator
        .recorded_identifier(REGION)
        .expect("the first call must create the account's endpoint");

    // Repeat every type several times: one account's endpoint is one endpoint, so the
    // address never changes between calls.
    for round in 0..3 {
        for type_case in TYPE_CASES {
            let case = format!("{} round {round}", type_case.name);
            let reply = emulator
                .describe_endpoint(signing, type_case.endpoint_type)
                .await;
            assert_eq!(
                reply.status,
                200,
                "case {case}: expected 200, got {} body {}",
                reply.status,
                reply.text()
            );
            assert_eq!(
                reply.endpoint_address(),
                served,
                "case {case}: the endpoint changed between calls, got {:?}",
                reply.endpoint_address()
            );
            assert_eq!(
                emulator.recorded_identifier(REGION).as_deref(),
                Some(created.as_str()),
                "case {case}: a later call replaced the created endpoint"
            );
        }
    }
}

/// Nothing in the request moves the published address: every signed region, a request
/// signed for one region on another region's host, and a request with no readable
/// credential scope are all told the one address this emulator answers on — while each
/// region still gets its own endpoint record.
#[tokio::test]
async fn the_published_address_is_the_served_one_whatever_the_request_names() {
    // Regions from four partitions' naming forms, none of them cited by the issue.
    const SIGNED_REGIONS: &[&str] = &[
        "us-east-1",
        "us-west-2",
        "eu-central-1",
        "ap-southeast-2",
        "me-south-1",
        "us-gov-east-1",
        "cn-north-1",
    ];

    let emulator = Emulator::new();
    let served = service_common::data_plane_addressing::url_authority(SERVED_AT);
    let mut identifiers: BTreeSet<String> = BTreeSet::new();

    for region in SIGNED_REGIONS {
        let signing = Signing::SignedFor(region);
        let reply = emulator
            .describe_endpoint(signing, Some("iot:Data-ATS"))
            .await;
        assert_eq!(
            reply.status,
            200,
            "region {region}: expected 200, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.endpoint_address(),
            served,
            "region {region}: expected the served address {served:?}, got {:?}",
            reply.endpoint_address()
        );
        assert_addresses_the_emulator(region, &reply.endpoint_address());
        let identifier = emulator
            .recorded_identifier(region)
            .unwrap_or_else(|| panic!("region {region}: no endpoint was recorded"));
        assert!(
            identifiers.insert(identifier.clone()),
            "region {region}: identifier {identifier} is already used by another region"
        );
    }

    // A request signed for one region but addressed to another region's endpoint host,
    // and a request whose region cannot be read from a credential scope at all: the
    // address is the served one in every case, and the record is keyed by the signed
    // region (or the default region when there is none).
    let foreign = emulator
        .describe_endpoint(Signing::SignedForOnForeignHost("sa-east-1"), None)
        .await;
    assert_eq!(
        foreign.status,
        200,
        "case signed for sa-east-1 on an {FALLBACK_HOST_REGION} host: expected 200, got {} body {}",
        foreign.status,
        foreign.text()
    );
    assert_eq!(
        foreign.endpoint_address(),
        served,
        "case signed for sa-east-1 on an {FALLBACK_HOST_REGION} host: expected {served:?}, got {:?}",
        foreign.endpoint_address()
    );
    assert!(
        emulator.recorded_identifier("sa-east-1").is_some(),
        "case signed for sa-east-1 on a foreign host: no endpoint was recorded for the \
         signed region"
    );

    for signing in [Signing::Unparseable, Signing::Anonymous] {
        let reply = emulator.describe_endpoint(signing, None).await;
        assert_eq!(
            reply.status,
            200,
            "case {signing:?}: expected 200, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.endpoint_address(),
            served,
            "case {signing:?}: expected the served address {served:?}, got {:?}",
            reply.endpoint_address()
        );
        assert!(
            emulator.recorded_identifier(DEFAULT_REGION).is_some(),
            "case {signing:?}: no endpoint was recorded for the default region"
        );
    }
}

/// The address is where this emulator is SERVED, not something rendered from state: a
/// record seeded with another identifier changes nothing, and two emulators served at
/// two addresses answer with their own.
#[tokio::test]
async fn the_published_address_is_not_rendered_from_the_endpoint_record() {
    // Two seeded records, so no single hardcoded value can satisfy both rows.
    const SEEDED: &[(&str, &str)] = &[
        ("us-east-1", "seededprefix1"),
        ("ap-south-1", "zz9examplepre0"),
    ];

    for (region, seeded) in SEEDED {
        for (base, expected) in SERVING_CASES {
            let emulator = Emulator::served_at(base);
            emulator
                .state()
                .lock()
                .endpoint_identifiers
                .insert((*region).to_string(), (*seeded).to_string());

            for type_case in TYPE_CASES {
                let case = format!("{} in {region} served at {base}", type_case.name);
                let reply = emulator
                    .describe_endpoint(Signing::SignedFor(region), type_case.endpoint_type)
                    .await;
                assert_eq!(
                    reply.status,
                    200,
                    "case {case}: expected 200, got {} body {}",
                    reply.status,
                    reply.text()
                );
                let address = reply.endpoint_address();
                assert_eq!(
                    &address, expected,
                    "case {case}: expected the served address {expected:?}, got {address:?}"
                );
                assert!(
                    !address.contains(seeded),
                    "case {case}: the published address carries the recorded identifier \
                     {seeded:?}, got {address:?}"
                );
            }
            assert_eq!(
                emulator.recorded_identifier(region).as_deref(),
                Some(*seeded),
                "the recorded endpoint for {region} must survive a DescribeEndpoint call"
            );
        }
    }
}

/// An `endpointType` outside the documented catalog is refused with the modelled
/// `InvalidRequestException` (400), and the refusal creates no endpoint.
#[tokio::test]
async fn an_endpoint_type_outside_the_catalog_is_refused() {
    // 128 characters is the member's modelled maximum length, so this value reaches
    // the handler and is refused there rather than at the boundary.
    let longest_valid_length = "x".repeat(128);
    let cases: Vec<(&str, String)> = vec![
        ("an empty endpointType", String::new()),
        ("a type that is not a token", "Data".to_string()),
        ("a plausible but undocumented type", "iot:Data-TLS".to_string()),
        ("a type crossing two endpoints", "iot:Jobs-ATS".to_string()),
        ("a token with inner whitespace", "iot: Jobs".to_string()),
        ("the longest value the model allows", longest_valid_length),
    ];

    for (name, endpoint_type) in &cases {
        let emulator = Emulator::new();
        let reply = emulator
            .describe_endpoint(Signing::SignedFor(REGION), Some(endpoint_type))
            .await;
        assert_eq!(
            reply.status,
            400,
            "case {name}: expected 400, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.error_code(),
            "InvalidRequestException",
            "case {name}: expected InvalidRequestException, got {}",
            reply.text()
        );
        assert_eq!(
            emulator.recorded_identifier(REGION),
            None,
            "case {name}: a refused call created an endpoint"
        );
    }

    // One past the modelled maximum length: refused before the handler runs, so the
    // status is what this asserts — the code belongs to the boundary validator.
    let emulator = Emulator::new();
    let too_long = "x".repeat(129);
    let reply = emulator
        .describe_endpoint(Signing::SignedFor(REGION), Some(&too_long))
        .await;
    assert_eq!(
        reply.status,
        400,
        "case endpointType one character past its 128-character bound: expected 400, got {} body {}",
        reply.status,
        reply.text()
    );
    assert_eq!(
        emulator.recorded_identifier(REGION),
        None,
        "case endpointType one character past its 128-character bound: a refused call created an endpoint"
    );
}

/// Past `MAX_ENDPOINT_REGIONS` the store stops recording new regions — and every region
/// is still told the address this emulator is served at, because that is where its
/// endpoint answers whether or not a record for it fit.
#[tokio::test]
async fn the_address_stays_stable_past_the_recorded_region_bound() {
    let emulator = Emulator::new();
    let served = service_common::data_plane_addressing::url_authority(SERVED_AT);
    // Enough distinct signed regions to cross the bound twice over.
    let regions: Vec<String> = (0..(MAX_ENDPOINT_REGIONS * 2))
        .map(|index| format!("zz-test-{index}"))
        .collect();

    for region in &regions {
        let reply = emulator
            .describe_endpoint(Signing::SignedFor(region), Some("iot:Jobs"))
            .await;
        assert_eq!(
            reply.status,
            200,
            "region {region}: expected 200, got {} body {}",
            reply.status,
            reply.text()
        );
        assert_eq!(
            reply.endpoint_address(),
            served,
            "region {region}: expected the served address {served:?}, got {:?}",
            reply.endpoint_address()
        );
    }

    assert!(
        emulator.state().lock().endpoint_identifiers.len() <= MAX_ENDPOINT_REGIONS,
        "the recorded regions must stay within MAX_ENDPOINT_REGIONS, got {}",
        emulator.state().lock().endpoint_identifiers.len()
    );

    // Every region — recorded or not — answers with the same address on a later call.
    for region in &regions {
        let again = emulator
            .describe_endpoint(Signing::SignedFor(region), Some("iot:Jobs"))
            .await;
        assert_eq!(
            again.endpoint_address(),
            served,
            "region {region}: the endpoint changed after the recorded-region bound was reached"
        );
    }
}
