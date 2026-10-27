// SPDX-License-Identifier: CC0-1.0

//! The custom endpoint, end to end over the wire: `RegisterCustomEndpoint` ->
//! `GetCustomEndpoint`.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, status codes, bodies and modelled errors asserted here are the ones a client
//! sees.
//!
//! RULE: the custom endpoint is a per-account, per-region SINGLETON that a register MINTS
//! (`smithy_query(query="operation",operation="RegisterCustomEndpoint,GetCustomEndpoint")`
//! -> `POST /custom-endpoint` (201) and `GET /custom-endpoint` (200, `@readonly`), each with
//! an EMPTY request shape and a `@required` `EndpointAddress` on its response, the read
//! modelling `ResourceNotFoundException` and the register `ThrottlingException`). So:
//!
//! * the first register of a region MINTS that region's dedicated `EndpointAddress`, stores
//!   it, and answers it; the address satisfies the modelled `EndpointAddress` constraints
//!   and carries the region it was registered for;
//! * the register is a GET-OR-CREATE: repeating it answers the address the first call
//!   minted, however many times it is repeated, and mints no second endpoint;
//! * a read PROJECTS the stored address, so a register is observable through
//!   `GetCustomEndpoint`, and repeated reads report the same address;
//! * the endpoint exists only where a register minted one: a region none has registered
//!   answers the modelled `ResourceNotFoundException`, and reading it registers nothing;
//! * the endpoint is scoped to the region the request was signed for, so each region
//!   reports its own address and no other region's;
//! * the store is BOUNDED in the regions it holds: the key is the region of the request's
//!   own credential scope, so a register for a further region past `MAX_CONFIGURED_REGIONS`
//!   is refused, and a scope region that is not shaped like a region name never becomes a
//!   key — the same guard and bound this service's per-region puts apply.
//!
//! The regions below span the class the rule generalises over — the test's default, regions
//! in other partitions' naming styles, the longest and shortest names the key guard admits,
//! and names that fail its shape — together with repeat counts from one call to several.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    CUSTOM_ENDPOINT_LABEL_LENGTH, CUSTOM_ENDPOINT_RESOURCE_TYPE, MAX_CONFIGURED_REGIONS,
    MAX_REGION_NAME_LENGTH,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The route both operations are bound to (`smithy_query` above): the method is what tells
/// the register from the read.
const CUSTOM_ENDPOINT_ROUTE: &str = "/custom-endpoint";

/// The modelled length bounds of `EndpointAddress`
/// (`smithy_query(query="shape",shape="EndpointAddress")` ->
/// `{"type": "string", "length": {"min": 1, "max": 256}, "pattern": "^[A-Za-z0-9._@-]+$"}`).
const ENDPOINT_ADDRESS_LENGTH: (usize, usize) = (1, 256);

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn endpoint_address(&self) -> Option<&str> {
        self.member("EndpointAddress").and_then(Value::as_str)
    }

    fn message(&self) -> Option<&str> {
        self.member("Message")
            .or_else(|| self.member("message"))
            .and_then(Value::as_str)
    }
}

/// Whether `address` satisfies the modelled `EndpointAddress` constraints — its length
/// bounds and its pattern `^[A-Za-z0-9._@-]+$`, both quoted on
/// [`ENDPOINT_ADDRESS_LENGTH`].
fn is_modelled_endpoint_address(address: &str) -> bool {
    let length = address.chars().count();
    length >= ENDPOINT_ADDRESS_LENGTH.0
        && length <= ENDPOINT_ADDRESS_LENGTH.1
        && address
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._@-".contains(character))
}

/// The dedicated label of `address`: everything before its first separator, which is what a
/// register MINTS for one region.
fn label_of(address: &str) -> &str {
    address.split('.').next().unwrap_or(address)
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
}

/// [`emulator`] over an engine the caller keeps a handle to, so a test can inspect the
/// service's store directly — what the store HOLDS is not visible on the wire.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request and reduce the answer. The `Authorization` credential scope is what
/// routes the request to this service and names the region it addresses.
async fn call_signed(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://api.iotmanagedintegrations.{region}.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// Register the custom endpoint of `region`.
async fn register_in(router: &MultiServiceRouter, region: &str) -> Reply {
    call_signed(
        router,
        region,
        "POST",
        CUSTOM_ENDPOINT_ROUTE,
        Some(json!({})),
    )
    .await
}

/// Read the custom endpoint of `region`.
async fn read_in(router: &MultiServiceRouter, region: &str) -> Reply {
    call_signed(router, region, "GET", CUSTOM_ENDPOINT_ROUTE, None).await
}

/// The regions the table-driven cases span: the test's default, two other naming styles AWS
/// publishes, a government-partition name, and the shortest and longest names the store's
/// key guard admits.
fn region_cases() -> Vec<(&'static str, String)> {
    vec![
        ("the test's default region", REGION.to_string()),
        ("another commercial region", "eu-west-1".to_string()),
        ("a three-part region name", "ap-southeast-2".to_string()),
        ("a government-partition region", "us-gov-east-1".to_string()),
        ("the shortest admissible key", "a".to_string()),
        (
            "the longest admissible key",
            "r".repeat(MAX_REGION_NAME_LENGTH),
        ),
    ]
}

/// RULE (see the module docs): the first register of a region mints that region's dedicated
/// `EndpointAddress`, and a read projects the stored address — for every region the key
/// guard admits.
#[tokio::test]
async fn a_register_mints_an_endpoint_the_read_reports() {
    for (label, region) in region_cases() {
        // A fresh store per row, so each row registers a region for the first time.
        let router = emulator();

        let registered = register_in(&router, &region).await;
        assert_eq!(
            registered.status, 201,
            "case {label:?}: expected HTTP 201 from RegisterCustomEndpoint, got {} with body \
             {:?}",
            registered.status, registered.body
        );
        let address = registered.endpoint_address().unwrap_or_else(|| {
            panic!(
                "case {label:?}: the register must answer the required EndpointAddress, got {:?}",
                registered.body
            )
        });
        assert!(
            is_modelled_endpoint_address(address),
            "case {label:?}: the minted address must satisfy the modelled EndpointAddress \
             constraints, got {address:?}"
        );
        assert!(
            address.contains(region.as_str()),
            "case {label:?}: the address is the endpoint of the region it was registered for, \
             so it must carry {region:?}, got {address:?}"
        );
        assert_eq!(
            label_of(address).chars().count(),
            CUSTOM_ENDPOINT_LABEL_LENGTH,
            "case {label:?}: the dedicated label must be minted at its full length, got {:?}",
            label_of(address)
        );

        let read = read_in(&router, &region).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: expected HTTP 200 from GetCustomEndpoint, got {} with body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.endpoint_address(),
            Some(address),
            "case {label:?}: the read must project the stored address {address:?}, got {:?}",
            read.body
        );

        // A read is a pure read: repeating it reports the same address and mints nothing.
        let again = read_in(&router, &region).await;
        assert_eq!(
            again.endpoint_address(),
            Some(address),
            "case {label:?}: a repeated read must report the same address {address:?}, got {:?}",
            again.body
        );
    }
}

/// RULE: the register is a get-or-create, so repeating it answers the address the first call
/// minted and mints no second endpoint — for one repeat and for many.
#[tokio::test]
async fn repeating_the_register_answers_the_endpoint_already_minted() {
    // The rule generalises over HOW MANY times the call is repeated.
    let repeats: &[(&str, usize)] = &[
        ("registered twice", 2),
        ("registered three times", 3),
        ("registered ten times", 10),
    ];

    for (label, count) in repeats {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_over(Arc::clone(&engine));

        let first = register_in(&router, REGION).await;
        assert_eq!(
            first.status, 201,
            "case {label:?}: expected HTTP 201 from the first register, got {} with body {:?}",
            first.status, first.body
        );
        let minted = first
            .endpoint_address()
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?}: the first register must answer an address, got {:?}",
                    first.body
                )
            })
            .to_string();

        for repeat in 2..=*count {
            let again = register_in(&router, REGION).await;
            assert_eq!(
                again.status, 201,
                "case {label:?}: register #{repeat} must also answer HTTP 201, got {} with body \
                 {:?}",
                again.status, again.body
            );
            assert_eq!(
                again.endpoint_address(),
                Some(minted.as_str()),
                "case {label:?}: register #{repeat} must answer the address already minted \
                 ({minted:?}), got {:?}",
                again.body
            );
        }

        // One endpoint for the region, however many registers ran, and the read still
        // reports it.
        let held = engine.state::<ServiceState>().lock().custom_endpoints.len();
        assert_eq!(
            held, 1,
            "case {label:?}: {count} registers of one region must leave ONE stored endpoint, the \
             store holds {held}"
        );
        let read = read_in(&router, REGION).await;
        assert_eq!(
            read.endpoint_address(),
            Some(minted.as_str()),
            "case {label:?}: the read must still report {minted:?}, got {:?}",
            read.body
        );
    }
}

/// RULE: the endpoint exists only where a register minted one, so a region none has
/// registered answers the modelled `ResourceNotFoundException` — and reading it registers
/// nothing.
#[tokio::test]
async fn a_region_with_no_registered_endpoint_is_not_found() {
    for (label, region) in region_cases() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_over(Arc::clone(&engine));

        let read = read_in(&router, &region).await;
        assert_eq!(
            read.status, 404,
            "case {label:?}: expected HTTP 404 before any register, got {} with body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected the modelled ResourceNotFoundException code, got {:?}",
            read.error_type
        );
        assert_eq!(
            read.member("ResourceType").and_then(Value::as_str),
            Some(CUSTOM_ENDPOINT_RESOURCE_TYPE),
            "case {label:?}: the 404 reports which KIND of resource is absent, got {:?}",
            read.body
        );
        let message = read.message().unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {label:?}: the 404 must state why the request failed, got {:?}",
            read.body
        );

        // The read registered nothing: the store is still empty and a second read is still
        // a 404.
        let held = engine.state::<ServiceState>().lock().custom_endpoints.len();
        assert_eq!(
            held, 0,
            "case {label:?}: a read must not mint an endpoint, the store holds {held}"
        );
        let again = read_in(&router, &region).await;
        assert_eq!(
            again.status, 404,
            "case {label:?}: the second read must still be a 404, got {} with body {:?}",
            again.status, again.body
        );

        // And a register is what makes the read succeed.
        let registered = register_in(&router, &region).await;
        assert_eq!(
            registered.status, 201,
            "case {label:?}: expected HTTP 201 from the register, got {} with body {:?}",
            registered.status, registered.body
        );
        let read = read_in(&router, &region).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the read must succeed once an endpoint is registered, got {} with \
             body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.endpoint_address(),
            registered.endpoint_address(),
            "case {label:?}: the read must report the registered address {:?}, got {:?}",
            registered.endpoint_address(),
            read.body
        );
    }
}

/// RULE: the endpoint is scoped to the region the request was signed for, the way every
/// other resource of this service is — so each region reports the address registered through
/// its own endpoint, and a region nobody registered reports none.
#[tokio::test]
async fn the_endpoint_is_scoped_to_the_signing_region() {
    let router = emulator();

    let rows = region_cases();
    let mut registered: Vec<(String, String)> = Vec::new();
    for (label, region) in &rows {
        let reply = register_in(&router, region).await;
        assert_eq!(
            reply.status, 201,
            "case {label:?}: expected HTTP 201, got {} with body {:?}",
            reply.status, reply.body
        );
        let address = reply
            .endpoint_address()
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?}: the register must answer an address, got {:?}",
                    reply.body
                )
            })
            .to_string();
        registered.push((region.clone(), address));
    }

    for (region, address) in &registered {
        let read = read_in(&router, region).await;
        assert_eq!(
            read.endpoint_address(),
            Some(address.as_str()),
            "case {region:?}: each region reports the address registered through its own \
             endpoint ({address:?}), got {:?}",
            read.body
        );
        for (other_region, other_address) in &registered {
            if other_region == region {
                continue;
            }
            assert_ne!(
                address, other_address,
                "case {region:?}: each region's endpoint is its own, so it must differ from \
                 {other_region:?}'s ({other_address:?})"
            );
        }
    }

    // A region nobody registered holds nothing, with other regions registered: a register
    // through one endpoint does not leak into another's.
    let unregistered = "sa-east-1";
    let read = read_in(&router, unregistered).await;
    assert_eq!(
        read.status, 404,
        "case {unregistered:?}: another region's endpoint must not leak here, expected HTTP 404, \
         got {} with body {:?}",
        read.status, read.body
    );
}

/// RULE: the endpoint store is keyed by a region the CALLER chooses (the region of its own
/// credential scope), so the number of regions it holds is BOUNDED: a register for a region
/// it already holds always succeeds and answers the stored address, and one for a further
/// region past `MAX_CONFIGURED_REGIONS` is refused with the modelled `ThrottlingException`
/// (HTTP 429) rather than growing the store.
#[tokio::test]
async fn the_number_of_registered_regions_is_bounded() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // Fill the store to its bound, one distinct signing region per register.
    let regions: Vec<String> = (0..MAX_CONFIGURED_REGIONS)
        .map(|index| format!("us-test-{index}"))
        .collect();
    for region in &regions {
        let reply = register_in(&router, region).await;
        assert_eq!(
            reply.status, 201,
            "case {region:?}: expected HTTP 201 while under the bound, got {} with body {:?}",
            reply.status, reply.body
        );
    }

    // One region past the bound is refused, and nothing is stored for it: its read is still
    // a 404.
    let past = "us-test-past-the-bound";
    let refused = register_in(&router, past).await;
    assert_eq!(
        refused.status, 429,
        "case {past:?}: expected HTTP 429 past the bound of {MAX_CONFIGURED_REGIONS} regions, got \
         {} with body {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ThrottlingException"),
        "case {past:?}: expected the modelled ThrottlingException code, got {:?}",
        refused.error_type
    );
    let read = read_in(&router, past).await;
    assert_eq!(
        read.status, 404,
        "case {past:?}: a refused region holds no endpoint, expected HTTP 404, got {} with body \
         {:?}",
        read.status, read.body
    );
    let held = engine.state::<ServiceState>().lock().custom_endpoints.len();
    assert_eq!(
        held, MAX_CONFIGURED_REGIONS,
        "case {past:?}: the store must stay at {MAX_CONFIGURED_REGIONS} endpoints, it holds {held}"
    );

    // A region the store already holds is still served at the bound, with the address it
    // already minted.
    let already_held = regions.last().expect("a registered region");
    let stored = read_in(&router, already_held)
        .await
        .endpoint_address()
        .map(str::to_string)
        .expect("a registered region reports its endpoint");
    let again = register_in(&router, already_held).await;
    assert_eq!(
        again.status, 201,
        "case {already_held:?}: a region the store already holds stays served at the bound, got \
         {} with body {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.endpoint_address(),
        Some(stored.as_str()),
        "case {already_held:?}: the repeat must answer the stored address {stored:?}, got {:?}",
        again.body
    );
}

/// RULE: a credential scope whose region is not shaped like a region name never becomes a
/// store key: the register is refused with the modelled `ValidationException` (HTTP 400) and
/// mints nothing. The rows span the ways the shape can fail — wrong characters, and a name
/// past `MAX_REGION_NAME_LENGTH`.
#[tokio::test]
async fn a_scope_region_that_is_not_a_region_name_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    let too_long = "a".repeat(MAX_REGION_NAME_LENGTH + 1);
    let cases: &[(&str, &str)] = &[
        ("uppercase letters", "US-EAST-1"),
        ("punctuation outside the shape", "us_east_1"),
        ("one past the length maximum", too_long.as_str()),
    ];

    for (label, region) in cases {
        let reply = register_in(&router, region).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ValidationException"),
            "case {label:?}: expected the modelled ValidationException code, got {:?}",
            reply.error_type
        );
        let held = engine.state::<ServiceState>().lock().custom_endpoints.len();
        assert_eq!(
            held, 0,
            "case {label:?}: a refused register must mint nothing, the store holds {held}"
        );
    }
}
