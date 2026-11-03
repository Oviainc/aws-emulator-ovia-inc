// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListRules`.
//!
//! Every case drives the emulator the way a client does — a restJson1 request through
//! the router that mounts the service — and compares the listing against what
//! `CreateRule` actually persisted and `GetRule` reads back, so nothing here can pass on
//! a canned body: a summary built from the request instead of from state fails the ARN,
//! `isDefault` and timestamp assertions, and a listing that ignores its listener fails
//! the scoping assertions.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! 1. The listing reports exactly the rules persisted under the listener the request
//!    names — in either modelled identifier form for the service and the listener — and
//!    never a rule of another listener or of a listener of another service.
//! 2. Every member of a reported summary is the value `GetRule` answers with for the
//!    same rule.
//! 3. One stable order and a round-tripping `nextToken`: every page size reports each
//!    rule of the listener exactly once, and `maxResults` is honoured at each published
//!    bound of its `@range(1, 100)` while a value past either bound is refused.
//! 4. The listing follows state: a listener nothing has created a rule on reports exactly
//!    its implicit default rule with no `nextToken`, a rule created afterwards shows up,
//!    and deleting the service takes its listener and rules with it.
//! 5. An identifier naming no persisted service or listener is a 404
//!    `ResourceNotFoundException` carrying this service's not-found construction, and a
//!    `nextToken` this service never minted is a 400 `ValidationException`.

use crate::listeners::{a_service, listener_kind};
use crate::resource_not_found_messages::absent_id;
use crate::rules::{a_listener, create_rule, fixed_response_rule, get_rule, rule_kind};
use crate::vpclattice_service_slice::{
    as_label, expected_arn, is_lattice_id, is_timestamp, message_of, Answer, Lattice,
};
use service_com_amazonaws_vpclattice_mercurycontrolplane::state;

/// `GET /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules`, with the
/// query members this case sends.
pub(crate) async fn list_page(
    lattice: &Lattice,
    service_identifier: &str,
    listener_identifier: &str,
    query: &[(&str, String)],
) -> Answer {
    let mut path =
        format!("/services/{service_identifier}/listeners/{listener_identifier}/rules");
    if !query.is_empty() {
        let pairs: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        path.push('?');
        path.push_str(&pairs.join("&"));
    }
    lattice.call("GET", &path, None).await
}

/// The `items` member of a page, which `ListRulesResponse` declares required.
pub(crate) fn items_of(case: &str, answer: &Answer) -> Vec<serde_json::Value> {
    answer.body["items"].as_array().cloned().unwrap_or_else(|| {
        panic!(
            "case {case:?}: the required member items is missing from {}",
            answer.body
        )
    })
}

/// Every rule id the listing reports for one listener, paging with `max_results` until
/// the service stops handing back a `nextToken`.
async fn list_every_rule_id(
    lattice: &Lattice,
    service_identifier: &str,
    listener_identifier: &str,
    max_results: Option<u32>,
) -> Vec<String> {
    let case = format!("list {listener_identifier} maxResults={max_results:?}");
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    // One page per persisted rule is the worst case; the bound stops a broken token
    // round-trip from looping forever instead of failing.
    for _ in 0..64 {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(max) = max_results {
            query.push(("maxResults", max.to_string()));
        }
        if let Some(next) = &token {
            query.push(("nextToken", next.clone()));
        }
        let answer = list_page(lattice, service_identifier, listener_identifier, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        let items = items_of(&case, &answer);
        if let Some(max) = max_results {
            assert!(
                items.len() <= max as usize,
                "case {case:?}: a page carried {} items, past maxResults={max}",
                items.len()
            );
        }
        for item in &items {
            ids.push(
                item["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a RuleSummary without an id: {item}"))
                    .to_string(),
            );
        }
        match answer.body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return ids,
        }
    }
    panic!(
        "case {case:?}: the listing never stopped paging, so nextToken does not round-trip \
         to a last page"
    );
}

/// The id of the listener's implicit DEFAULT rule: "Each listener has a default rule"
/// (`CreateListener`'s `defaultAction`), so exactly one summary of a listener's listing
/// is the one `isDefault` marks.
pub(crate) async fn default_rule_id(
    lattice: &Lattice,
    service_identifier: &str,
    listener_identifier: &str,
) -> String {
    let case = format!("the default rule of {listener_identifier}");
    let answer = list_page(lattice, service_identifier, listener_identifier, &[]).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    let defaults: Vec<serde_json::Value> = items_of(&case, &answer)
        .into_iter()
        .filter(|item| item["isDefault"] == serde_json::json!(true))
        .collect();
    assert_eq!(
        defaults.len(),
        1,
        "case {case:?}: expected exactly one rule with isDefault true, got {:?}",
        defaults
    );
    defaults[0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a default rule without an id: {:?}", defaults[0]))
        .to_string()
}

/// Persist `rules` under one listener, answering the rule ids in creation order.
async fn rules_under(
    lattice: &Lattice,
    service_id: &str,
    listener_id: &str,
    rules: &[(&str, i64)],
) -> Vec<String> {
    let mut ids = Vec::new();
    for (name, priority) in rules {
        let created = create_rule(
            lattice,
            service_id,
            listener_id,
            fixed_response_rule(name, *priority),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case fixture rule {name:?} priority {priority}: expected 201, got {} / {}",
            created.status, created.body
        );
        ids.push(created.str_field("id"));
    }
    ids
}

/// Two services, three listeners and rules spread over them, so every scoping case has
/// both a match and a non-match.
struct Topology {
    service_id: String,
    service_arn: String,
    /// The listener the listing cases are run against, and its rules in creation order —
    /// its implicit default rule first, since the listener was created with it.
    listener_id: String,
    listener_rules: Vec<String>,
    /// The id of that listener's default rule, the one entry of `listener_rules` whose
    /// `isDefault` is true.
    listener_default: String,
    /// A second listener OF THE SAME service, with its own rules.
    sibling_id: String,
    sibling_rules: Vec<String>,
    /// A listener of ANOTHER service, with its own rules.
    other_service_id: String,
    other_listener_id: String,
    other_rules: Vec<String>,
}

async fn build_topology(lattice: &Lattice) -> Topology {
    let (service_id, service_arn) = a_service(lattice, "list-rules-host").await;
    let listener_id = a_listener(lattice, &service_id, "list-rules-listener").await;
    let sibling_id = a_listener(lattice, &service_id, "list-rules-sibling").await;
    let (other_service_id, _) = a_service(lattice, "list-rules-other").await;
    let other_listener_id =
        a_listener(lattice, &other_service_id, "list-rules-other-listener").await;

    // Each listener was created WITH its default rule, so that rule is the first one of
    // its listing; everything created here follows it.
    let listener_default = default_rule_id(lattice, &service_id, &listener_id).await;
    let sibling_default = default_rule_id(lattice, &service_id, &sibling_id).await;
    let other_default = default_rule_id(lattice, &other_service_id, &other_listener_id).await;

    // Priorities deliberately out of order, so a listing that sorted by priority (or by
    // the hash map's order) would not match creation order.
    let mut listener_rules = vec![listener_default.clone()];
    listener_rules.extend(
        rules_under(
            lattice,
            &service_id,
            &listener_id,
            &[("alpha", 30), ("bravo", 7), ("charlie", 2000), ("delta", 1)],
        )
        .await,
    );
    let mut sibling_rules = vec![sibling_default];
    sibling_rules.extend(
        rules_under(lattice, &service_id, &sibling_id, &[("echo", 30), ("foxtrot", 7)]).await,
    );
    let mut other_rules = vec![other_default];
    other_rules.extend(
        rules_under(
            lattice,
            &other_service_id,
            &other_listener_id,
            &[("golf", 30)],
        )
        .await,
    );

    Topology {
        service_id,
        service_arn,
        listener_id,
        listener_rules,
        listener_default,
        sibling_id,
        sibling_rules,
        other_service_id,
        other_listener_id,
        other_rules,
    }
}

/// Rule 1: the listing is scoped to the listener the request names, in either modelled
/// identifier form for the service and the listener.
#[tokio::test]
async fn the_listing_reports_exactly_the_rules_of_the_named_listener() {
    let lattice = Lattice::new();
    let topology = build_topology(&lattice).await;

    let listener_arn = expected_arn(
        &listener_kind(&topology.service_id),
        &topology.listener_id,
    );
    let other_listener_arn = expected_arn(
        &listener_kind(&topology.other_service_id),
        &topology.other_listener_id,
    );
    let empty_listener = a_listener(&lattice, &topology.service_id, "list-rules-empty").await;
    // A listener nothing has created a rule on still holds one: its default rule.
    let empty_listener_default =
        default_rule_id(&lattice, &topology.service_id, &empty_listener).await;

    // (case, service identifier, listener identifier, the rule ids state says come back)
    let cases: Vec<(&str, String, String, Vec<String>)> = vec![
        (
            "both identifiers as bare ids",
            topology.service_id.clone(),
            topology.listener_id.clone(),
            topology.listener_rules.clone(),
        ),
        (
            "the sibling listener of the same service",
            topology.service_id.clone(),
            topology.sibling_id.clone(),
            topology.sibling_rules.clone(),
        ),
        (
            "a listener of another service",
            topology.other_service_id.clone(),
            topology.other_listener_id.clone(),
            topology.other_rules.clone(),
        ),
        (
            "the service as its ARN",
            as_label(&topology.service_arn),
            topology.listener_id.clone(),
            topology.listener_rules.clone(),
        ),
        (
            "the listener as its ARN",
            topology.service_id.clone(),
            as_label(&listener_arn),
            topology.listener_rules.clone(),
        ),
        (
            "both identifiers as ARNs",
            as_label(&topology.service_arn),
            as_label(&listener_arn),
            topology.listener_rules.clone(),
        ),
        (
            "the other service's listener by ARN",
            topology.other_service_id.clone(),
            as_label(&other_listener_arn),
            topology.other_rules.clone(),
        ),
        (
            "a listener no rule was created on",
            topology.service_id.clone(),
            empty_listener,
            vec![empty_listener_default],
        ),
    ];

    for (case, service_identifier, listener_identifier, expected) in cases {
        let reported =
            list_every_rule_id(&lattice, &service_identifier, &listener_identifier, None).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the rules {expected:?}, got {reported:?}"
        );
    }
}

/// Rule 2: every member of a reported summary is the persisted value `GetRule` answers
/// with, for every rule of the listener.
#[tokio::test]
async fn each_summary_carries_the_persisted_rule() {
    let lattice = Lattice::new();
    let topology = build_topology(&lattice).await;

    let page = list_page(
        &lattice,
        &topology.service_id,
        &topology.listener_id,
        &[],
    )
    .await;
    assert_eq!(
        page.status, 200,
        "case the listing: expected 200, got {} / {}",
        page.status, page.body
    );
    let items = items_of("the listing", &page);
    assert_eq!(
        items.len(),
        topology.listener_rules.len(),
        "case the listing: expected {} summaries, got {}",
        topology.listener_rules.len(),
        page.body
    );

    for (summary, rule_id) in items.iter().zip(&topology.listener_rules) {
        let read = get_rule(
            &lattice,
            &topology.service_id,
            &topology.listener_id,
            rule_id,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case get {rule_id}: expected 200, got {} / {}",
            read.status, read.body
        );
        // Every member `RuleSummary` declares, against the record behind it.
        for member in [
            "id",
            "arn",
            "name",
            "isDefault",
            "priority",
            "createdAt",
            "lastUpdatedAt",
        ] {
            assert_eq!(
                summary[member], read.body[member],
                "case summary member {member:?} of {rule_id}: expected the persisted {:?}, \
                 got {:?}",
                read.body[member], summary[member]
            );
        }
        assert!(
            is_lattice_id(summary["id"].as_str().unwrap_or_default(), state::PREFIX_RULE),
            "case id of {rule_id}: expected a modelled rule- id, got {:?}",
            summary["id"]
        );
        assert_eq!(
            summary["arn"].as_str().unwrap_or_default(),
            expected_arn(
                &rule_kind(&topology.service_id, &topology.listener_id),
                rule_id
            ),
            "case arn of {rule_id}: expected the ARN nested in its listener's, got {:?}",
            summary["arn"]
        );
        assert_eq!(
            summary["isDefault"],
            serde_json::json!(*rule_id == topology.listener_default),
            "case isDefault of {rule_id}: only the listener's default rule reports true, \
             got {:?}",
            summary["isDefault"]
        );
        for member in ["createdAt", "lastUpdatedAt"] {
            assert!(
                is_timestamp(&summary[member]),
                "case {member:?} of {rule_id}: expected an instant, got {:?}",
                summary[member]
            );
        }
    }
}

/// Rule 3: one stable order, every page size reporting each rule exactly once, and
/// `maxResults` honoured at each published bound of its range.
#[tokio::test]
async fn every_page_size_reports_each_rule_exactly_once() {
    let lattice = Lattice::new();
    let topology = build_topology(&lattice).await;

    let unpaged = list_every_rule_id(
        &lattice,
        &topology.service_id,
        &topology.listener_id,
        None,
    )
    .await;
    assert_eq!(
        unpaged, topology.listener_rules,
        "case unpaged: expected the listener's rules in creation order {:?}, got {unpaged:?}",
        topology.listener_rules
    );

    // Each row is a different page size — one, one that divides the result set exactly,
    // the whole set, more than the whole set, and the published upper bound of
    // `maxResults`' modelled `@range(1, 100)`.
    for max_results in [1_u32, 2, 3, 4, 5, 100] {
        let paged = list_every_rule_id(
            &lattice,
            &topology.service_id,
            &topology.listener_id,
            Some(max_results),
        )
        .await;
        assert_eq!(
            paged, unpaged,
            "case maxResults={max_results}: expected the same rules in the same order as the \
             unpaged listing {unpaged:?}, got {paged:?}"
        );
    }

    // The first page of a one-per-page listing hands back a token, and the token is
    // opaque: it is not one of the ids it pages past.
    let first = list_page(
        &lattice,
        &topology.service_id,
        &topology.listener_id,
        &[("maxResults", "1".to_string())],
    )
    .await;
    let token = first.body["nextToken"].as_str().unwrap_or_else(|| {
        panic!(
            "case maxResults=1 of {} rules: expected a nextToken, got {}",
            unpaged.len(),
            first.body
        )
    });
    assert!(
        !unpaged.iter().any(|id| id == token),
        "case the cursor is opaque: expected a token that is not a rule id, got {token:?}"
    );

    // One past each published bound of `@range(1, 100)` is refused, never served as a
    // page of some other size.
    for (case, max_results) in [("below the lower bound", "0"), ("past the upper bound", "101")] {
        let answer = list_page(
            &lattice,
            &topology.service_id,
            &topology.listener_id,
            &[("maxResults", max_results.to_string())],
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case maxResults {case} ({max_results}): expected HTTP 400, got {} / {}",
            answer.status, answer.body
        );
    }
}

/// Rule 4: the listing follows state — a rule created after an earlier listing shows up
/// in the next one, and deleting the service takes its listener and rules with it.
#[tokio::test]
async fn the_listing_follows_what_is_persisted() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "list-rules-state").await;
    let listener_id = a_listener(&lattice, &service_id, "list-rules-state-listener").await;

    // A listener nothing has created a rule on reports exactly its default rule: "Each
    // listener has a default rule" (`CreateListener`'s `defaultAction`).
    let fresh = list_page(&lattice, &service_id, &listener_id, &[]).await;
    assert_eq!(
        fresh.status, 200,
        "case a listener no rule was created on: expected 200, got {} / {}",
        fresh.status, fresh.body
    );
    let fresh_items = items_of("a listener no rule was created on", &fresh);
    assert_eq!(
        fresh_items.len(),
        1,
        "case a listener no rule was created on: expected only its default rule, got {}",
        fresh.body
    );
    assert_eq!(
        fresh_items[0]["isDefault"],
        serde_json::json!(true),
        "case a listener no rule was created on: expected its one rule to be the default \
         rule, got {}",
        fresh.body
    );
    assert!(
        fresh.body["nextToken"].is_null(),
        "case a listener no rule was created on: expected no nextToken, got {}",
        fresh.body
    );

    // Each create adds exactly its own rule to the listing, after the default rule the
    // listener was created with, in creation order.
    let mut expected: Vec<String> =
        vec![default_rule_id(&lattice, &service_id, &listener_id).await];
    for (name, priority) in [("state-one", 5_i64), ("state-two", 4), ("state-three", 3)] {
        let created = create_rule(
            &lattice,
            &service_id,
            &listener_id,
            fixed_response_rule(name, priority),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case create {name}: expected 201, got {} / {}",
            created.status, created.body
        );
        expected.push(created.str_field("id"));
        let reported = list_every_rule_id(&lattice, &service_id, &listener_id, None).await;
        assert_eq!(
            reported, expected,
            "case after creating {name}: expected {expected:?}, got {reported:?}"
        );
    }

    // "If you delete a service, all resources related to the service, such as … the
    // listeners, listener rules … are also deleted" (`DeleteService`), so the listing of
    // a deleted service's listener names nothing persisted.
    let deleted = lattice
        .call("DELETE", &format!("/services/{service_id}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the service: expected 200, got {} / {}",
        deleted.status, deleted.body
    );
    let after = list_page(&lattice, &service_id, &listener_id, &[]).await;
    assert_eq!(
        after.status, 404,
        "case list after the service was deleted: expected 404, got {} / {}",
        after.status, after.body
    );
}

/// Rule 5: a request this operation refuses carries the modelled error — 404 with this
/// service's not-found construction for an identifier naming nothing persisted, 400 for a
/// `nextToken` this service never minted — never a 501 and never a whole-store listing.
#[tokio::test]
async fn a_refused_request_carries_the_modelled_error() {
    let lattice = Lattice::new();
    let topology = build_topology(&lattice).await;

    // The not-found construction, written out here rather than read from the service, so
    // no row can pass by agreeing with the code under test: the resource kind in AWS's
    // CamelCase token, the identifier the caller submitted, then the fixed sentence.
    let not_found_message = |label: &str, identifier: &str| format!("{label} {identifier} cannot be found.");

    // (case, service identifier, listener identifier, the kind token the message names)
    let cases: Vec<(&str, String, String, &str)> = vec![
        (
            "a service id nothing created",
            absent_id("svc-", "0"),
            topology.listener_id.clone(),
            "Service",
        ),
        (
            "a listener id nothing created",
            topology.service_id.clone(),
            absent_id("listener-", "1"),
            "Listener",
        ),
        (
            "a listener of ANOTHER service, reached through this one",
            topology.service_id.clone(),
            topology.other_listener_id.clone(),
            "Listener",
        ),
        (
            "this service's listener, reached through the other service",
            topology.other_service_id.clone(),
            topology.listener_id.clone(),
            "Listener",
        ),
    ];

    for (case, service_identifier, listener_identifier, label) in cases {
        let answer = list_page(&lattice, &service_identifier, &listener_identifier, &[]).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} / {}",
            answer.status, answer.body
        );
        assert!(
            answer
                .error_code
                .as_deref()
                .is_some_and(|code| code.contains("ResourceNotFoundException")),
            "case {case:?}: expected ResourceNotFoundException, got {:?} / {}",
            answer.error_code,
            answer.body
        );
        let identifier = if label == "Service" {
            &service_identifier
        } else {
            &listener_identifier
        };
        assert_eq!(
            message_of(case, &answer),
            not_found_message(label, identifier),
            "case {case:?}: expected the not-found construction over the submitted identifier, \
             got {}",
            answer.body
        );
    }

    // A cursor no listing of this service ever minted — one that is not hex at all, and
    // one that is hex but decodes to no page.
    for (case, next_token) in [
        ("a nextToken this service never minted", "not-a-cursor"),
        ("a nextToken that is hex but names no page", "6162636465"),
    ] {
        let answer = list_page(
            &lattice,
            &topology.service_id,
            &topology.listener_id,
            &[("nextToken", next_token.to_string())],
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} / {}",
            answer.status, answer.body
        );
        assert!(
            answer
                .error_code
                .as_deref()
                .is_some_and(|code| code.contains("ValidationException")),
            "case {case:?}: expected ValidationException, got {:?} / {}",
            answer.error_code,
            answer.body
        );
    }
}
