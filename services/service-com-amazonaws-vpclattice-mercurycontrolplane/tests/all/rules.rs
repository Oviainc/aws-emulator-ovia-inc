// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the listener-rule slice (`CreateRule` + `GetRule`).
//!
//! Every case drives the service the way a client does — a signed restJson1 request
//! through the router that mounts it — and reads the answer back out of a LATER
//! request, so nothing here can pass on a canned body: an id that is never stored
//! fails the read-back, and a response echoed from the request fails the ARN,
//! defaulting and nesting assertions.
//!
//! The harness (`Lattice`, `Answer`, `as_label`, `expected_arn`, …) and the service /
//! listener fixtures are the ones the suites beside this file already build; nothing is
//! duplicated here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! 1. `CreateRule` mints a rule under the listener the request names: an id of
//!    `^rule-[0-9a-z]{17}$`, an ARN nested in the listener's
//!    (`…:service/svc-…/listener/listener-…/rule/rule-…`) built from the request's
//!    region and the emulator's account; the record is persisted, and `GetRule` by the
//!    bare id and by the ARN both report exactly what was stored — including the
//!    `match`, whose HTTP criteria carry `headerMatches` as a present list (the empty
//!    one when the request configured no header match), `isDefault` false and its
//!    timestamps.
//! 2. "Each rule for a specific listener must have a unique priority": a second create
//!    taking a priority another rule of the SAME listener holds is a 409
//!    `ConflictException`, while that priority under another listener is fine.
//! 3. A rule name is unique within its LISTENER: a second create under a taken name is
//!    a 409 `ConflictException`, while the same name under another listener is fine.
//! 4. A `clientToken` retry answers with the first rule instead of a second one, and a
//!    retry whose parameters differ in ANY member is refused with a 409.
//! 5. An identifier naming no persisted service, listener or rule is a 404
//!    `ResourceNotFoundException` carrying this service's not-found construction
//!    (`"<ResourceType> <identifier> cannot be found."`) — including a rule read through
//!    the wrong listener or the wrong service.
//! 6. Input outside a documented or modelled constraint is a 400 `ValidationException`:
//!    a name the `RuleName` pattern forbids, a `priority` past each published bound of
//!    `@range(1, 2000)`, and a fixed-response status code outside the catalog its
//!    member documents.
//! 7. Deleting the service deletes its listener rules, as `DeleteService` documents.

use crate::listeners::{a_service, create_listener, target_group};
use crate::vpclattice_service_slice::{
    as_label, expected_arn, is_lattice_id, is_timestamp, Answer, Lattice,
};

/// The ARN kind of a rule of `listener_id`: a rule ARN nests under its listener's,
/// which nests under its service's (`RuleArn`'s modelled pattern).
pub(crate) fn rule_kind(service_id: &str, listener_id: &str) -> String {
    format!("service/{service_id}/listener/{listener_id}/rule")
}

/// A listener port no other fixture listener of this suite has asked for, inside the
/// modelled `Port` range (1..65535).
fn next_fixture_port() -> i64 {
    static NEXT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(20_000);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Create a listener to hang rules off, answering its id.
pub(crate) async fn a_listener(lattice: &Lattice, service_identifier: &str, name: &str) -> String {
    let created = create_listener(
        lattice,
        service_identifier,
        serde_json::json!({
            "name": name,
            "protocol": "HTTP",
            // Its own port: a listener's port is unique within its service (the rule
            // `listener_conflicts_and_deletion.rs` pins), so fixture listeners that share
            // a service must not share a port.
            "port": next_fixture_port(),
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case fixture listener {name:?}: expected 201, got {} / {}",
        created.status, created.body
    );
    created.str_field("id")
}

/// `POST /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules`.
pub(crate) async fn create_rule(
    lattice: &Lattice,
    service_identifier: &str,
    listener_identifier: &str,
    body: serde_json::Value,
) -> Answer {
    lattice
        .call(
            "POST",
            &format!("/services/{service_identifier}/listeners/{listener_identifier}/rules"),
            Some(body),
        )
        .await
}

/// `GET /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules/{ruleIdentifier}`.
pub(crate) async fn get_rule(
    lattice: &Lattice,
    service_identifier: &str,
    listener_identifier: &str,
    rule_identifier: &str,
) -> Answer {
    lattice
        .call(
            "GET",
            &format!(
                "/services/{service_identifier}/listeners/{listener_identifier}/rules/{rule_identifier}"
            ),
            None,
        )
        .await
}

/// A rule body with a fixed-response action, for the cases where only the priority or
/// the name is under test.
pub(crate) fn fixed_response_rule(name: &str, priority: i64) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "priority": priority,
        "match": { "httpMatch": { "pathMatch": { "match": { "prefix": "/" } } } },
        "action": { "fixedResponse": { "statusCode": 404 } },
    })
}

/// Rule 1, over a table that varies the match form (exact path, prefix path, method,
/// header matches, an httpMatch with no member at all), the action kind, the
/// target-group count and weights, and the priority. The recorded case is one row of it.
#[tokio::test]
async fn creating_rules_persists_them_and_both_identifier_forms_read_them_back() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "rule-host").await;
    let listener_id = a_listener(&lattice, &service_id, "rule-host-listener").await;

    // (case, request body, the match the stored rule must report, the action it must
    // report). A rule's HTTP match is reported with its `headerMatches` present — the
    // empty list when the request configured no header match — so the match in the
    // answer is not always the one the request spelled out.
    type Row = (&'static str, serde_json::Value, serde_json::Value, serde_json::Value);
    let cases: Vec<Row> = vec![
        (
            "exact path, fixed response 404, lowest priority",
            serde_json::json!({
                "name": "exact-path",
                "priority": 1,
                "match": {
                    "httpMatch": {
                        "pathMatch": { "match": { "exact": "/orders" }, "caseSensitive": true },
                    },
                },
                "action": { "fixedResponse": { "statusCode": 404 } },
            }),
            serde_json::json!({
                "httpMatch": {
                    "pathMatch": { "match": { "exact": "/orders" }, "caseSensitive": true },
                    "headerMatches": [],
                },
            }),
            serde_json::json!({ "fixedResponse": { "statusCode": 404 } }),
        ),
        (
            "method + prefix path, forward to one target group with no weight",
            serde_json::json!({
                "name": "prefix-path",
                "priority": 2000,
                "match": {
                    "httpMatch": {
                        "method": "GET",
                        "pathMatch": { "match": { "prefix": "/api/" } },
                    },
                },
                "action": {
                    "forward": {
                        "targetGroups": [{ "targetGroupIdentifier": target_group("1") }],
                    },
                },
            }),
            serde_json::json!({
                "httpMatch": {
                    "method": "GET",
                    "pathMatch": { "match": { "prefix": "/api/" } },
                    "headerMatches": [],
                },
            }),
            // `weight`'s own documentation: "If there's only one target group
            // specified, then the default value is 100."
            serde_json::json!({
                "forward": {
                    "targetGroups": [
                        { "targetGroupIdentifier": target_group("1"), "weight": 100 },
                    ],
                },
            }),
        ),
        (
            "every header match type, forward to two weighted target groups",
            serde_json::json!({
                "name": "header-matches",
                "priority": 17,
                "match": {
                    "httpMatch": {
                        "headerMatches": [
                            { "name": "x-version", "match": { "exact": "v2" } },
                            { "name": "x-tenant", "match": { "prefix": "acme" }, "caseSensitive": false },
                            { "name": "x-trace", "match": { "contains": "abc" } },
                        ],
                    },
                },
                "action": {
                    "forward": {
                        "targetGroups": [
                            { "targetGroupIdentifier": target_group("2"), "weight": 30 },
                            { "targetGroupIdentifier": target_group("3"), "weight": 70 },
                        ],
                    },
                },
            }),
            // Header matches the request DID configure are reported as sent.
            serde_json::json!({
                "httpMatch": {
                    "headerMatches": [
                        { "name": "x-version", "match": { "exact": "v2" } },
                        { "name": "x-tenant", "match": { "prefix": "acme" }, "caseSensitive": false },
                        { "name": "x-trace", "match": { "contains": "abc" } },
                    ],
                },
            }),
            serde_json::json!({
                "forward": {
                    "targetGroups": [
                        { "targetGroupIdentifier": target_group("2"), "weight": 30 },
                        { "targetGroupIdentifier": target_group("3"), "weight": 70 },
                    ],
                },
            }),
        ),
        (
            "an httpMatch with no member, fixed response 500",
            serde_json::json!({
                "name": "match-anything",
                "priority": 500,
                "match": { "httpMatch": {} },
                "action": { "fixedResponse": { "statusCode": 500 } },
                "tags": { "team": "platform" },
            }),
            serde_json::json!({ "httpMatch": { "headerMatches": [] } }),
            serde_json::json!({ "fixedResponse": { "statusCode": 500 } }),
        ),
        (
            "two target groups, one without a weight beside a sibling",
            serde_json::json!({
                "name": "mixed-weights",
                "priority": 42,
                "match": {
                    "httpMatch": { "method": "POST" },
                },
                "action": {
                    "forward": {
                        "targetGroups": [
                            { "targetGroupIdentifier": target_group("4"), "weight": 10 },
                            { "targetGroupIdentifier": target_group("5") },
                        ],
                    },
                },
            }),
            serde_json::json!({
                "httpMatch": { "method": "POST", "headerMatches": [] },
            }),
            // The documented default covers the single-target-group case only, so an
            // omitted weight beside a sibling stays absent.
            serde_json::json!({
                "forward": {
                    "targetGroups": [
                        { "targetGroupIdentifier": target_group("4"), "weight": 10 },
                        { "targetGroupIdentifier": target_group("5") },
                    ],
                },
            }),
        ),
    ];

    for (case, body, expected_match, expected_action) in cases {
        let created = create_rule(&lattice, &service_id, &listener_id, body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} / {}",
            created.status, created.body
        );

        let id = created.str_field("id");
        assert!(
            is_lattice_id(&id, "rule-"),
            "case {case:?}: expected an id of rule- plus 17 [0-9a-z], got {id:?}"
        );
        let arn = created.str_field("arn");
        assert_eq!(
            arn,
            expected_arn(&rule_kind(&service_id, &listener_id), &id),
            "case {case:?}: the rule ARN must nest under its listener's, got {arn:?}"
        );
        assert_eq!(
            created.body["name"], body["name"],
            "case {case:?}: expected name {}, got {}",
            body["name"], created.body
        );
        assert_eq!(
            created.body["priority"], body["priority"],
            "case {case:?}: expected priority {}, got {}",
            body["priority"], created.body
        );
        assert_eq!(
            created.body["match"], expected_match,
            "case {case:?}: expected the stored match {expected_match}, got {}",
            created.body
        );
        assert_eq!(
            created.body["action"], expected_action,
            "case {case:?}: expected action {expected_action}, got {}",
            created.body
        );

        // The record is PERSISTED: both identifier forms the model allows read it back.
        for (form, identifier) in [("the bare id", id.clone()), ("the ARN", as_label(&arn))] {
            let read = get_rule(&lattice, &service_id, &listener_id, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} read by {form}: expected 200, got {} / {}",
                read.status, read.body
            );
            assert_eq!(
                read.str_field("id"),
                id,
                "case {case:?} read by {form}: expected id {id}, got {}",
                read.body
            );
            assert_eq!(
                read.str_field("arn"),
                arn,
                "case {case:?} read by {form}: expected arn {arn}, got {}",
                read.body
            );
            assert_eq!(
                read.body["name"], body["name"],
                "case {case:?} read by {form}: expected name {}, got {}",
                body["name"], read.body
            );
            assert_eq!(
                read.body["priority"], body["priority"],
                "case {case:?} read by {form}: expected priority {}, got {}",
                body["priority"], read.body
            );
            assert_eq!(
                read.body["match"], expected_match,
                "case {case:?} read by {form}: expected the stored match {expected_match}, got {}",
                read.body
            );
            assert_eq!(
                read.body["action"], expected_action,
                "case {case:?} read by {form}: expected action {expected_action}, got {}",
                read.body
            );
            // A rule `CreateRule` mints is an additional rule, never the listener's
            // default one.
            assert_eq!(
                read.body["isDefault"].as_bool(),
                Some(false),
                "case {case:?} read by {form}: expected isDefault false, got {}",
                read.body
            );
            for member in ["createdAt", "lastUpdatedAt"] {
                assert!(
                    is_timestamp(&read.body[member]),
                    "case {case:?} read by {form}: {member} must be an RFC 3339 instant, got {}",
                    read.body
                );
            }
        }

        // A read changes nothing a later caller sees.
        let again = get_rule(&lattice, &service_id, &listener_id, &id).await;
        assert_eq!(
            again.body,
            get_rule(&lattice, &service_id, &listener_id, &id).await.body,
            "case {case:?}: two reads of one rule must agree"
        );
    }
}

/// Rule 2: "Each rule for a specific listener must have a unique priority" (the
/// `priority` member's own documentation), tested at each published bound of
/// `@range(1, 2000)` and in between. The recorded unit is the middle row.
#[tokio::test]
async fn a_rule_priority_is_unique_within_its_listener_but_not_across_listeners() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "priority-host").await;
    let first_listener = a_listener(&lattice, &service_id, "priority-one").await;
    let second_listener = a_listener(&lattice, &service_id, "priority-two").await;

    // The lowest priority the model allows, one in the middle, and the highest.
    for priority in [1_i64, 7, 2000] {
        let original = create_rule(
            &lattice,
            &service_id,
            &first_listener,
            fixed_response_rule(&format!("first-at-{priority}"), priority),
        )
        .await;
        assert_eq!(
            original.status, 201,
            "case first rule at priority {priority}: expected 201, got {} / {}",
            original.status, original.body
        );
        let original_id = original.str_field("id");

        // A DIFFERENT rule name, so only the priority collides.
        let duplicate = create_rule(
            &lattice,
            &service_id,
            &first_listener,
            fixed_response_rule(&format!("second-at-{priority}"), priority),
        )
        .await;
        assert_eq!(
            duplicate.status, 409,
            "case duplicate priority {priority} in one listener: expected 409, got {} / {}",
            duplicate.status, duplicate.body
        );
        assert_eq!(
            duplicate.error_code.as_deref(),
            Some("ConflictException"),
            "case duplicate priority {priority}: expected the modelled ConflictException, got {:?} / {}",
            duplicate.error_code,
            duplicate.body
        );
        // The message is built from the request: it names the priority that collided.
        assert!(
            duplicate.body["message"]
                .as_str()
                .is_some_and(|text| text.contains(&priority.to_string())),
            "case duplicate priority {priority}: the conflict message must name the offending priority, got {}",
            duplicate.body
        );

        // The refused create stored nothing: the rule that holds the priority is
        // unchanged, and the name the refused call used is still free.
        let read = get_rule(&lattice, &service_id, &first_listener, &original_id).await;
        assert_eq!(
            read.body["priority"].as_i64(),
            Some(priority),
            "case duplicate priority {priority}: the original rule must keep it, got {}",
            read.body
        );
        assert_eq!(
            read.str_field("name"),
            format!("first-at-{priority}"),
            "case duplicate priority {priority}: the original rule must keep its name, got {}",
            read.body
        );
        let retried = create_rule(
            &lattice,
            &service_id,
            &first_listener,
            // A priority inside `@range(1, 2000)` that no rule of this listener holds.
            fixed_response_rule(
                &format!("second-at-{priority}"),
                if priority >= 2000 {
                    priority - 1
                } else {
                    priority + 1
                },
            ),
        )
        .await;
        assert_eq!(
            retried.status, 201,
            "case a refused create stores nothing, so second-at-{priority} is still free: got {} / {}",
            retried.status, retried.body
        );

        // The same priority under another listener of the same service is a different
        // rule, not a conflict.
        let elsewhere = create_rule(
            &lattice,
            &service_id,
            &second_listener,
            fixed_response_rule(&format!("first-at-{priority}"), priority),
        )
        .await;
        assert_eq!(
            elsewhere.status, 201,
            "case priority {priority} under a second listener: expected 201, got {} / {}",
            elsewhere.status, elsewhere.body
        );
        assert_ne!(
            elsewhere.str_field("id"),
            original_id,
            "case priority {priority} under a second listener: expected a distinct rule"
        );
    }
}

/// Rule 3: "The name must be unique within the listener" (the `name` member's own
/// documentation), and only within it.
#[tokio::test]
async fn a_rule_name_is_unique_within_its_listener_but_not_across_listeners() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "name-host").await;
    let first_listener = a_listener(&lattice, &service_id, "name-one").await;
    let second_listener = a_listener(&lattice, &service_id, "name-two").await;

    // Names differing along length and hyphenation, each inside `RuleName`'s 3..63.
    for (index, name) in ["abc", "orders-v2", "a-very-long-rule-name-that-is-still-valid"]
        .into_iter()
        .enumerate()
    {
        let priority = i64::try_from(index).expect("the table is small") * 10 + 1;
        let created = create_rule(
            &lattice,
            &service_id,
            &first_listener,
            fixed_response_rule(name, priority),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case first create of {name:?}: expected 201, got {} / {}",
            created.status, created.body
        );
        let first_id = created.str_field("id");

        // A different priority, so only the name collides.
        let duplicate = create_rule(
            &lattice,
            &service_id,
            &first_listener,
            fixed_response_rule(name, priority + 1000),
        )
        .await;
        assert_eq!(
            duplicate.status, 409,
            "case duplicate name {name:?} in one listener: expected 409, got {} / {}",
            duplicate.status, duplicate.body
        );
        assert_eq!(
            duplicate.error_code.as_deref(),
            Some("ConflictException"),
            "case duplicate name {name:?}: expected the modelled ConflictException, got {:?} / {}",
            duplicate.error_code,
            duplicate.body
        );

        let elsewhere = create_rule(
            &lattice,
            &service_id,
            &second_listener,
            fixed_response_rule(name, priority),
        )
        .await;
        assert_eq!(
            elsewhere.status, 201,
            "case {name:?} under a second listener: expected 201, got {} / {}",
            elsewhere.status, elsewhere.body
        );
        assert_ne!(
            elsewhere.str_field("id"),
            first_id,
            "case {name:?} under a second listener: expected a distinct rule"
        );
    }
}

/// Rule 4: `clientToken` is an `@idempotencyToken`, so a retry carrying it answers with
/// the rule the first call created rather than a second one.
#[tokio::test]
async fn a_client_token_retry_answers_with_the_first_rule() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "token-host").await;
    let listener_id = a_listener(&lattice, &service_id, "token-listener").await;

    let mut body = fixed_response_rule("token-one", 5);
    body["clientToken"] = serde_json::json!("token-abc");

    let first = create_rule(&lattice, &service_id, &listener_id, body.clone()).await;
    assert_eq!(
        first.status, 201,
        "case first create with a token: expected 201, got {} / {}",
        first.status, first.body
    );
    let retry = create_rule(&lattice, &service_id, &listener_id, body.clone()).await;
    assert_eq!(
        retry.status, 201,
        "case retry with the same token and parameters: expected 201, got {} / {}",
        retry.status, retry.body
    );
    assert_eq!(
        retry.str_field("id"),
        first.str_field("id"),
        "case retry with the same token: expected the first rule, got {}",
        retry.body
    );

    // A retry whose parameters differ is refused rather than served from the token store,
    // whichever member differs: `clientToken`'s documentation makes an identical
    // parameter set the condition for the idempotent answer.
    let diverging: Vec<(&str, serde_json::Value)> = vec![
        ("a different name", serde_json::json!({ "name": "token-two" })),
        ("a different priority", serde_json::json!({ "priority": 6 })),
        (
            "a different action",
            serde_json::json!({ "action": { "fixedResponse": { "statusCode": 500 } } }),
        ),
        (
            "a different match",
            serde_json::json!({
                "match": { "httpMatch": { "pathMatch": { "match": { "exact": "/exact" } } } },
            }),
        ),
        ("different tags", serde_json::json!({ "tags": { "team": "x" } })),
    ];
    for (case, overrides) in diverging {
        let mut altered = body.clone();
        for (member, value) in overrides.as_object().expect("each row is a JSON object") {
            altered[member.as_str()] = value.clone();
        }
        let answer = create_rule(&lattice, &service_id, &listener_id, altered).await;
        assert_eq!(
            answer.status, 409,
            "case retry with {case}: expected 409, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ConflictException"),
            "case retry with {case}: expected the modelled ConflictException, got {:?} / {}",
            answer.error_code,
            answer.body
        );
    }

    // A fresh token under the taken priority is refused by the priority rule, not
    // served from the token store.
    let mut fresh = fixed_response_rule("token-three", 5);
    fresh["clientToken"] = serde_json::json!("token-xyz");
    let fresh = create_rule(&lattice, &service_id, &listener_id, fresh).await;
    assert_eq!(
        fresh.status, 409,
        "case a new token at a taken priority: expected 409, got {} / {}",
        fresh.status, fresh.body
    );
}

/// Rule 5: an identifier that names no persisted resource, or one reached through the
/// wrong parent, is a 404 `ResourceNotFoundException`.
#[tokio::test]
async fn unknown_or_mis_parented_identifiers_are_404() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "refusal-host").await;
    let (other_service, _) = a_service(&lattice, "refusal-other").await;
    let listener_id = a_listener(&lattice, &service_id, "refusal-listener").await;
    let other_listener = a_listener(&lattice, &service_id, "refusal-other-listener").await;
    let foreign_listener = a_listener(&lattice, &other_service, "refusal-foreign").await;

    let created = create_rule(
        &lattice,
        &service_id,
        &listener_id,
        fixed_response_rule("only-rule", 1),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case fixture rule: expected 201, got {} / {}",
        created.status, created.body
    );
    let rule_id = created.str_field("id");

    // Ids of the right shape that name nothing stored.
    let absent_service = format!("svc-{:0>17}", "0");
    let absent_listener = format!("listener-{:0>17}", "0");
    let absent_rule = format!("rule-{:0>17}", "0");

    // (case, is a create, serviceIdentifier, listenerIdentifier, ruleIdentifier, the
    // kind token the message must carry, the identifier it must name)
    type Row = (&'static str, bool, String, String, String, &'static str, String);
    let cases: Vec<Row> = vec![
        (
            "create under a service that does not exist",
            true,
            absent_service.clone(),
            listener_id.clone(),
            String::new(),
            "Service",
            absent_service.clone(),
        ),
        (
            "create under a listener that does not exist",
            true,
            service_id.clone(),
            absent_listener.clone(),
            String::new(),
            "Listener",
            absent_listener.clone(),
        ),
        (
            "create under a listener of another service",
            true,
            service_id.clone(),
            foreign_listener.clone(),
            String::new(),
            "Listener",
            foreign_listener.clone(),
        ),
        (
            "read a rule that does not exist",
            false,
            service_id.clone(),
            listener_id.clone(),
            absent_rule.clone(),
            "Rule",
            absent_rule.clone(),
        ),
        (
            "read a rule through the wrong listener",
            false,
            service_id.clone(),
            other_listener.clone(),
            rule_id.clone(),
            "Rule",
            rule_id.clone(),
        ),
        (
            "read a rule through the wrong service",
            false,
            other_service.clone(),
            listener_id.clone(),
            rule_id.clone(),
            "Listener",
            listener_id.clone(),
        ),
        (
            "read through a service that does not exist",
            false,
            absent_service.clone(),
            listener_id.clone(),
            rule_id.clone(),
            "Service",
            absent_service.clone(),
        ),
        (
            "read through a listener that does not exist",
            false,
            service_id.clone(),
            absent_listener.clone(),
            rule_id.clone(),
            "Listener",
            absent_listener.clone(),
        ),
    ];

    for (case, is_create, service, listener, rule, label, named) in cases {
        let answer = if is_create {
            create_rule(
                &lattice,
                &service,
                &listener,
                fixed_response_rule("nowhere-rule", 99),
            )
            .await
        } else {
            get_rule(&lattice, &service, &listener, &rule).await
        };
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected the ResourceNotFoundException wire code, got {:?} / {}",
            answer.error_code,
            answer.body
        );
        // The not-found construction this service answers with, asserted here for the
        // rule kind the suite beside this one does not reach:
        // `"<ResourceType> <identifier> cannot be found."` over the submitted identifier.
        let expected = format!("{label} {named} cannot be found.");
        assert_eq!(
            answer.body["message"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected the not-found construction {expected:?}, got {}",
            answer.body
        );
    }
}

/// Rule 6: every refusal of malformed input is a 400 `ValidationException` — the name
/// pattern, the priority range at each published bound and one past it, and the
/// fixed-response status-code catalog.
#[tokio::test]
async fn input_outside_a_documented_or_modelled_constraint_is_400() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "validation-host").await;
    let listener_id = a_listener(&lattice, &service_id, "validation-listener").await;

    // `RuleName`'s pattern is written with negative lookahead; each row breaks one
    // clause of it.
    let forbidden_names: Vec<(&str, &str)> = vec![
        ("the reserved rule- prefix", "rule-abc"),
        ("a leading hyphen", "-api"),
        ("a trailing hyphen", "api-"),
        ("two adjacent hyphens", "a--b"),
        ("an uppercase letter", "Api"),
        ("a character outside [a-z0-9-]", "api_v1"),
    ];
    for (case, name) in forbidden_names {
        let refused = create_rule(
            &lattice,
            &service_id,
            &listener_id,
            fixed_response_rule(name, 100),
        )
        .await;
        assert_refused(&refused, &format!("{case} ({name:?})"));
    }

    // `priority`'s modelled `@range(1, 2000)`: each published bound is accepted and one
    // past each is refused.
    let priorities: Vec<(&str, i64, bool)> = vec![
        ("the published minimum", 1, true),
        ("one below the minimum", 0, false),
        ("the published maximum", 2000, true),
        ("one above the maximum", 2001, false),
    ];
    for (case, priority, accepted) in priorities {
        let answer = create_rule(
            &lattice,
            &service_id,
            &listener_id,
            fixed_response_rule(&format!("priority-{}", priority.unsigned_abs()), priority),
        )
        .await;
        if accepted {
            assert_eq!(
                answer.status, 201,
                "case {case:?} (priority {priority}): expected 201, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["priority"].as_i64(),
                Some(priority),
                "case {case:?}: expected priority {priority}, got {}",
                answer.body
            );
        } else {
            assert_refused(&answer, &format!("{case} (priority {priority})"));
        }
    }

    // `FixedResponseAction`'s `statusCode` documentation: "Only 404 and 500 status
    // codes are supported", narrower than the modelled `@range(100, 599)`.
    let status_codes: Vec<(i32, bool)> = vec![
        (404, true),
        (500, true),
        (200, false),
        (403, false),
        (503, false),
    ];
    for (index, (status_code, accepted)) in status_codes.into_iter().enumerate() {
        let priority = 1_000 + i64::try_from(index).expect("the table is small");
        let body = serde_json::json!({
            "name": format!("status-{status_code}"),
            "priority": priority,
            "match": { "httpMatch": {} },
            "action": { "fixedResponse": { "statusCode": status_code } },
        });
        let answer = create_rule(&lattice, &service_id, &listener_id, body).await;
        if accepted {
            assert_eq!(
                answer.status, 201,
                "case status code {status_code}: expected 201, got {} / {}",
                answer.status, answer.body
            );
        } else {
            assert_refused(&answer, &format!("status code {status_code}"));
        }
    }
}

/// A refusal is HTTP 400 carrying the modelled `ValidationException` wire code.
pub(crate) fn assert_refused(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected 400, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ValidationException"),
        "case {case}: expected the modelled ValidationException, got {:?} / {}",
        answer.error_code,
        answer.body
    );
}

/// Rule 7: "If you delete a service, all resources related to the service, such as …
/// listeners, listener rules, and access log subscriptions, are also deleted"
/// (`DeleteService`'s documentation), so no rule outlives the service that holds it.
#[tokio::test]
async fn deleting_a_service_deletes_its_listener_rules() {
    let lattice = Lattice::new();
    let (doomed, _) = a_service(&lattice, "doomed-rule-host").await;
    let (survivor, _) = a_service(&lattice, "surviving-rule-host").await;
    let doomed_listener = a_listener(&lattice, &doomed, "doomed-listener").await;
    let kept_listener = a_listener(&lattice, &survivor, "kept-listener").await;

    let mut doomed_rules = Vec::new();
    for (index, name) in ["first", "second"].into_iter().enumerate() {
        let priority = i64::try_from(index).expect("the table is small") + 1;
        let created = create_rule(
            &lattice,
            &doomed,
            &doomed_listener,
            fixed_response_rule(name, priority),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case rule {name:?}: expected 201, got {} / {}",
            created.status, created.body
        );
        doomed_rules.push(created.str_field("id"));
    }
    // Same name and same priority, under the service that stays.
    let kept = create_rule(
        &lattice,
        &survivor,
        &kept_listener,
        fixed_response_rule("first", 1),
    )
    .await;
    let kept_id = kept.str_field("id");

    let deleted = lattice
        .call("DELETE", &format!("/services/{doomed}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case delete the service: expected 200, got {} / {}",
        deleted.status, deleted.body
    );

    for id in &doomed_rules {
        let read = get_rule(&lattice, &doomed, &doomed_listener, id).await;
        assert_eq!(
            read.status, 404,
            "case rule {id} of a deleted service: expected 404, got {} / {}",
            read.status, read.body
        );
    }

    let still_there = get_rule(&lattice, &survivor, &kept_listener, &kept_id).await;
    assert_eq!(
        still_there.status, 200,
        "case the surviving service's rule: expected 200, got {} / {}",
        still_there.status, still_there.body
    );
}
