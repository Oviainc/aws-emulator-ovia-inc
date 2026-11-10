// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a listener's implicit DEFAULT rule and for `UpdateRule`.
//!
//! Every case drives the emulator the way a client does — a restJson1 request through the
//! router that mounts the service — and reads what it asserts back out of a LATER request
//! (`ListRules`, `GetRule`), so nothing here can pass on a canned body: a default rule the
//! create did not persist fails the read-back, and an update echoed from the request
//! instead of stored fails the read that follows it.
//!
//! The harness (`Lattice`, `Answer`, `as_label`, `expected_arn`, …) and the service /
//! listener / rule fixtures are the ones the suites beside this file already build.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! 1. "Each listener has a default rule" (`CreateListener`'s `defaultAction`; `DeleteRule`
//!    states it as "Each listener has a default rule for checking connection requests, but
//!    you can define additional rules"), so creating a listener materialises that rule:
//!    `ListRules` on a listener nothing has created a rule on reports exactly it, and
//!    `GetRule` reads it back — `isDefault` true, the default name, a priority above every
//!    one `RulePriority` lets a caller ask for, the listener's own `defaultAction` as its
//!    action, a `match` whose `headerMatches` is a present list, an ARN nested under its
//!    listener's and both timestamps. Each row is a listener differing in protocol, port
//!    and default-action kind; the default rule of one listener is never the default rule
//!    of another.
//! 2. A default rule takes part in the per-listener namespaces: its name is taken, and its
//!    priority is out of reach of a create, so a rule at either published bound of
//!    `@range(1, 2000)` is accepted beside it.
//! 3. `UpdateRule` applies the members the request supplies onto the stored rule and
//!    leaves the rest, so `GetRule` and `ListRules` report the NEW values afterwards and
//!    `lastUpdatedAt` moves to the call that changed them while `createdAt` stays. Rows
//!    vary which members are supplied (priority only at each published bound and in
//!    between, match only — with and without header matches — action only, all three at
//!    once, and none at all).
//! 4. "Each rule for a specific listener must have a unique priority": an update onto a
//!    priority another rule of the listener holds is a 409 `ConflictException` and stores
//!    nothing, while an update onto the priority the rule already holds is served.
//! 5. "You can't modify a default listener rule" (`UpdateRule`'s documentation), so an
//!    update naming the listener's default rule is a 400 `ValidationException` and changes
//!    nothing.
//! 6. An identifier naming no persisted service, listener or rule is a 404
//!    `ResourceNotFoundException` carrying this service's not-found construction —
//!    including a rule updated through the wrong listener or the wrong service — and an
//!    action outside the documented fixed-response catalog is a 400.
//! 7. A RULE's and a TARGET GROUP's minted id draws its 17-character suffix from
//!    lowercase hex, as every minted id of this service does (`minted_id_alphabet.rs`
//!    asserts that over every kind).

use crate::batch_update_rule::instant_of;
use crate::list_rules::{default_rule_id, items_of, list_page};
use crate::listeners::{a_service, create_listener, target_group};
use crate::resource_not_found_messages::absent_id;
use crate::rules::{assert_refused, create_rule, fixed_response_rule, get_rule, rule_kind};
use crate::target_groups::create_target_group;
use crate::vpclattice_service_slice::{
    as_label, expected_arn, is_lattice_id, is_timestamp, message_of, Answer, Lattice,
};
use service_com_amazonaws_vpclattice_mercurycontrolplane::state;

/// `PATCH /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules/{ruleIdentifier}`.
async fn update_rule(
    lattice: &Lattice,
    service_identifier: &str,
    listener_identifier: &str,
    rule_identifier: &str,
    body: serde_json::Value,
) -> Answer {
    lattice
        .call(
            "PATCH",
            &format!(
                "/services/{service_identifier}/listeners/{listener_identifier}/rules/{rule_identifier}"
            ),
            Some(body),
        )
        .await
}

/// Create a listener with the body a row describes, answering its id.
async fn a_listener_with(lattice: &Lattice, service_identifier: &str, body: serde_json::Value) -> String {
    let created = create_listener(lattice, service_identifier, body.clone()).await;
    assert_eq!(
        created.status, 201,
        "case fixture listener {body}: expected 201, got {} / {}",
        created.status, created.body
    );
    created.str_field("id")
}

/// Rule 1: every listener is created WITH its default rule, whatever the listener looks
/// like, and that rule is readable through `GetRule` exactly as the listing reports it.
#[tokio::test]
async fn every_created_listener_holds_its_default_rule() {
    let lattice = Lattice::new();
    let (service_id, service_arn) = a_service(&lattice, "default-rule-host").await;

    // Each row is a listener that differs in protocol, in whether it sends a port, and in
    // the kind and shape of the `defaultAction` the default rule carries.
    type Row = (&'static str, serde_json::Value, serde_json::Value);
    let cases: Vec<Row> = vec![
        (
            "HTTP on its documented default port, fixed response 404",
            serde_json::json!({
                "name": "default-rule-http",
                "protocol": "HTTP",
                "defaultAction": { "fixedResponse": { "statusCode": 404 } },
            }),
            serde_json::json!({ "fixedResponse": { "statusCode": 404 } }),
        ),
        (
            "HTTPS on an explicit port, fixed response 500",
            serde_json::json!({
                "name": "default-rule-https",
                "protocol": "HTTPS",
                "port": 8443,
                "defaultAction": { "fixedResponse": { "statusCode": 500 } },
            }),
            serde_json::json!({ "fixedResponse": { "statusCode": 500 } }),
        ),
        (
            "TLS_PASSTHROUGH forwarding to one target group",
            serde_json::json!({
                "name": "default-rule-passthrough",
                "protocol": "TLS_PASSTHROUGH",
                "port": 9443,
                "defaultAction": {
                    "forward": {
                        "targetGroups": [{ "targetGroupIdentifier": target_group("1") }],
                    },
                },
            }),
            // `weight`'s own documentation: "If there's only one target group specified,
            // then the default value is 100."
            serde_json::json!({
                "forward": {
                    "targetGroups": [
                        { "targetGroupIdentifier": target_group("1"), "weight": 100 },
                    ],
                },
            }),
        ),
        (
            "HTTP forwarding to two weighted target groups",
            serde_json::json!({
                "name": "default-rule-weighted",
                "protocol": "HTTP",
                "port": 8080,
                "defaultAction": {
                    "forward": {
                        "targetGroups": [
                            { "targetGroupIdentifier": target_group("2"), "weight": 30 },
                            { "targetGroupIdentifier": target_group("3"), "weight": 70 },
                        ],
                    },
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
    ];

    let mut default_ids: Vec<String> = Vec::new();
    for (case, listener_body, expected_action) in cases {
        let listener_id = a_listener_with(&lattice, &service_id, listener_body).await;

        // The listing of a listener nothing has created a rule on carries exactly one
        // summary: its default rule.
        let listed = list_page(&lattice, &service_id, &listener_id, &[]).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            listed.status, listed.body
        );
        let items = items_of(case, &listed);
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: expected the listener's one default rule, got {}",
            listed.body
        );
        let summary = &items[0];
        assert_eq!(
            summary["isDefault"],
            serde_json::json!(true),
            "case {case:?}: expected isDefault true, got {summary}"
        );
        assert_eq!(
            summary["name"].as_str(),
            Some(state::DEFAULT_RULE_NAME),
            "case {case:?}: expected the default rule's name {:?}, got {summary}",
            state::DEFAULT_RULE_NAME
        );
        assert_eq!(
            summary["priority"].as_i64(),
            Some(i64::from(state::DEFAULT_RULE_PRIORITY)),
            "case {case:?}: expected the default rule's priority {}, got {summary}",
            state::DEFAULT_RULE_PRIORITY
        );
        // The default rule is evaluated last, so its priority is beyond the range a
        // caller may submit (`RulePriority`'s `@range(1, 2000)`).
        assert!(
            i64::from(state::DEFAULT_RULE_PRIORITY) > 2000,
            "case {case:?}: the default rule's priority must be out of a caller's reach, \
             got {}",
            state::DEFAULT_RULE_PRIORITY
        );

        let rule_id = summary["id"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: a default rule summary without an id: {summary}")
        });
        assert!(
            is_lattice_id(rule_id, state::PREFIX_RULE),
            "case {case:?}: expected a modelled rule- id, got {rule_id:?}"
        );
        assert_eq!(
            summary["arn"].as_str().unwrap_or_default(),
            expected_arn(&rule_kind(&service_id, &listener_id), rule_id),
            "case {case:?}: the default rule's ARN must nest under its listener's, got \
             {summary}"
        );
        for member in ["createdAt", "lastUpdatedAt"] {
            assert!(
                is_timestamp(&summary[member]),
                "case {case:?}: {member} must carry an instant, got {summary}"
            );
        }

        // The record is PERSISTED: `GetRule` reads it back through either identifier form,
        // with the listener's own `defaultAction` as its action and a materialised match.
        let arn = summary["arn"].as_str().unwrap_or_default().to_string();
        for (form, identifier) in [("the bare id", rule_id.to_string()), ("the ARN", as_label(&arn))] {
            let read = get_rule(&lattice, &service_id, &listener_id, &identifier).await;
            assert_eq!(
                read.status, 200,
                "case {case:?} read by {form}: expected 200, got {} / {}",
                read.status, read.body
            );
            assert_eq!(
                read.str_field("id"),
                rule_id,
                "case {case:?} read by {form}: expected id {rule_id}, got {}",
                read.body
            );
            assert_eq!(
                read.body["isDefault"].as_bool(),
                Some(true),
                "case {case:?} read by {form}: expected isDefault true, got {}",
                read.body
            );
            assert_eq!(
                read.body["action"], expected_action,
                "case {case:?} read by {form}: expected the listener's defaultAction \
                 {expected_action}, got {}",
                read.body
            );
            assert!(
                read.body["match"]["httpMatch"]["headerMatches"].is_array(),
                "case {case:?} read by {form}: match.httpMatch.headerMatches must be a \
                 present list, got {}",
                read.body
            );
        }

        // Reading the service by its ARN form reaches the same rule, so the default rule
        // is scoped to its listener's service and not to the request's spelling of it.
        let through_arn =
            get_rule(&lattice, &as_label(&service_arn), &listener_id, rule_id).await;
        assert_eq!(
            through_arn.status, 200,
            "case {case:?} through the service ARN: expected 200, got {} / {}",
            through_arn.status, through_arn.body
        );

        default_ids.push(rule_id.to_string());
    }

    // One default rule per listener, never one shared between them.
    let mut unique = default_ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        default_ids.len(),
        "case one default rule per listener: expected distinct rule ids, got {default_ids:?}"
    );
}

/// Rule 2: the default rule holds its name and its priority in the listener's namespaces,
/// and a create at either published bound of `@range(1, 2000)` still fits beside it.
#[tokio::test]
async fn the_default_rule_occupies_its_name_and_leaves_every_callable_priority_free() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "default-rule-namespace").await;
    let listener_id = a_listener_with(
        &lattice,
        &service_id,
        serde_json::json!({
            "name": "namespace-listener",
            "protocol": "HTTP",
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;
    let default_id = default_rule_id(&lattice, &service_id, &listener_id).await;

    // "The name must be unique within the listener" (`CreateRule`'s `name`), and the
    // default rule holds the default name.
    let clash = create_rule(
        &lattice,
        &service_id,
        &listener_id,
        fixed_response_rule(state::DEFAULT_RULE_NAME, 10),
    )
    .await;
    assert_eq!(
        clash.status, 409,
        "case a create under the default rule's name: expected 409, got {} / {}",
        clash.status, clash.body
    );
    assert_eq!(
        clash.body["resourceId"].as_str(),
        Some(default_id.as_str()),
        "case a create under the default rule's name: expected the conflict over the \
         default rule {default_id}, got {}",
        clash.body
    );

    // Each published bound of the priority a caller may submit, and one in between: none
    // of them is the default rule's, so each is accepted.
    let mut beside: Vec<String> = Vec::new();
    for priority in [1_i64, 1000, 2000] {
        let created = create_rule(
            &lattice,
            &service_id,
            &listener_id,
            fixed_response_rule(&format!("beside-default-{priority}"), priority),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case a create at priority {priority} beside the default rule: expected 201, \
             got {} / {}",
            created.status, created.body
        );
        beside.push(created.str_field("id"));
    }

    // The default rule's priority is not a value a caller can even submit: it is past
    // `RulePriority`'s `@range(1, 2000)`, so an update asking for it is refused.
    let past_the_bound = update_rule(
        &lattice,
        &service_id,
        &listener_id,
        &beside[0],
        serde_json::json!({ "priority": state::DEFAULT_RULE_PRIORITY }),
    )
    .await;
    assert_refused(&past_the_bound, "an update onto the default rule's priority");
}

/// Rule 3: an update is persisted — whatever subset of members it supplies.
#[tokio::test]
async fn an_update_is_persisted_and_read_back_by_get_rule() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "update-rule-host").await;
    let listener_id = a_listener_with(
        &lattice,
        &service_id,
        serde_json::json!({
            "name": "update-rule-listener",
            "protocol": "HTTP",
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;

    // The match `fixed_response_rule` creates a rule with, as the rule reports it.
    let created_match = serde_json::json!({
        "httpMatch": {
            "pathMatch": { "match": { "prefix": "/" } },
            "headerMatches": [],
        },
    });
    let created_action = serde_json::json!({ "fixedResponse": { "statusCode": 404 } });

    // (case, the update body, the priority / match / action the rule must report after it)
    type Row = (
        &'static str,
        serde_json::Value,
        i64,
        serde_json::Value,
        serde_json::Value,
    );
    let cases: Vec<Row> = vec![
        (
            "priority only, at the lowest value the model allows",
            serde_json::json!({ "priority": 1 }),
            1,
            created_match.clone(),
            created_action.clone(),
        ),
        (
            "priority only, in between the published bounds",
            serde_json::json!({ "priority": 15 }),
            15,
            created_match.clone(),
            created_action.clone(),
        ),
        (
            "priority only, at the highest value the model allows",
            serde_json::json!({ "priority": 2000 }),
            2000,
            created_match.clone(),
            created_action.clone(),
        ),
        (
            "match only, with no header match configured",
            serde_json::json!({
                "match": { "httpMatch": { "method": "POST" } },
            }),
            10,
            serde_json::json!({ "httpMatch": { "method": "POST", "headerMatches": [] } }),
            created_action.clone(),
        ),
        (
            "match only, with header matches",
            serde_json::json!({
                "match": {
                    "httpMatch": {
                        "pathMatch": { "match": { "exact": "/orders" } },
                        "headerMatches": [{ "name": "x-version", "match": { "exact": "v2" } }],
                    },
                },
            }),
            10,
            serde_json::json!({
                "httpMatch": {
                    "pathMatch": { "match": { "exact": "/orders" } },
                    "headerMatches": [{ "name": "x-version", "match": { "exact": "v2" } }],
                },
            }),
            created_action.clone(),
        ),
        (
            "action only",
            serde_json::json!({
                "action": {
                    "forward": {
                        "targetGroups": [{ "targetGroupIdentifier": target_group("4") }],
                    },
                },
            }),
            10,
            created_match.clone(),
            serde_json::json!({
                "forward": {
                    "targetGroups": [
                        { "targetGroupIdentifier": target_group("4"), "weight": 100 },
                    ],
                },
            }),
        ),
        (
            "every member at once",
            serde_json::json!({
                "priority": 42,
                "match": { "httpMatch": {} },
                "action": { "fixedResponse": { "statusCode": 500 } },
            }),
            42,
            serde_json::json!({ "httpMatch": { "headerMatches": [] } }),
            serde_json::json!({ "fixedResponse": { "statusCode": 500 } }),
        ),
        (
            "no member at all",
            serde_json::json!({}),
            10,
            created_match.clone(),
            created_action.clone(),
        ),
    ];

    for (index, (case, body, expected_priority, expected_match, expected_action)) in
        cases.into_iter().enumerate()
    {
        // One rule per row, created the same way, so each row starts from one state.
        let name = format!("update-target-{index}");
        let created = create_rule(
            &lattice,
            &service_id,
            &listener_id,
            fixed_response_rule(&name, 10),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected the fixture rule to be created, got {} / {}",
            created.status, created.body
        );
        let rule_id = created.str_field("id");
        let before = get_rule(&lattice, &service_id, &listener_id, &rule_id).await;
        let created_at = instant_of(case, &before.body, "createdAt");
        let updated_before = instant_of(case, &before.body, "lastUpdatedAt");

        let answer = update_rule(&lattice, &service_id, &listener_id, &rule_id, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status, answer.body
        );
        // The answer is the stored rule, not an echo of the request: it carries the
        // members the update never mentioned.
        assert_eq!(
            answer.str_field("id"),
            rule_id,
            "case {case:?}: expected id {rule_id}, got {}",
            answer.body
        );
        assert_eq!(
            answer.str_field("arn"),
            expected_arn(&rule_kind(&service_id, &listener_id), &rule_id),
            "case {case:?}: expected the ARN nested in its listener's, got {}",
            answer.body
        );
        assert_eq!(
            answer.body["name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected name {name}, got {}",
            answer.body
        );
        assert_eq!(
            answer.body["isDefault"].as_bool(),
            Some(false),
            "case {case:?}: a rule CreateRule minted is not the listener's default rule, \
             got {}",
            answer.body
        );

        // What the UPDATE answered and what a later read answers must agree, member by
        // member — the read is what proves the write reached state.
        let read_back = get_rule(&lattice, &service_id, &listener_id, &rule_id).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: expected 200 on the read-back, got {} / {}",
            read_back.status, read_back.body
        );
        for (where_, reported) in [
            ("the update", &answer.body),
            ("the read", &read_back.body),
        ] {
            assert_eq!(
                reported["priority"].as_i64(),
                Some(expected_priority),
                "case {case:?} in {where_}: expected priority {expected_priority}, got {reported}"
            );
            assert_eq!(
                reported["match"], expected_match,
                "case {case:?} in {where_}: expected the stored match {expected_match}, got \
                 {reported}"
            );
            assert_eq!(
                reported["action"], expected_action,
                "case {case:?} in {where_}: expected the stored action {expected_action}, got \
                 {reported}"
            );
        }

        // `createdAt` belongs to the create; `lastUpdatedAt` moves to the call that
        // changed the rule, and stays put for an update that changed nothing.
        let read = read_back;
        assert_eq!(
            instant_of(case, &read.body, "createdAt"),
            created_at,
            "case {case:?}: createdAt must not move, got {}",
            read.body
        );
        let updated_after = instant_of(case, &read.body, "lastUpdatedAt");
        assert!(
            updated_after >= updated_before,
            "case {case:?}: lastUpdatedAt must not go backwards, got {updated_after} after \
             {updated_before}"
        );

        // `ListRules` reports the same priority the read does, so the listing follows the
        // same record.
        let listed = list_page(&lattice, &service_id, &listener_id, &[]).await;
        let summary = items_of(case, &listed)
            .into_iter()
            .find(|item| item["id"].as_str() == Some(rule_id.as_str()))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the updated rule is missing from {}", listed.body)
            });
        assert_eq!(
            summary["priority"].as_i64(),
            Some(expected_priority),
            "case {case:?} in the listing: expected priority {expected_priority}, got {summary}"
        );

        // Each row leaves its rule at a priority of its own, so the next row's fixture
        // create at priority 10 is free: move this one out of the way when it stayed.
        if expected_priority == 10 {
            let moved = update_rule(
                &lattice,
                &service_id,
                &listener_id,
                &rule_id,
                serde_json::json!({ "priority": 1500 + i64::try_from(index).unwrap_or(0) }),
            )
            .await;
            assert_eq!(
                moved.status, 200,
                "case {case:?}: expected the row's rule to be moved aside, got {} / {}",
                moved.status, moved.body
            );
        }
    }
}

/// Rule 4: "Each rule for a specific listener must have a unique priority", checked over
/// the whole listener on update.
#[tokio::test]
async fn an_update_onto_a_taken_priority_is_refused_and_stores_nothing() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "update-priority-host").await;
    let listener_id = a_listener_with(
        &lattice,
        &service_id,
        serde_json::json!({
            "name": "update-priority-listener",
            "protocol": "HTTP",
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;
    let sibling_listener = a_listener_with(
        &lattice,
        &service_id,
        serde_json::json!({
            "name": "update-priority-sibling",
            "protocol": "HTTP",
            "port": 8081,
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;

    let mut ids = Vec::new();
    for (name, priority) in [("taken-one", 5_i64), ("taken-two", 6)] {
        let created = create_rule(
            &lattice,
            &service_id,
            &listener_id,
            fixed_response_rule(name, priority),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case fixture rule {name}: expected 201, got {} / {}",
            created.status, created.body
        );
        ids.push(created.str_field("id"));
    }
    // A rule of ANOTHER listener at one of those priorities, so the check is scoped to the
    // listener rather than to the whole store.
    let elsewhere = create_rule(
        &lattice,
        &service_id,
        &sibling_listener,
        fixed_response_rule("elsewhere", 5),
    )
    .await;
    assert_eq!(
        elsewhere.status, 201,
        "case a rule of another listener at a taken priority: expected 201, got {} / {}",
        elsewhere.status, elsewhere.body
    );

    // (case, the rule to update, the priority it asks for, whether the service serves it)
    let cases: Vec<(&str, &String, i64, bool)> = vec![
        ("onto the priority a sibling rule holds", &ids[0], 6, false),
        ("onto the priority it already holds", &ids[0], 5, true),
        ("onto a free priority", &ids[0], 7, true),
        (
            // The uniqueness rule is scoped to one listener, so a priority only another
            // listener's rule holds is free here.
            "onto a priority only a rule of another listener holds",
            &ids[1],
            5,
            true,
        ),
        (
            "onto the priority the sibling rule now holds",
            &ids[1],
            7,
            false,
        ),
    ];

    for (case, rule_id, priority, served) in cases {
        let before = get_rule(&lattice, &service_id, &listener_id, rule_id).await;
        let answer = update_rule(
            &lattice,
            &service_id,
            &listener_id,
            rule_id,
            serde_json::json!({ "priority": priority }),
        )
        .await;
        if served {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["priority"].as_i64(),
                Some(priority),
                "case {case:?}: expected priority {priority}, got {}",
                answer.body
            );
            continue;
        }
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected 409, got {} / {}",
            answer.status, answer.body
        );
        assert!(
            answer
                .error_code
                .as_deref()
                .is_some_and(|code| code.contains("ConflictException")),
            "case {case:?}: expected ConflictException, got {:?} / {}",
            answer.error_code,
            answer.body
        );
        // A refused update stores nothing.
        let after = get_rule(&lattice, &service_id, &listener_id, rule_id).await;
        assert_eq!(
            after.body["priority"], before.body["priority"],
            "case {case:?}: a refused update must leave the stored priority {}, got {}",
            before.body["priority"], after.body
        );
    }
}

/// Rule 5: "You can't modify a default listener rule. To modify a default listener rule,
/// use UpdateListener." (`UpdateRule`'s own documentation).
#[tokio::test]
async fn an_update_of_the_default_rule_is_refused_and_changes_nothing() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "default-rule-update-host").await;
    let listener_id = a_listener_with(
        &lattice,
        &service_id,
        serde_json::json!({
            "name": "default-rule-update-listener",
            "protocol": "HTTP",
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;
    let default_id = default_rule_id(&lattice, &service_id, &listener_id).await;
    let before = get_rule(&lattice, &service_id, &listener_id, &default_id).await;

    // Each row is a different member set — none of them may reach the default rule.
    for (case, body) in [
        ("a priority", serde_json::json!({ "priority": 10 })),
        (
            "a match",
            serde_json::json!({ "match": { "httpMatch": { "method": "GET" } } }),
        ),
        (
            "an action",
            serde_json::json!({ "action": { "fixedResponse": { "statusCode": 500 } } }),
        ),
        ("no member at all", serde_json::json!({})),
    ] {
        let answer = update_rule(&lattice, &service_id, &listener_id, &default_id, body).await;
        assert_refused(&answer, &format!("updating the default rule with {case}"));
        assert_eq!(
            message_of(case, &answer),
            state::DEFAULT_RULE_NOT_MODIFIABLE_MESSAGE,
            "case updating the default rule with {case}: expected the rule's own sentence, \
             got {}",
            answer.body
        );
        let after = get_rule(&lattice, &service_id, &listener_id, &default_id).await;
        assert_eq!(
            after.body, before.body,
            "case updating the default rule with {case}: the default rule must be unchanged, \
             got {}",
            after.body
        );
    }
}

/// Rule 6: a request this operation refuses carries the modelled error — never a 501 and
/// never a silent success.
#[tokio::test]
async fn a_refused_update_carries_the_modelled_error() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "update-refusal-host").await;
    let listener_id = a_listener_with(
        &lattice,
        &service_id,
        serde_json::json!({
            "name": "update-refusal-listener",
            "protocol": "HTTP",
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;
    let (other_service_id, _) = a_service(&lattice, "update-refusal-other").await;
    let other_listener = a_listener_with(
        &lattice,
        &other_service_id,
        serde_json::json!({
            "name": "update-refusal-other-listener",
            "protocol": "HTTP",
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;
    let created = create_rule(
        &lattice,
        &service_id,
        &listener_id,
        fixed_response_rule("refusal-target", 3),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case fixture rule: expected 201, got {} / {}",
        created.status, created.body
    );
    let rule_id = created.str_field("id");

    // The not-found construction, written out here rather than read from the service, so
    // no row can pass by agreeing with the code under test.
    let not_found_message = |label: &str, identifier: &str| format!("{label} {identifier} cannot be found.");

    // (case, service identifier, listener identifier, rule identifier, the kind the
    // message names)
    let cases: Vec<(&str, String, String, String, &str)> = vec![
        (
            "a service id nothing created",
            absent_id("svc-", "0"),
            listener_id.clone(),
            rule_id.clone(),
            "Service",
        ),
        (
            "a listener id nothing created",
            service_id.clone(),
            absent_id("listener-", "1"),
            rule_id.clone(),
            "Listener",
        ),
        (
            "a rule id nothing created",
            service_id.clone(),
            listener_id.clone(),
            absent_id("rule-", "2"),
            "Rule",
        ),
        (
            "the rule reached through a listener of another service",
            other_service_id.clone(),
            other_listener.clone(),
            rule_id.clone(),
            "Rule",
        ),
        (
            "the rule reached through the wrong service",
            other_service_id.clone(),
            listener_id.clone(),
            rule_id.clone(),
            "Listener",
        ),
    ];

    for (case, service_identifier, listener_identifier, rule_identifier, label) in cases {
        let answer = update_rule(
            &lattice,
            &service_identifier,
            &listener_identifier,
            &rule_identifier,
            serde_json::json!({ "priority": 11 }),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
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
        let identifier = match label {
            "Service" => &service_identifier,
            "Listener" => &listener_identifier,
            _ => &rule_identifier,
        };
        assert_eq!(
            message_of(case, &answer),
            not_found_message(label, identifier),
            "case {case:?}: expected the not-found construction over the submitted \
             identifier, got {}",
            answer.body
        );
    }

    // "Only `404` and `500` status codes are supported" (`FixedResponseAction`'s
    // `statusCode`), so an update carrying any other one is refused — the modelled
    // `@range(100, 599)` is wider than the documented catalog.
    for status_code in [200_i64, 503] {
        let answer = update_rule(
            &lattice,
            &service_id,
            &listener_id,
            &rule_id,
            serde_json::json!({ "action": { "fixedResponse": { "statusCode": status_code } } }),
        )
        .await;
        assert_refused(&answer, &format!("an update to status code {status_code}"));
    }

    // A refused update stored nothing: the rule still carries what the create gave it.
    let read = get_rule(&lattice, &service_id, &listener_id, &rule_id).await;
    assert_eq!(
        read.body["priority"].as_i64(),
        Some(3),
        "case after the refusals: expected the created priority 3, got {}",
        read.body
    );
}

/// Rule 7: which alphabet a minted id's 17-character suffix is drawn from.
///
/// A rule's and a target group's suffix is lowercase hex, and so is the suffix of the
/// service and the listener they hang off. The rows span the kinds and, for a rule,
/// several mints of one listener plus the default rule the create materialised.
#[tokio::test]
async fn a_rule_and_a_target_groups_minted_id_is_hex() {
    /// Whether `id`'s suffix after `prefix` is 17 lowercase-hex characters.
    fn is_hex_id(id: &str, prefix: &str) -> bool {
        id.strip_prefix(prefix).is_some_and(|suffix| {
            suffix.len() == 17 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
                && suffix.bytes().all(|byte| !byte.is_ascii_uppercase())
        })
    }

    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "hex-id-host").await;
    let listener_id = a_listener_with(
        &lattice,
        &service_id,
        serde_json::json!({
            "name": "hex-id-listener",
            "protocol": "HTTP",
            "defaultAction": { "fixedResponse": { "statusCode": 404 } },
        }),
    )
    .await;

    // Every rule of the listener: the default rule the create materialised, and three
    // mints of CreateRule.
    let mut rule_ids = vec![default_rule_id(&lattice, &service_id, &listener_id).await];
    for priority in [1_i64, 2, 3] {
        let created = create_rule(
            &lattice,
            &service_id,
            &listener_id,
            fixed_response_rule(&format!("hex-rule-{priority}"), priority),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case rule at priority {priority}: expected 201, got {} / {}",
            created.status, created.body
        );
        rule_ids.push(created.str_field("id"));
    }
    for rule_id in &rule_ids {
        assert!(
            is_hex_id(rule_id, state::PREFIX_RULE),
            "case rule id {rule_id:?}: expected rule- plus 17 lowercase-hex characters"
        );
    }

    // A target group's id is minted from the same alphabet.
    for name in ["hex-tg-one", "hex-tg-two"] {
        let created = create_target_group(
            &lattice,
            serde_json::json!({ "name": name, "type": "LAMBDA" }),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case target group {name:?}: expected 201, got {} / {}",
            created.status, created.body
        );
        let id = created.str_field("id");
        assert!(
            is_hex_id(&id, state::PREFIX_TARGET_GROUP),
            "case target-group id {id:?}: expected tg- plus 17 lowercase-hex characters"
        );
    }

    // The service's and the listener's own ids are minted from the same alphabet, and
    // satisfy the wider modelled pattern as well.
    for (case, id, prefix) in [
        ("the service", service_id.clone(), state::PREFIX_SERVICE),
        ("the listener", listener_id.clone(), state::PREFIX_LISTENER),
    ] {
        assert!(
            is_lattice_id(&id, prefix),
            "case {case}: expected a modelled {prefix} id, got {id:?}"
        );
        assert!(
            is_hex_id(&id, prefix),
            "case {case}: expected {prefix} plus 17 lowercase-hex characters, got {id:?}"
        );
    }
}
