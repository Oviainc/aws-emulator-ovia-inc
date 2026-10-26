// SPDX-License-Identifier: CC0-1.0

//! Endpoint discovery: what `DescribeEndpoints` reports, that it names the endpoint
//! identity a client reads it for, and that what it names still reaches this process.
//!
//! The rule these tests pin:
//!
//! > endpoint discovery reports exactly one endpoint, whose `Address` is the endpoint of the
//! > REGION THE REQUEST ADDRESSES as this emulator serves it — this service's endpoint
//! > prefix, then that region, then the loopback name and the port this process serves, with
//! > no scheme and no path — with a positive cache TTL in minutes, whatever endpoint the
//! > request arrived on; and the answer is stable across an immediate repeat call.
//!
//! One form of arrival is one row of [`ARRIVALS`]; the rows differ along the FORM of the
//! endpoint a client reaches this emulator through (the regional host, its FIPS sibling, the
//! dual-stack endpoint as an HTTP/2 `:authority` with no `Host` header at all, the legacy
//! global host, loopback with a port, a bare name on a port of the caller's choosing, no
//! host at all, a `Host` that is not an address, a `Host` carrying a path) and along the
//! REGION each addresses — regions of six different AWS partitions — because the arrival is
//! what the reported address must NOT depend on and the region is the only thing it may.
//! [`the_address_does_not_depend_on_the_arrival`] states that independence directly, by
//! sending every arrival form for ONE region, and
//! [`the_reported_address_reaches_this_process_and_no_public_host`] pins the other half: an
//! address handed to a client is one that resolves to this machine, never a real AWS
//! endpoint host.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::emulator;

/// How one request names the endpoint it arrived on and the region it addresses.
struct Arrival {
    /// What this row is, for an assertion message.
    case: &'static str,
    /// The region the request's credential scope names, which is the region it addresses.
    region: &'static str,
    /// The `Host` header the request carries, or `None` for a request that carries none.
    host: Option<&'static str>,
    /// The absolute-form URI the request carries, whose authority is the endpoint the
    /// request arrived on when it carries no `Host` header; `None` for the origin form.
    authority_uri: Option<&'static str>,
}

/// Every form of endpoint a client reaches this emulator through, each addressing a region
/// of a different AWS partition (commercial, GovCloud, China and the four ISO partitions —
/// the partitions of the AWS SDKs' endpoint metadata), none of which may change the form of
/// the address reported back.
const ARRIVALS: &[Arrival] = &[
    Arrival {
        case: "the regional endpoint host of the region it is signed for",
        region: "us-west-2",
        host: Some("dynamodb.us-west-2.amazonaws.com"),
        authority_uri: None,
    },
    Arrival {
        case: "the FIPS regional endpoint host",
        region: "us-gov-west-1",
        host: Some("dynamodb-fips.us-gov-west-1.amazonaws.com"),
        authority_uri: None,
    },
    Arrival {
        case: "the dual-stack endpoint as an HTTP/2 :authority, with no Host header",
        region: "ap-southeast-2",
        host: None,
        authority_uri: Some("https://dynamodb.ap-southeast-2.api.aws/"),
    },
    Arrival {
        case: "the legacy global endpoint host",
        region: "eu-central-1",
        host: Some("dynamodb.amazonaws.com"),
        authority_uri: None,
    },
    Arrival {
        case: "loopback on the port the emulator serves, signed for a China region",
        region: "cn-north-1",
        host: Some("127.0.0.1:9297"),
        authority_uri: None,
    },
    Arrival {
        case: "a bare name on a port of the caller's choosing, signed for an ISO region",
        region: "us-iso-east-1",
        host: Some("dynamodb.test.internal:34567"),
        authority_uri: Some("https://dynamodb.test.internal:34567/"),
    },
    Arrival {
        case: "no Host header and no authority, signed for an ISO-B region",
        region: "us-isob-east-1",
        host: None,
        authority_uri: None,
    },
    Arrival {
        case: "a Host that is not an address at all, signed for an ISO-E region",
        region: "eu-isoe-west-1",
        host: Some("dynamodb us-east-1"),
        authority_uri: None,
    },
    Arrival {
        case: "a Host carrying a path, signed for an ISO-F region",
        region: "us-isof-south-1",
        host: Some("dynamodb.us-isof-south-1.csp.hci.ic.gov/shard"),
        authority_uri: None,
    },
];

/// The first label an endpoint host of this service carries: its `aws.api#service`
/// `endpointPrefix` (`smithy_query(query="service_info")` — `endpointPrefix: "dynamodb"`).
const ENDPOINT_PREFIX: &str = "dynamodb";

/// The last label of an address this emulator may report: the name reserved for the
/// loopback interface.
const LOOPBACK_ENDPOINT_NAME: &str = "localhost";

/// The one `Address` a request addressing `region` must be answered with: the service, the
/// region, the loopback name and the port this emulator serves.
fn expected_address(region: &str) -> String {
    format!(
        "{ENDPOINT_PREFIX}.{region}.{LOOPBACK_ENDPOINT_NAME}:{}",
        emulator_server::data_port_from_env()
    )
}

/// Send one operation that arrives on `host`/`authority_uri` signed for `region`, and
/// answer its HTTP status and parsed body.
async fn call_through(
    router: &MultiServiceRouter,
    region: &str,
    host: Option<&str>,
    authority_uri: Option<&str>,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(authority_uri.unwrap_or("/"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/dynamodb/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        );
    if let Some(host) = host {
        builder = builder.header("host", host);
    }
    let request = builder
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// The addresses a `DescribeEndpoints` body reports, in order, with every entry checked for
/// shape: a host and a port and nothing else (no scheme, no path), on the loopback name and
/// the port this process serves — never a host outside this machine — with a positive cache
/// TTL in minutes.
fn reported_addresses(case: &str, body: &Value) -> Vec<String> {
    let endpoints = body["Endpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Endpoints is a list, got {body}"));
    assert_eq!(
        endpoints.len(),
        1,
        "case {case:?}: the region this request addresses has one endpoint to discover, got \
         {body}"
    );
    let serving_port = emulator_server::data_port_from_env();
    endpoints
        .iter()
        .map(|endpoint| {
            let address = endpoint["Address"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: Address is a string, got {body}"))
                .to_string();
            assert!(
                !address.contains("://") && !address.contains('/') && !address.is_empty(),
                "case {case:?}: an Address is a host and a port, not a URL, got {address:?}"
            );
            let (host, port) = address
                .rsplit_once(':')
                .unwrap_or_else(|| panic!("case {case:?}: a host and a port, got {address:?}"));
            assert_eq!(
                port,
                serving_port.to_string(),
                "case {case:?}: the port this emulator serves, got {address:?}"
            );
            assert!(
                host.ends_with(&format!(".{LOOPBACK_ENDPOINT_NAME}")),
                "case {case:?}: an address a client is handed resolves to this machine, so it \
                 ends in the loopback name, got {address:?}"
            );
            let ttl = endpoint["CachePeriodInMinutes"].as_i64().unwrap_or_else(|| {
                panic!("case {case:?}: CachePeriodInMinutes is a long, got {body}")
            });
            assert!(
                ttl > 0,
                "case {case:?}: a cache TTL in minutes a client can honour, got {ttl}"
            );
            address
        })
        .collect()
}

/// The `CreateTable` body for a probe table whose only purpose is to be read back
/// through a discovered address.
fn probe_table(name: &str) -> Value {
    json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    })
}

#[tokio::test]
async fn discovery_reports_the_endpoint_of_the_region_addressed() {
    let router = emulator();

    for arrival in ARRIVALS {
        // One table in this row's region, so the address the answer names can be shown to
        // still place a request on this service, over THIS region's store.
        let probe = format!("Probe{}", arrival.region.replace('-', ""));
        let (status, created) = call_through(
            &router,
            arrival.region,
            arrival.host,
            arrival.authority_uri,
            "CreateTable",
            &probe_table(&probe),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: seeding {probe} expected HTTP 200, got {status} with {created}",
            arrival.case
        );

        let (status, body) = call_through(
            &router,
            arrival.region,
            arrival.host,
            arrival.authority_uri,
            "DescribeEndpoints",
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with {body}",
            arrival.case
        );
        let expected = expected_address(arrival.region);
        let addresses = reported_addresses(arrival.case, &body);
        assert_eq!(
            addresses,
            vec![expected.clone()],
            "case {:?}: the endpoint of {} as this emulator serves it, whatever endpoint the \
             request arrived on, expected {expected:?}, got {addresses:?}",
            arrival.case,
            arrival.region
        );

        for address in &addresses {
            // The reported address names the service and the region in the labels this
            // emulator's own routing reads them from, so a request sent to it reaches this
            // service over the same region's store — which is also what shows discovery
            // changed nothing a later caller sees, the probe table still being the one and
            // only table of that region.
            let (status, listed) = call_through(
                &router,
                arrival.region,
                Some(address.as_str()),
                None,
                "ListTables",
                &json!({}),
            )
            .await;
            assert_eq!(
                (status, listed["TableNames"].clone()),
                (200, json!([probe])),
                "case {:?}: the reported address {address:?} places a request on this service \
                 and {}'s store, got {status} with {listed}",
                arrival.case,
                arrival.region
            );
        }
    }
}

#[tokio::test]
async fn the_address_does_not_depend_on_the_arrival() {
    let router = emulator();
    // One region, every form of arrival: the reported address is a property of the region
    // the request addresses, so none of these may move it.
    let region = "eu-west-3";
    let expected = expected_address(region);
    let arrivals: [(&str, Option<&str>, Option<&str>); 7] = [
        (
            "the regional host",
            Some("dynamodb.eu-west-3.amazonaws.com"),
            None,
        ),
        (
            "the FIPS host",
            Some("dynamodb-fips.eu-west-3.amazonaws.com"),
            None,
        ),
        (
            "another region's host",
            Some("dynamodb.ap-south-1.amazonaws.com"),
            None,
        ),
        ("loopback on a port", Some("127.0.0.1:45283"), None),
        ("a bare name on a port", Some("emulator.internal:8080"), None),
        ("no host at all", None, None),
        (
            "an authority-form URI and no Host",
            None,
            Some("https://dynamodb.eu-west-3.api.aws/"),
        ),
    ];

    for (case, host, authority_uri) in arrivals {
        let (status, body) = call_through(
            &router,
            region,
            host,
            authority_uri,
            "DescribeEndpoints",
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
        let addresses = reported_addresses(case, &body);
        assert_eq!(
            addresses,
            vec![expected.clone()],
            "case {case:?}: the endpoint a request arrives on does not change the region's \
             endpoint, expected {expected:?}, got {addresses:?}"
        );
    }
}

#[tokio::test]
async fn the_reported_address_reaches_this_process_and_no_public_host() {
    let router = emulator();
    // Whatever region a request addresses, the address it is handed is one that resolves to
    // THIS machine: the labels carry the endpoint's identity, but the host is never a real
    // AWS endpoint, so a client that re-targets itself at a discovered endpoint cannot be
    // sent off this emulator with the credentials it is holding.
    //
    // The DNS suffixes ruled out here are those of the AWS partitions (the `dnsSuffix`
    // values of the AWS SDKs' endpoint metadata), one per partition.
    let partition_suffixes = [
        "amazonaws.com",
        "amazonaws.com.cn",
        "c2s.ic.gov",
        "sc2s.sgov.gov",
        "cloud.adc-e.uk",
        "csp.hci.ic.gov",
        "api.aws",
    ];
    for region in [
        "us-west-2",
        "cn-north-1",
        "us-gov-east-1",
        "us-iso-east-1",
        "us-isob-east-1",
        "eu-isoe-west-1",
        "us-isof-south-1",
    ] {
        let (status, body) = call_through(
            &router,
            region,
            Some(&format!("dynamodb.{region}.amazonaws.com")),
            None,
            "DescribeEndpoints",
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200, got {status} with {body}"
        );
        for address in reported_addresses(region, &body) {
            let host = address.rsplit_once(':').map_or(address.clone(), |(host, _)| host.to_string());
            for suffix in partition_suffixes {
                assert!(
                    !host.ends_with(suffix),
                    "case {region:?}: a reported address is never a public AWS endpoint host, \
                     but {address:?} ends in {suffix:?}"
                );
            }
            assert!(
                host.starts_with(&format!("{ENDPOINT_PREFIX}."))
                    && host.contains(&format!(".{region}."))
                    && host.ends_with(&format!(".{LOOPBACK_ENDPOINT_NAME}")),
                "case {region:?}: the address names this service and this region on the \
                 loopback name, got {address:?}"
            );
        }
    }
}

#[tokio::test]
async fn discovery_is_stable_across_calls_through_one_endpoint() {
    let router = emulator();
    // Two calls through the same endpoint report the same endpoints: a client that caches
    // what it was handed for the TTL it was handed is not being handed a moving target.
    for arrival in ARRIVALS {
        let mut answers = Vec::new();
        for _ in 0..2 {
            let (status, body) = call_through(
                &router,
                arrival.region,
                arrival.host,
                arrival.authority_uri,
                "DescribeEndpoints",
                &json!({}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected HTTP 200, got {status} with {body}",
                arrival.case
            );
            answers.push(body["Endpoints"].clone());
        }
        assert_eq!(
            answers[0], answers[1],
            "case {:?}: endpoint discovery is stable across reads, got {:?} then {:?}",
            arrival.case, answers[0], answers[1]
        );
    }
}

#[tokio::test]
async fn a_request_that_names_no_region_is_told_the_default_region_endpoint() {
    let router = emulator();
    // A request whose credential scope names nothing region-shaped, on no endpoint host,
    // addresses the region this emulator defaults to — and that region's endpoint is what
    // discovery reports, never an empty list and never a bare socket.
    let expected = expected_address("us-east-1");
    for scope in ["US_WEST_2", "not a region"] {
        let (status, body) =
            call_through(&router, scope, None, None, "DescribeEndpoints", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {scope:?}: expected HTTP 200, got {status} with {body}"
        );
        let addresses = reported_addresses(scope, &body);
        assert_eq!(
            addresses,
            vec![expected.clone()],
            "case {scope:?}: a request that addresses no region is served the default \
             region's endpoint, expected {expected:?}, got {addresses:?}"
        );
    }
}
