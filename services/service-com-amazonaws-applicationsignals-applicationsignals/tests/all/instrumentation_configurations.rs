// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the dynamic instrumentation configurations over HTTP:
//! `CreateInstrumentationConfiguration` and `ListInstrumentationConfigurations`.
//!
//! Every test drives the real `restJson1` surface through the router that production mounts,
//! so the routes, the status codes and the JSON bodies are the ones a client sees.
//!
//! The rules under test:
//!
//! * a list reports exactly the configurations STORED for the service, environment and
//!   instrumentation type it names, each projected without the service and environment the
//!   request already states, a page at a time in a stable order;
//! * a create stores one configuration under the identity (service, environment,
//!   instrumentation type, signal type, location) and answers the stored record — a 16
//!   lowercase-hex `LocationHash` that is a function of the location alone, the ARN the
//!   model documents, the clock's `CreatedAt`, and `ExpiresAt` only for the type that
//!   expires — and refuses a second create at the same identity;
//! * a list that supplies `SyncedAt` is an incremental sync: it reports `Changed: false` and
//!   omits the configuration details when the scope's set has not changed since then — a gate
//!   on starting a listing, so a request that also carries a `NextToken` is a continuation and
//!   is served the page its cursor names whatever watermark it supplies.
//!
//! One rule cannot be reached over HTTP at all: a configuration's lapse. The documented
//! floor on a BREAKPOINT lifetime is 5 minutes, so no request can create a configuration
//! that expires within a test. That rule is asserted on the store the handlers share,
//! through the same methods they call.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use service_com_amazonaws_applicationsignals_applicationsignals::ops::types::{
    CaptureConfiguration, CaptureLimitsConfig, CodeCaptureConfiguration, CodeLocation,
    InstrumentationConfiguration, Location,
};
use service_com_amazonaws_applicationsignals_applicationsignals::state::{
    ComAmazonawsApplicationsignalsApplicationsignalsState, InstrumentationScope,
    EMPTY_SCOPE_RETENTION, MAX_INSTRUMENTATION_CONFIGURATIONS,
};

// The conflict sentence, written once for this crate's tests in the suite that owns the rule
// wording it.
use crate::refusal_wording::CONFIGURATION_CONFLICT;
// The capture configuration a create STORES for a request, written once for this crate's
// tests in the suite that owns the defaulting rule.
use crate::instrumentation_capture_defaults::settled;

/// The SigV4 credential scope routes the request to this service. Signature verification is
/// off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

/// The canonical emulator account id this store belongs to (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationSignalType")` -> `["SNAPSHOT"]`).
const SNAPSHOT: &str = "SNAPSHOT";

/// The two `InstrumentationType` values
/// (`smithy_query(query="shape",shape="InstrumentationType")` -> `["BREAKPOINT", "PROBE"]`).
const BREAKPOINT: &str = "BREAKPOINT";
const PROBE: &str = "PROBE";

/// `ExpiresAt`'s documented window for a BREAKPOINT configuration: "must be between 5 min
/// and 24 hours", and "defaults to 24 hours".
const MIN_EXPIRY_SECONDS: i64 = 5 * 60;
const MAX_EXPIRY_SECONDS: i64 = 24 * 60 * 60;

/// How far INSIDE a bound a row that must be served probes it.
///
/// The service measures the window from the instant it serves the request, which is after
/// the instant a test reads the clock; normally by a fraction of a second, but by whole
/// seconds whenever the machine running the suite stalls in between. A margin wider than any
/// such stall is what makes "inside the bound" mean the same thing to the test and to the
/// service, while staying far from the bound's own value, which the refusing rows probe to
/// the second.
const SERVED_MARGIN_SECONDS: i64 = 30;

/// A router with just this service mounted on a fresh engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |body| {
        serde_json::to_vec(&body).expect("serialize the request body")
    });
    let request = http::Request::builder()
        .method(method)
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
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/create-instrumentation-configuration",
        Some(body),
    )
    .await
}

/// `POST /list-instrumentation-configurations`.
async fn list(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/list-instrumentation-configurations",
        Some(body),
    )
    .await
}

/// The scope every instrumentation operation addresses: the service, the environment it runs
/// in, and the instrumentation type ("Required to determine which backing store to query").
#[derive(Clone, Debug)]
struct Scope {
    instrumentation_type: &'static str,
    service: &'static str,
    environment: &'static str,
}

impl Scope {
    fn list_body(&self) -> Value {
        json!({
            "InstrumentationType": self.instrumentation_type,
            "Service": self.service,
            "Environment": self.environment,
        })
    }
}

/// One row of the configuration table: a scope, the location to instrument and the optional
/// members the create may carry.
#[derive(Clone, Debug)]
struct Case {
    label: &'static str,
    scope: Scope,
    /// The `CodeLocation` body of the `Location` union.
    location: Value,
    /// The `CodeCapture` body of the `CaptureConfiguration` union.
    capture: Value,
    description: Option<&'static str>,
    attribute_filters: Option<Value>,
}

impl Case {
    fn create_body(&self) -> Value {
        let mut body = json!({
            "InstrumentationType": self.scope.instrumentation_type,
            "Service": self.scope.service,
            "Environment": self.scope.environment,
            "SignalType": SNAPSHOT,
            "Location": { "CodeLocation": self.location },
            "CaptureConfiguration": { "CodeCapture": self.capture },
        });
        if let Some(description) = self.description {
            body["Description"] = json!(description);
        }
        if let Some(filters) = self.attribute_filters.as_ref() {
            body["AttributeFilters"] = filters.clone();
        }
        body
    }
}

/// The configurations the rules are asserted over.
///
/// The rows differ along what the rules generalise over: the instrumentation TYPE (which
/// decides whether the configuration expires), the service and the environment (which decide
/// which scope's set a configuration joins, including an environment that carries the `:` and
/// `/` the modelled `^[A-Za-z0-9:/+=,.@_-]+$` pattern admits and the ARN doc calls out), the
/// programming LANGUAGE (the whole modelled `ProgrammingLanguage` catalogue —
/// `smithy_query(query="shape",shape="ProgrammingLanguage")` -> `["Java", "Python",
/// "Javascript"]`), WHICH of `CodeLocation`'s optional members the location supplies
/// (`CodeUnit`, `ClassName`, `MethodName`, `LineNumber`), how much of `CodeCaptureConfiguration`
/// the capture settings fill in, and whether the create carries a `Description` or
/// `AttributeFilters` at all — one group of filters or several.
fn cases() -> Vec<Case> {
    let java = Scope {
        instrumentation_type: BREAKPOINT,
        service: "payment-service",
        environment: "eks:cluster-prod/namespace",
    };
    let probe = Scope {
        instrumentation_type: PROBE,
        service: "payment-service",
        environment: "eks:cluster-prod/namespace",
    };
    let other_environment = Scope {
        instrumentation_type: BREAKPOINT,
        service: "payment-service",
        environment: "ec2:production",
    };
    let other_service = Scope {
        instrumentation_type: BREAKPOINT,
        service: "checkout-service",
        environment: "eks:cluster-prod/namespace",
    };

    vec![
        Case {
            label: "java breakpoint, every location member supplied",
            scope: java.clone(),
            location: json!({
                "Language": "Java",
                "CodeUnit": "com.amazon.payment",
                "ClassName": "PaymentProcessor",
                "MethodName": "validateCreditCard",
                "FilePath": "src/payment/PaymentProcessor.java",
                "LineNumber": 42,
            }),
            capture: json!({
                "CaptureArguments": ["card", "amount"],
                "CaptureReturn": true,
                "CaptureStackTrace": false,
                "CaptureLocals": ["total"],
                "CaptureLimits": {
                    "MaxHits": 1000,
                    "MaxStringLength": 255,
                    "MaxCollectionWidth": 20,
                    "MaxCollectionDepth": 5,
                    "MaxStackFrames": 20,
                    "MaxStackTraceSize": 1000,
                    "MaxObjectDepth": 5,
                    "MaxFieldsPerObject": 20,
                },
            }),
            description: Some("validate card entry"),
            attribute_filters: Some(json!([{ "service.version": "1.2.3" }])),
        },
        Case {
            label: "java breakpoint, same class and method but another line",
            scope: java.clone(),
            location: json!({
                "Language": "Java",
                "CodeUnit": "com.amazon.payment",
                "ClassName": "PaymentProcessor",
                "MethodName": "validateCreditCard",
                "FilePath": "src/payment/PaymentProcessor.java",
                "LineNumber": 43,
            }),
            capture: json!({ "CaptureLimits": {} }),
            description: None,
            attribute_filters: None,
        },
        Case {
            label: "python breakpoint, module-level function with no class",
            scope: java.clone(),
            location: json!({
                "Language": "Python",
                "CodeUnit": "payment_service",
                "MethodName": "__init__",
                "FilePath": "src/payment/processor.py",
            }),
            capture: json!({
                "CaptureArguments": ["*"],
                "CaptureLimits": { "MaxHits": 1 },
            }),
            description: Some("module level entry"),
            attribute_filters: Some(json!([
                { "deployment.environment": "prod", "host.name": "ip-10-0-0-1" },
                { "deployment.environment": "canary" },
            ])),
        },
        Case {
            label: "javascript breakpoint, file path only",
            scope: java.clone(),
            location: json!({
                "Language": "Javascript",
                "FilePath": "src/payment/processor.js",
            }),
            capture: json!({ "CaptureStackTrace": true, "CaptureLimits": {} }),
            description: None,
            attribute_filters: None,
        },
        Case {
            label: "probe in the same service and environment",
            scope: probe,
            location: json!({
                "Language": "Java",
                "CodeUnit": "com.amazon.payment",
                "ClassName": "PaymentProcessor",
                "MethodName": "charge",
                "FilePath": "src/payment/PaymentProcessor.java",
            }),
            capture: json!({ "CaptureReturn": true, "CaptureLimits": {} }),
            description: Some("permanent probe"),
            attribute_filters: None,
        },
        Case {
            label: "breakpoint in another environment of the same service",
            scope: other_environment,
            location: json!({
                "Language": "Java",
                "CodeUnit": "com.amazon.payment",
                "ClassName": "PaymentProcessor",
                "MethodName": "validateCreditCard",
                "FilePath": "src/payment/PaymentProcessor.java",
                "LineNumber": 42,
            }),
            capture: json!({ "CaptureLimits": { "MaxStackFrames": 2 } }),
            description: None,
            attribute_filters: None,
        },
        Case {
            label: "breakpoint in another service",
            scope: other_service,
            location: json!({
                "Language": "Javascript",
                "CodeUnit": "checkout",
                "MethodName": "submit",
                "FilePath": "src/checkout/submit.js",
                "LineNumber": 7,
            }),
            capture: json!({ "CaptureLimits": {} }),
            description: None,
            attribute_filters: None,
        },
    ]
}

/// Whether `value` is a `LocationHash` as the model requires it: 16 characters of LOWERCASE
/// hex (`length 16..16`, and the ARN pattern's `[0-9a-f]{16}` suffix).
fn is_location_hash(value: &str) -> bool {
    value.len() == 16
        && value
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// The ARN the model documents for a configuration:
/// `arn:${Partition}:application-signals:${Region}:${Account}:instrumentationConfig/${service}/${environment}/${signalType}/${locationHash}`.
fn expected_arn(scope: &Scope, location_hash: &str) -> String {
    format!(
        "arn:aws:application-signals:{REGION}:{ACCOUNT}:instrumentationConfig/{}/{}/{SNAPSHOT}/{location_hash}",
        scope.service, scope.environment
    )
}

/// Create every row of [`cases`], asserting the stored record each one answers with, and
/// hand back the `LocationHash` of each.
async fn create_all(router: &MultiServiceRouter, cases: &[Case]) -> Vec<String> {
    let mut hashes = Vec::new();
    for case in cases {
        let body = case.create_body();
        let (status, answer) = create(router, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the create to answer 200, got {status} with {answer:?}",
            case.label
        );

        let location_hash = answer["LocationHash"].as_str().unwrap_or_default();
        assert!(
            is_location_hash(location_hash),
            "case {:?}: expected a 16-character lowercase hex LocationHash, got {:?}",
            case.label,
            answer["LocationHash"]
        );
        assert_eq!(
            answer["ARN"].as_str(),
            Some(expected_arn(&case.scope, location_hash).as_str()),
            "case {:?}: expected the documented ARN format, got {:?}",
            case.label,
            answer["ARN"]
        );
        assert_eq!(
            answer["Location"], body["Location"],
            "case {:?}: expected the stored location, got {:?}",
            case.label, answer["Location"]
        );
        // The stored capture settings are the ones the request sent, with every member the
        // model documents a default for settled to that default — the rule
        // `instrumentation_capture_defaults` owns, stated here by the same expectation.
        assert_eq!(
            answer["CaptureConfiguration"],
            json!({ "CodeCapture": settled(&case.capture) }),
            "case {:?}: expected the stored capture configuration, got {:?}",
            case.label,
            answer["CaptureConfiguration"]
        );
        assert_eq!(
            answer["Description"], body["Description"],
            "case {:?}: expected the stored description, got {:?}",
            case.label, answer["Description"]
        );
        assert_eq!(
            answer["AttributeFilters"], body["AttributeFilters"],
            "case {:?}: expected the stored attribute filters, got {:?}",
            case.label, answer["AttributeFilters"]
        );

        let created_at = answer["CreatedAt"].as_f64();
        assert!(
            created_at.is_some(),
            "case {:?}: expected a server-generated CreatedAt, got {:?}",
            case.label,
            answer["CreatedAt"]
        );

        // Only a BREAKPOINT configuration expires, and one created without `ExpiresAt`
        // expires 24 hours later; a PROBE "persists until explicitly deleted".
        let expires_at = answer["ExpiresAt"].as_f64();
        if case.scope.instrumentation_type == BREAKPOINT {
            let lifetime = expires_at.unwrap_or_default() - created_at.unwrap_or_default();
            assert!(
                (lifetime - MAX_EXPIRY_SECONDS as f64).abs() < 1.0,
                "case {:?}: expected the default 24 hour expiry, got a lifetime of {lifetime} \
                 seconds from {:?} to {:?}",
                case.label,
                answer["CreatedAt"],
                answer["ExpiresAt"]
            );
        } else {
            assert!(
                expires_at.is_none(),
                "case {:?}: expected no expiry for a {} configuration, got {:?}",
                case.label,
                case.scope.instrumentation_type,
                answer["ExpiresAt"]
            );
        }

        hashes.push(location_hash.to_string());
    }
    hashes
}

/// Every configuration a scope reports, following `NextToken` to the end.
async fn list_all(router: &MultiServiceRouter, scope: &Scope, max_results: Option<i64>) -> Vec<Value> {
    let mut items = Vec::new();
    let mut next_token: Option<String> = None;
    for _page in 0..32 {
        let mut body = scope.list_body();
        if let Some(max_results) = max_results {
            body["MaxResults"] = json!(max_results);
        }
        if let Some(token) = next_token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let (status, answer) = list(router, body).await;
        assert_eq!(
            status, 200,
            "scope {scope:?}: expected the list to answer 200, got {status} with {answer:?}"
        );
        let page = answer["LatestConfigurations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if let Some(max_results) = max_results {
            assert!(
                page.len() as i64 <= max_results,
                "scope {scope:?}: expected at most {max_results} configurations in a page, got {}",
                page.len()
            );
        }
        items.extend(page);
        next_token = answer["NextToken"].as_str().map(ToString::to_string);
        if next_token.is_none() {
            return items;
        }
    }
    panic!("scope {scope:?}: the list never ran out of pages");
}

#[tokio::test]
async fn list_reports_the_configurations_stored_for_the_scope() {
    let router = router();
    let cases = cases();
    let hashes = create_all(&router, &cases).await;

    // Every distinct scope the table covers, each reporting exactly its own rows.
    let scopes: Vec<Scope> = {
        let mut seen = BTreeSet::new();
        cases
            .iter()
            .filter(|case| {
                seen.insert((
                    case.scope.instrumentation_type,
                    case.scope.service,
                    case.scope.environment,
                ))
            })
            .map(|case| case.scope.clone())
            .collect()
    };

    for scope in &scopes {
        let expected: BTreeSet<String> = cases
            .iter()
            .zip(hashes.iter())
            .filter(|(case, _)| {
                case.scope.instrumentation_type == scope.instrumentation_type
                    && case.scope.service == scope.service
                    && case.scope.environment == scope.environment
            })
            .map(|(_, hash)| hash.clone())
            .collect();

        let items = list_all(&router, scope, None).await;
        let reported: BTreeSet<String> = items
            .iter()
            .map(|item| item["LocationHash"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, expected,
            "scope {scope:?}: expected exactly the configurations stored for it, got {items:?}"
        );

        for item in &items {
            assert_eq!(
                item["InstrumentationType"].as_str(),
                Some(scope.instrumentation_type),
                "scope {scope:?}: expected the scope's instrumentation type, got {item:?}"
            );
            assert_eq!(
                item["SignalType"].as_str(),
                Some(SNAPSHOT),
                "scope {scope:?}: expected the stored signal type, got {item:?}"
            );
            // The page states the service and the environment once, so an item "omits
            // service and environment because they are provided in the request".
            assert!(
                item.get("Service").is_none() && item.get("Environment").is_none(),
                "scope {scope:?}: expected an item without Service and Environment, got {item:?}"
            );
            assert!(
                item["ARN"].as_str().is_some_and(|arn| arn
                    == expected_arn(scope, item["LocationHash"].as_str().unwrap_or_default())),
                "scope {scope:?}: expected the documented ARN format, got {item:?}"
            );
            assert!(
                item["CreatedAt"].as_f64().is_some(),
                "scope {scope:?}: expected the stored CreatedAt, got {item:?}"
            );
        }
    }

    // The page's own members: the scope it reports over, the instant to sync from next, and
    // the suggested interval ("at least 60 seconds", the member's modelled `range min 60`).
    let scope = &scopes[0];
    let (status, answer) = list(&router, scope.list_body()).await;
    assert_eq!(status, 200, "expected 200, got {status} with {answer:?}");
    assert_eq!(
        (answer["Service"].as_str(), answer["Environment"].as_str()),
        (Some(scope.service), Some(scope.environment)),
        "expected the page to name the scope it reports over, got {answer:?}"
    );
    assert_eq!(
        answer["Changed"].as_bool(),
        Some(true),
        "expected a list with no SyncedAt to report Changed, got {answer:?}"
    );
    assert!(
        answer["SyncedAt"].as_f64().is_some(),
        "expected the server instant to sync from next, got {answer:?}"
    );
    assert!(
        answer["SyncInterval"].as_i64().is_some_and(|seconds| seconds >= 60),
        "expected a SyncInterval of at least 60 seconds, got {answer:?}"
    );
}

#[tokio::test]
async fn an_unknown_scope_reports_no_configurations() {
    let router = router();
    let _ = create_all(&router, &cases()[..1]).await;

    for scope in [
        Scope {
            instrumentation_type: BREAKPOINT,
            service: "no-such-service",
            environment: "eks:cluster-prod/namespace",
        },
        Scope {
            instrumentation_type: BREAKPOINT,
            service: "payment-service",
            environment: "no-such-environment",
        },
        // The same service and environment, the OTHER backing store.
        Scope {
            instrumentation_type: PROBE,
            service: "payment-service",
            environment: "eks:cluster-prod/namespace",
        },
    ] {
        let (status, answer) = list(&router, scope.list_body()).await;
        assert_eq!(
            status, 200,
            "scope {scope:?}: expected 200, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["LatestConfigurations"].as_array().map(Vec::len),
            Some(0),
            "scope {scope:?}: expected no configurations, got {answer:?}"
        );
        assert!(
            answer["NextToken"].is_null(),
            "scope {scope:?}: expected no NextToken, got {answer:?}"
        );
    }
}

#[tokio::test]
async fn a_page_boundary_visits_each_configuration_exactly_once() {
    let router = router();
    let cases = cases();
    let hashes = create_all(&router, &cases).await;
    let scope = cases[0].scope.clone();
    let stored: BTreeSet<String> = cases
        .iter()
        .zip(hashes.iter())
        .filter(|(case, _)| {
            case.scope.instrumentation_type == scope.instrumentation_type
                && case.scope.service == scope.service
                && case.scope.environment == scope.environment
        })
        .map(|(_, hash)| hash.clone())
        .collect();

    // Every page size from "one at a time" through the stored count to the modelled maximum
    // of `MaxResults` (`range 1..100`), plus the absent member that takes its `@default`.
    for max_results in [Some(1), Some(2), Some(stored.len() as i64), Some(100), None] {
        let items = list_all(&router, &scope, max_results).await;
        let reported: Vec<String> = items
            .iter()
            .map(|item| item["LocationHash"].as_str().unwrap_or_default().to_string())
            .collect();
        let distinct: BTreeSet<String> = reported.iter().cloned().collect();
        assert_eq!(
            reported.len(),
            distinct.len(),
            "case {max_results:?}: expected every configuration once, got {reported:?}"
        );
        assert_eq!(
            distinct, stored,
            "case {max_results:?}: expected exactly the stored configurations, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_is_only_honoured_for_the_scope_that_issued_it() {
    let router = router();
    let cases = cases();
    let _ = create_all(&router, &cases).await;
    let scope = cases[0].scope.clone();

    let mut body = scope.list_body();
    body["MaxResults"] = json!(1);
    let (status, answer) = list(&router, body).await;
    assert_eq!(status, 200, "expected 200, got {status} with {answer:?}");
    let token = answer["NextToken"]
        .as_str()
        .expect("a scope holding more than one configuration pages")
        .to_string();

    // A token this service issued for ANOTHER scope, and one it never issued at all, are
    // both refused with 400 rather than honoured as a cursor into this scope.
    let other_scope = Scope {
        instrumentation_type: BREAKPOINT,
        service: "checkout-service",
        environment: "eks:cluster-prod/namespace",
    };
    for (label, scope, forged) in [
        ("a token from another scope", other_scope, token.clone()),
        (
            "a token this service never issued",
            scope.clone(),
            ::hex::encode("instrumentation-configurations/1/forged"),
        ),
        ("a token that is not hex", scope.clone(), "not-a-token".to_string()),
    ] {
        let mut body = scope.list_body();
        body["NextToken"] = json!(forged);
        let (status, answer) = list(&router, body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["__type"].as_str().map(|kind| kind
                .rsplit('#')
                .next()
                .unwrap_or(kind)
                .to_string()),
            Some("ValidationException".to_string()),
            "case {label:?}: expected a ValidationException, got {answer:?}"
        );
    }

    // The token the scope DID issue advances that scope's page.
    let mut body = scope.list_body();
    body["MaxResults"] = json!(1);
    body["NextToken"] = json!(token);
    let (status, answer) = list(&router, body).await;
    assert_eq!(status, 200, "expected 200, got {status} with {answer:?}");
    assert_eq!(
        answer["LatestConfigurations"].as_array().map(Vec::len),
        Some(1),
        "expected the next page of one configuration, got {answer:?}"
    );
}

#[tokio::test]
async fn synced_at_makes_the_list_an_incremental_sync() {
    let router = router();
    let cases = cases();
    let first = &cases[0..1];
    let _ = create_all(&router, first).await;
    let scope = first[0].scope.clone();

    let (status, answer) = list(&router, scope.list_body()).await;
    assert_eq!(status, 200, "expected 200, got {status} with {answer:?}");
    let synced_at = answer["SyncedAt"]
        .as_f64()
        .expect("the page carries the instant to sync from next");
    assert_eq!(
        answer["LatestConfigurations"].as_array().map(Vec::len),
        Some(1),
        "expected the stored configuration, got {answer:?}"
    );

    // Nothing has changed since that instant: `Changed` is false and the configuration
    // details are omitted entirely.
    let mut body = scope.list_body();
    body["SyncedAt"] = json!(synced_at);
    let (status, answer) = list(&router, body).await;
    assert_eq!(status, 200, "expected 200, got {status} with {answer:?}");
    assert_eq!(
        answer["Changed"].as_bool(),
        Some(false),
        "expected Changed to be false when nothing changed since SyncedAt, got {answer:?}"
    );
    assert!(
        answer["LatestConfigurations"].is_null() && answer["NextToken"].is_null(),
        "expected an unchanged sync to omit the configuration details, got {answer:?}"
    );
    assert!(
        answer["SyncedAt"].as_f64().is_some_and(|next| next >= synced_at),
        "expected the next instant to sync from, got {answer:?}"
    );

    // A create in the SAME scope is news to that caller; one in another scope is not.
    let other_scope_case = cases
        .iter()
        .find(|case| case.scope.service != scope.service)
        .expect("the table covers more than one service")
        .clone();
    let (status, answer) = create(&router, other_scope_case.create_body()).await;
    assert_eq!(
        status, 200,
        "expected the create in another scope to answer 200, got {status} with {answer:?}"
    );

    let mut body = scope.list_body();
    body["SyncedAt"] = json!(synced_at);
    let (status, answer) = list(&router, body.clone()).await;
    assert_eq!(status, 200, "expected 200, got {status} with {answer:?}");
    assert_eq!(
        answer["Changed"].as_bool(),
        Some(false),
        "expected another scope's create not to change this one, got {answer:?}"
    );

    let same_scope_case = cases
        .iter()
        .find(|case| {
            case.scope.instrumentation_type == scope.instrumentation_type
                && case.scope.service == scope.service
                && case.scope.environment == scope.environment
                && case.label != first[0].label
        })
        .expect("the table covers more than one configuration in a scope")
        .clone();
    let (status, answer) = create(&router, same_scope_case.create_body()).await;
    assert_eq!(
        status, 200,
        "expected the create in the same scope to answer 200, got {status} with {answer:?}"
    );

    let (status, answer) = list(&router, body).await;
    assert_eq!(status, 200, "expected 200, got {status} with {answer:?}");
    assert_eq!(
        answer["Changed"].as_bool(),
        Some(true),
        "expected a create in the scope to be reported as a change, got {answer:?}"
    );
    assert_eq!(
        answer["LatestConfigurations"].as_array().map(Vec::len),
        Some(2),
        "expected the full current set of the scope, got {answer:?}"
    );
}

/// What a continuation request carries in `SyncedAt` beside its cursor. The rows differ along
/// the watermark alone, because the rule is that NONE of them suppresses the page the cursor
/// names — including the one the member docs tell a syncing client to send back ("The server
/// timestamp to supply on the next sync call").
#[derive(Clone, Copy, Debug)]
enum Watermark {
    /// No watermark at all: a plain paginated walk.
    Absent,
    /// The instant the PREVIOUS page handed back, which is what a client following both member
    /// docs sends.
    PreviousPage,
    /// The instant the FIRST page handed back, kept for the whole walk.
    FirstPage,
    /// An instant no stored change can be after, so the sync gate alone would report nothing.
    AnHourAhead,
    /// An instant every stored configuration is newer than.
    AnHourBehind,
}

impl Watermark {
    /// The value to send, given the instants the first and the previous page handed back.
    fn value(self, first_page: f64, previous_page: f64) -> Option<f64> {
        match self {
            Watermark::Absent => None,
            Watermark::PreviousPage => Some(previous_page),
            Watermark::FirstPage => Some(first_page),
            Watermark::AnHourAhead => Some(first_page + 3_600.0),
            Watermark::AnHourBehind => Some(first_page - 3_600.0),
        }
    }
}

/// The `LocationHash` of every configuration a page reports, in the order it reports them.
fn reported_hashes(answer: &Value) -> Vec<String> {
    answer["LatestConfigurations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["LocationHash"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Walk `scope` a page at a time the way the two member docs tell a client to: the first call
/// starts the listing, and every call after it carries the `NextToken` of the page before it
/// together with `watermark`.
async fn walk_following_the_cursor_and_the_watermark(
    router: &MultiServiceRouter,
    scope: &Scope,
    max_results: i64,
    watermark: Watermark,
) -> Vec<String> {
    let label = format!("{max_results} per page carrying {watermark:?}");

    let mut body = scope.list_body();
    body["MaxResults"] = json!(max_results);
    let (status, first) = list(router, body).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected the first page to answer 200, got {status} with {first:?}"
    );
    let first_synced = first["SyncedAt"].as_f64().unwrap_or_else(|| {
        panic!("case {label:?}: expected the instant to sync from next, got {first:?}")
    });
    let mut previous_synced = first_synced;
    let mut walked = reported_hashes(&first);
    let mut token = first["NextToken"].as_str().map(ToString::to_string);

    for page in 1..=32 {
        let Some(cursor) = token else { return walked };
        let mut body = scope.list_body();
        body["MaxResults"] = json!(max_results);
        body["NextToken"] = json!(cursor);
        if let Some(synced_at) = watermark.value(first_synced, previous_synced) {
            body["SyncedAt"] = json!(synced_at);
        }
        let (status, answer) = list(router, body).await;
        assert_eq!(
            status, 200,
            "case {label:?} page {page}: expected 200, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["Changed"].as_bool(),
            Some(true),
            "case {label:?} page {page}: expected a continuation to report the page its cursor \
             names, got {answer:?}"
        );
        let reported = reported_hashes(&answer);
        assert!(
            !reported.is_empty() && reported.len() as i64 <= max_results,
            "case {label:?} page {page}: expected between one and {max_results} configurations, \
             got {answer:?}"
        );
        walked.extend(reported);
        previous_synced = answer["SyncedAt"].as_f64().unwrap_or_else(|| {
            panic!("case {label:?} page {page}: expected the next instant to sync from, got {answer:?}")
        });
        token = answer["NextToken"].as_str().map(ToString::to_string);
    }
    panic!("case {label:?}: the walk never ran out of pages");
}

#[tokio::test]
async fn a_continuation_serves_its_page_whatever_watermark_it_carries() {
    let router = router();
    let cases = cases();
    let hashes = create_all(&router, &cases).await;
    let scope = cases[0].scope.clone();
    let stored: BTreeSet<String> = cases
        .iter()
        .zip(hashes.iter())
        .filter(|(case, _)| {
            case.scope.instrumentation_type == scope.instrumentation_type
                && case.scope.service == scope.service
                && case.scope.environment == scope.environment
        })
        .map(|(_, hash)| hash.clone())
        .collect();
    assert!(
        stored.len() > 2,
        "expected a scope a cursor can divide more than once, got {stored:?}"
    );

    // Every page bound that divides the stored set, over every watermark a continuation can
    // carry: the cursor decides the page, so each walk reports the same set exactly once.
    for watermark in [
        Watermark::Absent,
        Watermark::PreviousPage,
        Watermark::FirstPage,
        Watermark::AnHourAhead,
        Watermark::AnHourBehind,
    ] {
        for max_results in [1, 2, stored.len() as i64 - 1] {
            let walked =
                walk_following_the_cursor_and_the_watermark(&router, &scope, max_results, watermark)
                    .await;
            let distinct: BTreeSet<String> = walked.iter().cloned().collect();
            assert_eq!(
                walked.len(),
                distinct.len(),
                "case {max_results} per page carrying {watermark:?}: expected every \
                 configuration once, got {walked:?}"
            );
            assert_eq!(
                distinct, stored,
                "case {max_results} per page carrying {watermark:?}: expected exactly the \
                 stored configurations, got {walked:?}"
            );
        }
    }

    // The sync gate still governs a listing that has NOT started: with no cursor, a watermark
    // the scope holds no change after suppresses the details, and one it does answers them.
    let (status, probe) = list(&router, scope.list_body()).await;
    assert_eq!(status, 200, "expected 200, got {status} with {probe:?}");
    let synced_at = probe["SyncedAt"]
        .as_f64()
        .expect("the page carries the instant to sync from next");
    for (watermark, expected_changed) in [
        (Watermark::FirstPage, false),
        (Watermark::AnHourAhead, false),
        (Watermark::AnHourBehind, true),
    ] {
        let mut body = scope.list_body();
        body["MaxResults"] = json!(1);
        body["SyncedAt"] = json!(watermark
            .value(synced_at, synced_at)
            .expect("the rows above all carry a watermark"));
        let (status, answer) = list(&router, body).await;
        assert_eq!(
            status, 200,
            "case {watermark:?} with no cursor: expected 200, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["Changed"].as_bool(),
            Some(expected_changed),
            "case {watermark:?} with no cursor: expected Changed to be {expected_changed}, got \
             {answer:?}"
        );
        assert_eq!(
            answer["LatestConfigurations"].is_array(),
            expected_changed,
            "case {watermark:?} with no cursor: expected the configuration details only when \
             something changed, got {answer:?}"
        );
    }
}

#[tokio::test]
async fn the_location_hash_is_a_function_of_the_location_alone() {
    let router = router();
    let cases = cases();
    let hashes = create_all(&router, &cases).await;

    // Distinct locations hash distinctly: two of the rows differ only in their line number.
    let distinct_hashes: BTreeSet<&String> = hashes.iter().collect();
    let distinct_locations: BTreeSet<String> =
        cases.iter().map(|case| case.location.to_string()).collect();
    assert_eq!(
        distinct_hashes.len(),
        distinct_locations.len(),
        "expected distinct locations to hash distinctly, got {hashes:?}"
    );

    // The SAME location in two different scopes hashes the same: the hash identifies the
    // instrumentation point, and the scope is what the ARN around it carries.
    let shared = cases
        .iter()
        .position(|case| case.scope.environment == "ec2:production")
        .expect("the table covers two environments of one service");
    let twin = cases
        .iter()
        .position(|case| case.location == cases[shared].location && case.scope.environment != "ec2:production")
        .expect("the same location is instrumented in two environments");
    assert_eq!(
        hashes[shared], hashes[twin],
        "expected one location to hash the same in every scope, got {:?} and {:?}",
        hashes[shared], hashes[twin]
    );
}

#[tokio::test]
async fn a_second_create_at_the_same_identity_conflicts() {
    let router = router();
    let case = cases()[0].clone();
    let (status, answer) = create(&router, case.create_body()).await;
    assert_eq!(status, 200, "expected 200, got {status} with {answer:?}");
    let arn = answer["ARN"].as_str().unwrap_or_default().to_string();

    // The documented conflict key is the service, the environment, the signal type and the
    // location — the four parts the ARN spells, and NOT the instrumentation type: so the
    // same create repeated and a create of the other type at that same location are both
    // refused, each with the conflict worded over those four dimensions and over no value of
    // them (`refusal_wording` owns that rule).
    let mut other_type = case.create_body();
    other_type["InstrumentationType"] = json!(PROBE);
    // `ExpiresAt` is not supported for PROBE, and the default-expiring row carries none.
    for (label, body) in [
        ("the same identity again", case.create_body()),
        ("the other instrumentation type", other_type),
    ] {
        let (status, answer) = create(&router, body).await;
        assert_eq!(
            status, 409,
            "case {label:?}: expected the create to conflict with 409, got {status} with {answer:?}"
        );
        assert_eq!(
            answer["Message"].as_str(),
            Some(CONFIGURATION_CONFLICT),
            "case {label:?}: expected the conflict to name the identity dimensions, got {answer:?}"
        );
        assert!(
            !answer["Message"]
                .as_str()
                .is_some_and(|message| message.contains(&arn)),
            "case {label:?}: expected the conflict to name no ARN, got {answer:?}"
        );
    }

    // The identity is the whole tuple, so a create that differs in ANY of its parts is a
    // different configuration rather than a conflict.
    let mut distinct: Vec<(&str, Value)> = Vec::new();
    let mut other_service = case.create_body();
    other_service["Service"] = json!("another-service");
    distinct.push(("another service", other_service));
    let mut other_environment = case.create_body();
    other_environment["Environment"] = json!("ec2:staging");
    distinct.push(("another environment", other_environment));
    let mut other_location = case.create_body();
    other_location["Location"]["CodeLocation"]["LineNumber"] = json!(99);
    distinct.push(("another location", other_location));

    for (label, body) in distinct {
        let (status, answer) = create(&router, body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected a distinct identity to be accepted, got {status} with {answer:?}"
        );
    }
}

#[tokio::test]
async fn expires_at_follows_the_documented_window() {
    let router = router();
    let base = cases()[0].clone();

    // The window `ExpiresAt`'s doc publishes for BREAKPOINT ("must be between 5 min and 24
    // hours"): each published bound and one second past it, plus the absent member. The
    // window is measured from the instant the create is SERVED, which is AFTER this test
    // reads the clock — by a fraction of a second normally, and by whole seconds when the
    // machine running the suite stalls between the two — so a row that must be SERVED is
    // probed [`SERVED_MARGIN_SECONDS`] inside the floor rather than one second inside it,
    // which is the same probe without the race. A row that must be REFUSED needs no such
    // margin: drift moves the floor further above it, so it stays refused. PROBE supports no
    // expiry at all.
    let rows: Vec<(&str, &str, Option<i64>, u16)> = vec![
        ("breakpoint, no expiry", BREAKPOINT, None, 200),
        (
            "breakpoint, just inside the 5 minute floor",
            BREAKPOINT,
            Some(MIN_EXPIRY_SECONDS + SERVED_MARGIN_SECONDS),
            200,
        ),
        (
            "breakpoint, one second under the floor",
            BREAKPOINT,
            Some(MIN_EXPIRY_SECONDS - 1),
            400,
        ),
        (
            "breakpoint, just inside the 24 hour ceiling",
            BREAKPOINT,
            Some(MAX_EXPIRY_SECONDS - 1),
            200,
        ),
        (
            "breakpoint, one second over the ceiling",
            BREAKPOINT,
            Some(MAX_EXPIRY_SECONDS + 1),
            400,
        ),
        ("breakpoint, an hour", BREAKPOINT, Some(60 * 60), 200),
        ("breakpoint, already past", BREAKPOINT, Some(-1), 400),
        ("probe, no expiry", PROBE, None, 200),
        ("probe, an hour", PROBE, Some(60 * 60), 400),
        ("probe, the 24 hour ceiling", PROBE, Some(MAX_EXPIRY_SECONDS), 400),
    ];

    for (index, (label, instrumentation_type, offset, expected_status)) in
        rows.into_iter().enumerate()
    {
        let mut body = base.create_body();
        body["InstrumentationType"] = json!(instrumentation_type);
        // A fresh location per row, so a row is never refused as a conflict with an earlier
        // one.
        body["Location"]["CodeLocation"]["LineNumber"] = json!(1000 + index as i64);
        let now = ::std::time::SystemTime::now()
            .duration_since(::std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs() as i64;
        if let Some(offset) = offset {
            body["ExpiresAt"] = json!(now + offset);
        }

        let (status, answer) = create(&router, body).await;
        assert_eq!(
            status, expected_status,
            "case {label:?}: expected {expected_status}, got {status} with {answer:?}"
        );
        if expected_status != 200 {
            continue;
        }
        match (instrumentation_type, offset) {
            (BREAKPOINT, Some(offset)) => {
                let expires_at = answer["ExpiresAt"].as_f64().unwrap_or_default();
                assert!(
                    (expires_at - (now + offset) as f64).abs() < 1.0,
                    "case {label:?}: expected the requested expiry to be stored, got {answer:?}"
                );
            }
            (BREAKPOINT, None) => {
                let lifetime = answer["ExpiresAt"].as_f64().unwrap_or_default()
                    - answer["CreatedAt"].as_f64().unwrap_or_default();
                assert!(
                    (lifetime - MAX_EXPIRY_SECONDS as f64).abs() < 1.0,
                    "case {label:?}: expected the default 24 hour expiry, got {answer:?}"
                );
            }
            _ => assert!(
                answer["ExpiresAt"].is_null(),
                "case {label:?}: expected no expiry for a {instrumentation_type} configuration, \
                 got {answer:?}"
            ),
        }
    }
}

#[tokio::test]
async fn the_store_is_bounded_by_the_quota_including_the_scopes_a_caller_mints() {
    let router = router();
    let base = cases()[0].clone();

    // Each create lands in a FRESH scope, so the quota is asserted over the scope keys the
    // caller chooses and not only over the records inside one scope.
    for index in 0..MAX_INSTRUMENTATION_CONFIGURATIONS {
        let mut body = base.create_body();
        body["Service"] = json!(format!("service-{index}"));
        let (status, answer) = create(&router, body).await;
        assert_eq!(
            status, 200,
            "case {index}: expected a create inside the quota to answer 200, got {status} with \
             {answer:?}"
        );
    }

    let mut body = base.create_body();
    body["Service"] = json!("one-too-many");
    let (status, answer) = create(&router, body).await;
    assert_eq!(
        status, 402,
        "expected the create past the quota of {MAX_INSTRUMENTATION_CONFIGURATIONS} to answer \
         402, got {status} with {answer:?}"
    );
    assert_eq!(
        answer["__type"]
            .as_str()
            .map(|kind| kind.rsplit('#').next().unwrap_or(kind).to_string()),
        Some("ServiceQuotaExceededException".to_string()),
        "expected a ServiceQuotaExceededException, got {answer:?}"
    );
}

/// One stored configuration, for the store-level assertions below.
fn stored_configuration(
    scope: &InstrumentationScope,
    location_hash: &str,
    created_at: DateTime,
    expires_at: Option<DateTime>,
) -> InstrumentationConfiguration {
    InstrumentationConfiguration {
        instrumentation_type: scope.instrumentation_type.clone(),
        service: scope.service.clone(),
        environment: scope.environment.clone(),
        signal_type: SNAPSHOT.to_string(),
        location: Location::CodeLocation(CodeLocation {
            language: "Java".to_string(),
            // `CodeLocation` documents `ClassName` as required for Java, so a record staged
            // directly on the store names one too — a store-level fixture should hold what a
            // create could have written.
            class_name: Some("PaymentProcessor".to_string()),
            file_path: "src/payment/PaymentProcessor.java".to_string(),
            ..CodeLocation::default()
        }),
        location_hash: location_hash.to_string(),
        description: None,
        expires_at,
        attribute_filters: None,
        capture_configuration: CaptureConfiguration::CodeCapture(CodeCaptureConfiguration {
            capture_limits: CaptureLimitsConfig::default(),
            ..CodeCaptureConfiguration::default()
        }),
        created_at,
        arn: format!(
            "arn:aws:application-signals:{REGION}:{ACCOUNT}:instrumentationConfig/{}/{}/{SNAPSHOT}/{location_hash}",
            scope.service, scope.environment
        ),
    }
}

#[test]
fn an_expired_configuration_lapses_and_its_emptied_scope_is_dropped() {
    // The documented floor on a BREAKPOINT lifetime is 5 minutes, so no HTTP request can
    // create a configuration that expires during a test. The lapse is therefore asserted on
    // the store the handlers share, through the very methods they call.
    let mut store = ComAmazonawsApplicationsignalsApplicationsignalsState::default();
    let created_at = DateTime::from_secs_f64(1_700_000_000.0);
    let expires_at = DateTime::from_secs_f64(created_at.as_secs_f64() + MIN_EXPIRY_SECONDS as f64);

    let breakpoint = InstrumentationScope::new(BREAKPOINT, "payment-service", "ec2:production");
    let probe = InstrumentationScope::new(PROBE, "payment-service", "ec2:production");
    store.put_instrumentation_configuration(
        breakpoint.clone(),
        stored_configuration(&breakpoint, "0123456789abcdef", created_at, Some(expires_at)),
        created_at,
    );
    store.put_instrumentation_configuration(
        probe.clone(),
        stored_configuration(&probe, "fedcba9876543210", created_at, None),
        created_at,
    );

    // A second before it falls due the configuration is still reported.
    let before = DateTime::from_secs_f64(expires_at.as_secs_f64() - 1.0);
    store.expire_instrumentation_configurations(&before);
    let (page, next_token) = store
        .page_instrumentation_configurations(&breakpoint, None, 10)
        .expect("a cursorless read resolves no boundary");
    assert_eq!(
        (page.len(), next_token.is_none()),
        (1, true),
        "case \"before the expiry\": expected the configuration to still be reported, got \
         {page:?}"
    );

    // At the instant it falls due it is gone, and the lapse is news to a caller that synced
    // before it but not to one that synced after it.
    store.expire_instrumentation_configurations(&expires_at);
    let (page, _) = store
        .page_instrumentation_configurations(&breakpoint, None, 10)
        .expect("a cursorless read resolves no boundary");
    assert!(
        page.is_empty(),
        "case \"at the expiry\": expected the lapsed configuration to be gone, got {page:?}"
    );
    assert!(
        store.instrumentation_changed_since(&breakpoint, &created_at),
        "case \"at the expiry\": expected the lapse to be reported as a change to a caller \
         that synced before it"
    );
    assert!(
        !store.instrumentation_changed_since(&breakpoint, &expires_at),
        "case \"at the expiry\": expected no change for a caller that synced at the lapse"
    );

    // The PROBE configuration does not expire, so only what the clock owed was dropped.
    let (page, _) = store
        .page_instrumentation_configurations(&probe, None, 10)
        .expect("a cursorless read resolves no boundary");
    assert_eq!(
        page.len(),
        1,
        "case \"at the expiry\": expected a PROBE configuration to persist, got {page:?}"
    );

    // Past the retention horizon the emptied scope's own record goes too, so the scope keys
    // a caller mints cannot accumulate; the scope that still holds a record stays.
    let past_horizon = DateTime::from_secs_f64(
        expires_at.as_secs_f64() + EMPTY_SCOPE_RETENTION.as_secs_f64() + 1.0,
    );
    store.expire_instrumentation_configurations(&past_horizon);
    assert_eq!(
        store.instrumentation_entry_count(),
        1,
        "case \"past the retention horizon\": expected only the PROBE scope to remain, got {:?}",
        store.instrumentation_configurations.keys().collect::<Vec<_>>()
    );
    assert!(
        !store
            .instrumentation_configurations
            .contains_key(&breakpoint),
        "case \"past the retention horizon\": expected the emptied scope to be dropped, got {:?}",
        store.instrumentation_configurations.keys().collect::<Vec<_>>()
    );
}
