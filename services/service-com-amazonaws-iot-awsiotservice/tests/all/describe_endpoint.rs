// SPDX-License-Identifier: CC0-1.0

//! `DescribeEndpoint`, end to end over HTTP.
//!
//! Every test drives real requests through the `MultiServiceRouter` this crate's
//! service is mounted in, so routing, the restJson1 `GET /endpoint` binding with its
//! `endpointType` query parameter, the handler and the response serialization are all
//! exercised.
//!
//! The rules under test, none of them specific to one endpoint type or one region:
//!
//! * an account has ONE endpoint identifier per region — created by the first call
//!   there, recorded in this service's state, and reused by every later call — and
//!   every endpoint type's address is rendered from that one identifier;
//! * the address names the region the request was signed for, and a request with no
//!   readable credential scope gets the emulator's default region;
//! * the address is computed from the recorded identifier, not from the request: a
//!   different recorded identifier produces a different address;
//! * an `endpointType` outside the catalog the model documents is refused with the
//!   modelled `InvalidRequestException` (400) and creates no endpoint.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use service_com_amazonaws_iot_awsiotservice::state::{
    ComAmazonawsIotAwsiotserviceState, ENDPOINT_IDENTIFIER_LENGTH, MAX_ENDPOINT_REGIONS,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region used wherever a test is not varying the region. Deliberately NOT the
/// default region, so an assertion about the signed region cannot pass by accident
/// through the fallback.
const REGION: &str = "eu-west-2";

/// The region `DescribeEndpoint` falls back to when the request carries no readable
/// SigV4 credential scope (`state::DEFAULT_REGION`).
const DEFAULT_REGION: &str = "us-east-1";

/// The endpoint host region of a request whose region cannot be read from a
/// credential scope. It is neither [`REGION`] nor [`DEFAULT_REGION`], so the
/// fallback assertion also proves the address is not taken from the request host.
const FALLBACK_HOST_REGION: &str = "ap-northeast-1";

/// One endpoint-type case: what the request asks for, and the address form the AWS
/// IoT Developer Guide publishes for it. `{identifier}` is the account's endpoint
/// identifier and `{region}` the signed region.
struct TypeCase {
    name: &'static str,
    endpoint_type: Option<&'static str>,
    address: &'static str,
}

/// Every endpoint type the `endpointType` member documents (read with
/// `smithy_query(query="doc",shape="DescribeEndpointRequest",member="endpointType",
/// model_path="services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json")`),
/// plus the request that names none.
const TYPE_CASES: &[TypeCase] = &[
    TypeCase {
        name: "no endpointType at all",
        endpoint_type: None,
        address: "{identifier}.iot.{region}.amazonaws.com",
    },
    TypeCase {
        name: "iot:Data",
        endpoint_type: Some("iot:Data"),
        address: "{identifier}.iot.{region}.amazonaws.com",
    },
    TypeCase {
        name: "iot:Data-ATS",
        endpoint_type: Some("iot:Data-ATS"),
        address: "{identifier}-ats.iot.{region}.amazonaws.com",
    },
    TypeCase {
        name: "iot:CredentialProvider",
        endpoint_type: Some("iot:CredentialProvider"),
        address: "{identifier}.credentials.iot.{region}.amazonaws.com",
    },
    TypeCase {
        name: "iot:Jobs",
        endpoint_type: Some("iot:Jobs"),
        address: "{identifier}.jobs.iot.{region}.amazonaws.com",
    },
];

/// How a request is signed, which is where the address's region comes from.
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

/// One emulator: the shared engine plus a router with this service mounted.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
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
            // The host names a region the credential scope does not, so the address
            // can only come from the scope (or the fallback).
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
/// letter, which is the form of the account-specific prefix every endpoint address is
/// built from.
fn is_well_formed_identifier(identifier: &str) -> bool {
    identifier.len() == ENDPOINT_IDENTIFIER_LENGTH
        && identifier.starts_with(|c: char| c.is_ascii_lowercase())
        && identifier
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Render a case's address form for one identifier and region.
fn expected_address(form: &str, identifier: &str, region: &str) -> String {
    form.replace("{identifier}", identifier)
        .replace("{region}", region)
}

/// Every documented endpoint type is served, and all of an account's addresses in one
/// region are rendered from the SAME recorded identifier.
#[tokio::test]
async fn every_documented_endpoint_type_is_served_from_one_account_identifier() {
    // A region the issue did not cite, to keep the rule region-independent.
    let signing = Signing::SignedFor("eu-west-1");
    let region = "eu-west-1";
    let emulator = Emulator::new();

    for case in TYPE_CASES {
        let reply = emulator.describe_endpoint(signing, case.endpoint_type).await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected 200, got {} body {}",
            case.name,
            reply.status,
            reply.text()
        );
        let identifier = emulator.recorded_identifier(region).unwrap_or_else(|| {
            panic!(
                "case {}: the call recorded no endpoint identifier for {region}",
                case.name
            )
        });
        assert!(
            is_well_formed_identifier(&identifier),
            "case {}: expected a 14-character lowercase-alphanumeric label, got {identifier:?}",
            case.name
        );
        assert_eq!(
            reply.endpoint_address(),
            expected_address(case.address, &identifier, region),
            "case {}: unexpected endpointAddress {:?}",
            case.name,
            reply.endpoint_address()
        );
    }
}

/// The endpoint is created once: the first call records an identifier and every later
/// call — of any type — answers from that record.
#[tokio::test]
async fn the_endpoint_is_created_once_and_every_later_call_returns_it() {
    let emulator = Emulator::new();
    let signing = Signing::SignedFor(REGION);

    assert_eq!(
        emulator.recorded_identifier(REGION),
        None,
        "no endpoint may exist before the first DescribeEndpoint call"
    );

    let first = emulator.describe_endpoint(signing, None).await;
    assert_eq!(
        first.status, 200,
        "the first call must succeed, got {} body {}",
        first.status,
        first.text()
    );
    let created = emulator
        .recorded_identifier(REGION)
        .expect("the first call must create the account's endpoint identifier");

    // Repeat every type several times: the address may differ in form per type, but
    // the identifier in it never changes.
    for round in 0..3 {
        for case in TYPE_CASES {
            let reply = emulator.describe_endpoint(signing, case.endpoint_type).await;
            assert_eq!(
                reply.status, 200,
                "case {} round {round}: expected 200, got {} body {}",
                case.name,
                reply.status,
                reply.text()
            );
            assert_eq!(
                reply.endpoint_address(),
                expected_address(case.address, &created, REGION),
                "case {} round {round}: the endpoint changed between calls, got {:?}",
                case.name,
                reply.endpoint_address()
            );
            assert_eq!(
                emulator.recorded_identifier(REGION).as_deref(),
                Some(created.as_str()),
                "case {} round {round}: a later call replaced the created endpoint",
                case.name
            );
        }
    }
}

/// The address names the region the request was signed for, each region has its own
/// identifier, and a request with no readable credential scope gets the default
/// region.
#[tokio::test]
async fn the_address_names_the_signed_region_and_each_region_has_its_own_identifier() {
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
    let mut identifiers: BTreeSet<String> = BTreeSet::new();

    for region in SIGNED_REGIONS {
        let signing = Signing::SignedFor(region);
        let reply = emulator.describe_endpoint(signing, Some("iot:Data-ATS")).await;
        assert_eq!(
            reply.status, 200,
            "region {region}: expected 200, got {} body {}",
            reply.status,
            reply.text()
        );
        let identifier = emulator
            .recorded_identifier(region)
            .unwrap_or_else(|| panic!("region {region}: no endpoint identifier was recorded"));
        assert_eq!(
            reply.endpoint_address(),
            format!("{identifier}-ats.iot.{region}.amazonaws.com"),
            "region {region}: unexpected endpointAddress {:?}",
            reply.endpoint_address()
        );
        assert!(
            identifiers.insert(identifier.clone()),
            "region {region}: identifier {identifier} is already used by another region"
        );
        // Asking again in the same region returns the same address.
        let again = emulator.describe_endpoint(signing, Some("iot:Data-ATS")).await;
        assert_eq!(
            again.endpoint_address(),
            reply.endpoint_address(),
            "region {region}: a repeated call returned a different endpoint"
        );
    }

    // A request signed for one region but addressed to another region's endpoint
    // host: the address names the SIGNED region, never the host.
    let foreign = emulator
        .describe_endpoint(Signing::SignedForOnForeignHost("sa-east-1"), None)
        .await;
    assert_eq!(
        foreign.status, 200,
        "case signed for sa-east-1 on an {FALLBACK_HOST_REGION} host: expected 200, got {} body {}",
        foreign.status,
        foreign.text()
    );
    let foreign_identifier = emulator
        .recorded_identifier("sa-east-1")
        .expect("case signed for sa-east-1 on a foreign host: no identifier was recorded");
    assert_eq!(
        foreign.endpoint_address(),
        format!("{foreign_identifier}.iot.sa-east-1.amazonaws.com"),
        "case signed for sa-east-1 on an {FALLBACK_HOST_REGION} host: unexpected endpointAddress {:?}",
        foreign.endpoint_address()
    );

    // A request whose region cannot be read from a credential scope is answered in
    // the emulator's default region — with that region's own identifier. The host
    // names FALLBACK_HOST_REGION, so this cannot pass by reading the host.
    for signing in [Signing::Unparseable, Signing::Anonymous] {
        let reply = emulator.describe_endpoint(signing, None).await;
        assert_eq!(
            reply.status, 200,
            "case {signing:?}: expected 200, got {} body {}",
            reply.status,
            reply.text()
        );
        let identifier = emulator
            .recorded_identifier(DEFAULT_REGION)
            .unwrap_or_else(|| panic!("case {signing:?}: no identifier for the default region"));
        assert_eq!(
            reply.endpoint_address(),
            format!("{identifier}.iot.{DEFAULT_REGION}.amazonaws.com"),
            "case {signing:?}: unexpected endpointAddress {:?}",
            reply.endpoint_address()
        );
    }
}

/// The address is computed from the RECORDED identifier, not from the request: a
/// record written with a different identifier changes every address in that region.
#[tokio::test]
async fn the_address_is_computed_from_the_recorded_identifier() {
    // Two seeded records, so no single hardcoded value can satisfy both rows.
    const SEEDED: &[(&str, &str)] = &[
        ("us-east-1", "seededprefix1"),
        ("ap-south-1", "zz9examplepre0"),
    ];

    for (region, seeded) in SEEDED {
        let emulator = Emulator::new();
        emulator
            .state()
            .lock()
            .endpoint_identifiers
            .insert((*region).to_string(), (*seeded).to_string());

        for case in TYPE_CASES {
            let reply = emulator
                .describe_endpoint(Signing::SignedFor(region), case.endpoint_type)
                .await;
            assert_eq!(
                reply.status, 200,
                "case {} in {region}: expected 200, got {} body {}",
                case.name,
                reply.status,
                reply.text()
            );
            assert_eq!(
                reply.endpoint_address(),
                expected_address(case.address, seeded, region),
                "case {} in {region}: the address ignored the recorded identifier, got {:?}",
                case.name,
                reply.endpoint_address()
            );
        }
        assert_eq!(
            emulator.recorded_identifier(region).as_deref(),
            Some(*seeded),
            "the recorded identifier for {region} must survive a DescribeEndpoint call"
        );
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
            reply.status, 400,
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
        reply.status, 400,
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

/// Past `MAX_ENDPOINT_REGIONS` the store stops recording new regions — and the
/// addresses stay stable anyway, because the identifier is a pure function of the
/// account and the region. This pins the claim the bound's documentation makes.
#[tokio::test]
async fn the_address_stays_stable_past_the_recorded_region_bound() {
    let emulator = Emulator::new();
    // Enough distinct signed regions to cross the bound twice over.
    let regions: Vec<String> = (0..(MAX_ENDPOINT_REGIONS * 2))
        .map(|index| format!("zz-test-{index}"))
        .collect();

    let mut first_addresses: Vec<String> = Vec::with_capacity(regions.len());
    for region in &regions {
        let reply = emulator
            .describe_endpoint(Signing::SignedFor(region), Some("iot:Jobs"))
            .await;
        assert_eq!(
            reply.status, 200,
            "region {region}: expected 200, got {} body {}",
            reply.status,
            reply.text()
        );
        let address = reply.endpoint_address();
        assert!(
            address.ends_with(&format!(".jobs.iot.{region}.amazonaws.com")),
            "region {region}: unexpected endpointAddress {address:?}"
        );
        first_addresses.push(address);
    }

    assert!(
        emulator.state().lock().endpoint_identifiers.len() <= MAX_ENDPOINT_REGIONS,
        "the recorded regions must stay within MAX_ENDPOINT_REGIONS, got {}",
        emulator.state().lock().endpoint_identifiers.len()
    );

    // Every region — recorded or not — answers with the same address it did first.
    for (region, first) in regions.iter().zip(&first_addresses) {
        let again = emulator
            .describe_endpoint(Signing::SignedFor(region), Some("iot:Jobs"))
            .await;
        assert_eq!(
            &again.endpoint_address(),
            first,
            "region {region}: the endpoint changed after the recorded-region bound was reached"
        );
    }
}
