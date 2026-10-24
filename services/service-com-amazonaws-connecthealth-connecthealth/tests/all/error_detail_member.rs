// SPDX-License-Identifier: CC0-1.0

//! Where a ConnectHealth failure reports its detail text: the `Message` member of the
//! error body.
//!
//! The RULE under test, asserted over a table rather than one recorded response: every
//! error this service answers carries the exception's detail text in a body member spelled
//! `Message` — the spelling ConnectHealth uses on the wire — whichever layer produced the
//! failure (a handler's modelled exception, the boundary validator's refusal, or the
//! scaffold's 501), and the body carries that text under no other key. The text itself is
//! built from the REQUEST:
//!
//! * a not-found names the identifiers the request asked for — the subscription and its
//!   parent domain when a subscription could not be found, the domain when the domain
//!   could not be found;
//! * a value outside a member's modelled `@range` gets AWS's standard constraint sentence,
//!   `N validation error(s) detected: Value '<value>' at '<member>' failed to satisfy
//!   constraint: <requirement>`, naming the offending value, the member path and the bound
//!   it violated.
//!
//! The rows differ along what the rule generalises over: which operation is called, which
//! of the two identifiers is the missing one, the form of the identifiers (both modelled
//! `DomainId` prefixes, the shortest and longest the length constraint allows, mixed-case
//! `SubscriptionId` suffixes), which layer renders the error, and — for the range — each
//! published bound and one past each. The ids the issue recorded are two rows of that table.
//!
//! Model citations, all from
//! `smithy_query(query="operation",operation="GetSubscription,ActivateSubscription,DeactivateSubscription,ListSubscriptions,CreateSubscription",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`
//! and `smithy_query(query="shape",shape="ResourceNotFoundException,ValidationException")`:
//! `DomainId` is `@length 20..25` with pattern `^(hai-|dom-)[a-z0-9]+$`, `SubscriptionId` is
//! `@length 25..25` with pattern `^sub-[a-zA-Z0-9]{21}$`, `ListSubscriptionsInput.maxResults`
//! is an `Integer` with `@range{min:1,max:100}` bound as `@httpQuery("maxResults")`, and both
//! `ResourceNotFoundException` (404) and `ValidationException` (400) model exactly one
//! member, `message` -> `String`.

use serde_json::{json, Value};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};

/// The body member ConnectHealth reports an exception's detail text in.
const DETAIL_MEMBER: &str = "Message";

/// The member name the framework's restJson1 renderers would otherwise write it under —
/// the Smithy member name of every exception shape in this model. No error body may carry
/// the text here: a client reading one key must not find the text under the other.
const RENDERED_DETAIL_MEMBER: &str = "message";

/// The modelled `@range` of `maxResults` on both list operations, asserted at each bound
/// and one past each (`smithy_query(query="operation",operation="ListSubscriptions")`
/// reports `range {min: 1, max: 100}`).
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// The detail text an error body carries, failing with the whole body when it carries
/// none — or carries it under the key this service does not use.
fn detail_text(case: &str, body: &Value) -> String {
    assert!(
        body.get(RENDERED_DETAIL_MEMBER).is_none(),
        "case {case:?}: the detail text must be reported under {DETAIL_MEMBER} only, \
         but the body also carries {RENDERED_DETAIL_MEMBER}: {body}"
    );
    let text = body
        .get(DETAIL_MEMBER)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the error body must carry a {DETAIL_MEMBER} member, got {body}")
        });
    assert!(
        !text.trim().is_empty(),
        "case {case:?}: {DETAIL_MEMBER} must carry the detail text, got {text:?}"
    );
    text.to_string()
}

/// AWS's standard constraint-violation sentence for one violation, written here
/// independently of the emulator's own renderer.
///
/// Source: the envelope AWS documents for a modelled constraint violation, and the form
/// the corpus recorded for `maxResults=101`
/// ("1 validation error detected: Value '101' at 'maxResults' failed to satisfy
/// constraint: Member must have value less than or equal to 100").
fn one_validation_error(value: i64, member_path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member_path}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// A well-formed `DomainId` that names nothing: `prefix` + `filler` repeated to `len`.
///
/// Both modelled prefixes and the whole modelled length range are reachable this way, so
/// the table does not depend on one recorded id.
fn absent_domain_id(prefix: &str, len: usize) -> String {
    let mut id = prefix.to_string();
    while id.len() < len {
        id.push('z');
    }
    id
}

/// One not-found row: the request to make, and every identifier its answer must name.
struct NotFound {
    case: String,
    method: &'static str,
    path: String,
    named: Vec<String>,
}

#[tokio::test]
async fn every_not_found_names_the_requested_identifiers_in_the_message_member() {
    let emulator = Emulator::new("us-east-1");
    let domain_id = string_at(
        &emulator.create_domain(&json!({ "name": "clinic" })).await,
        "domainId",
    );
    let subscription_id = string_at(
        &emulator
            .send(
                "POST",
                &format!("/domains/{domain_id}/subscriptions"),
                Some(&json!({})),
            )
            .await
            .1,
        "subscriptionId",
    );

    // Round trip first: while both exist, the readers find them — so every 404 below is a
    // genuine absence rather than a blanket refusal.
    let (status, found) = emulator
        .send(
            "GET",
            &format!("/domains/{domain_id}/subscriptions/{subscription_id}"),
            None,
        )
        .await;
    assert_eq!(status, 200, "GetSubscription of a live subscription: {found}");
    assert!(
        found.get(DETAIL_MEMBER).is_none() && found.get(RENDERED_DETAIL_MEMBER).is_none(),
        "a success carries no detail member, got {found}"
    );

    // Absent domains across both modelled prefixes and the ends of the modelled length
    // range (20..25), and absent subscriptions across the character classes the
    // `SubscriptionId` pattern allows (21 of `[a-zA-Z0-9]`).
    let absent_domains = [
        absent_domain_id("dom-doesnotexist", 21),
        absent_domain_id("dom-", 20),
        absent_domain_id("dom-", 25),
        absent_domain_id("hai-", 20),
        absent_domain_id("hai-legacy", 25),
    ];
    let absent_subscriptions = [
        "sub-000000000000000000000",
        "sub-d4d4d4d4d4d4d4d4d4d4d",
        "sub-AbCdEfGhIjKlMnOpQrStU",
        "sub-zzzzzzzzzzzzzzzzzzzzz",
        "sub-000000000000000000001",
    ];

    // A domain this service allocated and has since deleted: its id names a domain that
    // is GONE, so every reader of it — including the repeated delete — names it in the
    // modelled not-found. (`DeleteDomain` of an id with NO allocation history is not a
    // not-found at all; see `domain_account_cap_and_lookup_semantics`.)
    let deleted_domain = string_at(
        &emulator
            .create_domain(&json!({ "name": "clinic-deleted" }))
            .await,
        "domainId",
    );
    let (status, deleted) = emulator
        .send("DELETE", &format!("/domain/{deleted_domain}"), None)
        .await;
    assert_eq!(status, 200, "DeleteDomain of a live domain: {deleted}");

    let mut rows: Vec<NotFound> = Vec::new();
    for (operation, method) in [("GetDomain", "GET"), ("DeleteDomain", "DELETE")] {
        rows.push(NotFound {
            case: format!("{operation} of the deleted domain {deleted_domain}"),
            method,
            path: format!("/domain/{deleted_domain}"),
            named: vec![deleted_domain.clone()],
        });
    }
    for absent_subscription in absent_subscriptions {
        // A subscription that is not in a domain that EXISTS: the answer must tell the
        // two identifiers apart, so it names both.
        for (operation, method, suffix) in [
            ("GetSubscription", "GET", ""),
            ("ActivateSubscription", "POST", "/activate"),
            ("DeactivateSubscription", "POST", "/deactivate"),
        ] {
            rows.push(NotFound {
                case: format!("{operation} of {absent_subscription} in a live domain"),
                method,
                path: format!(
                    "/domains/{domain_id}/subscriptions/{absent_subscription}{suffix}"
                ),
                named: vec![domain_id.clone(), absent_subscription.to_string()],
            });
        }
    }
    for absent_domain in &absent_domains {
        // The domain itself is the missing one: every operation addressed to it names it.
        for (operation, method, path) in [
            ("GetDomain", "GET", format!("/domain/{absent_domain}")),
            (
                "ListSubscriptions",
                "GET",
                format!("/domains/{absent_domain}/subscriptions"),
            ),
            (
                "CreateSubscription",
                "POST",
                format!("/domains/{absent_domain}/subscriptions"),
            ),
        ] {
            rows.push(NotFound {
                case: format!("{operation} under the absent domain {absent_domain}"),
                method,
                path,
                named: vec![absent_domain.clone()],
            });
        }
        // A live subscription id under an absent domain: both identifiers again.
        rows.push(NotFound {
            case: format!("GetSubscription of a live subscription under {absent_domain}"),
            method: "GET",
            path: format!("/domains/{absent_domain}/subscriptions/{subscription_id}"),
            named: vec![absent_domain.clone(), subscription_id.clone()],
        });
    }

    for row in &rows {
        let (status, body) = emulator
            .send(row.method, &row.path, Some(&json!({})))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {:?}: expected 404 ResourceNotFoundException, got {status} with {body}",
            row.case
        );
        let text = detail_text(&row.case, &body);
        for identifier in &row.named {
            assert!(
                text.contains(identifier),
                "case {:?}: the message must name {identifier:?}, got {text:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused_with_the_standard_constraint_message() {
    let emulator = Emulator::new("eu-central-1");
    let domain_id = string_at(
        &emulator.create_domain(&json!({ "name": "clinic" })).await,
        "domainId",
    );

    let (min, max) = MAX_RESULTS_RANGE;
    // Each published bound, and one past each, plus two further values outside it so the
    // sentence is not keyed to the recorded 101.
    let rows: [(i64, u16); 7] = [
        (min - 1, 400),
        (-7, 400),
        (min, 200),
        (max, 200),
        (max + 1, 400),
        (1_000, 400),
        (i64::from(i32::MAX), 400),
    ];
    // Both list operations bind `maxResults` the same way, so the rule covers both.
    for (operation, path) in [
        (
            "ListSubscriptions",
            format!("/domains/{domain_id}/subscriptions"),
        ),
        ("ListDomains", "/domain".to_string()),
    ] {
        for (max_results, expected_status) in rows {
            let case = format!("{operation} with maxResults={max_results}");
            let (status, body) = emulator
                .send("GET", &format!("{path}?maxResults={max_results}"), None)
                .await;
            assert_eq!(
                status, expected_status,
                "case {case:?}: expected HTTP {expected_status}, got {status} with {body}"
            );
            if expected_status == 200 {
                continue;
            }
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: got {body}"
            );
            let requirement = if max_results < min {
                format!("Member must have value greater than or equal to {min}")
            } else {
                format!("Member must have value less than or equal to {max}")
            };
            let text = detail_text(&case, &body);
            assert_eq!(
                text,
                one_validation_error(max_results, "maxResults", &requirement),
                "case {case:?}: expected AWS's standard constraint sentence, got {text:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_error_the_framework_renders_reports_its_text_in_the_message_member_too() {
    let emulator = Emulator::new("us-west-2");

    // A `domainId` outside the model's own length/pattern never reaches a handler: the
    // boundary validator refuses it. The sentence still has to arrive readable, and it
    // still names the member and the value the request supplied.
    let malformed: [(&str, &str); 5] = [
        ("too short (19)", "dom-aaaaaaaaaaaaaaa"),
        ("too long (26)", "dom-aaaaaaaaaaaaaaaaaaaaaa"),
        ("uppercase, outside the character class", "dom-AAAAAAAAAAAAAAAA"),
        ("underscore, outside the character class", "dom-aaaaaaaaaaaa_aaa"),
        ("an unmodelled prefix", "zzz-aaaaaaaaaaaaaaaa"),
    ];
    for (case, bad_id) in malformed {
        let (status, body) = emulator
            .send("GET", &format!("/domain/{bad_id}"), None)
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?} ({bad_id}): expected 400 ValidationException, got {status} with {body}"
        );
        let text = detail_text(case, &body);
        assert!(
            text.starts_with("1 validation error detected: ")
                && text.contains("'domainId'")
                && text.contains(bad_id),
            "case {case:?}: the refusal must name the member and the value it was given, \
             got {text:?}"
        );
    }

    // The scaffold's 501 is rendered by the framework as well, from a synthetic shape of
    // its own; it reports which operation is unimplemented, in the same member.
    // `ListTagsForResource` is one of this service's remaining stubs
    // (`src/ops/list_tags_for_resource.rs`).
    let arn = "arn:aws:health-agent:us-west-2:000000000000:domain/dom-zzzzzzzzzzzzzzzz";
    let encoded = arn.replace(':', "%3A").replace('/', "%2F");
    let (status, body) = emulator
        .send("GET", &format!("/tags/{encoded}"), None)
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (501, "NotImplemented"),
        "a stub answers 501 NotImplemented, got {status} with {body}"
    );
    let text = detail_text("the scaffold's 501", &body);
    assert!(
        text.contains("ListTagsForResource"),
        "the 501 must name the unimplemented operation, got {text:?}"
    );
}
