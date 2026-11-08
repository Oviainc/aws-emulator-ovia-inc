// SPDX-License-Identifier: CC0-1.0

//! How Account words the refusals whose wording this suite pins, end to end over
//! HTTP. Three rules, each driven over a table that spans the class rather than one
//! request:
//!
//! 1. A request body that does not carry a property the schema REQUIRES is refused
//!    400 `ValidationException` whose message is the JSON-Schema finding
//!    `[object has missing required properties (["<Property>"])]` — the property
//!    named as the model declares it, with no member path, no value and no count.
//! 2. A number outside the `@range` its member declares is refused 400
//!    `ValidationException` whose message is the JSON-Schema finding for the bound
//!    it broke: `[numeric instance is greater than the required maximum (maximum:
//!    <max>, found: <value>)]` above the maximum and `[numeric instance is lower
//!    than the required minimum (minimum: <min>, found: <value>)]` below the
//!    minimum, with the bound from the model and the number from the request.
//! 3. A Region the partition turns on for every account is not a Region-opt target,
//!    so both opt operations refuse it on its `RegionName` with
//!    `<RegionName> is not a valid region for opt-in or opt-out.` — the Region code
//!    taken from the request.
//!
//! Each rule holds for every so-constrained member of every operation and at any
//! depth in the input, so the tables differ along what the rules generalise over:
//! the operation, the member and its depth (top level and nested inside a
//! structure), the member's target (a string, an enumerated type, a structure, a
//! number), the bound (each published one and a value past it), the sign and
//! magnitude of a refused number, and the Region and the opt operation. Every
//! constraint a row states — the required members, the `@range` bounds — is spelled
//! out here from the model rather than imported from the crate, so an
//! implementation that renders the wrong one cannot make this suite pass. Each rule
//! also carries its counter-case, so none of them collapses into "refuse
//! everything" or "word every refusal this way".

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The wire error code and status of the modelled `ValidationException`, from
/// `smithy_query(query="shape", shape="ValidationException")` (`errorKind:
/// client`, `httpError: 400`).
const VALIDATION_EXCEPTION: &str = "ValidationException";
const VALIDATION_STATUS: u16 = 400;

/// The `ValidationExceptionReason` a refusal detected on one input member
/// reports, from
/// `smithy_query(query="shape", shape="ValidationExceptionReason")`.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// `MaxResults`' modelled `@range(min: 1, max: 50)` on `ListRegions`, from
/// `smithy_query(query="operation", operation="ListRegions")`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 50;

/// The `RegionOptStatus` a Region the partition turns on for every account
/// carries, from `smithy_query(query="shape", shape="RegionOptStatus")`.
const ENABLED_BY_DEFAULT: &str = "ENABLED_BY_DEFAULT";

/// A member account id (`^\d{12}$`) for the rows whose operation requires one.
const MEMBER_ACCOUNT: &str = "123456789012";

/// Regions Amazon Web Services turns on for every account, so neither opt call can
/// act on them. SOURCE: the AWS General Reference Region table "Available Regions"
/// (<https://docs.aws.amazon.com/general/latest/gr/rande-manage.html>), the rows
/// marked "Enabled by default". Spelled out here rather than imported, so a wrong
/// catalog in the implementation cannot make this suite pass; five rows from five
/// geographies, four of them not named by the issue this suite was written for.
const DEFAULT_ENABLED: [&str; 5] = [
    "us-east-1",
    "eu-west-2",
    "sa-east-1",
    "ap-northeast-1",
    "us-west-1",
];

/// Regions of the same table marked "Must be enabled": each IS a Region-opt
/// target, so the opt-target refusal must not reach it.
const OPT_IN: [&str; 2] = ["af-south-1", "eu-south-2"];

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error `message`, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// One member of this answer's body, as a string (empty when absent).
    fn member(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    /// The `name`/`message` pairs of this answer's `fieldList` (empty when absent).
    fn field_list(&self) -> Vec<(String, String)> {
        self.body["fieldList"]
            .as_array()
            .map(|fields| {
                fields
                    .iter()
                    .map(|field| {
                        (
                            field["name"].as_str().unwrap_or_default().to_string(),
                            field["message"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, uri: &str, body: Value) -> Answer {
    let payload = body.to_string();
    let request = ::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// The whole `ValidationException.message` for a refusal stating `findings`.
fn bracketed(findings: &[String]) -> String {
    format!("[{}]", findings.join(", "))
}

/// The finding stated for an object missing the required properties `properties`.
fn missing_properties(properties: &[&str]) -> String {
    let quoted: Vec<String> = properties
        .iter()
        .map(|property| format!("\"{property}\""))
        .collect();
    format!(
        "object has missing required properties ([{}])",
        quoted.join(",")
    )
}

/// The finding stated for a number above the maximum its member declares.
fn above_maximum(value: i64, max: i64) -> String {
    format!(
        "numeric instance is greater than the required maximum (maximum: {max}, found: {value})"
    )
}

/// The finding stated for a number below the minimum its member declares.
fn below_minimum(value: i64, min: i64) -> String {
    format!("numeric instance is lower than the required minimum (minimum: {min}, found: {value})")
}

/// The sentence both opt operations refuse a Region that is not an opt target with.
fn not_an_opt_target(region_name: &str) -> String {
    format!("{region_name} is not a valid region for opt-in or opt-out.")
}

/// A complete `ContactInformation`, every member of it valid: the rows below omit
/// ONE required member of it, so each isolates the property it is about.
///
/// `StateOrRegion` is present because that member's own documentation requires it
/// in the United States.
fn contact_information() -> Value {
    json!({
        "FullName": "Ada Lovelace",
        "AddressLine1": "410 Terry Ave N",
        "City": "Seattle",
        "StateOrRegion": "WA",
        "PostalCode": "98109",
        "CountryCode": "US",
        "PhoneNumber": "+1 206-555-0100",
    })
}

/// A `PutContactInformation` body whose `ContactInformation` carries every member
/// but `omitted`.
fn contact_information_without(omitted: &str) -> Value {
    let mut contact = contact_information();
    contact
        .as_object_mut()
        .expect("the contact information is a JSON object")
        .remove(omitted);
    json!({ "ContactInformation": contact })
}

/// Every `@required` member of this service's operations, left out one at a time:
/// the refusal is exactly the bracketed JSON-Schema finding naming that property as
/// the model declares it.
///
/// The required members are from `smithy_query(query="operation", operation="…")`
/// over this service's operations and `smithy_query(query="shape",
/// shape="ContactInformation")` for the nested ones. Each row omits exactly ONE of
/// them, so the finding names that one property.
#[tokio::test]
async fn an_absent_required_property_is_reported_as_the_schema_finding() {
    // (case, uri, request, the property the request left out)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "the primary contact's nested postal code",
            "/putContactInformation",
            contact_information_without("PostalCode"),
            "PostalCode",
        ),
        (
            "the primary contact's nested two-letter country code",
            "/putContactInformation",
            contact_information_without("CountryCode"),
            "CountryCode",
        ),
        (
            "the primary contact's nested phone number",
            "/putContactInformation",
            contact_information_without("PhoneNumber"),
            "PhoneNumber",
        ),
        (
            "the primary contact's nested full name",
            "/putContactInformation",
            contact_information_without("FullName"),
            "FullName",
        ),
        (
            "the primary contact's nested first address line",
            "/putContactInformation",
            contact_information_without("AddressLine1"),
            "AddressLine1",
        ),
        (
            "the primary contact's nested city",
            "/putContactInformation",
            contact_information_without("City"),
            "City",
        ),
        (
            // The required member is a STRUCTURE rather than a scalar, and it is
            // at the TOP level of the request.
            "the whole contact information structure",
            "/putContactInformation",
            json!({}),
            "ContactInformation",
        ),
        (
            "the account name on a write",
            "/putAccountName",
            json!({}),
            "AccountName",
        ),
        (
            "the Region code on an opt-in",
            "/enableRegion",
            json!({}),
            "RegionName",
        ),
        (
            "the Region code on an opt-out",
            "/disableRegion",
            json!({}),
            "RegionName",
        ),
        (
            "the Region code on a status read",
            "/getRegionOptStatus",
            json!({}),
            "RegionName",
        ),
        (
            // The required member targets an enumerated type.
            "the alternate contact type on a read",
            "/getAlternateContact",
            json!({}),
            "AlternateContactType",
        ),
        (
            "the alternate contact type on a delete",
            "/deleteAlternateContact",
            json!({}),
            "AlternateContactType",
        ),
        (
            "the one-time passcode on a phone verification",
            "/verifyPhoneNumber",
            json!({}),
            "Otp",
        ),
        (
            "the primary email address of an update",
            "/startPrimaryEmailUpdate",
            json!({"AccountId": MEMBER_ACCOUNT}),
            "PrimaryEmail",
        ),
        (
            "the account id of an update",
            "/startPrimaryEmailUpdate",
            json!({"PrimaryEmail": "ada@example.com"}),
            "AccountId",
        ),
        (
            "the one-time passcode of an update acceptance",
            "/acceptPrimaryEmailUpdate",
            json!({"AccountId": MEMBER_ACCOUNT, "PrimaryEmail": "ada@example.com"}),
            "Otp",
        ),
    ];

    let router = emulator();
    for (case, uri, request, omitted) in cases {
        let answer = call(&router, uri, request).await;
        assert_eq!(
            answer.status, VALIDATION_STATUS,
            "case {case:?}: {uri} must answer {VALIDATION_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.message(),
            bracketed(&[missing_properties(&[omitted])]),
            "case {case:?}: expected the missing-property finding, got {:?}",
            answer.message()
        );
    }
}

/// The counter-case for rule 1: a request that DOES carry every required property
/// is not refused on one. A stub or a missing resource answers something other than
/// 200 here, which is still an acceptance of the input: what matters is that it is
/// not the refusal an omitted property earns.
#[tokio::test]
async fn a_request_carrying_every_required_property_is_not_refused_for_a_missing_one() {
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "the complete contact information",
            "/putContactInformation",
            json!({ "ContactInformation": contact_information() }),
        ),
        (
            "an account name",
            "/putAccountName",
            json!({"AccountName": "Demo Account"}),
        ),
        (
            "a Region code on a status read",
            "/getRegionOptStatus",
            json!({"RegionName": OPT_IN[0]}),
        ),
        (
            "an alternate contact type",
            "/getAlternateContact",
            json!({"AlternateContactType": "SECURITY"}),
        ),
        (
            "a one-time passcode",
            "/verifyPhoneNumber",
            json!({"Otp": "a1B2c3"}),
        ),
    ];

    let router = emulator();
    for (case, uri, request) in cases {
        let answer = call(&router, uri, request).await;
        assert!(
            !answer.message().starts_with("[object has missing"),
            "case {case:?}: {uri} must not report a missing property for a complete request, \
             got {} with {:?}",
            answer.status,
            answer.body
        );
    }
}

/// A body that leaves out TWO required properties of the same object is still
/// reported as ONE finding: absence is found while READING the request, and that
/// read stops at the first property it misses. Which of the two it misses first is
/// the order the read visits the object's members in, so the message is pinned
/// against the two findings that list admits.
#[tokio::test]
async fn two_absent_required_properties_are_reported_as_one_finding() {
    let router = emulator();
    let mut contact = contact_information();
    let members = contact
        .as_object_mut()
        .expect("the contact information is a JSON object");
    members.remove("City");
    members.remove("PostalCode");
    let answer = call(
        &router,
        "/putContactInformation",
        json!({ "ContactInformation": contact }),
    )
    .await;

    assert_eq!(
        answer.status, VALIDATION_STATUS,
        "case \"two absent required properties\": expected {VALIDATION_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
    let admitted = [
        bracketed(&[missing_properties(&["City"])]),
        bracketed(&[missing_properties(&["PostalCode"])]),
    ];
    assert!(
        admitted.contains(&answer.message().to_string()),
        "case \"two absent required properties\": expected one of {admitted:?}, got {:?}",
        answer.message()
    );
}

/// A number outside its member's modelled `@range` is refused with the bracketed
/// JSON-Schema finding for the bound it broke, the bound read off the model and the
/// number taken from the request. Each published bound itself is accepted, so the
/// refusals are the bound and not a blanket rejection of the member.
#[tokio::test]
async fn a_number_outside_its_range_is_reported_as_the_schema_finding() {
    // (case, MaxResults, the finding it earns; `None` for an admitted value)
    let cases: Vec<(&str, i64, Option<String>)> = vec![
        ("the minimum itself", MAX_RESULTS_MIN, None),
        ("inside the range", 7, None),
        ("the maximum itself", MAX_RESULTS_MAX, None),
        (
            "one below the minimum",
            MAX_RESULTS_MIN - 1,
            Some(below_minimum(MAX_RESULTS_MIN - 1, MAX_RESULTS_MIN)),
        ),
        (
            "a negative page size",
            -17,
            Some(below_minimum(-17, MAX_RESULTS_MIN)),
        ),
        (
            "one past the maximum",
            MAX_RESULTS_MAX + 1,
            Some(above_maximum(MAX_RESULTS_MAX + 1, MAX_RESULTS_MAX)),
        ),
        (
            "far past the maximum",
            1_000,
            Some(above_maximum(1_000, MAX_RESULTS_MAX)),
        ),
    ];

    let router = emulator();
    for (case, max_results, finding) in cases {
        let answer = call(&router, "/listRegions", json!({"MaxResults": max_results})).await;
        match finding {
            None => assert_eq!(
                answer.status, 200,
                "case {case:?}: MaxResults={max_results} is inside the modelled range, got {} \
                 with {:?}",
                answer.status, answer.body
            ),
            Some(finding) => {
                assert_eq!(
                    answer.status, VALIDATION_STATUS,
                    "case {case:?}: MaxResults={max_results} is outside the modelled range, so \
                     the call must answer {VALIDATION_STATUS}, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    VALIDATION_EXCEPTION,
                    "case {case:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.message(),
                    bracketed(std::slice::from_ref(&finding)),
                    "case {case:?}: expected the numeric-bound finding, got {:?}",
                    answer.message()
                );
            }
        }
    }
}

/// The two operations that decide by the Region-opt-target rule, each as the name
/// this suite reports it by and the URI it is reached at.
const OPT_OPERATIONS: [(&str, &str); 2] = [
    ("EnableRegion", "/enableRegion"),
    ("DisableRegion", "/disableRegion"),
];

/// Both opt operations refuse a Region the partition turns on for every account
/// with the same sentence about the `RegionName` the request named — and leave that
/// Region's opt status exactly as the catalog has it.
#[tokio::test]
async fn a_region_that_is_not_an_opt_target_is_refused_with_the_opt_target_sentence() {
    let router = emulator();

    for region in DEFAULT_ENABLED {
        for (operation, uri) in OPT_OPERATIONS {
            let answer = call(&router, uri, json!({"RegionName": region})).await;
            assert_eq!(
                answer.status, VALIDATION_STATUS,
                "case {region:?}/{operation}: a Region that is not an opt target must answer \
                 {VALIDATION_STATUS}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                VALIDATION_EXCEPTION,
                "case {region:?}/{operation}: expected {VALIDATION_EXCEPTION}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.message(),
                not_an_opt_target(region),
                "case {region:?}/{operation}: expected the opt-target sentence, got {:?}",
                answer.message()
            );
            assert_eq!(
                answer.member("reason"),
                FIELD_VALIDATION_FAILED,
                "case {region:?}/{operation}: expected reason {FIELD_VALIDATION_FAILED}, \
                 got {:?}",
                answer.body
            );
            assert_eq!(
                answer.field_list(),
                vec![(
                    "RegionName".to_string(),
                    not_an_opt_target(region).to_string()
                )],
                "case {region:?}/{operation}: fieldList must name RegionName with the same \
                 sentence, got {:?}",
                answer.body
            );
        }

        // The refused calls wrote nothing: the status a later caller reads is the
        // one the catalog gives the Region.
        let status = call(&router, "/getRegionOptStatus", json!({"RegionName": region})).await;
        assert_eq!(
            status.member("RegionOptStatus"),
            ENABLED_BY_DEFAULT,
            "case {region:?}: a refused opt call must leave the status alone, got {:?}",
            status.body
        );
    }
}

/// The counter-case for rule 3: a Region that IS an opt target is not refused with
/// that sentence, and a value that is no Region code at all earns the OTHER refusal
/// of the same member rather than this one.
#[tokio::test]
async fn an_opt_target_and_an_unknown_region_do_not_get_the_opt_target_sentence() {
    let router = emulator();

    for region in OPT_IN {
        let answer = call(&router, "/disableRegion", json!({"RegionName": region})).await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: an opt-in Region is an opt target, so the call must answer 200, \
             got {} with {:?}",
            answer.status, answer.body
        );
    }

    for value in ["eu-west-9", "somewhere"] {
        let answer = call(&router, "/enableRegion", json!({"RegionName": value})).await;
        assert_eq!(
            answer.status, VALIDATION_STATUS,
            "case {value:?}: a value that is not a Region code must answer {VALIDATION_STATUS}, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert_ne!(
            answer.message(),
            not_an_opt_target(value),
            "case {value:?}: a value that is not a Region code must not be reported as a Region \
             that cannot be opted into, got {:?}",
            answer.body
        );
    }
}
