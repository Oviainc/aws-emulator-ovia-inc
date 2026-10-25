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
//! rather than merging into it; a refused put stores nothing; and an account
//! this emulator holds no record of is an `AccessDeniedException` rather than an
//! empty success.
//!
//! Before any put there is still a primary contact to report: every account has
//! primary contact information, captured when the account is signed up for, and
//! the model makes `ContactInformation`'s `FullName`, `AddressLine1`, `City`,
//! `PostalCode`, `CountryCode` and `PhoneNumber` `@required` — so an account
//! record this emulator holds reports a complete, modelled-valid structure from
//! the moment it exists, whatever call brought it into existence.
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

/// The members `ContactInformation` marks `@required`, each with its modelled
/// `@length` bounds, from
/// `smithy_query(query="shape", shape="ContactInformation")`.
const REQUIRED_MEMBER_BOUNDS: [(&str, usize, usize); 6] = [
    ("FullName", 1, FULL_NAME_MAX),
    ("AddressLine1", 1, ADDRESS_LINE_MAX),
    ("City", 1, CITY_MAX),
    ("PostalCode", 1, POSTAL_CODE_MAX),
    ("CountryCode", COUNTRY_CODE_LEN, COUNTRY_CODE_LEN),
    ("PhoneNumber", 1, PHONE_NUMBER_MAX),
];

/// The members `ContactInformation` leaves optional, each with its modelled
/// `@length` bounds: absent is admitted, a value has to sit inside the bound.
const OPTIONAL_MEMBER_BOUNDS: [(&str, usize, usize); 6] = [
    ("AddressLine2", 1, ADDRESS_LINE_MAX),
    ("AddressLine3", 1, ADDRESS_LINE_MAX),
    ("StateOrRegion", 1, STATE_OR_REGION_MAX),
    ("DistrictOrCounty", 1, DISTRICT_OR_COUNTY_MAX),
    ("CompanyName", 1, COMPANY_NAME_MAX),
    ("WebsiteUrl", 1, WEBSITE_URL_MAX),
];

/// The countries that make `StateOrRegion` mandatory, from that member's own
/// documentation — "This field is required in the following countries: US, CA,
/// GB, DE, JP, IN, and BR"
/// (`smithy_query(query="doc", shape="ContactInformation", member="StateOrRegion")`).
const COUNTRIES_REQUIRING_STATE_OR_REGION: [&str; 7] =
    ["US", "CA", "GB", "DE", "JP", "IN", "BR"];

/// Whether `value` is inside `ContactInformationPhoneNumber`'s modelled
/// `^[+][\s0-9()-]+$`: a leading `+` and then at least one character drawn from
/// whitespace, the digits, the parentheses and the hyphen.
fn matches_phone_number_pattern(value: &str) -> bool {
    let mut characters = value.chars();
    if characters.next() != Some('+') {
        return false;
    }
    let rest: Vec<char> = characters.collect();
    !rest.is_empty()
        && rest.iter().all(|character| {
            character.is_ascii_digit()
                || character.is_ascii_whitespace()
                || matches!(character, '(' | ')' | '-')
        })
}

/// Assert `answer` is a 200 reporting a primary contact that satisfies every
/// constraint the model puts on `ContactInformation`: each required member
/// present and inside its length bound, each optional member absent or inside
/// its own, `CountryCode` a two-letter country code, `PhoneNumber` inside its
/// pattern, and a `StateOrRegion` wherever the country code demands one.
fn assert_modelled_contact_information(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: GetContactInformation must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    let contact = answer.contact_information();
    for (member, min, max) in REQUIRED_MEMBER_BOUNDS {
        let value = contact[member].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: {member} is required, got {:?}", contact[member])
        });
        let length = value.chars().count();
        assert!(
            (min..=max).contains(&length),
            "case {case:?}: {member} must be {min}..={max} characters, got {length} \
             in {value:?}"
        );
    }
    for (member, min, max) in OPTIONAL_MEMBER_BOUNDS {
        if contact[member].is_null() {
            continue;
        }
        let value = contact[member].as_str().unwrap_or_else(|| {
            panic!(
                "case {case:?}: {member} must be a string when present, got {:?}",
                contact[member]
            )
        });
        let length = value.chars().count();
        assert!(
            (min..=max).contains(&length),
            "case {case:?}: {member} must be {min}..={max} characters, got {length} \
             in {value:?}"
        );
    }
    let country_code = contact["CountryCode"].as_str().unwrap_or_default();
    assert!(
        country_code.chars().count() == COUNTRY_CODE_LEN
            && country_code.chars().all(|c| c.is_ascii_uppercase()),
        "case {case:?}: CountryCode must be an ISO-3166 two-letter code, got \
         {country_code:?}"
    );
    let phone_number = contact["PhoneNumber"].as_str().unwrap_or_default();
    assert!(
        matches_phone_number_pattern(phone_number),
        "case {case:?}: PhoneNumber must match ^[+][\\s0-9()-]+$, got {phone_number:?}"
    );
    if COUNTRIES_REQUIRING_STATE_OR_REGION.contains(&country_code) {
        assert!(
            contact["StateOrRegion"].is_string(),
            "case {case:?}: StateOrRegion is required in {country_code}, got {:?}",
            contact["StateOrRegion"]
        );
    }
    assert!(
        PHONE_NUMBER_VERIFICATION_STATUSES.contains(&answer.verification_status()),
        "case {case:?}: expected a PhoneNumberVerificationStatus among \
         {PHONE_NUMBER_VERIFICATION_STATUSES:?}, got {:?}",
        answer.verification_status()
    );
}

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
/// `AccountId` lands on the caller's own record, another 12-digit id is a record
/// of its own, and naming the caller's own id is refused rather than resolved.
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

    // Each account reports its own contact, reached the one way that account can
    // be named.
    for (case, request, expected) in [
        ("the caller's own account, unnamed", json!({}), &implicit),
        (
            "another account in the organization",
            json!({"AccountId": other}),
            &named,
        ),
    ] {
        let answer = get_contact_information(&router, request).await;
        assert_contact_information_matches(case, expected, &answer);
    }

    // Naming the caller's own id is refused on both the read and the write, and
    // the refused write left the record it could not target untouched.
    let rewritten = contact_for("Rewritten");
    for (case, uri, request) in [
        (
            "GetContactInformation naming the caller's own id",
            "/getContactInformation",
            json!({"AccountId": own}),
        ),
        (
            "PutContactInformation naming the caller's own id",
            "/putContactInformation",
            json!({"AccountId": own, "ContactInformation": rewritten}),
        ),
    ] {
        let answer = call(&router, uri, request).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (403, "AccessDeniedException"),
            "case {case:?}: expected the 403 refusal, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
    let answer = get_contact_information(&router, json!({})).await;
    assert_contact_information_matches(
        "the caller's own account after the refused rewrite",
        &implicit,
        &answer,
    );
    // And the other account's record is untouched too.
    let answer = get_contact_information(&router, json!({"AccountId": other})).await;
    assert_contact_information_matches("the untouched other account", &named, &answer);
}

/// An account this emulator holds no record of at all is the
/// `AccessDeniedException` a read of an account outside the caller's
/// organization earns — never an empty 200, and never a record brought into
/// existence by the read.
#[tokio::test]
async fn a_read_of_an_account_with_no_record_is_refused() {
    let router = emulator();

    // (case, request) — each names an account this emulator was never told about.
    let cases: Vec<(&str, Value)> = vec![
        ("an account this emulator knows nothing about", json!({"AccountId": "999988887777"})),
        ("another unknown account", json!({"AccountId": "111122223333"})),
    ];

    for (case, request) in cases {
        // Twice: the first read must not create the record the second would find.
        for attempt in [1, 2] {
            let answer = get_contact_information(&router, request.clone()).await;
            assert_eq!(
                answer.status, 403,
                "case {case:?} (attempt {attempt}): expected HTTP 403, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "AccessDeniedException",
                "case {case:?} (attempt {attempt}): expected AccessDeniedException, got {:?}",
                answer.body
            );
        }
    }
}

/// Every account record this emulator holds reports a complete primary contact
/// on the FIRST read, before any `PutContactInformation` — whatever call brought
/// the record into existence, whatever 12-digit id keys it and however the
/// request names the account — and the read reports the same structure however
/// often it is repeated.
#[tokio::test]
async fn an_account_reports_a_primary_contact_before_any_put() {
    // (case, the call that brings the record into existence — none for the
    // caller's own account, which the store comes up holding — the read)
    type Row = (&'static str, Option<(&'static str, Value)>, Value);
    let cases: Vec<Row> = vec![
        ("the caller's own account, unnamed", None, json!({})),
        (
            "an account created by PutAccountName",
            Some((
                "/putAccountName",
                json!({"AccountId": "210987654321", "AccountName": "Member"}),
            )),
            json!({"AccountId": "210987654321"}),
        ),
        (
            "an account created by PutAlternateContact",
            Some((
                "/putAlternateContact",
                json!({
                    "AccountId": "300000000003",
                    "AlternateContactType": "BILLING",
                    "Name": "Billing Team",
                    "Title": "Accounts Payable",
                    "EmailAddress": "billing@example.com",
                    "PhoneNumber": "+1 206-555-0111",
                }),
            )),
            json!({"AccountId": "300000000003"}),
        ),
        (
            "an account created by EnableRegion",
            Some((
                "/enableRegion",
                // An opt-in Region, so the call has something to enable.
                json!({"AccountId": "400000000004", "RegionName": "ap-east-1"}),
            )),
            json!({"AccountId": "400000000004"}),
        ),
        (
            "an account id of all nines",
            Some((
                "/putAccountName",
                json!({"AccountId": "999999999999", "AccountName": "Nines"}),
            )),
            json!({"AccountId": "999999999999"}),
        ),
        (
            "an account id that is almost all zeros",
            Some((
                "/putAccountName",
                json!({"AccountId": "000000000001", "AccountName": "Zeros"}),
            )),
            json!({"AccountId": "000000000001"}),
        ),
    ];

    for (case, creation, read) in cases {
        // A fresh emulator per row, so nothing an earlier row wrote can be what
        // answers this one.
        let router = emulator();
        if let Some((uri, request)) = creation {
            let created = call(&router, uri, request).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the call that creates the record must answer 200, got {} \
                 with {:?}",
                created.status, created.body
            );
        }

        let answer = get_contact_information(&router, read.clone()).await;
        assert_modelled_contact_information(case, &answer);
        assert_eq!(
            answer.verification_status(),
            UNVERIFIED,
            "case {case:?}: no passcode flow has run on that phone number, so it must be \
             {UNVERIFIED}, got {:?}",
            answer.verification_status()
        );

        // A read changes nothing a later read sees.
        let again = get_contact_information(&router, read).await;
        assert_eq!(
            again.body, answer.body,
            "case {case:?}: a second read must report what the first did, got {:?} then {:?}",
            answer.body, again.body
        );
    }
}

/// The first `PutContactInformation` REPLACES the primary contact the record was
/// created with rather than merging into it, and it lands on one record only:
/// another account still reports the contact it was created with.
#[tokio::test]
async fn the_first_put_replaces_the_contact_the_record_was_created_with() {
    let router = emulator();
    let other = "210987654321";

    // This record comes into existence here, with a primary contact of its own,
    // and never gets a put.
    let created = call(
        &router,
        "/putAccountName",
        json!({"AccountId": other, "AccountName": "Member"}),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"the member account\": PutAccountName must answer 200, got {} with {:?}",
        created.status, created.body
    );
    let other_before = get_contact_information(&router, json!({"AccountId": other})).await;
    assert_modelled_contact_information("the member account before any put", &other_before);

    let own_before = get_contact_information(&router, json!({})).await;
    assert_modelled_contact_information("the caller's own account before any put", &own_before);

    // (case, the ContactInformation to put) — the first put over the created-with
    // contact, then one that drops every optional member the first carried.
    let cases: Vec<(&str, Value)> = vec![
        (
            "the first put over the created-with contact",
            json!({
                "FullName": "Grace Hopper",
                "AddressLine1": "1 Example Way",
                "AddressLine2": "Building 92",
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
            "a later put carrying only the required members",
            json!({
                "FullName": "Alan Turing",
                "AddressLine1": "2 Example Park",
                "City": "Milton Keynes",
                "PostalCode": "MK3 6EB",
                "CountryCode": "ZA",
                "PhoneNumber": "+27 (21) 555-0100",
            }),
        ),
    ];

    for (case, contact) in cases {
        let put =
            put_contact_information(&router, json!({"ContactInformation": contact.clone()})).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: PutContactInformation must answer 200, got {} with {:?}",
            put.status, put.body
        );

        let answer = get_contact_information(&router, json!({})).await;
        assert_contact_information_matches(case, &contact, &answer);
        assert_ne!(
            answer.contact_information(),
            own_before.contact_information(),
            "case {case:?}: the put must replace the contact the record was created with, \
             got {:?}",
            answer.body
        );
    }

    // One record: the member account still reports what it was created with.
    let other_after = get_contact_information(&router, json!({"AccountId": other})).await;
    assert_eq!(
        other_after.body, other_before.body,
        "a put on one account must leave another account's primary contact untouched, \
         got {:?} after {:?}",
        other_after.body, other_before.body
    );
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
