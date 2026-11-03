// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchUpdateRule`, and for the match a rule reports.
//!
//! Every case drives the emulator the way a client does — a restJson1 request through the
//! router that mounts the service — and reads the updated rule back out of a LATER
//! `GetRule`, so nothing here can pass on a canned body: an entry the handler only echoes
//! fails the read-back, and a batch that persists nothing fails every priority assertion.
//!
//! The harness (`Lattice`, `Answer`, `as_label`, `expected_arn`, …) and the service /
//! listener / rule fixtures are the ones the suites beside this file already build.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! 1. An accepted entry is PERSISTED: the `priority`, `match` and `action` it supplies
//!    become the stored rule's, a member it leaves out is left alone, the rule's
//!    `lastUpdatedAt` moves to the call that changed it, and the entry reported under
//!    `successful` carries exactly what `GetRule` answers with afterwards.
//! 2. A rule's HTTP match is reported with `headerMatches` present — the empty list when
//!    no header match is configured, the submitted entries when some are — whether the
//!    match was stored by the create or by an update.
//! 3. "A listener can't have multiple rules with the same priority" (`RuleUpdate`'s
//!    `priority` documentation) is checked over the whole batch, so swapping or rotating
//!    the priorities of rules that are all in the batch is accepted, while a batch that
//!    would leave two rules of the listener at one priority is refused with the modelled
//!    409 `ConflictException` and stores nothing.
//! 4. A partial failure keeps the successful updates: an entry naming no rule of this
//!    listener is reported under `unsuccessful` with the code and message the single-rule
//!    path refuses it with, while its sibling entries are applied. A value a documented
//!    service rule refuses is refused for the whole call instead, the way the single-rule
//!    path refuses it.
//! 5. A `serviceIdentifier` or `listenerIdentifier` naming nothing persisted is a 404
//!    `ResourceNotFoundException`; both modelled identifier forms (the bare id and the
//!    ARN) reach the same listener and the same rule.
//! 6. The batch is bounded by what the model publishes for it: a `rules` list whose
//!    length `RuleUpdateList`'s `@length(1, 10)` admits is applied in full, and one past
//!    either published bound is a 400 `ValidationException` that stores nothing — as is
//!    an entry whose `priority` falls past either bound of `RuleUpdate`'s
//!    `@range(1, 2000)`.
//! 7. "You can't modify a default listener rule. To modify a default listener rule, use
//!    UpdateListener" (`UpdateRule`'s documentation, the rule for an update whichever
//!    path carries it): an entry naming the listener's default rule is refused for the
//!    whole call and stores nothing, as the single-rule path refuses it.

use crate::list_rules::default_rule_id;
use crate::listeners::{a_service, listener_kind, target_group};
use crate::resource_not_found_messages::absent_id;
use crate::rules::{
    a_listener, assert_refused, create_rule, fixed_response_rule, get_rule, rule_kind,
};
use crate::vpclattice_service_slice::{
    as_label, expected_arn, is_timestamp, message_of, Answer, Lattice,
};

/// `PATCH /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules`.
async fn batch_update(
    lattice: &Lattice,
    service_identifier: &str,
    listener_identifier: &str,
    rules: serde_json::Value,
) -> Answer {
    lattice
        .call(
            "PATCH",
            &format!("/services/{service_identifier}/listeners/{listener_identifier}/rules"),
            Some(serde_json::json!({ "rules": rules })),
        )
        .await
}

/// The `successful` list of a 200 answer, which carries one entry per applied update.
fn successful_of(case: &str, answer: &Answer) -> Vec<serde_json::Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer.body["successful"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a successful list in {}",
                answer.body
            )
        })
}

/// The `unsuccessful` list of a 200 answer, which carries one entry per refused update.
fn unsuccessful_of(case: &str, answer: &Answer) -> Vec<serde_json::Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    answer.body["unsuccessful"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an unsuccessful list in {}",
                answer.body
            )
        })
}

/// One timestamp member of a rule, as the instant it names, so two reads can be ordered.
pub(crate) fn instant_of(case: &str, body: &serde_json::Value, member: &str) -> f64 {
    let text = body[member].as_str().unwrap_or_else(|| {
        panic!("case {case:?}: expected the instant member {member} in {body}")
    });
    ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {member} must be RFC 3339 ({e}), got {text:?}"))
        .as_secs_f64()
}

/// Rule 2's assertion, applied to every body that carries a rule's `match`.
fn assert_header_matches_present(case: &str, where_: &str, body: &serde_json::Value) {
    assert!(
        body["match"]["httpMatch"]["headerMatches"].is_array(),
        "case {case:?} in {where_}: match.httpMatch.headerMatches must be a present list, got {}",
        body["match"]
    );
}

/// Create one rule to update, answering its id.
async fn a_rule(lattice: &Lattice, service_id: &str, listener_id: &str, name: &str, priority: i64) -> String {
    let created = create_rule(
        lattice,
        service_id,
        listener_id,
        fixed_response_rule(name, priority),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case fixture rule {name:?}: expected 201, got {} / {}",
        created.status, created.body
    );
    created.str_field("id")
}

/// The match `fixed_response_rule` creates a rule with, as the rule reports it: the
/// submitted path match plus the materialised header-match list.
fn created_match() -> serde_json::Value {
    serde_json::json!({
        "httpMatch": {
            "pathMatch": { "match": { "prefix": "/" } },
            "headerMatches": [],
        },
    })
}

/// Rules 1 and 2, over a table that varies WHICH members an entry supplies (priority
/// only at each published bound of `@range(1, 2000)` and in between, match only with and
/// without header matches, action only, all three at once, and none at all). The recorded
/// case — a priority-only update — is one row of it.
#[tokio::test]
async fn an_accepted_entry_is_persisted_and_read_back_by_get_rule() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "batch-host").await;
    let listener_id = a_listener(&lattice, &service_id, "batch-listener").await;

    // (case, the entry's members, the priority / match / action the rule must report
    // afterwards). Every row starts from a rule created at its own priority with
    // `fixed_response_rule`'s match and a 404 fixed response.
    type Row = (
        &'static str,
        serde_json::Value,
        i64,
        serde_json::Value,
        serde_json::Value,
    );
    let cases: Vec<Row> = vec![
        (
            "priority only, the published minimum",
            serde_json::json!({ "priority": 1 }),
            1,
            created_match(),
            serde_json::json!({ "fixedResponse": { "statusCode": 404 } }),
        ),
        (
            "priority only, the published maximum",
            serde_json::json!({ "priority": 2000 }),
            2000,
            created_match(),
            serde_json::json!({ "fixedResponse": { "statusCode": 404 } }),
        ),
        (
            "priority only, in between",
            serde_json::json!({ "priority": 700 }),
            700,
            created_match(),
            serde_json::json!({ "fixedResponse": { "statusCode": 404 } }),
        ),
        (
            "match only, header matches configured",
            serde_json::json!({
                "match": {
                    "httpMatch": {
                        "method": "GET",
                        "headerMatches": [
                            { "name": "x-tenant", "match": { "exact": "acme" } },
                            { "name": "x-trace", "match": { "contains": "abc" }, "caseSensitive": true },
                        ],
                    },
                },
            }),
            // Untouched by an entry that supplies no priority.
            41,
            serde_json::json!({
                "httpMatch": {
                    "method": "GET",
                    "headerMatches": [
                        { "name": "x-tenant", "match": { "exact": "acme" } },
                        { "name": "x-trace", "match": { "contains": "abc" }, "caseSensitive": true },
                    ],
                },
            }),
            serde_json::json!({ "fixedResponse": { "statusCode": 404 } }),
        ),
        (
            "match only, no header match configured",
            serde_json::json!({
                "match": { "httpMatch": { "pathMatch": { "match": { "exact": "/orders" } } } },
            }),
            51,
            serde_json::json!({
                "httpMatch": {
                    "pathMatch": { "match": { "exact": "/orders" } },
                    "headerMatches": [],
                },
            }),
            serde_json::json!({ "fixedResponse": { "statusCode": 404 } }),
        ),
        (
            "action only, forward to one target group with no weight",
            serde_json::json!({
                "action": {
                    "forward": {
                        "targetGroups": [{ "targetGroupIdentifier": target_group("1") }],
                    },
                },
            }),
            61,
            created_match(),
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
            "priority, match and action at once",
            serde_json::json!({
                "priority": 777,
                "match": { "httpMatch": { "method": "DELETE" } },
                "action": { "fixedResponse": { "statusCode": 500 } },
            }),
            777,
            serde_json::json!({ "httpMatch": { "method": "DELETE", "headerMatches": [] } }),
            serde_json::json!({ "fixedResponse": { "statusCode": 500 } }),
        ),
        (
            "no member beyond the identifier",
            serde_json::json!({}),
            81,
            created_match(),
            serde_json::json!({ "fixedResponse": { "statusCode": 404 } }),
        ),
    ];

    for (index, (case, members, expected_priority, expected_match, expected_action)) in
        cases.into_iter().enumerate()
    {
        // Its own name and create-time priority, so the rows cannot collide with one
        // another inside the one listener.
        let created_priority = i64::try_from(index).expect("the table is small") * 10 + 11;
        let name = format!("entry-{created_priority}");
        let rule_id = a_rule(&lattice, &service_id, &listener_id, &name, created_priority).await;
        let before = get_rule(&lattice, &service_id, &listener_id, &rule_id).await;
        let created_at = instant_of(case, &before.body, "createdAt");
        let updated_before = instant_of(case, &before.body, "lastUpdatedAt");

        let mut entry = members.clone();
        entry["ruleIdentifier"] = serde_json::json!(rule_id);
        let answer = batch_update(
            &lattice,
            &service_id,
            &listener_id,
            serde_json::json!([entry]),
        )
        .await;

        let successful = successful_of(case, &answer);
        assert_eq!(
            successful.len(),
            1,
            "case {case:?}: expected exactly one applied entry, got {}",
            answer.body
        );
        assert!(
            unsuccessful_of(case, &answer).is_empty(),
            "case {case:?}: expected no refused entry, got {}",
            answer.body
        );
        let applied = &successful[0];
        assert_eq!(
            applied["id"].as_str(),
            Some(rule_id.as_str()),
            "case {case:?}: expected the updated rule's id, got {applied}"
        );
        assert_eq!(
            applied["arn"].as_str(),
            Some(expected_arn(&rule_kind(&service_id, &listener_id), &rule_id).as_str()),
            "case {case:?}: expected the rule's ARN, got {applied}"
        );
        assert_eq!(
            applied["name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: an update carries no name, so it must keep its own, got {applied}"
        );
        assert_eq!(
            applied["isDefault"].as_bool(),
            Some(false),
            "case {case:?}: expected isDefault false, got {applied}"
        );
        assert_eq!(
            applied["priority"].as_i64(),
            Some(expected_priority),
            "case {case:?}: expected priority {expected_priority}, got {applied}"
        );
        assert_eq!(
            applied["match"], expected_match,
            "case {case:?}: expected match {expected_match}, got {applied}"
        );
        assert_eq!(
            applied["action"], expected_action,
            "case {case:?}: expected action {expected_action}, got {applied}"
        );
        assert_header_matches_present(case, "the batch answer", applied);

        // PERSISTED: a later read reports what the entry asked for, not what the create
        // stored.
        let read = get_rule(&lattice, &service_id, &listener_id, &rule_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from the read-back, got {} / {}",
            read.status, read.body
        );
        assert_eq!(
            read.body["priority"].as_i64(),
            Some(expected_priority),
            "case {case:?}: expected the read-back priority {expected_priority}, got {}",
            read.body
        );
        assert_eq!(
            read.body["match"], expected_match,
            "case {case:?}: expected the read-back match {expected_match}, got {}",
            read.body
        );
        assert_eq!(
            read.body["action"], expected_action,
            "case {case:?}: expected the read-back action {expected_action}, got {}",
            read.body
        );
        assert_header_matches_present(case, "the read-back", &read.body);
        assert!(
            is_timestamp(&read.body["lastUpdatedAt"]),
            "case {case:?}: lastUpdatedAt must be an RFC 3339 instant, got {}",
            read.body
        );
        assert_eq!(
            instant_of(case, &read.body, "createdAt"),
            created_at,
            "case {case:?}: an update must not move createdAt, got {}",
            read.body
        );
        let updated_after = instant_of(case, &read.body, "lastUpdatedAt");
        if members.as_object().is_some_and(serde_json::Map::is_empty) {
            assert_eq!(
                updated_after, updated_before,
                "case {case:?}: an entry that changes nothing must leave lastUpdatedAt, got {}",
                read.body
            );
        } else {
            assert!(
                updated_after >= updated_before,
                "case {case:?}: expected lastUpdatedAt at or after {updated_before}, got {updated_after}"
            );
        }
    }
}

/// Rule 3's accepted side: a batch is read as ONE change, so every entry's priority is
/// checked against what the whole batch leaves — two rules may exchange their priorities
/// in one call, and a bulk re-prioritisation onto free priorities goes through. The table
/// varies the batch size and the shape of the move.
#[tokio::test]
async fn one_batch_may_exchange_or_bulk_move_the_priorities_it_names() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "swap-host").await;

    // (case, the listener the row's rules live under, the priorities the rules are
    // created at, the priorities the batch assigns in request order)
    let cases: Vec<(&str, &str, Vec<i64>, Vec<i64>)> = vec![
        (
            "two rules exchanging their priorities",
            "swap-two",
            vec![10, 20],
            vec![20, 10],
        ),
        (
            "three rules rotated through each other's priorities",
            "swap-three",
            vec![1, 2, 3],
            vec![3, 1, 2],
        ),
        (
            "four rules moved onto free priorities across the published bounds",
            "swap-four",
            vec![10, 11, 12, 13],
            vec![2000, 1000, 500, 1],
        ),
    ];

    for (case, listener_name, created, assigned) in cases {
        // One listener per row, so each row is a batch over exactly its own rules.
        let listener_id = a_listener(&lattice, &service_id, listener_name).await;
        let mut ids = Vec::new();
        for (index, priority) in created.iter().enumerate() {
            ids.push(
                a_rule(
                    &lattice,
                    &service_id,
                    &listener_id,
                    &format!("at-{index}-{priority}"),
                    *priority,
                )
                .await,
            );
        }

        let entries: Vec<serde_json::Value> = ids
            .iter()
            .zip(assigned.iter())
            .map(|(id, priority)| serde_json::json!({ "ruleIdentifier": id, "priority": priority }))
            .collect();
        let answer = batch_update(
            &lattice,
            &service_id,
            &listener_id,
            serde_json::json!(entries),
        )
        .await;
        assert_eq!(
            successful_of(case, &answer).len(),
            ids.len(),
            "case {case:?}: expected every entry applied, got {}",
            answer.body
        );
        assert!(
            unsuccessful_of(case, &answer).is_empty(),
            "case {case:?}: expected no refused entry, got {}",
            answer.body
        );

        for ((id, expected), was) in ids.iter().zip(assigned.iter()).zip(created.iter()) {
            let read = get_rule(&lattice, &service_id, &listener_id, id).await;
            assert_eq!(
                read.body["priority"].as_i64(),
                Some(*expected),
                "case {case:?}: rule {id} was created at {was} and assigned {expected}, got {}",
                read.body
            );
        }

        // `BatchUpdateRule` is `@idempotent`: re-sending the batch that already applied
        // answers the way it did the first time, because every entry's priority is held
        // by no rule but its own.
        let again = batch_update(
            &lattice,
            &service_id,
            &listener_id,
            serde_json::json!(entries),
        )
        .await;
        let resent = format!("{case} re-sent");
        assert_eq!(
            successful_of(&resent, &again).len(),
            ids.len(),
            "case {resent:?}: expected every entry applied again, got {}",
            again.body
        );
        for (id, expected) in ids.iter().zip(assigned.iter()) {
            let read = get_rule(&lattice, &service_id, &listener_id, id).await;
            assert_eq!(
                read.body["priority"].as_i64(),
                Some(*expected),
                "case {resent:?}: rule {id} must still be at {expected}, got {}",
                read.body
            );
        }
    }
}

/// Rule 3's refused side: an entry whose priority a rule the batch does not move would
/// still hold refuses the call with the modelled 409 and changes nothing. The table
/// varies where the held priority comes from — a rule outside the batch, another entry
/// of the same batch, and a rule the batch leaves where it is.
///
/// The refusal is reported over the rule the refused entry NAMED, at the priority that
/// rule holds: the priority that, as the message puts it, already exists.
#[tokio::test]
async fn a_batch_that_would_duplicate_a_priority_is_refused_and_changes_nothing() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "collide-host").await;
    let listener_id = a_listener(&lattice, &service_id, "collide-listener").await;

    let kept = a_rule(&lattice, &service_id, &listener_id, "kept", 10).await;
    let moved = a_rule(&lattice, &service_id, &listener_id, "moved", 20).await;
    let other = a_rule(&lattice, &service_id, &listener_id, "other", 30).await;

    // (case, the entries, the rule the refusal must name, the priority it must name)
    let cases: Vec<(&str, serde_json::Value, &String, i64)> = vec![
        (
            "a priority a rule outside the batch keeps",
            serde_json::json!([{ "ruleIdentifier": moved, "priority": 10 }]),
            &moved,
            20,
        ),
        (
            "two entries asking for the same new priority",
            serde_json::json!([
                { "ruleIdentifier": moved, "priority": 500 },
                { "ruleIdentifier": other, "priority": 500 },
            ]),
            &moved,
            20,
        ),
        (
            "a priority the batch does not free, beside an entry that would apply",
            serde_json::json!([
                { "ruleIdentifier": moved, "priority": 40 },
                { "ruleIdentifier": other, "priority": 10 },
            ]),
            &other,
            30,
        ),
        (
            "a priority exchanged with a rule the batch leaves where it is",
            serde_json::json!([{ "ruleIdentifier": moved, "priority": 30 }]),
            &moved,
            20,
        ),
    ];
    for (case, entries, named_rule, priority) in cases {
        let answer = batch_update(&lattice, &service_id, &listener_id, entries).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected 409, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ConflictException"),
            "case {case:?}: expected the modelled ConflictException, got {:?} / {}",
            answer.error_code,
            answer.body
        );
        assert_eq!(
            message_of(case, &answer),
            format!("Rule with priority {priority} already exist!"),
            "case {case:?}: expected this service's duplicate-priority construction, got {}",
            answer.body
        );
        assert_eq!(
            answer.body["resourceType"].as_str(),
            Some("RULE"),
            "case {case:?}: expected the rule resource-type token, got {}",
            answer.body
        );
        assert_eq!(
            answer.body["resourceId"].as_str(),
            Some(named_rule.as_str()),
            "case {case:?}: the conflict must name the rule the refused entry named, got {}",
            answer.body
        );

        // A refused batch stores NOTHING: every rule keeps the priority it had, so the
        // entry that could have applied did not.
        for (id, expected) in [(&kept, 10), (&moved, 20), (&other, 30)] {
            let read = get_rule(&lattice, &service_id, &listener_id, id).await;
            assert_eq!(
                read.body["priority"].as_i64(),
                Some(expected),
                "case {case:?}: rule {id} must still be at {expected}, got {}",
                read.body
            );
        }
    }
}

/// Rule 4: an entry naming no rule of this listener does not refuse the batch. The table
/// varies why no rule is reachable — an identifier naming nothing, and a rule of ANOTHER
/// listener.
#[tokio::test]
async fn an_entry_naming_no_rule_of_this_listener_leaves_its_siblings_applied() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "partial-host").await;
    let listener_id = a_listener(&lattice, &service_id, "partial-listener").await;
    let elsewhere = a_listener(&lattice, &service_id, "partial-elsewhere").await;
    let foreign = a_rule(&lattice, &service_id, &elsewhere, "foreign", 10).await;
    let absent_rule = absent_id("rule-", "0");

    // (case, the identifier the refused entry names, the text its failure message must
    // carry)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "an identifier naming no rule",
            absent_rule.clone(),
            format!("Rule {absent_rule} cannot be found."),
        ),
        (
            "a rule of another listener of the same service",
            foreign.clone(),
            format!("Rule {foreign} cannot be found."),
        ),
    ];

    for (index, (case, refused_identifier, message)) in cases.into_iter().enumerate() {
        let step = i64::try_from(index).expect("the table is small");
        let sibling = a_rule(
            &lattice,
            &service_id,
            &listener_id,
            &format!("sibling-{step}"),
            step * 10 + 1,
        )
        .await;
        let applied_priority = step * 10 + 2;

        let answer = batch_update(
            &lattice,
            &service_id,
            &listener_id,
            serde_json::json!([
                { "ruleIdentifier": refused_identifier, "priority": 900 },
                { "ruleIdentifier": sibling, "priority": applied_priority },
            ]),
        )
        .await;

        let successful = successful_of(case, &answer);
        assert_eq!(
            successful.len(),
            1,
            "case {case:?}: expected the sibling entry applied on its own, got {}",
            answer.body
        );
        assert_eq!(
            successful[0]["id"].as_str(),
            Some(sibling.as_str()),
            "case {case:?}: expected the sibling under successful, got {}",
            answer.body
        );
        let refused = unsuccessful_of(case, &answer);
        assert_eq!(
            refused.len(),
            1,
            "case {case:?}: expected exactly one refused entry, got {}",
            answer.body
        );
        assert_eq!(
            refused[0]["ruleIdentifier"].as_str(),
            Some(refused_identifier.as_str()),
            "case {case:?}: a refused entry names the identifier it was sent for, got {}",
            answer.body
        );
        assert_eq!(
            refused[0]["failureCode"].as_str(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected the code the same refusal carries on one rule, got {}",
            answer.body
        );
        assert_eq!(
            refused[0]["failureMessage"].as_str(),
            Some(message.as_str()),
            "case {case:?}: expected the failure message {message:?}, got {}",
            answer.body
        );

        // The sibling really moved, and the refused entry wrote nothing.
        let read = get_rule(&lattice, &service_id, &listener_id, &sibling).await;
        assert_eq!(
            read.body["priority"].as_i64(),
            Some(applied_priority),
            "case {case:?}: expected the sibling at {applied_priority}, got {}",
            read.body
        );
        // A rule of another listener is never reached through this one.
        let untouched = get_rule(&lattice, &service_id, &elsewhere, &foreign).await;
        assert_eq!(
            untouched.body["priority"].as_i64(),
            Some(10),
            "case {case:?}: the rule of another listener must keep its priority, got {}",
            untouched.body
        );
    }
}

/// Rule 4's other half: a value a documented service rule refuses is refused for the
/// whole call, exactly as the single-rule path refuses it, and stores nothing. The table
/// is the catalog the member documents plus values outside it.
#[tokio::test]
async fn an_action_outside_the_documented_catalog_refuses_the_call() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "catalog-host").await;
    let listener_id = a_listener(&lattice, &service_id, "catalog-listener").await;
    let rule_id = a_rule(&lattice, &service_id, &listener_id, "catalog-rule", 10).await;

    // `FixedResponseAction`'s `statusCode` documentation: "Only 404 and 500 status codes
    // are supported", narrower than the modelled `@range(100, 599)`.
    let cases: Vec<(i64, bool)> = vec![(404, true), (500, true), (200, false), (403, false), (503, false)];

    for (status_code, accepted) in cases {
        let case = format!("fixed response {status_code}");
        let answer = batch_update(
            &lattice,
            &service_id,
            &listener_id,
            serde_json::json!([{
                "ruleIdentifier": rule_id,
                "action": { "fixedResponse": { "statusCode": status_code } },
            }]),
        )
        .await;
        if accepted {
            assert_eq!(
                successful_of(&case, &answer).len(),
                1,
                "case {case:?}: expected the entry applied, got {}",
                answer.body
            );
            let read = get_rule(&lattice, &service_id, &listener_id, &rule_id).await;
            assert_eq!(
                read.body["action"],
                serde_json::json!({ "fixedResponse": { "statusCode": status_code } }),
                "case {case:?}: expected the stored action, got {}",
                read.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400, got {} / {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected the modelled ValidationException, got {:?} / {}",
                answer.error_code,
                answer.body
            );
            // The refused call stored nothing: the rule keeps the action the last
            // accepted write left on it.
            let read = get_rule(&lattice, &service_id, &listener_id, &rule_id).await;
            assert_eq!(
                read.body["action"],
                serde_json::json!({ "fixedResponse": { "statusCode": 500 } }),
                "case {case:?}: a refused call must store nothing, got {}",
                read.body
            );
        }
    }
}

/// Rule 7: an entry naming the listener's DEFAULT rule is refused for the whole call.
///
/// The rows differ along WHICH member the entry supplies and along where the default
/// rule's entry sits in the batch, because the rule is about the rule it names and not
/// about the member or the position: an update that would change the default rule is
/// refused whatever it asks for, and its sibling entries are not applied either.
#[tokio::test]
async fn an_entry_naming_the_default_rule_refuses_the_call() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "default-rule-batch-host").await;
    let listener_id = a_listener(&lattice, &service_id, "default-rule-batch-listener").await;
    let default_id = default_rule_id(&lattice, &service_id, &listener_id).await;
    let sibling_id = a_rule(&lattice, &service_id, &listener_id, "batch-sibling", 10).await;

    // (case, the member the default rule's entry supplies, where that entry sits)
    let cases: Vec<(&str, serde_json::Value, bool)> = vec![
        (
            "a priority on the default rule, alone",
            serde_json::json!({ "ruleIdentifier": default_id, "priority": 7 }),
            false,
        ),
        (
            "a match on the default rule, alone",
            serde_json::json!({
                "ruleIdentifier": default_id,
                "match": { "httpMatch": { "pathMatch": {
                    "match": { "exact": "/default-batch" },
                    "caseSensitive": true,
                } } },
            }),
            false,
        ),
        (
            "an action on the default rule, after a sibling entry",
            serde_json::json!({
                "ruleIdentifier": default_id,
                "action": { "fixedResponse": { "statusCode": 404 } },
            }),
            true,
        ),
        (
            "an entry on the default rule supplying nothing",
            serde_json::json!({ "ruleIdentifier": default_id }),
            true,
        ),
    ];

    for (case, entry, after_a_sibling) in cases {
        let rules = if after_a_sibling {
            serde_json::json!([
                { "ruleIdentifier": sibling_id, "priority": 20 },
                entry,
            ])
        } else {
            serde_json::json!([entry])
        };
        let answer = batch_update(&lattice, &service_id, &listener_id, rules).await;
        assert_refused(&answer, case);
        assert_eq!(
            message_of(case, &answer),
            "You can't modify a default listener rule. To modify a default listener rule, \
             use UpdateListener.",
            "case {case:?}: expected the sentence UpdateRule publishes for a default rule, \
             got {}",
            answer.body
        );
        // The refused call stored nothing: neither the default rule nor the sibling the
        // batch also named moved.
        let read_default = get_rule(&lattice, &service_id, &listener_id, &default_id).await;
        assert_eq!(
            read_default.body["priority"].as_i64(),
            Some(99_999),
            "case {case:?}: the default rule must keep its priority, got {}",
            read_default.body
        );
        let read_sibling = get_rule(&lattice, &service_id, &listener_id, &sibling_id).await;
        assert_eq!(
            read_sibling.body["priority"].as_i64(),
            Some(10),
            "case {case:?}: a refused call must store nothing for the sibling either, got {}",
            read_sibling.body
        );
    }
}

/// Rule 5: the request's own identifiers are resolved against persisted records, in
/// either modelled form.
#[tokio::test]
async fn unknown_identifiers_are_404_and_both_identifier_forms_reach_the_rule() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "batch-refusal-host").await;
    let (other_service, _) = a_service(&lattice, "batch-refusal-other").await;
    let listener_id = a_listener(&lattice, &service_id, "batch-refusal-listener").await;
    let foreign_listener = a_listener(&lattice, &other_service, "batch-refusal-foreign").await;
    let rule_id = a_rule(&lattice, &service_id, &listener_id, "refusal-rule", 10).await;

    let absent_service = absent_id("svc-", "0");
    let absent_listener = absent_id("listener-", "0");

    // (case, serviceIdentifier, listenerIdentifier, the kind token the message names,
    // the identifier it names)
    type Row = (&'static str, String, String, &'static str, String);
    let cases: Vec<Row> = vec![
        (
            "a service that does not exist",
            absent_service.clone(),
            listener_id.clone(),
            "Service",
            absent_service.clone(),
        ),
        (
            "a listener that does not exist",
            service_id.clone(),
            absent_listener.clone(),
            "Listener",
            absent_listener.clone(),
        ),
        (
            "a listener of another service",
            service_id.clone(),
            foreign_listener.clone(),
            "Listener",
            foreign_listener.clone(),
        ),
    ];

    for (case, service_identifier, listener_identifier, label, identifier) in cases {
        let answer = batch_update(
            &lattice,
            &service_identifier,
            &listener_identifier,
            serde_json::json!([{ "ruleIdentifier": rule_id, "priority": 20 }]),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected the modelled ResourceNotFoundException, got {:?} / {}",
            answer.error_code,
            answer.body
        );
        assert_eq!(
            message_of(case, &answer),
            format!("{label} {identifier} cannot be found."),
            "case {case:?}: expected this service's not-found construction, got {}",
            answer.body
        );
        // The refused call stored nothing.
        let read = get_rule(&lattice, &service_id, &listener_id, &rule_id).await;
        assert_eq!(
            read.body["priority"].as_i64(),
            Some(10),
            "case {case:?}: expected the rule untouched, got {}",
            read.body
        );
    }

    // Every identifier of the request, in its ARN form: the service's, the listener's
    // and the rule's.
    let service_arn = expected_arn("service", &service_id);
    let listener_arn = expected_arn(&listener_kind(&service_id), &listener_id);
    let rule_arn = expected_arn(&rule_kind(&service_id, &listener_id), &rule_id);
    let answer = batch_update(
        &lattice,
        &as_label(&service_arn),
        &as_label(&listener_arn),
        serde_json::json!([{ "ruleIdentifier": rule_arn, "priority": 25 }]),
    )
    .await;
    let case = "every identifier in its ARN form";
    assert_eq!(
        successful_of(case, &answer).len(),
        1,
        "case {case:?}: expected the entry applied, got {}",
        answer.body
    );
    let read = get_rule(&lattice, &service_id, &listener_id, &rule_id).await;
    assert_eq!(
        read.body["priority"].as_i64(),
        Some(25),
        "case {case:?}: expected the ARN-addressed update persisted, got {}",
        read.body
    );
}

/// Rule 6: the published bounds of the batch itself.
///
/// `rules`' length bound is `RuleUpdateList`'s `@length(1, 10)` and an entry's priority
/// bound is `RuleUpdate`'s `priority` `@range(1, 2000)` — both read off the model
/// (`smithy_query(query="shape",shape="RuleUpdateList,RuleUpdate")`). Each published
/// bound is driven, and so is one value past each: a batch the bound admits applies
/// every entry it carries, and a request outside the bound is the modelled 400 and
/// leaves every rule of the listener where it was.
#[tokio::test]
async fn the_published_batch_size_and_priority_bounds_bound_the_call() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "bounds-host").await;

    // (case, how many entries the batch carries, whether `@length(1, 10)` admits it)
    let sizes: Vec<(&str, usize, bool)> = vec![
        ("the published minimum", 1, true),
        ("between the published bounds", 5, true),
        ("the published maximum", 10, true),
        ("one below the published minimum", 0, false),
        ("one above the published maximum", 11, false),
    ];

    for (case, size, admitted) in sizes {
        // One listener per row, so each row's batch spans exactly its own rules.
        let listener_id = a_listener(&lattice, &service_id, &format!("bounds-of-{size}")).await;
        // A rule per entry the row sends — and one even when it sends none, so every row
        // has a stored rule whose priority a refused call must leave alone.
        let mut ids = Vec::new();
        for index in 0..size.max(1) {
            let step = i64::try_from(index).expect("the table is small");
            ids.push(
                a_rule(
                    &lattice,
                    &service_id,
                    &listener_id,
                    &format!("bounds-{size}-{index}"),
                    step + 1,
                )
                .await,
            );
        }
        // Every entry moves its rule onto a priority no rule of this listener holds, so
        // only the batch SIZE decides how the call is answered.
        let entries: Vec<serde_json::Value> = ids
            .iter()
            .take(size)
            .enumerate()
            .map(|(index, id)| {
                let assigned = i64::try_from(index).expect("the table is small") + 1_000;
                serde_json::json!({ "ruleIdentifier": id, "priority": assigned })
            })
            .collect();

        let answer = batch_update(
            &lattice,
            &service_id,
            &listener_id,
            serde_json::json!(entries),
        )
        .await;

        if admitted {
            assert_eq!(
                successful_of(case, &answer).len(),
                size,
                "case {case:?}: expected all {size} entries applied, got {}",
                answer.body
            );
            assert!(
                unsuccessful_of(case, &answer).is_empty(),
                "case {case:?}: expected no refused entry, got {}",
                answer.body
            );
        } else {
            assert_refused(&answer, &format!("{case} ({size} entries)"));
        }

        // Read every rule back: an admitted batch persisted each entry, and a refused
        // one stored nothing, so each rule still holds the priority it was created at.
        for (index, id) in ids.iter().enumerate() {
            let step = i64::try_from(index).expect("the table is small");
            let expected = if admitted { step + 1_000 } else { step + 1 };
            let read = get_rule(&lattice, &service_id, &listener_id, id).await;
            assert_eq!(
                read.body["priority"].as_i64(),
                Some(expected),
                "case {case:?}: expected rule {id} at {expected}, got {}",
                read.body
            );
        }
    }

    // One past each published bound of an entry's `priority`. The bounds themselves (1
    // and 2000) are driven by the accepted-entry table above; a value past either is
    // outside the modelled range, so the whole call is refused and the entry beside it
    // is not applied either.
    let listener_id = a_listener(&lattice, &service_id, "bounds-of-priority").await;
    let moved = a_rule(&lattice, &service_id, &listener_id, "bounds-moved", 7).await;
    let sibling = a_rule(&lattice, &service_id, &listener_id, "bounds-sibling", 8).await;
    for (case, priority) in [
        ("one below the published minimum priority", 0),
        ("one above the published maximum priority", 2001),
    ] {
        let answer = batch_update(
            &lattice,
            &service_id,
            &listener_id,
            serde_json::json!([
                { "ruleIdentifier": sibling, "priority": 1_500 },
                { "ruleIdentifier": moved, "priority": priority },
            ]),
        )
        .await;
        assert_refused(&answer, &format!("{case} (priority {priority})"));
        for (id, expected) in [(&moved, 7), (&sibling, 8)] {
            let read = get_rule(&lattice, &service_id, &listener_id, id).await;
            assert_eq!(
                read.body["priority"].as_i64(),
                Some(expected),
                "case {case:?}: a refused batch stores nothing, so rule {id} must still \
                 be at {expected}, got {}",
                read.body
            );
        }
    }
}
