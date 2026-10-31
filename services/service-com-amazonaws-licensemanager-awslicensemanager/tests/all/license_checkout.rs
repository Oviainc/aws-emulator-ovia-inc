// SPDX-License-Identifier: CC0-1.0

//! `CheckoutLicense`, `CheckInLicense` and `ExtendLicenseConsumption` over the wire,
//! driven as awsJson1_1 requests (`POST /`, `X-Amz-Target:
//! AWSLicenseManager.<Operation>`) through the same `MultiServiceRouter` the
//! production emulator mounts.
//!
//! The rules under test:
//!
//! * a checkout is taken on the ONE license whose `ProductSKU` and issuer
//!   `KeyFingerprint` the request names, and a pair naming no stored license is
//!   refused rather than granted;
//! * a requested entitlement is matched against the ones the license DECLARES by
//!   name and unit together, and the units it takes are the count its `Value` names
//!   — one when it names none, and one grant on the resource for an entitlement
//!   whose unit is `None`;
//! * a grant is allowed only while the units it adds leave that entitlement at or
//!   below the maximum count it caps, unless the entitlement allows overage, and an
//!   entitlement capping no maximum count caps nothing;
//! * an accepted checkout PERSISTS its grant, so `GetLicenseUsage` reports the units
//!   it consumed, the repeat of it under the same `ClientToken` reports that same
//!   grant, and a refused one persists nothing;
//! * `CheckInLicense` releases the grant its token names exactly once: the units
//!   leave the usage, and a token that names no checkout to release — released
//!   already by a check-in or by its expiration, or never issued — is refused;
//! * a grant stops holding its units once its expiration passes, with no write and
//!   without anybody calling, so the usage drops and the entitlement can be granted
//!   again;
//! * `ExtendLicenseConsumption` moves a held grant to the bound a checkout of its
//!   type taken now would carry, persisting it unless the request is a dry run, and
//!   refuses a token that names no held grant.
//!
//! Each table row differs in what those rules generalise over: the entitlement's
//! unit, whether it caps a count and allows overage, how many units are requested,
//! how many entitlements one checkout asks for, and the checkout type.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_licensemanager_awslicensemanager::state::service_state;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix: the service shape's name,
/// `com.amazonaws.licensemanager#AWSLicenseManager`.
const TARGET_PREFIX: &str = "AWSLicenseManager";

/// The region the harness signs for.
const SIGNING_REGION: &str = "us-east-1";

/// The provisional time to live every license this suite creates carries, in
/// minutes, which is what bounds a `PROVISIONAL` checkout of it.
const PROVISIONAL_MINUTES: i64 = 60;

/// The validity end every license this suite creates carries, which is what bounds
/// a `PERPETUAL` checkout of it.
const VALIDITY_END: &str = "2099-12-31T23:59:59Z";

/// The modelled minimum length of a `SignedToken`
/// (`smithy_query(query="shape",shape="SignedToken")`).
const SIGNED_TOKEN_MIN_LEN: usize = 4096;

/// The message AWS words a `NoEntitlementsAllowedException` with.
const NO_ENTITLEMENTS_ALLOWED: &str = "No Entitlements Allowed.";

/// The message AWS words the `ResourceNotFoundException` of a consumption token
/// that names no checkout with.
const INVALID_CONSUMPTION_TOKEN: &str = "Invalid value of consumptionToken.";

/// The most grants this emulator's store holds at once: the bound
/// `insert_consumption` enforces, as `state::MAX_LICENSE_CONSUMPTIONS` sets it.
const MAX_HELD_GRANTS: usize = 500;

/// One emulator: a single shared engine behind the router, exactly as `core/router`
/// assembles it, plus the handle on that engine a case reads a stored grant through
/// when no operation reports the member it is about.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The harness signs nothing, so authentication is off for this router only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_licensemanager_awslicensemanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// The message a refusal body carries, under either casing the protocols use.
fn error_message(body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Send one awsJson1_1 request and return its status and body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTHARNESS/20260101/{SIGNING_REGION}/license-manager/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{operation}: response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code of a failure body: the last segment of awsJson's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|t| t.split('#').next_back().unwrap_or(t).to_string())
        .unwrap_or_else(|| format!("<no __type in {body}>"))
}

/// One entitlement the license declares, with only the members the row sets.
fn entitlement(name: &str, unit: &str, extra: &[(&str, Value)]) -> Value {
    let mut entitlement = Map::new();
    entitlement.insert("Name".to_string(), json!(name));
    entitlement.insert("Unit".to_string(), json!(unit));
    for (member, value) in extra {
        entitlement.insert((*member).to_string(), value.clone());
    }
    Value::Object(entitlement)
}

/// One `EntitlementData` a checkout asks for.
fn requested(name: &str, unit: &str, value: Option<&str>) -> Value {
    let mut data = Map::new();
    data.insert("Name".to_string(), json!(name));
    data.insert("Unit".to_string(), json!(unit));
    if let Some(value) = value {
        data.insert("Value".to_string(), json!(value));
    }
    Value::Object(data)
}

/// A license of the product `sku`, issued by `issuer`, declaring `entitlements`.
fn license_body(sku: &str, issuer: &str, entitlements: Value) -> Value {
    json!({
        "LicenseName": format!("license-{sku}"),
        "ProductName": "Checkout Suite",
        "ProductSKU": sku,
        "Issuer": {"Name": issuer},
        "HomeRegion": "us-east-1",
        "Validity": {"Begin": "2026-01-01T00:00:00Z", "End": VALIDITY_END},
        "Entitlements": entitlements,
        "Beneficiary": "arn:aws:iam::000000000000:root",
        "ConsumptionConfiguration": {
            "ProvisionalConfiguration": {"MaxTimeToLiveInMinutes": PROVISIONAL_MINUTES},
        },
        "ClientToken": format!("create-{sku}"),
    })
}

/// A license of this suite's product `sku` declaring `entitlements`: its ARN and the
/// issuer key fingerprint a checkout of it has to present, read back through
/// `GetLicense` rather than assumed.
async fn create_license(
    router: &MultiServiceRouter,
    case: &str,
    sku: &str,
    entitlements: Value,
) -> (String, String) {
    let body = license_body(sku, &format!("issuer-{sku}"), entitlements);
    let (status, response) = call(router, "CreateLicense", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateLicense to answer 200, got {status} {response}"
    );
    let arn = response["LicenseArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateLicense answered no LicenseArn: {response}"))
        .to_string();

    let (status, read) = call(router, "GetLicense", &json!({"LicenseArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetLicense to answer 200, got {status} {read}"
    );
    let fingerprint = read["License"]["Issuer"]["KeyFingerprint"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: GetLicense reported no KeyFingerprint: {read}"))
        .to_string();
    (arn, fingerprint)
}

/// A checkout request body.
fn checkout_body(
    sku: &str,
    fingerprint: &str,
    checkout_type: &str,
    client_token: &str,
    entitlements: Value,
    node_id: Option<&str>,
) -> Value {
    let mut body = Map::new();
    body.insert("ProductSKU".to_string(), json!(sku));
    body.insert("KeyFingerprint".to_string(), json!(fingerprint));
    body.insert("CheckoutType".to_string(), json!(checkout_type));
    body.insert("ClientToken".to_string(), json!(client_token));
    body.insert("Entitlements".to_string(), entitlements);
    if let Some(node_id) = node_id {
        body.insert("NodeId".to_string(), json!(node_id));
    }
    Value::Object(body)
}

/// The units `GetLicenseUsage` reports as consumed for the entitlement `name` of
/// `arn`.
async fn consumed_units(router: &MultiServiceRouter, case: &str, arn: &str, name: &str) -> i64 {
    let (status, response) = call(router, "GetLicenseUsage", &json!({"LicenseArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetLicenseUsage to answer 200, got {status} {response}"
    );
    let usages = response["LicenseUsage"]["EntitlementUsages"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: GetLicenseUsage answered no EntitlementUsages: {response}")
        })
        .clone();
    let usage = usages
        .iter()
        .find(|usage| usage["Name"].as_str() == Some(name))
        .unwrap_or_else(|| {
            panic!("case {case:?}: no usage reported for the entitlement {name:?}: {usages:?}")
        });
    usage["ConsumedValue"]
        .as_str()
        .and_then(|consumed| consumed.parse::<i64>().ok())
        .unwrap_or_else(|| {
            panic!("case {case:?}: usage {usage:?} carries no numeric ConsumedValue")
        })
}

/// The claims of a compact-JWS signed token, decoded: the token is three base64url
/// segments, and the middle one is the document the checkout signed.
fn signed_token_claims(case: &str, token: &str) -> Value {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "case {case:?}: expected a three-segment compact JWS, got {} segments",
        segments.len()
    );
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(segments[1])
        .unwrap_or_else(|e| panic!("case {case:?}: the claims segment is not base64url ({e})"));
    serde_json::from_slice(&claims)
        .unwrap_or_else(|e| panic!("case {case:?}: the claims segment is not JSON ({e})"))
}

/// An ISO8601 instant as seconds since the Unix epoch.
fn epoch_secs(case: &str, member: &str, instant: &str) -> f64 {
    DateTime::from_str(instant, Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {member} {instant:?} is not ISO8601 ({e})"))
        .as_secs_f64()
}

/// Now, in whole seconds since the Unix epoch.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// `secs` since the Unix epoch, as the ISO8601-UTC instant this service renders.
fn iso8601(secs: i64) -> String {
    DateTime::from_secs(secs)
        .fmt(Format::DateTime)
        .expect("an instant renders")
}

/// The expiration the store holds for the grant `token`: no operation REPORTS a
/// stored grant's bound, so a case about what an extension persisted reads it here.
fn stored_expiration(engine: &engine::InMemoryEngine, token: &str) -> Option<String> {
    let state = service_state(engine).expect("the license store is reachable");
    let store = state.lock();
    store
        .consumption(token)
        .unwrap_or_else(|| panic!("the store holds no grant under {token:?}"))
        .expiration
        .clone()
}

/// Move the stored expiration of the grant `token` to `expiration`, so a case can
/// start from a grant with less time left on it than a checkout taken now would
/// have.
fn set_stored_expiration(engine: &engine::InMemoryEngine, token: &str, expiration: &str) {
    let state = service_state(engine).expect("the license store is reachable");
    let moved = state
        .lock()
        .extend_consumption(token, Some(expiration.to_string()));
    assert!(moved, "the store holds no held grant under {token:?}");
}

/// One row of the allowed-checkout table.
struct AllowedRow {
    case: &'static str,
    /// What the license declares.
    declared: Vec<Value>,
    /// What the checkout asks for.
    requested: Vec<Value>,
    /// The `CheckoutType` the checkout asks for.
    checkout_type: &'static str,
    /// The node the checkout names, when it names one.
    node_id: Option<&'static str>,
    /// The `Value` the checkout must report per allowed entitlement, and the units
    /// the usage must then report consumed for it.
    expected: Vec<(&'static str, Option<&'static str>, i64)>,
}

/// The table: one row per form an allowed checkout takes.
fn allowed_rows() -> Vec<AllowedRow> {
    vec![
        AllowedRow {
            case: "one counted unit, the count named by Value",
            declared: vec![entitlement(
                "seats",
                "Count",
                &[("MaxCount", json!(10)), ("AllowCheckIn", json!(true))],
            )],
            requested: vec![requested("seats", "Count", Some("3"))],
            checkout_type: "PROVISIONAL",
            node_id: Some("node-counted"),
            expected: vec![("seats", Some("3"), 3)],
        },
        AllowedRow {
            case: "a request that names no Value takes one unit",
            declared: vec![entitlement("seats", "Count", &[("MaxCount", json!(10))])],
            requested: vec![requested("seats", "Count", None)],
            checkout_type: "PROVISIONAL",
            node_id: None,
            expected: vec![("seats", Some("1"), 1)],
        },
        AllowedRow {
            case: "the whole maximum count at once, which is at the cap and not past it",
            declared: vec![entitlement("runtime", "Seconds", &[("MaxCount", json!(3600))])],
            requested: vec![requested("runtime", "Seconds", Some("3600"))],
            checkout_type: "PERPETUAL",
            node_id: None,
            expected: vec![("runtime", Some("3600"), 3600)],
        },
        AllowedRow {
            case: "an entitlement that caps no maximum count caps nothing",
            declared: vec![entitlement("headroom", "Percent", &[])],
            requested: vec![requested("headroom", "Percent", Some("1000000"))],
            checkout_type: "PROVISIONAL",
            node_id: None,
            expected: vec![("headroom", Some("1000000"), 1_000_000)],
        },
        AllowedRow {
            case: "unit None carries the entitlement resource, and one grant on it",
            declared: vec![entitlement("site", "None", &[("Value", json!("site-wide"))])],
            requested: vec![requested("site", "None", None)],
            checkout_type: "PERPETUAL",
            node_id: Some("node-resource"),
            expected: vec![("site", Some("site-wide"), 1)],
        },
        AllowedRow {
            case: "three entitlements across three units in one checkout",
            declared: vec![
                entitlement(
                    "bandwidth",
                    "Gigabits/Second",
                    &[("MaxCount", json!(40))],
                ),
                entitlement("storage", "Terabytes", &[("MaxCount", json!(8))]),
                entitlement("site", "None", &[("Value", json!("whole-site"))]),
            ],
            requested: vec![
                requested("bandwidth", "Gigabits/Second", Some("40")),
                requested("storage", "Terabytes", Some("2")),
                requested("site", "None", Some("whole-site")),
            ],
            checkout_type: "PROVISIONAL",
            node_id: None,
            expected: vec![
                ("bandwidth", Some("40"), 40),
                ("storage", Some("2"), 2),
                ("site", Some("whole-site"), 1),
            ],
        },
        AllowedRow {
            case: "the same entitlement twice in one checkout sums the units it takes",
            declared: vec![entitlement("seats", "Count", &[("MaxCount", json!(10))])],
            requested: vec![
                requested("seats", "Count", Some("2")),
                requested("seats", "Count", Some("4")),
            ],
            checkout_type: "PROVISIONAL",
            node_id: None,
            expected: vec![("seats", Some("2"), 6), ("seats", Some("4"), 6)],
        },
        AllowedRow {
            case: "a count of zero takes nothing and is still a grant",
            declared: vec![entitlement("seats", "Count", &[("MaxCount", json!(0))])],
            requested: vec![requested("seats", "Count", Some("0"))],
            checkout_type: "PROVISIONAL",
            node_id: None,
            expected: vec![("seats", Some("0"), 0)],
        },
        AllowedRow {
            case: "an entitlement that allows overage grants past its maximum count",
            declared: vec![entitlement(
                "seats",
                "Count",
                &[("MaxCount", json!(2)), ("Overage", json!(true))],
            )],
            requested: vec![requested("seats", "Count", Some("5"))],
            checkout_type: "PROVISIONAL",
            node_id: None,
            expected: vec![("seats", Some("5"), 5)],
        },
    ]
}

#[tokio::test]
async fn an_allowed_checkout_is_granted_and_persisted() {
    let (router, _engine) = emulator();

    for (index, row) in allowed_rows().into_iter().enumerate() {
        let case = row.case;
        let sku = format!("sku-allowed-{index}");
        let (arn, fingerprint) =
            create_license(&router, case, &sku, Value::Array(row.declared.clone())).await;

        let (status, response) = call(
            &router,
            "CheckoutLicense",
            &checkout_body(
                &sku,
                &fingerprint,
                row.checkout_type,
                &format!("checkout-allowed-{index}"),
                Value::Array(row.requested.clone()),
                row.node_id,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CheckoutLicense to answer 200, got {status} {response}"
        );

        assert_eq!(
            response["LicenseArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the checkout to name the license it was taken on, got {response}"
        );
        assert_eq!(
            response["CheckoutType"].as_str(),
            Some(row.checkout_type),
            "case {case:?}: expected the checkout type {:?}, got {response}",
            row.checkout_type
        );
        assert_eq!(
            response["NodeId"].as_str(),
            row.node_id,
            "case {case:?}: expected the node {:?}, got {response}",
            row.node_id
        );
        let token = response["LicenseConsumptionToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the checkout answered no consumption token: {response}")
            });
        assert!(
            !token.is_empty(),
            "case {case:?}: the consumption token is empty: {response}"
        );

        // The entitlements allowed are the ones the license declares, each carrying
        // the value this grant holds of it.
        let allowed = response["EntitlementsAllowed"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the checkout answered no EntitlementsAllowed: {response}")
            })
            .clone();
        assert_eq!(
            allowed.len(),
            row.expected.len(),
            "case {case:?}: expected {} allowed entitlements, got {allowed:?}",
            row.expected.len()
        );
        for (position, (name, value, _)) in row.expected.iter().enumerate() {
            assert_eq!(
                allowed[position]["Name"].as_str(),
                Some(*name),
                "case {case:?}: allowed entitlement {position} is {:?}, expected {name:?}",
                allowed[position]
            );
            assert_eq!(
                allowed[position]["Value"].as_str(),
                *value,
                "case {case:?}: allowed entitlement {position} carries {:?}, expected the value {value:?}",
                allowed[position]
            );
        }

        // The grant is persisted: the usage of each entitlement reports the units it
        // consumed.
        for (name, _, units) in &row.expected {
            let consumed = consumed_units(&router, case, &arn, name).await;
            assert_eq!(
                consumed, *units,
                "case {case:?}: expected {units} units of {name:?} consumed, got {consumed}"
            );
        }

        // `IssuedAt` is an instant, and the expiration is the one the checkout type
        // bounds this grant at: the license's provisional time to live for a
        // PROVISIONAL checkout, and the end of the license's validity for a
        // PERPETUAL one.
        let issued_at = response["IssuedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the checkout answered no IssuedAt"));
        let issued = epoch_secs(case, "IssuedAt", issued_at);
        let expiration = response["Expiration"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the checkout answered no Expiration"));
        let expires = epoch_secs(case, "Expiration", expiration);
        if row.checkout_type == "PROVISIONAL" {
            let expected = issued + (PROVISIONAL_MINUTES * 60) as f64;
            assert!(
                (expires - expected).abs() <= 1.0,
                "case {case:?}: expected a PROVISIONAL checkout to expire {PROVISIONAL_MINUTES} \
                 minutes after {issued_at}, got {expiration}"
            );
        } else {
            assert_eq!(
                expiration, VALIDITY_END,
                "case {case:?}: expected a PERPETUAL checkout to expire with the license's \
                 validity, got {expiration}"
            );
        }

        // The signed token is a compact JWS over this grant, long enough for the
        // member the model bounds at a minimum length.
        let signed = response["SignedToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the checkout answered no SignedToken"));
        assert!(
            signed.len() >= SIGNED_TOKEN_MIN_LEN,
            "case {case:?}: the signed token is {} characters, fewer than the modelled \
             {SIGNED_TOKEN_MIN_LEN}",
            signed.len()
        );
        let claims = signed_token_claims(case, signed);
        assert_eq!(
            claims["jti"].as_str(),
            Some(token),
            "case {case:?}: expected the signed token to carry the consumption token, got {claims}"
        );
        assert_eq!(
            claims["sub"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the signed token to carry the license ARN, got {claims}"
        );
        assert_eq!(
            claims["ProductSKU"].as_str(),
            Some(sku.as_str()),
            "case {case:?}: expected the signed token to carry the product SKU, got {claims}"
        );
    }
}

/// One row of the refusal table: what the license declares, what the checkout asks
/// for, and the modelled error that refusal is answered on.
struct RefusalRow {
    case: &'static str,
    declared: Vec<Value>,
    requested: Vec<Value>,
    error: &'static str,
}

/// The table: one row per form a refused checkout takes. Every row asks for a
/// checkout the license cannot grant, so none of them may consume anything.
fn refusal_rows() -> Vec<RefusalRow> {
    vec![
        RefusalRow {
            case: "an entitlement name the license does not declare",
            declared: vec![entitlement("seats", "Count", &[("MaxCount", json!(10))])],
            requested: vec![requested("cores", "Count", Some("1"))],
            error: "NoEntitlementsAllowedException",
        },
        RefusalRow {
            case: "the declared name in a unit the license does not declare it in",
            declared: vec![entitlement("seats", "Count", &[("MaxCount", json!(10))])],
            requested: vec![requested("seats", "Seconds", Some("1"))],
            error: "NoEntitlementsAllowedException",
        },
        RefusalRow {
            case: "a unit of None against a counted entitlement",
            declared: vec![entitlement("seats", "Count", &[("MaxCount", json!(10))])],
            requested: vec![requested("seats", "None", None)],
            error: "NoEntitlementsAllowedException",
        },
        RefusalRow {
            case: "one unit past the maximum count",
            declared: vec![entitlement("seats", "Count", &[("MaxCount", json!(4))])],
            requested: vec![requested("seats", "Count", Some("5"))],
            error: "NoEntitlementsAllowedException",
        },
        RefusalRow {
            case: "any unit of an entitlement capped at none",
            declared: vec![entitlement("storage", "Terabytes", &[("MaxCount", json!(0))])],
            requested: vec![requested("storage", "Terabytes", Some("1"))],
            error: "NoEntitlementsAllowedException",
        },
        RefusalRow {
            case: "two requests of one entitlement summing past its maximum count",
            declared: vec![entitlement("seats", "Count", &[("MaxCount", json!(4))])],
            requested: vec![
                requested("seats", "Count", Some("3")),
                requested("seats", "Count", Some("2")),
            ],
            error: "NoEntitlementsAllowedException",
        },
        RefusalRow {
            case: "one allowed entitlement beside one the license does not declare",
            declared: vec![entitlement("seats", "Count", &[("MaxCount", json!(10))])],
            requested: vec![
                requested("seats", "Count", Some("1")),
                requested("cores", "Count", Some("1")),
            ],
            error: "NoEntitlementsAllowedException",
        },
        RefusalRow {
            case: "no entitlements at all",
            declared: vec![entitlement("seats", "Count", &[("MaxCount", json!(10))])],
            requested: Vec::new(),
            error: "NoEntitlementsAllowedException",
        },
        RefusalRow {
            case: "a Value that is not a count of the entitlement's unit",
            declared: vec![entitlement("seats", "Count", &[("MaxCount", json!(10))])],
            requested: vec![requested("seats", "Count", Some("two"))],
            error: "InvalidParameterValueException",
        },
        RefusalRow {
            case: "a negative count",
            declared: vec![entitlement("seats", "Count", &[("MaxCount", json!(10))])],
            requested: vec![requested("seats", "Count", Some("-1"))],
            error: "InvalidParameterValueException",
        },
    ]
}

#[tokio::test]
async fn a_checkout_the_license_cannot_grant_is_refused_and_consumes_nothing() {
    let (router, _engine) = emulator();

    for (index, row) in refusal_rows().into_iter().enumerate() {
        let case = row.case;
        let sku = format!("sku-refused-{index}");
        let (arn, fingerprint) =
            create_license(&router, case, &sku, Value::Array(row.declared.clone())).await;

        let (status, response) = call(
            &router,
            "CheckoutLicense",
            &checkout_body(
                &sku,
                &fingerprint,
                "PROVISIONAL",
                &format!("checkout-refused-{index}"),
                Value::Array(row.requested.clone()),
                None,
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the checkout to be refused with 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            row.error,
            "case {case:?}: expected {}, got {response}",
            row.error
        );
        if row.error == "NoEntitlementsAllowedException" {
            assert_eq!(
                error_message(&response),
                NO_ENTITLEMENTS_ALLOWED,
                "case {case:?}: expected the refusal AWS words for that error class, got \
                 {response}"
            );
        }

        // A refused checkout persisted nothing, so every entitlement the license
        // declares is still wholly unconsumed.
        for declared in &row.declared {
            let name = declared["Name"].as_str().expect("the row names it");
            let consumed = consumed_units(&router, case, &arn, name).await;
            assert_eq!(
                consumed, 0,
                "case {case:?}: expected the refusal to consume no units of {name:?}, got \
                 {consumed}"
            );
        }
    }
}

#[tokio::test]
async fn a_product_and_fingerprint_naming_no_license_is_not_found() {
    let (router, _engine) = emulator();

    let declared = vec![entitlement("seats", "Count", &[("MaxCount", json!(10))])];
    let (_, fingerprint) = create_license(
        &router,
        "a stored license",
        "sku-resolution",
        Value::Array(declared),
    )
    .await;
    let other = create_license(
        &router,
        "a license of another issuer",
        "sku-resolution-other",
        json!([entitlement("seats", "Count", &[("MaxCount", json!(10))])]),
    )
    .await;

    // Each row names a (product SKU, key fingerprint) pair no stored license
    // answers, and the member of the pair whose value named nothing: an unknown
    // product, the stored product under another issuer's fingerprint, and the stored
    // product under a fingerprint no issuer has.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a product SKU no license carries",
            "sku-nothing",
            &fingerprint,
            "productSKU",
        ),
        (
            "the stored product under another issuer's fingerprint",
            "sku-resolution",
            &other.1,
            "keyFingerprint",
        ),
        (
            "a fingerprint no issuer of this product has",
            "sku-resolution",
            "aws:000000000000:nobody:0000000000000000000000000000000000000000000000000000000000000000",
            "keyFingerprint",
        ),
    ];

    for (index, (case, sku, fingerprint, member)) in rows.into_iter().enumerate() {
        let (status, response) = call(
            &router,
            "CheckoutLicense",
            &checkout_body(
                sku,
                fingerprint,
                "PROVISIONAL",
                &format!("checkout-resolution-{index}"),
                json!([requested("seats", "Count", Some("1"))]),
                None,
            ),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the checkout to be refused with 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            format!("Invalid value of {member}."),
            "case {case:?}: expected the refusal to name the member that resolved nothing, got \
             {response}"
        );
    }
}

#[tokio::test]
async fn the_repeat_of_a_checkout_reports_the_grant_it_minted() {
    let (router, _engine) = emulator();

    let case = "the same ClientToken twice";
    let sku = "sku-idempotent";
    let (arn, fingerprint) = create_license(
        &router,
        case,
        sku,
        json!([entitlement("seats", "Count", &[("MaxCount", json!(10))])]),
    )
    .await;
    let body = checkout_body(
        sku,
        &fingerprint,
        "PROVISIONAL",
        "checkout-idempotent",
        json!([requested("seats", "Count", Some("4"))]),
        Some("node-idempotent"),
    );

    let (status, first) = call(&router, "CheckoutLicense", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the first checkout to answer 200, got {status} {first}"
    );
    let (status, again) = call(&router, "CheckoutLicense", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the repeat to answer 200, got {status} {again}"
    );
    assert_eq!(
        again, first,
        "case {case:?}: expected the repeat to report the grant the first call minted, got {again}"
    );

    let consumed = consumed_units(&router, case, &arn, "seats").await;
    assert_eq!(
        consumed, 4,
        "case {case:?}: expected the repeat to consume nothing more than the 4 units the first \
         call took, got {consumed}"
    );

    // Another client token on the same license is another grant, which consumes its
    // own units and answers its own tokens.
    let other = checkout_body(
        sku,
        &fingerprint,
        "PROVISIONAL",
        "checkout-idempotent-second",
        json!([requested("seats", "Count", Some("3"))]),
        None,
    );
    let (status, second) = call(&router, "CheckoutLicense", &other).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected a second checkout to answer 200, got {status} {second}"
    );
    assert_ne!(
        second["LicenseConsumptionToken"], first["LicenseConsumptionToken"],
        "case {case:?}: expected a second grant to carry its own consumption token, got {second}"
    );
    assert_ne!(
        second["SignedToken"], first["SignedToken"],
        "case {case:?}: expected a second grant to carry its own signed token, got {second}"
    );
    let consumed = consumed_units(&router, case, &arn, "seats").await;
    assert_eq!(
        consumed, 7,
        "case {case:?}: expected the two grants to consume 4 + 3 units, got {consumed}"
    );

    // The token is the caller's key on the license it was used against, so the SAME
    // token against another license is that license's own grant, not this one's.
    let elsewhere_sku = "sku-idempotent-elsewhere";
    let (elsewhere_arn, elsewhere_fingerprint) = create_license(
        &router,
        case,
        elsewhere_sku,
        json!([entitlement("seats", "Count", &[("MaxCount", json!(10))])]),
    )
    .await;
    let (status, elsewhere) = call(
        &router,
        "CheckoutLicense",
        &checkout_body(
            elsewhere_sku,
            &elsewhere_fingerprint,
            "PROVISIONAL",
            "checkout-idempotent",
            json!([requested("seats", "Count", Some("2"))]),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the token reused on another license to answer 200, got {status} \
         {elsewhere}"
    );
    assert_eq!(
        elsewhere["LicenseArn"].as_str(),
        Some(elsewhere_arn.as_str()),
        "case {case:?}: expected the grant to be taken on the license the request named, got \
         {elsewhere}"
    );
    assert_eq!(
        consumed_units(&router, case, &elsewhere_arn, "seats").await,
        2,
        "case {case:?}: expected the other license to carry its own 2 units"
    );
    assert_eq!(
        consumed_units(&router, case, &arn, "seats").await,
        7,
        "case {case:?}: expected the first license's usage to be untouched by it"
    );
}

#[tokio::test]
async fn a_check_in_releases_its_grant_exactly_once() {
    let (router, _engine) = emulator();

    let case = "a checked-out license checked back in";
    let sku = "sku-checkin";
    let (arn, fingerprint) = create_license(
        &router,
        case,
        sku,
        json!([entitlement(
            "seats",
            "Count",
            &[("MaxCount", json!(4)), ("AllowCheckIn", json!(true))]
        )]),
    )
    .await;

    // The whole maximum count is checked out, so nothing else can be granted until
    // the grant is released.
    let (status, checkout) = call(
        &router,
        "CheckoutLicense",
        &checkout_body(
            sku,
            &fingerprint,
            "PROVISIONAL",
            "checkout-checkin",
            json!([requested("seats", "Count", Some("4"))]),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the checkout to answer 200, got {status} {checkout}"
    );
    let token = checkout["LicenseConsumptionToken"]
        .as_str()
        .expect("the checkout answered a consumption token")
        .to_string();
    assert_eq!(
        consumed_units(&router, case, &arn, "seats").await,
        4,
        "case {case:?}: expected the checkout to consume the whole maximum count"
    );

    let (status, blocked) = call(
        &router,
        "CheckoutLicense",
        &checkout_body(
            sku,
            &fingerprint,
            "PROVISIONAL",
            "checkout-checkin-blocked",
            json!([requested("seats", "Count", Some("1"))]),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a checkout past the maximum count to be refused, got {status} \
         {blocked}"
    );
    assert_eq!(
        error_code(&blocked),
        "NoEntitlementsAllowedException",
        "case {case:?}: expected NoEntitlementsAllowedException, got {blocked}"
    );

    // The check-in releases the units: the usage drops to none, and the entitlement
    // can be granted again.
    let (status, released) = call(
        &router,
        "CheckInLicense",
        &json!({"LicenseConsumptionToken": token}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CheckInLicense to answer 200, got {status} {released}"
    );
    assert_eq!(
        consumed_units(&router, case, &arn, "seats").await,
        0,
        "case {case:?}: expected the check-in to release every unit the grant held"
    );

    let (status, regranted) = call(
        &router,
        "CheckoutLicense",
        &checkout_body(
            sku,
            &fingerprint,
            "PROVISIONAL",
            "checkout-checkin-regrant",
            json!([requested("seats", "Count", Some("4"))]),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the released units to be grantable again, got {status} \
         {regranted}"
    );
    assert_ne!(
        regranted["LicenseConsumptionToken"].as_str(),
        Some(token.as_str()),
        "case {case:?}: expected the regrant to mint its own token, got {regranted}"
    );

    // The released token is not reusable: checking it in again names no checkout
    // this service can check in, and neither does a token no checkout minted.
    let (status, refused) = call(
        &router,
        "CheckInLicense",
        &json!({"LicenseConsumptionToken": token}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a second check-in of {token:?} to answer 400, got {status} \
         {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {refused}"
    );
    assert_eq!(
        error_message(&refused),
        INVALID_CONSUMPTION_TOKEN,
        "case {case:?}: expected the refusal AWS words for that error class, got {refused}"
    );
    assert_eq!(
        consumed_units(&router, case, &arn, "seats").await,
        4,
        "case {case:?}: expected the refused check-in to release nothing of the regrant"
    );

    for (case, unknown) in [
        ("a token no checkout minted", "00000000000000000000000000000000"),
        ("a token of another shape entirely", "not-a-consumption-token"),
    ] {
        let (status, response) = call(
            &router,
            "CheckInLicense",
            &json!({"LicenseConsumptionToken": unknown}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the check-in of {unknown:?} to answer 400, got {status} \
             {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            INVALID_CONSUMPTION_TOKEN,
            "case {case:?}: expected the refusal AWS words for that error class, got {response}"
        );
    }
}

#[tokio::test]
async fn an_extension_moves_a_held_grant_to_the_bound_a_checkout_taken_now_would_carry() {
    let (router, engine) = emulator();

    let case = "a provisional grant with a minute left on it";
    let sku = "sku-extend";
    let (_, fingerprint) = create_license(
        &router,
        case,
        sku,
        json!([entitlement("seats", "Count", &[("MaxCount", json!(4))])]),
    )
    .await;

    let (status, checkout) = call(
        &router,
        "CheckoutLicense",
        &checkout_body(
            sku,
            &fingerprint,
            "PROVISIONAL",
            "checkout-extend",
            json!([requested("seats", "Count", Some("1"))]),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the checkout to answer 200, got {status} {checkout}"
    );
    let token = checkout["LicenseConsumptionToken"]
        .as_str()
        .expect("the checkout answered a consumption token")
        .to_string();

    // Start from a grant with a minute left on it, as one issued nearly its whole
    // time to live ago carries, so an extension is visible as a move rather than as
    // the bound the checkout already had.
    let nearly_over = iso8601(now_secs() + 60);
    set_stored_expiration(&engine, &token, &nearly_over);

    // A dry run says what the extension WOULD do and persists nothing.
    let (status, dry) = call(
        &router,
        "ExtendLicenseConsumption",
        &json!({"LicenseConsumptionToken": token, "DryRun": true}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the dry run to answer 200, got {status} {dry}"
    );
    assert_eq!(
        dry["LicenseConsumptionToken"].as_str(),
        Some(token.as_str()),
        "case {case:?}: expected the dry run to name the token it was asked about, got {dry}"
    );
    let dry_expiration = dry["Expiration"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the dry run answered no Expiration: {dry}"));
    let expected = (now_secs() + PROVISIONAL_MINUTES * 60) as f64;
    assert!(
        (epoch_secs(case, "Expiration", dry_expiration) - expected).abs() <= 2.0,
        "case {case:?}: expected the extension to reach {PROVISIONAL_MINUTES} minutes from now, \
         got {dry_expiration}"
    );
    assert_eq!(
        stored_expiration(&engine, &token).as_deref(),
        Some(nearly_over.as_str()),
        "case {case:?}: the dry run persisted an extension it was asked not to make"
    );

    // The extension itself persists the new bound, so it outlives the call.
    let (status, extended) = call(
        &router,
        "ExtendLicenseConsumption",
        &json!({"LicenseConsumptionToken": token, "DryRun": false}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the extension to answer 200, got {status} {extended}"
    );
    let moved_to = extended["Expiration"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the extension answered no Expiration"));
    assert!(
        (epoch_secs(case, "Expiration", moved_to) - expected).abs() <= 2.0,
        "case {case:?}: expected the extension to reach {PROVISIONAL_MINUTES} minutes from now, \
         got {moved_to}"
    );
    assert_eq!(
        stored_expiration(&engine, &token).as_deref(),
        Some(moved_to),
        "case {case:?}: expected the grant to hold the bound the extension answered"
    );

    // A grant a check-in has released, and a token no checkout minted, name no held
    // consumption, so neither can be extended.
    let (status, released) = call(
        &router,
        "CheckInLicense",
        &json!({"LicenseConsumptionToken": token}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the check-in to answer 200, got {status} {released}"
    );

    for (case, unextendable) in [
        ("a token a check-in released", token.as_str()),
        (
            "a token no checkout minted",
            "00000000000000000000000000000000",
        ),
    ] {
        let (status, response) = call(
            &router,
            "ExtendLicenseConsumption",
            &json!({"LicenseConsumptionToken": unextendable, "DryRun": false}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the extension of {unextendable:?} to answer 400, got \
             {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            INVALID_CONSUMPTION_TOKEN,
            "case {case:?}: expected the refusal AWS words for that error class, got {response}"
        );
    }

    // The released grant's bound is the one the extension left: a refused extension
    // moved nothing.
    assert_eq!(
        stored_expiration(&engine, &token).as_deref(),
        Some(moved_to),
        "case {case:?}: the refused extension moved the grant's bound"
    );
}

#[tokio::test]
async fn the_store_bounds_the_grants_it_holds_rather_than_their_history() {
    let (router, _engine) = emulator();

    let case = "a store filled to its cap of held grants";
    let sku = "sku-cap";
    // One entitlement capping no maximum count, so the cap under test is the store's
    // own and not the license's.
    let (arn, fingerprint) =
        create_license(&router, case, sku, json!([entitlement("seats", "Count", &[])])).await;
    let one_unit = json!([requested("seats", "Count", Some("1"))]);

    let mut tokens = Vec::new();
    for index in 0..MAX_HELD_GRANTS {
        let (status, response) = call(
            &router,
            "CheckoutLicense",
            &checkout_body(
                sku,
                &fingerprint,
                "PROVISIONAL",
                &format!("checkout-cap-{index}"),
                one_unit.clone(),
                None,
            ),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected checkout {index} of {MAX_HELD_GRANTS} to answer 200, got \
             {status} {response}"
        );
        tokens.push(
            response["LicenseConsumptionToken"]
                .as_str()
                .expect("the checkout answered a consumption token")
                .to_string(),
        );
    }
    assert_eq!(
        consumed_units(&router, case, &arn, "seats").await,
        MAX_HELD_GRANTS as i64,
        "case {case:?}: expected every grant taken to be held"
    );

    // At the cap with every grant still held, there is nothing to reclaim, so the
    // checkout is refused on the server channel and consumes nothing.
    let (status, refused) = call(
        &router,
        "CheckoutLicense",
        &checkout_body(
            sku,
            &fingerprint,
            "PROVISIONAL",
            "checkout-cap-over",
            one_unit.clone(),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 500,
        "case {case:?}: expected a checkout past the store's cap to be refused, got {status} \
         {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServerInternalException",
        "case {case:?}: expected ServerInternalException, got {refused}"
    );
    assert_eq!(
        consumed_units(&router, case, &arn, "seats").await,
        MAX_HELD_GRANTS as i64,
        "case {case:?}: expected the refused checkout to consume nothing"
    );

    // Releasing one grant makes room for another: the cap bounds what the store
    // HOLDS, so the history of released grants cannot wedge it shut.
    let (status, released) = call(
        &router,
        "CheckInLicense",
        &json!({"LicenseConsumptionToken": tokens[0]}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the check-in to answer 200, got {status} {released}"
    );
    let (status, after) = call(
        &router,
        "CheckoutLicense",
        &checkout_body(
            sku,
            &fingerprint,
            "PROVISIONAL",
            "checkout-cap-after",
            one_unit,
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the freed slot to take another grant, got {status} {after}"
    );
    assert_eq!(
        consumed_units(&router, case, &arn, "seats").await,
        MAX_HELD_GRANTS as i64,
        "case {case:?}: expected the released unit to be replaced by the new grant's"
    );

    // The reclaimed record changes nothing a caller can see: its token still names
    // no checkout to check in.
    let (status, response) = call(
        &router,
        "CheckInLicense",
        &json!({"LicenseConsumptionToken": tokens[0]}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the released token to be refused, got {status} {response}"
    );
    assert_eq!(
        error_message(&response),
        INVALID_CONSUMPTION_TOKEN,
        "case {case:?}: expected the refusal AWS words for that error class, got {response}"
    );
}

#[tokio::test]
async fn an_expiration_the_clock_has_passed_releases_the_grant_without_a_write() {
    let (router, engine) = emulator();

    let case = "a grant whose expiration the clock has passed";
    let sku = "sku-expiry";
    // A maximum count of one, so the released units are visible as a checkout that
    // the cap would otherwise refuse.
    let (arn, fingerprint) = create_license(
        &router,
        case,
        sku,
        json!([entitlement("seats", "Count", &[("MaxCount", json!(1))])]),
    )
    .await;
    let one_seat = json!([requested("seats", "Count", Some("1"))]);

    let (status, checkout) = call(
        &router,
        "CheckoutLicense",
        &checkout_body(
            sku,
            &fingerprint,
            "PROVISIONAL",
            "checkout-expiry",
            one_seat.clone(),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the checkout to answer 200, got {status} {checkout}"
    );
    let token = checkout["LicenseConsumptionToken"]
        .as_str()
        .expect("the checkout answered a consumption token")
        .to_string();
    assert_eq!(
        consumed_units(&router, case, &arn, "seats").await,
        1,
        "case {case:?}: expected the grant to hold the one seat it took"
    );
    // The license caps one seat, so while that grant holds it no other checkout can
    // be granted.
    let (status, blocked) = call(
        &router,
        "CheckoutLicense",
        &checkout_body(
            sku,
            &fingerprint,
            "PROVISIONAL",
            "checkout-expiry-blocked",
            one_seat.clone(),
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the capped entitlement to refuse a second grant, got {status} \
         {blocked}"
    );

    // Pull the grant's stored expiration behind now: the clock has made its release
    // due, and nothing has to be called for that to hold.
    let over = iso8601(now_secs() - 1);
    set_stored_expiration(&engine, &token, &over);

    assert_eq!(
        consumed_units(&router, case, &arn, "seats").await,
        0,
        "case {case:?}: expected the expired grant to hold no units"
    );
    // Reporting that usage wrote nothing: the grant's stored bound is still the one
    // the case set, and the reading did not release it a second way.
    assert_eq!(
        stored_expiration(&engine, &token).as_deref(),
        Some(over.as_str()),
        "case {case:?}: reporting the usage rewrote the grant"
    );

    // The units are free, so the capped entitlement can be granted again.
    let (status, regranted) = call(
        &router,
        "CheckoutLicense",
        &checkout_body(
            sku,
            &fingerprint,
            "PROVISIONAL",
            "checkout-expiry-regrant",
            one_seat,
            None,
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the expired grant's seat to be grantable again, got {status} \
         {regranted}"
    );

    // The expired token names no checkout either operation can act on, and both
    // refuse it the way they refuse one no checkout minted.
    for (operation, body) in [
        (
            "CheckInLicense",
            json!({"LicenseConsumptionToken": token.clone()}),
        ),
        (
            "ExtendLicenseConsumption",
            json!({"LicenseConsumptionToken": token.clone(), "DryRun": false}),
        ),
    ] {
        let (status, response) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to refuse the expired token, got {status} \
             {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected {operation} to answer ResourceNotFoundException, got \
             {response}"
        );
        assert_eq!(
            error_message(&response),
            INVALID_CONSUMPTION_TOKEN,
            "case {case:?}: expected {operation} to answer the refusal AWS words for that error \
             class, got {response}"
        );
    }
}
