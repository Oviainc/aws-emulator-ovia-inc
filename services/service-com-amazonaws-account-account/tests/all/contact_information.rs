// SPDX-License-Identifier: CC0-1.0

//! The primary contact information of an account, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted store and serialization: the same
//! path a client takes.
//!
//! The rule under test: `PutContactInformation` stores the submitted
//! `ContactInformation` structure as the ONE primary contact of the resolved
//! account (the `AccountId` the request names, else the caller's own), and
//! `GetContactInformation` reports that structure back member for member — every
//! member the put carried present with its stored value, every member it left out
//! absent — together with the phone number's
//! `PhoneNumberVerificationStatus`; a second put replaces the whole structure
//! rather than merging into it; a refused put stores nothing; and an account with
//! no primary contact stored is a `ResourceNotFoundException` rather than an empty
//! success.
//!
//! The table below spans that class: rows differ along which optional members are
//! present (none, some, all), the lengths of the values (each published bound and
//! one past it), their form (ASCII, non-ASCII, parenthesised phone numbers) and
//! which account the request resolves to.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::state::default_account_id;
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The values `PhoneNumberVerificationStatus` admits, from
/// `smithy_query(query="operation", operation="GetContactInformation")` —
/// `NOT_SUPPORTED | PENDING | UNVERIFIED | VERIFIED`. Spelled out here, not
/// imported, so a wrong set in the implementation cannot make this test pass.
const PHONE_NUMBER_VERIFICATION_STATUSES: [&str; 4] =
    ["NOT_SUPPORTED", "PENDING", "UNVERIFIED", "VERIFIED"];

/// The status a phone number this emulator was merely TOLD about carries: it has
/// not been through the one-time-passcode flow, so it is not verified.
const UNVERIFIED: &str = "UNVERIFIED";

/// The modelled `@length` bounds of `ContactInformation`'s members, from
/// `smithy_query(query="shape", shape="ContactInformation")`:
/// `FullName` 1–50, `AddressLine1..3` 1–60, `City` 1–50, `StateOrRegion` 1–50,
/// `DistrictOrCounty` 1–50, `PostalCode` 1–20, `CountryCode` 2–2,
/// `PhoneNumber` 1–20, `CompanyName` 1–50, `WebsiteUrl` 1–256.
const FULL_NAME_MAX: usize = 50;
const ADDRESS_LINE_MAX: usize = 60;
const CITY_MAX: usize = 50;
const STATE_OR_REGION_MAX: usize = 50;
const DISTRICT_OR_COUNTY_MAX: usize = 50;
const POSTAL_CODE_MAX: usize = 20;
const COUNTRY_CODE_LEN: usize = 2;
const PHONE_NUMBER_MAX: usize = 20;
const COMPANY_NAME_MAX: usize = 50;
const WEBSITE_URL_MAX: usize = 256;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

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

    /// The `ContactInformation` structure `GetContactInformation` returns.
    fn contact_information(&self) -> &Value {
        &self.body["ContactInformation"]
    }

    /// The reported `VerificationStatus` (empty when absent).
    fn verification_status(&self) -> &str {
        self.body["VerificationStatus"].as_str().unwrap_or_default()
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

async fn put_contact_information(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/putContactInformation", request).await
}

async fn get_contact_information(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "/getContactInformation", request).await
}

/// A `ContactInformation` carrying only the members the model marks required.
fn required_only() -> Value {
    json!({
        "FullName": "Ada Lovelace",
        "AddressLine1": "410 Terry Ave N",
        "City": "Seattle",
        "PostalCode": "98109",
        "CountryCode": "US",
        "PhoneNumber": "+1 206-555-0100",
    })
}

/// A string of `len` repetitions of `fill`, for a row that sits on a length bound.
fn sized(fill: char, len: usize) -> String {
    std::iter::repeat_n(fill, len).collect()
}

/// A phone number exactly `len` characters long and inside
/// `ContactInformationPhoneNumber`'s `^[+][\s0-9()-]+$`.
fn sized_phone_number(len: usize) -> String {
    format!("+{}", sized('1', len - 1))
}

/// Every member of a stored `ContactInformation`, so a row that leaves one out is
/// checked for its ABSENCE as well as for the presence of the rest.
const CONTACT_INFORMATION_MEMBERS: [&str; 12] = [
    "FullName",
    "AddressLine1",
    "AddressLine2",
    "AddressLine3",
    "City",
    "StateOrRegion",
    "DistrictOrCounty",
    "PostalCode",
    "CountryCode",
    "PhoneNumber",
    "CompanyName",
    "WebsiteUrl",
];

/// Assert `answer` reports exactly the contact `stored` asked to store: every
/// member of the request read back, and every member it omitted absent.
fn assert_contact_information_matches(case: &str, stored: &Value, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetContactInformation must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    for member in CONTACT_INFORMATION_MEMBERS {
        let expected = &stored[member];
        let actual = &answer.contact_information()[member];
        if expected.is_null() {
            assert!(
                actual.is_null(),
                "case {case:?}: {member} was not stored, so it must be absent, got {actual:?}"
            );
        } else {
            assert_eq!(
                actual, expected,
                "case {case:?}: {member} must be read back as stored, expected \
                 {expected:?}, got {actual:?}"
            );
        }
    }
    assert!(
        PHONE_NUMBER_VERIFICATION_STATUSES.contains(&answer.verification_status()),
        "case {case:?}: expected a PhoneNumberVerificationStatus among \
         {PHONE_NUMBER_VERIFICATION_STATUSES:?}, got {:?}",
        answer.verification_status()
    );
    assert_eq!(
        answer.verification_status(),
        UNVERIFIED,
        "case {case:?}: a phone number no verification flow has run on must be \
         {UNVERIFIED}, got {:?}",
        answer.verification_status()
    );
}

/// Every form of `ContactInformation` the model admits round-trips: it is read
/// back member for member, and each put REPLACES the one stored structure, so the
/// optional members of the row before it are gone.
#[tokio::test]
async fn a_stored_contact_information_round_trips_member_for_member() {
    // (case, the ContactInformation to store)
    let cases: Vec<(&str, Value)> = vec![
        ("only the required members", required_only()),
        (
            "every optional member present",
            json!({
                "FullName": "Grace Hopper",
                "AddressLine1": "1 Microsoft Way",
                "AddressLine2": "Building 92",
                "AddressLine3": "Mail Stop 7",
                "City": "Redmond",
                "StateOrRegion": "WA",
                "DistrictOrCounty": "King",
                "PostalCode": "98052-8300",
                "CountryCode": "US",
                "PhoneNumber": "+1 (425) 555-0101",
                "CompanyName": "Example Industries",
                "WebsiteUrl": "https://example.com/contact",
            }),
        ),
        (
            "a subset of the optional members",
            json!({
                "FullName": "Katherine Johnson",
                "AddressLine1": "100 NASA Road",
                "AddressLine2": "Suite 3",
                "City": "Hampton",
                "PostalCode": "23666",
                "CountryCode": "US",
                "PhoneNumber": "+1 757-555-0199",
                "CompanyName": "Example Aerospace",
            }),
        ),
        (
            "the full state name rather than its two-letter code",
            json!({
                "FullName": "Alan Turing",
                "AddressLine1": "2 Bletchley Park",
                "City": "Milton Keynes",
                "StateOrRegion": "New Jersey",
                "PostalCode": "MK3 6EB",
                "CountryCode": "GB",
                "PhoneNumber": "+44 (20) 7123-4567",
            }),
        ),
        (
            "values outside ASCII",
            json!({
                "FullName": "José Álvarez Muñoz",
                "AddressLine1": "Avenida Paulista 1578",
                "City": "São Paulo",
                "StateOrRegion": "São Paulo",
                "PostalCode": "01310-200",
                "CountryCode": "BR",
                "PhoneNumber": "+55 (11) 5555-0100",
                "CompanyName": "Indústrias Exemplo",
            }),
        ),
        (
            "every member at its minimum length",
            json!({
                "FullName": "A",
                "AddressLine1": "B",
                "AddressLine2": "C",
                "AddressLine3": "D",
                "City": "E",
                "StateOrRegion": "F",
                "DistrictOrCounty": "G",
                "PostalCode": "H",
                "CountryCode": "ZA",
                "PhoneNumber": sized_phone_number(2),
                "CompanyName": "I",
                "WebsiteUrl": "J",
            }),
        ),
        (
            "every member at its maximum length",
            json!({
                "FullName": sized('n', FULL_NAME_MAX),
                "AddressLine1": sized('a', ADDRESS_LINE_MAX),
                "AddressLine2": sized('b', ADDRESS_LINE_MAX),
                "AddressLine3": sized('c', ADDRESS_LINE_MAX),
                "City": sized('t', CITY_MAX),
                "StateOrRegion": sized('s', STATE_OR_REGION_MAX),
                "DistrictOrCounty": sized('d', DISTRICT_OR_COUNTY_MAX),
                "PostalCode": sized('9', POSTAL_CODE_MAX),
                "CountryCode": sized('Z', COUNTRY_CODE_LEN),
                "PhoneNumber": sized_phone_number(PHONE_NUMBER_MAX),
                "CompanyName": sized('c', COMPANY_NAME_MAX),
                "WebsiteUrl": sized('u', WEBSITE_URL_MAX),
            }),
        ),
    ];

    let router = emulator();
    for (case, contact) in cases {
        let put = put_contact_information(&router, json!({"ContactInformation": contact})).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: PutContactInformation must answer 200, got {} with {:?}",
            put.status, put.body
        );

        let answer = get_contact_information(&router, json!({})).await;
        assert_contact_information_matches(case, &contact, &answer);
    }
}

/// The stored contact is keyed by the RESOLVED account: a request that names no
/// `AccountId` and one that names the caller's own id land on the same record,
/// and another 12-digit id is a record of its own.
#[tokio::test]
async fn the_stored_contact_is_keyed_by_the_resolved_account() {
    let own = default_account_id().to_string();
    let other = "210987654321";

    let router = emulator();

    // A contact distinguishable per target account.
    let contact_for = |tag: &str| {
        let mut contact = required_only();
        contact["FullName"] = json!(format!("{tag} Contact"));
        contact["AddressLine1"] = json!(format!("{tag} Street 1"));
        contact
    };

    // The caller's own account, reached without naming it.
    let implicit = contact_for("Implicit");
    let put = put_contact_information(&router, json!({"ContactInformation": implicit})).await;
    assert_eq!(
        put.status, 200,
        "case \"no AccountId\": PutContactInformation must answer 200, got {} with {:?}",
        put.status, put.body
    );

    // Another account: a record of its own, created by this write.
    let named = contact_for("Named");
    let put = put_contact_information(
        &router,
        json!({"AccountId": other, "ContactInformation": named}),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "case \"another AccountId\": PutContactInformation must answer 200, got {} with {:?}",
        put.status, put.body
    );

    // Each account reports its own contact, by either way of naming it.
    for (case, request, expected) in [
        ("the caller's own account, unnamed", json!({}), &implicit),
        (
            "the caller's own account, named explicitly",
            json!({"AccountId": own}),
            &implicit,
        ),
        (
            "another account in the organization",
            json!({"AccountId": other}),
            &named,
        ),
    ] {
        let answer = get_contact_information(&router, request).await;
        assert_contact_information_matches(case, expected, &answer);
    }

    // Writing through the explicit id lands on the SAME record as the implicit
    // one: one account, one primary contact.
    let rewritten = contact_for("Rewritten");
    let put = put_contact_information(
        &router,
        json!({"AccountId": own, "ContactInformation": rewritten}),
    )
    .await;
    assert_eq!(
        put.status, 200,
        "case \"explicit own AccountId\": PutContactInformation must answer 200, got {} \
         with {:?}",
        put.status, put.body
    );
    let answer = get_contact_information(&router, json!({})).await;
    assert_contact_information_matches("a rewrite through the explicit own id", &rewritten, &answer);
    // And it left the other account's record alone.
    let answer = get_contact_information(&router, json!({"AccountId": other})).await;
    assert_contact_information_matches("the untouched other account", &named, &answer);
}

/// An account with no primary contact stored is a `ResourceNotFoundException`,
/// and an account this emulator holds no record of at all is the
/// `AccessDeniedException` a read of an account outside the caller's organization
/// earns — never an empty 200.
#[tokio::test]
async fn a_contact_that_was_never_stored_is_refused() {
    let router = emulator();

    // (case, request, expected status, expected error code)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "the caller's own account before any put",
            json!({}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the caller's own account named explicitly",
            json!({"AccountId": default_account_id()}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an account this emulator knows nothing about",
            json!({"AccountId": "999988887777"}),
            403,
            "AccessDeniedException",
        ),
    ];

    for (case, request, status, code) in cases {
        let answer = get_contact_information(&router, request).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code}, got {:?}",
            answer.body
        );
    }
}

/// A put the boundary refuses — a missing required member, or a value one past a
/// published length bound — stores nothing: the contact stored before it is still
/// the one reported afterwards.
#[tokio::test]
async fn a_refused_put_leaves_the_stored_contact_unchanged() {
    let router = emulator();

    let stored = required_only();
    let put = put_contact_information(&router, json!({"ContactInformation": stored})).await;
    assert_eq!(
        put.status, 200,
        "case \"the accepted put\": expected 200, got {} with {:?}",
        put.status, put.body
    );

    // Each row is `required_only()` with one member broken: absent, or one
    // character past (or, for the fixed-length country code, either side of) its
    // modelled bound.
    let mut cases: Vec<(&str, Value)> = Vec::new();
    for member in ["FullName", "AddressLine1", "City", "PostalCode", "CountryCode", "PhoneNumber"] {
        let mut contact = required_only();
        contact
            .as_object_mut()
            .expect("the contact body is a JSON object")
            .remove(member);
        cases.push((member, contact));
    }
    for (member, too_long) in [
        ("FullName", sized('n', FULL_NAME_MAX + 1)),
        ("AddressLine1", sized('a', ADDRESS_LINE_MAX + 1)),
        ("City", sized('t', CITY_MAX + 1)),
        ("PostalCode", sized('9', POSTAL_CODE_MAX + 1)),
        ("CountryCode", sized('Z', COUNTRY_CODE_LEN + 1)),
        ("CountryCode", sized('Z', COUNTRY_CODE_LEN - 1)),
        ("PhoneNumber", sized_phone_number(PHONE_NUMBER_MAX + 1)),
        ("FullName", String::new()),
    ] {
        let mut contact = required_only();
        contact[member] = json!(too_long);
        cases.push((member, contact));
    }

    for (member, contact) in cases {
        let put = put_contact_information(&router, json!({"ContactInformation": contact})).await;
        assert_eq!(
            put.status, 400,
            "case {member:?} outside its bound: PutContactInformation must answer 400, \
             got {} with {:?}",
            put.status, put.body
        );
        assert_eq!(
            put.error_code(),
            "ValidationException",
            "case {member:?} outside its bound: expected ValidationException, got {:?}",
            put.body
        );

        let answer = get_contact_information(&router, json!({})).await;
        assert_contact_information_matches(
            &format!("the store after {member:?} was refused"),
            &stored,
            &answer,
        );
    }
}
