// SPDX-License-Identifier: CC0-1.0

//! Every pre-dispatch refusal this service answers wears the error identity its OWN model
//! declares — driven over the same `MultiServiceRouter` front door the production emulator
//! serves, with request authentication ON (`set_request_authentication(true)`, the
//! `--auth on` path).
//!
//! The rule: whatever makes the emulator refuse a caller before any handler runs, the
//! answer is `403` naming `AccessDeniedException` — the only 403 shape this model declares
//! (`errorKind client`, `@httpError 403`, required `Message` with `jsonName "message"`; the
//! service's `common_errors`, bound to all four operations) — carried in BOTH places
//! `restJson1` puts an error shape name, `x-amzn-errortype` and the body's `__type`, with
//! the required `message` present. The framework's own pre-dispatch code for a refused
//! caller is the generic `AccessDenied`, which this model does not declare and no SDK can
//! map onto a modelled exception, so naming the modelled shape is the whole point.
//!
//! The table's rows differ along the dimension the rule generalises over — WHY the caller
//! cannot be verified — and they span the space rather than one recorded case: where the
//! credential scope travels (`Authorization` header or a presigned query), what the scope
//! is shaped like (region-less, a terminator that is not `aws4_request`, a scope naming
//! another service), what the presigned validity says (elapsed, or longer than the maximum
//! a presigned URL may carry), which signing algorithm is named
//! (`AWS4-ECDSA-P256-SHA256`, which this model's `auth` order makes an SDK send), and a
//! header-signed request with no signing date at all. Each row is walked against all four
//! modelled routes, so no single request is the specification.
//!
//! Nothing here is signed: every row is a presentation the emulator's verifier rejects
//! BEFORE it compares a signature, which is what lets this suite span the refusal space
//! without a signer (the verifier's own accept path — a request header-signed with the
//! emulator's key verifies, whatever the serving signing name — is pinned by the framework
//! beside the verifier it belongs to). What this service does with a caller it DOES verify
//! is pinned, route by route, by `tests/all/signed_request_refusals.rs` and
//! `tests/all/request_claiming.rs`, which drive the same four routes with authentication
//! off: the modelled `200` with the required `Results`, or the modelled
//! `ValidationException` for an input outside the model.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sustainability_awssustainabilityapiservice::state as sustainability;
use service_com_amazonaws_sustainability_awssustainabilityapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The four modelled routes (`OPERATIONS.md`; `smithy_query(query="digest")`).
const CARBON: &str = "/v1/estimated-carbon-emissions";
const CARBON_DIMENSIONS: &str = "/v1/estimated-carbon-emissions-dimension-values";
const WATER: &str = "/v1/estimated-water-allocation";
const WATER_DIMENSIONS: &str = "/v1/estimated-water-allocation-dimension-values";

/// The modelled 403 shape of this service — what every row below must see.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// The loopback address an endpoint-pinned client reaches the emulator on: it names no
/// service, so such a request is claimed by its credential scope alone.
const LOOPBACK: &str = "127.0.0.1:9297";

/// One host this service's endpoint rules resolve to (`endpointResolution.urlTemplates`:
/// `https://api.sustainability.{Region}.{dualStackDnsSuffix}`): it names the service, so a
/// request whose scope names something else — or nothing — still reaches this service and
/// is refused by authentication rather than left unclaimed.
const ENDPOINT_HOST: &str = "api.sustainability.us-east-1.api.aws";

/// The longest validity AWS accepts for a presigned URL, in seconds (seven days): a URL
/// that claims more is refused on that ground alone.
const MAX_PRESIGNED_VALIDITY_SECS: u64 = 604_800;

/// A router with only this service mounted and request authentication ON, so every request
/// goes through the same verification the `--auth on` front door performs.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(true);
        router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
        Self { router }
    }

    /// POST `path` with `body`, addressed as `presentation` says; returns the status, the
    /// `x-amzn-errortype` header and the parsed body.
    async fn post(&self, presentation: &Presentation, path: &str, body: &Value) -> (u16, String, Value) {
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://{host}{path}{query}",
                host = presentation.addressed_to,
                query = presentation.query,
            ))
            .header("host", presentation.addressed_to)
            .header("content-type", "application/json");
        for (name, value) in presentation.headers() {
            builder = builder.header(name, value);
        }
        let request = builder
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let errortype = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, errortype, parsed)
    }
}

/// How one caller presents its credentials: the host it addresses, an `Authorization`
/// header when it carries one, a signing date when it sends one, and the query string a
/// presigned caller carries its scope in.
struct Presentation {
    what: &'static str,
    addressed_to: &'static str,
    authorization: Option<String>,
    /// `x-amz-date`, which a header-signed request must carry for its signature to be read
    /// at all.
    amz_date: Option<&'static str>,
    query: String,
}

impl Presentation {
    fn headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = Vec::new();
        if let Some(authorization) = &self.authorization {
            headers.push(("authorization", authorization.clone()));
        }
        if let Some(date) = self.amz_date {
            headers.push(("x-amz-date", date.to_string()));
        }
        headers
    }
}

/// A fixed signing date, as SigV4 writes it, and its credential-scope date.
const AMZ_DATE: &str = "20260101T000000Z";
const SCOPE_DATE: &str = "20260101";

/// A header-signed presentation carrying `scope` and `algorithm`, with no valid signature:
/// the verifier reads the scope and the algorithm before it ever compares one.
fn header_signed(
    what: &'static str,
    addressed_to: &'static str,
    algorithm: &str,
    scope: &str,
    amz_date: Option<&'static str>,
) -> Presentation {
    Presentation {
        what,
        addressed_to,
        authorization: Some(format!(
            "{algorithm} Credential={scope}, SignedHeaders=host;x-amz-date, \
             Signature=0123456789abcdef"
        )),
        amz_date,
        query: String::new(),
    }
}

/// A presigned presentation: the same scope fields in the query string, where a presigned
/// URL carries them (`/` percent-encoded, as a URL carries it), with a validity.
fn presigned(
    what: &'static str,
    addressed_to: &'static str,
    scope: &str,
    signed_at: &str,
    validity_secs: u64,
) -> Presentation {
    Presentation {
        what,
        addressed_to,
        authorization: None,
        amz_date: None,
        query: format!(
            "?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={scope}&X-Amz-Date={signed_at}\
             &X-Amz-Expires={validity_secs}&X-Amz-SignedHeaders=host\
             &X-Amz-Signature=0123456789abcdef",
            scope = scope.replace('/', "%2F"),
        ),
    }
}

/// `at` in `YYYYMMDDTHHMMSSZ`, the format SigV4 writes a signing date in, rendered through
/// the SDK's own timestamp type rather than any date arithmetic written here (as
/// `src/state.rs` does for this crate's own civil-date work).
fn amz_date(at: SystemTime) -> String {
    let text = aws_smithy_types::DateTime::from(at)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("an instant renders as a date-time");
    // `Format::DateTime` is RFC 3339 (`2026-01-01T00:00:00.123Z`); SigV4 writes the same
    // instant without separators or a fraction.
    let whole_seconds = text.split('.').next().unwrap_or(&text);
    let compact: String = whole_seconds
        .chars()
        .filter(|character| character.is_ascii_digit() || *character == 'T')
        .collect();
    format!("{compact}Z")
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

/// A well-formed body for each of the four modelled routes, so a row is refused for the way
/// it presents its credentials and never for its input.
fn accepted_bodies() -> Vec<(&'static str, Value)> {
    let (start, end) = (
        sustainability::month_start_secs(sustainability::SEED_FIRST_YEAR, 1),
        sustainability::month_start_secs(sustainability::SEED_FIRST_YEAR, 7),
    );
    let period = json!({ "Start": start, "End": end });
    vec![
        (
            CARBON,
            json!({ "TimePeriod": period, "GroupBy": ["SERVICE"], "MaxResults": 5 }),
        ),
        (
            CARBON_DIMENSIONS,
            json!({ "TimePeriod": period, "Dimensions": ["REGION"], "MaxResults": 5 }),
        ),
        (
            WATER,
            json!({
                "TimePeriod": period,
                "Granularity": "YEARLY_CALENDAR",
                "GroupBy": ["REGION"],
                "MaxResults": 5
            }),
        ),
        (
            WATER_DIMENSIONS,
            json!({ "TimePeriod": period, "Dimensions": ["SERVICE"], "MaxResults": 5 }),
        ),
    ]
}

/// Every way a caller can fail verification, each addressed so that THIS service claims the
/// request (by its credential scope, or by its own endpoint host when the scope names
/// something else).
fn presentations() -> Vec<Presentation> {
    let scope = format!("AKIDEXAMPLE/{SCOPE_DATE}/us-east-1/sustainability/aws4_request");
    // A window that really has closed, rather than one merely written in the past.
    let hour_ago = amz_date(SystemTime::now() - Duration::from_secs(3600));
    vec![
        // The algorithm this model's `auth` order makes an SDK send. The emulator's only
        // authenticator reads `AWS4-HMAC-SHA256`, so this caller is refused before dispatch
        // — a vendor-level gap this issue DECLINES (a SigV4A authenticator has to land in
        // the core library beside the SigV4 one; see the `## Resolution` of
        // `20261022T232611Z-sustainability-all-4-operations-refused-403-accessdenied-sign`).
        // When it does, this row's expectation becomes the operation's 200: FLIP it, never
        // loosen it.
        header_signed(
            "an AWS4-ECDSA-P256-SHA256 Authorization the authenticator cannot read",
            LOOPBACK,
            "AWS4-ECDSA-P256-SHA256",
            &format!("AKIDEXAMPLE/{SCOPE_DATE}/sustainability/aws4_request"),
            Some(AMZ_DATE),
        ),
        // A SigV4 scope with no region segment: shaped like a SigV4A scope, named under the
        // algorithm the authenticator does read.
        header_signed(
            "a region-less credential scope under AWS4-HMAC-SHA256",
            LOOPBACK,
            "AWS4-HMAC-SHA256",
            &format!("AKIDEXAMPLE/{SCOPE_DATE}/sustainability/aws4_request"),
            Some(AMZ_DATE),
        ),
        // A scope that does not end in the SigV4 terminator: claimed by the endpoint host,
        // since the scope cannot name the service.
        header_signed(
            "a credential scope whose terminator is not aws4_request",
            ENDPOINT_HOST,
            "AWS4-HMAC-SHA256",
            &format!("AKIDEXAMPLE/{SCOPE_DATE}/us-east-1/sustainability/aws5_request"),
            Some(AMZ_DATE),
        ),
        // Signed for another service, addressed to THIS service's endpoint: a credential
        // scoped elsewhere cannot be spent here.
        header_signed(
            "a credential scope naming another service",
            ENDPOINT_HOST,
            "AWS4-HMAC-SHA256",
            &format!("AKIDEXAMPLE/{SCOPE_DATE}/us-east-1/s3/aws4_request"),
            Some(AMZ_DATE),
        ),
        // An access key this emulator never minted: nothing in its credential store holds
        // the secret such a signature would need.
        header_signed(
            "an access key the emulator never minted",
            LOOPBACK,
            "AWS4-HMAC-SHA256",
            &scope,
            Some(AMZ_DATE),
        ),
        // Header-signed with no signing date: a SigV4 signature cannot even be read without
        // the instant it was made at.
        header_signed(
            "header-signed with no x-amz-date",
            LOOPBACK,
            "AWS4-HMAC-SHA256",
            &scope,
            None,
        ),
        // The presigned carrier, with the scope in the query: valid window, unknown key.
        presigned(
            "presigned with an access key the emulator never minted",
            LOOPBACK,
            &scope,
            AMZ_DATE,
            600,
        ),
        // The same presigned carrier whose window has closed. The ground is
        // over-determined — this scope's key was never minted either — which is why the two
        // validity rows sit beside the valid-window row above: what they add is that a
        // presigned URL is refused on its validity at all, and all three must wear the same
        // modelled identity.
        presigned(
            "a presigned URL whose validity has elapsed",
            LOOPBACK,
            &scope,
            &hour_ago,
            60,
        ),
        // A presigned URL claiming more validity than AWS accepts at all (same
        // over-determination).
        presigned(
            "a presigned URL claiming more than the maximum validity",
            LOOPBACK,
            &scope,
            AMZ_DATE,
            MAX_PRESIGNED_VALIDITY_SECS + 1,
        ),
        // Nothing at all, on this service's own endpoint host.
        Presentation {
            what: "unsigned, on this service's endpoint host",
            addressed_to: ENDPOINT_HOST,
            authorization: None,
            amz_date: None,
            query: String::new(),
        },
    ]
}

#[tokio::test]
async fn every_refused_caller_gets_this_services_modelled_access_denied_error() {
    let emulator = Emulator::new();

    for presentation in presentations() {
        for (path, body) in accepted_bodies() {
            let case = format!("{} -> {path}", presentation.what);
            let (status, errortype, parsed) = emulator.post(&presentation, path, &body).await;
            assert_eq!(
                status, 403,
                "case {case:?}: expected the modelled AccessDeniedException's 403, got \
                 {status} {errortype:?} {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                ACCESS_DENIED,
                "case {case:?}: expected the modelled {ACCESS_DENIED} in __type, got {parsed}"
            );
            assert_eq!(
                errortype, ACCESS_DENIED,
                "case {case:?}: restJson1 carries the error shape name in x-amzn-errortype, \
                 got {errortype:?} with {parsed}"
            );
            assert!(
                parsed
                    .get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: the modelled AccessDeniedException requires a message member, \
                 got {parsed}"
            );
            assert!(
                !parsed.to_string().contains("\"AccessDenied\"")
                    && errortype != "AccessDenied"
                    && errortype != "SignatureDoesNotMatch",
                "case {case:?}: no unmodelled framework code may reach the caller, got \
                 {errortype:?} with {parsed}"
            );
        }
    }
}
