// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the ARN `CreateInstrumentationConfiguration` mints, over HTTP.
//!
//! Every test drives the real `restJson1` surface through the router that production mounts,
//! so the routes, the status codes and the JSON bodies are the ones a client sees.
//!
//! The rule under test: the ARN a create mints is built from the REQUEST — the region of the
//! caller's SigV4 credential scope, or the emulator's default region when the request carries
//! no scope to read one from — and from the identity the request names, in the format the
//! modelled `InstrumentationConfigurationArn` documents; it is then STORED, so every later
//! read of that configuration reports the very same ARN rather than one re-minted for the
//! reading request.
//!
//! The rows below differ along what that rule generalises over: the region the caller signs
//! in (including the two fallback forms — a credential scope whose region field is empty, and
//! a request with no credential at all, which reaches this service by its endpoint host), the
//! instrumentation type, and the identity parts the ARN's resource path is built from (the
//! service, the environment — plain and with the `:` and `/` the documented
//! `eks:cluster/namespace` form carries — and the location the hash is taken over).
//!
//! `CreateInstrumentationConfiguration`'s other rules (the hash, the expiry window, the
//! conflict, the quota) are asserted in `instrumentation_configurations.rs`, and the get's own
//! rules in `get_instrumentation_configuration.rs`.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_applicationsignals_applicationsignals::state::DEFAULT_REGION;
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account id every minted ARN carries
/// (`engine::emulator_account_id`), which is also what the modelled ARN pattern's
/// `[0-9]{12}` account segment admits.
const ACCOUNT: &str = "000000000000";

/// This service's ARN namespace, from the modelled `InstrumentationConfigurationArn` pattern
/// (`^arn:[^:]+:application-signals:...`).
const NAMESPACE: &str = "application-signals";

/// The partition every ARN this emulator mints is in (`engine::emulator_arn`).
const PARTITION: &str = "aws";

/// The resource-type segment of a configuration's ARN, from the modelled
/// `InstrumentationConfigurationArn` doc:
/// `arn:${Partition}:application-signals:${Region}:${Account}:instrumentationConfig/${service}/${environment}/${signalType}/${locationHash}`.
const ARN_RESOURCE: &str = "instrumentationConfig";

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationSignalType")` -> `["SNAPSHOT"]`).
const SNAPSHOT: &str = "SNAPSHOT";

/// The two `InstrumentationType` values
/// (`smithy_query(query="shape",shape="InstrumentationType")` -> `["BREAKPOINT", "PROBE"]`).
const BREAKPOINT: &str = "BREAKPOINT";
const PROBE: &str = "PROBE";

/// The endpoint host every request below is addressed to, in a region deliberately DIFFERENT
/// from the one each signed row signs in: the region an ARN carries is read from the caller's
/// credential scope, so the endpoint's own region must never leak into it.
const ENDPOINT_REGION: &str = "us-west-2";

/// A router with just this service mounted on a fresh engine. Signature verification is off,
/// so the signature bytes of the `Authorization` header below are never read — only its
/// credential scope is.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine),
    );
    router
}

/// How a request names its caller.
#[derive(Clone, Copy, Debug)]
enum Caller {
    /// A SigV4 credential scope in this region.
    Signed(&'static str),
    /// A credential scope whose region field is empty, which names no region at all.
    SignedWithoutRegion,
    /// No credential: the request reaches this service by its endpoint host.
    Unsigned,
}

impl Caller {
    /// The `Authorization` header this caller sends, if any.
    fn authorization(self) -> Option<String> {
        let region = match self {
            Caller::Signed(region) => region,
            Caller::SignedWithoutRegion => "",
            Caller::Unsigned => return None,
        };
        Some(format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{NAMESPACE}/aws4_request, \
             SignedHeaders=host, Signature=0000"
        ))
    }

    /// The region an ARN minted for this caller's request carries: the credential scope's,
    /// else the emulator's default.
    fn arn_region(self) -> &'static str {
        match self {
            Caller::Signed(region) => region,
            Caller::SignedWithoutRegion | Caller::Unsigned => DEFAULT_REGION,
        }
    }
}

/// Send one request as `caller` and return its status and parsed JSON body (`Value::Null`
/// when empty).
async fn call(
    router: &MultiServiceRouter,
    caller: Caller,
    uri: &str,
    body: &Value,
) -> (u16, Value) {
    let payload = serde_json::to_vec(body).expect("serialize the request body");
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://{NAMESPACE}.{ENDPOINT_REGION}.amazonaws.com{uri}"
        ))
        .header("content-type", "application/json");
    if let Some(authorization) = caller.authorization() {
        request = request.header("authorization", authorization);
    }
    let request = request
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
async fn create(router: &MultiServiceRouter, caller: Caller, body: &Value) -> (u16, Value) {
    call(router, caller, "/create-instrumentation-configuration", body).await
}

/// `POST /get-instrumentation-configuration`.
async fn get(router: &MultiServiceRouter, caller: Caller, body: &Value) -> (u16, Value) {
    call(router, caller, "/get-instrumentation-configuration", body).await
}

/// A `CaptureConfiguration` with only its `@required` members, whose own members are all
/// optional (`smithy_query(query="shape",shape="CodeCaptureConfiguration,CaptureLimitsConfig")`).
fn minimal_capture() -> Value {
    json!({ "CodeCapture": { "CaptureLimits": {} } })
}

/// One row: a caller and the identity it creates a configuration under.
struct Case {
    label: &'static str,
    caller: Caller,
    service: &'static str,
    environment: &'static str,
    instrumentation_type: &'static str,
    location: Value,
}

impl Case {
    /// The create request this row sends.
    fn create_body(&self) -> Value {
        json!({
            "Service": self.service,
            "Environment": self.environment,
            "SignalType": SNAPSHOT,
            "InstrumentationType": self.instrumentation_type,
            "Location": { "CodeLocation": self.location },
            "CaptureConfiguration": minimal_capture(),
        })
    }

    /// A get request naming the configuration this row created, by its location hash.
    fn get_body(&self, location_hash: &str) -> Value {
        json!({
            "Service": self.service,
            "Environment": self.environment,
            "SignalType": SNAPSHOT,
            "InstrumentationType": self.instrumentation_type,
            "LocationIdentifier": { "LocationHash": location_hash },
        })
    }

    /// The ARN the documented format puts this row's configuration at.
    fn expected_arn(&self, location_hash: &str) -> String {
        format!(
            "arn:{PARTITION}:{NAMESPACE}:{}:{ACCOUNT}:{ARN_RESOURCE}/{}/{}/{SNAPSHOT}/{location_hash}",
            self.caller.arn_region(),
            self.service,
            self.environment,
        )
    }
}

/// The rows the rule is asserted over: four regions a caller can sign in (none of them the
/// endpoint's own, and none of them the default a fallback row lands on), both fallbacks,
/// both instrumentation types, and a different identity in every row so the rows share one
/// store without conflicting.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "ca-central-1 scope, BREAKPOINT",
            caller: Caller::Signed("ca-central-1"),
            service: "checkout",
            environment: "ec2:production",
            instrumentation_type: BREAKPOINT,
            location: json!({
                "Language": "Java",
                "FilePath": "src/main/java/com/example/CartService.java",
                "LineNumber": 142
            }),
        },
        Case {
            label: "eu-west-2 scope, PROBE, an environment carrying ':' and '/'",
            caller: Caller::Signed("eu-west-2"),
            service: "fulfilment",
            environment: "eks:cluster-prod/namespace",
            instrumentation_type: PROBE,
            location: json!({
                "Language": "Python",
                "FilePath": "app/handlers/orders.py"
            }),
        },
        Case {
            label: "ap-southeast-2 scope, BREAKPOINT",
            caller: Caller::Signed("ap-southeast-2"),
            service: "pricing",
            environment: "eks:cluster-staging/pricing",
            instrumentation_type: BREAKPOINT,
            location: json!({
                "Language": "Javascript",
                "FilePath": "src/pricing/quote.js",
                "MethodName": "quote",
                "LineNumber": 1
            }),
        },
        Case {
            label: "sa-east-1 scope, PROBE",
            caller: Caller::Signed("sa-east-1"),
            service: "search",
            environment: "ec2:staging",
            instrumentation_type: PROBE,
            location: json!({
                "Language": "Java",
                "CodeUnit": "search-service.jar",
                "ClassName": "com.example.SearchService",
                "MethodName": "query",
                "FilePath": "src/main/java/com/example/SearchService.java"
            }),
        },
        Case {
            label: "a credential scope whose region is empty",
            caller: Caller::SignedWithoutRegion,
            service: "billing",
            environment: "ec2:production",
            instrumentation_type: BREAKPOINT,
            location: json!({
                "Language": "Python",
                "FilePath": "billing/invoice.py",
                "LineNumber": 7
            }),
        },
        Case {
            label: "no credential at all",
            caller: Caller::Unsigned,
            service: "notifications",
            environment: "eks:cluster-prod/notifications",
            instrumentation_type: PROBE,
            location: json!({
                "Language": "Javascript",
                "FilePath": "src/notify/send.js"
            }),
        },
    ]
}

/// Whether `value` is a `LocationHash`: 16 lower-case hex characters, which is both the
/// modelled `length 16..16` and the ARN pattern's `[0-9a-f]{16}` suffix.
fn is_location_hash(value: &str) -> bool {
    value.len() == 16
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The ARN a create mints carries the region of the caller's credential scope (the emulator's
/// default when the request names none) and the identity the request named, in the documented
/// format — and the stored configuration reports that very ARN when it is read back.
#[tokio::test]
async fn the_created_arn_is_minted_from_the_request_and_then_stored() {
    let router = router();
    let cases = cases();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for case in &cases {
        let label = case.label;
        let (status, created) = create(&router, case.caller, &case.create_body()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 from the create, got {status} with {created}"
        );

        let hash = created["LocationHash"].as_str().unwrap_or_default();
        assert!(
            is_location_hash(hash),
            "case {label:?}: expected 16 lower-case hex characters of LocationHash, got {hash:?}"
        );

        let arn = created["ARN"].as_str().unwrap_or_default();
        assert_eq!(
            arn,
            case.expected_arn(hash),
            "case {label:?}: expected the documented ARN, got {arn:?}"
        );
        assert!(
            minted.insert(arn.to_string()),
            "case {label:?}: expected a distinct identity to mint a distinct ARN, got {arn:?} twice"
        );

        // The ARN is part of the STORED record, so a read reports the one the create minted —
        // even a read whose own credential scope names another region.
        for (who, reader) in [
            ("its own caller", case.caller),
            ("a caller in another region", Caller::Signed(ENDPOINT_REGION)),
        ] {
            let (status, fetched) = get(&router, reader, &case.get_body(hash)).await;
            assert_eq!(
                status, 200,
                "case {label:?} read by {who}: expected HTTP 200 from the get, got {status} with {fetched}"
            );
            assert_eq!(
                fetched["Configuration"]["ARN"].as_str(),
                Some(arn),
                "case {label:?} read by {who}: expected the stored ARN {arn:?}, got {:?}",
                fetched["Configuration"]["ARN"]
            );
            assert_eq!(
                fetched["Configuration"], created,
                "case {label:?} read by {who}: expected the whole stored record {created}, got {:?}",
                fetched["Configuration"]
            );
        }
    }

    assert_eq!(
        minted.len(),
        cases.len(),
        "expected one ARN per row, got {minted:?}"
    );
}

/// The region is the only part of the ARN the caller's credential scope decides: the SAME
/// identity created in three regions is minted at three ARNs that differ in their region
/// segment alone.
#[tokio::test]
async fn only_the_region_segment_follows_the_credential_scope() {
    // One identity, created unchanged in every region below. The caller is the loop's, so the
    // request body carries no region of its own.
    let body = json!({
        "Service": "checkout",
        "Environment": "eks:cluster-prod/namespace",
        "SignalType": SNAPSHOT,
        "InstrumentationType": BREAKPOINT,
        "Location": { "CodeLocation": {
            "Language": "Java",
            "FilePath": "src/main/java/com/example/CartService.java",
            "LineNumber": 142
        }},
        "CaptureConfiguration": minimal_capture(),
    });

    let mut by_region = Vec::new();
    for region in ["us-east-1", "eu-central-1", "ap-northeast-1"] {
        // A fresh store per region: the identity is the same in each, and a second create at
        // one identity is a conflict rather than a second ARN.
        let router = router();
        let (status, created) = create(&router, Caller::Signed(region), &body).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200 from the create, got {status} with {created}"
        );
        let hash = created["LocationHash"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // An ARN's fields are `arn:<partition>:<service>:<region>:<account>:<resource>`, and
        // only the resource may carry a `:`, so splitting into six is exact.
        let fields: Vec<String> = created["ARN"]
            .as_str()
            .unwrap_or_default()
            .splitn(6, ':')
            .map(str::to_string)
            .collect();
        assert_eq!(
            fields.len(),
            6,
            "case {region:?}: expected six ARN fields, got {:?}",
            created["ARN"]
        );
        assert_eq!(
            fields[3], region,
            "case {region:?}: expected the ARN's region field to be the signing region, got {:?}",
            fields[3]
        );
        by_region.push((region, hash, fields));
    }

    // The identity hashes the same everywhere, so every other ARN field is the same too.
    let (first_region, first_hash, first_fields) = by_region[0].clone();
    for (region, hash, fields) in by_region.iter().skip(1) {
        assert_eq!(
            *hash, first_hash,
            "case {region:?}: expected one location to hash the same in every region, got {hash:?} and {first_hash:?}"
        );
        for (index, (field, first_field)) in fields.iter().zip(&first_fields).enumerate() {
            if index == 3 {
                assert_ne!(
                    field, first_field,
                    "case {region:?}: expected a different region from {first_region:?}, got {field:?}"
                );
            } else {
                assert_eq!(
                    field, first_field,
                    "case {region:?}: expected ARN field {index} to be unchanged, got {field:?} against {first_field:?}"
                );
            }
        }
    }
}
