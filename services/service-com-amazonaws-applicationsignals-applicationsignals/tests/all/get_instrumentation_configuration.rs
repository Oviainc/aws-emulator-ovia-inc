// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetInstrumentationConfiguration` over HTTP.
//!
//! Every test drives the real `restJson1` surface through the router that production mounts,
//! so the routes, the status codes and the JSON bodies are the ones a client sees.
//!
//! The rules under test:
//!
//! * a get reports exactly the configuration STORED under the identity it names — service,
//!   environment, instrumentation type, signal type and location — and reports it unchanged
//!   however often it is called;
//! * `LocationIdentifier` is "either full code location or a pre-computed hash", so a get
//!   naming the location and a get naming the hash the create answered reach the same
//!   record;
//! * an identity that names no stored configuration is the modelled
//!   `ResourceNotFoundException` (404), including one that differs from a stored
//!   configuration in a single part of its identity;
//! * a configuration whose `ExpiresAt` the clock has reached is "no longer served to
//!   clients".
//!
//! The last rule cannot be reached by requests alone: the documented floor on a BREAKPOINT
//! lifetime is 5 minutes, so no create can make a configuration that lapses during a test.
//! It is staged on the store the handlers share, through the very method they call.
//!
//! `CreateInstrumentationConfiguration`'s own rules (the hash, the ARN, the expiry window,
//! the conflict, the quota) are asserted in `instrumentation_configurations.rs`; here the
//! create is the writer whose record the get must report.

use std::sync::Arc;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_applicationsignals_applicationsignals::state::{
    instrumentation_key, ComAmazonawsApplicationsignalsApplicationsignalsState,
    InstrumentationScope,
};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification is
/// off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationSignalType")` -> `["SNAPSHOT"]`).
const SNAPSHOT: &str = "SNAPSHOT";

/// The two `InstrumentationType` values
/// (`smithy_query(query="shape",shape="InstrumentationType")` -> `["BREAKPOINT", "PROBE"]`).
const BREAKPOINT: &str = "BREAKPOINT";
const PROBE: &str = "PROBE";

/// The shortest expiry a BREAKPOINT create accepts, in seconds. `ExpiresAt`'s doc: "must be
/// between 5 min and 24 hours."
const MIN_EXPIRY_SECONDS: f64 = 5.0 * 60.0;

/// A router with just this service mounted on a fresh engine, together with the engine the
/// handlers share so a test can stage what the clock would have done.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// [`router_with_engine`] when the test only drives the wire surface.
fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when empty).
async fn call(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    let payload = serde_json::to_vec(body).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://application-signals.{REGION}.amazonaws.com{uri}"
        ))
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `POST /create-instrumentation-configuration`.
async fn create(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "/create-instrumentation-configuration", request).await
}

/// `POST /get-instrumentation-configuration`.
async fn get(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "/get-instrumentation-configuration", request).await
}

/// Seconds since the epoch, the form `restJson1` puts a `Timestamp` on the wire in.
fn now_seconds() -> f64 {
    DateTime::from(std::time::SystemTime::now())
        .as_secs_f64()
        .trunc()
}

/// A `CaptureConfiguration` with only its `@required` members, whose own members are all
/// optional (`smithy_query(query="shape",shape="CodeCaptureConfiguration,CaptureLimitsConfig")`).
fn minimal_capture() -> Value {
    json!({ "CodeCapture": { "CaptureLimits": {} } })
}

/// A get request naming the configuration `create_request` creates, by `identifier`.
fn get_request(create_request: &Value, identifier: Value) -> Value {
    json!({
        "Service": create_request["Service"],
        "Environment": create_request["Environment"],
        "SignalType": create_request["SignalType"],
        "InstrumentationType": create_request["InstrumentationType"],
        "LocationIdentifier": identifier,
    })
}

/// One row of the configuration table: a label and the create request that stores it.
struct Case {
    label: &'static str,
    request: Value,
}

/// The configurations the get is asserted over.
///
/// The rows differ along what the rules generalise over: the instrumentation TYPE (which
/// decides whether the record carries an `ExpiresAt` for the get to report), the programming
/// LANGUAGE (the whole modelled `ProgrammingLanguage` catalogue —
/// `smithy_query(query="shape",shape="ProgrammingLanguage")` -> `["Java", "Python",
/// "Javascript"]`), WHICH of `CodeLocation`'s optional members the location supplies (so the
/// hash the get resolves is taken over a different member set each time, including the
/// shortest and the longest the model admits), how many `AttributeFilters` groups and
/// `CodeCaptureConfiguration` members the stored record carries, whether it carries a
/// `Description`, and whether the create named an expiry or took the documented default.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "BREAKPOINT, Java, every optional location member, filters and tags",
            request: json!({
                "Service": "checkout",
                "Environment": "eks:cluster-prod/namespace",
                "SignalType": SNAPSHOT,
                "InstrumentationType": BREAKPOINT,
                "Description": "capture the cart total",
                "Location": { "CodeLocation": {
                    "Language": "Java",
                    "CodeUnit": "checkout-service.jar",
                    "ClassName": "com.example.CartService",
                    "MethodName": "total",
                    "FilePath": "src/main/java/com/example/CartService.java",
                    "LineNumber": 142
                }},
                "AttributeFilters": [
                    { "deployment.environment": "prod", "host.name": "ip-10-0-0-1" },
                    { "service.version": "1.4.2" }
                ],
                "CaptureConfiguration": { "CodeCapture": {
                    "CaptureArguments": ["cartId", "customerId"],
                    "CaptureReturn": true,
                    "CaptureStackTrace": true,
                    "CaptureLocals": ["total", "discount"],
                    "CaptureLimits": { "MaxHits": 25, "MaxStringLength": 120 }
                }},
                "Tags": [{ "Key": "team", "Value": "payments" }]
            }),
        },
        Case {
            label: "PROBE, Python, only the required location members",
            request: json!({
                "Service": "fulfilment",
                "Environment": "ec2:production",
                "SignalType": SNAPSHOT,
                "InstrumentationType": PROBE,
                "Location": { "CodeLocation": {
                    "Language": "Python",
                    "FilePath": "app/handlers/orders.py"
                }},
                "CaptureConfiguration": minimal_capture()
            }),
        },
        Case {
            label: "BREAKPOINT, Javascript, an expiry named inside the documented window",
            request: json!({
                "Service": "pricing",
                "Environment": "eks:cluster-staging/pricing",
                "SignalType": SNAPSHOT,
                "InstrumentationType": BREAKPOINT,
                // One minute inside the published 5-minute floor: the window is measured
                // from the instant the SERVICE reads its clock, which is after this request
                // is built, and a minute is far more than one in-process call takes.
                "ExpiresAt": now_seconds() + MIN_EXPIRY_SECONDS + 60.0,
                "Location": { "CodeLocation": {
                    "Language": "Javascript",
                    "FilePath": "src/pricing/quote.js",
                    "MethodName": "quote",
                    // The smallest `LineNumber` the modelled `range min 1` admits.
                    "LineNumber": 1
                }},
                "AttributeFilters": [{ "k": "v" }],
                "CaptureConfiguration": { "CodeCapture": {
                    "CaptureReturn": false,
                    "CaptureLimits": { "MaxStackFrames": 3 }
                }}
            }),
        },
        Case {
            // The shortest `Service`, `Environment` and `FilePath` the modelled
            // `length 1..255` / `length 1..1024` admit.
            label: "PROBE, shortest service, environment and file path the model admits",
            request: json!({
                "Service": "a",
                "Environment": "b",
                "SignalType": SNAPSHOT,
                "InstrumentationType": PROBE,
                "Location": { "CodeLocation": {
                    "Language": "Java",
                    "FilePath": "c"
                }},
                "CaptureConfiguration": minimal_capture()
            }),
        },
        Case {
            // The longest the same members admit, with `Description` at its modelled
            // `length 1..50` maximum and `MethodName` at its `length 1..80` maximum.
            label: "BREAKPOINT, longest service, environment, file path and description",
            request: json!({
                "Service": "s".repeat(255),
                "Environment": "e".repeat(255),
                "SignalType": SNAPSHOT,
                "InstrumentationType": BREAKPOINT,
                "Description": "d".repeat(50),
                "Location": { "CodeLocation": {
                    "Language": "Javascript",
                    "CodeUnit": "u".repeat(128),
                    "MethodName": "m".repeat(80),
                    "FilePath": "f".repeat(1024)
                }},
                "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": {
                    "MaxHits": 1000,
                    "MaxStringLength": 255,
                    "MaxCollectionWidth": 20,
                    "MaxCollectionDepth": 5,
                    "MaxStackFrames": 20,
                    "MaxStackTraceSize": 1000,
                    "MaxObjectDepth": 5,
                    "MaxFieldsPerObject": 20
                }}}
            }),
        },
    ]
}

/// A get reports exactly the record the create stored, under either form of
/// `LocationIdentifier`, and reports it unchanged on every call.
#[tokio::test]
async fn a_get_reports_the_stored_configuration_under_either_location_identifier() {
    for case in cases() {
        let label = case.label;
        let router = router();

        let (status, created) = create(&router, &case.request).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 from the create, got {status} with {created}"
        );
        let hash = created["LocationHash"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // `GetInstrumentationConfigurationResponse$Configuration` is "the complete
        // instrumentation configuration", whose members are exactly those the create
        // answered, so the whole object must come back equal to the stored record — the
        // location, the capture settings, the hash, the ARN, `CreatedAt` and `ExpiresAt`
        // included.
        let by_location = get_request(
            &case.request,
            json!({ "CodeLocation": case.request["Location"]["CodeLocation"] }),
        );
        let by_hash = get_request(&case.request, json!({ "LocationHash": hash }));
        for (form, request) in [("code location", &by_location), ("location hash", &by_hash)] {
            // Twice, because a read must not change what a later caller sees.
            for attempt in 1..=2 {
                let (status, fetched) = get(&router, request).await;
                assert_eq!(
                    status, 200,
                    "case {label:?} by {form} attempt {attempt}: expected HTTP 200 from the get, got {status} with {fetched}"
                );
                assert_eq!(
                    fetched["Configuration"], created,
                    "case {label:?} by {form} attempt {attempt}: expected the stored record {created}, got {:?}",
                    fetched["Configuration"]
                );
            }
        }
    }
}

/// An identity that names no stored configuration is the modelled
/// `ResourceNotFoundException` (404), which identifies the resource by the type name and by
/// the ARN that identity would carry — and the stored configuration survives every miss.
#[tokio::test]
async fn a_get_of_an_identity_that_names_nothing_is_not_found() {
    for case in cases() {
        let label = case.label;
        let router = router();
        let (status, created) = create(&router, &case.request).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the setup create to succeed, got {status} with {created}"
        );
        let hash = created["LocationHash"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // Each row perturbs exactly ONE part of the stored identity.
        let other_type = if case.request["InstrumentationType"] == json!(BREAKPOINT) {
            PROBE
        } else {
            BREAKPOINT
        };
        let mut misses = vec![
            (
                "the other instrumentation type",
                json!({
                    "Service": case.request["Service"],
                    "Environment": case.request["Environment"],
                    "SignalType": SNAPSHOT,
                    "InstrumentationType": other_type,
                    "LocationIdentifier": { "LocationHash": hash.clone() },
                }),
            ),
            (
                "another service",
                json!({
                    "Service": "no-such-service",
                    "Environment": case.request["Environment"],
                    "SignalType": SNAPSHOT,
                    "InstrumentationType": case.request["InstrumentationType"],
                    "LocationIdentifier": { "LocationHash": hash.clone() },
                }),
            ),
            (
                "another environment",
                json!({
                    "Service": case.request["Service"],
                    "Environment": "ec2:no-such-environment",
                    "SignalType": SNAPSHOT,
                    "InstrumentationType": case.request["InstrumentationType"],
                    "LocationIdentifier": { "LocationHash": hash.clone() },
                }),
            ),
            (
                "a location hash nothing was stored under",
                json!({
                    "Service": case.request["Service"],
                    "Environment": case.request["Environment"],
                    "SignalType": SNAPSHOT,
                    "InstrumentationType": case.request["InstrumentationType"],
                    "LocationIdentifier": { "LocationHash": "0123456789abcdef" },
                }),
            ),
        ];
        // A location differing from the stored one in a single member hashes differently, so
        // it names no stored configuration either.
        let mut moved = case.request["Location"]["CodeLocation"].clone();
        moved["FilePath"] = json!("src/elsewhere/Other.java");
        misses.push((
            "another code location",
            get_request(&case.request, json!({ "CodeLocation": moved })),
        ));

        for (why, request) in misses {
            let (status, error) = get(&router, &request).await;
            assert_eq!(
                status, 404,
                "case {label:?} with {why}: expected HTTP 404, got {status} with {error}"
            );
            assert_eq!(
                error["__type"].as_str(),
                Some("ResourceNotFoundException"),
                "case {label:?} with {why}: expected a ResourceNotFoundException, got {error}"
            );
            assert_eq!(
                error["ResourceType"].as_str(),
                Some("InstrumentationConfiguration"),
                "case {label:?} with {why}: expected the resource type name, got {error}"
            );
            let reported = error["ResourceId"].as_str().unwrap_or_default();
            assert!(
                reported.contains(":instrumentationConfig/"),
                "case {label:?} with {why}: expected ResourceId to be a configuration ARN, got {reported:?}"
            );
        }

        // None of those misses disturbed the stored record.
        let (status, fetched) = get(
            &router,
            &get_request(&case.request, json!({ "LocationHash": hash })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the stored configuration to survive the misses, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["Configuration"], created,
            "case {label:?}: expected the stored record to be unchanged, got {:?}",
            fetched["Configuration"]
        );
    }
}

/// A get of a scope that holds nothing at all — nothing was ever created in this emulator —
/// is the same 404, so an empty store and a wrong identity answer alike.
#[tokio::test]
async fn a_get_on_an_empty_store_is_not_found() {
    let router = router();
    for (label, identifier) in [
        (
            "by a code location",
            json!({ "CodeLocation": {
                "Language": "Java",
                "FilePath": "src/main/java/com/example/CartService.java",
                "LineNumber": 142
            }}),
        ),
        (
            "by a location hash",
            json!({ "LocationHash": "fedcba9876543210" }),
        ),
    ] {
        let (status, error) = get(
            &router,
            &json!({
                "Service": "checkout",
                "Environment": "ec2:production",
                "SignalType": SNAPSHOT,
                "InstrumentationType": BREAKPOINT,
                "LocationIdentifier": identifier,
            }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404 on an empty store, got {status} with {error}"
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected a ResourceNotFoundException, got {error}"
        );
    }
}

/// `ExpiresAt` is "the timestamp after which this configuration is no longer served to
/// clients": once the clock has reached it the get reports the configuration as not found,
/// while a configuration that does not expire keeps being served.
///
/// The documented floor on a BREAKPOINT lifetime is 5 minutes, so the passage of time is
/// staged on the store the handlers share, by storing the same record with the expiry the
/// clock has already passed — through `put_instrumentation_configuration`, the very method
/// the create path calls.
#[tokio::test]
async fn a_configuration_whose_expiry_has_passed_is_no_longer_served() {
    let (router, engine) = router_with_engine();
    let service = "checkout";
    let environment = "ec2:production";
    let location = json!({
        "Language": "Java",
        "FilePath": "src/main/java/com/example/CartService.java",
        "LineNumber": 142
    });

    // One BREAKPOINT configuration, which expires, and one PROBE configuration at the same
    // location, which does not.
    let breakpoint_request = json!({
        "Service": service,
        "Environment": environment,
        "SignalType": SNAPSHOT,
        "InstrumentationType": BREAKPOINT,
        "Location": { "CodeLocation": location },
        "CaptureConfiguration": minimal_capture(),
        "Tags": [{ "Key": "team", "Value": "payments" }]
    });
    let mut probe_request = breakpoint_request.clone();
    probe_request["InstrumentationType"] = json!(PROBE);
    probe_request["Tags"] = json!([]);

    let (status, created) = create(&router, &breakpoint_request).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the BREAKPOINT create, got {status} with {created}"
    );
    let (status, permanent) = create(&router, &probe_request).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the PROBE create, got {status} with {permanent}"
    );

    let hash = created["LocationHash"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let temporary = get_request(&breakpoint_request, json!({ "LocationHash": hash.clone() }));
    let permanent_request = get_request(&probe_request, json!({ "LocationHash": hash.clone() }));

    let (status, fetched) = get(&router, &temporary).await;
    assert_eq!(
        status, 200,
        "expected the configuration to be served before it expires, got {status} with {fetched}"
    );

    // Stage what the clock will do: the same record, with an expiry it has already passed.
    let scope = InstrumentationScope::new(BREAKPOINT, service, environment);
    let key = instrumentation_key(SNAPSHOT, &hash);
    let lapsed_at = DateTime::from_secs_f64(now_seconds() - 1.0);
    {
        let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
        let mut store = state.lock();
        let mut record = store
            .instrumentation_configuration(&scope, &key)
            .expect("the create stored the configuration under its identity")
            .clone();
        record.expires_at = Some(lapsed_at);
        store.put_instrumentation_configuration(scope.clone(), record, lapsed_at);
    }

    let (status, error) = get(&router, &temporary).await;
    assert_eq!(
        status, 404,
        "expected an expired configuration to be reported as not found, got {status} with {error}"
    );
    assert_eq!(
        error["__type"].as_str(),
        Some("ResourceNotFoundException"),
        "expected a ResourceNotFoundException for an expired configuration, got {error}"
    );

    // The record is gone from the store rather than merely hidden, and so are its tags.
    {
        let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
        let store = state.lock();
        assert!(
            store.instrumentation_configuration(&scope, &key).is_none(),
            "expected the lapsed configuration to be dropped from the store"
        );
        assert!(
            store.tags.is_empty(),
            "expected the lapsed configuration's tags to be dropped, {} remain",
            store.tags.len()
        );
    }

    // The PROBE configuration at the same location "persists until explicitly deleted", so
    // the expiry pass left it alone and the get still reports it.
    let (status, still_there) = get(&router, &permanent_request).await;
    assert_eq!(
        status, 200,
        "expected a permanent configuration to survive the expiry pass, got {status} with {still_there}"
    );
    assert_eq!(
        still_there["Configuration"], permanent,
        "expected the get to report the stored PROBE record, got {:?}",
        still_there["Configuration"]
    );

    // And the lapsed location is free again: a new create succeeds and the get reports the
    // NEW record.
    let (status, recreated) = create(&router, &breakpoint_request).await;
    assert_eq!(
        status, 200,
        "expected the freed location to accept a new configuration, got {status} with {recreated}"
    );
    let (status, fetched) = get(&router, &temporary).await;
    assert_eq!(
        status, 200,
        "expected the new configuration to be served, got {status} with {fetched}"
    );
    assert_eq!(
        fetched["Configuration"], recreated,
        "expected the get to report the new record, got {:?}",
        fetched["Configuration"]
    );
}
