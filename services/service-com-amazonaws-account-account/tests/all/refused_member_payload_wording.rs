// SPDX-License-Identifier: CC0-1.0

//! The BODY this service answers with when it refuses a request because one
//! named input member is at fault, driven end to end over HTTP.
//!
//! Two rules, both about the payload the model declares for
//! `ValidationException` — `message` (required), `reason`
//! (`ValidationExceptionReason`) and `fieldList` ("The field where the invalid
//! entry was detected"), from
//! `smithy_query(query="shape", shape="ValidationException")`:
//!
//! 1. A member of a submitted structure that the value of a SIBLING member makes
//!    required, and that the request leaves out, is refused 400
//!    `ValidationException` whose sentence is
//!    `'<MemberName>' field is required for country '<CountryCode>'` — the
//!    modelled member name and the submitted country code quoted — with reason
//!    `fieldValidationFailed` and a one-entry `fieldList` naming that member and
//!    carrying the same sentence.
//! 2. A `RegionName` the partition turns on for every account is not a Region-opt
//!    target, so both opt operations refuse it 400 `ValidationException` with the
//!    sentence `<RegionName> is not a valid region for opt-in or opt-out.`, reason
//!    `fieldValidationFailed` and a one-entry `fieldList` naming `RegionName`
//!    with that same sentence.
//!
//! Both are instances of one payload rule: a refusal this service decides on the
//! value of a single input member reports `fieldValidationFailed` and names that
//! member in `fieldList` with the sentence the `message` carries, so two refusals
//! of the same member are told apart by the sentence alone. The tables differ
//! along what the rules generalise over — rule 1 over every country its
//! documentation names, rule 2 over Regions from five geographies and both opt
//! operations — and each rule carries its counter-cases, so neither collapses
//! into "refuse everything" or "word every refusal this way".

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

/// The countries whose primary contact address must carry a `StateOrRegion`,
/// with a state of that country. SOURCE: that member's own documentation —
/// "This field is required in the following countries: US, CA, GB, DE, JP, IN,
/// and BR" (`smithy_query(query="doc", shape="ContactInformation",
/// member="StateOrRegion")`). Spelled out here rather than imported, so a wrong
/// set in the implementation cannot make this suite pass; every row is checked,
/// not only the one the issue behind this suite cited.
const REQUIRES_STATE_OR_REGION: [(&str, &str); 7] = [
    ("US", "WA"),
    ("CA", "ON"),
    ("GB", "England"),
    ("DE", "Bayern"),
    ("JP", "Tokyo"),
    ("IN", "Karnataka"),
    ("BR", "São Paulo"),
];

/// Assigned country codes OUTSIDE the seven above: an address in one of them is
/// complete with no state or region, so the refusal is not a blanket one.
/// SOURCE: the ISO 3166-1 alpha-2 code list as published.
const REQUIRES_NO_STATE_OR_REGION: [&str; 3] = ["AU", "FR", "ZA"];

/// Regions Amazon Web Services turns on for every account, so neither opt call
/// can act on them. SOURCE: the AWS General Reference Region table "Available
/// Regions" (<https://docs.aws.amazon.com/general/latest/gr/rande-manage.html>),
/// the rows marked "Enabled by default": five rows from five geographies, four of
/// them not named by the issue behind this suite.
const DEFAULT_ENABLED: [&str; 5] = [
    "us-east-1",
    "ap-northeast-2",
    "ca-central-1",
    "eu-west-3",
    "sa-east-1",
];

/// Regions of the same table marked "Must be enabled": each IS a Region-opt
/// target, so neither refusal reaches it.
const OPT_IN: [&str; 2] = ["af-south-1", "il-central-1"];

/// Values that are no Region code of this partition at all: the OTHER refusal of
/// the same member, which reports the same reason with a different sentence.
const NOT_REGION_CODES: [&str; 2] = ["zz-nowhere-1", "somewhere"];

/// The two operations that decide by the Region-opt-target rule, as the name this
/// suite reports each by and the URI it is reached at.
const OPT_OPERATIONS: [(&str, &str); 2] = [
    ("EnableRegion", "/enableRegion"),
    ("DisableRegion", "/disableRegion"),
];

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

    /// The `name`/`message` pairs of this answer's `fieldList` (empty when
    /// absent).
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

/// A `ContactInformation` whose every member satisfies its modelled constraint,
/// in `country_code`, carrying `state_or_region` only when one is given — so a
/// refusal of it is a refusal of the missing state and nothing else.
fn contact_in(country_code: &str, state_or_region: Option<&str>) -> Value {
    let mut contact = json!({
        "FullName": "Grace Hopper",
        "AddressLine1": "1 Compiler Road",
        "City": "Example City",
        "PostalCode": "00003",
        "CountryCode": country_code,
        "PhoneNumber": "+1 206-555-0188",
    });
    if let Some(state) = state_or_region {
        contact["StateOrRegion"] = json!(state);
    }
    contact
}

/// The sentence rule 1 states, for the member that is missing and the country
/// code the request submitted.
fn required_for_country(member: &str, country_code: &str) -> String {
    format!("'{member}' field is required for country '{country_code}'")
}

/// The sentence rule 2 states, for the `RegionName` the request submitted.
fn not_an_opt_target(region_name: &str) -> String {
    format!("{region_name} is not a valid region for opt-in or opt-out.")
}

/// Assert `answer` is the 400 `ValidationException` whose `message` is `stated`,
/// whose `reason` is `fieldValidationFailed` and whose `fieldList` is the single
/// entry naming `member` with the same sentence.
fn assert_member_refusal(case: &str, answer: &Answer, member: &str, stated: &str) {
    assert_eq!(
        answer.status, VALIDATION_STATUS,
        "case {case:?}: expected {VALIDATION_STATUS}, got {} with {:?}",
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
        stated,
        "case {case:?}: expected message {stated:?}, got {:?}",
        answer.message()
    );
    assert_eq!(
        answer.member("reason"),
        FIELD_VALIDATION_FAILED,
        "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.field_list(),
        vec![(member.to_string(), stated.to_string())],
        "case {case:?}: fieldList must be the one entry naming {member} with that sentence, \
         got {:?}",
        answer.body
    );
}

/// Rule 1, over every country whose addresses must carry a state or region: the
/// refusal quotes the missing member and the submitted country code, reports
/// `fieldValidationFailed` and names the member in `fieldList`.
#[tokio::test]
async fn a_member_the_country_requires_is_refused_quoting_the_member_and_the_country() {
    let router = emulator();

    for (country_code, state_or_region) in REQUIRES_STATE_OR_REGION {
        let case = format!("{country_code} with no StateOrRegion");
        let answer = call(
            &router,
            "/putContactInformation",
            json!({ "ContactInformation": contact_in(country_code, None) }),
        )
        .await;
        assert_member_refusal(
            &case,
            &answer,
            "StateOrRegion",
            &required_for_country("StateOrRegion", country_code),
        );

        // The counter-case of the same row: the same address carrying a state of
        // that country is accepted, so the refusal is about the absent member.
        let accepted = call(
            &router,
            "/putContactInformation",
            json!({ "ContactInformation": contact_in(country_code, Some(state_or_region)) }),
        )
        .await;
        assert_eq!(
            accepted.status, 200,
            "case {case:?}: the same address carrying {state_or_region:?} must answer 200, \
             got {} with {:?}",
            accepted.status, accepted.body
        );
    }
}

/// The counter-case for rule 1: a country its documentation does not name
/// requires no state or region, so the sentence is never reached for it.
#[tokio::test]
async fn a_country_that_requires_no_state_or_region_is_not_refused() {
    let router = emulator();

    for country_code in REQUIRES_NO_STATE_OR_REGION {
        let answer = call(
            &router,
            "/putContactInformation",
            json!({ "ContactInformation": contact_in(country_code, None) }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {country_code:?}: a country outside the documented seven needs no state, \
             so the put must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// Rule 2, over Regions from five geographies and both opt operations: the same
/// payload every time — AWS's sentence, `fieldValidationFailed`, and a
/// `fieldList` naming `RegionName`.
#[tokio::test]
async fn a_region_that_is_not_an_opt_target_is_refused_naming_the_region_member() {
    let router = emulator();

    for region in DEFAULT_ENABLED {
        for (operation, uri) in OPT_OPERATIONS {
            let case = format!("{operation} on {region}");
            let answer = call(&router, uri, json!({ "RegionName": region })).await;
            assert_member_refusal(
                &case,
                &answer,
                "RegionName",
                &not_an_opt_target(region),
            );
        }
    }
}

/// The counter-cases for rule 2: a Region that IS an opt target is not refused
/// at all, and a value that is no Region code earns the OTHER refusal of the
/// same member — the same reason and the same named member, a different
/// sentence.
#[tokio::test]
async fn an_opt_target_and_a_value_that_is_no_region_code_do_not_get_that_sentence() {
    let router = emulator();

    for region in OPT_IN {
        let answer = call(&router, "/disableRegion", json!({ "RegionName": region })).await;
        assert_eq!(
            answer.status, 200,
            "case {region:?}: an opt-in Region is an opt target, so the call must answer 200, \
             got {} with {:?}",
            answer.status, answer.body
        );
    }

    for value in NOT_REGION_CODES {
        for (operation, uri) in OPT_OPERATIONS {
            let case = format!("{operation} on {value}");
            let answer = call(&router, uri, json!({ "RegionName": value })).await;
            assert_eq!(
                answer.status, VALIDATION_STATUS,
                "case {case:?}: a value that is no Region code must answer \
                 {VALIDATION_STATUS}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.member("reason"),
                FIELD_VALIDATION_FAILED,
                "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {:?}",
                answer.body
            );
            let named: Vec<String> = answer
                .field_list()
                .into_iter()
                .map(|(name, _)| name)
                .collect();
            assert_eq!(
                named,
                vec!["RegionName".to_string()],
                "case {case:?}: fieldList must name RegionName, got {:?}",
                answer.body
            );
            assert_ne!(
                answer.message(),
                not_an_opt_target(value),
                "case {case:?}: a value that is no Region code must not be reported as a \
                 Region that cannot be opted into, got {:?}",
                answer.body
            );
        }
    }
}
