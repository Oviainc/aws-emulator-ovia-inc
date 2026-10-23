// SPDX-License-Identifier: CC0-1.0

//! The hub configuration, end to end over the wire: `PutHubConfiguration` ->
//! `GetHubConfiguration`.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`),
//! so the routes, status codes, bodies and modelled errors asserted here are the ones a
//! client sees.
//!
//! RULE: the hub configuration is a per-account, per-region SINGLETON
//! (`smithy_query(query="operation",operation="GetHubConfiguration,PutHubConfiguration")`
//! -> `PUT /hub-configuration` (201) with a `@required`
//! `HubTokenTimerExpirySettingInSeconds` (`long`, range 1..) and `GET /hub-configuration`
//! (200, `@readonly`) with that member plus `UpdatedAt`, neither carrying an
//! identifier). So:
//!
//! * a put STORES the expiry it was given and answers that same value, whatever its
//!   magnitude inside the modelled range;
//! * a get PROJECTS the configuration in force — the expiry of the last accepted put,
//!   and the time that put ran on `UpdatedAt`;
//! * the configuration EXISTS before any put: a region nobody has configured reports the
//!   service default expiry and no `UpdatedAt` (nothing has updated it);
//! * a put outside the modelled range is refused and stores nothing, so the previously
//!   stored configuration is what a later get still reports;
//! * a get is a pure read: repeating it changes nothing a later caller sees;
//! * the store is BOUNDED in the regions it holds: the key is the region of the request's
//!   own credential scope, so a put for a further region past `MAX_CONFIGURED_REGIONS` is
//!   refused, and a scope region that is not shaped like a region name never becomes a
//!   key — the same guard and bound this service's other per-region put applies.
//!
//! The expiry values below span the class the rule generalises over — the published
//! range minimum, one and two past it, minutes/hours/days-sized settings and the largest
//! `long` — together with each value the model refuses (the bound minus one, zero, a
//! negative) and an absent `@required` member.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS, MAX_CONFIGURED_REGIONS, MAX_REGION_NAME_LENGTH,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState,
    EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for.
const REGION: &str = "us-east-1";

/// The modelled minimum of `HubTokenTimerExpirySettingInSeconds`
/// (`smithy_query(query="shape",shape="HubTokenTimerExpirySettingInSeconds")` ->
/// `{"type": "long", "range": {"min": 1}}`). The model declares no maximum, so the
/// largest `long` is inside the range.
const EXPIRY_MINIMUM: i64 = 1;

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

    fn expiry(&self) -> Option<i64> {
        self.member("HubTokenTimerExpirySettingInSeconds")
            .and_then(Value::as_i64)
    }

    fn updated_at(&self) -> Option<f64> {
        self.member("UpdatedAt").and_then(Value::as_f64)
    }
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
            "https://iotmanagedintegrations.{region}.amazonaws.com{path}"
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

/// [`call_signed`] for the test's default region.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    call_signed(router, REGION, method, path, body).await
}

/// Put `expiry` in the default region.
async fn put_expiry(router: &MultiServiceRouter, expiry: i64) -> Reply {
    call(
        router,
        "PUT",
        "/hub-configuration",
        Some(json!({"HubTokenTimerExpirySettingInSeconds": expiry})),
    )
    .await
}

/// Read the default region's hub configuration.
async fn get_configuration(router: &MultiServiceRouter) -> Reply {
    call(router, "GET", "/hub-configuration", None).await
}

/// RULE (see the module docs): a get reports what the last accepted put stored, for
/// every expiry inside the modelled range.
#[tokio::test]
async fn a_get_reports_the_expiry_the_last_put_stored() {
    let router = emulator();

    // The class the rule generalises over: the published minimum, the values just past
    // it, settings sized in minutes, hours and days, and the largest `long` — the model
    // declares no maximum, so that value is inside the range too.
    let cases: &[(&str, i64)] = &[
        ("the published range minimum", EXPIRY_MINIMUM),
        ("one past the minimum", EXPIRY_MINIMUM + 1),
        ("two past the minimum", EXPIRY_MINIMUM + 2),
        ("a minute", 60),
        ("an hour", 3_600),
        ("a day", 86_400),
        ("a value past u32", 5_000_000_000),
        ("the largest long", i64::MAX),
    ];

    for (label, expiry) in cases {
        let put = put_expiry(&router, *expiry).await;
        assert_eq!(
            put.status, 201,
            "case {label:?}: expected HTTP 201 from PutHubConfiguration, got {} with body {:?}",
            put.status, put.body
        );
        assert_eq!(
            put.expiry(),
            Some(*expiry),
            "case {label:?}: the put must answer the stored expiry {expiry}, got {:?}",
            put.body
        );

        let read = get_configuration(&router).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: expected HTTP 200 from GetHubConfiguration, got {} with body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.expiry(),
            Some(*expiry),
            "case {label:?}: the read must project the stored expiry {expiry}, got {:?}",
            read.body
        );
        assert!(
            read.updated_at().is_some(),
            "case {label:?}: a configured region reports when it was configured, got {:?}",
            read.body
        );
    }
}

/// RULE: the hub configuration exists before any put, so an unconfigured region reports
/// the service default expiry and no `UpdatedAt` — and reading it neither sets it nor
/// stamps it.
#[tokio::test]
async fn an_unconfigured_region_reports_the_default_configuration() {
    let router = emulator();

    let label = "GetHubConfiguration before any put";
    let first = get_configuration(&router).await;
    assert_eq!(
        first.status, 200,
        "case {label:?}: expected HTTP 200, got {} with body {:?}",
        first.status, first.body
    );
    assert_eq!(
        first.expiry(),
        Some(DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS),
        "case {label:?}: an unconfigured region reports the default expiry, got {:?}",
        first.body
    );
    // The reported default must itself satisfy the member's modelled range.
    let reported = first.expiry().unwrap_or_default();
    assert!(
        reported >= EXPIRY_MINIMUM,
        "case {label:?}: the reported default must satisfy the modelled range minimum \
         {EXPIRY_MINIMUM}, got {reported}"
    );
    assert_eq!(
        first.updated_at(),
        None,
        "case {label:?}: nothing has updated the configuration, so it reports no UpdatedAt, \
         got {:?}",
        first.body
    );

    // A read is a pure read: repeating it must not set or stamp the configuration.
    let again = get_configuration(&router).await;
    assert_eq!(
        again.expiry(),
        Some(DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS),
        "case {label:?}: a read must not change the expiry it reported, got {:?}",
        again.body
    );
    assert_eq!(
        again.updated_at(),
        None,
        "case {label:?}: a read must not stamp the configuration it read, got {:?}",
        again.body
    );

    // And a put is what makes `UpdatedAt` appear.
    let configured = DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS + 1;
    put_expiry(&router, configured).await;
    let read = get_configuration(&router).await;
    assert_eq!(
        read.expiry(),
        Some(configured),
        "case {label:?}: a put replaces the default, got {:?}",
        read.body
    );
    assert!(
        read.updated_at().is_some(),
        "case {label:?}: a put stamps the configuration, got {:?}",
        read.body
    );
}

/// RULE: a put REPLACES the configuration and stamps it with the time of the call, so
/// the last accepted value wins and `UpdatedAt` moves forward; a get repeated against an
/// unchanged configuration reports the same instant.
#[tokio::test]
async fn a_later_put_replaces_the_configuration_and_restamps_it() {
    let router = emulator();

    // Each row replaces the previous one, including a row that repeats the value already
    // stored and a row that lowers it: the rule is about the ORDER of accepted puts.
    let sequence: &[(&str, i64)] = &[
        ("first put", 900),
        ("a larger expiry", 7_200),
        ("the same expiry again", 7_200),
        ("a smaller expiry", EXPIRY_MINIMUM),
    ];

    let mut previous_updated_at: Option<f64> = None;
    for (label, expiry) in sequence {
        // Separate the puts so the stamps are distinguishable at the timestamp's
        // precision, as `managed_thing_lifecycle` does for an update's `UpdatedAt`.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let put = put_expiry(&router, *expiry).await;
        assert_eq!(
            put.status, 201,
            "case {label:?}: expected HTTP 201, got {} with body {:?}",
            put.status, put.body
        );

        let read = get_configuration(&router).await;
        assert_eq!(
            read.expiry(),
            Some(*expiry),
            "case {label:?}: the read must report the last accepted expiry {expiry}, got {:?}",
            read.body
        );
        let updated_at = read.updated_at().unwrap_or_else(|| {
            panic!(
                "case {label:?}: a configured region reports UpdatedAt, got {:?}",
                read.body
            )
        });
        if let Some(earlier) = previous_updated_at {
            assert!(
                updated_at > earlier,
                "case {label:?}: a put re-stamps the configuration, expected an UpdatedAt \
                 later than {earlier}, got {updated_at}"
            );
        }

        // Reading again reports the same instant: a get neither re-stamps nor mutates.
        let repeated = get_configuration(&router).await;
        assert_eq!(
            repeated.updated_at(),
            Some(updated_at),
            "case {label:?}: a repeated read must report the same UpdatedAt {updated_at}, got \
             {:?}",
            repeated.body
        );
        assert_eq!(
            repeated.expiry(),
            Some(*expiry),
            "case {label:?}: a repeated read must report the same expiry, got {:?}",
            repeated.body
        );
        previous_updated_at = Some(updated_at);
    }
}

/// RULE: a put the model refuses stores nothing, so the configuration a later get
/// reports is still the last ACCEPTED one. The refused rows are the published bound minus
/// one, zero, a negative value and an absent `@required` member.
#[tokio::test]
async fn a_refused_put_leaves_the_stored_configuration_alone() {
    let router = emulator();

    let accepted: i64 = 1_800;
    let put = put_expiry(&router, accepted).await;
    assert_eq!(
        put.status, 201,
        "case \"seed the configuration\": expected HTTP 201, got {} with body {:?}",
        put.status, put.body
    );
    let stamped = get_configuration(&router)
        .await
        .updated_at()
        .expect("the seeded configuration reports UpdatedAt");

    let refused: &[(&str, Value)] = &[
        (
            "one below the range minimum",
            json!({"HubTokenTimerExpirySettingInSeconds": EXPIRY_MINIMUM - 1}),
        ),
        (
            "a negative expiry",
            json!({"HubTokenTimerExpirySettingInSeconds": -1}),
        ),
        (
            "the smallest long",
            json!({"HubTokenTimerExpirySettingInSeconds": i64::MIN}),
        ),
        ("the required member absent", json!({})),
    ];

    for (label, body) in refused {
        let reply = call(&router, "PUT", "/hub-configuration", Some(body.clone())).await;
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

        let read = get_configuration(&router).await;
        assert_eq!(
            read.expiry(),
            Some(accepted),
            "case {label:?}: a refused put must leave the stored expiry {accepted} alone, got \
             {:?}",
            read.body
        );
        assert_eq!(
            read.updated_at(),
            Some(stamped),
            "case {label:?}: a refused put must not re-stamp the configuration, got {:?}",
            read.body
        );
    }
}

/// RULE: the configuration store is keyed by a region the CALLER chooses (the region of
/// its own credential scope), so the number of regions it holds is BOUNDED: a put for a
/// region it already holds always succeeds, and one for a further region past
/// `MAX_CONFIGURED_REGIONS` is refused with the modelled `ThrottlingException` (HTTP 429)
/// rather than growing the store. A scope region that is not shaped like a region name
/// is refused before it can become a key at all.
#[tokio::test]
async fn the_number_of_configured_regions_is_bounded() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    // Fill the store to its bound, one distinct signing region per put.
    let regions: Vec<String> = (0..MAX_CONFIGURED_REGIONS)
        .map(|index| format!("us-test-{index}"))
        .collect();
    for (index, region) in regions.iter().enumerate() {
        let expiry = i64::try_from(index).unwrap_or(i64::MAX) + EXPIRY_MINIMUM;
        let put = call_signed(
            &router,
            region,
            "PUT",
            "/hub-configuration",
            Some(json!({"HubTokenTimerExpirySettingInSeconds": expiry})),
        )
        .await;
        assert_eq!(
            put.status, 201,
            "case {region:?}: expected HTTP 201 while under the bound, got {} with body {:?}",
            put.status, put.body
        );
    }

    // One region past the bound is refused, and nothing is stored for it: it still
    // reports the default expiry and no stamp.
    let past = "us-test-past-the-bound";
    let refused = call_signed(
        &router,
        past,
        "PUT",
        "/hub-configuration",
        Some(json!({"HubTokenTimerExpirySettingInSeconds": 2_400})),
    )
    .await;
    assert_eq!(
        refused.status, 429,
        "case {past:?}: expected HTTP 429 past the bound of {MAX_CONFIGURED_REGIONS} regions, \
         got {} with body {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ThrottlingException"),
        "case {past:?}: expected the modelled ThrottlingException code, got {:?}",
        refused.error_type
    );
    let read = call_signed(&router, past, "GET", "/hub-configuration", None).await;
    assert_eq!(
        read.expiry(),
        Some(DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS),
        "case {past:?}: a refused region holds no configuration, got {:?}",
        read.body
    );
    assert_eq!(
        read.updated_at(),
        None,
        "case {past:?}: a refused region was never stamped, got {:?}",
        read.body
    );
    let held = engine
        .state::<ServiceState>()
        .lock()
        .hub_configurations
        .len();
    assert_eq!(
        held, MAX_CONFIGURED_REGIONS,
        "case {past:?}: the store must stay at {MAX_CONFIGURED_REGIONS} configurations, it \
         holds {held}"
    );

    // A region the store already holds is still writable at the bound.
    let already_held = regions.last().expect("a configured region");
    let again = call_signed(
        &router,
        already_held,
        "PUT",
        "/hub-configuration",
        Some(json!({"HubTokenTimerExpirySettingInSeconds": 777})),
    )
    .await;
    assert_eq!(
        again.status, 201,
        "case {already_held:?}: a region the store already holds stays writable at the bound, \
         got {} with body {:?}",
        again.status, again.body
    );
    let read = call_signed(&router, already_held, "GET", "/hub-configuration", None).await;
    assert_eq!(
        read.expiry(),
        Some(777),
        "case {already_held:?}: the overwrite must be what the region reports, got {:?}",
        read.body
    );
}

/// RULE: a credential scope whose region is not shaped like a region name never becomes a
/// store key: the put is refused with the modelled `ValidationException` (HTTP 400). The
/// rows span the ways the shape can fail \u2014 wrong characters, and a name past
/// `MAX_REGION_NAME_LENGTH`.
#[tokio::test]
async fn a_scope_region_that_is_not_a_region_name_is_refused() {
    let router = emulator();

    let too_long = "a".repeat(MAX_REGION_NAME_LENGTH + 1);
    let cases: &[(&str, &str)] = &[
        ("uppercase letters", "US-EAST-1"),
        ("punctuation outside the shape", "us_east_1"),
        ("one past the length maximum", too_long.as_str()),
    ];

    for (label, region) in cases {
        let reply = call_signed(
            &router,
            region,
            "PUT",
            "/hub-configuration",
            Some(json!({"HubTokenTimerExpirySettingInSeconds": 60})),
        )
        .await;
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
    }
}

/// RULE: the configuration is scoped to the region the request was signed for, the way
/// every other resource of this service is — so a put through one region's endpoint is
/// invisible through another's.
#[tokio::test]
async fn the_configuration_is_scoped_to_the_signing_region() {
    let router = emulator();

    // Two regions beside the test's default, so each row is read back both through its
    // own endpoint and through the others'.
    let rows: &[(&str, i64)] = &[
        ("us-east-1", 120),
        ("eu-west-1", 4_800),
        ("ap-southeast-2", EXPIRY_MINIMUM),
    ];

    for (region, expiry) in rows {
        let put = call_signed(
            &router,
            region,
            "PUT",
            "/hub-configuration",
            Some(json!({"HubTokenTimerExpirySettingInSeconds": expiry})),
        )
        .await;
        assert_eq!(
            put.status, 201,
            "case {region:?}: expected HTTP 201, got {} with body {:?}",
            put.status, put.body
        );
    }

    for (region, expiry) in rows {
        let read = call_signed(&router, region, "GET", "/hub-configuration", None).await;
        assert_eq!(
            read.expiry(),
            Some(*expiry),
            "case {region:?}: each region reports the expiry put through its own endpoint \
             ({expiry}), got {:?}",
            read.body
        );
    }

    // A region nobody configured still reports the default, with other regions
    // configured: a put through one endpoint does not leak into another's.
    let unconfigured = "sa-east-1";
    let read = call_signed(&router, unconfigured, "GET", "/hub-configuration", None).await;
    assert_eq!(
        read.status, 200,
        "case {unconfigured:?}: expected HTTP 200, got {} with body {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.expiry(),
        Some(DEFAULT_HUB_TOKEN_TIMER_EXPIRY_IN_SECONDS),
        "case {unconfigured:?}: another region's configuration must not leak here, so the \
         default is reported, got {:?}",
        read.body
    );
    assert_eq!(
        read.updated_at(),
        None,
        "case {unconfigured:?}: no put has stamped this region, got {:?}",
        read.body
    );
}
