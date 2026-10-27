// SPDX-License-Identifier: CC0-1.0

//! The rules `PutContactInformation` applies to the primary contact ADDRESS
//! itself, driven end to end over HTTP.
//!
//! Two rules, both stated by the model in prose rather than in a constraint, so
//! neither is refused at the modelled boundary:
//!
//! 1. `CountryCode` holds "the ISO-3166 two-letter country code", so a
//!    two-letter string the standard assigns to no country is refused 400
//!    `ValidationException` with reason `fieldValidationFailed` naming that
//!    member, and every assigned code is accepted;
//! 2. `StateOrRegion` "is required in the following countries: US, CA, GB, DE,
//!    JP, IN, and BR", so an address in one of those countries that carries no
//!    state or region is refused the same way naming `StateOrRegion`, while an
//!    address in any other country is complete without one.
//!
//! Every case goes through the router, so it exercises routing,
//! deserialization, the handler, the persisted store and serialization: the same
//! path a client takes. And every refusal is checked to have written NOTHING:
//! the contact stored before it is still the one `GetContactInformation` reports.
//!
//! The tables differ along what the rules generalise over — the country (each of
//! the seven that require a state, and countries on four other continents that
//! do not), the form of a value that is no country code (the standard's
//! user-assigned ranges, a code in the wrong case, and two letters that are not
//! letters), and whether `StateOrRegion` carries a two-letter code or a full
//! name.

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

/// The `ValidationExceptionReason` a refused input member reports, from
/// `smithy_query(query="shape", shape="ValidationExceptionReason")`.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The countries whose addresses must carry a state or region, from
/// `StateOrRegion`'s own documentation — "This field is required in the
/// following countries: US, CA, GB, DE, JP, IN, and BR"
/// (`smithy_query(query="doc", shape="ContactInformation",
/// member="StateOrRegion")`). Each row carries a state of that country, mixing
/// the two-character code and the full name the same documentation admits.
/// Spelled out here rather than imported, so a wrong set in the implementation
/// cannot make this suite pass.
const REQUIRES_STATE_OR_REGION: [(&str, &str); 7] = [
    ("US", "WA"),
    ("CA", "ON"),
    ("GB", "England"),
    ("DE", "Bayern"),
    ("JP", "Tokyo"),
    ("IN", "Karnataka"),
    ("BR", "São Paulo"),
];

/// Country codes the ISO 3166-1 alpha-2 list assigns that are NOT among the
/// seven above, so an address in one of them is complete with no state or
/// region. SOURCE: the ISO 3166-1 alpha-2 code list as published (the officially
/// assigned codes); six rows from six geographies, none of them named by the
/// issue this suite was written for.
const ASSIGNED_ELSEWHERE: [&str; 6] = ["AQ", "AU", "FR", "MX", "SG", "ZA"];

/// Two-letter strings that are NOT assigned country codes. The first six are
/// the ranges ISO 3166-1 leaves for USER assignment — `AA`, `QM`–`QZ`,
/// `XA`–`XZ` and `ZZ` — so the standard assigns them to no country; the rest are
/// forms the modelled `@length(min: 2, max: 2)` admits that no published code
/// takes: an assigned code in the wrong case (the codes are published in upper
/// case), and pairs that are not two letters at all.
const NOT_ASSIGNED: [&str; 11] = [
    "AA", "QM", "QZ", "XA", "XZ", "ZZ", "us", "Fr", "12", "A1", "--",
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

    /// One member of this answer's body, as a string (empty when absent).
    fn member(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    /// The `ContactInformation` structure `GetContactInformation` returns.
    fn contact_information(&self) -> &Value {
        &self.body["ContactInformation"]
    }

    /// The members this refusal's `fieldList` names, in the order it lists them.
    fn invalid_members(&self) -> Vec<&str> {
        self.body["fieldList"]
            .as_array()
            .map(|fields| {
                fields
                    .iter()
                    .map(|field| field["name"].as_str().unwrap_or_default())
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

/// A `ContactInformation` for `country_code`, carrying `state_or_region` when
/// the row names one and leaving that member out altogether otherwise. Every
/// other member is inside its modelled constraint, so a row is about the two
/// members under test and nothing else.
fn contact_in(country_code: &str, state_or_region: Option<&str>) -> Value {
    let mut contact = json!({
        "FullName": "Ada Lovelace",
        "AddressLine1": "1 Analytical Way",
        "City": "Example City",
        "PostalCode": "00001",
        "CountryCode": country_code,
        "PhoneNumber": "+1 206-555-0100",
    });
    if let Some(state) = state_or_region {
        contact["StateOrRegion"] = json!(state);
    }
    contact
}

/// Store `contact` as the caller's own primary contact.
async fn put(router: &MultiServiceRouter, contact: Value) -> Answer {
    call(
        router,
        "/putContactInformation",
        json!({ "ContactInformation": contact }),
    )
    .await
}

/// The caller's own primary contact, as `GetContactInformation` reports it.
async fn stored(router: &MultiServiceRouter) -> Value {
    let answer = call(router, "/getContactInformation", json!({})).await;
    assert_eq!(
        answer.status, 200,
        "GetContactInformation must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.contact_information().clone()
}

/// Assert `answer` is the modelled 400 refusal naming `member` as the invalid
/// one, and that its message says which member was invalid.
fn assert_refused_naming(case: &str, answer: &Answer, member: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: PutContactInformation must answer 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.member("reason"),
        FIELD_VALIDATION_FAILED,
        "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.invalid_members(),
        vec![member],
        "case {case:?}: fieldList must name {member}, got {:?}",
        answer.body
    );
    assert!(
        answer.member("message").contains(member),
        "case {case:?}: the message must say which member was invalid, got {:?}",
        answer.body
    );
}

/// A `CountryCode` the ISO 3166-1 alpha-2 list assigns to no country is refused
/// naming that member, and the refused put writes nothing.
#[tokio::test]
async fn a_country_code_the_standard_assigns_to_no_country_is_refused() {
    let router = emulator();

    // The contact every row below must leave untouched.
    let accepted = contact_in("FR", None);
    let put_accepted = put(&router, accepted.clone()).await;
    assert_eq!(
        put_accepted.status, 200,
        "case \"the accepted put\": expected 200, got {} with {:?}",
        put_accepted.status, put_accepted.body
    );
    let before = stored(&router).await;

    for country_code in NOT_ASSIGNED {
        // With and without a state or region: the country code decides on its
        // own, so neither form of the rest of the address rescues it.
        for state_or_region in [None, Some("WA")] {
            let case = format!("{country_code:?} with StateOrRegion {state_or_region:?}");
            let answer = put(&router, contact_in(country_code, state_or_region)).await;
            assert_refused_naming(&case, &answer, "CountryCode");

            assert_eq!(
                stored(&router).await,
                before,
                "case {case:?}: a refused put must leave the stored contact alone, \
                 got {:?} after {before:?}",
                stored(&router).await
            );
        }
    }
}

/// Every assigned country code outside the seven is accepted with no
/// `StateOrRegion`, and the address is read back as stored — so the refusals
/// above are the published table and not a blanket rejection.
#[tokio::test]
async fn an_assigned_country_code_is_accepted_without_a_state_or_region() {
    let router = emulator();

    for country_code in ASSIGNED_ELSEWHERE {
        let contact = contact_in(country_code, None);
        let answer = put(&router, contact.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {country_code:?}: PutContactInformation must answer 200, got {} with {:?}",
            answer.status, answer.body
        );

        let reported = stored(&router).await;
        assert_eq!(
            reported["CountryCode"], contact["CountryCode"],
            "case {country_code:?}: CountryCode must be read back as stored, got {reported:?}"
        );
        assert!(
            reported["StateOrRegion"].is_null(),
            "case {country_code:?}: a country that requires no state or region must report \
             that member absent, got {:?}",
            reported["StateOrRegion"]
        );
    }
}

/// In each of the seven countries its documentation names, an address with no
/// `StateOrRegion` is refused naming that member and writes nothing; the same
/// address carrying one is accepted and reads back as stored.
#[tokio::test]
async fn a_state_or_region_is_required_in_the_countries_its_documentation_names() {
    let router = emulator();

    // The contact every refused row below must leave untouched.
    let put_accepted = put(&router, contact_in("AU", None)).await;
    assert_eq!(
        put_accepted.status, 200,
        "case \"the accepted put\": expected 200, got {} with {:?}",
        put_accepted.status, put_accepted.body
    );
    let mut before = stored(&router).await;

    for (country_code, state_or_region) in REQUIRES_STATE_OR_REGION {
        let absent = put(&router, contact_in(country_code, None)).await;
        assert_refused_naming(
            &format!("{country_code:?} with no StateOrRegion"),
            &absent,
            "StateOrRegion",
        );
        assert!(
            absent.member("message").contains(country_code),
            "case {country_code:?}: the message must name the country that requires it, got {:?}",
            absent.body
        );
        let after_refusal = stored(&router).await;
        assert_eq!(
            after_refusal, before,
            "case {country_code:?}: a refused put must leave the stored contact alone, \
             got {after_refusal:?} after {before:?}"
        );

        let contact = contact_in(country_code, Some(state_or_region));
        let accepted = put(&router, contact.clone()).await;
        assert_eq!(
            accepted.status, 200,
            "case {country_code:?} with StateOrRegion {state_or_region:?}: \
             PutContactInformation must answer 200, got {} with {:?}",
            accepted.status, accepted.body
        );
        before = stored(&router).await;
        assert_eq!(
            before["StateOrRegion"], contact["StateOrRegion"],
            "case {country_code:?}: StateOrRegion must be read back as stored, got {before:?}"
        );
        assert_eq!(
            before["CountryCode"], contact["CountryCode"],
            "case {country_code:?}: CountryCode must be read back as stored, got {before:?}"
        );
    }
}

/// A `StateOrRegion` is never REFUSED: a country outside the seven may carry one
/// as well, so the rule is about absence in those seven and nothing else.
#[tokio::test]
async fn a_state_or_region_is_admitted_in_every_assigned_country() {
    let router = emulator();

    for country_code in ASSIGNED_ELSEWHERE {
        let contact = contact_in(country_code, Some("Example Province"));
        let answer = put(&router, contact.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {country_code:?}: a state or region is admitted everywhere, so the put must \
             answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        let reported = stored(&router).await;
        assert_eq!(
            reported["StateOrRegion"], contact["StateOrRegion"],
            "case {country_code:?}: StateOrRegion must be read back as stored, got {reported:?}"
        );
    }
}
