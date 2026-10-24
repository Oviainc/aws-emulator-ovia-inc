// SPDX-License-Identifier: CC0-1.0

//! The runtime-log-configuration cycle, end to end over the wire:
//! `PutRuntimeLogConfiguration` -> `GetRuntimeLogConfiguration` ->
//! `ResetRuntimeLogConfiguration`.
//!
//! RULE: a runtime log configuration is installed ON a managed thing. A put
//! (PUT `/runtime-log-configurations/{ManagedThingId}`, success 200, `@idempotent`) resolves
//! the managed thing FIRST — a `ManagedThingId` the caller's region holds no managed thing
//! under is the modelled `ResourceNotFoundException` (HTTP 404) and installs nothing — and
//! otherwise REPLACES the thing's configuration with exactly the `RuntimeLogConfigurations`
//! structure submitted, whichever of its members the request carries. A get (GET on the
//! same URI, `@readonly`) reports back exactly the installed structure and the managed thing
//! it belongs to, and is the modelled 404 while the thing has none installed. A reset
//! (DELETE on the same URI, success 200, `@idempotent`) returns the thing to having none, so
//! the get is the modelled 404 again and a second reset is the same success. Runtime logging
//! is supported only on a managed thing that RUNS the managed-integrations runtime — a hub,
//! which is a managed thing carrying a `HubNetworkMode` — so a put against any other managed
//! thing is the modelled `ValidationException` (HTTP 400) and YET installs the submitted
//! configuration, which is the recorded behaviour of this cycle.
//!
//! The recorded unit this was derived from
//! (`iot-managed-integrations.runtime-log-configuration-put-get-reset-cycle`) is ONE row of
//! the tables below. The rows differ along what the rule generalises over: which members of
//! the configuration the request carries, the `LogLevel` values (the model's whole value
//! set), the sign and size of the numeric members, the booleans, whether the managed thing
//! is a hub (and under which `HubNetworkMode`), the form of a `ManagedThingId` that resolves
//! to nothing, and the region the request is signed for.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses, bodies and modelled errors asserted here are the ones a client
//! sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::ops::types::RuntimeLogConfigurations;
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    HUB_NETWORK_MODE_VALUES, LOG_LEVEL_VALUES, MANAGED_THING_RESOURCE_TYPE,
    RUNTIME_LOG_CONFIGURATION_RESOURCE_TYPE,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::RegionalName;
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::{
    ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState as ServiceState, EmulatedService,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The wire error codes the model's `@httpError` shapes are rendered under.
const VALIDATION: &str = "ValidationException";
const NOT_FOUND: &str = "ResourceNotFoundException";

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

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    /// The reason an error body carries, on the member the model declares for it.
    fn message(&self) -> &str {
        self.string("Message")
            .or_else(|| self.string("message"))
            .unwrap_or_default()
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    emulator_over(Arc::new(engine::InMemoryEngine::new()))
}

/// [`emulator`] over an engine the caller keeps a handle to, so a test can seed the
/// service's store directly instead of driving requests it has no API for.
fn emulator_over(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call_in(
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

/// The runtime-log-configuration URI of `id`
/// (`smithy_query(query="operation",operation="PutRuntimeLogConfiguration")` ->
/// `/runtime-log-configurations/{ManagedThingId}`).
fn uri(id: &str) -> String {
    format!("/runtime-log-configurations/{id}")
}

/// `PutRuntimeLogConfiguration` of `configurations` on `id`, signed for `region`.
async fn put_in(
    router: &MultiServiceRouter,
    region: &str,
    id: &str,
    configurations: &Value,
) -> Reply {
    call_in(
        router,
        region,
        "PUT",
        &uri(id),
        Some(json!({"RuntimeLogConfigurations": configurations})),
    )
    .await
}

/// [`put_in`] in [`REGION`].
async fn put(router: &MultiServiceRouter, id: &str, configurations: &Value) -> Reply {
    put_in(router, REGION, id, configurations).await
}

/// `GetRuntimeLogConfiguration` of `id`, signed for `region`.
async fn get_in(router: &MultiServiceRouter, region: &str, id: &str) -> Reply {
    call_in(router, region, "GET", &uri(id), None).await
}

/// [`get_in`] in [`REGION`].
async fn get(router: &MultiServiceRouter, id: &str) -> Reply {
    get_in(router, REGION, id).await
}

/// `ResetRuntimeLogConfiguration` of `id`, signed for `region`.
async fn reset_in(router: &MultiServiceRouter, region: &str, id: &str) -> Reply {
    call_in(router, region, "DELETE", &uri(id), None).await
}

/// [`reset_in`] in [`REGION`].
async fn reset(router: &MultiServiceRouter, id: &str) -> Reply {
    reset_in(router, REGION, id).await
}

/// Create a managed thing in `region` and answer its generated id.
async fn managed_thing_in(router: &MultiServiceRouter, region: &str, case: &str) -> String {
    let created = call_in(
        router,
        region,
        "POST",
        "/managed-things",
        Some(json!({
            "Role": "CONTROLLER",
            "AuthenticationMaterialType": "WIFI_SETUP_QR_BAR_CODE",
            "AuthenticationMaterial": "wifi:setup:0123456789",
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateManagedThing, got {} with body {:?}",
        created.status, created.body
    );
    created
        .string("Id")
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected CreateManagedThing to answer an Id, got body {:?}",
                created.body
            )
        })
        .to_string()
}

/// [`managed_thing_in`] in [`REGION`].
async fn managed_thing(router: &MultiServiceRouter, case: &str) -> String {
    managed_thing_in(router, REGION, case).await
}

/// Make the managed thing `id` a HUB by setting its `HubNetworkMode` through
/// `UpdateManagedThing` — the only call that sets it, and what makes runtime logging
/// supported on the thing.
async fn make_hub(router: &MultiServiceRouter, case: &str, id: &str, mode: &str) {
    let updated = call_in(
        router,
        REGION,
        "PUT",
        &format!("/managed-things/{id}"),
        Some(json!({"HubNetworkMode": mode})),
    )
    .await;
    assert_eq!(
        updated.status, 204,
        "case {case:?}: expected HTTP 204 from UpdateManagedThing, got {} with body {:?}",
        updated.status, updated.body
    );
}

/// Assert one answer is the modelled refusal `status`/`code` carrying a reason, and report
/// the `ResourceType` it named (absent for a refusal that declares no resource).
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(code),
        "case {case:?}: expected the modelled {code} code, got {:?}",
        reply.error_type
    );
    assert!(
        !reply.message().is_empty(),
        "case {case:?}: expected the modelled Message member to carry a reason, got body {:?}",
        reply.body
    );
}

/// Assert one get reports exactly `configurations` installed on `id`.
fn assert_reports(case: &str, reply: &Reply, id: &str, configurations: &Value) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("ManagedThingId"),
        Some(id),
        "case {case:?}: expected ManagedThingId {id:?}, got {:?} in body {:?}",
        reply.string("ManagedThingId"),
        reply.body
    );
    assert_eq!(
        reply.member("RuntimeLogConfigurations"),
        Some(configurations),
        "case {case:?}: expected the installed configuration {configurations:?}, got {:?}",
        reply.member("RuntimeLogConfigurations")
    );
}

/// The configuration table: every row is a `RuntimeLogConfigurations` structure a put may
/// carry, and the rows differ along which members are present, the `LogLevel` values (the
/// model's whole value set, on both `LogLevel` and `LogFlushLevel`), the numbers and the
/// booleans. Member names and types are the model's
/// (`smithy_query(query="shape",shape="RuntimeLogConfigurations")`).
fn configurations() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = Vec::new();
    // One member only, once for every value `LogLevel` declares.
    for level in LOG_LEVEL_VALUES {
        rows.push((format!("LogLevel {level}"), json!({"LogLevel": level})));
    }
    // The two level members together, pairing each value with the next one in the set.
    for (index, level) in LOG_LEVEL_VALUES.iter().enumerate() {
        let flush = LOG_LEVEL_VALUES[(index + 1) % LOG_LEVEL_VALUES.len()];
        rows.push((
            format!("LogLevel {level} flushing at {flush}"),
            json!({"LogLevel": level, "LogFlushLevel": flush}),
        ));
    }
    rows.push((
        "uploading enabled".to_string(),
        json!({"LogLevel": "INFO", "UploadLog": true, "UploadPeriodMinutes": 5}),
    ));
    rows.push((
        "uploading disabled".to_string(),
        json!({"LogLevel": "ERROR", "UploadLog": false}),
    ));
    rows.push((
        "every member".to_string(),
        json!({
            "LogLevel": "DEBUG",
            "LogFlushLevel": "WARN",
            "LocalStoreLocation": "/var/log/managed-integrations",
            "LocalStoreFileRotationMaxFiles": 10,
            "LocalStoreFileRotationMaxBytes": 1_048_576,
            "UploadLog": true,
            "UploadPeriodMinutes": 60,
            "DeleteLocalStoreAfterUpload": true
        }),
    ));
    rows.push((
        // The numeric members are plain `integer`s, so zero and a negative value are
        // inside their shapes and must round-trip unchanged.
        "zero and negative numbers".to_string(),
        json!({
            "LogLevel": "WARN",
            "LocalStoreFileRotationMaxFiles": 0,
            "LocalStoreFileRotationMaxBytes": -1,
            "UploadPeriodMinutes": 0,
            "DeleteLocalStoreAfterUpload": false
        }),
    ));
    rows.push((
        // The structure is `@required`, but every one of its members is optional: an empty
        // structure installs a configuration that sets nothing.
        "no members at all".to_string(),
        json!({}),
    ));
    rows
}

/// RULE: a put installs exactly the submitted configuration on a hub, a get reads it back
/// unchanged, and a reset returns the hub to having none — for every shape of the
/// configuration structure and every `HubNetworkMode` a hub can carry.
#[tokio::test]
async fn a_put_installs_the_configuration_a_get_reads_it_back_and_a_reset_removes_it() {
    for (row, (case, configuration)) in configurations().into_iter().enumerate() {
        // A fresh store per row, so the only configuration it holds is the row's own.
        let router = emulator();
        let id = managed_thing(&router, &case).await;
        // Every `HubNetworkMode` the model declares is exercised, cycling over the rows.
        let mode = HUB_NETWORK_MODE_VALUES[row % HUB_NETWORK_MODE_VALUES.len()];
        make_hub(&router, &case, &id, mode).await;
        let case = format!("{case} on a {mode} hub");

        // Before the put the hub has none installed.
        assert_refused(&format!("{case}/before"), &get(&router, &id).await, 404, NOT_FOUND);

        let installed = put(&router, &id, &configuration).await;
        assert_eq!(
            installed.status, 200,
            "case {case:?}: expected HTTP 200 from the put, got {} with body {:?}",
            installed.status, installed.body
        );

        // The get reports exactly what was submitted, and repeating it reports the same
        // thing: a get is a pure read.
        assert_reports(&case, &get(&router, &id).await, &id, &configuration);
        assert_reports(
            &format!("{case}/reread"),
            &get(&router, &id).await,
            &id,
            &configuration,
        );

        // A put REPLACES the configuration: installing another one leaves exactly that one.
        let replacement = json!({"LogLevel": "ERROR", "UploadLog": true});
        let replaced = put(&router, &id, &replacement).await;
        assert_eq!(
            replaced.status, 200,
            "case {case:?}: expected HTTP 200 from the replacing put, got {} with body {:?}",
            replaced.status, replaced.body
        );
        assert_reports(
            &format!("{case}/replaced"),
            &get(&router, &id).await,
            &id,
            &replacement,
        );

        // The reset removes it, so the get is the modelled 404 again.
        let cleared = reset(&router, &id).await;
        assert_eq!(
            cleared.status, 200,
            "case {case:?}: expected HTTP 200 from the reset, got {} with body {:?}",
            cleared.status, cleared.body
        );
        let after = get(&router, &id).await;
        assert_refused(&format!("{case}/after reset"), &after, 404, NOT_FOUND);
        assert_eq!(
            after.string("ResourceType"),
            Some(RUNTIME_LOG_CONFIGURATION_RESOURCE_TYPE),
            "case {case:?}: expected the 404 of a hub with nothing installed to name the \
             runtime-log-configuration resource type, got {:?}",
            after.string("ResourceType")
        );

        // The reset is `@idempotent`: resetting again is the same success.
        let again = reset(&router, &id).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: expected a second reset to be HTTP 200, got {} with body {:?}",
            again.status, again.body
        );
    }
}

/// RULE: a `ManagedThingId` the caller's region holds no managed thing under is the modelled
/// 404 on all three operations and installs nothing — whatever form the identifier takes,
/// and including a managed thing another region's endpoint created.
#[tokio::test]
async fn a_managed_thing_the_region_does_not_hold_is_the_modelled_not_found() {
    let router = emulator();

    // A managed thing created through another region's endpoint, to address from this one.
    let elsewhere = managed_thing_in(&router, "eu-central-1", "eu-central-1 thing").await;

    // The rows differ along why this region holds no managed thing under the id: nothing
    // ever created it (in several forms the `ManagedThingId` pattern `^[a-zA-Z0-9:_-]*$`
    // admits), or it belongs to another region's endpoint.
    let cases: Vec<(&str, String)> = vec![
        ("never created", "sim2realbogusmanagedthingid".to_string()),
        ("never created, mixed case", "NoSuchManagedThing1".to_string()),
        ("never created, punctuation", "thing:1_2-3".to_string()),
        (
            "never created, at the modelled maximum",
            "a".repeat(64),
        ),
        ("another region's managed thing", elsewhere.clone()),
    ];

    let configuration = json!({"LogLevel": "INFO"});
    for (why, id) in cases {
        let refused = put(&router, &id, &configuration).await;
        assert_refused(why, &refused, 404, NOT_FOUND);
        assert_eq!(
            refused.string("ResourceType"),
            Some(MANAGED_THING_RESOURCE_TYPE),
            "case {why:?}: expected the 404 to name the managed-thing resource type, got {:?}",
            refused.string("ResourceType")
        );
        assert_eq!(
            refused.string("ResourceId"),
            Some(id.as_str()),
            "case {why:?}: expected the 404 to report the identifier the request carried, \
             got {:?}",
            refused.string("ResourceId")
        );

        // Nothing was installed: the get and the reset report the same 404.
        assert_refused(&format!("{why}/get"), &get(&router, &id).await, 404, NOT_FOUND);
        assert_refused(
            &format!("{why}/reset"),
            &reset(&router, &id).await,
            404,
            NOT_FOUND,
        );
    }

    // The managed thing the other region created is still addressable there, and holds
    // nothing: the refused puts installed nothing anywhere.
    assert_refused(
        "eu-central-1 read back",
        &get_in(&router, "eu-central-1", &elsewhere).await,
        404,
        NOT_FOUND,
    );
    let installed = put_in(&router, "eu-central-1", &elsewhere, &configuration).await;
    assert_eq!(
        installed.status, 400,
        "case \"eu-central-1 put\": a managed thing that is not a hub is refused, got {} with \
         body {:?}",
        installed.status, installed.body
    );
}

/// RULE: runtime logging is supported only on a managed thing that runs the
/// managed-integrations runtime — a hub — so a put against any other managed thing is the
/// modelled 400 and YET installs the submitted configuration, which the get then reports and
/// the reset removes.
#[tokio::test]
async fn a_put_against_a_managed_thing_that_is_not_a_hub_is_refused_and_still_installs_it() {
    for (case, configuration) in configurations() {
        let router = emulator();
        // A managed thing no call has given a `HubNetworkMode`: not a hub, so runtime
        // logging is not supported on it.
        let id = managed_thing(&router, &case).await;

        let refused = put(&router, &id, &configuration).await;
        assert_refused(&case, &refused, 400, VALIDATION);

        // And yet the configuration is installed: the get that answered 404 a moment
        // earlier now reports exactly what the refused put submitted.
        assert_reports(
            &format!("{case}/installed anyway"),
            &get(&router, &id).await,
            &id,
            &configuration,
        );

        // The reset removes it on the same terms as on a hub.
        let cleared = reset(&router, &id).await;
        assert_eq!(
            cleared.status, 200,
            "case {case:?}: expected HTTP 200 from the reset, got {} with body {:?}",
            cleared.status, cleared.body
        );
        assert_refused(
            &format!("{case}/after reset"),
            &get(&router, &id).await,
            404,
            NOT_FOUND,
        );

        // Making the thing a hub is what makes the same put succeed, so the refusal is the
        // device class and nothing about the request.
        make_hub(&router, &case, &id, HUB_NETWORK_MODE_VALUES[0]).await;
        let accepted = put(&router, &id, &configuration).await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: expected the same put on a hub to be HTTP 200, got {} with body {:?}",
            accepted.status, accepted.body
        );
    }
}

/// RULE: a configuration belongs to the managed thing of ONE region, so the put, the get and
/// the reset all resolve the SAME (region, `ManagedThingId`) pair — the gate that resolves
/// the managed thing and the write that installs or removes its configuration address one
/// keyspace, so a call signed for one region cannot read or remove the configuration another
/// region holds under the same id.
#[tokio::test]
async fn a_configuration_belongs_to_one_region_s_managed_thing() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_over(Arc::clone(&engine));

    let id = managed_thing(&router, "the managed thing").await;
    let here = json!({"LogLevel": "DEBUG"});

    // A configuration another region holds under the SAME id. Identifiers are minted per
    // record, so no request can put two regions' records under one id: the other region's
    // entry is seeded directly, which is exactly the state a region-blind key would let
    // this region's call reach.
    let other_region = "eu-central-1";
    let other_key = RegionalName::new(other_region, &id);
    {
        let state = engine.state::<ServiceState>();
        let mut store = state.lock();
        store.runtime_log_configurations.insert(
            other_key.clone(),
            RuntimeLogConfigurations {
                log_level: Some("ERROR".to_string()),
                upload_log: Some(true),
                ..RuntimeLogConfigurations::default()
            },
        );
    }

    // This region installs its own, which neither reads nor replaces the other region's.
    assert_eq!(
        put(&router, &id, &here).await.status,
        400,
        "case \"the put\": the managed thing is not a hub, so the put is refused and still \
         installs"
    );
    assert_reports("the get", &get(&router, &id).await, &id, &here);

    // And the reset removes only this region's entry: the other region's is untouched.
    assert_eq!(
        reset(&router, &id).await.status,
        200,
        "case \"the reset\": expected the reset to be accepted"
    );
    assert_refused("after the reset", &get(&router, &id).await, 404, NOT_FOUND);
    let state = engine.state::<ServiceState>();
    let store = state.lock();
    assert_eq!(
        store
            .runtime_log_configuration_in(other_region, &id)
            .and_then(|configuration| configuration.log_level.clone()),
        Some("ERROR".to_string()),
        "case \"after the reset\": the other region's configuration is untouched, got {:?}",
        store.runtime_log_configuration_in(other_region, &id)
    );
    assert!(
        store.runtime_log_configuration_in(REGION, &id).is_none(),
        "case \"after the reset\": this region's configuration is gone, got {:?}",
        store.runtime_log_configuration_in(REGION, &id)
    );
}

/// RULE: a configuration is installed on ONE managed thing, so a put on one thing does not
/// change what another thing reports.
#[tokio::test]
async fn a_configuration_belongs_to_the_managed_thing_it_was_installed_on() {
    let router = emulator();

    let first = managed_thing(&router, "first").await;
    let second = managed_thing(&router, "second").await;
    make_hub(&router, "first", &first, "STANDARD").await;
    make_hub(&router, "second", &second, "NETWORK_WIDE_EXCLUSION").await;

    let first_configuration = json!({"LogLevel": "DEBUG", "UploadLog": true});
    let second_configuration = json!({"LogLevel": "ERROR", "UploadPeriodMinutes": 15});

    assert_eq!(
        put(&router, &first, &first_configuration).await.status,
        200,
        "case \"first\": expected the put to be accepted"
    );
    // The second thing still has none while only the first was configured.
    assert_refused(
        "second before its own put",
        &get(&router, &second).await,
        404,
        NOT_FOUND,
    );

    assert_eq!(
        put(&router, &second, &second_configuration).await.status,
        200,
        "case \"second\": expected the put to be accepted"
    );
    assert_reports(
        "first after both puts",
        &get(&router, &first).await,
        &first,
        &first_configuration,
    );
    assert_reports(
        "second after both puts",
        &get(&router, &second).await,
        &second,
        &second_configuration,
    );

    // Resetting one leaves the other installed.
    assert_eq!(
        reset(&router, &first).await.status,
        200,
        "case \"first\": expected the reset to be accepted"
    );
    assert_refused(
        "first after its reset",
        &get(&router, &first).await,
        404,
        NOT_FOUND,
    );
    assert_reports(
        "second after the other's reset",
        &get(&router, &second).await,
        &second,
        &second_configuration,
    );
}
