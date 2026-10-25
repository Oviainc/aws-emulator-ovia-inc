// SPDX-License-Identifier: CC0-1.0

//! Endpoint discovery: what `DescribeEndpoints` reports, and that what it reports is an
//! address a client can keep calling.
//!
//! The rule these tests pin:
//!
//! > endpoint discovery reports exactly one endpoint, whose `Address` is a bare host with
//! > an optional port (no scheme, no path) — the endpoint THIS request arrived on — with a
//! > positive cache TTL in minutes, whatever region the request addresses; and a request
//! > sent back to that address is served by this same emulator, over that region's store.
//!
//! One form of arrival is one row of [`ARRIVALS`]; the rows differ along the FORM of the
//! endpoint a client reaches this emulator through (the regional host, its FIPS sibling,
//! the dual-stack endpoint as an HTTP/2 `:authority` with no `Host` header at all, the
//! legacy global host, loopback with a port, a bare name on a port of the caller's
//! choosing) and along the region each addresses, because the form of the arrival is what
//! the rule generalises over and the region is what it must NOT depend on. The arrivals a
//! row cannot express have their own tests: an origin-form request carrying neither `Host`
//! nor authority, and a `Host` that is not an address at all.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::emulator;

/// How one request names the endpoint it arrived on and the region it addresses, and the
/// address discovery must therefore report back.
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
    /// The one `Address` the answer must carry.
    expected: &'static str,
}

/// Every form of endpoint a client reaches this emulator through, each addressing a
/// different region.
const ARRIVALS: &[Arrival] = &[
    Arrival {
        case: "the regional endpoint host of the region it is signed for",
        region: "us-east-1",
        host: Some("dynamodb.us-east-1.amazonaws.com"),
        authority_uri: None,
        expected: "dynamodb.us-east-1.amazonaws.com",
    },
    Arrival {
        case: "the FIPS regional endpoint host",
        region: "us-gov-west-1",
        host: Some("dynamodb-fips.us-gov-west-1.amazonaws.com"),
        authority_uri: None,
        expected: "dynamodb-fips.us-gov-west-1.amazonaws.com",
    },
    Arrival {
        case: "the dual-stack endpoint as an HTTP/2 :authority, with no Host header",
        region: "ap-southeast-2",
        host: None,
        authority_uri: Some("https://dynamodb.ap-southeast-2.api.aws/"),
        expected: "dynamodb.ap-southeast-2.api.aws",
    },
    Arrival {
        case: "the legacy global endpoint host",
        region: "eu-central-1",
        host: Some("dynamodb.amazonaws.com"),
        authority_uri: None,
        expected: "dynamodb.amazonaws.com",
    },
    Arrival {
        case: "loopback on the port the emulator serves",
        region: "us-west-2",
        host: Some("127.0.0.1:9297"),
        authority_uri: None,
        expected: "127.0.0.1:9297",
    },
    Arrival {
        case: "a bare name on a port of the caller's choosing",
        region: "sa-east-1",
        host: Some("dynamodb.test.internal:34567"),
        authority_uri: Some("https://dynamodb.test.internal:34567/"),
        expected: "dynamodb.test.internal:34567",
    },
];

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

/// The addresses a `DescribeEndpoints` body reports, in order, with every entry checked
/// for shape: a bare host (no scheme, no path) and a positive cache TTL in minutes.
fn reported_addresses(case: &str, body: &Value) -> Vec<String> {
    let endpoints = body["Endpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: Endpoints is a list, got {body}"));
    assert_eq!(
        endpoints.len(),
        1,
        "case {case:?}: the endpoint serving this request is the one endpoint there is to \
         discover, got {body}"
    );
    endpoints
        .iter()
        .map(|endpoint| {
            let address = endpoint["Address"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: Address is a string, got {body}"))
                .to_string();
            assert!(
                !address.contains("://") && !address.contains('/') && !address.is_empty(),
                "case {case:?}: an Address is a host and an optional port, not a URL, got \
                 {address:?}"
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
async fn discovery_reports_the_endpoint_the_request_arrived_on() {
    let router = emulator();

    for arrival in ARRIVALS {
        // One table in this row's region, so the address the answer names can be shown
        // to reach this emulator AND this region's store.
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
        let addresses = reported_addresses(arrival.case, &body);
        assert_eq!(
            addresses,
            vec![arrival.expected.to_string()],
            "case {:?}: the endpoint this request arrived on, whatever region it \
             addresses, expected {:?}, got {addresses:?}",
            arrival.case,
            arrival.expected
        );

        for address in &addresses {
            // The reported address is one a client can keep calling: a request sent to
            // it is served by this same emulator, over the same region's store — which is
            // also what shows discovery changed nothing a later caller sees, the probe
            // table still being the one and only table of that region.
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
                "case {:?}: the reported address {address:?} reaches this emulator and \
                 {}'s store, got {status} with {listed}",
                arrival.case,
                arrival.region
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
async fn a_host_that_is_not_an_address_is_not_reported_as_one() {
    let router = emulator();
    // A `Host` header is the caller's own value, so a value that is not a host with an
    // optional port names no endpoint: each row is one way of not being one, and every
    // one of them falls through to loopback on the port this emulator serves rather than
    // being echoed back as an `Address`.
    let loopback = format!("127.0.0.1:{}", emulator_server::data_port_from_env());
    let rows: [(&str, &str); 6] = [
        ("a path", "dynamodb.us-east-1.amazonaws.com/shard"),
        ("a scheme", "https://dynamodb.us-east-1.amazonaws.com"),
        ("empty", ""),
        ("whitespace only", "   "),
        ("userinfo", "admin@dynamodb.us-east-1.amazonaws.com"),
        ("a space inside the host", "dynamodb us-east-1"),
    ];

    for (case, host) in rows {
        let (status, body) = call_through(
            &router,
            "us-east-1",
            Some(host),
            None,
            "DescribeEndpoints",
            &json!({}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?} ({host:?}): expected HTTP 200, got {status} with {body}"
        );
        let addresses = reported_addresses(case, &body);
        assert_eq!(
            addresses,
            vec![loopback.clone()],
            "case {case:?} ({host:?}): a value that is not an address falls through to the \
             address that reaches this process, got {addresses:?}"
        );
    }
}

#[tokio::test]
async fn an_origin_form_request_is_told_an_address_that_reaches_this_process() {
    let router = emulator();
    // Neither a `Host` header nor a URI authority: the address still has to be one that
    // reaches THIS process, never an empty list and never a name that resolves elsewhere.
    let (status, body) =
        call_through(&router, "us-east-1", None, None, "DescribeEndpoints", &json!({})).await;
    assert_eq!(
        status, 200,
        "an origin-form request: expected HTTP 200, got {status} with {body}"
    );
    let addresses = reported_addresses("an origin-form request", &body);
    assert_eq!(
        addresses,
        vec![format!(
            "127.0.0.1:{}",
            emulator_server::data_port_from_env()
        )],
        "an origin-form request: loopback on the port this emulator serves is the only \
         other address that reaches it, got {addresses:?}"
    );
}
