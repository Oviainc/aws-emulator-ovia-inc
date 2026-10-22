// SPDX-License-Identifier: CC0-1.0

//! WHICH requests this service claims, over the `MultiServiceRouter` front door.
//!
//! The rule under test: a request that NAMES this service — by a credential scope of any
//! SigV4-family flavour (the region-bearing `AWS4-HMAC-SHA256` one, the region-less
//! `AWS4-ECDSA-P256-SHA256` one SigV4A signs with, carried in the `Authorization` header
//! or in a presigned URL's `X-Amz-Credential`), or by a host-style endpoint of the forms
//! this service's endpoint rules publish — reaches the operation its modelled route names
//! and is answered by that operation: its modelled output, or its modelled error. A
//! request that names a DIFFERENT service reaches nobody, which the router answers
//! `404 UnknownService` — a code this service's model does not declare, and therefore
//! never a correct answer to a request that named this service.
//!
//! The model is the source of both halves: `smithy_query(query="service_info")` reports
//! `authSchemes ["sigv4", "sigv4a"]` with `smithy.api#auth` listing `aws.auth#sigv4a`
//! FIRST (so an SDK signs this service's requests with SigV4A), and
//! `endpointResolution.urlTemplates`
//! `https://api.sustainability.{Region}.{…}` / `https://sustainability.global.{…}` /
//! `https://sustainability-fips.global.{…}` / `https://sustainability.us-gov.{…}`.
//!
//! Every table row is an addressing form; the four modelled routes are walked for each
//! one, so no single request is the specification.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sustainability_awssustainabilityapiservice::state as sustainability;
use service_com_amazonaws_sustainability_awssustainabilityapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The four modelled routes with a request body that satisfies each one, and the output
/// member the operation answers with (`OPERATIONS.md`).
fn routes() -> Vec<(&'static str, &'static str, Value)> {
    let (start, end) = (
        sustainability::month_start_secs(sustainability::SEED_FIRST_YEAR, 1),
        sustainability::month_start_secs(sustainability::SEED_FIRST_YEAR, 7),
    );
    let period = json!({ "Start": start, "End": end });
    vec![
        (
            "/v1/estimated-carbon-emissions",
            "Results",
            json!({ "TimePeriod": period, "GroupBy": ["SERVICE"], "MaxResults": 5 }),
        ),
        (
            "/v1/estimated-carbon-emissions-dimension-values",
            "Results",
            json!({ "TimePeriod": period, "Dimensions": ["REGION"], "MaxResults": 5 }),
        ),
        (
            "/v1/estimated-water-allocation",
            "Results",
            json!({
                "TimePeriod": period,
                "Granularity": "YEARLY_CALENDAR",
                "GroupBy": ["REGION"],
                "MaxResults": 5
            }),
        ),
        (
            "/v1/estimated-water-allocation-dimension-values",
            "Results",
            json!({ "TimePeriod": period, "Dimensions": ["SERVICE"], "MaxResults": 5 }),
        ),
    ]
}

/// How one request addresses the emulator: the `Host` it is sent to, the request-URI
/// authority, and the `Authorization` header and query string it carries.
struct Addressing {
    name: &'static str,
    /// The authority in the request URI and in the `Host` header.
    host: &'static str,
    authorization: Option<&'static str>,
    /// Query string appended to the path, for the presigned forms.
    query: &'static str,
}

/// A SigV4A `Authorization` header: algorithm `AWS4-ECDSA-P256-SHA256` and a credential
/// scope with NO region segment (a SigV4A signature covers a region SET, which travels in
/// `X-Amz-Region-Set`). This is what an SDK configured for this service sends, because the
/// model lists `aws.auth#sigv4a` first.
const SIGV4A: &str = "AWS4-ECDSA-P256-SHA256 \
     Credential=AKIDEXAMPLE/20260101/sustainability/aws4_request, \
     SignedHeaders=host;x-amz-date;x-amz-region-set, Signature=0123456789abcdef";

/// The region-bearing SigV4 form, for comparison: same scope with a region segment.
const SIGV4: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/sustainability/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A SigV4A scope naming a different service: this service must NOT claim it.
const SIGV4A_OTHER_SERVICE: &str = "AWS4-ECDSA-P256-SHA256 \
     Credential=AKIDEXAMPLE/20260101/s3/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// The loopback address an endpoint-pinned client reaches the emulator on: it names no
/// service, so such a request routes on its credential scope alone.
const LOOPBACK: &str = "127.0.0.1:9297";

/// Addressing forms this service MUST claim.
const CLAIMED: &[Addressing] = &[
    Addressing {
        name: "SigV4A-signed (region-less scope), endpoint-pinned at the loopback address",
        host: LOOPBACK,
        authorization: Some(SIGV4A),
        query: "",
    },
    Addressing {
        name: "SigV4-signed (region-bearing scope), endpoint-pinned at the loopback address",
        host: LOOPBACK,
        authorization: Some(SIGV4),
        query: "",
    },
    Addressing {
        name: "presigned with a region-less SigV4A scope, no Authorization header",
        host: LOOPBACK,
        authorization: None,
        query: "?X-Amz-Algorithm=AWS4-ECDSA-P256-SHA256&X-Amz-Credential=\
                AKIDEXAMPLE%2F20260101%2Fsustainability%2Faws4_request",
    },
    Addressing {
        name: "presigned with a region-bearing SigV4 scope, no Authorization header",
        host: LOOPBACK,
        authorization: None,
        query: "?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=\
                AKIDEXAMPLE%2F20260101%2Fus-east-1%2Fsustainability%2Faws4_request",
    },
    // The host forms this service's endpoint rules resolve to, unsigned: the endpoint
    // alone names the service.
    Addressing {
        name: "the regional endpoint host, behind its api. prefix",
        host: "api.sustainability.us-east-1.api.aws",
        authorization: None,
        query: "",
    },
    Addressing {
        name: "the global endpoint host",
        host: "sustainability.global.api.aws",
        authorization: None,
        query: "",
    },
    Addressing {
        name: "the global FIPS endpoint host",
        host: "sustainability-fips.global.api.aws",
        authorization: None,
        query: "",
    },
    Addressing {
        name: "the us-gov endpoint host",
        host: "sustainability.us-gov.api.aws",
        authorization: None,
        query: "",
    },
    Addressing {
        name: "the regional endpoint host with an explicit port",
        host: "api.sustainability.eu-west-1.api.aws:443",
        authorization: None,
        query: "",
    },
];

/// Addressing forms this service must NOT claim: nothing in them names it.
const DECLINED: &[Addressing] = &[
    Addressing {
        name: "a SigV4A scope naming another service",
        host: LOOPBACK,
        authorization: Some(SIGV4A_OTHER_SERVICE),
        query: "",
    },
    Addressing {
        name: "another service's endpoint host",
        host: "api.ec2.us-east-1.api.aws",
        authorization: None,
        query: "",
    },
    Addressing {
        name: "the loopback address with no credential scope at all",
        host: LOOPBACK,
        authorization: None,
        query: "",
    },
    Addressing {
        name: "a scope whose terminator is not aws4_request",
        host: LOOPBACK,
        authorization: Some(
            "AWS4-ECDSA-P256-SHA256 Credential=AKIDEXAMPLE/20260101/sustainability/aws5_request, \
             Signature=0123456789abcdef",
        ),
        query: "",
    },
    Addressing {
        name: "an Authorization header that is not a SigV4-family signature",
        host: LOOPBACK,
        authorization: Some("Bearer Credential=AKIDEXAMPLE/20260101/sustainability/aws4_request"),
        query: "",
    },
];

/// A router with only this service mounted — one emulator "session". Request
/// authentication is off, so a signature value is never checked: what is under test is
/// which service CLAIMS the request.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
        Self { router }
    }

    /// POST `path` with `body`, addressed as `addressing` says; returns the status and the
    /// parsed response body.
    async fn post(&self, addressing: &Addressing, path: &str, body: &Value) -> (u16, Value) {
        let mut request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://{}{path}{}",
                addressing.host, addressing.query
            ))
            .header("host", addressing.host)
            .header("content-type", "application/json")
            .header("x-amz-date", "20260101T000000Z");
        if let Some(authorization) = addressing.authorization {
            request = request.header("authorization", authorization);
        }
        let request = request
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }
}

/// The wire error code of a failure response: the last segment of its `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Every addressing form that names this service reaches every one of its four modelled
/// routes and is answered by the operation, not by the router's `UnknownService`.
#[tokio::test]
async fn a_request_that_names_this_service_reaches_its_modelled_routes() {
    let emulator = Emulator::new();

    for addressing in CLAIMED {
        for (path, output_member, body) in routes() {
            let case = format!("{} -> {path}", addressing.name);
            let (status, parsed) = emulator.post(addressing, path, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the operation's modelled 200, got {status} with \
                 {parsed}"
            );
            assert!(
                parsed.get(output_member).is_some_and(Value::is_array),
                "case {case:?}: expected a {output_member} list in the response, got {parsed}"
            );
        }
    }
}

/// A request that names no service, or names another one, is claimed by nobody — so this
/// service's widened claim cannot swallow a peer's traffic.
#[tokio::test]
async fn a_request_that_does_not_name_this_service_is_not_claimed() {
    let emulator = Emulator::new();

    for addressing in DECLINED {
        for (path, _, body) in routes() {
            let case = format!("{} -> {path}", addressing.name);
            let (status, parsed) = emulator.post(addressing, path, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a request this service does not claim must reach nobody, got \
                 {status} with {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "UnknownService",
                "case {case:?}: expected the router's UnknownService, got {parsed}"
            );
        }
    }
}

/// A claimed request that breaks a modelled constraint is answered with this service's
/// own modelled error — `ValidationException` (HTTP 400) — for every addressing form and
/// every route, never with the router's `UnknownService`.
///
/// The rejected inputs are one per route, each outside a modelled value set or range: an
/// out-of-enum `Dimension`/`EmissionsType`/`WaterAllocationType`, an inverted
/// `TimePeriod`, a `Granularity` the operation does not support (`WATER_SUPPORTED_GRANULARITIES`
/// is `YEARLY_CALENDAR` alone), and a `MaxResults` past the modelled range
/// (`MAX_RESULTS_RANGE`).
#[tokio::test]
async fn a_claimed_request_outside_the_model_gets_the_modelled_validation_error() {
    let emulator = Emulator::new();
    let first = sustainability::month_start_secs(sustainability::SEED_FIRST_YEAR, 1);
    let later = sustainability::month_start_secs(sustainability::SEED_FIRST_YEAR, 7);
    let period = json!({ "Start": first, "End": later });
    let inverted = json!({ "Start": later, "End": first });
    let past_range = *sustainability::MAX_RESULTS_RANGE.end() + 1;

    // (case, route, body the model refuses)
    let refused: Vec<(&str, &str, Value)> = vec![
        (
            "an out-of-enum GroupBy dimension",
            "/v1/estimated-carbon-emissions",
            json!({ "TimePeriod": period, "GroupBy": ["NOT_A_REAL_DIMENSION"] }),
        ),
        (
            "an out-of-enum EmissionsType",
            "/v1/estimated-carbon-emissions",
            json!({ "TimePeriod": period, "EmissionsTypes": ["NOT_A_REAL_EMISSIONS_TYPE"] }),
        ),
        (
            "an inverted TimePeriod",
            "/v1/estimated-carbon-emissions",
            json!({ "TimePeriod": inverted }),
        ),
        (
            "an out-of-enum Dimensions entry",
            "/v1/estimated-carbon-emissions-dimension-values",
            json!({ "TimePeriod": period, "Dimensions": ["NOT_A_REAL_DIMENSION"] }),
        ),
        (
            "an inverted TimePeriod on the dimension-values route",
            "/v1/estimated-carbon-emissions-dimension-values",
            json!({ "TimePeriod": inverted, "Dimensions": ["REGION"] }),
        ),
        (
            "a granularity water allocation does not support",
            "/v1/estimated-water-allocation",
            json!({ "TimePeriod": period, "Granularity": "MONTHLY" }),
        ),
        (
            "an out-of-enum AllocationTypes entry",
            "/v1/estimated-water-allocation",
            json!({ "TimePeriod": period, "AllocationTypes": ["NOT_A_REAL_ALLOCATION_TYPE"] }),
        ),
        (
            "a MaxResults past the modelled range",
            "/v1/estimated-water-allocation-dimension-values",
            json!({ "TimePeriod": period, "Dimensions": ["REGION"], "MaxResults": past_range }),
        ),
    ];

    for addressing in CLAIMED {
        for (what, path, body) in &refused {
            let case = format!("{} -> {path}: {what}", addressing.name);
            let (status, parsed) = emulator.post(addressing, path, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the modelled ValidationException's 400, got {status} \
                 with {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {parsed}"
            );
        }
    }
}
