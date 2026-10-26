// SPDX-License-Identifier: CC0-1.0

//! `CreateReceiptRuleSet`, end to end over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE the operation follows over a
//! SET of inputs, not one recorded request:
//!
//! * an accepted create records exactly ONE new, empty rule set under the name it
//!   was given and changes nothing else the account holds — every set already
//!   stored keeps the name, creation instant and rules it had;
//! * a create naming a set the account already holds is a naming conflict, and a
//!   conflict is not an upsert: the held set keeps its rules, its creation instant
//!   and its activation, so the refused call stores nothing at all.
//!
//! The name form, the duplicate and limit refusals and the empty-set round trip
//! are asserted in `ses_receipt_rule_sets`; this suite is about what a create
//! leaves UNCHANGED.

use crate::harness::{call, element, elements, emulator_over, field};
use service_com_amazonaws_ses_simpleemailservice::ops::types::ReceiptRule;
use service_com_amazonaws_ses_simpleemailservice::state::{
    service_state, ReceiptRuleSet, MAX_RECEIPT_RULE_SET_NAME,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

/// The epoch second a seeded set is stored as created at, so the timestamp a
/// reader reports afterwards is a KNOWN value rather than "about now": a write
/// that re-stamped the held set would report something else.
const SEEDED_CREATED_AT_SECS: u64 = 1_767_225_600;

/// A router holding only this service, beside the engine it was built over, so a
/// row can seed a rule set holding RULES — `CreateReceiptRule` is still a
/// scaffold stub, so the rules of a set cannot be written over the wire yet.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_over(Arc::clone(&engine)), engine)
}

/// `CreateReceiptRuleSet`.
async fn create_rule_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "CreateReceiptRuleSet",
        &[field("RuleSetName", name)],
    )
    .await
}

/// `DescribeReceiptRuleSet`, asserted to succeed, as `(the `Metadata` element's
/// inner XML, the `Rules` element's inner XML)`.
async fn describe_rule_set(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, body) = call(
        router,
        "DescribeReceiptRuleSet",
        &[field("RuleSetName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case describe {name:?}: expected 200, got {status} with {body}"
    );
    (
        element(&body, "Metadata").unwrap_or_default(),
        element(&body, "Rules").unwrap_or_default(),
    )
}

/// `SetActiveReceiptRuleSet` naming `name`, asserted to succeed.
async fn set_active(router: &MultiServiceRouter, name: &str) {
    let (status, body) = call(
        router,
        "SetActiveReceiptRuleSet",
        &[field("RuleSetName", name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case activate {name:?}: expected 200, got {status} with {body}"
    );
}

/// `DescribeActiveReceiptRuleSet`, as `(the active set's name if one is active,
/// the `Rules` element's inner XML)`.
async fn describe_active(router: &MultiServiceRouter) -> (Option<String>, String) {
    let (status, body) = call(router, "DescribeActiveReceiptRuleSet", &[]).await;
    assert_eq!(
        status, 200,
        "case describe the active set: expected 200, got {status} with {body}"
    );
    (
        element(&body, "Metadata").and_then(|metadata| element(&metadata, "Name")),
        element(&body, "Rules").unwrap_or_default(),
    )
}

/// The names `ListReceiptRuleSets` reports for this account, in the order it
/// serves them.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "ListReceiptRuleSets", &[]).await;
    assert_eq!(
        status, 200,
        "case list the rule sets: expected 200, got {status} with {body}"
    );
    let page = element(&body, "RuleSets")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default();
    page.iter()
        .map(|member| {
            element(member, "Name")
                .unwrap_or_else(|| panic!("a listed rule set carries Name, got {member}"))
        })
        .collect()
}

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope.
fn error(body: &str) -> (Option<String>, Option<String>) {
    (element(body, "Code"), element(body, "Message"))
}

/// A rule to seed a held set with, so a conflicting create has something it
/// could destroy.
fn seeded_rule(name: &str, recipients: &[&str], enabled: bool) -> ReceiptRule {
    ReceiptRule {
        name: name.to_string(),
        enabled,
        tls_policy: Some("Optional".to_string()),
        recipients: Some(recipients.iter().map(|at| (*at).to_string()).collect()),
        actions: None,
        scan_enabled: true,
    }
}

/// Store a receipt rule set holding `rules`, created at [`SEEDED_CREATED_AT_SECS`],
/// directly in the service's state.
fn seed_rule_set(engine: &engine::InMemoryEngine, name: &str, rules: Vec<ReceiptRule>) {
    let store = service_state(engine).expect("this service has a store on the shared engine");
    store.lock().receipt_rule_sets.insert(
        name.to_string(),
        ReceiptRuleSet {
            name: name.to_string(),
            created_at: UNIX_EPOCH + Duration::from_secs(SEEDED_CREATED_AT_SECS),
            rules,
        },
    );
}

/// Each accepted create records ONE empty set and leaves every set the account
/// already holds exactly as it was: same metadata, same (empty) rule list, and
/// the listing grows by exactly that one name.
#[tokio::test]
async fn each_create_records_one_empty_set_and_leaves_the_others_as_they_were() {
    // One row is a SEQUENCE of creates on a fresh account, so the rows differ in
    // how many sets are already held when the next create lands, and in the form
    // of the names involved: the documented form
    // (`CreateReceiptRuleSetRequest$RuleSetName`) is ASCII letters, numbers,
    // underscores or dashes, starting and ending with a letter or number, 64
    // characters or fewer. The orders are deliberately not lexicographic, so a
    // create that disturbed a neighbour would show up in the comparison below.
    let rows: Vec<Vec<String>> = vec![
        vec!["a".to_string()],
        vec!["z-last".to_string(), "9".to_string()],
        vec![
            "Prod-Rules_2024".to_string(),
            "prod-rules_2024".to_string(),
            "x".repeat(MAX_RECEIPT_RULE_SET_NAME),
            "inbound".to_string(),
        ],
    ];
    for names in &rows {
        let (router, _engine) = emulator_with_engine();
        // (name, the Metadata element this set reported when it was created).
        let mut held: Vec<(String, String)> = Vec::new();
        for name in names {
            let (status, body) = create_rule_set(&router, name).await;
            assert_eq!(
                status, 200,
                "case {names:?} create {name:?}: expected 200, got {status} with {body}"
            );

            let (metadata, rules) = describe_rule_set(&router, name).await;
            assert_eq!(
                element(&metadata, "Name").as_deref(),
                Some(name.as_str()),
                "case {names:?} create {name:?}: the new set carries the requested name, \
                 got {metadata}"
            );
            // "Creates an empty receipt rule set": the new set holds no rules.
            assert_eq!(
                rules, "",
                "case {names:?} create {name:?}: a created set holds no rules, got {rules}"
            );

            // Every set created before this one is untouched by this create.
            for (earlier, snapshot) in &held {
                let (metadata, rules) = describe_rule_set(&router, earlier).await;
                assert_eq!(
                    &metadata, snapshot,
                    "case {names:?} create {name:?}: {earlier:?} must be unchanged, \
                     got {metadata}"
                );
                assert_eq!(
                    rules, "",
                    "case {names:?} create {name:?}: {earlier:?} must still hold no rules, \
                     got {rules}"
                );
            }
            held.push((name.clone(), metadata));

            // And the account holds exactly the sets created so far — no more,
            // no fewer.
            let mut expected: Vec<String> = held.iter().map(|(name, _)| name.clone()).collect();
            expected.sort();
            let mut listed = listed_names(&router).await;
            listed.sort();
            assert_eq!(
                listed, expected,
                "case {names:?} create {name:?}: expected the account to hold {expected:?}, \
                 got {listed:?}"
            );
        }
    }
}

/// A create naming a set the account already holds is refused as a naming
/// conflict and stores NOTHING: the held set keeps its rules, its creation
/// instant and its activation.
#[tokio::test]
async fn a_conflicting_create_leaves_the_held_set_exactly_as_it_was() {
    // The rows differ along what a destructive create would destroy: how many
    // rules the held set carries, what those rules carry, and whether the set is
    // the account's ACTIVE one.
    let rows: Vec<(&str, Vec<ReceiptRule>, bool)> = vec![
        ("held-empty", Vec::new(), false),
        ("held_one-rule", vec![seeded_rule("only", &[], true)], false),
        (
            "Held9",
            vec![
                seeded_rule("catch-all", &["example.com"], true),
                seeded_rule("sales", &["sales@example.com"], false),
                seeded_rule("audit", &["a@example.net", "b@example.org"], true),
            ],
            true,
        ),
    ];
    for (name, rules, activate) in &rows {
        let (router, engine) = emulator_with_engine();
        let rule_count = rules.len();
        seed_rule_set(&engine, name, rules.clone());
        if *activate {
            set_active(&router, name).await;
        }

        let (metadata_before, rules_before) = describe_rule_set(&router, name).await;
        let expected_created = ::aws_smithy_types::DateTime::from_secs(
            SEEDED_CREATED_AT_SECS as i64,
        )
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .expect("the seeded instant renders as a date-time");
        assert_eq!(
            element(&metadata_before, "CreatedTimestamp").as_deref(),
            Some(expected_created.as_str()),
            "case {name:?}: the held set reports the instant it was stored, \
             got {metadata_before}"
        );

        let (status, body) = create_rule_set(&router, name).await;
        assert_eq!(
            status, 400,
            "case {name:?} with {rule_count} rule(s): expected HTTP 400, got {status} \
             with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("AlreadyExists"),
            "case {name:?}: expected the modelled AlreadyExists, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Rule set already exists: {name}").as_str()),
            "case {name:?}: expected the conflicting name in the message, got {body}"
        );

        // The refused create stored nothing: same metadata (so the creation
        // instant was not re-stamped) and the same rules it held.
        let (metadata_after, rules_after) = describe_rule_set(&router, name).await;
        assert_eq!(
            metadata_after, metadata_before,
            "case {name:?}: the refused create must not re-stamp the held set, \
             got {metadata_after}"
        );
        assert_eq!(
            rules_after, rules_before,
            "case {name:?}: the refused create must keep the set's {rule_count} rule(s), \
             got {rules_after}"
        );

        // Nor did it change which set is active, or what the active reader
        // reports for it.
        let (active, active_rules) = describe_active(&router).await;
        let expected_active = if *activate { Some(*name) } else { None };
        assert_eq!(
            active.as_deref(),
            expected_active,
            "case {name:?}: the refused create must not change the active set, got {active:?}"
        );
        if *activate {
            assert_eq!(
                active_rules, rules_before,
                "case {name:?}: the active set still reports its {rule_count} rule(s), \
                 got {active_rules}"
            );
        }

        // And the account still holds exactly the one set.
        let listed = listed_names(&router).await;
        assert_eq!(
            listed,
            vec![(*name).to_string()],
            "case {name:?}: the refused create must not add a set, got {listed:?}"
        );
    }
}
