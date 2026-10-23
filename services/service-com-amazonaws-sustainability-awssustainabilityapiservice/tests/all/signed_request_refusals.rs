// SPDX-License-Identifier: CC0-1.0

//! How this service answers a request it REFUSES, and what it answers one it serves —
//! driven over the same `MultiServiceRouter` front door the production emulator serves.
//!
//! Three rules, each asserted over a TABLE whose rows differ along what it generalises
//! over — the member, the kind of constraint, the value, the count and order of a list,
//! the route:
//!
//! 1. A request that reaches one of this service's four modelled routes is answered BY
//!    that operation: its modelled 200 with the required `Results` list present (empty
//!    when the request selects nothing), or one of its modelled errors — never the
//!    router's own `UnknownService`.
//! 2. A value outside what the model declares for a member is refused with the modelled
//!    `ValidationException` (HTTP 400) whose message is the AWS validation envelope,
//!    naming that member's wire path and the constraint it failed — identically whether
//!    the framework's validating walk found it (an absent required member, a `@length`)
//!    or an operation's own check did (an enum list, a `@range`).
//! 3. With request authentication ON, a request the emulator does not authenticate is
//!    refused 403 naming a shape THIS service's model declares —
//!    `AccessDeniedException` — because `restJson1` resolves the error shape from
//!    `x-amzn-errortype` / `__type`.
//!
//! Rules 1 and 2 are about what the OPERATION answers, so they run with authentication
//! off (as this crate's other suites do) and a credential scope that names this service;
//! rule 3 is about the authentication path itself, so it runs with authentication ON and
//! drives the emulator's real verification: a request with no signature at all, and one
//! whose signature does not verify.
//!
//! The value sets and bounds written out below come from the model
//! (`smithy_query(query="shape", shape="WaterAllocationType,Dimension,TimeGranularity,MaxResults,GranularityConfiguration,NextToken")`):
//! `WaterAllocationType` = [TOTAL_WATER_WITHDRAWALS]; `Dimension` = [REGION, SERVICE,
//! USAGE_ACCOUNT_ID]; `TimeGranularity` = [MONTHLY, QUARTERLY_CALENDAR, QUARTERLY_FISCAL,
//! YEARLY_CALENDAR, YEARLY_FISCAL]; `MaxResults` `@range(1..5000)`;
//! `GranularityConfiguration.FiscalYearStartMonth` `@range(1..12)`; `NextToken`
//! `@length(1..2000)`. `GetEstimatedWaterAllocation` supports only `YEARLY_CALENDAR`
//! ("Only YEARLY_CALENDAR time granularity is currently supported for water allocation").

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sustainability_awssustainabilityapiservice::ops::types;
use service_com_amazonaws_sustainability_awssustainabilityapiservice::state as sustainability;
use service_com_amazonaws_sustainability_awssustainabilityapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The four modelled routes (`OPERATIONS.md`).
const CARBON: &str = "/v1/estimated-carbon-emissions";
const CARBON_DIMENSIONS: &str = "/v1/estimated-carbon-emissions-dimension-values";
const WATER: &str = "/v1/estimated-water-allocation";
const WATER_DIMENSIONS: &str = "/v1/estimated-water-allocation-dimension-values";

/// A SigV4 `Authorization` header whose credential scope names this service's signing name
/// (`smithy_query(query="service_info")`, `sigv4Name` `sustainability`). That scope is how
/// the router picks the service which claims the request; the signature value itself
/// matters only where authentication is ON, and there it is deliberately not a valid one.
const SIGV4: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/sustainability/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// The loopback address an endpoint-pinned client reaches the emulator on: it names no
/// service, so such a request is routed on its credential scope alone.
const LOOPBACK: &str = "127.0.0.1:9297";

/// One host this service's endpoint rules resolve to
/// (`endpointResolution.urlTemplates`): it names the service, so a request carrying no
/// credential scope at all still reaches this service and is refused by authentication
/// rather than by routing.
const ENDPOINT_HOST: &str = "api.sustainability.us-east-1.api.aws";

/// A router with only this service mounted — one emulator "session", with request
/// authentication as the test asks for it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    /// A session that serves what the operation answers: authentication off, so a
    /// signature value is never checked and what is under test is the operation.
    fn unauthenticated() -> Self {
        Self::new(false)
    }

    /// A session that verifies every signature the way the served emulator does.
    fn authenticating() -> Self {
        Self::new(true)
    }

    fn new(authentication: bool) -> Self {
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(authentication);
        router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
        Self { router }
    }

    /// POST `path` with `body`, carrying a credential scope that names this service.
    async fn post(&self, path: &str, body: &Value) -> (u16, Value, Option<String>) {
        self.send(request_to(LOOPBACK, Some(SIGV4), path, body))
            .await
    }

    /// POST `path` with `body` and NO signature at all, addressed to this service's own
    /// endpoint host (an unsigned request to the loopback address names no service at
    /// all, and the router answers nobody's request with `404 UnknownService`).
    async fn post_unsigned(&self, path: &str, body: &Value) -> (u16, Value, Option<String>) {
        self.send(request_to(ENDPOINT_HOST, None, path, body)).await
    }

    /// Dispatch and return (status, parsed body, `x-amzn-errortype`).
    async fn send(&self, request: http::Request<bytes::Bytes>) -> (u16, Value, Option<String>) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let errortype = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed, errortype)
    }
}

/// A `POST <path>` carrying `body` as JSON, addressed to `host`, with `authorization` when
/// one is given.
fn request_to(
    host: &str,
    authorization: Option<&str>,
    path: &str,
    body: &Value,
) -> http::Request<bytes::Bytes> {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header("x-amz-date", "20260101T000000Z");
    if let Some(authorization) = authorization {
        request = request.header("authorization", authorization);
    }
    request
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the test request is well formed")
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

/// The `message` member of an error body (`jsonName: "message"` on every one of this
/// service's error shapes).
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A request body: the required `TimePeriod` plus whatever members a case adds.
fn body(start: i64, end: i64, members: &[(&str, Value)]) -> Value {
    let mut object = serde_json::Map::new();
    object.insert(
        "TimePeriod".to_string(),
        json!({ "Start": start, "End": end }),
    );
    for (name, value) in members {
        object.insert((*name).to_string(), value.clone());
    }
    Value::Object(object)
}

/// A window the seeded dataset covers: the whole first seeded year.
fn seeded_year() -> (i64, i64) {
    (
        sustainability::month_start_secs(sustainability::SEED_FIRST_YEAR, 1),
        sustainability::month_start_secs(sustainability::SEED_FIRST_YEAR + 1, 1),
    )
}

/// The fragments an enum-valued member's requirement must state: the clause and every
/// value in the set. The ORDER the set is listed in is the model's, not this test's, so
/// membership is what is asserted.
fn enum_set(allowed: &[&str]) -> Vec<String> {
    let mut wanted = vec!["Member must satisfy enum value set: [".to_string()];
    wanted.extend(allowed.iter().map(|value| (*value).to_string()));
    wanted
}

/// The one fragment a bound's requirement must state.
fn clause(requirement: &str) -> Vec<String> {
    vec![requirement.to_string()]
}

/// One refused request: the route, the body, the member its message must name, the value
/// it must echo back and the constraint clause it must state.
///
/// The member is matched as the PREFIX of the reported path, because a model-declared
/// constraint is found by the framework's validating walk, which names the collection step
/// it descended through (`allocationTypes.1.member`), while a constraint an operation owns
/// is reported on the member itself (`granularity`). Both name the member and the
/// requirement, which is what a caller needs to correct the request.
struct Refusal {
    case: &'static str,
    route: &'static str,
    body: Value,
    member: &'static str,
    value: String,
    requirement: Vec<String>,
}

/// Every refusal class these four operations can take for a value outside what the model
/// (or the operation's documented value set) allows: one row per (member, constraint kind,
/// route) the surface has.
fn refusals() -> Vec<Refusal> {
    let (start, end) = seeded_year();
    // Water allocation supports only `YEARLY_CALENDAR`, so that is the effective value set
    // of its `Granularity`; carbon's is every modelled one.
    let water_granularities = sustainability::WATER_SUPPORTED_GRANULARITIES;
    // The modelled value sets, from the generated enum constants — so a row cannot drift
    // from the model.
    let dimensions = types::DIMENSION_VALUES;
    let allocation_types = types::WATER_ALLOCATION_TYPE_VALUES;
    let emissions_types = types::EMISSIONS_TYPE_VALUES;
    let granularities = types::TIME_GRANULARITY_VALUES;
    let row = |case: &'static str,
               route: &'static str,
               members: &[(&str, Value)],
               member: &'static str,
               value: &str,
               requirement: Vec<String>| Refusal {
        case,
        route,
        body: body(start, end, members),
        member,
        value: value.to_string(),
        requirement,
    };
    vec![
        // The recorded case: an AllocationTypes member outside WaterAllocationType.
        row(
            "an out-of-enum AllocationTypes member",
            WATER,
            &[("AllocationTypes", json!(["NOT_A_REAL_ALLOCATION_TYPE"]))],
            "allocationTypes",
            "NOT_A_REAL_ALLOCATION_TYPE",
            enum_set(allocation_types),
        ),
        // Same member, two entries, the offending one SECOND.
        row(
            "a legal and an out-of-enum AllocationTypes member",
            WATER,
            &[(
                "AllocationTypes",
                json!(["TOTAL_WATER_WITHDRAWALS", "TOTAL_WATER_CONSUMPTION"]),
            )],
            "allocationTypes",
            "TOTAL_WATER_CONSUMPTION",
            enum_set(allocation_types),
        ),
        row(
            "an out-of-enum GroupBy dimension on the water route",
            WATER,
            &[("GroupBy", json!(["REGION", "AVAILABILITY_ZONE"]))],
            "groupBy",
            "AVAILABILITY_ZONE",
            enum_set(dimensions),
        ),
        row(
            "an out-of-enum GroupBy dimension on the carbon route",
            CARBON,
            &[("GroupBy", json!(["AVAILABILITY_ZONE"]))],
            "groupBy",
            "AVAILABILITY_ZONE",
            enum_set(dimensions),
        ),
        row(
            "an out-of-enum EmissionsTypes member",
            CARBON,
            &[("EmissionsTypes", json!(["TOTAL_SCOPE_4_CARBON_EMISSIONS"]))],
            "emissionsTypes",
            "TOTAL_SCOPE_4_CARBON_EMISSIONS",
            enum_set(emissions_types),
        ),
        row(
            "an out-of-enum Dimensions entry on the carbon dimension-values route",
            CARBON_DIMENSIONS,
            &[("Dimensions", json!(["SERVICE", "INSTANCE_TYPE"]))],
            "dimensions",
            "INSTANCE_TYPE",
            enum_set(dimensions),
        ),
        row(
            "an out-of-enum Dimensions entry on the water dimension-values route",
            WATER_DIMENSIONS,
            &[("Dimensions", json!(["INSTANCE_TYPE"]))],
            "dimensions",
            "INSTANCE_TYPE",
            enum_set(dimensions),
        ),
        // A granularity the model declares but THIS operation does not support, and one
        // the model does not declare at all: both leave the set of values the operation
        // accepts.
        row(
            "a modelled granularity water allocation does not support",
            WATER,
            &[("Granularity", json!("MONTHLY"))],
            "granularity",
            "MONTHLY",
            enum_set(water_granularities),
        ),
        row(
            "a granularity the model does not declare, water route",
            WATER,
            &[("Granularity", json!("HOURLY"))],
            "granularity",
            "HOURLY",
            enum_set(granularities),
        ),
        row(
            "a granularity the model does not declare, carbon route",
            CARBON,
            &[("Granularity", json!("HOURLY"))],
            "granularity",
            "HOURLY",
            enum_set(granularities),
        ),
        // `MaxResults` @range(1..5000): one past each published bound.
        row(
            "MaxResults one below the modelled minimum",
            WATER,
            &[("MaxResults", json!(0))],
            "maxResults",
            "0",
            clause("Member must have value greater than or equal to 1"),
        ),
        row(
            "MaxResults one above the modelled maximum",
            CARBON,
            &[("MaxResults", json!(5001))],
            "maxResults",
            "5001",
            clause("Member must have value less than or equal to 5000"),
        ),
        // `GranularityConfiguration.FiscalYearStartMonth` @range(1..12): a nested member,
        // reported at its dotted path.
        row(
            "a fiscal year starting below the modelled minimum month",
            CARBON,
            &[(
                "GranularityConfiguration",
                json!({ "FiscalYearStartMonth": 0 }),
            )],
            "granularityConfiguration.fiscalYearStartMonth",
            "0",
            clause("Member must have value greater than or equal to 1"),
        ),
        row(
            "a fiscal year starting above the modelled maximum month",
            CARBON,
            &[(
                "GranularityConfiguration",
                json!({ "FiscalYearStartMonth": 13 }),
            )],
            "granularityConfiguration.fiscalYearStartMonth",
            "13",
            clause("Member must have value less than or equal to 12"),
        ),
        // The filter's map KEY is the `Dimension` enum (`DimensionListMap`).
        row(
            "a FilterBy dimension outside the Dimension enum",
            WATER,
            &[(
                "FilterBy",
                json!({ "Dimensions": { "AVAILABILITY_ZONE": ["use1-az1"] } }),
            )],
            "filterBy.dimensions",
            "AVAILABILITY_ZONE",
            enum_set(dimensions),
        ),
        // A `@length` bound, found by the framework's validating walk.
        row(
            "a NextToken past the modelled length",
            WATER,
            &[("NextToken", json!("a".repeat(2001)))],
            "nextToken",
            &"a".repeat(256),
            clause("Member must have length less than or equal to 2000"),
        ),
    ]
}

/// Rule 2: every refused value is answered 400 `ValidationException` whose message is the
/// AWS validation envelope naming the offending member, echoing its value and stating the
/// constraint it failed.
#[tokio::test]
async fn a_value_outside_the_model_is_refused_with_the_modelled_validation_exception() {
    let emulator = Emulator::unauthenticated();

    for Refusal {
        case,
        route,
        body,
        member,
        value,
        requirement,
    } in refusals()
    {
        let (status, parsed, errortype) = emulator.post(route, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled ValidationException's HTTP 400 for \
             {route}, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case:?}: expected __type ValidationException, got {parsed}"
        );
        assert_eq!(
            errortype.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected x-amzn-errortype ValidationException, got {errortype:?}"
        );
        let reported = message(&parsed);
        assert!(
            reported.starts_with("1 validation error detected: "),
            "case {case:?}: expected the AWS validation envelope, got {reported:?}"
        );
        assert!(
            reported.contains(&format!("Value '{value}")),
            "case {case:?}: the message must echo the rejected value {value:?}, got \
             {reported:?}"
        );
        assert!(
            reported.contains(&format!("at '{member}")),
            "case {case:?}: the message must name the member {member:?}, got {reported:?}"
        );
        assert!(
            reported.contains("failed to satisfy constraint: "),
            "case {case:?}: the message must state the constraint it failed, got \
             {reported:?}"
        );
        for wanted in &requirement {
            assert!(
                reported.contains(wanted.as_str()),
                "case {case:?}: the constraint must state {wanted:?}, got {reported:?}"
            );
        }
    }
}

/// An absent required member is refused in the SAME envelope, so a caller cannot tell the
/// framework's walk from a handler's own check.
#[tokio::test]
async fn an_absent_required_member_is_refused_in_the_same_envelope() {
    let emulator = Emulator::unauthenticated();
    // (case, route, body missing a required member, the member's wire path)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        ("no TimePeriod, water route", WATER, json!({}), "timePeriod"),
        (
            "no TimePeriod, carbon route",
            CARBON,
            json!({ "GroupBy": ["REGION"] }),
            "timePeriod",
        ),
        (
            "no TimePeriod, water dimension-values route",
            WATER_DIMENSIONS,
            json!({ "Dimensions": ["REGION"] }),
            "timePeriod",
        ),
    ];

    for (case, route, body, member) in cases {
        let (status, parsed, _) = emulator.post(route, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {parsed}"
        );
        let expected = format!(
            "1 validation error detected: Value null at '{member}' failed to satisfy \
             constraint: Member must not be null"
        );
        assert_eq!(
            message(&parsed),
            expected,
            "case {case:?}: expected {expected:?}, got {parsed}"
        );
    }
}

/// A rejected value is echoed back bounded, never amplified: a member a caller can make
/// arbitrarily long is elided in the message.
#[tokio::test]
async fn a_rejected_value_is_echoed_back_bounded() {
    let emulator = Emulator::unauthenticated();
    let (start, end) = seeded_year();
    // One enum element of 4 KiB: legal JSON, far past any modelled value.
    let huge = "X".repeat(4096);
    let request = body(start, end, &[("GroupBy", json!([huge]))]);

    let (status, parsed, _) = emulator.post(WATER, &request).await;
    let reported = message(&parsed);
    assert_eq!(
        status, 400,
        "a 4 KiB dimension must be refused, got {status} with {parsed}"
    );
    assert!(
        reported.starts_with("1 validation error detected: Value 'XXX"),
        "the message must still be the validation envelope, got {reported:?}"
    );
    assert!(
        reported.contains("... [4096 bytes]"),
        "the echoed value must be elided with its length, got {reported:?}"
    );
    assert!(
        reported.len() < 1024,
        "the message must stay bounded, got {} bytes: {reported:?}",
        reported.len()
    );
}

/// Rule 1: a request that reaches one of the four modelled routes is answered by that
/// operation — HTTP 200 with the required `Results` member always present, empty when the
/// request selects nothing.
#[tokio::test]
async fn a_claimed_request_is_served_by_the_operation_it_names() {
    let emulator = Emulator::unauthenticated();
    let (start, end) = seeded_year();
    let before_the_dataset = (
        sustainability::month_start_secs(sustainability::SEED_FIRST_YEAR - 5, 1),
        sustainability::month_start_secs(sustainability::SEED_FIRST_YEAR - 4, 1),
    );
    let bounds = sustainability::MAX_RESULTS_RANGE;
    // (case, route, body, whether the request can select a row)
    let cases: Vec<(&str, &str, Value, bool)> = vec![
        (
            "the water route with no optional member at all",
            WATER,
            body(start, end, &[]),
            true,
        ),
        (
            "the water route with the only allocation type it models",
            WATER,
            body(
                start,
                end,
                &[
                    ("AllocationTypes", json!(["TOTAL_WATER_WITHDRAWALS"])),
                    ("GroupBy", json!(["REGION", "SERVICE"])),
                    ("Granularity", json!("YEARLY_CALENDAR")),
                ],
            ),
            true,
        ),
        (
            "the water route at the modelled minimum page size",
            WATER,
            body(start, end, &[("MaxResults", json!(*bounds.start()))]),
            true,
        ),
        (
            "the water route at the modelled maximum page size",
            WATER,
            body(start, end, &[("MaxResults", json!(*bounds.end()))]),
            true,
        ),
        (
            "the water route over a window the dataset does not cover",
            WATER,
            body(before_the_dataset.0, before_the_dataset.1, &[]),
            false,
        ),
        (
            "the water route filtered to a region the dataset does not hold",
            WATER,
            body(
                start,
                end,
                &[(
                    "FilterBy",
                    json!({ "Dimensions": { "REGION": ["ap-southeast-7"] } }),
                )],
            ),
            false,
        ),
        (
            "the carbon route",
            CARBON,
            body(start, end, &[("GroupBy", json!(["SERVICE"]))]),
            true,
        ),
        (
            "the carbon dimension-values route",
            CARBON_DIMENSIONS,
            body(start, end, &[("Dimensions", json!(["REGION"]))]),
            true,
        ),
        (
            "the water dimension-values route",
            WATER_DIMENSIONS,
            body(start, end, &[("Dimensions", json!(["SERVICE"]))]),
            true,
        ),
    ];

    for (case, route, request, expect_rows) in cases {
        let (status, parsed, errortype) = emulator.post(route, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the modelled HTTP 200, got {status} with {parsed} \
             (errortype {errortype:?})"
        );
        let results = parsed.get("Results").and_then(Value::as_array);
        let results = results.unwrap_or_else(|| {
            panic!("case {case:?}: the required Results member must be a list, got {parsed}")
        });
        assert_eq!(
            !results.is_empty(),
            expect_rows,
            "case {case:?}: expected rows={expect_rows}, got {} row(s): {parsed}",
            results.len()
        );
    }
}

/// Rule 3: with request authentication ON, a request the emulator does not authenticate —
/// one carrying no signature, and one whose signature does not verify — is refused 403
/// naming the error SHAPE this service's model declares, never a code the model does not
/// carry.
#[tokio::test]
async fn an_unauthenticated_request_is_refused_with_the_modelled_access_denied_exception() {
    let emulator = Emulator::authenticating();
    let (start, end) = seeded_year();
    let request = body(start, end, &[]);

    for (case, answered) in [
        (
            "no signature at all, addressed to this service's endpoint host",
            emulator.post_unsigned(WATER, &request).await,
        ),
        (
            "a credential scope naming this service with a signature that does not verify",
            emulator.post(CARBON, &request).await,
        ),
    ] {
        let (status, parsed, errortype) = answered;
        assert_eq!(
            status, 403,
            "case {case:?}: expected the modelled AccessDeniedException's HTTP 403, got \
             {status} with {parsed}"
        );
        // The literals come from the MODEL (`AccessDeniedException`, `@httpError(403)`,
        // documented "You do not have sufficient access to perform this action."), not
        // from the crate's constants, so a constant that drifts from the model fails here.
        assert_eq!(
            error_code(&parsed),
            "AccessDeniedException",
            "case {case:?}: expected __type AccessDeniedException, got {parsed}"
        );
        assert_eq!(
            errortype.as_deref(),
            Some("AccessDeniedException"),
            "case {case:?}: expected x-amzn-errortype AccessDeniedException, got \
             {errortype:?}"
        );
        let reported = message(&parsed);
        assert!(
            reported.starts_with("You do not have sufficient access to perform this action."),
            "case {case:?}: expected the modelled message, got {reported:?}"
        );
        // …and it says what to do about it, which the shape's documentation does not.
        assert!(
            reported.contains("emulator credentials"),
            "case {case:?}: the refusal must name what the caller must present, got \
             {reported:?}"
        );
    }
}

/// A value set an OPERATION narrows — water allocation supports only `YEARLY_CALENDAR` of
/// the five modelled granularities — is stated in full, so the refusal names the set that
/// operation accepts rather than every modelled value.
#[tokio::test]
async fn an_operation_narrowed_value_set_is_stated_in_full() {
    let emulator = Emulator::unauthenticated();
    let (start, end) = seeded_year();
    let supported = sustainability::WATER_SUPPORTED_GRANULARITIES;
    assert_eq!(
        supported,
        ["YEARLY_CALENDAR"],
        "water allocation's documented value set is YEARLY_CALENDAR alone, got {supported:?}"
    );

    // Every modelled granularity the water route does NOT support (the model declares
    // five; this operation accepts one).
    for granularity in types::TIME_GRANULARITY_VALUES
        .iter()
        .filter(|value| !supported.contains(*value))
    {
        let request = body(start, end, &[("Granularity", json!(granularity))]);
        let (status, parsed, _) = emulator.post(WATER, &request).await;
        assert_eq!(
            status, 400,
            "case {granularity:?}: water allocation must refuse it, got {status} with \
             {parsed}"
        );
        let expected = format!(
            "1 validation error detected: Value '{granularity}' at 'granularity' failed to \
             satisfy constraint: Member must satisfy enum value set: [YEARLY_CALENDAR]"
        );
        assert_eq!(
            message(&parsed),
            expected,
            "case {granularity:?}: expected {expected:?}, got {parsed}"
        );
    }
}

/// A refusal is a function of the REQUEST, not of map iteration order: a filter naming
/// several dimensions outside the `Dimension` enum is refused for the same one on every
/// run, so a caller (or a test) reads a stable message.
#[tokio::test]
async fn a_refusal_does_not_depend_on_map_iteration_order() {
    let emulator = Emulator::unauthenticated();
    let (start, end) = seeded_year();
    // Three unmodelled dimension keys; the lexicographically first is the one reported.
    let request = body(
        start,
        end,
        &[(
            "FilterBy",
            json!({
                "Dimensions": {
                    "INSTANCE_TYPE": ["m5.large"],
                    "AVAILABILITY_ZONE": ["use1-az1"],
                    "OPERATION": ["RunInstances"]
                }
            }),
        )],
    );
    let expected = format!(
        "1 validation error detected: Value 'AVAILABILITY_ZONE' at 'filterBy.dimensions' \
         failed to satisfy constraint: Member must satisfy enum value set: [{}]",
        types::DIMENSION_VALUES.join(", ")
    );

    // Repeated so a hasher reseeded per router cannot make it pass by luck.
    for attempt in 1..=8 {
        let (status, parsed, _) = emulator.post(WATER, &request).await;
        assert_eq!(
            status, 400,
            "attempt {attempt}: expected HTTP 400, got {status} with {parsed}"
        );
        assert_eq!(
            message(&parsed),
            expected,
            "attempt {attempt}: expected {expected:?}, got {parsed}"
        );
    }
}
