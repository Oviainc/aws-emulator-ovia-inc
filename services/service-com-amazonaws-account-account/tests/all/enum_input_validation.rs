// SPDX-License-Identifier: CC0-1.0

//! Enum-valued request members, end to end over HTTP.
//!
//! The rule under test: every input member this service models as a string enum —
//! a member that targets an enum, and every ELEMENT of a list whose member targets
//! one — admits exactly the values the model spells and nothing else, matched
//! case-sensitively and without trimming; a value outside that set is refused with
//! HTTP 400 `ValidationException` whose message names the offending value, the
//! member's dotted position in the request and the admitted value set, and is
//! never served a success, whatever else the request carries. An admitted value,
//! conversely, always reaches the handler.
//!
//! This service models two such members
//! (`smithy_query(query="operation", operation="…")` over its eighteen
//! operations): `AlternateContactType`, which targets the `AlternateContactType`
//! enum (`BILLING | OPERATIONS | SECURITY`) on `GetAlternateContact`,
//! `PutAlternateContact` and `DeleteAlternateContact`, and
//! `RegionOptStatusContains`, which targets `RegionOptStatusList` whose member is
//! the `RegionOptStatus` enum (`ENABLED | ENABLING | DISABLING | DISABLED |
//! ENABLED_BY_DEFAULT`) on `ListRegions`. The table below spans both: its rows
//! differ along the member, the operation carrying it, the enum behind it, the
//! form of the inadmissible value (an unknown token, an admitted value in the
//! wrong case, one padded with spaces, the empty string, a near-miss spelling, an
//! admitted value with a character appended, a value belonging to the OTHER enum
//! this service models, and a token past the length any enum value has) and — for
//! the list-valued member — the number of elements and the position of the
//! offending one.
//!
//! Each value set is spelled out here rather than imported from the crate, so an
//! implementation that admits the wrong set cannot make this suite pass. The
//! message clauses are likewise spelled out: they are what the request itself
//! determines (the value sent, the member path, the modelled value set), not a
//! recorded sentence.

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

/// The values `RegionOptStatus` admits, in the order the model declares them,
/// from `smithy_query(query="shape", shape="RegionOptStatus")`.
const REGION_OPT_STATUSES: [&str; 5] = [
    "ENABLED",
    "ENABLING",
    "DISABLING",
    "DISABLED",
    "ENABLED_BY_DEFAULT",
];

/// The values `AlternateContactType` admits, in the order the model declares
/// them, from `smithy_query(query="shape", shape="AlternateContactType")`.
const ALTERNATE_CONTACT_TYPES: [&str; 3] = ["BILLING", "OPERATIONS", "SECURITY"];

/// The wire error code `ValidationException` renders under, from
/// `smithy_query(query="operation", operation="ListRegions")`
/// (`errorKind: client`, `httpError: 400`).
const VALIDATION_EXCEPTION: &str = "ValidationException";
const VALIDATION_STATUS: u16 = 400;

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

    /// The `Regions` list as `(RegionName, RegionOptStatus)` pairs, in the order
    /// the response carries them.
    fn regions(&self) -> Vec<(String, String)> {
        self.body["Regions"]
            .as_array()
            .unwrap_or_else(|| panic!("a ListRegions success carries Regions, got {:?}", self.body))
            .iter()
            .map(|region| {
                (
                    region["RegionName"].as_str().unwrap_or_default().to_string(),
                    region["RegionOptStatus"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect()
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

/// A `PutAlternateContact` request whose every member but the contact type is
/// valid, so a refusal of it is a refusal of `AlternateContactType` alone.
fn contact_request(contact_type: &str) -> Value {
    json!({
        "AlternateContactType": contact_type,
        "Name": "Ada",
        "Title": "CFO",
        "EmailAddress": "ada@example.com",
        "PhoneNumber": "+1 206-555-0100",
    })
}

/// Values no string enum admits, derived from the enum's OWN value set plus a
/// value of the other enum this service models: the forms a client gets wrong.
///
/// Returns `(case label, the value)` pairs, each `(label, value)` a row of the
/// class "a value outside the modelled value set". A form that an enum happens to
/// admit — a value set with no underscore to hyphenate, say — is dropped rather
/// than asserted on, so every row really is outside the set.
fn inadmissible_values(admitted: &[&str], foreign: &str) -> Vec<(String, String)> {
    let first = admitted[0];
    let last = admitted[admitted.len() - 1];
    let title_cased = {
        let mut lowered = first.to_lowercase();
        lowered.replace_range(0..1, &first[0..1]);
        lowered
    };
    let mut forms = vec![
        ("an unknown token".to_string(), "BOGUS".to_string()),
        (format!("{first:?} lower-cased"), first.to_lowercase()),
        (format!("{first:?} title-cased"), title_cased),
        (format!("{last:?} padded with spaces"), format!(" {last} ")),
        ("the empty string".to_string(), String::new()),
        (
            format!("{last:?} spelled with hyphens"),
            last.replace('_', "-"),
        ),
        (
            format!("{first:?} with a character appended"),
            format!("{first}X"),
        ),
        (
            format!("{foreign:?}, a value of another enum in this service"),
            foreign.to_string(),
        ),
        // Longer than the framework echoes verbatim, so this row checks the
        // constraint clause and not the echo.
        ("a token of 300 characters".to_string(), "X".repeat(300)),
    ];
    forms.retain(|(_, value)| !admitted.contains(&value.as_str()));
    assert!(
        forms.len() >= 7,
        "the table must span the forms of an inadmissible value, got {forms:?}"
    );
    forms
}

/// The dotted position a message states for a top-level member: the member name
/// in lowerCamel.
fn member_path(member: &str) -> String {
    let mut path = member.to_string();
    path.replace_range(0..1, &member[0..1].to_lowercase());
    path
}

/// The dotted position a message states for the 1-based `position`th element of a
/// list-valued member.
fn element_path(member: &str, position: usize) -> String {
    format!("{}.{position}.member", member_path(member))
}

/// The clause a refusal states for a value outside `admitted` — the constraint
/// itself, as the request and the model determine it.
fn enum_clause(admitted: &[&str]) -> String {
    format!(
        "failed to satisfy constraint: Member must satisfy enum value set: [{}]",
        admitted.join(", ")
    )
}

/// The framework's ceiling on the value a refusal echoes verbatim: a longer value
/// reaches the message elided, so no row past this length asserts the echo.
const ECHOED_VALUE_CAP: usize = 256;

/// One refused request: it must answer 400 `ValidationException`, naming the
/// offending value (when it is short enough to be echoed), its dotted position
/// and the admitted value set.
fn assert_refused(case: &str, answer: &Answer, value: &str, path: &str, admitted: &[&str]) {
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
    let clause = enum_clause(admitted);
    assert!(
        answer.message().contains(&clause),
        "case {case:?}: the message must state {clause:?}, got {:?}",
        answer.message()
    );
    let position = format!("at '{path}'");
    assert!(
        answer.message().contains(&position),
        "case {case:?}: the message must state {position:?}, got {:?}",
        answer.message()
    );
    if value.len() <= ECHOED_VALUE_CAP {
        let echo = format!("Value '{value}'");
        assert!(
            answer.message().contains(&echo),
            "case {case:?}: the message must echo {echo:?}, got {:?}",
            answer.message()
        );
    }
}

/// `RegionOptStatusContains` refuses an element the `RegionOptStatus` enum does
/// not admit — at any position, in a list of any length, and whatever else the
/// request carries: never a 200 with an empty `Regions` list, which a client
/// could not tell from "no Region currently has that status".
#[tokio::test]
async fn the_region_status_filter_refuses_a_value_the_enum_does_not_admit() {
    let router = emulator();
    let forms = inadmissible_values(&REGION_OPT_STATUSES, ALTERNATE_CONTACT_TYPES[0]);

    for (index, (label, value)) in forms.iter().enumerate() {
        // The offending element moves through the list and the list grows with it,
        // so neither the position nor the element count can be what is refused.
        let leading = index % 3;
        let trailing = index % 2;
        let mut statuses: Vec<String> = REGION_OPT_STATUSES
            .iter()
            .take(leading)
            .map(|status| (*status).to_string())
            .collect();
        statuses.push(value.clone());
        statuses.extend(
            REGION_OPT_STATUSES
                .iter()
                .rev()
                .take(trailing)
                .map(|status| (*status).to_string()),
        );
        let position = leading + 1;
        let case = format!(
            "ListRegions filtered by {label} at position {position} of {}",
            statuses.len()
        );

        let answer = call(
            &router,
            "/listRegions",
            json!({"RegionOptStatusContains": statuses}),
        )
        .await;
        assert_refused(
            &case,
            &answer,
            value,
            &element_path("RegionOptStatusContains", position),
            &REGION_OPT_STATUSES,
        );
        assert!(
            answer.body["Regions"].is_null(),
            "case {case:?}: a refused filter must list nothing, got {:?}",
            answer.body
        );
    }

    // The refusal does not depend on the rest of the request: a page size, a named
    // account and an account this emulator holds no record of (which an admitted
    // filter is answered 403 for) all still answer the 400.
    for (case, request) in [
        (
            "paged and filtered by an unknown status",
            json!({"MaxResults": 3, "RegionOptStatusContains": ["BOGUS"]}),
        ),
        (
            "a named account, filtered by an unknown status",
            json!({"AccountId": "123456789012", "RegionOptStatusContains": ["BOGUS"]}),
        ),
        (
            "an unknown account, filtered by an unknown status",
            json!({"AccountId": "210987654321", "RegionOptStatusContains": ["BOGUS"]}),
        ),
        (
            "an unissued NextToken and an unknown status",
            json!({"NextToken": "6e6f742061207265616c20746f6b656e", "RegionOptStatusContains": ["BOGUS"]}),
        ),
    ] {
        let answer = call(&router, "/listRegions", request).await;
        assert_refused(
            case,
            &answer,
            "BOGUS",
            &element_path("RegionOptStatusContains", 1),
            &REGION_OPT_STATUSES,
        );
    }
}

/// `AlternateContactType` refuses a value the enum does not admit, on every
/// operation that carries it — the same rule as the Region status filter, for the
/// service's other enum-valued member.
#[tokio::test]
async fn the_alternate_contact_type_refuses_a_value_the_enum_does_not_admit() {
    let router = emulator();
    let forms = inadmissible_values(&ALTERNATE_CONTACT_TYPES, REGION_OPT_STATUSES[0]);

    for (label, value) in &forms {
        for (operation, uri, request) in [
            (
                "GetAlternateContact",
                "/getAlternateContact",
                json!({"AlternateContactType": value}),
            ),
            (
                "DeleteAlternateContact",
                "/deleteAlternateContact",
                json!({"AlternateContactType": value}),
            ),
            (
                "PutAlternateContact",
                "/putAlternateContact",
                contact_request(value),
            ),
        ] {
            let case = format!("{operation} for {label}");
            let answer = call(&router, uri, request).await;
            assert_refused(
                &case,
                &answer,
                value,
                &member_path("AlternateContactType"),
                &ALTERNATE_CONTACT_TYPES,
            );
        }
    }
}

/// The refusals above are the modelled value set and not a blanket rejection:
/// every value each enum admits reaches the handler, and the Region status filter
/// still keeps exactly the Regions carrying the listed statuses.
#[tokio::test]
async fn every_value_the_enum_admits_reaches_the_handler() {
    let router = emulator();

    for status in REGION_OPT_STATUSES {
        let answer = call(
            &router,
            "/listRegions",
            json!({"RegionOptStatusContains": [status]}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {status:?}: an admitted RegionOptStatus must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        for (region, reported) in answer.regions() {
            assert_eq!(
                reported, status,
                "case {status:?}: the filter must keep only that status, got {reported:?} \
                 for {region:?}"
            );
        }
    }

    // Every admitted status at once keeps the whole catalog: the set the filter
    // admits is exactly the complement of the set refused above.
    let filtered = call(
        &router,
        "/listRegions",
        json!({"RegionOptStatusContains": REGION_OPT_STATUSES}),
    )
    .await;
    let unfiltered = call(&router, "/listRegions", json!({})).await;
    assert_eq!(
        filtered.regions(),
        unfiltered.regions(),
        "case \"every admitted status at once\": expected the whole list, got {:?}",
        filtered.body
    );

    // An admitted `AlternateContactType` reaches the handler too: with no contact
    // stored the answer is the modelled 404, never the 400 above.
    for contact_type in ALTERNATE_CONTACT_TYPES {
        let answer = call(
            &router,
            "/getAlternateContact",
            json!({"AlternateContactType": contact_type}),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {contact_type:?}: an admitted contact type must reach the handler \
             (404 with nothing stored), got {} with {:?}",
            answer.status, answer.body
        );
    }

    // And a contact stored under an admitted value reads back under it, so the
    // admitted set is what the store round-trips.
    for contact_type in ALTERNATE_CONTACT_TYPES {
        let put = call(&router, "/putAlternateContact", contact_request(contact_type)).await;
        assert_eq!(
            put.status, 200,
            "case {contact_type:?}: storing a contact under an admitted type must answer \
             200, got {} with {:?}",
            put.status, put.body
        );
        let stored = call(
            &router,
            "/getAlternateContact",
            json!({"AlternateContactType": contact_type}),
        )
        .await;
        assert_eq!(
            stored.body["AlternateContact"]["AlternateContactType"].as_str(),
            Some(contact_type),
            "case {contact_type:?}: the stored contact must report the type it was \
             stored under, got {:?}",
            stored.body
        );
    }
}

/// A refused value changes nothing a later caller sees: the Region list an
/// inadmissible filter was refused for is the list an unfiltered call still
/// reports, and a refused contact write stores no contact.
#[tokio::test]
async fn a_refused_enum_value_changes_nothing() {
    let router = emulator();
    let before = call(&router, "/listRegions", json!({})).await;
    assert_eq!(
        before.status, 200,
        "case \"the whole list\": expected 200, got {} with {:?}",
        before.status, before.body
    );

    for value in ["BOGUS", "enabled", ""] {
        let refused = call(
            &router,
            "/listRegions",
            json!({"RegionOptStatusContains": [value]}),
        )
        .await;
        assert_eq!(
            refused.status, VALIDATION_STATUS,
            "case {value:?}: expected {VALIDATION_STATUS}, got {} with {:?}",
            refused.status, refused.body
        );
    }
    for value in ["MARKETING", "billing", ""] {
        let refused = call(&router, "/putAlternateContact", contact_request(value)).await;
        assert_eq!(
            refused.status, VALIDATION_STATUS,
            "case {value:?}: expected {VALIDATION_STATUS}, got {} with {:?}",
            refused.status, refused.body
        );
    }

    let after = call(&router, "/listRegions", json!({})).await;
    assert_eq!(
        after.regions(),
        before.regions(),
        "case \"after the refusals\": a refused read must leave the Region list untouched"
    );
    for contact_type in ALTERNATE_CONTACT_TYPES {
        let answer = call(
            &router,
            "/getAlternateContact",
            json!({"AlternateContactType": contact_type}),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {contact_type:?}: a refused put must store no contact, got {} with {:?}",
            answer.status, answer.body
        );
    }
}
